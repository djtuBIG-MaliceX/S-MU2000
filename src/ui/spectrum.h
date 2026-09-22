// license:BSD-3-Clause
//
// パートの音のスペクトラム（音色の窓のフィルタの絵の背景）。
// 波形（bridge::read_scope。44.1kHz）にハン窓を掛けて FFT し、周波数ごとの大きさを dB で返す。
// 画面にも Windows にも依存しない。

#ifndef S_MU2000_UI_SPECTRUM_H
#define S_MU2000_UI_SPECTRUM_H

#pragma once

#include <cmath>
#include <complex>
#include <vector>

namespace ui {
namespace spectrum {

constexpr double RATE = 44100.0;

// n は 2 の冪。out は n / 2 個（bin k の周波数は k × RATE / n）。無音の bin は -200 dB
inline void magnitude_db(const float *wave, size_t n, std::vector<float> &out)
{
	std::vector<std::complex<float>> a(n);
	const double pi = 3.14159265358979323846;
	for (size_t i = 0; i < n; i++) {
		const float w = float(0.5 - 0.5 * std::cos(2.0 * pi * double(i) / double(n - 1)));
		a[i] = std::complex<float>(wave[i] * w, 0.0f);
	}
	// ビット反転
	for (size_t i = 1, j = 0; i < n; i++) {
		size_t bit = n >> 1;
		for (; j & bit; bit >>= 1)
			j ^= bit;
		j ^= bit;
		if (i < j)
			std::swap(a[i], a[j]);
	}
	for (size_t len = 2; len <= n; len <<= 1) {
		const double ang = -2.0 * pi / double(len);
		const std::complex<float> wl(float(std::cos(ang)), float(std::sin(ang)));
		for (size_t i = 0; i < n; i += len) {
			std::complex<float> w(1.0f, 0.0f);
			for (size_t k = 0; k < len / 2; k++) {
				const std::complex<float> u = a[i + k], v = a[i + k + len / 2] * w;
				a[i + k] = u + v;
				a[i + k + len / 2] = u - v;
				w *= wl;
			}
		}
	}
	out.resize(n / 2);
	for (size_t k = 0; k < n / 2; k++) {
		const float m = std::abs(a[k]);
		out[k] = m > 0.0f ? 20.0f * std::log10(m) : -200.0f;
	}
}

} // namespace spectrum
} // namespace ui

#endif // S_MU2000_UI_SPECTRUM_H
