// license:BSD-3-Clause
//
// 音源の状態を変えない MIDI の再送を、直列に載せる前に落とす（issue #18 の続き）。
//
// firmware が MIDI をさばける速さは 1 秒に 3kB ほど（mu2000.h の MIDI_QUEUE_LIMIT の説明）。
// DAW はブロックの境界ごとに Automation の値をまとめて送り直すので、何も変わらない
// CC・ピッチベンド・プログラムチェンジの洪水が同じ 3kB/s の列に載り、そのうしろに
// 控えた鍵の音が遅れて出てくる（issue #18 で実測した「40 秒遅れて全部さばく」）。
// 実機でも同じことが起きるが、実機は 28MHz の SH-2 を速くできない。这里是
// emulator なので、直列に載せる前に数えることができる。
//
// 見立て: チャンネルの制御値は「最後の書き込みが勝つ」状態なので、音源がすでに
// 持っている値と同じメッセージをもう一度渡しても状態は 1 ビットも変わらない。
// 落とすのはそうした「書き込み先と同じ値の書き込み」だけ。音の強弱やノートは
// 毎回仕事なので落とさない。
//
// 落としてはいけないもの（値が同じでも意味が変わるもの）:
//   ・CC6 / CC38（データ入力）: 値でなく「選ばれたパラメータ」に書く。同じ値でも
//     選んでいる RPN/NRPN が違えば別の仕事（VST3 版 plugin.cpp の 1059 行の注記と同じ罠）
//   ・CC96 / CC97（インクリメント）: 同じ値を繰り返すこと自体が仕事
//   ・CC120 / CC123（オールノートオフほか）: 2 回目のあいだに新しい鍵が鳴っていたら
//     2 回目は本物の消音になる
//   ・CC121（全コントローラ初期化）: 通したあと表を白紙にする（下表）
//   ・SysEx・ケーブルメッセージ（F5）・リアルタイム: 素通し。 SysEx は末尾で
//     表ごと白紙にする（XG のパラメータ dump が裏で値を変えているかもしれない）
//
// 表は音源に届いたものなら何でも更新する（DAW の口からでも、画面からでも、
// 状態を戻したときでも）。画面で音量を 90 にしてからホストが 100 を送り直したら、
// 表は 90 なので 100 は素通しされる — 落としたつもりで音を奪うことが無い。

#ifndef S_MU2000_UI_MIDI_FILTER_H
#define S_MU2000_UI_MIDI_FILTER_H

#pragma once

#include <cstdint>
#include <cstring>

#include "mu2000.h"

namespace ui {

class midi_filter
{
public:
	static constexpr uint8_t kUnknown = 0xFF;   // まだ何も届いていない

	midi_filter() { wipe_all(); }

	// 音源に届けようとしているメッセージがこの口にもう入っている値とまったく
	// 同じなら true（落としてよい）。msg はステータスバイトを含む完成した
	// メッセージ（ランニングステータスは読まない — プラグインの口は毎回
	// ステータスをくれる）。SysEx と F5 は呼ぶ前に除けておくこと
	bool duplicate(int port, const uint8_t *msg, int n) const
	{
		if (port < 0 || port >= mu2000::MIDI_PORTS || n < 2)
			return false;
		const uint8_t st = msg[0];
		if (st < 0x80 || st >= 0xf0)
			return false;                       // ステータス無しとシステムは通す
		const int ch = st & 0x0f;
		const chan &c = m_c[port][ch];
		switch (st & 0xf0) {
		case 0xb0:                              // コントロールチェンジ
			if (n < 3)
				return false;
			// 上の一覧: 値が同じでも仕事が変わるものは数えない
			if (msg[1] == 6 || msg[1] == 38 || msg[1] == 96 || msg[1] == 97 ||
			    msg[1] == 120 || msg[1] == 121 || msg[1] == 123)
				return false;
			return c.cc[msg[1]] == msg[2];
		case 0xe0:                              // ピッチベンド
			if (n < 3)
				return false;
			return c.bend == uint16_t(msg[1] | (uint16_t(msg[2]) << 7));
		case 0xd0:                              // チャンネルプレッシャー
			return c.press == msg[1];
		case 0xc0:                              // プログラムチェンジ。バンクレールが
			// 前に適用したときと同じでないと、同じプログラムでも別の音色になる
			return c.prog == msg[1] && c.prog != kUnknown &&
			       c.pc_bank[0] == c.cc[0] && c.pc_bank[1] == c.cc[32];
		default:
			return false;                       // ノートオン・オフ、ポリAT は毎回仕事
		}
	}

	// 完成したチャンネルメッセージが音源に届いた。控制台の最新値として覚える
	// （watch が全経路から呼ぶので、画面で触った値もここを通る）
	void apply(int port, uint8_t st, uint8_t d0, uint8_t d1)
	{
		if (port < 0 || port >= mu2000::MIDI_PORTS || st < 0x80 || st >= 0xf0)
			return;
		const int ch = st & 0x0f;
		chan &c = m_c[port][ch];
		switch (st & 0xf0) {
		case 0xb0:
			c.cc[d0] = d1;
			// 全コントローラ初期化は音源の値ごと白紙になる。控制台も白紙に
			if (d0 == 121)
				wipe_channel(port, ch);
			break;
		case 0xe0:
			c.bend = uint16_t(d0 | (uint16_t(d1) << 7));
			break;
		case 0xd0:
			c.press = d0;
			break;
		case 0xc0:
			c.prog = d0;
			c.pc_bank[0] = c.cc[0];
			c.pc_bank[1] = c.cc[32];
			break;
		default:
			break;
		}
	}

	// この口の表を白紙に。SysEx が通ったとき（中身で裏から値が変わっているかも）と、
	// 状態の読み戻しのあとに呼ぶ
	void wipe(int port)
	{
		if (port < 0 || port >= mu2000::MIDI_PORTS)
			return;
		for (int ch = 0; ch < 16; ch++)
			wipe_channel(port, ch);
	}

	void wipe_all()
	{
		for (int p = 0; p < mu2000::MIDI_PORTS; p++)
			wipe(p);
	}

	// 落としたバイト数（一覧や記録に出す）
	uint64_t dropped() const { return m_dropped; }
	void count_dropped(int n) { m_dropped += uint64_t(n); }

private:
	void wipe_channel(int port, int ch)
	{
		chan &c = m_c[port][ch];
		std::memset(c.cc, kUnknown, sizeof(c.cc));
		c.bend = 0xFFFF;
		c.press = kUnknown;
		c.prog = kUnknown;
		c.pc_bank[0] = c.pc_bank[1] = kUnknown;
	}

	struct chan {
		uint8_t  cc[128];         // 最後に届いた CC の値（kUnknown は未到達）
		uint16_t bend;            // 最後に届いた 14bit ベンド
		uint8_t  press;           // チャンネルプレッシャー
		uint8_t  prog;            // 最後に適用したプログラム
		uint8_t  pc_bank[2];      // そのときに効いていた CC0 / CC32
	};
	chan m_c[mu2000::MIDI_PORTS][16];
	uint64_t m_dropped = 0;
};

} // namespace ui

#endif // S_MU2000_UI_MIDI_FILTER_H
