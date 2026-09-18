// license:BSD-3-Clause
//
// macOS side of the window that hosts an ImGui view (imgui_view) for gui.
// Draws with Metal into an AppKit window.
//
// ui::pc_window.cpp is the Windows side (Win32 + Direct3D 11); this is the
// same class, the same shape. The contents (ui::pc_editor / ui::overview /
// ui::fx_editor) and ui::xgui are **the very same files** as on Windows.
// Only the window differs.
//
// The window draws in frame(), called from gui's render thread (app::draw,
// 30 frames a second). Closing only hides it, so reopening comes back in the
// same state. **Each window owns an ImGui context**, so the editor and the
// overview can be open at the same time.
//
// Differences from the Windows version, collected here:
//   * show() takes no owner handle (macOS has none)
//   * set_drop_handler passes a UTF-8 std::string (Windows uses std::wstring)
//   * imgui_impl_osx is not used. It installs one event monitor for the whole
//     process and feeds whatever context is current, so with a context per
//     window the input would arrive in another window. The view hands the
//     events to io itself, the way the Windows side does in its wnd_proc

#ifndef S_MU2000_UI_PC_WINDOW_MAC_H
#define S_MU2000_UI_PC_WINDOW_MAC_H

#pragma once

#include "xg_ui.h"

#include <memory>
#include <string>

struct ImGuiContext;

namespace ui {

class pc_window
{
public:
	explicit pc_window(std::unique_ptr<imgui_view> view) : m_view(std::move(view)) {}
	~pc_window();

	// Show it. Creates the window and the Metal device on the first call.
	// On failure err says why
	bool show(std::string &err);
	// Hide it without destroying anything, so showing it again comes back
	// in the same state. The plug-in calls this when its own view closes
	void hide();
	bool visible() const;
	// gui is ending. Tell the contents it closed (unmute the overview, etc.)
	void shutdown(bridge &br);

	// From gui's timer. Does nothing while the window is not visible
	void frame(xg::model &m, const xg_snapshot &ram, bridge &br);

	// Who to call when a file is dropped on the window (gui plays a MIDI
	// file). Decide before the window exists; without it drops are ignored
	static void set_drop_handler(void (*fn)(const std::string &path));

private:
	bool create(std::string &err);
	void destroy();

	std::unique_ptr<imgui_view> m_view;
	void *m_ns = nullptr;                    // the window/view/Metal plumbing, inside pc_window_mac.mm
	ImGuiContext *m_imgui = nullptr;
	bool m_was_visible = false;              // was visible in the previous frame (to catch hiding)
};

} // namespace ui

#endif // S_MU2000_UI_PC_WINDOW_MAC_H
