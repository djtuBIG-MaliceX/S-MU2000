// license:BSD-3-Clause
//
// S-MU2000: MEG のプログラムをその場で x86-64 の機械語にする（JIT）。
//
// firmware が MEG に書いたプログラムを、プログラムか番地の割り当てが変わるたびに訳し直す。
// 訳した機械語は meg_state::run_program() と**ビット単位で同じこと**をする:
//
//   * 命令ごとの判定（ALU の種類・読み先・書き先・メモリ操作）は訳すときに決める
//   * 3 命令遅れの書き込み・2 命令遅れのメモリポートと t の値は、遅れの輪のどの枠かが
//     訳すときに決まるので、反映する命令の位置に直に置く。サンプルを跨ぐ分
//     （頭の 3 命令が読む枠と、終わりの 3 命令が書く枠）だけ輪そのものを使う
//   * 定数・番地表・LFO は firmware が動かしている最中にも書くので、実行時に読む
//   * 乱数（ディザ）は同じ順に同じ回数だけ引く
//
// 分岐（bit 0x3f）を含むプログラム（LO-FI と DYNA 系）は、遅れの輪を毎命令で読み書きする形で訳す。
// 訳した物はどこにも保存しない（firmware 由来のものを配らない。実行時に作って捨てる）。
//
// Windows の x86-64 だけ。ほかでは build() が false を返し、今までどおり解釈実行する。

#include "swp30.h"

// JIT を使うか:
//   第一段階 = x86-64（MinGW の __x86_64__ と MSVC の _M_X64 の両方。これで MSVC x64 も JIT を使う）
//   第二段階 = x86-32 の移植（Phase 5）。SMU_JIT32_PORT_MEG をビルドで定義した時だけ JIT を有効にし、
//              SMU2000_MEG_JIT32 が建つ。マクロを切った Win32 は今までどおり解釈実行専用（動きは不変）
#if defined(_WIN32) && (defined(__i386__) || defined(_M_IX86)) && !defined(SMU_JIT32_NO_MEG)
	#define SMU_JIT32_PORT_MEG        // win32 は既定で JIT 有効（最適化しない）。解除は SMU_JIT32_NO_MEG
#endif
#if defined(_WIN32) && (defined(__x86_64__) || defined(_M_X64))
#define SMU2000_MEG_JIT 1
#elif defined(_WIN32) && (defined(__i386__) || defined(_M_IX86)) && defined(SMU_JIT32_PORT_MEG)
#define SMU2000_MEG_JIT 1
#define SMU2000_MEG_JIT32 1
#else
#define SMU2000_MEG_JIT 0
#endif

#include <cstdlib>
#include <cstring>
#include <vector>

#if SMU2000_MEG_JIT
#include <windows.h>
#include "x64asm.h"
#endif

namespace {

// 機械語部品（x86-64 と x86-32 の両方）。64bit 命令はこの 3 つには無い（全て 32bit 命令）
// encode/decode が余りに使う scratch: x64 は R11 / R8、32bit は ESI（どちらの呼び所も ESI は空き）
#if SMU2000_MEG_JIT

using namespace x64asm;

#if SMU_X64ASM_MODE == 32
	#define RENC ESI   // encode の余り scratch（x64 では R11）
	#define RDEC ESI   // decode の余り scratch（x64 では R8）
#else
	#define RENC R11
	#define RDEC R8
#endif

// meg_state::revram_encode と同じことをする。入力 eax（u32）、出力 eax（u16）。rcx rdx r11 を壊す。
// 分岐を使わない。符号は音の値しだいで読めないので、分岐にすると予測の外れで遅くなる
void emit_revram_encode(assembler &a)
{
	a.and32i(RAX, 0x7ffffff);
	a.mov32(RDX, RAX);
	a.shl32(RDX, 5);
	a.sar32(RDX, 31);                                    // bit 26 が立っていれば -1
	a.mov32(RCX, RDX);
	a.and32i(RCX, 0x7ffffff);
	a.xor32(RAX, RCX);
	a.and32i(RDX, 1);                                    // s
	// e は bit 11〜25 のうち一番上の 1 の位置 - 10。無ければ e = 0 で m = v（v < 0x800）
	a.mov32(RCX, RAX);
	a.or32ri(RCX, 0x400);
	a.bsr32(RCX, RCX);
	a.sub32ri(RCX, 10);                                  // e
#if SMU_X64ASM_MODE == 32
	// 32bit では setcc の先が ESI（modrm rm=6）だと REX が無く AH を壊す（SIL は REX 必須、EAX の上 8bit を壊すと仮数が出る）。
	// 分岐を使わず算術で e!=0 を作る（x64 側は従来通り）
	a.mov32(RENC, RCX);                                  // e
	a.sub32ri(RENC, 1);
	a.shr32(RENC, 31);                                   // e == 0 なら 1
	a.xor32ri(RENC, 1);                                  // e != 0
	a.sub32(RCX, RENC);                                  // e ? e - 1 : 0
	a.add32(RENC, RCX);                                  // e
#else
	a.xor32(RENC, RENC);
	a.test32(RCX, RCX);
	a.setcc(0x95, RENC);                                 // e != 0
	a.sub32(RCX, RENC);                                  // e ? e - 1 : 0
	a.add32(RENC, RCX);                                  // e
#endif
	a.shr32cl(RAX);
	a.and32i(RAX, 0x7ff);
	a.shl32(RENC, 12);
	a.or32(RAX, RENC);
	a.shl32(RDX, 11);
	a.or32(RAX, RDX);
}

// meg_state::m1_expand と同じことをする。入力 eax（下の 16bit が s16）、出力 rax（0〜0x7ffc）。rcx を壊す
void emit_m1_expand(assembler &a)
{
	a.test32ri(RAX, 0x8000);
	const size_t neg = a.jcc_fwd(0x85);
	a.mov32(RCX, RAX);
	a.shr32(RCX, 12);
	a.and32i(RCX, 7);                                    // s
	a.and32i(RAX, 0xfff);
	a.or32ri(RAX, 0x1000);
	a.cmp32ri(RCX, 5);
	const size_t done1 = a.jcc_fwd(0x84);                // s == 5
	const size_t less = a.jcc_fwd(0x82);                 // s < 5（jb）
	a.sub32ri(RCX, 5);
	a.shl32cl(RAX);
	const size_t done2 = a.jmp_fwd();
	a.patch(less);
	a.neg32(RCX);
	a.add32ri(RCX, 5);
	a.shr32cl(RAX);
	const size_t done3 = a.jmp_fwd();
	a.patch(neg);
	a.xor32(RAX, RAX);
	a.patch(done1);
	a.patch(done2);
	a.patch(done3);
}

// meg_state::revram_decode と同じことをする。入力 eax（u16）、出力 eax。rcx rdx と余り scratch を壊す。分岐を使わない
void emit_revram_decode(assembler &a)
{
	a.mov32(RDEC, RAX);                                  // v
	a.mov32(RCX, RAX);
	a.shr32(RCX, 12);                                    // e
	a.and32i(RAX, 0x7ff);                                // m
	a.xor32(RDX, RDX);
	a.test32(RCX, RCX);
	a.setcc(0x95, RDX);                                  // e != 0
	a.sub32(RCX, RDX);                                   // e ? e - 1 : 0
	a.shl32(RDX, 11);
	a.or32(RAX, RDX);                                    // e ? m | 0x800 : m
	a.shl32cl(RAX);
	a.imm32(RDX, 0xffffffff);
	a.shl32cl(RDX);                                      // 反転の範囲
	a.mov32(RCX, RDEC);
	a.shl32(RCX, 20);
	a.sar32(RCX, 31);                                    // s ? -1 : 0
	a.and32(RDX, RCX);
	a.xor32(RAX, RDX);
}

#endif

} // namespace


struct swp30_device::meg_jit {
	using fn_t = void (*)(meg_state *, swp30_device *, u16 *);

	// 訳した機械語 1 本
	struct code {
		fn_t fn = nullptr;
		void *buf = nullptr;
		size_t buf_size = 0;
		u32 d3 = 0, d2 = 0;
		~code()
		{
#if SMU2000_MEG_JIT
			if (buf)
				VirtualFree(buf, 0, MEM_RELEASE);
#endif
		}
	};

	// gen: 定数を実行時に読む版（いつでも使える）。spec: 今の定数を焼き込んだ版。
	// 定数は firmware がエフェクトを組むときにまとめて書き、そのあとはほとんど変わらない。
	// 変わってから STABLE サンプルのあいだ動かなければ spec を作り、次に変わった瞬間に gen へ戻す
	code gen, spec;
	const meg_state::op *ops = nullptr;
	u32 spec_const_gen = 0;
	u32 seen_const_gen = 0;
	u32 stable = 0;
	bool spec_tried = false;
	static constexpr u32 STABLE = 8192;

#if SMU2000_MEG_JIT
	static u32 call_lfo(meg_state *ms, u32 lfo) { return ms->get_lfo(int(lfo)); }
#endif

	bool build(code &c, meg_state &ms, const meg_state::op *ops, swp30_device &swp, bool bake);
};

void swp30_device::meg_jit_delete(meg_jit *j)
{
	delete j;
}

bool swp30_device::meg_jit_enabled()
{
#if SMU2000_MEG_JIT
	static const bool on = [] {
		const char *e = std::getenv("SMU2000_MEG_JIT");
		return !(e && e[0] == '0');
	}();
	return on;
#else
	return false;
#endif
}

void swp30_device::meg_jit_rebuild()
{
	if (!meg_jit_enabled()) {
		m_jit.reset();
		return;
	}
	if (!m_jit)
		m_jit.reset(new meg_jit);
	meg_jit &j = *m_jit;
	j.ops = m_meg_ops.data();
	if (!j.build(j.gen, *m_meg, j.ops, *this, false))
		j.gen.fn = nullptr;
	// プログラムか番地が変わったので、焼き込んだ版は作り直す
	j.spec.fn = nullptr;
	j.spec_tried = false;
	j.seen_const_gen = m_meg_const_gen;
	j.stable = 0;
}

// 訳した物を使えなくする（次に meg_jit_rebuild() を呼ぶまで解釈実行で回る）
void swp30_device::meg_jit_invalidate()
{
	if (m_jit) {
		m_jit->gen.fn = nullptr;
		m_jit->spec.fn = nullptr;
	}
}

bool swp30_device::meg_jit_run()
{
	meg_jit *j = m_jit.get();
	if (!j || !j->gen.fn)
		return false;

	static const bool bake_on = [] {
		const char *e = std::getenv("SMU2000_MEG_BAKE");
		return !(e && e[0] == '0');
	}();

	meg_jit::code *c = &j->gen;
	if (bake_on) {
		const u32 cg = m_meg_const_gen;
		if (cg != j->seen_const_gen) {
			j->seen_const_gen = cg;
			j->stable = 0;
			j->spec_tried = false;
		} else if (j->stable < meg_jit::STABLE)
			j->stable++;
		if (j->spec.fn && j->spec_const_gen == cg)
			c = &j->spec;
		else if (j->stable >= meg_jit::STABLE && !j->spec_tried) {
			j->spec_tried = true;
			if (j->build(j->spec, *m_meg, j->ops, *this, true)) {
				j->spec_const_gen = cg;
				c = &j->spec;
			} else
				j->spec.fn = nullptr;
		}
	}

	if (c->d3 != m_meg->m_delay_3 || c->d2 != m_meg->m_delay_2)
		return false;
	c->fn(m_meg, this, m_reverb_ram.data());
	m_meg->m_pc = 0;
	m_meg->m_icount -= 0x180;
	return true;
}

// 機械語にしたリバーブ RAM の詰め方・戻し方を、meg_state の関数と全部の入力で突き合わせる。
// 食い違った入力の数を返す（JIT が無い環境では 0）。make test の verify から呼ぶ
u64 swp30_device::meg_jit_selftest()
{
	// x86-32: cdecl で呼び、入口で eax に引数を移す。m1_expand も入出力とも 32bit に収まるので 3 つとも同じ形
#if SMU2000_MEG_JIT
	u64 bad = 0;
	for (int which = 0; which < 3; which++) {
		assembler a;
#if SMU_X64ASM_MODE == 32
		a.push(ESI);                                     // RENC/RDEC は ESI。cdecl では callee-saved なのでセルフテストのスタブでは守る
		a.load32(RAX, mem{ RSP, NOREG, 1, 8 });          // cdecl: 引数（push ぶんずれた [esp+8]）
#else
		a.mov32(RAX, RCX);
#endif
		if (which == 0) emit_revram_encode(a); else if (which == 1) emit_revram_decode(a); else emit_m1_expand(a);
#if SMU_X64ASM_MODE == 32
		a.pop(ESI);
#endif
		a.ret();
		void *buf = VirtualAlloc(nullptr, a.code.size(), MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
		if (!buf)
			return ~u64(0);
		std::memcpy(buf, a.code.data(), a.code.size());
		const auto fn = reinterpret_cast<u32 (*)(u32)>(buf);
		if (which == 0) {
			for (u32 v = 0; v < 0x8000000; v++)        // encode は下の 27bit しか見ない
				if ((fn(v) & 0xffff) != meg_state::revram_encode(v))
					bad++;
			for (u32 v : { 0xffffffffu, 0x80000000u, 0xf8000001u })
				if ((fn(v) & 0xffff) != meg_state::revram_encode(v))
					bad++;
		} else if (which == 1) {
			for (u32 v = 0; v < 0x10000; v++)
				if (fn(v) != meg_state::revram_decode(u16(v)))
					bad++;
		} else {
#if SMU_X64ASM_MODE == 32
			// m1_expand の入出力は 32bit に収まる（出力は 0〜0x7ffc）。MSVC x86 の cdecl: 引数 esp+4、戻り eax
			for (s32 v = -0x8000; v < 0x8000; v++)
				if (s32(fn(u32(v))) != s32(meg_state::m1_expand(s16(v))))
					bad++;
#else
			// 呼ぶ側は loads16 で 64bit に符号拡張した値を渡す。出力は 64bit のまま使う
			const auto fn64 = reinterpret_cast<s64 (*)(s64)>(buf);
			for (s32 v = -0x8000; v < 0x8000; v++)
				if (fn64(v) != s64(meg_state::m1_expand(s16(v))))
					bad++;
#endif
		}
		VirtualFree(buf, 0, MEM_RELEASE);
	}
	return bad;
#else
	return 0;
#endif
}

// build(): JIT を使わなければ空（解釈実行）。使うなら x86-64 / x86-32 共用の 1 本
// （命令の出し分けは中の #if SMU_X64ASM_MODE == 32 で行う。x86-64 の出す機械語は従来と 1 バイトも同じ）
#if !SMU2000_MEG_JIT

bool swp30_device::meg_jit::build(code &, meg_state &, const meg_state::op *, swp30_device &, bool)
{
	return false;
}

#else

bool swp30_device::meg_jit::build(code &cd, meg_state &ms, const meg_state::op *ops, swp30_device &swp, bool bake)
{
	fn_t &fn = cd.fn;
	void *&buf = cd.buf;
	size_t &buf_size = cd.buf_size;
	u32 &d3 = cd.d3, &d2 = cd.d2;
	fn = nullptr;
	// 分岐（前へ飛ばすだけ）のあるプログラムは、遅れの輪を解釈実行と同じく毎命令で読み書きする形で訳す。
	// 飛ばされた命令は、その命令が輪に入れるはずだった書き込みを消し、t の値だけを入れる（run_program と同じ）
	bool branchy = false;
	for (u32 pc = 0; pc != 0x180; pc++)
		if (ops[pc].jump)
			branchy = true;
	if (swp.m_reverb_ram.size() < 0x40000)
		return false;

	d3 = ms.m_delay_3;
	d2 = ms.m_delay_2;
	const auto slot3 = [&](u32 k) { return (d3 + k) % 3; };
	const auto slot2 = [&](u32 k) { return (d2 + k) % 2; };

	// 要素の位置（meg_state と swp30_device の中）
	const auto off = [](const void *base, const void *field) { return s32(intptr_t(field) - intptr_t(base)); };
	const s32 o_m        = off(&ms, ms.m_m.data());
	const s32 o_r        = off(&ms, ms.m_r.data());
	const s32 o_t        = off(&ms, ms.m_t.data());
	const s32 o_p        = off(&ms, &ms.m_p);
	const s32 o_const    = off(&ms, ms.m_const.data());
	const s32 o_offset   = off(&ms, ms.m_offset.data());
	const s32 o_mw_value = off(&ms, ms.m_mw_value.data());
	const s32 o_mw_reg   = off(&ms, ms.m_mw_reg.data());
	const s32 o_rw_value = off(&ms, ms.m_rw_value.data());
	const s32 o_rw_reg   = off(&ms, ms.m_rw_reg.data());
	const s32 o_ix_value = off(&ms, ms.m_index_value.data());
	const s32 o_ix_act   = off(&ms, ms.m_index_active.data());
	const s32 o_memw_val = off(&ms, ms.m_memw_value.data());
	const s32 o_memr_val = off(&ms, ms.m_memr_value.data());
	const s32 o_t_value  = off(&ms, ms.m_t_value.data());
	const s32 o_memw_act = off(&ms, ms.m_memw_active.data());
	const s32 o_memr_act = off(&ms, ms.m_memr_active.data());
	const s32 o_ram_read = off(&ms, &ms.m_ram_read);
	const s32 o_ram_write = off(&ms, &ms.m_ram_write);
	const s32 o_ram_index = off(&ms, &ms.m_ram_index);
	const s32 o_sample   = off(&ms, &ms.m_sample_counter);
	const s32 o_lfo      = off(&ms, ms.m_lfo.data());
	const s32 o_lfo_counter = off(&ms, ms.m_lfo_counter.data());
	// get_lfo を機械語にするのは、sin 表が 1/4 周期ぶん（0x8000 個）そろっているときだけ
	const u16 *sintab = swp.m_sintab.count() >= 0x8000 ? swp.m_sintab.target() : nullptr;
	const s32 o_seed     = off(&swp, &swp.m_rand_seed);
	const s32 o_flag_n   = off(&swp, &swp.m_meg_flag_n);
	const s32 o_flag_z   = off(&swp, &swp.m_meg_flag_z);
	const s32 o_ix2_value = off(&swp, swp.m_meg_ix2_value.data());   // 2 つ目の idx（doc/upstream.md の 32）
	const s32 o_ix2_act   = off(&swp, swp.m_meg_ix2_act.data());
	const s32 o_ram_index2 = off(&swp, &swp.m_meg_ram_index2);
	const s32 o_skip     = off(&swp, &swp.m_meg_jit_skip);

	if (sizeof(ms.m_mw_reg[0]) != 1 || sizeof(ms.m_index_active[0]) != 1 || sizeof(ms.m_memw_active[0]) != 1 ||
	    sizeof(swp.m_meg_flag_n) != 1 || sizeof(ms.m_t_value[0]) != 2 || sizeof(ms.m_const[0]) != 2 ||
	    sizeof(ms.m_offset[0]) != 2 || sizeof(ms.m_m[0]) != 4 || sizeof(ms.m_r[0]) != 4)
		return false;

	// t の値（2 命令遅れ）を書いておく必要がある命令: 2 つ後に t を p から書く命令があるか、終わりの 2 つ
	bool need_tval[0x180] = {};
	for (u32 k = 0; k != 0x180; k++) {
		if (k >= 0x17e)
			need_tval[k] = true;
		if (k + 2 < 0x180 && ops[k + 2].t_write && ops[k + 2].t_from_p)
			need_tval[k] = true;
	}

	// 3 命令遅れの書き込みを、書いた命令の場所で直に入れてよいレジスタを調べる。
	// 読む命令の 1 つ前か 2 つ前（サンプルを跨いでも）に同じレジスタへの書き込みが無ければ、
	// どの時点で読んでも見える値は変わらない。終わりの 3 命令（0x17d-0x17f）が書くレジスタは、
	// 次のサンプルの頭で輪から入るので対象にしない
	bool early_r[128] = {}, early_m[128] = {};
	{
		bool bad_r[128] = {}, bad_m[128] = {};
		const auto reads = [&](const meg_state::op &o, bool m, u32 x) {
			if (!x)
				return false;
			bool rd = false;
			if (o.alu && (o.mmode == 2 || o.mmode == 3) && (o.m2_from_m != 0) == m && (m ? o.sm : o.sr) == x)
				rd = true;
			if (o.alu && !m && o.asel == 1 && o.sr == x)
				rd = true;
			if (o.alu && m && o.asel == 2 && o.sm == x)
				rd = true;
			if (!m && o.dr && o.dr_from_r && o.sr == x)
				rd = true;
			if (m && o.dm && o.dm_src == 7 && o.sm == x)
				rd = true;
			return rd;
		};
		for (u32 j = 0; j != 0x180; j++)
			for (u32 back = 1; back <= 2; back++) {
				const meg_state::op &w = ops[(j + 0x180 - back) % 0x180];
				if (w.dr && reads(ops[j], false, w.dr))
					bad_r[w.dr] = true;
				if (w.dm && reads(ops[j], true, w.dm))
					bad_m[w.dm] = true;
			}
		for (u32 k = 0x17d; k != 0x180; k++) {
			if (ops[k].dr) bad_r[ops[k].dr] = true;
			if (ops[k].dm) bad_m[ops[k].dm] = true;
		}
		static const bool early_on = [] {
			const char *e = std::getenv("SMU2000_MEG_EARLY");
			return !(e && e[0] == '0');
		}();
		for (u32 x = 1; x != 128; x++) {
			early_r[x] = early_on && !branchy && !bad_r[x];
			early_m[x] = early_on && !branchy && !bad_m[x];
		}
	}

	// 前倒しで書いた書き込みでも、輪の枠の最後の値になるもの（同じ枠へ後で書く命令が無いもの）は枠にも書く。
	// 読まれはしないが、状態の保存の中身を解釈実行と同じにしておく
	bool last_slot_r[0x180] = {}, last_slot_m[0x180] = {};
	for (u32 c = 0; c != 3; c++) {
		for (int k = 0x17f; k >= 0; k--)
			if (u32(k) % 3 == c && ops[k].dr) { last_slot_r[k] = true; break; }
		for (int k = 0x17f; k >= 0; k--)
			if (u32(k) % 3 == c && ops[k].dm) { last_slot_m[k] = true; break; }
	}

	assembler a;
#if SMU_X64ASM_MODE == 32
	// 32bit: ebx=ms, edi=swp, ebp=ram（callee-saved の 3 つを pins）。eax ecx edx esi は scratch。
	// p(s64)・seed・sample・一時的な acc 退避は esp 基準の置き場。限界値は全部即値（load_p_limits 不要）
	const u8 MS = EBX, SWP = EDI, RAM = EBP;
	const s32 F_PLO = 0, F_PHI = 4, F_SEED = 8, F_SC = 12, F_ALO = 16, F_AHI = 20;
	const auto M = [&](s32 disp) { return mem{MS, NOREG, 1, disp}; };
	const auto FM = [&](s32 disp) { return mem{RSP, NOREG, 1, disp}; };
	const auto load_p_limits = [&]() {};

	// 入口（cdecl）。push4(16) + subrsp(24) = 40 → ms/swp/ram は esp+44/48/52（retaddr を含む引数は押し出し分ずれる）
	a.push(RBX); a.push(RSI); a.push(RDI); a.push(RBP);
	a.subrsp(24);                                    // 置き場 6 個ぶん
	a.load32(MS, mem{RSP, NOREG, 1, 44});
	a.load32(SWP, mem{RSP, NOREG, 1, 48});
	a.load32(RAM, mem{RSP, NOREG, 1, 52});
	a.load32(RAX, M(o_p)); a.store32(FM(F_PLO), RAX);
	a.load32(RAX, M(o_p + 4)); a.store32(FM(F_PHI), RAX);
	a.load32(RAX, M(o_sample)); a.store32(FM(F_SC), RAX);
	a.load32(RAX, mem{SWP, NOREG, 1, o_seed}); a.store32(FM(F_SEED), RAX);
	if (branchy)
		a.store32i(mem{SWP, NOREG, 1, o_skip}, 0);

	// p を 24bit に詰める（meg_pack24）。入出力 acc=(eax,edx)→eax
	const auto pack24 = [&]() {
		a.mov32(RCX, RAX); a.mov32(RSI, RDX);         // (ecx,esi) = p
		a.sar64(RCX, RSI, 63);                        // 負なら -1
		a.and32i(RCX, 0x7fff); a.xor32(RSI, RSI);     // 負なら 0x7fff（hi は 0）
		a.add64(RAX, RDX, RCX, RSI);                  // p += (負?0x7fff:0)
		a.sar64(RAX, RDX, 15);
		// 1 つだけはみ出したときは限界に止める（cf/ zf を壊さぬよう源レジスタは cmp 前に用意）
		a.imm32(RCX, 0x7fffff); a.xor32(RSI, RSI);      // K_MAX ペア
		a.cmp64i(RAX, RDX, 0x800000);                   // ==0x800000?
		a.cmovcc64(0x44, RAX, RDX, RCX, RSI);
		a.imm32(RCX, u32(s32(-0x800000))); a.imm32(RSI, 0xffffffffu);   // K_MIN ペア
		a.cmp64i(RAX, RDX, u32(s32(-0x800001)));        // ==-0x800001?
		a.cmovcc64(0x44, RAX, RDX, RCX, RSI);
		a.shl32(RAX, 8); a.sar32(RAX, 8);             // 24bit の符号拡張
	};
	// 乱数を 1 つ引く（swp30_device::rand）。出力 eax
	const auto rnd = [&]() {
		a.load32(RAX, FM(F_SEED));
		a.imul32i(RAX, RAX, 1664525);
		a.add32i(RAX, 1013904223);
		a.store32(FM(F_SEED), RAX);
		a.rol32(RAX, 16);
	};
	// p に雑音を足して詰める（dm の 6 番、dr の p）。出力 eax（acc=(eax,edx)）
	const auto p_packed = [&](bool noise) {
		if (noise) {
			rnd();
			a.and32i(RAX, 0x07e0);                    // 雑音（正）
			a.xor32(RSI, RSI);                        // 0（cf を壊さぬよう先に）
			a.mov32(RCX, RAX);                        // 雑音 lo
			a.load32(RAX, FM(F_PLO));                 // p lo
			a.add32(RAX, RCX);                        // lo 和。cf = 上がり
			a.load32(RDX, FM(F_PHI));                 // p hi（mov なので cf 保持）
			a.adc32(RDX, RSI);                        // cf を hi へ
		} else {
			a.load32(RAX, FM(F_PLO)); a.load32(RDX, FM(F_PHI));
		}
		pack24();
	};
	// p を acc=(eax,edx) へ／acc を n 右（算術）。結果の下 32bit は eax（この先は 32bit に収まる値）
	const auto AccFromP = [&]() { a.load32(RAX, FM(F_PLO)); a.load32(RDX, FM(F_PHI)); };
	const auto ShrAcc = [&](u8 n) { a.sar64(RAX, RDX, n); };
#else
	const u8 MS = RBX, SWP = R12, P = R13, SC = R14, RAM = R15, SEED = RSI, K_MAX = RDI, K_MIN = RBP;
	const u8 P_MAX = R9, P_MIN = R10;                    // p の飽和の限界。r9 r10 は呼ぶ先で壊れるので、呼んだあと積み直す
	const auto load_p_limits = [&]() {
		a.imm64(P_MAX, 0x3fffffffff);
		a.imm64(P_MIN, u64(s64(-0x4000000000)));
	};   // SEED: 乱数の種を回しているあいだ持つ
	const auto M = [&](s32 disp) { return mem{MS, NOREG, 1, disp}; };

	// 入口（Windows x64: rcx = ms, rdx = swp, r8 = リバーブ RAM）
	a.push(RBX); a.push(R12); a.push(R13); a.push(R14); a.push(R15); a.push(RSI); a.push(RDI); a.push(RBP);
	a.subrsp(56);                                    // 呼ぶ先の影 32 + 自分の置き場 16 + 揃え 8。rsp は 16 の倍数
	a.mov64(MS, RCX);
	a.mov64(SWP, RDX);
	a.mov64(RAM, R8);
	a.load64(P, M(o_p));
	a.load32(SC, M(o_sample));
	a.load32(SEED, mem{SWP, NOREG, 1, o_seed});
	a.imm64(K_MAX, 0x7fffff);                            // pack24 の限界（即値を毎回積まないため）
	a.imm64(K_MIN, u64(s64(-0x800000)));
	load_p_limits();
	if (branchy)
		a.store32i(mem{SWP, NOREG, 1, o_skip}, 0);

	// p を 24bit に詰める（meg_pack24）。入力 rax、出力 eax
	const auto pack24 = [&]() {
		// meg_pack24 と同じく 0 の側へ切り捨てる（負なら 0x7fff を足してから右へ）
		a.mov64(RCX, RAX);
		a.sar64(RCX, 63);
		a.and32i(RCX, 0x7fff);
		a.add64(RAX, RCX);
		a.sar64(RAX, 15);
		// 1 つだけはみ出したときは限界に止め、ほかは 24bit で折り返す（meg_pack24 と同じ）
		a.cmp64ri(RAX, 0x800000);
		a.cmove64(RAX, K_MAX);
		a.cmp64ri(RAX, u32(s32(-0x800001)));
		a.cmove64(RAX, K_MIN);
		a.shl32(RAX, 8);
		a.sar32(RAX, 8);
	};
	// 乱数を 1 つ引く（swp30_device::rand）。出力 eax
	const auto rnd = [&]() {
		a.imul32i(RAX, SEED, 1664525);
		a.add32i(RAX, 1013904223);
		a.mov32(SEED, RAX);
		a.rol32(RAX, 16);
	};
	// p に雑音を足して詰める（dm の 6 番、dr の p）。出力 eax
	const auto p_packed = [&](bool noise) {
		if (noise) {
			rnd();
			a.and32i(RAX, 0x07e0);
			a.mov64(RDX, RAX);
			a.mov64(RAX, P);
			a.add64(RAX, RDX);
		} else
			a.mov64(RAX, P);
		pack24();
	};
	// p を acc(rax) へ／acc を n 右（算術）
	const auto AccFromP = [&]() { a.mov64(RAX, P); };
	const auto ShrAcc = [&](u8 n) { a.sar64(RAX, n); };
#endif

	for (u32 k = 0; k != 0x180; k++) {
		const meg_state::op &o = ops[k];

		// ---- 反映（遅れて入る書き込み）----
		if (k < 3 || branchy) {
			const u32 s = slot3(k);
			// m
			a.loadu8(RAX, M(o_mw_reg + s));
			a.test32(RAX, RAX);
			size_t j1 = a.jz_fwd();
			a.load32(RCX, M(o_mw_value + 4 * s));
			a.store32(mem{MS, RAX, 4, o_m}, RCX);
			a.patch(j1);
			// r
			a.loadu8(RAX, M(o_rw_reg + s));
			a.test32(RAX, RAX);
			size_t j2 = a.jz_fwd();
			a.load32(RCX, M(o_rw_value + 4 * s));
			a.store32(mem{MS, RAX, 4, o_r}, RCX);
			a.patch(j2);
			// index
			a.loadu8(RAX, M(o_ix_act + s));
			a.test32(RAX, RAX);
			size_t j3 = a.jz_fwd();
			a.load32(RCX, M(o_ix_value + 4 * s));
			a.store32(M(o_ram_index), RCX);
			a.patch(j3);
			// 2 つ目の index
			a.loadu8(RAX, mem{SWP, NOREG, 1, s32(o_ix2_act + s)});
			a.test32(RAX, RAX);
			size_t j4 = a.jz_fwd();
			a.load32(RCX, mem{SWP, NOREG, 1, o_ix2_value + 4 * s32(s)});
			a.store32(mem{SWP, NOREG, 1, o_ram_index2}, RCX);
			a.patch(j4);
		} else {
			const meg_state::op &w = ops[k - 3];
			const u32 s = slot3(k);
			if (w.dm && !early_m[w.dm]) {
				a.load32(RCX, M(o_mw_value + 4 * s));
				a.store32(M(o_m + 4 * w.dm), RCX);
			}
			if (w.dr && !early_r[w.dr]) {
				a.load32(RCX, M(o_rw_value + 4 * s));
				a.store32(M(o_r + 4 * w.dr), RCX);
			}
			if (w.index) {
				a.load32(RCX, M(o_ix_value + 4 * s));
				a.store32(M(o_ram_index), RCX);
			}
			if (w.index2) {
				a.load32(RCX, mem{SWP, NOREG, 1, o_ix2_value + 4 * s32(s)});
				a.store32(mem{SWP, NOREG, 1, o_ram_index2}, RCX);
			}
		}
		if (k < 2 || branchy) {
			const u32 s = slot2(k);
			a.loadu8(RAX, M(o_memw_act + s));
			a.test32(RAX, RAX);
			size_t j1 = a.jz_fwd();
			a.load32(RCX, M(o_memw_val + 4 * s));
			a.store32(M(o_ram_write), RCX);
			a.store8i(M(o_memw_act + s), 0);
			a.patch(j1);
			a.loadu8(RAX, M(o_memr_act + s));
			a.test32(RAX, RAX);
			size_t j2 = a.jz_fwd();
			a.load32(RCX, M(o_memr_val + 4 * s));
			a.store32(M(o_ram_read), RCX);
			a.store8i(M(o_memr_act + s), 0);
			a.patch(j2);
		} else {
			const meg_state::op &w = ops[k - 2];
			const u32 s = slot2(k);
			if (w.memw) {
				a.load32(RCX, M(o_memw_val + 4 * s));
				a.store32(M(o_ram_write), RCX);
			}
			if (w.memop == 2 || w.memop == 3) {
				a.load32(RCX, M(o_memr_val + 4 * s));
				a.store32(M(o_ram_read), RCX);
			}
		}

		// ---- 分岐と、飛ばされた命令 ----
		size_t skip_jump = 0, jump_done = 0;
		if (branchy) {
			a.cmp32i_mem(mem{SWP, NOREG, 1, o_skip}, k);
			skip_jump = a.jcc_fwd(0x87);                             // ja: 飛ばす位置がこの命令より後
			if (o.jump) {
				// meg_cond を機械語で
				size_t no_jump = 0;
				if (o.cond & 8) {
					a.loadu8(RAX, mem{SWP, NOREG, 1, o_flag_n});
					if (!(o.cond & 4))
						a.xor32ri(RAX, 1);
					if (o.cond & 2) {
						a.loadu8(RCX, mem{SWP, NOREG, 1, o_flag_z});
						a.or32(RAX, RCX);
					}
					a.test32(RAX, RAX);
					no_jump = a.jz_fwd();
				}
				if (o.target > k)
					a.store32i(mem{SWP, NOREG, 1, o_skip}, o.target);
				if (no_jump)
					a.patch(no_jump);
				jump_done = a.jmp_fwd();                             // 分岐の命令そのものも、飛ばされた命令と同じ後始末をする
			}
		}
		if (!(branchy && o.jump)) {

		// ---- ALU ----
		// bake のときは定数（と、そこから決まる m1）を焼き込む。係数 0 の掛け算は省き、
		// 「p = 0 * x + p」のように p が変わらない命令は丸ごと省く（結果はビット単位で同じ）
		bool alu_skip = false;
		if (o.alu && bake && !o.m1_from_t && o.mmode != 3) {
			s64 c = ms.m_const[k];
			if (o.m1_expand)
				c = meg_state::m1_expand(s16(c));
			const bool m_zero = o.mmode == 0 || c == 0;
			if (m_zero && o.asel == 0 && o.rop == 0 && o.shift == 0 && o.clamp == 0 && !o.latch)
				alu_skip = true;                                 // p はもう 42bit に収まっている
			else if (m_zero) {
				a.xor32(RAX, RAX);
#if SMU_X64ASM_MODE == 32
				a.xor32(RDX, RDX);                         // acc=(eax,edx)。hi も 0
#endif
			}
			else if (o.mmode == 1)
#if SMU_X64ASM_MODE == 32
				{ a.imm32(RAX, u32(u64(c) << (8 + 15))); a.imm32(RDX, u32(c >> 9)); }   // hi は算術右シフト（c<0 でも符号を保つ）
#else
				a.imm64(RAX, u64(c << (8 + 15)));
#endif
			else {
#if SMU_X64ASM_MODE == 32
				// acc=(eax,edx)。積の hi は imul64i の制約で esp 不可。hi を esi に置いて掛け、eax,edx に戻す
				a.load32(RAX, o.m2_from_m ? M(o_m + 4 * o.sm) : M(o_r + 4 * o.sr));  // lo（movsx は使わない＝32bit で読む）
				a.mov32(RSI, RAX); a.sar32(RSI, 31);                                 // hi = 符号
				a.imul64i(RAX, RSI, u32(s32(c)));                                    // 結果 eax(lo), esi(hi)
				a.mov32(RDX, RSI);
#else
				a.loads32(RAX, o.m2_from_m ? M(o_m + 4 * o.sm) : M(o_r + 4 * o.sr));
				a.imul64i(RAX, RAX, u32(s32(c)));
#endif
			}
		}
		if (o.alu && !alu_skip) {
			if (!(bake && !o.m1_from_t && o.mmode != 3)) {
#if SMU_X64ASM_MODE == 32
			// 掛ける値 m1 を eax に。acc は (eax,edx)。mmode==2 だけ hi を esi に置く（imul64 の制約）
			if (o.m1_from_t == 2) {
				// 印（負）が立っていれば t、そうでなければ定数（meg_state::step と同じ、doc/upstream.md の 29）
				a.loads16(ECX, M(o_t + 2 * o.t));                  // t（負のとき使う）
				a.loads16(RAX, M(o_const + 2 * s32(k)));           // 定数
				a.loadu8(RDX, mem{SWP, NOREG, 1, o_flag_n});
				a.test32(RDX, RDX);
				a.cmovcc64(0x44, RAX, RDX, RCX, RDX);              // zf（flag_n==0）なら定数 ecx を選択（hi は元 acc_hi のまま）
			} else if (o.m1_from_t)
				a.loads16(RAX, M(o_t + 2 * o.t));
			else
				a.loads16(RAX, M(o_const + 2 * s32(k)));
			if (o.m1_expand)
				emit_m1_expand(a);                           // 0〜0x7ffc。ecx を壊す
			switch (o.mmode) {
			case 0:
				a.xor32(RAX, RAX); a.xor32(RDX, RDX);
				break;
			case 1:
				a.mov32(RDX, RAX); a.sar32(RDX, 31); a.shl64(RAX, RDX, 8 + 15);   // m1 の符号を hi に（m1_expand 済みなら 0）
				break;
			case 2:
				// acc=(eax,esi)。掛ける相手を ecx:ebx に置く（ebx は MS を一時的に預ける）
				if (o.m1_expand) a.xor32(RSI, RSI);
				else { a.mov32(RSI, RAX); a.sar32(RSI, 31); }
				a.load32(ECX, o.m2_from_m ? M(o_m + 4 * o.sm) : M(o_r + 4 * o.sr));
				a.push(RBX);
				a.mov32(RBX, RCX); a.sar32(RBX, 31);         // 相手の hi
				a.imul64(RAX, RSI, ECX, RBX);
				a.mov32(RDX, RSI);
				a.pop(RBX);
				break;
			default:
				// mmode==3: m/r の値を 15bit 左（m1 は使わない。x64 と同じくロードし直す）
				a.load32(RAX, o.m2_from_m ? M(o_m + 4 * o.sm) : M(o_r + 4 * o.sr));
				a.mov32(RDX, RAX); a.sar32(RDX, 31); a.shl64(RAX, RDX, 15);
				break;
			}
#else
			if (o.m1_from_t == 2) {
				// 印（負）が立っていれば t、そうでなければ定数（meg_state::step と同じ、doc/upstream.md の 29）
				a.loads16(RAX, M(o_t + 2 * o.t));
				a.loads16(RCX, M(o_const + 2 * s32(k)));
				a.loadu8(RDX, mem{SWP, NOREG, 1, o_flag_n});
				a.test32(RDX, RDX);
				a.cmove64(RAX, RCX);
			} else if (o.m1_from_t)
				a.loads16(RAX, M(o_t + 2 * o.t));
			else
				a.loads16(RAX, M(o_const + 2 * s32(k)));
			if (o.m1_expand)
				emit_m1_expand(a);                           // meg_state::m1_expand を機械語で
			switch (o.mmode) {
			case 0:
				a.xor32(RAX, RAX);
				break;
			case 1:
				a.shl64(RAX, 8 + 15);
				break;
			case 2:
				a.loads32(RCX, o.m2_from_m ? M(o_m + 4 * o.sm) : M(o_r + 4 * o.sr));
				a.imul64(RAX, RCX);
				break;
			default:
				a.loads32(RAX, o.m2_from_m ? M(o_m + 4 * o.sm) : M(o_r + 4 * o.sr));
				a.shl64(RAX, 15);
				break;
			}
#endif
			}
			switch (o.asel) {
#if SMU_X64ASM_MODE == 32
			// acc=(eax,edx)。相手 b を (ecx,esi) に
			case 0: a.load32(RCX, FM(F_PLO)); a.load32(RSI, FM(F_PHI)); break;
			case 1: a.load32(RCX, M(o_r + 4 * o.sr)); a.mov32(RSI, RCX); a.sar32(RSI, 31); a.shl64(RCX, RSI, 15); break;
			case 2: a.load32(RCX, M(o_m + 4 * o.sm)); a.mov32(RSI, RCX); a.sar32(RSI, 31); a.shl64(RCX, RSI, 15); break;
			case 3: a.load32(RCX, FM(F_PLO)); a.load32(RSI, FM(F_PHI)); a.sar64(RCX, RSI, 15); break;
			default: a.xor32(RCX, RCX); a.xor32(RSI, RSI); break;
#else
			case 0: a.mov64(RCX, P); break;
			case 1: a.loads32(RCX, M(o_r + 4 * o.sr)); a.shl64(RCX, 15); break;
			case 2: a.loads32(RCX, M(o_m + 4 * o.sm)); a.shl64(RCX, 15); break;
			case 3: a.mov64(RCX, P); a.sar64(RCX, 15); break;
			default: a.xor32(RCX, RCX); break;
#endif
			}
			switch (o.rop) {
			case 0:
#if SMU_X64ASM_MODE == 32
				a.add32(RAX, RCX); a.adc32(RDX, RSI);
#else
				a.add64(RAX, RCX);
#endif
				break;
			case 1:
#if SMU_X64ASM_MODE == 32
				a.sub32(RAX, RCX); a.sbb32(RDX, RSI);
#else
				a.sub64(RAX, RCX);
#endif
				break;
			case 2:
#if SMU_X64ASM_MODE == 32
				// a + |b|。acc=(eax,edx) を一時スロットへ退避し、|b| を eax:edx に作って足す
				a.store32(FM(F_ALO), RAX); a.store32(FM(F_AHI), RDX);
				a.mov32(RAX, RCX); a.mov32(RDX, RSI);          // (eax,edx) = b
				a.neg64(RAX, RDX);                              // -b（sf = その符号）
				a.cmovs64(RAX, RDX, RCX, RSI);                  // b が正なら b を戻す → |b|
				a.add32rm(RAX, FM(F_ALO));                       // + a_lo
				a.load32(RCX, FM(F_AHI));                        // a_hi（mov なので cf は保持）
				a.adc32(RDX, RCX);
#else
				a.mov64(RDX, RCX);
				a.neg64(RDX);
				a.cmovs64(RDX, RCX);
				a.add64(RAX, RDX);
#endif
				break;
			default:
#if SMU_X64ASM_MODE == 32
				a.and32(RAX, RCX); a.and32(RDX, RSI);
#else
				a.and64(RAX, RCX);
#endif
				break;
			}
			if (o.shift)
#if SMU_X64ASM_MODE == 32
				a.shl64(RAX, RDX, o.shift);
#else
				a.shl64(RAX, o.shift);
#endif
			if (o.clamp == 0) {
#if SMU_X64ASM_MODE == 32
				a.shl64(RAX, RDX, 22);
				a.sar64(RAX, RDX, 22);
#else
				a.shl64(RAX, 22);
				a.sar64(RAX, 22);
#endif
			}
			switch (o.clamp) {
			case 0: break;
			case 1:
#if SMU_X64ASM_MODE == 32
				// 飽和（acc=(eax,edx) は ±2^41 収まり。hi の値だけで判定できる）
				// P_MIN=-2^38 の hi=0xffffffc0。lo の tie（0）は hi==0xffffffc0 の時必ず acc>=P_MIN
				a.cmp32ri(RDX, 0xffffffc0u);
				a.imm32(RCX, 0u); a.imm32(RSI, 0xffffffc0u);
				a.cmovcc64(0x4c, RAX, RDX, RCX, RSI);          // cmovl → P_MIN
				// P_MAX=+2^38-1 の hi=0x3f。lo の tie（0xffffffff）は hi==0x3f の時必ず acc<=P_MAX
				a.cmp32ri(RDX, 0x3f);
				a.imm32(RCX, 0xffffffffu); a.imm32(RSI, 0x3f);
				a.cmovcc64(0x4f, RAX, RDX, RCX, RSI);          // cmovg → P_MAX
#else
				a.cmp64(RAX, P_MIN);
				a.cmovl64(RAX, P_MIN);
				a.cmp64(RAX, P_MAX);
				a.cmovg64(RAX, P_MAX);
#endif
				break;
			case 2:
#if SMU_X64ASM_MODE == 32
				a.xor32(RCX, RCX); a.xor32(RSI, RSI);
				a.test32(RDX, RDX);                             // 符号は hi の bit31
				a.cmovcc64(0x48, RAX, RDX, RCX, RSI);          // cmovs（負）→ 0
				a.cmp32ri(RDX, 0x3f);
				a.imm32(RCX, 0xffffffffu); a.imm32(RSI, 0x3f);
				a.cmovcc64(0x4f, RAX, RDX, RCX, RSI);          // cmovg → P_MAX
#else
				a.xor32(RCX, RCX);
				a.cmp64(RAX, RCX);
				a.cmovl64(RAX, RCX);
				a.cmp64(RAX, P_MAX);
				a.cmovg64(RAX, P_MAX);
#endif
				break;
			default:
#if SMU_X64ASM_MODE == 32
				a.mov32(RCX, RAX); a.mov32(RSI, RDX);
				a.neg64(RCX, RSI);                              // -acc
				a.cmovs64(RCX, RSI, RAX, RDX);                  // 正なら元のまま → |acc|
				a.mov32(RAX, RCX); a.mov32(RDX, RSI);
				a.cmp32ri(RDX, 0x3f);
				a.imm32(RCX, 0xffffffffu); a.imm32(RSI, 0x3f);
				a.cmovcc64(0x4f, RAX, RDX, RCX, RSI);          // cmovg → P_MAX
#else
				a.mov64(RDX, RAX);
				a.neg64(RDX);
				a.cmovs64(RDX, RAX);
				a.mov64(RAX, RDX);
				a.cmp64(RAX, P_MAX);
				a.cmovg64(RAX, P_MAX);
#endif
				break;
			}
#if SMU_X64ASM_MODE == 32
			a.store32(FM(F_PLO), RAX); a.store32(FM(F_PHI), RDX);   // p を置き場へ
			if (o.latch) {
				a.test32(RDX, RDX);                             // 符号（hi の bit31。test32 は of=0）
				a.setl_mem(mem{SWP, NOREG, 1, o_flag_n});
				a.test64(RAX, RDX, RAX, RDX);                   // ゼロ
				a.sete_mem(mem{SWP, NOREG, 1, o_flag_z});
			}
#else
			a.mov64(P, RAX);
			if (o.latch) {
				a.test64(P, P);
				a.setl_mem(mem{SWP, NOREG, 1, o_flag_n});
				a.test64(P, P);
				a.sete_mem(mem{SWP, NOREG, 1, o_flag_z});
			}
#endif
		}

		// ---- dm ----
		if (o.dm) {
			switch (o.dm_src) {
			case 0: case 1: case 2: case 3:
				if (sintab && o.lfo < 0x18) {
					// meg_state::get_lfo を機械語で（関数は呼ばない）。出力 eax。rcx rdx r8 を壊す
					static constexpr u32 lfo_offsets[16] = {
						0x00000, 0x02aaa, 0x04000, 0x05555, 0x08000, 0x0aaaa, 0x0c000, 0x0d555,
						0x10000, 0x12aaa, 0x14000, 0x15555, 0x18000, 0x1aaaa, 0x1c000, 0x1d555,
					};
					a.load32(RAX, M(o_lfo_counter + 4 * s32(o.lfo)));
					a.shr32(RAX, 5);
					a.loadu16(RDX, M(o_lfo + 2 * s32(o.lfo)));
					a.mov32(RCX, RDX);
					a.shr32(RCX, 8);
					a.and32i(RCX, 3);
					a.shl32cl(RAX);
					a.mov32(RCX, RDX);
					a.shr32(RCX, 12);
#if SMU_X64ASM_MODE == 32
					a.imm32(RSI, u32(uintptr_t(lfo_offsets)));
					a.load32(RCX, mem{RSI, RCX, 4, 0});
#else
					a.imm64(R8, u64(uintptr_t(lfo_offsets)));
					a.load32(RCX, mem{R8, RCX, 4, 0});
#endif
					a.add32(RAX, RCX);
					a.and32i(RAX, 0x1ffff);
					a.shr32(RDX, 10);
					a.and32i(RDX, 3);
					std::vector<size_t> done;
					a.test32(RDX, RDX);
					const size_t not_sine = a.jcc_fwd(0x85);
					{   // sine
						a.mov32(RCX, RAX);
						a.and32i(RCX, 0x7fff);
						a.test32ri(RAX, 0x8000);
						const size_t no_rev = a.jcc_fwd(0x84);
						a.xor32ri(RCX, 0x7fff);
						a.patch(no_rev);
#if SMU_X64ASM_MODE == 32
						a.imm32(RSI, u32(uintptr_t(sintab)));
						a.mov32(RDX, RAX);
						a.loadu16(RAX, mem{RSI, RCX, 2, 0});
#else
						a.imm64(R8, u64(uintptr_t(sintab)));
						a.mov32(RDX, RAX);
						a.loadu16(RAX, mem{R8, RCX, 2, 0});
#endif
						a.test32ri(RDX, 0x10000);
						const size_t no_neg = a.jcc_fwd(0x84);
						a.xor32ri(RAX, 0xffff);
						a.patch(no_neg);
						done.push_back(a.jmp_fwd());
					}
					a.patch(not_sine);
					a.cmp32ri(RDX, 1);
					const size_t not_tri = a.jcc_fwd(0x85);
					{   // tri
						a.add32ri(RAX, 0x8000);
						a.and32i(RAX, 0x1ffff);
						a.test32ri(RAX, 0x10000);
						const size_t no_fold = a.jcc_fwd(0x84);
						a.xor32ri(RAX, 0x1ffff);
						a.patch(no_fold);
						done.push_back(a.jmp_fwd());
					}
					a.patch(not_tri);
					a.cmp32ri(RDX, 2);
					const size_t not_up = a.jcc_fwd(0x85);
					a.shr32(RAX, 1);                                     // saw up
					done.push_back(a.jmp_fwd());
					a.patch(not_up);
					a.xor32ri(RAX, 0x1ffff);                             // saw down
					a.shr32(RAX, 1);
					for (size_t d : done) a.patch(d);
					a.shl32(RAX, 7);
				} else {
#if SMU_X64ASM_MODE == 32
					// cdecl: 引数を後ろから積む（lfo, ms）。p/seed は置き場にあって callee-saved を壊さない
					a.imm32(RDX, o.lfo);
					a.push(RDX);
					a.push(MS);
					a.call_abs(reinterpret_cast<void *>(&meg_jit::call_lfo));
					a.addrsp(8);
#else
					a.mov64(RCX, MS);
					a.imm32(RDX, o.lfo);
					a.call_abs(reinterpret_cast<void *>(&meg_jit::call_lfo));
					load_p_limits();
#endif
				}
				break;
			case 4:
				a.load32(RAX, M(o_ram_read));
				break;
			case 5:
				rnd();
				a.shl32(RAX, 8);
				a.sar32(RAX, 8);
				break;
			case 6:
				p_packed(!o.no_noise);
				break;
			default:
				a.load32(RAX, M(o_m + 4 * o.sm));
				break;
			}
			if (k < 0x17d && early_m[o.dm]) {
				a.store32(M(o_m + 4 * o.dm), RAX);
				if (last_slot_m[k])
					a.store32(M(o_mw_value + 4 * slot3(k)), RAX);
			} else
				a.store32(M(o_mw_value + 4 * slot3(k)), RAX);
		}
		if (k >= 0x17d || branchy)
			a.store8i(M(o_mw_reg + slot3(k)), o.dm);

		// ---- dr ----
		if (o.dr) {
			if (o.dr_from_r)
				a.load32(RAX, M(o_r + 4 * o.sr));
			else
				p_packed(!o.no_noise);
			if (k < 0x17d && early_r[o.dr]) {
				a.store32(M(o_r + 4 * o.dr), RAX);
				if (last_slot_r[k])
					a.store32(M(o_rw_value + 4 * slot3(k)), RAX);
			} else
				a.store32(M(o_rw_value + 4 * slot3(k)), RAX);
		}
		if (k >= 0x17d || branchy)
			a.store8i(M(o_rw_reg + slot3(k)), o.dr);

		// ---- メモリへの書き値 ----
		if (o.memw) {
			AccFromP();
			ShrAcc(15);
			a.store32(M(o_memw_val + 4 * slot2(k)), RAX);
		}
		if (k >= 0x17e || branchy)
			a.store8i(M(o_memw_act + slot2(k)), o.memw ? 1 : 0);

		// ---- index ----
		if (o.index) {
			AccFromP();
			ShrAcc(15 + 8);
			a.store32(M(o_ix_value + 4 * slot3(k)), RAX);
		}
		if (k >= 0x17d || branchy)
			a.store8i(M(o_ix_act + slot3(k)), o.index ? 1 : 0);
		if (o.index2) {
			AccFromP();
			ShrAcc(15 + 8);
			a.store32(mem{SWP, NOREG, 1, o_ix2_value + 4 * s32(slot3(k))}, RAX);
		}
		if (k >= 0x17d || branchy)
			a.store8i(mem{SWP, NOREG, 1, o_ix2_act + s32(slot3(k))}, o.index2 ? 1 : 0);

		// ---- t ----
		if (o.t_write) {
			if (o.t_from_p)
				a.loadu16(RAX, M(o_t_value + 2 * slot2(k)));
			else if (bake)
				a.imm32(RAX, u16(ms.m_const[k]));
			else
				a.loadu16(RAX, M(o_const + 2 * s32(k)));
			a.store16(M(o_t + 2 * o.t), RAX);
		}
		if (need_tval[k]) {
			AccFromP();
			if (o.index || o.index2) {
				ShrAcc(8);
				a.and32i(RAX, 0x7fff);
			} else {
				ShrAcc(15 + 8);
#if SMU_X64ASM_MODE == 32
				// t は ±0x8000 に飽和。値は eax（edx は符号拡張）
				a.jlt64i(RAX, RDX, u32(s32(-0x8000)), [&] { a.imm32(RAX, u32(s32(-0x8000))); a.imm32(RDX, 0xffffffffu); });
				a.jgt64i(RAX, RDX, 0x7fff, [&] { a.imm32(RAX, 0x7fff); a.xor32(RDX, RDX); });
#else
				a.imm64(RCX, u64(s64(-0x8000)));
				a.cmp64(RAX, RCX);
				a.cmovl64(RAX, RCX);
				a.imm64(RCX, 0x7fff);
				a.cmp64(RAX, RCX);
				a.cmovg64(RAX, RCX);
#endif
			}
			a.store16(M(o_t_value + 2 * slot2(k)), RAX);
		}

		// ---- メモリ操作 ----
		size_t table_done = 0;
		if (o.memop >= 2 && o.mem_table) {
			// bit 0x23 の読み出しはリバーブ RAM の絶対番地（meg_state::step、doc/upstream.md の 24）
			a.loadu16(RAX, M(o_offset + 2 * s32(o.offset_index)));
			if (o.mem_use_index) {
				a.load32(RCX, M(o_ram_index));
				a.add32(RAX, RCX);
			}
			if (o.mem_use_index2) {
				a.load32(RCX, mem{SWP, NOREG, 1, o_ram_index2});
				a.add32(RAX, RCX);
			}
			if (o.memop == 3)
				a.add32i(RAX, 1);
			a.and32i(RAX, 0x3ffff);
			a.loadu16(RAX, mem{RAM, RAX, 2, 0});
			emit_revram_decode(a);
			a.store32(M(o_memr_val + 4 * slot2(k)), RAX);
			table_done = a.jmp_fwd();
		}
		if (o.memop) {
			a.loadu16(RAX, M(o_offset + 2 * s32(o.offset_index)));
			if (o.mem_use_index) {
				a.load32(RCX, M(o_ram_index));
				a.add32(RAX, RCX);
			}
			if (o.mem_use_index2) {
				a.load32(RCX, mem{SWP, NOREG, 1, o_ram_index2});
				a.add32(RAX, RCX);
			}
#if SMU_X64ASM_MODE == 32
			a.load32(RCX, FM(F_SC));
			a.sub32(RAX, RCX);
#else
			a.sub32(RAX, SC);
#endif
			if (o.memop == 3)
				a.add32i(RAX, 1);
			a.and32i(RAX, o.addr_mask);
			a.add32i(RAX, o.addr_base);
			a.and32i(RAX, 0x3ffff);
			if (o.memop == 1) {
				// meg_state::revram_encode を機械語で（関数は呼ばない）。番地を取っておく
#if SMU_X64ASM_MODE == 32
				// encode は eax ecx edx esi を全部壊すので番地は置き場へ退避
				a.store32(FM(F_ALO), RAX);
				a.load32(RAX, M(o_ram_write));
				emit_revram_encode(a);
				a.load32(RCX, FM(F_ALO));
				a.store16(mem{RAM, RCX, 2, 0}, RAX);
#else
				a.mov64(R8, RAX);
				a.load32(RAX, M(o_ram_write));
				emit_revram_encode(a);
				a.store16(mem{RAM, R8, 2, 0}, RAX);
#endif
			} else {
				// meg_state::revram_decode を機械語で（関数は呼ばない）
				a.loadu16(RAX, mem{RAM, RAX, 2, 0});
				emit_revram_decode(a);
				a.store32(M(o_memr_val + 4 * slot2(k)), RAX);
			}
		}
		if (table_done)
			a.patch(table_done);
		if (k >= 0x17e || branchy)
			a.store8i(M(o_memr_act + slot2(k)), (o.memop == 2 || o.memop == 3) ? 1 : 0);
		}   // !(branchy && o.jump)

		if (branchy) {
			const size_t normal_done = o.jump ? 0 : a.jmp_fwd();
			a.patch(skip_jump);
			if (jump_done)
				a.patch(jump_done);
			// 飛ばされた命令（と分岐の命令）: 輪に入れる書き込みを消し、t の値を入れる
			if (o.jump && o.t_write) {
				// 分岐の命令も t を書く（meg_state::step と同じ、doc/upstream.md の 31）
				if (o.t_from_p)
					a.loadu16(RAX, M(o_t_value + 2 * slot2(k)));
				else
					a.loadu16(RAX, M(o_const + 2 * s32(k)));
				a.store16(M(o_t + 2 * o.t), RAX);
			}
			a.store8i(M(o_mw_reg + slot3(k)), 0);
			a.store8i(M(o_rw_reg + slot3(k)), 0);
			a.store8i(M(o_memw_act + slot2(k)), 0);
			a.store8i(M(o_ix_act + slot3(k)), 0);
			a.store8i(mem{SWP, NOREG, 1, o_ix2_act + s32(slot3(k))}, 0);
			if (need_tval[k]) {
				AccFromP();
				ShrAcc(15 + 8);
#if SMU_X64ASM_MODE == 32
				a.jlt64i(RAX, RDX, u32(s32(-0x8000)), [&] { a.imm32(RAX, u32(s32(-0x8000))); a.imm32(RDX, 0xffffffffu); });
				a.jgt64i(RAX, RDX, 0x7fff, [&] { a.imm32(RAX, 0x7fff); a.xor32(RDX, RDX); });
#else
				a.imm64(RCX, u64(s64(-0x8000)));
				a.cmp64(RAX, RCX);
				a.cmovl64(RAX, RCX);
				a.imm64(RCX, 0x7fff);
				a.cmp64(RAX, RCX);
				a.cmovg64(RAX, RCX);
#endif
				a.store16(M(o_t_value + 2 * slot2(k)), RAX);
			}
			if (normal_done)
				a.patch(normal_done);
		}
	}

	// 出口
#if SMU_X64ASM_MODE == 32
	a.load32(RAX, FM(F_PLO)); a.store32(M(o_p), RAX);
	a.load32(RAX, FM(F_PHI)); a.store32(M(o_p + 4), RAX);
	a.load32(RAX, FM(F_SEED)); a.store32(mem{SWP, NOREG, 1, o_seed}, RAX);
	a.addrsp(24);
	a.pop(RBP); a.pop(RDI); a.pop(RSI); a.pop(RBX);
#else
	a.store64(M(o_p), P);
	a.store32(mem{SWP, NOREG, 1, o_seed}, SEED);
	a.addrsp(56);
	a.pop(RBP); a.pop(RDI); a.pop(RSI); a.pop(R15); a.pop(R14); a.pop(R13); a.pop(R12); a.pop(RBX);
#endif
	a.ret();

	if (a.code.size() > buf_size) {
		if (buf)
			VirtualFree(buf, 0, MEM_RELEASE);
		buf_size = (a.code.size() + 0xffff) & ~size_t(0xffff);
		buf = VirtualAlloc(nullptr, buf_size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
		if (!buf) {
			buf_size = 0;
			return false;
		}
	}
	DWORD old;
	VirtualProtect(buf, buf_size, PAGE_READWRITE, &old);
	std::memcpy(buf, a.code.data(), a.code.size());
	VirtualProtect(buf, buf_size, PAGE_EXECUTE_READ, &old);
	FlushInstructionCache(GetCurrentProcess(), buf, a.code.size());
	fn = reinterpret_cast<fn_t>(buf);
#if SMU_X64ASM_MODE == 32
	// x86-32 の JIT が本当に効いている証明（環境変数で 1 回だけ）。ビルドは meg_jit_run からしか来ない
	static bool done32 = false;
	if (!done32 && std::getenv("SMU2000_MEG_JIT32_LOG")) {
		done32 = true;
		std::fprintf(stderr, "meg-jit32: x86-32 build() emitted %zu bytes\n", a.code.size());
	}
	if (const char *dp = std::getenv("SMU2000_MEG_JIT32_DUMP")) {   // ALU を含む大きめのプログラムを 1 本
		static int dumped = 0;
		if (dumped < 1 && a.code.size() > 4000) {
			dumped++;
			if (FILE *f = std::fopen(dp, "wb")) { std::fwrite(a.code.data(), 1, a.code.size(), f); std::fclose(f); }
		}
	}
	if (const char *ds = std::getenv("SMU2000_MEG_JIT32_DUMPSAMPLE")) {   // 分岐サンプル周辺の全プログラムを番号付きで
		static int n = 0;
		if (n < 40 && a.code.size() > 1500) {
			char nm[512]; std::snprintf(nm, sizeof nm, "%s_%02d_%ld_%zu.bin", ds, n, (long)ms.m_sample_counter, a.code.size());
			if (FILE *f = std::fopen(nm, "wb")) { std::fwrite(a.code.data(), 1, a.code.size(), f); std::fclose(f); }
			n++;
		}
	}
#endif
	return true;
}

#endif
