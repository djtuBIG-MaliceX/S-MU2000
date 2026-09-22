// license:BSD-3-Clause
//
// The Linux front end: the real machine's front-panel look, drawn with Cairo
// and shown in an SDL3 window.
//
//   gui <rom directory> [--midi N] [--midi-b N] [--midi-c N] [--midi-d N]
//       [--nomidi] [--latency ms] [--audio part-of-name] [--audio-in name]
//       [--exclusive] [--factory] [--fast-midi] [--usb | --host-midi]
//       [--play song.mid] [--seconds N] [--size WxH] [--layout panel.txt]
//   gui --shot image.png [--size 1400x360] [--grid] [--layout panel.txt]
//   gui <rom directory> --boot --shot image.png [--mid song.mid seconds]
//   gui --list
//   gui --dump-layout panel.txt
//   gui --selftest [<rom directory>]   paint twice, compare the bytes
//
// Everything the window shows is English (ui::english_texts, installed first
// thing in main). The shared panel sources stay untouched: their Japanese
// defaults are what Windows and macOS still show.
//
// Sound works like live: the synth owns no clock (doc/design.md) and the
// ALSA worker thread makes exactly what the device asks for. Port/channel
// menus, the card slot menu and the PC editor windows are later phases
// (doc/porting-linux-gui.md); until then ports come from argv.

#include "compat/console.h"
#include "compat/gdi.h"
#include "compat/paths.h"
#include "mu2000.h"
#include "nvram.h"
#include "smartmedia.h"
#include "smf.h"
#include "ui/audio_in.h"
#include "ui/audio_out.h"
#include "ui/bridge.h"
#include "ui/engine.h"
#include "ui/layout.h"
#include "ui/midi_in.h"
#include "ui/midi_out.h"
#include "ui/panel.h"
#include "ui/player.h"
#include "ui/png.h"
#include "ui/texts.h"
#include "ui/xg_ui.h"
#include "ui/pc_editor.h"
#include "ui/overview.h"
#include "ui/fx_editor.h"
#include "ui/part_shapes.h"
#include "ui/master_editor.h"
#include "ui/pc_window_linux.h"
#include "ui/pc_host.h"
#include "ui/sdl_popup.h"

#include <SDL3/SDL.h>

#include <cairo/cairo.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace {

constexpr u32 RATE = ui::AUDIO_RATE;

using ui::engine;

// ---- A Cairo image surface behind a GDI DC --------------------------------
//
// Both --shot and the window paint through this: same surface, same paint
// call, so the bytes agree by construction (the DIB-vs-window check in
// doc/porting-linux-gui.md).

struct framebuf {
	HBITMAP bmp = nullptr;
	HDC     dc = nullptr;
	void   *bits = nullptr;
	int     w = 0, h = 0;

	bool reset(int nw, int nh)
	{
		if (nw == w && nh == h && dc)
			return true;
		free();
		if (nw <= 0 || nh <= 0)
			return false;
		BITMAPINFO bi{};
		bi.bmiHeader.biSize = sizeof(bi.bmiHeader);
		bi.bmiHeader.biWidth = nw;
		bi.bmiHeader.biHeight = -nh;            // top down
		bi.bmiHeader.biPlanes = 1;
		bi.bmiHeader.biBitCount = 32;
		bi.bmiHeader.biCompression = BI_RGB;
		dc = CreateCompatibleDC(nullptr);
		bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &bits, nullptr, 0);
		if (!bmp) {
			free();
			return false;
		}
		SelectObject(dc, bmp);
		w = nw;
		h = nh;
		return true;
	}

	void free()
	{
		if (bmp) DeleteObject(bmp);
		if (dc) DeleteDC(dc);
		bmp = nullptr;
		dc = nullptr;
		bits = nullptr;
		w = h = 0;
	}

	~framebuf() { free(); }
};

// ---- Write just the picture, with no window. Used to check the looks ------

int shot(const std::string &path, int w, int h, ui::bridge &br, bool grid,
         const std::string &layout_path)
{
	ui::panel p;
	std::string lerr;
	if (!layout_path.empty() && !p.lay().load(layout_path, lerr))
		std::fprintf(stderr, "Cannot open layout: %s\n", layout_path.c_str());
	if (!lerr.empty())
		std::fprintf(stderr, "%s", lerr.c_str());
	p.resize(w, h);
	p.set_grid(grid);

	framebuf fb;
	if (!fb.reset(w, h)) {
		std::fprintf(stderr, "Cannot make a %dx%d surface\n", w, h);
		return 1;
	}

	ui::snapshot s;
	br.read(s);
	p.set_volume(0.8);
	p.paint(fb.dc, s, 0, "");
	GdiFlush();

	const bool ok = ui::write_png(path, static_cast<const u8 *>(fb.bits), w, h, w * 4);
	std::printf(ok ? "Wrote %s (%dx%d)\n" : "Cannot write %s\n", path.c_str(), w, h);
	return ok ? 0 : 1;
}

// Paint twice into separate surfaces and compare. The automatable core of the
// DIB-vs-window check: the window uploads these same bytes, so deterministic
// painting here plus a memcpy upload means the screen agrees by construction.
// (Reading pixels back off the GPU is the one part that needs a real display.)
int selftest(const std::string &dir, int w, int h)
{
	static ui::bridge br;
	static ui::midi_in midi_ports[mu2000::MIDI_PORTS];
	static ui::engine eng(br, midi_ports[0]);

	if (!dir.empty()) {
		if (!eng.load(dir)) {
			std::fprintf(stderr, "%s\n", eng.message.c_str());
			return 1;
		}
		ui::xgui::set_voice_rom(eng.mu.program_rom());
		if (!eng.boot()) {
			std::fprintf(stderr, "%s\n", eng.message.c_str());
			return 1;
		}
		eng.state.store(1);
		s32 l, r;
		for (size_t i = 0; i < size_t(1.0 * RATE); i++)
			eng.mu.run_sample(l, r);
		eng.publish();
	} else {
		ui::snapshot s;
		std::snprintf(s.message, sizeof(s.message), "S-MU2000");
		br.publish(s);
	}

	ui::panel p;
	p.resize(w, h);
	framebuf a, b;
	if (!a.reset(w, h) || !b.reset(w, h)) {
		std::fprintf(stderr, "Cannot make a %dx%d surface\n", w, h);
		return 1;
	}
	ui::snapshot s;
	br.read(s);
	p.paint(a.dc, s, 0, "");
	p.paint(b.dc, s, 0, "");
	GdiFlush();
	const size_t bytes = size_t(w) * size_t(h) * 4;
	if (std::memcmp(a.bits, b.bits, bytes) != 0) {
		std::printf("SELFTEST FAIL: %dx%d differs\n", w, h);
		return 1;
	}
	std::printf("SELFTEST PASS: %dx%d deterministic (%zu bytes)\n", w, h, bytes);

	// The rest of the DIB-vs-window check: upload and read the pixels back.
	// A hidden window keeps this polite on a real display and working under
	// SDL_VIDEODRIVER=dummy. SKIP (not FAIL) when no video is available.
	if (!SDL_Init(SDL_INIT_VIDEO)) {
		std::printf("SELFTEST SKIP roundtrip: %s\n", SDL_GetError());
		return 0;
	}
	int rc = 0;
	SDL_Window *win = SDL_CreateWindow("selftest", w, h, SDL_WINDOW_HIDDEN);
	SDL_Renderer *ren = win ? SDL_CreateRenderer(win, nullptr) : nullptr;
	SDL_Texture *tex = ren ? SDL_CreateTexture(ren, SDL_PIXELFORMAT_ARGB8888,
	                                           SDL_TEXTUREACCESS_STREAMING, w, h)
	                       : nullptr;
	if (!tex) {
		std::printf("SELFTEST SKIP roundtrip: %s\n", SDL_GetError());
	} else {
		SDL_SetTextureBlendMode(tex, SDL_BLENDMODE_NONE);
		SDL_UpdateTexture(tex, nullptr, a.bits, w * 4);
		SDL_RenderTexture(ren, tex, nullptr, nullptr);
		SDL_RenderPresent(ren);
		SDL_Surface *got = SDL_RenderReadPixels(ren, nullptr);
		if (!got) {
			std::printf("SELFTEST SKIP roundtrip: %s\n", SDL_GetError());
		} else if (got->w != w || got->h != h || got->pitch != w * 4) {
			std::printf("SELFTEST FAIL: readback is %dx%d/%d, want %dx%d/%d\n",
			            got->w, got->h, got->pitch, w, h, w * 4);
			SDL_DestroySurface(got);
			rc = 1;
		} else {
			// Convert to ARGB8888 first: the readback format is the
			// renderer's choice (XRGB8888, ABGR8888, ...) and varies.
			// RGB must then agree exactly. Alpha may be normalized by the
			// readback (opaque alpha, premultiplied RGB kept), so it is
			// compared separately and only reported.
			std::vector<u8> conv(bytes);
			const bool cvt = SDL_ConvertPixels(w, h, got->format, got->pixels,
			                                   got->pitch, SDL_PIXELFORMAT_ARGB8888,
			                                   conv.data(), w * 4);
			const u32 *pa = static_cast<const u32 *>(a.bits);
			const u32 *pb = reinterpret_cast<const u32 *>(conv.data());
			size_t rgb_diff = 0, alpha_diff = 0;
			if (!cvt) {
				std::printf("SELFTEST SKIP roundtrip: %s\n", SDL_GetError());
				SDL_DestroySurface(got);
				if (tex) SDL_DestroyTexture(tex);
				if (ren) SDL_DestroyRenderer(ren);
				if (win) SDL_DestroyWindow(win);
				SDL_Quit();
				return 0;
			}
			for (int i = 0; i < w * h; i++) {
				if ((pa[i] & 0x00ffffffu) != (pb[i] & 0x00ffffffu))
					rgb_diff++;
				else if (pa[i] != pb[i])
					alpha_diff++;
			}
			SDL_DestroySurface(got);
			if (rgb_diff) {
				std::printf("SELFTEST FAIL: upload/readback differs in %zu px\n",
				            rgb_diff);
				ui::write_png("/tmp/selftest_want.png",
				              static_cast<const u8 *>(a.bits), w, h, w * 4);
				ui::write_png("/tmp/selftest_got.png", conv.data(), w, h, w * 4);
				std::printf("wrote /tmp/selftest_want.png and /tmp/selftest_got.png\n");
				rc = 1;
			} else {
				std::printf("SELFTEST PASS: upload/readback RGB-identical "
				            "(%zu px, %zu alpha-only)\n",
				            size_t(w) * size_t(h), alpha_diff);
			}
		}
	}
	if (tex) SDL_DestroyTexture(tex);
	if (ren) SDL_DestroyRenderer(ren);
	if (win) SDL_DestroyWindow(win);
	SDL_Quit();
	return rc;
}

// ---- Keys ------------------------------------------------------------------
//
// Character-based, like the macOS front end: panels buttons, not positions.

bool key_to_button(int code, mu2000::button &out)
{
	switch (code) {
	case 'a': out = mu2000::button::play;         return true;
	case 'e': out = mu2000::button::edit;         return true;
	case 'u': out = mu2000::button::util;         return true;
	case 'f': out = mu2000::button::effect;       return true;
	case 's': out = mu2000::button::mute_solo;    return true;
	case ']': out = mu2000::button::part_plus;    return true;
	case '[': out = mu2000::button::part_minus;   return true;
	case '=': case '+': out = mu2000::button::value_plus;  return true;
	case '-': out = mu2000::button::value_minus;  return true;
	case '\r': out = mu2000::button::enter;       return true;
	case 0x7f: case 0x08: out = mu2000::button::exit; return true;
	case '.': out = mu2000::button::select_right; return true;
	case ',': out = mu2000::button::select_left;  return true;
	case 'q': out = mu2000::button::seq;          return true;
	case 'z': out = mu2000::button::audition;     return true;
	case 'x': out = mu2000::button::select;       return true;
	case 'm': out = mu2000::button::sampling_mode; return true;
	default: break;
	}
	return false;
}

// ---- The windowed app -------------------------------------------------------

struct app;
void flush_card(app &gui);
void alert(app &gui, const char *title, const std::string &text);

struct app {
	ui::bridge &br;
	ui::midi_in *midi;   // MIDI_PORTS of them
	ui::midi_out *mout = nullptr;      // MIDI THRU A
	ui::midi_out *mout_b = nullptr;    // MIDI THRU B
	ui::midi_out *mout_mu = nullptr;   // MIDI OUT (from the MU2000)
	ui::panel  panel;
	ui::player play;

	// The PC windows (F2/F3 or right-click). Same views as on Windows; only
	// the window is SDL3 here. fx/shapes/master also open from the overview
	// by double-click (ui::pc_frame_all).
	ui::pc_window pc{ std::make_unique<ui::pc_editor>() };
	ui::pc_window list{ std::make_unique<ui::overview>() };
	ui::pc_window fx{ std::make_unique<ui::fx_editor>() };
	ui::pc_window shapes{ std::make_unique<ui::part_shapes>() };
	ui::pc_window master{ std::make_unique<ui::master_editor>() };
	bool open_pc = false, open_list = false, open_fx = false;

	std::string layout_path;
	std::string audio_name;    // ALSA PCM, by substring. Empty is "default"
	std::string ain_name;      // recording device for A/D INPUT. Empty is off
	std::string card_path;     // SmartMedia image file. Empty is none
	std::string in_name[mu2000::MIDI_PORTS];
	std::string out_name, out_name_b, out_name_mu;
	// Names that failed to open at startup. Kept until deliberately changed,
	// so a device that is not running yet is not forgotten by the next start.
	std::string in_keep[mu2000::MIDI_PORTS];
	std::string out_keep, out_keep_b, out_keep_mu;
	int in_dev[mu2000::MIDI_PORTS] = { -1, -1, -1, -1 };
	int out_dev = -1, out_dev_b = -1, out_dev_mu = -1;
	bool keep_settings = false;   // --nomidi: remember nothing

	ui::audio_out *out = nullptr;
	ui::audio_in  *ain = nullptr;
	engine        *eng = nullptr;
	std::thread   reboot;              // factory reset worker
	std::string   last_error;          // shown after a menu action fails

	// SDL handles, set by run_window for the popup menus to share.
	SDL_Window   *win = nullptr;
	SDL_Renderer *ren = nullptr;
	framebuf     *fb = nullptr;
	int ww = 0, wh = 0;

	u64 reported_drops = 0;
	u64 last_card_flush = 0;

	bool ready() const { return eng && eng->state.load() == 1; }

	void open_pc_window(ui::pc_window &w)
	{
		std::string err;
		if (!w.show(err))
			alert(*this, "S-MU2000", err);
	}

	void set_layout(const std::string &path)
	{
		layout_path = path;
		if (path.empty())
			return;
		std::string err;
		if (!panel.lay().load(path, err))
			std::fprintf(stderr, "Cannot open layout: %s\n", path.c_str());
		if (!err.empty())
			std::fprintf(stderr, "%s", err.c_str());
	}

	void play_song(const std::string &path)
	{
		play.stop();
		std::string err;
		if (!play.start(path, br, err)) {
			std::fprintf(stderr, "Cannot play %s: %s\n", path.c_str(), err.c_str());
			return;
		}
		std::printf("Playing %s (%d ports)\n", play.name().c_str(), play.ports_used());
	}

	void report_drops()
	{
		if (!eng)
			return;
		const u64 drops = eng->guard_a.dropped() + eng->guard_b.dropped() +
		                  eng->mu.midi_dropped();
		if (drops == reported_drops)
			return;
		reported_drops = drops;
		std::fprintf(stderr,
		             "Dropped %llu MIDI bytes (THRU A %llu / THRU B %llu / in %llu); "
		             "check for a MIDI loop\n",
		             (unsigned long long)drops,
		             (unsigned long long)eng->guard_a.dropped(),
		             (unsigned long long)eng->guard_b.dropped(),
		             (unsigned long long)eng->mu.midi_dropped());
	}

	// One frame: shared tick/status/paint sequence, into any GDI DC.
	void frame(HDC dc, int w, int h)
	{
		// The timer owns this: it touches the bridge, so it must not run on
		// the audio thread (same rule as gui.cpp's WM_TIMER).
		panel.tick(br);
		if (out && out->produced())
			br.set_cpu(float(out->cpu_percent()));
		report_drops();
		card_tick();
		// The PC windows (no-ops while hidden). Double-clicks in the overview
		// open the insertion/part/master windows through open_pc_window.
		ui::pc_frame_all(list, pc, fx, shapes, master, panel.xg(), panel.ram(), br,
		                 [&](ui::pc_window &w) { open_pc_window(w); });

		ui::snapshot s;
		br.read(s);
		const u64 pressed = br.buttons();

		char status[320] = {};
		if (out && out->produced())
			std::snprintf(status, sizeof(status),
			              "Voices %d/128  CPU %.0f%%  worst %.1f ms  starved %llu   IN: %s",
			              s.voices_master + s.voices_slave,
			              out->cpu_percent(), out->worst_ms(),
			              (unsigned long long)out->starved(),
			              in_name[0].empty() ? "none" : in_name[0].c_str());
		else
			std::snprintf(status, sizeof(status), "Booting...");
		panel.set_volume(br.gain());
		panel.paint(dc, s, pressed, status);
		GdiFlush();
		(void)w; (void)h;
	}

	bool hand_cursor(int x, int y)
	{
		return panel.on_midi_jack(x, y) || panel.on_ad_input(x, y) ||
		       panel.on_phones(x, y) || panel.on_card_slot(x, y);
	}

	void card_tick()
	{
		// What the machine wrote to the card goes back to the file every
		// couple of seconds, so a crash loses almost nothing (same as
		// gui.cpp; closing and ejecting flush as well).
		if (!eng || card_path.empty())
			return;
		const u64 now = SDL_GetTicks();
		if (now - last_card_flush < 2000)
			return;
		last_card_flush = now;
		flush_card(*this);
	}
};

// ---- Menu ids ------------------------------------------------------------------
//
// Ranges must not overlap (same warning as gui.cpp: A/D INPUT once lived
// inside MIDI OUT's range and picked the wrong device). Numbered like
// gui.cpp so the two menus stay comparable.

enum {
	ID_IN_NONE = 900, ID_IN_BASE = 901, ID_IN_STRIDE = 500,   // 4 ports
	ID_OUT_NONE = 3000, ID_OUT_BASE = 3001,
	ID_OUTB_NONE = 3500, ID_OUTB_BASE = 3501,
	ID_OUTMU_NONE = 4000, ID_OUTMU_BASE = 4001,
	ID_AIN_NONE = 4500, ID_AIN_BASE = 4501,
	ID_CARD_NEW16 = 5000, ID_CARD_NEW32, ID_CARD_NEW64, ID_CARD_NEW128,
	ID_CARD_OPEN = 5010, ID_CARD_EJECT = 5011,
	ID_PLAY_FILE = 5100, ID_STOP_FILE = 5101,
	ID_PORTS34_FOLD = 5102, ID_PORTS34_DROP = 5103,
	ID_FACTORY = 5200, ID_PC_EDITOR = 5201, ID_OVERVIEW = 5202,
	ID_OUTPUT_DIGITAL = 5300, ID_OUTPUT_ANALOG = 5301,
};
// Submenu markers (returned by the top level, never dispatched).
enum { SUB_IN0, SUB_IN1, SUB_IN2, SUB_IN3, SUB_OUT, SUB_OUTB, SUB_OUTMU, SUB_AIN, SUB_CARD_NEW };

using popup_item = ui::sdl_popup::item;

// Thin wrappers: the shared SDL popup/dialog component works on windows,
// these bind it to this front end.
void alert(app &gui, const char *title, const std::string &text)
{
	ui::sdl_popup::alert(gui.win, title, text);
}

bool confirm(app &gui, const char *title, const std::string &text, const char *ok_label)
{
	return ui::sdl_popup::confirm(gui.win, title, text, ok_label);
}

std::string ask_open_midi(app &gui, std::atomic<bool> &quit)
{
	static const ui::sdl_popup::file_filter f[] = {
		{ "MIDI files", "mid;midi" },
		{ "All files", "*" },
	};
	return ui::sdl_popup::open_file(gui.win, quit, f, 2, "");
}

std::string ask_open_card(app &gui, std::atomic<bool> &quit)
{
	static const ui::sdl_popup::file_filter f[] = {
		{ "SmartMedia images", "img" },
		{ "All files", "*" },
	};
	return ui::sdl_popup::open_file(gui.win, quit, f, 2, "");
}

std::string ask_save_card(app &gui, std::atomic<bool> &quit)
{
	static const ui::sdl_popup::file_filter f[] = {
		{ "SmartMedia images", "img" },
	};
	return ui::sdl_popup::save_file(gui.win, quit, f, 1, "smartmedia.img");
}


// ---- Settings --------------------------------------------------------------------
//
// gui.ini in the per-user settings directory, with the same keys the Windows
// build uses, so one file serves every platform. Names, not numbers, because
// replugging shifts ALSA indices.

const char *const IN_KEYS[mu2000::MIDI_PORTS] = { "midi_in", "midi_in_b",
                                                  "midi_in_c", "midi_in_d" };

std::string settings_path()
{
	const std::string dir = smu2000::config_dir();
	if (dir.empty())
		return {};
	smu2000::ensure_config_dir();
	return dir + "gui.ini";
}

void load_settings(app &gui)
{
	const std::string path = settings_path();
	if (path.empty())
		return;
	FILE *f = std::fopen(path.c_str(), "rb");
	if (!f)
		return;
	char line[512];
	while (std::fgets(line, sizeof(line), f)) {
		std::string t(line);
		while (!t.empty() && (t.back() == '\n' || t.back() == '\r'))
			t.pop_back();
		const size_t eq = t.find('=');
		if (eq == std::string::npos)
			continue;
		const std::string key = t.substr(0, eq), val = t.substr(eq + 1);
		for (int p = 0; p < mu2000::MIDI_PORTS; p++)
			if (key == IN_KEYS[p])
				gui.in_name[p] = val;
		if (key == "midi_out")    gui.out_name = val;
		if (key == "midi_out_b")  gui.out_name_b = val;
		if (key == "midi_out_mu") gui.out_name_mu = val;
		if (key == "audio_out")   gui.audio_name = val;
		if (key == "audio_in")    gui.ain_name = val;
		if (key == "smartmedia")  gui.card_path = val;
		if (key == "ports34")     gui.play.set_fold_extra_ports(val != "drop");
		if (key == "output" && gui.eng) gui.eng->analog.store(val == "analog");
		if (key == "volume" && !val.empty())
			gui.br.set_gain(std::clamp(float(std::atof(val.c_str())), 0.0f, 1.0f));
	}
	std::fclose(f);
}

void save_settings(app &gui)
{
	if (gui.keep_settings)                // --nomidi: leave the file alone
		return;
	const std::string path = settings_path();
	if (path.empty())
		return;
	FILE *f = std::fopen(path.c_str(), "wb");
	if (!f)
		return;
	auto pick = [](const std::string &now, const std::string &keep) {
		return (now.empty() ? keep : now).c_str();
	};
	for (int p = 0; p < mu2000::MIDI_PORTS; p++)
		std::fprintf(f, "%s=%s\n", IN_KEYS[p], pick(gui.in_name[p], gui.in_keep[p]));
	std::fprintf(f, "midi_out=%s\n",    pick(gui.out_name, gui.out_keep));
	std::fprintf(f, "midi_out_b=%s\n",  pick(gui.out_name_b, gui.out_keep_b));
	std::fprintf(f, "midi_out_mu=%s\n", pick(gui.out_name_mu, gui.out_keep_mu));
	std::fprintf(f, "audio_out=%s\n", gui.audio_name.c_str());
	std::fprintf(f, "audio_in=%s\n", gui.ain_name.c_str());
	std::fprintf(f, "smartmedia=%s\n", gui.card_path.c_str());
	// The VOLUME knob is analogue on the real machine, outside the firmware's
	// RAM, so it is remembered here.
	std::fprintf(f, "volume=%.3f\n", gui.br.gain());
	std::fprintf(f, "ports34=%s\n", gui.play.fold_extra_ports() ? "fold" : "drop");
	if (gui.eng)
		std::fprintf(f, "output=%s\n", gui.eng->analog.load() ? "analog" : "digital");
	std::fclose(f);
}

// Exact-name lookup. -1 when missing (a device that is not running yet keeps
// its remembered name instead of being forgotten).
int find_device(const std::vector<std::string> &names, const std::string &want)
{
	if (want.empty())
		return -1;
	for (size_t i = 0; i < names.size(); i++)
		if (names[i] == want)
			return int(i);
	return -1;
}

// ---- Port choosing ------------------------------------------------------------------
//
// keep == true at startup: a port that will not open keeps its remembered
// name, so a device that is not running yet is not forgotten by the next start.

bool choose_in(app &gui, int port, int dev, bool keep = false)
{
	if (port < 0 || port >= mu2000::MIDI_PORTS)
		return false;
	if (!keep)
		gui.in_keep[port].clear();
	std::string err;
	gui.last_error.clear();
	if (!gui.midi[port].open(dev, err)) {
		gui.last_error = err;
		std::fprintf(stderr, "MIDI in %c: %s\n", char('A' + port), err.c_str());
		gui.midi[port].open(-1, err);
		dev = -1;
	}
	gui.in_dev[port]  = gui.midi[port].is_open() ? dev : -1;
	gui.in_name[port] = gui.midi[port].device_name();
	save_settings(gui);
	return err.empty();
}

bool choose_out(app &gui, int dev, bool keep = false)
{
	if (!gui.mout)
		return false;
	if (!keep)
		gui.out_keep.clear();
	std::string err;
	gui.last_error.clear();
	if (!gui.mout->open(dev, err)) {
		gui.last_error = err;
		std::fprintf(stderr, "MIDI THRU A: %s\n", err.c_str());
		gui.mout->open(-1, err);
		dev = -1;
	}
	gui.out_dev  = gui.mout->is_open() ? dev : -1;
	gui.out_name = gui.mout->device_name();
	save_settings(gui);
	return err.empty();
}

bool choose_out_b(app &gui, int dev, bool keep = false)
{
	if (!gui.mout_b)
		return false;
	if (!keep)
		gui.out_keep_b.clear();
	std::string err;
	gui.last_error.clear();
	if (!gui.mout_b->open(dev, err)) {
		gui.last_error = err;
		std::fprintf(stderr, "MIDI THRU B: %s\n", err.c_str());
		gui.mout_b->open(-1, err);
		dev = -1;
	}
	gui.out_dev_b  = gui.mout_b->is_open() ? dev : -1;
	gui.out_name_b = gui.mout_b->device_name();
	save_settings(gui);
	return err.empty();
}

bool choose_out_mu(app &gui, int dev, bool keep = false)
{
	if (!gui.mout_mu)
		return false;
	if (!keep)
		gui.out_keep_mu.clear();
	std::string err;
	gui.last_error.clear();
	if (!gui.mout_mu->open(dev, err)) {
		gui.last_error = err;
		std::fprintf(stderr, "MIDI OUT: %s\n", err.c_str());
		gui.mout_mu->open(-1, err);
		dev = -1;
	}
	gui.out_dev_mu  = gui.mout_mu->is_open() ? dev : -1;
	gui.out_name_mu = gui.mout_mu->device_name();
	save_settings(gui);
	return err.empty();
}

bool choose_ain(app &gui, const std::string &name)
{
	if (!gui.ain)
		return false;
	gui.ain->stop();
	if (name.empty()) {
		gui.ain_name.clear();
		save_settings(gui);
		return true;
	}
	std::string err;
	if (!gui.ain->start(name, err)) {
		std::fprintf(stderr, "A/D INPUT: %s\n", err.c_str());
		alert(gui, "S-MU2000", "A/D INPUT: " + err);
		return false;
	}
	gui.ain_name = gui.ain->device_name();
	std::printf("A/D INPUT: %s (%s)\n", gui.ain_name.c_str(),
	            gui.ain->format_line().c_str());
	save_settings(gui);
	return true;
}

// ---- SmartMedia -----------------------------------------------------------------------

void flush_card(app &gui)
{
	if (!gui.eng || gui.card_path.empty())
		return;
	std::vector<smu2000::smartmedia::block> blocks;
	{
		const std::lock_guard<std::mutex> hold(gui.eng->card_lock);
		gui.eng->mu.card().take_dirty_blocks(blocks);
	}
	std::string err;
	if (!smu2000::smartmedia::write_blocks(gui.card_path, blocks, err))
		std::fprintf(stderr, "SmartMedia: %s\n", err.c_str());
}

void eject_card(app &gui)
{
	if (!gui.eng)
		return;
	flush_card(gui);
	{
		const std::lock_guard<std::mutex> hold(gui.eng->card_lock);
		gui.eng->mu.card().eject();
	}
	if (!gui.card_path.empty())
		std::printf("Ejected SmartMedia: %s\n", gui.card_path.c_str());
	gui.card_path.clear();
}

// Reads without stopping the audio thread; only the swap takes the lock.
bool insert_card(app &gui, const std::string &path, bool quiet = false)
{
	if (!gui.eng || path.empty())
		return false;
	smu2000::smartmedia card;
	std::string err;
	if (!card.load(path, err)) {
		if (!quiet)
			alert(gui, "S-MU2000", "SmartMedia: " + err);
		std::fprintf(stderr, "SmartMedia: %s\n", err.c_str());
		return false;
	}
	eject_card(gui);
	{
		const std::lock_guard<std::mutex> hold(gui.eng->card_lock);
		gui.eng->mu.card() = std::move(card);
	}
	gui.card_path = path;
	std::printf("Inserted SmartMedia: %s (%u MB)\n", path.c_str(),
	            gui.eng->mu.card().megabytes());
	save_settings(gui);
	return true;
}

// A blank card, physical layout only: format it on the machine
// (UTIL -> CARD -> Format) before it holds anything.
void new_card(app &gui, std::atomic<bool> &quit, u32 megabytes)
{
	const std::string path = ask_save_card(gui, quit);
	if (path.empty() || quit.load())
		return;
	smu2000::smartmedia card;
	card.create(megabytes);
	std::string err;
	if (!card.save(path, err)) {
		alert(gui, "S-MU2000", "SmartMedia: " + err);
		return;
	}
	if (insert_card(gui, path))
		alert(gui, "S-MU2000",
		      "Inserted a blank SmartMedia.\nFormat it first: UTIL -> CARD -> Format.");
}

void play_midi_file(app &gui, const std::string &path)
{
	std::string err;
	if (!gui.play.start(path, gui.br, err)) {
		alert(gui, "S-MU2000", "Cannot open: " + err);
		return;
	}
	std::printf("Playing %s (%.1f s)\n", path.c_str(), gui.play.length());
	if (gui.play.ports_used() > 2)
		std::printf("  This song uses %d ports. Ports 3+ are %s\n",
		            gui.play.ports_used(),
		            gui.play.fold_extra_ports() ? "folded into A and B" : "dropped");
	std::fflush(stdout);
}

void choose_factory_reset(app &gui)
{
	if (!gui.eng || gui.eng->state.load() != 1)
		return;
	if (!confirm(gui, "S-MU2000",
	             "Reset the MU2000 to factory state and reboot?\n"
	             "Utility settings and remembered volume/voice settings are all lost.",
	             "Reset"))
		return;
	gui.play.stop();
	if (gui.reboot.joinable())
		gui.reboot.join();
	gui.reboot = std::thread([&gui] { gui.eng->factory_reset(); });
}

// ---- Popup menu contents -----------------------------------------------------------
//
// Same items in the same order as gui.cpp / gui_mac.cpp, in English. Port
// categories open a second flat list (two levels at most).

const char *const IN_TITLES[mu2000::MIDI_PORTS] = {
	"MIDI IN A (parts 1-16)", "MIDI IN B (parts 17-32)",
	"MIDI IN C (parts 33-48)", "MIDI IN D (parts 49-64)",
};

popup_item mi(const char *label, int id, bool checked, bool enabled)
{
	return popup_item{ label, id, checked, enabled, false, false, -1 };
}

popup_item misep()
{
	popup_item it;
	it.separator = true;
	return it;
}

popup_item misub(const char *label, int sub)
{
	popup_item it;
	it.label = label;
	it.id = -1;
	it.submenu = true;
	it.sub = sub;
	return it;
}

void fill_port_items(std::vector<popup_item> &out, const std::vector<std::string> &names,
                     int now, int id_none, int id_base)
{
	out.push_back(mi("Off", id_none, now < 0, true));
	out.push_back(misep());
	if (names.empty()) {
		out.push_back(mi("(no devices)", 0, false, false));
		return;
	}
	for (size_t i = 0; i < names.size(); i++)
		out.push_back(mi(names[i].c_str(), id_base + int(i), int(i) == now, true));
}

// Shows one flat list; follows a single submenu level. Returns the final id or -1.
// One flat list through the shared popup runner, repainting the live panel
// behind it. Follows a single submenu level for the two-tier menus.
int run_list(app &gui, framebuf &fb, SDL_Texture *tex, int ww, int wh,
             std::atomic<bool> &quit, const std::vector<popup_item> &items,
             int x, int y, int &sub)
{
	auto behind = [&] { gui.frame(fb.dc, ww, wh); };
	return ui::sdl_popup::run(gui.win, gui.ren, tex, fb.bits, ww, wh, behind, quit,
	                      items, x, y, sub);
}

int show_menu(app &gui, framebuf &fb, SDL_Texture *tex, int ww, int wh,
              std::atomic<bool> &quit, const std::vector<popup_item> &items, int x, int y,
              const std::vector<popup_item> &sub, int sx, int sy)
{
	int which = -1;
	const int id = run_list(gui, fb, tex, ww, wh, quit, items, x, y, which);
	if (id < 0 || which < 0 || sub.empty())
		return id;
	int dummy = -1;
	return run_list(gui, fb, tex, ww, wh, quit, sub, sx, sy, dummy);
}

void fail_note(app &gui, bool ok)
{
	if (!ok && !gui.last_error.empty()) {
		alert(gui, "S-MU2000", gui.last_error);
		gui.last_error.clear();
	}
}

void menu_chosen(app &gui, std::atomic<bool> &quit, int id)
{
	for (int p = 0; p < mu2000::MIDI_PORTS; p++) {
		const int none = ID_IN_NONE + p * ID_IN_STRIDE, base = ID_IN_BASE + p * ID_IN_STRIDE;
		if (id == none)                         { fail_note(gui, choose_in(gui, p, -1)); return; }
		if (id >= base && id < base + 256)      { fail_note(gui, choose_in(gui, p, id - base)); return; }
	}
	if (id == ID_OUT_NONE)                                   { fail_note(gui, choose_out(gui, -1)); return; }
	if (id >= ID_OUT_BASE && id < ID_OUT_BASE + 256)         { fail_note(gui, choose_out(gui, id - ID_OUT_BASE)); return; }
	if (id == ID_OUTB_NONE)                                  { fail_note(gui, choose_out_b(gui, -1)); return; }
	if (id >= ID_OUTB_BASE && id < ID_OUTB_BASE + 256)       { fail_note(gui, choose_out_b(gui, id - ID_OUTB_BASE)); return; }
	if (id == ID_OUTMU_NONE)                                 { fail_note(gui, choose_out_mu(gui, -1)); return; }
	if (id >= ID_OUTMU_BASE && id < ID_OUTMU_BASE + 256)     { fail_note(gui, choose_out_mu(gui, id - ID_OUTMU_BASE)); return; }
	if (id == ID_AIN_NONE)                                   { choose_ain(gui, ""); return; }
	if (id >= ID_AIN_BASE && id < ID_AIN_BASE + 256) {
		const auto names = ui::audio_in::list();
		if (id - ID_AIN_BASE < int(names.size()))
			choose_ain(gui, names[size_t(id - ID_AIN_BASE)]);
		return;
	}
	if (id >= ID_CARD_NEW16 && id <= ID_CARD_NEW128) {
		new_card(gui, quit, 16u << (id - ID_CARD_NEW16));
		return;
	}
	if (id == ID_CARD_OPEN) {
		const std::string path = ask_open_card(gui, quit);
		if (!path.empty() && !quit.load())
			insert_card(gui, path);
		return;
	}
	if (id == ID_CARD_EJECT) { eject_card(gui); save_settings(gui); return; }
	if (id == ID_PLAY_FILE) {
		const std::string path = ask_open_midi(gui, quit);
		if (!path.empty() && !quit.load())
			play_midi_file(gui, path);
		return;
	}
	if (id == ID_STOP_FILE) { gui.play.stop(); return; }
	if (id == ID_PORTS34_FOLD || id == ID_PORTS34_DROP) {
		gui.play.set_fold_extra_ports(id == ID_PORTS34_FOLD);
		save_settings(gui);
		return;
	}
	if (id == ID_FACTORY) { choose_factory_reset(gui); return; }
	if (id == ID_PC_EDITOR) { gui.open_pc_window(gui.pc); return; }
	if (id == ID_OVERVIEW) { gui.open_pc_window(gui.list); return; }
	if ((id == ID_OUTPUT_DIGITAL || id == ID_OUTPUT_ANALOG) && gui.eng) {
		gui.eng->analog.store(id == ID_OUTPUT_ANALOG);
		std::printf("Audio path: %s\n", id == ID_OUTPUT_ANALOG ? "analog (cuts DC)" : "digital");
		save_settings(gui);
		return;
	}
}

void show_port_sub(app &gui, framebuf &fb, SDL_Texture *tex, int ww, int wh,
                   std::atomic<bool> &quit, int x, int y, const char *title,
                   const std::vector<std::string> &names, int now, int id_none, int id_base)
{
	std::vector<popup_item> items;
	items.push_back(mi(title, 0, false, false));
	items.push_back(misep());
	fill_port_items(items, names, now, id_none, id_base);
	int dummy = -1;
	menu_chosen(gui, quit, run_list(gui, fb, tex, ww, wh, quit, items, x, y, dummy));
}

void show_top_menu(app &gui, framebuf &fb, SDL_Texture *tex, int ww, int wh,
                   std::atomic<bool> &quit, int x, int y)
{
	// MIDI IN is four ports. C and D exist only over USB on the real machine
	// and reach parts 33-64.
	std::vector<popup_item> top;
	for (int p = 0; p < mu2000::MIDI_PORTS; p++)
		top.push_back(misub(IN_TITLES[p], SUB_IN0 + p));
	top.push_back(misub("MIDI OUT (from the MU2000)", SUB_OUTMU));
	top.push_back(misub("MIDI THRU A (echo of A)", SUB_OUT));
	top.push_back(misub("MIDI THRU B (echo of B)", SUB_OUTB));
	top.push_back(misub("A/D INPUT (sample source)", SUB_AIN));
	top.push_back(misep());
	top.push_back(mi("Open list (F3)", ID_OVERVIEW, false, true));
	top.push_back(mi("Open editor (F2)", ID_PC_EDITOR, false, true));
	top.push_back(misep());
	top.push_back(mi("Factory reset...", ID_FACTORY, false, gui.ready()));

	int sub = -1;
	const int id = run_list(gui, fb, tex, ww, wh, quit, top, x, y, sub);
	if (id == ID_OVERVIEW || id == ID_PC_EDITOR || id == ID_FACTORY || quit.load()) {
		menu_chosen(gui, quit, id);
		return;
	}
	if (sub < 0)
		return;
	const auto ins = ui::midi_in::list();
	const auto outs = ui::midi_out::list();
	const int sx = std::min(x + 200, std::max(0, ww - 300));
	if (sub >= SUB_IN0 && sub <= SUB_IN3) {
		const int p = sub - SUB_IN0;
		show_port_sub(gui, fb, tex, ww, wh, quit, sx, y, IN_TITLES[p], ins,
		              gui.in_dev[p], ID_IN_NONE + p * ID_IN_STRIDE,
		              ID_IN_BASE + p * ID_IN_STRIDE);
	} else if (sub == SUB_OUTMU) {
		show_port_sub(gui, fb, tex, ww, wh, quit, sx, y, "MIDI OUT (from the MU2000)",
		              outs, gui.out_dev_mu, ID_OUTMU_NONE, ID_OUTMU_BASE);
	} else if (sub == SUB_OUT) {
		show_port_sub(gui, fb, tex, ww, wh, quit, sx, y, "MIDI THRU A (echo of A)",
		              outs, gui.out_dev, ID_OUT_NONE, ID_OUT_BASE);
	} else if (sub == SUB_OUTB) {
		show_port_sub(gui, fb, tex, ww, wh, quit, sx, y, "MIDI THRU B (echo of B)",
		              outs, gui.out_dev_b, ID_OUTB_NONE, ID_OUTB_BASE);
	} else if (sub == SUB_AIN) {
		// Chosen by name: the recording device list carries no stable index.
		std::vector<popup_item> items;
		items.push_back(mi("A/D INPUT (sample source)", 0, false, false));
		items.push_back(misep());
		const auto names = ui::audio_in::list();
		items.push_back(mi("Off", ID_AIN_NONE, gui.ain_name.empty(), true));
		items.push_back(misep());
		if (names.empty())
			items.push_back(mi("(no devices)", 0, false, false));
		for (size_t i = 0; i < names.size(); i++)
			items.push_back(mi(names[i].c_str(), ID_AIN_BASE + int(i),
			                   names[i] == gui.ain_name, true));
		int dummy = -1;
		menu_chosen(gui, quit, run_list(gui, fb, tex, ww, wh, quit, items, sx, y, dummy));
	}
}

void show_card_menu(app &gui, framebuf &fb, SDL_Texture *tex, int ww, int wh,
                    std::atomic<bool> &quit, int x, int y)
{
	// The card slot is about the SmartMedia in it, then about the MIDI file
	// player. Same items in the same order as gui.cpp.
	const bool card_in = !gui.card_path.empty();
	std::vector<popup_item> fresh;
	fresh.push_back(mi("16MB", ID_CARD_NEW16, false, true));
	fresh.push_back(mi("32MB", ID_CARD_NEW32, false, true));
	fresh.push_back(mi("64MB", ID_CARD_NEW64, false, true));
	fresh.push_back(mi("128MB", ID_CARD_NEW128, false, true));

	std::vector<popup_item> top;
	top.push_back(misub("New SmartMedia...", SUB_CARD_NEW));
	top.push_back(mi("Open SmartMedia...", ID_CARD_OPEN, false, true));
	std::string eject = "Eject";
	if (card_in) {
		const size_t slash = gui.card_path.find_last_of('/');
		eject += " (" + gui.card_path.substr(slash == std::string::npos ? 0 : slash + 1) + ")";
	}
	top.push_back(mi(eject.c_str(), ID_CARD_EJECT, false, card_in));
	top.push_back(misep());
	top.push_back(mi("Play MIDI file...", ID_PLAY_FILE, false, true));
	std::string stop = "Stop";
	if (gui.play.playing())
		stop += " (" + gui.play.name() + ")";
	top.push_back(mi(stop.c_str(), ID_STOP_FILE, false, gui.play.playing()));
	top.push_back(misep());
	const bool fold = gui.play.fold_extra_ports();
	top.push_back(mi("Fold ports 3+4 into A+B", ID_PORTS34_FOLD, fold, true));
	top.push_back(mi("Drop ports 3+4", ID_PORTS34_DROP, !fold, true));

	const int sx = std::min(x + 200, std::max(0, ww - 200));
	menu_chosen(gui, quit, show_menu(gui, fb, tex, ww, wh, quit, top, x, y, fresh, sx, y));
}

void show_phones_menu(app &gui, framebuf &fb, SDL_Texture *tex, int ww, int wh,
                      std::atomic<bool> &quit, int x, int y)
{
	// The PHONES jack is about the output. Digital is what S/PDIF carries,
	// DPCM DC included; analogue removes the DC (src/analog_out.h).
	const bool analog = gui.eng && gui.eng->analog.load();
	std::vector<popup_item> items;
	items.push_back(mi("Audio output", 0, false, false));
	items.push_back(misep());
	items.push_back(mi("Digital (S/PDIF, keeps DPCM DC)", ID_OUTPUT_DIGITAL, !analog, true));
	items.push_back(mi("Analog (LINE OUT/PHONES, cuts DC)", ID_OUTPUT_ANALOG, analog, true));
	int dummy = -1;
	menu_chosen(gui, quit, run_list(gui, fb, tex, ww, wh, quit, items, x, y, dummy));
}

void show_ain_menu(app &gui, framebuf &fb, SDL_Texture *tex, int ww, int wh,
                   std::atomic<bool> &quit, int x, int y)
{
	std::vector<popup_item> items;
	items.push_back(mi("A/D INPUT (sample source)", 0, false, false));
	items.push_back(misep());
	const auto names = ui::audio_in::list();
	items.push_back(mi("Off", ID_AIN_NONE, gui.ain_name.empty(), true));
	items.push_back(misep());
	if (names.empty())
		items.push_back(mi("(no devices)", 0, false, false));
	for (size_t i = 0; i < names.size(); i++)
		items.push_back(mi(names[i].c_str(), ID_AIN_BASE + int(i),
		                   names[i] == gui.ain_name, true));
	int dummy = -1;
	menu_chosen(gui, quit, run_list(gui, fb, tex, ww, wh, quit, items, x, y, dummy));
}

static app *g_app = nullptr;

// A MIDI file dropped on any window plays (the panel or an editor window).
void play_dropped_file(const std::string &path)
{
	if (g_app)
		g_app->play_song(path);
}

int run_window(app &gui, engine &eng, int win_w, int win_h, int latency,
               bool exclusive, const std::string &play_path, const std::string &ain_name,
               double seconds)
{
	if (!SDL_Init(SDL_INIT_VIDEO)) {
		std::fprintf(stderr, "Cannot open a window: %s\n", SDL_GetError());
		return 1;
	}

	SDL_Window *win = SDL_CreateWindow("S-MU2000", win_w, win_h, SDL_WINDOW_RESIZABLE);
	if (!win) {
		std::fprintf(stderr, "Cannot open a window: %s\n", SDL_GetError());
		SDL_Quit();
		return 1;
	}
	SDL_Renderer *ren = SDL_CreateRenderer(win, nullptr);
	if (!ren) {
		std::fprintf(stderr, "Cannot make a renderer: %s\n", SDL_GetError());
		SDL_DestroyWindow(win);
		SDL_Quit();
		return 1;
	}
	SDL_Texture *tex = SDL_CreateTexture(ren, SDL_PIXELFORMAT_ARGB8888,
	                                     SDL_TEXTUREACCESS_STREAMING, win_w, win_h);
	if (!tex) {
		std::fprintf(stderr, "Cannot make a texture: %s\n", SDL_GetError());
		SDL_DestroyRenderer(ren);
		SDL_DestroyWindow(win);
		SDL_Quit();
		return 1;
	}
	// Bytes copy as-is: Cairo's premultiplied edges are uploaded untouched,
	// which is what makes the DIB-vs-window comparison meaningful.
	SDL_SetTextureBlendMode(tex, SDL_BLENDMODE_NONE);
	SDL_SetTextureScaleMode(tex, SDL_SCALEMODE_NEAREST);

	SDL_Cursor *cur_hand = SDL_CreateSystemCursor(SDL_SYSTEM_CURSOR_POINTER);
	SDL_Cursor *cur_arrow = SDL_CreateSystemCursor(SDL_SYSTEM_CURSOR_DEFAULT);

	framebuf fb;
	if (!fb.reset(win_w, win_h)) {
		std::fprintf(stderr, "Cannot make a %dx%d surface\n", win_w, win_h);
		return 1;
	}
	gui.panel.resize(win_w, win_h);

	static ui::audio_out out;
	static ui::audio_in ain;
	gui.out = &out;
	gui.ain = &ain;
	gui.eng = &eng;
	gui.win = win;
	gui.ren = ren;
	gui.fb = &fb;
	gui.ww = win_w;
	gui.wh = win_h;

	std::atomic<bool> quit{ false };

	// Boot on a separate thread, and start the audio once it is done
	std::thread boot_thread([&] {
		eng.publish();   // the window comes up showing the boot message
		if (!eng.boot()) {
			eng.state.store(2);
			eng.publish();
			return;
		}
		eng.state.store(1);
		eng.publish();
		if (quit.load())
			return;

		std::string err;
		if (!out.start(latency, [&](s16 *o, u32 n) { eng.fill(o, n); }, err, exclusive,
		               gui.audio_name)) {
			std::fprintf(stderr, "Audio: %s\n", err.c_str());
			eng.message = "Cannot open the audio device";
			eng.state.store(2);
			eng.publish();
			return;
		}
		gui.audio_name = out.device_name();
		std::printf("Audio out: %s\n", out.device_name().c_str());
		if (!ain_name.empty()) {
			std::string aerr;
			if (ain.start(ain_name, aerr))
				std::printf("A/D input: %s (%s)\n", ain.device_name().c_str(),
				            ain.format_line().c_str());
			else
				std::printf("A/D input: none (%s)\n", aerr.c_str());
		}
		if (exclusive)
			std::printf("Exclusive: %s\n", out.exclusive() ? "held" : "refused");
		std::printf("Playing (latency %.1f ms)\n", 1000.0 * out.buffer_frames() / RATE);
		std::fflush(stdout);
		if (!play_path.empty())
			gui.play_song(play_path);
	});

	bool down = false;   // left button held: drags go to the panel
	int ww = win_w, wh = win_h;
	const Uint64 quit_at = seconds > 0.0 ? SDL_GetTicks() + Uint64(seconds * 1000.0) : 0;

	g_app = &gui;
	ui::pc_window::set_drop_handler(play_dropped_file);
	// With --editor and friends, open those windows with the panel
	if (gui.open_pc)
		gui.open_pc_window(gui.pc);
	if (gui.open_fx)
		gui.open_pc_window(gui.fx);
	if (gui.open_list)
		gui.open_pc_window(gui.list);

	while (!quit.load()) {
		if (quit_at && SDL_GetTicks() >= quit_at)
			break;   // --seconds: timed run, for smoke tests and demos
		const Uint64 frame_at = SDL_GetTicks() + 33;
		SDL_Event ev;
		while (SDL_PollEvent(&ev)) {
			// PC editor windows first: they own their SDL windows and eat
			// their events (including drops and the close button).
			if (ui::pc_window::route_event(ev))
				continue;
			switch (ev.type) {
			case SDL_EVENT_QUIT:
				quit.store(true);
				break;
			case SDL_EVENT_WINDOW_RESIZED:
				ww = ev.window.data1;
				wh = ev.window.data2;
				gui.ww = ww;
				gui.wh = wh;
				if (fb.reset(ww, wh)) {
					SDL_DestroyTexture(tex);
					tex = SDL_CreateTexture(ren, SDL_PIXELFORMAT_ARGB8888,
					                        SDL_TEXTUREACCESS_STREAMING, ww, wh);
					if (tex)
						SDL_SetTextureBlendMode(tex, SDL_BLENDMODE_NONE);
					gui.panel.resize(ww, wh);
				}
				break;
			case SDL_EVENT_WINDOW_FOCUS_LOST:
				gui.br.release_all();   // leaving the window releases everything
				break;
			case SDL_EVENT_MOUSE_BUTTON_DOWN: {
				const int mx = int(ev.button.x), my = int(ev.button.y);
				const bool card = gui.panel.on_card_slot(mx, my);
				const bool phones = gui.panel.on_phones(mx, my);
				const bool jack = gui.panel.on_midi_jack(mx, my);
				const bool ad = gui.panel.on_ad_input(mx, my);
				if (ev.button.button == SDL_BUTTON_LEFT) {
					// The jacks and the card slot open menus (like the other
					// platforms); anywhere else presses a panel button.
					if (card)
						show_card_menu(gui, fb, tex, ww, wh, quit, mx, my);
					else if (phones)
						show_phones_menu(gui, fb, tex, ww, wh, quit, mx, my);
					else if (jack)
						show_top_menu(gui, fb, tex, ww, wh, quit, mx, my);
					else if (ad)
						show_ain_menu(gui, fb, tex, ww, wh, quit, mx, my);
					else {
						down = true;
						gui.panel.press(mx, my, gui.br);
					}
				} else if (ev.button.button == SDL_BUTTON_RIGHT) {
					if (card)
						show_card_menu(gui, fb, tex, ww, wh, quit, mx, my);
					else if (phones)
						show_phones_menu(gui, fb, tex, ww, wh, quit, mx, my);
					else
						show_top_menu(gui, fb, tex, ww, wh, quit, mx, my);
				}
				break;
			}
			case SDL_EVENT_MOUSE_BUTTON_UP:
				if (ev.button.button == SDL_BUTTON_LEFT && down) {
					down = false;
					gui.panel.release(gui.br);
				}
				break;
			case SDL_EVENT_MOUSE_MOTION:
				if (down)
					gui.panel.drag(int(ev.motion.x), int(ev.motion.y), gui.br);
				else if (cur_hand && cur_arrow)
					SDL_SetCursor(gui.hand_cursor(int(ev.motion.x), int(ev.motion.y))
					                  ? cur_hand
					                  : cur_arrow);
				break;
			case SDL_EVENT_MOUSE_WHEEL: {
				float fx, fy;
				SDL_GetMouseState(&fx, &fy);
				const int steps = int(ev.wheel.y > 0 ? 1 : ev.wheel.y < 0 ? -1 : 0);
				if (steps)
					gui.panel.wheel_at(int(fx), int(fy), steps, gui.br);
				break;
			}
			case SDL_EVENT_KEY_DOWN: {
				if (ev.key.repeat)
					break;   // held-key repeats are ignored
				const SDL_Keycode k = ev.key.key;
				if (k == SDLK_F5) {
					gui.set_layout(gui.layout_path);
					break;
				}
				if (k == SDLK_F2) {             // PC editor
					gui.open_pc_window(gui.pc);
					break;
				}
				if (k == SDLK_F3) {             // overview
					gui.open_pc_window(gui.list);
					break;
				}
				mu2000::button b = mu2000::button::count;
				if (key_to_button(k < 128 ? int(k) : 0, b))
					gui.br.press(b, true);
				break;
			}
			case SDL_EVENT_KEY_UP: {
				mu2000::button b = mu2000::button::count;
				if (key_to_button(ev.key.key < 128 ? int(ev.key.key) : 0, b))
					gui.br.press(b, false);
				break;
			}
			case SDL_EVENT_DROP_FILE:
				if (ev.drop.data) {
					gui.play_song(ev.drop.data);
					SDL_free(const_cast<char *>(ev.drop.data));
				}
				break;
			default:
				break;
			}
		}

		gui.frame(fb.dc, ww, wh);
		if (tex) {
			SDL_UpdateTexture(tex, nullptr, fb.bits, ww * 4);
			SDL_RenderTexture(ren, tex, nullptr, nullptr);
			SDL_RenderPresent(ren);
		}
		const Uint64 now = SDL_GetTicks();
		if (frame_at > now)
			SDL_Delay(Uint32(frame_at - now));
	}

	// Tell the editor windows we are closing, then tear their SDL
	// resources down here: SDL_Quit below would strand them.
	ui::pc_shutdown_all(gui.list, gui.pc, gui.fx, gui.shapes, gui.master, gui.br);
	gui.list.close();
	gui.pc.close();
	gui.fx.close();
	gui.shapes.close();
	gui.master.close();
	g_app = nullptr;

	// Snapshot the audio counters first: stop() drops the impl they live in.
	const u64 audio_done = out.produced();
	const double audio_cpu = out.cpu_percent(), audio_worst = out.worst_ms();
	const u64 audio_starved = out.starved();
	gui.play.stop();
	out.stop();
	ain.stop();
	// Leaving THRU open with held notes would stick them on whatever listens.
	if (gui.mout)
		for (int ch = 0; ch < 16; ch++)
			for (u8 v : { u8(0xb0 | ch), u8(120), u8(0), u8(0xb0 | ch), u8(123), u8(0) }) {
				gui.mout->send(v);
				if (gui.mout_b)
					gui.mout_b->send(v);
			}
	flush_card(gui);   // the sound has stopped; keep what was written to the card
	save_settings(gui);
	// The sound has stopped by now. Keep the machine's settings only if it came up
	if (eng.state.load() == 1 && !smu2000::nvram::save(eng.mu))
		std::fprintf(stderr, "Cannot save settings: %s\n",
		             smu2000::nvram::path(eng.mu).c_str());
	for (int p = 0; p < mu2000::MIDI_PORTS; p++)
		gui.midi[p].close();
	if (gui.mout)
		gui.mout->close();
	if (gui.mout_b)
		gui.mout_b->close();
	if (gui.mout_mu)
		gui.mout_mu->close();

	if (gui.reboot.joinable())
		gui.reboot.join();
	if (boot_thread.joinable())
		boot_thread.join();
	if (audio_done)
		std::printf("CPU %.1f%%, worst %.2f ms, starved %llu\n", audio_cpu,
		            audio_worst, (unsigned long long)audio_starved);

	if (cur_hand) SDL_DestroyCursor(cur_hand);
	if (cur_arrow) SDL_DestroyCursor(cur_arrow);
	SDL_DestroyTexture(tex);
	SDL_DestroyRenderer(ren);
	SDL_DestroyWindow(win);
	SDL_Quit();
	return 0;
}

} // namespace


int main(int argc, char **argv)
{
	// The Linux GUI shows English. Installed before anything paints or prints.
	// The PC views read editor.ini when first asked; a lang= there still wins.
	ui::set_texts(ui::english_texts());
	ui::xgui::set_help_lang(1);
	smu2000::init_console_utf8();

	std::string dir, shot_path, dump_layout, play_path;
	std::string layout_path, audio_dev, ain_dev;
	int in_dev[mu2000::MIDI_PORTS] = { -2, -2, -2, -2 };  // -2 unset, -1 unused
	int out_dev = -2, out_dev_b = -2, out_dev_mu = -2;
	int latency = 30;
	bool exclusive = false, factory = false, fast_midi = false, nomidi = false;
	bool usb_host = true;
	int win_w = 1400, win_h = 360;
	bool grid = false;
	bool boot_for_shot = false;
	bool run_selftest = false;
	bool open_pc = false;          // open the PC editor with the panel
	bool open_list = false;        // open the overview with the panel
	bool open_fx = false;          // open the insertion settings with the panel
	double seconds = 0.0;   // 0 runs until the window closes
	std::string shot_mid;
	double shot_secs = 0.0;

	for (int i = 1; i < argc; i++) {
		if (!std::strcmp(argv[i], "--list")) {
			std::printf("MIDI in (--midi N; ports A-D):\n");
			const auto ins = ui::midi_in::list();
			for (size_t k = 0; k < ins.size(); k++)
				std::printf("  %zu: %s\n", k, ins[k].c_str());
			if (ins.empty())
				std::printf("  (none)\n");
			std::printf("Audio out (--audio part-of-name):\n");
			const auto aouts = ui::audio_out::list();
			for (size_t k = 0; k < aouts.size(); k++)
				std::printf("  %zu: %s\n", k, aouts[k].c_str());
			std::printf("Audio in / A/D INPUT (--audio-in name):\n");
			const auto ains = ui::audio_in::list();
			for (size_t k = 0; k < ains.size(); k++)
				std::printf("  %zu: %s\n", k, ains[k].c_str());
			if (ains.empty())
				std::printf("  (none)\n");
			return 0;
		}
		else if (!std::strcmp(argv[i], "--midi") && i + 1 < argc) in_dev[0] = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--midi-b") && i + 1 < argc) in_dev[1] = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--midi-c") && i + 1 < argc) in_dev[2] = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--midi-d") && i + 1 < argc) in_dev[3] = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--midiout") && i + 1 < argc) out_dev = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--midiout-b") && i + 1 < argc) out_dev_b = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--midiout-mu") && i + 1 < argc) out_dev_mu = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--nomidi")) {
			for (int &d : in_dev) d = -1;
			nomidi = true;
		}
		else if (!std::strcmp(argv[i], "--latency") && i + 1 < argc) latency = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--audio") && i + 1 < argc) audio_dev = argv[++i];
		else if (!std::strcmp(argv[i], "--audio-in") && i + 1 < argc) ain_dev = argv[++i];
		else if (!std::strcmp(argv[i], "--exclusive")) exclusive = true;
		else if (!std::strcmp(argv[i], "--factory")) factory = true;
		else if (!std::strcmp(argv[i], "--fast-midi")) fast_midi = true;
		else if (!std::strcmp(argv[i], "--usb")) usb_host = true;
		else if (!std::strcmp(argv[i], "--host-midi")) usb_host = false;
		else if (!std::strcmp(argv[i], "--play") && i + 1 < argc) play_path = argv[++i];
		else if (!std::strcmp(argv[i], "--editor")) open_pc = true;
		else if (!std::strcmp(argv[i], "--list-window")) open_list = true;
		else if (!std::strcmp(argv[i], "--fx-window")) open_fx = true;
		else if (!std::strcmp(argv[i], "--shot") && i + 1 < argc) shot_path = argv[++i];
		else if (!std::strcmp(argv[i], "--boot")) boot_for_shot = true;
		else if (!std::strcmp(argv[i], "--grid")) grid = true;
		else if (!std::strcmp(argv[i], "--layout") && i + 1 < argc) layout_path = argv[++i];
		else if (!std::strcmp(argv[i], "--dump-layout") && i + 1 < argc) dump_layout = argv[++i];
		else if (!std::strcmp(argv[i], "--selftest")) run_selftest = true;
		else if (!std::strcmp(argv[i], "--seconds") && i + 1 < argc) seconds = std::atof(argv[++i]);
		else if (!std::strcmp(argv[i], "--mid") && i + 2 < argc) {
			shot_mid = argv[++i];
			shot_secs = std::atof(argv[++i]);
			boot_for_shot = true;
		}
		else if (!std::strcmp(argv[i], "--size") && i + 1 < argc) {
			if (std::sscanf(argv[++i], "%dx%d", &win_w, &win_h) != 2) { win_w = 1400; win_h = 360; }
		}
		else if (dir.empty()) dir = argv[i];
	}
	// Without --layout, look through the usual places in order
	if (layout_path.empty())
		layout_path = ui::layout::find_default();

	if (!dump_layout.empty()) {
		ui::layout l;
		std::string lerr;
		if (!layout_path.empty())
			l.load(layout_path, lerr);
		if (!l.save(dump_layout)) {
			std::fprintf(stderr, "Cannot write %s\n", dump_layout.c_str());
			return 1;
		}
		std::printf("Wrote the current layout: %s\n", dump_layout.c_str());
		return 0;
	}

	static ui::bridge br;
	static ui::midi_in midi_ports[mu2000::MIDI_PORTS];

	if (run_selftest)
		return selftest(dir, win_w, win_h);

	// Picture only. An empty screen can be drawn even without any ROMs.
	if (!shot_path.empty() && (dir.empty() || !boot_for_shot)) {
		ui::snapshot s;
		std::snprintf(s.message, sizeof(s.message), "S-MU2000");
		br.publish(s);
		return shot(shot_path, win_w, win_h, br, grid, layout_path);
	}

	if (dir.empty()) {
		std::fprintf(stderr,
			"Usage: gui <rom directory> [--midi N] [--midi-b N] [--midi-c N] [--midi-d N]\n"
			"       [--midiout N] [--midiout-b N] [--midiout-mu N] [--nomidi]\n"
			"       [--latency ms] [--audio part-of-name] [--audio-in name]\n"
			"       [--exclusive] [--factory] [--fast-midi] [--usb | --host-midi]\n"
			"       [--play song.mid] [--seconds N] [--size WxH] [--layout panel.txt]\n"
			"       gui --shot image.png [--size 1400x360] [--grid] [--layout panel.txt]\n"
			"       gui <rom directory> --boot --shot image.png [--mid song.mid seconds]\n"
			"       gui --list | gui --dump-layout panel.txt | gui --selftest [rom dir]\n");
		return 1;
	}

	// ---- Windowed run

	static ui::engine eng(br, midi_ports[0]);
	eng.mu.set_fast_midi(fast_midi);
	for (int p = 1; p < mu2000::MIDI_PORTS; p++)
		eng.midi_p[p] = &midi_ports[p];
	static ui::midi_out mout, mout_b, mout_mu;
	static app gui{ br, midi_ports };
	gui.mout = &mout;
	gui.mout_b = &mout_b;
	gui.mout_mu = &mout_mu;
	eng.mout = &mout;
	eng.mout_b = &mout_b;
	eng.mout_mu = &mout_mu;
	gui.eng = &eng;
	gui.keep_settings = nomidi;
	load_settings(gui);   // remembered names, volume, fold, analog
	if (!audio_dev.empty())
		gui.audio_name = audio_dev;   // argv wins over remembered
	if (!ain_dev.empty())
		gui.ain_name = ain_dev;
	// Open the ports. Explicit argv wins; otherwise the remembered name
	// (kept when the device is not running yet); port A also takes the first
	// input when nothing was ever chosen. --nomidi opens nothing at all.
	{
		const auto ins = ui::midi_in::list();
		const auto outs = ui::midi_out::list();
		for (int p = 0; p < mu2000::MIDI_PORTS; p++) {
			if (nomidi)
				choose_in(gui, p, -1, false);
			else if (in_dev[p] != -2)
				choose_in(gui, p, in_dev[p], false);
			else if (!gui.in_name[p].empty())
				choose_in(gui, p, find_device(ins, gui.in_name[p]), true);
			else if (p == 0 && !ins.empty())
				choose_in(gui, p, 0, false);
		}
		if (!nomidi) {
			if (out_dev != -2)
				choose_out(gui, out_dev, false);
			else if (!gui.out_name.empty())
				choose_out(gui, find_device(outs, gui.out_name), true);
			if (out_dev_b != -2)
				choose_out_b(gui, out_dev_b, false);
			else if (!gui.out_name_b.empty())
				choose_out_b(gui, find_device(outs, gui.out_name_b), true);
			if (out_dev_mu != -2)
				choose_out_mu(gui, out_dev_mu, false);
			else if (!gui.out_name_mu.empty())
				choose_out_mu(gui, find_device(outs, gui.out_name_mu), true);
		}
	}
	eng.use_nvram = !factory;
	if (factory)
		std::printf("Factory reset boot (settings will be overwritten on exit)\n");
	eng.mu.set_usb_host(usb_host);
	std::printf(usb_host ? "MIDI on USB ports (A-D, 64 parts)\n"
	                     : "--host-midi: DIN ports A and B only (parts 1-32)\n");
	if (!eng.load(dir)) {
		std::fprintf(stderr, "%s\n", eng.message.c_str());
		return 1;
	}
	// the panel reads voice names from the user's ROM (xg/voices.h)
	ui::xgui::set_voice_rom(eng.mu.program_rom());

	// The remembered card goes back in before boot, like a card left in a
	// real machine. A missing file stays remembered (quietly).
	if (shot_path.empty() && !gui.card_path.empty())
		insert_card(gui, gui.card_path, true);

	if (!shot_path.empty()) {
		// Picture only, but taken after boot so the LCD has something on it
		if (!eng.boot()) { std::fprintf(stderr, "%s\n", eng.message.c_str()); return 1; }
		eng.state.store(1);
		{
			s32 l, r;
			for (size_t i = 0; i < size_t(2.0 * RATE); i++)
				eng.mu.run_sample(l, r);
		}
		if (!shot_mid.empty()) {
			std::vector<smf::event> evs;
			std::string err;
			if (!smf::load(shot_mid, evs, err)) {
				std::fprintf(stderr, "%s\n", err.c_str());
			} else {
				std::printf("Streaming %zu MIDI events (%.1f s)\n", evs.size(), shot_secs);
				size_t at = 0;
				s32 l, r;
				for (size_t i = 0; i < size_t(shot_secs * RATE); i++) {
					const double now = double(i) / RATE;
					while (at < evs.size() && evs[at].time <= now) {
						for (u8 b : evs[at].bytes)
							eng.mu.midi_in(b);
						at++;
					}
					eng.mu.run_sample(l, r);
				}
			}
		}
		eng.publish();
		return shot(shot_path, win_w, win_h, br, grid, layout_path);
	}

	gui.set_layout(layout_path);
	gui.panel.resize(win_w, win_h);
	gui.open_pc = open_pc;
	gui.open_list = open_list;
	gui.open_fx = open_fx;
	return run_window(gui, eng, win_w, win_h, latency, exclusive, play_path, gui.ain_name,
	                  seconds);
}
