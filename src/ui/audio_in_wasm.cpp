// license:BSD-3-Clause
//
// A/D INPUT from the browser microphone (Emscripten). The page opens the
// mic (getUserMedia) and a ScriptProcessor hands over s16 blocks at the
// AudioContext's own rate; smu_audio_in_push (js_wasm.cpp) forwards them
// here, where ui::resampler turns 48000 into the 44100 the machine
// insists on -- the same converter the ALSA back-end uses.
//
// Same contract as the other two: list the inputs, open one by name, and
// hand the machine one 44100 frame at a time (0 when the ring is empty).

#include "audio_in.h"
#include "audio_out.h"   // ui::AUDIO_RATE
#include "resampler.h"
#include "wasm_glue.h"

#include <cstdio>
#include <deque>
#include <mutex>

namespace ui {

namespace {

std::mutex g_mic_lock;
std::string g_mic_name;                // what the page registered
double g_mic_rate = 48000.0;

constexpr u32 OUT_BATCH   = 256;       // frames per resampler pull
constexpr size_t QUEUE_MAX = 13230;    // 300ms; beyond that drop oldest

// The real state. The header's impl is a private nested type the free
// functions in this file may not even name, so they work on this struct
// and impl just carries one.
struct mic_state
{
	resampler rs;
	std::deque<s32> outq;              // 44100 frames, s16 scale
	u64 empty = 0, dropped = 0;
	bool running = false;
	std::string name;
};

mic_state *g_open = nullptr;           // the input start() opened

} // namespace

void wasm_set_audio_in_device(std::string name, double rate)
{
	std::lock_guard<std::mutex> hold(g_mic_lock);
	g_mic_name = std::move(name);
	if (rate > 0.0)
		g_mic_rate = rate;
}

struct audio_in::impl
{
	mic_state st;
};

audio_in::audio_in() : m_impl(new impl()) {}
audio_in::~audio_in() { stop(); }

std::vector<std::string> audio_in::list()
{
	std::lock_guard<std::mutex> hold(g_mic_lock);
	if (g_mic_name.empty())
		return {};
	return { g_mic_name };
}

bool audio_in::start(const std::string &device, std::string &err)
{
	stop();
	std::lock_guard<std::mutex> hold(g_mic_lock);
	if (g_mic_name.empty()) {
		err = "マイクが開かれていない";
		return false;
	}
	if (!device.empty() && device != g_mic_name) {
		err = "その名前の録音デバイスが無い: " + device;
		return false;
	}
	m_impl->st.rs.configure(g_mic_rate, double(AUDIO_RATE));
	m_impl->st.outq.clear();
	m_impl->st.running = true;
	m_impl->st.name = g_mic_name;
	g_open = &m_impl->st;
	return true;
}

void audio_in::stop()
{
	if (!m_impl)
		return;
	if (g_open == &m_impl->st)
		g_open = nullptr;
	m_impl->st.running = false;
	m_impl->st.outq.clear();
	m_impl->st.name.clear();
}

bool audio_in::running() const { return m_impl->st.running; }

std::string audio_in::device_name() const { return m_impl->st.name; }

std::string audio_in::format_line() const
{
	std::lock_guard<std::mutex> hold(g_mic_lock);
	char buf[160];
	std::snprintf(buf, sizeof(buf), "A/D IN: %s %dHz->%dHz (%s)",
	              m_impl->st.name.empty() ? "-" : m_impl->st.name.c_str(),
	              int(g_mic_rate + 0.5), int(AUDIO_RATE),
	              m_impl->st.rs.direct() ? "direct" : "sinc");
	return buf;
}

u64 audio_in::empty_count() const { return m_impl->st.empty; }
u64 audio_in::dropped_count() const { return m_impl->st.dropped; }

void audio_in::pop(s32 &l, s32 &r)
{
	if (m_impl->st.outq.size() >= 2) {
		l = m_impl->st.outq.front(); m_impl->st.outq.pop_front();
		r = m_impl->st.outq.front(); m_impl->st.outq.pop_front();
		return;
	}
	l = r = 0;
	if (m_impl->st.running)
		m_impl->st.empty++;
}

// ---- called from js_wasm.cpp's smu_audio_in_push -------------------------

void wasm_audio_in_push(const s16 *frames, u32 frames_count)
{
	if (!frames || !frames_count || !g_open || !g_open->running)
		return;
	mic_state &i = *g_open;
	i.rs.push(frames, int(frames_count));

	float tmp[OUT_BATCH * 2];
	while (i.rs.output_available() >= int(OUT_BATCH)) {
		i.rs.pull(tmp, int(OUT_BATCH));
		for (int k = 0; k < int(OUT_BATCH) * 2; k++) {
			const float f = tmp[k] * 32768.0f;
			s32 v = s32(f >= 0.0f ? f + 0.5f : f - 0.5f);
			if (v > 32767) v = 32767;
			if (v < -32768) v = -32768;
			i.outq.push_back(v);
		}
	}
	// Two clocks again: if the page pushed faster than fill() drains,
	// throw the old away like the other back-ends do
	while (i.outq.size() > QUEUE_MAX * 2) {
		i.outq.pop_front();
		i.outq.pop_front();
		i.dropped++;
	}
}

} // namespace ui
