// license:BSD-3-Clause
//
// 描くための小物。パネルとエディタで共用する。GDI しか使わない。
//
// What it includes is compat/gdi.h: real GDI on Windows, and on macOS a thin
// shim with the same surface, so callers cannot tell the two apart.
// See doc/porting-macos.md.

#ifndef S_MU2000_UI_DRAW_H
#define S_MU2000_UI_DRAW_H

#pragma once

#include "compat/gdi.h"

namespace ui {

// ---- 色

inline constexpr COLORREF BODY      = RGB(28, 30, 34);
inline constexpr COLORREF BODY_TOP  = RGB(44, 47, 53);
inline constexpr COLORREF BEZEL     = RGB(12, 12, 12);
inline constexpr COLORREF LCD_BACK  = RGB(150, 205, 45);
inline constexpr COLORREF LCD_GHOST = RGB(140, 194, 44);   // 消えている点。実物もうっすら見える
inline constexpr COLORREF LCD_DOT   = RGB(18, 22, 14);
inline constexpr COLORREF LED_OFF   = RGB(20, 28, 10);
inline constexpr COLORREF LED_ON    = RGB(178, 255, 51);
inline constexpr COLORREF BTN_FACE  = RGB(58, 62, 68);
inline constexpr COLORREF BTN_EDGE  = RGB(92, 97, 104);
inline constexpr COLORREF BTN_DOWN  = RGB(126, 170, 70);
inline constexpr COLORREF TEXT      = RGB(226, 229, 233);
inline constexpr COLORREF TEXT_DIM  = RGB(150, 155, 162);
inline constexpr COLORREF WHEEL     = RGB(46, 49, 54);
inline constexpr COLORREF WHEEL_EDGE= RGB(96, 101, 108);
inline constexpr COLORREF ACCENT    = RGB(126, 200, 90);

inline void fill(HDC dc, const RECT &r, COLORREF c)
{
	HBRUSH b = CreateSolidBrush(c);
	FillRect(dc, &r, b);
	DeleteObject(b);
}

// 角を落とした四角。ボタンはこれで描く
inline void round_box(HDC dc, const RECT &r, COLORREF face, COLORREF edge, int radius)
{
	HBRUSH b = CreateSolidBrush(face);
	HPEN   p = CreatePen(PS_SOLID, 1, edge);
	HGDIOBJ ob = SelectObject(dc, b), op = SelectObject(dc, p);
	RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
	SelectObject(dc, ob);
	SelectObject(dc, op);
	DeleteObject(b);
	DeleteObject(p);
}

inline void disc(HDC dc, int cx, int cy, int r, COLORREF face, COLORREF edge, int pen = 2)
{
	HBRUSH b = CreateSolidBrush(face);
	HPEN   p = CreatePen(PS_SOLID, pen, edge);
	HGDIOBJ ob = SelectObject(dc, b), op = SelectObject(dc, p);
	Ellipse(dc, cx - r, cy - r, cx + r, cy + r);
	SelectObject(dc, ob);
	SelectObject(dc, op);
	DeleteObject(b);
	DeleteObject(p);
}

inline void line(HDC dc, int x1, int y1, int x2, int y2, COLORREF c, int width)
{
	HPEN p = CreatePen(PS_SOLID, width, c);
	HGDIOBJ op = SelectObject(dc, p);
	MoveToEx(dc, x1, y1, nullptr);
	LineTo(dc, x2, y2);
	SelectObject(dc, op);
	DeleteObject(p);
}

// 字は UTF-8 で渡す。DrawTextA だと ANSI と見なされて日本語が化けるので、
// 一度 UTF-16 に直してから描く
inline void text_in(HDC dc, const RECT &r, const char *s, COLORREF c, HFONT f, UINT flags)
{
	if (!s || !s[0])
		return;
	wchar_t buf[256];
	const int n = MultiByteToWideChar(CP_UTF8, 0, s, -1, buf, 256);
	if (n <= 0)
		return;

	HGDIOBJ of = SelectObject(dc, f);
	SetTextColor(dc, c);
	SetBkMode(dc, TRANSPARENT);
	RECT rr = r;
	DrawTextW(dc, buf, n - 1, &rr, flags);
	SelectObject(dc, of);
}

} // namespace ui

#endif // S_MU2000_UI_DRAW_H
