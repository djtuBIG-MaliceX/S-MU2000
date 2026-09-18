// license:BSD-3-Clause
//
// **音色の写し取りを取っておいて、次からはそれを読む。**
//
// native の口（doc/native-engine.md の段 2）は、その音色の 1 音目を firmware に
// 鳴らさせて、スロットに書かれた値を写し取る。2 音目からは SH-2 を止めたまま
// 鳴らせるが、**1 音目のぶんだけは firmware が要る**。16 パートの曲なら
// 出だしの 0.5 秒ほどがそれで埋まる（`render --native-engine` の内訳の「写し取り」）。
//
// 写し取りの中身は (ROM, 設定) が同じなら毎回同じなので、ファイルに残せば
// 2 回目からは 1 音目から native で鳴らせる。DAW に何枚も挿したときも、
// 2 枚目からは写し取りをやり直さない。
//
// 置き場は <設定>/S-MU2000/voicecal/<鍵>.bin。鍵は ROM から作る。
// **中身は利用者の ROM から起こした値なので、配らない。**
//
// **既定では使わない。** 写し取りにはパートのエフェクト経路（素通しの量・
// インサーションの掛かり方）が入っているので、**別の曲で取った写しを使うと
// その曲の音にならない**（lofi をほかの曲の写しで鳴らすと 3.85dB 違った）。
// 同じ曲を何度も鳴らすとき・同じ設定で何枚も挿すときだけ効く。
// render は `--voicecache`、プラグインは plugin.ini の `voicecache=1` で入る。

#ifndef S_MU2000_VOICECACHE_H
#define S_MU2000_VOICECACHE_H

#pragma once

#include "mu2000.h"
#include "nvram.h"

#include "compat/paths.h"

#include <cstdio>
#include <string>
#include <vector>

namespace smu2000 {
namespace voicecache {

// 鍵。**ROM だけ**から作る（写し取りは ROM で決まる。パートの音量などは
// 写し取りの中に一緒に入れてあるので、設定が違っても使い回せる）
inline u64 key(const mu2000 &mu)
{
	auto mix = [](u64 h, u8 b) { h ^= b; return h * 0x100000001b3ull; };
	u64 h = nvram::rom_key(mu);          // プログラム ROM 4MB
	if (const auto wave = mu.wave_rom()) {
		const size_t n = wave->size();
		for (int k = 0; k < 8; k++)
			h = mix(h, u8(n >> (k * 8)));
		const size_t spots[3] = { 0, n / 2, n > 4096 ? n - 4096 : 0 };
		for (size_t at : spots)
			for (size_t i = 0; i < 4096 && at + i < n; i++)
				h = mix(h, (*wave)[at + i]);
	}
	return h;
}

// 置き場。作れなければ空
inline std::string path(u64 k)
{
	const std::string base = smu2000::ensure_config_dir();
	if (base.empty())
		return {};
	const std::string dir = smu2000::join(base, "voicecal");
	if (!smu2000::ensure_dir(dir))
		return {};
	char name[32];
	std::snprintf(name, sizeof(name), "%016llx.bin", (unsigned long long)k);
	return smu2000::join(dir, name);
}

// 読む。読めたぶんだけ機械に入る（足りなければそのぶんだけ写し取りが走る）
inline bool load(mu2000 &mu, u64 k)
{
	const std::string p = path(k);
	if (p.empty())
		return false;
	std::FILE *f = std::fopen(p.c_str(), "rb");
	if (!f)
		return false;
	std::vector<u8> buf;
	u8 chunk[65536];
	size_t got;
	while ((got = std::fread(chunk, 1, sizeof(chunk), f)) > 0)
		buf.insert(buf.end(), chunk, chunk + got);
	std::fclose(f);
	return mu.native_cal_load(buf.data(), buf.size());
}

// 残す。中身が増えていなければ何もしない
inline bool save(const mu2000 &mu, u64 k)
{
	const std::vector<u8> out = mu.native_cal_save();
	if (out.empty())
		return false;
	const std::string p = path(k);
	if (p.empty())
		return false;
	// 書いている途中で落ちても壊れた写しを残さないよう、別名で書いてから置き換える
	const std::string tmp = p + ".new";
	std::FILE *f = std::fopen(tmp.c_str(), "wb");
	if (!f)
		return false;
	const bool ok = std::fwrite(out.data(), 1, out.size(), f) == out.size();
	std::fclose(f);
	if (!ok) {
		std::remove(tmp.c_str());
		return false;
	}
	return smu2000::replace_file(tmp, p);
}

} // namespace voicecache
} // namespace smu2000

#endif // S_MU2000_VOICECACHE_H
