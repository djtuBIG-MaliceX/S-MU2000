// license:BSD-3-Clause
//
// **窓の最上段に出す、マウスで押せるボタンの帯**。
//
// 一覧やエディタはショートカットキー（F3・F2）でも開けるが、**F3 を自分の
// コマンドとして先に食う DAW がある**ので、キーだけだとプラグインでは
// 開けないことがある。だれでも押せる入口として帯を出しておく。
//
// 帯はパネルの絵の**上に足す**（`panel::set_top_inset`）。パネルの絵は
// 1000 × 385 の全面を使っていて空きが無いので、重ねると絵が隠れてしまう。
//
// 描くのも当たりを見るのも `ui/draw.h` の口しか使わないので、Windows でも
// macOS でも同じように動く（doc/pc-editor.md）。

#ifndef S_MU2000_UI_TOOLBAR_H
#define S_MU2000_UI_TOOLBAR_H

#pragma once

#include <string>
#include <vector>

#include "compat/gdi.h"

#include "compat/gdi.h"
#include "ui/draw.h"

namespace ui {

// 帯に並べる 1 つ。`id` は窓の側が決める（押されたらそれが返る）
struct tool_item {
	std::string label;
	int id = 0;
};

// Which window each button opens. The one vocabulary for gui.exe, the Mac
// GUI, --shot and the plug-ins: every painter builds the same strip from
// window_bar_items() and dispatches the hit id through these, so a button
// can never name one window in one program and another elsewhere.
enum bar_window {
	BAR_LIST = 0,   // 一覧
	BAR_EDITOR = 1, // エディタ
	BAR_FX = 2,     // インサーションの設定
	BAR_SHAPES = 3, // パートの音色
	BAR_MASTER = 4, // マスター
};

// The strip every window shows, in the same order with the same ids
inline std::vector<tool_item> window_bar_items()
{
	return { { "一覧", BAR_LIST }, { "エディタ", BAR_EDITOR },
	         { "音色", BAR_SHAPES }, { "エフェクト", BAR_FX },
	         { "マスター", BAR_MASTER } };
}

class toolbar
{
public:
	static constexpr int HEIGHT = 26;     // 帯の高さ（画素）

	~toolbar()
	{
		if (m_font)
			DeleteObject(m_font);
	}

	void set_items(std::vector<tool_item> items) { m_items = std::move(items); }
	bool empty() const { return m_items.empty(); }

	// 帯の中で押された場所の `id`。帯の外や隙間なら -1
	int hit(int x, int y) const
	{
		if (m_items.empty() || y < 0 || y >= HEIGHT)
			return -1;
		int left = PAD;
		for (const tool_item &it : m_items) {
			const int w = width_of(it.label);
			if (x >= left && x < left + w)
				return it.id;
			left += w + GAP;
		}
		return -1;
	}

	// 押されている最中のものを覚えておくと、押した感じが出る
	void set_down(int id) { m_down = id; }
	int  down() const { return m_down; }

	void paint(HDC dc, int w) const
	{
		if (m_items.empty())
			return;
		if (!m_font)
			m_font = CreateFontA(-13, 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE,
			                     DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_DEFAULT_PRECIS,
			                     CLEARTYPE_QUALITY, VARIABLE_PITCH, "Segoe UI");

		RECT bar{ 0, 0, w, HEIGHT };
		fill(dc, bar, BAR_BG);
		line(dc, 0, HEIGHT - 1, w, HEIGHT - 1, BAR_EDGE, 1);

		int left = PAD;
		for (const tool_item &it : m_items) {
			const int bw = width_of(it.label);
			const bool down = it.id == m_down;
			RECT r{ left, 3, left + bw, HEIGHT - 4 };
			fill(dc, r, down ? BTN_DOWN : BTN_BG);
			line(dc, r.left, r.top, r.right, r.top, BTN_EDGE, 1);
			line(dc, r.left, r.bottom, r.right, r.bottom, BTN_EDGE, 1);
			line(dc, r.left, r.top, r.left, r.bottom, BTN_EDGE, 1);
			line(dc, r.right, r.top, r.right, r.bottom, BTN_EDGE, 1);
			text_in(dc, r, it.label.c_str(), TEXT, m_font,
			        DT_CENTER | DT_VCENTER | DT_SINGLELINE);
			left += bw + GAP;
		}
	}

private:
	static constexpr int PAD = 8;         // 帯の左の余白
	static constexpr int GAP = 6;         // ボタンの間
	static constexpr int SIDE = 11;       // 字の左右の余白
	static constexpr int CHAR_W = 13;     // 字 1 つぶんの見当（全角で測る）

	static constexpr COLORREF BAR_BG   = RGB(0x1c, 0x1c, 0x20);
	static constexpr COLORREF BAR_EDGE = RGB(0x38, 0x38, 0x40);
	static constexpr COLORREF BTN_BG   = RGB(0x2c, 0x2c, 0x33);
	static constexpr COLORREF BTN_DOWN = RGB(0x4a, 0x4a, 0x56);
	static constexpr COLORREF BTN_EDGE = RGB(0x50, 0x50, 0x5c);
	static constexpr COLORREF TEXT     = RGB(0xe0, 0xe0, 0xe6);

	// 字の幅は測らずに見当で出す。**測る口（GetTextExtent）が compat/gdi.h に
	// 無い**ので、UTF-8 の字数（半角は半分）から出す。少し広めに取る
	static int width_of(const std::string &s)
	{
		int n = 0;
		for (size_t i = 0; i < s.size();) {
			const unsigned char c = static_cast<unsigned char>(s[i]);
			if (c < 0x80) { n += 1; i += 1; }             // 半角
			else if (c < 0xe0) { n += 2; i += 2; }
			else if (c < 0xf0) { n += 2; i += 3; }        // 漢字・かな
			else { n += 2; i += 4; }
		}
		return SIDE * 2 + n * CHAR_W / 2;
	}

	std::vector<tool_item> m_items;
	mutable HFONT m_font = nullptr;
	int m_down = -1;
};

} // namespace ui

#endif // S_MU2000_UI_TOOLBAR_H
