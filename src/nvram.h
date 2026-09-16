// license:BSD-3-Clause
//
// ワーク RAM（NVRAM）をファイルに残す。
//
// 実機の 0x400000-0x43ffff（256KB）は電池で保持されていて、MAME も NVRAM と
// して保存している。firmware はここを読んで起動するので、残しておくと
// **設定が電源を入れ直しても残る**（試したら XG のマスタボリュームまで
// 残った）。残した RAM で起動すると、工場出荷の初期化を飛ばすぶん起動も
// 7.9 秒 → 5.4 秒（音の時間）に縮む。
//
// 置き場は %LOCALAPPDATA%\S-MU2000\nvram\<プログラム ROM のハッシュ>.bin。
// firmware の版が違えば中身の並びも違うかもしれないので、ROM ごとに分ける。
// 中身は 256KB をそのまま。消せば工場出荷状態に戻る。
//
// 使い方: reset() の前に load()、止めたあとに save()。
// **起動に成功したときだけ save() すること。** 起動しなかった回の RAM で
// 良い NVRAM を上書きしてしまう。
// render や make test は使わない（試験は毎回同じ状態から始める）。

#ifndef S_MU2000_NVRAM_H
#define S_MU2000_NVRAM_H

#pragma once

#include "mu2000.h"

#include "compat/paths.h"

#include <cstdio>
#include <string>
#include <vector>

namespace smu2000 {
namespace nvram {

// プログラム ROM の FNV-1a 64bit。4MB で数ミリ秒
inline u64 rom_key(const mu2000 &mu)
{
	u64 h = 0xcbf29ce484222325ull;
	if (const auto prog = mu.program_rom())
		for (u8 b : *prog) {
			h ^= b;
			h *= 0x100000001b3ull;
		}
	return h;
}

// 置き場。作れなければ空
//
// The directory comes from compat/paths.h, which answers with the setting this
// project has always used on Windows (%LOCALAPPDATA%\S-MU2000) and the
// equivalent place on macOS. "nvram" goes under it, so the Windows layout is
// exactly what it was: <settings>\S-MU2000\nvram\<hash>.bin
inline std::string path(const mu2000 &mu)
{
	const std::string base = smu2000::ensure_config_dir();
	if (base.empty())
		return {};
	const std::string dir = smu2000::join(base, "nvram");
	if (!smu2000::ensure_dir(dir))
		return {};
	char name[32];
	std::snprintf(name, sizeof(name), "%016llx.bin", (unsigned long long)rom_key(mu));
	return smu2000::join(dir, name);
}

// reset() の前に呼ぶ。無い・大きさが違うときは何もせず false（工場出荷状態で起動する）
inline bool load(mu2000 &mu)
{
	const std::string p = path(mu);
	if (p.empty())
		return false;
	std::FILE *f = std::fopen(p.c_str(), "rb");
	if (!f)
		return false;
	std::vector<u8> buf(mu.nvram().size() + 1);
	const size_t got = std::fread(buf.data(), 1, buf.size(), f);
	std::fclose(f);
	return got == mu.nvram().size() && mu.set_nvram(buf.data(), got);
}

// 機械が止まっているときに呼ぶ。一時ファイルに書いてから置き換えるので、
// 途中で落ちても前の NVRAM は壊れない
inline bool save(const mu2000 &mu)
{
	const std::string p = path(mu);
	if (p.empty())
		return false;
	const std::string tmp = p + ".tmp";
	std::FILE *f = std::fopen(tmp.c_str(), "wb");
	if (!f)
		return false;
	const std::vector<u8> &ram = mu.nvram();
	const bool ok = std::fwrite(ram.data(), 1, ram.size(), f) == ram.size();
	if (std::fclose(f) != 0 || !ok) {
		std::remove(tmp.c_str());
		return false;
	}
	return smu2000::replace_file(tmp, p);
}

} // namespace nvram
} // namespace smu2000

#endif // S_MU2000_NVRAM_H
