// license:BSD-3-Clause
//
// 音の出口をアナログ（LINE OUT・PHONES）にしたときの、直流を切る段。
//
// MU2000 の S/PDIF（デジタル）の出口には、一部の DPCM のサンプルが持つ直流がそのまま出る。
// Alto Sax Legato（0/18/65）の A2 と A5 で、伸ばしている所の平均が最大値の 36%。実機の S/PDIF を
// 録ってエミュと比べると 0.1LSB まで同じだった（2026-09-17、issue #3）。エミュの既定はこのデジタルの形。
//
// アナログの出口は、DAC の後ろのコンデンサで直流が切れる（はず）。ここではそれを 1 次のハイパスで置き換える。
// **切れる周波数は実機で測っていない**（アナログ出力を録る線が繋がっていない）。一般的な出力の結合の
// 数 Hz として CUTOFF_HZ を仮に置いている。測ったらここを合わせる。
//
// 音量のつまみより前（DAC のすぐ後ろ）に掛ける。

#ifndef S_MU2000_ANALOG_OUT_H
#define S_MU2000_ANALOG_OUT_H

#pragma once

#include <cmath>

namespace smu2000 {

class analog_out
{
public:
	static constexpr double CUTOFF_HZ = 3.0;   // 仮の値（上の説明）

	explicit analog_out(double rate = 44100.0) { set_rate(rate); }

	void set_rate(double rate)
	{
		m_a = std::exp(-2.0 * 3.14159265358979323846 * CUTOFF_HZ / rate);
	}

	// 切り替えたときに前の状態を引きずらないよう、使い始めに呼ぶ
	void reset()
	{
		m_x[0] = m_x[1] = m_y[0] = m_y[1] = 0.0;
	}

	// y[n] = x[n] - x[n-1] + a * y[n-1]。ch は 0 = 左、1 = 右
	double run(int ch, double x)
	{
		const double y = x - m_x[ch] + m_a * m_y[ch];
		m_x[ch] = x;
		m_y[ch] = y;
		return y;
	}

private:
	double m_a = 0.0;
	double m_x[2] = {}, m_y[2] = {};
};

} // namespace smu2000

#endif // S_MU2000_ANALOG_OUT_H
