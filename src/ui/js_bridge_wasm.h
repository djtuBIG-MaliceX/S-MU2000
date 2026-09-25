// license:BSD-3-Clause
//
// Declarations of the C++ -> JS door (js_wasm.cpp implements them with
// EM_JS / EM_ASYNC_JS). wasm-side code never touches Module.* directly;
// the page exposes window.SmuHooks for its own glue and these calls are
// what land there.

#ifndef S_MU2000_UI_JS_BRIDGE_WASM_H
#define S_MU2000_UI_JS_BRIDGE_WASM_H

#pragma once

#include "compat/mamecompat.h"

#include <cstddef>
#include <string>

namespace jsbridge {

// Blit the panel DIB ([R,G,B,A] straight alpha) to the page canvas.
// serial lets the page skip work when nothing was painted.
void present_panel(const u8 *bits, int w, int h, unsigned long long serial);

// One assembled THRU/MIDI-OUT message; the page routes it to the WebMIDI
// output the dropdown picked. (ui::wasm_midi_out_bytes forwards here.)
void midi_out_bytes(const u8 *p, size_t n);

// Console line for the page (devtools + its log pane)
void log(const char *line);

// The panel window title, mirrored to the page header
void set_title(const char *title);

// A one-line status for the page's status bar (is_error colours it)
void set_status(const char *text, bool is_error);

// The tab strip of the reserved imgui pane changed (a window opened or
// hid); the page re-reads the smu_pane_* exports
void panes_changed();

// ---- dialogs: DOM round-trips. These block the wasm stack (Asyncify)
// until the user answers; ""/false means cancelled.

// File picker. Filters are a comma list of extensions or MIME types.
// The chosen file's bytes are written to the wasm FS by the page and the
// FS path comes back.
std::string open_file(const char *filters);
std::string save_file(const char *suggest, const char *filters);
bool confirm(const char *text);

} // namespace jsbridge

#endif // S_MU2000_UI_JS_BRIDGE_WASM_H
