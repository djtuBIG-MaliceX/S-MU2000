// license:BSD-3-Clause
//
// 画面を PNG に書き出すだけのもの。圧縮はしない（deflate の「無圧縮ブロック」）。
// 画面を持たない場所（自動での見た目確認、不具合の報告）で使う。

#include "png.h"

#include <cstdio>
#include <cstdlib>
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
		for (int x = 0; x < w; x++) {
			raw.push_back(src[x * 4 + 2]);   // R
			raw.push_back(src[x * 4 + 1]);   // G
			raw.push_back(src[x * 4 + 0]);   // B
		}
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


// ---- 読むほう。deflate をほどく（zlib を持ち込まないために自前で書く。
//      形は Mark Adler の puff.c と同じ、正準ハフマン符号の数え上げ）

namespace {

struct inflater {
	const u8 *in = nullptr;
	size_t len = 0, pos = 0;
	u32 buf = 0;
	int cnt = 0;
	bool err = false;
	std::vector<u8> *out = nullptr;

	int bits(int need)
	{
		u32 v = buf;
		while (cnt < need) {
			if (pos >= len) {
				err = true;
				return 0;
			}
			v |= u32(in[pos++]) << cnt;
			cnt += 8;
		}
		buf = v >> need;
		cnt -= need;
		return int(v & ((1u << need) - 1));
	}
};

struct huff {
	short count[16];
	short symbol[288];
};

int decode(inflater &s, const huff &h)
{
	int code = 0, first = 0, index = 0;
	for (int len = 1; len < 16; len++) {
		code |= s.bits(1);
		const int count = h.count[len];
		if (code - count < first)
			return h.symbol[index + (code - first)];
		index += count;
		first += count;
		first <<= 1;
		code <<= 1;
		if (s.err)
			return -1;
	}
	return -1;
}

// 符号の長さから表を作る。負は符号が多すぎる（壊れている）
int construct(huff &h, const short *length, int n)
{
	for (int len = 0; len < 16; len++)
		h.count[len] = 0;
	for (int sym = 0; sym < n; sym++)
		h.count[length[sym]]++;
	if (h.count[0] == n)
		return 0;
	int left = 1;
	for (int len = 1; len < 16; len++) {
		left <<= 1;
		left -= h.count[len];
		if (left < 0)
			return left;
	}
	short offs[16];
	offs[1] = 0;
	for (int len = 1; len < 15; len++)
		offs[len + 1] = short(offs[len] + h.count[len]);
	for (int sym = 0; sym < n; sym++)
		if (length[sym])
			h.symbol[offs[length[sym]]++] = short(sym);
	return left;
}

bool codes(inflater &s, const huff &lencode, const huff &distcode)
{
	static const short lbase[29] = { 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31,
	                                 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258 };
	static const short lext[29] = { 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2,
	                                3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0 };
	static const short dbase[30] = { 1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193,
	                                 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
	                                 8193, 12289, 16385, 24577 };
	static const short dext[30] = { 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6,
	                                7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13 };
	std::vector<u8> &out = *s.out;
	for (;;) {
		int sym = decode(s, lencode);
		if (sym < 0 || s.err)
			return false;
		if (sym < 256) {
			out.push_back(u8(sym));
			continue;
		}
		if (sym == 256)
			return true;
		sym -= 257;
		if (sym >= 29)
			return false;
		const int len = lbase[sym] + s.bits(lext[sym]);
		const int dsym = decode(s, distcode);
		if (dsym < 0 || dsym >= 30)
			return false;
		const size_t dist = size_t(dbase[dsym] + s.bits(dext[dsym]));
		if (s.err || dist > out.size())
			return false;
		const size_t from = out.size() - dist;
		for (int i = 0; i < len; i++)
			out.push_back(out[from + size_t(i)]);
	}
}

bool inflate_raw(const u8 *in, size_t len, std::vector<u8> &out)
{
	inflater s;
	s.in = in;
	s.len = len;
	s.out = &out;
	int last;
	do {
		last = s.bits(1);
		const int type = s.bits(2);
		if (s.err)
			return false;
		if (type == 0) {
			// 無圧縮。残りのビットを捨ててバイトの境目から
			s.buf = 0;
			s.cnt = 0;
			if (s.pos + 4 > s.len)
				return false;
			const size_t n = size_t(s.in[s.pos]) | (size_t(s.in[s.pos + 1]) << 8);
			s.pos += 4;
			if (s.pos + n > s.len)
				return false;
			out.insert(out.end(), s.in + s.pos, s.in + s.pos + n);
			s.pos += n;
		} else if (type == 1) {
			short lengths[288 + 30];
			int sym = 0;
			for (; sym < 144; sym++) lengths[sym] = 8;
			for (; sym < 256; sym++) lengths[sym] = 9;
			for (; sym < 280; sym++) lengths[sym] = 7;
			for (; sym < 288; sym++) lengths[sym] = 8;
			huff lc, dc;
			construct(lc, lengths, 288);
			for (sym = 0; sym < 30; sym++) lengths[sym] = 5;
			construct(dc, lengths, 30);
			if (!codes(s, lc, dc))
				return false;
		} else if (type == 2) {
			static const short order[19] = { 16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15 };
			const int nlen = s.bits(5) + 257, ndist = s.bits(5) + 1, ncode = s.bits(4) + 4;
			if (s.err || nlen > 286 || ndist > 30)
				return false;
			short lengths[320] = {};
			for (int i = 0; i < ncode; i++)
				lengths[order[i]] = short(s.bits(3));
			huff lc, dc;
			if (construct(lc, lengths, 19) < 0)
				return false;
			int index = 0;
			while (index < nlen + ndist) {
				int sym = decode(s, lc);
				if (sym < 0 || s.err)
					return false;
				if (sym < 16) {
					lengths[index++] = short(sym);
					continue;
				}
				short prev = 0;
				int rep;
				if (sym == 16) {
					if (index == 0)
						return false;
					prev = lengths[index - 1];
					rep = 3 + s.bits(2);
				} else if (sym == 17) {
					rep = 3 + s.bits(3);
				} else {
					rep = 11 + s.bits(7);
				}
				if (index + rep > nlen + ndist)
					return false;
				while (rep--)
					lengths[index++] = prev;
			}
			if (lengths[256] == 0)
				return false;
			if (construct(lc, lengths, nlen) < 0 || construct(dc, lengths + nlen, ndist) < 0)
				return false;
			if (!codes(s, lc, dc))
				return false;
		} else {
			return false;
		}
	} while (!last);
	return true;
}

u32 get32(const u8 *p)
{
	return (u32(p[0]) << 24) | (u32(p[1]) << 16) | (u32(p[2]) << 8) | u32(p[3]);
}

} // namespace


bool read_png(const std::string &path, int &w, int &h, std::vector<u32> &out)
{
	std::FILE *f = std::fopen(path.c_str(), "rb");
	if (!f)
		return false;
	std::vector<u8> file;
	u8 buf[65536];
	size_t n;
	while ((n = std::fread(buf, 1, sizeof(buf), f)) > 0)
		file.insert(file.end(), buf, buf + n);
	std::fclose(f);

	static const u8 SIG[8] = { 0x89, 'P', 'N', 'G', 0x0d, 0x0a, 0x1a, 0x0a };
	if (file.size() < 8 || std::memcmp(file.data(), SIG, 8) != 0)
		return false;

	int depth = 0, ctype = -1, interlace = 0;
	w = h = 0;
	std::vector<u8> z;
	for (size_t at = 8; at + 12 <= file.size();) {
		const u32 len = get32(&file[at]);
		const char *type = reinterpret_cast<const char *>(&file[at + 4]);
		if (at + 12 + len > file.size())
			return false;
		const u8 *data = &file[at + 8];
		if (!std::memcmp(type, "IHDR", 4) && len >= 13) {
			w = int(get32(data));
			h = int(get32(data + 4));
			depth = data[8];
			ctype = data[9];
			interlace = data[12];
		} else if (!std::memcmp(type, "IDAT", 4)) {
			z.insert(z.end(), data, data + len);
		} else if (!std::memcmp(type, "IEND", 4)) {
			break;
		}
		at += 12 + len;
	}
	int ch = 0;
	switch (ctype) {
	case 0: ch = 1; break;
	case 2: ch = 3; break;
	case 4: ch = 2; break;
	case 6: ch = 4; break;
	default: return false;
	}
	if (w <= 0 || h <= 0 || w > 16384 || h > 16384 || depth != 8 || interlace != 0 || z.size() < 2)
		return false;

	// zlib の頭 2 バイトを飛ばして deflate をほどく
	std::vector<u8> raw;
	const size_t stride = size_t(w) * ch;
	raw.reserve((stride + 1) * size_t(h));
	if (!inflate_raw(z.data() + 2, z.size() - 2, raw) || raw.size() < (stride + 1) * size_t(h))
		return false;

	// 行ごとのフィルタを戻す
	std::vector<u8> img(stride * size_t(h));
	for (int y = 0; y < h; y++) {
		const u8 ft = raw[size_t(y) * (stride + 1)];
		const u8 *src = &raw[size_t(y) * (stride + 1) + 1];
		u8 *row = &img[size_t(y) * stride];
		const u8 *up = y ? row - stride : nullptr;
		for (size_t i = 0; i < stride; i++) {
			const int a = i >= size_t(ch) ? row[i - ch] : 0;
			const int b = up ? up[i] : 0;
			const int c = (up && i >= size_t(ch)) ? up[i - ch] : 0;
			int v = src[i];
			switch (ft) {
			case 0: break;
			case 1: v += a; break;
			case 2: v += b; break;
			case 3: v += (a + b) / 2; break;
			case 4: {
				const int p = a + b - c;
				const int pa = std::abs(p - a), pb = std::abs(p - b), pc = std::abs(p - c);
				v += (pa <= pb && pa <= pc) ? a : (pb <= pc ? b : c);
				break;
			}
			default: return false;
			}
			row[i] = u8(v);
		}
	}

	out.resize(size_t(w) * size_t(h));
	for (size_t i = 0; i < out.size(); i++) {
		const u8 *p = &img[i * ch];
		u32 r, g, b, a = 255;
		if (ch <= 2) {
			r = g = b = p[0];
			if (ch == 2) a = p[1];
		} else {
			r = p[0]; g = p[1]; b = p[2];
			if (ch == 4) a = p[3];
		}
		out[i] = (a << 24) | (r << 16) | (g << 8) | b;
	}
	return true;
}

} // namespace ui
