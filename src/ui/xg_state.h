// license:BSD-3-Clause
//
// 音声の糸が写した XG の値（ui::xg_snapshot。ワーク RAM の並び）を使う小物。
//
//   load_model      写しをパラメータの層（xg::model）へ入れる。画面が値を得る道
//   read_value      写しから 1 つの値を読む。層を持たないところ（プラグインのパラメータ）が使う
//   setup_messages  写しを、別の機械に流し込めば同じ設定になる MIDI に直す。
//                   プラグインの状態に「XG の値だけ」の控えとして入れる（機械まるごとの状態が
//                   版違いで読めなかったときに、これを流して戻す）
//
// どれも Windows にも画面にも依存しない。

#ifndef S_MU2000_UI_XG_STATE_H
#define S_MU2000_UI_XG_STATE_H

#pragma once

#include "snapshot.h"
#include "xg/model.h"
#include "xg/ram.h"

#include <vector>

namespace ui {

// インサーションのパラメータ 1-10 は、RAM では 16bit の数。XG の 2 バイトの番地（03 0n 30-43）の形に崩す
inline void ins_wide_bytes(const xg_snapshot &ram, int n, u8 bytes[20])
{
	const u8 *w = ram.effect + (xg::ram::INS_BLOCK[n] - xg::ram::EFFECT) + xg::ram::INS_WIDE;
	for (int i = 0; i < 10; i++) {
		const int v = w[2 * i] << 8 | w[2 * i + 1];
		bytes[2 * i] = u8((v >> 7) & 0x7f);
		bytes[2 * i + 1] = u8(v & 0x7f);
	}
}

inline void load_model(xg::model &m, const xg_snapshot &ram, u64 now_ms)
{
	m.load(xg::pack(0x00, 0x00, 0x00), ram.system, XG_SYSTEM_SIZE, now_ms);
	for (const xg::ram::block &blk : xg::ram::EFFECTS)
		m.load(xg::pack(blk.hi, blk.mid, blk.lo), ram.effect + (blk.ram - xg::ram::EFFECT), blk.size, now_ms);
	for (int p = 0; p < XG_PARTS; p++) {
		m.load(xg::pack(0x08, u8(p), 0x00), ram.parts[p], xg::ram::PART_XG_SIZE, now_ms);
		m.load(xg::pack(0x08, u8(p), xg::ram::PART_EQ_XG), ram.parts[p] + xg::ram::PART_EQ_RAM, xg::ram::PART_EQ_SIZE, now_ms);
	}
	for (int n = 0; n < 4; n++) {
		u8 bytes[20];
		ins_wide_bytes(ram, n, bytes);
		m.load(xg::pack(0x03, u8(n), 0x30), bytes, sizeof(bytes), now_ms);
	}
}

// 番地の 1 バイトの、写しの中の場所。無ければ nullptr
// （インサーションの 16bit の番地 03 0n 30-43 は扱わない）
inline const u8 *locate_byte(const xg_snapshot &ram, u8 hi, u8 mid, u8 lo)
{
	if (hi == 0x00 && mid == 0x00 && lo < XG_SYSTEM_SIZE)
		return ram.system + lo;
	if (hi == 0x08 && mid < XG_PARTS) {
		if (lo < xg::ram::PART_XG_SIZE)
			return ram.parts[mid] + lo;
		if (lo >= xg::ram::PART_EQ_XG && lo < xg::ram::PART_EQ_XG + xg::ram::PART_EQ_SIZE)
			return ram.parts[mid] + xg::ram::PART_EQ_RAM + (lo - xg::ram::PART_EQ_XG);
		return nullptr;
	}
	for (const xg::ram::block &b : xg::ram::EFFECTS)
		if (hi == b.hi && mid == b.mid && lo >= b.lo && lo < b.lo + b.size)
			return ram.effect + (b.ram - xg::ram::EFFECT) + (lo - b.lo);
	return nullptr;
}

// 定義表の 1 つの値を写しから読む。読めない番地なら false
inline bool read_value(const xg::param &p, int part, const xg_snapshot &ram, int &value)
{
	const u8 mid = p.where == xg::area::part ? u8(part) : p.mid;
	int v = 0;
	for (int i = 0; i < p.size; i++) {
		const u8 *b = locate_byte(ram, p.hi, mid, u8(p.lo + i));
		if (!b)
			return false;
		v = p.enc == xg::coding::nibble ? (v << 4) | (*b & 0x0f) : (v << 7) | (*b & 0x7f);
	}
	value = v;
	return true;
}

// ---- 写しを MIDI に直す
//
// 一括ダンプ（F0 43 0n 4C 数 数 番地 中身 和 F7）で塊ごとに流す。firmware で確かめた決まり:
//   ・エフェクト（02 01 00/20/40・03 0n 00）とマスター EQ（02 40 00）は、先頭の種類を書くと
//     firmware が中身を種類の既定値に書き戻す。だから種類はパラメータチェンジで先に送り、
//     残りを種類の次の番地から一括ダンプで流す
//   ・パートの EQ（08 pp 72-77）は一括ダンプでは周波数が入らない。1 つずつのパラメータチェンジで送る
//     （74・75 は番地が無いので送らない）
// 間を空けずに続けて流してよい（全部を一度に溜めへ積んでも戻ることを確かめた）。
// 起動しただけの機械へ PHAZE1.mid の頭で変えた値を流すと、94 か所の違いが 1 か所（コーラスの
// 02 01 2B）まで戻る
inline std::vector<u8> setup_messages(const xg_snapshot &ram)
{
	std::vector<u8> out;
	out.reserve(8192);
	auto bulk = [&](u8 hi, u8 mid, u8 lo, const u8 *data, int n) {
		const size_t at = out.size();
		const u8 head[] = { 0xf0, 0x43, 0x00, 0x4c, u8((n >> 7) & 0x7f), u8(n & 0x7f), hi, mid, lo };
		out.insert(out.end(), head, head + sizeof(head));
		for (int i = 0; i < n; i++)
			out.push_back(data[i] & 0x7f);
		u32 sum = 0;
		for (size_t i = at + 4; i < out.size(); i++)
			sum += out[i];
		out.push_back(u8((0x80 - (sum & 0x7f)) & 0x7f));
		out.push_back(0xf7);
	};
	auto change = [&](u8 hi, u8 mid, u8 lo, const u8 *data, int n) {
		const u8 head[] = { 0xf0, 0x43, 0x10, 0x4c, hi, mid, lo };
		out.insert(out.end(), head, head + sizeof(head));
		for (int i = 0; i < n; i++)
			out.push_back(data[i] & 0x7f);
		out.push_back(0xf7);
	};

	bulk(0x00, 0x00, 0x00, ram.system, XG_SYSTEM_SIZE);
	for (const xg::ram::block &b : xg::ram::EFFECTS) {
		const u8 *data = ram.effect + (b.ram - xg::ram::EFFECT);
		const bool fx = (b.hi == 0x02 && b.mid == 0x01 && (b.lo == 0x00 || b.lo == 0x20 || b.lo == 0x40)) ||
		                (b.hi == 0x03 && b.lo == 0x00);
		const bool meq = b.hi == 0x02 && b.mid == 0x40 && b.lo == 0x00;
		const int head = fx ? 2 : meq ? 1 : 0;
		if (head)
			change(b.hi, b.mid, b.lo, data, head);
		bulk(b.hi, b.mid, u8(b.lo + head), data + head, int(b.size) - head);
	}
	for (int n = 0; n < 4; n++) {
		u8 bytes[20];
		ins_wide_bytes(ram, n, bytes);
		bulk(0x03, u8(n), 0x30, bytes, sizeof(bytes));
	}
	for (int p = 0; p < XG_PARTS; p++) {
		bulk(0x08, u8(p), 0x00, ram.parts[p], xg::ram::PART_XG_SIZE);
		const u8 *eq = ram.parts[p] + xg::ram::PART_EQ_RAM;
		for (int k : { 0, 1, 4, 5 })
			change(0x08, u8(p), u8(xg::ram::PART_EQ_XG + k), eq + k, 1);
	}
	return out;
}

} // namespace ui

#endif // S_MU2000_UI_XG_STATE_H
