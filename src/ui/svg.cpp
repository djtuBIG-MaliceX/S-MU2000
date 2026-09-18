// license:BSD-3-Clause

#include "svg.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>

namespace ui {

namespace {

// 2 × 3 の変換。点は (a x + c y + e, b x + d y + f) へ移る
struct mat {
	double a = 1, b = 0, c = 0, d = 1, e = 0, f = 0;

	mat mul(const mat &m) const           // this を先、m をあと
	{
		mat r;
		r.a = a * m.a + b * m.c;
		r.b = a * m.b + b * m.d;
		r.c = c * m.a + d * m.c;
		r.d = c * m.b + d * m.d;
		r.e = e * m.a + f * m.c + m.e;
		r.f = e * m.b + f * m.d + m.f;
		return r;
	}
	void apply(double x, double y, double &ox, double &oy) const
	{
		ox = a * x + c * y + e;
		oy = b * x + d * y + f;
	}
};

// 数を 1 つ取り出す。SVG は区切りに空白でも , でも - でも来る
bool take_num(const char *&p, double &out)
{
	while (*p == ' ' || *p == ',' || *p == '\t' || *p == '\n' || *p == '\r')
		p++;
	if (!*p)
		return false;
	char *end = nullptr;
	out = std::strtod(p, &end);
	if (end == p)
		return false;
	p = end;
	return true;
}

bool is_cmd(char c)
{
	return std::strchr("MmLlHhVvCcZzAaQqSsTt", c) != nullptr;
}

// 属性を 1 つ取り出す。name="…" の中身。
// 名前の前が空白か < のものだけを見る（d を探して id="…" の中の d=" に当たらないように。
// width を探して stroke-width に当たらないように。Inkscape で保存した絵は id が d の前に来ることがある）
std::string attr(const std::string &tag, const char *name)
{
	const std::string key = std::string(name) + "=\"";
	size_t at = 0;
	while ((at = tag.find(key, at)) != std::string::npos) {
		const char before = at ? tag[at - 1] : ' ';
		if (before == ' ' || before == '\t' || before == '\n' || before == '\r' || before == '<')
			break;
		at += key.size();
	}
	if (at == std::string::npos)
		return {};
	const size_t start = at + key.size();
	const size_t end = tag.find('"', start);
	return tag.substr(start, end == std::string::npos ? end : end - start);
}

// style="fill:#404040;stroke:none;…" から 1 つ
std::string style_of(const std::string &style, const char *name)
{
	size_t at = 0;
	const std::string key(name);
	while ((at = style.find(key, at)) != std::string::npos) {
		// 前が区切りで、後ろが : であること（fill と fill-opacity を混ぜない）
		const bool head = (at == 0 || style[at - 1] == ';' || style[at - 1] == ' ');
		const size_t colon = at + key.size();
		if (head && colon < style.size() && style[colon] == ':') {
			const size_t end = style.find(';', colon);
			std::string v = style.substr(colon + 1,
			                             end == std::string::npos ? end : end - colon - 1);
			while (!v.empty() && v.front() == ' ')
				v.erase(0, 1);
			while (!v.empty() && v.back() == ' ')
				v.pop_back();
			return v;
		}
		at += key.size();
	}
	return {};
}

bool parse_color(const std::string &v, COLORREF &out)
{
	if (v.empty() || v == "none")
		return false;
	if (v[0] == '#') {
		unsigned n = 0;
		if (v.size() == 7 && std::sscanf(v.c_str() + 1, "%6x", &n) == 1) {
			out = RGB((n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff);
			return true;
		}
		if (v.size() == 4 && std::sscanf(v.c_str() + 1, "%3x", &n) == 1) {
			const int r = (n >> 8) & 0xf, g = (n >> 4) & 0xf, b = n & 0xf;
			out = RGB(r * 17, g * 17, b * 17);
			return true;
		}
		return false;
	}
	if (v == "black") { out = RGB(0, 0, 0); return true; }
	if (v == "white") { out = RGB(255, 255, 255); return true; }
	return false;
}

mat parse_transform(const std::string &t)
{
	mat m;
	if (t.empty())
		return m;
	const char *p = t.c_str();
	if (const char *q = std::strstr(p, "matrix(")) {
		q += 7;
		double v[6] = { 1, 0, 0, 1, 0, 0 };
		for (int i = 0; i < 6; i++)
			take_num(q, v[i]);
		m.a = v[0]; m.b = v[1]; m.c = v[2]; m.d = v[3]; m.e = v[4]; m.f = v[5];
	} else if (const char *q = std::strstr(p, "translate(")) {
		q += 10;
		double x = 0, y = 0;
		take_num(q, x);
		take_num(q, y);
		m.e = x; m.f = y;
	} else if (const char *q = std::strstr(p, "scale(")) {
		q += 6;
		double x = 1, y = 0;
		take_num(q, x);
		if (!take_num(q, y))
			y = x;
		m.a = x; m.d = y;
	}
	return m;
}

// 数の属性（"12.5" や "12.5px"）。無ければ def
double num_attr(const std::string &tag, const char *name, double def = 0.0)
{
	const std::string v = attr(tag, name);
	return v.empty() ? def : std::atof(v.c_str());
}

// 四角・丸・楕円・多角形を、同じ形のパスの d に直す（読み手は d だけを読む）。
// 丸みは 3 次ベジエで近づける（弧の命令 A は読まないので）
std::string shape_to_d(const std::string &name, const std::string &tag)
{
	char buf[512];
	constexpr double K = 0.5522847498;       // 円の 1/4 をベジエで描くときの係数
	auto ellipse = [&](double cx, double cy, double rx, double ry) {
		std::snprintf(buf, sizeof(buf),
		              "M %g,%g C %g,%g %g,%g %g,%g C %g,%g %g,%g %g,%g C %g,%g %g,%g %g,%g C %g,%g %g,%g %g,%g Z",
		              cx + rx, cy,
		              cx + rx, cy + ry * K, cx + rx * K, cy + ry, cx, cy + ry,
		              cx - rx * K, cy + ry, cx - rx, cy + ry * K, cx - rx, cy,
		              cx - rx, cy - ry * K, cx - rx * K, cy - ry, cx, cy - ry,
		              cx + rx * K, cy - ry, cx + rx, cy - ry * K, cx + rx, cy);
		return std::string(buf);
	};
	if (name == "circle") {
		const double r = num_attr(tag, "r");
		return r > 0 ? ellipse(num_attr(tag, "cx"), num_attr(tag, "cy"), r, r) : std::string();
	}
	if (name == "ellipse") {
		const double rx = num_attr(tag, "rx"), ry = num_attr(tag, "ry");
		return rx > 0 && ry > 0 ? ellipse(num_attr(tag, "cx"), num_attr(tag, "cy"), rx, ry) : std::string();
	}
	if (name == "rect") {
		const double x = num_attr(tag, "x"), y = num_attr(tag, "y");
		const double w = num_attr(tag, "width"), h = num_attr(tag, "height");
		if (w <= 0 || h <= 0)
			return {};
		// 片方だけ書いてあれば、もう片方も同じ（SVG の決まり）
		double rx = num_attr(tag, "rx", -1), ry = num_attr(tag, "ry", -1);
		if (rx < 0) rx = ry;
		if (ry < 0) ry = rx;
		rx = std::clamp(rx, 0.0, w / 2);
		ry = std::clamp(ry, 0.0, h / 2);
		if (rx <= 0 || ry <= 0) {
			std::snprintf(buf, sizeof(buf), "M %g,%g L %g,%g L %g,%g L %g,%g Z", x, y, x + w, y, x + w, y + h, x, y + h);
			return buf;
		}
		std::snprintf(buf, sizeof(buf),
		              "M %g,%g L %g,%g C %g,%g %g,%g %g,%g L %g,%g C %g,%g %g,%g %g,%g "
		              "L %g,%g C %g,%g %g,%g %g,%g L %g,%g C %g,%g %g,%g %g,%g Z",
		              x + rx, y, x + w - rx, y,
		              x + w - rx + rx * K, y, x + w, y + ry - ry * K, x + w, y + ry,
		              x + w, y + h - ry,
		              x + w, y + h - ry + ry * K, x + w - rx + rx * K, y + h, x + w - rx, y + h,
		              x + rx, y + h,
		              x + rx - rx * K, y + h, x, y + h - ry + ry * K, x, y + h - ry,
		              x, y + ry,
		              x, y + ry - ry * K, x + rx - rx * K, y, x + rx, y);
		return buf;
	}
	if (name == "polygon" || name == "polyline") {
		const std::string pts = attr(tag, "points");
		const char *q = pts.c_str();
		std::string d;
		double px, py;
		bool first = true;
		while (take_num(q, px) && take_num(q, py)) {
			std::snprintf(buf, sizeof(buf), "%s %g,%g ", first ? "M" : "L", px, py);
			d += buf;
			first = false;
		}
		if (!d.empty() && name == "polygon")
			d += "Z";
		return d;
	}
	return {};
}

} // namespace


bool svg_art::load_file(const std::string &path)
{
	FILE *f = std::fopen(path.c_str(), "rb");
	if (!f)
		return false;
	std::string all;
	char buf[8192];
	size_t n;
	while ((n = std::fread(buf, 1, sizeof(buf), f)) > 0)
		all.append(buf, n);
	std::fclose(f);
	return load_text(all);
}

bool svg_art::load_text(const std::string &text)
{
	clear();

	// コメントは読まない（<!-- --> の中に残した古い形を描かないように）
	std::string s;
	s.reserve(text.size());
	for (size_t at = 0; at < text.size();) {
		const size_t open = text.find("<!--", at);
		if (open == std::string::npos) {
			s.append(text, at, std::string::npos);
			break;
		}
		s.append(text, at, open - at);
		const size_t close = text.find("-->", open + 4);
		if (close == std::string::npos)
			break;
		at = close + 3;
	}

	// viewBox。無ければ width / height を使う
	{
		const std::string vb = attr(s, "viewBox");
		if (!vb.empty()) {
			const char *p = vb.c_str();
			for (int i = 0; i < 4; i++)
				take_num(p, m_vb[i]);
		} else {
			m_vb[0] = m_vb[1] = 0;
			m_vb[2] = std::atof(attr(s, "width").c_str());
			m_vb[3] = std::atof(attr(s, "height").c_str());
		}
		if (m_vb[2] <= 0 || m_vb[3] <= 0)
			return false;
	}

	// <g> の変換を 1 段だけ拾う。MAME の絵はこれで足りる
	mat gm;
	{
		const size_t at = s.find("<g ");
		if (at != std::string::npos) {
			const size_t end = s.find('>', at);
			gm = parse_transform(attr(s.substr(at, end - at), "transform"));
		}
	}

	// 形は書いてある順に描く。path のほか、rect・circle・ellipse・polygon・polyline も読む
	// （パスに直して同じように扱う）
	static const char *const ELEMENTS[] = { "path", "rect", "circle", "ellipse", "polygon", "polyline" };
	size_t at = 0;
	for (;;) {
		size_t found = std::string::npos;
		std::string name;
		for (const char *e : ELEMENTS) {
			const std::string open = std::string("<") + e;
			size_t f = at;
			while ((f = s.find(open, f)) != std::string::npos) {
				const char next = f + open.size() < s.size() ? s[f + open.size()] : '>';
				if (next == ' ' || next == '\t' || next == '\n' || next == '\r' || next == '/' || next == '>')
					break;
				f += open.size();            // <pathology> のような別の名前
			}
			if (f < found) {
				found = f;
				name = e;
			}
		}
		if (found == std::string::npos)
			break;
		const size_t end = s.find('>', found);
		if (end == std::string::npos)
			break;
		const std::string tag = s.substr(found, end - found);
		at = end + 1;

		const std::string d = name == "path" ? attr(tag, "d") : shape_to_d(name, tag);
		if (d.empty())
			continue;
		const std::string style = attr(tag, "style");
		const mat pm = parse_transform(attr(tag, "transform"));
		const mat m = pm.mul(gm);          // 自分の変換を先、g の変換をあと

		shape sh;
		COLORREF c = 0;
		std::string v = style_of(style, "fill");
		if (v.empty())
			v = attr(tag, "fill");
		if (parse_color(v, c)) {
			sh.fill = c;
			sh.has_fill = true;
		}
		v = style_of(style, "stroke");
		if (v.empty())
			v = attr(tag, "stroke");
		if (parse_color(v, c)) {
			sh.stroke = c;
			sh.has_stroke = true;
			const std::string w = style_of(style, "stroke-width");
			sh.stroke_w = w.empty() ? 1.0 : std::atof(w.c_str());
		}
		if (!sh.has_fill && !sh.has_stroke)
			continue;

		// d を読む。曲線はここで折れ線にしておく
		std::vector<pt> cur;
		double x = 0, y = 0, sx = 0, sy = 0;
		char cmd = 0;
		const char *p = d.c_str();
		auto push = [&](double px, double py) {
			double ox, oy;
			m.apply(px, py, ox, oy);
			cur.push_back({ ox, oy });
		};
		auto flush = [&](bool closed) {
			if (cur.size() >= 2) {
				sh.subs.push_back(cur);
				sh.closed.push_back(closed);
			}
			cur.clear();
		};

		while (*p) {
			while (*p == ' ' || *p == ',' || *p == '\n' || *p == '\r' || *p == '\t')
				p++;
			if (!*p)
				break;
			if (is_cmd(*p)) {
				cmd = *p++;
			} else if (!cmd) {
				p++;
				continue;
			}

			const bool rel = (cmd >= 'a' && cmd <= 'z');
			const char c2 = char(std::toupper((unsigned char)cmd));

			if (c2 == 'Z') {
				flush(true);
				x = sx; y = sy;
				cmd = 0;
				continue;
			}

			double a1, a2, a3, a4, a5, a6;
			if (c2 == 'M') {
				if (!take_num(p, a1) || !take_num(p, a2)) break;
				if (rel) { a1 += x; a2 += y; }
				flush(false);
				x = a1; y = a2; sx = x; sy = y;
				push(x, y);
				cmd = rel ? 'l' : 'L';           // 続きは線として読む
			} else if (c2 == 'L') {
				if (!take_num(p, a1) || !take_num(p, a2)) break;
				if (rel) { a1 += x; a2 += y; }
				x = a1; y = a2;
				push(x, y);
			} else if (c2 == 'H') {
				if (!take_num(p, a1)) break;
				x = rel ? x + a1 : a1;
				push(x, y);
			} else if (c2 == 'V') {
				if (!take_num(p, a1)) break;
				y = rel ? y + a1 : a1;
				push(x, y);
			} else if (c2 == 'C') {
				if (!take_num(p, a1) || !take_num(p, a2) || !take_num(p, a3) ||
				    !take_num(p, a4) || !take_num(p, a5) || !take_num(p, a6))
					break;
				if (rel) { a1 += x; a2 += y; a3 += x; a4 += y; a5 += x; a6 += y; }
				const double x0 = x, y0 = y;
				const int steps = 12;
				for (int i = 1; i <= steps; i++) {
					const double t = double(i) / steps, u = 1 - t;
					const double bx = u * u * u * x0 + 3 * u * u * t * a1 +
					                  3 * u * t * t * a3 + t * t * t * a5;
					const double by = u * u * u * y0 + 3 * u * u * t * a2 +
					                  3 * u * t * t * a4 + t * t * t * a6;
					push(bx, by);
				}
				x = a5; y = a6;
			} else {
				// 読まない命令。数を食い潰して次へ。数でも命令でもない字なら 1 つ飛ばす（止まらないように）
				double junk;
				const char *before = p;
				while (*p && !is_cmd(*p) && take_num(p, junk))
					;
				if (p == before && *p && !is_cmd(*p)) {
					p++;
					cmd = 0;
				}
			}
		}
		flush(false);

		if (!sh.subs.empty())
			m_shapes.push_back(std::move(sh));
	}
	return ok();
}

void svg_art::draw(HDC dc, const RECT &dst, double deg) const
{
	if (m_shapes.empty())
		return;

	const double dw = double(dst.right - dst.left), dh = double(dst.bottom - dst.top);
	if (dw <= 0 || dh <= 0)
		return;
	// 縦横比は保つ。余りは真ん中に
	const double k = std::min(dw / m_vb[2], dh / m_vb[3]);
	const double ox = dst.left + (dw - m_vb[2] * k) / 2 - m_vb[0] * k;
	const double oy = dst.top  + (dh - m_vb[3] * k) / 2 - m_vb[1] * k;

	// 回すときの軸は、当てはめた四角の真ん中
	const double cx = (dst.left + dst.right) / 2.0;
	const double cy = (dst.top + dst.bottom) / 2.0;
	const double rad = deg * 3.14159265358979 / 180.0;
	const double cs = std::cos(rad), sn = std::sin(rad);
	const bool turn = (deg != 0.0);

	std::vector<POINT> pts;
	std::vector<INT>   counts;

	for (const shape &sh : m_shapes) {
		pts.clear();
		counts.clear();
		for (const auto &sub : sh.subs) {
			counts.push_back(INT(sub.size()));
			for (const pt &q : sub) {
				double px = ox + q.x * k, py = oy + q.y * k;
				if (turn) {
					const double dx = px - cx, dy = py - cy;
					px = cx + dx * cs - dy * sn;
					py = cy + dx * sn + dy * cs;
				}
				pts.push_back({ int(std::lround(px)), int(std::lround(py)) });
			}
		}
		if (pts.empty())
			continue;

		if (sh.has_fill) {
			HBRUSH b = CreateSolidBrush(sh.fill);
			HGDIOBJ ob = SelectObject(dc, b);
			HGDIOBJ op = SelectObject(dc, GetStockObject(NULL_PEN));
			const int old = SetPolyFillMode(dc, ALTERNATE);
			PolyPolygon(dc, pts.data(), counts.data(), INT(counts.size()));
			SetPolyFillMode(dc, old);
			SelectObject(dc, op);
			SelectObject(dc, ob);
			DeleteObject(b);
		}
		if (sh.has_stroke) {
			HPEN pen = CreatePen(PS_SOLID, std::max(1, int(sh.stroke_w * k + 0.5)),
			                     sh.stroke);
			HGDIOBJ op = SelectObject(dc, pen);
			size_t at = 0;
			for (size_t i = 0; i < counts.size(); i++) {
				Polyline(dc, pts.data() + at, counts[i]);
				if (sh.closed[i] && counts[i] >= 2) {
					MoveToEx(dc, pts[at + counts[i] - 1].x, pts[at + counts[i] - 1].y,
					         nullptr);
					LineTo(dc, pts[at].x, pts[at].y);
				}
				at += size_t(counts[i]);
			}
			SelectObject(dc, op);
			DeleteObject(pen);
		}
	}
}

} // namespace ui
