// license:BSD-3-Clause
//
// S-MU2000: SH-2 の命令をその場で x86-64 の機械語にする（JIT）。
//
// プログラム ROM（0x000000-0x3FFFFF）の上の命令を、分岐までのひと続き（ブロック）ごとに訳す。
// 訳した機械語は sh2_device::execute_run() の 1 周と**同じ順に同じこと**をする:
//
//   1. pc を進める（遅延スロットの命令なら m_delay を見て分岐先へ）
//   2. 命令を実行する（よく使う命令は機械語で直に、ほかは解釈実行の execute_one を呼ぶ）
//   3. m_test_irq が立っていて m_delay が 0 なら割り込みを確かめる
//   4. icount を 1 減らし、0 以下なら戻る
//
// メモリはプログラム ROM とワーク RAM だけを直に読み書きし（mem_bus の fast() と同じ範囲）、
// ほかは解釈実行と同じ関数を通す。周辺に触った命令のあとで、周辺が割り込みを起こして pc が
// 変わっていたり m_test_irq が立っていたりすれば、ブロックを抜けて execute_run に戻る。
// 訳すのは m_delay が 0 のときに入る所だけで、それ以外（RAM の上のコード、遅延スロットから
// 始まる所、PC の追跡中）は今までどおり解釈実行する。遅延スロットの命令は、pc が実行時に
// 決まるので、いつも解釈実行の関数を呼ぶ。
// ROM は書き換わらないので、訳した物は捨て直さない（リセットのときだけ捨てる）。
// 訳した物はどこにも保存しない（firmware 由来のものを配らない。実行時に作って捨てる）。
//
// Windows の x86（x86-64 と x86-32 の両方。Phase 3 で 32bit 対応）。ほかの環境と SMU2000_SH2_JIT=0 では使わない。
// SMU2000_SH2_JIT=1 では命令を機械語で書かず、全部 execute_one を呼ぶ（食い違いを探すとき用）。

#include "sh2.h"

// JIT を使うか:
//   第一段階 = x86-64（MinGW の __x86_64__ と MSVC の _M_X64 の両方。これで MSVC x64 も JIT を使う）
//   第二段階 = x86-32 の移植（Phase 3 で完了）。32bit では SMU_JIT32_PORT_SH2 を自分で define して
//              JIT を自動で有効にする（SMU2000_SH2_JIT32 が建つ）。止めるには SMU_JIT32_NO_SH2 を定義。
//              実行時の SMU2000_SH2_JIT=0 は引き続く解釈実行専用の逃げ道（A/B 検証用）
#if defined(_WIN32) && (defined(__i386__) || defined(_M_IX86)) && !defined(SMU_JIT32_NO_SH2)
#define SMU_JIT32_PORT_SH2
#endif
#if defined(_WIN32) && (defined(__x86_64__) || defined(_M_X64))
#define SMU2000_SH2_JIT 1
#elif defined(_WIN32) && (defined(__i386__) || defined(_M_IX86)) && defined(SMU_JIT32_PORT_SH2)
#define SMU2000_SH2_JIT 1
#define SMU2000_SH2_JIT32 1
#else
#define SMU2000_SH2_JIT 0
#endif

#if SMU2000_SH2_JIT
#include <windows.h>
#include "x64asm.h"
#if SMU_X64ASM_MODE == 32
	// x86-32 の割当: ebx=cpu rsi=状態 ebp=ROM edi=RAM（全部 callee 保存＝ヘルパ呼び出しを跨ぐ）
	// 傷物（scratch）は eax/ecx/edx。ecx は mwrite の書き込み値を兼務（x64 の r8 の役）
	#define JIT_ROM RBP
	#define JIT_RAM RDI
	#define JIT_VAL RCX
#else
	#define JIT_ROM R12
	#define JIT_RAM R13
	#define JIT_VAL R8
#endif
#endif

#include <array>
#include <cstddef>
#include <cstdlib>
#include <memory>

struct sh2_device::jit {
	// 入口の関数（x64: rcx = cpu、rdx = 状態、r8 = ROM、r9 = RAM ／ x86-32: cdecl で [esp+4,8,12,16]）。
	// entry のブロックから回し始め、
	// ブロックの終わりで次のブロックへ直に飛ぶ。icount が尽きるか、訳していない所・m_delay・割り込みの印に当たると戻る
	using enter_t = void (*)(sh2_device *, internal_sh2_state *, const u8 *rom, u8 *ram);
	using code_t = void *;

	static constexpr u32 ROM_END   = 0x400000;   // ここより下だけ訳す
	static constexpr int MAX_INSNS = 48;         // 1 ブロックの命令数の上限（遅延スロットは別）
	static constexpr size_t BUF_SIZE = 16 * 1024 * 1024;

	void *buf = nullptr;
	size_t used = 0;
	std::array<std::unique_ptr<code_t[]>, (ROM_END >> 12)> pages;
	enter_t enter = nullptr;      // 置き場の頭に作る
	void *next_block = nullptr;   // ブロックの終わりから飛ぶ先（置き場の頭に作る）
	code_t entry = nullptr;       // enter が最初に飛ぶブロック
	size_t base_used = 0;         // 入口と飛ぶ先の大きさ（捨てても残す）

	~jit()
	{
#if SMU2000_SH2_JIT
		if (buf)
			VirtualFree(buf, 0, MEM_RELEASE);
#endif
	}

	code_t *slot(u32 pc)
	{
		auto &p = pages[pc >> 12];
		if (!p) {
			p.reset(new code_t[0x800]);
			std::fill(p.get(), p.get() + 0x800, nullptr);
		}
		return &p[(pc & 0xfff) >> 1];
	}

	void flush()
	{
		for (auto &p : pages)
			p.reset();
		used = base_used;
	}

	bool init(sh2_device &cpu);
	code_t compile(sh2_device &cpu, u32 pc);
};

void sh2_device::jit_delete(jit *j)
{
	delete j;
}

bool sh2_device::jit_enabled()
{
#if SMU2000_SH2_JIT
	static const bool on = [] {
		const char *e = std::getenv("SMU2000_SH2_JIT");
		return !(e && e[0] == '0');
	}();
	return on;
#else
	return false;
#endif
}

// 命令を機械語で書くか。SMU2000_SH2_JIT=1 なら全部 execute_one を呼ぶ
static bool native_enabled()
{
	static const bool on = [] {
		const char *e = std::getenv("SMU2000_SH2_JIT");
		return !(e && e[0] == '1');
	}();
	return on;
}

void sh2_device::jit_flush()
{
	if (m_jit)
		m_jit->flush();
}

void sh2_device::jit_exec(sh2_device *c, u32 opcode)
{
	c->execute_one(u16(opcode));
}

// 食い違いを探す用。SH2_JIT_TRACE=サイクル,命令数,ファイル で、そのサイクルから 1 命令ごとに PC とレジスタを書く
// （解釈実行の道でも同じ形で書くので、SMU2000_SH2_JIT=0 の追跡と突き合わせられる）
static std::FILE *g_jt_file = nullptr;
static u64 g_jt_from = 0, g_jt_left = 0;
bool sh2_device::jit_trace_on()
{
	static const bool on = [] {
		const char *e = std::getenv("SH2_JIT_TRACE");
		if (!e) return false;
		char path[512] = {};
		unsigned long long from = 0, count = 0;
		if (std::sscanf(e, "%llu,%llu,%511s", &from, &count, path) != 3) return false;
		g_jt_file = std::fopen(path, "w");
		g_jt_from = from; g_jt_left = count;
		return g_jt_file != nullptr;
	}();
	return on;
}
void sh2_device::jit_trace(sh2_device *c)
{
	const u64 cyc = c->total_cycles();
	if (cyc < g_jt_from || !g_jt_left) return;
	g_jt_left--;
	std::fprintf(g_jt_file, "%08X C=%llu%s ic=%d ti=%u dl=%08X pi=%u il=%d\n", c->m_sh2_state->pc, (unsigned long long)cyc, c->regs_text(),
	             c->m_sh2_state->icount, c->m_test_irq, c->m_sh2_state->m_delay, c->m_sh2_state->pending_irq, c->m_sh2_state->internal_irq_level);
	if (!g_jt_left) std::fflush(g_jt_file);
}

void sh2_device::jit_irq(sh2_device *c)
{
	c->check_pending_irq("mame_sh2_execute");
	c->m_test_irq = 0;
}

u32 sh2_device::jit_rb(sh2_device *c, u32 a) { return c->read_byte(a); }
u32 sh2_device::jit_rw(sh2_device *c, u32 a) { return c->read_word(a); }
u32 sh2_device::jit_rl(sh2_device *c, u32 a) { return c->read_long(a); }
void sh2_device::jit_wb(sh2_device *c, u32 a, u32 v) { c->write_byte(a, u8(v)); }
void sh2_device::jit_ww(sh2_device *c, u32 a, u32 v) { c->write_word(a, u16(v)); }
void sh2_device::jit_wl(sh2_device *c, u32 a, u32 v) { c->write_long(a, v); }

bool sh2_device::jit_run()
{
	// 割り込みの印が外（周辺）で立っているときは、解釈実行で 1 命令進めて確かめさせる。
	// ブロックの中では、周辺に触らない命令のあとで印を見ないので
	if (m_sh2_state->m_delay || m_test_irq || smu2000::g_pc_trace || smu2000::g_pc_hash)
		return false;
	const u32 pc = m_sh2_state->pc;
	if (pc >= jit::ROM_END - 0x100 || (pc & 1))
		return false;
	if (!m_jit)
		m_jit.reset(new jit);
	jit::code_t code = *m_jit->slot(pc);
	if (!code) {
		// 訳している途中で置き場が一杯になると全部捨てるので、置き場は訳した後に引き直す
		code = m_jit->compile(*this, pc);
		if (!code)
			return false;
		*m_jit->slot(pc) = code;
	}
	m_jit->entry = code;
	m_jit->enter(this, m_sh2_state, m_program->hot_rom(), m_program->hot_ram());
	return true;
}

namespace {

// 命令の種類。execute_one の振り分けと同じ表から引く
enum class kind { normal, delayed, ends };

kind classify(u16 op)
{
	switch (op >> 12) {
	case 0x0:
		switch (op & 0x3f) {
		case 0x03: case 0x0b: case 0x23: case 0x2b:          // BSRF, RTS, BRAF, RTE
			return kind::delayed;
		case 0x1b:                                            // SLEEP
		case 0x00: case 0x01: case 0x10: case 0x11: case 0x13: case 0x20: case 0x21:
		case 0x30: case 0x31: case 0x32: case 0x33: case 0x38: case 0x39: case 0x3a: case 0x3b:
			return kind::ends;                                // ILLEGAL
		}
		return kind::normal;
	case 0x2:
		return (op & 15) == 3 ? kind::ends : kind::normal;
	case 0x3:
		return ((op & 15) == 1 || (op & 15) == 9) ? kind::ends : kind::normal;
	case 0x4:
		switch (op & 0x3f) {
		case 0x0b: case 0x2b:                                 // JSR, JMP
			return kind::delayed;
		case 0x0c: case 0x0d: case 0x14: case 0x1c: case 0x1d: case 0x2c: case 0x2d:
			return kind::ends;
		}
		return (op & 0x3f) >= 0x30 && (op & 0x3f) != 0x3f ? kind::ends : kind::normal;
	case 0x8:
		switch ((op >> 8) & 15) {
		case 0xd: case 0xf:                                   // BTS, BFS
			return kind::delayed;
		case 0x9: case 0xb:                                   // BT, BF
		case 0x2: case 0x3: case 0x6: case 0x7: case 0xa: case 0xc: case 0xe:
			return kind::ends;
		}
		return kind::normal;
	case 0xa: case 0xb:                                       // BRA, BSR
		return kind::delayed;
	case 0xc:
		return ((op >> 8) & 15) == 3 ? kind::ends : kind::normal;   // TRAPA
	case 0xf:
		return kind::ends;
	}
	return kind::normal;
}

} // namespace

// JIT を切った所（SMU2000_SH2_JIT=0 の環境）は解釈実行専用。
// ここから下が命令生成（x86-64 と x86-32 は SMU_X64ASM_MODE で #if で分ける。出す機械語だけ違い、
// ブロックを組み立てる流れは共通）
#if !SMU2000_SH2_JIT

bool sh2_device::jit::init(sh2_device &)
{
	return false;
}

sh2_device::jit::code_t sh2_device::jit::compile(sh2_device &, u32)
{
	return nullptr;
}

#else

// 置き場の頭に、入口（enter）とブロックの終わりから飛ぶ先（next_block）を作る
bool sh2_device::jit::init(sh2_device &cpu)
{
	using namespace x64asm;
	buf = VirtualAlloc(nullptr, BUF_SIZE, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
	if (!buf)
		return false;
#if SMU_X64ASM_MODE == 32
	if (sizeof(pages[0]) != 4)
#else
	if (sizeof(pages[0]) != 8)
#endif
		return false;
	const internal_sh2_state *st = cpu.m_sh2_state;
	const auto S = [st](const void *f) { return mem{ RSI, NOREG, 1, s32(intptr_t(f) - intptr_t(st)) }; };
	const mem C_test { RBX, NOREG, 1, s32(intptr_t(&cpu.m_test_irq) - intptr_t(&cpu)) };

	assembler a;
	// enter: ebx rsi + ROM/RAM 基を保存して entry へ飛ぶ
#if SMU_X64ASM_MODE == 32
	// x86-32 cdecl: 呼び出し側が積んだ引数 cpu/ROM... は [esp+4,8,12,16]。4 積んだ後は +16 ずれる
	a.push(RBX); a.push(RSI); a.push(RBP); a.push(RDI);
	a.load32(RBX, mem{ RSP, NOREG, 1, 20 });   // cpu
	a.load32(RSI, mem{ RSP, NOREG, 1, 24 });   // 状態
	a.load32(RBP, mem{ RSP, NOREG, 1, 28 });   // ROM
	a.load32(RDI, mem{ RSP, NOREG, 1, 32 });   // RAM
	a.imm32(RAX, u32(uintptr_t(&entry)));
	a.load32(RAX, mem{ RAX, NOREG, 1, 0 });
	a.rr(0, {0xff}, 4, RAX);             // jmp eax
#else
	a.push(RBX); a.push(RSI); a.push(R12); a.push(R13);
	a.subrsp(40);                        // 影 32 + 詰め物 8。rsp は 16 の倍数になる
	a.mov64(RBX, RCX);
	a.mov64(RSI, RDX);
	a.mov64(R12, R8);
	a.mov64(R13, R9);
	a.imm64(RAX, u64(uintptr_t(&entry)));
	a.load64(RAX, mem{ RAX, NOREG, 1, 0 });
	a.rr(0, false, {0xff}, 4, RAX);      // jmp rax
#endif

	// next_block: jit_run が見ることを見て、次のブロックが訳してあればそこへ飛ぶ。無ければ戻る
	const size_t next = a.code.size();
	std::vector<size_t> to_exit;
	a.cmp32i_mem(S(&st->icount), 0);
	to_exit.push_back(a.jcc_fwd(0x8e));                     // jle
	a.load32(RAX, S(&st->m_delay));
	a.test32(RAX, RAX);
	to_exit.push_back(a.jcc_fwd(0x85));
	a.load32(RAX, C_test);
	a.test32(RAX, RAX);
	to_exit.push_back(a.jcc_fwd(0x85));
	a.load32(RAX, S(&st->pc));
	a.cmp32ri(RAX, ROM_END - 0x100);
	to_exit.push_back(a.jcc_fwd(0x83));                     // jae
	a.test32ri(RAX, 1);
	to_exit.push_back(a.jcc_fwd(0x85));
	a.mov32(RCX, RAX);
	a.shr32(RCX, 12);
#if SMU_X64ASM_MODE == 32
	a.imm32(RDX, u32(uintptr_t(pages.data())));
	a.load32(RDX, mem{ RDX, RCX, 4, 0 });
	a.test32(RDX, RDX);
	to_exit.push_back(a.jcc_fwd(0x84));
	a.and32i(RAX, 0xfff);
	a.shr32(RAX, 1);
	a.load32(RAX, mem{ RDX, RAX, 4, 0 });
	a.test32(RAX, RAX);
	to_exit.push_back(a.jcc_fwd(0x84));
	a.rr(0, {0xff}, 4, RAX);             // jmp eax
#else
	a.imm64(RDX, u64(uintptr_t(pages.data())));
	a.load64(RDX, mem{ RDX, RCX, 8, 0 });
	a.test64(RDX, RDX);
	to_exit.push_back(a.jcc_fwd(0x84));
	a.and32i(RAX, 0xfff);
	a.shr32(RAX, 1);
	a.load64(RAX, mem{ RDX, RAX, 8, 0 });
	a.test64(RAX, RAX);
	to_exit.push_back(a.jcc_fwd(0x84));
	a.rr(0, false, {0xff}, 4, RAX);      // jmp rax
#endif
	for (size_t p : to_exit)
		a.patch(p);
#if SMU_X64ASM_MODE == 32
	a.pop(RDI); a.pop(RBP); a.pop(RSI); a.pop(RBX);   // 引き数は呼び出し側が取る（cdecl）
#else
	a.addrsp(40);
	a.pop(R13); a.pop(R12); a.pop(RSI); a.pop(RBX);
#endif
	a.ret();

#if SMU_X64ASM_MODE == 32
	// x86-32 ポートが生きている証明用: SMU2000_SH2_JIT32_LOG=1 で 1 回だけ印を書く
	if (std::getenv("SMU2000_SH2_JIT32_LOG"))
		std::fprintf(stderr, "sh2-jit32: x86-32 prologue emitted (%zu bytes, enter=%p)\n", a.code.size(), (void *)buf);
#endif
	std::memcpy(buf, a.code.data(), a.code.size());
	enter = reinterpret_cast<enter_t>(buf);
	next_block = static_cast<u8 *>(buf) + next;
	base_used = used = a.code.size();
	return true;
}

sh2_device::jit::code_t sh2_device::jit::compile(sh2_device &cpu, u32 pc)
{
	using namespace x64asm;
	if (!buf && !init(cpu))
		return nullptr;

	// 直に読み書きする領域（mem_bus の fast() と同じ）。番地を折り返さない設定のときだけ訳す
	const mem_bus &bus = *cpu.m_program;
	if (cpu.m_am != 0xffffffff || cpu.m_decrypted_program != cpu.m_program || !bus.hot_rom() || !bus.hot_ram())
		return nullptr;
	const u32 rom_end   = bus.hot_rom_end();      // 含む
	const u32 ram_start = bus.hot_ram_start();
	const u32 ram_len   = bus.hot_ram_len();      // 端 - 始め
	if (rom_end < 0xffff || rom_end >= 0x40000000 || ram_len < 0xffff || ram_start <= rom_end)
		return nullptr;

	const internal_sh2_state *st = cpu.m_sh2_state;
	const auto S = [st](const void *f) { return mem{ RSI, NOREG, 1, s32(intptr_t(f) - intptr_t(st)) }; };
	const mem S_pc = S(&st->pc), S_delay = S(&st->m_delay), S_icount = S(&st->icount), S_ea = S(&st->ea);
	const mem S_sr = S(&st->sr), S_pr = S(&st->pr), S_gbr = S(&st->gbr), S_vbr = S(&st->vbr);
	const mem S_mach = S(&st->mach), S_macl = S(&st->macl);
	const auto R = [&](int n) { return S(&st->r[n]); };
	const mem C_test { RBX, NOREG, 1, s32(intptr_t(&cpu.m_test_irq) - intptr_t(&cpu)) };
	if (sizeof(st->pc) != 4 || sizeof(st->icount) != 4 || sizeof(cpu.m_test_irq) != 4 || sizeof(st->r[0]) != 4)
		return nullptr;

	assembler a;
	// ブロックの中では rbx = cpu、rsi = 状態、ROM/RAM 基 = JIT_ROM/JIT_RAM（enter が入れる）

	std::vector<size_t> to_finish, to_ret;

	const auto call = [&](void *fn) { a.call_abs(fn); };
	const auto setT = [&]() {           // al の 0/1 を T へ
		a.movzx8(RAX, RAX);
		a.and32i_mem(S_sr, ~u32(SH_T));
		a.or32mr(S_sr, RAX);
	};
	// 読む。番地は edx、値は eax（sz バイトを並べた値。符号は広げない）
	const auto mread = [&](int sz) {
		std::vector<size_t> to_slow, to_done;
		if (sz > 1) {
			a.test32ri(RDX, 1);
			to_slow.push_back(a.jcc_fwd(0x85));
		}
		a.cmp32ri(RDX, rom_end + 1 - u32(sz));
		const size_t not_rom = a.jcc_fwd(0x87);                // ja
		const mem mr{ JIT_ROM, RDX, 1, 0 };
		if (sz == 1) a.loadu8(RAX, mr);
		else if (sz == 2) { a.loadu16(RAX, mr); a.bswap32(RAX); a.shr32(RAX, 16); }
		else { a.load32(RAX, mr); a.bswap32(RAX); }
		to_done.push_back(a.jmp_fwd());
		a.patch(not_rom);
		a.lea32(RAX, mem{ RDX, NOREG, 1, -s32(ram_start) });
		a.cmp32ri(RAX, ram_len + 1 - u32(sz));
		to_slow.push_back(a.jcc_fwd(0x87));
		const mem mw{ JIT_RAM, RAX, 1, 0 };
		if (sz == 1) a.loadu8(RAX, mw);
		else if (sz == 2) { a.loadu16(RAX, mw); a.bswap32(RAX); a.shr32(RAX, 16); }
		else { a.load32(RAX, mw); a.bswap32(RAX); }
		to_done.push_back(a.jmp_fwd());
		for (size_t p : to_slow) a.patch(p);
#if SMU_X64ASM_MODE == 32
		a.push(RDX); a.push(RBX);        // cdecl: jit_r?(c, a)
		call(sz == 1 ? reinterpret_cast<void *>(&sh2_device::jit_rb) :
		     sz == 2 ? reinterpret_cast<void *>(&sh2_device::jit_rw) : reinterpret_cast<void *>(&sh2_device::jit_rl));
		a.addrsp(8);
#else
		a.mov64(RCX, RBX);
		call(sz == 1 ? reinterpret_cast<void *>(&sh2_device::jit_rb) :
		     sz == 2 ? reinterpret_cast<void *>(&sh2_device::jit_rw) : reinterpret_cast<void *>(&sh2_device::jit_rl));
#endif
		for (size_t p : to_done) a.patch(p);
	};
	// 書く。番地は edx、値は r8d
	// 書く。番地は edx、値は r8d（x86-32 では ecx。速い道で edx を潰したので遅い道は先に積む）
	const auto mwrite = [&](int sz) {
		std::vector<size_t> to_slow;
#if SMU_X64ASM_MODE == 32
		if (sz > 1) {
			a.test32ri(RDX, 1);
			to_slow.push_back(a.jcc_fwd(0x85));
		}
		a.lea32(RAX, mem{ RDX, NOREG, 1, -s32(ram_start) });
		a.cmp32ri(RAX, ram_len + 1 - u32(sz));
		to_slow.push_back(a.jcc_fwd(0x87));
		const mem mw{ JIT_RAM, RAX, 1, 0 };
		if (sz == 1) a.store8(mw, JIT_VAL);
		else if (sz == 2) { a.mov32(RDX, JIT_VAL); a.bswap32(RDX); a.shr32(RDX, 16); a.store16(mw, RDX); }
		else { a.mov32(RDX, JIT_VAL); a.bswap32(RDX); a.store32(mw, RDX); }
		const size_t done = a.jmp_fwd();
		for (size_t p : to_slow) a.patch(p);
		a.push(JIT_VAL); a.push(RDX); a.push(RBX);   // cdecl: jit_w?(c, a, v)
		call(sz == 1 ? reinterpret_cast<void *>(&sh2_device::jit_wb) :
		     sz == 2 ? reinterpret_cast<void *>(&sh2_device::jit_ww) : reinterpret_cast<void *>(&sh2_device::jit_wl));
		a.addrsp(12);
		a.patch(done);
#else
		if (sz > 1) {
			a.test32ri(RDX, 1);
			to_slow.push_back(a.jcc_fwd(0x85));
		}
		a.lea32(RAX, mem{ RDX, NOREG, 1, -s32(ram_start) });
		a.cmp32ri(RAX, ram_len + 1 - u32(sz));
		to_slow.push_back(a.jcc_fwd(0x87));
		const mem mw{ R13, RAX, 1, 0 };
		if (sz == 1) a.store8(mw, R8);
		else if (sz == 2) { a.mov32(RCX, R8); a.bswap32(RCX); a.shr32(RCX, 16); a.store16(mw, RCX); }
		else { a.mov32(RCX, R8); a.bswap32(RCX); a.store32(mw, RCX); }
		const size_t done = a.jmp_fwd();
		for (size_t p : to_slow) a.patch(p);
		a.mov64(RCX, RBX);
		call(sz == 1 ? reinterpret_cast<void *>(&sh2_device::jit_wb) :
		     sz == 2 ? reinterpret_cast<void *>(&sh2_device::jit_ww) : reinterpret_cast<void *>(&sh2_device::jit_wl));
		a.patch(done);
#endif
	};
	const auto sext8 = [](u32 v) { return u32(s32(s8(v))); };

	// 解釈実行と同じことを機械語で書く。書けない命令は none を返す
	enum res { none, pure, memop, delayed, ends };
	const auto native = [&](u16 op, u32 at) -> res {
		const int n = (op >> 8) & 15, m = (op >> 4) & 15;
		const u32 pcv = at + 2;                                  // 実行中の pc
		const auto rd_ea = [&](int base, u32 disp, int sz) {    // ea = r[base] + disp で読む
			a.load32(RDX, R(base));
			if (disp) a.add32ri(RDX, disp);
			a.store32(S_ea, RDX);
			mread(sz);
		};
		const auto wr_ea = [&](int base, u32 disp, int src, int sz) {   // ea = r[base] + disp へ r[src] を書く
			a.load32(RDX, R(base));
			if (disp) a.add32ri(RDX, disp);
			a.store32(S_ea, RDX);
			a.load32(JIT_VAL, R(src));
			mwrite(sz);
		};
		const auto to_r = [&](int d, int sz) {                  // eax を符号拡張して r[d] へ
			if (sz == 1) a.movsx8(RAX, RAX);
			else if (sz == 2) a.movsx16(RAX, RAX);
			a.store32(R(d), RAX);
		};
		const auto cmp_t = [&](u8 cc) { a.load32(RAX, R(n)); a.cmp32rm(RAX, R(m)); a.setcc(cc, RAX); setT(); return pure; };
		const auto copy = [&](const mem &from, const mem &to) { a.load32(RAX, from); a.store32(to, RAX); return pure; };
		const auto branch_to = [&](u32 target, bool delay) {
			if (delay) { a.store32i(S_delay, target); a.store32i(S_ea, target); a.sub32i_mem(S_icount, 1); }
			else { a.store32i(S_pc, target); a.store32i(S_ea, target); a.sub32i_mem(S_icount, 2); }
		};

		switch (op >> 12) {
		case 0x0:
			switch (op & 0x3f) {
			case 0x04: case 0x14: case 0x24: case 0x34:
			case 0x05: case 0x15: case 0x25: case 0x35:
			case 0x06: case 0x16: case 0x26: case 0x36: {       // MOV.x Rm,@(R0,Rn)
				const int sz = 1 << ((op & 15) - 4);
				a.load32(RDX, R(n)); a.add32rm(RDX, R(0)); a.store32(S_ea, RDX);
				a.load32(JIT_VAL, R(m));
				mwrite(sz);
				return memop;
			}
			case 0x0c: case 0x1c: case 0x2c: case 0x3c:
			case 0x0d: case 0x1d: case 0x2d: case 0x3d:
			case 0x0e: case 0x1e: case 0x2e: case 0x3e: {       // MOV.x @(R0,Rm),Rn
				const int sz = 1 << ((op & 15) - 12);
				a.load32(RDX, R(m)); a.add32rm(RDX, R(0)); a.store32(S_ea, RDX);
				mread(sz);
				to_r(n, sz);
				return memop;
			}
			case 0x07: case 0x17: case 0x27: case 0x37:          // MUL.L
				a.load32(RAX, R(n)); a.load32(RCX, R(m)); a.imul32(RAX, RCX); a.store32(S_macl, RAX);
				a.sub32i_mem(S_icount, 1);
				return pure;
			case 0x08: a.and32i_mem(S_sr, ~u32(SH_T)); return pure;                 // CLRT
			case 0x18: a.or32i_mem(S_sr, SH_T); return pure;                        // SETT
			case 0x19: a.and32i_mem(S_sr, ~u32(SH_M | SH_Q | SH_T)); return pure;   // DIV0U
			case 0x09: return pure;                                                  // NOP
			case 0x28: a.store32i(S_mach, 0); a.store32i(S_macl, 0); return pure;   // CLRMAC
			case 0x02: return copy(S_sr, R(n));                                      // STC SR,Rn
			case 0x12: return copy(S_gbr, R(n));
			case 0x22: return copy(S_vbr, R(n));
			case 0x0a: return copy(S_mach, R(n));                                    // STS x,Rn
			case 0x1a: return copy(S_macl, R(n));
			case 0x2a: return copy(S_pr, R(n));
			case 0x29:                                                               // MOVT
				a.load32(RAX, S_sr); a.and32i(RAX, SH_T); a.store32(R(n), RAX);
				return pure;
			case 0x0b:                                                               // RTS
				a.load32(RAX, S_pr); a.store32(S_delay, RAX); a.store32(S_ea, RAX); a.sub32i_mem(S_icount, 1);
				return delayed;
			}
			return none;
		case 0x1:                                                                    // MOV.L Rm,@(disp,Rn)
			wr_ea(n, (op & 15) * 4, m, 4);
			return memop;
		case 0x2:
			switch (op & 15) {
			case 0: case 1: case 2: {                                                // MOV.x Rm,@Rn
				const int sz = 1 << (op & 15);
				wr_ea(n, 0, m, sz);
				return memop;
			}
			case 4: case 5: case 6: {                                                // MOV.x Rm,@-Rn
				const int sz = 1 << ((op & 15) - 4);
				a.load32(JIT_VAL, R(m));
				a.sub32i_mem(R(n), sz);
				a.load32(RDX, R(n));
				mwrite(sz);
				return memop;
			}
			case 8: a.load32(RAX, R(n)); a.test32rm(RAX, R(m)); a.setcc(0x94, RAX); setT(); return pure;   // TST
			case 9: a.load32(RAX, R(m)); a.and32mr(R(n), RAX); return pure;
			case 10: a.load32(RAX, R(m)); a.xor32mr(R(n), RAX); return pure;
			case 11: a.load32(RAX, R(m)); a.or32mr(R(n), RAX); return pure;
			case 14: a.loadu16(RAX, R(n)); a.loadu16(RCX, R(m)); a.imul32(RAX, RCX); a.store32(S_macl, RAX); return pure;       // MULU
			case 15: a.loads16_32(RAX, R(n)); a.loads16_32(RCX, R(m)); a.imul32(RAX, RCX); a.store32(S_macl, RAX); return pure; // MULS
			}
			return none;
		case 0x3:
			switch (op & 15) {
			case 0: return cmp_t(0x94);          // CMP/EQ
			case 2: return cmp_t(0x93);          // CMP/HS
			case 3: return cmp_t(0x9d);          // CMP/GE
			case 6: return cmp_t(0x97);          // CMP/HI
			case 7: return cmp_t(0x9f);          // CMP/GT
			case 8: a.load32(RAX, R(m)); a.sub32mr(R(n), RAX); return pure;
			case 12: a.load32(RAX, R(m)); a.add32mr(R(n), RAX); return pure;
			}
			return none;
		case 0x4:
			switch (op & 0x3f) {
			case 0x00: case 0x20:                                                    // SHLL / SHAL
				a.load32(RAX, R(n)); a.mov32(RCX, RAX); a.shr32(RCX, 31); a.shl32(RAX, 1); a.store32(R(n), RAX);
				a.and32i_mem(S_sr, ~u32(SH_T)); a.or32mr(S_sr, RCX);
				return pure;
			case 0x01: case 0x21:                                                    // SHLR / SHAR
				a.load32(RAX, R(n)); a.mov32(RCX, RAX); a.and32i(RCX, 1);
				if ((op & 0x3f) == 0x01) a.shr32(RAX, 1); else a.sar32(RAX, 1);
				a.store32(R(n), RAX);
				a.and32i_mem(S_sr, ~u32(SH_T)); a.or32mr(S_sr, RCX);
				return pure;
			case 0x08: a.shl32i_mem(R(n), 2); return pure;
			case 0x18: a.shl32i_mem(R(n), 8); return pure;
			case 0x28: a.shl32i_mem(R(n), 16); return pure;
			case 0x09: a.shr32i_mem(R(n), 2); return pure;
			case 0x19: a.shr32i_mem(R(n), 8); return pure;
			case 0x29: a.shr32i_mem(R(n), 16); return pure;
			case 0x10: a.sub32i_mem(R(n), 1); a.setcc(0x94, RAX); setT(); return pure;   // DT
			case 0x11: a.cmp32i_mem(R(n), 0); a.setcc(0x9d, RAX); setT(); return pure;   // CMP/PZ
			case 0x15: a.cmp32i_mem(R(n), 0); a.setcc(0x9f, RAX); setT(); return pure;   // CMP/PL
			case 0x0a: return copy(R(n), S_mach);                                    // LDS Rn,x
			case 0x1a: return copy(R(n), S_macl);
			case 0x2a: return copy(R(n), S_pr);
			case 0x1e: return copy(R(n), S_gbr);                                     // LDC Rn,GBR / VBR
			case 0x2e: return copy(R(n), S_vbr);
			case 0x02: case 0x12: case 0x22: {                                       // STS.L x,@-Rn
				const mem src = (op & 0x3f) == 0x02 ? S_mach : (op & 0x3f) == 0x12 ? S_macl : S_pr;
				a.sub32i_mem(R(n), 4); a.load32(RDX, R(n)); a.store32(S_ea, RDX);
				a.load32(JIT_VAL, src);
				mwrite(4);
				return memop;
			}
			case 0x06: case 0x16: case 0x26: {                                       // LDS.L @Rn+,x
				const mem dst = (op & 0x3f) == 0x06 ? S_mach : (op & 0x3f) == 0x16 ? S_macl : S_pr;
				a.load32(RDX, R(n)); a.store32(S_ea, RDX);
				mread(4);
				a.store32(dst, RAX);
				a.add32i_mem(R(n), 4);
				return memop;
			}
			case 0x0b:                                                               // JSR
				a.store32i(S_pr, pcv + 2);
				[[fallthrough]];
			case 0x2b:                                                               // JMP
				a.load32(RAX, R(n)); a.store32(S_delay, RAX); a.store32(S_ea, RAX); a.sub32i_mem(S_icount, 1);
				return delayed;
			}
			return none;
		case 0x5:                                                                    // MOV.L @(disp,Rm),Rn
			rd_ea(m, (op & 15) * 4, 4);
			to_r(n, 4);
			return memop;
		case 0x6:
			switch (op & 15) {
			case 0: case 1: case 2: {                                                // MOV.x @Rm,Rn
				const int sz = 1 << (op & 15);
				rd_ea(m, 0, sz);
				to_r(n, sz);
				return memop;
			}
			case 3: return copy(R(m), R(n));
			case 4: case 5: case 6: {                                                // MOV.x @Rm+,Rn
				const int sz = 1 << ((op & 15) - 4);
				a.load32(RDX, R(m));
				mread(sz);
				to_r(n, sz);
				if (n != m) a.add32i_mem(R(m), sz);
				return memop;
			}
			case 7: a.load32(RAX, R(m)); a.not32(RAX); a.store32(R(n), RAX); return pure;
			case 11: a.load32(RAX, R(m)); a.neg32(RAX); a.store32(R(n), RAX); return pure;
			case 12: a.loadu8(RAX, R(m)); a.store32(R(n), RAX); return pure;
			case 13: a.loadu16(RAX, R(m)); a.store32(R(n), RAX); return pure;
			case 14: a.loads8_32(RAX, R(m)); a.store32(R(n), RAX); return pure;
			case 15: a.loads16_32(RAX, R(m)); a.store32(R(n), RAX); return pure;
			}
			return none;
		case 0x7:                                                                    // ADD #imm,Rn
			a.add32i_mem(R(n), sext8(op & 0xff));
			return pure;
		case 0x8: {
			const u32 target = pcv + u32(s32(s8(op & 0xff)) * 2) + 2;
			switch (n) {
			case 0: wr_ea(m, op & 15, 0, 1); return memop;                          // MOV.B R0,@(disp,Rm)
			case 1: wr_ea(m, (op & 15) * 2, 0, 2); return memop;
			case 4: rd_ea(m, op & 15, 1); to_r(0, 1); return memop;                 // MOV.B @(disp,Rm),R0
			case 5: rd_ea(m, (op & 15) * 2, 2); to_r(0, 2); return memop;
			case 8: a.cmp32i_mem(R(0), sext8(op & 0xff)); a.setcc(0x94, RAX); setT(); return pure;   // CMP/EQ #imm
			case 9: case 11: case 13: case 15: {                                    // BT / BF / BT/S / BF/S
				a.test32i_mem(S_sr, SH_T);
				const size_t skip = a.jcc_fwd((n == 9 || n == 13) ? 0x84 : 0x85);
				branch_to(target, n >= 13);
				a.patch(skip);
				return n >= 13 ? delayed : ends;
			}
			}
			return none;
		}
		case 0x9: {                                                                  // MOV.W @(disp,PC),Rn
			const u32 ea = pcv + (op & 0xff) * 2 + 2;
			if (ea + 1 > rom_end)
				return none;
			a.store32i(S_ea, ea);
			a.store32i(R(n), u32(s32(s16(cpu.m_program->read_word(ea)))));
			return pure;
		}
		case 0xa:                                                                    // BRA
			branch_to(pcv + u32(util::sext(op & 0xfff, 12) * 2) + 2, true);
			return delayed;
		case 0xb:                                                                    // BSR
			a.store32i(S_pr, pcv + 2);
			branch_to(pcv + u32(util::sext(op & 0xfff, 12) * 2) + 2, true);
			return delayed;
		case 0xc: {
			const u32 d = op & 0xff;
			switch (n) {
			case 0: case 1: case 2: {                                                // MOV.x R0,@(disp,GBR)
				const int sz = 1 << n;
				a.load32(RDX, S_gbr); a.add32ri(RDX, d * sz); a.store32(S_ea, RDX);
				a.load32(JIT_VAL, R(0));
				mwrite(sz);
				return memop;
			}
			case 4: case 5: case 6: {                                                // MOV.x @(disp,GBR),R0
				const int sz = 1 << (n - 4);
				a.load32(RDX, S_gbr); a.add32ri(RDX, d * sz); a.store32(S_ea, RDX);
				mread(sz);
				to_r(0, sz);
				return memop;
			}
			case 7: {                                                                // MOVA
				const u32 ea = ((pcv + 2) & ~3u) + d * 4;
				a.store32i(S_ea, ea); a.store32i(R(0), ea);
				return pure;
			}
			case 8: a.test32i_mem(R(0), d); a.setcc(0x94, RAX); setT(); return pure;   // TST #imm,R0
			case 9: a.and32i_mem(R(0), d); return pure;
			case 10: a.xor32i_mem(R(0), d); return pure;
			case 11: a.or32i_mem(R(0), d); return pure;
			}
			return none;
		}
		case 0xd: {                                                                  // MOV.L @(disp,PC),Rn
			const u32 ea = ((pcv + 2) & ~3u) + (op & 0xff) * 4;
			if (ea + 3 > rom_end)
				return none;
			a.store32i(S_ea, ea);
			a.store32i(R(n), cpu.m_program->read_dword(ea));
			return pure;
		}
		case 0xe:                                                                    // MOV #imm,Rn
			a.store32i(R(n), sext8(op & 0xff));
			return pure;
		}
		return none;
	};

	// pc を書くのを遅らせる。レジスタだけの命令（pure）は pc を見ないので、続いているあいだは書かず、
	// 周辺に触る命令・解釈実行・ブロックを抜ける所の手前でだけ、そのとき正しい pc を書く
	static const bool lazy_pc = [] {
		const char *e = std::getenv("SMU2000_SH2_LAZYPC");
		return !(e && e[0] == '0');
	}();
	bool pc_stale = false;              // メモリの pc が古い
	u32 stale_pc = 0;                   // そのとき正しい pc
	std::vector<std::pair<size_t, u32>> stale_rets;   // pc を書いてから ret へ行く出口
	const auto store_pc_at = [&](size_t pos, u32 v) {
		assembler t;
		t.store32i(S_pc, v);
		a.code.insert(a.code.begin() + std::ptrdiff_t(pos), t.code.begin(), t.code.end());
	};

	bool slot = false;
	for (int i = 0; ; i++) {
		const u32 at = pc + 2 * u32(i);
		const u16 op = cpu.m_decrypted_program->read_word(at);
		const kind k = classify(op);

		if (jit_trace_on()) {
			if (pc_stale) { a.store32i(S_pc, stale_pc); pc_stale = false; }
#if SMU_X64ASM_MODE == 32
			a.push(RBX);                 // cdecl: jit_trace(c)
			call(reinterpret_cast<void *>(&sh2_device::jit_trace));
			a.addrsp(4);
#else
			a.mov64(RCX, RBX);
			call(reinterpret_cast<void *>(&sh2_device::jit_trace));
#endif
		}

		// 1. pc を進める
		if (slot) {
			a.load32(RAX, S_delay);
			a.test32(RAX, RAX);
			const size_t no_delay = a.jcc_fwd(0x84);          // je
			a.store32(S_pc, RAX);
			a.store32i(S_delay, 0);
			const size_t done = a.jmp_fwd();
			a.patch(no_delay);
			a.store32i(S_pc, at + 2);
			a.patch(done);
		} else if (!lazy_pc || jit_trace_on())
			a.store32i(S_pc, at + 2);

		// 2. 実行する
		res r = none;
		const size_t op_begin = a.code.size();
		// 遅延スロットの命令も、pc を使わない普通の命令なら機械語で書く（解釈実行でも pc は見ない）。
		// pc を使うのは MOV.W/MOV.L @(disp,PC) と MOVA だけ（分岐はスロットに来ない）
		static const bool slot_native = [] {
			const char *e = std::getenv("SMU2000_SH2_SLOTNATIVE");
			return !(e && e[0] == '0');
		}();
		const bool pc_rel = (op >> 12) == 0x9 || (op >> 12) == 0xd || (op >> 8) == 0xc7;
		if (native_enabled() && (!slot || (slot_native && k == kind::normal && !pc_rel)))
			r = native(op, at);
		if (r == none) {
			if (!slot && lazy_pc && !jit_trace_on())
				a.store32i(S_pc, at + 2);
#if SMU_X64ASM_MODE == 32
			a.imm32(RDX, op);
			a.push(RDX); a.push(RBX);    // cdecl: jit_exec(c, op)
			call(reinterpret_cast<void *>(&sh2_device::jit_exec));
			a.addrsp(8);
#else
			a.mov64(RCX, RBX);
			a.imm32(RDX, op);
			call(reinterpret_cast<void *>(&sh2_device::jit_exec));
#endif
			r = k == kind::delayed ? delayed : k == kind::ends ? ends : memop;
			pc_stale = false;
		} else if (!slot && lazy_pc && !jit_trace_on()) {
			if (r == pure) {
				pc_stale = true;
				stale_pc = at + 2;
			} else {
				store_pc_at(op_begin, at + 2);                // 命令の頭に差し込む（中の飛び先は相対なのでずれない）
				pc_stale = false;
			}
		}
		if (slot)
			pc_stale = false;

		// 分岐した・止まる命令・遅延スロットの後はブロックを終える
		if (slot || r == ends || (r != delayed && (i + 1 >= MAX_INSNS || at + 4 >= ROM_END)))
			break;

		// 3. 4. 途中の確かめ。周辺に触りうる命令は、割り込みの印か思わぬ pc の変化があれば終わりの処理へ
		if (r == memop) {
			a.load32(RAX, C_test);
			a.test32(RAX, RAX);
			to_finish.push_back(a.jcc_fwd(0x85));             // jne
			a.cmp32i_mem(S_pc, at + 2);
			to_finish.push_back(a.jcc_fwd(0x85));             // jne
		}
		a.sub32i_mem(S_icount, 1);
		if (pc_stale)
			stale_rets.emplace_back(a.jcc_fwd(0x8e), stale_pc);   // jle（pc を書いてから ret へ）
		else
			to_ret.push_back(a.jcc_fwd(0x8e));                // jle

		slot = r == delayed;
		if (slot && at + 4 >= ROM_END)
			return nullptr;
	}

	// 最後の命令のあとで pc がまだ古ければ書く（終わりの処理と next_block が pc を見る）
	if (pc_stale)
		a.store32i(S_pc, stale_pc);

	// 終わりの処理: 3. と 4.
	const size_t finish = a.code.size();
	for (size_t p : to_finish)
		a.patch_to(p, finish);
	a.load32(RAX, C_test);
	a.test32(RAX, RAX);
	const size_t no_irq1 = a.jcc_fwd(0x84);
	a.load32(RAX, S_delay);
	a.test32(RAX, RAX);
	const size_t no_irq2 = a.jcc_fwd(0x85);
#if SMU_X64ASM_MODE == 32
	a.push(RBX);                     // cdecl: jit_irq(c)
	call(reinterpret_cast<void *>(&sh2_device::jit_irq));
	a.addrsp(4);
#else
	a.mov64(RCX, RBX);
	call(reinterpret_cast<void *>(&sh2_device::jit_irq));
#endif
	a.patch(no_irq1);
	a.patch(no_irq2);
	a.sub32i_mem(S_icount, 1);

	const size_t ret = a.code.size();
	for (size_t p : to_ret)
		a.patch_to(p, ret);
#if SMU_X64ASM_MODE == 32
	a.imm32(RAX, u32(uintptr_t(next_block)));
	a.rr(0, {0xff}, 4, RAX);             // jmp eax
#else
	a.imm64(RAX, u64(uintptr_t(next_block)));
	a.rr(0, false, {0xff}, 4, RAX);      // jmp rax
#endif
	// pc が古いまま抜ける所: 書いてから ret へ
	for (const auto &[pos, v] : stale_rets) {
		a.patch_to(pos, a.code.size());
		a.store32i(S_pc, v);
		const size_t j = a.jmp_fwd();
		a.patch_to(j, ret);
	}

	if (used + a.code.size() > BUF_SIZE) {
		flush();
		// 捨てたので、呼び出し元の置き場も消えている。次の呼び出しで訳し直す
		return nullptr;
	}
	u8 *dst = static_cast<u8 *>(buf) + used;
	std::memcpy(dst, a.code.data(), a.code.size());
	used += a.code.size();
	return dst;
}

#endif
