// license:BSD-3-Clause
//
// 状態の保存と復元が正しいかを確かめる。
//
//   statetest <rom ディレクトリ> [<MIDI ファイル>] [--warm 秒] [--steps 数]
//
// やること
//
//   1. 起動して MIDI を流し、warm 秒ぶん進める
//   2. そこで状態を保存する
//   3. まっさらな機械を作り、その状態を読み戻す
//   4. **両方を 1 サンプルずつ同じだけ進めて、状態を突き合わせる**
//
// 写し忘れた状態が 1 つでもあれば、そこから先がずれる。ずれた場所の
// 直前の目印を出すので、どの区画が足りないかがすぐ分かる。
//
// 音で比べるより先に状態で比べるのが要点。音は最後の出口なので、
// ずれても原因の場所が分からない。
//
// MIDI を流したあとで止めるのも要点。無音のまま比べても、エンベロープも
// フィルタも動いていないので何も見つからない。

#include "mu2000.h"
#include "smf.h"
#include "compat/console.h"

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

namespace {

constexpr u32 RATE = 44100;

bool boot(mu2000 &mu, const std::string &dir)
{
	if (!mu.load_program(dir + "/mu2000_flash.bin")) {
		std::fprintf(stderr, "%s\n", mu.error().c_str());
		return false;
	}
	if (!mu.load_wave(dir + "/dump")) {
		std::fprintf(stderr, "%s\n", mu.error().c_str());
		return false;
	}
	mu.load_sintab(dir + "/standin/sin-table.bin");
	if (!mu.load_lcd_font(dir + "/hd44780u_b04.bin"))
		mu.load_lcd_font(dir + "/standin/hd44780u_b04.bin");
	mu.set_threaded(false);          // 突き合わせなので 1 本で回す
	mu.reset();

	const size_t limit = size_t(30.0 * RATE);
	s32 l, r;
	for (size_t i = 0; i < limit && !mu.midi_ready(); i++)
		mu.run_sample(l, r);
	return true;
}

// n サンプルぶん進める。events はそのあいだに流す MIDI
void advance(mu2000 &mu, size_t n, const std::vector<smf::event> &events,
             size_t &at, double &clock)
{
	for (size_t i = 0; i < n; i++) {
		while (at < events.size() && events[at].time <= clock) {
			for (u8 b : events[at].bytes)
				mu.midi_in(b);
			at++;
		}
		s32 l = 0, r = 0;
		mu.run_sample(l, r);
		clock += 1.0 / RATE;
	}
}

// ずれた場所の直前にある目印を探す
void report_where(const std::vector<u8> &blob, size_t at)
{
	static const char *tags[] = {
		"mu2000", "mach", "shcore", "sh2", "sh7042", "intc", "adc", "bsc",
		"cmt", "dmac", "dmach", "mtu", "mtuch", "port16", "port32", "sci",
		"swp30", "meg", "lcd", "sci4", "panel", "midi",
	};
	std::string last = "（無し）";
	size_t last_at = 0;
	for (size_t i = 0; i + 8 <= at && i + 8 <= blob.size(); i++)
		for (const char *t : tags) {
			char buf[8] = {};
			std::strncpy(buf, t, 7);
			if (!std::memcmp(&blob[i], buf, 8)) {
				last_at = i;
				last = t;
			}
		}
	std::printf("  直前の目印: %s（%zu 番目、そこから %zu バイト先）\n",
	            last.c_str(), last_at, at - last_at);
}

} // namespace


int main(int argc, char **argv)
{
	smu2000::init_console_utf8();
	// 落ちても途中まで見えるように。調べ物の道具なので速さは要らない
	std::setvbuf(stdout, nullptr, _IONBF, 0);

	std::string dir, mid;
	double warm = 3.0;
	int steps = 400;
	for (int i = 1; i < argc; i++) {
		if (!std::strcmp(argv[i], "--warm") && i + 1 < argc) warm = std::atof(argv[++i]);
		else if (!std::strcmp(argv[i], "--steps") && i + 1 < argc) steps = std::atoi(argv[++i]);
		else if (dir.empty()) dir = argv[i];
		else if (mid.empty()) mid = argv[i];
	}
	if (dir.empty()) {
		std::fprintf(stderr,
			"使い方: statetest <rom ディレクトリ> [<MIDI ファイル>]"
			" [--warm 秒] [--steps 数]\n");
		return 1;
	}

	std::vector<smf::event> events;
	if (!mid.empty()) {
		std::string err;
		if (!smf::load(mid, events, err)) {
			std::fprintf(stderr, "%s\n", err.c_str());
			return 1;
		}
		std::printf("MIDI %zu 件\n", events.size());
	}

	// ---- 1. 起動して warm 秒
	std::printf("起動中...\n");
	// **積み場には置かない**。mu2000 は 2 台ぶんが積み場に収まらず、
	// 何も出さずに落ちる
	auto one_p = std::make_unique<mu2000>();
	mu2000 &one = *one_p;
	if (!boot(one, dir))
		return 1;
	size_t at = 0;
	double clock = 0;
	advance(one, size_t(warm * RATE), events, at, clock);

	// ---- 2. 保存
	const std::vector<u8> saved = one.save_state();
	std::printf("保存した: %zu バイト（%.2f 秒のところ）\n", saved.size(), warm);
	{
		// 詰めた形も往復できることを見ておく。DAW に入れるのはこちら
		const std::vector<u8> packed = state_pack(saved);
		std::vector<u8> back;
		const bool ok = state_unpack(packed.data(), packed.size(), back) &&
		                back == saved;
		std::printf("詰めると %zu バイト（%.1f%%）。戻し: %s\n",
		            packed.size(), 100.0 * packed.size() / saved.size(),
		            ok ? "一致" : "だめ");
	}

	// ---- 3. まっさらな機械へ読み戻す
	auto two_p = std::make_unique<mu2000>();
	mu2000 &two = *two_p;
	if (!boot(two, dir))
		return 1;
	std::string err;
	if (!two.load_state(saved.data(), saved.size(), err)) {
		std::fprintf(stderr, "読み戻せない: %s\n", err.c_str());
		return 1;
	}
	{
		const std::vector<u8> again = two.save_state();
		if (again == saved) {
			std::printf("戻した直後の状態は一致\n");
		} else {
			size_t d = 0;
			while (d < again.size() && d < saved.size() && again[d] == saved[d])
				d++;
			std::printf("戻した直後から食い違う。%zu 番目\n", d);
			report_where(saved, d);
			return 1;
		}
	}

	// ---- 4. 両方を同じだけ進めて、1 サンプルごとに突き合わせる
	size_t at1 = at, at2 = at;
	double c1 = clock, c2 = clock;
	for (int k = 1; k <= steps; k++) {
		advance(one, 1, events, at1, c1);
		advance(two, 1, events, at2, c2);
		const std::vector<u8> x = one.save_state(), y = two.save_state();
		if (x == y)
			continue;
		size_t d = 0;
		while (d < x.size() && d < y.size() && x[d] == y[d])
			d++;
		std::printf("---- %d サンプル目でずれた。%zu 番目から ----\n", k, d);
		// 違っているところを固まりごとに並べる。どこが本命かを見る
		{
			int shown = 0;
			size_t i = 0;
			while (i < x.size() && i < y.size() && shown < 8) {
				if (x[i] == y[i]) { i++; continue; }
				const size_t from = i;
				while (i < x.size() && i < y.size() && x[i] != y[i])
					i++;
				std::printf("  %zu-%zu（%zu バイト）\n", from, i - 1, i - from);
				report_where(x, from);
				shown++;
			}
		}
		std::printf("写し忘れている状態がある\n");
		return 1;
	}

	std::printf("---- %d サンプル進めても状態は完全に一致 ----\n", steps);
	return 0;
}
