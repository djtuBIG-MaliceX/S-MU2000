// license:BSD-3-Clause
//
// The Linux half of compat/gdi.h: the slice of GDI the panel draws through,
// implemented over Cairo (image surfaces) with fonts from fontconfig/FreeType.
//
// Coordinates and colours follow GDI, not Cairo:
//
//   - every context handed out here draws y-down from the top-left, the way
//     GDI works. A Cairo image surface already starts that way, so unlike the
//     macOS half there is nothing to flip.
//   - a pen of odd width is nudged half a pixel, which is what lands GDI's
//     rules and tick marks on one crisp column instead of smearing over two
//     (same rule as gdi_mac.cpp, so both ports agree).
//   - a filled rectangle covers [left, right) x [top, bottom), like GDI's.
//   - a 32-bit DIB comes down as B, G, R, X per pixel: Cairo's ARGB32 on a
//     little-endian machine, and the order ui::write_png expects.
//
// Text is the one thing that cannot match exactly. GDI is asked for "Segoe UI"
// and Linux does not have it, so that request is answered with fontconfig's
// sans-serif (usually DejaVu Sans). Glyph metrics differ slightly from the
// Windows build, which means layout that was tuned around text via panel.txt
// may want a nudge. The wrapping rules (hard breaks, CJK token breaks) are
// the same code as the macOS half, so line *breaks* agree even when widths do
// not.
//
// Pixels are Cairo's (including FreeType's rasterizer), so a --shot PNG is
// not expected to be byte-identical to the Windows or macOS ones. What must
// agree: the palette, the geometry, the SVG art, and Linux DIB-vs-window at
// 0 differing bytes (doc/porting-linux-gui.md).

#include "compat/gdi.h"

#include <cairo/cairo-ft.h>
#include <cairo/cairo.h>
#include <fontconfig/fontconfig.h>

#include <algorithm>
#include <cmath>
#include <cstring>
#include <string>
#include <vector>

namespace {

enum obj_kind { OBJ_BRUSH, OBJ_PEN, OBJ_FONT, OBJ_BITMAP, OBJ_DC };

} // namespace

// ---- The objects an HGDIOBJ can point at ---------------------------------
//
// Declared in gdi.h and defined only here, so nothing outside this file can
// do anything with a handle except pass it back in. Same shape as gdi_mac.cpp.

struct gdi_object {
	int  kind;
	bool stock = false;          // a GetStockObject() one, so never freed

	explicit gdi_object(int k) : kind(k) {}
	virtual ~gdi_object() {}
};

struct gdi_brush : gdi_object {
	COLORREF color = 0;
	bool     none  = false;      // NULL_BRUSH: fills nothing

	gdi_brush() : gdi_object(OBJ_BRUSH) {}
};

struct gdi_pen : gdi_object {
	COLORREF color = 0;
	int      width = 1;
	bool     none  = false;      // NULL_PEN: outlines nothing

	gdi_pen() : gdi_object(OBJ_PEN) {}
};

struct gdi_font : gdi_object {
	int         height = -12;    // GDI: negative means character height
	int         weight = FW_NORMAL;
	std::string face;
	cairo_font_face_t *face_ft = nullptr;   // built once: the panel makes 3
	double      px = 12.0;

	gdi_font() : gdi_object(OBJ_FONT) {}
	~gdi_font() override { if (face_ft) cairo_font_face_destroy(face_ft); }
};

struct gdi_bitmap : gdi_object {
	int               w = 0, h = 0;
	std::vector<BYTE> data;
	cairo_surface_t  *surf = nullptr;
	cairo_t          *ctx = nullptr;

	gdi_bitmap() : gdi_object(OBJ_BITMAP) {}
	~gdi_bitmap() override
	{
		if (ctx) cairo_destroy(ctx);
		if (surf) cairo_surface_destroy(surf);
	}
};

struct gdi_dc : gdi_object {
	cairo_t         *ctx = nullptr;
	bool             owns = false;   // only free what we created
	cairo_surface_t *surf = nullptr; // flushed by GdiFlush(), owned elsewhere
	int              w = 0, h = 0;

	HGDIOBJ pen = nullptr, brush = nullptr, font = nullptr;
	COLORREF text = RGB(0, 0, 0);
	int      fill_mode = ALTERNATE;
	POINT    cur{ 0, 0 };

	gdi_dc() : gdi_object(OBJ_DC) {}
	~gdi_dc() override { if (owns && ctx) cairo_destroy(ctx); }
};

namespace {

// Every live DC, so GdiFlush() can reach them all. Drawing is single-threaded.
std::vector<gdi_dc *> g_dcs;

void set_fill(gdi_dc *dc, COLORREF c)
{
	cairo_set_source_rgb(dc->ctx, GetRValue(c) / 255.0, GetGValue(c) / 255.0,
	                     GetBValue(c) / 255.0);
}

void set_stroke(gdi_dc *dc, COLORREF c)
{
	cairo_set_source_rgb(dc->ctx, GetRValue(c) / 255.0, GetGValue(c) / 255.0,
	                     GetBValue(c) / 255.0);
}

gdi_brush *brush_of(gdi_dc *dc)
{
	return (dc->brush && dc->brush->kind == OBJ_BRUSH)
	           ? static_cast<gdi_brush *>(dc->brush)
	           : nullptr;
}

gdi_pen *pen_of(gdi_dc *dc)
{
	return (dc->pen && dc->pen->kind == OBJ_PEN)
	           ? static_cast<gdi_pen *>(dc->pen)
	           : nullptr;
}

// Fills the current path using the DC's brush and fill rule. GDI's default
// rule is ALTERNATE, so the even-odd one, and the SVG art depends on it.
void fill_path(gdi_dc *dc)
{
	gdi_brush *br = brush_of(dc);
	if (!br || br->none)
		return;
	set_fill(dc, br->color);
	cairo_set_fill_rule(dc->ctx, dc->fill_mode == ALTERNATE ? CAIRO_FILL_RULE_EVEN_ODD
	                                                       : CAIRO_FILL_RULE_WINDING);
	cairo_fill_preserve(dc->ctx);
}

void stroke_path(gdi_dc *dc)
{
	gdi_pen *pen = pen_of(dc);
	if (!pen || pen->none)
		return;

	cairo_save(dc->ctx);
	// GDI paints an odd-width line on exact pixel boundaries: a 1-pixel rule at
	// y = 5 covers row 5. Centred on y = 5 it would straddle rows 4 and 5, so
	// shift by half a pixel. Even widths already line up. (Same rule as the
	// macOS half. save/restore undoes the shift along with the stroke
	// parameters, so what follows never sees it.)
	if (pen->width & 1)
		cairo_translate(dc->ctx, 0.5, 0.5);
	set_stroke(dc, pen->color);
	cairo_set_line_width(dc->ctx, std::max(1, pen->width));
	cairo_set_line_cap(dc->ctx, CAIRO_LINE_CAP_BUTT);   // GDI's ends are square
	cairo_set_line_join(dc->ctx, CAIRO_LINE_JOIN_MITER);
	cairo_stroke(dc->ctx);
	cairo_restore(dc->ctx);
}

// A rounded-rectangle path in device coordinates (no transform, so the stroke
// stays uniform). GDI takes the corner ellipse's width and height; the radius
// is half the smaller, like the macOS half.
void path_round_rect(cairo_t *cr, double left, double top, double right, double bottom,
                     double ew, double eh)
{
	const double rx = std::abs(ew) / 2.0, ry = std::abs(eh) / 2.0;
	const double r = std::max(0.0, std::min(rx, ry));
	if (r <= 0.0) {
		cairo_rectangle(cr, left, top, right - left, bottom - top);
		return;
	}
	const double pi = 3.14159265358979323846;
	cairo_move_to(cr, left + r, top);
	cairo_line_to(cr, right - r, top);
	cairo_arc(cr, right - r, top + r, r, -pi / 2, 0);
	cairo_line_to(cr, right, bottom - r);
	cairo_arc(cr, right - r, bottom - r, r, 0, pi / 2);
	cairo_line_to(cr, left + r, bottom);
	cairo_arc(cr, left + r, bottom - r, r, pi / 2, pi);
	cairo_line_to(cr, left, top + r);
	cairo_arc(cr, left + r, top + r, r, pi, pi * 3 / 2);
	cairo_close_path(cr);
}

// An ellipse path, sampled rather than drawn through a scaled transform, so
// the stroke width stays uniform (same reason the macOS half flattens Arc).
void path_ellipse(cairo_t *cr, double left, double top, double right, double bottom)
{
	const double cx = (left + right) / 2.0, cy = (top + bottom) / 2.0;
	const double rx = std::abs(right - left) / 2.0, ry = std::abs(bottom - top) / 2.0;
	if (rx <= 0.0 || ry <= 0.0)
		return;
	const double pi = 3.14159265358979323846;
	const int steps = std::max(16, int((rx + ry) * pi / 2.0) + 1);
	for (int i = 0; i <= steps; i++) {
		const double a = 2.0 * pi * i / steps;
		const double px = cx + std::cos(a) * rx, py = cy + std::sin(a) * ry;
		if (i == 0)
			cairo_move_to(cr, px, py);
		else
			cairo_line_to(cr, px, py);
	}
	cairo_close_path(cr);
}

// ---- Text -----------------------------------------------------------------
//
// The u16 pipeline (hard breaks, CJK-aware wrapping) is the same code as the
// macOS half, so line breaks agree there. Only the measuring and the glyph
// drawing go through Cairo: the u16 is encoded to UTF-8 at that boundary.

cairo_font_face_t *match_face(const std::string &family, double px, bool bold)
{
	FcPattern *pat = FcPatternCreate();
	if (!pat)
		return nullptr;
	FcPatternAddString(pat, FC_FAMILY, reinterpret_cast<const FcChar8 *>(family.c_str()));
	FcPatternAddDouble(pat, FC_SIZE, px);
	if (bold)
		FcPatternAddInteger(pat, FC_WEIGHT, FC_WEIGHT_BOLD);
	FcConfigSubstitute(nullptr, pat, FcMatchPattern);
	FcDefaultSubstitute(pat);
	FcResult res = FcResultNoMatch;
	FcPattern *m = FcFontMatch(nullptr, pat, &res);
	cairo_font_face_t *face = nullptr;
	if (m) {
		face = cairo_ft_font_face_create_for_pattern(m);
		FcPatternDestroy(m);
	}
	FcPatternDestroy(pat);
	return face;
}

cairo_font_face_t *default_face(double px)
{
	static cairo_font_face_t *f = match_face("sans-serif", 12.0, false);
	(void)px;
	return f;
}

void apply_font(cairo_t *cr, gdi_font *f)
{
	if (f && f->face_ft) {
		cairo_set_font_face(cr, f->face_ft);
		cairo_set_font_size(cr, f->px);
	} else {
		if (cairo_font_face_t *d = default_face(12.0)) {
			cairo_set_font_face(cr, d);
			cairo_set_font_size(cr, 12.0);
		}
	}
}

// Scratch context for measuring. Kept off to the side so measuring never
// disturbs the DC being drawn into.
cairo_t *measure_cr()
{
	static cairo_surface_t *surf = cairo_image_surface_create(CAIRO_FORMAT_A8, 8, 8);
	static cairo_t *cr = cairo_create(surf);
	return cr;
}

std::string to_utf8(const std::u16string &s)
{
	std::string out;
	out.reserve(s.size());
	for (size_t i = 0; i < s.size();) {
		char32_t cp = s[i];
		size_t step = 1;
		if (cp >= 0xD800 && cp <= 0xDBFF && i + 1 < s.size() &&
		    s[i + 1] >= 0xDC00 && s[i + 1] <= 0xDFFF) {
			cp = 0x10000 + ((char32_t(cp) - 0xD800) << 10) + (char32_t(s[i + 1]) - 0xDC00);
			step = 2;
		}
		if (cp < 0x80) {
			out.push_back(char(cp));
		} else if (cp < 0x800) {
			out.push_back(char(0xC0 | (cp >> 6)));
			out.push_back(char(0x80 | (cp & 0x3F)));
		} else if (cp < 0x10000) {
			out.push_back(char(0xE0 | (cp >> 12)));
			out.push_back(char(0x80 | ((cp >> 6) & 0x3F)));
			out.push_back(char(0x80 | (cp & 0x3F)));
		} else {
			out.push_back(char(0xF0 | (cp >> 18)));
			out.push_back(char(0x80 | ((cp >> 12) & 0x3F)));
			out.push_back(char(0x80 | ((cp >> 6) & 0x3F)));
			out.push_back(char(0x80 | (cp & 0x3F)));
		}
		i += step;
	}
	return out;
}

double measure(gdi_font *font, cairo_t *dc_cr, const std::u16string &s)
{
	cairo_t *cr = measure_cr();
	// Same face and size as the drawing context is about to use.
	if (font && font->face_ft) {
		cairo_set_font_face(cr, font->face_ft);
		cairo_set_font_size(cr, font->px);
	} else {
		(void)dc_cr;
		if (cairo_font_face_t *d = default_face(12.0)) {
			cairo_set_font_face(cr, d);
			cairo_set_font_size(cr, 12.0);
		}
	}
	const std::string u8 = to_utf8(s);
	cairo_text_extents_t ex{};
	cairo_text_extents(cr, u8.c_str(), &ex);
	return ex.x_advance;
}

std::vector<std::u16string> split_hard(const std::u16string &s)
{
	std::vector<std::u16string> out;
	std::u16string cur;
	for (char16_t c : s) {
		if (c == u'\n' || c == u'\r') {
			out.push_back(cur);
			cur.clear();
			continue;
		}
		cur.push_back(c);
	}
	out.push_back(cur);
	return out;
}

// CJK has no spaces, so wrapping on spaces alone would leave a Japanese
// sentence as one unbreakable line. GDI breaks between CJK characters, so
// treat each one as its own token. (Same rule as the macOS half.)
bool breakable(char32_t cp)
{
	return (cp >= 0x2E80 && cp <= 0x9FFF) ||      // CJK radicals through unified
	       (cp >= 0xAC00 && cp <= 0xD7AF) ||      // Hangul syllables
	       (cp >= 0xF900 && cp <= 0xFAFF) ||      // CJK compatibility
	       (cp >= 0xFF00 && cp <= 0xFF60);        // fullwidth forms
}

// Splits into tokens: a run of non-space Latin is one token, each CJK
// character is one, and runs of spaces are their own so they can be dropped at
// a line break. (Same code as the macOS half.)
std::vector<std::u16string> tokenize(const std::u16string &s)
{
	std::vector<std::u16string> out;
	std::u16string word;
	auto flush = [&] { if (!word.empty()) { out.push_back(word); word.clear(); } };

	for (size_t i = 0; i < s.size();) {
		char16_t c = s[i];
		char32_t cp = c;
		size_t step = 1;
		if (c >= 0xD800 && c <= 0xDBFF && i + 1 < s.size() &&
		    s[i + 1] >= 0xDC00 && s[i + 1] <= 0xDFFF) {
			cp = 0x10000 + ((char32_t(c) - 0xD800) << 10) + (char32_t(s[i + 1]) - 0xDC00);
			step = 2;
		}

		if (c == u' ' || c == u'\t') {
			flush();
			std::u16string sp;
			while (i < s.size() && (s[i] == u' ' || s[i] == u'\t')) {
				sp.push_back(s[i]);
				i++;
			}
			out.push_back(sp);
			continue;
		}
		if (breakable(cp)) {
			flush();
			out.push_back(s.substr(i, step));
			i += step;
			continue;
		}
		word.append(s, i, step);
		i += step;
	}
	flush();
	return out;
}

std::vector<std::u16string> wrap_text(const std::u16string &s, gdi_font *font, cairo_t *dc_cr,
                                      double max_w)
{
	std::vector<std::u16string> out;
	if (max_w <= 0.0)
		return split_hard(s);

	for (const std::u16string &para : split_hard(s)) {
		std::u16string line;
		for (const std::u16string &tok : tokenize(para)) {
			const bool space = tok.find_first_not_of(u" \t") == std::u16string::npos;
			if (space && line.empty())
				continue;
			const std::u16string cand = line + tok;
			if (measure(font, dc_cr, cand) <= max_w || line.empty()) {
				line = cand;
				continue;
			}
			out.push_back(line);
			line = space ? std::u16string() : tok;
		}
		out.push_back(line);
	}
	return out;
}

} // namespace


// ---- Making objects -------------------------------------------------------

HBRUSH CreateSolidBrush(COLORREF color)
{
	auto *b = new gdi_brush();
	b->color = color;
	return b;
}

HPEN CreatePen(int style, int width, COLORREF color)
{
	auto *p = new gdi_pen();
	p->color = color;
	p->width = std::max(1, width);
	p->none  = (style == PS_NULL);
	return p;
}

HFONT CreateFontA(int height, int width, int escapement, int orientation,
                  int weight, DWORD italic, DWORD underline, DWORD strike_out,
                  DWORD charset, DWORD out_precision, DWORD clip_precision,
                  DWORD quality, DWORD pitch_and_family, const char *face)
{
	(void)width; (void)escapement; (void)orientation; (void)italic;
	(void)underline; (void)strike_out; (void)charset; (void)out_precision;
	(void)clip_precision; (void)quality; (void)pitch_and_family;

	auto *f = new gdi_font();
	f->height = height ? height : -12;
	f->weight = weight;
	f->face   = face ? face : "";

	f->px = std::max(1.0, double(std::abs(f->height)));

	// "Segoe UI" is what panel.cpp asks for and it does not exist here, so
	// that request is answered with fontconfig's sans-serif (usually DejaVu
	// Sans) -- the same fallback idea as the macOS half answering with the
	// system UI font.
	const std::string family =
	    (!f->face.empty() && f->face != "Segoe UI") ? f->face : "sans-serif";
	// FW_SEMIBOLD and up. panel.cpp uses FW_BOLD for the labels.
	f->face_ft = match_face(family, f->px, f->weight >= FW_SEMIBOLD);
	return f;
}

HGDIOBJ GetStockObject(int which)
{
	static gdi_brush white_brush = [] { gdi_brush b; b.color = RGB(255,255,255); b.stock = true; return b; }();
	static gdi_brush null_brush  = [] { gdi_brush b; b.none = true; b.stock = true; return b; }();
	static gdi_pen   black_pen   = [] { gdi_pen   p; p.color = RGB(0,0,0); p.stock = true; return p; }();
	static gdi_pen   white_pen   = [] { gdi_pen   p; p.color = RGB(255,255,255); p.stock = true; return p; }();
	static gdi_pen   null_pen    = [] { gdi_pen   p; p.none = true; p.stock = true; return p; }();

	switch (which) {
	// NULL_BRUSH and HOLLOW_BRUSH are the same value, so one case covers both
	case NULL_BRUSH: return &null_brush;
	case NULL_PEN:   return &null_pen;
	case BLACK_PEN:                     return &black_pen;
	case WHITE_PEN:                     return &white_pen;
	default:                            return &white_brush;
	}
}

HGDIOBJ SelectObject(HDC hdc, HGDIOBJ obj)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !obj)
		return nullptr;

	switch (obj->kind) {
	case OBJ_BRUSH: {
		HGDIOBJ old = dc->brush;
		dc->brush = obj;
		return old;
	}
	case OBJ_PEN: {
		HGDIOBJ old = dc->pen;
		dc->pen = obj;
		return old;
	}
	case OBJ_FONT: {
		HGDIOBJ old = dc->font;
		dc->font = obj;
		return old;
	}
	case OBJ_BITMAP: {
		// Binding a bitmap to a DC: this is how --shot gets a surface to draw
		// on. The bitmap keeps ownership of the surface and context.
		auto *bm = static_cast<gdi_bitmap *>(obj);
		dc->ctx  = bm->ctx;
		dc->surf = bm->surf;
		dc->owns = false;
		dc->w = bm->w;
		dc->h = bm->h;
		return nullptr;
	}
	default:
		break;
	}
	return nullptr;
}

BOOL DeleteObject(HGDIOBJ obj)
{
	if (!obj)
		return FALSE;
	if (obj->stock)
		return TRUE;                 // stock objects are not ours to free
	delete obj;
	return TRUE;
}

// ---- Drawing --------------------------------------------------------------

int FillRect(HDC hdc, const RECT *r, HBRUSH brush)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->ctx || !r || !brush)
		return 0;
	auto *br = static_cast<gdi_brush *>(brush);
	if (br->kind != OBJ_BRUSH || br->none)
		return 0;
	set_fill(dc, br->color);
	cairo_rectangle(dc->ctx, double(r->left), double(r->top),
	                double(r->right - r->left), double(r->bottom - r->top));
	cairo_fill(dc->ctx);
	return 1;
}

BOOL RoundRect(HDC hdc, int left, int top, int right, int bottom, int ew, int eh)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->ctx)
		return FALSE;

	path_round_rect(dc->ctx, double(left), double(top), double(right), double(bottom),
	                double(ew), double(eh));
	fill_path(dc);
	stroke_path(dc);
	return TRUE;
}

BOOL Ellipse(HDC hdc, int left, int top, int right, int bottom)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->ctx)
		return FALSE;
	path_ellipse(dc->ctx, double(left), double(top), double(right), double(bottom));
	fill_path(dc);
	stroke_path(dc);
	return TRUE;
}

BOOL Arc(HDC hdc, int left, int top, int right, int bottom,
         int xr1, int yr1, int xr2, int yr2)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->ctx)
		return FALSE;

	const double cx = (left + right) / 2.0, cy = (top + bottom) / 2.0;
	const double rx = std::abs(right - left) / 2.0, ry = std::abs(bottom - top) / 2.0;
	if (rx <= 0.0 || ry <= 0.0)
		return FALSE;

	// GDI's Arc runs counterclockwise from the start point to the end point of
	// the inscribed ellipse. Angles are taken the mathematical way up (y
	// increasing upward), which is what makes "counterclockwise" mean what it
	// says; the points themselves are still in GDI's y-down space. (Same math
	// as the macOS half, which flattens for the same reason: sampling the
	// angles in y-down space is unambiguous.)
	const double pi = 3.14159265358979323846;
	auto angle_of = [&](int x, int y) {
		return std::atan2((cy - y) / ry, (x - cx) / rx);
	};
	const double a0 = angle_of(xr1, yr1);
	double a1 = angle_of(xr2, yr2);
	while (a1 <= a0 + 1e-9)
		a1 += 2.0 * pi;

	const double sweep = a1 - a0;
	const int steps = std::max(8, int(std::max(rx, ry) * sweep / 2.0) + 1);

	for (int i = 0; i <= steps; i++) {
		const double a = a0 + sweep * i / steps;
		const double px = cx + std::cos(a) * rx;
		const double py = cy - std::sin(a) * ry;
		if (i == 0)
			cairo_move_to(dc->ctx, px, py);
		else
			cairo_line_to(dc->ctx, px, py);
	}
	stroke_path(dc);
	return TRUE;
}

BOOL MoveToEx(HDC hdc, int x, int y, POINT *prev)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc)
		return FALSE;
	if (prev)
		*prev = dc->cur;
	dc->cur.x = x;
	dc->cur.y = y;
	return TRUE;
}

BOOL LineTo(HDC hdc, int x, int y)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->ctx)
		return FALSE;

	cairo_move_to(dc->ctx, double(dc->cur.x), double(dc->cur.y));
	cairo_line_to(dc->ctx, double(x), double(y));
	stroke_path(dc);

	dc->cur.x = x;
	dc->cur.y = y;
	return TRUE;
}

BOOL Polygon(HDC hdc, const POINT *pts, int n)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->ctx || !pts || n < 2)
		return FALSE;

	cairo_move_to(dc->ctx, double(pts[0].x), double(pts[0].y));
	for (int i = 1; i < n; i++)
		cairo_line_to(dc->ctx, double(pts[i].x), double(pts[i].y));
	cairo_close_path(dc->ctx);

	fill_path(dc);
	stroke_path(dc);
	return TRUE;
}

BOOL PolyPolygon(HDC hdc, const POINT *pts, const INT *counts, int n)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->ctx || !pts || !counts || n < 1)
		return FALSE;

	int at = 0;
	for (int s = 0; s < n; s++) {
		const int c = counts[s];
		if (c >= 2) {
			cairo_move_to(dc->ctx, double(pts[at].x), double(pts[at].y));
			for (int i = 1; i < c; i++)
				cairo_line_to(dc->ctx, double(pts[at + i].x), double(pts[at + i].y));
			cairo_close_path(dc->ctx);
		}
		at += c;
	}
	fill_path(dc);
	stroke_path(dc);
	return TRUE;
}

BOOL Polyline(HDC hdc, const POINT *pts, int n)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->ctx || !pts || n < 2)
		return FALSE;

	cairo_move_to(dc->ctx, double(pts[0].x), double(pts[0].y));
	for (int i = 1; i < n; i++)
		cairo_line_to(dc->ctx, double(pts[i].x), double(pts[i].y));
	stroke_path(dc);
	return TRUE;
}

// ---- State ----------------------------------------------------------------

COLORREF SetTextColor(HDC hdc, COLORREF color)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc)
		return 0;
	const COLORREF old = dc->text;
	dc->text = color;
	return old;
}

int SetBkMode(HDC hdc, int mode)
{
	(void)hdc; (void)mode;   // text here is always transparent; see below
	return TRANSPARENT;
}

int SetBkColor(HDC hdc, COLORREF color)
{
	(void)hdc; (void)color;
	return 0;
}

int SetPolyFillMode(HDC hdc, int mode)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc)
		return ALTERNATE;
	const int old = dc->fill_mode;
	dc->fill_mode = mode;
	return old;
}

int DrawTextW(HDC hdc, const wchar_t *text, int count, RECT *r, UINT flags)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->ctx || !text || !r)
		return 0;
	if (count < 0)
		count = int(std::wcslen(text));
	if (count <= 0)
		return 0;

	// wchar_t here is UTF-32, so re-encode to the UTF-16 the wrapping code
	// shares with the macOS half.
	std::u16string u16;
	u16.reserve(size_t(count));
	for (int i = 0; i < count; i++) {
		char32_t cp = char32_t(text[i]);
		if (cp < 0x10000) {
			u16.push_back(char16_t(cp));
		} else {
			cp -= 0x10000;
			u16.push_back(char16_t(0xD800 + (cp >> 10)));
			u16.push_back(char16_t(0xDC00 + (cp & 0x3FF)));
		}
	}

	gdi_font *font = (dc->font && dc->font->kind == OBJ_FONT)
	                     ? static_cast<gdi_font *>(dc->font)
	                     : nullptr;
	apply_font(dc->ctx, font);
	cairo_font_extents_t fe{};
	cairo_font_extents(dc->ctx, &fe);
	const double line_h = fe.ascent + fe.descent;

	const double rect_w = double(r->right - r->left);
	const double rect_h = double(r->bottom - r->top);

	const bool single = (flags & DT_SINGLELINE) != 0;
	std::vector<std::u16string> lines =
	    (single || !(flags & DT_WORDBREAK))
	        ? split_hard(u16)
	        : wrap_text(u16, font, dc->ctx, rect_w);
	if (lines.empty())
		lines.emplace_back();

	const double block_h = line_h * double(lines.size());

	double y;
	if (flags & DT_VCENTER)
		y = r->top + (rect_h - block_h) / 2.0;
	else if (flags & DT_BOTTOM)
		y = r->bottom - block_h;
	else
		y = r->top;

	cairo_save(dc->ctx);
	if (!(flags & DT_NOCLIP)) {
		cairo_rectangle(dc->ctx, double(r->left), double(r->top), rect_w, rect_h);
		cairo_clip(dc->ctx);
	}
	set_stroke(dc, dc->text);   // the glyph colour

	for (size_t i = 0; i < lines.size(); i++) {
		const double w = measure(font, dc->ctx, lines[i]);
		double x;
		if (flags & DT_CENTER)
			x = r->left + (rect_w - w) / 2.0;
		else if (flags & DT_RIGHT)
			x = r->right - w;
		else
			x = r->left;

		// The image surface is y-down natively, so stand on the baseline and
		// draw -- no axis flip like the macOS half needs.
		const std::string u8 = to_utf8(lines[i]);
		cairo_move_to(dc->ctx, x, y + fe.ascent + line_h * double(i));
		cairo_show_text(dc->ctx, u8.c_str());
	}
	cairo_restore(dc->ctx);

	return int(block_h);
}

int MultiByteToWideChar(UINT codepage, DWORD flags, const char *src, int src_len,
                        wchar_t *dst, int dst_len)
{
	(void)flags;
	if (!src)
		return 0;

	const bool nul_terminated = (src_len < 0);
	const auto *p = reinterpret_cast<const unsigned char *>(src);

	std::vector<char32_t> out;
	for (size_t i = 0; nul_terminated ? p[i] != 0 : int(i) < src_len; i++) {
		char32_t cp;
		if (codepage == CP_UTF8) {
			const unsigned char c = p[i];
			int extra = 0;
			if (c < 0x80)      { cp = c; }
			else if (c < 0xE0) { cp = c & 0x1F; extra = 1; }
			else if (c < 0xF0) { cp = c & 0x0F; extra = 2; }
			else               { cp = c & 0x07; extra = 3; }
			bool ok = true;
			for (int k = 0; k < extra; k++) {
				const unsigned char n = p[i + 1 + size_t(k)];
				if ((n & 0xC0) != 0x80) { ok = false; break; }
				cp = (cp << 6) | (n & 0x3F);
			}
			if (!ok) { cp = 0xFFFD; extra = 0; }
			i += size_t(extra);
		} else {
			cp = p[i];                      // treat anything else as Latin-1
		}
		out.push_back(cp);
	}
	if (nul_terminated)
		out.push_back(0);

	if (!dst)
		return int(out.size());
	if (int(out.size()) > dst_len)
		return 0;
	for (size_t i = 0; i < out.size(); i++)
		dst[i] = wchar_t(out[i]);
	return int(out.size());
}

// ---- Surfaces with no window -----------------------------------------------

HDC CreateCompatibleDC(HDC like)
{
	(void)like;
	auto *dc = new gdi_dc();
	dc->pen   = GetStockObject(BLACK_PEN);      // GDI's initial DC state
	dc->brush = GetStockObject(WHITE_BRUSH);
	g_dcs.push_back(dc);
	return dc;
}

HBITMAP CreateDIBSection(HDC hdc, const BITMAPINFO *info, UINT usage,
                         void **bits, void *section, DWORD offset)
{
	(void)hdc; (void)usage; (void)section; (void)offset;
	if (!info)
		return nullptr;

	const int w = int(info->bmiHeader.biWidth);
	const int h = std::abs(int(info->bmiHeader.biHeight));
	if (w <= 0 || h <= 0)
		return nullptr;

	auto *bm = new gdi_bitmap();
	bm->w = w;
	bm->h = h;
	bm->data.assign(size_t(w) * size_t(h) * 4, 0);

	// ARGB32 on a little-endian machine comes down as B, G, R, X per pixel:
	// the order GDI uses for a 32-bit BI_RGB DIB and the order ui::write_png
	// expects. The unused byte is never read as alpha.
	// An image surface is y-down from the start, so unlike the macOS half
	// there is nothing to flip for GDI coordinates.
	bm->surf = cairo_image_surface_create_for_data(
	    bm->data.data(), CAIRO_FORMAT_ARGB32, w, h, w * 4);
	if (cairo_surface_status(bm->surf) != CAIRO_STATUS_SUCCESS) {
		delete bm;
		return nullptr;
	}
	bm->ctx = cairo_create(bm->surf);
	if (cairo_status(bm->ctx) != CAIRO_STATUS_SUCCESS) {
		delete bm;
		return nullptr;
	}

	if (bits)
		*bits = bm->data.data();
	return bm;
}

void GdiFlush(void)
{
	for (gdi_dc *dc : g_dcs)
		if (dc->surf)
			cairo_surface_flush(dc->surf);
}

BOOL DeleteDC(HDC hdc)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc)
		return FALSE;
	g_dcs.erase(std::remove(g_dcs.begin(), g_dcs.end(), dc), g_dcs.end());
	delete dc;
	return TRUE;
}

void *smu_gdi_wrap_view_context(void *native, int w, int h)
{
	// No AppKit context exists on Linux. Hand back a memory DC of the same
	// size: the SDL window paints into its own image surface and uploads it,
	// so this only serves the headless plug-in view, which never paints
	// (plug_window_linux.cpp).
	(void)native;
	auto *dc = new gdi_dc();
	dc->pen   = GetStockObject(BLACK_PEN);
	dc->brush = GetStockObject(WHITE_BRUSH);
	dc->w = w;
	dc->h = h;
	g_dcs.push_back(dc);
	return dc;
}
