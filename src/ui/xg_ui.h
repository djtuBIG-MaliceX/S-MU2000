// license:BSD-3-Clause
//
// ImGui で XG の値を触る窓（エディタ・一覧）が共通で使う小物。

#ifndef S_MU2000_UI_XG_UI_H
#define S_MU2000_UI_XG_UI_H

#pragma once

#include "bridge.h"
#include "snapshot.h"
#include "xg/fx_types.h"
#include "xg/model.h"
#include "xg/voices.h"

#include <string>

namespace ui {

// ImGui の窓 1 枚ぶんの中身。pc_window が窓と描画装置を用意して、1 コマごとに draw を呼ぶ
class imgui_view
{
public:
	virtual ~imgui_view() = default;
	virtual const wchar_t *title() const = 0;
	virtual int default_width() const = 0;
	virtual int default_height() const = 0;
	// ram は音声の糸が写した RAM と MIDI の見張り。m は同じものを読んだパラメータの層
	virtual void draw(xg::model &m, const xg_snapshot &ram, bridge &br) = 0;
	// 窓を閉じた（隠した）とき。マウスで鳴らしている音を止めるなど
	virtual void hidden(bridge &) {}
};

namespace xgui {

const xg::param &P(const char *key);

std::string part_name(int part);        // A1-A16・B1-B16
std::string channel_name(int value);    // 受信チャンネル。127 は OFF
const char *gm_name(int program);       // General MIDI の楽器名（規格の名前）
std::string voice_text(int msb, int lsb, int program);

// 音色の名前と絵を読む ROM。音源を読み込んだあとで 1 回渡す（無ければ GM の名前で出す）
void set_voice_rom(std::shared_ptr<const std::vector<u8>> rom);
const xg::voice_rom *voices();

// 右クリックで出す品書き（プログラムとバンク）。ROM から読めれば MU2000 の音色の名前で並べる。
// ram は音色の引き方を知るため（無ければ XG の既定）
void program_menu(int part, xg::model &m, const xg_snapshot *ram, bridge &br);

// エフェクトの種類の品書き（開いている品書きの中に並べる）。音色と同じく
// 分類 → 系統（MSB）→ LSB 違い の 3 段。LSB 違いの無い系統は 2 段目でそのまま選ぶ。
// 表の中身が 1 つの分類にしか無ければ（リバーブの表など）分類の段は省く。
// current は今の種類（MSB << 7 | LSB。分からなければ -1）。選ばれたら chosen に入れて true
bool fx_type_menu(const std::vector<xg::fx_type> &types, int current, int &chosen);

// ---- インサーションの設定の窓を開く頼み。一覧が出して、gui がタイマーで拾って窓を出す
void request_fx(int slot);              // slot は 1-4
bool take_fx_request();                 // 頼みがあれば true（1 回だけ）
int  fx_window_slot();                         // 設定の窓で見ているインサーション（1-4）
void set_fx_window_slot(int slot);

// ---- パートの音色の窓（VIB・FILTER・EG・EQ を大きく）を開く頼み。一覧の絵のダブルクリックから
void request_part(int part);            // part は 0-31
bool take_part_request();               // 頼みがあれば true（1 回だけ）
int  shape_window_part();               // パートの音色の窓で見ているパート
void set_shape_window_part(int part);

// ---- 一覧の表示の大きさ（文字の大きさの倍率、0.5〜1.5）。editor.ini に覚えておく
float &overview_zoom();
void set_overview_zoom(float zoom);

// ---- パートの音色の窓の表示の大きさ（0.4〜1.5、既定 0.6）。editor.ini に覚えておく
float &shapes_zoom();
void set_shapes_zoom(float zoom);

// 出しっぱなしで音色を選ぶ面。分類・音色・バンクの 3 つの並びを縦に出す。
// 押すとその場でプログラムチェンジを送るので、続けて選べる（program_menu の常設版）
void program_pane(int part, xg::model &m, const xg_snapshot *ram, bridge &br);

// ---- 説明（ヘルプ）。見出しや名前にカーソルを当てると、何に効くのかを出す（日本語・英語）。
// 邪魔な人もいるので、窓の上のチェックボックスで消せる。選んだ状態は
// %LOCALAPPDATA%\S-MU2000\editor.ini に覚えておく（窓どうしで共通）
bool &help_on();
int help_lang();                        // 0 が日本語、1 が English
// 直前の部品にカーソルが載っていれば、説明を出す。name は列の見出しかパラメータのキー
void help_tip(const char *name);
// 「説明を出す」のチェックボックスと、言語の選択
void help_checkbox();
// 表の見出しの行を、説明つきで出す（ImGui::TableHeadersRow の代わり）
void headers_with_help(int columns);

} // namespace xgui
} // namespace ui

#endif // S_MU2000_UI_XG_UI_H
