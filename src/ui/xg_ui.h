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

std::string part_name(int part);        // A1-A16 ... D1-D16
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
void request_part(int part);            // part は 0-63
bool take_part_request();               // 頼みがあれば true（1 回だけ）
int  shape_window_part();               // パートの音色の窓で見ているパート（一覧で行を選んでも替わる）
void set_shape_window_part(int part);

// ---- マスターの窓（マスターボリューム・移調・システムエフェクトの戻り・マスター EQ）を開く頼み。
// 一覧のマスターの行（MASTER の名前、MASTER EQ の絵）のダブルクリックから
void request_master();
bool take_master_request();             // 頼みがあれば true（1 回だけ）

// 値の棒 1 本。表示は層の書式（xg::format）で、ダブルクリックか Ctrl+クリックで数を打てる。
// EQ の周波数は表の番号でなく Hz、マスター EQ の Q は 10 分の 1 で出す。戻り値は「値を変えたか」。
// label を渡すとパラメータの名前の代わりにそれを出す（"##" で始めれば名前を出さない）
bool param_slider(const char *key, int part, xg::model &m, bridge &br, const char *label = nullptr);

// ---- 一覧の表示の大きさ（文字の大きさの倍率、0.5〜1.5）。editor.ini に覚えておく
float &overview_zoom();
void set_overview_zoom(float zoom);

// ---- パートの音色の窓の表示の大きさ（0.4〜1.5、既定 0.6）。editor.ini に覚えておく
float &shapes_zoom();
void set_shapes_zoom(float zoom);

// ---- マスターの窓の表示の大きさ（0.4〜1.5、既定 0.8）。editor.ini に覚えておく
float &master_zoom();
void set_master_zoom(float zoom);

// 出しっぱなしで音色を選ぶ面。左に分類、右の上に音色、右の下にバンク違い。
// 押すとその場でプログラムチェンジを送るので、続けて選べる（program_menu の常設版）。
// 今見ているのと違う分類を押すと、その分類の先頭の音色（キットなら先頭のキット）に替える。
// 音色を替えたら、そのパートで 1 秒だけ音を鳴らして聴かせる
void program_pane(int part, xg::model &m, const xg_snapshot *ram, bridge &br);
// 試聴で鳴らしている音を止める（窓を閉じたとき）
void audition_stop(bridge &br);
// 試聴で鳴らす鍵。パートの音色の窓の鍵盤を右クリックして決める（目印が付く）。
// -1 なら決まっていない（ドラムキットはスネア、ほかは C3 = 60）。editor.ini に覚える
int  audition_note();
void set_audition_note(int note);

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
