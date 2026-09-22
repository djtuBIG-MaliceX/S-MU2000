// license:BSD-3-Clause
//
// Popup menus and native dialogs for the SDL3 front ends (gui and the
// plug-in editor). SDL has no menus, so popups are drawn into the window:
// Cairo text over the live panel, modal until chosen or cancelled. Two menu
// levels at most (a category opens a second flat list), mirroring the
// NSMenu/HMENU structure on the other platforms.
//
// File panels are asynchronous in SDL3: the result arrives in a callback that
// may run on another thread, so a modal event pump waits for it. Esc abandons
// the wait (a late callback lands in shared state, harmlessly). Message boxes
// block and answer directly.

#ifndef S_MU2000_UI_SDL_POPUP_H
#define S_MU2000_UI_SDL_POPUP_H

#pragma once

#include <SDL3/SDL.h>

#include <atomic>
#include <functional>
#include <string>
#include <vector>

namespace ui {
namespace sdl_popup {

struct item {
	std::string label;
	int  id = -1;          // final choice; SUB_* with submenu == true
	bool checked = false;
	bool enabled = true;
	bool separator = false;
	bool submenu = false;
	int  sub = -1;
};

// Modal: returns the chosen id, a submenu marker via sub_chosen, or -1.
// behind() repaints the background bits (the live panel); this uploads and
// presents. quit is set when the window closes underneath.
int run(SDL_Window *win, SDL_Renderer *ren, SDL_Texture *tex, void *bits,
        int ww, int wh, std::function<void()> behind, std::atomic<bool> &quit,
        const std::vector<item> &items, int x, int y, int &sub_chosen);

struct file_filter {
	const char *name;      // e.g. "MIDI files"
	const char *pattern;   // e.g. "mid;midi"
};

std::string open_file(SDL_Window *win, std::atomic<bool> &quit,
                      const file_filter *filters, int nfilters,
                      const std::string &defloc);
std::string save_file(SDL_Window *win, std::atomic<bool> &quit,
                      const file_filter *filters, int nfilters,
                      const std::string &defloc);

int message_box(SDL_Window *win, const char *title, const char *text,
                unsigned flags,
                std::initializer_list<SDL_MessageBoxButtonData> buttons);
void alert(SDL_Window *win, const char *title, const std::string &text);
bool confirm(SDL_Window *win, const char *title, const std::string &text,
             const char *ok_label);

} // namespace sdl_popup
} // namespace ui

#endif // S_MU2000_UI_SDL_POPUP_H
