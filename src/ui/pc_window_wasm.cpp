// license:BSD-3-Clause
//
// The reserved pane: one WebGL2 canvas on the page, one shared ImGui
// context, and the same five views everyone else renders. js_wasm.cpp
// calls init_gl once the page is up (it hands over the canvas' DOM id);
// the frame loop drives every visible window's frame(), and only the
// active one actually draws.

#include "pc_window_wasm.h"

#include <emscripten.h>
#include <emscripten/html5.h>
#include "imgui.h"
#include <GLES3/gl3.h>            // glClear in frame(); imgui.h must precede it
#include "backends/imgui_impl_opengl3.h"

#include <algorithm>
#include <cstdio>
#include <mutex>
#include <vector>

namespace ui {

namespace {

ImGuiContext *g_imgui = nullptr;
bool g_gl_ready = false;
int  g_active = -1;                 // pane index owning the canvas
int  g_view_w = 0, g_view_h = 0;
float g_dpr = 1.0f;

// Registration order == app member order (list, pc, fx, shapes, master):
// constructed in that order in app, and push_back keeps it.
std::vector<pc_window *> &registry()
{
	static std::vector<pc_window *> r;
	return r;
}

std::mutex &registry_mutex()
{
	static std::mutex m;
	return m;
}

// The same font the GDI shim uses (gdi_wasm.cpp reads /fonts too), so a
// Japanese string looks the same in the panel and the editor pane.
std::string cjk_font_file()
{
	static const char *cand[] = {
		"/fonts/NotoSansJP-VF.ttf",
		"/fonts/NotoSansJP-Regular.ttf",
		"/fonts/NotoSansCJK-Regular.ttc",
		"/fonts/NotoSans-Regular.ttf",
	};
	for (const char *c : cand) {
		FILE *f = std::fopen(c, "rb");
		if (f) {
			std::fclose(f);
			return c;
		}
	}
	return {};
}

void utf8_from_wide(const wchar_t *in, std::string &out)
{
	out.clear();
	for (const wchar_t *p = in; *p; ++p) {
		unsigned c = unsigned(*p);
		if (c >= 0xD800 && c < 0xDC00 && p[1] >= wchar_t(0xDC00) && p[1] <= wchar_t(0xDFFF)) {
			c = 0x10000 + ((c - 0xD800) << 10) + (unsigned(p[1]) - 0xDC00);
			++p;
		}
		if (c < 0x80) {
			out.push_back(char(c));
		} else if (c < 0x800) {
			out.push_back(char(0xC0 | (c >> 6)));
			out.push_back(char(0x80 | (c & 0x3F)));
		} else if (c < 0x10000) {
			out.push_back(char(0xE0 | (c >> 12)));
			out.push_back(char(0x80 | ((c >> 6) & 0x3F)));
			out.push_back(char(0x80 | (c & 0x3F)));
		} else {
			out.push_back(char(0xF0 | (c >> 18)));
			out.push_back(char(0x80 | ((c >> 12) & 0x3F)));
			out.push_back(char(0x80 | ((c >> 6) & 0x3F)));
			out.push_back(char(0x80 | (c & 0x3F)));
		}
	}
}

} // namespace

// ---- shared context ------------------------------------------------------

bool pc_window::init_gl(const std::string &canvas_selector, std::string &err)
{
	if (g_gl_ready)
		return true;

	// The canvas' WebGL2 context, created and made current here so every
	// GL call from the backend and the frame loop lands on it.
	EmscriptenWebGLContextAttributes attrs;
	emscripten_webgl_init_context_attributes(&attrs);
	attrs.majorVersion = 2;
	attrs.minorVersion = 0;
	attrs.enableExtensionsByDefault = 1;
	attrs.explicitSwapControl = 0;
	EMSCRIPTEN_WEBGL_CONTEXT_HANDLE gl =
	    emscripten_webgl_create_context(canvas_selector.c_str(), &attrs);
	if (!gl) {
		err = "WebGL2 context could not be made on " + canvas_selector;
		return false;
	}
	if (emscripten_webgl_make_context_current(gl) != EMSCRIPTEN_RESULT_SUCCESS) {
		emscripten_webgl_destroy_context(gl);
		err = "the WebGL2 context would not become current";
		return false;
	}

	ImGui::SetCurrentContext(nullptr);
	g_imgui = ImGui::CreateContext();
	ImGui::SetCurrentContext(g_imgui);
	ImGuiIO &io = ImGui::GetIO();
	io.ConfigFlags |= ImGuiConfigFlags_NavEnableKeyboard;

	const std::string font = cjk_font_file();
	if (!font.empty()) {
		ImFontConfig fc;
		fc.SizePixels = 16.0f;
		if (!io.Fonts->AddFontFromFileTTF(font.c_str(), 16.0f))
			io.Fonts->AddFontDefault();
	} else {
		io.Fonts->AddFontDefault();
	}

	if (!ImGui_ImplOpenGL3_Init("#version 300 es")) {
		err = "ImGui OpenGL3 backend failed to start";
		ImGui::DestroyContext(g_imgui);
		g_imgui = nullptr;
		ImGui::SetCurrentContext(nullptr);
		return false;
	}
	g_gl_ready = true;
	return true;
}

bool pc_window::gl_ready() { return g_gl_ready; }

void pc_window::set_viewport(int w, int h, float dpr)
{
	g_view_w = w;
	g_view_h = h;
	g_dpr = dpr > 0.0f ? dpr : 1.0f;
}

// ---- one window ----------------------------------------------------------

pc_window::~pc_window()
{
	std::lock_guard<std::mutex> hold(registry_mutex());
	auto &r = registry();
	r.erase(std::remove(r.begin(), r.end(), this), r.end());
	if (g_active >= int(r.size()))
		g_active = r.empty() ? -1 : int(r.size()) - 1;
}

bool pc_window::show(std::string &err)
{
	if (!g_gl_ready) {
		err = "the editor canvas is not ready yet";
		return false;
	}
	bool fresh = false;
	{
		std::lock_guard<std::mutex> hold(registry_mutex());
		fresh = std::find(registry().begin(), registry().end(), this) == registry().end();
		if (fresh)
			registry().push_back(this);
	}
	const bool was = m_visible;
	m_visible = true;
	// Newly opened windows take the pane, like a window coming forward.
	// Re-showing the current one does not disturb a deliberate switch.
	if (!was || active_index() < 0) {
		std::lock_guard<std::mutex> hold(registry_mutex());
		auto it = std::find(registry().begin(), registry().end(), this);
		g_active = it == registry().end() ? -1 : int(it - registry().begin());
	}
	(void)err;
	return true;
}

void pc_window::hide()
{
	m_visible = false;
	if (active_index() >= 0 && pane(active_index()) == this) {
		// Hand the canvas to another visible tab, or to nobody
		std::lock_guard<std::mutex> hold(registry_mutex());
		g_active = -1;
		for (size_t i = 0; i < registry().size(); i++) {
			if (registry()[i] && registry()[i]->m_visible) {
				g_active = int(i);
				break;
			}
		}
	}
}

void pc_window::close()
{
	hide();   // nothing else to tear down: the context is shared
}

void pc_window::shutdown(bridge &br)
{
	if (!m_visible && !m_was_visible)
		return;
	if (g_imgui) {
		ImGui::SetCurrentContext(g_imgui);
		m_view->hidden(br);
	}
	m_visible = false;
	m_was_visible = false;
}

void pc_window::frame(xg::model &m, const xg_snapshot &ram, bridge &br)
{
	if (m_was_visible && !m_visible && g_imgui) {
		ImGui::SetCurrentContext(g_imgui);
		m_view->hidden(br);              // closed between frames: release what it held
	}
	m_was_visible = m_visible;
	if (!m_visible || active_index() < 0 || pane(active_index()) != this)
		return;
	draw(m, ram, br);
}

void pc_window::draw(xg::model &m, const xg_snapshot &ram, bridge &br)
{
	if (!g_imgui || g_view_w <= 0 || g_view_h <= 0)
		return;
	ImGui::SetCurrentContext(g_imgui);
	ImGuiIO &io = ImGui::GetIO();
	io.DisplaySize = ImVec2(float(g_view_w), float(g_view_h));
	io.DisplayFramebufferScale = ImVec2(g_dpr, g_dpr);

	ImGui_ImplOpenGL3_NewFrame();
	ImGui::NewFrame();
	m_view->draw(m, ram, br);
	xgui::drag_flush(br);          // the thinned sends of a dragged value
	ImGui::Render();

	glClearColor(0.10f, 0.10f, 0.11f, 1.0f);
	glClear(GL_COLOR_BUFFER_BIT);
	ImGui_ImplOpenGL3_RenderDrawData(ImGui::GetDrawData());
}

void pc_window::set_drop_handler(void (*fn)(const std::string &path))
{
	(void)fn;   // the page routes drops through smu_open_smf
}

// ---- pane list (js side) ---------------------------------------------------

int pc_window::pane_count()
{
	std::lock_guard<std::mutex> hold(registry_mutex());
	int n = 0;
	for (pc_window *w : registry())
		if (w && w->m_visible)
			n++;
	return n;
}

pc_window *pc_window::pane(int i)
{
	std::lock_guard<std::mutex> hold(registry_mutex());
	if (i < 0 || i >= int(registry().size()))
		return nullptr;
	return registry()[size_t(i)];
}

int pc_window::active_index() { return g_active; }

bool pc_window::activate(int i)
{
	pc_window *w = pane(i);
	if (!w || !w->m_visible)
		return false;
	g_active = i;
	return true;
}

std::string pc_window::title_utf8() const
{
	std::string out;
	utf8_from_wide(m_view->title(), out);
	return out;
}

// ---- input ---------------------------------------------------------------

void pc_window::on_mouse_move(float x, float y)
{
	if (!g_imgui)
		return;
	ImGui::SetCurrentContext(g_imgui);
	ImGui::GetIO().AddMousePosEvent(x, y);
}

void pc_window::on_mouse_button(int button, bool down)
{
	if (!g_imgui)
		return;
	ImGui::SetCurrentContext(g_imgui);
	ImGui::GetIO().AddMouseButtonEvent(button, down);
}

void pc_window::on_wheel(float dx, float dy)
{
	if (!g_imgui)
		return;
	ImGui::SetCurrentContext(g_imgui);
	ImGui::GetIO().AddMouseWheelEvent(dx, dy);
}

// GLFW-style codes (js_wasm.cpp maps DOM key codes onto these). Only the
// keys ImGui's widgets actually read.
void pc_window::on_key(int key_code, bool down)
{
	if (!g_imgui)
		return;
	ImGui::SetCurrentContext(g_imgui);
	ImGuiIO &io = ImGui::GetIO();
	ImGuiKey k = ImGuiKey_None;
	switch (key_code) {
	case 256: k = ImGuiKey_Escape; break;
	case 257: k = ImGuiKey_Enter; break;
	case 258: k = ImGuiKey_Tab; break;
	case 259: k = ImGuiKey_Backspace; break;
	case 261: k = ImGuiKey_Delete; break;
	case 262: k = ImGuiKey_RightArrow; break;
	case 263: k = ImGuiKey_LeftArrow; break;
	case 264: k = ImGuiKey_DownArrow; break;
	case 265: k = ImGuiKey_UpArrow; break;
	case 266: k = ImGuiKey_PageUp; break;
	case 267: k = ImGuiKey_PageDown; break;
	case 268: k = ImGuiKey_Home; break;
	case 269: k = ImGuiKey_End; break;
	default:
		// Letters/digits arrive through on_char; ImGui reads Home etc. above
		if (key_code >= 32 && key_code < 127) {
			if (down)
				io.AddInputCharacter(unsigned(key_code));
			return;
		}
		return;
	}
	io.AddKeyEvent(k, down);
}

void pc_window::on_char(unsigned codepoint)
{
	if (!g_imgui)
		return;
	ImGui::SetCurrentContext(g_imgui);
	ImGui::GetIO().AddInputCharacter(codepoint);
}

bool pc_window::wants_text()
{
	if (!g_imgui)
		return false;
	ImGui::SetCurrentContext(g_imgui);
	return ImGui::GetIO().WantTextInput;
}

} // namespace ui
