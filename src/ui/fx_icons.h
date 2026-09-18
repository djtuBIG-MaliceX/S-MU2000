// license:BSD-3-Clause
//
// エフェクトの種類の印（小さな絵）。xg::fx_categories() の分け方ごとに 1 つ。
// 絵は ImGui の線で描くので、どの大きさでも粗くならず、文字の色に合わせて色を変えられる。
//
//   リバーブ              点から広がる 3 本の弧
//   初期反射・ゲート      長さのばらばらな縦の線（反射）
//   ディレイ・エコー      だんだん小さくなる丸
//   カラオケ              台に立てたマイク
//   コーラス・セレステ    ずれた 2 本の波
//   フランジャー・フェイザー  だんだん細かくなる波
//   回転・トレモロ・パン  回る矢印
//   歪み・アンプ          頭のつぶれた波
//   EQ・ワウ・フィルタ    山のある特性の線
//   コンプ・ゲート        上下から押す矢印
//   組み合わせ            線でつないだ 3 つの箱
//   ローファイ・テクノ    階段の波
//   ピッチ・その他        音符と上向きの矢印
//   NO EFFECT             斜線の入った丸
//   THRU                  まっすぐな矢印

#ifndef S_MU2000_UI_FX_ICONS_H
#define S_MU2000_UI_FX_ICONS_H

#pragma once

#include "imgui.h"

namespace ui {
namespace xgui {

// 種類の MSB が入っている分類の番号（xg::fx_categories() の並び）。NO EFFECT・THRU・表に無いものは -1
int fx_category_of(int msb);

// 種類の MSB の印を、左上 min・一辺 size の正方形に描く
void fx_icon(ImDrawList *dl, ImVec2 min, float size, int msb, ImU32 color);

// 文字の行に印を置く（行の高さの正方形。カーソルは印の右へ進む）
void fx_icon_inline(int msb, ImU32 color);

// 種類を選ぶ箱。閉じているときの表示に、種類の名前と一緒に印を出す（type が負なら "--"）。
// 開いていれば true を返すので、中身を描いて ImGui::EndCombo() を呼ぶ
bool begin_fx_combo(const char *id, int type, ImGuiComboFlags flags = 0);

} // namespace xgui
} // namespace ui

#endif // S_MU2000_UI_FX_ICONS_H
