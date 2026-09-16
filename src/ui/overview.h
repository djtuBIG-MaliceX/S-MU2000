// license:BSD-3-Clause
//
// 一覧の窓（doc/pc-editor.md）。Domino のトラック一覧のように、32 パートを 1 行ずつ並べて、
// 曲を流しながら全体のバランスを見て整える。
//
// 1 行に: パートと音色、VEL メーター、VOL / EXP / PAN / P.BEND / MOD / HOLD の棒と数、
// VIB / FILTER / EG / EQ の絵（点をつまんで変える）、INS、VAR / CHO / REV の棒と数、鳴っている鍵盤。
// 値は RAM の写し（panel::tick が層に入れたもの）と、MIDI の見張り（押さえている鍵）から。

#ifndef S_MU2000_UI_OVERVIEW_H
#define S_MU2000_UI_OVERVIEW_H

#pragma once

#include "xg_ui.h"
#include "xg/fx_types.h"

namespace ui {

class overview : public imgui_view
{
public:
	const wchar_t *title() const override { return L"S-MU2000 一覧"; }
	// 表示の大きさ（xgui::overview_zoom、既定 0.625）で描くので、窓もその分だけ小さく出す
	int default_width() const override  { return 1200; }
	int default_height() const override { return 500; }
	void draw(xg::model &m, const xg_snapshot &ram, bridge &br) override;
	// 閉じたら、鳴らしている鍵を離し、ミュートとソロを外す（受信チャンネルを戻す）
	void hidden(bridge &br) override;

	struct column;                   // 列の中身（overview.cpp）

	// 絵の 1 マス。パートの音色の窓（part_shapes）も同じものを大きく描く。
	// compact は一覧の中の小さなマスのとき。ダブルクリックでパートの音色の窓を頼む
	//
	// EG: アタック・ディケイ・リリースの形を描き、点をつまんで動かす
	static void eg_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact);
	// フィルタ: 周波数特性の山を描き、山の頂をつまんで横でカットオフ、縦でレゾナンス
	static void filter_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact);
	// パートの EQ: 低音と高音の点をつまんで、横で周波数、縦でゲイン
	static void eq_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact);
	// ビブラート: 揺れの波の山をつまんで速さと深さ、平らな所の終わりで掛かり始め
	static void vib_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact);

private:
	void release_keys(bridge &br);          // マウスで鳴らしている鍵を全部離す
	// ミュートとソロを音源に効かせる。消すパートは受信チャンネルを OFF にし（先に
	// そのチャンネルへオールサウンドオフ）、戻すパートは覚えておいたチャンネルに戻す。
	// 曲の XG リセットなどで受信チャンネルが書き換わったら、覚えを捨ててもう一度消す
	void apply_mutes(xg::model &m, bridge &br);
	// パートの欄の右端の M / S の印
	void mute_buttons(int part, float x, float y, float w, float h);
	void row(int part, xg::model &m, const xg_snapshot &ram, bridge &br, float h);
	// INS 列の 1 マス。右クリックで掛ける・外す・種類、印のドラッグで別のパートへ
	void ins_cell(int part, xg::model &m, bridge &br, float h);
	// マスター EQ の 1 マス。5 つの帯の点、ホイールで Q、右クリックで種類
	void master_eq_cell(xg::model &m, bridge &br, float h);
	// 上のマスターの表。マスターボリューム、移調、リバーブ・コーラス・バリエーションの種類と戻り、
	// インサーション 1-4 の種類と掛け先、全パートの鍵盤
	void master_pane(xg::model &m, const xg_snapshot &ram, bridge &br);
	void system_fx_cell(const char *title, const std::vector<xg::fx_type> &types, const char *type_key,
	                    const char *return_col, bool variation, xg::model &m, const xg_snapshot &ram,
	                    bridge &br, float h);
	void insertion_cell(int slot_index, xg::model &m, bridge &br, float h);
	// 棒 1 つ。XG のパラメータなら触れる。part が -1 ならマスターの行
	void cell(const column &c, int part, xg::model &m, const xg_snapshot &ram, bridge &br, float w, float h);

	int    m_part = 0;
	float  m_level[32] = {};          // VEL メーターの今の高さ（0-1）
	u32    m_seen_ons[32] = {};       // 見張りのノートオンの回数を最後に見た値
	int    m_playing[32] = { -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
	                         -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1 };
	int    m_playing_slot[32] = {};   // 鳴らしている鍵（-1 は無し）と、そのときの受信の口×チャンネル
	xg::model *m_model = nullptr;     // 閉じたときに受信チャンネルを戻すため（draw で覚える）
	bool   m_mute[32] = {}, m_solo[32] = {};
	int    m_saved_rcv[32] = { -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
	                           -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1 };
	                                  // ミュートで OFF にする前の受信チャンネル（-1 は消していない）
	double m_scrolled_at = -1;        // ホイールで表をスクロールした時刻（エディタと同じ決まり）
	bool   m_wheel_taken = false;
};

} // namespace ui

#endif // S_MU2000_UI_OVERVIEW_H
