// license:BSD-3-Clause
//
// ImGui で XG の値を触る窓（エディタ・一覧）が共通で使う小物。

#ifndef S_MU2000_UI_XG_UI_H
#define S_MU2000_UI_XG_UI_H

#pragma once

#include "bridge.h"
#include "snapshot.h"
#include "ui/lang.h"
#include "ui/texts.h"
#include "xg/fx_types.h"
#include "xg/model.h"
#include "xg/voices.h"

#include <string>
#include <vector>

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

// ---- 説明の帯。窓の下に固定で出す説明の欄（パートの音色の窓）。
// 帯のある窓は描く前に begin_hint_bar、描き終えたら end_hint_bar。その間は、絵や名前にカーソルを
// 載せたときの説明をマウスのそばのツールチップでなく帯へ出す（hint）。帯の無い窓ではツールチップのまま
void begin_hint_bar();
void end_hint_bar();
bool hint_bar();
// 説明を出す。帯があれば帯へ、無ければ直前の部品のツールチップへ（printf の書式）
void hint(const char *fmt, ...);
const std::string &hint_text();
// 絵の点の字（実際の時間や音程）を集める。begin_values と end_values の間に描いた字を、
// 出せなかった分も含めて 1 行ずつ返す（音色の窓が、区画にカーソルが載ったとき帯に並べる）
void begin_values();
std::vector<std::string> end_values();
void shape_value(const char *text);

// マウスで動かしている値の送信。押している間は 60 ms に 1 回、行き先ごとに最新の値だけ送り、
// 離したらすぐ送る（毎コマ送ると直列が詰まって反応が遅れる）。窓の持ち主は毎コマ描いた後に drag_flush を呼ぶ
void drag_send(bridge &br, std::vector<u8> bytes);
void drag_flush(bridge &br);

// 今のコマの RAM の写し。窓が描く前に置き、絵（音色の中身を読むもの）が読む
void set_current_ram(const xg_snapshot *ram);
const xg_snapshot *current_ram();

// 右クリックで出す品書き（プログラムとバンク）。ROM から読めれば MU2000 の音色の名前で並べる。
// ram は音色の引き方を知るため（無ければ XG の既定）
void program_menu(int part, xg::model &m, const xg_snapshot *ram, bridge &br);

// エフェクトの種類の品書き（開いている品書きの中に並べる）。音色と同じく
// 分類 → 系統（MSB）→ LSB 違い の 3 段。LSB 違いの無い系統は 2 段目でそのまま選ぶ。
// 表の中身が 1 つの分類にしか無ければ（リバーブの表など）分類の段は省く。
// current は今の種類（MSB << 7 | LSB。分からなければ -1）。選ばれたら chosen に入れて true
bool fx_type_menu(const std::vector<xg::fx_type> &types, int current, int &chosen);

// ---- インサーションの設定の窓を開く頼み。一覧が出して、gui がタイマーで拾って窓を出す
void request_fx(int slot);              // slot は 1-4 がインサーション、5-7 がリバーブ・コーラス・バリエーション
bool take_fx_request();                 // 頼みがあれば true（1 回だけ）
int  fx_window_slot();                         // 設定の窓で見ているエフェクト（1-7）
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

// ---- ファイルの窓（.syx の書き出し・読み込み）。
// 描画の中からは開けない（窓が回っている間にタイマーが次のコマを描きに来て ImGui に入り直す）。
// だから頼みだけ置き、窓の持ち主（pc_window）が描き終えてから開いて、読み書きもする。
// 持ち主が開けない所（今は macOS）では file_dialogs() が false
enum class file_ask { none, save, open };
void set_file_dialogs(bool on);
bool file_dialogs();
void ask_save_file(std::vector<u8> bytes);          // 書き出す中身を渡して、名前を聞いてもらう
void ask_open_file();                               // 読み込むファイルを聞いてもらう
file_ask take_file_ask(std::vector<u8> &bytes);     // 持ち主が取る（save のときは中身も）
void give_opened_file(std::vector<u8> bytes);       // 持ち主が、読んだ中身を返す
bool take_opened_file(std::vector<u8> &bytes);      // 頼んだ側が受け取る（1 回だけ）
void set_file_note(std::string text);               // 結果のひとこと（「書き出した」など）
const std::string &file_note();

// ---- パートのパラメータの組。エディタのパートの面と、音色の窓の「すべて」が同じ表を使う
// （片方だけに項目が増えないように）。keys は nullptr まで。見出しは UI 言語で
// 付けるので、表は言語が替わると作り直す（part_groups() が覚えておく）。
struct part_group { const char *title; const char *const keys[12]; };
inline const std::vector<part_group> &part_groups()
{
	static std::vector<part_group> g;
	static int built = -1;
	if (built != int(get_lang())) {
		const ui_texts &t = texts();
		std::vector<part_group> fresh = {
			{ t.xgui_group_voice,  { "part.bank_msb", "part.bank_lsb", "part.program", "part.mode", "part.element_reserve" } },
			{ t.xgui_group_vol,    { "part.volume", "part.pan", "part.dry_level", "part.reverb_send", "part.chorus_send", "part.variation_send" } },
			{ t.xgui_group_rcv,    { "part.rcv_channel", "part.mono_poly", "part.key_assign", "part.note_low", "part.note_high",
			                         "part.note_shift", "part.detune", "part.vel_depth", "part.vel_offset",
			                         "part.vel_limit_low", "part.vel_limit_high" } },
			{ t.xgui_group_filter, { "part.cutoff", "part.resonance", "part.hpf_cutoff", "part.attack", "part.decay", "part.release" } },
			{ t.xgui_group_peg,    { "part.peg_init_level", "part.peg_attack_time", "part.peg_rel_level", "part.peg_rel_time" } },
			{ t.xgui_group_porta,  { "part.porta_switch", "part.porta_time" } },
			{ t.xgui_group_vib,    { "part.vib_rate", "part.vib_depth", "part.vib_delay" } },
			{ t.xgui_group_eq,     { "part.eq_bass_gain", "part.eq_bass_freq", "part.eq_treble_gain", "part.eq_treble_freq" } },
			{ t.xgui_group_mod,    { "part.mw_pitch", "part.mw_filter", "part.mw_amp", "part.mw_lfo_pmod", "part.mw_lfo_fmod", "part.mw_lfo_amod" } },
			{ t.xgui_group_bend,   { "part.bend_pitch", "part.bend_filter", "part.bend_amp", "part.bend_lfo_pmod", "part.bend_lfo_fmod", "part.bend_lfo_amod" } },
			{ t.xgui_group_cat,    { "part.cat_pitch", "part.cat_filter", "part.cat_amp", "part.cat_lfo_pmod", "part.cat_lfo_fmod", "part.cat_lfo_amod" } },
			{ t.xgui_group_pat,    { "part.pat_pitch", "part.pat_filter", "part.pat_amp", "part.pat_lfo_pmod", "part.pat_lfo_fmod", "part.pat_lfo_amod" } },
			{ "AC1",               { "part.ac1_cc", "part.ac1_pitch", "part.ac1_filter", "part.ac1_amp", "part.ac1_lfo_pmod", "part.ac1_lfo_fmod", "part.ac1_lfo_amod" } },
			{ "AC2",               { "part.ac2_cc", "part.ac2_pitch", "part.ac2_filter", "part.ac2_amp", "part.ac2_lfo_pmod", "part.ac2_lfo_fmod", "part.ac2_lfo_amod" } },
		};
		g.swap(fresh);
		built = int(get_lang());
	}
	return g;
}

// 値の棒 1 本。表示は層の書式（xg::format）で、ダブルクリックか Ctrl+クリックで数を打てる。
// EQ の周波数は表の番号でなく Hz、マスター EQ の Q は 10 分の 1 で出す。戻り値は「値を変えたか」。
// label を渡すとパラメータの名前の代わりにそれを出す（"##" で始めれば名前を出さない）
bool param_slider(const char *key, int part, xg::model &m, bridge &br, const char *label = nullptr);
// 値の棒と同じ書き方の値（EQ の周波数は Hz など）と、「名前 : 値」の 1 行
std::string value_text(const char *key, int value);
std::string param_line(const char *key, int part, xg::model &m);

// ---- 一覧の表示の大きさ（文字の大きさの倍率、0.5〜1.5）。editor.ini に覚えておく
float &overview_zoom();
void set_overview_zoom(float zoom);

// ---- パートの音色の窓の表示の大きさ（0.4〜1.5、既定 0.6）。editor.ini に覚えておく
float &shapes_zoom();
void set_shapes_zoom(float zoom);
// 音色の窓の区画（番号）ごとに、絵で触るか（false）つまみで触るか（true）。editor.ini に覚えておく
bool shapes_knobs(int panel);
void set_shapes_knobs(int panel, bool knobs);

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
// 言語は ui::lang が持つひとつ（--lang・editor.ini・ロケールの順）。texts() の
// パネル文言と同じ言語になる
bool &help_on();
int help_lang();                        // 0 が日本語、1 が English (ui::lang と同じ番号)
// 言語を選ぶ。editor.ini に書き戻すので、次もその言語になる（--lang があればそちらが勝つ）
void set_help_lang(int lang);
// 直前の部品にカーソルが載っていれば、説明を出す。name は列の見出しかパラメータのキー
void help_tip(const char *name);
// 説明の文そのもの（説明を消していれば、または無ければ nullptr）
const char *help_for(const char *name);
// XG の仕様書のパラメータ名と番地（"MW LFO PMOD DEPTH（08 pp 20）"）。パートの項目だけ。無ければ空
std::string official_name(const char *key);
// 「説明を出す」のチェックボックスと、言語の選択
void help_checkbox();
// 表の見出しの行を、説明つきで出す（ImGui::TableHeadersRow の代わり）。
// keys は列ごとの HELP キーで、見出しの表示文言とは別（訳すと変わるため）
void headers_with_help(int columns, const char *const *keys);

} // namespace xgui
} // namespace ui

#endif // S_MU2000_UI_XG_UI_H
