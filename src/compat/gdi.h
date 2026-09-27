// license:BSD-3-Clause
//
// The drawing surface, cut down to just the calls this project uses.
//
// On Windows this is <windows.h> and nothing else. On macOS it declares the
// small slice of GDI the panel actually calls (about twenty functions), and
// gdi_mac.cpp fills that in with CoreGraphics and CoreText.
//
// It is shaped this way so that panel.cpp / editor.cpp / effects.cpp /
// layout.cpp / svg.cpp -- some 2500 lines -- compile unchanged on both
// platforms. A hand-ported drawing layer has to be compared by eye to know it
// matches; with the surface pinned here, the two panels agree by construction.
//
// The colour packing is Windows's, with red in the low byte and blue in bits
// 16-23. panel.txt writes colours as #rrggbb, so this must not be changed.

#ifndef S_MU2000_COMPAT_GDI_H
#define S_MU2000_COMPAT_GDI_H

#pragma once

#if defined(_WIN32)

// ---- Windows: use the real thing ----------------------------------------
//
// Nothing to shim. Everything below this point exists only for macOS.

#include <windows.h>

#else

// ---- macOS: imitate the slice of GDI that the panel uses ----------------

#include <cstddef>
#include <cstdint>

// ---- Base types

// So we don't collide with the old Quickdraw Polygon
#define Polygon GdiPolygon

using BYTE  = uint8_t;
using WORD  = uint16_t;
using DWORD = uint32_t;
using UINT  = uint32_t;
using INT   = int32_t;
// bool, agreeing with objc/objc.h's BOOL, so this header can share a
// translation unit with Cocoa. Nothing here depends on int-ness: the
// functions return TRUE/FALSE and callers ignore the result.
using BOOL  = bool;
using UINT_PTR = uintptr_t;

// LONG is `long`, as it is in the Windows headers, rather than a fixed
// 32-bit type. On 64-bit macOS that makes it 64 bits wide, which nothing here
// depends on -- but the drawing code writes things like std::max(1L, ...) with
// a coordinate, and those only deduce if the two are the same type.
//
// Structures whose layout is dictated from outside (BITMAPINFOHEADER) spell
// their widths out instead of using LONG or DWORD.
using LONG = long;

#ifndef TRUE
#define TRUE  1
#define FALSE 0
#endif

// ---- Colour, packed exactly as Windows packs it: red low, blue in bits 16-23

using COLORREF = DWORD;

inline constexpr COLORREF RGB(int r, int g, int b)
{
	return COLORREF((DWORD(r) & 0xff) | ((DWORD(g) & 0xff) << 8) | ((DWORD(b) & 0xff) << 16));
}

inline constexpr BYTE GetRValue(COLORREF c) { return BYTE(c & 0xff); }
inline constexpr BYTE GetGValue(COLORREF c) { return BYTE((c >> 8) & 0xff); }
inline constexpr BYTE GetBValue(COLORREF c) { return BYTE((c >> 16) & 0xff); }

// ---- Coordinates

struct RECT  { LONG left, top, right, bottom; };
struct POINT { LONG x, y; };
struct SIZE  { LONG cx, cy; };

inline void SetRect(RECT *r, int l, int t, int rr, int b)
{
	r->left = l; r->top = t; r->right = rr; r->bottom = b;
}

inline void InflateRect(RECT *r, int dx, int dy)
{
	r->left -= dx; r->right += dx; r->top -= dy; r->bottom += dy;
}

// ---- Handles
//
// GDI lets an HBRUSH, an HPEN and an HFONT all be passed as an HGDIOBJ, so
// this does too. What they point at is gdi_mac.cpp's business.

struct gdi_object;

using HGDIOBJ = gdi_object *;
using HBRUSH  = gdi_object *;
using HPEN    = gdi_object *;
using HFONT   = gdi_object *;
using HBITMAP = gdi_object *;
using HDC     = gdi_object *;

// ---- Constants

// Pens
enum { PS_SOLID = 0, PS_DASH = 1, PS_DOT = 2, PS_NULL = 5 };

// Stock objects
enum { WHITE_BRUSH = 0, LTGRAY_BRUSH = 1, GRAY_BRUSH = 2, DKGRAY_BRUSH = 3,
       BLACK_BRUSH = 4, NULL_BRUSH = 5, HOLLOW_BRUSH = 5,
       WHITE_PEN = 6, BLACK_PEN = 7, NULL_PEN = 8 };

// Weights
enum { FW_DONTCARE = 0, FW_THIN = 100, FW_NORMAL = 400, FW_MEDIUM = 500,
       FW_SEMIBOLD = 600, FW_BOLD = 700, FW_HEAVY = 900 };

// Text background
enum { OPAQUE = 2, TRANSPARENT = 1 };

// Polygon fill rule
enum { ALTERNATE = 1, WINDING = 2 };

// Text placement. Note that DT_LEFT and DT_TOP are 0: in GDI being left- or
// top-aligned is the default, so you cannot test for it, only for the others.
enum { DT_TOP = 0x0000, DT_LEFT = 0x0000, DT_CENTER = 0x0001, DT_RIGHT = 0x0002,
       DT_VCENTER = 0x0004, DT_BOTTOM = 0x0008, DT_WORDBREAK = 0x0010,
       DT_SINGLELINE = 0x0020, DT_NOCLIP = 0x0100, DT_END_ELLIPSIS = 0x8000 };

// Font description bits
enum { DEFAULT_CHARSET = 1, ANSI_CHARSET = 0 };
enum { OUT_DEFAULT_PRECIS = 0, OUT_TT_PRECIS = 4 };
enum { CLIP_DEFAULT_PRECIS = 0 };
enum { DEFAULT_QUALITY = 0, DRAFT_QUALITY = 1, PROOF_QUALITY = 2,
       CLEARTYPE_QUALITY = 5, ANTIALIASED_QUALITY = 4 };
enum { DEFAULT_PITCH = 0, FIXED_PITCH = 1, VARIABLE_PITCH = 2 };
enum { FF_DONTCARE = 0, FF_SWISS = 0x20, FF_ROMAN = 0x10 };

// DIB, which gui's --shot uses to get at the pixels
enum { BI_RGB = 0, DIB_RGB_COLORS = 0, SRCCOPY = 0x00CC0020 };

struct RGBQUAD { BYTE rgbBlue, rgbGreen, rgbRed, rgbReserved; };

// Spelled out at fixed widths rather than with LONG and DWORD: this structure
// is exactly 40 bytes, and it has to stay that way
struct BITMAPINFOHEADER {
	int32_t  biSize;
	int32_t  biWidth, biHeight;
	uint16_t biPlanes, biBitCount;
	uint32_t biCompression, biSizeImage;
	int32_t  biXPelsPerMeter, biYPelsPerMeter;
	uint32_t biClrUsed, biClrImportant;
};

struct BITMAPINFO { BITMAPINFOHEADER bmiHeader; RGBQUAD bmiColors[1]; };

// Code pages
enum { CP_ACP = 0, CP_UTF8 = 65001 };

// ---- Making objects

HBRUSH CreateSolidBrush(COLORREF color);
HPEN   CreatePen(int style, int width, COLORREF color);
HFONT  CreateFontA(int height, int width, int escapement, int orientation,
                   int weight, DWORD italic, DWORD underline, DWORD strike_out,
                   DWORD charset, DWORD out_precision, DWORD clip_precision,
                   DWORD quality, DWORD pitch_and_family, const char *face);
HGDIOBJ GetStockObject(int which);
HGDIOBJ SelectObject(HDC dc, HGDIOBJ obj);
BOOL    DeleteObject(HGDIOBJ obj);

// ---- Drawing

int  FillRect(HDC dc, const RECT *r, HBRUSH brush);
BOOL RoundRect(HDC dc, int left, int top, int right, int bottom, int ew, int eh);
BOOL Ellipse(HDC dc, int left, int top, int right, int bottom);
BOOL Arc(HDC dc, int left, int top, int right, int bottom,
         int xr1, int yr1, int xr2, int yr2);
BOOL MoveToEx(HDC dc, int x, int y, POINT *prev);
BOOL LineTo(HDC dc, int x, int y);
BOOL Polygon(HDC dc, const POINT *pts, int n);
BOOL PolyPolygon(HDC dc, const POINT *pts, const INT *counts, int n);
BOOL Polyline(HDC dc, const POINT *pts, int n);

// Not GDI: pastes w x h pixels at (x, y), each 0xAARRGGBB with alpha already
// multiplied in (the layout Cairo's ARGB32 and a little-endian CGImage use).
// Windows does the same through GdiAlphaBlend (see ui/svg.cpp)
BOOL smu_blit_premul(HDC dc, int x, int y, int w, int h, const uint32_t *px);

COLORREF SetTextColor(HDC dc, COLORREF color);
int      SetBkMode(HDC dc, int mode);
int      SetBkColor(HDC dc, COLORREF color);
int      SetPolyFillMode(HDC dc, int mode);
int      DrawTextW(HDC dc, const wchar_t *text, int count, RECT *r, UINT flags);

int MultiByteToWideChar(UINT codepage, DWORD flags, const char *src, int src_len,
                        wchar_t *dst, int dst_len);

// ---- Drawing with no window at all: --shot makes one DIB and draws into it

HDC     CreateCompatibleDC(HDC like);
HBITMAP CreateDIBSection(HDC dc, const BITMAPINFO *info, UINT usage,
                         void **bits, void *section, DWORD offset);
void    GdiFlush(void);
BOOL    DeleteDC(HDC dc);

// The way in for drawing straight into a window (only gui_mac.cpp uses it).
//
// The CGContext AppKit hands us already has its origin top-left and y running
// down, so unlike a surface we allocated ourselves it must not be flipped.
// Wrapping it here hides that difference, so both kinds of DC draw in the same
// coordinates GDI uses.
void *smu_gdi_wrap_view_context(void *cg_context, int w, int h);

#endif // _WIN32

#endif // S_MU2000_COMPAT_GDI_H
