// license:BSD-3-Clause
//
// パートの音色の窓（doc/pc-editor.md）。一覧の VIB・FILTER・EG・EQ の小さな絵をダブルクリックすると開く。
// 同じ絵を大きく描き（一覧の小さな絵は見るだけ。点をつまんで動かすのはこの窓）、その下に値の棒を並べて数でも合わせられるようにする。
// 左には音色を選ぶ面（xgui::program_pane。左に分類、右の上に音色・下にバンク違い）を出しっぱなしにする。品書きと違って閉じないので、
// 続けて音色を替えながら絵の変わり方を見られる。一覧で行を選ぶと、この窓もそのパートに替わる。
// 上のペインには、掛かっているエフェクト（種類の名前まで）、VOL〜HOLD と VAR〜REV の棒（触れる）、鍵盤。
// 右はタブ。「形」は 3 × 2 の区画（VIB・FILTER（HPF も）・EG・ピッチ EG・EQ・ポルタメント）、「すべて」はパートのパラメータ全部の棒
// （エディタのパートの面と同じ組。xgui::PART_GROUPS）。パートの細かい設定はこの窓だけで全部触れる
//
// 表示の大きさは xgui::shapes_zoom（既定 0.6）。窓の大きさもその分だけ小さくしてある

#ifndef S_MU2000_UI_PART_SHAPES_H
#define S_MU2000_UI_PART_SHAPES_H

#pragma once

#include "overview.h"
#include "xg_ui.h"

namespace ui {

class part_shapes : public imgui_view
{
public:
	const wchar_t *title() const override { return L"S-MU2000 パートの音色"; }
	// 900 × 640 の 6 割（540 × 384）に、右の音色の面（左に分類、右に音色とバンク違いの 2 列）のぶんを足し、
	// 分類の 18 行が収まる高さにした大きさ
	// 上のペイン（エフェクト・棒・鍵盤）のぶん高くした
	// 右の絵を 3 列にしたぶん広げた
	int default_width() const override  { return 1020; }
	int default_height() const override { return 530; }
	void draw(xg::model &m, const xg_snapshot &ram, bridge &br) override;
	// 窓を閉じたら、試聴で鳴らしている音と、鍵盤で鳴らしている音を止める
	void hidden(bridge &br) override
	{
		xgui::audition_stop(br);
		m_strip.strip_hidden(br);
	}

private:
	// 上のペインは一覧と同じ部品で描く（棒のドラッグや鍵盤の押さえを覚える入れ物として持つ）
	overview m_strip;
};

} // namespace ui

#endif // S_MU2000_UI_PART_SHAPES_H
