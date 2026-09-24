// license:BSD-3-Clause
//
// The Linux front end's app globals; the class is ui/app_linux.h. The
// Windows and macOS twins are ui/app_win.cpp and ui/app_mac.cpp.

#include "app_linux.h"

#include "compat/paths.h"

#include "ui/midi_in.h"
#include "ui/midi_out.h"

namespace ui {

// gui.ini lives next to the other settings (compat/paths.h), with the same
// key spellings every platform writes
std::string settings_file_path()
{
	const std::string dir = smu2000::config_dir();
	if (dir.empty())
		return {};
	smu2000::ensure_config_dir();
	return dir + "gui.ini";
}

linux_app *g_linux = nullptr;

void play_dropped_file(const std::string &path)
{
	if (g_linux)
		g_linux->file_dropped(path);
}

} // namespace ui
