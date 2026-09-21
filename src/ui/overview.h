// license:BSD-3-Clause
//
// 一覧の窓（doc/pc-editor.md）。Domino のトラック一覧のように、32 パートを 1 行ずつ並べて、
// 曲を流しながら全体のバランスを見て整える。
//
// 1 行に: パートと音色、VEL メーター、VOL / EXP / PAN / P.BEND / MOD / HOLD の棒と数、
// VIB / FILTER / EG / EQ の絵（見るだけ。ダブルクリックでパートの音色の窓）、INS、VAR / CHO / REV の棒と数、鳴っている鍵盤。
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
	overview()
	{
		for (int i = 0; i < XG_PARTS; i++)
			m_playing[i] = m_saved_rcv[i] = -1;
	}

	const wchar_t *title() const override { return L"S-MU2000 一覧"; }
	// 表示の大きさ（xgui::overview_zoom、既定 0.625）で描くので、窓もその分だけ小さく出す
	int default_width() const override  { return 1200; }
	int default_height() const override { return 700; }   // 64 パートぶん並ぶので高めに
	void draw(xg::model &m, const xg_snapshot &ram, bridge &br) override;
	// 閉じたら、鳴らしている鍵を離し、ミュートとソロを外す（受信チャンネルを戻す）
	void hidden(bridge &br) override;

	struct column;                   // 列の中身（overview.cpp）

	// 絵の 1 マス。パートの音色の窓（part_shapes）も同じものを大きく描く。
	// compact は一覧の中の小さなマスのとき。描くだけでマウスでは触れない（ダブルクリックで
	// パートの音色の窓を頼むのは呼ぶ側）。点をつまんで変えるのは compact でないときだけ
	//
	// EG: アタック・ディケイ・リリースの形を描き、点をつまんで動かす
	static void eg_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact);
	// ピッチ EG: 音程の動き（出だし → 本来の音程 → 離してからリリースレベル）を描き、点をつまんで動かす
	static void peg_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact);
	// フィルタ: 周波数特性の山を描き、山の頂をつまんで横でカットオフ、縦でレゾナンス
	static void filter_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact);
	// パートの EQ: 低音と高音の点をつまんで、横で周波数、縦でゲイン
	static void eq_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact);
	// ビブラート: 揺れの波の山をつまんで速さと深さ、平らな所の終わりで掛かり始め
	static void vib_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact);
	// マスター EQ の 5 つの帯の特性。edit なら点をつまんで周波数とゲイン、ホイールで Q（マスターの窓）。
	// edit でなければ描くだけ（一覧のマスターの行）
	static void master_eq_plot(xg::model &m, bridge &br, float w, float h, bool edit);

	// パートの音色の窓の上のペイン: 掛かっているエフェクト（種類の名前まで）、VOL〜HOLD と VAR〜REV の棒
	// （一覧と同じく触れる）、このパートの鍵盤。窓を閉じたら strip_hidden で鳴らしている鍵を離す
	void part_strip(int part, xg::model &m, const xg_snapshot &ram, bridge &br);
	void strip_hidden(bridge &br)
	{
		release_keys(br);
		release_pc_keys(br);
	}

private:
	// 1 パートの鍵盤（押さえている鍵が光る。押すと鳴らす）。slot は受信の口 × 16 + ch（無ければ -1）。
	// marker（パートの音色の窓）なら、右クリックで試聴の鍵を決め（目印を描く）、左で鳴らす。
	// 一覧では左右どちらでも鳴らす。pc_low は PC のキーボードで弾ける範囲の下の端（-1 なら描かない）
	void keys_cell(int part, int slot, const xg_snapshot &ram, bridge &br, float w, float h,
	               bool marker = false, int pc_low = -1);
	// モジュレーションホイール（CC1）。カーソルを載せてホイールか、上下にドラッグで変える
	void mod_wheel(int part, int slot, const xg_snapshot &ram, bridge &br, float w, float h);
	// PC のキーボードで弾く（A W S E D F T G Y H U J K O L P ; が C から、Z / X でオクターブ）
	void pc_keys(int slot, bridge &br);
	void release_pc_keys(bridge &br);
	// 行を選ぶ。パートの音色の窓も同じパートに替える
	void select_part(int part);
	void release_keys(bridge &br);          // マウスで鳴らしている鍵を全部離す
	// 上の帯の右端の、同時発音数（マスタとスレーブの内訳）と CPU の負荷。数字の後ろに棒
	void meters(bridge &br);
	// ミュートとソロを音源に効かせる。消すパートは受信チャンネルを OFF にし（先に
	// そのチャンネルへオールサウンドオフ）、戻すパートは覚えておいたチャンネルに戻す。
	// 曲の XG リセットなどで受信チャンネルが書き換わったら、覚えを捨ててもう一度消す
	void apply_mutes(xg::model &m, bridge &br);
	// パートの欄の右端の M / S の印
	void mute_buttons(int part, float x, float y, float w, float h);
	void row(int part, xg::model &m, const xg_snapshot &ram, bridge &br, float h);
	// INS 列の 1 マス。掛かっているエフェクトの印（1-4、V）を横に並べる。names なら種類の名前も。
	// which で並べるものを絞る（パートの音色の窓はインサーションとバリエーションを別の場所に出す）。
	// 右クリックで掛ける・外す・種類、印のドラッグで別のパートへ、印のダブルクリックで設定の窓
	enum class fx_which { all, insertions, variation };
	void ins_cell(int part, xg::model &m, bridge &br, float h, bool names = false, fx_which which = fx_which::all);
	// パートの帯の 1 行目（VAR の棒の真上）に、バリエーションの種類と繋がり方（x0-x1 の幅に収める）
	void variation_label(int part, xg::model &m, bridge &br, float x0, float y, float x1);
	// マスター EQ の 1 マス。見るだけで、ダブルクリックでマスターの窓
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
	float  m_level[XG_PARTS] = {};          // VEL メーターの今の高さ（0-1）
	u32    m_seen_ons[XG_PARTS] = {};       // 見張りのノートオンの回数を最後に見た値
	// 鳴らしている鍵（-1 は無し）と、そのときの受信の口×チャンネル。
	// 配列の初期化で -1 を並べるのは 64 個では長いので、開くときに埋める（reset_rows）
	int    m_playing[XG_PARTS];
	int    m_playing_slot[XG_PARTS] = {};
	// PC のキーボードで弾いている音（キーごと。-1 は鳴らしていない）と、その口×チャンネル
	int    m_pc_note[17] = { -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1 };
	int    m_pc_slot[17] = {};
	int    m_pc_base = 60;                 // A の鍵（C3）
	// モジュレーションホイールで送った値と時刻（RAM の写しが追いつくまではこちらを出す）
	int    m_mod_sent = -1;
	double m_mod_sent_at = -10.0;
	xg::model *m_model = nullptr;     // 閉じたときに受信チャンネルを戻すため（draw で覚える）
	bool   m_mute[XG_PARTS] = {}, m_solo[XG_PARTS] = {};
	int    m_saved_rcv[XG_PARTS];     // ミュートで OFF にする前の受信チャンネル（-1 は消していない）
	double m_scrolled_at = -1;        // ホイールで表をスクロールした時刻（エディタと同じ決まり）
	bool   m_wheel_taken = false;
};

} // namespace ui

#endif // S_MU2000_UI_OVERVIEW_H
