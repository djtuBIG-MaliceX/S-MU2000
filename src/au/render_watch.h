// license:BSD-3-Clause
//
// What the host actually asked for, counted while it happens.
//
// The probes and auval render one way and a DAW renders another, and the
// difference is not visible from the outside: block size, stream format,
// whether MIDI offsets land inside the block, and whether a block took longer
// than it plays. This is the tally both AU wrappers keep (au/plugin.cpp and
// auv3/audio_unit.mm share it, the same way they share the engine). It is
// deliberately cheap enough to leave on: plain counters the audio thread
// bumps, and a line written only when something is off -- a late block, more
// frames than the host promised, or an event's offset outside the block -- so
// a healthy run stays silent. A summary is written when the instance goes
// away, which is the one line worth having after a session in a DAW.
//
// Nothing here allocates or locks. The audio thread formats into a buffer on
// its own stack and hands the line to the engine's logger, which is the same
// thing the engine already does when it fails to restore a card.

#ifndef S_MU2000_AU_RENDER_WATCH_H
#define S_MU2000_AU_RENDER_WATCH_H

#pragma once

#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstring>

namespace smu2000 {
namespace au {

class render_watch
{
public:
	// How many buffers of the first block are described
	static constexpr int kMaxBuffers = 4;
	// One running report at most this often while something is wrong
	static constexpr uint64_t kReportEveryMs = 3000;

	static uint64_t now_ms()
	{
		using clock = std::chrono::steady_clock;
		return uint64_t(std::chrono::duration_cast<std::chrono::milliseconds>(
		                    clock::now().time_since_epoch()).count());
	}

	// The rate the blocks are measured against. Set where the host's stream
	// format is taken, read when a block is timed
	void set_rate(double rate) { m_rate.store(rate, std::memory_order_relaxed); }

	// ---- the audio thread

	// One block: the frames asked for and the wall time it took to make them
	void note_block(uint32_t frames, uint64_t us)
	{
		m_renders.fetch_add(1, std::memory_order_relaxed);
		m_frames.fetch_add(frames, std::memory_order_relaxed);
		bump_min(m_min_frames, frames);
		bump_max(m_max_frames, frames);
		bump_max(m_worst_us, uint32_t(us));
		const double rate = m_rate.load(std::memory_order_relaxed);
		if (rate > 0.0 && us > uint64_t(double(frames) * 1e6 / rate))
			m_late.fetch_add(1, std::memory_order_relaxed);
	}

	// One MIDI message the host handed over, with the offset it gave. An
	// offset at or past the end of the block is a host that sends ahead of the
	// render rather than inside it
	void note_midi(uint32_t offset, uint32_t frames)
	{
		m_midi.fetch_add(1, std::memory_order_relaxed);
		bump_min(m_midi_min, offset);
		bump_max(m_midi_max, offset);
		if (offset >= frames)
			m_midi_outside.fetch_add(1, std::memory_order_relaxed);
	}

	// The host asked for more frames than MaximumFramesPerSlice promised
	void note_over_max(uint32_t frames, uint32_t limit)
	{
		if (m_over_max.fetch_add(1, std::memory_order_relaxed) == 0) {
			m_over_frames.store(frames, std::memory_order_relaxed);
			m_over_limit.store(limit, std::memory_order_relaxed);
		}
	}

	// The first block's shape: how many buffers and the channels in each
	void note_layout(uint32_t buffers, const uint32_t *chans, uint32_t n,
	                 uint32_t bytes0, const void *data0)
	{
		if (m_layout_done.exchange(true))
			return;
		if (n > kMaxBuffers)
			n = kMaxBuffers;
		m_buffers.store(buffers, std::memory_order_relaxed);
		m_nchans.store(n, std::memory_order_relaxed);
		for (int i = 0; i < kMaxBuffers; i++)
			m_chans[i].store(i < int(n) ? chans[i] : 0u, std::memory_order_relaxed);
		m_bytes0.store(bytes0, std::memory_order_relaxed);
		m_data0.store(data0 != nullptr, std::memory_order_relaxed);
	}

	// ---- lines. Formatted into the caller's buffer: no allocation here

	// The first block, once
	bool layout_line(char *dst, size_t n)
	{
		if (!m_layout_done.load(std::memory_order_relaxed) || m_layout_said)
			return false;
		m_layout_said = true;
		char chans[64] = {};
		const uint32_t nchans = m_nchans.load(std::memory_order_relaxed);
		for (uint32_t i = 0; i < nchans; i++)
			std::snprintf(chans + std::strlen(chans), sizeof(chans) - std::strlen(chans),
			              "%s%u", i ? "/" : "", m_chans[i].load(std::memory_order_relaxed));
		std::snprintf(dst, n,
		              "描き出し 1 回目: %u フレーム（上限 %u）、入れ物 %u 本 [ch %s]、"
		              "1 本目 %u バイト%s、%u 回目までに %llu フレーム",
		              unsigned(m_last_frames), unsigned(m_last_limit), unsigned(m_buffers.load()),
		              chans, unsigned(m_bytes0.load()),
		              m_data0.load() ? "" : "（mData が null）",
		              unsigned(m_renders.load()), (unsigned long long)m_frames.load());
		return true;
	}

	// While something is wrong, at most once every kReportEveryMs
	bool report_line(char *dst, size_t n)
	{
		if (!saw_problem())
			return false;
		const uint64_t now = now_ms();
		uint64_t next = m_next_report.load(std::memory_order_relaxed);
		if (now < next)
			return false;
		m_next_report.store(now + kReportEveryMs, std::memory_order_relaxed);
		std::snprintf(dst, n,
		              "描き出し %llu 回 / %u〜%u フレーム、最悪 %u us、間に合わなかった %llu 回、"
		              "上限超え %llu 回、MIDI %llu 通（ブロック外 %llu 通、オフセット %u〜%u）",
		              (unsigned long long)m_renders.load(), unsigned(m_min_frames.load()),
		              unsigned(m_max_frames.load()), unsigned(m_worst_us.load()),
		              (unsigned long long)m_late.load(), (unsigned long long)m_over_max.load(),
		              (unsigned long long)m_midi.load(), (unsigned long long)m_midi_outside.load(),
		              unsigned(m_midi_min.load()), unsigned(m_midi_max.load()));
		return true;
	}

	// Everything, when the instance is closed
	void summary_line(char *dst, size_t n) const
	{
		const double rate = m_rate.load(std::memory_order_relaxed);
		const uint64_t frames = m_frames.load(std::memory_order_relaxed);
		std::snprintf(dst, n,
		              "描き出しのまとめ: %llu 回 / %.1f 秒ぶん（%g Hz）、%u〜%u フレーム、"
		              "最悪 %u us、間に合わなかった %llu 回、上限超え %llu 回、"
		              "MIDI %llu 通（ブロック外 %llu 通、オフセット %u〜%u）",
		              (unsigned long long)m_renders.load(),
		              rate > 0.0 ? double(frames) / rate : 0.0, rate,
		              unsigned(m_min_frames.load()), unsigned(m_max_frames.load()),
		              unsigned(m_worst_us.load()), (unsigned long long)m_late.load(),
		              (unsigned long long)m_over_max.load(), (unsigned long long)m_midi.load(),
		              (unsigned long long)m_midi_outside.load(),
		              unsigned(m_midi_min.load()), unsigned(m_midi_max.load()));
	}

	bool saw_problem() const
	{
		return m_late.load(std::memory_order_relaxed) ||
		       m_over_max.load(std::memory_order_relaxed) ||
		       m_midi_outside.load(std::memory_order_relaxed);
	}

	// Called once per block so the layout line can name the block it describes
	void set_last_block(uint32_t frames, uint32_t limit)
	{
		m_last_frames = frames;
		m_last_limit = limit;
	}

private:
	static void bump_min(std::atomic<uint32_t> &v, uint32_t x)
	{
		uint32_t cur = v.load(std::memory_order_relaxed);
		while (x < cur && !v.compare_exchange_weak(cur, x, std::memory_order_relaxed)) {}
	}

	static void bump_max(std::atomic<uint32_t> &v, uint32_t x)
	{
		uint32_t cur = v.load(std::memory_order_relaxed);
		while (x > cur && !v.compare_exchange_weak(cur, x, std::memory_order_relaxed)) {}
	}

	std::atomic<double>   m_rate{0.0};
	std::atomic<uint64_t> m_renders{0}, m_frames{0}, m_late{0}, m_over_max{0};
	std::atomic<uint64_t> m_midi{0}, m_midi_outside{0};
	std::atomic<uint32_t> m_min_frames{0xffffffffu}, m_max_frames{0};
	std::atomic<uint32_t> m_worst_us{0};
	std::atomic<uint32_t> m_midi_min{0xffffffffu}, m_midi_max{0};
	std::atomic<uint32_t> m_buffers{0}, m_nchans{0}, m_bytes0{0};
	std::atomic<uint32_t> m_chans[kMaxBuffers] = {};
	std::atomic<bool>     m_data0{false};
	std::atomic<bool>     m_layout_done{false};
	std::atomic<uint32_t> m_over_frames{0}, m_over_limit{0};
	std::atomic<uint64_t> m_next_report{0};
	uint32_t m_last_frames = 0, m_last_limit = 0;   // audio thread only
	bool     m_layout_said = false;                 // audio thread only
};

} // namespace au
} // namespace smu2000

#endif // S_MU2000_AU_RENDER_WATCH_H
