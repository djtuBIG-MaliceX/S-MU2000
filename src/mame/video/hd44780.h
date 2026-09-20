// license:BSD-3-Clause
// copyright-holders:Sandro Ronco
//
// Hitachi HD44780 LCD コントローラ。
// MAME の src/devices/video/hd44780.* から、firmware が触る部分だけを取った。
//
// **ビジーフラグは音を出すのにも要る**。MU2000 の firmware は LCD にコマンドを
// 送るたびにビジーが立つのを見ており、常に「空いている」と返すと初期化の途中で
// 先へ進まなくなる。
//
// 表示の組み立て（render）は MAME と同じ形にしてある。文字の絵は CGROM から
// 引き、0x00-0x0f だけは CGRAM から引く。SVG のレイアウトは持たない。
//
// MAME はビジーの計測に emu_timer を使っていたが、こちらは CPU のサイクル数で
// 数える。LCD の発振は 270kHz、命令は 10 サイクル（37us）、クリアと
// ホームだけ 410 サイクル（1.52ms）。

#ifndef S_MU2000_HD44780_H
#define S_MU2000_HD44780_H

#pragma once

#include "state.h"
#include "../../compat/mamecompat.h"

class hd44780_device
{
public:
	// 状態の保存と復元（src/state.h）
	void state(state_io &s);

	// cpu_hz: ビジーの残り時間を数えるための CPU 側の周波数
	hd44780_device(u32 cpu_hz = 28000000, u32 lcd_hz = 270000)
		: m_cpu_hz(cpu_hz), m_lcd_hz(lcd_hz) {}

	void reset();

	// 現在の CPU サイクル。読み書きの前に入れておく
	void set_now(u64 cycles) { m_now = cycles; }

	void control_w(u8 data);
	u8   control_r() const;
	void data_w(u8 data);
	u8   data_r();

	bool busy() const { return m_now < m_busy_until; }

	// 表示内容。生の DDRAM
	const u8 *ddram() const { return m_ddram; }
	// 利用者が作った字（0x00-0x07）の絵。1 文字 8 バイト
	const u8 *cgram() const { return m_cgram; }

	// **外から 1 マス書き替える**。native の口で液晶のメーターを自前で
	// 描くのに使う（doc/native-engine.md の 6.148）。firmware の手順を
	// 通らないので、表示の状態（カーソルなど）は何も変えない
	void poke_ddram(u32 i, u8 v) { if (i < 0x80) m_ddram[i] = v; }

	// 文字の絵。HD44780U B04 の CGROM 4KB（1 文字 16 バイト、下位 5bit が絵）
	void set_cgrom(const u8 *rom, size_t size)
	{ m_cgrom = (rom && size >= 0x1000) ? rom : nullptr; }

	// 画面を組み立てる。MAME と同じ並びで、80 マス × 16 バイトを返す。
	// マス (行, 桁) は render()[16 * (行 * 桁数 + 桁)]、各バイトの下位 5bit が
	// 1 行ぶんの点。左端が bit4
	static constexpr int RENDER_SIZE = 80 * 16;
	const u8 *render();

	int  lines() const     { return m_num_line; }
	int  line_size() const { return 80 / m_num_line; }
	int  char_size() const { return m_char_size; }
	bool display_on() const { return m_display_on; }

private:
	void set_busy(u16 lcd_cycles)
	{
		m_busy_until = m_now + u64(lcd_cycles) * m_cpu_hz / m_lcd_hz;
	}
	void correct_ac();
	void update_ac(int direction);
	void shift_display(int direction);

	enum { DDRAM, CGRAM };

	u32 m_cpu_hz = 0, m_lcd_hz = 0;
	u64 m_now = 0, m_busy_until = 0;

	const u8 *m_cgrom = nullptr;
	u8  m_render_buf[RENDER_SIZE] = {};
	u8  m_ddram[0x80] = {};
	u8  m_cgram[0x40] = {};
	int m_ac = 0;
	int m_active_ram = DDRAM;
	int m_direction = 1;
	int m_disp_shift = 0;
	int m_num_line = 1;
	int m_char_size = 8;
	int m_data_len = 8;
	bool m_shift_on = false;
	bool m_display_on = false, m_cursor_on = false, m_blink_on = false;
	bool m_nibble = false;      // 4bit 接続のときの上位/下位
	u8  m_ir = 0, m_dr = 0;
};

#endif // S_MU2000_HD44780_H
