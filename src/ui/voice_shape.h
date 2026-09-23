// license:BSD-3-Clause
//
// 音色の中身（ROM の要素の記録）とパートの値から、**実際の動き**を組んで絵に渡す。
// パートの音色の窓の絵（ピッチ EG など）が使う。
//
// 式は native の口（xg/native_voice.h）と同じもので、どれも実機の firmware が書くレジスタと
// 突き合わせて確かめてある。時間はチップの包絡線の刻み（swp30 の level_step）を平均して出す。
// 描く鍵と強さは決め打ち（鍵 60・強さ 100）で、鍵や強さで動く分は入らない。
//
// Windows にも画面にも依存しない。

#ifndef S_MU2000_UI_VOICE_SHAPE_H
#define S_MU2000_UI_VOICE_SHAPE_H

#pragma once

#include "mame/sound/swp30.h"
#include "xg/native_voice.h"
#include "xg/ram.h"

#include <algorithm>
#include <array>
#include <cmath>
#include <complex>
#include <cstring>
#include <vector>

namespace ui {
namespace shape {

constexpr int NOTE = 60;
constexpr int VEL = 100;
constexpr double RATE = 44100.0;

// 包絡線の 1 サンプルあたりの平均の歩幅（チップの刻みを 65536 サンプルぶん数える）
inline double avg_step(int speed)
{
	static std::array<double, 160> cache = [] {
		std::array<double, 160> c{};
		for (int i = 0; i < 160; i++) {
			const int sp = i - 32;
			u64 sum = 0;
			for (u32 sc = 0; sc < 65536; sc++)
				sum += swp30_device::envelope_step(sp, sc);
			c[size_t(i)] = double(sum) / 65536.0;
		}
		return c;
	}();
	return cache[size_t(std::clamp(speed + 32, 0, 159))];
}

// 音程の包絡線の 1 点（時刻 ms、ずれ セント）
struct pt { float ms, cents; };

// 要素 1 つぶんの音程の動き。鍵を離した時刻（ms）と、この鍵・強さで鳴る要素か
struct peg_line {
	std::vector<pt> pts;
	float keyoff_ms = 0;
	bool active = true;
	bool moves = false;       // 素のままでも動く要素か（素の高さが全部 64 でない）
};

// レジスタ 0x10 の値（14bit 符号つき、1 オクターブ 1024）→ セント
inline float units_cents(u16 reg)
{
	int v = int(reg & 0x3fff);
	if (v & 0x2000)
		v -= 0x4000;
	return float(v) * 1200.0f / 1024.0f;
}

// 音色（rec）とパートの塊（part_ram。XG のパート番号の並びに直した写し）から、要素ごとの
// 音程の動きを組む。hold_ms は「行き着いてから離すまで」の間
inline std::vector<peg_line> peg_lines(const u8 *rom, u32 rec, const u8 *part, float hold_ms)
{
	namespace nv = xg::nv;
	std::vector<peg_line> out;
	if (!rom || !rec)
		return out;
	const int n = nv::element_count(rom, rec);
	std::vector<float> at_end(size_t(n), 0.0f), level(size_t(n), 0.0f);
	for (int e = 0; e < n; e++) {
		const u8 *el = nv::element(rom, rec, e);
		peg_line line;
		line.active = nv::element_active(el, NOTE, VEL);
		line.moves = el[30] != 64 || el[31] != 64 || el[32] != 64 || el[33] != 64 || el[34] != 64;
		// 鍵を押した瞬間（build_note と同じ）
		const int part_init = part[0x62], part_atk = part[0x63];
		const int cc_atk = part[0x1a], cc_dec = part[0x1b], cc_rel = part[0x1c];
		const int praw = part_atk == 64 ? int(el[26]) : nv::eg_rate_cc(rom, int(el[26]), part_atk);
		const int prate = nv::peg_rate_reg_raw(rom, el, praw, NOTE, VEL, 64, cc_atk);
		const u16 start = nv::peg_reg(rom, prate == 127 ? nv::peg_cents(el, el[31], VEL)
		                                                : nv::peg_cents(el, el[30], VEL) + nv::part_peg_cents(part_init),
		                              el);
		double ms = 0;
		float cur = units_cents(start);
		line.pts.push_back({ 0, cur });
		// 1 段ぶん。速さのレジスタと行き先（0x10 の値）から、着くまでの時間
		auto go = [&](int rate_reg, u16 target_reg) {
			const float tgt = units_cents(target_reg);
			const double step = avg_step(rate_reg - 16) * 1200.0 / 1024.0;     // セント / サンプル
			const double dist = std::fabs(double(tgt - cur));
			if (dist > 0 && step > 0) {
				ms += dist / step / RATE * 1000.0;
				line.pts.push_back({ float(ms), tgt });
			}
			cur = tgt;
		};
		go(prate, nv::peg_reg(rom, nv::peg_cents(el, el[31], VEL), el));
		// 段 1・2（行き先が前の段と同じなら飛ばす。peg_advance と同じ）
		if (nv::peg_level_of(el, 0) != nv::peg_level_of(el, 1))
			go(nv::peg_rate_reg_stage(rom, el, 1, NOTE, VEL, 64, 64),
			   nv::peg_reg(rom, nv::peg_cents(el, el[32], VEL), el));
		if (nv::peg_level_of(el, 1) != nv::peg_level_of(el, 2))
			go(nv::peg_rate_reg_stage(rom, el, 2, NOTE, VEL, 64, cc_dec),
			   nv::peg_reg(rom, nv::peg_cents(el, el[33], VEL), el));
		at_end[size_t(e)] = float(ms);
		level[size_t(e)] = cur;
		out.push_back(std::move(line));
	}
	// 鍵を離すのは全部の要素で同時。いちばん遅く行き着いた要素から hold_ms 後
	float keyoff = 0;
	for (float t : at_end)
		keyoff = std::max(keyoff, t);
	keyoff += hold_ms;
	for (int e = 0; e < n; e++) {
		const u8 *el = nv::element(rom, rec, e);
		peg_line &line = out[size_t(e)];
		float cur = level[size_t(e)];
		double ms = keyoff;
		line.keyoff_ms = keyoff;
		line.pts.push_back({ keyoff, cur });
		const int cc_rel = part[0x1c];
		auto go = [&](int rate_reg, u16 target_reg) {
			const float tgt = units_cents(target_reg);
			const double step = avg_step(rate_reg - 16) * 1200.0 / 1024.0;
			const double dist = std::fabs(double(tgt - cur));
			if (dist > 0 && step > 0) {
				ms += dist / step / RATE * 1000.0;
				line.pts.push_back({ float(ms), tgt });
			}
			cur = tgt;
		};
		// 離し（peg_release と同じ）
		int raw = int(el[29]);
		const int d = int(part[0x65]) - 64;
		if (d > 0) {
			const int tb = int(rom[nv::PEG_REL_TAB + u32(d)]);
			if (raw > tb)
				raw = tb;
		} else {
			raw -= d >> 1;
			if (raw > 63)
				raw = 63;
		}
		const int lv = std::clamp(int(el[34]) + (int(part[0x64]) - 64), 0, 127);
		go(nv::peg_rate_reg_raw(rom, el, raw, NOTE, VEL, 64, cc_rel),
		   nv::peg_reg(rom, nv::peg_cents(el, lv, VEL), el));
	}
	return out;
}

// ---- 音量の包絡線
//
// チップ（swp30 の envelope_block::step）と同じ動きを、128 サンプルずつまとめて進める。
// 減衰量は 1024 で 6.02 dB（4.10 の浮動小数）。はじめの減衰量は立ち上がりのレジスタの下位、
// 立ち上がりの速さは今の減衰量でも速くなる（`(減衰量 >> 9) << 2` を足す）。減衰 1・2 は
// 下位の行き先へ、離しは下へ。レジスタは native の口の build_note・release_reg そのまま
struct amp_line {
	std::vector<pt> pts;          // ms と dB（cents の欄に dB を入れる）
	float keyoff_ms = 0;
	float attack_ms = 0, decay1_ms = 0, sustain_db = 0;   // つまむ点に使う
	bool active = true;
};

inline float att_db(double level) { return float(-level * 6.0206 / 1024.0); }

// 1 要素ぶんを進める。keyoff_ms が負なら離さない（減衰 2 の終わりまで、上限 limit_ms）
inline amp_line amp_run(const u8 *rom, const u8 *el, const u8 *part, float keyoff_ms, float limit_ms)
{
	namespace nv = xg::nv;
	amp_line line;
	line.active = nv::element_active(el, NOTE, VEL);
	const int cc_atk = part[0x1a], cc_dec = part[0x1b], cc_rel = part[0x1c];
	const nv::slot_regs sr = nv::build_note(rom, el, NOTE, 0, nullptr, nv::defaults(), 0, VEL, cc_atk, cc_dec,
	                                        64, 64, -1, NOTE, false, part[0x62], part[0x63]);
	const u16 atk = sr.v[0x06], dc1 = sr.v[0x07], dc2 = sr.v[0x08];
	const u16 rel = nv::release_reg(rom, el, NOTE, 0, cc_rel);
	constexpr int CHUNK = 128;
	const double chunk_ms = CHUNK / RATE * 1000.0;
	double level = (atk & 0xff) ? double((atk & 0xff) << 6) : 0.0;
	int mode = (atk & 0xff) ? 0 : 1;            // 0 立ち上がり、1 減衰 1、2 減衰 2、3 離し
	double ms = 0;
	line.pts.push_back({ 0, att_db(level) });
	// 立ち上がりは、減衰量 512 ごとの帯で速さが決まる（`(減衰量 >> 9) << 2`）。はじめの方は
	// 1 サンプルで 127 も進むので、128 サンプルずつでは一跳びで着いてしまう。帯ごとに
	// 平均の歩幅で割って、着くまでの時間を出す
	if (mode == 0) {
		double samples = 0;
		while (level > 0) {
			const int k = int(level) >> 9;
			const double lo = k ? double(k * 512 - 1) : 0.0;     // この帯を抜けたところ
			samples += (level - lo) / avg_step((atk >> 8) + (k << 2));
			level = lo;
			ms = samples / RATE * 1000.0;
			if (ms >= limit_ms)
				break;
			line.pts.push_back({ float(ms), att_db(level) });
		}
		if (level <= 0) {
			level = 0;
			mode = 1;
			line.attack_ms = float(ms);
		}
	}
	int last_mode = mode;
	int since = 0;
	while (ms < limit_ms) {
		if (keyoff_ms >= 0 && mode != 3 && ms >= keyoff_ms) {
			line.pts.push_back({ float(ms), att_db(level) });
			mode = 3;
		}
		if (mode == 0) {
			level -= avg_step((atk >> 8) + ((int(level) >> 9) << 2)) * CHUNK;
			if (level <= 0) {
				level = 0;
				mode = 1;
				line.attack_ms = float(ms + chunk_ms);
			}
		} else if (mode == 1 || mode == 2) {
			const u16 reg = mode == 1 ? dc1 : dc2;
			const double limit = double((reg & 0xff) << 6);
			const double step = avg_step(reg >> 8) * CHUNK;
			if (level < limit)
				level = std::min(limit, level + step);
			else if (level > limit)
				level = std::max(limit, level - step);
			if (level == limit) {
				if (mode == 1)
					line.decay1_ms = float(ms + chunk_ms);
				if (mode == 2 && keyoff_ms < 0) {
					ms += chunk_ms;
					line.pts.push_back({ float(ms), att_db(level) });
					break;                          // 減衰 2 の終わり（伸ばしている音量）
				}
				if (mode == 1)
					mode = 2;
			}
		} else {
			level = std::min(double(0x3fff), level + avg_step(((rel >> 8) ^ 0x80) & 0xff) * CHUNK);
		}
		ms += chunk_ms;
		// 点は形が変わるところと、途中は間引いて
		if (mode != last_mode || ++since >= 8) {
			line.pts.push_back({ float(ms), att_db(level) });
			last_mode = mode;
			since = 0;
		}
		if (mode == 3 && att_db(level) < -72.0f)
			break;
	}
	line.sustain_db = att_db(double((dc2 & 0xff) << 6));
	line.keyoff_ms = keyoff_ms;
	return line;
}

// 要素ごとの音量の動き。鍵を離すのは、いちばん遅く減衰 2 を終えた要素から hold_ms 後
// （減衰 2 がとても長い音色――ピアノなど――は 2 秒で打ち切って離す）
inline std::vector<amp_line> amp_lines(const u8 *rom, u32 rec, const u8 *part, float hold_ms)
{
	namespace nv = xg::nv;
	std::vector<amp_line> out;
	if (!rom || !rec)
		return out;
	const int n = nv::element_count(rom, rec);
	float keyoff = 0;
	for (int e = 0; e < n; e++) {
		const amp_line a = amp_run(rom, nv::element(rom, rec, e), part, -1, 2000.0f);
		keyoff = std::max(keyoff, a.pts.back().ms);
	}
	keyoff += hold_ms;
	for (int e = 0; e < n; e++)
		out.push_back(amp_run(rom, nv::element(rom, rec, e), part, keyoff, keyoff + 12000.0f));
	return out;
}

// ---- ビブラート（音程の LFO）
//
// 鍵を押したときのレジスタ 0x0a（型・刻み・深さ）を native の口の build_note で組み、
// チップの LFO そのもの（swp30 の lfo_pitch_trace）を回す。遅れて掛かる音色（xg/native_voice.h の
// vib_ramps）は、遅れの間は 0、そのあと 20ms ごとに深さをせり上げる（native_driver と同じ）
struct vib_line {
	std::vector<pt> pts;          // ms とセント
	float hz = 0;                 // 揺れの速さ
	float depth_cents = 0;        // 行き着いた深さ（片側）
	float delay_ms = 0;           // 掛かり始めるまで
	bool active = true;
};

// 遅れて掛かる音色の、せり上がりの途中の深さ（native と同じ式。xg/native_voice.h の vib_ramp_value）
inline int vib_ramp_value(const u8 *rom, int dpt, int c1, int c2)
{
	return xg::nv::vib_ramp_value(rom, dpt, c1, c2);
}

// せり上がりきった深さ
inline int vib_ramp_settled(const u8 *rom, const u8 *el, int dpt)
{
	return vib_ramp_value(rom, dpt, xg::nv::vib_ramp_target(el), 127);
}

inline std::vector<vib_line> vib_lines(const u8 *rom, u32 rec, const u8 *part, float span_ms)
{
	namespace nv = xg::nv;
	std::vector<vib_line> out;
	if (!rom || !rec)
		return out;
	const int n = nv::element_count(rom, rec);
	const int N = int(span_ms / 1000.0f * float(RATE));
	std::vector<s16> wave(size_t(std::max(N, 1)));
	for (int e = 0; e < n; e++) {
		const u8 *el = nv::element(rom, rec, e);
		vib_line line;
		line.active = nv::element_active(el, NOTE, VEL);
		const nv::slot_regs sr = nv::build_note(rom, el, NOTE, 0, nullptr, nv::defaults(), 0, VEL, part[0x1a], part[0x1b],
		                                        part[0x15], part[0x16], -1, NOTE, false, part[0x62], part[0x63]);
		const u16 reg = sr.v[0x0a];
		// 深さ（レジスタ 0x0a の下位 8bit）。下位 7bit が深さ、bit7 が「8 倍の目盛り」（lfo_depth_cents）。
		// 遅れの無い音色は押した瞬間の値のまま（build_note が Vib Depth 込みで作る）。
		// **遅れて掛かる音色**は、遅れが明けてから 20ms ごとに 2 本のせり上がりを進め、大きいほうが効く
		// （2026-09-23 に firmware の 0x0a と 20ms ごとに突き合わせた。Violin・Dyna Saw・Flute・Cello・Oboe の
		// Vib Depth 0-127 で、行き着く値はすべて一致。Depth 65-68 の出だし 100ms ほどの上がり方だけが少し違う）:
		//   音色のぶん  = 表[c1]。c1 は 0 から音色の刻み（vib_ramp_step）で目標（byte14）まで。
		//                 Depth が 64 より下なら、1 段ごとに 14 目盛り引く（62 以下はまず 0）
		//   Depth のぶん = 表[c2] を VIB_DEPTH_TAB[Depth] で止めたもの。c2 は 0 から 5 ずつ（表の 63 より先も引く）
		// 表[c] = VIB_REG_TAB[VIB_CNT_TAB[c]]。前はせり上がりを音色の小さな表で止め、bit7 も落としていたので、
		// Vib Depth を上げても絵が数セントのまま動かなかった
		const int dpt = part[0x16];
		int dly = 0, step = 0, tgt = 0;
		const bool ramps = nv::vib_ramps(el);
		if (ramps) {
			tgt  = nv::vib_ramp_target(el);
			step = nv::vib_ramp_step(el);
			dly  = nv::vib_delay_ticks(el);
		}
		// bit7 を落とした深さ 127 で回して、深さの比（と bit7 なら 8 倍）で伸び縮みさせる（get_pitch は深さに比例）
		swp30_device::lfo_pitch_trace(u16((reg & 0xff00) | 0x7f), wave.data(), N);
		const double unit = 1200.0 / 1024.0;
		const int tick = int(nv::VIB_TICK);          // 20ms
		auto scale = [](int d) { return double(d & 0x7f) / 127.0 * ((d & 0x80) ? 8.0 : 1.0); };
		int depth = ramps ? 0 : (reg & 0xff);
		int c1 = 0, c2 = 0, left = dly;
		bool started = false;
		float peak = 0;
		for (int i = 0; i < N; i += 32) {
			if (ramps && i > 0 && i % tick < 32) {
				if (left > 0)
					left--;
				else {
					c1 = std::min(tgt, c1 + step);
					c2 = std::min(127, c2 + 5);
					depth = vib_ramp_value(rom, dpt, c1, c2);
					if (!started && dly > 0)
						line.delay_ms = float(i / RATE * 1000.0);
					started = true;
				}
			}
			const float c = float(double(wave[size_t(i)]) * scale(depth) * unit);
			line.pts.push_back({ float(i / RATE * 1000.0), c });
			peak = std::max(peak, std::fabs(c));
		}
		const int stepv = (reg >> 8) & 0x3f;
		line.hz = float(stepv * RATE / 262144.0);
		line.depth_cents = peak;
		out.push_back(std::move(line));
	}
	return out;
}

// ---- モジュレーションのビブラート
//
// レジスタ 0x0a の下位（LFO の音程の深さ）→ 片側のセント。下位 7bit が深さ、bit7 で 8 倍
// （swp30 の get_pitch: 状態 ±0x800 × 深さ を 12bit か 9bit 右へ。音程は 1 オクターブ 1024）
inline float lfo_depth_cents(int low)
{
	const int d = low & 0x7f;
	const double units = (low & 0x80) ? 2048.0 * d / 512.0 : 2048.0 * d / 4096.0;
	return float(units * 1200.0 / 1024.0);
}

// ホイールの位置ごとの揺れの深さ。実機は 表[max(つまみの合計の頭打ち, 音色自身の目盛り)]
// で、足さない（doc/native-engine.md の 6.215）。音色自身は Vib Depth と遅れてせり上がる
// 分の行き着く先を含む。ホイール以外のつまみ（AT・AC など）は 0 と見る
struct mod_line {
	float own_cents = 0;                    // 音色自身の揺れ（Vib Depth 込み、行き着いた深さ）
	std::array<float, 128> wheel{};         // ホイールのぶんだけの深さ（位置ごと）
	std::array<float, 128> eff{};           // 実際に効く深さ（大きいほう）
	bool active = true;
};

inline std::vector<mod_line> mod_lines(const u8 *rom, u32 rec, const u8 *part)
{
	namespace nv = xg::nv;
	std::vector<mod_line> out;
	if (!rom || !rec)
		return out;
	const int n = nv::element_count(rom, rec);
	const int depth = part[0x20];                // MW LFO PM
	for (int e = 0; e < n; e++) {
		const u8 *el = nv::element(rom, rec, e);
		mod_line line;
		line.active = nv::element_active(el, NOTE, VEL);
		const nv::slot_regs sr = nv::build_note(rom, el, NOTE, 0, nullptr, nv::defaults(), 0, VEL, part[0x1a], part[0x1b],
		                                        part[0x15], part[0x16], -1, NOTE, false, part[0x62], part[0x63]);
		int own = sr.v[0x0a] & 0xff;
		if (nv::vib_ramps(el))                   // 遅れてせり上がる音色は、行き着く先（vib_lines と同じ式）
			own = vib_ramp_settled(rom, el, part[0x16]);
		line.own_cents = lfo_depth_cents(own);
		for (int w = 0; w < 128; w++) {
			const int wheel = nv::pmod_reg(rom, depth * w / 128);
			line.wheel[size_t(w)] = lfo_depth_cents(wheel);
			line.eff[size_t(w)] = lfo_depth_cents(own > wheel ? own : wheel);
		}
		out.push_back(std::move(line));
	}
	return out;
}

// ---- フィルタ
//
// 鍵を押したときのフィルタのレジスタ（0x00-0x04）を native の口と同じ式で組み、チップの
// フィルタそのもの（swp30 の filter_impulse）にインパルスを通して、周波数ごとの大きさを出す。
// 切る高さは押した瞬間の値（フィルタの包絡線はその後で動かす）
struct filter_line {
	std::vector<pt> pts;          // Hz と dB（cents の欄に dB）
	u16 regs[5] = {};
	bool active = true;
	bool hpf = false;             // 第 2 段がハイパスで効いているか
};

// 鍵を押した瞬間のフィルタのレジスタ（native_driver の cut_plain・reso・filter2_reg と同じ）
inline void filter_regs(const u8 *rom, const u8 *el, const u8 *part, u16 out[5])
{
	namespace nv = xg::nv;
	const nv::slot_regs sr = nv::build_note(rom, el, NOTE, 0, nullptr, nv::defaults(), 0, VEL, part[0x1a], part[0x1b],
	                                        64, 64, -1, NOTE, false, part[0x62], part[0x63]);
	const u16 base = nv::cutoff_keyon(rom, el, NOTE, VEL, false, part[0x1a]);
	int v = int(base & 0xfff);
	if (part[0x18] != 64)
		v += nv::bright_shift(part[0x18]);
	v = std::clamp(v, 0, 0x7ff);
	out[0] = nv::cutoff_cap(u16((base & 0xf000) | u16(v)), el, VEL, part[0x19]);
	out[1] = 0xffff;
	out[2] = nv::filter2_reg(el[82], part[xg::ram::PART_HPF_RAM]);
	out[3] = sr.v[0x03];
	out[4] = u16((sr.v[0x04] & 0x07ff) | u16(u16(nv::reso_level(el, VEL, part[0x19])) << 11));
}

// 周波数（Hz）の並びで、そのレジスタのフィルタの大きさ（dB）。post の音量のぶんは除く
// 同じレジスタなら前に出した値を使う（毎コマ出すと重い）
inline std::vector<pt> filter_response(const u16 regs[5], const std::vector<float> &hz)
{
	struct entry { u16 regs[5]; size_t n; std::vector<pt> pts; };
	static std::vector<entry> cache;
	for (const entry &c : cache)
		if (!std::memcmp(c.regs, regs, sizeof(c.regs)) && c.n == hz.size())
			return c.pts;
	constexpr int N = 8192;
	static std::vector<float> ir(N);
	swp30_device::filter_impulse(regs[0], regs[1], regs[2], regs[3], regs[4], ir.data(), N);
	const int level = regs[3] & 0xff;
	const double post = (32.0 - (level & 0xf)) / 32.0 / double(1 << (level >> 4));
	std::vector<pt> out;
	out.reserve(hz.size());
	for (float f : hz) {
		// e^{-jwi} を掛け算で回していく（三角関数を 8192 回呼ばない）
		const double w = 2.0 * 3.14159265358979323846 * double(f) / RATE;
		const double cr = std::cos(w), ci = -std::sin(w);
		double pr = 1, pi = 0, re = 0, im = 0;
		for (int i = 0; i < N; i++) {
			re += ir[size_t(i)] * pr;
			im += ir[size_t(i)] * pi;
			const double nr = pr * cr - pi * ci;
			pi = pr * ci + pi * cr;
			pr = nr;
		}
		const double mag = std::sqrt(re * re + im * im) / (post > 0 ? post : 1.0);
		out.push_back({ f, float(20.0 * std::log10(std::max(mag, 1e-6))) });
	}
	if (cache.size() >= 64)
		cache.erase(cache.begin());
	entry e;
	std::memcpy(e.regs, regs, sizeof(e.regs));
	e.n = hz.size();
	e.pts = out;
	cache.push_back(std::move(e));
	return out;
}

// 20 Hz-20 kHz を対数で n 点
inline std::vector<float> log_hz(int n)
{
	std::vector<float> hz;
	for (int i = 0; i < n; i++)
		hz.push_back(float(20.0 * std::pow(1000.0, double(i) / double(n - 1))));
	return hz;
}

inline std::vector<filter_line> filter_lines(const u8 *rom, u32 rec, const u8 *part, int points = 96)
{
	namespace nv = xg::nv;
	std::vector<filter_line> out;
	if (!rom || !rec)
		return out;
	const std::vector<float> hz = log_hz(points);
	const int n = nv::element_count(rom, rec);
	for (int e = 0; e < n; e++) {
		const u8 *el = nv::element(rom, rec, e);
		filter_line line;
		line.active = nv::element_active(el, NOTE, VEL);
		filter_regs(rom, el, part, line.regs);
		line.hpf = (line.regs[2] & 0x7ff) != 0;
		line.pts = filter_response(line.regs, hz);
		out.push_back(std::move(line));
	}
	return out;
}

// ---- パートの EQ（08 pp 72・73・76・77）
//
// firmware は声ごとのレジスタ 0x20-0x2B に、低音と高音の 1 次の IIR を 1 つずつ書く（native の eq_set と同じ表）。
// チップ（swp30 の iir1_block::step）は y = (a0·x + a1·x[-1] + b1·y[-1]) >> 13 を 2 段。
// だから 1 段の特性は H(z) = (a0 + a1·z⁻¹) / (8192 − b1·z⁻¹)。フィルタのすぐ後ろ、声ごとに掛かる
inline std::vector<pt> eq_response(const u8 *rom, const u8 *part, const std::vector<float> &hz)
{
	namespace nv = xg::nv;
	std::vector<pt> out;
	if (!rom)
		return out;
	nv::slot_regs r{};
	nv::eq_set(rom, r, part[xg::ram::PART_EQ_LGAIN], part[xg::ram::PART_EQ_HGAIN],
	           part[xg::ram::PART_EQ_LFREQ], part[xg::ram::PART_EQ_HFREQ]);
	// 段 0（低音）: 0x20 a1・0x22 b1・0x24 a0。段 1（高音）: 0x26 b1・0x28 a1・0x2A a0（swp30 の書き込みの割り当て）
	const double a0[2] = { double(s16(r.v[0x24])), double(s16(r.v[0x2a])) };
	const double a1[2] = { double(s16(r.v[0x20])), double(s16(r.v[0x28])) };
	const double b1[2] = { double(s16(r.v[0x22])), double(s16(r.v[0x26])) };
	for (float f : hz) {
		const double w = 2.0 * 3.14159265358979323846 * double(f) / RATE;
		const std::complex<double> z1 = std::polar(1.0, -w);
		std::complex<double> h = 1.0;
		for (int k = 0; k < 2; k++)
			h *= (a0[k] + a1[k] * z1) / (8192.0 - b1[k] * z1);
		out.push_back({ f, float(20.0 * std::log10(std::max(std::abs(h), 1e-6))) });
	}
	return out;
}

} // namespace shape
} // namespace ui

#endif // S_MU2000_UI_VOICE_SHAPE_H
