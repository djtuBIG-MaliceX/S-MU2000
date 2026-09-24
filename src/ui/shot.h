// license:BSD-3-Clause
//
// Write just the panel picture, with no window. Used to check the looks.
//
// gui.exe and the Mac GUI had their own copies; the only differences were
// historical (a missing null check here, GetDC vs nullptr there). One copy
// keeps --shot byte-identical on both.

#ifndef S_MU2000_UI_SHOT_H
#define S_MU2000_UI_SHOT_H

#pragma once

#include <cstdio>
#include <string>

#include "compat/gdi.h"
#include "ui/bridge.h"
#include "ui/panel.h"
#include "ui/png.h"
#include "ui/snapshot.h"
#include "ui/toolbar.h"

namespace ui {

// Renders the panel (with the window button bar, unless lcd_only) to a PNG.
// Same picture the window shows.
inline int write_shot(const std::string &path, int w, int h, bridge &br,
                      bool grid, bool lcd_only, const std::string &layout_path)
{
	panel p;
	std::string lerr;
	if (!layout_path.empty() && !p.lay().load(layout_path, lerr))
		std::fprintf(stderr, "配置: %s を開けない\n", layout_path.c_str());
	if (!lerr.empty())
		std::fprintf(stderr, "%s", lerr.c_str());
	p.set_lcd_only(lcd_only);
	// Like the window (leave room for the bar on top)
	toolbar bar;
	if (!lcd_only) {
		bar.set_items(window_bar_items());
		p.set_top_inset(toolbar::HEIGHT);
	}
	p.resize(w, h);
	p.set_grid(grid);

	BITMAPINFO bi{};
	bi.bmiHeader.biSize = sizeof(bi.bmiHeader);
	bi.bmiHeader.biWidth = w;
	bi.bmiHeader.biHeight = -h;                 // top down
	bi.bmiHeader.biPlanes = 1;
	bi.bmiHeader.biBitCount = 32;
	bi.bmiHeader.biCompression = BI_RGB;

	void *bits = nullptr;
	HDC dc = CreateCompatibleDC(nullptr);
	HBITMAP bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &bits, nullptr, 0);
	if (!bmp) {
		std::fprintf(stderr, "画面を作れない\n");
		DeleteDC(dc);
		return 1;
	}
	SelectObject(dc, bmp);

	snapshot s;
	br.read(s);
	p.set_volume(0.8);
	p.paint(dc, s, 0, "");
	bar.paint(dc, w);
	GdiFlush();

	const bool ok = write_png(path, static_cast<const u8 *>(bits), w, h, w * 4);

	DeleteObject(bmp);
	DeleteDC(dc);

	std::printf(ok ? "書き出した: %s（%d×%d）\n" : "書き出せない: %s\n",
	            path.c_str(), w, h);
	return ok ? 0 : 1;
}

} // namespace ui

#endif // S_MU2000_UI_SHOT_H
