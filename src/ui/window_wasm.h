// license:BSD-3-Clause
//
// The main window of the browser front end. There is no window: the panel
// paints into a DIB (gdi_wasm.cpp rasterizes the GDI calls) and the frame
// loop blits that DIB to a 2D canvas on the page each frame.
//
// window_sdl.cpp is the SDL3 twin (DIB + streaming texture), window_win
// the GDI/DXGI one. Same framebuf idea, different upload.

#ifndef S_MU2000_UI_WINDOW_WASM_H
#define S_MU2000_UI_WINDOW_WASM_H

#pragma once

#include "compat/gdi.h"

namespace ui {

class wasm_app;

// The surface the panel paints into. Created when the app opens the main
// window (open_main_window), sized to the window. bits stays valid until
// resize/destroy; it is [R,G,B,A] straight alpha (see gdi_wasm.cpp).
struct framebuf
{
	HDC     dc = nullptr;
	HBITMAP bmp = nullptr;
	void   *bits = nullptr;
	int     w = 0, h = 0;
};

framebuf &panel_surface();
bool surface_create(int w, int h);
void surface_destroy();

// Installs the rAF-driven frame loop (pump_window calls this). The loop:
// run the boot job once drawn, pump the MIDI-file player, paint the panel
// into the DIB, hand the bytes to JS, and let the active imgui pane draw.
void install_frame_loop(wasm_app &app);
wasm_app *frame_loop_app();

// Painted frame -> canvas: bumps the serial and calls present_panel.
void present_panel();

// The JS side pulls these to know what to copy (serial bumps each paint)
unsigned long long panel_serial();

} // namespace ui

#endif // S_MU2000_UI_WINDOW_WASM_H
