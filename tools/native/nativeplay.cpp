// license:BSD-3-Clause
//
// **SH-2 を止めたまま音を出す**（doc/native-engine.md の段 2）。
//
// やること:
//   1. firmware で起動して、SWP30 を実機と同じ初期状態にする
//   2. CPU を止める（ここから先、firmware は 1 命令も走らない）
//   3. 音色の記録から xg::nv::build_note でレジスタを組み立て、スロットに書いて鳴らす
//   4. WAV に書き出す
//
// 使い方:
//   build/nativeplay.exe <rom ディレクトリ> <出力.wav> [-b msb,lsb,prog] [-n 鍵] [-v 強さ]
//                        [-s 秒] [--firmware]
//   --firmware を付けると、比べるための「firmware に鳴らさせた版」を出す。
#include "compat/platform.h"
#include "mu2000.h"
#include "xg/native_voice.h"
#include "xg/ram.h"
#include "xg/voices.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <algorithm>
#include <cmath>
#include <map>
#include <string>
#include <vector>

namespace {

void put32(std::vector<u8> &v, u32 x)
{ v.push_back(u8(x)); v.push_back(u8(x >> 8)); v.push_back(u8(x >> 16)); v.push_back(u8(x >> 24)); }
void put16(std::vector<u8> &v, u16 x) { v.push_back(u8(x)); v.push_back(u8(x >> 8)); }

bool write_wav(const std::string &path, const std::vector<s16> &pcm, u32 rate)
{
	std::vector<u8> h;
	const u32 bytes = u32(pcm.size()) * 2;
	h.insert(h.end(), { 'R', 'I', 'F', 'F' }); put32(h, 36 + bytes);
	h.insert(h.end(), { 'W', 'A', 'V', 'E', 'f', 'm', 't', ' ' }); put32(h, 16);
	put16(h, 1); put16(h, 2); put32(h, rate); put32(h, rate * 4); put16(h, 4); put16(h, 16);
	h.insert(h.end(), { 'd', 'a', 't', 'a' }); put32(h, bytes);
	std::FILE *f = std::fopen(path.c_str(), "wb");
	if (!f)
		return false;
	std::fwrite(h.data(), 1, h.size(), f);
	std::fwrite(pcm.data(), 2, pcm.size(), f);
	std::fclose(f);
	return true;
}

// スロット 1 つぶんのレジスタを SWP30 へ入れる
void poke_slot(mu2000 &mu, int slot, const xg::nv::slot_regs &r)
{
	for (int i = 0; i < 0x40; i++)
		if (r.write & (u64(1) << i))
			mu.poke_swp(true, u32(slot) * 64 + u32(i), r.v[i]);
}

// まとめて keyon する
void key_on_mask(mu2000 &mu, u64 mask)
{
	const u32 MASK_REG[4] = { 0x1cf, 0x1ce, 0x18f, 0x18e };
	for (int i = 0; i < 4; i++)
		mu.poke_swp(true, MASK_REG[i], u16((mask >> (i * 16)) & 0xffff));
	mu.poke_swp(true, 0x20e, 1);
}

// keyon のマスクを立てて引き金を引く
void key_on(mu2000 &mu, int slot)
{
	const u32 MASK_REG[4] = { 0x1cf, 0x1ce, 0x18f, 0x18e };   // 0-15 / 16-31 / 32-47 / 48-63
	for (int i = 0; i < 4; i++)
		mu.poke_swp(true, MASK_REG[i], (slot >> 4) == i ? u16(1 << (slot & 15)) : 0);
	mu.poke_swp(true, 0x20e, 1);
}

// firmware に 1 音鳴らしてもらって、そのときのレジスタを写し取る。
// 要素が複数ある音色では、鳴ったスロットの数だけ返す（スロット番号の順）
std::vector<xg::nv::voice_cal> take_cals(mu2000 &mu, const u8 *rom, u32 rec,
                                         int note, int vel, u32 rate)
{
	std::vector<xg::nv::voice_cal> out;
	std::map<u32, u16> latest, seen;      // latest = 最後の値、seen = 引き金を引いた瞬間の写し
	u64 mask = 0, keyed = 0;
	mu.set_swp_watch([&](bool master, u32 reg, u16 value) {
		if (!master) return;
		latest[reg] = value;
		switch (reg) {
		case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(value) << 48); break;
		case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(value) << 32); break;
		case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(value) << 16); break;
		case 0x1cf: mask = (mask & ~u64(0xffff)) | value; break;
		// 引き金を引いた瞬間が「鳴らすときの値」。そのあとの書き直しは
		// LFO や包絡線の更新なので、写し取りには使わない
		case 0x20e: keyed |= mask; if (seen.empty()) seen = latest; break;
		default: break;
		}
	});
	s32 l = 0, r = 0;
	for (u8 b : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
		mu.midi_in(b, 0);
	for (u32 i = 0; i < rate / 25; i++)
		mu.run_sample(l, r);
	mu.set_swp_watch(nullptr);
	for (int ch = 0; ch < 64; ch++) {
		if (!(keyed & (u64(1) << ch)))
			continue;
		xg::nv::voice_cal cal;
		// 鳴らす値として何を採るか。
		//   0x05・0x0a・0x11 は LFO の更新で動き続けるので**引き金の瞬間**
		//   ほかは、鳴らしたあとに落ち着いた値（0x01 は鳴らしてから上がっていく）
		for (int i = 0; i < 0x40; i++) {
			const bool at_key = (i == 0x05 || i == 0x0a || i == 0x11);
			const std::map<u32, u16> &src = at_key ? seen : latest;
			const auto it = src.find(u32(ch) * 64 + u32(i));
			if (it != src.end())
				cal.set(i, it->second);
		}
		// **要素ごとに**音量を校正する。そのスロットに書かれた 0x09 が、
		// firmware がその要素に使った減衰そのもの。
		// どの要素かは、そのスロットが鳴らしている波形の番地で見分ける
		const int nel = xg::nv::element_count(rom, rec);
		int idx = -1;
		if (cal.has(0x16) && cal.has(0x17)) {
			const u32 want = u32(cal.reg[0x16]) << 16 | cal.reg[0x17];
			for (int k = 0; k < nel; k++) {
				const u8 *e2 = xg::nv::element(rom, rec, k);
				const u8 *w2 = xg::nv::wave_entry(rom, xg::nv::wave_set(e2), note);
				if (w2 && xg::nv::read_wave(w2).format_addr == want) { idx = k; break; }
			}
		}
		if (idx < 0) {                      // 見分けられなければ順番で
			int cnt = int(out.size());
			idx = 0;
			for (int k = 0; k < nel; k++) {
				const u8 *e2 = xg::nv::element(rom, rec, k);
				if (!xg::nv::element_active(e2, note, vel))
					continue;
				if (cnt-- == 0) { idx = k; break; }
			}
		}
		const u8 *el = xg::nv::element(rom, rec, idx);
		cal.base_level = xg::nv::calibrate_level(rom, el,
		    cal.has(9) ? (cal.reg[9] & 0xff) : mu.nvram()[0x3e96a], note, vel);
		cal.have = true;
		out.push_back(cal);
	}
	return out;
}

// 1 つだけ要るとき
xg::nv::voice_cal take_cal(mu2000 &mu, const u8 *rom, u32 rec,
                           int note, int vel, u32 rate)
{
	const std::vector<xg::nv::voice_cal> v = take_cals(mu, rom, rec, note, vel, rate);
	return v.empty() ? xg::nv::voice_cal() : v[0];
}

} // namespace


int main(int argc, char **argv)
{
	if (argc < 3) {
		std::fprintf(stderr, "nativeplay <rom ディレクトリ> <出力.wav> [-b msb,lsb,prog] [-n 鍵] [-v 強さ] [-s 秒] [--firmware]%c", 10);
		return 1;
	}
	const std::string dir = argv[1], out_path = argv[2];
	int msb = 0, lsb = 0, prog = 0, note = 60, vel = 100, slot = 0;
	double seconds = 2.0;
	bool firmware = false, compare = false, song = false, dump_voice = false, copyall = false;
	int sweep = 0, volsweep = 0, levels = 0;
	bool bench = false, ccwatch = false, ccsweep = false, ccram = false, attsweep = false, cutsweep = false, listvoices = false, ccfilter = false;
	int ccreg = -1;
	int ccbyte = -1;
	int porta = -1;
	int portasweep = 0;
	bool atwatch = false;
	int catoff = -1;
	bool xgmap = false;
	const char *sxsettle = nullptr;
	bool egwatch = false;
	bool slotalloc = false;
	bool levelcheck = false;
	for (int i = 3; i < argc; i++) {
		if (!std::strcmp(argv[i], "-b") && i + 1 < argc)
			std::sscanf(argv[++i], "%d,%d,%d", &msb, &lsb, &prog);
		else if (!std::strcmp(argv[i], "-n") && i + 1 < argc) note = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "-v") && i + 1 < argc) vel = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "-s") && i + 1 < argc) seconds = std::atof(argv[++i]);
		else if (!std::strcmp(argv[i], "--slot") && i + 1 < argc) slot = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--firmware")) firmware = true;
		else if (!std::strcmp(argv[i], "--compare")) compare = true;
		else if (!std::strcmp(argv[i], "--song")) song = true;
		else if (!std::strcmp(argv[i], "--dump-voice")) dump_voice = true;
		else if (!std::strcmp(argv[i], "--copyall")) copyall = true;
		else if (!std::strcmp(argv[i], "--bench")) bench = true;
		else if (!std::strcmp(argv[i], "--ccwatch")) ccwatch = true;
		else if (!std::strcmp(argv[i], "--ccsweep")) ccsweep = true;
		else if (!std::strcmp(argv[i], "--ccram")) ccram = true;
		else if (!std::strcmp(argv[i], "--attsweep")) attsweep = true;
		else if (!std::strcmp(argv[i], "--cutsweep")) cutsweep = true;
		else if (!std::strcmp(argv[i], "--list")) listvoices = true;
		else if (!std::strcmp(argv[i], "--ccfilter")) ccfilter = true;
		else if (!std::strcmp(argv[i], "--ccreg") && i + 1 < argc) ccreg = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--ccbyte") && i + 1 < argc) ccbyte = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--porta") && i + 1 < argc) porta = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--portasweep") && i + 1 < argc) portasweep = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--at")) atwatch = true;
		else if (!std::strcmp(argv[i], "--xgmap")) xgmap = true;
		else if (!std::strcmp(argv[i], "--sxsettle") && i + 1 < argc) sxsettle = argv[++i];
		else if (!std::strcmp(argv[i], "--egwatch")) egwatch = true;
		else if (!std::strcmp(argv[i], "--slotalloc")) slotalloc = true;
		else if (!std::strcmp(argv[i], "--levelcheck")) levelcheck = true;
		else if (!std::strcmp(argv[i], "--catoff") && i + 1 < argc) catoff = int(std::strtol(argv[++i], nullptr, 0));
		else if (!std::strcmp(argv[i], "--sweep") && i + 1 < argc) sweep = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--volsweep") && i + 1 < argc) volsweep = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--levels") && i + 1 < argc) levels = std::atoi(argv[++i]);
	}

	mu2000 mu;
	if (!mu.load_program(dir + "/mu2000_flash.bin") || !mu.load_wave(dir + "/dump")) {
		std::fprintf(stderr, "%s%c", mu.error().c_str(), 10);
		return 1;
	}
	mu.load_sintab(dir + "/standin/sin-table.bin");
	mu.reset();

	const u32 RATE = 44100;
	s32 l = 0, r = 0;
	for (u32 i = 0; i < 30 * RATE && !mu.midi_ready(); i++)
		mu.run_sample(l, r);
	for (u32 i = 0; i < RATE; i++)                     // 起動が落ち着くまで
		mu.run_sample(l, r);

	// 音色を選ぶ。**ここは firmware に頼る**（バンクとプログラムの引き方は
	// xg/voices.h にあるが、パートの塊を作るのは firmware の仕事）
	for (u8 b : { u8(0xb0), u8(0x00), u8(msb & 0x7f), u8(0xb0), u8(0x20), u8(lsb & 0x7f),
	              u8(0xc0), u8(prog & 0x7f) })
		mu.midi_in(b, 0);
	for (u32 i = 0; i < RATE / 2; i++)
		mu.run_sample(l, r);

	const u8 *ram = mu.nvram().data();
	const u32 part0 = xg::ram::part_base(0);
	const u8 *pr = ram + part0;
	const u32 rec = u32(pr[xg::ram::PART_VOICE]) << 24 | u32(pr[xg::ram::PART_VOICE + 1]) << 16 |
	                u32(pr[xg::ram::PART_VOICE + 2]) << 8 | pr[xg::ram::PART_VOICE + 3];
	if (!rec) {
		std::fprintf(stderr, "音色の記録が引けなかった（ドラムか、まだ真似していない組）%c", 10);
		return 1;
	}
	const u8 *rom = mu.program_rom()->data();
	xg::voice_rom vr(mu.program_rom());
	std::printf("音色 %d,%d,%d = %s（記録 %06x、要素 %d）%c", msb, lsb, prog,
	            vr.record_name(rec).c_str(), rec, xg::nv::element_count(rom, rec), 10);

	// --ccwatch: 1 音鳴らしたまま、コントローラを振って
	// 「firmware がどのスロットのどのレジスタを書き替えるか」を見る。
	// 音量・パン・ベンドを native 側で作るための下調べ
	if (ccwatch) {
		// firmware が MIDI を受けてから実際に鳴らすまでの遅れを測る
		{
			bool got_key = false;
			mu.set_swp_watch([&](bool master, u32 r2, u16) {
				if (master && r2 == 0x20e) got_key = true;
			});
			for (u8 bb : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
				mu.midi_in(bb, 0);
			u32 n = 0;
			while (!got_key && n < RATE / 5) { mu.run_sample(l, r); n++; }
			std::printf("MIDI を受けてから鳴るまで %u サンプル（%.2f ms）%c",
			            n, double(n) * 1000.0 / RATE, 10);
			for (u8 bb : { u8(0x80), u8(note & 0x7f), u8(64) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 2; i++)
				mu.run_sample(l, r);
			mu.set_swp_watch(nullptr);
		}
		// SysEx（XG On・エフェクトの種類）のあと、firmware が落ち着くまで何サンプルか
		{
			u32 last = 0, n2 = 0;
			mu.set_swp_watch([&](bool master, u32, u16) { if (master) last = n2; });
			for (u8 bb : { u8(0xf0), u8(0x43), u8(0x10), u8(0x4c), u8(0x00), u8(0x00),
			               u8(0x7e), u8(0x00), u8(0xf7) })
				mu.midi_in(bb, 0);
			for (; n2 < RATE * 2; n2++)
				mu.run_sample(l, r);
			std::printf("XG On のあと SWP30 を触り終わるまで %u サンプル（%.1f ms）%c",
			            last, double(last) * 1000.0 / RATE, 10);
			mu.set_swp_watch(nullptr);
			for (u32 i = 0; i < RATE; i++)
				mu.run_sample(l, r);
			// エフェクトの種類を変える SysEx（リバーブを HALL1 に）
			last = 0; n2 = 0;
			mu.set_swp_watch([&](bool master, u32, u16) { if (master) last = n2; });
			for (u8 bb : { u8(0xf0), u8(0x43), u8(0x10), u8(0x4c), u8(0x02), u8(0x01),
			               u8(0x00), u8(0x10), u8(0x00), u8(0xf7) })
				mu.midi_in(bb, 0);
			for (; n2 < RATE * 2; n2++)
				mu.run_sample(l, r);
			std::printf("リバーブの種類を変えたあと %u サンプル（%.1f ms）%c",
			            last, double(last) * 1000.0 / RATE, 10);
			mu.set_swp_watch(nullptr);
			for (u32 i = 0; i < RATE; i++)
				mu.run_sample(l, r);
			// インサーションの種類を変える（03 00 00 に 41 00 ＝ ディストーション）
			last = 0; n2 = 0;
			mu.set_swp_watch([&](bool master, u32, u16) { if (master) last = n2; });
			for (u8 bb : { u8(0xf0), u8(0x43), u8(0x10), u8(0x4c), u8(0x03), u8(0x00),
			               u8(0x00), u8(0x41), u8(0x00), u8(0xf7) })
				mu.midi_in(bb, 0);
			for (; n2 < RATE * 2; n2++)
				mu.run_sample(l, r);
			std::printf("インサーションを変えたあと %u サンプル（%.1f ms）%c",
			            last, double(last) * 1000.0 / RATE, 10);
			mu.set_swp_watch(nullptr);
		}
		// 音色を選び直すのに firmware が何サンプル要るか
		{
			const std::vector<u8> &wr = mu.nvram();
			auto rec_of = [&]() {
				const u8 *q = wr.data() + part0;
				return u32(q[xg::ram::PART_VOICE]) << 24 | u32(q[xg::ram::PART_VOICE + 1]) << 16 |
				       u32(q[xg::ram::PART_VOICE + 2]) << 8 | q[xg::ram::PART_VOICE + 3];
			};
			for (int pg : { 48, 0 }) {
				const u32 was = rec_of();
				for (u8 bb : { u8(0xc0), u8(pg) })
					mu.midi_in(bb, 0);
				u32 n2 = 0;
				while (rec_of() == was && n2 < RATE) { mu.run_sample(l, r); n2++; }
				std::printf("音色を %d に変えるのに %u サンプル（%.2f ms）%c",
				            pg, n2, double(n2) * 1000.0 / RATE, 10);
				for (u32 i = 0; i < RATE / 10; i++)
					mu.run_sample(l, r);
			}
		}
		// ベンド幅（RPN 0,0）がワーク RAM のどこに入るか
		{
			const std::vector<u8> &wr = mu.nvram();
			std::vector<u8> a0 = wr;
			for (u8 bb : { u8(0xb0), u8(74), u8(100), u8(0xb0), u8(71), u8(77),
			               u8(0xb0), u8(72), u8(55), u8(0xb0), u8(73), u8(33) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 10; i++)
				mu.run_sample(l, r);
			for (size_t a = part0; a < part0 + 0x134 && a < wr.size(); a++)
				if (wr[a] != a0[a])
					std::printf("CC74=100 CC71=77 CC72=55 CC73=33: パートの塊 +%02x  %d -> %d%c",
					            unsigned(a - part0), a0[a], wr[a], 10);
			for (u8 bb : { u8(0xb0), u8(6), u8(2) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 10; i++)
				mu.run_sample(l, r);
		}
		std::map<u32, u16> before, now;
		mu.set_swp_watch([&](bool master, u32 reg, u16 value) {
			if (master) now[reg] = value;
		});
		for (u8 bb : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
			mu.midi_in(bb, 0);
		for (u32 i = 0; i < RATE / 10; i++)
			mu.run_sample(l, r);
		before = now;
		std::FILE *tf = std::fopen("cctrace.txt", "w");
		mu.set_swp_trace(tf);

		struct step { const char *name; u8 a, b1, b2; };
		const step steps[] = {
			{ "CC7=64",   0xb0, 0x07, 64 },  { "CC7=127",  0xb0, 0x07, 127 },
			{ "CC11=64",  0xb0, 0x0b, 64 },  { "CC11=127", 0xb0, 0x0b, 127 },
			{ "CC10=0",   0xb0, 0x0a, 0 },   { "CC10=127", 0xb0, 0x0a, 127 },
			{ "CC10=64",  0xb0, 0x0a, 64 },
			{ "bend+",    0xe0, 0x00, 0x7f },{ "bend0",    0xe0, 0x00, 0x40 },
			{ "CC1=64",   0xb0, 0x01, 64 },  { "CC1=0",    0xb0, 0x01, 0 },
			{ "CC91=0",   0xb0, 0x5b, 0 },   { "CC91=127", 0xb0, 0x5b, 127 },
			{ "CC93=127", 0xb0, 0x5d, 127 }, { "CC93=0",   0xb0, 0x5d, 0 },
			{ "CC94=127", 0xb0, 0x5e, 127 }, { "CC94=0",   0xb0, 0x5e, 0 },
			{ "CC74=0",   0xb0, 0x4a, 0 },   { "CC74=127", 0xb0, 0x4a, 127 },
			{ "CC74=64",  0xb0, 0x4a, 64 },
			{ "CC71=0",   0xb0, 0x47, 0 },   { "CC71=127", 0xb0, 0x47, 127 },
			{ "CC71=64",  0xb0, 0x47, 64 },
			{ "CC72=127", 0xb0, 0x48, 127 }, { "CC73=127", 0xb0, 0x49, 127 },
			{ "AT=100",   0xd0, 100, 0 },
		};
		for (const step &st : steps) {
			std::map<u32, u16> prev = now;
			for (u8 bb : { st.a, st.b1, st.b2 })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 20; i++)
				mu.run_sample(l, r);
			std::printf("%s:", st.name);
			int shown = 0;
			for (const auto &kv : now) {
				const auto it = prev.find(kv.first);
				if (it != prev.end() && it->second == kv.second)
					continue;
				if (kv.first >= 0x400)
					continue;
				std::printf(" [%d.%02x]=%04x", int(kv.first / 64), int(kv.first % 64), kv.second);
				if (++shown >= 14) { std::printf(" ..."); break; }
			}
			std::putchar(10);
		}
		mu.set_swp_watch(nullptr);
		mu.set_swp_trace(nullptr);
		if (tf) std::fclose(tf);
		return 0;
	}

	// --list: 音色ごとに、要素の遅らせ（byte72）を並べる
	if (listvoices) {
		for (int pg = 0; pg < 128; pg++) {
			for (u8 bb : { u8(0xb0), u8(0x00), u8(msb & 0x7f), u8(0xb0), u8(0x20), u8(lsb & 0x7f),
			               u8(0xc0), u8(pg & 0x7f) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 8; i++)
				mu.run_sample(l, r);
			const u8 *pr2 = mu.nvram().data() + part0;
			const u32 rec2 = u32(pr2[xg::ram::PART_VOICE]) << 24 | u32(pr2[xg::ram::PART_VOICE + 1]) << 16 |
			                 u32(pr2[xg::ram::PART_VOICE + 2]) << 8 | pr2[xg::ram::PART_VOICE + 3];
			if (!rec2)
				continue;
			const int n2 = xg::nv::element_count(rom, rec2);
			// こちらで引いた記録と突き合わせる（firmware に頼らずに音色を決められるか）
			const int mode = mu.nvram()[xg::ram::VOICE_MODE];
			const int vset = mu.nvram()[xg::ram::VOICE_SET];
			const u32 mine = vr.lookup(mode, vset, msb, lsb, pg);
			std::printf("V %3d %06x %d%s", pg, rec2, n2,
			            mine == rec2 ? "" : "  ★ちがう ");
			if (mine != rec2)
				std::printf("こちら %06x（mode=%d set=%d）", mine, mode, vset);
			for (int k = 0; k < n2; k++)
				std::printf(" %d", xg::nv::element(rom, rec2, k)[72]);
			std::putchar(10);
		}
		return 0;
	}

	// --ccreg N: その CC を 1..127 まで振って、**鳴らし始めの**スロットのレジスタを
	// 並べる。値の変わったレジスタだけ出す（包絡線が動く前を見たいので毎回鳴らし直す）
	if (ccreg >= 0) {
		std::map<u32, u16> now, seen;
		u64 mask = 0, keyed = 0;
		mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
			if (!master) return;
			now[r2] = v2;
			if (r2 == 0x20e && seen.empty()) seen = now;
			switch (r2) {
			case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
			case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
			case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
			case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
			case 0x20e: keyed |= mask; break;
			default: break;
			}
		});
		std::printf("== CC%d（鍵 %d 強さ %d）%c", ccreg, note, vel, 10);
		for (int v = 1; v <= 127; v += 2) {
			if (ccreg >= 128)                       // 128 以上はチャンネルアフタータッチ
				for (u8 bb : { u8(0xd0), u8(v) })
					mu.midi_in(bb, 0);
			else
				for (u8 bb : { u8(0xb0), u8(ccreg), u8(v) })
					mu.midi_in(bb, 0);
			keyed = 0;
			now.clear();
			seen.clear();
			for (u8 bb : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 50; i++)
				mu.run_sample(l, r);
			for (int ch = 0; ch < 64; ch++)
				if (keyed & (u64(1) << ch)) {
					std::printf("%3d", v);
					for (int rr = 0; rr < 0x40; rr++) {
						const auto it = seen.find(u32(ch) * 64 + u32(rr));
						std::printf(" %04x", it == seen.end() ? 0xffff : it->second);
					}
					std::putchar(10);
					break;
				}
			for (u8 bb : { u8(0x80), u8(note & 0x7f), u8(64), u8(0xb0), u8(0x78), u8(0) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 8; i++)
				mu.run_sample(l, r);
		}
		mu.set_swp_watch(nullptr);
		return 0;
	}

	// --ccfilter: CC74（明るさ）・CC71（レゾナンス）を振って、鳴らし始めの
	// 0x00・0x04 を並べる。包絡線が動く前の値を見たいので、毎回鳴らし直す
	if (ccfilter) {
		std::map<u32, u16> now, seen;
		u64 mask = 0, keyed = 0;
		mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
			if (!master) return;
			now[r2] = v2;
			if (r2 == 0x20e && seen.empty()) seen = now;
			switch (r2) {
			case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
			case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
			case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
			case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
			case 0x20e: keyed |= mask; break;
			default: break;
			}
		});
		for (int which = 1; which >= 0; which--) {
			const u8 cc = which ? 0x47 : 0x4a;
			std::printf("== CC%d%c", int(cc), 10);
			for (int v = 1; v <= 127; v += 2) {
				for (u8 bb : { u8(0xb0), cc, u8(v) })
					mu.midi_in(bb, 0);
				keyed = 0;
				now.clear();
				seen.clear();
				for (u8 bb : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
					mu.midi_in(bb, 0);
				for (u32 i = 0; i < RATE / 50; i++)
					mu.run_sample(l, r);
				for (int ch = 0; ch < 64; ch++)
					if (keyed & (u64(1) << ch)) {
						const auto a0 = seen.find(u32(ch) * 64 + 0);
						const auto a4 = seen.find(u32(ch) * 64 + 4);
						std::printf("%d %04x %04x%c", v,
						            a0 == seen.end() ? 0xffff : a0->second,
						            a4 == seen.end() ? 0xffff : a4->second, 10);
						break;
					}
				for (u8 bb : { u8(0x80), u8(note & 0x7f), u8(64), u8(0xb0), u8(0x78), u8(0) })
					mu.midi_in(bb, 0);
				for (u32 i = 0; i < RATE / 8; i++)
					mu.run_sample(l, r);
			}
			for (u8 bb : { u8(0xb0), cc, u8(64) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 10; i++)
				mu.run_sample(l, r);
		}
		mu.set_swp_watch(nullptr);
		return 0;
	}

	// --cutsweep: 強さを 1 から 127 まで振って、フィルタ（0x00）と共振（0x04）を並べる
	if (cutsweep) {
		std::map<u32, u16> now, seen;
		u64 mask = 0, keyed = 0;
		mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
			if (!master) return;
			now[r2] = v2;
			if (r2 == 0x20e && seen.empty())
				seen = now;
			switch (r2) {
			case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
			case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
			case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
			case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
			case 0x20e: keyed |= mask; break;
			default: break;
			}
		});
		const u8 *el0 = xg::nv::element(rom, rec, 0);
		std::printf("# 表の値 %d（byte35=%d byte36=%d byte37=%d byte38=%d byte39=%d byte40=%d byte41=%d）%c",
		            int(xg::nv::rd16(rom, xg::nv::CUTOFF_TAB + u32(el0[37]) * 2) & 0x7ff),
		            el0[35], el0[36], el0[37], el0[38], el0[39], el0[40], el0[41], 10);
		// 声は 64 しか無く、この試しでは返ってこないので、1 回で見られるのは 64 段まで。
		// 1 つ飛ばしで端から端まで見る
		for (int vv = 1; vv <= 127; vv += 2) {
			keyed = 0;
			now.clear();
			seen.clear();
			for (u8 bb : { u8(0x90), u8(note & 0x7f), u8(vv) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 50; i++)
				mu.run_sample(l, r);
			if (!keyed)
				std::printf("NOKEY %d%c", vv, 10);
			for (int ch = 0; ch < 64; ch++)
				if (keyed & (u64(1) << ch)) {
					// 引き金の瞬間の値（そのあとは包絡線が動かす）
					const auto a0 = seen.find(u32(ch) * 64 + 0);
					const auto a4 = seen.find(u32(ch) * 64 + 4);
					const auto a9 = seen.find(u32(ch) * 64 + 9);
					std::printf("CUT %d %04x %04x %04x ch%d%c", vv,
					            a0 == seen.end() ? 0xffff : a0->second,
					            a4 == seen.end() ? 0xffff : a4->second,
					            a9 == seen.end() ? 0xffff : a9->second, ch, 10);
					break;
				}
			for (u8 bb : { u8(0x80), u8(note & 0x7f), u8(64), u8(0xb0), u8(0x78), u8(0) })
				mu.midi_in(bb, 0);                  // 全部切ってからつぎへ
			// 声が返ってくるまで待つ。まとめて長く待たないと 64 声を使い切る
			for (u32 i = 0; i < RATE / 8; i++)
				mu.run_sample(l, r);
		}
		mu.set_swp_watch(nullptr);
		return 0;
	}

	// --attsweep: 鍵と強さを振って、firmware が 0x09 に入れる減衰を並べ、
	// こちらの式（calibrate_level + volume_att）と突き合わせる
	if (attsweep) {
		std::map<u32, u16> now;
		u64 mask = 0, keyed = 0;
		mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
			if (!master) return;
			now[r2] = v2;
			switch (r2) {
			case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
			case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
			case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
			case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
			case 0x20e: keyed |= mask; break;
			default: break;
			}
		});
		struct one { int note, vel, att, cut, res; };
		std::vector<one> all;
		std::vector<std::pair<int, int>> got;      // 鍵, 減衰
		for (int nn = 12; nn <= 108; nn++) {
		  for (int vv : { 20, 60, vel & 0x7f, 127 }) {
			keyed = 0;
			now.clear();
			for (u8 bb : { u8(0x90), u8(nn), u8(vv) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 50; i++)
				mu.run_sample(l, r);
			for (int ch = 0; ch < 64; ch++)
				if (keyed & (u64(1) << ch)) {
					const auto a9 = now.find(u32(ch) * 64 + 9);
					const auto a0 = now.find(u32(ch) * 64 + 0);
					const auto a4 = now.find(u32(ch) * 64 + 4);
					if (a9 != now.end()) {
						all.push_back({ nn, vv, int(a9->second & 0xff),
						                a0 == now.end() ? -1 : int(a0->second),
						                a4 == now.end() ? -1 : int(a4->second) });
						if (vv == (vel & 0x7f))
							got.push_back({ nn, int(a9->second & 0xff) });
					}
					break;
				}
			for (u8 bb : { u8(0x80), u8(nn), u8(64), u8(0xb0), u8(0x78), u8(0) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 4; i++)
				mu.run_sample(l, r);
		  }
		  {
		  }
		}
		mu.set_swp_watch(nullptr);
		const u8 *el0 = xg::nv::element(rom, rec, 0);
		int ref = -1;
		for (const auto &g : got)
			if (g.first == 60)
				ref = g.second;
		if (ref < 0 && !got.empty())
			ref = got[got.size() / 2].second;
		const int base_lv = xg::nv::calibrate_level(rom, el0, ref, 60, vel);
		std::printf("# 素の音量 = %d（鍵 60・強さ %d の減衰 %d から）%c", base_lv, vel, ref, 10);
		int bad = 0, worst = 0;
		for (const auto &g : got) {
			const int mine = xg::nv::volume_att(rom, el0, base_lv, g.first, vel);
			const int d = mine - g.second;
			if (d) bad++;
			if (std::abs(d) > worst) worst = std::abs(d);
			const u8 *we2 = xg::nv::wave_entry(rom, xg::nv::wave_set(el0), g.first);
			std::printf("ATT %3d  実機 %3d  式 %3d  差 %+d  波形 [0]=%d [1]=%d [3]=%d 曲線=%d%c",
			            g.first, g.second, mine, d, we2 ? we2[0] : -1, we2 ? we2[1] : -1,
			            we2 ? we2[3] : -1, xg::nv::level_key_curve(rom, el0, g.first), 10);
		}
		std::printf("# 合わない鍵 %d / %d、最大の差 %d（%.2f dB）%c",
		            bad, int(got.size()), worst, worst * 0.1875, 10);
		// フィルタ（0x00 の下 11bit）が鍵と強さでどう動くか
		const int tab = int(xg::nv::rd16(rom, xg::nv::CUTOFF_TAB + u32(el0[37]) * 2) & 0x7ff);
		std::printf("# フィルタ: 表の値 %d（byte37=%d, byte35=%d, byte38=%d, byte39=%d）%c",
		            tab, el0[37], el0[35], el0[38], el0[39], 10);
		for (const one &o : all)
			if (o.vel == 100 && o.cut >= 0)
				std::printf("CUT 鍵 %3d 強さ %3d  0x00=%04x 切る高さ %4d（表との差 %+d） 0x04=%04x%c",
				            o.note, o.vel, o.cut, o.cut & 0x7ff, (o.cut & 0x7ff) - tab, o.res, 10);
		return 0;
	}

	// --levelcheck: **音量の式を実機と並べる**。
	// まず鍵 60・強さ 100 で 1 音鳴らして校正し（写し取りと同じ）、
	// そのあと鍵と強さを振って、実機が 0x09 に入れる値とこちらの式を比べる
	if (levelcheck) {
		auto play_att = [&](int nn, int vv) -> int {
			u64 mask = 0, keyed = 0;
			int got = -1;
			std::map<u32, u16> last;
			mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
				if (!master) return;
				switch (r2) {
				case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
				case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
				case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
				case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
				case 0x20e: keyed |= mask; break;
				default:
					if (r2 < 0x1000 && (r2 % 64) == 9)
						last[r2] = v2;
					break;
				}
			});
			for (u8 bb : { u8(0x90), u8(nn & 0x7f), u8(vv & 0x7f) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 20; i++)
				mu.run_sample(l, r);
			for (int ch = 0; ch < 64; ch++)
				if ((keyed & (u64(1) << ch)) && last.count(u32(ch) * 64 + 9)) {
					got = int(last[u32(ch) * 64 + 9] & 0xff);
					break;
				}
			for (u8 bb : { u8(0x80), u8(nn & 0x7f), u8(64) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 20; i++)
				mu.run_sample(l, r);
			mu.set_swp_watch(nullptr);
			return got;
		};
		const u8 *el = xg::nv::element(rom, rec, 0);
		const int ref = play_att(60, 100);
		if (ref < 0) {
			std::fprintf(stderr, "校正の音が鳴らなかった%c", 10);
			return 1;
		}
		const int base = xg::nv::calibrate_level(rom, el, ref, 60, 100);
		std::printf("== 音量の式くらべ（鍵 60・強さ 100 で校正。素の音量 %d）%c", base, 10);
		std::printf("   鍵  強さ   実機   こちら   ずれ   曲線  段の音量  目盛り%c", 10);
		int bad = 0, n = 0;
		for (int nn : { 36, 48, 60, 72, 84, 96 })
			for (int vv : { 20, 60, 100, 127 }) {
				const int got = play_att(nn, vv);
				if (got < 0)
					continue;
				const int mine = xg::nv::volume_att(rom, el, base, nn, vv);
				n++;
				if (got != mine)
					bad++;
				const int cv = xg::nv::level_key_curve(rom, el, nn);
				const int wl = xg::nv::wave_level(rom, el, nn);
				std::printf("  %3d  %3d   %4d   %4d   %+4d %-6s %+4d  %4d  %4d%c", nn, vv, got, mine,
				            mine - got, got == mine ? "" : "← 違う", cv, wl, base + cv, 10);
			}
		std::printf("ずれた点: %d / %d%c", bad, n, 10);
		return 0;
	}

	// --slotalloc: firmware が「どのスロットを使っているか」を持っている場所を
	// ワーク RAM から探す。音を 1 つずつ足していき、そのたびに
	// **鳴っているスロットの 64bit のマスク**と同じ並びが RAM に無いかを見る。
	// 見つかれば、native が使うスロットもそこに立てておけば firmware が避ける
	if (slotalloc) {
		const std::vector<u8> &wr = mu.nvram();
		u64 mask = 0, live = 0;
		mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
			if (!master) return;
			switch (r2) {
			case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
			case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
			case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
			case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
			case 0x20e: live |= mask; break;
			default: break;
			}
		});
		// 候補を全部の位置で持ち、合わなくなったら落としていく
		std::vector<size_t> cand;
		const int STEPS = 10;
		for (int step = 0; step < STEPS; step++) {
			for (u8 bb : { u8(0x90), u8((40 + step * 3) & 0x7f), u8(vel & 0x7f) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 5; i++)
				mu.run_sample(l, r);
			// いま鳴っているスロットの並びを、8 バイトの窓として作る
			u8 want[8];
			for (int b = 0; b < 8; b++)
				want[b] = u8(live >> (b * 8));
			u8 wantr[8];                      // バイトの順が逆のもの
			for (int b = 0; b < 8; b++)
				wantr[b] = want[7 - b];
			if (step == 0) {
				for (size_t o = 0; o + 8 <= wr.size(); o++)
					if (!std::memcmp(&wr[o], want, 8) || !std::memcmp(&wr[o], wantr, 8))
						cand.push_back(o);
			} else {
				std::vector<size_t> keep;
				for (size_t o : cand)
					if (!std::memcmp(&wr[o], want, 8) || !std::memcmp(&wr[o], wantr, 8))
						keep.push_back(o);
				cand.swap(keep);
			}
			std::printf("  %2d 音目: 鳴っているスロット %d 個（%016llx）候補 %zu%c",
			            step + 1, __builtin_popcountll(live),
			            (unsigned long long)live, cand.size(), 10);
			if (cand.empty())
				break;
		}
		mu.set_swp_watch(nullptr);
		std::printf("== 64bit のマスクとして持っていそうな場所%c", 10);
		for (size_t o : cand)
			std::printf("  RAM %08x%c", unsigned(0x400000 + o), 10);
		if (cand.empty())
			std::printf("  見つからなかった（64bit のまとまりでは持っていないらしい）%c", 10);
		for (u8 bb : { u8(0xb0), u8(0x78), u8(0) })
			mu.midi_in(bb, 0);
		return 0;
	}

	// --egwatch: 1 音鳴らして、**包絡線のレジスタ（0x06/0x07/0x08/0x09）を
	// firmware がいつ書くか**を鍵を押した時刻からの相対で出す。
	// 写し取りの窓（鳴り始めてから 5ms）がこれを取り切れているかを見るための口
	if (egwatch) {
		u64 mask = 0, keyed = 0, t = 0, t0 = 0;
		struct ev { u64 t; u8 ch, reg; u16 v; };
		std::vector<ev> log;
		mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
			if (!master) return;
			switch (r2) {
			case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
			case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
			case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
			case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
			case 0x20e: keyed |= mask; if (!t0) t0 = t; break;
			default: {
				const u32 rr = r2 % 64;
				if (r2 < 0x1000 && ((rr >= 0x06 && rr <= 0x09) || rr == 0x00 || rr == 0x04))
					log.push_back({ t, u8(r2 / 64), u8(rr), v2 });
				break;
			}
			}
		});
		for (u8 bb : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
			mu.midi_in(bb, 0);
		for (; t < RATE / 2; t++)
			mu.run_sample(l, r);
		const u64 key_at = t0;
		for (u8 bb : { u8(0x80), u8(note & 0x7f), u8(64) })
			mu.midi_in(bb, 0);
		for (u32 i = 0; i < RATE / 2; i++) { mu.run_sample(l, r); t++; }
		mu.set_swp_watch(nullptr);
		{
			const std::vector<u8> &wr2 = mu.nvram();
			std::printf("== 全体に効く値: 0x4226A8=%d 0x4226BC=%d 0x4226A9=%d%c",
			            wr2[0x226A8], wr2[0x226BC], wr2[0x226A9], 10);
		}
		std::printf("== 包絡線のレジスタを書いた時刻（鍵を押した時点を 0 とする）%c", 10);
		std::printf("   写し取りの窓は「鳴り始めから 5ms」= +5.0ms まで%c", 10);
		int n = 0;
		for (const ev &e : log) {
			if (!(keyed & (u64(1) << e.ch)))
				continue;
			const double ms = (double(s64(e.t)) - double(s64(key_at))) * 1000.0 / RATE;
			std::printf("  %+9.2f ms  スロット%2d  0x%02x = %04x%s%c", ms, int(e.ch),
			            int(e.reg), e.v, ms > 5.0 && ms < 300.0 ? "   ← 窓の外" : "", 10);
			if (++n > 60)
				break;
		}
		std::printf("  書いた回数 %d%c", int(log.size()), 10);
		return 0;
	}

	// --sxsettle hh,mm,ll,dd[,dd...]: その XG パラメータチェンジを送って、
	// **firmware が SWP30 を触り終わるまで**の時間を測る。
	// 「種類を変える」と「値だけ変える」で桁が違うかを見るための口。
	// 毎サンプル書き替わるレジスタ（MEG の戻りのミキサ 0x38-0x3F と 0x0E/0x0F）は
	// 数えない。数えると永久に落ち着かない
	if (sxsettle) {
		std::vector<u8> body;
		for (const char *p = sxsettle; *p; ) {
			body.push_back(u8(std::strtol(p, nullptr, 16)));
			const char *c = std::strchr(p, ',');
			if (!c)
				break;
			p = c + 1;
		}
		if (body.size() < 4) {
			std::fprintf(stderr, "--sxsettle は hh,mm,ll,dd の形で（16 進）%c", 10);
			return 1;
		}
		u32 last = 0, n2 = 0;
		mu.set_swp_watch([&](bool master, u32 r2, u16) {
			if (!master)
				return;
			const u32 rr = r2 % 64;
			if (r2 < 0x1000 && (rr == 0x0e || rr == 0x0f || (rr >= 0x38 && rr <= 0x3f)))
				return;              // 毎サンプル書き替わるので数えない
			last = n2;
		});
		std::vector<u8> msg = { 0xf0, 0x43, 0x10, 0x4c };
		u32 sum = 0;
		for (u8 b : body) { msg.push_back(b); sum += b; }
		msg.push_back(u8((0x80 - (sum & 0x7f)) & 0x7f));
		msg.push_back(0xf7);
		for (u8 b : msg)
			mu.midi_in(b, 0);
		for (; n2 < RATE * 2; n2++)
			mu.run_sample(l, r);
		mu.set_swp_watch(nullptr);
		std::printf("== 08 xx …: ");
		for (u8 b : body)
			std::printf("%02X ", b);
		std::printf("%c   SWP30 を触り終わるまで %u サンプル（%.1f ms）%c",
		            10, last, double(last) * 1000.0 / RATE, 10);
		return 0;
	}

	// --xgmap: XG のパート parameter（08 pp rr）を 1 つずつ書いて、
	// **ワーク RAM のどのバイトに入るか**と、書く前の値（＝既定）を出す。
	// どの設定がどこにあるか分からないと、写し取りの「経路の印」に何を
	// 混ぜればよいかが決められない
	if (xgmap) {
		const std::vector<u8> &wr = mu.nvram();
		std::printf("== XG のパート parameter → ワーク RAM（パートの塊からの位置）%c", 10);
		for (int o = 0; o <= 0x7f; o++) {
			const std::vector<u8> prev = wr;
			const u8 body[4] = { 0x08, 0x00, u8(o), 0x7f };
			u32 sum = 0;
			for (u8 x : body) sum += x;
			for (u8 bb : { u8(0xf0), u8(0x43), u8(0x10), u8(0x4c), body[0], body[1],
			               body[2], body[3], u8((0x80 - (sum & 0x7f)) & 0x7f), u8(0xf7) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 20; i++)
				mu.run_sample(l, r);
			std::printf("  08 pp %02x →", o);
			int n = 0;
			for (u32 q = 0; q < 0x100; q++)
				if (wr[part0 + q] != prev[part0 + q] && ++n <= 4)
					std::printf(" +0x%02x（前 %d → 後 %d）", q, prev[part0 + q], wr[part0 + q]);
			if (!n)
				std::printf(" 効かない");
			std::printf("%c", 10);
		}
		return 0;
	}

	// --at: 音を鳴らしたまま**アフタータッチ**（触れた強さ）を振って、
	// firmware がどのレジスタ・どのワーク RAM を書き替えるかを見る
	if (atwatch) {
		std::map<u32, u16> now;
		u64 mask = 0, keyed = 0;
		mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
			if (!master) return;
			now[r2] = v2;
			switch (r2) {
			case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
			case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
			case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
			case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
			case 0x20e: keyed |= mask; break;
			default: break;
			}
		});
		for (u8 bb : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
			mu.midi_in(bb, 0);
		for (u32 i = 0; i < RATE / 5; i++)
			mu.run_sample(l, r);
		int ch = 0;
		for (int c2 = 0; c2 < 64; c2++)
			if (keyed & (u64(1) << c2)) { ch = c2; break; }
		const std::vector<u8> &wr = mu.nvram();
		// **行き来する順**で振る。包絡線がひとりでに減っているだけなら単調に動く
		const int vals[] = { 0, 127, 0, 127, 0, 64 };
		std::vector<std::map<u32, u16>> rs;
		std::vector<std::vector<u8>> ms;
		for (int v : vals) {
			for (u8 bb : { u8(0xd0), u8(v & 0x7f) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 10; i++)
				mu.run_sample(l, r);
			rs.push_back(now);
			ms.push_back(wr);
		}
		mu.set_swp_watch(nullptr);
		std::printf("== アフタータッチ（スロット %d）%c", ch, 10);
		int found = 0;
		for (int rr = 0; rr < 0x40; rr++) {
			const u32 key = u32(ch) * 64 + u32(rr);
			bool same = true;
			for (size_t k = 1; k < rs.size(); k++) {
				const auto a = rs[k].find(key), b = rs[0].find(key);
				if ((a == rs[k].end()) != (b == rs[0].end()) ||
				    (a != rs[k].end() && a->second != b->second))
					same = false;
			}
			if (same) continue;
			std::printf("  レジスタ 0x%02x :", rr);
			for (size_t k = 0; k < rs.size(); k++) {
				const auto a = rs[k].find(key);
				std::printf(" %d=%04x", vals[k], a == rs[k].end() ? 0xffff : a->second);
			}
			std::printf("%c", 10);
			found++;
		}
		for (u32 o = 0; o < 0x100; o++) {
			bool same = true;
			for (size_t k = 1; k < ms.size(); k++)
				if (ms[k][part0 + o] != ms[0][part0 + o]) same = false;
			if (same) continue;
			std::printf("  パート+0x%02x :", o);
			for (size_t k = 0; k < ms.size(); k++)
				std::printf(" %d=%d", vals[k], ms[k][part0 + o]);
			std::printf("%c", 10);
			found++;
		}
		if (!found)
			std::printf("  何も動かなかった%c", 10);
		// パートの塊の 0x29-0x34（XG の CAT・PAT コントロール）を出す
		std::printf("  パート+0x29-0x34:");
		for (u32 o = 0x29; o <= 0x34; o++)
			std::printf(" %d", wr[part0 + o]);
		std::printf("%c", 10);
		// **CAT フィルタコントロール**（08 pp 2A）を既定から外して、もう一度振る
		for (u8 bb : { u8(0x80), u8(note & 0x7f), u8(64) })
			mu.midi_in(bb, 0);
		for (u32 i = 0; i < RATE / 4; i++) mu.run_sample(l, r);
		// XG のパラメータは **チェックサムが要る**。08 pp <o> <v>
		const std::vector<u8> prev = wr;
		auto xgset = [&](u8 o, u8 v) {
			const u8 body[4] = { 0x08, 0x00, o, v };
			u32 sum = 0;
			for (u8 x : body) sum += x;
			const u8 ck = u8((0x80 - (sum & 0x7f)) & 0x7f);
			for (u8 bb : { u8(0xf0), u8(0x43), u8(0x10), u8(0x4c),
			               body[0], body[1], body[2], body[3], ck, u8(0xf7) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 4; i++) mu.run_sample(l, r);
		};
		// CAT の 6 つ（XG 08 pp 30-35）をまとめて極端にする。
		// --catoff を渡したときはその 1 つだけ 0 にする
		if (catoff >= 0) {
			xgset(u8(catoff), 0);
		} else {
			// CAT は 08 pp 4D-52（ワーク RAM のパート +0x46-0x4B）。--xgmap で見つけた
			xgset(0x4d, 0x58);   // CAT ピッチ（最大に上げる）
			xgset(0x4e, 0x00);   // CAT フィルタ（最大に下げる）
			xgset(0x4f, 0x00);   // CAT アンプ（最大に下げる）
			xgset(0x50, 0x7f);   // CAT LFO PMOD
			xgset(0x51, 0x7f);   // CAT LFO FMOD
			xgset(0x52, 0x7f);   // CAT LFO AMOD
		}
		std::printf("  上の書き込みでパートの塊が動いたバイト:");
		for (size_t o = 0; o < wr.size(); o++)
			if (wr[o] != prev[o]) {
				if (o >= part0 && o < part0 + 0x100)
					std::printf(" +0x%02x(%d)", unsigned(o - part0), wr[o]);
			}
		std::printf("%c", 10);
		now.clear(); keyed = 0; mask = 0;
		mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
			if (!master) return;
			now[r2] = v2;
			switch (r2) {
			case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
			case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
			case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
			case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
			case 0x20e: keyed |= mask; break;
			default: break;
			}
		});
		for (u8 bb : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
			mu.midi_in(bb, 0);
		for (u32 i = 0; i < RATE / 5; i++) mu.run_sample(l, r);
		int ch2 = 0;
		for (int c2 = 0; c2 < 64; c2++)
			if (keyed & (u64(1) << c2)) { ch2 = c2; break; }
		std::printf("== CAT フィルタコントロール = 0 にしてから（スロット %d）%c", ch2, 10);
		for (int v : vals) {
			for (u8 bb : { u8(0xd0), u8(v & 0x7f) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 10; i++) mu.run_sample(l, r);
			const auto it0 = now.find(u32(ch2) * 64 + 0x00);
			const auto it9 = now.find(u32(ch2) * 64 + 0x09);
			std::printf("  AT=%3d  0x00=%04x  0x09=%04x  0x11=%04x%c", v,
			            it0 == now.end() ? 0xffff : it0->second,
			            it9 == now.end() ? 0xffff : it9->second,
			            now.count(u32(ch2) * 64 + 0x11) ? now[u32(ch2) * 64 + 0x11] : 0xffff, 10);
		}
		mu.set_swp_watch(nullptr);
		for (u8 bb : { u8(0xd0), u8(0), u8(0x80), u8(note & 0x7f), u8(64) })
			mu.midi_in(bb, 0);
		return 0;
	}

	// --portasweep S: CC5 を S 刻みで振って、**滑る速さ**（10ms の刻みあたり
	// いくつ音程のレジスタが動くか）を出す。ROM の表を探す材料
	if (portasweep > 0) {
		std::printf("== ポルタメントの速さ（鍵 24 → %d、CC5 %d 刻み）%c", note, portasweep, 10);
		for (int cc5 = 0; cc5 <= 127; cc5 += portasweep) {
			u64 mask = 0, keyed = 0, t = 0;
			struct plog { u64 t; u16 v; u8 ch; };
			std::vector<plog> log;
			mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
				if (!master) return;
				switch (r2) {
				case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
				case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
				case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
				case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
				case 0x20e: keyed |= mask; break;
				default:
					if ((r2 % 64) == 0x11 && r2 < 0x1000 && (keyed & (u64(1) << (r2 / 64))))
						log.push_back({ t, v2, u8(r2 / 64) });
					break;
				}
			});
			for (u8 bb : { u8(0xb0), u8(0x41), u8(127), u8(0xb0), u8(0x05), u8(cc5 & 0x7f) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 10; i++) { mu.run_sample(l, r); t++; }
			for (u8 bb : { u8(0x90), u8(24), u8(vel & 0x7f) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 4; i++) { mu.run_sample(l, r); t++; }
			const size_t before = log.size();
			for (u8 bb : { u8(0x80), u8(24), u8(64), u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE * 4; i++) { mu.run_sample(l, r); t++; }
			mu.set_swp_watch(nullptr);
			// いちばん後に鳴り始めたスロットの、上がっていく所だけを見る
			const u8 want = log.empty() ? 0 : log.back().ch;
			u64 t0 = 0, t1 = 0;
			int v0 = -1, v1 = -1;
			for (size_t i = before ? before - 1 : 0; i < log.size(); i++) {
				if (log[i].ch != want)
					continue;
				if (v0 < 0) { v0 = log[i].v; t0 = log[i].t; }
				if (int(log[i].v) != v1) { v1 = log[i].v; t1 = log[i].t; }
			}
			const double ms = double(t1 - t0) * 1000.0 / RATE;
			std::printf("  CC5=%3d  %5d → %5d（%+5d）を %8.1f ms  刻みあたり %8.3f%c",
			            cc5, v0, v1, v1 - v0, ms,
			            ms > 0 ? double(v1 - v0) * 10.0 / ms : 0.0, 10);
			for (u8 bb : { u8(0x80), u8(note & 0x7f), u8(64), u8(0xb0), u8(0x78), u8(0),
			               u8(0xb0), u8(0x41), u8(0) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 4; i++) { mu.run_sample(l, r); t++; }
		}
		return 0;
	}

	// --porta N: ポルタメント（CC65 入・CC5 が速さ）で、firmware が音程の
	// レジスタ 0x11 をどう動かすかを時刻つきで出す。N は CC5 の値。
	// 低い音を鳴らしたまま高い音を鳴らし、滑っていく間の 0x11 を全部並べる
	if (porta >= 0) {
		u64 mask = 0, keyed = 0;
		u64 t = 0, t0 = 0;
		struct plog { u64 t; u16 v; u8 ch; };
		std::vector<plog> log;
		mu.set_swp_watch([&](bool master, u32 r2, u16 v2) {
			if (!master) return;
			switch (r2) {
			case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(v2) << 48); break;
			case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(v2) << 32); break;
			case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(v2) << 16); break;
			case 0x1cf: mask = (mask & ~u64(0xffff)) | v2; break;
			case 0x20e: keyed |= mask; if (!t0) t0 = t; break;
			default:
				if ((r2 % 64) == 0x11 && r2 < 0x1000 && (keyed & (u64(1) << (r2 / 64))))
					log.push_back({ t, v2, u8(r2 / 64) });
				break;
			}
		});
		for (u8 bb : { u8(0xb0), u8(0x41), u8(127), u8(0xb0), u8(0x05), u8(porta & 0x7f) })
			mu.midi_in(bb, 0);
		for (u32 i = 0; i < RATE / 4; i++) { mu.run_sample(l, r); t++; }
		// 1 音目（低い方）。ここは滑らない
		for (u8 bb : { u8(0x90), u8(48), u8(vel & 0x7f) })
			mu.midi_in(bb, 0);
		for (u32 i = 0; i < RATE / 2; i++) { mu.run_sample(l, r); t++; }
		const size_t before = log.size();
		const u64 mark = t;
		// 2 音目（12 半音上）。ここから滑る
		for (u8 bb : { u8(0x80), u8(48), u8(64), u8(0x90), u8(60), u8(vel & 0x7f) })
			mu.midi_in(bb, 0);
		for (u32 i = 0; i < RATE * 3; i++) { mu.run_sample(l, r); t++; }
		mu.set_swp_watch(nullptr);
		std::printf("== ポルタメント CC5=%d（鍵 48 → 60）%c", porta, 10);
		// **いちばん最後に鳴り始めたスロット**だけを見る（前の音の尾が混ざるので）
		u8 want = log.empty() ? 0 : log.back().ch;
		int last = -1, prev = -1, n = 0;
		for (size_t i = before ? before - 1 : 0; i < log.size(); i++) {
			if (log[i].ch != want || int(log[i].v) == last)
				continue;
			prev = last;
			last = int(log[i].v);
			std::printf("  %+8.2f ms  スロット%2d  0x11=%04x (%5d)  差 %+d%c",
			            double(s64(log[i].t) - s64(mark)) * 1000.0 / RATE,
			            int(want), unsigned(last), last, prev < 0 ? 0 : last - prev, 10);
			if (++n > 400)
				break;
		}
		std::printf("  段の数 %d%c", n, 10);
		for (u8 bb : { u8(0x80), u8(60), u8(64), u8(0xb0), u8(0x41), u8(0), u8(0xb0), u8(0x78), u8(0) })
			mu.midi_in(bb, 0);
		return 0;
	}

	// --ccbyte N: その CC を振って、**パートの塊のどのバイトが動くか**を出す。
	// 動くバイトが分かれば、写し取りの「経路の印」にそのバイトを混ぜるだけで、
	// つまみが変わったときに写し取りを取り直せる（式を起こさなくて済む）
	if (ccbyte >= 0) {
		const std::vector<u8> &wr = mu.nvram();
		const int ccs[] = { 0, 32, 64, 96, 127 };
		std::vector<std::vector<u8>> snap;
		for (int cc : ccs) {
			for (u8 bb : { u8(0xb0), u8(ccbyte & 0x7f), u8(cc) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 10; i++)
				mu.run_sample(l, r);
			snap.push_back(wr);
		}
		std::printf("== CC%d で動くワーク RAM のバイト%c", ccbyte, 10);
		int shown = 0;
		// **パートの塊を先に出す**（ほかの所は毎サンプル動く雑音が多く、
		// 上限をそれで使い切ってしまう）
		for (u32 q = 0; q < 0x200; q++) {
			bool same = true;
			for (size_t k = 1; k < snap.size(); k++)
				if (snap[k][part0 + q] != snap[0][part0 + q])
					same = false;
			if (same)
				continue;
			std::printf("  パート+0x%02x :", q);
			for (size_t k = 0; k < snap.size(); k++)
				std::printf(" cc%d=%d", ccs[k], snap[k][part0 + q]);
			std::printf("%c", 10);
			shown++;
		}
		for (size_t o = 0; o < wr.size() && shown < 40; o++) {
			if (o >= part0 && o < part0 + 0x200)
				continue;
			bool same = true;
			for (size_t k = 1; k < snap.size(); k++)
				if (snap[k][o] != snap[0][o])
					same = false;
			if (same)
				continue;
			if (o >= part0 && o < part0 + 0x100)
				std::printf("  パート+0x%02x :", unsigned(o - part0));
			else
				std::printf("  RAM %08x  :", unsigned(0x400000 + o));
			for (size_t k = 0; k < snap.size(); k++)
				std::printf(" cc%d=%d", ccs[k], snap[k][o]);
			std::printf("%c", 10);
			shown++;
		}
		return 0;
	}

	// --ccram: CC7 を振って、ワーク RAM のどのバイトが「その音量の減衰」を
	// 持っているかを探す。見つかれば、native 側はそれを読むだけで済む
	if (ccram) {
		const std::vector<u8> &wr = mu.nvram();
		const size_t N = wr.size();
		std::vector<std::vector<u8>> snap;
		const int ccs[] = { 127, 100, 64, 32, 16 };
		for (int cc : ccs) {
			for (u8 bb : { u8(0xb0), u8(0x07), u8(cc) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 20; i++)
				mu.run_sample(l, r);
			snap.push_back(wr);
		}
		// 期待する差（0x09 の下位バイトの実測から）
		const int want[] = { 0, 12, 32, 66, 94 };   // cc=127 を 0 とした差（2 倍の単位）
		int found = 0;
		for (size_t a = 0; a < N && found < 40; a++) {
			bool ok8 = true, ok8h = true;
			for (int k = 1; k < 5; k++) {
				const int d = int(snap[k][a]) - int(snap[0][a]);
				if (d != want[k]) ok8 = false;
				if (d != want[k] / 2) ok8h = false;
			}
			if (ok8 || ok8h) {
				std::printf("RAM %08x  %s  値 %d %d %d %d %d%c",
				            unsigned(0x400000 + a), ok8 ? "2倍単位" : "そのまま",
				            snap[0][a], snap[1][a], snap[2][a], snap[3][a], snap[4][a], 10);
				found++;
			}
		}
		// 16bit でも
		for (size_t a = 0; a + 1 < N && found < 80; a += 2) {
			auto g = [&](int k) { return int(snap[k][a]) << 8 | snap[k][a + 1]; };
			bool ok = true;
			for (int k = 1; k < 5; k++)
				if (g(k) - g(0) != want[k]) ok = false;
			if (ok) {
				std::printf("RAM %08x 16bit  値 %d %d %d %d %d%c", unsigned(0x400000 + a),
				            g(0), g(1), g(2), g(3), g(4), 10);
				found++;
			}
		}
		std::printf("見つかった数 %d%c", found, 10);
		for (size_t k = 0; k < snap.size(); k++)
			std::printf("CC7=%3d  +0b=%3d +0e=%3d +7e=%3d%c", ccs[k],
			            snap[k][part0 + 0x0b], snap[k][part0 + 0x0e],
			            snap[k][part0 + 0x7e], 10);
		return 0;
	}

	// --ccsweep: コントローラを 0..127 まで振って、対応するレジスタの値を並べる。
	// 音量・パン・ベンド・モジュレーションの式を起こすため
	if (ccsweep) {
		std::map<u32, u16> now;
		mu.set_swp_watch([&](bool master, u32 reg, u16 value) {
			if (master) now[reg] = value;
		});
		for (u8 bb : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
			mu.midi_in(bb, 0);
		for (u32 i = 0; i < RATE / 10; i++)
			mu.run_sample(l, r);

		struct item { const char *name; u8 cc; u32 reg; };
		const item items[] = {
			{ "CC7",  0x07, 0x09 }, { "CC11", 0x0b, 0x09 }, { "CC10", 0x0a, 0x32 },
			{ "CC1",  0x01, 0x0a }, { "CC91", 0x5b, 0x33 }, { "CC93", 0x5d, 0x34 },
			{ "CC94", 0x5e, 0x35 },
		};
		for (const item &it : items) {
			std::printf("== %s -> [%02x]%c", it.name, int(it.reg), 10);
			for (int v = 0; v < 128; v++) {
				for (u8 bb : { u8(0xb0), it.cc, u8(v) })
					mu.midi_in(bb, 0);
				for (u32 i = 0; i < RATE / 50; i++)
					mu.run_sample(l, r);
				std::printf("%d %04x%c", v, now[it.reg], 10);
			}
			for (u8 bb : { u8(0xb0), it.cc, u8(it.cc == 0x0a ? 64 : (it.cc == 0x07 ? 100 : (it.cc == 0x0b ? 127 : 0))) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 20; i++)
				mu.run_sample(l, r);
		}
		// ベンドは 14bit なので別に
		std::printf("== BEND -> [11]%c", 10);
		for (int v = 0; v <= 127; v++) {
			for (u8 bb : { u8(0xe0), u8(0), u8(v) })
				mu.midi_in(bb, 0);
			for (u32 i = 0; i < RATE / 50; i++)
				mu.run_sample(l, r);
			std::printf("%d %04x%c", v, now[0x11], 10);
		}
		mu.set_swp_watch(nullptr);
		return 0;
	}

	// --dump-voice: firmware に 1 音鳴らさせて、声の構造体をワーク RAM から探す。
	// 音量の式の残り（+119・+120・level）を目で見るため
	if (dump_voice) {
		for (u8 b : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
			mu.midi_in(b, 0);
		for (u32 i = 0; i < RATE / 50; i++)
			mu.run_sample(l, r);
		const std::vector<u8> &wr = mu.nvram();
		const int att = wr[0x3e96a];
		const int b119 = xg::nv::velocity_att(rom, vel);
		std::printf("ram[0x43E96A] = %d（att）。強さのぶん = %d、att/2 - それ = %d%c",
		            att, b119, att / 2 - b119, 10);
		int shown = 0;
		for (size_t base = 0; base + 130 < wr.size() && shown < 12; base++) {
			if (wr[base + 119] != b119)
				continue;
			// 声の塊らしさ: +118 が 1-127、+120 が 0-63、そして
			// +32 と +44 が「ワーク RAM か ROM を指すポインタ」であること
			if (wr[base + 118] == 0 || wr[base + 118] > 127 || wr[base + 120] > 63)
				continue;
			auto ptr = [&](size_t o) {
				return u32(wr[base + o]) << 24 | u32(wr[base + o + 1]) << 16 |
				       u32(wr[base + o + 2]) << 8 | wr[base + o + 3];
			};
			const u32 p32 = ptr(32), p44 = ptr(44);
			auto plausible = [](u32 a) {
				return (a >= 0x400000 && a < 0x440000) || (a >= 0x200000 && a < 0x400000);
			};
			if (!plausible(p32) || !plausible(p44))
				continue;
			std::printf("  +32=%08x +44=%08x%c", p32, p44, 10);
			std::printf("  候補 %06zx: +117=%3d +118=%3d +119=%3d +120=%3d +121=%3d  |  +2=%3d +29=%3d%c",
			            0x400000 + base, wr[base + 117], wr[base + 118], wr[base + 119],
			            wr[base + 120], wr[base + 121], wr[base + 2], wr[base + 29], 10);
			shown++;
		}
		return 0;
	}

	// --bench: 「firmware を走らせたまま」と「SH-2 を止めて」で、
	// 1 サンプルを作るのに何ナノ秒かかるかを測る。これが軽量化の答え
	if (bench) {
		const u8 *elem0 = xg::nv::element(rom, rec, 0);
		const int nelem = xg::nv::element_count(rom, rec);
		const std::vector<u8> base = mu.save_state();
		const u32 SECS = 6;
		auto run = [&](bool cpu_on, int voices) {
			std::string e2;
			mu.load_state(base.data(), base.size(), e2);
			mu.set_cpu_enabled(true);
			// 声を鳴らす
			if (cpu_on) {
				for (int v = 0; v < voices; v++)
					for (u8 b : { u8(0x90), u8(48 + v * 2), u8(vel & 0x7f) })
						mu.midi_in(b, 0);
				for (u32 i = 0; i < RATE / 10; i++) { s32 a = 0, b2 = 0; mu.run_sample(a, b2); }
			} else {
				const std::vector<xg::nv::voice_cal> cs = take_cals(mu, rom, rec, 60, vel, RATE);
				mu.load_state(base.data(), base.size(), e2);
				mu.set_cpu_enabled(false);
				u64 km = 0;
				int slot = 0;
				for (int v = 0; v < voices; v++) {
					for (int k = 0; k < nelem && slot < 60; k++) {
						const u8 *el = xg::nv::element(rom, rec, k);
						if (!xg::nv::element_active(el, 48 + v * 2, vel))
							continue;
						const xg::nv::voice_cal *c = cs.empty() ? nullptr : &cs[std::min(cs.size() - 1, size_t(k))];
						const int att = xg::nv::volume_att(rom, el, c ? c->base_level : 64, 48 + v * 2, vel);
						poke_slot(mu, slot, xg::nv::build_note(rom, el, 48 + v * 2, att, c));
						km |= u64(1) << slot;
						slot++;
					}
				}
				key_on_mask(mu, km);
			}
			const u64 t0 = smu2000::perf_ticks();
			for (u32 i = 0; i < SECS * RATE; i++) { s32 a = 0, b2 = 0; mu.run_sample(a, b2); }
			const u64 t1 = smu2000::perf_ticks();
			mu.set_cpu_enabled(true);
			return 1e9 * double(t1 - t0) / double(smu2000::perf_freq()) / double(SECS * RATE);
		};
		std::printf("%c声の数   firmware あり   SH-2 なし   速さの比%c", 10, 10);
		for (int v : { 1, 8, 16, 32 }) {
			const double a = run(true, v);
			const double b = run(false, v);
			std::printf("%6d %13.0f ns %10.0f ns %8.2f 倍%c", v, a, b, a / b, 10);
			std::fflush(stdout);
		}
		return 0;
	}

	// --levels: 音色ごとに「firmware で鳴らした音」と「SH-2 なしで鳴らした音」の
	// 大きさを比べる。音量の式の残りが、実際どれだけ効くかを見る
	if (levels) {
		std::printf("# prog  実機rms  SH-2なしrms    差dB  波形の残差dB%c", 10);
		double worst = 0.0, sum = 0.0, rsum = 0.0;
		int n = 0;
		for (int pg = 0; pg < levels; pg++) {
			for (u8 b : { u8(0xb0), u8(0x00), u8(msb & 0x7f), u8(0xb0), u8(0x20), u8(lsb & 0x7f),
			              u8(0xc0), u8(pg & 0x7f) })
				mu.midi_in(b, 0);
			for (u32 i = 0; i < RATE / 4; i++)
				mu.run_sample(l, r);
			const u8 *pr2 = mu.nvram().data() + part0;
			const u32 rec2 = u32(pr2[xg::ram::PART_VOICE]) << 24 | u32(pr2[xg::ram::PART_VOICE + 1]) << 16 |
			                 u32(pr2[xg::ram::PART_VOICE + 2]) << 8 | pr2[xg::ram::PART_VOICE + 3];
			if (!rec2)
				continue;
			const int nelem = xg::nv::element_count(rom, rec2);
			// **写し取り**: firmware に 1 回鳴らしてもらって、その音色の癖を覚える
			const std::vector<u8> before = mu.save_state();
			const std::vector<xg::nv::voice_cal> cals =
			    take_cals(mu, rom, rec2, note, vel, RATE);
			std::string err0;
			if (!mu.load_state(before.data(), before.size(), err0)) { std::fprintf(stderr, "%s%c", err0.c_str(), 10); return 1; }

			// firmware で鳴らす
			const std::vector<u8> saved = mu.save_state();
			for (u8 b : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
				mu.midi_in(b, 0);
			double fw = 0.0;
			std::vector<double> fw_pcm, nv_pcm;
			fw_pcm.reserve(RATE / 4);
			for (u32 i = 0; i < RATE / 4; i++) {
				s32 li = 0, ri = 0;
				mu.run_sample(li, ri);
				fw_pcm.push_back(0.5 * (double(li) + double(ri)));
				if (i > RATE / 20) fw += double(li) * li + double(ri) * ri;
			}
			fw = std::sqrt(fw / double(RATE / 4 - RATE / 20) / 2);
			// 同じところへ戻して、SH-2 なしで鳴らす
			std::string err2;
			if (!mu.load_state(saved.data(), saved.size(), err2)) { std::fprintf(stderr, "%s%c", err2.c_str(), 10); return 1; }
			mu.set_cpu_enabled(false);
			u64 km = 0;
			int used = 0;
			for (int k = 0; k < nelem; k++) {
				const u8 *el = xg::nv::element(rom, rec2, k);
				if (!xg::nv::element_active(el, note, vel))
					continue;
				const u8 *we = xg::nv::wave_entry(rom, xg::nv::wave_set(el), note);
				const xg::nv::voice_cal *c = we ? xg::nv::match_cal(cals, xg::nv::read_wave(we).format_addr) : nullptr;
				if (!c && size_t(used) < cals.size())
					c = &cals[used];
				const int att = xg::nv::volume_att(rom, el, c ? c->base_level : 64, note, vel);
				poke_slot(mu, used, xg::nv::build_note(rom, el, note, att, c));
				km |= u64(1) << used;
				used++;
			}
			if (!km)
				continue;
			key_on_mask(mu, km);
			double nv = 0.0;
			nv_pcm.reserve(RATE / 4);
			for (u32 i = 0; i < RATE / 4; i++) {
				s32 li = 0, ri = 0;
				mu.run_sample(li, ri);
				nv_pcm.push_back(0.5 * (double(li) + double(ri)));
				if (i > RATE / 20) nv += double(li) * li + double(ri) * ri;
			}
			nv = std::sqrt(nv / double(RATE / 4 - RATE / 20) / 2);
			// 波形そのものも比べる（時間をずらして一番合う所で）
			double resid = 0.0;
			{
				int bshift = 0;
				double bbest = -1e30;
				for (int sh = -400; sh <= 400; sh++) {
					double acc = 0.0;
					for (size_t i = 3000; i + 400 < fw_pcm.size() && i < 20000; i += 5) {
						const size_t j = size_t(int(i) + sh);
						if (j < nv_pcm.size()) acc += fw_pcm[i] * nv_pcm[j];
					}
					if (acc > bbest) { bbest = acc; bshift = sh; }
				}
				double dsum = 0.0, asum = 0.0;
				size_t cnt = 0;
				for (size_t i = 3000; i + 400 < fw_pcm.size(); i++) {
					const size_t j = size_t(int(i) + bshift);
					if (j >= nv_pcm.size()) break;
					const double d = fw_pcm[i] - nv_pcm[j];
					dsum += d * d; asum += fw_pcm[i] * fw_pcm[i]; cnt++;
				}
				if (cnt && asum > 0.0)
					resid = 20.0 * std::log10(std::sqrt(dsum / double(cnt)) /
					                          std::sqrt(asum / double(cnt)));
			}
			mu.set_cpu_enabled(true);
			if (!mu.load_state(saved.data(), saved.size(), err2)) { std::fprintf(stderr, "%s%c", err2.c_str(), 10); return 1; }
			if (fw > 1.0 && nv > 1.0) {
				const double db = 20.0 * std::log10(nv / fw);
				std::printf("%5d %9.1f %11.1f %+7.2f %+9.1f  要素%d 実機%zu こちら%d%c",
				            pg, fw, nv, db, resid, nelem, cals.size(), used, 10);
				worst = std::max(worst, std::fabs(db));
				sum += std::fabs(db);
				rsum += resid;
				n++;
			}
			std::fflush(stdout);
		}
		if (n)
			std::printf("%c大きさの差: 平均 %.2fdB・最大 %.2fdB / 波形の残差: 平均 %.1fdB（%d 音色）%c",
			            10, sum / n, worst, rsum / n, n, 10);
		return 0;
	}

	// --volsweep: パート音量を振って att を測る。音量の掛け算の元を解くため
	if (volsweep) {
		std::printf("# prog  CC7  att  （強さ %d、鍵 %d）%c", vel, note, 10);
		for (int pg = 0; pg < volsweep; pg++) {
			for (u8 b : { u8(0xb0), u8(0x00), u8(msb & 0x7f), u8(0xb0), u8(0x20), u8(lsb & 0x7f),
			              u8(0xc0), u8(pg & 0x7f) })
				mu.midi_in(b, 0);
			for (u32 i = 0; i < RATE / 4; i++)
				mu.run_sample(l, r);
			const u8 *pr2 = mu.nvram().data() + part0;
			const u32 rec2 = u32(pr2[xg::ram::PART_VOICE]) << 24 | u32(pr2[xg::ram::PART_VOICE + 1]) << 16 |
			                 u32(pr2[xg::ram::PART_VOICE + 2]) << 8 | pr2[xg::ram::PART_VOICE + 3];
			if (!rec2 || xg::nv::element_count(rom, rec2) != 1)
				continue;
			std::printf("VOICE %d rec=%06x elem=", pg, rec2);
			for (int i = 0; i < 84; i++)
				std::printf("%02x", xg::nv::element(rom, rec2, 0)[i]);
			std::putchar(10);
			for (int cc : { 16, 32, 48, 64, 80, 100, 112, 127 }) {
				for (u8 b : { u8(0xb0), u8(0x07), u8(cc) })
					mu.midi_in(b, 0);
				for (u32 i = 0; i < RATE / 20; i++)
					mu.run_sample(l, r);
				for (u8 b : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
					mu.midi_in(b, 0);
				for (u32 i = 0; i < RATE / 25; i++)
					mu.run_sample(l, r);
				std::printf("ATT %d %d%c", cc, mu.nvram()[0x3e96a], 10);
				for (u8 b : { u8(0x80), u8(note & 0x7f), u8(0x40) })
					mu.midi_in(b, 0);
				for (u32 i = 0; i < RATE / 10; i++)
					mu.run_sample(l, r);
			}
			std::fflush(stdout);
		}
		return 0;
	}

	// --sweep: 音色をいくつも替えて、レジスタの一致率をまとめて出す
	if (sweep > 0) {
		int total_same = 0, total_diff = 0;
		std::map<int, int> bad;                 // レジスタ番号 → 合わなかった回数
		int cases = 0, shown = 0;
		for (int pg = 0; pg < sweep; pg++) {
			for (u8 b : { u8(0xb0), u8(0x00), u8(msb & 0x7f), u8(0xb0), u8(0x20), u8(lsb & 0x7f),
			              u8(0xc0), u8(pg & 0x7f) })
				mu.midi_in(b, 0);
			for (u32 i = 0; i < RATE / 4; i++)
				mu.run_sample(l, r);
			const u8 *pr2 = mu.nvram().data() + part0;
			const u32 rec2 = u32(pr2[xg::ram::PART_VOICE]) << 24 | u32(pr2[xg::ram::PART_VOICE + 1]) << 16 |
			                 u32(pr2[xg::ram::PART_VOICE + 2]) << 8 | pr2[xg::ram::PART_VOICE + 3];
			if (!rec2 || xg::nv::element_count(rom, rec2) != 1)
				continue;                        // 1 要素のものだけ
			for (int nt : { 36, 60, 84 }) {
				std::map<u32, u16> seen;
				u64 mask = 0, keyed = 0;
				mu.set_swp_watch([&](bool master, u32 reg, u16 value) {
					if (!master) return;
					seen.emplace(reg, value);        // **最初の書き込み**を採る
					switch (reg) {
					case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(value) << 48); break;
					case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(value) << 32); break;
					case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(value) << 16); break;
					case 0x1cf: mask = (mask & ~u64(0xffff)) | value; break;
					// 引き金が引かれた瞬間の値を残す。これより後は LFO などの
					// 書き直しなので、組み立ての答え合わせには使えない
					case 0x20e: keyed |= mask; break;
					default: break;
					}
				});
				for (u8 b : { u8(0x90), u8(nt), u8(vel & 0x7f) })
					mu.midi_in(b, 0);
				for (u32 i = 0; i < RATE / 25; i++)
					mu.run_sample(l, r);
				mu.set_swp_watch(nullptr);
				for (u8 b : { u8(0x80), u8(nt), u8(0x40) })
					mu.midi_in(b, 0);
				for (u32 i = 0; i < RATE / 10; i++)
					mu.run_sample(l, r);
				int ch = -1, n = 0;
				for (int c = 0; c < 64; c++)
					if (keyed & (u64(1) << c)) { ch = ch < 0 ? c : ch; n++; }
				if (ch < 0 || n != 1)
					continue;                    // 1 スロットだけのものを使う
				const xg::nv::slot_regs mine =
				    xg::nv::build_note(rom, xg::nv::element(rom, rec2, 0), nt,
				                       std::min(0xff, (xg::nv::velocity_att(rom, vel) +
				                                       xg::nv::VOICE_ATT_TYPICAL) * 2));
				cases++;
				for (int i = 0; i < 0x40; i++) {
					if (!(mine.write & (u64(1) << i)))
						continue;
					const auto it = seen.find(u32(ch) * 64 + u32(i));
					if (it == seen.end())
						continue;
					if (it->second == mine.v[i]) {
						total_same++;
					} else {
						total_diff++;
						bad[i]++;
						if (i == 0x11 && shown < 6) {
							shown++;
							std::printf("  [音程] prog%3d 鍵%3d  firmware %04x  こちら %04x  byte19=%02x%c",
							            pg, nt, it->second, mine.v[i], xg::nv::element(rom, rec2, 0)[19], 10);
						}
					}
				}
			}
		}
		std::printf("%d 件（音色 × 鍵）で、レジスタ 一致 %d / 違い %d（%.1f%% 一致）%c",
		            cases, total_same, total_diff,
		            100.0 * total_same / std::max(1, total_same + total_diff), 10);
		std::printf("合わないレジスタ:%c", 10);
		for (auto [reg, cnt] : bad)
			std::printf("  %02x  %d 回（%.0f%%）%c", reg, cnt, 100.0 * cnt / std::max(1, cases), 10);
		return 0;
	}

	// --compare: firmware に 1 音鳴らさせて、そのとき書かれたレジスタと
	// 自分で組み立てたものを突き合わせる（段 2 の「レジスタ列が一致」の物差し）
	if (compare) {
		std::map<u32, u16> seen;
		u64 mask = 0, keyed = 0;
		mu.set_swp_watch([&](bool master, u32 reg, u16 value) {
			if (!master)
				return;
			seen.emplace(reg, value);        // **最初の書き込み**を採る
			switch (reg) {
			case 0x18e: mask = (mask & ~(u64(0xffff) << 48)) | (u64(value) << 48); break;
			case 0x18f: mask = (mask & ~(u64(0xffff) << 32)) | (u64(value) << 32); break;
			case 0x1ce: mask = (mask & ~(u64(0xffff) << 16)) | (u64(value) << 16); break;
			case 0x1cf: mask = (mask & ~u64(0xffff)) | value; break;
			case 0x20e: keyed |= mask; break;
			default: break;
			}
		});
		for (u8 b : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
			mu.midi_in(b, 0);
		for (u32 i = 0; i < RATE / 25; i++)
			mu.run_sample(l, r);
		mu.set_swp_watch(nullptr);
		int ch = -1;
		for (int c = 0; c < 64; c++)
			if (keyed & (u64(1) << c)) { ch = c; break; }
		if (ch < 0) {
			std::fprintf(stderr, "firmware が鳴らしたスロットが分からない%c", 10);
			return 1;
		}
		const u8 *elem = xg::nv::element(rom, rec, 0);
		const xg::nv::slot_regs mine = xg::nv::build_note(rom, elem, note,
		    std::min(0xff, (xg::nv::velocity_att(rom, vel) + xg::nv::VOICE_ATT_TYPICAL) * 2));
		// firmware がこのスロットに書いたもののうち、こちらが**書いていない**ものを出す
		std::printf("こちらが書いていないレジスタ:");
		for (const auto &[reg, val] : seen) {
			const int c = int(reg >> 6) & 0x3f, sl = int(reg) & 0x3f;
			if (c != ch || reg >= 0x1000 || sl == 0x0e || sl == 0x0f)
				continue;
			if (!(mine.write & (u64(1) << sl)))
				std::printf(" %02x=%04x", sl, val);
		}
		std::putchar(10);
		int same = 0, diff = 0, missing = 0;
		std::printf("firmware はスロット %d を鳴らした。突き合わせ:%c", ch, 10);
		for (int i = 0; i < 0x40; i++) {
			if (!(mine.write & (u64(1) << i)))
				continue;
			const auto it = seen.find(u32(ch) * 64 + u32(i));
			if (it == seen.end()) { missing++; continue; }
			if (it->second == mine.v[i]) {
				same++;
			} else {
				diff++;
				std::printf("  %02x  firmware %04x  こちら %04x%c", i, it->second, mine.v[i], 10);
			}
		}
		std::printf("一致 %d / 違い %d / firmware が書かなかった %d%c", same, diff, missing, 10);
		return diff ? 2 : 0;
	}

	// --song: SH-2 を止めたまま、何音かを順に鳴らす（段 2 の「和音と声の取り合い」の入口）
	if (song) {
		const int nelem = std::min(4, xg::nv::element_count(rom, rec));
		// 先に 1 音だけ firmware に鳴らしてもらって、この音色の癖を写し取る
		const std::vector<u8> before = mu.save_state();
		const std::vector<xg::nv::voice_cal> cals =
		    take_cals(mu, rom, rec, 60, vel, RATE);
		std::string errc;
		if (!mu.load_state(before.data(), before.size(), errc)) { std::fprintf(stderr, "%s%c", errc.c_str(), 10); return 1; }
		std::printf("要素 %d、写し取ったスロット %zu%c", nelem, cals.size(), 10);
		mu.set_cpu_enabled(false);
		std::vector<s16> out;
		struct ev { double t; int note; bool on; int slot; };
		// ドレミファソラシド＋和音
		static const int SCALE[] = { 60, 62, 64, 65, 67, 69, 71, 72 };
		std::vector<ev> evs;
		int slot_at = 0;
		for (int i = 0; i < 8; i++) {
			evs.push_back({ 0.25 * i,        SCALE[i], true,  slot_at });
			evs.push_back({ 0.25 * i + 0.22, SCALE[i], false, slot_at });
			slot_at = (slot_at + 1) & 7;
		}
		for (int i = 0; i < 3; i++) {      // 最後に和音
			const int n = 60 + i * 4;
			evs.push_back({ 2.2, n, true,  slot_at });
			evs.push_back({ 3.4, n, false, slot_at });
			slot_at = (slot_at + 1) & 7;
		}
		std::sort(evs.begin(), evs.end(), [](const ev &a, const ev &b) { return a.t < b.t; });
		size_t at = 0;
		const size_t total = size_t(std::max(seconds, 4.5) * RATE);
		out.reserve(total * 2);
		for (size_t i = 0; i < total; i++) {
			const double t = double(i) / RATE;
			while (at < evs.size() && evs[at].t <= t) {
				const ev &e = evs[at];
				// 要素の数だけスロットを使う。鍵と強さの範囲に入るものだけ
				u64 keymask = 0;
				int used = 0;
				for (int k = 0; k < nelem; k++) {
					const u8 *el = xg::nv::element(rom, rec, k);
					if (!xg::nv::element_active(el, e.note, vel))
						continue;
					const u8 *we2 = xg::nv::wave_entry(rom, xg::nv::wave_set(el), e.note);
					const xg::nv::voice_cal *c = we2 ? xg::nv::match_cal(cals, xg::nv::read_wave(we2).format_addr) : nullptr;
					if (!c && size_t(used) < cals.size())
						c = &cals[used];
					used++;
					const int slot = (e.slot * 4 + k) & 0x3f;
					const int att = xg::nv::volume_att(rom, el,
					    c ? c->base_level : 64, e.note, vel);
					if (e.on) {
						poke_slot(mu, slot, xg::nv::build_note(rom, el, e.note, att, c));
						keymask |= u64(1) << slot;
					} else {
						mu.poke_swp(true, u32(slot) * 64 + 9,
						            xg::nv::release_reg(rom, el, e.note, att));
					}
				}
				if (e.on)
					key_on_mask(mu, keymask);
				at++;
			}
			s32 li = 0, ri = 0;
			mu.run_sample(li, ri);
			out.push_back(s16(std::clamp(li * 32768 / mu2000::DAC_FULL_SCALE, -32768, 32767)));
			out.push_back(s16(std::clamp(ri * 32768 / mu2000::DAC_FULL_SCALE, -32768, 32767)));
		}
		double pk = 0.0, sm = 0.0;
		for (s16 v : out) { pk = std::max(pk, double(std::abs(v))); sm += double(v) * v; }
		std::printf("SH-2 なしで %zu 音: 山 %.0f  rms %.1f%c", evs.size() / 2, pk,
		            std::sqrt(sm / double(out.size())), 10);
		if (!write_wav(out_path, out, RATE)) { std::fprintf(stderr, "書けない%c", 10); return 1; }
		std::printf("書き出した: %s%c", out_path.c_str(), 10);
		return 0;
	}

	std::vector<s16> pcm;
	pcm.reserve(size_t(seconds * RATE) * 2);

	if (firmware) {
		// 比べるための版。firmware にそのまま鳴らさせる
		for (u8 b : { u8(0x90), u8(note & 0x7f), u8(vel & 0x7f) })
			mu.midi_in(b, 0);
	} else {
		const u8 *elem = xg::nv::element(rom, rec, 0);
		// 先に 1 音だけ firmware に鳴らしてもらって癖を写し取る
		const std::vector<u8> before1 = mu.save_state();
		const xg::nv::voice_cal cal = take_cal(mu, rom, rec, note, vel, RATE);
		std::string err1;
		if (!mu.load_state(before1.data(), before1.size(), err1)) { std::fprintf(stderr, "%s%c", err1.c_str(), 10); return 1; }
		// ---- ここから SH-2 を止める
		mu.set_cpu_enabled(false);
		const int att = xg::nv::volume_att(rom, elem, cal.base_level, note, vel);
		xg::nv::slot_regs regs = xg::nv::build_note(rom, elem, note, att, &cal);
		if (copyall) {
			// 切り分け用: 写し取った値をそのまま全部使う（式を一切使わない）
			for (int i = 0; i < 0x40; i++)
				if (cal.has(i))
					regs.set(i, cal.reg[i]);
		}
		if (!regs.write) {
			std::fprintf(stderr, "波形の記録が引けなかった%c", 10);
			return 1;
		}
		std::printf("スロット %d に書くレジスタ:%c", slot, 10);
		for (int i = 0; i < 0x40; i++)
			if (regs.write & (u64(1) << i))
				std::printf("  %02x=%04x%s", i, regs.v[i], (i % 8 == 7) ? "\n" : "");
		std::putchar(10);
		poke_slot(mu, slot, regs);
		key_on(mu, slot);
	}

	for (size_t i = 0; i < size_t(seconds * RATE); i++) {
		s32 li = 0, ri = 0;
		mu.run_sample(li, ri);
		pcm.push_back(s16(std::clamp(li * 32768 / mu2000::DAC_FULL_SCALE, -32768, 32767)));
		pcm.push_back(s16(std::clamp(ri * 32768 / mu2000::DAC_FULL_SCALE, -32768, 32767)));
	}

	double peak = 0.0, sum = 0.0;
	for (s16 v : pcm) { peak = std::max(peak, double(std::abs(v))); sum += double(v) * v; }
	std::printf("%s: 山 %.0f  rms %.1f%c", firmware ? "firmware" : "SH-2 なし",
	            peak, std::sqrt(sum / double(pcm.size())), 10);
	if (!write_wav(out_path, pcm, RATE)) {
		std::fprintf(stderr, "書けない: %s%c", out_path.c_str(), 10);
		return 1;
	}
	std::printf("書き出した: %s%c", out_path.c_str(), 10);
	return 0;
}
