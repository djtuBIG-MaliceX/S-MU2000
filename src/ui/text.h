// license:BSD-3-Clause
//
// 文字の入れ替え。ソースは UTF-8 で書いてあるので、Windows へ渡すときは
// UTF-16 に直す。**A 付きの API（AppendMenuA など）に UTF-8 をそのまま
// 渡すと、CP932 と思われて文字化けする**。
//
// 機器の名前も同じで、W 付きの API から取って UTF-8 に直しておく。
// そうしておけば、こちらの中は全部 UTF-8 で揃う。

#ifndef S_MU2000_UI_TEXT_H
#define S_MU2000_UI_TEXT_H

#pragma once

#include <string>

#include <windows.h>

namespace ui {

inline std::wstring to_wide(const std::string &utf8)
{
	if (utf8.empty())
		return {};
	const int n = MultiByteToWideChar(CP_UTF8, 0, utf8.data(), int(utf8.size()),
	                                  nullptr, 0);
	if (n <= 0)
		return {};
	std::wstring out(size_t(n), L'\0');
	MultiByteToWideChar(CP_UTF8, 0, utf8.data(), int(utf8.size()), out.data(), n);
	return out;
}

inline std::string to_utf8(const wchar_t *w)
{
	if (!w || !*w)
		return {};
	const int n = WideCharToMultiByte(CP_UTF8, 0, w, -1, nullptr, 0, nullptr, nullptr);
	if (n <= 1)
		return {};
	std::string out(size_t(n - 1), '\0');   // 終端は入れない
	WideCharToMultiByte(CP_UTF8, 0, w, -1, out.data(), n, nullptr, nullptr);
	return out;
}

// A file dialog filter from translated descriptions: "desc (pat)" NUL
// "pat" NUL, ending double NUL. Patterns (*.img) stay untranslated;
// to_wide counts bytes explicitly, so the embedded NULs survive.
inline std::wstring dlg_filter(const char *desc1, const char *pat1,
                               const char *desc2 = nullptr, const char *pat2 = nullptr)
{
	std::string n;
	const auto one = [&](const char *d, const char *p) {
		n += d;
		n += " (";
		n += p;
		n += ')';
		n += '\0';
		n += p;
		n += '\0';
	};
	one(desc1, pat1);
	if (desc2 && pat2)
		one(desc2, pat2);
	n += '\0';
	return to_wide(n);
}

} // namespace ui

#endif // S_MU2000_UI_TEXT_H
