// license:BSD-3-Clause
//
// The SDL3 window system: the SDL window, its event pump and the paint
// surface, declared here and kept in ui/window_sdl.cpp. The Win32 and
// AppKit twins are ui/window_win.* and ui/window_mac.*; the layout is the
// same three files on every platform -- window system (this, plus the
// shared ui/sdl_popup for menus and dialogs), app class (ui/app_linux.* on
// Linux), main (gui_linux.cpp).
//
// The pump is Linux's: it takes a linux_app and keeps the SDL handles to
// itself (ui::app hooks drive everything, so a front end on another platform
// can reuse the shape by answering the same hooks).
//
// Nothing SDL appears in this header: the declarations below are all the
// app class and main need.

#ifndef S_MU2000_UI_WINDOW_SDL_H
#define S_MU2000_UI_WINDOW_SDL_H

#pragma once

#include <string>

namespace ui {

class linux_app;

// The SDL3 event loop: it owns the window, renderer and framebuffer, feeds
// the app mouse/key/drop events and calls paint_main() every 33 ms. Returns
// when the window closes. seconds_limit on the app ends the run after that
// many seconds (--seconds: timed runs for smoke tests and demos)
int run_window(linux_app &gui, const char *title, int w, int h);

// --selftest: paint twice into separate surfaces and compare the bytes,
// then upload and read the pixels back through a hidden window. The
// automatable core of the DIB-vs-window check (doc/porting-linux-gui.md)
int selftest(const std::string &rom_dir, int w, int h);

} // namespace ui

#endif // S_MU2000_UI_WINDOW_SDL_H
