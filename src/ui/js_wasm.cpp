// license:BSD-3-Clause
//
// The JS door of the browser build. Two directions:
//
//   wasm -> JS   EM_JS wrappers calling window.SmuHooks.* (the page
//                installs them before the wasm boots). The async ones
//                (file pickers, confirm) round-trip a DOM promise via
//                Asyncify and block the wasm stack until answered.
//
//   JS -> wasm   extern "C" exports the page calls. Audio and panel
//                pulls (smu_audio_render, smu_frame) are kept OFF the
//                Asyncify instrument set (see CMakeLists.txt, ASYNCIFY_
//                REMOVE): they never sleep, and the page keeps calling
//                them while a dialog is unwound in smu_menu.
//
// MIDI bytes from WebMIDI come in through smu_midi_push and go out
// through SmuHooks.onMidiOutBytes; the dropdown routing lives entirely
// in the page.

#include "js_bridge_wasm.h"

#include "app_wasm.h"
#include "audio_out.h"
#include "midi_in.h"
#include "midi_out.h"
#include "pc_window_wasm.h"
#include "wasm_glue.h"
#include "window_wasm.h"

#include <emscripten.h>
#include <emscripten/heap.h>

#include <string>
#include <vector>

namespace ui { wasm_app *g_wasm = nullptr; }

// Set once by the page (via smu_allow_boot) when the ROM files are in the
// FS. The frame loop's boot gate holds it until load+boot succeed.
static bool g_allow_boot = false;

using ::u8;
using ::s16;
using ::u32;

// ============================ wasm -> JS ==================================

extern "C" {

EM_JS(void, js_present_panel, (const u8 *bits, int w, int h,
                              unsigned long long serial_lo), {
	if (typeof window !== "undefined" && window.SmuHooks && SmuHooks.presentPanel)
		SmuHooks.presentPanel(bits, w, h, Number(serial_lo));
})

EM_JS(void, js_midi_out_bytes, (const u8 *p, int n), {
	if (typeof window !== "undefined" && window.SmuHooks && SmuHooks.onMidiOutBytes) {
		const bytes = HEAPU8.slice(p, p + n);
		SmuHooks.onMidiOutBytes(bytes);
	}
})

EM_JS(void, js_log, (const char *line), {
	const s = UTF8ToString(line >>> 0);
	if (typeof window !== "undefined" && window.SmuHooks && SmuHooks.onLog)
		SmuHooks.onLog(s);
	else
		console.log(s);
})

EM_JS(void, js_set_title, (const char *title), {
	if (typeof window !== "undefined" && window.SmuHooks && SmuHooks.onTitle)
		SmuHooks.onTitle(UTF8ToString(title >>> 0));
})

EM_JS(void, js_set_status, (const char *text, int is_error), {
	if (typeof window !== "undefined" && window.SmuHooks && SmuHooks.onStatus)
		SmuHooks.onStatus(UTF8ToString(text >>> 0), !!is_error);
})

EM_JS(void, js_panes_changed, (void), {
	if (typeof window !== "undefined" && window.SmuHooks && SmuHooks.onPanesChanged)
		SmuHooks.onPanesChanged();
})

// Dialog round-trips. JS returns a _malloc'd UTF-8 buffer (0 = cancel);
// the wrapper copies it into a std::string and frees it here.

EM_ASYNC_JS(char *, js_open_file, (const char *filters), {
	if (typeof window === "undefined" || !window.SmuHooks || !SmuHooks.openFile)
		return 0;
	const path = await SmuHooks.openFile(UTF8ToString(filters >>> 0));
	if (!path)
		return 0;
	const len = lengthBytesUTF8(path) + 1;
	const buf = _malloc(len);
	stringToUTF8(path, buf, len);
	return buf;
})

EM_ASYNC_JS(char *, js_save_file, (const char *suggest, const char *filters), {
	if (typeof window === "undefined" || !window.SmuHooks || !SmuHooks.saveFile)
		return 0;
	const path = await SmuHooks.saveFile(UTF8ToString(suggest >>> 0),
	                                     UTF8ToString(filters >>> 0));
	if (!path)
		return 0;
	const len = lengthBytesUTF8(path) + 1;
	const buf = _malloc(len);
	stringToUTF8(path, buf, len);
	return buf;
})

EM_ASYNC_JS(int, js_confirm, (const char *text), {
	if (typeof window === "undefined" || !window.SmuHooks || !SmuHooks.confirm)
		return 0;
	return await SmuHooks.confirm(UTF8ToString(text >>> 0)) ? 1 : 0;
})

} // extern "C"

namespace jsbridge {

void present_panel(const u8 *bits, int w, int h, unsigned long long serial)
{
	js_present_panel(bits, w, h, serial);
}

void midi_out_bytes(const u8 *p, size_t n)
{
	js_midi_out_bytes(p, int(n));
}

void log(const char *line)
{
	js_log(line);
	std::fflush(stdout);
}

void set_title(const char *title) { js_set_title(title); }
void set_status(const char *text, bool is_error) { js_set_status(text, is_error ? 1 : 0); }
void panes_changed() { js_panes_changed(); }

std::string open_file(const char *filters)
{
	char *p = js_open_file(filters);
	if (!p)
		return {};
	std::string s(p);
	std::free(p);
	return s;
}

std::string save_file(const char *suggest, const char *filters)
{
	char *p = js_save_file(suggest, filters);
	if (!p)
		return {};
	std::string s(p);
	std::free(p);
	return s;
}

bool confirm(const char *text) { return js_confirm(text) != 0; }

} // namespace jsbridge

namespace ui {

// wasm_midi_out_bytes is declared in wasm_glue.h and called from
// midi_out_wasm.cpp's emit().
void wasm_midi_out_bytes(const u8 *p, size_t n)
{
	jsbridge::midi_out_bytes(p, n);
}

} // namespace ui

// ============================ JS -> wasm ==================================

namespace {

std::string g_message_scratch;
std::string g_pane_title_scratch;
std::string g_menu_json_scratch;

// A tiny JSON string escaper (no library pulled in for this)
std::string jstr(const std::string &in)
{
	std::string out = "\"";
	for (unsigned char c : in) {
		switch (c) {
		case '"':  out += "\\\""; break;
		case '\\': out += "\\\\"; break;
		case '\n': out += "\\n";  break;
		case '\r': out += "\\r";  break;
		case '\t': out += "\\t";  break;
		default:
			if (c < 0x20) {
				char hex[8];
				std::snprintf(hex, sizeof(hex), "\\u%04x", c);
				out += hex;
			} else {
				out.push_back(char(c));
			}
		}
	}
	out.push_back('"');
	return out;
}

constexpr unsigned AUDIO_PULL_MAX = 16384;  // frames per smu_audio_render
static thread_local s16 g_audio_buf[AUDIO_PULL_MAX * 2];

} // namespace

extern "C" {

// ---- editor pane (imgui / WebGL2) ----------------------------------------

EMSCRIPTEN_KEEPALIVE
int smu_gl_init(const char *canvas_selector)
{
	std::string err;
	if (!ui::pc_window::init_gl(canvas_selector ? canvas_selector : "#editor-canvas", err)) {
		jsbridge::log(("gl init failed: " + err).c_str());
		return -1;
	}
	return 0;
}

EMSCRIPTEN_KEEPALIVE
void smu_gl_resize(int w, int h, float dpr)
{
	ui::pc_window::set_viewport(w, h, dpr);
}

EMSCRIPTEN_KEEPALIVE
void smu_gl_mouse_move(float x, float y) { ui::pc_window::on_mouse_move(x, y); }

EMSCRIPTEN_KEEPALIVE
void smu_gl_mouse_button(int button, int down)
{
	ui::pc_window::on_mouse_button(button, down != 0);
}

EMSCRIPTEN_KEEPALIVE
void smu_gl_wheel(float dx, float dy) { ui::pc_window::on_wheel(dx, dy); }

EMSCRIPTEN_KEEPALIVE
void smu_gl_key(int code, int down) { ui::pc_window::on_key(code, down != 0); }

EMSCRIPTEN_KEEPALIVE
void smu_gl_char(unsigned codepoint) { ui::pc_window::on_char(codepoint); }

EMSCRIPTEN_KEEPALIVE
int smu_gl_wants_text() { return ui::pc_window::wants_text() ? 1 : 0; }

EMSCRIPTEN_KEEPALIVE
int smu_pane_count() { return ui::pc_window::pane_count(); }

EMSCRIPTEN_KEEPALIVE
const char *smu_pane_title(int i)
{
	ui::pc_window *w = ui::pc_window::pane(i);
	if (!w)
		return "";
	g_pane_title_scratch = w->title_utf8();
	return g_pane_title_scratch.c_str();
}

EMSCRIPTEN_KEEPALIVE
int smu_pane_active() { return ui::pc_window::active_index(); }

EMSCRIPTEN_KEEPALIVE
int smu_pane_activate(int i) { return ui::pc_window::activate(i) ? 1 : 0; }

// ---- the frame tick -------------------------------------------------------

// Panel paint + pane draw + boot gate. Asyncify-removed (never sleeps);
// the page drives it from requestAnimationFrame.
EMSCRIPTEN_KEEPALIVE
void smu_frame()
{
	if (ui::g_wasm)
		ui::g_wasm->frame();
}

// ---- panel surface --------------------------------------------------------

EMSCRIPTEN_KEEPALIVE
const u8 *smu_panel_bits()
{
	return static_cast<const u8 *>(ui::panel_surface().bits);
}

EMSCRIPTEN_KEEPALIVE
int smu_panel_w() { return ui::panel_surface().w; }

EMSCRIPTEN_KEEPALIVE
int smu_panel_h() { return ui::panel_surface().h; }

EMSCRIPTEN_KEEPALIVE
unsigned long long smu_panel_serial() { return ui::panel_serial(); }

// ---- engine state ----------------------------------------------------------

EMSCRIPTEN_KEEPALIVE
int smu_state()
{
	if (!ui::g_wasm || !ui::g_wasm->state)
		return 0;
	return ui::g_wasm->state->load();
}

EMSCRIPTEN_KEEPALIVE
const char *smu_message()
{
	if (!ui::g_wasm || !ui::g_wasm->eng)
		return "";
	g_message_scratch = ui::g_wasm->eng->message;
	return g_message_scratch.c_str();
}

EMSCRIPTEN_KEEPALIVE
void smu_allow_boot()
{
	g_allow_boot = true;
	if (ui::g_wasm)
		ui::g_wasm->allow_boot();
}

// ---- menus -----------------------------------------------------------------

// Everything the right-click menu shows, as JSON the page can render:
// [{title, items:[{label,id,checked,enabled,separator}]}]. The page calls
// this when smu_mouse_down reports show_menu, then smu_menu(id) on a pick.
EMSCRIPTEN_KEEPALIVE
const char *smu_context_menu_json(int x, int y)
{
	if (!ui::g_wasm) {
		g_menu_json_scratch = "[]";
		return g_menu_json_scratch.c_str();
	}
	const std::vector<ui::menu_group> groups = ui::g_wasm->context_menu(x, y);
	std::string j = "[";
	for (size_t g = 0; g < groups.size(); g++) {
		if (g)
			j += ",";
		j += "{\"title\":" + jstr(groups[g].title) + ",\"items\":[";
		for (size_t i = 0; i < groups[g].items.size(); i++) {
			const ui::menu_item &it = groups[g].items[i];
			if (i)
				j += ",";
			j += "{\"label\":" + jstr(it.label) +
			     ",\"id\":" + std::to_string(it.id) +
			     ",\"checked\":" + (it.checked ? "true" : "false") +
			     ",\"enabled\":" + (it.enabled ? "true" : "false") +
			     ",\"separator\":" + (it.separator ? "true" : "false") +
			     ",\"shortcut\":" + jstr(it.shortcut) + "}";
		}
		j += "]}";
	}
	j += "]";
	g_menu_json_scratch = std::move(j);
	return g_menu_json_scratch.c_str();
}

// One menu id (the 26 shared ones, ui/menu.h). May sleep the wasm stack
// for a file picker or a confirm; the page must keep smu_frame pulling.
EMSCRIPTEN_KEEPALIVE
void smu_menu(int id)
{
	if (ui::g_wasm)
		ui::g_wasm->menu_chosen(id);
}

// ---- panel input -----------------------------------------------------------
// Returns a bitmask of what the press did: 1 panel control, 2 opened a
// window (tab strip may have changed), 4 wants the context menu.

EMSCRIPTEN_KEEPALIVE
int smu_mouse_down(int x, int y, int right)
{
	if (!ui::g_wasm)
		return 0;
	const ui::mouse_out o = ui::g_wasm->mouse_down(x, y, right != 0);
	int mask = 0;
	if (o.panel_pressed)  mask |= 1;
	if (o.opened_window)  mask |= 2;
	if (o.show_menu)      mask |= 4;
	jsbridge::panes_changed();
	return mask;
}

EMSCRIPTEN_KEEPALIVE
int smu_mouse_drag(int x, int y)
{
	return ui::g_wasm ? (ui::g_wasm->mouse_drag(x, y) ? 1 : 0) : 0;
}

EMSCRIPTEN_KEEPALIVE
void smu_mouse_up()
{
	if (ui::g_wasm)
		ui::g_wasm->mouse_up();
}

EMSCRIPTEN_KEEPALIVE
int smu_wheel(int x, int y, int steps)
{
	return ui::g_wasm ? (ui::g_wasm->wheel(x, y, steps) ? 1 : 0) : 0;
}

EMSCRIPTEN_KEEPALIVE
int smu_hand_cursor(int x, int y)
{
	return ui::g_wasm ? (ui::g_wasm->hand_cursor(x, y) ? 1 : 0) : 0;
}

// Panel keyboard: printables as characters (handle_panel_key), the F-keys
// as the shared KEY_F* codes (ui/menu.h).
EMSCRIPTEN_KEEPALIVE
void smu_key(int code, int down)
{
	if (!ui::g_wasm)
		return;
	if (code >= 0 && code < 0x100)
		ui::g_wasm->handle_panel_key(code, down != 0);
	else
		ui::g_wasm->key(code, down != 0);
}

// ---- audio -----------------------------------------------------------------

// The page's ScriptProcessor calls this when it needs frames; the buffer
// stays owned by wasm (static, valid until the next call) and holds
// frames*2 s16 interleaved at 44100Hz. Null when not sounding yet.
EMSCRIPTEN_KEEPALIVE
const s16 *smu_audio_render(unsigned frames)
{
	if (frames == 0 || frames > AUDIO_PULL_MAX)
		return nullptr;
	if (!ui::wasm_audio_pull(g_audio_buf, frames))
		return nullptr;
	return g_audio_buf;
}

EMSCRIPTEN_KEEPALIVE
void smu_audio_in_push(const s16 *frames, int frames_count)
{
	ui::wasm_audio_in_push(frames, unsigned(frames_count));
}

// ---- MIDI ------------------------------------------------------------------

// Whole messages off a WebMIDI input. port is 0..3 (A-D).
EMSCRIPTEN_KEEPALIVE
void smu_midi_push(int port, const u8 *p, unsigned n)
{
	if (ui::g_wasm)
		ui::g_wasm->push_midi(port, p, n);
}

// The dropdown contents, newline separated (page order == device numbers
// the menu IDs use).
EMSCRIPTEN_KEEPALIVE
void smu_set_midi_in_ports(const char *names_nl)
{
	std::vector<std::string> out;
	if (names_nl) {
		std::string s(names_nl);
		size_t at = 0;
		while (at < s.size()) {
			const size_t nl = s.find('\n', at);
			const std::string line = s.substr(at, nl == std::string::npos
			                                   ? std::string::npos : nl - at);
			if (!line.empty())
				out.push_back(line);
			if (nl == std::string::npos)
				break;
			at = nl + 1;
		}
	}
	ui::wasm_set_midi_in_ports(std::move(out));
}

EMSCRIPTEN_KEEPALIVE
void smu_set_midi_out_ports(const char *names_nl)
{
	std::vector<std::string> out;
	if (names_nl) {
		std::string s(names_nl);
		size_t at = 0;
		while (at < s.size()) {
			const size_t nl = s.find('\n', at);
			const std::string line = s.substr(at, nl == std::string::npos
			                                   ? std::string::npos : nl - at);
			if (!line.empty())
				out.push_back(line);
			if (nl == std::string::npos)
				break;
			at = nl + 1;
		}
	}
	ui::wasm_set_midi_out_ports(std::move(out));
}

// The microphone the page opened ("" until getUserMedia succeeds). rate
// is the AudioContext rate the pushed blocks come at.
EMSCRIPTEN_KEEPALIVE
void smu_set_mic(const char *name, double rate)
{
	ui::wasm_set_audio_in_device(name ? name : "", rate);
}

// ---- MIDI-file playback ------------------------------------------------------

// The page wrote the file into the FS; play it. Returns 1 when rolling.
EMSCRIPTEN_KEEPALIVE
int smu_open_smf(const char *path)
{
	if (!ui::g_wasm || !path)
		return 0;
	return ui::g_wasm->play_song(path) ? 1 : 0;
}

} // extern "C"
