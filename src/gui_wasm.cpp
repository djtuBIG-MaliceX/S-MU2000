// license:BSD-3-Clause
//
// The browser front end: main() and nothing else, the twin of gui_linux.cpp.
// The shared program (bridge, panel, player, remembered ports, menus, PC
// windows, bring-up) is ui::app, answered by ui::wasm_app (ui/app_wasm.h);
// the "window" is a DIB painted each requestAnimationFrame tick and a DOM
// canvas, and the pump is the page's own loop calling smu_frame().
//
// There is no ROM gate here: the page uploads the flash image later, and
// run()'s boot job retries until smu_allow_boot() says the files are in.
//
//   smu2000.js -- <rom dir> [--lang N] ...   (ui/tool_args.h, via callMain)

#include "compat/console.h"
#include "mu2000.h"

#include "ui/app_wasm.h"
#include "ui/bridge.h"
#include "ui/engine.h"
#include "ui/lang.h"
#include "ui/midi_in.h"
#include "ui/midi_out.h"
#include "ui/tool_args.h"

#include <cstdio>

int main(int argc, char **argv)
{
	ui::tool_args a;
	a.latency = 20;        // pulled by the ScriptProcessor; a small target is fine
	ui::output_options out_opts;
	ui::window_options win_opts;
	ui::engine_options eng_opts;

	// The flags are shared (ui/tool_args.h)
	const int parsed = ui::parse_tool_args(argc, argv, a, eng_opts, out_opts, win_opts);
	// The language resolves here, from the parsed --lang (then gui.ini,
	// then the locale), before any texts() use below
	ui::init_lang(a.lang.c_str());
	if (parsed >= 0)
		return parsed;

	static ui::bridge br;
	static ui::midi_in  midi_ports[mu2000::MIDI_PORTS];
	static ui::midi_out mout, mout_b, mout_mu;
	static ui::wasm_app gui(br, midi_ports, mout, mout_b, mout_mu);
	ui::g_wasm = &gui;

	static ui::engine eng(br, midi_ports[0]);
	gui.wire_engine(eng, eng_opts);
	gui.eng = &eng;
	gui.state = &eng.state;
	// No load_machine here: the ROMs may arrive after start-up. run() hands
	// that to the boot job, which the frame loop retries until they are in.

	// A MIDI file dropped on any window plays
	ui::pc_window::set_drop_handler(ui::play_dropped_file);

	// The window (a DIB), the boot job, and the return; the page pumps
	// frames through smu_frame() from here on
	return gui.run(a, eng_opts, out_opts, win_opts);
}
