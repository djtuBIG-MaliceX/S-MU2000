// license:BSD-3-Clause
//
// Windows の MIDI 出力へ、音源が受け取ったのと同じものを流す（実機の THRU）。
//
// 音声スレッドから Windows の API を直に叩くと、そこで待たされることがある。
// **音声スレッドは待たせてはいけない**（doc/design.md）ので、
// 音声スレッドは輪っかにバイトを積むだけにして、別のスレッドが送る。
// 積むのは音声スレッド、取るのは送りスレッドの一本ずつなので錠は要らない。
//
// macOS uses CoreMIDI instead of WinMM. The class surface is identical and only
// the wake-up differs: a Win32 event against a condition variable.

#ifndef S_MU2000_UI_MIDI_OUT_H
#define S_MU2000_UI_MIDI_OUT_H

#pragma once

#include "compat/mamecompat.h"

#include <atomic>
#include <string>
#include <thread>
#include <vector>

#if !defined(_WIN32)
#include <condition_variable>
#include <mutex>
#endif

namespace ui {

#if !defined(_WIN32)

// macOS: CoreMIDI. Linux: the ALSA sequencer (midi_out_linux.cpp). The shape
// matches the Windows class, and so does the lock-free ring, because the audio
// thread must still be able to hand a byte over without ever waiting. What
// differs is the wake-up: neither CoreMIDI nor ALSA has a Win32 event object
// to signal, so a condition variable stands in for it.
class midi_out
{
public:
	~midi_out();

	static std::vector<std::string> list();

	bool open(int device, std::string &err);
	void close();

	bool        is_open() const { return m_open.load(std::memory_order_acquire); }
	std::string device_name() const { return m_name; }

	// From the audio thread: push one byte at a time, drop it when not open
	void send(u8 v);

private:
	void run();                       // the sender thread
	void emit(const u8 *p, size_t n); // send one assembled message to CoreMIDI

	static constexpr size_t SIZE = 8192, MASK = SIZE - 1;
	u8 m_buf[SIZE] = {};
	std::atomic<size_t> m_read{0}, m_write{0};

	// The wake-up signal. **Closing does not destroy it**: the window thread
	// closes while the audio thread pushes, and losing it in between would
	// drop a wake-up
	std::condition_variable m_wake;
	std::mutex              m_wake_mutex;

	struct ctx;                       // CoreMIDI client / port / destination,
	                                // or the ALSA sequencer handle, in the .cpp
	ctx        *m_ctx = nullptr;
	std::atomic<bool> m_open{false};
	std::string m_name;
	std::thread m_thread;
	std::atomic<bool> m_quit{false};

	// Assemble one message from the byte stream. Only the sender thread touches this
	u8     m_msg[3] = {};
	int    m_have = 0, m_want = 0;
	u8     m_status = 0;
	bool   m_in_sysex = false;
	std::vector<u8> m_sysex;
};

#else // _WIN32

class midi_out
{
public:
	~midi_out();

	static std::vector<std::string> list();

	// 番号が負なら開かない（どこへも出さない）
	bool open(int device, std::string &err);
	void close();

	bool        is_open() const { return m_open.load(); }
	std::string device_name() const { return m_name; }

	// 音声スレッドから。1 バイトずつ積む。開いていなければ捨てる
	void send(u8 v);

	// 相手が受け取らなくなった（送り終わらないまま時間切れになった）
	bool stuck() const { return m_stuck.load(std::memory_order_acquire); }

private:
	void run(unsigned gen, void *handle);  // 送りスレッド
	void emit(void *handle, const u8 *p, size_t n); // 組み上がった 1 通を Windows へ

	static constexpr size_t SIZE = 8192, MASK = SIZE - 1;
	u8 m_buf[SIZE] = {};
	std::atomic<size_t> m_read{0}, m_write{0};

	void       *m_handle = nullptr;   // HMIDIOUT
	// 起こす合図。**閉じても捨てない**。閉じるのは窓のスレッド、
	// 積むのは音声スレッドなので、途中で無くなると掴み損ねる
	void       *m_wake   = nullptr;   // HANDLE（イベント）
	std::atomic<bool> m_open{false};
	std::string m_name;
	std::thread m_thread;
	std::atomic<bool> m_quit{false};
	// 送りスレッドが抜けたか。**相手が固まると Windows の呼び出しから戻らず、
	// 抜けられない**ので、閉じるときはこれを決めた時間だけ待って、だめなら置いていく
	std::atomic<bool> m_thread_done{true};
	std::atomic<bool> m_stuck{false};
	// 開き直すたびに増やす。置いていかれた古い送りスレッドが、あとで戻ってきても
	// 新しい口の輪に触らないように
	std::atomic<unsigned> m_gen{0};

	// バイトの並びから 1 通を組み立てる。送りスレッドだけが触る
	u8     m_msg[3] = {};
	int    m_have = 0, m_want = 0;
	u8     m_status = 0;
	bool   m_in_sysex = false;
	std::vector<u8> m_sysex;
};

#endif // _WIN32

} // namespace ui

#endif // S_MU2000_UI_MIDI_OUT_H
