// license:BSD-3-Clause
//
// The Emscripten half of compat/gdi.h: the same slice of GDI the panel draws
// through, implemented as a plain-CPU rasterizer over an in-memory DIB. No
// Cairo, no FreeType, no window system -- just a byte buffer that the wasm
// window pump (ui/window_wasm.cpp) hands to the page as an ImageData, plus
// glyphs from the vendored stb_truetype.
//
// Coordinates and colours follow GDI, like the other two halves:
//
//   - everything draws y-down from the top-left;
//   - a filled rectangle covers [left, right) x [top, bottom);
//   - colour stays a COLORREF (red in the low byte) end to end. A pixel of
//     the DIB is the COLORREF value stored as a little-endian u32 with the
//     top byte 0xFF, so the memory bytes come out [R, G, B, A]. That is a
//     byte for byte what browser ImageData and a RGBA8 WebGL upload want,
//     and the pump can memcpy the DIB straight through. (The Cairo half
//     lands on B, G, R, X instead -- same u32 value, opposite byte order.
//     Nothing below this comment looks at raw bytes except through the one
//     pixel-blend helper.)
//
// Geometry is scanline-rasterised by hand. Each fill -- a polygon, a stroke
// turned into quads, a glyph stamp aside -- is one path of sub-polygons
// rasterised with GDI's two fill rules: ALTERNATE (even-odd, which the SVG
// art depends on) and WINDING. Antialiasing is 4 sub-scanlines per pixel
// row with exact fractional coverage along x: the Cairo port gets away with
// 11x17 sub-pixels, and this is smoother than jagged while staying cheap
// enough to run in a browser tab. Strokes are the winding union of one quad
// per segment plus a small disc at each join, so overlapping parts blend
// once instead of double-darkening.
//
// Text goes through stb_truetype (third_party/imgui/imstb_truetype.h), which
// the ImGui draw code also compiles. To keep the two out of each other's
// way the header is included here with STBTT_STATIC: this file's copy of the
// rasteriser has internal linkage and does not collide with the extern one
// in imgui_draw.cpp. Font *files* are fetched once at run time from
// "/fonts" (the --preload-file mount): NotoSansJP-VF first because the panel
// carries Japanese strings, then NotoSans, then whatever else is there. A
// wasm wchar_t is UTF-16, so DrawTextW decodes UTF-16 with surrogate pairs
// straight into codepoints; the wrapping rules are the same tokeniser the
// Linux and macOS halves use, so line breaks agree across the ports.
//
// Pixels will not be byte-identical to the Windows, macOS, or Linux --shot
// images. What must agree: the palette, the geometry, the SVG art, and DIB
// versus canvas at zero differing bytes in the page.

#include "compat/gdi.h"

#if defined(__EMSCRIPTEN__)

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <cwchar>
#include <fstream>
#include <iterator>
#include <string>
#include <vector>

#include <dirent.h>

// ---- stb_truetype, internal to this file ----------------------------------
//
// STBTT_STATIC makes every stbtt_* function static, so this TU's copy cannot
// collide with the extern implementation imgui_draw.cpp compiles from the
// same header. Unused statics are normal for a single-file library -- the
// panel uses perhaps a dozen of them -- so -Wunused-function is quiet here.

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunused-function"
#define STBTT_STATIC
#define STB_TRUETYPE_IMPLEMENTATION
#include "imstb_truetype.h"
#pragma clang diagnostic pop

// The wchar_t handed to DrawTextW is a UTF-16 *code unit* sequence, one unit
// per element: the same units this file's MultiByteToWideChar produces, and
// the same ones Windows would deliver. wchar_t's width does not matter --
// the current wasm ABI makes it 4 bytes where the old emscripten one was 2
// -- because every element holds one code unit and surrogate pairs are two
// elements either way.

namespace {

// ---- Object kinds ----------------------------------------------------------

enum obj_kind { OBJ_BRUSH, OBJ_PEN, OBJ_FONT, OBJ_BITMAP, OBJ_DC };

} // namespace

// ---- The objects an HGDIOBJ can point at ---------------------------------
//
// Declared in gdi.h and defined only here, so nothing outside this file can
// do anything with a handle except pass it back in. Same shape as the macOS
// and Linux halves.

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

	// Resolved at CreateFontA time against the one cached font file. When no
	// font file was found -- the page forgot --preload-file /fonts -- these
	// say so and DrawTextW quietly draws nothing.
	bool  have  = false;
	float scale = 0.f;           // stbtt_ScaleForPixelHeight
	int   asc   = 0;             // scaled vmetrics, ascender in pixels
	int   desc  = 0;             // ... descender, below the baseline
	float px    = 12.f;

	gdi_font() : gdi_object(OBJ_FONT) {}
};

struct gdi_bitmap : gdi_object {
	int               w = 0, h = 0;
	std::vector<BYTE> data;      // 4 bytes per pixel, [R, G, B, A]

	gdi_bitmap() : gdi_object(OBJ_BITMAP) {}
};

struct gdi_dc : gdi_object {
	gdi_bitmap *target = nullptr;  // the selected DIB, or nothing to draw on

	HGDIOBJ  pen = nullptr, brush = nullptr, font = nullptr;
	COLORREF text = RGB(0, 0, 0);
	COLORREF bk   = RGB(255, 255, 255);
	int      bk_mode   = TRANSPARENT;  // see SetBkMode below
	int      fill_mode = ALTERNATE;
	POINT    cur{ 0, 0 };

	gdi_dc() : gdi_object(OBJ_DC) {}
};

namespace {

// Every live DC, so DeleteObject of a bitmap can unselect itself from any DC
// still holding it. Drawing is single-threaded; there is no second thread in
// a wasm tab that could race this.
std::vector<gdi_dc *> g_dcs;

// ---- Colour helpers --------------------------------------------------------
//
// A COLORREF already *is* the little-endian u32 we store (red in bits 0-7),
// so drawing a pixel means storing `0xff000000 | colour`. These three pull
// the channels back out for the blending maths.

inline BYTE cr_r(COLORREF c) { return BYTE(c & 0xff); }
inline BYTE cr_g(COLORREF c) { return BYTE((c >> 8) & 0xff); }
inline BYTE cr_b(COLORREF c) { return BYTE((c >> 16) & 0xff); }

// Blend `cov` of an opaque source colour over the destination pixel, keeping
// straight (non-premultiplied) alpha: out.a = cov + dst.a*(1-cov), and the
// colour of what remains of the destination contributes the rest. Pixels
// nobody ever drew keep whatever alpha the DIB came with (0 from calloc),
// which is what lets the pump show them as transparent.
inline void blend_px(BYTE *p, BYTE r, BYTE g, BYTE b, float cov)
{
	if (cov <= 0.f)
		return;
	if (cov > 1.f)
		cov = 1.f;

	const float sa = cov;
	const float da = float(p[3]) / 255.f;
	const float oa = sa + da * (1.f - sa);
	if (oa <= 0.f) {
		p[0] = p[1] = p[2] = p[3] = 0;
		return;
	}
	const float k = (1.f - sa) * da;
	p[0] = BYTE((float(r) * sa + float(p[0]) * k) / oa + 0.5f);
	p[1] = BYTE((float(g) * sa + float(p[1]) * k) / oa + 0.5f);
	p[2] = BYTE((float(b) * sa + float(p[2]) * k) / oa + 0.5f);
	p[3] = BYTE(oa * 255.f + 0.5f);
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

gdi_font *font_of(gdi_dc *dc)
{
	return (dc->font && dc->font->kind == OBJ_FONT)
	           ? static_cast<gdi_font *>(dc->font)
	           : nullptr;
}

// ---- The scanline rasteriser -----------------------------------------------

struct pt2 { double x, y; };

// One non-horizontal edge, normalised so y0 < y1. `dir` is +1 for an edge
// that ran downward in its original winding, which is what the WINDING rule
// counts.
struct edge {
	double x0, y0, x1, y1;
	int    dir;
};

// A path: sub-polygons flattened to edges, plus their bounding box. Fills
// are one path drawn with the caller's fill rule, exactly like cairo's
// fill_preserve over a multi-subpath SVG `d`.
struct region {
	std::vector<edge> edges;
	double x_lo = 1e30, y_lo = 1e30, x_hi = -1e30, y_hi = -1e30;

	// Append a closed sub-polygon. force_sign, when nonzero, demands that
	// the vertices run with that shoelace sign (in y-down space, -1 for the
	// quads add_stroke builds); reversing one sub-polygon of a winding
	// *union* would punch a hole in it, and the stroke quads and join discs
	// must all run the same way for the union to work.
	void add_poly(const pt2 *p, int n, int force_sign)
	{
		if (n < 3)
			return;                       // zero area, nothing to cover

		double sl = 0.0;
		for (int i = 0; i < n; i++) {
			const pt2 &a = p[i];
			const pt2 &b = p[(i + 1) % n];
			sl += a.x * b.y - b.x * a.y;
		}
		if (sl == 0.0)
			return;
		const bool flip = force_sign != 0 && ((sl < 0.0) != (force_sign < 0));

		for (int i = 0; i < n; i++) {
			const int j = (i + 1) % n;
			add_edge(p[flip ? n - 1 - i : i], p[flip ? n - 1 - j : j]);
		}
	}

	void add_edge(const pt2 &a, const pt2 &b)
	{
		if (a.y == b.y)
			return;                       // horizontal: never crossed
		edge e;
		e.dir = (a.y < b.y) ? 1 : -1;
		e.x0 = a.x; e.y0 = a.y; e.x1 = b.x; e.y1 = b.y;
		if (e.y0 > e.y1) {
			std::swap(e.x0, e.x1);
			std::swap(e.y0, e.y1);
		}
		edges.push_back(e);
		x_lo = std::min({ x_lo, a.x, b.x });
		x_hi = std::max({ x_hi, a.x, b.x });
		y_lo = std::min(y_lo, std::min(a.y, b.y));
		y_hi = std::max(y_hi, std::max(a.y, b.y));
	}
};

// Append the stroked outline of a polyline as a winding union: one quad per
// segment (butt caps, like GDI and the cairo port) and a small disc at each
// join so a 1-2 pixel needle does not notch at its turns. GDI mitres here;
// at these pen widths the two are indistinguishable once antialiased.
void add_stroke(region &rg, const pt2 *p, int n, bool closed, double width)
{
	if (n < 2)
		return;
	if (width < 1.0)
		width = 1.0;
	const double h = width / 2.0;
	const int segs = closed ? n : n - 1;

	for (int i = 0; i < segs; i++) {
		const pt2 &a = p[i];
		const pt2 &b = p[(i + 1) % n];
		const double dx = b.x - a.x, dy = b.y - a.y;
		const double len = std::hypot(dx, dy);
		if (len < 1e-9)
			continue;
		const double nx = -dy / len, ny = dx / len;
		const pt2 q[4] = {
			{ a.x - h * nx, a.y - h * ny },
			{ a.x + h * nx, a.y + h * ny },
			{ b.x + h * nx, b.y + h * ny },
			{ b.x - h * nx, b.y - h * ny },
		};
		rg.add_poly(q, 4, -1);
	}

	// Joins: every vertex of a closed path, the interior ones of an open
	// path. The disc is a regular 16-gon; its orientation is forced to
	// match the quads' so the union counts +1 wherever either covers.
	if (closed || n > 2) {
		const int steps = 16;
		std::vector<pt2> disc;
		disc.reserve(size_t(steps));
		for (int i = 0; i < n; i++) {
			if (!closed && (i == 0 || i == n - 1))
				continue;
			for (int k = 0; k < steps; k++) {
				const double a = 2.0 * 3.14159265358979323846 * k / steps;
				disc.push_back({ p[i].x + h * std::cos(a),
				                 p[i].y + h * std::sin(a) });
			}
			rg.add_poly(disc.data() + disc.size() - size_t(steps), steps, -1);
		}
	}
}

enum fill_rule { RULE_EVEN_ODD = 0, RULE_WINDING = 1 };

// Rasterise one region onto the bitmap's pixels. The AA method: four
// sub-scanlines per pixel row (sampled at their centres, edges half-open so
// shared vertices are counted once), and along x each span contributes its
// exact fractional overlap with every pixel column it touches. Sum the four
// sub-rows, divide by four -- one coverage value per pixel -- and blend.
void raster_fill(gdi_bitmap *bm, const region &rg, int rule, COLORREF color,
                 const RECT *clip)
{
	if (!bm || rg.edges.empty())
		return;

	const int cx_lo = clip ? std::max(0, int(clip->left)) : 0;
	const int cx_hi = clip ? std::min(bm->w, int(clip->right)) : bm->w;
	const int cy_lo = clip ? std::max(0, int(clip->top)) : 0;
	const int cy_hi = clip ? std::min(bm->h, int(clip->bottom)) : bm->h;
	if (cx_lo >= cx_hi || cy_lo >= cy_hi)
		return;

	int y0 = std::max(cy_lo, int(std::floor(rg.y_lo)));
	int y1 = std::min(cy_hi, int(std::ceil(rg.y_hi)));
	if (y1 <= y0)
		return;

	const BYTE sr = cr_r(color), sg = cr_g(color), sb = cr_b(color);

	std::vector<float> cov;
	cov.resize(size_t(cx_hi - cx_lo));
	std::vector<std::pair<double, int>> xs;
	xs.reserve(64);

	for (int row = y0; row < y1; row++) {
		std::fill(cov.begin(), cov.end(), 0.f);

		for (int sub = 0; sub < 4; sub++) {
			const double yc = double(row) + (double(sub) + 0.5) * 0.25;
			xs.clear();
			for (const edge &e : rg.edges) {
				// half-open in y: [e.y0, e.y1)
				if (yc < e.y0 || yc >= e.y1)
					continue;
				const double t = (yc - e.y0) / (e.y1 - e.y0);
				xs.emplace_back(e.x0 + t * (e.x1 - e.x0), e.dir);
			}
			if (xs.size() < 2)
				continue;
			std::sort(xs.begin(), xs.end(),
			          [](const std::pair<double, int> &a,
			             const std::pair<double, int> &b) {
				          return a.first < b.first;
			          });

			// Sweep the crossings and collect inside spans, under whichever
			// rule GDI asked for. ALTERNATE is the even-odd parity; WINDING
			// sums the edge directions and calls anything nonzero inside.
			int    wn = 0;
			bool   ins = false;
			double span0 = 0.0;
			for (const auto &x : xs) {
				const bool was = ins;
				if (rule == RULE_EVEN_ODD)
					ins = !ins;
				else {
					wn += x.second;
					ins = wn != 0;
				}
				if (ins && !was) {
					span0 = x.first;
				} else if (!ins && was) {
					const double xa = std::max(span0, double(cx_lo));
					const double xb = std::min(x.first, double(cx_hi));
					if (xb > xa) {
						int ca = int(std::floor(xa));
						int cb = int(std::ceil(xb));
						if (cb > cx_hi)
							cb = cx_hi;
						for (int c = ca; c < cb; c++)
							cov[size_t(c - cx_lo)] +=
							     std::min(xb, double(c + 1))
							     - std::max(xa, double(c));
					}
				}
			}
		}

		BYTE *row_px = bm->data.data() + size_t(row) * size_t(bm->w) * 4;
		for (size_t c = 0; c < cov.size(); c++) {
			const float v = cov[c] * 0.25f;
			if (v > 0.f)
				blend_px(row_px + (cx_lo + int(c)) * 4, sr, sg, sb, v);
		}
	}
}

// Fill + stroke like cairo's fill_preserve-then-stroke: the brush first
// (NULL_BRUSH paints nothing), then the pen (NULL_PEN strokes nothing).
// GDI itself uses NULL_BRUSH/NULL_PEN as the "skip this stage" marker, which
// is what panel.cpp and svg.cpp select around their calls.
void paint_closed(gdi_dc *dc, const pt2 *pts, int n, int rule)
{
	gdi_bitmap *bm = dc->target;
	if (!bm || n < 2)
		return;

	if (gdi_brush *br = brush_of(dc)) {
		if (!br->none && n >= 3) {
			region rg;
			rg.add_poly(pts, n, 0);       // user order: winding respects it
			raster_fill(bm, rg, rule, br->color, nullptr);
		}
	}
	if (gdi_pen *pen = pen_of(dc)) {
		if (!pen->none) {
			region rg;
			add_stroke(rg, pts, n, true, std::max(1, pen->width));
			raster_fill(bm, rg, RULE_WINDING, pen->color, nullptr);
		}
	}
}

void paint_open(gdi_dc *dc, const pt2 *pts, int n)
{
	gdi_bitmap *bm = dc->target;
	if (!bm || n < 2)
		return;
	if (gdi_pen *pen = pen_of(dc)) {
		if (!pen->none) {
			region rg;
			add_stroke(rg, pts, n, false, std::max(1, pen->width));
			raster_fill(bm, rg, RULE_WINDING, pen->color, nullptr);
		}
	}
}

// Stroke a closed loop without filling it: what PolyPolygon does per
// sub-path after the combined fill pass.
void stroke_closed(gdi_dc *dc, const pt2 *pts, int n)
{
	gdi_bitmap *bm = dc->target;
	if (!bm || n < 3)
		return;
	if (gdi_pen *pen = pen_of(dc)) {
		if (!pen->none) {
			region rg;
			add_stroke(rg, pts, n, true, std::max(1, pen->width));
			raster_fill(bm, rg, RULE_WINDING, pen->color, nullptr);
		}
	}
}

std::vector<pt2> to_pts(const POINT *pts, int n)
{
	std::vector<pt2> out;
	out.resize(size_t(n));
	for (int i = 0; i < n; i++)
		out[size_t(i)] = { double(pts[i].x), double(pts[i].y) };
	return out;
}

// The rounded-rect corner loop. GDI gives the corner ellipse's width and
// height; the radius is the smaller of the two halves, like the other ports.
std::vector<pt2> round_rect_pts(double left, double top, double right,
                                double bottom, double ew, double eh)
{
	const double rx = std::abs(ew) / 2.0, ry = std::abs(eh) / 2.0;
	const double r = std::max(0.0, std::min(rx, ry));
	if (r <= 0.0)
		return { { left, top }, { right, top }, { right, bottom }, { left, bottom } };

	std::vector<pt2> p;
	const double pi = 3.14159265358979323846;
	auto corner = [&](double cx, double cy, double a0) {
		const int steps = 8;
		for (int i = 0; i <= steps; i++) {
			const double a = a0 + (pi / 2.0) * i / steps;
			p.push_back({ cx + std::cos(a) * r, cy + std::sin(a) * r });
		}
	};
	corner(right - r, top + r, -pi / 2.0);      // top-right
	corner(right - r, bottom - r, 0.0);         // bottom-right
	corner(left + r, bottom - r, pi / 2.0);     // bottom-left
	corner(left + r, top + r, pi);              // top-left
	return p;
}

// An ellipse sampled into a closed loop, like the cairo port flattens its
// arc -- uniform stroke width comes free when nothing is scaled.
std::vector<pt2> ellipse_pts(double left, double top, double right, double bottom)
{
	const double cx = (left + right) / 2.0, cy = (top + bottom) / 2.0;
	const double rx = std::abs(right - left) / 2.0, ry = std::abs(bottom - top) / 2.0;
	if (rx <= 0.0 || ry <= 0.0)
		return {};
	const double pi = 3.14159265358979323846;
	const int steps = std::max(16, int((rx + ry) * pi / 2.0) + 1);
	std::vector<pt2> p;
	p.reserve(size_t(steps) + 1);
	for (int i = 0; i <= steps; i++) {
		const double a = 2.0 * pi * i / steps;
		p.push_back({ cx + std::cos(a) * rx, cy + std::sin(a) * ry });
	}
	return p;
}

// The open arc between two points on the inscribed ellipse, counterclockwise
// in GDI's y-down space -- the same angle maths as gdi_linux.cpp. Like that
// half, Arc here strokes only: panel.cpp draws its fan wedges with a plain
// pen and no brush decision, and filling the chord would paint white.
std::vector<pt2> arc_pts(double left, double top, double right, double bottom,
                         int xr1, int yr1, int xr2, int yr2)
{
	const double cx = (left + right) / 2.0, cy = (top + bottom) / 2.0;
	const double rx = std::abs(right - left) / 2.0, ry = std::abs(bottom - top) / 2.0;
	if (rx <= 0.0 || ry <= 0.0)
		return {};

	const double pi = 3.14159265358979323846;
	auto angle_of = [&](int x, int y) {
		return std::atan2((cy - double(y)) / ry, (double(x) - cx) / rx);
	};
	const double a0 = angle_of(xr1, yr1);
	double a1 = angle_of(xr2, yr2);
	while (a1 <= a0 + 1e-9)
		a1 += 2.0 * pi;

	const double sweep = a1 - a0;
	const int steps = std::max(8, int(std::max(rx, ry) * sweep / 2.0) + 1);
	std::vector<pt2> p;
	p.reserve(size_t(steps) + 1);
	for (int i = 0; i <= steps; i++) {
		const double a = a0 + sweep * i / steps;
		p.push_back({ cx + std::cos(a) * rx, cy - std::sin(a) * ry });
	}
	return p;
}

// ---- Fonts -----------------------------------------------------------------
//
// One font file, loaded once, serving every HFONT. The wasm UI asks for
// "Segoe UI", which does not exist here; like the Linux half answering with
// fontconfig's sans-serif, the request is answered with whatever is mounted
// at /fonts. NotoSansJP first -- its coverage includes the Japanese strings
// the panel carries -- then NotoSans, then any other .ttf/.otf in the
// directory, in readdir's order.

struct face_cache {
	std::vector<unsigned char> bytes;
	stbtt_fontinfo             info{};
	bool                       tried = false;
	bool                       ok    = false;
};

face_cache &the_face()
{
	static face_cache fc;
	if (fc.tried)
		return fc;
	fc.tried = true;

	auto load = [&](const char *path) -> bool {
		std::ifstream in(path, std::ios::binary);
		if (!in)
			return false;
		std::vector<unsigned char> bytes((std::istreambuf_iterator<char>(in)),
		                                std::istreambuf_iterator<char>());
		if (bytes.size() < 16)
			return false;
		// GetFontOffsetForIndex handles a bare sfnt (offset 0) and a TTC
		// collection alike; InitFont then validates the tables.
		const int off = stbtt_GetFontOffsetForIndex(bytes.data(), 0);
		if (off < 0 || !stbtt_InitFont(&fc.info, bytes.data(), off))
			return false;
		fc.bytes.swap(bytes);
		fc.ok = true;
		return true;
	};

	if (!load("/fonts/NotoSansJP-VF.ttf") && !load("/fonts/NotoSans.ttf")) {
		// Any .ttf / .otf in /fonts, sorted so the pick does not depend on
		// the order the packer stored them in.
		std::vector<std::string> names;
		if (DIR *d = opendir("/fonts")) {
			while (struct dirent *de = readdir(d)) {
				std::string name = de->d_name;
				std::string lower;
				lower.reserve(name.size());
				for (char ch : name)
					lower.push_back((ch >= 'A' && ch <= 'Z') ? char(ch + 32) : ch);
				if (lower.size() > 4 &&
				    (lower.compare(lower.size() - 4, 4, ".ttf") == 0 ||
				     lower.compare(lower.size() - 4, 4, ".otf") == 0))
					names.push_back(std::move(name));
			}
			closedir(d);
		}
		std::sort(names.begin(), names.end());
		for (const std::string &n : names) {
			if (load(("/fonts/" + n).c_str()))
				break;
		}
	}
	return fc;
}

// Decode one UTF-16 code unit pair at s[i] into a codepoint. `step` says how
// many units it consumed (2 for a surrogate pair, else 1). Lone surrogates
// become U+FFFD rather than derailing the run.
char32_t decode16(const char16_t *s, size_t n, size_t i, size_t &step)
{
	const char16_t c = s[i];
	if (c >= 0xD800 && c <= 0xDBFF && i + 1 < n &&
	    s[i + 1] >= 0xDC00 && s[i + 1] <= 0xDFFF) {
		step = 2;
		return char32_t(0x10000) +
		       ((char32_t(c) - 0xD800) << 10) +
		       (char32_t(s[i + 1]) - 0xDC00);
	}
	step = 1;
	return (c >= 0xD800 && c <= 0xDFFF) ? char32_t(0xFFFD) : char32_t(c);
}

std::vector<char32_t> decode_u16(const std::u16string &s)
{
	std::vector<char32_t> out;
	out.reserve(s.size());
	for (size_t i = 0; i < s.size();) {
		size_t step = 1;
		out.push_back(decode16(reinterpret_cast<const char16_t *>(s.data()),
		                      s.size(), i, step));
		i += step;
	}
	return out;
}

std::u16string encode_u16(const std::vector<char32_t> &cps)
{
	std::u16string out;
	out.reserve(cps.size());
	for (char32_t cp : cps) {
		if (cp < 0x10000) {
			out.push_back(char16_t(cp));
		} else {
			const char32_t v = cp - 0x10000;
			out.push_back(char16_t(0xD800 + (v >> 10)));
			out.push_back(char16_t(0xDC00 + (v & 0x3FF)));
		}
	}
	return out;
}

// The advance of one codepoint, kern included, in device pixels. This is the
// only measuring there is -- GetCodepointHMetrics is cheap once stb has the
// font in memory, and the strings are panel labels, not paragraphs.
double cp_advance(const face_cache &fc, float scale, char32_t cp, char32_t next)
{
	int adv = 0, lsb = 0;
	stbtt_GetCodepointHMetrics(&fc.info, int(cp), &adv, &lsb);
	double w = double(adv) * scale;
	if (next != char32_t(-1))
		w += double(stbtt_GetCodepointKernAdvance(&fc.info, int(cp), int(next))) * scale;
	return w;
}

double measure_line(const face_cache &fc, float scale, const std::u16string &line)
{
	const std::vector<char32_t> cps = decode_u16(line);
	double w = 0.0;
	for (size_t i = 0; i < cps.size(); i++)
		w += cp_advance(fc, scale, cps[i],
		                i + 1 < cps.size() ? cps[i + 1] : char32_t(-1));
	return w;
}

// ---- Text wrapping ----------------------------------------------------------
//
// The same tokeniser the macOS and Linux halves carry, so a Japanese
// paragraph breaks between characters and an English one breaks on spaces in
// all three ports. Only the measuring numbers differ between them.

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

bool breakable(char32_t cp)
{
	return (cp >= 0x2E80 && cp <= 0x9FFF) ||      // CJK radicals through unified
	       (cp >= 0xAC00 && cp <= 0xD7AF) ||      // Hangul syllables
	       (cp >= 0xF900 && cp <= 0xFAFF) ||      // CJK compatibility
	       (cp >= 0xFF00 && cp <= 0xFF60);        // fullwidth forms
}

std::vector<std::u16string> tokenize(const std::u16string &s)
{
	std::vector<std::u16string> out;
	std::u16string word;
	auto flush = [&] { if (!word.empty()) { out.push_back(word); word.clear(); } };

	for (size_t i = 0; i < s.size();) {
		size_t step = 1;
		const char32_t cp = decode16(reinterpret_cast<const char16_t *>(s.data()),
		                            s.size(), i, step);

		if (s[i] == u' ' || s[i] == u'\t') {
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

std::vector<std::u16string> wrap_text(const face_cache &fc, float scale,
                                      const std::u16string &s, double max_w)
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
			if (measure_line(fc, scale, cand) <= max_w || line.empty()) {
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

// DT_END_ELLIPSIS, as far as panel code needs it: a single line that will
// not fit loses whole codepoints from the end until it does with a trailing
// horizontal-ellipsis character, which is GDI's behaviour down to the
// characters it drops.
std::u16string ellipsize(const face_cache &fc, float scale, const std::u16string &line,
                         double avail)
{
	if (avail <= 0.0 || measure_line(fc, scale, line) <= avail)
		return line;

	std::vector<char32_t> cps = decode_u16(line);
	const char32_t ell = 0x2026;   // '…'
	while (!cps.empty()) {
		cps.pop_back();
		std::vector<char32_t> trial = cps;
		trial.push_back(ell);
		if (measure_line(fc, scale, encode_u16(trial)) <= avail)
			return encode_u16(trial);
	}
	return encode_u16({ ell });
}

// Stamp one glyph's coverage bitmap onto the DIB at integer device offsets,
// honouring the pixel clip the caller established. `bold` double-stamps one
// pixel to the right -- synthetic emboldening, permitted to be an
// approximation because there is no second face file to pick.
void stamp_glyph(gdi_bitmap *bm, const unsigned char *gray, int gw, int gh,
                 int px0, int py0, COLORREF color, const RECT &clip, bool bold)
{
	const BYTE sr = cr_r(color), sg = cr_g(color), sb = cr_b(color);
	const int x_lo = std::max(0, px0), y_lo = std::max(0, py0);
	const int x_hi = std::min(bm->w, px0 + gw);
	const int y_hi = std::min(bm->h, py0 + gh);
	for (int y = y_lo; y < y_hi; y++) {
		if (y < clip.top || y >= clip.bottom)
			continue;
		const unsigned char *grow = gray + size_t(y - py0) * size_t(gw);
		BYTE *drow = bm->data.data() + size_t(y) * size_t(bm->w) * 4;
		for (int x = x_lo; x < x_hi; x++) {
			if (x < clip.left || x >= clip.right)
				continue;
			const unsigned char g = grow[x - px0];
			if (!g)
				continue;
			const float cov = float(g) / 255.f;
			blend_px(drow + x * 4, sr, sg, sb, cov);
			if (bold && x + 1 < clip.right && x + 1 < bm->w)
				blend_px(drow + (x + 1) * 4, sr, sg, sb, cov);
		}
	}
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
	// PS_DASH and PS_DOT are drawn solid, exactly like the cairo and macOS
	// halves: the panel only ever asks for PS_SOLID or a NULL pen, and a
	// dashed LCD needle has never been drawn on a real MU2000.
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
	f->px     = std::max(1.0f, float(std::abs(f->height)));

	face_cache &fc = the_face();
	if (fc.ok) {
		f->have  = true;
		f->scale = stbtt_ScaleForPixelHeight(&fc.info, f->px);
		int asc = 0, desc = 0, gap = 0;
		stbtt_GetFontVMetrics(&fc.info, &asc, &desc, &gap);
		f->asc  = int(std::lround(float(asc) * f->scale));
		f->desc = int(std::lround(float(desc) * f->scale));
	}
	// A weight of FW_BOLD or up gets the same face with the emboldening
	// stamp in DrawTextW; there is one file and the fonts under /fonts are
	// variable ones the page could not ask to instance anyway.
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
		// Binding a bitmap to a DC: this is the framebuf's sequence --
		// CreateCompatibleDC / CreateDIBSection / SelectObject -- and the
		// bitmap keeps ownership of the pixel bytes.
		auto *bm = static_cast<gdi_bitmap *>(obj);
		HGDIOBJ old = dc->target;
		dc->target = bm;
		return old;
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
	if (obj->kind == OBJ_BITMAP) {
		// The framebuf frees the bitmap before the DC; unhook every DC that
		// still draws through it so none keeps a dead target.
		for (gdi_dc *dc : g_dcs)
			if (dc->target == static_cast<gdi_bitmap *>(obj))
				dc->target = nullptr;
	}
	delete obj;
	return TRUE;
}

// ---- Drawing --------------------------------------------------------------

int FillRect(HDC hdc, const RECT *r, HBRUSH brush)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->target || !r || !brush)
		return 0;
	auto *br = static_cast<gdi_brush *>(brush);
	if (br->kind != OBJ_BRUSH || br->none)
		return 0;

	gdi_bitmap *bm = dc->target;
	const int x0 = std::max(0, int(r->left)),  x1 = std::min(bm->w, int(r->right));
	const int y0 = std::max(0, int(r->top)),   y1 = std::min(bm->h, int(r->bottom));
	if (x0 >= x1 || y0 >= y1)
		return 1;                    // GDI reports the fill it was asked for

	// The brush colour with the alpha byte set, stored as-is: on wasm that
	// puts [R, G, B, 255] at the pixel's four address bytes.
	const uint32_t v = 0xff000000u | (br->color & 0x00ffffffu);
	for (int y = y0; y < y1; y++) {
		BYTE *row = bm->data.data() + size_t(y) * size_t(bm->w) * 4;
		for (int x = x0; x < x1; x++)
			std::memcpy(row + size_t(x) * 4, &v, 4);
	}
	return 1;
}

BOOL RoundRect(HDC hdc, int left, int top, int right, int bottom, int ew, int eh)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->target)
		return FALSE;
	const std::vector<pt2> p = round_rect_pts(double(left), double(top),
	                                          double(right), double(bottom),
	                                          double(ew), double(eh));
	paint_closed(dc, p.data(), int(p.size()),
	             dc->fill_mode == WINDING ? RULE_WINDING : RULE_EVEN_ODD);
	return TRUE;
}

BOOL Ellipse(HDC hdc, int left, int top, int right, int bottom)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->target)
		return FALSE;
	const std::vector<pt2> p = ellipse_pts(double(left), double(top),
	                                       double(right), double(bottom));
	paint_closed(dc, p.data(), int(p.size()),
	             dc->fill_mode == WINDING ? RULE_WINDING : RULE_EVEN_ODD);
	return TRUE;
}

BOOL Arc(HDC hdc, int left, int top, int right, int bottom,
         int xr1, int yr1, int xr2, int yr2)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->target)
		return FALSE;
	// Stroke only, no chord fill -- the same reading gdi_linux.cpp made: the
	// panel's fan wedges must paint nothing between the arc and its chord.
	const std::vector<pt2> p = arc_pts(double(left), double(top),
	                                   double(right), double(bottom),
	                                   xr1, yr1, xr2, yr2);
	paint_open(dc, p.data(), int(p.size()));
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
	if (!dc || !dc->target)
		return FALSE;
	const pt2 p[2] = { { double(dc->cur.x), double(dc->cur.y) },
	                   { double(x), double(y) } };
	paint_open(dc, p, 2);
	dc->cur.x = x;
	dc->cur.y = y;
	return TRUE;
}

BOOL Polygon(HDC hdc, const POINT *pts, int n)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->target || !pts || n < 2)
		return FALSE;
	const std::vector<pt2> p = to_pts(pts, n);
	paint_closed(dc, p.data(), int(p.size()),
	             dc->fill_mode == WINDING ? RULE_WINDING : RULE_EVEN_ODD);
	return TRUE;
}

BOOL PolyPolygon(HDC hdc, const POINT *pts, const INT *counts, int n)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->target || !pts || !counts || n < 1)
		return FALSE;

	// One fill over every sub-polygon in one region: that is what lets the
	// even-odd parity (or the winding count) combine across sub-paths and
	// carve the holes svg.cpp's nested outlines form. A 2-point sub-path
	// has no area to fill but is stroked below, like cairo's two-point
	// closed path.
	if (gdi_brush *br = brush_of(dc)) {
		if (!br->none) {
			region rg;
			int at = 0;
			for (int s = 0; s < n; s++) {
				const int c = counts[s];
				if (c >= 3) {
					const std::vector<pt2> p = to_pts(pts + at, c);
					rg.add_poly(p.data(), int(p.size()), 0);
				}
				at += c;
			}
			raster_fill(dc->target, rg,
			            dc->fill_mode == WINDING ? RULE_WINDING : RULE_EVEN_ODD,
			            br->color, nullptr);
		}
	}

	int at = 0;
	for (int s = 0; s < n; s++) {
		const int c = counts[s];
		if (c >= 3) {
			const std::vector<pt2> p = to_pts(pts + at, c);
			stroke_closed(dc, p.data(), int(p.size()));
		} else if (c == 2) {
			const std::vector<pt2> p = to_pts(pts + at, c);
			paint_open(dc, p.data(), int(p.size()));
		}
		at += c;
	}
	return TRUE;
}

BOOL Polyline(HDC hdc, const POINT *pts, int n)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc || !dc->target || !pts || n < 2)
		return FALSE;
	const std::vector<pt2> p = to_pts(pts, n);
	paint_open(dc, p.data(), int(p.size()));
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
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc)
		return TRANSPARENT;
	const int old = dc->bk_mode;
	dc->bk_mode = mode;
	return old;
}

int SetBkColor(HDC hdc, COLORREF color)
{
	gdi_dc *dc = static_cast<gdi_dc *>(hdc);
	if (!dc)
		return 0;
	const COLORREF old = dc->bk;
	dc->bk = color;
	return int(old);
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
	if (!dc || !dc->target || !text || !r)
		return 0;
	if (count < 0)
		count = int(std::wcslen(text));
	if (count <= 0)
		return 0;

	gdi_font *font = font_of(dc);
	if (!font || !font->have)
		return 0;                    // no font file mounted: draw nothing

	// Each wchar_t holds one UTF-16 code unit (see the note above); narrow
	// them into char16_t and let decode_u16 stitch surrogate pairs back
	// into codepoints.
	std::u16string u16;
	u16.reserve(size_t(count));
	for (int i = 0; i < count; i++)
		u16.push_back(char16_t(uint16_t(text[i])));

	face_cache &fc = the_face();
	const float scale = font->scale;
	const double line_h = double(font->asc + (-font->desc));

	const double rect_w = double(r->right - r->left);
	const double rect_h = double(r->bottom - r->top);

	const bool single = (flags & DT_SINGLELINE) != 0;
	std::vector<std::u16string> lines =
	    (single || !(flags & DT_WORDBREAK))
	        ? split_hard(u16)
	        : wrap_text(fc, scale, u16, rect_w);
	if (lines.empty())
		lines.emplace_back();

	if (single && (flags & DT_END_ELLIPSIS))
		for (std::u16string &ln : lines)
			ln = ellipsize(fc, scale, ln, rect_w);

	const double block_h = line_h * double(lines.size());

	double y;
	if (flags & DT_VCENTER)
		y = r->top + (rect_h - block_h) / 2.0;
	else if (flags & DT_BOTTOM)
		y = r->bottom - block_h;
	else
		y = r->top;

	// What may be painted over: the rectangle clip GDI applies to text, or
	// just the bitmap when DT_NOCLIP was asked for. Kept as plain ints so
	// the maths does not mix LONG with int.
	const gdi_bitmap *bm = dc->target;
	int cx0 = 0, cy0 = 0, cx1 = bm->w, cy1 = bm->h;
	if (!(flags & DT_NOCLIP)) {
		cx0 = std::max(cx0, int(r->left));
		cy0 = std::max(cy0, int(r->top));
		cx1 = std::min(cx1, int(r->right));
		cy1 = std::min(cy1, int(r->bottom));
	}
	RECT clip;
	SetRect(&clip, cx0, cy0, cx1, cy1);

	// OPAQUE fills the whole rectangle first, like GDI lays a strip of
	// background tape under the run. The default is TRANSPARENT here, as in
	// the cairo half, because every caller in panel and editor code sets
	// it; a caller that *wants* tape says SetBkMode(OPAQUE) and gets it.
	if (dc->bk_mode == OPAQUE && clip.left < clip.right && clip.top < clip.bottom) {
		gdi_brush tape;
		tape.color = dc->bk;
		FillRect(hdc, &clip, &tape);
	}

	for (size_t i = 0; i < lines.size(); i++) {
		const double w = measure_line(fc, scale, lines[i]);
		double x;
		if (flags & DT_CENTER)
			x = r->left + (rect_w - w) / 2.0;
		else if (flags & DT_RIGHT)
			x = r->right - w;
		else
			x = r->left;

		const double baseline = double(y) + double(font->asc) + line_h * double(i);

		const std::vector<char32_t> cps = decode_u16(lines[i]);
		for (size_t k = 0; k < cps.size(); k++) {
			const char32_t cp = cps[k];
			int gw = 0, gh = 0, xoff = 0, yoff = 0;
			unsigned char *gray = stbtt_GetCodepointBitmap(
			    &fc.info, scale, scale, int(cp), &gw, &gh, &xoff, &yoff);
			if (gray) {
				if (gw > 0 && gh > 0) {
					const int px0 = int(std::lround(x)) + xoff;
					const int py0 = int(std::lround(baseline)) + yoff;
					stamp_glyph(dc->target, gray, gw, gh, px0, py0, dc->text,
					            clip, font->weight >= FW_BOLD);
				}
				stbtt_FreeBitmap(gray, nullptr);
			}
			x += cp_advance(fc, scale, cp,
			                k + 1 < cps.size() ? cps[k + 1] : char32_t(-1));
		}
	}

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

	std::vector<char32_t> cps;
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
		cps.push_back(cp);
	}
	if (nul_terminated)
		cps.push_back(0);

	// UTF-16 units, not codepoints: a surrogate pair needs two wchar_t.
	const std::u16string u16 = encode_u16(cps);
	if (!dst)
		return int(u16.size());
	if (int(u16.size()) > dst_len)
		return 0;
	for (size_t i = 0; i < u16.size(); i++)
		dst[i] = wchar_t(u16[i]);
	return int(u16.size());
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
	// Negative height asks for top-down storage, which is how this buffer is
	// always laid out; positive (bottom-up, the old GDI default) is read by
	// its absolute size and stored top-down too -- the cairo port does the
	// same, and no caller here asks for bottom-up.
	const int h = std::abs(int(info->bmiHeader.biHeight));
	if (w <= 0 || h <= 0)
		return nullptr;

	auto *bm = new gdi_bitmap();
	bm->w = w;
	bm->h = h;
	bm->data.assign(size_t(w) * size_t(h) * 4, 0);

	// 32 bpp BI_RGB, stored as the COLORREF u32 itself: little-endian wasm
	// writes the bytes as [R, G, B, A] with A left at 0 until something is
	// blended over a pixel, and fully-drawn pixels carry 255. That is the
	// byte order ImageData.putImageData and an RGBA8 texture upload eat
	// without conversion.
	if (bits)
		*bits = bm->data.data();
	return bm;
}

void GdiFlush(void)
{
	// Drawing lands in the bytes as it is asked for; there is no queue.
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
	// No native view context exists in a page; the pump hands the canvas the
	// DIB bytes instead. Like the Linux half, hand back a memory DC of the
	// same size for the headless plug-in view, which never paints.
	(void)native;
	auto *dc = new gdi_dc();
	dc->pen   = GetStockObject(BLACK_PEN);
	dc->brush = GetStockObject(WHITE_BRUSH);
	(void)w;
	(void)h;
	g_dcs.push_back(dc);
	return dc;
}

#endif // __EMSCRIPTEN__
