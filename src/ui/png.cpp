// license:BSD-3-Clause
//
// 画面を PNG に書き出すだけのもの。圧縮はしない（deflate の「無圧縮ブロック」）。
// 画面を持たない場所（自動での見た目確認、不具合の報告）で使う。

#include "png.h"

#include <cstdio>
#include <cstring>
#include <vector>

namespace ui {

namespace {

u32 crc32_of(const u8 *p, size_t n, u32 crc = 0)
{
	static u32 table[256];
	static bool ready = false;
	if (!ready) {
		for (u32 i = 0; i < 256; i++) {
			u32 c = i;
			for (int k = 0; k < 8; k++)
				c = (c & 1) ? (0xedb88320u ^ (c >> 1)) : (c >> 1);
			table[i] = c;
		}
		ready = true;
	}
	crc = ~crc;
	for (size_t i = 0; i < n; i++)
		crc = table[(crc ^ p[i]) & 0xff] ^ (crc >> 8);
	return ~crc;
}

void put32(std::vector<u8> &v, u32 x)
{
	v.push_back(u8(x >> 24)); v.push_back(u8(x >> 16));
	v.push_back(u8(x >> 8));  v.push_back(u8(x));
}

void chunk(std::vector<u8> &out, const char *type, const std::vector<u8> &data)
{
	put32(out, u32(data.size()));
	const size_t at = out.size();
	out.insert(out.end(), type, type + 4);
	out.insert(out.end(), data.begin(), data.end());
	put32(out, crc32_of(out.data() + at, out.size() - at));
}

} // namespace


bool write_png(const std::string &path, const u8 *bgra, int w, int h, int stride)
{
	if (w <= 0 || h <= 0)
		return false;

	// 生データ。行ごとに「フィルタなし」の 0 を先頭に置く
	std::vector<u8> raw;
	raw.reserve(size_t(h) * (size_t(w) * 3 + 1));
	for (int y = 0; y < h; y++) {
		raw.push_back(0);
		const u8 *src = bgra + size_t(y) * stride;
#ifdef __EMSCRIPTEN__
		// The wasm DIB (gdi_wasm.cpp) already lands [R,G,B,A] in memory
		for (int x = 0; x < w; x++) {
			raw.push_back(src[x * 4 + 0]);   // R
			raw.push_back(src[x * 4 + 1]);   // G
			raw.push_back(src[x * 4 + 2]);   // B
		}
#else
		for (int x = 0; x < w; x++) {
			raw.push_back(src[x * 4 + 2]);   // R
			raw.push_back(src[x * 4 + 1]);   // G
			raw.push_back(src[x * 4 + 0]);   // B
		}
#endif
	}

	// zlib。無圧縮ブロックを並べるだけ
	std::vector<u8> z;
	z.push_back(0x78);
	z.push_back(0x01);
	size_t at = 0;
	while (at < raw.size()) {
		const size_t n = std::min<size_t>(65535, raw.size() - at);
		const bool last = (at + n == raw.size());
		z.push_back(last ? 1 : 0);
		z.push_back(u8(n));
		z.push_back(u8(n >> 8));
		z.push_back(u8(~n));
		z.push_back(u8((~n) >> 8));
		z.insert(z.end(), raw.begin() + at, raw.begin() + at + n);
		at += n;
	}
	u32 a = 1, b = 0;
	for (u8 c : raw) { a = (a + c) % 65521; b = (b + a) % 65521; }
	put32(z, (b << 16) | a);

	std::vector<u8> out = { 0x89, 'P', 'N', 'G', 0x0d, 0x0a, 0x1a, 0x0a };

	std::vector<u8> ihdr;
	put32(ihdr, u32(w));
	put32(ihdr, u32(h));
	ihdr.push_back(8);    // 8bit
	ihdr.push_back(2);    // RGB
	ihdr.push_back(0); ihdr.push_back(0); ihdr.push_back(0);
	chunk(out, "IHDR", ihdr);
	chunk(out, "IDAT", z);
	chunk(out, "IEND", {});

	std::FILE *f = std::fopen(path.c_str(), "wb");
	if (!f)
		return false;
	const size_t put = std::fwrite(out.data(), 1, out.size(), f);
	std::fclose(f);
	return put == out.size();
}

} // namespace ui
