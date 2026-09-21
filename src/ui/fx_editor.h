// license:BSD-3-Clause
//
// インサーションエフェクトの設定の窓（doc/pc-editor.md）。一覧のインサーションの欄
// （マスターの INS 1-4、パートの INS の印）をダブルクリックすると開く。
// 実物のエフェクターのように、種類ごとのつまみを並べる。
//
// パラメータの並び・番地・範囲・表示は xg/fx_params.h。firmware の LCD の編集画面で調べたもの。

#ifndef S_MU2000_UI_FX_EDITOR_H
#define S_MU2000_UI_FX_EDITOR_H

#pragma once

#include "xg_ui.h"
#include "imgui.h"
#include "xg/fx_params.h"
#include "xg/sysfx.h"

namespace ui {

class fx_editor : public imgui_view
{
public:
	const wchar_t *title() const override { return L"S-MU2000 エフェクト"; }
	int default_width() const override  { return 1000; }
	int default_height() const override { return 720; }
	void draw(xg::model &m, const xg_snapshot &ram, bridge &br) override;

private:
	int m_focus = -1;                     // 説明を出しているつまみ（カーソルが載った・最後に触った）
	int m_focus_type = -1;                // そのときの種類（替わったら忘れる）
	// つまみ 1 つ。戻り値は「値が変わったか」
	bool knob(const char *id, int &v, int lo, int hi, float size, const char *label, const char *text);
	// EQ のパラメータを持つ種類の、特性のグラフ（p0-p1 の四角に描く）
	void eq_graph(const xg::fx_def &def, xg::model &m, bridge &br, ImVec2 p0, ImVec2 p1);
	// パラメータの番地とバイト数。見ているエフェクト（m_slot）の塊に無ければ false
	bool where(const xg::fx_param &fp, u32 &addr, int &size) const;
	int m_slot = 1;                       // 1-4 がインサーション、5-7 がリバーブ・コーラス・バリエーション
};

} // namespace ui

#endif // S_MU2000_UI_FX_EDITOR_H
