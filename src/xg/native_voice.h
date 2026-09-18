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

#include <cmath>
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

// 音色ごとの下駄。firmware は「音色の音量 → 表」と、鍵ごとの足し込みで作る。
// 式そのものはまだ解けていないので、**1 回だけ実機に鳴らしてもらって校正する**（下）。
// 校正しないときの当て値（実測の中央値。5〜19 の幅がある）
constexpr int VOICE_ATT_TYPICAL = 12;

constexpr u32 LEVEL_CURVE = 0x23CED0;   // 鍵による音量の曲線（128 バイトの行が並ぶ）

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

// 減衰 → 音量の目盛り（表を逆に引く）。同じ減衰になる目盛りが複数あるので真ん中を返す
inline int level_from_att(const u8 *rom, int att)
{
	int lo = -1, hi = -1;
	for (int i = 0; i < 128; i++)
		if (rom[LEVEL_TAB + 0x80 + i] == att) {
			if (lo < 0) lo = i;
			hi = i;
		}
	return lo < 0 ? 64 : (lo + hi) / 2;
}

// **校正**: 1 回だけ実機（firmware）に鳴らしてもらった減衰から、その音色の
// 「素の音量」を出す。これがあれば、ほかの鍵・強さの減衰は式で出せる
inline int wave_level(const u8 *rom, const u8 *elem, int note)
{
	const u8 *we = wave_entry(rom, wave_set(elem), wave_note(elem, note));
	return we ? int(we[0]) : 0;
}

// 鍵の曲線が音量の目盛りに効く倍率。実測（GrandPno の鍵 12-75）では 1 倍
constexpr int LEVEL_CURVE_MUL = 1;

inline int calibrate_level(const u8 *rom, const u8 *elem, int att_ref, int note_ref, int vel_ref)
{
	const int rest = att_ref / 2 - velocity_att(rom, vel_ref) - wave_level(rom, elem, note_ref);
	return level_from_att(rom, rest) - LEVEL_CURVE_MUL * level_key_curve(rom, elem, note_ref);
}

// 校正した素の音量から、その鍵・強さの減衰（0x09 に入れる値）
inline int volume_att(const u8 *rom, const u8 *elem, int base_level, int note, int vel)
{
	int l = base_level + LEVEL_CURVE_MUL * level_key_curve(rom, elem, note);
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
	u32 at;            // 鳴らし始めてからのサンプル数
	u8  reg;
	u16 v;
};

struct voice_cal {
	bool have = false;
	int  base_level = 64;      // 校正した素の音量
	int  cal_vel = 100;        // 写し取ったときの強さ（強さを変えるときの基準）
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

// 1 音ぶんのレジスタを作る。att は 0x09 に入れる減衰（0-255。小さいほど大きい音）
inline slot_regs build_note(const u8 *rom, const u8 *elem, int note, int att,
                            const voice_cal *cal = nullptr,
                            const defaults &d = defaults(), int cents_extra = 0)
{
	slot_regs r;
	const u8 *we = wave_entry(rom, wave_set(elem), wave_note(elem, note));
	if (!we)
		return r;
	const wave_info w = read_wave(we);

	// --- フィルタ。切る高さは ROM の表（0x1E5B58）を byte37 で引く。
	// 実機はここに鍵と強さの倍率を掛ける（`0x127FA4`）が、その係数がまだ分からない。
	// 倍率 1 として表を引くだけでも、開き切りよりはずっと実機に近い
	r.set(0x00, u16(0x1000 | (rd16(rom, CUTOFF_TAB + u32(elem[37]) * 2) & 0x7ff)));
	r.set(0x01, d.bypass);
	r.set(0x02, u16(0x8000 | elem[82]));       // 402 組の 97%
	r.set(0x03, d.post);
	// フィルタの第 2 パラメータ（共振）。firmware は byte35 を 1 ビット落として
	// 5bit にし、レジスタの上 5bit に置く（0x1280FC）。402 組の 90% が一致
	r.set(0x04, u16((((elem[35] >> 1) & 31) << 11)));
	r.set(0x05, d.lfo_amp);
	// LFO の型と刻み。上位は 0x40 | byte11（402 組で例外なし）、下位（音程の深さ）は 0
	r.set(0x0a, u16((0x40 | (elem[11] & 0x3f)) << 8));
	r.set(0x0b, d.r0b);
	r.set(0x10, u16(elem[83] << 8));           // 402 組の 96%

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
	const u8 dc2 = rom[DECAY_TAB  + clamp_idx(rate_scale(elem[75], corr) + a2)];
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
	r.set(0x12, u16(w.pre_loop >> 16));
	r.set(0x13, u16(w.pre_loop));
	r.set(0x14, u16(w.loop_len >> 16));
	r.set(0x15, u16(w.loop_len));
	r.set(0x16, u16(w.format_addr >> 16));
	r.set(0x17, u16(w.format_addr));

	// --- 声の EQ とミキサ
	for (int i = 0; i < 6; i++)
		r.set(0x20 + i * 2, d.iir[i]);
	for (int i = 0; i < 6; i++)
		r.set(0x32 + i, d.mix[i]);

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
		static const int COPY[] = { 0x00, 0x01, 0x02, 0x04, 0x05, 0x06, 0x0a, 0x0b, 0x10,
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
