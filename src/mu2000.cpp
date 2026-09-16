// license:BSD-3-Clause
//
// MU2000 一台ぶんの組み立て。配置は MAME の ymmu2000.cpp と同じ。

#include "mu2000.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>

#include "compat/platform.h"


namespace {

// MIDI は 31250bps。28MHz の CPU から見て 1 ビット = 896 サイクル
constexpr u64 MIDI_BIT_CYCLES = 28000000 / 31250;

bool read_file(const std::string &path, std::vector<u8> &out, size_t expect)
{
	std::FILE *f = std::fopen(path.c_str(), "rb");
	if (!f)
		return false;
	std::fseek(f, 0, SEEK_END);
	const long size = std::ftell(f);
	std::fseek(f, 0, SEEK_SET);
	if (expect && size_t(size) != expect) {
		std::fclose(f);
		return false;
	}
	out.resize(size_t(size));
	const size_t got = std::fread(out.data(), 1, out.size(), f);
	std::fclose(f);
	return got == out.size();
}

} // namespace


static std::atomic<int> g_live_instances{0};

mu2000::mu2000()
{
	g_live_instances++;
	// CPU。MAME は 7MHz の水晶を PLL で 4 倍していた
	m_cpu = &m_config.make<sh7043a_device>(m_cpu_finder, 7000000u * 4);

	// 内蔵周辺を作る。MAME の device_add_mconfig をそのまま呼ぶ
	m_cpu->device_add_mconfig(m_config);

	// PLG ボード用のシリアル。ボードは挿さないが、firmware はレジスタを触る
	m_sci4 = &m_config.make<sci4_device>(m_sci4_finder);

	// 時計とタイマの置き場を全デバイスに配る
	m_machine.set_clock_hz(7000000 * 4);
	for (auto &d : m_config.m_devices)
		d->set_machine(&m_machine);

	m_ram.assign(0x40000, 0);        // 256KB
	m_dram.assign(0x80000, 0);       // 512KB
	m_iram.assign(0x1000, 0);        // CPU 内蔵 4KB
	m_sampram.assign(0x400000, 0);   // SWP30 のサンプリング RAM
	m_swpm.set_sample_ram(m_sampram.data(), m_sampram.size());
	m_swps.set_sample_ram(m_sampram.data(), m_sampram.size());

	build_bus();
}

mu2000::~mu2000()
{
	set_threaded(false);
	g_live_instances--;
}

// スレーブを別スレッドで回すのは、動いている台数が少ないときだけ。
// 別スレッドは 1 台で 2 コアを回して使う（書き出しは 2 割ほど速い）が、1 つのプロセスで何台も
// 動かす（DAW に何枚も挿す）とコアの取り合いになる。16 論理コア（8 物理）で dense を並べて回すと、
// 4 台は別スレッドが速い（2.64 / 1 本 3.19 秒）が、8 台で逆転し（3.97 / 3.48）、16 台では 1 本が
// 2 倍速い（10.89 / 5.35）。だから動いている台数が論理コア数の 1/4 以下のときだけ別スレッドにする。
// 台数は途中で変わるので、run_sample がときどき見直す（1 本でも別スレッドでも出る音は同じ）。
// SMU2000_THREADED_MAX で台数の境を変えられる（0 なら全部 1 本）
static int threaded_max()
{
	static const int n = [] {
		if (const char *e = std::getenv("SMU2000_THREADED_MAX"))
			return std::max(0, std::atoi(e));
		return std::max(1, int(std::thread::hardware_concurrency() / 4));
	}();
	return n;
}

void mu2000::set_threaded(bool on)
{
	m_want_threaded = on;
	apply_threading();
}

// 頼まれていて、台数が境を超えていなければ別スレッドにする。そうでなければ 1 本に戻す
void mu2000::apply_threading()
{
	const bool on = m_want_threaded && g_live_instances.load(std::memory_order_relaxed) <= threaded_max();
	if (on == m_slave_thread.joinable())
		return;

	if (!on) {
		m_slave_quit = true;
		m_slave_go++;
		m_slave_go.notify_one();
		m_slave_thread.join();
		m_slave_quit = false;
		return;
	}
	// 合図の数は前に回した分だけ進んでいるので、今の数から待ち始める（0 からだと着いた途端に 1 サンプル余計に回す）
	const u64 seen = m_slave_go.load(std::memory_order_acquire);
	m_slave_done.store(seen, std::memory_order_release);
	m_slave_thread = std::thread([this, seen] { slave_loop(seen); });
}

// 空振りを何回続けたら眠るか。0 以下なら永久に回す（比較用）
#ifndef SLAVE_SPINS
#define SLAVE_SPINS 20000
#endif

void mu2000::slave_loop(u64 seen)
{
	for (;;) {
		// 合図を待つ。1 サンプルの中の待ちは 1 マイクロ秒に満たないので、
		// まず回して待つ。眠っていては 44100 回/秒には間に合わない。
		//
		// ただし DAW の中では、1 ブロック作り終えてから次に呼ばれるまでの
		// 数ミリ秒がまるごと空く。そこまで回し続けると 1 コアを常時
		// 焼くことになるので、しばらく空振りしたら本当に眠る
		int spins = 0;
		while (m_slave_go.load(std::memory_order_acquire) == seen) {
			if (m_slave_quit.load(std::memory_order_relaxed))
				return;
			if (SLAVE_SPINS <= 0 || ++spins < SLAVE_SPINS)
				smu2000::cpu_pause();
			else
				m_slave_go.wait(seen, std::memory_order_acquire);
		}
		seen = m_slave_go.load(std::memory_order_acquire);
		if (m_slave_quit.load(std::memory_order_relaxed))
			return;

		m_slave_l = m_slave_r = 0;
		m_swps.run_sample(m_slave_l, m_slave_r);
		m_slave_done.store(seen, std::memory_order_release);
	}
}


bool mu2000::load_program(const std::string &path)
{
	auto rom = std::make_shared<std::vector<u8>>();
	if (!read_file(path, *rom, 0x400000)) {
		m_error = "プログラム ROM を読めない（4MB でないか、見つからない）: " + path;
		return false;
	}
	set_program_rom(std::move(rom));
	return true;
}

// ROM は読むだけなので、何台の MU2000 で分け合っても構わない。
// VST3 を複数挿したときに 36MB を人数分持たずに済む
void mu2000::set_program_rom(u8rom p)
{
	m_prog = std::move(p);
	build_bus();
}

void mu2000::set_wave_rom(u8rom p)
{
	m_wave = std::move(p);
	if (!m_wave)
		return;
	m_swpm.set_wave_rom(m_wave->data(), m_wave->size());
	m_swps.set_wave_rom(m_wave->data(), m_wave->size());
}

void mu2000::set_sintab_rom(u16rom p)
{
	m_sintab = std::move(p);
	if (!m_sintab)
		return;
	m_swpm.set_sintab(m_sintab->data(), m_sintab->size());
	m_swps.set_sintab(m_sintab->data(), m_sintab->size());
}


bool mu2000::load_wave(const std::string &dir)
{
	// MAME は 4 つの 8MB を 32bit 語に交互に置いている。
	//   ic49 -> 語の下位 16bit（0x0000000 から）
	//   ic50 -> 語の上位 16bit
	//   ic53 / ic54 -> 0x1000000 語目から同じ形で
	static const char *names[4] = {
		"xv364a0.ic49", "xv365a0.ic50", "xw848a0.ic53", "xw849a0.ic54"
	};

	auto rom = std::make_shared<std::vector<u8>>(0x2000000, 0);   // 32MB
	for (int i = 0; i < 4; i++) {
		std::vector<u8> part;
		const std::string path = dir + "/" + names[i];
		if (!read_file(path, part, 0x800000)) {
			m_error = "波形 ROM を読めない（8MB でないか、見つからない）: " + path;
			return false;
		}
		const size_t base = (i >= 2) ? 0x1000000 : 0;
		const size_t off  = (i & 1) ? 2 : 0;
		for (size_t j = 0; j < part.size(); j += 2) {
			const size_t dst = base + j * 2 + off;
			(*rom)[dst + 0] = part[j + 0];
			(*rom)[dst + 1] = part[j + 1];
		}
	}

	set_wave_rom(std::move(rom));
	return true;
}


bool mu2000::load_sintab(const std::string &path)
{
	std::vector<u8> raw;
	if (!read_file(path, raw, 0x10000)) {
		m_error = "sin 表を読めない（64KB でないか、見つからない）: " + path;
		return false;
	}
	auto rom = std::make_shared<std::vector<u16>>(raw.size() / 2);
	for (size_t i = 0; i < rom->size(); i++)
		(*rom)[i] = u16(raw[i * 2] | (raw[i * 2 + 1] << 8));
	// 表は 1/4 周期を 0x8000（中心）から 0xffff（山）まで持つ形。MEG は後ろ半周期を ^0xffff で作るので、
	// 0 から始まる表だと山と谷の境目で値が 0 と 0xffff の間を跳び、深いコーラス（CELESTE・SYMPHONIC・CHORUS 3）に
	// 雑音が乗っていた。前の make_standins.py が作った 0 始まりの代替品は、ここで中心から始まる形に作り直す
	if (rom->size() == 0x8000 && (*rom)[0] < 0x4000) {
		for (size_t i = 0; i < rom->size(); i++)
			(*rom)[i] = u16(std::min(65535.0, std::round(0x8000 + std::sin((i + 0.5) / 0x8000 * 3.14159265358979323846 / 2) * 0x7fff)));
	}
	set_sintab_rom(std::move(rom));
	return true;
}


// ---- フロントパネル
//
// firmware は c80000 に「行」を書いてから同じ番地を読む。押されている桁が 0。
// 並びは MAME の mu500 の入力ポート（SWS0-SWS5）と同じ。

namespace {

struct button_slot { u8 row, bit; const char *name; };

// mu2000::button の並びと 1 対 1
const button_slot BUTTONS[] = {
	{ 0, 2, "Strings" },      { 0, 3, "Bass" },        { 0, 4, "Guitar" },
	{ 0, 5, "Organ" },        { 0, 6, "Chrom. Perc." }, { 0, 7, "Piano" },
	{ 1, 2, "Synth pad" },    { 1, 3, "Synth lead" },  { 1, 4, "Pipe" },
	{ 1, 5, "Reed" },         { 1, 6, "Brass" },       { 1, 7, "Ensemble" },
	{ 2, 2, "Drum" },         { 2, 3, "Model excl." }, { 2, 4, "SFX" },
	{ 2, 5, "Percussive" },   { 2, 6, "Ethnic" },      { 2, 7, "Synth effects" },
	{ 3, 1, "Part +" },       { 3, 2, "Part -" },      { 3, 3, "Mute/Solo" },
	{ 3, 4, "Effect" },       { 3, 5, "Util" },        { 3, 6, "Edit" },
	{ 3, 7, "Play" },
	{ 4, 1, "Value +" },      { 4, 2, "Value -" },     { 4, 3, "Exit" },
	{ 4, 4, "Select >" },     { 4, 5, "Select <" },    { 4, 6, "Enter" },
	{ 4, 7, "Seq" },
	{ 5, 5, "Audition" },     { 5, 6, "Select" },      { 5, 7, "Sampling/Mode" },
};

static_assert(sizeof(BUTTONS) / sizeof(BUTTONS[0]) == size_t(mu2000::button::count),
              "ボタンの表と enum がずれている");

} // namespace

const char *mu2000::button_name(button b)
{
	const int i = int(b);
	return (i >= 0 && i < int(button::count)) ? BUTTONS[i].name : "";
}

void mu2000::set_button(button b, bool pressed)
{
	const int i = int(b);
	if (i < 0 || i >= int(button::count))
		return;
	const button_slot &s = BUTTONS[i];
	if (pressed)
		m_sws[s.row] &= u8(~(1 << s.bit));
	else
		m_sws[s.row] |= u8(1 << s.bit);
}

bool mu2000::button_pressed(button b) const
{
	const int i = int(b);
	if (i < 0 || i >= int(button::count))
		return false;
	const button_slot &s = BUTTONS[i];
	return !BIT(m_sws[s.row], s.bit);
}

// 選ばれている行の押し具合を重ねて返す（MAME の mu500_state::ledsw_r と同じ）
u8 mu2000::ledsw_r() const
{
	u8 res = 0xff;
	for (u32 i = 0; i != 6; i++)
		if (BIT(m_ledsw1, i))
			res &= m_sws[i];
	return res;
}

// MAME の mulcd_device::set_leds に渡していた並びに直す
u16 mu2000::leds() const
{
	const u16 v = u16((u16(m_ledsw2) << 8) | m_ledsw1);
	// bitswap(v, 9,8,7,6,10,11,12,13,14,15) — 先頭が出来上がりの bit9
	static const int from[10] = { 9, 8, 7, 6, 10, 11, 12, 13, 14, 15 };
	u16 out = 0;
	for (int i = 0; i < 10; i++)
		out |= u16(BIT(v, from[i])) << (9 - i);
	return out;
}


bool mu2000::load_lcd_font(const std::string &path)
{
	auto rom = std::make_shared<std::vector<u8>>();
	if (!read_file(path, *rom, 0x1000)) {
		m_error = "LCD の字を読めない（4KB でないか、見つからない）: " + path;
		return false;
	}
	set_lcd_font(std::move(rom));
	return true;
}

// 代用の字の絵に足りない分を起こす。
//
// MU2000 の firmware は、LCD 下段の左 9 マスにレベルメータを描く。
// 1 マスに 2 本のバーが入っていて、文字コードが
//
//     0x89 + 9 × 左の高さ + 右の高さ      （高さは 0-8）
//
// になっている。無音だと全マス 0x89（両方 0）、鳴らすと 0xcf（両方いっぱい）
// まで上がる。実機の CGROM にはその絵が入っているが、こちらが持っている
// 代用フォントは ASCII しか無くて空白になってしまうので、規則から起こす。
// 0x80-0x88 は幅いっぱいの 1 本バーとして使われている。
//
// **本物の CGROM（MAME の mulcd.zip の hd44780u_b04.bin）を置けば、
// そちらが優先される**。空いているところだけ埋める
void mu2000::fill_missing_glyphs(std::vector<u8> &rom)
{
	auto blank = [&](int code) {
		for (int y = 0; y < 8; y++)
			if (rom[code * 16 + y] & 0x1f)
				return false;
		return true;
	};
	// 1 マスに 2 本。**バーの幅は 2 ドット**。左は 0-1 列、右は 3-4 列
	auto bar = [&](int code, int left, int right) {
		for (int y = 0; y < 8; y++) {
			u8 v = 0;
			if (y >= 8 - left)  v |= 0x18;
			if (y >= 8 - right) v |= 0x03;
			rom[code * 16 + y] = v;
		}
	};

	// レベルメータの字。この LCD の字の絵は手に入らないので、firmware が
	// 何を書くかを**測って**割り出した（doc/gui.md）。
	//
	//   コード = 0x7f + 9a + b   （a, b は 0-8）
	//   a = 左のバーの点の数、b = 右のバーの点の数
	//
	// 上の行と下の行で同じ表を使う。バーが上の行まで届かないときは
	// その側が 0、全部消えているマスには空白 (0x20) が入る。
	// 鳴っていないパートも 1 点だけ出る（a = b = 1、コード 0x89）
	for (int a = 0; a <= 8; a++)
		for (int b = 0; b <= 8; b++) {
			const int code = 0x7f + a * 9 + b;
			if (blank(code))
				bar(code, a, b);
		}
}

void mu2000::set_lcd_font(u8rom p)
{
	if (p && p->size() >= 0x1000) {
		auto patched = std::make_shared<std::vector<u8>>(*p);
		fill_missing_glyphs(*patched);
		m_lcd_font = std::move(patched);
	} else {
		m_lcd_font = std::move(p);
	}
	if (m_lcd_font)
		m_lcd.set_cgrom(m_lcd_font->data(), m_lcd_font->size());
}


void mu2000::build_bus()
{
	m_bus = mem_bus();

	// 000000-3fffff: プログラム ROM
	if (m_prog && !m_prog->empty())
		m_bus.add_region(0x000000, 0x3fffff, m_prog->data(), false);
	// 400000-43ffff: ワーク RAM
	m_bus.add_region(0x400000, 0x43ffff, m_ram.data(), true);
	// 1000000-107ffff: DRAM
	m_bus.add_region(0x1000000, 0x107ffff, m_dram.data(), true);
	// fffff000-ffffffff: CPU 内蔵 RAM
	m_bus.add_region(0xfffff000, 0xffffffff, m_iram.data(), true);

	// 800000-801fff: SWP30 マスタ / 802000-803fff: スレーブ。
	// レジスタは 16bit 単位なので、番地を 2 で割って渡す
	auto swp = [this](swp30_device &dev, u32 base) {
		// 実機のマスタの SWP30 は、書き込みを 1 サンプルに 1 回しか受け取れないらしく、書くたびに CPU は
		// 次のサンプルの区切りまで待たされる（BSC の WAIT）。遅れて鳴る層（firmware が 2.5ms ごとに数を
		// 減らして鍵を押す）の遅れが、これで実機と音ごとに ±10 サンプルで合う。待たせないと、音の設定の
		// 書き込み百回ほどが一瞬で終わり、実機より約 64 サンプル長くなっていた。
		// スレーブは待たせない（2.4kHz の割り込みが毎回ミキサを 7 つ書くので、待たせると CPU の 4 割が
		// 止まる。待たせると遅れが実機より 10 サンプル余計に長くなり、SLICE の位相も遠ざかる）。
		// 制御の 2 つ（0x0e / 0x0f）は、中身を書くもの（MEG のプログラムの中身 = チャンネル 0x11・0x12、
		// リバーブ RAM へ直に書く中身 = 0x26）だけ待たせ、番地・合図・状態は待たせない。エフェクトの種類を
		// 替えたときの読み込みの時間が、これで実機と合う（SLICE は表を 2052 項目書くので実機で 62ms 長い。
		// 刻みの位相もこれで合う。doc/upstream.md の 36）
		const bool waits = base == 0x800000;
		auto hold = [this, waits](offs_t reg) {
			const u32 slot = reg & 0x3f;
			const u32 chan = (reg >> 6) & 0x3f;
			const bool control = slot == 0x0e || slot == 0x0f;
			const bool data = chan == 0x11 || chan == 0x12 || chan == 0x26;
			if (waits && (!control || data)) {
				m_swp_hold = true;
				m_cpu->abort_timeslice();
			}
		};
		mem_bus::device d;
		d.start = base;
		d.end   = base + 0x1fff;
		d.r16 = [this, &dev, base](offs_t a) {
			const u16 v = dev.read16((a - base) >> 1);
			if (m_swp_trace && m_swp_trace_reads)
				std::fprintf(m_swp_trace, "R %08x %04x %04x  pc=%08x\n", base, (a - base) >> 1, v, m_cpu->pc());
			return v;
		};
		// 幅の内訳を数える。MAME は 16bit ハンドラに mem_mask を渡せるが
		// こちらは渡せないので、byte 幅の書き込みがあると片側が壊れる
		d.w8 = [this, &dev, base, hold](offs_t a, u8 v) {
			m_swp_w8++;
			const offs_t reg = (a - base) >> 1;
			const u16 old = dev.read16(reg);
			dev.write16(reg, (a & 1) ? u16((old & 0xff00) | v)
			                         : u16((old & 0x00ff) | (u16(v) << 8)));
			hold(reg);
		};
		d.r8 = [this, &dev, base](offs_t a) {
			m_swp_r8++;
			return u8(dev.read16((a - base) >> 1) >> ((a & 1) ? 0 : 8));
		};
		d.w32 = [this, &dev, base, hold](offs_t a, u32 v) {
			m_swp_w32++;
			const offs_t reg = (a - base) >> 1;
			if (m_swp_trace) {
				std::fprintf(m_swp_trace, "%s%08x %04x %04x  pc=%08x\n",
				             m_swp_trace_reads ? "W " : "", base, reg, u16(v >> 16), m_cpu->pc());
				std::fprintf(m_swp_trace, "%s%08x %04x %04x  pc=%08x\n",
				             m_swp_trace_reads ? "W " : "", base, reg + 1, u16(v), m_cpu->pc());
			}
			dev.write16(reg, u16(v >> 16));
			dev.write16(reg + 1, u16(v));
			hold(reg);
		};
		d.w16 = [this, &dev, base, hold](offs_t a, u16 v) {
			m_swp_w16++;
			if (m_swp_trace)
				std::fprintf(m_swp_trace, "%s%08x %04x %04x  pc=%08x\n",
				             m_swp_trace_reads ? "W " : "", base, (a - base) >> 1, v, m_cpu->pc());
			dev.write16((a - base) >> 1, v);
			hold((a - base) >> 1);
		};
		return d;
	};
	m_bus.add_device(swp(m_swpm, 0x800000));
	m_bus.add_device(swp(m_swps, 0x802000));

	// c80000: LED ラッチとスイッチ走査、e00000: LED ラッチその 2。
	// 音には関わらないが、firmware が起動時に触るので受けておく
	{
		mem_bus::device d;
		d.start = 0xc80000; d.end = 0xc80000;
		d.r8 = [this](offs_t) { return ledsw_r(); };
		d.w8 = [this](offs_t, u8 v) { m_ledsw1 = v; };
		m_bus.add_device(d);
	}
	{
		mem_bus::device d;
		d.start = 0xe00000; d.end = 0xe00000;
		d.w8 = [this](offs_t, u8 v) { m_ledsw2 = v; };
		m_bus.add_device(d);
	}

	// c00000: SmartMedia のデータ、d00000: 制御の留め金（smartmedia.h）
	{
		mem_bus::device d;
		d.start = 0xc00000; d.end = 0xc7ffff;
		d.r8 = [this](offs_t) { return m_card.data_r(); };
		d.w8 = [this](offs_t, u8 v) { m_card.data_w(v); };
		m_bus.add_device(d);
	}
	{
		mem_bus::device d;
		d.start = 0xd00000; d.end = 0xd7ffff;
		d.r8 = [](offs_t) -> u8 { return 0xff; };
		d.w8 = [this](offs_t, u8 v) { m_card.control_w(v); };
		m_bus.add_device(d);
	}

	// f00000-f0003f: PLG ボード用の SCI4。ボードは挿さないが register は生きている
	{
		mem_bus::device d;
		d.start = 0xf00000; d.end = 0xf0003f;
		d.r8 = [this](offs_t a) { return m_sci4->read8(a - 0xf00000); };
		d.w8 = [this](offs_t a, u8 v) { m_sci4->write8(a - 0xf00000, v); };
		m_bus.add_device(d);
	}

	// ffff8000-ffff9fff: CPU の内蔵周辺（sh7042_map.hxx が振り分ける）
	{
		mem_bus::device d;
		d.start = 0xffff8000; d.end = 0xffff9fff;
		d.r8  = [this](offs_t a) { return m_cpu->internal_r8(a); };
		d.r16 = [this](offs_t a) { return m_cpu->internal_r16(a); };
		d.r32 = [this](offs_t a) { return m_cpu->internal_r32(a); };
		d.w8  = [this](offs_t a, u8 v)  { m_cpu->internal_w8(a, v); };
		d.w16 = [this](offs_t a, u16 v) { m_cpu->internal_w16(a, v); };
		d.w32 = [this](offs_t a, u32 v) { m_cpu->internal_w32(a, v); };
		m_bus.add_device(d);
	}

	m_cpu->set_program_bus(&m_bus);
}


// ポート E は LCD の 8bit バス。上位バイトがデータ、下位が制御線。
// MAME の mu500_state::pe_r / pe_w と同じ形にしてある
//   bit 4: E（立ち下がりで確定）  bit 2: RS（1 でデータ）  bit 0: R/W
u16 mu2000::lcd_port_r()
{
	m_lcd.set_now(m_cpu->total_cycles());
	if (BIT(m_pe, 4)) {
		if (BIT(m_pe, 0))
			return u16((BIT(m_pe, 2) ? m_lcd.data_r() : m_lcd.control_r()) << 8);
		return 0x0000;
	}
	return 0;
}

void mu2000::lcd_port_w(u16 data)
{
	m_lcd.set_now(m_cpu->total_cycles());
	if (BIT(m_pe, 4) && !BIT(data, 4)) {        // E の立ち下がり
		if (!BIT(data, 0)) {                    // R/W = 0、つまり書き込み
			if (BIT(data, 2))
				m_lcd.data_w(u8(data >> 8));
			else
				m_lcd.control_w(u8(data >> 8));
		}
	}
	m_pe = data;
}

void mu2000::update_sci_irq()
{
	m_cpu->execute_set_input(0, (m_sci_irq[0] || m_sci_irq[1]) ? ASSERT_LINE : CLEAR_LINE);
}

void mu2000::start_devices()
{
	// MAME はスケジューラが順に呼ぶ。こちらは生成順にそのまま呼ぶ
	for (auto &d : m_config.m_devices)
		d->device_start();
}


void mu2000::reset()
{
	// ポート A。MAME の mu500_state::pa_r は 0xffff を返すだけだったが、
	// そこに付いていた覚え書きに配線が書いてある。
	//   21 出力（前面と背面の MIDI A を切り替える）
	//   20 smvprt / 19 smvins / 18 smbusy（スマートカード）
	//   17 rea / 16 reb        ← **前面の大きなダイヤル**
	//
	// firmware は 2.5ms ごと（400Hz）にここを読む。読んだときに
	// bit17 が立っていれば 1 目盛りぶん動いたとみなし、bit16 で向きを決める。
	// 位相を細かく作るのではなく、走査 1 回につき 1 目盛りを渡せばよい。
	// **0xffff には bit16/17 が入っていない**（MAME が返していた値は
	// 「ダイヤルが止まっている」に当たる）ので、立てる側で書く。
	//
	// この決まりは実測で出した。bit17 を上げっぱなしにすると音色番号が
	// 最後（128 Gunshot）まで走り、bit16 も一緒に上げると逆に動く
	m_cpu->read_porta().set([this]() {
		u32 v = 0xffff;
		// SmartMedia の線（firmware は 0xFFFF8380 の下の 8bit で見る）:
		//   PA18 (0x04) 忙しい（0 で準備ができている。firmware は 0 になるのを待つ）/ PA19 (0x08) 差し込まれている /
		//   PA20 (0x10) 書き込みを禁じていない
		// 読み書きはその場で済むので、忙しい印は立てない
		if (m_card.inserted()) {
			v |= 1u << 19;
			if (!m_card.write_protected)
				v |= 1u << 20;
		}
		if (m_enc_pending) {
			if (m_enc_pending < 0) v |= 1u << 16;   // B 相は向きのあいだ立てておく
			if (m_enc_high) {
				v |= 1u << 17;                      // A 相の立ち上がりで 1 目盛り
				m_enc_high = false;
			} else {
				m_enc_high = true;
				m_enc_pending += (m_enc_pending > 0) ? -1 : 1;
			}
		}
		return v;
	});

	// A/D 変換。MAME の配線と同じ。
	// **電池の残量を返さないと起動画面が「Battery Low!」のままになる**
	// AN0 と AN2 は A/D INPUT の大きさ（AD1 と AD2）。サンプリングの REC の画面のレベルメーターとトリガに使う。
	// firmware は起動から AN0-AN3 を回し続け（ADCSR0 = 0xb3）、ADDR の上 8bit を 0xff から引いて使う（2.01 の 0x116196、0x13b6e6）。
	// つまり静かなほど値が大きい。引いた値が 0x18 以下でメーター 0、0x85 以上で振り切れる（0x13b78c）。
	// 実機の検波の回路は分からないので、ピーク（すぐ上がり、0.1 秒で 1/e に下がる）を 0x18 から 0x85 に割り当てる
	m_cpu->read_adc<0>().set([this]() { return ad_level_adc(0); });
	m_cpu->read_adc<1>().set_constant(0);
	m_cpu->read_adc<2>().set([this]() { return ad_level_adc(1); });
	m_cpu->read_adc<3>().set_constant(0);
	m_cpu->read_adc<4>().set_constant(0);        // ホストスイッチ = MIDI
	m_cpu->read_adc<5>().set_constant(0);
	m_cpu->read_adc<6>().set_constant(0x3ff);    // 電池は満タン
	m_cpu->read_adc<7>().set_constant(0);
	m_cpu->read_porte().set([this]() { return lcd_port_r(); });
	m_cpu->write_porte().set([this](u16 v) { lcd_port_w(v); });

	m_lcd.reset();

	// SCI4 の割り込み。MAME は 0 と 1 を input_merger で束ねて CPU の IRQ0 に、
	// 3 を IRQ1 に入れていた
	m_sci4->write_irq<0>().set([this](int s) { m_sci_irq[0] = s; update_sci_irq(); });
	m_sci4->write_irq<1>().set([this](int s) { m_sci_irq[1] = s; update_sci_irq(); });
	m_sci4->write_irq<3>().set([this](int s) { m_cpu->execute_set_input(1, s); });

	// 2 個のチップで乱数の数列を分ける。同じ種だと雑音まで揃ってしまう
	m_swpm.set_rand_seed(0x9d14abd7);
	m_swps.set_rand_seed(0x6c1f35e9);
	m_swpm.reset();
	m_swps.reset();

	// MIDI IN の線は何も来ていないとき High
	m_cpu->sci_rx_w<0>(1);
	m_cpu->sci_rx_w<1>(1);

	// MIDI OUT。SCI ch0 の送信線（MAME も ch0 を mdout へ繋いでいる）
	m_tx_r = m_tx_w = 0;
	m_tx_bit = -1;
	m_cpu->write_sci_tx<0>().set([this](int s) { tx_line(s); });

	start_devices();

	for (auto &d : m_config.m_devices)
		d->device_reset();
}


void mu2000::run_cycles(u64 n)
{
	// 前回はみ出した分を先に返す
	if (m_overrun >= n) { m_overrun -= n; return; }
	n -= m_overrun;
	m_overrun = 0;

	// MAME ではスケジューラがやっていたこと。周辺の予定を跨がないように区切る。
	// MAME は予定の時刻ちょうどで CPU を止めてタイマを鳴らし、そのあと再開する。
	// 周辺がレジスタ書き込みに反応して新しい予定を入れた場合は、CPU が
	// abort_timeslice() でその場で戻ってくるので、ここで組み直す
	int idle = 0;
	while (n) {
		m_loops++;
		const u64 now = m_cpu->total_cycles();
		m_machine.set_cycles(now);

		// MAME のスケジューラが持っていたタイマ（SCI4 の送受信など）
		const u64 tmr = m_machine.next_timer_cycles();
		if (tmr <= now) {
			m_timer_fires++;
			m_machine.run_timers(now);
			m_machine.set_cycles(now);
			continue;
		}

		const u64 ev  = m_cpu->event_cycles();

		if (ev && now >= ev) {
			m_event_fires++;
			m_cpu->event_tick();
			if (m_cpu->event_cycles() == ev && ++idle > 2)
				break;          // 予定が動かない。放っておくと止まる
			continue;
		}
		idle = 0;

		// MIDI のビット送出も跨がないように
		midi_step(now);

		u64 chunk = n;
		if (ev && ev - now < chunk)
			chunk = ev - now;
		if (tmr != ~u64(0) && tmr - now < chunk)
			chunk = tmr - now;
		if (!m_fast_midi)
			for (const midi_line &m : m_midi)
				if (m.bit >= 0 || !m.queue.empty()) {
					const u64 left = m.next > now ? m.next - now : 1;
					if (left < chunk)
						chunk = left;
				}
		// SWP30 に書いた後は、このサンプルの残りを命令を進めずに過ごす（上の swp の説明）。
		// 周辺のタイマや MIDI の送出は、区切りごとにここまでで進めている
		if (m_swp_hold) {
			m_cpu->skip_cycles(chunk);
			n = chunk >= n ? 0 : n - chunk;
			continue;
		}

		const int done = m_cpu->run_cycles(int(chunk));
		if (done <= 0) {
			if (m_cpu->event_cycles() == ev)
				break;
			continue;
		}
		// 命令の途中では止まれないので、頼まれた数より少し多く走ることがある。
		// 出た分は捨てずに次の呼び出しから引く（捨てると CPU が音より速くなる）
		if (u64(done) >= n) {
			m_overrun += u64(done) - n;
			n = 0;
		} else
			n -= u64(done);
	}
	m_swp_hold = false;
}

// ダイヤルを 1 位相ぶん進める。
//
// 実機のエンコーダは A 相と B 相が 1/4 周期ずれて開閉する。firmware は
// その順番で向きを読むので、位相をまとめて飛ばしてはいけない。
void mu2000::tx_line(int state)
{
	if (m_tx_bit < 0) {
		if (!state) {            // スタートビット
			m_tx_bit = 0;
			m_tx_cur = 0;
		}
		return;
	}
	if (m_tx_bit < 8) {
		m_tx_cur |= u8((state ? 1 : 0) << m_tx_bit);
		m_tx_bit++;
		return;
	}
	// ストップビット。0 なら枠がずれているので、その 1 バイトは捨てる
	m_tx_bit = -1;
	if (!state)
		return;
	const size_t next = (m_tx_w + 1) & TX_MASK;
	if (next == m_tx_r)
		return;                  // 溢れ。誰も読んでいない
	m_tx_buf[m_tx_w] = m_tx_cur;
	m_tx_w = next;
}

void mu2000::midi_step(u64 now)
{
	// A と B は別々の SCI に繋がっている。互いに待たせない
	for (int port = 0; port < MIDI_PORTS; port++) {
		midi_line &m = m_midi[port];
		sh_sci_device *sci = m_cpu->sci(port);
		if (m_fast_midi) {
			if (!m.queue.empty() && sci->rx_can_accept()) {
				const u8 byte = m.queue.front();
				m.queue.pop_front();
				logerror("midi in %c %02x @ %llu (fast)\n", 'A' + port, byte,
				         (unsigned long long)now);
				sci->receive_byte(byte);
			}
			continue;
		}

		if (m.bit < 0) {
			// 直前のバイトのストップビットぶんは空けてから次を出す
			if (m.queue.empty() || now < m.next)
				continue;
			m.cur = m.queue.front();
			m.queue.pop_front();
			m.bit  = 0;
			m.next = now + MIDI_BIT_CYCLES;
			logerror("midi in %c %02x @ %llu\n", 'A' + port, m.cur,
			         (unsigned long long)now);
			sci->do_rx_w(0);            // スタートビット
			continue;
		}

		if (now < m.next)
			continue;

		m.bit++;
		m.next = now + MIDI_BIT_CYCLES;
		if (m.bit <= 8)
			sci->do_rx_w((m.cur >> (m.bit - 1)) & 1);   // 下位ビットから
		else {
			sci->do_rx_w(1);            // ストップビット
			m.bit = -1;
		}
	}
}


void mu2000::run_sample(s32 &left, s32 &right)
{
	// 台数が変わっていたら別スレッドの使い方を見直す（8192 サンプルごと）
	if (m_want_threaded && !(++m_thread_check & 0x1fff))
		apply_threading();

	// SWP30 は 44100Hz で 1 サンプル。CPU はその間に 28MHz/44100 ≒ 634.9 サイクル
	m_cycle_debt += 28000000;
	const u64 cycles = m_cycle_debt / 44100;
	m_cycle_debt -= cycles * 44100;

	// 内訳を測る（set_profile(true) のときだけ）
	// (smu2000::perf_ticks() is QueryPerformanceCounter on Windows, so the
	//  measurement is the same one on both platforms -- see compat/platform.h)
	u64 pt0 = 0, pt1 = 0, pt2 = 0;
	if (m_profile)
		pt0 = smu2000::perf_ticks();

	run_cycles(cycles);

	if (m_profile) {
		pt1 = smu2000::perf_ticks();
		m_t_cpu += pt1 - pt0;
	}

	// マスタとスレーブを 1 サンプルずつ進める。
	// 別スレッドが空いていればスレーブをそちらに投げ、同時に走らせる
	s32 lm = 0, rm = 0, ls = 0, rs = 0;
	if (m_slave_thread.joinable()) {
		const u64 tag = m_slave_go.load(std::memory_order_relaxed) + 1;
		m_slave_go.store(tag, std::memory_order_release);
		m_slave_go.notify_one();   // 眠っていたら起こす。起きていれば素通り
		m_swpm.run_sample(lm, rm);
		while (m_slave_done.load(std::memory_order_acquire) != tag)
			smu2000::cpu_pause();
		ls = m_slave_l;
		rs = m_slave_r;
	} else {
		m_swpm.run_sample(lm, rm);
		m_swps.run_sample(ls, rs);
	}

	if (m_profile) {
		pt2 = smu2000::perf_ticks();
		m_t_swpm += pt2 - pt1;
		m_t_n++;
	}

	// 2 個の SWP30 は MELO/MELI のシリアルで相互に結ばれている。
	// スレーブの声は自分の DAC には出ず、この線でマスタのミキサに入る。
	// 結線は MAME の mu1000_state::mu1000() と同じ:
	//   スレーブ 出力 4..17 -> マスタ  入力 0..13
	//   マスタ   出力 4..9, 12..13 -> スレーブ 入力 0..5, 8..9
	// **マスタからスレーブの 6 と 7 の線は無い**。実機にも MAME にも無いので繋いではいけない。
	// 繋ぐと、マスタのミキサ出力 3 番（melo 6/7）がスレーブへ回り込み、
	// スレーブ→マスタの線と合わせて輪になってしまう（スレーブの 6 と 7 には A/D INPUT が入る。下を参照）
	// 相互に繋がっているので 1 サンプル遅れで渡す（MAME も同じ）
	for (int i = 0; i < 14; i++)
		m_swpm.set_meli(i, m_swps.melo(i));
	static const int TO_SLAVE[] = { 0, 1, 2, 3, 4, 5, 8, 9 };
	for (int i : TO_SLAVE)
		m_swps.set_meli(i, m_swpm.melo(i));
	// A/D INPUT はスレーブの入力 6（AD1）と 7（AD2）に入る。上のマスタからの線が飛ばしている 2 本で、
	// A/D パートの音量を上げると firmware がここをミキサに通す（エミュで線を 1 本ずつ試して決めた）。
	// サンプリングの録音も、この 2 本をミキサの出力 8 に集めて録る（swp30.cpp の sample_step）。
	// 目盛りは 16bit を 8bit 上げた 24bit にしている（実機の入力の大きさとはまだ突き合わせていない）
	m_swps.set_meli(6, m_ad_in[0] * 256);
	m_swps.set_meli(7, m_ad_in[1] * 256);
	// レベルメーター用の検波（AN0 / AN2）
	for (int i = 0; i < 2; i++) {
		const s32 a = std::min(std::abs(m_ad_in[i]), 32767);
		m_ad_peak[i] = a >= m_ad_peak[i] ? a : m_ad_peak[i] - ((m_ad_peak[i] >> 12) + 1);
	}

	// スピーカーに出るのはマスタの DAC だけ。
	// スレーブの DAC はどこにも繋がっていない
	left  = lm;
	right = rm;
}

// ---- 状態の保存と復元
//
// ROM（プログラム・波形・sin 表・字の絵）は入れない。戻すときは同じものを
// 積んでおくこと。調べもの用の数え上げも入れない。

namespace {

// 保存の形。中身の並びを変えたら上げる
constexpr u32 STATE_MAGIC   = 0x554d3253;   // "S2MU"
constexpr u32 STATE_VERSION = 6;   // 2: MIDI の入口が A/B の 2 口になった / 3: SWP30 のピッチ EG / 4: サンプリングの録音の位置 / 5: SmartMedia の命令の途中 / 6: MEG の印と 2 つ目の idx
constexpr u32 STATE_VERSION_OLDEST = 2;

} // namespace

void mu2000::state(state_io &s)
{
	s.tag("mu2000");
	m_machine.state_sync(s);

	// 主記憶。番地の割り振りは build_bus() と同じ
	s.mem(m_ram.data(),     m_ram.size());
	s.mem(m_dram.data(),    m_dram.size());
	s.mem(m_iram.data(),    m_iram.size());
	s.mem(m_sampram.data(), m_sampram.size());
	// 版 5 から: SmartMedia の命令の途中の状態（カードの中身は入れない）
	if (s.version() >= 5)
		m_card.state(s);

	if (m_cpu)  m_cpu->state(s);
	m_swpm.state(s);
	m_swps.state(s);
	m_lcd.state(s);
	if (m_sci4) m_sci4->state(s);

	s.tag("panel");
	s.v(m_ledsw1); s.v(m_ledsw2); s.arr(m_sws);
	s.v(m_enc_pending); s.v(m_enc_high); s.v(m_pe);
	s.arr(m_sci_irq);
	s.v(m_cycle_debt);
	// **前のサンプルからのはみ出し**。これが無いと、戻した直後の 1 サンプルで
	// CPU の回す量が数サイクルずれる
	s.v(m_overrun);

	// 受け取り途中の MIDI。A と B の 2 口ぶん
	s.tag("midi");
	for (midi_line &m : m_midi) {
		u32 n = u32(m.queue.size());
		s.v(n);
		if (s.writing()) {
			for (u8 b : m.queue)
				s.v(b);
		} else {
			m.queue.clear();
			for (u32 i = 0; i < n && s.ok(); i++) {
				u8 b = 0;
				s.v(b);
				m.queue.push_back(b);
			}
		}
		s.v(m.bit); s.v(m.cur); s.v(m.next);
	}
}

std::vector<u8> mu2000::save_state() const
{
	std::vector<u8> out;
	state_io s(out);
	u32 magic = STATE_MAGIC, ver = STATE_VERSION;
	s.v(magic);
	s.v(ver);
	s.set_version(ver);
	const_cast<mu2000 *>(this)->state(s);
	return out;
}

bool mu2000::load_state(const u8 *p, size_t n, std::string &err)
{
	state_io s(p, n);
	u32 magic = 0, ver = 0;
	s.v(magic);
	s.v(ver);
	if (!s.ok() || magic != STATE_MAGIC) {
		err = "これは S-MU2000 の状態ではない";
		return false;
	}
	if (ver < STATE_VERSION_OLDEST || ver > STATE_VERSION) {
		err = "状態の形が違う（この版では読めない）";
		return false;
	}
	s.set_version(ver);
	state(s);
	if (!s.ok()) {
		err = s.error();
		return false;
	}
	// MIDI OUT の途中の枠と溜めは保存していない。空から始める
	m_tx_r = m_tx_w = 0;
	m_tx_bit = -1;
	return true;
}
