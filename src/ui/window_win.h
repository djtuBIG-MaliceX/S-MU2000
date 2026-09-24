// license:BSD-3-Clause
//
// Windows-only companion to ui/window_mac.h: stock dialogs, popup menus
// and PC windows for the Windows front end, the way window_mac.h holds
// AppKit's open/save/confirm/alert panels for the Mac one.
//
// Deciding what to ask is shared (ui::app); only asking is here.
// Never included from Mac builds.
// The app class that answers these is ui/app_win.h.

#ifndef S_MU2000_UI_WINDOW_WIN_H
#define S_MU2000_UI_WINDOW_WIN_H

#pragma once

#include <string>
#include <vector>

#include <windows.h>
#include <commdlg.h>

#include "menu.h"
#include "menu_win.h"
#include "pc_window.h"
#include "text.h"

namespace ui {

// Creates and shows the main window (WM_CREATE sets g_win->hwnd through
// wnd_proc, DragAcceptFiles included). False when it could not be made
bool make_window(const char *title, int w, int h);

// Shows popup menu groups at a client point (already ClientToScreen'd)
inline void win_track_menu(HWND hwnd, POINT screen,
                           const std::vector<menu_group> &groups)
{
	track_menu(hwnd, screen, render_menu(groups));
}

// Shows a PC window, warning when it cannot be done
inline void win_open_window(HWND hwnd, pc_window &w)
{
	std::string err;
	if (!w.show(GetModuleHandleA(nullptr), err))
		MessageBoxW(hwnd, to_wide(err).c_str(), L"S-MU2000", MB_OK | MB_ICONWARNING);
}

// Opens an existing file. "" when cancelled
inline std::string win_open_file(HWND hwnd, const wchar_t *title,
                                 const wchar_t *filter, const wchar_t *defext)
{
	wchar_t file[MAX_PATH] = {};
	OPENFILENAMEW o{};
	o.lStructSize = sizeof(o);
	o.hwndOwner = hwnd;
	o.lpstrFilter = filter;
	o.lpstrFile = file;
	o.nMaxFile = MAX_PATH;
	o.lpstrTitle = title;
	o.lpstrDefExt = defext;
	o.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
	if (!GetOpenFileNameW(&o))
		return {};
	return to_utf8(file);
}

// Picks where to save a new file. "" when cancelled
inline std::string win_save_file(HWND hwnd, const wchar_t *title,
                                 const wchar_t *filter, const wchar_t *defext,
                                 const wchar_t *filename)
{
	wchar_t file[MAX_PATH] = {};
	wcsncpy(file, filename, MAX_PATH - 1);
	OPENFILENAMEW o{};
	o.lStructSize = sizeof(o);
	o.hwndOwner = hwnd;
	o.lpstrFilter = filter;
	o.lpstrFile = file;
	o.nMaxFile = MAX_PATH;
	o.lpstrTitle = title;
	o.lpstrDefExt = defext;
	o.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
	if (!GetSaveFileNameW(&o))
		return {};
	return to_utf8(file);
}

// Asks a yes/no question. True only when accepted; Cancel is the default,
// like confirm_modal on the Mac side
inline bool win_confirm(HWND hwnd, const std::string &text)
{
	return MessageBoxW(hwnd, to_wide(text).c_str(), L"S-MU2000",
	                   MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2) == IDOK;
}

// Tells the user something they only have to acknowledge
inline void win_note(HWND hwnd, const std::string &text)
{
	MessageBoxW(hwnd, to_wide(text).c_str(), L"S-MU2000", MB_OK | MB_ICONINFORMATION);
}

// Shows a failure straight away (outside a menu command, e.g. a dropped file)
inline void win_error(HWND hwnd, const std::string &text)
{
	MessageBoxW(hwnd, to_wide(text).c_str(), L"S-MU2000", MB_OK | MB_ICONWARNING);
}

LRESULT CALLBACK wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp);

} // namespace ui

#endif // S_MU2000_UI_WINDOW_WIN_H
