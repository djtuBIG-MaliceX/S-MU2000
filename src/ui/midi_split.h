// license:BSD-3-Clause
//
// MU2000 の MIDI OUT はバイトの列（実機と同じ 31250bps の直列）で出てくるが、
// 渡す先は 1 メッセージずつを欲しがる——AUv3 の MIDIOutputEventBlock も、
// CoreMIDI の MIDISend も。その間を埋める。
//
// **確保も錠もしない。** 音声スレッドの中で回る。
// 入りきらないメッセージ（8KB を超える SysEx）は、そのメッセージだけ捨てる。

#ifndef S_MU2000_UI_MIDI_SPLIT_H
#define S_MU2000_UI_MIDI_SPLIT_H

#pragma once

#include <cstddef>
#include <cstdint>

namespace ui {

class midi_split
{
public:
	// 溜まりきったメッセージを渡す先
	using emit_fn = void (*)(void *ctx, const uint8_t *bytes, size_t n);

	void reset() { m_n = 0; m_want = 0; m_status = 0; m_sysex = false; m_over = false; }

	// バイト列を食わせる。メッセージが 1 つ揃うたびに emit を呼ぶ
	void feed(const uint8_t *p, size_t n, emit_fn emit, void *ctx)
	{
		for (size_t i = 0; i < n; i++)
			one(p[i], emit, ctx);
	}

private:
	// そのステータスに続くデータバイトの数
	static int data_bytes(uint8_t status)
	{
		switch (status & 0xf0) {
		case 0x80: case 0x90: case 0xa0: case 0xb0: case 0xe0: return 2;
		case 0xc0: case 0xd0: return 1;
		default: break;
		}
		switch (status) {
		case 0xf1: case 0xf3: return 1;   // MTC クォーターフレーム、ソングセレクト
		case 0xf2: return 2;              // ソングポジション
		default: return 0;                // F6 / F7 と、F8 以上
		}
	}

	void flush(emit_fn emit, void *ctx)
	{
		if (m_n && !m_over)
			emit(ctx, m_buf, m_n);
		m_n = 0;
		m_over = false;
	}

	void put(uint8_t v)
	{
		if (m_n < SIZE)
			m_buf[m_n++] = v;
		else
			m_over = true;            // 長すぎる。このメッセージは捨てる
	}

	void one(uint8_t b, emit_fn emit, void *ctx)
	{
		// リアルタイム（F8-FF）はどこに挟まってもよい。単独で出す。
		// **溜めかけを壊さない**（SysEx の途中に来るのが決まりどおり）
		if (b >= 0xf8) {
			const uint8_t rt = b;
			emit(ctx, &rt, 1);
			return;
		}

		if (b >= 0x80) {
			// 新しいステータス。溜めかけがあれば、そこで切れている
			if (m_sysex) {
				if (b == 0xf7)
					put(b);           // 正しく終わった
				flush(emit, ctx);
				m_sysex = false;
				if (b == 0xf7)
					return;
			} else if (m_n) {
				flush(emit, ctx);     // 足りないまま次が来た。捨てずに出す
			}

			if (b == 0xf0) {
				m_sysex = true;
				m_status = 0;
				put(b);
				return;
			}
			m_status = (b < 0xf0) ? b : 0;   // F1-F7 はランニングステータスを持たない
			m_want = data_bytes(b);
			put(b);
			if (!m_want)
				flush(emit, ctx);            // F6 など、データの無いもの
			return;
		}

		// データバイト
		if (m_sysex) {
			put(b);
			return;
		}
		if (!m_n) {
			// ランニングステータス。ステータスは前のまま
			if (!m_status)
				return;                      // 宙ぶらりん。捨てる
			put(m_status);
			m_want = data_bytes(m_status);
		}
		put(b);
		if (m_n >= size_t(m_want) + 1)
			flush(emit, ctx);
	}

	// バルクダンプが通るくらい。MU2000 の TX の溜めは 4096 バイト
	static constexpr size_t SIZE = 8192;
	uint8_t m_buf[SIZE] = {};
	size_t  m_n = 0;
	int     m_want = 0;
	uint8_t m_status = 0;
	bool    m_sysex = false;
	bool    m_over = false;
};

} // namespace ui

#endif // S_MU2000_UI_MIDI_SPLIT_H
