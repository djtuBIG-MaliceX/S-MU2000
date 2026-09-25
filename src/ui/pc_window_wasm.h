// license:BSD-3-Clause
//
// Browser side of the window that hosts an ImGui view (imgui_view).
// pc_window_linux.h is the SDL3 one, pc_window.h the Win32/D3D11 one and
// pc_window_mac.* the Metal one; this is the same class with the same
// shape, except there is no window at all.
//
// The page reserves a pane with one WebGL2 canvas. Every visible view
// registers a tab there; only the *active* one draws (one shared ImGui
// context, like the page has one canvas). Opening a view makes it the
// active tab and pokes JS to redraw the tab strip -- no dialog, no
// second window, as requested.

#ifndef S_MU2000_UI_PC_WINDOW_WASM_H
#define S_MU2000_UI_PC_WINDOW_WASM_H

#pragma once

#include "xg_ui.h"

#include <memory>
#include <string>

namespace ui {

class pc_window
{
public:
	explicit pc_window(std::unique_ptr<imgui_view> view) : m_view(std::move(view)) {}
	~pc_window();

	// Show it: registers the tab and makes it the active one. On failure
	// err says why (there is no GL context yet, say).
	bool show(std::string &err);
	// Hide it without destroying anything, so showing it again comes back
	// in the same state. If it was the active tab, another visible tab
	// takes the canvas.
	void hide();
	// Nothing to tear down per-window (one shared context); kept so the
	// shared code paths compile the same on every platform.
	void close();
	bool visible() const { return m_visible; }
	// gui is ending. Tell the contents it closed (unmute the overview, etc.)
	void shutdown(bridge &br);

	// From the frame loop (app::frame_work -> pc_frame_all). Draws only
	// when this window owns the pane; invisible windows cost nothing.
	void frame(xg::model &m, const xg_snapshot &ram, bridge &br);

	// gui plays MIDI files dropped on a window. In the browser drops are
	// handled by the page (smu_open_smf), so this only stores the hook.
	static void set_drop_handler(void (*fn)(const std::string &path));

	// ---- the pane, as the page sees it ------------------------------------

	// One shared ImGui context + GL backend. The canvas' WebGL2 context is
	// created here (canvas DOM selector like "#editor-canvas"), so it is
	// current when the backend and the frame loop draw (smu_gl_init).
	static bool init_gl(const std::string &canvas_selector, std::string &err);
	static bool gl_ready();
	// Viewport of the canvas in CSS pixels; device pixel ratio.
	static void set_viewport(int w, int h, float dpr);

	// Tab list of the visible windows (stable order: the app member order
	// list, pc, fx, shapes, master -- the same everywhere).
	static int  pane_count();
	static pc_window *pane(int i);           // nullptr when out of range
	// Which pane index owns the canvas (-1 = none visible)
	static int  active_index();
	// Make pane i active (JS tab click). False when hidden/gone.
	static bool activate(int i);

	// Input, forwarded from the canvas' DOM events (JS -> js_wasm.cpp).
	static void on_mouse_move(float x, float y);
	static void on_mouse_button(int button, bool down);       // 0 left, 1 right, 2 middle
	static void on_wheel(float dx, float dy);
	static void on_key(int key_code, bool down);              // GLFW-style codes
	static void on_char(unsigned codepoint);
	// The active window's view wants text (a box is focused): JS should
	// keep sending chars then.
	static bool wants_text();

	std::string title_utf8() const;          // m_view->title() as UTF-8

private:
	void draw(xg::model &m, const xg_snapshot &ram, bridge &br);

	std::unique_ptr<imgui_view> m_view;
	bool m_visible = false;
	bool m_was_visible = false;              // to catch hiding, like the SDL one
};

} // namespace ui

#endif // S_MU2000_UI_PC_WINDOW_WASM_H
