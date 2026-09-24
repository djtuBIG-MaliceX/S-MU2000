// license:BSD-3-Clause
//
// The Windows front end's app globals; the class is ui/app_win.h. The
// Mac's twin is ui/app_mac.cpp.

#include "app_win.h"

#include "ui/midi_in.h"
#include "ui/midi_out.h"

namespace ui {

// ---- 選んだ口を覚えておく
//
// 番号ではなく**名前**で覚える。USB の機器を挿し直すと番号がずれるので、
// 番号で覚えると次に開いたとき別の機器に繋がってしまう。

std::string settings_file_path()
{
	const char *base = std::getenv("LOCALAPPDATA");
	if (!base || !*base)
		return {};
	std::string dir = std::string(base) + "\\S-MU2000";
	CreateDirectoryA(dir.c_str(), nullptr);
	return dir + "\\gui.ini";
}

win_app *g_win = nullptr;

void play_dropped_file(const std::string &path)
{
	// Outside a menu command, so a failure shows straight away rather than
	// through last_error at the end of WM_COMMAND. What a drop means is
	// the app's (ui::app::file_dropped)
	g_win->file_dropped(path);
	if (!g_win->last_error.empty()) {
		win_error(GetForegroundWindow(), g_win->last_error);
		g_win->last_error.clear();
	}
}

} // namespace ui
