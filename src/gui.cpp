// license:BSD-3-Clause
//
// 実機のフロントパネル風の画面で MU2000 を動かす。
//
//   gui <rom ディレクトリ> [--midi 番号] [--midi-b 番号] [--fast-midi]
//       [--midiout 番号] [--midiout-b 番号] [--midiout-mu 番号] [--latency ミリ秒]
//   gui --list                             MIDI の入口と出口の一覧
//   gui <rom ディレクトリ> --shot 絵.png    窓を出さずに絵だけ書き出す（見た目の確認用）
//
// 入口と出口は**動かしたまま画面から選べる**。パネルの MIDI IN A の
// ジャックを押すか、どこでも右クリックすると品書きが出る。選んだものは
// %LOCALAPPDATA%\S-MU2000\gui.ini に覚えておいて、次から使う。
//
// MU2000 の設定（ワーク RAM）は終わるときに残し、次の起動で使う（src/nvram.h）。
// --factory か右クリックの「工場出荷状態に戻す」で捨てられる。
//
// 音の作り方は live.exe と同じ。**時計を自分で持たない**（doc/design.md）。
// 画面は別スレッドで、音源とは ui::bridge 越しにしか触れ合わない。
//
// マウスホイールはダイヤルに割り当ててある。実機にもロータリー
// エンコーダがあり、VALUE -/+ のボタンと同じ働きをする。

#include "mu2000.h"
#include "nvram.h"
#include "smf.h"
#include "ui/audio_out.h"
#include "ui/audio_in.h"
#include "ui/bridge.h"
#include "ui/driver.h"
#include "ui/engine.h"
#include "ui/midi_in.h"
#include "ui/midi_guard.h"
#include "ui/midi_out.h"
#include "ui/layout.h"
#include "ui/panel.h"
#include "ui/fx_editor.h"
#include "ui/overview.h"
#include "ui/part_shapes.h"
#include "ui/pc_editor.h"
#include "ui/pc_window.h"
#include "ui/player.h"
#include "ui/text.h"
#include "ui/png.h"

#include <algorithm>
#include <atomic>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>

#include <windows.h>
#include <windowsx.h>
#include <commdlg.h>
#include <shellapi.h>

namespace {

constexpr u32 RATE = ui::AUDIO_RATE;

// ---- 音源側
//
// The engine itself now lives in ui/engine.h: the macOS front end needs the
// same boot sequence and, more to the point, the same MIDI routing, and one
// copy is the only way to keep the two from drifting. Only the name is pulled
// in here, so the rest of this file reads as it always did.

using ui::engine;


// ---- 窓

struct window_state {
	ui::panel   panel;
	bool lcd_only = false;
	ui::pc_window pc{ std::make_unique<ui::pc_editor>() };    // PC エディタ（F2 か右クリック）
	ui::pc_window list{ std::make_unique<ui::overview>() };   // 一覧（F3 か右クリック）
	ui::pc_window fx{ std::make_unique<ui::fx_editor>() };    // インサーションの設定（一覧でダブルクリック）
	ui::pc_window shapes{ std::make_unique<ui::part_shapes>() };   // パートの音色（一覧の絵をダブルクリック）
	ui::bridge *br = nullptr;
	engine     *eng = nullptr;
	ui::audio_out *out = nullptr;
	ui::audio_in  *ain = nullptr;      // A/D INPUT（録音デバイス）
	std::string ain_name;              // 選んだ録音デバイスの名前。空なら使わない
	std::string card_path;             // 差している SmartMedia のファイル。空なら差していない

	std::string audio_name;            // 音の出口（名前の一部）。空なら既定
	std::string layout_path;           // 読んでいる panel.txt。F5 で読み直す
	ui::player    play_file;
	ui::midi_in  *midi = nullptr;      // MIDI IN A
	ui::midi_in  *midi_b = nullptr;    // MIDI IN B
	ui::midi_out *mout = nullptr;      // MIDI THRU A
	ui::midi_out *mout_b = nullptr;    // MIDI THRU B
	ui::midi_out *mout_mu = nullptr;   // MIDI OUT（MU2000 が送り出すもの）
	std::thread   reboot;              // 工場出荷状態に戻す作業
	int  in_dev  = -1;                 // いま開いている番号。-1 は使っていない
	int  in_dev_b = -1;
	int  out_dev = -1;
	int  out_dev_b = -1;
	int  out_dev_mu = -1;
	std::string in_name, in_name_b, out_name, out_name_b, out_name_mu;
	// 起動したときに開けなかった口の名前。**選び直すまで gui.ini に残す**。
	// 残さないと、loopMIDI を起動し忘れた・機器を挿していなかっただけで、
	// 選んでおいた口を忘れてしまう
	std::string in_keep, in_keep_b, out_keep, out_keep_b, out_keep_mu;
	std::string last_error;            // 品書きから選んで開けなかったときの理由
	u64 reported_drops = 0;            // 画面の糸が最後に知らせた、捨てた MIDI の量
	bool keep_settings = false;        // gui.ini を書き換えない（--nomidi）


	// 二重書き用
	HDC     mem_dc = nullptr;
	HBITMAP mem_bmp = nullptr;
	int     mem_w = 0, mem_h = 0;
};

window_state g_win;

// ---- パネルの配置。作り直さずに文字ファイルで直せる（doc/panel-editing.md）

void apply_layout(const std::string &path, bool quiet)
{
	g_win.panel.lay() = ui::layout();          // まず既定値に戻す
	std::string err;
	if (!path.empty() && g_win.panel.lay().load(path, err)) {
		if (!quiet)
			std::printf("配置: %s\n", path.c_str());
	} else if (!path.empty() && !quiet) {
		std::printf("配置: %s を開けない。組み込みの配置を使う\n", path.c_str());
	}
	if (!err.empty())
		std::fprintf(stderr, "%s", err.c_str());
	std::fflush(stdout);
	g_win.panel.resize(g_win.panel.width(), g_win.panel.height());
}

// ---- 選んだ口を覚えておく
//
// 番号ではなく**名前**で覚える。USB の機器を挿し直すと番号がずれるので、
// 番号で覚えると次に開いたとき別の機器に繋がってしまう。

std::string settings_path()
{
	const char *base = std::getenv("LOCALAPPDATA");
	if (!base || !*base)
		return {};
	std::string dir = std::string(base) + "\\S-MU2000";
	CreateDirectoryA(dir.c_str(), nullptr);
	return dir + "\\gui.ini";
}

void load_settings(std::string &in_name, std::string &in_name_b,
                   std::string &out_name, std::string &out_name_b,
                   std::string &audio_name, float *volume = nullptr,
                   std::string *out_name_mu = nullptr, bool *fold_ports34 = nullptr,
                   std::string *ain_name = nullptr, std::string *card_path = nullptr)
{
	const std::string path = settings_path();
	if (path.empty())
		return;
	FILE *f = std::fopen(path.c_str(), "rb");
	if (!f)
		return;
	char line[512];
	while (std::fgets(line, sizeof(line), f)) {
		std::string t(line);
		while (!t.empty() && (t.back() == '\n' || t.back() == '\r'))
			t.pop_back();
		const size_t eq = t.find('=');
		if (eq == std::string::npos)
			continue;
		const std::string key = t.substr(0, eq), val = t.substr(eq + 1);
		if (key == "midi_in")   in_name   = val;
		if (key == "midi_in_b") in_name_b = val;
		if (key == "midi_out")   out_name   = val;
		if (key == "midi_out_b") out_name_b = val;
		if (key == "midi_out_mu" && out_name_mu) *out_name_mu = val;
		if (key == "audio_out")  audio_name = val;
		if (key == "audio_in" && ain_name) *ain_name = val;
		if (key == "smartmedia" && card_path) *card_path = val;
		if (key == "ports34" && fold_ports34) *fold_ports34 = val != "drop";
		if (key == "volume" && volume && !val.empty())
			*volume = std::clamp(float(std::atof(val.c_str())), 0.0f, 1.0f);
	}
	std::fclose(f);
}

void save_settings()
{
	if (g_win.keep_settings)                 // --nomidi。覚えている口を消さない
		return;
	const std::string path = settings_path();
	if (path.empty())
		return;
	FILE *f = std::fopen(path.c_str(), "wb");
	if (!f)
		return;
	auto pick = [](const std::string &now, const std::string &keep) {
		return (now.empty() ? keep : now).c_str();
	};
	std::fprintf(f, "midi_in=%s\n",    pick(g_win.in_name,     g_win.in_keep));
	std::fprintf(f, "midi_in_b=%s\n",  pick(g_win.in_name_b,   g_win.in_keep_b));
	std::fprintf(f, "midi_out=%s\n",   pick(g_win.out_name,    g_win.out_keep));
	std::fprintf(f, "midi_out_b=%s\n", pick(g_win.out_name_b,  g_win.out_keep_b));
	std::fprintf(f, "midi_out_mu=%s\n", pick(g_win.out_name_mu, g_win.out_keep_mu));
	std::fprintf(f, "audio_out=%s\n", g_win.audio_name.c_str());
	std::fprintf(f, "audio_in=%s\n", g_win.ain_name.c_str());
	std::fprintf(f, "smartmedia=%s\n", g_win.card_path.c_str());
	// パネルの VOLUME のつまみ。実機でも DAC の後ろのアナログのつまみで、
	// firmware の RAM には入らないので、こちらで覚える
	if (g_win.br)
		std::fprintf(f, "volume=%.3f\n", g_win.br->gain());
	std::fprintf(f, "ports34=%s\n", g_win.play_file.fold_extra_ports() ? "fold" : "drop");
	std::fclose(f);
}

// 名前で探す。見つからなければ -1
int find_device(const std::vector<std::string> &names, const std::string &want)
{
	if (want.empty())
		return -1;
	for (size_t i = 0; i < names.size(); i++)
		if (names[i] == want)
			return int(i);
	return -1;
}

// ---- 口を選ぶ品書き

enum : UINT {
	ID_IN_NONE = 900, ID_IN_BASE = 901,
	ID_INB_NONE = 1400, ID_INB_BASE = 1401,
	ID_OUT_NONE = 1900, ID_OUT_BASE = 1901,
	ID_OUTB_NONE = 2400, ID_OUTB_BASE = 2401,
	ID_OUTMU_NONE = 3100, ID_OUTMU_BASE = 3101,
	// A/D INPUT and SmartMedia. Kept out of the ID_BASE..ID_BASE+255 ranges
	// above, or the dispatch in the command handler takes them for that port:
	// both used to sit inside ID_OUTMU_BASE's 256, so picking a recording
	// device (or a card item) selected MIDI OUT instead
	ID_AIN_NONE = 3400, ID_AIN_BASE = 3401,
	ID_CARD_NEW16 = 3700, ID_CARD_NEW32, ID_CARD_NEW64, ID_CARD_NEW128,
	ID_CARD_OPEN = 3710, ID_CARD_EJECT = 3711,
	ID_PLAY_FILE = 2900, ID_STOP_FILE = 2901, ID_PORTS34_FOLD = 2902, ID_PORTS34_DROP = 2903,
	ID_FACTORY = 3000,
	ID_PC_EDITOR = 3001,
	ID_OVERVIEW = 3002,
};

// 品書きは **W 版**で作る。ソースは UTF-8 なので、A 版に渡すと
// CP932 と思われて文字化けする
void add_item(HMENU m, UINT flags, UINT_PTR id, const char *utf8)
{
	const std::wstring w = ui::to_wide(utf8);
	AppendMenuW(m, flags, id, w.c_str());
}

void fill_port_menu(HMENU m, const std::vector<std::string> &names, int now,
                    UINT id_none, UINT id_base)
{
	add_item(m, MF_STRING | (now < 0 ? MF_CHECKED : 0), id_none, "使わない");
	if (names.empty()) {
		AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
		add_item(m, MF_STRING | MF_GRAYED, 0, "（機器が無い）");
		return;
	}
	AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
	for (size_t i = 0; i < names.size(); i++)
		add_item(m, MF_STRING | (int(i) == now ? MF_CHECKED : 0),
		         id_base + UINT(i), names[i].c_str());
}

// A/D INPUT に入れる録音デバイス。選んでいる名前に印を付ける
void fill_ain_menu(HMENU m, const std::vector<std::string> &names)
{
	add_item(m, MF_STRING | (g_win.ain_name.empty() ? MF_CHECKED : 0), ID_AIN_NONE, "使わない");
	AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
	if (names.empty())
		add_item(m, MF_STRING | MF_GRAYED, 0, "（録音デバイスが無い）");
	for (size_t i = 0; i < names.size(); i++)
		add_item(m, MF_STRING | (names[i] == g_win.ain_name ? MF_CHECKED : 0),
		         ID_AIN_BASE + UINT(i), names[i].c_str());
}

void show_ain_menu(HWND hwnd, POINT screen)
{
	HMENU m = CreatePopupMenu();
	add_item(m, MF_STRING | MF_GRAYED, 0, "A/D INPUT（サンプリングで録る音）");
	AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
	fill_ain_menu(m, ui::audio_in::list());
	TrackPopupMenu(m, TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RIGHTBUTTON, screen.x, screen.y, 0, hwnd, nullptr);
	DestroyMenu(m);
}

// 録音デバイスを選ぶ。-1 は使わない
void choose_ain(int dev)
{
	if (!g_win.ain)
		return;
	g_win.ain->stop();
	g_win.last_error.clear();
	if (dev < 0) {
		g_win.ain_name.clear();
	} else {
		const auto names = ui::audio_in::list();
		if (dev < int(names.size())) {
			std::string err;
			if (!g_win.ain->start(names[size_t(dev)], err)) {
				g_win.last_error = err;
				std::fprintf(stderr, "A/D INPUT: %s\n", err.c_str());
			} else {
				std::printf("A/D INPUT: %s（%s）\n", g_win.ain->device_name().c_str(), g_win.ain->format_line().c_str());
				std::fflush(stdout);
			}
			g_win.ain_name = names[size_t(dev)];
		}
	}
	save_settings();
}

void show_port_menu(HWND hwnd, POINT screen)
{
	HMENU top = CreatePopupMenu();
	HMENU mi  = CreatePopupMenu();
	HMENU mib = CreatePopupMenu();
	HMENU mo  = CreatePopupMenu();
	HMENU mob = CreatePopupMenu();
	HMENU mom = CreatePopupMenu();

	const auto ins  = ui::midi_in::list();
	const auto outs = ui::midi_out::list();
	fill_port_menu(mi,  ins,  g_win.in_dev,    ID_IN_NONE,   ID_IN_BASE);
	fill_port_menu(mib, ins,  g_win.in_dev_b,  ID_INB_NONE,  ID_INB_BASE);
	fill_port_menu(mo,  outs, g_win.out_dev,   ID_OUT_NONE,  ID_OUT_BASE);
	fill_port_menu(mob, outs, g_win.out_dev_b, ID_OUTB_NONE, ID_OUTB_BASE);
	fill_port_menu(mom, outs, g_win.out_dev_mu, ID_OUTMU_NONE, ID_OUTMU_BASE);

	add_item(top, MF_POPUP, UINT_PTR(mi),  "MIDI IN A（パート 1-16）");
	add_item(top, MF_POPUP, UINT_PTR(mib), "MIDI IN B（パート 17-32）");
	add_item(top, MF_POPUP, UINT_PTR(mom), "MIDI OUT（MU2000 が送り出すもの）");
	add_item(top, MF_POPUP, UINT_PTR(mo),  "MIDI THRU A（A で受けたものを外へ）");
	add_item(top, MF_POPUP, UINT_PTR(mob), "MIDI THRU B（B で受けたものを外へ）");
	HMENU mai = CreatePopupMenu();
	fill_ain_menu(mai, ui::audio_in::list());
	add_item(top, MF_POPUP, UINT_PTR(mai), "A/D INPUT（サンプリングで録る音）");
	AppendMenuW(top, MF_SEPARATOR, 0, nullptr);
	add_item(top, MF_STRING, ID_OVERVIEW, "一覧を開く	F3");
	add_item(top, MF_STRING, ID_PC_EDITOR, "エディタを開く	F2");
	const bool ready = g_win.eng && g_win.eng->state.load() == 1;
	add_item(top, MF_STRING | (ready ? 0 : MF_GRAYED), ID_FACTORY, "工場出荷状態に戻す...");

	TrackPopupMenu(top, TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RIGHTBUTTON,
	               screen.x, screen.y, 0, hwnd, nullptr);
	DestroyMenu(top);
}

// ---- カードの差し込み口。SmartMedia を差す・MIDI ファイルを流す

// 書き換えたブロックをファイルへ書き戻す。写すときだけ音声の糸を止める
void flush_card()
{
	if (!g_win.eng || g_win.card_path.empty())
		return;
	std::vector<smu2000::smartmedia::block> blocks;
	{
		const std::lock_guard<std::mutex> hold(g_win.eng->card_lock);
		g_win.eng->mu.card().take_dirty_blocks(blocks);
	}
	std::string err;
	if (!smu2000::smartmedia::write_blocks(g_win.card_path, blocks, err)) {
		std::fprintf(stderr, "SmartMedia: %s\n", err.c_str());
		std::fflush(stderr);
	}
}

void eject_card()
{
	if (!g_win.eng)
		return;
	flush_card();
	{
		const std::lock_guard<std::mutex> hold(g_win.eng->card_lock);
		g_win.eng->mu.card().eject();
	}
	if (!g_win.card_path.empty())
		std::printf("SmartMedia を抜いた: %s\n", g_win.card_path.c_str());
	std::fflush(stdout);
	g_win.card_path.clear();
}

// ファイルの SmartMedia を差す。読むのは糸を止めずに済ませ、差し替えるときだけ止める
bool insert_card(const std::string &path, bool quiet = false)
{
	if (!g_win.eng)
		return false;
	smu2000::smartmedia card;
	std::string err;
	if (!card.load(path, err)) {
		if (!quiet)
			g_win.last_error = err;
		std::fprintf(stderr, "SmartMedia: %s\n", err.c_str());
		return false;
	}
	eject_card();
	{
		const std::lock_guard<std::mutex> hold(g_win.eng->card_lock);
		g_win.eng->mu.card() = std::move(card);
	}
	g_win.card_path = path;
	std::printf("SmartMedia を差した: %s（%uMB）\n", path.c_str(), g_win.eng->mu.card().megabytes());
	std::fflush(stdout);
	return true;
}

std::string ask_card_path(HWND hwnd, bool create)
{
	wchar_t file[MAX_PATH] = {};
	if (create)
		wcscpy(file, L"smartmedia.img");
	OPENFILENAMEW o{};
	o.lStructSize = sizeof(o);
	o.hwndOwner = hwnd;
	o.lpstrFilter = L"SmartMedia の中身 (*.img)\0*.img\0すべて (*.*)\0*.*\0";
	o.lpstrFile = file;
	o.nMaxFile = MAX_PATH;
	o.lpstrDefExt = L"img";
	if (create) {
		o.lpstrTitle = L"新しい SmartMedia の保存先";
		o.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
		if (!GetSaveFileNameW(&o))
			return {};
	} else {
		o.lpstrTitle = L"差す SmartMedia";
		o.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
		if (!GetOpenFileNameW(&o))
			return {};
	}
	return ui::to_utf8(file);
}

// 空の SmartMedia（物理の書式だけ）を作って差す。使う前に UTIL → CARD → Format で書式化する
void new_card(HWND hwnd, u32 megabytes)
{
	const std::string path = ask_card_path(hwnd, true);
	if (path.empty())
		return;
	smu2000::smartmedia card;
	card.create(megabytes);
	std::string err;
	if (!card.save(path, err)) {
		g_win.last_error = err;
		return;
	}
	if (insert_card(path)) {
		save_settings();
		MessageBoxW(hwnd, L"空の SmartMedia を差しました。\n"
		                  L"使う前に、本体の UTIL → CARD → Format で書式化してください。",
		            L"S-MU2000", MB_OK | MB_ICONINFORMATION);
	}
}

void show_card_menu(HWND hwnd, POINT screen)
{
	HMENU m = CreatePopupMenu();
	const bool card_in = !g_win.card_path.empty();
	HMENU mnew = CreatePopupMenu();
	add_item(mnew, MF_STRING, ID_CARD_NEW16, "16MB");
	add_item(mnew, MF_STRING, ID_CARD_NEW32, "32MB");
	add_item(mnew, MF_STRING, ID_CARD_NEW64, "64MB");
	add_item(mnew, MF_STRING, ID_CARD_NEW128, "128MB");
	add_item(m, MF_POPUP, UINT_PTR(mnew), "新しい SmartMedia を作って差す");
	add_item(m, MF_STRING, ID_CARD_OPEN, "SmartMedia を差す...");
	std::string eject = "SmartMedia を抜く";
	if (card_in)
		eject += "（" + g_win.card_path.substr(g_win.card_path.find_last_of("\\/") + 1) + "）";
	add_item(m, MF_STRING | (card_in ? 0 : MF_GRAYED), ID_CARD_EJECT, eject.c_str());
	AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
	const bool on = g_win.play_file.playing();
	add_item(m, MF_STRING, ID_PLAY_FILE, "MIDI ファイルを再生...");
	std::string stop = "止める";
	if (on)
		stop += "（" + g_win.play_file.name() + "）";
	add_item(m, MF_STRING | (on ? 0 : MF_GRAYED), ID_STOP_FILE, stop.c_str());
	AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
	// エミュの口は A・B の 2 つ（実機の C・D は未対応）。3〜4 口の MIDI ファイルの口 3・4 をどうするか
	const bool fold = g_win.play_file.fold_extra_ports();
	add_item(m, MF_STRING | (fold ? MF_CHECKED : 0), ID_PORTS34_FOLD, "口 3・4 を A・B に重ねて鳴らす");
	add_item(m, MF_STRING | (fold ? 0 : MF_CHECKED), ID_PORTS34_DROP, "口 3・4 は鳴らさない");
	TrackPopupMenu(m, TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RIGHTBUTTON,
	               screen.x, screen.y, 0, hwnd, nullptr);
	DestroyMenu(m);
}

// MIDI ファイルを流す（品書きから選んだとき・窓に落とされたとき）。鳴っていれば止めて流し直す
void play_midi_file(HWND hwnd, const std::string &path)
{
	if (!g_win.br)
		return;
	std::string err;
	if (!g_win.play_file.start(path, *g_win.br, err)) {
		const std::wstring w = ui::to_wide("開けない: " + err);
		MessageBoxW(hwnd, w.c_str(), L"S-MU2000", MB_OK | MB_ICONWARNING);
		return;
	}
	std::printf("再生: %s（%.1f 秒）\n", path.c_str(), g_win.play_file.length());
	if (g_win.play_file.ports_used() > 2)
		std::printf("  この曲は %d 口ぶん。C・D は未対応なので、口 3 以降は%s\n", g_win.play_file.ports_used(),
		            g_win.play_file.fold_extra_ports() ? " A・B に重ねて鳴らす" : "鳴らさない");
	std::fflush(stdout);
}

// 窓に落とされたファイル（エディタや一覧の窓から）。本体の窓は WM_DROPFILES で受ける
void play_dropped_file(const std::wstring &path)
{
	play_midi_file(GetForegroundWindow(), ui::to_utf8(path.c_str()));
}

void choose_midi_file(HWND hwnd)
{
	wchar_t file[MAX_PATH] = {};
	OPENFILENAMEW o{};
	o.lStructSize = sizeof(o);
	o.hwndOwner = hwnd;
	o.lpstrFilter = L"MIDI ファイル (*.mid;*.midi)\0*.mid;*.midi\0すべて (*.*)\0*.*\0";
	o.lpstrFile = file;
	o.nMaxFile = MAX_PATH;
	o.lpstrTitle = L"流す MIDI ファイル";
	o.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
	if (!GetOpenFileNameW(&o))
		return;

	play_midi_file(hwnd, ui::to_utf8(file));
}

// 覚えている設定を捨てて、電源を入れ直す
void choose_factory_reset(HWND hwnd)
{
	if (!g_win.eng || g_win.eng->state.load() != 1)
		return;
	if (MessageBoxW(hwnd,
	                L"MU2000 を工場出荷状態に戻して、電源を入れ直します。\n"
	                L"ユーティリティの設定や、覚えている音量・音色の設定はすべて消えます。",
	                L"S-MU2000", MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2) != IDOK)
		return;
	g_win.play_file.stop();
	if (g_win.reboot.joinable())
		g_win.reboot.join();
	g_win.reboot = std::thread([] { g_win.eng->factory_reset(); });
}

// 品書きで選ばれたものを開く。開けなかったら「使わない」に戻す
// keep が true なのは起動したとき。開けなくても、覚えていた名前を残す
bool choose_in(int dev, bool keep = false)
{
	if (!g_win.midi)
		return false;
	if (!keep)
		g_win.in_keep.clear();
	std::string err;
	g_win.last_error.clear();
	if (!g_win.midi->open(dev, err)) {
		g_win.last_error = err;
		std::fprintf(stderr, "MIDI 入力: %s\n", err.c_str());
		g_win.midi->open(-1, err);
		dev = -1;
	}
	g_win.in_dev  = g_win.midi->is_open() ? dev : -1;
	g_win.in_name = g_win.midi->device_name();
	save_settings();
	return g_win.last_error.empty();
}

// keep が true なのは起動したとき。開けなくても、覚えていた名前を残す
bool choose_in_b(int dev, bool keep = false)
{
	if (!g_win.midi_b)
		return false;
	if (!keep)
		g_win.in_keep_b.clear();
	std::string err;
	g_win.last_error.clear();
	if (!g_win.midi_b->open(dev, err)) {
		g_win.last_error = err;
		std::fprintf(stderr, "MIDI 入力 B: %s\n", err.c_str());
		g_win.midi_b->open(-1, err);
		dev = -1;
	}
	g_win.in_dev_b  = g_win.midi_b->is_open() ? dev : -1;
	g_win.in_name_b = g_win.midi_b->device_name();
	save_settings();
	return g_win.last_error.empty();
}

// keep が true なのは起動したとき。開けなくても、覚えていた名前を残す
bool choose_out(int dev, bool keep = false)
{
	if (!g_win.mout)
		return false;
	if (!keep)
		g_win.out_keep.clear();
	std::string err;
	g_win.last_error.clear();
	if (!g_win.mout->open(dev, err)) {
		g_win.last_error = err;
		std::fprintf(stderr, "MIDI 出力: %s\n", err.c_str());
		g_win.mout->open(-1, err);
		dev = -1;
	}
	g_win.out_dev  = g_win.mout->is_open() ? dev : -1;
	g_win.out_name = g_win.mout->device_name();
	save_settings();
	return g_win.last_error.empty();
}

// keep が true なのは起動したとき。開けなくても、覚えていた名前を残す
bool choose_out_mu(int dev, bool keep = false)
{
	if (!g_win.mout_mu)
		return false;
	if (!keep)
		g_win.out_keep_mu.clear();
	std::string err;
	g_win.last_error.clear();
	if (!g_win.mout_mu->open(dev, err)) {
		g_win.last_error = err;
		std::fprintf(stderr, "MIDI 出力（本体の OUT）: %s\n", err.c_str());
		g_win.mout_mu->open(-1, err);
		dev = -1;
	}
	g_win.out_dev_mu  = g_win.mout_mu->is_open() ? dev : -1;
	g_win.out_name_mu = g_win.mout_mu->device_name();
	save_settings();
	return g_win.last_error.empty();
}

// keep が true なのは起動したとき。開けなくても、覚えていた名前を残す
bool choose_out_b(int dev, bool keep = false)
{
	if (!g_win.mout_b)
		return false;
	if (!keep)
		g_win.out_keep_b.clear();
	std::string err;
	g_win.last_error.clear();
	if (!g_win.mout_b->open(dev, err)) {
		g_win.last_error = err;
		std::fprintf(stderr, "MIDI 出力 B: %s\n", err.c_str());
		g_win.mout_b->open(-1, err);
		dev = -1;
	}
	g_win.out_dev_b  = g_win.mout_b->is_open() ? dev : -1;
	g_win.out_name_b = g_win.mout_b->device_name();
	save_settings();
	return g_win.last_error.empty();
}

void ensure_backing(HDC dc, int w, int h)
{
	if (g_win.mem_dc && g_win.mem_w == w && g_win.mem_h == h)
		return;
	if (g_win.mem_bmp) DeleteObject(g_win.mem_bmp);
	if (g_win.mem_dc)  DeleteDC(g_win.mem_dc);
	g_win.mem_dc = CreateCompatibleDC(dc);
	g_win.mem_bmp = CreateCompatibleBitmap(dc, w, h);
	SelectObject(g_win.mem_dc, g_win.mem_bmp);
	g_win.mem_w = w;
	g_win.mem_h = h;
}

// キーボードからも押せるように。並びは MAME の mu2000 と同じ
mu2000::button key_to_button(WPARAM vk, bool &ok)
{
	ok = true;
	switch (vk) {
	case 'A': return mu2000::button::play;
	case 'E': return mu2000::button::edit;
	case 'U': return mu2000::button::util;
	case 'F': return mu2000::button::effect;
	case 'S': return mu2000::button::mute_solo;
	case VK_OEM_6: return mu2000::button::part_plus;     // ]
	case VK_OEM_4: return mu2000::button::part_minus;    // [
	case VK_OEM_PLUS:  return mu2000::button::value_plus;
	case VK_OEM_MINUS: return mu2000::button::value_minus;
	case VK_BACK:   return mu2000::button::exit;
	case VK_RETURN: return mu2000::button::enter;
	case VK_OEM_PERIOD: return mu2000::button::select_right;
	case VK_OEM_COMMA:  return mu2000::button::select_left;
	case 'Q': return mu2000::button::seq;
	case 'Z': return mu2000::button::audition;
	case 'X': return mu2000::button::select;
	case 'M': return mu2000::button::sampling_mode;
	default: break;
	}
	ok = false;
	return mu2000::button::count;
}

void open_window(HWND hwnd, ui::pc_window &w)
{
	std::string err;
	if (!w.show(GetModuleHandleA(nullptr), err))
		MessageBoxW(hwnd, ui::to_wide(err).c_str(), L"S-MU2000", MB_OK | MB_ICONWARNING);
}

LRESULT CALLBACK wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp)
{
	switch (msg) {
	case WM_CREATE:
		SetTimer(hwnd, 1, 33, nullptr);        // 30 コマ／秒で描き直す
		return 0;

	case WM_TIMER: {
		// パラメータの層: 音源の返事を読み、見えている面の読み返しを頼む
		if (g_win.br) {
			g_win.panel.tick(*g_win.br);
			g_win.pc.frame(g_win.panel.xg(), g_win.panel.ram(), *g_win.br);
			g_win.list.frame(g_win.panel.xg(), g_win.panel.ram(), *g_win.br);
			g_win.fx.frame(g_win.panel.xg(), g_win.panel.ram(), *g_win.br);
			g_win.shapes.frame(g_win.panel.xg(), g_win.panel.ram(), *g_win.br);
			// 一覧でインサーションの欄をダブルクリックされたら、設定の窓を出す
			if (ui::xgui::take_fx_request())
				open_window(hwnd, g_win.fx);
			// 一覧で VIB・FILTER・EG・EQ の絵をダブルクリックされたら、パートの音色の窓を出す
			if (ui::xgui::take_part_request())
				open_window(hwnd, g_win.shapes);
		}
		InvalidateRect(hwnd, nullptr, FALSE);
		// SmartMedia に書いたものを 2 秒ごとにファイルへ書き戻す（抜いたとき・閉じたときも）
		static DWORD last_card = 0;
		if (GetTickCount() - last_card > 2000) {
			last_card = GetTickCount();
			flush_card();
		}
		// MIDI の輪などで溢れて捨てたものがあれば、1 秒に 1 回だけ知らせる
		static DWORD last = 0;
		if (g_win.eng && GetTickCount() - last > 1000) {
			last = GetTickCount();
			const u64 drops = g_win.eng->guard_a.dropped() + g_win.eng->guard_b.dropped() +
			                  g_win.eng->mu.midi_dropped();
			if (drops != g_win.reported_drops) {
				std::fprintf(stderr,
				             "MIDI が多すぎるので捨てた: THRU A %llu / THRU B %llu / 受信 %llu バイト"
				             "（MIDI の輪ができていないか確かめる）\n",
				             (unsigned long long)g_win.eng->guard_a.dropped(),
				             (unsigned long long)g_win.eng->guard_b.dropped(),
				             (unsigned long long)g_win.eng->mu.midi_dropped());
				g_win.reported_drops = drops;
			}
		}
		return 0;
	}

	case WM_DROPFILES: {
		// 窓に落とされたファイルの 1 つ目を流す
		const HDROP drop = HDROP(wp);
		wchar_t path[MAX_PATH * 4] = {};
		const bool got = DragQueryFileW(drop, 0, path, UINT(sizeof(path) / sizeof(path[0]))) > 0;
		DragFinish(drop);
		if (got)
			play_midi_file(hwnd, ui::to_utf8(path));
		return 0;
	}

	case WM_SIZE:
		g_win.panel.resize(LOWORD(lp), HIWORD(lp));
		InvalidateRect(hwnd, nullptr, FALSE);
		return 0;

	case WM_ERASEBKGND:
		return 1;                               // 全部自分で描く

	case WM_PAINT: {
		PAINTSTRUCT ps;
		HDC dc = BeginPaint(hwnd, &ps);
		RECT cr;
		GetClientRect(hwnd, &cr);
		const int w = cr.right, h = cr.bottom;
		ensure_backing(dc, w, h);

		ui::snapshot s;
		g_win.br->read(s);
		u64 pressed = g_win.br->buttons();
		char status[128] = {};
		if (g_win.out && g_win.out->produced())
			std::snprintf(status, sizeof(status),
			              "CPU %.0f%%  最悪 %.1f ms  待ち %.0f ms  遅れ %llu   IN: %s   OUT: %s"
			              "   （MIDI IN A のジャックか右クリックで口を選ぶ）",
			              g_win.out->cpu_percent(), g_win.out->worst_ms(),
			              g_win.out->output_ms(),
			              (unsigned long long)g_win.out->late(),
			              g_win.in_name.empty()  ? "なし" : g_win.in_name.c_str(),
			              g_win.out_name.empty() ? "なし" : g_win.out_name.c_str());
		else
			std::snprintf(status, sizeof(status), "起動中...");
		g_win.panel.set_volume(g_win.br->gain());
		g_win.panel.paint(g_win.mem_dc, s, pressed, status);

		BitBlt(dc, 0, 0, w, h, g_win.mem_dc, 0, 0, SRCCOPY);
		EndPaint(hwnd, &ps);
		return 0;
	}

	case WM_LBUTTONDOWN: {
		if (g_win.lcd_only)
			return 0;
		const int mx = GET_X_LPARAM(lp), my = GET_Y_LPARAM(lp);
		// パネルの MIDI IN A のジャックを押したら、口を選ぶ品書きを出す
		if (g_win.panel.on_midi_jack(mx, my)) {
			POINT pt{ mx, my };
			ClientToScreen(hwnd, &pt);
			show_port_menu(hwnd, pt);
			return 0;
		}
		// A/D INPUT のジャックは録音デバイス
		if (g_win.panel.on_ad_input(mx, my)) {
			POINT pt{ mx, my };
			ClientToScreen(hwnd, &pt);
			show_ain_menu(hwnd, pt);
			return 0;
		}
		// カードの差し込み口は MIDI ファイル
		if (g_win.panel.on_card_slot(mx, my)) {
			POINT pt{ mx, my };
			ClientToScreen(hwnd, &pt);
			show_card_menu(hwnd, pt);
			return 0;
		}
		SetCapture(hwnd);
		if (g_win.panel.press(mx, my, *g_win.br))
			InvalidateRect(hwnd, nullptr, FALSE);
		return 0;
	}

	case WM_RBUTTONUP: {
		if (g_win.lcd_only)
			return 0;
		const int mx = GET_X_LPARAM(lp), my = GET_Y_LPARAM(lp);
		POINT pt{ mx, my };
		ClientToScreen(hwnd, &pt);
		if (g_win.panel.on_card_slot(mx, my))
			show_card_menu(hwnd, pt);
		else
			show_port_menu(hwnd, pt);
		return 0;
	}

	case WM_COMMAND: {
		const UINT id = LOWORD(wp);
		g_win.last_error.clear();
		if (id == ID_IN_NONE)            choose_in(-1);
		else if (id >= ID_IN_BASE  && id < ID_IN_BASE + 256)  choose_in(int(id - ID_IN_BASE));
		else if (id == ID_INB_NONE)      choose_in_b(-1);
		else if (id >= ID_INB_BASE && id < ID_INB_BASE + 256) choose_in_b(int(id - ID_INB_BASE));
		else if (id == ID_OUT_NONE)      choose_out(-1);
		else if (id >= ID_OUT_BASE && id < ID_OUT_BASE + 256) choose_out(int(id - ID_OUT_BASE));
		else if (id == ID_OUTB_NONE)     choose_out_b(-1);
		else if (id >= ID_OUTB_BASE && id < ID_OUTB_BASE + 256) choose_out_b(int(id - ID_OUTB_BASE));
		else if (id == ID_OUTMU_NONE)    choose_out_mu(-1);
		else if (id >= ID_OUTMU_BASE && id < ID_OUTMU_BASE + 256) choose_out_mu(int(id - ID_OUTMU_BASE));
		else if (id == ID_AIN_NONE)      choose_ain(-1);
		else if (id >= ID_AIN_BASE && id < ID_AIN_BASE + 256) choose_ain(int(id - ID_AIN_BASE));
		else if (id >= ID_CARD_NEW16 && id <= ID_CARD_NEW128) new_card(hwnd, 16u << (id - ID_CARD_NEW16));
		else if (id == ID_CARD_OPEN) {
			const std::string path = ask_card_path(hwnd, false);
			if (!path.empty() && insert_card(path))
				save_settings();
		}
		else if (id == ID_CARD_EJECT) { eject_card(); save_settings(); }
		else if (id == ID_PLAY_FILE) choose_midi_file(hwnd);
		else if (id == ID_STOP_FILE) g_win.play_file.stop();
		else if (id == ID_PORTS34_FOLD || id == ID_PORTS34_DROP) {
			g_win.play_file.set_fold_extra_ports(id == ID_PORTS34_FOLD);
			save_settings();
		}
		else if (id == ID_FACTORY) choose_factory_reset(hwnd);
		else if (id == ID_PC_EDITOR) open_window(hwnd, g_win.pc);
		else if (id == ID_OVERVIEW) open_window(hwnd, g_win.list);
		if (!g_win.last_error.empty()) {
			const std::wstring w = ui::to_wide(g_win.last_error);
			MessageBoxW(hwnd, w.c_str(), L"S-MU2000", MB_OK | MB_ICONWARNING);
			g_win.last_error.clear();
		}
		InvalidateRect(hwnd, nullptr, FALSE);
		return 0;
	}

	case WM_SETCURSOR: {
		// ジャックの上では指の形にして、押せることを見せる
		POINT pt;
		GetCursorPos(&pt);
		ScreenToClient(hwnd, &pt);
		if (LOWORD(lp) == HTCLIENT &&
		    (g_win.panel.on_midi_jack(pt.x, pt.y) ||
		     g_win.panel.on_ad_input(pt.x, pt.y) ||
		     g_win.panel.on_card_slot(pt.x, pt.y))) {
			SetCursor(LoadCursor(nullptr, IDC_HAND));
			return TRUE;
		}
		break;
	}

	case WM_MOUSEMOVE:
		if (g_win.lcd_only)
			return 0;
		if (g_win.panel.drag(GET_X_LPARAM(lp), GET_Y_LPARAM(lp), *g_win.br))
			InvalidateRect(hwnd, nullptr, FALSE);
		return 0;

	case WM_LBUTTONUP:
		if (g_win.lcd_only)
			return 0;
		g_win.panel.release(*g_win.br);
		ReleaseCapture();
		InvalidateRect(hwnd, nullptr, FALSE);
		return 0;

	case WM_MOUSEWHEEL: {
		if (g_win.lcd_only)
			return 0;
		POINT pt{ GET_X_LPARAM(lp), GET_Y_LPARAM(lp) };
		ScreenToClient(hwnd, &pt);
		const int delta = GET_WHEEL_DELTA_WPARAM(wp) / WHEEL_DELTA;
		if (delta && g_win.panel.wheel_at(pt.x, pt.y, delta, *g_win.br))
			InvalidateRect(hwnd, nullptr, FALSE);
		return 0;
	}

	case WM_KEYDOWN: {
		if (g_win.lcd_only)
			return 0;
		if (lp & (1 << 30))                     // 押しっぱなしの繰り返しは無視
			return 0;
		if (wp == VK_F2) {                      // PC エディタ
			open_window(hwnd, g_win.pc);
			return 0;
		}
		if (wp == VK_F3) {                      // 一覧
			open_window(hwnd, g_win.list);
			return 0;
		}
		if (wp == VK_F5) {                      // 配置を読み直す
			apply_layout(g_win.layout_path, false);
			InvalidateRect(hwnd, nullptr, FALSE);
			return 0;
		}
		bool ok = false;
		const mu2000::button b = key_to_button(wp, ok);
		if (ok) g_win.br->press(b, true);
		return 0;
	}

	case WM_KEYUP: {
		bool ok = false;
		const mu2000::button b = key_to_button(wp, ok);
		if (ok) g_win.br->press(b, false);
		return 0;
	}

	case WM_KILLFOCUS:
		g_win.br->release_all();                // 窓から離れたら全部離す
		return 0;

	case WM_DESTROY:
		PostQuitMessage(0);
		return 0;
	}
	return DefWindowProcA(hwnd, msg, wp, lp);
}


// ---- 窓を出さずに絵だけ書き出す。見た目を直すときに使う

int shot(const std::string &path, int w, int h, ui::bridge &br, bool grid,
         bool lcd_only, const std::string &layout_path)
{
	ui::panel p;
	std::string lerr;
	if (!layout_path.empty() && !p.lay().load(layout_path, lerr))
		std::fprintf(stderr, "配置: %s を開けない\n", layout_path.c_str());
	if (!lerr.empty())
		std::fprintf(stderr, "%s", lerr.c_str());
	p.set_lcd_only(lcd_only);
	p.resize(w, h);
	p.set_grid(grid);

	BITMAPINFO bi{};
	bi.bmiHeader.biSize = sizeof(bi.bmiHeader);
	bi.bmiHeader.biWidth = w;
	bi.bmiHeader.biHeight = -h;                 // 上から下へ
	bi.bmiHeader.biPlanes = 1;
	bi.bmiHeader.biBitCount = 32;
	bi.bmiHeader.biCompression = BI_RGB;

	void *bits = nullptr;
	HDC screen = GetDC(nullptr);
	HDC dc = CreateCompatibleDC(screen);
	HBITMAP bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &bits, nullptr, 0);
	SelectObject(dc, bmp);

	ui::snapshot s;
	br.read(s);
	p.set_volume(0.8);
	p.paint(dc, s, 0, "");
	GdiFlush();

	const bool ok = ui::write_png(path, static_cast<const u8 *>(bits), w, h, w * 4);

	DeleteObject(bmp);
	DeleteDC(dc);
	ReleaseDC(nullptr, screen);

	std::printf(ok ? "書き出した: %s（%d×%d）\n" : "書き出せない: %s\n", path.c_str(), w, h);
	return ok ? 0 : 1;
}

} // namespace


int main(int argc, char **argv)
{
	SetConsoleOutputCP(CP_UTF8);

	std::string dir, shot_path;
	int midi_dev = -2;                 // -2 未指定（覚えているものを使う）/ -1 使わない
	int midib_dev = -2;                // MIDI IN B
	int moutb_dev = -2;                // MIDI OUT B
	int mout_dev = -2;
	int moutmu_dev = -2;               // MIDI OUT（本体）
	int latency = 20;        // 溜める目標
	bool exclusive = false;
	const char *audio_dev = nullptr;
	bool factory = false;
	bool open_editor = false;          // 起動したら PC エディタも出す
	bool open_list = false;            // 起動したら一覧も出す
	bool open_fx = false;              // 起動したらインサーションの設定の窓も出す
	bool open_shapes = false;          // 起動したらパートの音色の窓も出す
	int win_w = 1000, win_h = 400;   // パネルの論理寸法（1000 × 400）と同じ比
	bool size_given = false;
	bool lcd_only = false;
	bool fast_midi = false;
	bool grid = false;
	std::string layout_path, dump_layout, play_path;
	bool boot_for_shot = false;
	std::string shot_mid;
	double shot_secs = 0.0;

	for (int i = 1; i < argc; i++) {
		if (!std::strcmp(argv[i], "--list")) {
			const auto ins = ui::midi_in::list();
			std::printf("MIDI 入力（--midi 番号 / 画面からも選べる）:\n");
			for (size_t k = 0; k < ins.size(); k++)
				std::printf("  %zu: %s\n", k, ins[k].c_str());
			if (ins.empty())
				std::printf("  （なし）\n");
			const auto outs = ui::midi_out::list();
			std::printf("MIDI 出力（--midiout 番号 / 受けたものをそのまま外へ）:\n");
			for (size_t k = 0; k < outs.size(); k++)
				std::printf("  %zu: %s\n", k, outs[k].c_str());
			if (outs.empty())
				std::printf("  （なし）\n");
			const auto aouts = ui::audio_out::list();
			std::printf("音声の出口（--audio に名前の一部）:\n");
			for (size_t k = 0; k < aouts.size(); k++)
				std::printf("  %zu: %s\n", k, aouts[k].c_str());
			return 0;
		}
		else if (!std::strcmp(argv[i], "--midi") && i + 1 < argc) midi_dev = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--midi-b") && i + 1 < argc) midib_dev = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--midiout") && i + 1 < argc) mout_dev = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--midiout-b") && i + 1 < argc) moutb_dev = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--midiout-mu") && i + 1 < argc) moutmu_dev = std::atoi(argv[++i]);
		// 入口も出口も開かない。試しに動かすとき、覚えている THRU の先（実機）へ
		// 流れないように。覚えている口は書き換えない
		else if (!std::strcmp(argv[i], "--nomidi")) {
			midi_dev = midib_dev = mout_dev = moutb_dev = moutmu_dev = -1;
			g_win.keep_settings = true;
		}
		else if (!std::strcmp(argv[i], "--latency") && i + 1 < argc) latency = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--exclusive")) exclusive = true;
		else if (!std::strcmp(argv[i], "--audio") && i + 1 < argc) audio_dev = argv[++i];
		else if (!std::strcmp(argv[i], "--factory")) factory = true;
		else if (!std::strcmp(argv[i], "--editor")) open_editor = true;
		else if (!std::strcmp(argv[i], "--list-window")) open_list = true;
		else if (!std::strcmp(argv[i], "--fx-window")) open_fx = true;
		else if (!std::strcmp(argv[i], "--shapes-window")) open_shapes = true;
		else if (!std::strcmp(argv[i], "--lcd")) lcd_only = true;
		else if (!std::strcmp(argv[i], "--fast-midi")) fast_midi = true;
		else if (!std::strcmp(argv[i], "--shot") && i + 1 < argc) shot_path = argv[++i];
		else if (!std::strcmp(argv[i], "--boot")) boot_for_shot = true;
		else if (!std::strcmp(argv[i], "--grid")) grid = true;
		else if (!std::strcmp(argv[i], "--layout") && i + 1 < argc) layout_path = argv[++i];
		else if (!std::strcmp(argv[i], "--play") && i + 1 < argc) play_path = argv[++i];
		else if (!std::strcmp(argv[i], "--dump-layout") && i + 1 < argc) dump_layout = argv[++i];
		else if (!std::strcmp(argv[i], "--mid") && i + 2 < argc) {
			shot_mid = argv[++i];
			shot_secs = std::atof(argv[++i]);
			boot_for_shot = true;
		}
		else if (!std::strcmp(argv[i], "--size") && i + 1 < argc) {
			if (std::sscanf(argv[++i], "%dx%d", &win_w, &win_h) != 2) { win_w = 1000; win_h = 400; }
			size_given = true;
		}
		else if (dir.empty()) dir = argv[i];
	}
	if (lcd_only && !size_given) {
		win_w = 898;
		win_h = 290;
	}

	// --layout が無ければ、決まった場所を順に探す
	if (layout_path.empty())
		layout_path = ui::layout::find_default();

	if (!dump_layout.empty()) {
		ui::layout l;
		std::string lerr;
		if (!layout_path.empty())
			l.load(layout_path, lerr);
		if (!l.save(dump_layout)) {
			std::fprintf(stderr, "%s に書けない\n", dump_layout.c_str());
			return 1;
		}
		std::printf("いまの配置を書き出した: %s\n", dump_layout.c_str());
		std::printf("直したら --layout で渡すか、窓で F5 を押す\n");
		return 0;
	}

	static ui::bridge br;
	static ui::midi_in  midi, midi_b;
	static ui::midi_out mout, mout_b, mout_mu;

	// 絵だけ欲しい場合。ROM が無くても中身が空の画面は出せる
	if (!shot_path.empty() && (dir.empty() || !boot_for_shot)) {
		ui::snapshot s;
		std::snprintf(s.message, sizeof(s.message), "S-MU2000");
		br.publish(s);
		return shot(shot_path, win_w, win_h, br, grid, lcd_only, layout_path);
	}

	if (dir.empty()) {
		std::fprintf(stderr,
			"使い方: gui <rom ディレクトリ> [--midi 番号] [--midi-b 番号]"
			" [--midiout 番号] [--midiout-b 番号] [--midiout-mu 番号]"
			" [--latency ミリ秒] [--exclusive] [--layout panel.txt] [--play 曲.mid] [--lcd] [--fast-midi]\n"
			"        [--factory]   覚えている設定を捨てて工場出荷状態で起動する\n"
			"        [--editor]    PC エディタも開く（窓では F2 か右クリック）\n"
			"        [--list-window] 一覧の窓も開く（窓では F3 か右クリック）\n"
			"        [--fx-window] インサーションの設定の窓も開く（一覧でインサーションの欄をダブルクリック）\n"
			"        [--shapes-window] パートの音色の窓も開く（一覧で VIB などの絵をダブルクリック）\n"
			"        gui --dump-layout panel.txt   いまの配置を書き出す\n"
			"        gui --list\n"
			"        gui [<rom ディレクトリ> --boot] --shot 絵.png [--size 1000x400]\n");
		return 1;
	}

	static engine eng(br, midi);
	eng.mu.set_fast_midi(fast_midi);
	eng.midi_b = &midi_b;
	eng.mout_b = &mout_b;
	eng.mout_mu = &mout_mu;
	eng.mout = &mout;
	if (!eng.load(dir)) {
		std::fprintf(stderr, "%s\n", eng.message.c_str());
		return 1;
	}
	// 一覧の窓で、音色の名前と楽器の絵を利用者の ROM から読む（xg/voices.h）
	ui::xgui::set_voice_rom(eng.mu.program_rom());

	// 絵だけ、ただし起動後の LCD が欲しい場合
	if (!shot_path.empty()) {
		if (!eng.boot()) { std::fprintf(stderr, "%s\n", eng.message.c_str()); return 1; }
		eng.state.store(1);

		// 起動直後は表示が動いている途中。少し空回しして落ち着かせる
		{
			s32 l, r;
			for (size_t i = 0; i < size_t(2.0 * RATE); i++)
				eng.mu.run_sample(l, r);
		}

		// レベルメータを出したいので、指定があれば MIDI を流しておく
		if (!shot_mid.empty()) {
			std::vector<smf::event> evs;
			std::string err;
			if (!smf::load(shot_mid, evs, err)) {
				std::fprintf(stderr, "%s\n", err.c_str());
			} else {
				std::printf("MIDI %zu 件を %.1f 秒ぶん流す\n", evs.size(), shot_secs);
				size_t at = 0;
				s32 l, r;
				for (size_t i = 0; i < size_t(shot_secs * RATE); i++) {
					const double now = double(i) / RATE;
					while (at < evs.size() && evs[at].time <= now) {
						for (u8 b : evs[at].bytes)
							eng.mu.midi_in(b);
						at++;
					}
					eng.mu.run_sample(l, r);
				}
			}
		}

		eng.publish();
		return shot(shot_path, win_w, win_h, br, grid, lcd_only, layout_path);
	}

	// ---- 窓を出す

	const HINSTANCE inst = GetModuleHandleA(nullptr);
	WNDCLASSA wc{};
	wc.lpfnWndProc   = wnd_proc;
	wc.hInstance     = inst;
	wc.hCursor       = LoadCursor(nullptr, IDC_ARROW);
	wc.lpszClassName = "SMU2000Panel";
	wc.hbrBackground = nullptr;
	RegisterClassA(&wc);

	RECT want{ 0, 0, win_w, win_h };
	AdjustWindowRect(&want, WS_OVERLAPPEDWINDOW, FALSE);
	HWND hwnd = CreateWindowA("SMU2000Panel", "S-MU2000", WS_OVERLAPPEDWINDOW,
	                          CW_USEDEFAULT, CW_USEDEFAULT,
	                          want.right - want.left, want.bottom - want.top,
	                          nullptr, nullptr, inst, nullptr);
	if (!hwnd) {
		std::fprintf(stderr, "窓を出せない\n");
		return 1;
	}

	// MIDI ファイルを窓に落とせば流す（本体の窓も、エディタや一覧の窓も）
	DragAcceptFiles(hwnd, TRUE);
	ui::pc_window::set_drop_handler(play_dropped_file);

	g_win.br   = &br;
	g_win.eng  = &eng;
	g_win.lcd_only = lcd_only;
	g_win.panel.set_lcd_only(lcd_only);
	// 窓を出すときだけ、覚えている設定で起動する（--shot は毎回同じ絵にしたい）
	eng.use_nvram = !factory;
	if (factory)
		std::printf("工場出荷状態で起動する（覚えていた設定は終わるときに上書きされる）\n");
	g_win.layout_path = layout_path;
	g_win.midi   = &midi;
	g_win.midi_b = &midi_b;
	g_win.mout   = &mout;
	g_win.mout_b = &mout_b;
	g_win.mout_mu = &mout_mu;
	g_win.panel.resize(win_w, win_h);
	apply_layout(layout_path, false);
	g_win.panel.resize(win_w, win_h);
	{
		// VOLUME のつまみは前に閉じたときの位置から
		std::string a, b, c, d, e;
		float volume = 1.0f;
		bool fold34 = true;
		load_settings(a, b, c, d, e, &volume, nullptr, &fold34);
		br.set_gain(volume);
		g_win.play_file.set_fold_extra_ports(fold34);
	}

	eng.publish();
	ShowWindow(hwnd, SW_SHOW);
	if (open_editor && !lcd_only)
		open_window(hwnd, g_win.pc);
	if (open_fx && !lcd_only)
		open_window(hwnd, g_win.fx);
	if (open_list && !lcd_only)
		open_window(hwnd, g_win.list);
	if (open_shapes && !lcd_only)
		open_window(hwnd, g_win.shapes);
	UpdateWindow(hwnd);

	// 起動は別スレッド。終わったら音を出し始める
	static ui::audio_out out;
	g_win.out = &out;
	static ui::audio_in ain;
	g_win.ain = &ain;
	eng.ain = &ain;
	std::thread boot_thread([&] {
		if (!eng.boot()) {
			eng.state.store(2);
			eng.publish();
			return;
		}
		eng.state.store(1);
		eng.publish();

		// 前に選んだ口を名前で探す。--midi / --midiout があればそちらが勝つ
		std::string want_in, want_in_b, want_out, want_out_b, want_audio, want_out_mu;
		std::string want_ain, want_card;
		load_settings(want_in, want_in_b, want_out, want_out_b, want_audio, nullptr, &want_out_mu, nullptr, &want_ain, &want_card);
		g_win.ain_name = want_ain;
		// 前に差していた SmartMedia。ファイルが無くなっていたら差さない（覚えている名前も消える）
		if (!want_card.empty())
			insert_card(want_card, true);
		// --audio があればそちらが勝つ。無ければ前に選んだもの
		g_win.audio_name = audio_dev ? std::string(audio_dev) : want_audio;
		if (midi_dev == -2)
			midi_dev = find_device(ui::midi_in::list(), want_in);
		if (midib_dev == -2)
			midib_dev = find_device(ui::midi_in::list(), want_in_b);
		if (mout_dev == -2)
			mout_dev = find_device(ui::midi_out::list(), want_out);
		if (moutb_dev == -2)
			moutb_dev = find_device(ui::midi_out::list(), want_out_b);
		if (moutmu_dev == -2)
			moutmu_dev = find_device(ui::midi_out::list(), want_out_mu);

		g_win.in_keep     = want_in;
		g_win.in_keep_b   = want_in_b;
		g_win.out_keep    = want_out;
		g_win.out_keep_b  = want_out_b;
		g_win.out_keep_mu = want_out_mu;
		choose_in(midi_dev, true);
		choose_in_b(midib_dev, true);
		choose_out(mout_dev, true);
		choose_out_b(moutb_dev, true);
		choose_out_mu(moutmu_dev, true);
		// 開けなかった口は、覚えていた名前も出す（選び直すまで覚えている）
		auto show = [](const char *label, const std::string &now, const std::string &keep) {
			if (!now.empty())
				std::printf("%s: %s\n", label, now.c_str());
			else if (!keep.empty())
				std::printf("%s: なし（「%s」が見つからないか開けない。覚えたままにしてある）\n",
				            label, keep.c_str());
			else
				std::printf("%s: なし\n", label);
		};
		show("MIDI IN A",   g_win.in_name,     g_win.in_keep);
		show("MIDI IN B",   g_win.in_name_b,   g_win.in_keep_b);
		show("MIDI OUT",    g_win.out_name_mu, g_win.out_keep_mu);
		show("MIDI THRU A", g_win.out_name,    g_win.out_keep);
		show("MIDI THRU B", g_win.out_name_b,  g_win.out_keep_b);
		std::fflush(stdout);

		std::string err;

		if (!out.start(latency, [](s16 *o, u32 n) { eng.fill(o, n); }, err, exclusive,
		               g_win.audio_name)) {
			std::fprintf(stderr, "音声: %s\n", err.c_str());
			eng.message = "音声デバイスを開けない";
			eng.state.store(2);
			eng.publish();
			return;
		}
		// 開けた出口を覚える。**設定を読んで MIDI の口を開いた後でないと
		// いけない**。前はこれを起動直後にやっていて、まだ空の MIDI の名前で
		// gui.ini を上書きしていた（毎回 MIDI が「なし」に戻っていた）
		g_win.audio_name = out.device_name();
		std::printf("音声の出口: %s\n%s\n", out.device_name().c_str(),
		            out.format_line().c_str());
		// A/D INPUT。前に選んだ録音デバイスがあれば開く（開けなくても名前は覚えておく）
		if (!g_win.ain_name.empty()) {
			std::string aerr;
			if (ain.start(g_win.ain_name, aerr))
				std::printf("A/D INPUT: %s（%s）\n", ain.device_name().c_str(), ain.format_line().c_str());
			else
				std::printf("A/D INPUT: なし（%s）\n", aerr.c_str());
		}
		save_settings();
		// --play が付いていれば、鳴り始めたところで流し出す
		if (!play_path.empty()) {
			std::string perr;
			if (!g_win.play_file.start(play_path, br, perr))
				std::fprintf(stderr, "MIDI ファイル: %s\n", perr.c_str());
			else
				std::printf("再生: %s（%.1f 秒）\n", play_path.c_str(),
				            g_win.play_file.length());
		}
		std::printf("鳴らしている（待ち時間 %.1f ms、MMCSS %s）\n",
		            1000.0 * out.buffer_frames() / RATE,
		            out.mmcss() ? "登録できた" : "登録できない（途切れやすい）");
		std::fflush(stdout);
	});

	MSG msg;
	while (GetMessageA(&msg, nullptr, 0, 0) > 0) {
		TranslateMessage(&msg);
		DispatchMessageA(&msg);
	}

	// PC の窓に閉じたと知らせる（一覧のミュートを外して受信チャンネルを戻すなど）。
	// 送ったものは音声の糸が流すので、少し待ってから止める
	if (g_win.br) {
		g_win.list.shutdown(*g_win.br);
		g_win.pc.shutdown(*g_win.br);
		g_win.fx.shutdown(*g_win.br);
		g_win.shapes.shutdown(*g_win.br);
		Sleep(100);
	}

	// **先に MIDI ファイルを止める。** 止めたときのオールノートオフは音声の糸が THRU から
	// 外へ流すので、音を先に止めると外の機器（実機）に届かず鳴りっぱなしになる。
	// 止めてから、音声の糸が流し終えるのを少し待つ
	if (g_win.play_file.playing()) {
		g_win.play_file.stop();
		Sleep(150);
	}
	out.stop();
	// 念のため、THRU の先へ直にもオールサウンドオフ・オールノートオフを送る。
	// 音声の糸はもう止まっているので、ここから送っても取り合いにならない
	for (ui::midi_out *thru : { &mout, &mout_b }) {
		if (!thru->is_open())
			continue;
		for (int ch = 0; ch < 16; ch++) {
			for (u8 v : { u8(0xb0 | ch), u8(120), u8(0), u8(0xb0 | ch), u8(123), u8(0) })
				thru->send(v);
		}
	}
	if (boot_thread.joinable())
		boot_thread.join();
	if (g_win.reboot.joinable())
		g_win.reboot.join();
	flush_card();      // 音はもう止まっている。SmartMedia に書いたものを残す
	save_settings();   // VOLUME のつまみの位置
	// 音はもう止まっている。起動できていたときだけ残す
	if (eng.state.load() == 1 && !smu2000::nvram::save(eng.mu))
		std::fprintf(stderr, "設定を残せなかった: %s\n", smu2000::nvram::path(eng.mu).c_str());
	g_win.play_file.stop();
	midi.close();
	mout.close();
	mout_mu.close();
	midi_b.close();
	mout_b.close();
	ain.stop();

	if (out.produced())
		std::printf("CPU %.1f%%、1 回の最悪 %.2f ms、間に合わなかった %llu 回\n",
		            out.cpu_percent(), out.worst_ms(),
		            (unsigned long long)out.late());
		std::printf("%s\n%s\n", out.format_line().c_str(), out.latency_line().c_str());
	return 0;
}
