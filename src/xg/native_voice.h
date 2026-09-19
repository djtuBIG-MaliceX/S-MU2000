// license:BSD-3-Clause
//
// 音色の記録（ROM の 84 バイト）から、SWP30 のスロットのレジスタを組み立てる。
// **firmware を走らせずに音を出す**ための最初の部品（doc/native-engine.md の段 2）。
//
// 番地と式はすべて firmware を読んで決めた（同 6.2-6.6）。分かっていない所は
// 「まだ分からない」と書いて、実機を鳴らして測った値をそのまま置いてある。
// ここに入っているのは**式だけ**で、ROM の中身は持たない（実行時に読むだけ）。

#ifndef S_MU2000_XG_NATIVE_VOICE_H
#define S_MU2000_XG_NATIVE_VOICE_H

#pragma once

#include "compat/mamecompat.h"

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <vector>

namespace xg {
namespace nv {

// ROM の中の番地（MU2000 EX firmware v2.01）
constexpr u32 SET_TABLE  = 0x200AF0;   // 波形の組 → 波形の並びの中の位置（16bit を 503 個）
constexpr u32 SET_COUNT  = 0x1F8;
constexpr u32 WAVE_BASE  = 0x1F55A0;   // 波形の記録（16 バイトずつ）
constexpr u32 ATTACK_TAB = 0x1F4DB8;   // アタックの速さ（128 バイト）
constexpr u32 DECAY_TAB  = 0x1F4E38;   // 減衰の速さ（128 バイト）
constexpr u32 VEL_CURVE  = 0x1E5E5E;   // 強さの曲線（128 バイトの行が並ぶ。行 0 はそのまま）
constexpr u32 LEVEL_TAB  = 0x1E6798;   // 0-127 → 減衰（128 バイトの行が並ぶ。行 1 が 0x1E6818）
constexpr u32 SLOT_TABLE = 0x1F4F58;   // スロット番号 → レジスタの先頭（4 バイト × 64）
constexpr u32 CUTOFF_TAB = 0x1E5B58;   // フィルタの切る高さ（16bit。索引は記録の byte37）

inline int s8(u8 v) { return v >= 128 ? int(v) - 256 : int(v); }
inline u16 rd16(const u8 *rom, u32 a) { return u16(rom[a] << 8 | rom[a + 1]); }
inline u32 rd32(const u8 *rom, u32 a)
{ return u32(rom[a]) << 24 | u32(rom[a + 1]) << 16 | u32(rom[a + 2]) << 8 | rom[a + 3]; }

// 音色の記録の 84 バイト（要素 1 つぶん）。rec は xg::voice_rom::lookup の戻り値
inline const u8 *element(const u8 *rom, u32 rec, int index = 0)
{
	return rom + rec + 12 + u32(index) * 84;
}
// 記録の先頭のバイトは**要素のビットマスク**（1/3/7/15 ＝ 1〜4 要素）。
// 数ではないので、立っているビットを数える
inline int element_count(const u8 *rom, u32 rec)
{
	int n = 0;
	for (int i = 0; i < 4; i++)
		if (rom[rec] & (1 << i))
			n++;
	return n;
}

// その要素が、この鍵と強さで鳴るか（byte4,5 が鍵の範囲、byte6,7 が強さの範囲）
inline bool element_active(const u8 *elem, int note, int vel)
{
	return note >= elem[4] && note <= elem[5] && vel >= elem[6] && vel <= elem[7];
}

// 波形の組の番号（7bit が 2 つ）
inline int wave_set(const u8 *elem) { return (elem[2] << 7) | (elem[3] & 0x7f); }

// その鍵で使う波形の記録（16 バイト）。無ければ nullptr
inline const u8 *wave_entry(const u8 *rom, int setno, int note)
{
	if (setno < 0 || setno >= int(SET_COUNT))
		return nullptr;
	u32 s = WAVE_BASE + rd16(rom, SET_TABLE + u32(setno) * 2);
	for (int i = 0; i < 80; i++) {
		if (rom[s + 3] >= note || rom[s + 3] == 0x7f)
			return rom + s;
		s += 16;
	}
	return nullptr;
}

// 波形の記録の中身
struct wave_info {
	int level;         // この波形ぶんの減衰（0.375dB 目盛り。多段サンプルで段ごとに違う）
	int base_key;      // もとの音程（半音）
	int fine_cents;    // その細かい調整（セント。引く）
	int key_max;       // この記録を使う鍵の上限
	u32 pre_loop;      // ループ前のサンプル数（レジスタ 0x12/0x13）
	u32 loop_len;      // ループの長さ（0x14/0x15）
	u32 format_addr;   // 形式＋波形 ROM の番地（0x16/0x17）
};

inline wave_info read_wave(const u8 *e)
{
	wave_info w{};
	w.level      = e[0];
	w.base_key   = e[1];
	w.fine_cents = e[2] >= 128 ? int(e[2]) - 256 : int(e[2]);
	w.key_max    = e[3];
	w.pre_loop   = u32(e[4]) << 24 | u32(e[5]) << 16 | u32(e[6]) << 8 | e[7];
	w.loop_len   = u32(e[8]) << 24 | u32(e[9]) << 16 | u32(e[10]) << 8 | e[11];
	w.format_addr = u32(e[12]) << 24 | u32(e[13]) << 16 | u32(e[14]) << 8 | e[15];
	return w;
}

// 音程のレジスタ（0x11）。1 オクターブ = 1024、細かい調整はセント（**足す**）。
// 実測（鍵 0-127・18 区画）と ±0.7 目盛りで合う
// 鍵の追従率（記録の byte19）。0 が普通の 100 セント/半音で、
// 1 が半分、2 が 1/5、3 が 1/10。効果音の音色でよく使う
inline int key_follow(const u8 *elem)
{
	static const int F[4] = { 100, 50, 20, 10 };
	return F[elem[19] & 3];
}

// 要素を**遅らせて鳴らす**段（byte72）。実測（段 0,1,2,3 → 0,311,752,1634 サンプル）は
// 441 * 2^(n-1) - 130 でぴったり。MusicBox は 2 つ目の要素を 37ms 遅らせている
inline u32 elem_delay(const u8 *elem)
{
	const int n = elem[72] & 0x7f;
	if (n <= 0)
		return 0;
	return u32(441 * (1 << (n < 8 ? n - 1 : 7)) - 130);
}

// **波形を選ぶときの鍵**。要素の粗調（byte17）で移した鍵で選ぶ。
// GtHarmonics（音色 31）は要素が 12 半音下げていて、鍵 84 のときに
// 鍵 72 のぶんの波形を鳴らしていた（それで音程がぴったり合う）
inline int wave_note(const u8 *elem, int note)
{
	const int n = note + int(elem[17]) - 64;
	return n < 0 ? 0 : (n > 127 ? 127 : n);
}

// 要素ぶんの音程のずらし（セント）。byte17 が半音、byte18 がセント
inline int elem_tune(const u8 *elem)
{
	return (int(elem[17]) - 64) * 100 + (int(elem[18]) - 64);
}

// **ポルタメントの速さ**（doc/native-engine.md の 6.41）。
// ROM の表 0x1E6698（16bit・128 語）を CC5 で直に引く。目盛りが 2 通りある:
//   CC5 24-127 … 表 ÷ 128 = 10ms あたりのセント
//   CC5  0-23  … 表 × 2   = 10ms あたりのセント（256 倍の目盛り）
// 返すのは**セント × 256**（そのまま足し引きできる細かさ）
constexpr u32 PORTA_TAB = 0x1E6698;
constexpr u32 PORTA_TICK = 441;            // firmware は 10ms ごとに足す

inline int porta_step(const u8 *rom, int cc5)
{
	if (!rom || cc5 < 0 || cc5 > 127)
		return 0;
	const int raw = int(rd16(rom, PORTA_TAB + u32(cc5) * 2));
	return cc5 < 24 ? raw * 512 : raw * 2;
}

inline u16 pitch_reg(const wave_info &w, int note, int follow = 100, int cents_extra = 0)
{
	// 整数で計算する（firmware と同じ丸めになる。0 の側へ切り捨て）。
	// **鍵の追従は鍵 60 を支点にする**（波形の基準鍵ではない）。追従が 100 の
	// ときは同じ式になるが、50 や 20 の音色では基準鍵とのずれぶん食い違う
	// （Woodblock で 749 セント、TaikoDrum で 1700 セント。どちらも
	//  50 * (60 - 基準鍵) でぴったり）
	const int cents = (note - 60) * follow + (60 - w.base_key) * 100
	                + w.fine_cents + cents_extra;
	const int v = cents * 1024 / 1200;
	// ビット 14 は波形の**形式**で決まる（形式 3 のときだけ立つ。402 組で確かめた）
	const u16 flag = ((w.format_addr >> 30) & 3) == 3 ? 0x4000 : 0;
	return u16((v & 0x3fff) | flag);
}


// ---- コントローラ（doc/native-engine.md の 6.14）
//
// 実機が何を書くかは `nativeplay --ccwatch` で見た:
//   CC7・CC11 → レジスタ 0x09 の下位バイト（減衰）
//   CC10      → レジスタ 0x32（上が左・下が右の減衰）
//   ベンド    → レジスタ 0x11（音程）
//   CC1       → レジスタ 0x0a の下位バイト（LFO の深さ）

// 音量（CC7）・表現（CC11）の減衰。level→減衰の表（0.375dB 目盛り）を 2 倍すると
// レジスタ 0x09 の目盛り（0.1875dB）になる。cc>=8 で実測との差は 0.375dB 以内
// **音量と表現は掛けてから一度だけ減衰に直す**（実機の 0x12A404 がそうしている）。
// firmware はパートの塊の +0x12E-0x130 に `((CC7+1) * (CC11+1)) >> 7` を
// 線形のまま持っていて（`nativeplay --ccbyte 7` と `--ccbyte 11` で確かめた。
// CC7 だけ振ると cc+1、CC11 だけ振ると 101*(cc+1)/128 でぴったり）、
// それを音の level に掛けてから減衰に直す。
// 前は CC7 と CC11 を別々に減衰へ直して足していたので、実機とずれていた
inline int vol_gain(int vol, int expr)
{
	const int v = vol < 0 ? 100 : (vol > 127 ? 127 : vol);
	const int e = expr < 0 ? 127 : (expr > 127 ? 127 : expr);
	return ((v + 1) * (e + 1)) >> 7;        // 0-128
}

// その線形の値（0-128）を減衰に直す
inline int gain_att(const u8 *rom, int gain)
{
	if (gain <= 0)
		return 255;
	return 2 * int(rom[LEVEL_TAB + u32(std::min(128, gain) - 1)]);
}

inline int cc_vol_att(const u8 *rom, int cc)
{
	if (cc <= 0)
		return 255;
	return 2 * int(rom[LEVEL_TAB + u32(std::min(127, cc) - 1)]);
}

// パン（CC10）の減衰。中央で左右とも -3dB になる cos 則。
// 右側は pan_att(128 - cc10)。128 点すべて実測と 0.1875dB 以内で合う
inline int pan_att(int x)
{
	if (x <= 0)
		return 0;
	if (x >= 127)
		return 255;
	const double c = std::cos(double(x) / 127.0 * 1.5707963267948966);
	const int v = int(std::lround(-20.0 * std::log10(c) / 0.375));
	return v < 0 ? 0 : (v > 255 ? 255 : v);
}

// 明るさ（CC74）→ レジスタ 0x00 の下 12bit（切る高さ）。
// 実測（`nativeplay --ccfilter`）は **16 × (値 - 64)** でまっすぐ動き、1984 で頭打ち
constexpr int CUTOFF_MAX = 1984;
inline int bright_shift(int cc) { return 16 * (cc - 64); }

// 共振（CC71）→ レジスタ 0x04 の上 5bit。実測は **2 きざみで 1 段**
// （64 まで 0、67 で 1、127 で 31）
inline int reso_shift(int cc) { return (cc - 64) / 2; }

// 送り（CC91 リバーブ・CC93 コーラス）→ レジスタ 0x33・0x34 の下位（減衰）。
// 実測は **16 + level→減衰の表** で、音色によらない（GrandPno・Strings・Flute で同じ）。
// 使うのは差ぶんだけなので、下駄の 16 は要らない
inline int send_att(const u8 *rom, int cc)
{
	if (cc <= 0)
		return 255;
	return int(rom[LEVEL_TAB + u32(std::min(127, cc) - 1)]);
}

// モジュレーション（CC1）→ レジスタ 0x0a の下位（LFO の深さ）に足す。
// 実測は 10 段で、**音色によらない**（GrandPno・Strings・SawLead で同じ）。
// 0x0a の上位は LFO の型と刻みなので触らない
inline int mod_depth(int cc)
{
	static const u8 STEP[10] = { 0, 9, 17, 26, 35, 43, 52, 60, 72, 84 };
	static const u8 EDGE[9]  = { 13, 26, 39, 52, 64, 77, 90, 103, 116 };
	int i = 0;
	while (i < 9 && cc >= int(EDGE[i]))
		i++;
	return int(STEP[i]);
}

// ピッチベンド → セント。firmware は 2 回とも 0 の側へ切り捨てる
// （ベンド幅 2 半音・目一杯で 167 目盛り。実測と一致）
inline int bend_cents(int bend14, int range_semitones)
{
	return (bend14 - 8192) * range_semitones * 100 / 8192;
}

// 0..255 に収める
inline int clamp_att(int v) { return v < 0 ? 0 : (v > 255 ? 255 : v); }

// 組み立てたスロットのレジスタ。write が立っている所だけ書く
struct slot_regs {
	u16 v[0x40];
	u64 write;         // ビット n が立っていればレジスタ n を書く

	slot_regs() { std::memset(v, 0, sizeof(v)); write = 0; }
	void set(int reg, u16 value) { v[reg] = value; write |= u64(1) << reg; }
};

// 分かっていない所に置く値。**実機を鳴らして測った、素直な音色のときの値**で、
// これは「式が分かっていない」という印でもある（doc/native-engine.md の 6.6）
struct defaults {
	u16 filter1 = 0x1000 | 0x7ff;   // 開き切り
	u16 bypass  = 0xdcff;           // 上位は「前の値からの変わり方」で決まる（0x127F10）
	u16 filter2 = 0x8000;
	u16 post    = 0x5010;           // ここは定数だと分かっている
	u16 filter2p = 0x0000;
	u16 lfo_amp = 0xfa00;
	u16 lfo     = 0x5f00;
	u16 r0b     = 0x7f00;
	u16 r10     = 0x4000;
	// ミキサ（パート 1・音量 100・パン中央・リバーブ送り 40 のときの実測）。
	// **0x32-0x37 だけ**。0x38-0x3d は「入力 0x40 から先」＝ MEG の戻りや A/D の
	// ぶんで、声のスロットのものではない。ここを書くと残響の混ざり方が変わる
	u16 mix[6] = { 0x0808, 0x182b, 0xffff, 0x4d00, 0x4800, 0x4400 };
	// 声ごとの IIR（パートの EQ）。素通しのときの実測
	u16 iir[6] = { 0xe05d, 0x1fa3, 0x2000, 0x0257, 0xfda9, 0x2000 };
};

// 強さから、音量レジスタに足す減衰を出す（firmware の 0x128DA0）。
//
//   減衰 = 表2[0x1E6798 + 表1[0x1E5E5E + 曲線*128 + 強さ]]
//
// 曲線は音色ごと（普通は 0 ＝ そのまま）。GrandPno の強さ 1-127 の全段で、
// 実機の値とぴったり一致する。
inline int velocity_att(const u8 *rom, int vel, int curve = 0)
{
	const int i = rom[VEL_CURVE + u32(curve) * 128 + u32(vel & 0x7f)];
	return rom[LEVEL_TAB + u32(i & 0x7f)];
}

inline int rd16s(const u8 *rom, u32 a)
{
	const int v = int(rd16(rom, a));
	return v >= 0x8000 ? v - 0x10000 : v;
}

// ---- **音程の包絡線（ピッチ EG）**（doc/native-engine.md の 6.68）
//
// スロットの `0x10` は「行き先の音程のずれ」、`0x0b` の上位バイトは「その速さ」で、
// 近づけるのはチップの仕事（swp30.cpp の `peg_step`）。firmware は
// **キーオンの直前に初めの高さを書き、キーオンの直後に行き先を書く**だけ。
//
// 実機の `0x12BAC8`-`0x12BBA0` を起こした:
//
//   速さ = 表 0x1E6C94[ clamp(byte26 + ((64 - パート[26]) >> 2), 0, 63) ]
//   初めの高さ = 速さが 127（＝即到達）なら byte31、そうでなければ byte30
//   行き先     = byte31
//   セント = 段（byte21）で目盛りを変えた (高さ - 64) × 75
//   レジスタ = 0x4000 | (表 0x1E6D14[セント] & 0x3fff)
//
// Violin(40) は byte30=18・byte21=0 → (18-64)×75>>2 = -862 セント → -183。
// 実機の `0x10` は `7f49`（= 0x4000 | (-183 & 0x3fff)）でぴったり合った。
// Trumpet・BrssSec・MuteTrp・Piccolo・PanFlute・Clarinet・Recorder でも合う
constexpr u32 PEG_RATE_TAB  = 0x1E6C94;   // 速さの目盛り → レジスタ（64 語）
constexpr u32 CENT_PITCH_TAB = 0x1E6D14;  // セント → 音程の目盛り（0-4800）

// 16bit に切り詰める（実機の EXTS.W）
inline int s16v(int v) { return int(s16(u16(v))); }

// セント → 音程の目盛り（1 オクターブ = 256）。実機の `0x12B860`
inline int cents_to_pitch(const u8 *rom, int cents)
{
	if (!rom || !cents)
		return 0;
	if (cents > 0)
		return cents < 4801 ? rd16s(rom, CENT_PITCH_TAB + u32(cents) * 2) : 1024;
	return cents > -4801 ? -rd16s(rom, CENT_PITCH_TAB + u32(-cents) * 2) : -1024;
}

// 高さに掛かる**強さの効き**（実機の `0x12BD40`）。byte22 が 64 なら 0
inline int peg_vel_depth(const u8 *elem, int vel)
{
	const int d = int(elem[22]) - 64;
	if (!d)
		return 0;
	const int x = 36 * (d > 0 ? d : -d);
	const int m = d > 0 ? (0x80 - (vel & 0x7f)) : (vel & 0x7f);
	return int((u32(s16v(x * m)) * 2 & 0xffff) >> 8);
}

// 高さ（0-127、64 が中央）→ セント。実機の `0x12BBAE`。
// byte21 が目盛り: 0 なら 18.75・1 なら 37.5・2 なら 75・3 なら 150 セント刻み
inline int peg_cents(const u8 *elem, int level, int vel)
{
	const int d = level - 64;
	if (!d)
		return 0;
	const int k = peg_vel_depth(elem, vel);
	int m;
	if (d > 0) {
		m = d + 1;                                   // 実機は正の側だけ 1 を足す
		m -= int((u32(s16v(m * k)) & 0xffff) >> 8);
	} else {
		m = d + int((u32(-s16v(d * k)) & 0xffff) >> 8);
		m = -m;
	}
	int v = s16v(m * 75);
	switch (elem[21]) {
	case 0: v = s16v(v) >> 1; v = s16v(v) >> 1; break;
	case 1: v = s16v(v) >> 1; break;
	case 3: v = s16v(v) << 1; break;
	default: break;
	}
	return d > 0 ? v : -v;
}

// 速さの**鍵追従**（実機の `0x12BC72`）。byte24 が深さ、byte25 が折れ点。
// SquareLd（byte24=62・byte25=60）は鍵 60 で 63（即到達）、鍵 72 で 61、
// 鍵 84 で 60 になり、実機とぴったり合った
inline int peg_rate_key_adj(const u8 *elem, int note)
{
	const int d = int(elem[24]) - 64;
	if (!d)
		return 0;
	return s16v((note - int(elem[25])) * (d * 16)) >> 8;   // 算術シフト（下へ丸める）
}

// 速さの**強さ追従**（実機の `0x12BCB2`）。byte23 が深さ
inline int peg_rate_vel_adj(const u8 *elem, int vel)
{
	const int d = int(elem[23]) - 64;
	if (!d)
		return 0;
	const int a = d * 16;
	const int m = a >= 0 ? a * (vel & 0x7f) : (0x80 - (vel & 0x7f)) * (-a);
	return int((u32(s16v(m)) & 0xffff) >> 8);
}

// 速さのレジスタ（`0x0b` の上位バイト）。実機の `0x12BCF0`。
// 途中で何度も符号つき 1 バイトに切り詰めている
// 段ごとの生の速さ（段 0 は byte26、段 1 は byte27、段 2 は byte28、離しは byte29）
inline int peg_rate_raw(const u8 *elem, int stage)
{
	return int(elem[26 + (stage < 0 ? 0 : (stage > 3 ? 3 : stage))]);
}

// 段ごとの行き先の高さ（段 0 は byte31、段 1 は byte32、段 2 は byte33、離しは byte34）
inline int peg_level_of(const u8 *elem, int stage)
{
	return int(elem[31 + (stage < 0 ? 0 : (stage > 3 ? 3 : stage))]);
}

inline int peg_rate_idx_of(const u8 *elem, int raw, int note, int vel, int part_rate = 64)
{
	int r = raw + (int(s8(u8(64 - part_rate))) >> 2);
	if (s8(u8(r)) > 63) r = 63;
	if (s8(u8(r)) < 0)  r = 0;
	r += peg_rate_key_adj(elem, note);
	if (s8(u8(r)) < 0)  r = 0;
	if (s8(u8(r)) >= 63)
		return 63;
	r += peg_rate_vel_adj(elem, vel);
	if (s8(u8(r)) > 62) r = 62;
	return r;
}

inline int peg_rate_idx(const u8 *elem, int note, int vel, int part_rate = 64)
{
	return peg_rate_idx_of(elem, int(elem[26]), note, vel, part_rate);
}

inline int peg_rate_reg(const u8 *rom, const u8 *elem, int note = 60, int vel = 100,
                        int part_rate = 64)
{
	return rd16s(rom, PEG_RATE_TAB + u32(peg_rate_idx(elem, note, vel, part_rate)) * 2);
}

// 段 stage の速さのレジスタ
inline int peg_rate_reg_stage(const u8 *rom, const u8 *elem, int stage, int note, int vel,
                              int part_rate = 64)
{
	const int i = peg_rate_idx_of(elem, peg_rate_raw(elem, stage), note, vel, part_rate);
	return rd16s(rom, PEG_RATE_TAB + u32(i) * 2);
}

// `SMU2000_NO_PEG` を立てると音程の包絡線をやめる（比べるための逃げ道）
inline bool peg_on()
{
	static const bool on = std::getenv("SMU2000_NO_PEG") == nullptr;
	return on;
}

// `0x10` に書く値（セントを渡す）。**ビット 14 は byte10 で決まる**
// （実機の `0x12AD2E`。byte10 が 0 の音色は立てない。PanFlute・BirdTweet）
inline u16 peg_reg(const u8 *rom, int cents, const u8 *elem = nullptr)
{
	const u16 flag = (!elem || elem[10]) ? 0x4000 : 0;
	if (!peg_on())
		return flag;
	return u16(flag | (u16(cents_to_pitch(rom, cents)) & 0x3fff));
}

// ---- **フィルタの包絡線**（doc/native-engine.md の 6.63）
//
// 実機は firmware のソフトでこれを動かしていて、10ms ごとに
// 切る高さへ足す値を作り直す。折れ線で、状態は 3 つ:
//
//   累算  段の中でいまどこまで来たか（`[音+66]`）
//   目標  その段の行き先（`[音+68]`）
//   増分  1 段あたりの足し引き（`[音+70]`。0x8000 なら「すぐ次の段」）
//
// 10ms ごとに 累算 += 増分 して、向きに応じて目標を越えたら次の段へ。
// 切る高さに足す値は **累算 >> 2**。
//
// 段は要素のバイトで決まる（要素 + 2 を基準に読んでいるので、ここでは
// 要素そのものの番号で書く）:
//
//   はじめの累算 = 目標(byte55)
//   段 1: 目標 = 目標(byte56)、速さ = byte51
//   段 2: 目標 = 目標(byte57)、速さ = byte52
//
// Kitayama（0,72,5）鍵 60・強さ 100 の実機の値で全部合わせた（6.63）
constexpr u32 FENV_INC_TAB = 0x1E5C58;   // 速さ → 増分（16bit 符号つき × 64）

// 0 の側へ丸める >>8（実機は符号で分けている）
inline int sh8(int v) { return v >= 0 ? (v >> 8) : -((-v) >> 8); }

// **包絡線の深さ**（実機の `[音+93]`。`0x128ADC`）。
// 強さの表を byte8 で選び、byte46 の深さと掛け合わせる。
//   深さ = ((36 × (byte46 - 64)) × (0x80 - 表[強さ]) × 2) >> 8
// 表は byte8 が 0 なら 0x1E5D58、そうでなければ 0x1E5DD8。
// GrandPno（byte46=70・byte8=1・強さ 100）で 111、
// Kitayama（byte46=71・byte8=0）で 72。どちらも実機の値と一致した。
// **パートの塊 +210 が 0 でないときの枝はまだ起こしていない**
// （そこは深さがもう一段変わる。既定の音色では 0）
constexpr u32 FENV_VEL_TAB0 = 0x1E5D58;
constexpr u32 FENV_VEL_TAB1 = 0x1E5DD8;

inline int fenv_depth(const u8 *rom, const u8 *elem, int vel)
{
	if (!rom || !elem)
		return 0;
	const int d = int(elem[46]) - 64;
	if (d < 0)
		return 0;                    // 負の枝はまだ起こしていない
	const u32 tab = elem[8] ? FENV_VEL_TAB1 : FENV_VEL_TAB0;
	const int t = rom[tab + u32(vel & 0x7f)];
	const int v = (36 * d) * (0x80 - t);
	return int((u32(v) * 2 & 0xffff) >> 8);
}

// レベルのバイト → 目標
inline int fenv_target(const u8 *rom, const u8 *elem, int level, int vel)
{
	const int x = (level - 64) * 2;
	return (x - sh8(x * fenv_depth(rom, elem, vel))) * 64;
}

// 速さへの足し込み。鍵のぶん（byte48 が深さ・byte49 が基準鍵）と
// 強さのぶん（byte47 が深さ）
inline int fenv_key_adj(const u8 *elem, int note)
{
	const int d = int(elem[48]) - 64;
	return d ? sh8((note - int(elem[49])) * (d * 16)) : 0;
}
inline int fenv_vel_adj(const u8 *elem, int vel)
{
	const int d = int(elem[47]) - 64;
	if (!d)
		return 0;
	const int a = d * 16;
	return sh8(a >= 0 ? a * vel : -((-a) * (0x80 - vel)));
}

// 速さ → 増分。63 以上は「すぐ次の段」の印
constexpr int FENV_NEXT = 0x8000;
inline int fenv_inc(const u8 *rom, int rate)
{
	if (rate >= 63)
		return FENV_NEXT;
	if (rate < 0)
		rate = 0;
	return rd16s(rom, FENV_INC_TAB + u32(rate) * 2);
}

// 音色ごとの下駄。firmware は「音色の音量 → 表」と、鍵ごとの足し込みで作る。
// 式そのものはまだ解けていないので、**1 回だけ実機に鳴らしてもらって校正する**（下）。
// 校正しないときの当て値（実測の中央値。5〜19 の幅がある）
constexpr int VOICE_ATT_TYPICAL = 12;

constexpr u32 LEVEL_CURVE = 0x23CED0;   // 鍵による音量の曲線（128 バイトの行が並ぶ）

// **鍵による切る高さのずれ**（実機の 0x12C1E4 → 0x12C20E）。
// 音量の鍵曲線とまったく同じ仕掛けで、記録の byte38 が 0xFF なら
// ROM の曲線表（LEVEL_CURVE）を byte44,byte45 が指す行で引き、
// **その符号つきの値を 32 倍**して 12bit の切る高さに足す。
// 32 倍なので、鍵を上げ下げすると 32 きざみの階段になる（実測と一致）。
// 実測（`nativeplay --keycut`）と Strngs2・GrandPno・DrawOrg で
// 差が完全に一定になった（doc/native-engine.md の 6.56）
inline int cutoff_key_curve(const u8 *rom, const u8 *elem, int note)
{
	if (!rom || !elem || elem[38] != 0xff)
		return 0;        // 折れ線の枝はまだ起こしていない
	const u32 row = (u32(elem[44]) << 8 | elem[45]) * 128;
	const u32 a = LEVEL_CURVE + row + u32(note & 0x7f);
	return s8(rom[a]) * 32;
}


// 音量の鍵による増減。記録の byte60 が 0xFF のときは ROM の曲線表を引く
// （byte66,byte67 が行の番号）。符号付きで、鍵ごとに ±10 ほど動く
inline int level_key_curve(const u8 *rom, const u8 *elem, int note)
{
	if (elem[60] != 0xff)
		return 0;                       // 折れ線の形はまだ入れていない
	const u32 idx = u32(elem[66]) << 8 | elem[67];
	const u32 a = LEVEL_CURVE + idx * 128 + u32(note & 0x7f);
	if (a >= 0x400000)
		return 0;
	return int(s8(rom[a]));
}

// 減衰 → 音量の目盛り（表を逆に引く）。同じ減衰になる目盛りが 3-4 段
// 並ぶので、**いちばん上（音量が大きい側）**を返す。真ん中を返していた
// ときは、鍵の曲線を足したあとで表の段を 1 つ踏み外していた
// （Strings の鍵 48 が 2 段ぶん静かになっていた）
inline int level_from_att(const u8 *rom, int att)
{
	int lo = -1, hi = -1;
	for (int i = 0; i < 128; i++)
		if (rom[LEVEL_TAB + 0x80 + i] == att) {
			if (lo < 0) lo = i;
			hi = i;
		}
	return lo < 0 ? 64 : hi;
}

// **校正**: 1 回だけ実機（firmware）に鳴らしてもらった減衰から、その音色の
// 「素の音量」を出す。これがあれば、ほかの鍵・強さの減衰は式で出せる
inline int wave_level(const u8 *rom, const u8 *elem, int note)
{
	const u8 *we = wave_entry(rom, wave_set(elem), wave_note(elem, note));
	return we ? int(we[0]) : 0;
}

// 鍵の曲線が音量の目盛りに効く倍率。**分数で持つ**（1 倍でも 2 倍でもない）。
// `nativeplay --levelcheck` で音色ごとに総当たりすると、外れがいちばん少なく
// なるのはどれも 6/4 = 1.5 倍のあたりに集まった（16 音色で確かめた）。
// 1 倍や 2 倍にすると、鍵 60 から離れたところでずれる
constexpr int LEVEL_CURVE_NUM = 6;
constexpr int LEVEL_CURVE_DEN = 4;

inline int level_curve_scaled(const u8 *rom, const u8 *elem, int note)
{
	return level_key_curve(rom, elem, note) * LEVEL_CURVE_NUM / LEVEL_CURVE_DEN;
}

inline int calibrate_level(const u8 *rom, const u8 *elem, int att_ref, int note_ref, int vel_ref)
{
	const int rest = att_ref / 2 - velocity_att(rom, vel_ref) - wave_level(rom, elem, note_ref);
	return level_from_att(rom, rest) - level_curve_scaled(rom, elem, note_ref);
}

// 校正した素の音量から、その鍵・強さの減衰（0x09 に入れる値）
inline int volume_att(const u8 *rom, const u8 *elem, int base_level, int note, int vel)
{
	int l = base_level + level_curve_scaled(rom, elem, note);
	if (l < 0) l = 0;
	if (l > 127) l = 127;
	// 波形の記録の先頭のバイトが、その段ぶんの減衰。多段サンプルの音色では
	// 段の変わり目で 1.5dB ほど動くので、これを入れないと段ごとにずれる
	const int a = rom[LEVEL_TAB + 0x80 + u32(l)] + velocity_att(rom, vel)
	            + wave_level(rom, elem, note);
	return std::min(0xff, a * 2);
}

// 減衰・離しの速さに乗る、鍵による補正（firmware の 0x12ADD0）
inline int rate_key_corr(const u8 *elem, int note)
{
	int c = (note - int(elem[71])) * (int(elem[70]) - 64) * 16;
	if (c < 0)
		c += 0xff;
	return c >> 8;
}

// 減衰の表の目盛りを 0-127 に収める
inline int clamp_idx(int i) { return i < 0 ? 0 : (i > 127 ? 127 : i); }

inline int rate_scale(int raw, int corr)
{
	int v = raw + corr;
	if (v <= 0) v = 1;
	if (v > 63) v = 63;
	return v * 2;
}

// **減衰 2 だけは下限が 0**（実機の `0x127338`。減衰 1 の `0x1272F4` は 1）。
// 表の頭は 1,1,2,2,… なので、0 と 1 で値が変わる。byte75 が 0 の音色
// （Trumpet・BrssSec・SquareLd）で実機は 1、こちらは 2 になっていた
inline int rate_scale2(int raw, int corr)
{
	int v = raw + corr;
	if (v < 0) v = 0;
	if (v > 63) v = 63;
	return v * 2;
}

// ---- **共振**（レジスタ `0x04`）。実機の `0x12806A` と `0x12810A`
//
//   目減り = (18 × |byte81 - 64| × (byte81>64 ? 0x80-強さ : 強さ)) & 0xffff >> 8
//   値     = max(byte35 - 目減り, 0)
//   パート（+25）の下駄を足して、>>1 して 5bit に収める
//
// 18 音色 × 強さ 30/100/127 の 54 通りで実機と一致した（EPiano1 は強さで
// 要素が切り替わる音色で、鳴っている側の要素で計算すれば合う）
inline int reso_vel_drop(const u8 *elem, int vel)
{
	const int d = int(elem[81]) - 64;
	if (!d)
		return 0;
	const int x = 18 * (d > 0 ? d : -d);
	const int m = d > 0 ? (0x80 - (vel & 0x7f)) : (vel & 0x7f);
	return int((u32(x * m) & 0xffff) >> 8);
}

inline int reso_level(const u8 *elem, int vel, int part_res = 64)
{
	int v = int(elem[35]) - reso_vel_drop(elem, vel);
	if (v < 0)
		v = 0;
	const int p = part_res - 64;
	int r = p >= 0 ? (p >= v ? p : v) : p + v;
	if (r < 0)
		r = 0;
	return (r >> 1) & 31;
}

// 鍵を離すときに 0x09 へ入れる値。
// 上位のビット 15 が「離せ」の印で、残りが離しの速さ（swp30.cpp の release_glo_w）。
// 速さは減衰と同じ表を **byte76** で引き、鍵の補正も同じだけ乗る
// （実機が離すときに書く値と、GrandPno の鍵 60 で一致する: 0xBE1E）
inline u16 release_reg(const u8 *rom, const u8 *elem, int note, int att)
{
	const int r = rom[DECAY_TAB + rate_scale(elem[76], rate_key_corr(elem, note))];
	return u16(((0x80 | (r & 0x7f)) << 8) | (att & 0xff));
}

// **音色の写し取り**。式が分かっていないレジスタ（フィルタ・素通しの量など）は、
// 起動のときに firmware へ 1 音だけ鳴らしてもらって、そのときの値を覚えておく。
// 鍵や強さで動かないものが多いので、これだけで実機にかなり近くなる。
// 覚えるのは**利用者の ROM から起こした値**で、配らない（起動のたびに作る）
// フィルタの包絡線の 1 段。firmware はこれをソフトで動かして、鳴っている間
// 0x00・0x01・0x04 を 10ms ごとに書き直す（doc/native-engine.md の 6.17）
struct fstep {
	u32 at;            // 鳴らし始めてからのサンプル数（rel なら離してから）
	u8  reg;
	u16 v;
	// **離したあとの段**。実機はフィルタを離しのあいだも動かし続ける。
	// 写し取りの元にした音が短いと、録れる段のほとんどがこちら側になる。
	// 押してからの並びと離してからの並びを分けて持ち、鳴らすときも
	// それぞれの時刻から流す（doc/native-engine.md の 6.57）
	u8  rel = 0;
};

struct voice_cal {
	bool have = false;
	int  base_level = 64;      // 校正した素の音量
	int  cal_vel = 100;        // 写し取ったときの強さ（強さを変えるときの基準）
	// 写し取ったときの鍵。レジスタ 0x00（切る高さ）は鍵でも動くので、
	// ここからの差ぶんだけずらす（doc/native-engine.md の 6.56）
	int  cal_note = 60;
	// 写し取ったときのコントローラの位置。ここからの差ぶんだけ動かす
	int  cal_vol = 100, cal_expr = 127, cal_pan = 64, cal_mod = 0;
	int  cal_rev = 40, cal_cho = 0;      // 写し取ったときの送り（CC91・CC93）
	int  cal_bri = 64, cal_res = 64;     // 写し取ったときの明るさ・共振（CC74・CC71）
	// **写し取ったときのパートの「経路」**（素通しの量・バリエーション送り・
	// パートの EQ・インサーションの掛かり先）をまとめた印。
	// ここが違うと、写し取った 0x20-0x2b・0x32-0x37 はそのまま使えない
	u32  cal_ctx = 0;
	// **減衰の表の目盛りのずれ**（写し取ったときの実機の値と、こちらの式の差）。
	// 減衰は鍵で変わるので写し取った値をそのまま使えないが、ずれは鍵に
	// よらないとみて、式で出した目盛りにこれを足す。これでパート側の
	// EG の設定（CC75 など）も、こちらの式の小さなずれも一緒に吸収できる。
	// **表の目盛りそのもの**で持つ（実機は奇数の目盛りも使うので、
	// rate_scale の「2 倍」の単位では足りない）
	int  dec_adj[2] = { 0, 0 };
	u16  reg[0x40] = {};       // 基準の鍵・強さでの値
	u64  mask = 0;             // 覚えているレジスタ

	bool has(int r) const { return (mask & (u64(1) << r)) != 0; }
	void set(int r, u16 v) { reg[r] = v; mask |= u64(1) << r; }

	// 写し取った音で、firmware がフィルタをどう動かしたか。
	// あとの音でも同じように動かす（鍵と強さは変わるが、形は近い）
	std::vector<fstep> filter_env;

	// そのスロットが鳴らしていた波形の番地（0x16/0x17）
	u32 wave_addr() const { return u32(reg[0x16]) << 16 | reg[0x17]; }
};

// 要素と、写し取ったスロットを**波形の番地で**結び付ける。
// 要素の並びとスロットの並びが同じとは限らないので、順番では当てにならない
// used には「もう使った写し取り」の印を立てる。同じ波形を鳴らす要素が
// 2 つあるとき（重ねの音色ではよくある）、両方が同じ写し取りを掴むと
// 片方の音量が丸ごと違ってしまう
inline const voice_cal *match_cal(const std::vector<voice_cal> &cals, u32 want, u32 *used = nullptr)
{
	for (size_t i = 0; i < cals.size(); i++) {
		if (used && (*used & (u32(1) << i)))
			continue;
		const voice_cal &c = cals[i];
		if (c.has(0x16) && c.has(0x17) && c.wave_addr() == want) {
			if (used)
				*used |= u32(1) << i;
			return &c;
		}
	}
	return nullptr;
}

// **フィルタの包絡線の初めの値**（実機の `0x1288E4`-`0x12895C`）。
// 立ち上がりが最速（byte50 が 63）の音色は**いきなり段 0 の行き先から
// 始まる**。そうでなければ byte54（既定は 64 ＝ ずれ 0）から始めて、
// byte50 の速さで段 0 の行き先へ登る。
// GrandPno（byte50=63）は 0x400、Flute（byte50=62）は 0 で実機と一致した
inline int fenv_start_level(const u8 *elem)
{
	return elem[50] >= 63 ? elem[55] : elem[54];
}

inline int fenv_init(const u8 *rom, const u8 *elem, int vel)
{
	return fenv_target(rom, elem, fenv_start_level(elem), vel);
}

// **鍵を押した瞬間の `0x00`**（実機の `0x12AC98`）。
//   表 0x1E5B58[byte37] ＋ 鍵の追従 を 0-0xFFF に収め、
//   そこへ包絡線の初めの値（>>2）を足して下 11bit を取る
// 包絡線の今の値（facc）を渡すと、そのときの `0x00` を返す。
// 実機は 10ms ごとにこれを書き直している
// `SMU2000_CUT_EXACT=1` で、鍵を押した瞬間の `0x00` を実機と同じ式にする。
// 既定は切（上の但し書きを見よ）
inline bool cut_exact()
{
	static const bool on = std::getenv("SMU2000_CUT_EXACT") != nullptr;
	return on;
}

inline u16 cutoff_of(const u8 *rom, const u8 *elem, int note, int vel, int facc)
{
	int cut = int(rd16(rom, CUTOFF_TAB + u32(elem[37]) * 2))
	        + cutoff_key_curve(rom, elem, note);
	cut = cut < 0 ? 0 : (cut > 0xfff ? 0xfff : cut);
	cut += facc >> 2;
	// 実機（0x127E08）は足したあとも 0xFFF で頭打ちにして、下 11bit を取る。
	// そのうえで `0x12E79C` が「**共振が 4 未満なら 0x7C0 で頭打ち**」を掛ける
	// （EPiano1 は強さ 100 で共振 0 → 0x7C0、強さ 127 で共振 4 → 0x7FF）
	if (cut > 0xfff) cut = 0xfff;
	cut &= 0x7ff;
	if (cut < 0) cut = 0;
	if (reso_level(elem, vel) < 4 && cut > CUTOFF_MAX)
		cut = CUTOFF_MAX;
	return u16(0x1000 | u16(cut));
}

inline u16 cutoff_keyon(const u8 *rom, const u8 *elem, int note, int vel)
{
	return cutoff_of(rom, elem, note, vel, fenv_init(rom, elem, vel));
}

// ---- **音色そのものが持つパン**（レジスタ `0x32`）。実機の `0x12AF40` と `0x12B794`
//
//   位置 = clamp(CC10 + 表 0x1E68DC[byte69] - 64, 0, 127)
//          （byte69 が 15 のときだけ鍵で 0x1E68EB を引く）
//   左 = 表 0x1E6B90[パート[14]] + 表 0x1E6C11[位置]
//   右 = 表 0x1E6B90[0x80-パート[14]] + 表 0x1E6C11[0x80-位置]
//   レジスタ = (左 << 8) | 右   （どちらも 255 で頭打ち）
//
// Warm Pad は 2 つの要素が byte69=2 と 12 で、表を引くと 13 と 115。
// 実機は片方に `083c`、もう片方に `3c08` を書いていて、式と一致する
constexpr u32 PAN_SEL_TAB   = 0x1E68DC;   // byte69 → パンの位置（16 個）
constexpr u32 PAN_SEL_KEY   = 0x1E68EB;   // byte69 が 15 のとき、鍵で引く
constexpr u32 PAN_BASE_TAB  = 0x1E6B90;   // パートのパン → 下駄（中央で 8 ＝ -3dB）
constexpr u32 PAN_CURVE_TAB = 0x1E6C11;   // パンの位置 → 減衰（0-128）

inline int elem_pan(const u8 *rom, const u8 *elem, int note)
{
	const int i = int(elem[69]);
	return i == 15 ? int(rom[PAN_SEL_KEY + u32(note & 0x7f)])
	               : int(rom[PAN_SEL_TAB + u32(i & 0xf)]);
}

// パンの位置（0-127）
inline int voice_pan_pos(const u8 *rom, const u8 *elem, int note, int cc10 = 64)
{
	const int p = cc10 + elem_pan(rom, elem, note) - 64;
	return p < 0 ? 0 : (p > 127 ? 127 : p);
}

// **送りはパンで目減りする**（実機の `0x12C3F8`）。真ん中で 16 を足し、
// 左右に振るほど減る（表 0x1F2198）。Warm Pad は位置 13 で 4 なので
// 既定の `2b` から 12 減って `1f`。実機と一致した
constexpr u32 PAN_SEND_TAB = 0x1F2198;

inline int pan_send_adj(const u8 *rom, int pan_pos)
{
	return int(rom[PAN_SEND_TAB + u32(pan_pos & 0x7f)])
	     - int(rom[PAN_SEND_TAB + 64]);
}

inline u16 voice_pan_reg(const u8 *rom, const u8 *elem, int note,
                         int cc10 = 64, int part_pan = 64)
{
	const int p = voice_pan_pos(rom, elem, note, cc10);
	const int q = part_pan & 0x7f;
	int l = int(rom[PAN_BASE_TAB + u32(q)]) + int(rom[PAN_CURVE_TAB + u32(p)]);
	int r = int(rom[PAN_BASE_TAB + u32(0x80 - q)])
	      + int(rom[PAN_CURVE_TAB + u32(0x80 - p)]);
	if (l > 255) l = 255;
	if (r > 255) r = 255;
	return u16((l << 8) | r);
}

// ---- **ドラムの 1 打**（doc/native-engine.md の 6.86・6.87）
//
// ドラムの記録は 42 バイトで、旋律の要素（84 バイト）とは別の並び。
// 波形の記録（16 バイト）が +26 にそのまま埋まっている。
//   +1  音程の微調（セント。64 が中央）   +10 減衰 2 の行き先
//   +11 切る高さ（表の索引）              +12 共振（>> 2）
//   +13 立ち上がりの速さ                  +14 減衰 1 の速さ
//   +15 減衰 2 の速さ                      +20 フィルタの第 2 係数
//   +26 音程の基準（半音。引く）           +27/+28 GM/XG の半音のずらし
//   +30..+41 ループ前・ループ長・形式と番地
constexpr u32 DRUM_PITCH_TAB = 0x1E9298;   // セント → 音程の目盛り（1200 = 1024）
constexpr u32 DRUM_KIT_TABLE = 0x292250;   // キット → 鍵ごとのずれの表（4 バイト）
constexpr u32 DRUM_RECORDS   = 0x283dd0;   // ずれの元になる番地
constexpr u32 PART_KIT       = 0x110;      // パートの塊の中の、キットの番号

// キットの番号と鍵から、ドラムの 1 打の記録（42 バイト）。無ければ nullptr。
// 実機の `0x134DB8`
inline const u8 *drum_record(const u8 *rom, int kit, int note)
{
	if (!rom || (kit & 0x80))
		return nullptr;                 // bit7 が立つキットは別の道（未対応）
	const u32 base = rd32(rom, DRUM_KIT_TABLE + u32(kit & 0x7f) * 4);
	if (base < 0x200000 || base > 0x2ffff0)
		return nullptr;
	const u16 off = rd16(rom, base + u32(note & 0x7f) * 2);
	return off == 0xffff ? nullptr : rom + DRUM_RECORDS + off;
}

// `SMU2000_DRUM_EXACT=1` で、ドラムを写し取りではなく式で組む
inline bool drum_exact()
{
	static const bool on = std::getenv("SMU2000_DRUM_EXACT") != nullptr;
	return on;
}

inline int drum_cents(const u8 *rec, int coarse = 64, int fine = 64, bool xg = true)
{
	const int semi = coarse + (xg ? s8(rec[28]) : int(rec[27])) - int(rec[26]);
	return semi * 100 + (int(rec[1]) - 64) + (fine - 64);
}

inline u16 drum_pitch_reg(const u8 *rom, const u8 *rec, int cents)
{
	int c = cents < 0 ? -cents : cents;
	if (c > 9600)
		c = 9600;
	const u16 t = rd16(rom, DRUM_PITCH_TAB + u32(c) * 2);
	u16 v = cents < 0 ? u16((-int(t)) & 0x3fff) : u16(t & 0x3fff);
	const u32 addr = u32(rec[38]) << 24 | u32(rec[39]) << 16 | u32(rec[40]) << 8 | rec[41];
	if (((addr >> 30) & 3) == 3)
		v = u16(v | 0x4000);
	return v;
}

// ドラムの 1 打のレジスタを、記録だけから組む（写し取りを使わない）。
// 4 打 × 全鍵 58 個で実機と一致したものだけを入れてある
inline slot_regs drum_note(const u8 *rom, const u8 *rec, int att,
                           const defaults &d = defaults(),
                           int coarse = 64, int fine = 64)
{
	slot_regs r;
	if (!rom || !rec)
		return r;
	r.set(0x00, u16(0x1000 | (rd16(rom, CUTOFF_TAB + u32(rec[11]) * 2) & 0x7ff)));
	r.set(0x01, 0xffff);
	r.set(0x02, u16(0x8000 | u16(std::min(0x7ff, int(rec[20]) * 16))));
	r.set(0x03, d.post);
	r.set(0x04, u16((rec[12] >> 2) << 11));
	r.set(0x05, d.lfo_amp);
	// **速さの表は 2 倍しない**（旋律は rate_scale で 2 倍する）
	r.set(0x06, u16(u16(rom[ATTACK_TAB + u32(rec[13] & 0x7f)]) << 8
	                | (rec[13] >= 0x7f ? 0x00 : 0x7e)));
	r.set(0x07, u16(u16(rom[DECAY_TAB + u32(rec[14] & 0x7f)]) << 8 | 0x04));
	r.set(0x08, u16(u16(rom[DECAY_TAB + u32(rec[15] & 0x7f)]) << 8
	                | u16(((0x7f - int(rec[10])) * 2) & 0xff)));
	r.set(0x09, u16(att & 0xff));
	r.set(0x0a, 0x7000);
	r.set(0x0b, 0x0000);
	r.set(0x10, 0x0000);
	r.set(0x11, drum_pitch_reg(rom, rec, drum_cents(rec, coarse, fine)));
	const wave_info w = read_wave(rec + 26);
	r.set(0x12, u16(w.pre_loop >> 16));
	r.set(0x13, u16(w.pre_loop));
	r.set(0x14, u16(w.loop_len >> 16));
	r.set(0x15, u16(w.loop_len));
	r.set(0x16, u16(w.format_addr >> 16));
	r.set(0x17, u16(w.format_addr));
	for (int i = 0; i < 6; i++)
		r.set(0x20 + i * 2, d.iir[i]);
	for (int i = 0; i < 6; i++)
		r.set(0x32 + i, d.mix[i]);
	return r;
}

// 1 音ぶんのレジスタを作る。att は 0x09 に入れる減衰（0-255。小さいほど大きい音）
inline slot_regs build_note(const u8 *rom, const u8 *elem, int note, int att,
                            const voice_cal *cal = nullptr,
                            const defaults &d = defaults(), int cents_extra = 0,
                            int vel = 100)
{
	slot_regs r;
	const u8 *we = wave_entry(rom, wave_set(elem), wave_note(elem, note));
	if (!we)
		return r;
	const wave_info w = read_wave(we);

	// --- フィルタ。切る高さは ROM の表（0x1E5B58）を byte37 で引く。
	// 実機はここに鍵と強さの倍率を掛ける（`0x127FA4`）が、その係数がまだ分からない。
	// 倍率 1 として表を引くだけでも、開き切りよりはずっと実機に近い
	// 実機（0x12AC98）は表を引いた値に**鍵の追従**（0x12C1E4）を足して
	// 0-0xFFF に収める。鍵の追従を入れていなかったので、Flute のように
	// 曲線を持つ音色で鍵を押した瞬間の値がずれていた（6.71）
	// **鍵を押した瞬間の値そのもの**は `cutoff_keyon` が出せる（14 音色 ×
	// 鍵 5 通り × 強さ 3 通りで実機と完全に一致）。けれども今はまだ既定で
	// 使えない: 写し取りの無いスロットはフィルタの包絡線が動かないので、
	// 包絡線の初めのぶんだけ開いたままになり、試し曲が 0.36dB 明るくなる。
	// `SMU2000_CUT_EXACT=1` で試せる（doc/native-engine.md の 6.71）
	r.set(0x00, cut_exact()
	            ? cutoff_keyon(rom, elem, note, vel)
	            : u16(0x1000 | (rd16(rom, CUTOFF_TAB + u32(elem[37]) * 2) & 0x7ff)));
	// **鍵を押した瞬間の 0x01 は 0xFFFF**（実機は毎回そう書いて、最初の
	// 包絡線の目で本当の値に置き換える）。14 音色を実機と突き合わせて
	// 確かめた（doc/native-engine.md の 6.67）
	r.set(0x01, 0xffff);
	// フィルタの第 2 係数。実機（0x12AFCE）は **byte82 を 16 倍**して
	// 0x800 の下駄を履かせ、0x800-0xFFF に収めてから下 11bit を取る。
	// つまり素直に byte82 * 16 で、0x7FF で頭打ち（DistGtr の 0x180、
	// Kitayama の 0x570 が実機と一致した）
	r.set(0x02, u16(0x8000 | u16(std::min(0x7ff, int(elem[82]) * 16))));
	r.set(0x03, d.post);
	// フィルタの第 2 パラメータ（共振）。byte35 から強さぶんを引いて（byte81）、
	// 1 ビット落として 5bit にする（0x12806A）。18 音色 × 強さ 3 通りで一致
	r.set(0x04, u16(reso_level(elem, vel) << 11));
	// LFO の深さ（音量側）。実機（0x129B34）は byte16 を 2 倍して下位に置くが、
	// **遅れ（byte12）と byte13 がどちらも 0 のときだけ**使う（0x127D18）。
	// Vibes（byte12=0・byte13=0・byte16=2）は 4、Koto（byte12=48）は 0
	r.set(0x05, u16((d.lfo_amp & 0xff00)
	                | u16((elem[12] || elem[13]) ? 0 : ((elem[16] * 2) & 0x7f))));
	// LFO の型と刻み。上位は byte11 に**byte9 が 0 でなければ** 0x40 を足したもの
	// （Rain は byte9=0 で `2d`）。下位は**音程の深さ = byte14 × 3**
	// （PanFlute の byte14=1 で 3、ChiffLead・TnklBell・Helicopter の 2 で 6）
	// 深さは `0x05` と同じく、**遅れ（byte12）と byte13 がどちらも 0 のとき**だけ
	r.set(0x0a, u16(((((elem[9] ? 0x40 : 0) | (elem[11] & 0x3f)) << 8))
	                | u16((elem[12] || elem[13]) ? 0 : ((elem[14] * 3) & 0x7f))));
	// 音程の包絡線。速さが 127（即到達）のときだけ初めの高さは byte31 を使う
	const int prate = peg_rate_reg(rom, elem, note, vel);
	r.set(0x0b, u16(prate << 8));
	r.set(0x10, peg_reg(rom, peg_cents(elem, prate == 127 ? elem[31] : elem[30], vel), elem));

	// --- 包絡線（doc/native-engine.md の 6.3・6.4）
	//
	// 減衰の速さは鍵で動く。firmware の 0x12ADD0 と 0x1272F4 がやっているのは
	//   補正 = ((鍵 - 折れ点) * ((depth - 64) * 16)) >> 8     （負は 0 の側へ）
	//   目盛り = clamp(記録の値 + 補正, 1, 63) * 2
	// で、その目盛りで ROM の表を引いたものがレジスタの上位バイトになる。
	// 深さは byte70、折れ点の鍵は byte71（鍵 36・60・84 で確かめた）。
	const int corr = rate_key_corr(elem, note);
	const u8 atk = rom[ATTACK_TAB + std::min(0x7f, int(elem[73]) * 2)];
	// 写し取りがあれば、そのときのずれを表の目盛りに足す（上の dec_adj を見よ）
	const int a1 = cal && cal->have ? cal->dec_adj[0] : 0;
	const int a2 = cal && cal->have ? cal->dec_adj[1] : 0;
	const u8 dc1 = rom[DECAY_TAB  + clamp_idx(rate_scale(elem[74], corr) + a1)];
	const u8 dc2 = rom[DECAY_TAB  + clamp_idx(rate_scale2(elem[75], corr) + a2)];
	// はじめの音量。アタックが最速（63）のときだけ 0 で、あとは 0x7e
	r.set(0x06, u16(atk << 8 | (elem[73] >= 0x3f ? 0x00 : 0x7e)));
	r.set(0x07, u16(dc1 << 8 | (((0x7f - elem[77]) * 2) & 0xff)));
	r.set(0x08, u16(dc2 << 8 | (((0x7f - elem[78]) * 2) & 0xff)));
	r.set(0x09, u16(att & 0xff));

	// --- 音程と波形（6.2）
	// 要素の byte17 は**半音単位の粗調**、byte18 は**セント単位の離調**（どちらも 64 が中央）。
	// 離調は重ねの音色で 2 つの層をずらすのに使う。入れないと層がぴったり重なって
	// 打ち消し合わず、3dB ほど大きくなる（doc/native-engine.md の 6.18）
	r.set(0x11, pitch_reg(w, note, key_follow(elem), cents_extra + elem_tune(elem)));
	// **鳴らし始める位置をずらす**（実機の `0x12A9C8`）。要素の byte79 が
	// 128 サンプル単位、byte80 が 1 サンプル単位の下駄で、ループ前の長さから
	// 引く。Oboe(7→896)・Clarinet(2→256)・Bagpipe(8→1024) で実機と一致した。
	// 入れていなかったので、その 3 音色は波形がまるで合っていなかった
	{
		const u32 skip = u32(elem[79]) * 128 + elem[80];
		const u32 pre = w.pre_loop > skip ? w.pre_loop - skip : 0;
		r.set(0x12, u16(pre >> 16));
		r.set(0x13, u16(pre));
	}
	r.set(0x14, u16(w.loop_len >> 16));
	r.set(0x15, u16(w.loop_len));
	r.set(0x16, u16(w.format_addr >> 16));
	r.set(0x17, u16(w.format_addr));

	// --- 声の EQ とミキサ
	for (int i = 0; i < 6; i++)
		r.set(0x20 + i * 2, d.iir[i]);
	for (int i = 0; i < 6; i++)
		r.set(0x32 + i, d.mix[i]);
	// **音色そのものが持つパン**（byte69）。写し取りがあれば下で上書きされる
	r.set(0x32, voice_pan_reg(rom, elem, note));
	// 送りはそのパンのぶん目減りする
	{
		const int adj = pan_send_adj(rom, voice_pan_pos(rom, elem, note));
		for (int i = 0; i < 2; i++) {
			// **切ってある送り（0xff）はそのまま**。実機も頭打ちなので、
			// ここでパンのぶん引くと切ったはずの送りが開いてしまう
			if ((d.mix[1 + i] & 0xff) >= 0xff)
				continue;
			int v = int(d.mix[1 + i] & 0xff) + adj;
			v = v < 0 ? 0 : (v > 255 ? 255 : v);
			r.set(0x33 + i, u16((d.mix[1 + i] & 0xff00) | u16(v)));
		}
	}

	// --- 写し取った値で上書き。式が分かっていない所だけ
	//
	// **0x06（立ち上がり）もここに入れる。** 入れていなかったので、
	// パート側の EG の設定（CC73 など）が native の音に一切効いていなかった。
	// CC73 を全パートに送る曲では全部の音の立ち上がりが狂う（実測で
	// firmware 417e に対しこちらは 387e ＝ ずっと遅い）。
	// 立ち上がりは鍵でも強さでも変わらないと測ってあるので（nativeplay
	// --egwatch を鍵 36-96・強さ 1-127 で確認）、写し取った値をそのまま使える。
	// 0x07・0x08（減衰）は鍵で変わるので、ここには入れられない（宿題）
	if (cal && cal->have) {
		// 0x20-0x2b は**偶数番だけ**でよい（奇数番と 0x30・0x31 は実機の
		// firmware も一度も書かない。記録を追って確かめた）
		// **0x0b・0x10 はもう写し取らない**。音程の包絡線を式で出すようになった
		// （写し取りは包絡線が終わったあとの値を拾うので、入れると出だしの
		//  しゃくりが丸ごと消えていた。doc/native-engine.md の 6.68）
		static const int COPY[] = { 0x00, 0x01, 0x06, 0x0a,
		                            0x20, 0x22, 0x24, 0x26, 0x28, 0x2a,
		                            0x32, 0x33, 0x34, 0x35, 0x36, 0x37 };
		for (int i : COPY)
			if (cal->has(i))
				r.set(i, cal->reg[i]);
	}
	return r;
}

} // namespace nv
} // namespace xg

#endif // S_MU2000_XG_NATIVE_VOICE_H
