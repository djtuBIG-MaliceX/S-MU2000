// license:BSD-3-Clause
//
// ごく小さい SVG の絵描き。パネルに絵を重ねるためだけのもの。
//
// 読めるのは
//   <path d="…"> の M L H V C Z（大文字小文字とも）
//   transform の translate(…) と matrix(…)
//   style の fill / fill-opacity / stroke / stroke-width
//
// 弧（A）も二次ベジエ（Q S T）も、勾配も、文字も読まない。**要るのは
// これだけ**で、MAME の mu2000.lay に入っている絵（CC0）はこれで出せる。
//
// 曲線は読み込むときに折れ線にしておく。描くときは viewBox を渡された
// 四角に当てはめて、点を移すだけ。だから窓の大きさが変わっても綺麗に出る。

#ifndef S_MU2000_UI_SVG_H
#define S_MU2000_UI_SVG_H

#pragma once

#include <string>
#include <vector>

#include "compat/gdi.h"

namespace ui {

class svg_art
{
public:
	bool load_file(const std::string &path);
	bool load_text(const std::string &text);
	bool ok() const { return !m_shapes.empty(); }
	void clear() { m_shapes.clear(); }

	// viewBox を dst に当てはめて描く。縦横比は保ったまま真ん中に置く。
	// deg を渡すと、dst の真ん中を軸にその角度だけ回す（つまみ用）
	void draw(HDC dc, const RECT &dst, double deg = 0.0) const;

private:
	struct pt { double x, y; };
	struct shape {
		std::vector<std::vector<pt>> subs;   // 折れ線にした輪郭
		std::vector<bool> closed;
		COLORREF fill = 0, stroke = 0;
		bool     has_fill = false, has_stroke = false;
		double   stroke_w = 1;
	};

	std::vector<shape> m_shapes;
	double m_vb[4] = { 0, 0, 1, 1 };          // viewBox
};

} // namespace ui

#endif // S_MU2000_UI_SVG_H
