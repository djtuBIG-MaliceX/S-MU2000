// license:BSD-3-Clause
//
// パートの音色の窓（doc/pc-editor.md）。一覧の VIB・FILTER・EG・EQ の小さな絵をダブルクリックすると開く。
// 同じ絵を大きく描き（点をつまんで動かすのは一覧と同じ）、その下に値の棒を並べて数でも合わせられるようにする。
// 右には音色を選ぶ面（xgui::program_pane）を出しっぱなしにする。品書きと違って閉じないので、
// 続けて音色を替えながら絵の変わり方を見られる。
//
// 表示の大きさは xgui::shapes_zoom（既定 0.6）。窓の大きさもその分だけ小さくしてある

#ifndef S_MU2000_UI_PART_SHAPES_H
#define S_MU2000_UI_PART_SHAPES_H

#pragma once

#include "xg_ui.h"

namespace ui {

class part_shapes : public imgui_view
{
public:
	const wchar_t *title() const override { return L"S-MU2000 パートの音色"; }
	// 900 × 640 の 6 割（540 × 384）に、右の音色の面のぶんを足した大きさ
	int default_width() const override  { return 740; }
	int default_height() const override { return 384; }
	void draw(xg::model &m, const xg_snapshot &ram, bridge &br) override;
};

} // namespace ui

#endif // S_MU2000_UI_PART_SHAPES_H
