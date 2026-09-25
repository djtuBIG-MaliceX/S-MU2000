// license:BSD-3-Clause
//
// Definitions for the browser app shell: where gui.ini lives, and the
// drop-handler trampoline. The frame loop lives in window_wasm.cpp, the
// JS bridge in js_wasm.cpp.
#include "ui/app_wasm.h"
#include "compat/paths.h"

namespace ui {

// gui.ini lives next to the other settings (compat/paths.h), with the same
// key spellings every platform writes. The page mounts IDBFS over the
// parent so it survives reloads.
std::string settings_file_path()
{
	const std::string dir = smu2000::config_dir();
	if (dir.empty())
		return {};
	smu2000::ensure_config_dir();
	return dir + "gui.ini";
}

void play_dropped_file(const std::string &path)
{
	if (g_wasm)
		g_wasm->file_dropped(path);
}

} // namespace ui
