// license:BSD-3-Clause
//
// The Linux window system: the SDL3 window, its event pump, the surface the
// panel paints into, and --selftest. The Windows and macOS twins are
// ui/window_win.* and ui/window_mac.*; the declarations live in
// ui/window_sdl.h and nothing here is seen by gui_linux.cpp, which keeps
// only main().
//
// SDL has no menus, so the popups are drawn into the window itself
// (ui/sdl_popup) over the live panel; the shared menu content comes from
// ui::app::context_menu, dispatched by ui::app::menu_chosen.

#include "window_sdl.h"

#include "app_linux.h"
#include "compat/gdi.h"
#include "mu2000.h"
#include "ui/engine.h"
#include "ui/panel.h"
#include "ui/pc_host.h"
#include "ui/png.h"
#include "ui/sdl_popup.h"
#include "ui/toolbar.h"
#include "ui/xg_ui.h"

#include <SDL3/SDL.h>

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

namespace {

constexpr u32 RATE = ui::AUDIO_RATE;

// SDL keycodes translated into the shared key space (ui/keymap.h): the four
// F-keys the app acts on, the panel characters, 0 for everything else
int sdl_key_to_shared(int k)
{
	switch (k) {
	case SDLK_F2: return ui::KEY_F2;
	case SDLK_F3: return ui::KEY_F3;
	case SDLK_F4: return ui::KEY_F4;
	case SDLK_F5: return ui::KEY_F5;
	default: break;
	}
	return k < 128 ? k : 0;
}

// ---- A Cairo image surface behind a GDI DC --------------------------------
//
// Both the window and --selftest paint through this: same surface, same
// paint call, so the bytes agree by construction (the DIB-vs-window check
// in doc/porting-linux-gui.md).

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

// The context menu at a point: the shared groups (ui::app::context_menu),
// rendered through sdl_popup by the app, then the shared dispatch. The id
// reaching menu_chosen is exactly what WM_COMMAND receives on Windows
void open_menu(ui::linux_app &gui, int x, int y)
{
	const std::vector<ui::menu_group> groups = gui.context_menu(x, y);
	if (groups.empty())
		return;
	const int id = gui.show_popup(groups, x, y);
	if (id >= 0)
		gui.menu_chosen(id);
}

} // namespace

namespace ui {

// ui::linux_app::run_list: the modal popup over the live panel. sdl_popup
// draws into this window's framebuffer, and its behind() repaints the panel
// between menu frames (the old hand-built run_list did the same)
int linux_app::run_list(const std::vector<sdl_popup::item> &items, int x, int y,
                        int &sub_chosen)
{
	auto behind = [&] { frame(); };
	return sdl_popup::run(win, ren, tex, panel_bits, ww, wh, behind, quit,
	                      items, x, y, sub_chosen);
}

// ---- the SDL3 window pump (ui::app::run calls it through pump_window) ------
//
// One window, one renderer, one streaming texture the panel paints into. The
// PC editor windows keep their own SDL windows and eat their events first
// (ui/pc_window_linux.cpp).

int run_window(linux_app &gui, const char *title, int w, int h)
{
	SDL_Window *win = SDL_CreateWindow(title, w, h, SDL_WINDOW_RESIZABLE);
	if (!win) {
		std::fprintf(stderr, "窓を出せない: %s\n", SDL_GetError());
		SDL_Quit();
		return 1;
	}
	SDL_Renderer *ren = SDL_CreateRenderer(win, nullptr);
	if (!ren) {
		std::fprintf(stderr, "描画器を作れない: %s\n", SDL_GetError());
		SDL_DestroyWindow(win);
		SDL_Quit();
		return 1;
	}
	SDL_Texture *tex = SDL_CreateTexture(ren, SDL_PIXELFORMAT_ARGB8888,
	                                     SDL_TEXTUREACCESS_STREAMING, w, h);
	if (!tex) {
		std::fprintf(stderr, "絵の置き場を作れない: %s\n", SDL_GetError());
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
	if (!fb.reset(w, h)) {
		std::fprintf(stderr, "画面を作れない\n");
		SDL_DestroyTexture(tex);
		SDL_DestroyRenderer(ren);
		SDL_DestroyWindow(win);
		SDL_Quit();
		return 1;
	}
	gui.win = win;
	gui.ren = ren;
	gui.tex = tex;
	gui.panel_dc = fb.dc;
	gui.panel_bits = fb.bits;
	gui.ww = w;
	gui.wh = h;

	bool down = false;   // left button held: drags go to the panel
	const Uint64 quit_at = gui.seconds_limit > 0.0
	                           ? SDL_GetTicks() + Uint64(gui.seconds_limit * 1000.0)
	                           : 0;
	while (!gui.quit.load()) {
		if (quit_at && SDL_GetTicks() >= quit_at)
			break;   // --seconds: timed run, for smoke tests and demos
		const Uint64 frame_at = SDL_GetTicks() + 33;
		SDL_Event ev;
		while (SDL_PollEvent(&ev)) {
			// PC editor windows first: they own their SDL windows and eat
			// their events (including drops and the close button).
			if (pc_window::route_event(ev))
				continue;
			switch (ev.type) {
			case SDL_EVENT_QUIT:
				gui.quit.store(true);
				break;
			case SDL_EVENT_WINDOW_RESIZED:
				gui.ww = ev.window.data1;
				gui.wh = ev.window.data2;
				if (fb.reset(gui.ww, gui.wh)) {
					SDL_DestroyTexture(tex);
					tex = SDL_CreateTexture(ren, SDL_PIXELFORMAT_ARGB8888,
					                        SDL_TEXTUREACCESS_STREAMING,
					                        gui.ww, gui.wh);
					if (tex)
						SDL_SetTextureBlendMode(tex, SDL_BLENDMODE_NONE);
					gui.tex = tex;
					gui.panel_dc = fb.dc;
					gui.panel_bits = fb.bits;
					gui.resized(gui.ww, gui.wh);
				}
				break;
			case SDL_EVENT_WINDOW_FOCUS_LOST:
				gui.focus_lost();   // leaving the window releases everything
				break;
			case SDL_EVENT_MOUSE_BUTTON_DOWN: {
				const int mx = int(ev.button.x), my = int(ev.button.y);
				const bool right = ev.button.button == SDL_BUTTON_RIGHT;
				const ui::mouse_out o = gui.mouse_down(mx, my, right);
				if (o.show_menu)
					open_menu(gui, mx, my);
				down = o.panel_pressed;
				break;
			}
			case SDL_EVENT_MOUSE_BUTTON_UP:
				gui.mouse_up();
				down = false;
				break;
			case SDL_EVENT_MOUSE_MOTION:
				if (down)
					gui.mouse_drag(int(ev.motion.x), int(ev.motion.y));
				else if (cur_hand && cur_arrow)
					SDL_SetCursor(gui.hand_cursor(int(ev.motion.x), int(ev.motion.y))
					                  ? cur_hand
					                  : cur_arrow);
				break;
			case SDL_EVENT_MOUSE_WHEEL: {
				float fx, fy;
				SDL_GetMouseState(&fx, &fy);
				const int steps = int(ev.wheel.y > 0 ? 1 : ev.wheel.y < 0 ? -1 : 0);
				gui.wheel(int(fx), int(fy), steps);
				break;
			}
			case SDL_EVENT_KEY_DOWN: {
				if (ev.key.repeat)
					break;   // held-key repeats are ignored
				gui.key(sdl_key_to_shared(ev.key.key), true);
				break;
			}
			case SDL_EVENT_KEY_UP:
				gui.key(sdl_key_to_shared(ev.key.key), false);
				break;
			case SDL_EVENT_DROP_FILE:
				if (ev.drop.data) {
					play_dropped_file(ev.drop.data);
					SDL_free(const_cast<char *>(ev.drop.data));
				}
				break;
			default:
				break;
			}
		}

		gui.frame();
		if (tex) {
			SDL_UpdateTexture(tex, nullptr, fb.bits, gui.ww * 4);
			SDL_RenderTexture(ren, tex, nullptr, nullptr);
			SDL_RenderPresent(ren);
		}
		const Uint64 now = SDL_GetTicks();
		if (frame_at > now)
			SDL_Delay(Uint32(frame_at - now));
	}

	// Tell the editor windows we are closing, then tear their SDL
	// resources down here: SDL_Quit below would strand them. (The shared
	// shutdown runs afterwards; on closed windows its calls do nothing.)
	pc_shutdown_all(gui.list, gui.pc, gui.fx, gui.shapes, gui.master, gui.br);
	gui.list.close();
	gui.pc.close();
	gui.fx.close();
	gui.shapes.close();
	gui.master.close();
	g_linux = nullptr;

	SDL_DestroyTexture(tex);
	if (cur_hand) SDL_DestroyCursor(cur_hand);
	if (cur_arrow) SDL_DestroyCursor(cur_arrow);
	SDL_DestroyRenderer(ren);
	SDL_DestroyWindow(win);
	SDL_Quit();
	return 0;
}

// Paint twice into separate surfaces and compare. The automatable core of the
// DIB-vs-window check: the window uploads these same bytes, so deterministic
// painting here plus a memcpy upload means the screen agrees by construction.
// (Reading pixels back off the GPU is the one part that needs a real display.)
int selftest(const std::string &rom_dir, int w, int h)
{
	static ui::bridge br;
	static ui::midi_in midi_ports[mu2000::MIDI_PORTS];
	static ui::engine eng(br, midi_ports[0]);

	if (!rom_dir.empty()) {
		if (!eng.load(rom_dir)) {
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
		std::fprintf(stderr, "画面を作れない\n");
		return 1;
	}
	ui::snapshot s;
	br.read(s);
	p.paint(a.dc, s, 0, "");
	p.paint(b.dc, s, 0, "");
	GdiFlush();
	const size_t bytes = size_t(w) * size_t(h) * 4;
	if (std::memcmp(a.bits, b.bits, bytes) != 0) {
		std::printf("自己診断: だめ（%dx%d で違う）\n", w, h);
		return 1;
	}
	std::printf("自己診断: 同じ（%dx%d、%zu バイト）\n", w, h, bytes);

	// The rest of the DIB-vs-window check: upload and read the pixels back.
	// A hidden window keeps this polite on a real display and working under
	// SDL_VIDEODRIVER=dummy. SKIP (not fail) when no video is available.
	if (!SDL_Init(SDL_INIT_VIDEO)) {
		std::printf("自己診断: 読み戻しは飛ばす（%s）\n", SDL_GetError());
		return 0;
	}
	int rc = 0;
	SDL_Window *win = SDL_CreateWindow("selftest", w, h, SDL_WINDOW_HIDDEN);
	SDL_Renderer *ren = win ? SDL_CreateRenderer(win, nullptr) : nullptr;
	SDL_Texture *tex = ren ? SDL_CreateTexture(ren, SDL_PIXELFORMAT_ARGB8888,
	                                           SDL_TEXTUREACCESS_STREAMING, w, h)
	                       : nullptr;
	if (!tex) {
		std::printf("自己診断: 読み戻しは飛ばす（%s）\n", SDL_GetError());
	} else {
		SDL_SetTextureBlendMode(tex, SDL_BLENDMODE_NONE);
		SDL_UpdateTexture(tex, nullptr, a.bits, w * 4);
		SDL_RenderTexture(ren, tex, nullptr, nullptr);
		SDL_RenderPresent(ren);
		SDL_Surface *got = SDL_RenderReadPixels(ren, nullptr);
		if (!got) {
			std::printf("自己診断: 読み戻しは飛ばす（%s）\n", SDL_GetError());
		} else if (got->w != w || got->h != h || got->pitch != w * 4) {
			std::printf("自己診断: だめ（読み戻しは %dx%d/%d、%dx%d/%d がほしい）\n",
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
			if (!cvt) {
				std::printf("自己診断: 読み戻しは飛ばす（%s）\n", SDL_GetError());
				SDL_DestroySurface(got);
				SDL_DestroyTexture(tex);
				SDL_DestroyRenderer(ren);
				SDL_DestroyWindow(win);
				SDL_Quit();
				return 0;
			}
			const u32 *pa = static_cast<const u32 *>(a.bits);
			const u32 *pb = reinterpret_cast<const u32 *>(conv.data());
			size_t rgb_diff = 0, alpha_diff = 0;
			for (int i = 0; i < w * h; i++) {
				if ((pa[i] & 0x00ffffffu) != (pb[i] & 0x00ffffffu))
					rgb_diff++;
				else if (pa[i] != pb[i])
					alpha_diff++;
			}
			SDL_DestroySurface(got);
			if (rgb_diff) {
				std::printf("自己診断: だめ（%zu ピクセル違う）\n", rgb_diff);
				ui::write_png("/tmp/selftest_want.png",
				              static_cast<const u8 *>(a.bits), w, h, w * 4);
				ui::write_png("/tmp/selftest_got.png", conv.data(), w, h, w * 4);
				std::printf("/tmp/selftest_want.png と /tmp/selftest_got.png に書き出した\n");
				rc = 1;
			} else {
				std::printf("自己診断: 読み戻しも同じ（%zu ピクセル、αだけ違う %zu）\n",
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

} // namespace ui
