// license:BSD-3-Clause
//
// The browser front end's app: ui::app with the page answering everything
// a window system would. gui_wasm.cpp keeps main(); the pump lives in
// ui/window_wasm.cpp; dialogs are DOM round-trips through js_wasm.cpp
// (Asyncify), errors go to the JS console and the page's status line.
//
// The Windows / macOS / Linux twins are app_win.h, app_mac.h and
// app_linux.h. Same hooks answered differently: no SDL, no popups, no
// threads -- the frame loop does what the boot thread used to.

#ifndef S_MU2000_UI_APP_WASM_H
#define S_MU2000_UI_APP_WASM_H

#pragma once

#include <atomic>
#include <cstddef>
#include <cstdio>
#include <string>
#include <vector>

#include "app.h"
#include "js_bridge_wasm.h"
#include "menu.h"
#include "window_wasm.h"

namespace ui {

// gui.ini lives under the per-user settings directory (compat/paths.h);
// the page mounts IDBFS there so it persists across reloads
std::string settings_file_path();

// A MIDI file dropped on a pane / picked in the page (ui::app::file_dropped)
void play_dropped_file(const std::string &path);

class wasm_app : public app
{
public:
	// mi is MIDI IN A-D, mu2000::MIDI_PORTS of them
	wasm_app(bridge &b, midi_in *mi,
	         midi_out &tha, midi_out &thb, midi_out &muo)
	    : app(b, mi, tha, thb, muo) {}

	// ---- ui::app hooks: dialogs are DOM round-trips (js_bridge), errors
	// go to the console and the page's status line

	std::string settings_path() const override { return settings_file_path(); }

	void menu_error(const std::string &text) override
	{
		jsbridge::log(("menu: " + text).c_str());
		jsbridge::set_status(text.c_str(), true);
	}
	void menu_note(const std::string &text) override
	{
		jsbridge::log(("note: " + text).c_str());
		jsbridge::set_status(text.c_str(), false);
	}
	std::string ask_card_open_path() override
	{
		return jsbridge::open_file(".img,.bin,application/octet-stream");
	}
	std::string ask_card_save_path() override
	{
		return jsbridge::save_file("smartmedia.img", ".img");
	}
	std::string ask_midi_file_path() override
	{
		return jsbridge::open_file(".mid,.midi,audio/midi");
	}
	bool confirm_factory_reset() override
	{
		return jsbridge::confirm(UI_TEXT(dlg_factory_text,
		    "Reset the MU2000 to factory state and restart it.\n"
		    "Utility settings and remembered volume/voice settings will all be erased."));
	}

	// ---- the window-system shell (ui::app::run drives these)

	// There is no window to make; the canvas is already in the page. The
	// panel surface is the DIB the frame loop paints and uploads.
	bool open_main_window(const char *title, int w, int h) override
	{
		if (!surface_create(w, h))
			return false;
		jsbridge::set_title(title);
		return true;
	}
	void pump_window(const char *title, int w, int h) override
	{
		(void)title; (void)w; (void)h;
		install_frame_loop(*this);       // returns; the loop runs on rAF
	}
	// The devices are objects here like on Linux; opening the output
	// itself waits for the firmware (start_audio, run by the frame loop)
	void make_audio() override
	{
		static audio_out dev_out;
		static audio_in  dev_in;
		out = &dev_out;
		ain = &dev_in;
		if (eng)
			eng->ain = &dev_in;
	}
	void say_audio_opened(bool) override
	{
		jsbridge::log(("audio out: " + out->device_name()).c_str());
	}
	void say_audio_running() override
	{
		jsbridge::log("sounding");
		jsbridge::set_status("Running", false);
	}
	u64 audio_drops() override { return out ? out->starved() : 0; }
	void print_audio_details() override {}

	// The status middle: pull-through output has no queue to measure
	void format_middle(char *dst, std::size_t n) override
	{
		std::snprintf(dst, n, UI_TEXT(status_middle_linux_fmt, "starved %llu"),
		              (unsigned long long)(out ? out->starved() : 0));
	}

	// An editor pane comes up in the reserved pane (and the tab strip
	// redraws); failure goes to the page's status line
	void open_pc_window(pc_window &w) override
	{
		std::string err;
		if (!w.show(err)) {
			char m[512];
			std::snprintf(m, sizeof(m), UI_TEXT(dlg_cannot_fmt, "Cannot open: %s"), err.c_str());
			jsbridge::set_status(m, true);
		} else {
			jsbridge::panes_changed();
		}
	}

	// ---- frame loop body (window_wasm.cpp calls this every rAF tick)

	void frame()
	{
		// Two drawn frames, then the boot job (the same steps the boot
		// thread ran on the desktop). Wait for the page to say the ROMs
		// are in (smu_allow_boot) before starting the spin; a missed ROM
		// set leaves the job armed so another smu_allow_boot can retry.
		if (m_boot_job && state && state->load() == 0 &&
		    m_frame_no >= 2 && m_allow_boot) {
			if (m_boot_job())
				m_boot_job = nullptr;
			else
				m_allow_boot = false;
		}
#ifdef __EMSCRIPTEN__
		play.pump();                     // MIDI-file timing, rAF paced
#endif
		paint_main(panel_surface().dc, panel_surface().w);
		m_frame_no++;
		present_panel();               // serial++ and blit to the page canvas
	}

	unsigned long long frame_no() const { return m_frame_no; }

	// smu_allow_boot from the page (ROMs are in the FS / upload done).
	// Also clears a boot that parked on "no ROMs" so the job can run.
	void allow_boot()
	{
		m_allow_boot = true;
		if (m_boot_job && state && state->load() == 2)
			state->store(0);
	}

	// MIDI bytes from WebMIDI into port A-D's rings (fill() drains these
	// on every platform the same way)
	void push_midi(int port, const u8 *p, size_t n)
	{
		if (port < 0 || port >= mu2000::MIDI_PORTS || !p)
			return;
		for (size_t i = 0; i < n; i++)
			midi[port].push(p[i]);
	}

private:
	unsigned long long m_frame_no = 0;
	bool m_allow_boot = false;
};

extern wasm_app *g_wasm;

} // namespace ui

#endif // S_MU2000_UI_APP_WASM_H
