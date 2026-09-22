// license:BSD-3-Clause
//
// Display strings for the GDI panel pages, in the author's Japanese by default.
//
// gui.exe and the macOS front end show these exact strings, so the defaults
// below are the source of truth: changing one changes every platform at once.
// A front end that shows another language installs its own table once at
// startup (the Linux one installs English in main()); nothing else in shared
// code changes, and a front end that installs nothing keeps showing exactly
// what it always did.
//
// Header-only on purpose: no build file on any platform needs a new source.

#ifndef S_MU2000_UI_TEXTS_H
#define S_MU2000_UI_TEXTS_H

#pragma once

namespace ui {

struct ui_texts {
	// Bottom strip tabs (panel.cpp)
	const char *tab_panel;
	const char *tab_editor;
	const char *tab_effects;
	// Front page hint (panel.cpp)
	const char *hint_front;
	// Editor page (editor.cpp)
	const char *editor_hint;         // may contain \n
	const char *editor_xg_reset;
	const char *editor_all_off;
	// Effects page (effects.cpp)
	const char *effects_title;
	const char *effects_values_note;
	const char *effects_insertion_note;   // may contain \n
	// panel.txt parse errors (layout.cpp; printf format taking %d and %s)
	const char *layout_line_error;
	// Engine boot log (engine.h, bootcache.h; shown on the terminal).
	// The *_fmt entries are printf formats taking the shown arguments.
	const char *engine_warn_fmt;          // %s: a ROM file problem
	const char *engine_nvram_fmt;         // %s: settings file in use
	const char *engine_boot_cached_fmt;   // %s: snapshot path
	const char *engine_boot_saved_fmt;    // %s: snapshot path
	const char *engine_boot_failed;       // shown in the window's message area
	const char *engine_resetting;         // shown in the window's message area
	const char *engine_reset_done;
	const char *bootcache_read_error_fmt; // %s
};

inline const ui_texts &default_texts()
{
	static const ui_texts t{
		"パネル",
		"エディタ",
		"エフェクト",
		"大きなダイヤルはホイールで回す ／ ボタンはクリック ／ "
		"キー: A=PLAY E=EDIT U=UTIL F=EFFECT [ ]=PART",
		"つまみは上下にドラッグ、またはホイール。\n"
		"送っているのは XG のパラメータチェンジ。\n"
		"値は MU2000 に問い合わせて読み返している。",
		"XG リセット",
		"オールノートオフ",
		"エフェクト（送っているのは XG のパラメータチェンジ）",
		"値は MU2000 に問い合わせて読み返している。パネルや曲で変えたものもここに出る。",
		"インサーションは掛けたいパートを選ぶと働く。バリエーションは\n"
		"CONNECT を INSERTION にするとインサーションとして使える。",
		"%d 行目: %s\n",
		"警告: %s\n",
		"設定: %s\n",
		"起動: 前の写しから（%s）\n",
		"起動の写しを残した: %s\n",
		"起動しなかった",
		"工場出荷状態に戻している...",
		"工場出荷状態に戻した\n",
		"起動の写しを読めない: %s\n",
	};
	return t;
}

inline const ui_texts &english_texts()
{
	static const ui_texts t{
		"Panel",
		"Editor",
		"Effects",
		"Turn the big dial with the wheel / click buttons / "
		"keys: A=PLAY E=EDIT U=UTIL F=EFFECT [ ]=PART",
		"Drag knobs up/down, or use the wheel.\n"
		"Sends XG parameter changes.\n"
		"Values are read back from the MU2000.",
		"XG reset",
		"All notes off",
		"Effects (sending XG parameter changes)",
		"Values are read back from the MU2000. Changes from the panel or songs appear here too.",
		"Insertion works on the selected part. Variation can be used as insertion\n"
		"by setting CONNECT to INSERTION.",
		"line %d: %s\n",
		"warning: %s\n",
		"Settings: %s\n",
		"Booted from snapshot (%s)\n",
		"Saved boot snapshot: %s\n",
		"Boot failed",
		"Factory resetting...",
		"Factory reset done\n",
		"Cannot read boot snapshot: %s\n",
	};
	return t;
}

inline const ui_texts *&text_override()
{
	static const ui_texts *p = nullptr;
	return p;
}

inline const ui_texts &texts()
{
	const ui_texts *p = text_override();
	return p ? *p : default_texts();
}

// Install another table. It must outlive the process (the built-in ones do).
// Never called on Windows or macOS, so those platforms cannot drift.
inline void set_texts(const ui_texts &t)
{
	text_override() = &t;
}

} // namespace ui

#endif // S_MU2000_UI_TEXTS_H
