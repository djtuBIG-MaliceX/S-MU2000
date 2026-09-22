// license:BSD-3-Clause
//
// Linux pc_window: one SDL3 window + renderer per ImGui view.
//
// Event routing needs a window-ID lookup because SDL events are global:
// every live window registers itself, and route_event() feeds the owner.
// Closing (the X button) only hides, mirroring the other platforms.

#include "pc_window_linux.h"

#include "imgui.h"
#include "backends/imgui_impl_sdl3.h"
#include "backends/imgui_impl_sdlrenderer3.h"

#include <fontconfig/fontconfig.h>

#include <mutex>
#include <vector>

namespace ui {

namespace {

using drop_fn = void (*)(const std::string &path);

drop_fn &drop_handler()
{
	static drop_fn fn = nullptr;
	return fn;
}

std::mutex &registry_mutex()
{
	static std::mutex m;
	return m;
}

std::vector<pc_window *> &registry()
{
	static std::vector<pc_window *> v;
	return v;
}

// The title comes from imgui_view as UTF-32 (wchar_t is 32 bits here, unlike
// Windows). SDL wants UTF-8, so convert plainly (same helper as macOS).
// The five known Japanese titles render in English on Linux; anything else
// converts as-is, so a future title still shows something.
std::string title_of(const imgui_view &view)
{
	const std::wstring t = view.title() ? view.title() : L"";
	if (t == L"S-MU2000 エディタ") return "S-MU2000 Editor";
	if (t == L"S-MU2000 一覧") return "S-MU2000 List";
	if (t == L"S-MU2000 インサーションエフェクト") return "S-MU2000 Insertion Effect";
	if (t == L"S-MU2000 パートの音色") return "S-MU2000 Part Voice";
	if (t == L"S-MU2000 マスター") return "S-MU2000 Master";
	std::string utf8;
	for (const wchar_t *w = view.title(); w && *w; w++) {
		const unsigned int c = unsigned(*w);
		if (c < 0x80) {
			utf8 += char(c);
		} else if (c < 0x800) {
			utf8 += char(0xc0 | (c >> 6));
			utf8 += char(0x80 | (c & 0x3f));
		} else if (c < 0x10000) {
			utf8 += char(0xe0 | (c >> 12));
			utf8 += char(0x80 | ((c >> 6) & 0x3f));
			utf8 += char(0x80 | (c & 0x3f));
		} else {
			utf8 += char(0xf0 | (c >> 18));
			utf8 += char(0x80 | ((c >> 12) & 0x3f));
			utf8 += char(0x80 | ((c >> 6) & 0x3f));
			utf8 += char(0x80 | (c & 0x3f));
		}
	}
	return utf8;
}

// A CJK-capable font for the views, found by family (same idea as the other
// platforms: Windows loads Yu Gothic, macOS asks CoreText). Noto Sans CJK
// covers the Latin labels too, so one font serves both languages. Empty when
// nothing matched, and then ImGui's embedded font stands in (English only).
std::string cjk_font_file()
{
	FcPattern *pat = FcPatternCreate();
	if (!pat)
		return {};
	FcPatternAddString(pat, FC_FAMILY, reinterpret_cast<const FcChar8 *>("Noto Sans CJK JP"));
	FcPatternAddDouble(pat, FC_SIZE, 16.0);
	FcConfigSubstitute(nullptr, pat, FcMatchPattern);
	FcDefaultSubstitute(pat);
	FcResult res = FcResultNoMatch;
	FcPattern *m = FcFontMatch(nullptr, pat, &res);
	std::string file;
	if (m) {
		FcChar8 *f = nullptr;
		if (FcPatternGetString(m, FC_FILE, 0, &f) == FcResultMatch && f)
			file = reinterpret_cast<const char *>(f);
		FcPatternDestroy(m);
	}
	FcPatternDestroy(pat);
	return file;
}

} // namespace

void pc_window::set_drop_handler(drop_fn fn)
{
	drop_handler() = fn;
}

pc_window::~pc_window()
{
	std::lock_guard<std::mutex> hold(registry_mutex());
	auto &v = registry();
	v.erase(std::remove(v.begin(), v.end(), this), v.end());
	destroy();
}

bool pc_window::show(std::string &err)
{
	if ((!m_win || !m_ren || !m_imgui) && !create(err))
		return false;
	SDL_ShowWindow(m_win);
	SDL_RaiseWindow(m_win);
	return true;
}

void pc_window::hide()
{
	if (m_win)
		SDL_HideWindow(m_win);
}

void pc_window::close()
{
	hide();
	destroy();
}

bool pc_window::visible() const
{
	if (!m_win)
		return false;
	const SDL_WindowFlags flags = SDL_GetWindowFlags(m_win);
	return !(flags & (SDL_WINDOW_HIDDEN | SDL_WINDOW_MINIMIZED));
}

void pc_window::shutdown(bridge &br)
{
	if (!m_imgui)
		return;
	ImGui::SetCurrentContext(m_imgui);
	m_view->hidden(br);
}

bool pc_window::create(std::string &err)
{
	destroy();
	m_win = SDL_CreateWindow(title_of(*m_view).c_str(), m_view->default_width(),
	                         m_view->default_height(), SDL_WINDOW_RESIZABLE);
	if (!m_win) {
		err = SDL_GetError();
		return false;
	}
	m_ren = SDL_CreateRenderer(m_win, nullptr);
	if (!m_ren) {
		err = SDL_GetError();
		destroy();
		return false;
	}
	m_id = SDL_GetWindowID(m_win);

	m_imgui = ImGui::CreateContext();
	ImGui::SetCurrentContext(m_imgui);
	ImGuiIO &io = ImGui::GetIO();
	io.ConfigFlags |= ImGuiConfigFlags_NavEnableKeyboard;

	const std::string font = cjk_font_file();
	if (!font.empty())
		io.Fonts->AddFontFromFileTTF(font.c_str(), 16.0f);

	if (!ImGui_ImplSDL3_InitForSDLRenderer(m_win, m_ren)) {
		err = "ImGui SDL3 backend failed to start";
		destroy();
		return false;
	}
	if (!ImGui_ImplSDLRenderer3_Init(m_ren)) {
		err = "ImGui renderer backend failed to start";
		destroy();
		return false;
	}

	std::lock_guard<std::mutex> hold(registry_mutex());
	registry().push_back(this);
	return true;
}

void pc_window::destroy()
{
	if (m_imgui) {
		ImGui::SetCurrentContext(m_imgui);
		ImGui_ImplSDLRenderer3_Shutdown();
		ImGui_ImplSDL3_Shutdown();
		ImGui::DestroyContext(m_imgui);
		m_imgui = nullptr;
	}
	if (m_ren) {
		SDL_DestroyRenderer(m_ren);
		m_ren = nullptr;
	}
	if (m_win) {
		SDL_DestroyWindow(m_win);
		m_win = nullptr;
		m_id = 0;
	}
}

void pc_window::frame(xg::model &m, const xg_snapshot &ram, bridge &br)
{
	const bool shown = visible();
	if (m_was_visible && !shown && m_imgui) {
		ImGui::SetCurrentContext(m_imgui);
		m_view->hidden(br);                  // closed or minimized: release what it held
	}
	m_was_visible = shown;
	if (!shown)
		return;
	ImGui::SetCurrentContext(m_imgui);

	ImGui_ImplSDLRenderer3_NewFrame();
	ImGui_ImplSDL3_NewFrame();
	ImGui::NewFrame();
	m_view->draw(m, ram, br);
	xgui::drag_flush(br);          // マウスで動かしている値の、間引いた送信
	ImGui::Render();

	SDL_SetRenderDrawColor(m_ren, 26, 26, 28, 255);
	SDL_RenderClear(m_ren);
	ImGui_ImplSDLRenderer3_RenderDrawData(ImGui::GetDrawData(), m_ren);
	SDL_RenderPresent(m_ren);
	// no wait: gui's timer (30 frames a second) decides the pace
}

bool pc_window::route_event(const SDL_Event &ev)
{
	// A file dropped on an editor window plays, the same as on the panel.
	if (ev.type == SDL_EVENT_DROP_FILE && ev.drop.data) {
		const Uint32 id = ev.drop.windowID;
		std::lock_guard<std::mutex> hold(registry_mutex());
		for (pc_window *w : registry()) {
			if (w && w->m_id == id) {
				if (drop_handler())
					drop_handler()(ev.drop.data);
				SDL_free(const_cast<char *>(ev.drop.data));
				return true;
			}
		}
		return false;
	}
	Uint32 id = 0;
	switch (ev.type) {
	case SDL_EVENT_MOUSE_MOTION:      id = ev.motion.windowID; break;
	case SDL_EVENT_MOUSE_BUTTON_DOWN:
	case SDL_EVENT_MOUSE_BUTTON_UP:   id = ev.button.windowID; break;
	case SDL_EVENT_MOUSE_WHEEL:       id = ev.wheel.windowID; break;
	case SDL_EVENT_KEY_DOWN:
	case SDL_EVENT_KEY_UP:            id = ev.key.windowID; break;
	case SDL_EVENT_WINDOW_CLOSE_REQUESTED:
		id = ev.window.windowID;
		break;
	default:
		return false;
	}
	std::lock_guard<std::mutex> hold(registry_mutex());
	for (pc_window *w : registry()) {
		if (!w || w->m_id != id || !w->m_imgui)
			continue;
		if (ev.type == SDL_EVENT_WINDOW_CLOSE_REQUESTED) {
			w->hide();   // the X button hides, like the other platforms
			return true;
		}
		ImGui::SetCurrentContext(w->m_imgui);
		// Text only while a text box is active. A held key (playing notes from the keyboard)
		// repeats text, and ImGui trickles text against mouse moves, so a drag lags behind
		if (ev.type == SDL_EVENT_TEXT_INPUT && !ImGui::GetIO().WantTextInput)
			return true;
		ImGui_ImplSDL3_ProcessEvent(&ev);
		return true;
	}
	return false;
}

} // namespace ui
