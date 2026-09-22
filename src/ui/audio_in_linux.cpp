// license:BSD-3-Clause
//
// ALSA capture input for Linux. Same interface as audio_in.cpp (WASAPI) and
// audio_in_mac.cpp (CoreAudio): the A/D INPUT feed.
//
// The device is opened for capture and a worker thread converts whatever rate
// it runs at down to 44100Hz with the same ui::resampler the output side uses.
// Input and output run on different clocks, so like the Windows side the ring
// drops the oldest frames when it grows past 200ms and reports 0 when empty.

#include "audio_in.h"
#include "audio_out.h"   // AUDIO_RATE, the rate both directions convert to/from
#include "resampler.h"

#include <alsa/asoundlib.h>

#include <atomic>
#include <cstdio>
#include <cstring>
#include <thread>
#include <vector>

namespace ui {

struct audio_in::impl {
	snd_pcm_t *pcm = nullptr;
	std::string dev_name;
	std::string err, want;
	u32 dev_rate = 0, dev_channels = 0;

	std::thread       thread;
	std::atomic<bool> quit{false};
	std::atomic<bool> running{false};
	std::atomic<int>  start_state{0};   // 0 waiting / 1 running / 2 failed

	static constexpr u32 RING = 1 << 16, MASK = RING - 1;   // ~1.5s
	static constexpr u32 TARGET_FRAMES = 2205;               // 50ms
	static constexpr u32 DROP_FRAMES = 8820;                 // drop past 200ms

	std::vector<s16> ring = std::vector<s16>(size_t(RING) * 2);
	std::atomic<u32> w{0}, r{0};
	std::atomic<u64> empty{0}, dropped{0};

	resampler rs;

	void push(const s16 *frames, u32 n)
	{
		for (u32 i = 0; i < n; i++) {
			const u32 at = w.load(std::memory_order_relaxed);
			ring[size_t(at) * 2 + 0] = frames[i * 2 + 0];
			ring[size_t(at) * 2 + 1] = frames[i * 2 + 1];
			w.store((at + 1) & MASK, std::memory_order_release);
		}
	}

	void run()
	{
		snd_pcm_t *cap = pcm;
		// Device-rate scratch converted down to 44100Hz.
		std::vector<s16>   dev(2048 * 2);
		std::vector<float> mixed(2048 * 2);
		std::vector<s16>   out(2048 * 2);
		while (!quit.load(std::memory_order_relaxed)) {
			snd_pcm_sframes_t got = snd_pcm_readi(cap, dev.data(), 1024);
			if (got == -EPIPE || got == -ESTRPIPE) {
				snd_pcm_recover(cap, int(got), 0);
				snd_pcm_prepare(cap);
				rs.reset();
				continue;
			}
			if (got <= 0) {
				if (snd_pcm_recover(cap, int(got), 0) != 0)
					break;
				continue;
			}
			rs.push(dev.data(), int(got));
			int avail = rs.output_available();
			while (avail > 0) {
				const int n = std::min(avail, 2048);
				rs.pull(mixed.data(), n);
				for (int i = 0; i < n * 2; i++) {
					const float v = std::clamp(mixed[size_t(i)], -1.0f, 1.0f);
					out[size_t(i)] = s16(v * 32767.0f);
				}
				push(out.data(), u32(n));
				avail = rs.output_available();
			}
		}
	}
};

audio_in::audio_in() : m_impl(new impl()) {}

audio_in::~audio_in()
{
	stop();
}

std::vector<std::string> audio_in::list()
{
	std::vector<std::string> names;
	void **hints = nullptr;
	if (snd_device_name_hint(-1, "pcm", &hints) == 0) {
		for (void **h = hints; *h; h++) {
			char *name = snd_device_name_get_hint(*h, "NAME");
			char *ioid = snd_device_name_get_hint(*h, "IOID");
			// Skip output-only devices.
			const bool output_only = ioid && !std::strcmp(ioid, "Output");
			if (name && !output_only)
				names.push_back(name);
			free(name);
			free(ioid);
		}
		snd_device_name_free_hint(hints);
	}
	if (names.empty())
		names.push_back("default");
	return names;
}

bool audio_in::start(const std::string &device, std::string &err)
{
	stop();
	auto up = std::make_unique<impl>();

	std::string pcm_name = "default";
	if (!device.empty()) {
		bool found = false;
		for (const std::string &n : list()) {
			if (n == device) {
				pcm_name = n;
				found    = true;
				break;
			}
		}
		if (!found)
			pcm_name = device;   // a raw ALSA PCM name
	}

	snd_pcm_t *pcm = nullptr;
	if (snd_pcm_open(&pcm, pcm_name.c_str(), SND_PCM_STREAM_CAPTURE, 0) < 0) {
		err = "その名前の録音デバイスが開けない: " + (device.empty() ? pcm_name : device);
		return false;
	}
	unsigned rate = AUDIO_RATE;
	if (snd_pcm_set_params(pcm, SND_PCM_FORMAT_S16_LE, SND_PCM_ACCESS_RW_INTERLEAVED,
	                       2, AUDIO_RATE, 0, 50000) < 0) {
		snd_pcm_close(pcm);
		err = "録音の形式を指定できない";
		return false;
	}
	{
		snd_pcm_hw_params_t *hw = nullptr;
		snd_pcm_hw_params_alloca(&hw);
		if (snd_pcm_hw_params_current(pcm, hw) == 0)
			snd_pcm_hw_params_get_rate(hw, &rate, nullptr);
	}
	if (snd_pcm_prepare(pcm) < 0) {
		snd_pcm_close(pcm);
		err = "録音を開始できない";
		return false;
	}

	up->pcm          = pcm;
	up->dev_name     = pcm_name;
	up->dev_rate     = rate ? rate : AUDIO_RATE;
	up->dev_channels = 2;
	up->rs.configure(double(up->dev_rate), double(AUDIO_RATE));
	up->running.store(true);

	impl *raw = up.get();
	m_impl = std::move(up);
	raw->thread = std::thread([raw] { raw->run(); });
	return true;
}

void audio_in::stop()
{
	if (!m_impl)
		return;
	m_impl->quit.store(true);
	if (m_impl->thread.joinable())
		m_impl->thread.join();
	m_impl->running.store(false);
	if (m_impl->pcm) {
		snd_pcm_drop(m_impl->pcm);
		snd_pcm_close(m_impl->pcm);
		m_impl->pcm = nullptr;
	}
	// Fresh ring for the next start, like a reopened device.
	m_impl->w.store(0);
	m_impl->r.store(0);
}

bool audio_in::running() const
{
	return m_impl && m_impl->running.load();
}

void audio_in::pop(s32 &l, s32 &r)
{
	u32 rd = m_impl->r.load(std::memory_order_relaxed);
	const u32 wr = m_impl->w.load(std::memory_order_acquire);
	u32 level = (wr - rd) & impl::MASK;
	if (level > impl::DROP_FRAMES) {
		rd    = (wr - impl::TARGET_FRAMES) & impl::MASK;
		level = impl::TARGET_FRAMES;
		m_impl->dropped.fetch_add(1, std::memory_order_relaxed);
	}
	if (!level) {
		l = r = 0;
		if (m_impl->running.load(std::memory_order_relaxed))
			m_impl->empty.fetch_add(1, std::memory_order_relaxed);
		return;
	}
	// 16bit scale, like the other platforms (no scaling here).
	l = m_impl->ring[size_t(rd) * 2 + 0];
	r = m_impl->ring[size_t(rd) * 2 + 1];
	m_impl->r.store((rd + 1) & impl::MASK, std::memory_order_relaxed);
}

std::string audio_in::device_name() const
{
	return m_impl ? m_impl->dev_name : std::string();
}

std::string audio_in::format_line() const
{
	char buf[160];
	std::snprintf(buf, sizeof(buf), "ALSA / %u Hz 2ch 16bit → 44100 Hz",
	              m_impl ? m_impl->dev_rate : 0);
	return buf;
}

u64 audio_in::empty_count() const   { return m_impl ? m_impl->empty.load() : 0; }
u64 audio_in::dropped_count() const { return m_impl ? m_impl->dropped.load() : 0; }

} // namespace ui
