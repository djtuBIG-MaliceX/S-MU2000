// license:BSD-3-Clause
//
// The main window of the browser front end: the DIB the panel paints into
// (gdi_wasm.cpp rasterizes the GDI calls) and the frame-loop plumbing.
// There is no OS window and no pump of our own -- the page's
// requestAnimationFrame loop calls smu_frame() (js_wasm.cpp) and that runs
// ui::wasm_app::frame(), which ends by handing these bytes to JS.
//
// window_sdl.cpp is the SDL3 twin (DIB + streaming texture), window_win
// the GDI/DXGI one. Same framebuf idea, different upload.

#include "window_wasm.h"
#include "app_wasm.h"
#include "js_bridge_wasm.h"

#include <cstring>

namespace ui {

namespace {

framebuf g_fb;
unsigned long long g_serial = 0;
wasm_app *g_app = nullptr;   // the app the page's rAF loop ticks

} // namespace

framebuf &panel_surface()
{
	return g_fb;
}

bool surface_create(int w, int h)
{
	if (g_fb.dc && g_fb.bmp && g_fb.w == w && g_fb.h == h)
		return true;
	surface_destroy();
	if (w <= 0 || h <= 0)
		return false;

	g_fb.dc = CreateCompatibleDC(nullptr);
	if (!g_fb.dc)
		return false;

	// Top-down 32bpp, straight [R,G,B,A] (see gdi_wasm.cpp). The page
	// uploads these bytes straight into an ImageData, so no conversion
	// ever happens between here and the canvas.
	BITMAPINFO bmi = {};
	bmi.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
	bmi.bmiHeader.biWidth = w;
	bmi.bmiHeader.biHeight = -h;      // top-down
	bmi.bmiHeader.biPlanes = 1;
	bmi.bmiHeader.biBitCount = 32;
	bmi.bmiHeader.biCompression = BI_RGB;

	void *bits = nullptr;
	g_fb.bmp = CreateDIBSection(g_fb.dc, &bmi, DIB_RGB_COLORS, &bits, nullptr, 0);
	if (!g_fb.bmp || !bits) {
		if (g_fb.bmp)
			DeleteObject(g_fb.bmp);
		DeleteDC(g_fb.dc);
		g_fb = {};
		return false;
	}
	SelectObject(g_fb.dc, g_fb.bmp);
	g_fb.bits = bits;
	g_fb.w = w;
	g_fb.h = h;
	return true;
}

void surface_destroy()
{
	if (g_fb.bmp)
		DeleteObject(g_fb.bmp);
	if (g_fb.dc)
		DeleteDC(g_fb.dc);
	g_fb = {};
}

void install_frame_loop(wasm_app &app)
{
	// The browser needs no loop of its own: the page drives smu_frame()
	// from requestAnimationFrame and that finds the app here.
	g_app = &app;
}

wasm_app *frame_loop_app()
{
	return g_app;
}

// Painted one frame: bump the serial and hand the bytes to the canvas.
void present_panel()
{
	if (!g_fb.bits)
		return;
	g_serial++;
	jsbridge::present_panel(static_cast<const u8 *>(g_fb.bits), g_fb.w, g_fb.h, g_serial);
}

unsigned long long panel_serial()
{
	return g_serial;
}

} // namespace ui
