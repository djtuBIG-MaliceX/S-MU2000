// license:BSD-3-Clause
//
// The Mac front end's app globals; the class is ui/app_mac.h. Windows' and
// Linux's twins are ui/app_win.cpp and ui/app_linux.cpp.

#include "app_mac.h"

#include "compat/paths.h"

namespace ui {

gui_app *g_gui = nullptr;

void play_dropped_file(const std::string &path)
{
	if (g_gui)
		g_gui->file_dropped(path);
}

std::string settings_file_path()
{
	const std::string dir = smu2000::ensure_config_dir();
	return dir.empty() ? std::string() : dir + "gui.ini";
}

} // namespace ui
