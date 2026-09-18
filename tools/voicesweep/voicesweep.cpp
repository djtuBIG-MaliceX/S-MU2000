// license:BSD-3-Clause
//
// 音色の記録（ROM の 84 バイト）と、firmware が SWP30 に書くレジスタを組で集める。
// doc/native-engine.md の段 1「84 バイトの意味を掃引で割り出す」の材料作り。
//
// やること: 音色を選び、1 音鳴らし、その間に SWP30 のスロットへ書かれた値を全部拾う。
// 同じ音色の記録を ROM から取り出して、両方を 1 つの塊として書き出す。
// あとは tools/voicesweep/solve.py が「どのバイトがどのレジスタになるか」を探す。
//
// 使い方:
//   build/voicesweep.exe <rom ディレクトリ> [音色の一覧] > voicesweep.txt
// 音色の一覧は 1 行 "msb lsb prog"（10 進）。省くと XG の主な組を一通り。
// 鍵と強さは -n 36,60,84 -v 32,100 のように変えられる。
#include "mu2000.h"
#include "xg/ram.h"
#include "xg/voices.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>
#include <vector>

namespace {

mu2000 mu;

void pump(int ms)
{
	s32 l, r;
	for (int i = 0; i < 44100 * ms / 1000; i++) {
		mu.run_sample(l, r);
		u8 b;
		while (mu.midi_out_take(b)) {}
	}
}

void send(const std::vector<u8> &m)
{
	for (u8 b : m)
		mu.midi_in(b, 0);
}

// 拾った書き込み。master かどうか・レジスタ番号 → 最後の値。
// g_all は最初から溜め続ける（鳴らすたびに全部を書き直すとは限らないので、
// 鳴り始めたスロットの中身は「今までに書かれた値」で見る）
std::map<u32, u16> g_writes[2], g_all[2];
u64 g_keyon[2];                 // 窓の中で keyon が掛かったスロット
u64 g_mask[2];                  // 今の keyon マスク

void watch_on()
{
	g_writes[0].clear();
	g_writes[1].clear();
	g_keyon[0] = g_keyon[1] = 0;
	mu.set_swp_watch([](bool master, u32 reg, u16 value) {
		const int c = master ? 1 : 0;
		g_writes[c][reg] = value;
		g_all[c][reg] = value;
		// keyon マスク（4 本で 64 スロット）と、掛ける合図
		switch (reg) {
		case 0x18e: g_mask[c] = (g_mask[c] & ~(u64(0xffff) << 48)) | (u64(value) << 48); break;
		case 0x18f: g_mask[c] = (g_mask[c] & ~(u64(0xffff) << 32)) | (u64(value) << 32); break;
		case 0x1ce: g_mask[c] = (g_mask[c] & ~(u64(0xffff) << 16)) | (u64(value) << 16); break;
		case 0x1cf: g_mask[c] = (g_mask[c] & ~u64(0xffff)) | value; break;
		case 0x20e: g_keyon[c] |= g_mask[c]; break;
		default: break;
		}
	});
}

void watch_off() { mu.set_swp_watch(nullptr); }

std::vector<int> numbers(const char *s)
{
	std::vector<int> v;
	while (*s) {
		v.push_back(int(std::strtol(s, const_cast<char **>(&s), 10)));
		while (*s == ',' || *s == ' ')
			s++;
	}
	return v;
}

} // namespace


int main(int argc, char **argv)
{
	if (argc < 2) {
		std::fprintf(stderr, "voicesweep <rom ディレクトリ> [音色の一覧] [-n 鍵,...] [-v 強さ,...]%c", 10);
		return 1;
	}
	const std::string dir = argv[1];
	const char *list_path = nullptr;
	std::vector<int> notes{ 36, 60, 84 }, vels{ 100 };
	for (int i = 2; i < argc; i++) {
		if (!std::strcmp(argv[i], "-n") && i + 1 < argc)
			notes = numbers(argv[++i]);
		else if (!std::strcmp(argv[i], "-v") && i + 1 < argc)
			vels = numbers(argv[++i]);
		else
			list_path = argv[i];
	}

	if (!mu.load_program(dir + "/mu2000_flash.bin") || !mu.load_wave(dir + "/dump")) {
		std::fprintf(stderr, "%s%c", mu.error().c_str(), 10);
		return 1;
	}
	mu.load_sintab(dir + "/standin/sin-table.bin");
	mu.reset();

	s32 l, r;
	for (int i = 0; i < 30 * 44100 && !mu.midi_ready(); i++)
		mu.run_sample(l, r);
	pump(1500);

	xg::voice_rom vr(mu.program_rom());
	if (!vr.ok())
		std::fprintf(stderr, "警告: ROM の版が違うので、記録の名前は出せない%c", 10);

	// 調べる音色の一覧
	struct pick { int msb, lsb, prog; };
	std::vector<pick> picks;
	if (list_path) {
		std::FILE *f = std::fopen(list_path, "r");
		if (!f) { std::fprintf(stderr, "開けない: %s%c", list_path, 10); return 1; }
		int a, b, c;
		while (std::fscanf(f, "%d %d %d", &a, &b, &c) == 3)
			picks.push_back({ a, b, c });
		std::fclose(f);
	} else {
		// XG の主な組。MSB 0（LSB 0/1/…）と 8・16・32・64 あたりを一通り
		for (int prog = 0; prog < 128; prog++)
			picks.push_back({ 0, 0, prog });
	}

	const u8 *ram = mu.nvram().data();
	const u8 *rom = mu.program_rom()->data();
	const u32 part0 = xg::ram::part_base(0);

	std::printf("# voicesweep: %zu 音色 × %zu 鍵 × %zu 強さ%c",
	            picks.size(), notes.size(), vels.size(), 10);

	for (const pick &p : picks) {
		// 音色を選ぶ（ch1）
		send({ 0xb0, 0x00, u8(p.msb), 0xb0, 0x20, u8(p.lsb), 0xc0, u8(p.prog) });
		pump(200);

		const u8 *pr = ram + part0;
		const u32 rec = u32(pr[xg::ram::PART_VOICE]) << 24 | u32(pr[xg::ram::PART_VOICE + 1]) << 16 |
		                u32(pr[xg::ram::PART_VOICE + 2]) << 8 | pr[xg::ram::PART_VOICE + 3];
		const std::string name = vr.record_name(rec);
		const int elems = rec ? rom[rec] : 0;
		std::printf("VOICE %d %d %d rec=%06x elems=%d name=%s%c",
		            p.msb, p.lsb, p.prog, rec, elems, name.c_str(), 10);
		if (!rec)
			continue;

		// 記録の中身。「印 1・1・名前 10」＋ 要素ごとに 84 バイト
		for (int e = 0; e < elems && e < 4; e++) {
			std::printf("ELEM %d", e);
			for (int i = 0; i < 84; i++)
				std::printf(" %02x", rom[rec + 12 + u32(e) * 84 + u32(i)]);
			std::putchar(10);
		}

		for (int note : notes) {
			for (int vel : vels) {
				watch_on();
				send({ 0x90, u8(note), u8(vel) });
				pump(40);                       // firmware がスロットを組み立てる間
				watch_off();
				send({ 0x80, u8(note), 0x40 });

				std::printf("NOTE %d %d%c", note, vel, 10);
				// 鳴り始めたスロットだけ、今までに書かれた全部の値を出す。
				// 制御レジスタ（下位 6bit が 0e/0f）と MEG のは要らない
				for (int chip = 0; chip < 2; chip++) {
					for (int ch = 0; ch < 64; ch++) {
						if (!(g_keyon[chip] & (u64(1) << ch)))
							continue;
						std::printf("CH %d %02x", chip, ch);
						for (int sl = 0; sl < 0x40; sl++) {
							if (sl == 0x0e || sl == 0x0f)
								continue;
							const auto it = g_all[chip].find(u32(ch) * 64 + u32(sl));
							if (it != g_all[chip].end())
								std::printf(" %02x=%04x", sl, it->second);
						}
						std::putchar(10);
					}
				}
				pump(120);                      // 離して、声が空くまで待つ
			}
		}
		std::fflush(stdout);
	}
	return 0;
}
