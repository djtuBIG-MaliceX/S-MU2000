// license:BSD-3-Clause
//
// The five PC windows every front end hosts (overview, editor, insertion
// settings, part voice, master), driven the same way everywhere.
//
// gui.exe, gui (macOS), and the VST3/CLAP/AU plug-ins on both platforms each
// repeated the same dozen lines: one frame() per window per tick, then the
// double-click follow-ups from the overview. Only opening a window differs
// per host (show() takes an owner handle on Windows, none on macOS, and each
// side reports failure its own way), so the caller passes that in and this
// stays platform-neutral: it never includes either pc_window.h, and the
// window type is duck-typed. Nothing else about the hosts moves here --
// menus, function keys, file drops, shutdown order and the card slot stay
// where they are, which is what keeps the standalone extras intact.

#ifndef S_MU2000_UI_PC_HOST_H
#define S_MU2000_UI_PC_HOST_H

#pragma once

#include "xg_ui.h"

namespace ui {

// One tick for all five windows. Invisible windows cost nothing (each
// frame() returns early while hidden). The overview asks for the insertion,
// part-voice or master window by double-click; open() shows it, however the
// host shows windows
template <typename Window, typename Open>
inline void pc_frame_all(Window &list, Window &editor, Window &fx, Window &shapes, Window &master,
                         xg::model &m, const xg_snapshot &ram, bridge &br, Open open)
{
	list.frame(m, ram, br);
	editor.frame(m, ram, br);
	fx.frame(m, ram, br);
	shapes.frame(m, ram, br);
	master.frame(m, ram, br);
	// A double-click on an insertion row in the overview asks for the
	// settings window; on a VIB/FILTER/EG/EQ cell for the part voice window;
	// on the MASTER name or MASTER EQ cell for the master window
	if (xgui::take_fx_request())
		open(fx);
	if (xgui::take_part_request())
		open(shapes);
	if (xgui::take_master_request())
		open(master);
}

// What a front end owes its windows on the way out (the overview unmutes,
// the editor releases held buttons, and so on)
template <typename Window>
inline void pc_shutdown_all(Window &list, Window &editor, Window &fx, Window &shapes, Window &master,
                            bridge &br)
{
	list.shutdown(br);
	editor.shutdown(br);
	fx.shutdown(br);
	shapes.shutdown(br);
	master.shutdown(br);
}

} // namespace ui

#endif // S_MU2000_UI_PC_HOST_H
