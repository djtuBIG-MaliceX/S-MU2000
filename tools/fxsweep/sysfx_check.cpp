// license:BSD-3-Clause
//
// リバーブ・コーラス・バリエーションのパラメータを、firmware に確かめる（issue #35）。
//
// インサーションの種類ごとの表（xg/fx_params.h。LCD から調べたもの）を、システムエフェクトの番地へ
// 読み替えて、種類ごとに下限と上限を書いて問い合わせで読み返す。読み替えは xg/sysfx.h の sysfx_addr。
//   リバーブ       1 バイトの 02-0B → 02 01 02-0B、20-25 → 02 01 10-15
//   コーラス       1 バイトの 02-0B → 02 01 22-2B、20-25 → 02 01 30-35
//   バリエーション 02-0B と 2 バイトの 30-43 → 02 01 42-55（どれも 2 バイト）、20-25 → 02 01 70-75
// 書き出し:
//   T <塊> <msb> <lsb> <名前>
//   P <インサーションの番地> <システムの番地> <下限> <上限> <読み返し 下限> <読み返し 上限> [<LCD の名前>]
// 使い方: build/sysfx_check.exe <rom ディレクトリ> > sysfx.txt
#include "mu2000.h"
#include "xg/fx_params.h"
#include "xg/fx_types.h"
#include "xg/sysfx.h"

#include <cstdio>
#include <fstream>
#include <iterator>
#include <string>
#include <vector>

static mu2000 mu;
static std::vector<u8> g_out;

static void pump(int ms)
{
	s32 l, r;
	for (int i = 0; i < 44100 * ms / 1000; i++) {
		mu.run_sample(l, r);
		u8 b;
		while (mu.midi_out_take(b))
			g_out.push_back(b);
	}
}

static void send(const std::vector<u8> &m) { for (u8 b : m) mu.midi_in(b, 0); }

// 02 01 lo の size バイトを書く（7bit ずつ、上の桁が先）
static void put(u8 lo, int size, int v)
{
	std::vector<u8> m = { 0xf0, 0x43, 0x10, 0x4c, 0x02, 0x01, lo };
	if (size == 2)
		m.push_back(u8((v >> 7) & 0x7f));
	m.push_back(u8(v & 0x7f));
	m.push_back(0xf7);
	send(m);
	pump(60);
}

// 問い合わせて読む。返事が無ければ -1
static int get(u8 lo, int size)
{
	g_out.clear();
	send({ 0xf0, 0x43, 0x30, 0x4c, 0x02, 0x01, lo, 0xf7 });
	pump(60);
	for (size_t i = 0; i + 7 < g_out.size(); i++) {
		if (g_out[i] != 0xf0 || g_out[i + 1] != 0x43 || (g_out[i + 2] & 0xf0) != 0x10 || g_out[i + 3] != 0x4c ||
		    g_out[i + 4] != 0x02 || g_out[i + 5] != 0x01 || g_out[i + 6] != lo)
			continue;
		size_t end = i + 7;
		while (end < g_out.size() && g_out[end] != 0xf7)
			end++;
		const size_t n = end - (i + 7);
		if (n < size_t(size))
			return -1;
		return size == 2 ? (g_out[i + 7] << 7 | g_out[i + 8]) : g_out[i + 7];
	}
	return -1;
}

int main(int argc, char **argv)
{
	if (argc < 2) {
		std::fprintf(stderr, "sysfx_check <rom ディレクトリ>\n");
		return 1;
	}
	const std::string dir = argv[1];
	mu.load_program(dir + "/mu2000_flash.bin");
	mu.load_wave(dir + "/dump");
	mu.load_sintab(dir + "/standin/sin-table.bin");
	mu.reset();
	s32 l, r;
	for (int s = 0; s < 30 * 44100 && !mu.midi_ready(); s++)
		mu.run_sample(l, r);
	pump(1500);
	{
		std::ifstream f(dir + "/mu2000_flash.bin", std::ios::binary);
		const std::vector<u8> rom((std::istreambuf_iterator<char>(f)), {});
		if (!xg::set_fx_type_rom(rom)) {
			std::fprintf(stderr, "ROM の種類の表が読めない\n");
			return 1;
		}
	}

	struct blk { xg::sysfx which; const char *name; const std::vector<xg::fx_type> *types; };
	const blk BLOCKS[] = {
		{ xg::sysfx::reverb,    "reverb",    &xg::rev_types() },
		{ xg::sysfx::chorus,    "chorus",    &xg::cho_types() },
		{ xg::sysfx::variation, "variation", &xg::ins_types() },
	};
	for (const blk &b : BLOCKS) {
		const u8 type_lo = xg::sysfx_type_lo(b.which);
		for (const xg::fx_type &t : *b.types) {
			if (t.msb == 0 || t.msb == 0x40)
				continue;
			send({ 0xf0, 0x43, 0x10, 0x4c, 0x02, 0x01, type_lo, t.msb, t.lsb, 0xf7 });
			pump(300);
			std::printf("T %s %02x %02x %s\n", b.name, t.msb, t.lsb, t.name);
			const xg::fx_def *def = xg::fx_find(t.msb << 7 | t.lsb);
			if (!def)
				continue;
			for (int i = 0; i < def->count; i++) {
				const xg::fx_param &fp = def->params[i];
				int size = 0;
				const int lo = xg::sysfx_addr(b.which, fp, size);
				if (lo < 0) {
					std::printf("P %02x -- [%s]\n", fp.addr, fp.label);
					continue;
				}
				const int orig = get(u8(lo), size);
				put(u8(lo), size, fp.lo);
				const int got_lo = get(u8(lo), size);
				put(u8(lo), size, fp.hi);
				const int got_hi = get(u8(lo), size);
				if (orig >= 0)
					put(u8(lo), size, orig);
				std::printf("P %02x %02x %d %d %d %d [%s]\n", fp.addr, lo, fp.lo, fp.hi, got_lo, got_hi, fp.label);
			}
			std::fflush(stdout);
		}
	}
	return 0;
}
