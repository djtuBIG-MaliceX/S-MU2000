// パラメータの層の定義表を、MU2000 の firmware に確かめさせる。
//
//   xgtest <rom ディレクトリ> [-v]
//
// 定義表の 1 行ずつ、範囲の端と真ん中の値をパラメータチェンジで書き、
// 同じ番地を問い合わせて、書いた値が返るかを見る（doc/params.md の 7 番）。
// 資料を写し間違えても、番地・大きさ・範囲のどれかが違えばここで食い違う。
// あわせて、一括ダンプから切り出した値が 1 つずつの問い合わせと一致するか、
// プログラムチェンジのような「XG 以外の道」で変えた値も読めるかを見る。
//
// 音は見ない。食い違いが 1 つでもあれば 1 を返す。
#include "mu2000.h"
#include "xg/model.h"
#include "xg/ram.h"

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

namespace {

constexpr u32 RATE = 44100;

struct rig {
	mu2000 mu;
	xg::model reader;           // MIDI OUT から読むだけ
	u64 samples = 0;

	u64 now_ms() const { return samples * 1000 / RATE; }

	void pump(u32 ms)
	{
		s32 l, r;
		const u64 until = samples + u64(ms) * RATE / 1000;
		for (; samples < until; samples++) {
			mu.run_sample(l, r);
			u8 b;
			while (mu.midi_out_take(b))
				reader.feed(b);
		}
	}

	void send(const std::vector<u8> &m)
	{
		for (u8 b : m)
			mu.midi_in(b, 0);
	}

	// 問い合わせて、写しに入るまで回す。読めなければ false
	bool ask(const xg::param &p, int part, int &value)
	{
		reader.forget(p, part);
		send(xg::param_request(p, part));
		for (int t = 0; t < 50; t++) {          // 最大 500ms
			pump(10);
			if (reader.get(p, part, value))
				return true;
		}
		return false;
	}
};

std::string label(const xg::param &p, int part)
{
	std::string s = p.key;
	if (p.where == xg::area::part)
		s += "[" + std::to_string(part + 1) + "]";
	return s;
}

} // namespace

int main(int argc, char **argv)
{
	if (argc < 2) {
		std::fprintf(stderr, "xgtest <rom ディレクトリ> [-v]\n");
		return 2;
	}
	const std::string dir = argv[1];
	bool verbose = false, usb_host = false;
	for (int i = 2; i < argc; i++) {
		if (!std::strcmp(argv[i], "-v"))    verbose = true;
		if (!std::strcmp(argv[i], "--usb")) usb_host = true;
	}

	rig g;
	if (!g.mu.load_program(dir + "/mu2000_flash.bin") || !g.mu.load_wave(dir + "/dump")) {
		std::fprintf(stderr, "%s\n", g.mu.error().c_str());
		return 2;
	}
	g.mu.load_sintab(dir + "/standin/sin-table.bin");
	g.mu.set_usb_host(usb_host);
	g.mu.reset();                                // NVRAM は使わない。毎回工場出荷状態
	s32 l, r;
	for (; g.samples < 30 * RATE && !g.mu.midi_ready(); g.samples++)
		g.mu.run_sample(l, r);
	std::printf("起動 %.2f 秒で SCI 受信 %s（USB モード %s）\n",
	            double(g.samples) / RATE, g.mu.midi_ready() ? "有効" : "無効",
	            usb_host ? "入" : "切");
	if (usb_host)                       // USB のときは DIN を使わないので待たない
		for (; g.samples < 10 * RATE; g.samples++)
			g.mu.run_sample(l, r);
	g.pump(500);

	int checked = 0, bad = 0;
	std::vector<std::string> problems;

	// ---- 1. 書いて読み返す
	const int PARTS[] = { 0, 16, 32, 63 };        // 口 A・B・C・D から 1 つずつ（1、17、33、64）
	for (const xg::param &p : xg::params()) {
		const int nparts = p.where == xg::area::part ? 4 : 1;
		for (int k = 0; k < nparts; k++) {
			const int part = p.where == xg::area::part ? PARTS[k] : 0;
			int original = 0;
			if (!g.ask(p, part, original)) {
				problems.push_back(label(p, part) + ": 問い合わせに返事が無い");
				bad++;
				continue;
			}

			// 試す値。種別は並びが飛び飛びなので、今の値と NO EFFECT（0）だけ。
			// エレメントリザーブは全パートの合計に枠があるので小さい値で
			std::vector<int> values;
			if (p.max == 0x3fff) {
				values = { 0, original };
			} else if (std::string(p.key) == "part.element_reserve") {
				values = { 0, 4, original };
			} else {
				values = { p.min, p.max, (p.min + p.max + 1) / 2 };
				if (p.special >= 0) values.push_back(p.special);
			}

			std::string seen;
			const xg::param *prog = xg::find("part.program");
			int program = 0;
			if (xg::applies_on_program(p))
				g.ask(*prog, part, program);
			for (int v : values) {
				g.send(xg::param_change(p, part, v));
				// バンクはプログラムを書くまで効かない（model::set はそうしている）
				if (xg::applies_on_program(p))
					g.send(xg::param_change(*prog, part, program));
				g.pump(30);
				int got = -1;
				const bool ok = g.ask(p, part, got);
				checked++;
				if (!ok || got != v) {
					char buf[128];
					std::snprintf(buf, sizeof(buf), "%s: %d を書いたら %s", label(p, part).c_str(), v,
					              ok ? std::to_string(got).c_str() : "返事が無い");
					problems.push_back(buf);
					bad++;
				}
				if (verbose)
					seen += " " + std::to_string(v) + "→" + (ok ? std::to_string(got) : "?");
			}
			g.send(xg::param_change(p, part, original));
			if (xg::applies_on_program(p))
				g.send(xg::param_change(*prog, part, program));
			g.pump(30);
			if (verbose)
				std::printf("  %-24s 元 %-6d %s\n", label(p, part).c_str(), original, seen.c_str());
		}
	}
	std::printf("書いて読み返す: %d 回、食い違い %d\n", checked, bad);

	// ---- 2. 一括ダンプから切り出した値が、1 つずつの問い合わせと一致するか
	{
		int n = 0, diff = 0;
		for (int part : PARTS) {
			xg::model dumped;
			const std::vector<u8> req = xg::part_dump_request(part);
			// 返事を別の写しに読む
			g.send(req);
			const u64 until = g.samples + RATE / 2;
			for (; g.samples < until; g.samples++) {
				g.mu.run_sample(l, r);
				u8 b;
				while (g.mu.midi_out_take(b)) {
					dumped.feed(b);
					g.reader.feed(b);
				}
			}
			for (const xg::param &p : xg::params()) {
				// パートの一括ダンプは 00-28 の 41 バイトだけ。EQ（72-77）は入らない
				if (p.where != xg::area::part || p.lo >= xg::ram::PART_XG_SIZE)
					continue;
				int a = 0, b = 0;
				const bool in_dump = dumped.get(p, part, a);
				const bool single = g.ask(p, part, b);
				n++;
				if (!in_dump || !single || a != b) {
					diff++;
					problems.push_back(label(p, part) + ": ダンプ " +
					                   (in_dump ? std::to_string(a) : "無し") + " / 問い合わせ " +
					                   (single ? std::to_string(b) : "無し"));
				}
			}
		}
		// システムとエフェクトの塊も（エフェクトの面はこれで読む）
		const u32 BLOCKS[] = { xg::pack(0x00, 0x00, 0x00), xg::pack(0x02, 0x01, 0x00), xg::pack(0x02, 0x01, 0x20),
		                       xg::pack(0x02, 0x01, 0x40), xg::pack(0x03, 0x00, 0x00), xg::pack(0x03, 0x01, 0x00) };
		for (u32 blk : BLOCKS) {
			xg::model dumped;
			g.send(xg::dump_request(blk));
			const u64 until = g.samples + RATE / 2;
			for (; g.samples < until; g.samples++) {
				g.mu.run_sample(l, r);
				u8 b;
				while (g.mu.midi_out_take(b))
					dumped.feed(b);
			}
			for (const xg::param &p : xg::params()) {
				// 02 01 は 20 ずつの 3 塊（リバーブ・コーラス・バリエーション）に分かれている
				const u32 at = xg::address(p);
				if (p.where == xg::area::part || at < blk || at - blk >= 0x20)
					continue;
				int a = 0, b = 0;
				const bool in_dump = dumped.get(p, 0, a);
				const bool single = g.ask(p, 0, b);
				n++;
				if (!in_dump || !single || a != b) {
					diff++;
					problems.push_back(label(p, 0) + ": ダンプ " +
					                   (in_dump ? std::to_string(a) : "無し") + " / 問い合わせ " +
					                   (single ? std::to_string(b) : "無し"));
				}
			}
		}
		std::printf("ダンプと問い合わせの一致: %d 個、食い違い %d\n", n, diff);
		bad += diff;
	}

	// ---- 3. XG 以外の道で変えた値も読めるか（プログラムチェンジとコントロールチェンジ）
	{
		const xg::param *prog = xg::find("part.program");
		const xg::param *vol  = xg::find("part.volume");
		g.send({ 0xc0, 0x30 });                 // パート 1 を 49 番に
		g.send({ 0xb0, 7, 77 });                // パート 1 の音量を 77 に
		g.send({ 0xc0 | 0x0, 0x30 });
		g.mu.midi_in(0xc2, 1);                   // 口 B の ch3 = パート 19 を 5 番に
		g.mu.midi_in(0x04, 1);
		g.pump(100);
		xg::model m;
		m.want_part(0);
		m.want_part(18);
		while (m.busy()) {
			const std::vector<u8> out = m.poll(g.now_ms());
			g.send(out);
			const u64 until = g.samples + RATE / 100;
			for (; g.samples < until; g.samples++) {
				g.mu.run_sample(l, r);
				u8 b;
				while (g.mu.midi_out_take(b))
					m.feed(b);
			}
		}
		int p1 = -1, v1 = -1, p19 = -1;
		const bool ok = m.get(*prog, 0, p1) && m.get(*vol, 0, v1) && m.get(*prog, 18, p19);
		const bool good = ok && p1 == 0x30 && v1 == 77 && p19 == 4;
		std::printf("プログラムチェンジと CC7 を読み返す（model の頼み方で）: %s"
		            "（パート 1 = %d / 音量 %d、パート 19 = %d。チェックサム違い %llu）\n",
		            good ? "合" : "違", p1, v1, p19, (unsigned long long)m.rejected());
		if (!good) {
			bad++;
			problems.push_back("プログラムチェンジ・CC7 の読み返し");
		}

		// model::set でバンクを書くと、写しにあるプログラムも続けて送ってすぐ効かせる
		const xg::param *msb = xg::find("part.bank_msb");
		g.send(m.set(*msb, 0, 64));
		m.forget(*msb, 0);
		m.want_part(0);
		while (m.busy()) {
			g.send(m.poll(g.now_ms()));
			const u64 until = g.samples + RATE / 100;
			for (; g.samples < until; g.samples++) {
				g.mu.run_sample(l, r);
				u8 b;
				while (g.mu.midi_out_take(b))
					m.feed(b);
			}
		}
		int bank = -1;
		const bool bank_ok = m.get(*msb, 0, bank) && bank == 64;
		std::printf("model::set でバンクを書いてすぐ効くか: %s（MSB = %d）\n", bank_ok ? "合" : "違", bank);
		if (!bank_ok) {
			bad++;
			problems.push_back("model::set のバンク");
		}

		// 書く前に頼んだ読み返しが、書いた後に届いても、書いた値が残るか。
		// つまみを回している最中に古い値へ戻って見えないためのもの
		const xg::param *pan = xg::find("part.pan");
		const u64 t0 = g.now_ms();
		m.want_part(0);
		g.send(m.poll(t0));                     // 頼む（音源はまだ今の値で答える）
		g.send(m.set(*pan, 0, 20));             // 返事が来る前に書く
		for (int t = 0; t < 10; t++) {
			const u64 until = g.samples + RATE / 100;
			for (; g.samples < until; g.samples++) {
				g.mu.run_sample(l, r);
				u8 b;
				while (g.mu.midi_out_take(b))
					m.feed(b);
			}
			g.send(m.poll(g.now_ms()));
		}
		int seen = -1, actual = -1;
		m.get(*pan, 0, seen);
		g.ask(*pan, 0, actual);
		const bool pin_ok = seen == 20 && actual == 20;
		std::printf("読み返しと書き込みが行き違っても書いた値が残るか: %s（写し %d / 音源 %d）\n",
		            pin_ok ? "合" : "違", seen, actual);
		if (!pin_ok) {
			bad++;
			problems.push_back("読み返しと書き込みの行き違い");
		}
	}

	// ---- 5. ワーク RAM から読んだ値が、問い合わせの返事と同じか（xg/ram.h）
	//         画面は問い合わせずに RAM を読むので、番地の表が合っているかをここで見る
	{
		int n = 0, diff = 0;
		// 既定のままだと 0 が多くて偶然合うので、少し散らしておく
		const int RAM_PARTS[] = { 0, 9, 16, 25, 31, 32, 41, 48, 63 };   // RAM では 10 と 26 が口の先頭にある
		for (int part : RAM_PARTS) {
			g.send(xg::param_change(*xg::find("part.volume"), part, 37 + part));
			g.send(xg::param_change(*xg::find("part.pan"), part, 20 + part));
			g.send(xg::param_change(*xg::find("part.cutoff"), part, 90 - part));
			g.send(xg::param_change(*xg::find("part.detune"), part, 0x5a + part));
		}
		g.send(xg::param_change(*xg::find("reverb.return"), 0, 0x33));
		g.send(xg::param_change(*xg::find("chorus.pan"), 0, 0x21));
		g.send(xg::param_change(*xg::find("variation.part"), 0, 3));
		g.send(xg::param_change(*xg::find("insertion2.part"), 0, 7));
		g.pump(200);
		const std::vector<u8> &ramv = g.mu.nvram();
		for (const xg::param &p : xg::params()) {
			const int nparts = p.where == xg::area::part ? 5 : 1;
			for (int k = 0; k < nparts; k++) {
				const int part = p.where == xg::area::part ? RAM_PARTS[k] : 0;
				u32 off = 0;
				if (!xg::ram::locate(xg::address(p, part), off)) {
					problems.push_back(label(p, part) + ": RAM の番地が表に無い");
					diff++;
					continue;
				}
				int v = 0;
				for (int i = 0; i < p.size; i++)
					v = (v << (p.enc == xg::coding::nibble ? 4 : 7)) | (ramv[off + i] & (p.enc == xg::coding::nibble ? 0x0f : 0x7f));
				int asked = -1;
				const bool ok = g.ask(p, part, asked);
				n++;
				if (!ok || asked != v) {
					diff++;
					problems.push_back(label(p, part) + ": RAM " + std::to_string(v) + " / 問い合わせ " +
					                   (ok ? std::to_string(asked) : "無し"));
				}
			}
		}
		std::printf("RAM と問い合わせの一致: %d 個、食い違い %d\n", n, diff);
		bad += diff;

		// インサーションのパラメータ 1-10（2 バイトの 30-43）は、RAM では塊の +0x18 から 16bit の数。
		// ディレイの時間のように 128 を超える値で確かめる。インサーション 3 を DELAY LCR にして Rch Delay を 1234 に
		g.send({ 0xf0, 0x43, 0x10, 0x4c, 0x03, 0x02, 0x00, 0x05, 0x00, 0xf7 });
		g.pump(200);
		g.send({ 0xf0, 0x43, 0x10, 0x4c, 0x03, 0x02, 0x32, u8(1234 >> 7), u8(1234 & 0x7f), 0xf7 });
		g.pump(200);
		const std::vector<u8> &rv = g.mu.nvram();
		const u32 at = xg::ram::INS_BLOCK[2] + xg::ram::INS_WIDE + 2;
		const int wide = rv[at] << 8 | rv[at + 1];
		std::printf("インサーションの 2 バイトのパラメータが RAM の 16bit の数に入るか: %s（%d）\n", wide == 1234 ? "合" : "違", wide);
		if (wide != 1234) {
			bad++;
			problems.push_back("インサーションの 2 バイトのパラメータの RAM の位置");
		}
	}

	for (const std::string &s : problems)
		std::printf("  NG %s\n", s.c_str());
	std::printf(bad ? "食い違いあり\n" : "全部合った\n");
	return bad ? 1 : 0;
}
