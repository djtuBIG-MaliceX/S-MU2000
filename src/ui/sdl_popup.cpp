// license:BSD-3-Clause

#include "sdl_popup.h"

#include <cairo/cairo.h>

#include <algorithm>
#include <cstring>
#include <memory>
#include <mutex>

namespace ui {
namespace sdl_popup {

namespace {

constexpr int ROW_H = 26, PAD_X = 12, GUTTER = 22;

void paint(cairo_t *cr, const std::vector<item> &items, int px, int py, int w, int hover)
{
	cairo_select_font_face(cr, "sans-serif", CAIRO_FONT_SLANT_NORMAL,
	                       CAIRO_FONT_WEIGHT_NORMAL);
	cairo_set_font_size(cr, 14.0);
	const int h = int(items.size()) * ROW_H + 12;
	// Dim the panel behind the menu.
	cairo_set_source_rgba(cr, 0, 0, 0, 0.35);
	cairo_paint(cr);
	// Box.
	cairo_set_source_rgb(cr, 0.16, 0.16, 0.17);
	cairo_rectangle(cr, px, py, w, h);
	cairo_fill(cr);
	cairo_set_source_rgb(cr, 0.55, 0.55, 0.58);
	cairo_set_line_width(cr, 1);
	cairo_rectangle(cr, px + 0.5, py + 0.5, w - 1, h - 1);
	cairo_stroke(cr);

	for (size_t i = 0; i < items.size(); i++) {
		const int ry = py + 6 + int(i) * ROW_H;
		const item &it = items[i];
		if (it.separator) {
			cairo_set_source_rgb(cr, 0.4, 0.4, 0.42);
			cairo_move_to(cr, px + 8, ry + ROW_H / 2);
			cairo_line_to(cr, px + w - 8, ry + ROW_H / 2);
			cairo_stroke(cr);
			continue;
		}
		if (int(i) == hover && it.enabled) {
			cairo_set_source_rgb(cr, 0.25, 0.45, 0.75);
			cairo_rectangle(cr, px + 3, ry, w - 6, ROW_H);
			cairo_fill(cr);
		}
		cairo_set_source_rgb(cr, it.enabled ? 0.92 : 0.5, it.enabled ? 0.92 : 0.5,
		                     it.enabled ? 0.92 : 0.5);
		cairo_move_to(cr, px + PAD_X + GUTTER, ry + 17);
		std::string text = it.label;
		if (it.submenu)
			text += "  >";
		cairo_show_text(cr, text.c_str());
		if (it.checked) {
			// A filled square: font glyphs for checks are hit and miss
			// across sans-serif faces (Noto Sans has neither ✓ nor ●).
			cairo_rectangle(cr, px + PAD_X + 2, ry + ROW_H / 2 - 4, 8, 8);
			cairo_fill(cr);
		}
	}
}

int width_for(cairo_t *cr, const std::vector<item> &items)
{
	cairo_select_font_face(cr, "sans-serif", CAIRO_FONT_SLANT_NORMAL,
	                       CAIRO_FONT_WEIGHT_NORMAL);
	cairo_set_font_size(cr, 14.0);
	double w = 0;
	for (const item &it : items) {
		if (it.separator)
			continue;
		cairo_text_extents_t ex{};
		const std::string t = it.submenu ? it.label + "  >" : it.label;
		cairo_text_extents(cr, t.c_str(), &ex);
		w = std::max(w, ex.x_advance);
	}
	return int(w) + PAD_X * 2 + GUTTER + 16;
}

} // namespace

int run(SDL_Window *win, SDL_Renderer *ren, SDL_Texture *tex, void *bits,
        int ww, int wh, std::function<void()> behind, std::atomic<bool> &quit,
        const std::vector<item> &items, int x, int y, int &sub_chosen)
{
	(void)win;
	sub_chosen = -1;
	cairo_surface_t *ms = cairo_image_surface_create(CAIRO_FORMAT_A8, 8, 8);
	cairo_t *mc = cairo_create(ms);
	const int w = width_for(mc, items);
	const int h = int(items.size()) * ROW_H + 12;
	cairo_destroy(mc);
	cairo_surface_destroy(ms);
	x = std::clamp(x, 0, std::max(0, ww - w));
	y = std::clamp(y, 0, std::max(0, wh - h));

	int hover = -1;
	auto repaint = [&] {
		behind();
		cairo_surface_t *surf = cairo_image_surface_create_for_data(
		    static_cast<unsigned char *>(bits), CAIRO_FORMAT_ARGB32, ww, wh, ww * 4);
		cairo_t *cr = cairo_create(surf);
		paint(cr, items, x, y, w, hover);
		cairo_destroy(cr);
		cairo_surface_destroy(surf);
		if (ren && tex) {
			SDL_UpdateTexture(tex, nullptr, bits, ww * 4);
			SDL_RenderTexture(ren, tex, nullptr, nullptr);
			SDL_RenderPresent(ren);
		}
	};
	auto at = [&](int mx, int my) {
		if (mx < x || mx >= x + w || my < y)
			return -1;
		const int i = (my - y - 6) / ROW_H;
		if (i < 0 || i >= int(items.size()))
			return -1;
		return i;
	};

	repaint();
	while (!quit.load()) {
		SDL_Event ev;
		if (!SDL_WaitEventTimeout(&ev, 30))
			continue;
		// Only this window's events. The plug-in shares the queue with its
		// PC windows and the host may share the process.
		if (ev.type != SDL_EVENT_QUIT) {
			Uint32 id = 0;
			switch (ev.type) {
			case SDL_EVENT_MOUSE_MOTION:      id = ev.motion.windowID; break;
			case SDL_EVENT_MOUSE_BUTTON_DOWN:
			case SDL_EVENT_MOUSE_BUTTON_UP:   id = ev.button.windowID; break;
			case SDL_EVENT_MOUSE_WHEEL:       id = ev.wheel.windowID; break;
			case SDL_EVENT_KEY_DOWN:
			case SDL_EVENT_KEY_UP:            id = ev.key.windowID; break;
			case SDL_EVENT_WINDOW_CLOSE_REQUESTED:
			case SDL_EVENT_WINDOW_RESIZED:
			case SDL_EVENT_WINDOW_FOCUS_LOST: id = ev.window.windowID; break;
			default: break;
			}
			Uint32 me = win ? SDL_GetWindowID(win) : 0;
			if (id && me && id != me)
				continue;
		}
		switch (ev.type) {
		case SDL_EVENT_QUIT:
			quit.store(true);
			return -1;
		case SDL_EVENT_MOUSE_MOTION: {
			const int i = at(int(ev.motion.x), int(ev.motion.y));
			const int h = (i >= 0 && !items[size_t(i)].separator) ? i : -1;
			if (h != hover) {
				hover = h;
				repaint();
			}
			break;
		}
		case SDL_EVENT_MOUSE_BUTTON_DOWN:
			if (ev.button.button == SDL_BUTTON_RIGHT)
				return -1;
			if (ev.button.button == SDL_BUTTON_LEFT) {
				const int i = at(int(ev.button.x), int(ev.button.y));
				if (i < 0)
					return -1;   // clicked outside: cancel
				const item &it = items[size_t(i)];
				if (!it.enabled || it.separator)
					break;
				if (it.submenu) {
					sub_chosen = it.sub;
					return it.id;
				}
				return it.id;
			}
			break;
		case SDL_EVENT_KEY_DOWN:
			if (ev.key.key == SDLK_ESCAPE)
				return -1;
			if (ev.key.key == SDLK_UP || ev.key.key == SDLK_DOWN) {
				const int d = ev.key.key == SDLK_DOWN ? 1 : -1;
				int i = hover;
				for (size_t k = 0; k < items.size(); k++) {
					i = (i + d + int(items.size())) % int(items.size());
					if (!items[size_t(i)].separator && items[size_t(i)].enabled)
						break;
				}
				hover = i;
				repaint();
			} else if (ev.key.key == SDLK_RETURN || ev.key.key == SDLK_KP_ENTER) {
				if (hover >= 0 && items[size_t(hover)].enabled &&
				    !items[size_t(hover)].separator) {
					if (items[size_t(hover)].submenu) {
						sub_chosen = items[size_t(hover)].sub;
						return items[size_t(hover)].id;
					}
					return items[size_t(hover)].id;
				}
			}
			break;
		default:
			break;
		}
	}
	return -1;
}

struct dialog_state {
	std::atomic<bool> done{ false };
	std::mutex        mutex;
	std::string       path;
};

static void SDLCALL dialog_done(void *ud, const char *const *list, int)
{
	auto *st = static_cast<std::shared_ptr<dialog_state> *>(ud);
	std::lock_guard<std::mutex> hold((*st)->mutex);
	if (list && *list)
		(*st)->path = *list;
	(*st)->done.store(true);
	delete st;   // one-shot: the wait below always consumes exactly once
}

static std::string file_dialog(SDL_Window *win, std::atomic<bool> &quit, bool save,
                               const file_filter *filters, int nfilters,
                               const std::string &defloc)
{
	auto *held = new std::shared_ptr<dialog_state>(std::make_shared<dialog_state>());
	std::shared_ptr<dialog_state> st = *held;
	std::vector<SDL_DialogFileFilter> f;
	for (int i = 0; i < nfilters; i++)
		f.push_back({ filters[i].name, filters[i].pattern });
	if (save)
		SDL_ShowSaveFileDialog(dialog_done, held, win, f.data(), int(f.size()),
		                       defloc.empty() ? nullptr : defloc.c_str());
	else
		SDL_ShowOpenFileDialog(dialog_done, held, win, f.data(), int(f.size()),
		                       defloc.empty() ? nullptr : defloc.c_str(), false);
	while (!st->done.load() && !quit.load()) {
		SDL_Event ev;
		if (!SDL_WaitEventTimeout(&ev, 50))
			continue;
		if (ev.type == SDL_EVENT_QUIT) {
			quit.store(true);
			break;
		}
		if (ev.type == SDL_EVENT_KEY_DOWN && ev.key.key == SDLK_ESCAPE)
			break;   // stop waiting; a late answer lands in shared state
	}
	std::lock_guard<std::mutex> hold(st->mutex);
	return st->path;
}

std::string open_file(SDL_Window *win, std::atomic<bool> &quit,
                      const file_filter *filters, int nfilters,
                      const std::string &defloc)
{
	return file_dialog(win, quit, false, filters, nfilters, defloc);
}

std::string save_file(SDL_Window *win, std::atomic<bool> &quit,
                      const file_filter *filters, int nfilters,
                      const std::string &defloc)
{
	return file_dialog(win, quit, true, filters, nfilters, defloc);
}

int message_box(SDL_Window *win, const char *title, const char *text,
                unsigned flags,
                std::initializer_list<SDL_MessageBoxButtonData> buttons)
{
	std::vector<SDL_MessageBoxButtonData> b(buttons);
	SDL_MessageBoxData d{};
	d.window = win;
	d.flags = flags;
	d.title = title;
	d.message = text;
	d.numbuttons = int(b.size());
	d.buttons = b.data();
	int id = -1;
	SDL_ShowMessageBox(&d, &id);
	return id;
}

void alert(SDL_Window *win, const char *title, const std::string &text)
{
	message_box(win, title, text.c_str(), SDL_MESSAGEBOX_WARNING,
	            { { 0, 0, "OK" } });
}

bool confirm(SDL_Window *win, const char *title, const std::string &text,
             const char *ok_label)
{
	const SDL_MessageBoxButtonData buttons[] = {
		{ SDL_MESSAGEBOX_BUTTON_ESCAPEKEY_DEFAULT |
		  SDL_MESSAGEBOX_BUTTON_RETURNKEY_DEFAULT, 0, "Cancel" },
		{ 0, 1, ok_label },
	};
	return message_box(win, title, text.c_str(), SDL_MESSAGEBOX_WARNING,
	                   { buttons[0], buttons[1] }) == 1;
}

} // namespace sdl_popup
} // namespace ui
