// license:BSD-3-Clause
//
// パネルの操作を音源に反映し、画面へ写しを返す。音声スレッドから呼ぶ。
// exe（gui.exe）と VST3 の両方が同じものを使うので、押し方も見え方も揃う。

#ifndef S_MU2000_UI_DRIVER_H
#define S_MU2000_UI_DRIVER_H

#pragma once

#include "bridge.h"
#include "mu2000.h"
#include "xg/ram.h"

#include <cstdio>
#include <initializer_list>
#include <memory>
#include <string>
#include <vector>

namespace ui {

class driver
{
public:
	// 1 ブロックの頭で。画面から押されているボタンを音源へ
	void apply_buttons(mu2000 &mu, const bridge &br)
	{
		const u64 want = br.buttons();
		if (want == m_applied)
			return;
		for (int i = 0; i < int(mu2000::button::count); i++)
			if (((want ^ m_applied) >> i) & 1)
				mu.set_button(mu2000::button(i), ((want >> i) & 1) != 0);
		m_applied = want;
	}

	// エディタから送られた MIDI を音源へ。echo には MIDI 出力の口を渡す
	// （実機の THRU と同じで、画面から出したものも外へ出る）
	template <typename F>
	void pump_midi(mu2000 &mu, bridge &br, F &&echo)
	{
		serve_defaults(mu, br);
		u8 b;
		while (br.take_midi(b)) {
			watch(b, mu.midi_in(b));
			echo(b);
		}
		// パラメータの層の問い合わせ。外へは流さない
		while (br.take_ask(b))
			mu.midi_in(b);
		// 画面から口 B・C・D へ（一覧の鍵盤）。外へは流さない
		for (int port = 1; port < mu2000::MIDI_PORTS; port++)
			while (br.take_midi_port(port, b))
				watch(b, mu.midi_in(b, port));
	}

	void pump_midi(mu2000 &mu, bridge &br)
	{
		pump_midi(mu, br, [](u8) {});
	}

	// 画面に頼まれたら、XG の既定値を 1 度だけ作って置く（bridge の request_defaults）。
	// 機械の姿を丸ごと控え、XG System On を流して firmware に既定値を書かせ、写してから
	// 控えを戻す。戻すので、鳴っている音も設定も元のまま。この区間は少し長くかかる
	// （firmware を 0.3 秒ほど回す）ので、音が一瞬途切れることがある
	void serve_defaults(mu2000 &mu, bridge &br)
	{
		if (!br.take_defaults_request())
			return;
		if (br.have_defaults())
			return;
		const std::vector<u8> saved = mu.save_state();
		const int native = mu.native_engine();
		if (native)
			mu.set_native_engine(0);             // XG System On は firmware に読ませる
		for (u8 b : { 0xf0, 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7e, 0x00, 0xf7 })
			mu.midi_in(b);
		s32 l, r;
		u8 v;
		const int rate = 44100;
		for (int i = 0; i < 3 * rate && mu.midi_pending(); i++) {   // 前に溜まっていた分ごと読ませる
			mu.run_sample(l, r);
			while (mu.midi_out_take(v)) {}
		}
		for (int i = 0; i < rate * 3 / 10; i++) {
			mu.run_sample(l, r);
			while (mu.midi_out_take(v)) {}
		}
		const std::unique_ptr<xg_snapshot> s = std::make_unique<xg_snapshot>();   // 大きいので糸の積み場に置かない
		copy_xg(mu, *s);
		std::string err;
		mu.load_state(saved.data(), saved.size(), err);
		if (native)
			mu.set_native_engine(native);
		br.publish_defaults(*s);
	}

	// ブロックの終わりで。音源が MIDI OUT から送り出したものを画面へ渡す。
	// echo には外の MIDI OUT の口を渡す
	template <typename F>
	void pump_out(mu2000 &mu, bridge &br, F &&echo)
	{
		u8 b;
		while (mu.midi_out_take(b)) {
			br.put_out(b);
			echo(b);
		}
	}

	void pump_out(mu2000 &mu, bridge &br)
	{
		pump_out(mu, br, [](u8) {});
	}

	// ホイールで回された分をダイヤルへ。実機と同じロータリーエンコーダなので、
	// 目盛りを渡すだけでよい（位相は音源が自分で進める）
	void pump_wheel(mu2000 &mu, bridge &br)
	{
		for (int step = br.take_turn(); step; step = br.take_turn())
			mu.turn_encoder(step);
	}

	// 音源へ入れた MIDI を 1 バイトずつ見せる。押さえている鍵とベロシティを写しに書く
	// （音源の中の鍵の状態はきれいに取り出せないので、入口で数える）
	void watch(u8 b, int port)
	{
		if (port < 0 || port >= mu2000::MIDI_PORTS)
			return;
		if (b >= 0xf8)
			return;                           // リアルタイム
		if (b == 0xf0) {
			m_sysex[port] = true;
			m_sx_len[port] = 0;
			m_status[port] = 0;                   // SysEx はランニングステータスを打ち切る
			return;
		}
		if (m_sysex[port]) {
			if (!(b & 0x80)) {
				if (m_sx_len[port] < sizeof(m_sx[port]))
					m_sx[port][m_sx_len[port]] = b;
				m_sx_len[port]++;
				return;
			}
			m_sysex[port] = false;                // F7 か、途中で別のものが来た
			if (b == 0xf7) {
				if (is_reset(m_sx[port], m_sx_len[port]))
					for (int ch = 0; ch < 16; ch++)
						m_xg.notes[port * 16 + ch][0] = m_xg.notes[port * 16 + ch][1] = 0;
				return;
			}
		}
		if (b & 0x80) {
			m_status[port] = b < 0xf0 ? b : 0;    // F1-F7 は無視して、ランニングステータスも捨てる
			m_have[port] = 0;
			return;
		}
		const u8 st = m_status[port];
		if (!st)
			return;
		const u8 kind = st & 0xf0;
		m_data[port][m_have[port]++] = b;
		const int need = (kind == 0xc0 || kind == 0xd0) ? 1 : 2;
		if (m_have[port] < need)
			return;
		m_have[port] = 0;
		const int slot = port * 16 + (st & 0x0f);
		const u8 d0 = m_data[port][0], d1 = m_data[port][1];
		u64 &bits = m_xg.notes[slot][d0 >> 6];
		const u64 bit = u64(1) << (d0 & 63);
		if (kind == 0x90 && d1) {
			bits |= bit;
			m_xg.velocity[slot] = d1;
			m_xg.note_ons[slot]++;
		} else if (kind == 0x80 || kind == 0x90) {
			bits &= ~bit;
		} else if (kind == 0xb0 && (d0 == 120 || d0 >= 123)) {
			// オールサウンドオフ・オールノートオフ、オムニ／モノ／ポリの切り替え（どれも全部離す）
			m_xg.notes[slot][0] = m_xg.notes[slot][1] = 0;
		}
	}

	// 音源を初期状態に戻す SysEx か（鳴っている音が全部止まる）。F0 と F7 を除いた中身
	static bool is_reset(const u8 *p, size_t n)
	{
		auto is = [&](std::initializer_list<int> want, int any_low_nibble_at = -1) {
			if (n != want.size())
				return false;
			int i = 0;
			for (int w : want) {
				const u8 v = i == any_low_nibble_at ? u8(p[i] & 0xf0) : p[i];
				if (v != w)
					return false;
				i++;
			}
			return true;
		};
		return is({ 0x7e, 0x7f, 0x09, 0x01 }) || is({ 0x7e, 0x7f, 0x09, 0x03 }) ||          // GM / GM2 On
		       is({ 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7e, 0x00 }, 1) ||                     // XG System On
		       is({ 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7f, 0x00 }, 1) ||                     // XG All Parameter Reset
		       is({ 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7f, 0x00, 0x41 });              // GS Reset
	}

	// ブロックの終わりで。25ms ごとに LCD と LED を画面へ渡す
	void publish(mu2000 &mu, bridge &br, u32 frames, u32 rate,
	             bool ready, const char *message)
	{
		br.advance_clock(frames, rate);
		m_since += frames;
		if (m_since < rate / 40)
			return;
		m_since = 0;
		publish_now(mu, br, ready, message);
		if (ready)
			publish_xg(mu, br);
	}

	// firmware のワーク RAM から XG の値を写す（xg/ram.h）
	void publish_xg(mu2000 &mu, bridge &br)
	{
		copy_xg(mu, m_xg);
		m_xg.serial++;
		br.publish_xg(m_xg);
	}

	// XG の値だけを写す（鍵の見張りの欄と serial には触らない）。機械を持っている糸から呼ぶこと
	static void copy_xg(mu2000 &mu, xg_snapshot &out)
	{
		const std::vector<u8> &ram = mu.nvram();
		std::memcpy(out.system, ram.data() + xg::ram::SYSTEM, XG_SYSTEM_SIZE);
		out.voice_mode = ram[xg::ram::VOICE_MODE];
		out.voice_set  = ram[xg::ram::VOICE_SET];
		std::memcpy(out.effect, ram.data() + xg::ram::EFFECT, XG_EFFECT_SIZE);
		for (int p = 0; p < XG_PARTS; p++)
			std::memcpy(out.parts[p], ram.data() + xg::ram::part_base(p), XG_PART_COPY);
	}

	static void publish_now(mu2000 &mu, bridge &br, bool ready, const char *message)
	{
		snapshot s;
		hd44780_device &lcd = mu.lcd();
		const u8 *img = lcd.render();
		const int cols = lcd.line_size();
		for (int row = 0; row < LCD_ROWS; row++)
			for (int col = 0; col < LCD_COLS; col++)
				for (int y = 0; y < CELL_H; y++)
					s.dots[(row * LCD_COLS + col) * CELL_H + y] =
						img[16 * (row * cols + col) + y];
		s.leds   = mu.leds();
		s.lcd_on = lcd.display_on();
		s.voices_master = u8(mu.swpm().sounding_voices());
		s.voices_slave  = u8(mu.swps().sounding_voices());
		s.ready  = ready;
		if (!ready && message)
			std::snprintf(s.message, sizeof(s.message), "%s", message);
		br.publish(s);
	}

	// 音源が無いとき（起動前、ROM が無い）の写し
	static void publish_message(bridge &br, const char *message)
	{
		snapshot s;
		std::snprintf(s.message, sizeof(s.message), "%s", message ? message : "");
		br.publish(s);
	}

private:
	u64 m_applied = 0;
	u64 m_since = 0;
	xg_snapshot m_xg;                        // 音声の糸だけが触る
	u8   m_status[mu2000::MIDI_PORTS] = {}, m_data[mu2000::MIDI_PORTS][2] = {};
	int  m_have[mu2000::MIDI_PORTS] = {};
	bool m_sysex[mu2000::MIDI_PORTS] = {};
	u8   m_sx[mu2000::MIDI_PORTS][16] = {};                    // SysEx の頭（リセットかを見るだけ）
	size_t m_sx_len[mu2000::MIDI_PORTS] = {};
};

} // namespace ui

#endif // S_MU2000_UI_DRIVER_H
