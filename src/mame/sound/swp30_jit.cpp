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

#if defined(_WIN32) && defined(__x86_64__)
#define SMU2000_MEG_JIT 1
#include <windows.h>
#else
#define SMU2000_MEG_JIT 0
#endif

#include <cstdlib>
#include <cstring>
#include <vector>

#if SMU2000_MEG_JIT
#include "x64asm.h"
#endif

namespace {

#if SMU2000_MEG_JIT

using namespace x64asm;

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
	a.xor32(R11, R11);
	a.test32(RCX, RCX);
	a.setcc(0x95, R11);                                  // e != 0
	a.sub32(RCX, R11);                                   // e ? e - 1 : 0
	a.add32(R11, RCX);                                   // e
	a.shr32cl(RAX);
	a.and32i(RAX, 0x7ff);
	a.shl32(R11, 12);
	a.or32(RAX, R11);
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

// meg_state::revram_decode と同じことをする。入力 eax（u16）、出力 eax。rcx rdx r8 を壊す。分岐を使わない
void emit_revram_decode(assembler &a)
{
	a.mov32(R8, RAX);                                    // v
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
	a.mov32(RCX, R8);
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
#if SMU2000_MEG_JIT
	u64 bad = 0;
	for (int which = 0; which < 3; which++) {
		assembler a;
		a.mov32(RAX, RCX);
		if (which == 0) emit_revram_encode(a); else if (which == 1) emit_revram_decode(a); else emit_m1_expand(a);
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
			// 呼ぶ側は loads16 で 64bit に符号拡張した値を渡す。出力は 64bit のまま使う
			const auto fn64 = reinterpret_cast<s64 (*)(s64)>(buf);
			for (s32 v = -0x8000; v < 0x8000; v++)
				if (fn64(v) != s64(meg_state::m1_expand(s16(v))))
					bad++;
		}
		VirtualFree(buf, 0, MEM_RELEASE);
	}
	return bad;
#else
	return 0;
#endif
}

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
			else if (m_zero)
				a.xor32(RAX, RAX);
			else if (o.mmode == 1)
				a.imm64(RAX, u64(c << (8 + 15)));
			else {
				a.loads32(RAX, o.m2_from_m ? M(o_m + 4 * o.sm) : M(o_r + 4 * o.sr));
				a.imul64i(RAX, RAX, u32(s32(c)));
			}
		}
		if (o.alu && !alu_skip) {
			if (!(bake && !o.m1_from_t && o.mmode != 3)) {
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
			}
			switch (o.asel) {
			case 0: a.mov64(RCX, P); break;
			case 1: a.loads32(RCX, M(o_r + 4 * o.sr)); a.shl64(RCX, 15); break;
			case 2: a.loads32(RCX, M(o_m + 4 * o.sm)); a.shl64(RCX, 15); break;
			case 3: a.mov64(RCX, P); a.sar64(RCX, 15); break;
			default: a.xor32(RCX, RCX); break;
			}
			switch (o.rop) {
			case 0: a.add64(RAX, RCX); break;
			case 1: a.sub64(RAX, RCX); break;
			case 2:
				a.mov64(RDX, RCX);
				a.neg64(RDX);
				a.cmovs64(RDX, RCX);
				a.add64(RAX, RDX);
				break;
			default: a.and64(RAX, RCX); break;
			}
			if (o.shift)
				a.shl64(RAX, o.shift);
			if (o.clamp == 0) {
				a.shl64(RAX, 22);
				a.sar64(RAX, 22);
			}
			switch (o.clamp) {
			case 0: break;
			case 1:
				a.cmp64(RAX, P_MIN);
				a.cmovl64(RAX, P_MIN);
				a.cmp64(RAX, P_MAX);
				a.cmovg64(RAX, P_MAX);
				break;
			case 2:
				a.xor32(RCX, RCX);
				a.cmp64(RAX, RCX);
				a.cmovl64(RAX, RCX);
				a.cmp64(RAX, P_MAX);
				a.cmovg64(RAX, P_MAX);
				break;
			default:
				a.mov64(RDX, RAX);
				a.neg64(RDX);
				a.cmovs64(RDX, RAX);
				a.mov64(RAX, RDX);
				a.cmp64(RAX, P_MAX);
				a.cmovg64(RAX, P_MAX);
				break;
			}
			a.mov64(P, RAX);
			if (o.latch) {
				a.test64(P, P);
				a.setl_mem(mem{SWP, NOREG, 1, o_flag_n});
				a.test64(P, P);
				a.sete_mem(mem{SWP, NOREG, 1, o_flag_z});
			}
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
					a.imm64(R8, u64(uintptr_t(lfo_offsets)));
					a.load32(RCX, mem{R8, RCX, 4, 0});
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
						a.imm64(R8, u64(uintptr_t(sintab)));
						a.mov32(RDX, RAX);
						a.loadu16(RAX, mem{R8, RCX, 2, 0});
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
					a.mov64(RCX, MS);
					a.imm32(RDX, o.lfo);
					a.call_abs(reinterpret_cast<void *>(&meg_jit::call_lfo));
					load_p_limits();
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
			a.mov64(RAX, P);
			a.sar64(RAX, 15);
			a.store32(M(o_memw_val + 4 * slot2(k)), RAX);
		}
		if (k >= 0x17e || branchy)
			a.store8i(M(o_memw_act + slot2(k)), o.memw ? 1 : 0);

		// ---- index ----
		if (o.index) {
			a.mov64(RAX, P);
			a.sar64(RAX, 15 + 8);
			a.store32(M(o_ix_value + 4 * slot3(k)), RAX);
		}
		if (k >= 0x17d || branchy)
			a.store8i(M(o_ix_act + slot3(k)), o.index ? 1 : 0);
		if (o.index2) {
			a.mov64(RAX, P);
			a.sar64(RAX, 15 + 8);
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
			a.mov64(RAX, P);
			if (o.index || o.index2) {
				a.sar64(RAX, 8);
				a.and32i(RAX, 0x7fff);
			} else {
				a.sar64(RAX, 15 + 8);
				a.imm64(RCX, u64(s64(-0x8000)));
				a.cmp64(RAX, RCX);
				a.cmovl64(RAX, RCX);
				a.imm64(RCX, 0x7fff);
				a.cmp64(RAX, RCX);
				a.cmovg64(RAX, RCX);
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
			a.sub32(RAX, SC);
			if (o.memop == 3)
				a.add32i(RAX, 1);
			a.and32i(RAX, o.addr_mask);
			a.add32i(RAX, o.addr_base);
			a.and32i(RAX, 0x3ffff);
			if (o.memop == 1) {
				// meg_state::revram_encode を機械語で（関数は呼ばない）。番地は r8 に取っておく
				a.mov64(R8, RAX);
				a.load32(RAX, M(o_ram_write));
				emit_revram_encode(a);
				a.store16(mem{RAM, R8, 2, 0}, RAX);
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
				a.mov64(RAX, P);
				a.sar64(RAX, 15 + 8);
				a.imm64(RCX, u64(s64(-0x8000)));
				a.cmp64(RAX, RCX);
				a.cmovl64(RAX, RCX);
				a.imm64(RCX, 0x7fff);
				a.cmp64(RAX, RCX);
				a.cmovg64(RAX, RCX);
				a.store16(M(o_t_value + 2 * slot2(k)), RAX);
			}
			if (normal_done)
				a.patch(normal_done);
		}
	}

	// 出口
	a.store64(M(o_p), P);
	a.store32(mem{SWP, NOREG, 1, o_seed}, SEED);
	a.addrsp(56);
	a.pop(RBP); a.pop(RDI); a.pop(RSI); a.pop(R15); a.pop(R14); a.pop(R13); a.pop(R12); a.pop(RBX);
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
	return true;
}

#endif
