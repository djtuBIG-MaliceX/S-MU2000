// license:BSD-3-Clause
//
// WASAPI の音声入力（録音側）を、自分のスレッドで回す。A/D INPUT に入れる音。
//
// デバイスは共有モードで開き、言ってきた形式（48000Hz の float など）を
// 44100Hz の 16bit 2ch に直して（ui::resampler）、輪の溜めに積む。
// 音源の糸は 1 サンプルずつ pop() で取り出す。
//
// **入力と出力は別の時計で動く。** 録音デバイスと再生デバイスの時計は少しずつずれるので、
// 溜めが増え過ぎたら古い分を捨て、空になったら 0 を返す（数えておく）。
// 溜める目標は 50ms。サンプリングの録音に使うだけなので、この程度の遅れは構わない。

#ifndef S_MU2000_UI_AUDIO_IN_H
#define S_MU2000_UI_AUDIO_IN_H

#pragma once

#include "compat/mamecompat.h"

#include <atomic>
#include <string>
#include <vector>

#if defined(__APPLE__)
#include <memory>
#else
#include <thread>
#endif

namespace ui {

#if defined(__APPLE__)

// macOS: the same contract as the WASAPI class below -- list the inputs, open
// one, and hand the machine 44100Hz 16bit 2ch frames one at a time -- but the
// device is a HAL input AudioUnit, which calls the input callback on its own
// real-time thread. So unlike the Windows side there is no worker thread here,
// and the ring the audio thread reads is filled straight from that callback.
// A pimpl for the same reason audio_out has one: nothing in here should have
// to know about CoreAudio.
class audio_in
{
public:
	audio_in();
	~audio_in();

	// 使える録音デバイスの名前
	static std::vector<std::string> list();

	// device is a name (exact match; empty is the system default input)
	bool start(const std::string &device, std::string &err);
	void stop();
	bool running() const;

	// One frame (44100Hz, 16bit scale). 0 while the ring is empty
	void pop(s32 &l, s32 &r);

	std::string device_name() const;
	std::string format_line() const;
	u64 empty_count() const;
	u64 dropped_count() const;

private:
	struct impl;
	std::unique_ptr<impl> m_impl;
};

#else

class audio_in
{
public:
	~audio_in() { stop(); }

	// 使える録音デバイスの名前
	static std::vector<std::string> list();

	// device は名前（完全一致、空なら Windows の既定の録音デバイス）
	bool start(const std::string &device, std::string &err);
	void stop();
	bool running() const { return m_running.load(); }

	// 1 フレーム取り出す（44100Hz、16bit の目盛り）。溜めが空なら 0
	void pop(s32 &l, s32 &r)
	{
		u32 rd = m_r.load(std::memory_order_relaxed);
		const u32 wr = m_w.load(std::memory_order_acquire);
		u32 level = (wr - rd) & MASK;
		if (level > DROP_FRAMES) {
			// 溜まり過ぎ。目標まで古い分を捨てる
			rd = (wr - TARGET_FRAMES) & MASK;
			level = TARGET_FRAMES;
			m_dropped.fetch_add(1, std::memory_order_relaxed);
		}
		if (!level) {
			l = r = 0;
			if (m_running.load(std::memory_order_relaxed))
				m_empty.fetch_add(1, std::memory_order_relaxed);
			return;
		}
		l = m_ring[rd * 2];
		r = m_ring[rd * 2 + 1];
		m_r.store((rd + 1) & MASK, std::memory_order_relaxed);
	}

	std::string device_name() const { return m_dev_name; }
	std::string format_line() const;
	u64 empty_count() const   { return m_empty.load(); }
	u64 dropped_count() const { return m_dropped.load(); }

private:
	static constexpr u32 RING = 1 << 16, MASK = RING - 1;      // 約 1.5 秒
	static constexpr u32 TARGET_FRAMES = 2205;                  // 50ms
	static constexpr u32 DROP_FRAMES = 8820;                    // 200ms を超えたら捨てる

	void run();
	void push(const s16 *frames, u32 n);

	std::thread       m_thread;
	std::atomic<bool> m_quit{false};
	std::atomic<bool> m_running{false};
	std::atomic<int>  m_start_state{0};   // 0 待ち / 1 動いた / 2 だめ
	std::string       m_err, m_want, m_dev_name;
	u32 m_dev_rate = 0, m_dev_channels = 0, m_dev_bits = 0;
	bool m_dev_float = false;

	std::vector<s16>  m_ring = std::vector<s16>(size_t(RING) * 2);
	std::atomic<u32>  m_w{0}, m_r{0};
	std::atomic<u64>  m_empty{0}, m_dropped{0};
};

#endif // __APPLE__

} // namespace ui

#endif // S_MU2000_UI_AUDIO_IN_H
