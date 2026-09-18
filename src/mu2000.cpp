// license:BSD-3-Clause
//
// MU2000 一台ぶんの組み立て。配置は MAME の ymmu2000.cpp と同じ。

#include "mu2000.h"

#if defined(__SSE2__) || defined(_M_X64) || defined(__x86_64__)
#include <xmmintrin.h>
#include <pmmintrin.h>
#endif

#include "xg/ram.h"
#include "xg/voices.h"
#include "xg/fx_params.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>

#include "compat/platform.h"


namespace {

// MIDI は 31250bps。28MHz の CPU から見て 1 ビット = 896 サイクル
constexpr u64 MIDI_BIT_CYCLES = 28000000 / 31250;

// USB は実機で 19,500 byte/s 出た（doc/dump/usb.md）。1 バイトぶんのサイクル数
constexpr u64 USB_BYTE_CYCLES = 28000000 / 19500;

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
		// 実機のマスタの SWP30 へ書くと、CPU は 1 本あたり **440 サイクル**（15.7 マイクロ秒、
		// 1 サンプルの 0.69 ぶん）待たされる（BSC の WAIT）。書き込み百回ほどが一瞬で終わる形にすると、
		// 遅れて鳴る層の遅れが実機より約 64 サンプル短くなる。
		//
		// 440 という数は実機から直に測った。XG モードでパート 1 と 2 を同じ受信チャンネルにして
		// 1 つのノートオンで鳴らし、左右へ振ると、左右の立ち上がりの差がそのまま
		// 「firmware が 1 パートぶんのレジスタを書く時間」になる（キーオンの間の待つ書き込みは 68 本）。
		// 実機 61.4 サンプル（8 音、標準偏差 1.0）に対し、この値で 61.1（doc/upstream.md の 36）。
		//
		// スレーブは待たせない（2.4kHz の割り込みが毎回ミキサを 7 つ書くので、待たせると CPU の 4 割が
		// 止まる。待たせると遅れが実機より 10 サンプル余計に長くなり、SLICE の位相も遠ざかる）。
		// 制御の 2 つ（0x0e / 0x0f）は、中身を書くもの（MEG のプログラムの中身 = チャンネル 0x11・0x12、
		// リバーブ RAM へ直に書く中身 = 0x26）だけ待たせ、番地・合図・状態は待たせない。エフェクトの種類を
		// 替えたときの読み込みの時間が、これで実機と合う（SLICE は表を 2052 項目書くので実機で 62ms 長い）
		const bool waits = base == 0x800000;
		auto hold = [this, waits](offs_t reg) {
			const u32 slot = reg & 0x3f;
			const u32 chan = (reg >> 6) & 0x3f;
			const bool control = slot == 0x0e || slot == 0x0f;
			const bool data = chan == 0x11 || chan == 0x12 || chan == 0x26;
			if (waits && (!control || data)) {
				m_swp_wait += SWP_WRITE_CYCLES;
				m_cpu->abort_timeslice();
			}
		};
		mem_bus::device d;
		d.start = base;
		d.end   = base + 0x1fff;
		d.r16 = [this, &dev, base](offs_t a) {
			const u16 v = dev.read16((a - base) >> 1);
			if (m_swp_trace && m_swp_trace_reads)
				std::fprintf(m_swp_trace, "R %08x %04x %04x  pc=%08x  t=%.6f s=%llu\n", base, (a - base) >> 1, v, m_cpu->pc(), double(m_cpu->total_cycles()) / 28000000.0, (unsigned long long)trace_sample());
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
				std::fprintf(m_swp_trace, "%s%08x %04x %04x  pc=%08x  t=%.6f s=%llu\n",
				             m_swp_trace_reads ? "W " : "", base, reg, u16(v >> 16), m_cpu->pc(), double(m_cpu->total_cycles()) / 28000000.0, (unsigned long long)trace_sample());
				std::fprintf(m_swp_trace, "%s%08x %04x %04x  pc=%08x  t=%.6f s=%llu\n",
				             m_swp_trace_reads ? "W " : "", base, reg + 1, u16(v), m_cpu->pc(), double(m_cpu->total_cycles()) / 28000000.0, (unsigned long long)trace_sample());
			}
			if (m_swp_watch) {
				m_swp_watch(base == 0x800000, reg, u16(v >> 16));
				m_swp_watch(base == 0x800000, reg + 1, u16(v));
			}
			dev.write16(reg, u16(v >> 16));
			dev.write16(reg + 1, u16(v));
			hold(reg);
		};
		d.w16 = [this, &dev, base, hold](offs_t a, u16 v) {
			m_swp_w16++;
			if (m_swp_trace)
				std::fprintf(m_swp_trace, "%s%08x %04x %04x  pc=%08x  t=%.6f s=%llu\n",
				             m_swp_trace_reads ? "W " : "", base, (a - base) >> 1, v, m_cpu->pc(), double(m_cpu->total_cycles()) / 28000000.0, (unsigned long long)trace_sample());
			if (m_swp_watch)
				m_swp_watch(base == 0x800000, (a - base) >> 1, v);
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

	// f80000-f80001: USB の M37640 マイコン。SH-2 から見えるのはこの 2 番地だけ。
	// 読みは 0 が受信バイト、1 が状態。書きは 0 が MIDI、1 が M37640 への指示
	{
		mem_bus::device d;
		d.start = 0xf80000; d.end = 0xf80001;
		d.r8 = [this](offs_t a) { return usb_r(a - 0xf80000); };
		d.w8 = [this](offs_t a, u8 v) { usb_w(a - 0xf80000, v); };
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
	// 実機の M37640 は、PC に繋がっていると「ホストが居る」を知らせてくる
	// （状態の bit6 を立てて F4 03 01 01 01。0x43810 が受け、0x43DAD1 を 1 にする）。
	// これが来ないと、HOST SELECT が USB のとき firmware は起動の途中（0x1167CE）で
	// 液晶に「HOST Is Offline!」を出す。エミュでは PC が常に繋がっているので、起動時に 1 回送る
	m_usb.cmd.clear();
	m_usb.cur_cmd = false;
	// ケーブルメッセージで回した口は、電源を入れ直せば元に戻る
	for (int p = 0; p < MIDI_PORTS; p++) {
		m_cable[p] = p;
		m_cable_wait[p] = false;
	}
	if (m_usb_host)
		for (u8 b : { 0xf4, 0x03, 0x01, 0x01, 0x01 })
			m_usb.cmd.push_back(b);

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
	// ホストスイッチ。firmware は 8 ビットに落として境で分ける（0x1098）。
	// 0x20 未満が MIDI、0xBA-0xE0 が USB
	m_cpu->read_adc<4>().set([this]() -> u16 { return m_usb_host ? 0x330 : 0; });
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
		usb_step(now);

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
		// SWP30 に書いた後は、その待ちぶんだけ命令を進めずに時間を送る（上の swp の説明）。
		// 周辺のタイマや MIDI の送出は、区切りごとにここまでで進めている
		if (m_swp_wait) {
			const u64 skip = std::min<u64>(m_swp_wait, chunk);
			m_cpu->skip_cycles(skip);
			m_swp_wait -= skip;
			n = skip >= n ? 0 : n - skip;
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

// ---- USB（M37640）の代役
//
// 溜めに積むときに口が変わっていれば `F5 <口>` を先に挟む。firmware 側は
// 0x042932 で 0xF5 を見て次のバイトを「今の口」として覚え、以後のバイトを
// その口として 0x04437C へ渡す。口は 1 始まり（1=A 2=B 3=C 4=D）

void mu2000::usb_midi_in(u8 byte, int port)
{
	usb_line &u = m_usb;
	if (u.rx.size() >= MIDI_QUEUE_LIMIT) {
		m_midi_dropped.fetch_add(1, std::memory_order_relaxed);
		return;
	}
	if (port != u.in_port) {
		u.rx.push_back(0xf5);
		u.rx.push_back(u8(port + 1));
		u.in_port = port;
	}
	u.rx.push_back(byte);
}

void mu2000::usb_step(u64 now)
{
	usb_line &u = m_usb;

	// USB を使っていないときは何もしない。割り込みを上げると firmware の
	// USB ドライバが動き出してしまう
	if (!m_usb_host && u.rx.empty() && !u.have)
		return;

	// 受信。1 バイト渡すごとに IRQ3（ベクタ 67）を上げる。
	// 間隔は実機で測った USB の実効帯域 19,500 byte/s に合わせる
	// （doc/dump/usb.md の実測）。DIN の 3,125 byte/s より 6 倍速いが、
	// 発音の間隔は firmware 側が頭打ちなので実測とは食い違わない。
	// 4 つの口が 1 本の流れを分け合うので、遅くすると互いに待たせてしまう
	if (!u.have && now >= u.next && (!u.cmd.empty() || !u.rx.empty())) {
		// コマンドを先に渡す
		std::deque<u8> &q = u.cmd.empty() ? u.rx : u.cmd;
		u.cur_cmd = !u.cmd.empty();
		u.cur  = q.front();
		u.have = true;
		q.pop_front();
		u.next = now + (m_fast_midi ? 0 : USB_BYTE_CYCLES);
	}
	// 送信の線を一度下ろす。下で上げ直すので、山は 1 標本ぶんになる
	m_cpu->execute_set_input(2, 0);
	// **読まれるまで上げておく**。実機の M37640 は「受信あり」を線で示しているので、
	// firmware が受け取りを止めている間に来たバイトも、止めるのをやめた時点で必ず拾われる。
	// 渡した瞬間に 1 回だけ上げる形にしていたため、firmware が受信を詰まらせて
	// IRQ3 の優先度を 0 に落としている隙に渡すと、優先度を戻しても二度と上がらず、
	// 以後 MIDI を 1 バイトも受け取らなくなっていた（USB の口へ 1 秒に 2 万バイト近い
	// 設定データを流すと起きる。X で報告された testxg.mid）
	if (u.have)
		m_cpu->execute_set_input(3, 1);

	// 送信。firmware は IRQ2（ベクタ 66）が来るたびに 1 バイト出す。
	// 上げないとリングが埋まり、0x437A0 の空き待ちで固まる（実機でやらかした）
	if (now >= u.tx_next) {
		u.tx_next = now + USB_BYTE_CYCLES;
		m_cpu->execute_set_input(2, 1);
	}
}

u8 mu2000::usb_r(offs_t a)
{
	usb_line &u = m_usb;
	if (a & 1)
		return u.have ? (u.cur_cmd ? 0x41 : 0x01) : 0x00;   // bit0 = 受信あり、bit6 = コマンド
	// 受け取られたのでその場で線を下ろす。次の標本まで待つと、その隙に
	// 割り込みがもう一度入って同じバイトを二度読まれてしまう
	u.have = false;
	m_cpu->execute_set_input(3, 0);
	return u.cur;
}

void mu2000::usb_w(offs_t a, u8 v)
{
	if (a & 1)
		return;                        // コマンド口。M37640 への指示なので捨てる
	usb_line &u = m_usb;
	if (u.tx.size() < TX_SIZE)
		u.tx.push_back(v);
}

bool mu2000::usb_out_take(u8 &v, int &port)
{
	usb_line &u = m_usb;
	while (!u.tx.empty()) {
		const u8 b = u.tx.front();
		u.tx.pop_front();
		if (b == 0xf5) {
			if (u.tx.empty()) {        // 口の番号がまだ来ていない。戻しておく
				u.tx.push_front(b);
				return false;
			}
			u.out_port = int(u.tx.front()) - 1;
			u.tx.pop_front();
			continue;
		}
		v = b;
		port = u.out_port;
		return true;
	}
	return false;
}

void mu2000::midi_step(u64 now)
{
	// A と B は別々の SCI に繋がっている。互いに待たせない
	for (int port = 0; port < MIDI_DIN_PORTS; port++) {
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



// ---- native の口（doc/native-engine.md の段 2）

void mu2000::set_native_engine(int mode)
{
	m_native_engine = mode;
	m_fw_hold = 0;
	m_learning = false;
	m_learn_left = 0;
	for (u8 &c : m_fw_notes)
		c = 0;
	for (part_prog &p : m_prog_sel)
		p = part_prog();
	m_fw_note_total = 0;
	m_fw_note_until = 0;
	m_nq.clear();
	m_sx_pos = -1;
	m_traj_rec = false;
	m_traj_left = 0;
	m_traj_cals = nullptr;
	m_ne_clock = 0;
	for (u64 &t : m_rx_at)
		t = 0;
	std::memset(m_nown, 0, sizeof(m_nown));
	for (nmidi &n : m_nmidi)
		n = nmidi();
	m_ne_samples.store(0, std::memory_order_relaxed);
	m_ne_fw_samples.store(0, std::memory_order_relaxed);
	m_ne_by_note.store(0, std::memory_order_relaxed);
	m_ne_by_sysex.store(0, std::memory_order_relaxed);
	m_ne_by_other.store(0, std::memory_order_relaxed);
	m_ne_by_learn.store(0, std::memory_order_relaxed);
	m_ne_by_midi.store(0, std::memory_order_relaxed);
	m_fw_why = 0;
	m_ne_stats = native_stats();
	if (!mode) {
		set_swp_watch(nullptr);
		return;
	}
	m_ndrv.reset();
	m_ndrv.set_rom(m_prog ? m_prog->data() : nullptr);
	m_ndrv.set_ram(m_ram.data());
	m_ndrv.set_poke([this](u32 reg, u16 value) { m_swpm.write16(reg, value); });
}

// 音色の 1 音目を firmware に鳴らさせて、スロットに書かれた値を写し取る
void mu2000::native_learn_start(u32 rec)
{
	m_learning = true;
	m_learn_rec = rec;
	// その鍵・強さで鳴るはずの要素の数
	m_learn_want = 1;
	if (rec && m_prog) {
		const u8 *rom0 = m_prog->data();
		int n = 0;
		const int nel = xg::nv::element_count(rom0, rec);
		for (int k = 0; k < nel; k++)
			if (xg::nv::element_active(xg::nv::element(rom0, rec, k), m_learn_note, m_learn_vel))
				n++;
		if (n > 0)
			m_learn_want = n;
	}
	m_learn_first.clear();
	m_learn_last.clear();
	m_learn_traj.clear();
	m_learn_key_clock = 0;
	m_learn_mask = m_learn_keyed = 0;
	// レジスタは鍵を押した所でまとめて書かれるので、短くてよい。
	// 長くすると、その間の音が全部 firmware に回ってしまう。
	// ただし短すぎると 0x01（鳴らしてから上がっていく）が落ち着く前に切れる
	m_learn_left = 44100 / 50;          // 20ms ぶん見る
	set_swp_watch([this](bool master, u32 reg, u16 value) {
		if (!master)
			return;
		m_learn_last[reg] = value;
		// 鍵を押したあとのフィルタ・LFO の動きを、時刻つきで控えておく
		if (m_learn_key_clock) {
			const int r2 = int(reg % 64);
			if ((r2 == 0x00 || r2 == 0x01 || r2 == 0x04 || r2 == 0x05 || r2 == 0x0a) &&
			    reg < 0x1000 && m_learn_traj.size() < 512)
				m_learn_traj.push_back({ int(reg / 64),
				    xg::nv::fstep{ u32(m_ne_clock - m_learn_key_clock), u8(r2), value } });
		}
		switch (reg) {
		case 0x18e: m_learn_mask = (m_learn_mask & ~(u64(0xffff) << 48)) | (u64(value) << 48); break;
		case 0x18f: m_learn_mask = (m_learn_mask & ~(u64(0xffff) << 32)) | (u64(value) << 32); break;
		case 0x1ce: m_learn_mask = (m_learn_mask & ~(u64(0xffff) << 16)) | (u64(value) << 16); break;
		case 0x1cf: m_learn_mask = (m_learn_mask & ~u64(0xffff)) | value; break;
		case 0x20e:
			// **要素のぶんだけ**。速い曲では、写し取りの窓の中に次の音の
			// 引き金が入ってしまい、余計なスロットまで拾っていた
			if (__builtin_popcountll(m_learn_keyed) < m_learn_want)
				m_learn_keyed |= m_learn_mask;
			if (m_learn_first.empty())
				m_learn_first = m_learn_last;
			if (!m_learn_key_clock)
				m_learn_key_clock = m_ne_clock;
			// 鳴り始めたら、あと少しだけ見て終える（0x01 が落ち着くぶん）。
			// ただし**要素がそろうまでは待つ**。MusicBox のように 2 つ目の要素を
			// 37ms 遅れて鳴らす音色があり、打ち切ると片方しか写し取れない。
			// 長く占有すると、その間ほかの音色が写し取りを始められないので、
			// そろったら 5ms で切り上げる
			m_learn_left = __builtin_popcountll(m_learn_keyed) >= m_learn_want
			             ? 44100 / 200 : 44100 / 16;
			break;
		default: break;
		}
	});
}

void mu2000::native_learn_finish()
{
	set_swp_watch(nullptr);
	m_learning = false;
	if (!m_learn_keyed || !m_prog)
		return;
	// ドラムは、音色の記録が引けないので中身を写すだけ（音ごとに覚える）
	if (m_learn_drum) {
		std::vector<xg::nv::voice_cal> cals;
		for (int ch = 0; ch < 64; ch++) {
			if (!(m_learn_keyed & (u64(1) << ch)))
				continue;
			xg::nv::voice_cal cal;
			for (int i = 0; i < 0x40; i++) {
				const bool at_key = (i == 0x05 || i == 0x0a || i == 0x11);
				const std::map<u32, u16> &src = at_key ? m_learn_first : m_learn_last;
				const auto it = src.find(u32(ch) * 64 + u32(i));
				if (it != src.end())
					cal.set(i, it->second);
			}
			if (!cal.has(0x16) || !cal.has(0x17))
				continue;
			if (int(cals.size()) >= m_learn_want)
				break;
			cal.cal_vel  = m_learn_vel;
			cal.cal_vol  = m_ndrv.part_vol(m_learn_part);
			cal.cal_expr = m_ndrv.part_expr(m_learn_part);
			cal.cal_pan  = m_ndrv.part_pan(m_learn_part);
			cal.cal_mod  = m_ndrv.part_mod(m_learn_part);
			cal.cal_rev  = m_ndrv.part_rev(m_learn_part);
			cal.cal_cho  = m_ndrv.part_cho(m_learn_part);
			cal.cal_bri  = m_ndrv.part_bri(m_learn_part);
			cal.cal_res  = m_ndrv.part_res(m_learn_part);
			cal.cal_ctx  = m_ndrv.part_ctx(m_learn_part);
			cal.have = true;
			cals.push_back(cal);
		}
		const int ndcal = int(cals.size());
		const u64 dkey = m_learn_drum;
		m_ndrv.learn_drum(m_learn_drum, std::move(cals));
		traj_start(0, dkey, ndcal);
		m_learn_drum = 0;
		return;
	}
	// 波形の番地まで取れていなければ、写し取りとして使えない（次の音でやり直す）
	{
		bool ok = false;
		for (int ch = 0; ch < 64 && !ok; ch++)
			if ((m_learn_keyed & (u64(1) << ch)) &&
			    m_learn_last.count(u32(ch) * 64 + 0x16) && m_learn_last.count(u32(ch) * 64 + 0x17))
				ok = true;
		if (!ok)
			return;
	}
	const u8 *rom = m_prog->data();
	const int nel = xg::nv::element_count(rom, m_learn_rec);
	unsigned used_elem = 0;
	std::vector<xg::nv::voice_cal> cals;
	for (int ch = 0; ch < 64; ch++) {
		if (!(m_learn_keyed & (u64(1) << ch)))
			continue;
		if (int(cals.size()) >= nel)     // 要素より多く拾わない
			break;
		xg::nv::voice_cal cal;
		for (int i = 0; i < 0x40; i++) {
			// 0x05・0x0a・0x11 は LFO が動かし続けるので引き金の瞬間、
			// ほかは落ち着いた値（doc/native-engine.md の 6.10）
			const bool at_key = (i == 0x05 || i == 0x0a || i == 0x11);
			const std::map<u32, u16> &src = at_key ? m_learn_first : m_learn_last;
			const auto it = src.find(u32(ch) * 64 + u32(i));
			if (it != src.end())
				cal.set(i, it->second);
		}
		// どの要素かは、そのスロットが鳴らしている波形の番地で見分ける。
		// 同じ波形の要素が 2 つあるときは、まだ使っていないほうを取る
		int idx = -1;
		if (cal.has(0x16) && cal.has(0x17)) {
			const u32 want = u32(cal.reg[0x16]) << 16 | cal.reg[0x17];
			for (int k = 0; k < nel; k++) {
				if (used_elem & (1u << k))
					continue;
				const u8 *e2 = xg::nv::element(rom, m_learn_rec, k);
				const u8 *w2 = xg::nv::wave_entry(rom, xg::nv::wave_set(e2), xg::nv::wave_note(e2, m_learn_note));
				if (w2 && xg::nv::read_wave(w2).format_addr == want) {
					idx = k;
					used_elem |= 1u << k;
					break;
				}
			}
		}
		if (idx < 0)
			idx = int(cals.size()) < nel ? int(cals.size()) : 0;
		cal.base_level = xg::nv::calibrate_level(rom, xg::nv::element(rom, m_learn_rec, idx),
		                                         cal.has(9) ? (cal.reg[9] & 0xff) : 64,
		                                         m_learn_note, m_learn_vel);
		cal.cal_vel  = m_learn_vel;
		cal.cal_vol  = m_ndrv.part_vol(m_learn_part);
		cal.cal_expr = m_ndrv.part_expr(m_learn_part);
		cal.cal_pan  = m_ndrv.part_pan(m_learn_part);
		cal.cal_mod  = m_ndrv.part_mod(m_learn_part);
		cal.cal_rev  = m_ndrv.part_rev(m_learn_part);
		cal.cal_cho  = m_ndrv.part_cho(m_learn_part);
		cal.cal_bri  = m_ndrv.part_bri(m_learn_part);
		cal.cal_res  = m_ndrv.part_res(m_learn_part);
		cal.cal_ctx  = m_ndrv.part_ctx(m_learn_part);
		cal.have = true;
		cals.push_back(cal);
	}
	const int ncal = int(cals.size());
	if (std::getenv("SMU2000_NATIVE_DEBUG")) {
		std::fprintf(stderr, "learn rec=%06x 要素 %d 写し %d 鍵いた %d part=%d ctx=%08x\n",
		             m_learn_rec, nel, ncal, __builtin_popcountll(m_learn_keyed),
		             m_learn_part, m_ndrv.part_ctx(m_learn_part));
		for (int k = 0; k < ncal; k++) {
			const xg::nv::voice_cal &c = cals[size_t(k)];
			const u8 *e2 = xg::nv::element(rom, m_learn_rec, k);
			const u8 *w2 = xg::nv::wave_entry(rom, xg::nv::wave_set(e2), xg::nv::wave_note(e2, m_learn_note));
			std::fprintf(stderr, "  写し%d 0x11=%04x 0x32=%04x 0x09=%04x 波形=%08x"
			                     " / 式 0x11=%04x 要素b18=%d b0=%d b1=%d\n",
			             k, c.reg[0x11], c.reg[0x32], c.reg[0x09], c.wave_addr(),
			             w2 ? xg::nv::pitch_reg(xg::nv::read_wave(w2), m_learn_note,
			                                    xg::nv::key_follow(e2)) : 0,
			             e2[18], e2[0], e2[1]);
			if (w2)
				std::fprintf(stderr, "        こちらの波形=%08x 基準鍵=%d 微調=%d 上限鍵=%d 追従=%d 組=%d%s",
				             xg::nv::read_wave(w2).format_addr, xg::nv::read_wave(w2).base_key,
				             xg::nv::read_wave(w2).fine_cents, xg::nv::read_wave(w2).key_max,
				             xg::nv::key_follow(e2), xg::nv::wave_set(e2), "\n");
		}
	}
	m_ndrv.learn(m_learn_rec, std::move(cals));
	traj_start(m_learn_rec, 0, ncal);
}


// ---- 写し取りをファイルに残す・戻す（voicecache.h）
//
// 形は「頭 → 記録ごと」。記録 1 つぶんは
//   種類(1) 鍵(8) 写しの数(2) ／ 写しごとに
//   覚えたレジスタの印(8)・素の音量(2)・写したときの強さ/音量/表現/パン(2 ずつ)
//   ・印の立っているレジスタの値(2 ずつ)・包絡線の段数(2)・段ごとに 時刻(4) 番地(1) 値(2)

namespace {

constexpr u32 CAL_MAGIC = 0x43563253u;   // "S2VC"
constexpr u32 CAL_VERSION = 6;

void put8(std::vector<u8> &v, u8 x) { v.push_back(x); }
void put16v(std::vector<u8> &v, u16 x) { v.push_back(u8(x)); v.push_back(u8(x >> 8)); }
void put32v(std::vector<u8> &v, u32 x) { for (int i = 0; i < 4; i++) v.push_back(u8(x >> (i * 8))); }
void put64v(std::vector<u8> &v, u64 x) { for (int i = 0; i < 8; i++) v.push_back(u8(x >> (i * 8))); }

struct rd {
	const u8 *p, *e;
	bool ok = true;
	u8 g8() { if (p + 1 > e) { ok = false; return 0; } return *p++; }
	u16 g16() { const u8 a = g8(), b = g8(); return u16(a | (b << 8)); }
	u32 g32() { u32 x = 0; for (int i = 0; i < 4; i++) x |= u32(g8()) << (i * 8); return x; }
	u64 g64() { u64 x = 0; for (int i = 0; i < 8; i++) x |= u64(g8()) << (i * 8); return x; }
};

void write_cals(std::vector<u8> &out, u8 kind, u64 key, const std::vector<xg::nv::voice_cal> &cals)
{
	put8(out, kind);
	put64v(out, key);
	put16v(out, u16(cals.size()));
	for (const xg::nv::voice_cal &c : cals) {
		put64v(out, c.mask);
		put16v(out, u16(c.base_level));
		put16v(out, u16(c.cal_vel));
		put16v(out, u16(c.cal_vol));
		put16v(out, u16(c.cal_expr));
		put16v(out, u16(c.cal_pan));
		put16v(out, u16(c.cal_mod));
		put16v(out, u16(c.cal_rev));
		put16v(out, u16(c.cal_cho));
		put16v(out, u16(c.cal_bri));
		put16v(out, u16(c.cal_res));
		put32v(out, c.cal_ctx);
		for (int i = 0; i < 0x40; i++)
			if (c.mask & (u64(1) << i))
				put16v(out, c.reg[i]);
		const u16 n = u16(std::min<size_t>(c.filter_env.size(), 4096));
		put16v(out, n);
		for (int i = 0; i < n; i++) {
			put32v(out, c.filter_env[i].at);
			put8(out, c.filter_env[i].reg);
			put16v(out, c.filter_env[i].v);
		}
	}
}

} // namespace

std::vector<u8> mu2000::native_cal_save() const
{
	const auto &cal = m_ndrv.cal_map();
	const auto &drum = m_ndrv.drum_map();
	if (cal.empty() && drum.empty())
		return {};
	std::vector<u8> out;
	put32v(out, CAL_MAGIC);
	put32v(out, CAL_VERSION);
	put32v(out, u32(cal.size() + drum.size()));
	for (const auto &kv : cal)
		write_cals(out, 0, kv.first, kv.second);
	for (const auto &kv : drum)
		write_cals(out, 1, kv.first, kv.second);
	return out;
}

bool mu2000::native_cal_load(const u8 *data, size_t n)
{
	rd r{ data, data + n };
	if (r.g32() != CAL_MAGIC || r.g32() != CAL_VERSION || !r.ok)
		return false;
	const u32 count = r.g32();
	if (count > 100000)
		return false;
	for (u32 k = 0; k < count && r.ok; k++) {
		const u8 kind = r.g8();
		const u64 key = r.g64();
		const u16 ncal = r.g16();
		if (ncal > 8 || !r.ok)
			return false;
		std::vector<xg::nv::voice_cal> cals;
		for (u16 c2 = 0; c2 < ncal && r.ok; c2++) {
			xg::nv::voice_cal c;
			c.mask = r.g64();
			c.base_level = s16(r.g16());
			c.cal_vel = s16(r.g16());
			c.cal_vol = s16(r.g16());
			c.cal_expr = s16(r.g16());
			c.cal_pan = s16(r.g16());
			c.cal_mod = s16(r.g16());
			c.cal_rev = s16(r.g16());
			c.cal_cho = s16(r.g16());
			c.cal_bri = s16(r.g16());
			c.cal_res = s16(r.g16());
			c.cal_ctx = r.g32();
			for (int i = 0; i < 0x40; i++)
				if (c.mask & (u64(1) << i))
					c.reg[i] = r.g16();
			const u16 steps = r.g16();
			if (steps > 4096 || !r.ok)
				return false;
			c.filter_env.reserve(steps);
			for (u16 i = 0; i < steps && r.ok; i++) {
				xg::nv::fstep s;
				s.at = r.g32();
				s.reg = r.g8();
				s.v = r.g16();
				c.filter_env.push_back(s);
			}
			c.have = true;
			cals.push_back(std::move(c));
		}
		if (!r.ok)
			return false;
		if (kind == 0)
			m_ndrv.learn(u32(key), std::move(cals));
		else
			m_ndrv.learn_drum(key, std::move(cals));
	}
	return true;
}

// 写し取った音が鳴っている間、firmware がフィルタ（0x00・0x01・0x04）を
// どう動かすかを録る。あとの音でも同じように動かせば、音色の動きまで揃う
void mu2000::traj_start(u32 rec, u64 drum_key, int ncal)
{
	if (!ncal)
		return;
	m_traj_cals = drum_key ? m_ndrv.drum_cals_of(drum_key) : m_ndrv.cals_of(rec, m_learn_part);
	if (!m_traj_cals)
		return;
	// 写し取ったチャンネルの順が、そのまま写し取りの並び
	int n = 0;
	for (int ch = 0; ch < 64; ch++)
		m_traj_chan[ch] = (m_learn_keyed & (u64(1) << ch)) ? n++ : -1;
	// 鍵を押した瞬間からの控えを、まず入れる
	{
		int n2 = 0;
		int idx[64];
		for (int ch = 0; ch < 64; ch++)
			idx[ch] = (m_learn_keyed & (u64(1) << ch)) ? n2++ : -1;
		for (const auto &e : m_learn_traj)
			if (e.first < 64 && idx[e.first] >= 0 &&
			    size_t(idx[e.first]) < m_traj_cals->size())
				(*m_traj_cals)[idx[e.first]].filter_env.push_back(e.second);
	}
	m_learn_traj.clear();
	m_traj_n = 0;
	m_traj_rec = true;
	m_ndrv.set_recording(true);
	m_traj_rec_key = rec;
	m_traj_drum_key = drum_key;
	m_traj_start = m_learn_key_clock ? m_learn_key_clock : m_ne_clock;
	m_traj_left = 44100;                 // 1 秒ぶん見る
	set_swp_watch([this](bool master, u32 reg, u16 value) {
		if (!master)
			return;
		const int ch = int(reg / 64), r = int(reg % 64);
		if (ch >= 64 || m_traj_chan[ch] < 0)
			return;
		// 離しに入ったらそこで打ち切る（離しの動きは鳴らすときには要らない）。
		// ただし鳴らし始めてすぐは見ない。前の音の離しが同じスロットに来る
		if (r == 0x09 && (value & 0x8000)) {
			if (m_ne_clock - m_traj_start > 44100 / 10)
				m_traj_left = 1;
			return;
		}
		// フィルタ（0x00・0x01・0x04）と LFO（0x05・0x0a）。
		// LFO は「かけ始めるまでの間」や深さの増やし方を firmware がソフトでやっている
		if (r != 0x00 && r != 0x01 && r != 0x04 && r != 0x05 && r != 0x0a)
			return;
		if (m_traj_n >= 2048 || size_t(m_traj_chan[ch]) >= m_traj_cals->size())
			return;
		// **その場で**写し取りに足す。いま鳴っている native の音も、
		// 次の tick でこの段を拾う（xg/native_driver.h の tick）
		(*m_traj_cals)[m_traj_chan[ch]].filter_env.push_back(
		    xg::nv::fstep{ u32(m_ne_clock - m_traj_start), u8(r), value });
		m_traj_n++;
	});
}

void mu2000::traj_finish()
{
	set_swp_watch(nullptr);
	m_traj_rec = false;
	m_ndrv.set_recording(false);
	if (std::getenv("SMU2000_NATIVE_DEBUG"))
		std::fprintf(stderr, "traj rec=%06x drum=%llx 段 %u%c", m_traj_rec_key,
		             (unsigned long long)m_traj_drum_key, m_traj_n, 10);
	m_traj_cals = nullptr;
}

// バンクとプログラムから音色の記録を引いて、native の口に渡す。
// firmware がワーク RAM に入れるのを待たなくて済む（引き方は
// xg::voice_rom::lookup。旋律系のバンク 640 音色で firmware と食い違い 0）
void mu2000::native_select_voice(int part)
{
	if (part < 0 || part >= 64 || !m_prog)
		return;
	const part_prog &p = m_prog_sel[part];
	const bool drum = (p.msb == 127 || p.msb == 126);
	if (drum) {
		m_ndrv.set_record(part, 0, 1);
		return;
	}
	const xg::voice_rom vr(m_prog);
	const int mode = m_ram.size() > xg::ram::VOICE_MODE ? m_ram[xg::ram::VOICE_MODE] : 1;
	const int set  = m_ram.size() > xg::ram::VOICE_SET  ? m_ram[xg::ram::VOICE_SET]  : 1;
	const u32 rec = vr.lookup(mode, set, p.msb, p.lsb, p.prog);
	m_ndrv.set_record(part, rec, rec ? 0 : -1);
}

// 待っている native の出来事を、時が来たものから実行する
void mu2000::native_pump()
{
	while (!m_nq.empty() && m_nq.front().at <= m_ne_clock) {
		const nev e = m_nq.front();
		m_nq.pop_front();
		switch (e.kind) {
		case 0: m_ndrv.note_off(e.part, e.d0); break;
		case 1:
			if (m_ndrv.note_on(e.part, e.d0, e.d1))
				m_ne_stats.note_native++;
			break;
		case 2: m_ndrv.control(e.part, e.d0, e.d1); break;
		case 3: m_ndrv.bend(e.part, int(e.d1) << 7 | e.d0); break;
		default: break;
		}
	}
}

// MIDI を 1 バイト受けて、native でさばけたら true。
// さばけなかったもの（音色の指定・コントローラ・SysEx）は firmware へ回す
bool mu2000::native_midi(u8 byte, int port)
{
	if (byte >= 0xf8)
		return false;                    // リアルタイムはそのまま
	// 実機は 1 バイトずつ線で受ける。和音のように何音も一度に来ると、
	// あとの音ほど遅れて鳴る。そのぶんをここで数える
	const u64 fire = rx_advance(port);
	nmidi &n = m_nmidi[port];
	if (byte & 0x80) {
		if (byte >= 0xf0) {              // SysEx など。以後は firmware に任せる
			n.status = 0;
			if (byte == 0xf0) {
				// 頭を見て決めるので、まずは短く。0x0b 番目までに分かる
				m_sx_pos = 0;
				m_fw_hold = std::max(m_fw_hold, u32(44100 / 30));
			} else {
				m_sx_pos = -1;
				m_fw_hold = std::max(m_fw_hold, u32(44100 / 30));
			}
			if (m_fw_why != 1)
				m_fw_why = 2;
			return false;
		}
		n.status = byte;
		n.have = 0;
		// アフタータッチ。**割り当て（CAT / PAT）が既定なら音に何も起きない**ので、
		// そのときは firmware に任せなくてよい（doc/native-engine.md の 6.43）
		if ((byte & 0xf0) == 0xd0 || (byte & 0xf0) == 0xa0)
			m_ndrv.aftertouch((byte & 0x0f) + port * 16, (byte & 0xf0) == 0xa0);
		// 鍵の上げ下げ・CC・ベンドはこちらで見る。残り（音色の指定など）は firmware へ
		const u8 kind = byte & 0xf0;
		if (kind == 0xc0)
			return true;                     // 音色の指定は下でバイトを見る
		if (kind != 0x80 && kind != 0x90 && kind != 0xb0 && kind != 0xe0) {
			m_ne_stats.other++;
			// 音色の指定。ワーク RAM に入るのは 30 サンプル（0.68ms）で済むが
			// （nativeplay --ccwatch）、firmware の中の下ごしらえはもっとかかる。
			// 10ms だと piano の残差が -58dB から -54dB に落ちたので 20ms 見る
			m_fw_hold = std::max(m_fw_hold, u32(44100 / 50));
			if (m_fw_why != 1)
				m_fw_why = 2;
			return false;
		}
		return true;                     // 状態のバイトは飲み込む
	}
	// SysEx の中身。エフェクトの種類を変えるものだけ長く回す（MEG のプログラムを
	// 1 万件以上書き直すので、途中で止めると音が出なくなる）。
	// パートの設定（08 pp xx）やドラムの設定は短くてよい
	if (m_sx_pos >= 0) {
		// 長い SysEx（MEG のプログラムなど）の間は待ちを切らさない
		m_fw_hold = std::max(m_fw_hold, u32(44100 / 200));
		if (m_sx_pos < 5)
			m_sx[m_sx_pos] = byte;
		if (++m_sx_pos == 5) {
			const bool yamaha_param = m_sx[0] == 0x43 && (m_sx[1] & 0xf0) == 0x10;
			// 43 1n 4C hh … の hh。00 システム / 02 エフェクト / 03 インサーション
			const u8 hh = m_sx[3];
			const bool heavy = !yamaha_param || hh == 0x00 || hh == 0x02 || hh == 0x03;
			if (heavy) {
				// 実測（nativeplay --ccwatch）で SWP30 を触り終わるまで
				// XG On が 224ms、リバーブの種類が 168ms、インサーションが 176ms。
				// 余裕を見て 300ms（前は 500ms だった）
				m_fw_hold = std::max(m_fw_hold, u32(44100 * 3 / 10));
				m_fw_why = 1;
			}
			m_sx_pos = -1;
		}
		return false;
	}

	const u8 kind = n.status & 0xf0;
	// 音色の指定（1 バイト）。自分で記録を引いて、firmware にも渡す
	if (kind == 0xc0) {
		const int part2 = (n.status & 0x0f) + port * 16;
		m_prog_sel[part2].prog = byte & 0x7f;
		native_select_voice(part2);
		m_ne_stats.other++;
		// 記録はこちらで引けたが、firmware も自分の下ごしらえに時間が要る
		// （5ms に詰めると piano の残差が -58dB から -53dB に落ちる）
		m_fw_hold = std::max(m_fw_hold, u32(44100 / 50));
		if (m_fw_why != 1)
			m_fw_why = 2;
		const int save2 = m_native_engine;
		m_native_engine = 0;
		midi_in(n.status, port);
		midi_in(byte, port);
		m_native_engine = save2;
		return true;
	}
	if (kind != 0x80 && kind != 0x90 && kind != 0xb0 && kind != 0xe0)
		return false;
	if (n.have == 0) {
		n.d0 = byte;
		n.have = 1;
		return true;
	}
	n.have = 0;
	const int part = (n.status & 0x0f) + port * 16;

	// ピッチベンドは、音程のレジスタを自分で作れるので firmware には渡さない。
	// ただし、そのパートで firmware が鳴らしている音がある間は渡す
	if (kind == 0xe0) {
		m_nq.push_back({ fire, 3, u8(part), u8(n.d0 & 0x7f), u8(byte & 0x7f) });
		if (m_fw_notes[part]) {
			m_fw_hold = std::max(m_fw_hold, u32(44100 / 200));
			replay_note(n.status, n.d0, byte, port);
		}
		return true;
	}
	// コントローラ。音量・表現・パン・ダンパーは自分でさばく。
	// それでも firmware には渡す（写し取りのとき同じ位置で鳴らしてほしい）が、
	// 回す時間は短くてよい
	if (kind == 0xb0) {
		m_ne_stats.other++;
		const int cc = n.d0 & 0x7f;
		if (cc == 0x00) { m_prog_sel[part].msb = byte & 0x7f; native_select_voice(part); }
		if (cc == 0x20) { m_prog_sel[part].lsb = byte & 0x7f; native_select_voice(part); }
		const bool mine = m_ndrv.handles_cc(n.d0 & 0x7f);
		if (mine)
			m_nq.push_back({ fire, 2, u8(part), u8(n.d0 & 0x7f), u8(byte & 0x7f) });
		else
			m_ndrv.control(part, n.d0 & 0x7f, byte & 0x7f);   // 音を全部切るなどは待たない
		// こちらでさばける CC（音量・パン・ダンパー）は firmware に渡すだけなので短く。
		// 知らない CC は firmware がすべてやるので、処理が終わるまで見る
		// （5ms に詰めたら bend の残差が -35.8dB から -14dB に落ちた）
		// こちらでさばける CC は渡すだけなので短く。ただし **そのパートを
		// firmware が鳴らしている間**は、firmware に効かせてもらうので長く見る
		// （渡したバイトは列に並ぶので、写し取りで回すときに順に処理される）
		// まだ写し取っていないパートは、1 音目を firmware が鳴らすので、
		// CC も firmware に効かせてもらう
		const bool quick = mine && !m_fw_notes[part] && m_ndrv.part_learned(part);
		m_fw_hold = std::max(m_fw_hold, u32(quick ? 44100 / 500 : 44100 / 50));
		replay_note(n.status, n.d0, byte, port);
		return true;
	}
	const int note = n.d0 & 0x7f, vel = byte & 0x7f;
	if (kind == 0x80 || vel == 0) {
		if (nown(part, note)) {
			nown_set(part, note, false);
			m_nq.push_back({ fire, 0, u8(part), u8(note), u8(vel) });
			return true;
		}
		// native で鳴っていない音は firmware に任せる
		if (m_fw_notes[part]) {
			m_fw_notes[part]--;
			if (m_fw_note_total)
				m_fw_note_total--;
		}
		m_fw_hold = std::max(m_fw_hold, u32(44100 / 50));    // 離しの下ごしらえまで
		replay_note(n.status, u8(note), u8(vel), port);
		return true;
	}
	if (m_ndrv.can_play(part, note)) {
		nown_set(part, note, true);
		m_nq.push_back({ fire, 1, u8(part), u8(note), u8(vel) });
		return true;
	}
	m_ne_stats.note_fw++;
	// firmware が鳴らす音でも、最後に押した鍵は覚えておく
	// （つぎの音のポルタメントの出発点になる）
	m_ndrv.note_fw(part, note);
	// まだ写し取っていない音（ドラムは音ごと）。firmware に鳴らさせて覚える
	const u32 rec = m_ndrv.record_of(part);
	const bool drum = m_ndrv.is_drum(part);
	// 知らない CC で firmware に任せているパートは、写し取っても使わない
	if ((rec || drum) && !m_learning && !m_ndrv.delegated(part)) {
		m_learn_note = note;
		m_learn_vel = vel;
		m_learn_drum = drum ? m_ndrv.drum_key(part, note) : 0;
		m_learn_part = part;
		m_ne_stats.learn++;
		native_learn_start(rec);
		if (m_fw_why != 1)
			m_fw_why = 3;
	}
	m_fw_hold = std::max(m_fw_hold, u32(44100 / 20));
	if (m_fw_notes[part] < 255) {
		m_fw_notes[part]++;
		m_fw_note_total++;
	}
	m_fw_note_until = m_ne_clock + FW_NOTE_RUN;
	replay_note(n.status, u8(note), u8(vel), port);
	return true;
}

// 飲み込んだバイトを firmware へ流し直す。
// **回す時間も作る**。渡しただけでは、CPU を止めたままなので誰も読まない
void mu2000::replay_note(u8 status, u8 d0, u8 d1, int port)
{
	const int save = m_native_engine;
	m_native_engine = 0;                 // 二重に読まない
	midi_in(status, port);
	midi_in(d0, port);
	midi_in(d1, port);
	m_native_engine = save;
	// **待ちはここでは置かない。** 以前はここで一律 20ms 回していて、
	// CC を 1 つ渡すたびに 20ms 走らせることになっていた（曲の出だしの重さの正体）。
	// 要るぶんは呼ぶ側が置く
}

// S-MU2000: 軽量モードの入り切り（doc/native-dsp.md）
void mu2000::set_native_fx(int mode)
{
	m_nfx_on = mode;
	// 遅延の線は作り直さない（音声の糸が読んでいる最中に切り替えても危なくないように）。
	// 大きさは 44100Hz ぶんで固定なので、1 度用意すれば足りる。
	// **台ごとに持つ**。前は関数の static で、DAW に 2 枚目を挿すと
	// 2 台目の遅延の線が空のままになって落ちていた
	// 非正規化数（0 に近すぎる値）を 0 に丸める設定は、**ここでは触らない**。
	// 糸ごとの設定なので、音声の糸が入れ替わると消えてしまうし、
	// 音源として挿されている側が host の糸の設定を変えたままにするのも行儀が悪い。
	// 1 ブロックごとに smu2000::denormals_off を置く（compat/platform.h）
	if (!m_nfx_ready) {
		m_nfx.set_rate(44100.0f);
		m_nfx_ready = true;
	}
	m_nfx.reset();
	int mask = 15;
	if (const char *e = std::getenv("SMU2000_NATIVE_SLOTS"))
		mask = std::atoi(e);
	m_swpm.set_native_fx(mode ? &m_nfx : nullptr, mode >= 2, mask);
	if (mode)
		native_fx_update();
}

namespace {

// XG の番地から、ワーク RAM の値を読む（7bit ずつ。無ければ -1）
int xg_read(const std::vector<u8> &ram, int hi, int mid, int lo, int size)
{
	int v = 0;
	for (int i = 0; i < size; i++) {
		u32 off = 0;
		if (!xg::ram::locate(u32(hi << 14 | mid << 7 | (lo + i)), off) || off >= ram.size())
			return -1;
		v = (v << 7) | (ram[off] & 0x7f);
	}
	return v;
}

// インサーション n のパラメータ 1-10 が 2 バイトの種類のときの値（16bit がそのまま並ぶ）
int ins_wide(const std::vector<u8> &ram, int n, int addr)
{
	// 2 バイトのパラメータは 0x30, 0x32, ... と 2 番地ずつ使い、RAM にも 2 バイトずつ並ぶ。
	// つまり RAM での位置は「番地の差」そのもの（前は 2 倍していて 1 つおきに読んでいた）
	const u32 off = xg::ram::INS_BLOCK[n] + xg::ram::INS_WIDE + u32(addr - 0x30);
	if (off + 1 >= ram.size())
		return -1;
	return ram[off] << 8 | ram[off + 1];
}

} // namespace

// RAM に入っている XG の設定を読んで、C++ のエフェクトに渡す。
// 音を作る糸から 512 サンプルごとに呼ぶ（設定はそんなに速く変わらない）
void mu2000::native_fx_update()
{
	using nfx = smu2000::dsp::native_fx;
	const std::vector<u8> &ram = m_ram;
	if (ram.size() < 0x30000)
		return;

	struct slot_def { nfx::slot_id id; int hi, mid, base, ret_lo, ins; };
	static const slot_def SLOTS[] = {
		{ nfx::REVERB,    0x02, 0x01, 0x00, 0x0c, -1 },
		{ nfx::CHORUS,    0x02, 0x01, 0x20, 0x2c, -1 },
		{ nfx::VARIATION, 0x02, 0x01, 0x40, 0x56, -1 },
		{ nfx::INS1,      0x03, 0x00, 0x00, -1,    0 },
	};

	// パラメータの並びは、置き場ごとに違う（表の addr はインサーションの番地）。
	//   リバーブ・コーラス … 1-10 は base+02〜0B の 1 バイト、11-16 は base+10〜15
	//   バリエーション     … 1-10 は 02 01 42 から 2 バイトずつ、11-16 は 02 01 70〜75
	//   インサーション     … 表の番地そのまま（2 バイトのものは +0x18 に 16bit で並ぶ）
	auto read_param = [&](const slot_def &s, const xg::fx_param &p, int index) {
		if (s.ins >= 0)
			return p.addr >= 0x30 ? ins_wide(ram, s.ins, p.addr)
			                      : xg_read(ram, s.hi, s.mid, s.base + p.addr, p.size);
		if (s.id == nfx::VARIATION) {
			if (p.addr >= 0x30 || index < 10)
				return xg_read(ram, s.hi, s.mid, 0x42 + 2 * index, 2);
			return xg_read(ram, s.hi, s.mid, 0x70 + (p.addr - 0x20), 1);
		}
		if (p.addr >= 0x20)
			return xg_read(ram, s.hi, s.mid, s.base + 0x10 + (p.addr - 0x20), 1);
		return xg_read(ram, s.hi, s.mid, s.base + p.addr, p.size);
	};

	for (const slot_def &s : SLOTS) {
		const int type = xg_read(ram, s.hi, s.mid, s.base, 2);
		if (type < 0)
			continue;
		const xg::fx_def *def = xg::fx_find(type);
		int raw[16] = {};
		const int n = def ? std::min(def->count, 16) : 0;
		for (int i = 0; i < n; i++) {
			const xg::fx_param &p = def->params[i];
			const int v = read_param(s, p, i);
			raw[i] = v < 0 ? int(p.lo) : v;
		}
		m_nfx.set(s.id, type, raw, n);
		// 調べもの用: SMU2000_NATIVE_FX_DEBUG=1 で、読んだ値を出す
		static const bool dbg = std::getenv("SMU2000_NATIVE_FX_DEBUG") != nullptr;
		if (dbg) {
			std::printf("nfx slot %d type %02x %02x kind %d:", int(s.id), type >> 7, type & 0x7f,
			            int(m_nfx.slot(s.id).current()));
			for (int i = 0; i < n; i++)
				std::printf(" %s=%d", def->params[i].label, raw[i]);
			std::putchar(10);
		}

		// 戻り量。XG の 64 を基準にする（送りに対する量で、実機の中身とは別物）
		if (s.ret_lo >= 0) {
			const int ret = xg_read(ram, s.hi, s.mid, s.ret_lo, 1);
			// 戻り量の基準。実機の混ざり具合に合わせた実測の値（SMU2000_NATIVE_RETURN で変えられる）
			static const float base = [] {
				const char *e = std::getenv("SMU2000_NATIVE_RETURN");
				return e ? float(std::atof(e)) : 0.8f;
			}();
			float g = ret < 0 ? base : base * float(ret) / 64.0f;
			// バリエーションを INSERTION でパートに掛けているときは、送りの目盛りが
			// インサーションと同じになる（戻り量は使われない）
			if (s.id == nfx::VARIATION && xg_read(ram, 0x02, 0x01, 0x5a, 1) == 0)
				g = 0.31f;
			m_nfx.set_return(s.id, g);
		}
	}

	// マスター EQ（02 40 00-14）
	{
		int gain[5], freq[5], q[5];
		static const int G[5] = { 0x01, 0x05, 0x09, 0x0d, 0x11 };
		bool ok = true;
		for (int i = 0; i < 5; i++) {
			gain[i] = xg_read(ram, 0x02, 0x40, G[i], 1);
			freq[i] = xg_read(ram, 0x02, 0x40, G[i] + 1, 1);
			q[i]    = xg_read(ram, 0x02, 0x40, G[i] + 2, 1);
			if (gain[i] < 0 || freq[i] < 0 || q[i] < 0)
				ok = false;
		}
		const int shape1 = xg_read(ram, 0x02, 0x40, 0x04, 1);
		const int shape5 = xg_read(ram, 0x02, 0x40, 0x14, 1);
		if (ok)
			m_nfx.meq().set_raw(gain, freq, q, shape1 < 0 ? 0 : shape1, shape5 < 0 ? 0 : shape5);
	}
}

void mu2000::run_sample(s32 &left, s32 &right)
{
	// S-MU2000: 軽量モードでは、XG の設定をときどき読み直す
	if (m_nfx_on && !(++m_nfx_tick & 0x1ff))
		native_fx_update();

	// 台数が変わっていたら別スレッドの使い方を見直す（8192 サンプルごと）
	if (m_want_threaded && !(++m_thread_check & 0x1fff))
		apply_threading();

	m_sample_count++;

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

	// native の口が動いているときは、firmware を回すのは
	//   * 渡した MIDI がまだ溜まっている間（受け取って処理させる）
	//   * そのあと少しの間（処理が終わるまで）
	// だけ。ふだんは止めておく
	bool run_cpu = m_cpu_enabled;
	if (m_native_engine) {
		m_ne_samples.fetch_add(1, std::memory_order_relaxed);
		// firmware が鳴らしている音がある間は止めない。LFO・包絡線・ベンドの
		// 追従をやっているのは firmware なので、止めるとその音だけ変わってしまう。
		// MIDI の溜まり具合は、止まっているときだけ見る（毎サンプル数えると重い）
		if (m_fw_note_total && m_ne_clock < m_fw_note_until)
			m_fw_hold = std::max(m_fw_hold, u32(2));
		// 「溜まっている間は回す」はやめた。渡した MIDI は 1 バイト 14 サンプルかけて
		// 線を流れるので、それを待つだけで実時間の 2 割を SH-2 に持っていかれていた。
		// メッセージごとに置く待ち（下の native_midi）で足りる
		if (m_fw_hold) {
			if (--m_fw_hold == 0) {
				// つまみの位置を RAM から取り直す。こちらが動かした値は
				// 上書きしない（native_driver::sync_cc）。ベンド幅のように
				// こちらが持たない値は、ここで拾う
				m_ndrv.sync_cc();
				m_fw_why = 0;
			}
		} else {
			run_cpu = false;
		}
		if (run_cpu) {
			m_ne_fw_samples.fetch_add(1, std::memory_order_relaxed);
			if (m_fw_note_total)
				m_ne_by_note.fetch_add(1, std::memory_order_relaxed);
			else if (m_fw_why == 1)
				m_ne_by_sysex.fetch_add(1, std::memory_order_relaxed);
			else if (m_fw_why == 3)
				m_ne_by_learn.fetch_add(1, std::memory_order_relaxed);
			else if (m_fw_why == 4)
				m_ne_by_midi.fetch_add(1, std::memory_order_relaxed);
			else
				m_ne_by_other.fetch_add(1, std::memory_order_relaxed);
		}
		if (m_learning && m_learn_left && --m_learn_left == 0)
			native_learn_finish();
		m_ne_clock++;
		if (!m_nq.empty())
			native_pump();
		m_ndrv.tick(m_ne_clock);
		if (m_traj_rec && m_traj_left && --m_traj_left == 0)
			traj_finish();
	}
	if (run_cpu)
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
constexpr u32 STATE_VERSION = 10;  // 2: MIDI の入口が A/B の 2 口になった / 3: SWP30 のピッチ EG / 4: サンプリングの録音の位置 / 5: SmartMedia の命令の途中 / 6: MEG の印と 2 つ目の idx / 7: USB の口（C・D）の受け取り途中 / 8: 2 つ目の A/D 変換器（AN4 = HOST SELECT） / 9: SWP30 の書き込みの待ち / 10: USB のコマンド（M37640 からの知らせ）
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
	// 版 9 から: SWP30 へ書いた待ちの残り（サンプルを跨ぐことがある）
	if (s.version() >= 9)
		s.v(m_swp_wait);

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

	// 版 7 から: USB の口（C・D）の受け取り途中。firmware へ渡す前のバイト列
	if (s.version() >= 7) {
		s.tag("usb");
		u32 n = u32(m_usb.rx.size());
		s.v(n);
		if (s.writing()) {
			for (u8 b : m_usb.rx)
				s.v(b);
		} else {
			m_usb.rx.clear();
			for (u32 i = 0; i < n && s.ok(); i++) {
				u8 b = 0;
				s.v(b);
				m_usb.rx.push_back(b);
			}
		}
		s.v(m_usb.in_port); s.v(m_usb.next); s.v(m_usb.have); s.v(m_usb.cur); s.v(m_usb.tx_next);
		if (s.version() >= 10) {
			u32 c = u32(m_usb.cmd.size());
			s.v(c);
			if (s.writing()) {
				for (u8 b : m_usb.cmd)
					s.v(b);
			} else {
				m_usb.cmd.clear();
				for (u32 i = 0; i < c && s.ok(); i++) {
					u8 b = 0;
					s.v(b);
					m_usb.cmd.push_back(b);
				}
			}
			s.v(m_usb.cur_cmd);
		}
	}
}

u32 mu2000::state_version()
{
	return STATE_VERSION;
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

	// 軽量モード（C++ のエフェクト）の中身は状態に**入れない**。ディレイと残響の
	// 遅延線だけで 5MB 近くあって、DAW の企画ファイルが膨らむわりに、得られるのは
	// 「尾が切れない」だけだから（MEG の側の尾は SWP30 のリバーブ RAM に入っている）。
	// ただし前の曲の尾が残ったままだと、戻した曲に混ざる。ここで消す
	if (m_nfx_on)
		m_nfx.reset();
	return true;
}
