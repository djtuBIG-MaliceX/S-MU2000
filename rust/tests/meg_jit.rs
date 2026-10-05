//! M9b Phase B1+B2a+B2b-1+B2b-2a+B2b-2b — MEG (SWP30) x86-64 JIT seam
//! pins, selftest sweeps, offset-table + op-analysis pins
//! (JIT_M10_HANDOFF §8.B5, B2b-1/B2b-2a/B2b-2b briefs). Mounted via
//! smu-machine `[[test]]` (like jit.rs).
//!
//! Five jobs:
//!  1. LAYOUT/ABI pins from the HANDOFF §3/§5 table (FRAME, LFO slots, ring
//!     fold, the 8-push rsp-alignment invariant) — a silent constant drift in
//!     meg_jit.rs fails here instead of mis-aligning the B2 prologue.
//!  2. The `emit_*` SELFTEST sweeps (C++ meg_jit_selftest :520-585) run the
//!     scalar transliterations in meg_jit.rs against the PAIRED `meg.rs`
//!     ground truth (revram_encode/decode, m1_expand).
//!  3. `stub_build_false_never_runs` — the B1 contract: with `build()==false`
//!     `rebuild()` returns false and `run()` never executes compiled code, so
//!     every call site falls to the interpreter (bit-identical).
//!  4. B2a (Windows x64, cfg'd): the EMITTED helper bytes executed via RWX
//!     stubs like the C++ selftest, the frame-skeleton byte/alignment pins,
//!     the exec-buffer alloc/release roundtrip, and the PHASE_B2_EMIT_OK
//!     inertness gate (build() false even past the RAM guard, fnp stays 0).
//!  5. B2b-2a: byte pins for the new 16-bit immediate emitter methods
//!     (`store16i`/`cmp16i_mem`), the branchy skip-reset skeleton shape,
//!     the folded-leg compile-time skip/store shape, and (x64) exec rigs
//!     proving the skip reset clears the u16 in place and the ring legs
//!     apply/clear live state — all executed STANDALONE, never as a
//!     half-emitted program (brief: no half-compile; `b2b2a_build_inert`
//!     re-pins `gen_code_state().0 == 0`).
//!  6. B2b-2b: byte pins for the dm/dr legs and the pack24/rnd/rnd_skip/
//!     p_packed helpers (swp30_jit.cpp:843-889, :988, :1414-1496), exec
//!     rigs for the dm src select / tail-byte store / dr apply against
//!     live Swp30 state (vs the PAIRED meg::meg_pack24 / voice::swp_rand /
//!     meg::rand_skip), and `b2b2b_build_inert` re-pinning
//!     `gen_code_state().0 == 0` on dm/dr-flagged programs.
//!  7. B2b-2c: byte pins for the AccFromP/ShrAcc/ShrAccTZ15 accumulator
//!     lambdas (:883-889) and the memw + index legs (:1501-1525) incl. the
//!     §7-B ix2 window deref (SIB base R12), exec rigs proving AccFromP
//!     returns P verbatim, ShrAccTZ15/memw truncate toward zero (==
//!     meg::meg_mem_value) while the index bank shifts arithmetically
//!     (`>>23`, meg.rs:893/:899) and the window deref writes the interpreter's
//!     slot, and `b2b2c_build_inert` re-pinning `gen_code_state().0 == 0`.
//!  8. B2b-2d: byte pins for the t/tval legs — the `>>8 &0x7fff` index form
//!     and the `>>23` ±0x8000 cmp/cmov clamp pair (:1543-1544 / :1546-1557)
//!     — exec rigs proving the folded leg's index publish equals the
//!     interpreter `(p>>8)&0x7fff` (meg.rs:4129) while the clamp publish
//!     equals `s16_p23_clamped` clamping +0x7fff/−0x8000 and passing the
//!     exact boundaries (meg.rs:618), and that the branchy twin (:1647-1675)
//!     publishes the same value as the folded clamp leg for a fixed t input
//!     while erasing the five ring bytes (ix2 through the §7-B window);
//!     `b2b2d_build_inert` re-pins `gen_code_state().0 == 0`.
//!  9. B2b-2e: byte pins for the memop address leg (:1566-1638) — the base
//!     sum with the §7-B ix2 window deref, the x64 `sub eax,r14d` −SC arm
//!     (:1606, the 32-mode FM spill dead), the baked `addr_mask`/`addr_base`
//!     imm32 pair (:1610-1611), the scale-2 RAM forms, the compile-time
//!     region gate branches (disabled READ = 10-byte forced-0 store,
//!     disabled WRITE = EMPTY — :1589-1591), and the table-leg jmp patch
//!     (:1585/:1635) — exec rigs proving the computed address equals the
//!     interpreter `(off+ix+ix2−SC & mask)+base & 0x3ffff` u32 math across
//!     combos incl. the −SC wrap (meg.rs:1468-1483), the revram encode/
//!     decode round-trip through the emitted scale-2 store/load legs, the
//!     disabled-region inert path on live state (RAM untouched, read forced
//!     0), the mem_table bypass of the gate, and `b2b2e_build_inert`
//!     re-pinning `gen_code_state().0 == 0`.
//!  10. B2b-3b: the FULL-PROGRAM parity rigs — hoist+in-place LFO, all
//!     dm/dr/memw/memop/t/index/ring/skip/branch legs together, compiled by
//!     `program_bytes` (the same `emit_program` build() assembles behind the
//!     inert PHASE gate) and executed whole through X64Exec against the
//!     PAIRED `meg::run_program` for N steps with FULL MegState+device
//!     equality after every step (the C++ CHECK idea, :426-511); plus
//!     `b2b3b_build_inert` (rich program, still `gen_code_state().0 == 0`).
//!  11. B2b-3c MIGRATION (2026-10-05): `PHASE_B2_EMIT_OK == true` — emission
//!     is LIVE. Every `*_build_inert` row became a `*_build_live` row: the
//!     SAME programs (same ops/scenario) now demand `rebuild()==true ⇒
//!     fnp!=0 ⇒ run()==true` (the compiled block executes); the RAM guard
//!     (:611-612, <0x40000) refusal cases are KEPT. Dense/piano byte-EQ and
//!     the CHECK A/B leg are the standing JIT-ON proof (`--only dense` 合
//!     peak 14627, JIT入切 合, CHECK 1.05M blocks / 0 divergences 2026-10-05).

use std::mem::size_of;
use std::ptr::addr_of_mut;

use smu_machine::meg_jit::{
    analyze_ops, code_field_offsets, emit_acc_from_p, emit_call_lfo, emit_dm_src, emit_dm_store,
    emit_dr_apply, emit_epilogue, emit_frame_skeleton, emit_index_legs, emit_lfo, emit_lfo_slot_store,
    emit_memop, emit_memop_addr, emit_memop_addr_base, emit_memop_table, emit_memw_acc,
    emit_p_packed, emit_pack24, emit_prologue, emit_revram_decode, emit_revram_encode, emit_rnd,
    emit_rnd_skip, emit_ring2_folded, emit_ring3_folded, emit_shr_acc, emit_shr_acc_tz15,
    emit_t_branchy, emit_t_leg, emit_tval_clamp, env_flag, env_flag_on,
    meg_jit_call_lfo, offs, selftest_m1_expand, selftest_revram_decode, selftest_revram_encode,
    slot2, slot3, Assembler, Mem, MegJit, MegSwpDev, Offs, FRAME, K_MAX, K_MIN, LFO_SLOT_BASE,
    MEG_OPS, P, STABLE, R12, RAX, RBP, SEED,
};
use smu_swp30::meg::{MegState, MegSwp, Op};
use smu_swp30::mix::MegJitHook;
use smu_swp30::Swp30;

/// HANDOFF §3/§5 layout + ABI constants. These are the values the B2
/// prologue/epilogue and LFO hoist bake; a drift here = corrupt frame.
#[test]
fn layout_abi_pins() {
    // :295 STABLE = 8192 (BAKE threshold), :332 MEG_OPS = 0x180 ops/program
    assert_eq!(STABLE, 8192, "BAKE STABLE (swp30_jit.cpp:295)");
    assert_eq!(MEG_OPS, 0x180, "MEG_OPS (swp30_jit.cpp:332)");

    // §3 FRAME = 152 = shadow 32 + 16 + LFO slots 96 + align 8; LFO base 48
    assert_eq!(FRAME, 152, "FRAME (swp30_jit.cpp:817)");
    assert_eq!(LFO_SLOT_BASE, 48, "LFO_SLOT_BASE (swp30_jit.cpp:816)");
    // LFO slots run 48..144 (§5.3-8, `lfo_slot[0x18]`): base + 0x18*4 == 144
    assert_eq!(LFO_SLOT_BASE as usize + 0x18 * 4, 144, "LFO slot span 48..144");
    assert!((LFO_SLOT_BASE as usize + 0x18 * 4) <= FRAME as usize, "LFO slots fit in FRAME");

    // risk 5 / §3: rsp stays 16-aligned ONLY with exactly 8 pushes + FRAME.
    // At a Win64 call: return addr (8) + 8 pushes (64) + FRAME (152) == 224.
    assert_eq!((8 + 8 * 8 + FRAME as usize) % 16, 0, "8-push + FRAME keeps rsp 16-aligned");

    // §6: slot3/slot2 are compile-time ring folds — plain modular arithmetic.
    for d3 in 0..3u32 {
        for k in 0..16u32 {
            assert_eq!(slot3(d3, k), ((d3 + k) % 3) as usize, "slot3({d3},{k})");
        }
    }
    for d2 in 0..2u32 {
        for k in 0..16u32 {
            assert_eq!(slot2(d2, k), ((d2 + k) % 2) as usize, "slot2({d2},{k})");
        }
    }

    // §4/§7-F env polarity defaults (unset environment ⇒ JIT on, CHECK off).
    // SMU2000_MEG_JIT is ON unless the first char is '0'; the inverted CHECK
    // helper is OFF unless set with a non-'0' first char. (Read, never mutate:
    // integration tests share one process, so we do NOT touch the env.)
    if std::env::var_os("SMU2000_MEG_JIT").is_none() {
        assert!(env_flag("SMU2000_MEG_JIT", "0"), "MEG_JIT default ON");
    }
    if std::env::var_os("SMU2000_MEG_JIT_CHECK").is_none() {
        assert!(!env_flag_on("SMU2000_MEG_JIT_CHECK"), "CHECK default OFF (inverted)");
    }

    // Host-only Code snapshot layout: offsets increase in declaration order
    // (this is a host read, NOT a baked C++-byte layout — §7-D). Just pin the
    // invariant so a field reorder that breaks the run-guard reads is caught.
    let (d3_off, d2_off, code_size) = code_field_offsets();
    assert!(d3_off < d2_off, "Code.d3 before Code.d2 (run guard order)");
    assert!(d2_off < code_size, "Code.d2 within Code");
    assert_eq!(size_of::<u32>(), 4);
    assert_eq!(size_of::<u16>(), 2);
    assert_eq!(size_of::<usize>(), 8, "x86-64 target");

    println!("meg_jit_layout: STABLE={STABLE} OPS=0x{MEG_OPS:x} FRAME={FRAME} LFO={LFO_SLOT_BASE} Code(d3={d3_off},d2={d2_off},sz={code_size})");
}

/// C++ meg_jit_selftest (:520-585): sweep the scalar transliterations in
/// meg_jit.rs against the PAIRED meg.rs ground truth. The C++ assembles the
/// same three emit_* helpers and sweeps identical value sets (:555-576).
#[test]
fn selftest_sweeps() {
    // revram_encode: full 0x10_0000 (dense mantissa/exponent low range) plus a
    // prime-stride walk of the whole 27-bit space plus every e-boundary edge.
    // (B2's in-engine selftest does the full 0..0x8000000; a stride sweep here
    // keeps the gate fast while still crossing every exponent band.)
    for v in 0..0x10_0000u32 {
        assert_eq!(
            selftest_revram_encode(v) as u16,
            MegState::revram_encode(v),
            "revram_encode({v:#x})"
        );
    }
    let mut v = 0u32;
    while v < 0x800_0000 {
        assert_eq!(
            selftest_revram_encode(v) as u16,
            MegState::revram_encode(v),
            "revram_encode stride {v:#x}"
        );
        v = v.wrapping_add(1009);
    }
    // exponent / sign edges
    let mut edges: Vec<u32> = vec![0, 1, 0x3ff_ffff, 0x400_0000, 0x400_0001, 0x7ff_ffff];
    for e in 0..16u32 {
        let base = 0x400u32 << e;
        for d in [0i64, -1, 1, 2] {
            let x = (base as i64 + d) as u32 & 0x7ff_ffff;
            edges.push(x);
        }
    }
    for v in edges {
        assert_eq!(
            selftest_revram_encode(v) as u16,
            MegState::revram_encode(v),
            "revram_encode edge {v:#x}"
        );
    }

    // revram_decode: full 16-bit input space (cheap).
    for v in 0..0x1_0000u32 {
        assert_eq!(
            selftest_revram_decode(v),
            MegState::revram_decode(v as u16),
            "revram_decode({v:#x})"
        );
    }

    // m1_expand: full s16 domain, matching the C++ selftest sweep (:573-576).
    for v in -0x8000i64..0x8000 {
        let want = MegState::m1_expand(v as i16) as i64;
        let got = selftest_m1_expand(v);
        assert_eq!(got, want, "m1_expand({v})");
    }

    println!("meg_jit_selftest: encode/decode/m1_expand sweeps clean vs meg.rs ground truth");
}

/// B2b-3c MIGRATION of the B1 `stub_build_false_never_runs` contract:
/// `PHASE_B2_EMIT_OK == true` flips the default-env contract — enabled ⇒
/// `rebuild()==true` (fnp published) ⇒ `run()` executes the compiled block.
/// The OFF contract (force_off / SMU2000_MEG_JIT=0) stays byte-identical to
/// B1: rebuild false, fnp cleared, run() bails at the :390-392 null guard.
#[test]
fn stub_build_false_never_runs() {
    let mut s = Swp30::new();
    let sintab: Vec<u16> = Vec::new(); // empty (device reads 0) — zero-op program never uses it

    let mut j = MegJit::new();
    // Default environment: JIT is *enabled* (can()==true) and LIVE now.
    assert!(j.can(), "expected MEG JIT enabled in the default env");
    assert!(smu_machine::meg_jit::PHASE_B2_EMIT_OK, "B2b-3c: emission is LIVE");

    // rebuild: the C++ meg_jit_rebuild → build(gen); with a real 0x40000
    // reverb RAM this now COMPILES and publishes fnp.
    let rebuilt = j.rebuild(
        &s.meg,
        &s.meg_ops,
        s.reverb_ram.len(),
        s.revram_enable,
        &sintab,
        s.meg_const_gen,
    );
    assert!(rebuilt, "LIVE build() compiles past the RAM guard");
    assert_ne!(j.gen_code_state().0, 0, "fnp published (:1712)");

    // run: executes the compiled block (zero-op program — rets clean).
    let mut seam = MegSwp {
        flag_n: &mut s.meg_flag_n,
        flag_z: &mut s.meg_flag_z,
        ix2_value: &mut s.meg_ix2_value,
        ix2_act: &mut s.meg_ix2_act,
        ram_index2: &mut s.meg_ram_index2,
        skip_to: &mut s.meg_skip_to,
        revram_enable: s.revram_enable,
        reverb_ram: &mut s.reverb_ram,
        seed: &mut s.rand_seed,
        sintab: &sintab,
    };
    let ran = j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen);
    assert!(ran, "LIVE run() executes the compiled block");
    let rlen = seam.reverb_ram.len(); // through the seam: s.reverb_ram is &mut-borrowed

    // OFF half (unchanged B1 contract): force_off ⇒ rebuild false, fnp dead,
    // run() interpreter-falls at the :390 null guard.
    j.force_off();
    assert!(!j.rebuild(&s.meg, &s.meg_ops, rlen, s.revram_enable, &sintab, s.meg_const_gen));
    assert_eq!(j.gen_code_state().0, 0, "disabled box keeps fnp==0 (:361-363 reset)");
    assert!(!j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "fnp==0 ⇒ interpreter path (:390-392)");

    println!("meg_jit_stub: LIVE default env compiles+runs; force_off restores the B1 interpreter contract");
}

// ---------------------------------------------------------------------------
// B2b-1 — offset table + op analysis pins (origin: swp30_jit.cpp:620-652,
// :654-657, :660-666, :668-721; handoff §5.3-3/5/6, §7-B).
// ---------------------------------------------------------------------------

fn zero_ops() -> [Op; 0x180] {
    [Op::ZERO; 0x180]
}

/// The baked offset table pinned against an INDEPENDENT second source:
/// the literals below recompute the repr(C) layout from meg.rs's declared
/// field order (C rules). A declaration reorder, a width change, or a
/// dropped `#[repr(C)]` fails here instead of mis-baking B2b-2's op loop.
#[test]
fn b2b1_offs_table_pins() {
    // :654-657 widths mirrored at runtime (also const-pinned in meg_jit.rs)
    assert_eq!(size_of::<bool>(), 1); // index_active/memw_/memr_active/flags
    assert_eq!(size_of::<i64>(), 8); // m_p (load64/store64 legs)
    assert_eq!(size_of::<usize>(), 8); // window slots, x86-64

    // repr(C) MegState, declared order: decoded[0x180]@0 (Decoded=25B
    // stride → 9600), program@9600, konst@12672, offset@13440, lfo@13696,
    // lfo_increment@13744, lfo_counter@13840, map@13936, m@13952,
    // r@14208, t@14720, p@14736, mw_value@14744, mw_reg@14756(+3),
    // rw_value@14760(align pad), rw_reg@14772(+3), index_value@14776,
    // index_active@14788(+3), memw_value@14792, memr_value@14804,
    // t_value@14816, memw_active@14820(+3), memr_active@14823(+3),
    // delay_3@14828, delay_2@14832, ram_read@14836, ram_write@14840,
    // ram_index@14844, sample_counter@14848, program_address@14852,
    // pc@14854, icount@14856, retval@14860 → size 14864 (align 8).
    assert_eq!(size_of::<MegState>(), 14_864, "repr(C) MegState size");

    let o = offs();
    // MS side (:621-643)
    assert_eq!(o.m, 13952, ":621 m_m");
    assert_eq!(o.r, 14208, ":622 m_r");
    assert_eq!(o.t, 14720, ":623 m_t");
    assert_eq!(o.p, 14736, ":624 m_p");
    assert_eq!(o.konst, 12672, ":625 m_const");
    assert_eq!(o.offset, 13440, ":626 m_offset");
    assert_eq!(o.mw_value, 14744, ":627");
    assert_eq!(o.mw_reg, 14756, ":628");
    assert_eq!(o.rw_value, 14760, ":629 (mw_reg+3 → align-4 pad)");
    assert_eq!(o.rw_reg, 14772, ":630");
    assert_eq!(o.ix_value, 14776, ":631");
    assert_eq!(o.ix_act, 14788, ":632");
    assert_eq!(o.memw_val, 14792, ":633");
    assert_eq!(o.memr_val, 14804, ":634");
    assert_eq!(o.t_value, 14816, ":635");
    assert_eq!(o.memw_act, 14820, ":636");
    assert_eq!(o.memr_act, 14823, ":637");
    assert_eq!(o.ram_read, 14836, ":638");
    assert_eq!(o.ram_write, 14840, ":639");
    assert_eq!(o.ram_index, 14844, ":640");
    assert_eq!(o.sample, 14848, ":641");
    assert_eq!(o.lfo, 13696, ":642 (declared BEFORE m — table order is the C++ one)");
    assert_eq!(o.lfo_counter, 13840, ":643");
    // SWP side (:646-652) = MegSwpDev SLOTS (base ARG1 = window, §7-B)
    assert_eq!((o.seed, o.flag_n, o.flag_z), (0, 8, 16), ":646-648 slots");
    assert_eq!((o.ix2_value, o.ix2_act, o.ram_index2, o.skip), (24, 32, 40, 48), ":649-652 slots");
    assert_eq!(size_of::<MegSwpDev>(), 56);
    // table itself: 23 MS + 7 SWP i32 entries
    assert_eq!(size_of::<Offs>(), 30 * 4);

    println!("meg_jit_b2b1 offs: m={} r={} t={} p={} lfo={} seed-slot={} skip-slot={}", o.m, o.r, o.t, o.p, o.lfo, o.seed, o.skip);
}

/// §7-B window deviates from the C++ single-base layout by design — the
/// slots must point AT THE LIVE Swp30 fields (interpreter semantics stay
/// single-sourced; the JIT and the interpreter poke the same bytes).
#[test]
fn b2b1_swpdev_window_targets_live_state() {
    let mut s = Swp30::new();
    let sintab: Vec<u16> = Vec::new();
    // addresses BEFORE the seam borrows s (s is never moved after)
    let w_seed = addr_of_mut!(s.rand_seed);
    let w_fn = addr_of_mut!(s.meg_flag_n);
    let w_fz = addr_of_mut!(s.meg_flag_z);
    let w_ix2v = addr_of_mut!(s.meg_ix2_value);
    let w_ix2a = addr_of_mut!(s.meg_ix2_act);
    let w_rix2 = addr_of_mut!(s.meg_ram_index2);
    let w_skip = addr_of_mut!(s.meg_skip_to);

    let mut seam = MegSwp {
        flag_n: &mut s.meg_flag_n,
        flag_z: &mut s.meg_flag_z,
        ix2_value: &mut s.meg_ix2_value,
        ix2_act: &mut s.meg_ix2_act,
        ram_index2: &mut s.meg_ram_index2,
        skip_to: &mut s.meg_skip_to,
        revram_enable: s.revram_enable,
        reverb_ram: &mut s.reverb_ram,
        seed: &mut s.rand_seed,
        sintab: &sintab,
    };
    let dev = MegSwpDev::from_swp(&mut seam);
    assert_eq!(dev.seed, w_seed);
    assert_eq!(dev.flag_n, w_fn);
    assert_eq!(dev.flag_z, w_fz);
    assert_eq!(dev.ix2_value, w_ix2v);
    assert_eq!(dev.ix2_act, w_ix2a);
    assert_eq!(dev.ram_index2, w_rix2);
    assert_eq!(dev.skip, w_skip);
    // read-through sanity: the window sees the device's current value
    assert_eq!(unsafe { *dev.seed }, unsafe { *w_seed });
    println!("meg_jit_b2b1 swpdev: 7 window slots bound to live Swp30 state");
}

/// :660-666 — t publish analysis.
#[test]
fn b2b1_need_tval_analysis() {
    // empty program: only the tail pair publishes
    let a = analyze_ops(&zero_ops(), true);
    assert!(!a.branchy);
    for k in 0..0x180 {
        assert_eq!(a.need_tval[k], k >= 0x17e, "need_tval[{k:#x}] empty program");
    }
    // op k publishes when k+2 writes t from p (:664)
    let mut ops = zero_ops();
    ops[0x82].t_write = 1;
    ops[0x82].t_from_p = 1;
    let a = analyze_ops(&ops, true);
    assert!(a.need_tval[0x80], "two before a t-write-from-p");
    assert!(!a.need_tval[0x81]);
    assert!(!a.need_tval[0x7f]);
    // t_write WITHOUT t_from_p does not force the publish (:664 needs both)
    let mut ops = zero_ops();
    ops[0x82].t_write = 1;
    let a = analyze_ops(&ops, true);
    assert!(!a.need_tval[0x80]);
    // no ring wrap: `k + 2 < 0x180` (:664) — the only k it excludes are
    // 0x17e/0x17f, already set by the tail rule (:662-663), so the bound
    // is unobservable-by-construction (identical shape both sides). Pin
    // the in-range boundary instead:
    let mut ops = zero_ops();
    ops[0x17e].t_write = 1;
    ops[0x17e].t_from_p = 1;
    let a = analyze_ops(&ops, true);
    assert!(a.need_tval[0x17c], "k+2 in range up to the boundary");
    assert!(!a.need_tval[0x17b]);
    // a low k+2 target must NOT retro-force far-tail ops beyond the tail pair
    let mut ops = zero_ops();
    ops[1].t_write = 1;
    ops[1].t_from_p = 1;
    let a = analyze_ops(&ops, true);
    assert!(!a.need_tval[0x17d], "tail pair only — no wrapped k+2 read");
    println!("meg_jit_b2b1 need_tval: tail pair + k+2 t-from-p rule pinned");
}

/// :668-711 — early-write analysis: all five reads() legs, the 1..2 window,
/// the tail trio, the MEG_EARLY gate, the branchy gate, slot 0.
#[test]
fn b2b1_early_write_analysis() {
    // clean write (no readers) ⇒ early candidate
    let mut ops = zero_ops();
    ops[0x10].dr = 5;
    let a = analyze_ops(&ops, true);
    assert!(a.early_r[5], "unread write folds early");
    let mut ops = zero_ops();
    ops[0x10].dm = 3;
    let a = analyze_ops(&ops, true);
    assert!(a.early_m[3]);

    // leg 1 (:679): mmode2 reads r from m2 side ((m2_from_m!=0)==!m)
    let mut ops = zero_ops();
    ops[0x21].dr = 5;
    ops[0x22].alu = 1;
    ops[0x22].mmode = 2;
    ops[0x22].sr = 5;
    let a = analyze_ops(&ops, true);
    assert!(!a.early_r[5], "read 1 op after the write");
    // window back=2 with mmode3
    let mut ops = zero_ops();
    ops[0x20].dr = 5;
    ops[0x22].alu = 1;
    ops[0x22].mmode = 3;
    ops[0x22].sr = 5;
    let a = analyze_ops(&ops, true);
    assert!(!a.early_r[5], "read 2 ops after the write");
    // back=3 folds
    let mut ops = zero_ops();
    ops[0x1f].dr = 5;
    ops[0x22].alu = 1;
    ops[0x22].mmode = 2;
    ops[0x22].sr = 5;
    let a = analyze_ops(&ops, true);
    assert!(a.early_r[5], "3 ops back stays ring-folded");
    // leg 1 m-side: ((m2_from_m!=0)==m) with m2_from_m=1
    let mut ops = zero_ops();
    ops[0x51].dm = 4;
    ops[0x52].alu = 1;
    ops[0x52].mmode = 2;
    ops[0x52].m2_from_m = 1;
    ops[0x52].sm = 4;
    let a = analyze_ops(&ops, true);
    assert!(!a.early_m[4]);

    // leg 2 (:681): asel==1 reads r[sr]
    let mut ops = zero_ops();
    ops[0x31].dr = 9;
    ops[0x32].alu = 1;
    ops[0x32].asel = 1;
    ops[0x32].sr = 9;
    let a = analyze_ops(&ops, true);
    assert!(!a.early_r[9]);
    // leg 3 (:683): asel==2 reads m[sm]
    let mut ops = zero_ops();
    ops[0x61].dm = 6;
    ops[0x62].alu = 1;
    ops[0x62].asel = 2;
    ops[0x62].sm = 6;
    let a = analyze_ops(&ops, true);
    assert!(!a.early_m[6]);

    // leg 4 (:685): dr reading r (dr_from_r)
    let mut ops = zero_ops();
    ops[0x41].dr = 12;
    ops[0x42].dr = 3;
    ops[0x42].dr_from_r = 1;
    ops[0x42].sr = 12;
    let a = analyze_ops(&ops, true);
    assert!(!a.early_r[12]);
    // leg 5 (:687): dm_src==7 reading m[sm]
    let mut ops = zero_ops();
    ops[0x71].dm = 8;
    ops[0x72].dm = 2;
    ops[0x72].dm_src = 7;
    ops[0x72].sm = 8;
    let a = analyze_ops(&ops, true);
    assert!(!a.early_m[8]);

    // ring wrap window (:693 mod): the wrapped write positions 0x17e/0x17f
    // are ALSO tail-trio (:699-702), so the observable here is bad either
    // way — pinned together (the trio rule itself is isolated below).
    let mut ops = zero_ops();
    ops[0x17f].dr = 7;
    ops[1].alu = 1;
    ops[1].mmode = 2;
    ops[1].sr = 7;
    let a = analyze_ops(&ops, true);
    assert!(!a.early_r[7], "0x17f write seen by j=1 across the sample seam");

    // tail trio 0x17d-0x17f (:699-702): never early candidates
    for k in [0x17dusize, 0x17e, 0x17f] {
        let mut ops = zero_ops();
        ops[k].dr = 11;
        let a = analyze_ops(&ops, true);
        assert!(!a.early_r[11], "tail trio write {k:#x} excluded (:699-702)");
    }

    // MEG_EARLY off (:703-706 + :707-709): nothing folds
    let mut ops = zero_ops();
    ops[0x10].dr = 5;
    ops[0x11].dm = 3;
    let a = analyze_ops(&ops, false);
    assert!(a.early_r.iter().all(|&b| !b) && a.early_m.iter().all(|&b| !b));

    // branchy (:607-610 + :708-709): a single jump disables ALL folds
    let mut ops = zero_ops();
    ops[0x10].dr = 5;
    ops[0x20].jump = 1;
    let a = analyze_ops(&ops, true);
    assert!(a.branchy, "one jump op ⇒ branchy");
    assert!(!a.early_r[5], "branchy ⇒ no early writes (:707-709)");

    // slot 0 is never a candidate (:707 `x = 1; x != 128`)
    let a = analyze_ops(&zero_ops(), true);
    assert!(!a.early_r[0] && !a.early_m[0]);
    println!("meg_jit_b2b1 early-write: 5 read legs + window + trio + gates pinned");
}

/// :715-721 — last-slot rule ("not read, but keeps the state save
/// identical"): the folded write is ALSO written to the ring slot when it
/// is the last writer for that k%3 class.
#[test]
fn b2b1_last_slot_pins() {
    let mut ops = zero_ops();
    ops[5].dm = 1; // k%3 = 2
    ops[6].dr = 2; // k%3 = 0
    ops[8].dm = 2; // k%3 = 2 — later op wins its class
    let a = analyze_ops(&ops, true);
    assert!(a.last_slot_m[8] && !a.last_slot_m[5], "descending scan: later op wins class 2");
    assert!(a.last_slot_r[6]);
    assert_eq!(a.last_slot_m.iter().filter(|&&b| b).count(), 1, "class 1 has no dm");
    assert_eq!(a.last_slot_r.iter().filter(|&&b| b).count(), 1);

    // one op doing both writes earns both flags
    let mut ops = zero_ops();
    ops[0x17f].dm = 1;
    ops[0x17f].dr = 1;
    let a = analyze_ops(&ops, true);
    assert!(a.last_slot_m[0x17f] && a.last_slot_r[0x17f]);

    // class-0 later winner deep in the program
    let mut ops = zero_ops();
    ops[3].dm = 1; // 3%3=0
    ops[0x17d].dm = 2; // 381%3=0 — wins
    let a = analyze_ops(&ops, true);
    assert!(a.last_slot_m[0x17d] && !a.last_slot_m[3]);
    assert_eq!(a.last_slot_m.iter().filter(|&&b| b).count(), 1);

    // empty program: no flags
    let a = analyze_ops(&zero_ops(), true);
    assert!(a.last_slot_m.iter().all(|&b| !b) && a.last_slot_r.iter().all(|&b| !b));
    println!("meg_jit_b2b1 last_slot: descending-per-class rule pinned");
}

// ---------------------------------------------------------------------------
// B2b-2a — new 16-bit emitter byte pins + skip-reset/folded-leg shape
// (origin: swp30_jit.cpp:839-840, :1000-1048, :1050-1077; brief §3). Pure
// byte-assembly checks — platform-free; the EXEC proofs live in the b2a mod.
// ---------------------------------------------------------------------------

/// Hand-derived encodings for the two additive jit_emit methods (Intel SDM;
/// the 0x66 prefix flips BOTH operand and immediate to 16-bit — C7's imm is
/// 2 bytes here, not the 4 of store32i). rm() is always disp32
/// (jit_emit.rs:105) so lengths are offset-independent, which the B2b-3
/// branch gate (:1082 twin) relies on for jump patching.
#[test]
fn b2b2a_emitter_byte_pins() {
    // store16i [rax+0], 0x1234: prefix 66; REX suppressed (reg 0, base 0,
    // no index); op C7; ModRM 80|(0<<3)|0=80; disp32 0; imm16 LE 34 12.
    let mut a = Assembler::new();
    a.store16i(Mem::b(RAX, 0), 0x1234);
    assert_eq!(a.code, [0x66, 0xc7, 0x80, 0x00, 0x00, 0x00, 0x00, 0x34, 0x12]);
    // SIB + scale-2 form (RAM-store pattern of :1626): index present ⇒
    // ModRM 80|(0<<3)|4=84; SIB (ss 1<<6 scale2)|(idx RAX 0<<3)|(base 5)=45.
    let mut a = Assembler::new();
    a.store16i(Mem { base: RBP, index: RAX, scale: 2, disp: 0x40 }, 7);
    assert_eq!(a.code, [0x66, 0xc7, 0x84, 0x45, 0x40, 0x00, 0x00, 0x00, 0x07, 0x00]);
    // cmp16i_mem [rbp+8], 0x17f: no SIB (base&7=5≠RSP) ⇒ ModRM
    // 80|(7<<3)|5=BD; imm16 LE 7f 01.
    let mut a = Assembler::new();
    a.cmp16i_mem(Mem::b(RBP, 8), 0x17f);
    assert_eq!(a.code, [0x66, 0x81, 0xbd, 0x08, 0x00, 0x00, 0x00, 0x7f, 0x01]);
    // R12 base (the SWP window) forces REX 0x41 AND SIB (base&7==4):
    // ModRM 80|(7<<3)|4=BC, SIB (0<<6)|(4<<3)|4=24 — the exact B2b-3
    // branch-gate shape against [SWP, o_skip].
    let mut a = Assembler::new();
    a.cmp16i_mem(Mem::b(R12, 0), 0x82);
    assert_eq!(a.code, [0x66, 0x41, 0x81, 0xbc, 0x24, 0x00, 0x00, 0x00, 0x00, 0x82, 0x00]);
    println!("meg_jit_b2b2a emitter: store16i/cmp16i_mem byte-pinned (4 forms)");
}

/// The :839-840 skip leg rides the branchy skeleton exactly where the C++
/// puts it (between prologue and epilogue), 16-bit through the window slot;
/// and the folded rings are compile-time gated: early writes emit NOTHING,
/// ring2 legs appear only for memw/memop-2-3 (:1069-1076).
#[test]
fn b2b2a_skeleton_and_fold_shape() {
    let o = offs();
    // non-branchy skeleton twins the B2b-1 prologue++epilogue exactly
    let mut pa = Assembler::new();
    emit_prologue(&mut pa, o.p, o.sample, o.seed);
    let mut ea = Assembler::new();
    emit_epilogue(&mut ea, o.p, o.seed);
    let mut nb = Assembler::new();
    emit_frame_skeleton(&mut nb, &o, false);
    let mut want_plain = pa.code.clone();
    want_plain.extend_from_slice(&ea.code);
    assert_eq!(nb.code, want_plain, "non-branchy skeleton unchanged");
    assert!(!nb.code.windows(2).any(|w| w == [0x66, 0xc7]), "no skip leg when !branchy");
    // branchy skeleton = prologue ++ [load64 rax,[r12+o.skip]; store16i
    // [rax+0],0] ++ epilogue (load64: REX 49 W+B, op 8b, SIB form 84 24)
    let mut sa = Assembler::new();
    emit_frame_skeleton(&mut sa, &o, true);
    let mut want = pa.code.clone();
    want.extend_from_slice(&[0x49u8, 0x8b, 0x84, 0x24]);
    want.extend_from_slice(&o.skip.to_le_bytes());
    want.extend_from_slice(&[0x66, 0xc7, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    want.extend_from_slice(&ea.code);
    assert_eq!(sa.code, want, "branchy skeleton = prologue ++ skip-reset(u16) ++ epilogue");

    // folded 3-ring: early dm/dr fold to NOTHING (:1033/:1038 skip)
    let mut ops = zero_ops();
    ops[0x10].dm = 20;
    ops[0x10].dr = 70;
    let an = analyze_ops(&ops, true); // no reads in the window ⇒ both early
    assert!(an.early_m[20] && an.early_r[70]);
    let mut a = Assembler::new();
    emit_ring3_folded(&mut a, &o, 0, &ops[0x10], &an);
    assert!(a.code.is_empty(), "early dm/dr fold emits nothing (:1033/:1038)");
    // index legs are unconditional once flagged (:1041-1048)
    ops[0x10].index = 1;
    ops[0x10].index2 = 1;
    emit_ring3_folded(&mut a, &o, 0, &ops[0x10], &an);
    assert!(!a.code.is_empty(), "index/index2 folds emit");
    // a read inside the 2-op window (:687 dm_src==7) blocks early ⇒ the dm
    // fold must store from the ring slot again. The reader needs its own
    // non-zero dm (the :687 `o.dm` term) — it sources m[20] into m[1].
    let mut ops2 = zero_ops();
    ops2[0x10].dm = 20;
    ops2[0x11].alu = 1;
    ops2[0x11].dm = 1;
    ops2[0x11].dm_src = 7;
    ops2[0x11].sm = 20;
    let an2 = analyze_ops(&ops2, true);
    assert!(!an2.early_m[20]);
    let mut a = Assembler::new();
    emit_ring3_folded(&mut a, &o, 0, &ops2[0x10], &an2);
    assert!(!a.code.is_empty(), "read-window-blocked write stores in the fold");

    // folded 2-ring: nothing for a plain op; memw adds a leg; memop selects
    // the memr leg (2/3 yes, 1 no) (:1069-1076). Fresh Assembler per shape —
    // each call re-emits its full (memw + optional memr) leg set.
    let mut a = Assembler::new();
    emit_ring2_folded(&mut a, &o, 0, &Op::ZERO);
    assert!(a.code.is_empty(), "no memw/memop ⇒ no bytes");
    let mut w = Op::ZERO;
    w.memw = 1;
    let mut a = Assembler::new();
    emit_ring2_folded(&mut a, &o, 0, &w);
    let memw_leg = a.code.len();
    assert!(memw_leg > 0, "memw fold stores");
    let mut a = Assembler::new();
    w.memop = 1;
    emit_ring2_folded(&mut a, &o, 0, &w);
    assert_eq!(a.code.len(), memw_leg, "memop==1 adds no memr leg");
    let mut a = Assembler::new();
    w.memop = 2;
    emit_ring2_folded(&mut a, &o, 0, &w);
    let two = a.code.len();
    assert!(two > memw_leg, "memop==2 adds the memr leg");
    let mut a = Assembler::new();
    w.memop = 3;
    emit_ring2_folded(&mut a, &o, 0, &w);
    assert_eq!(a.code.len(), two, "memop==3 emits the same leg set as memop==2");
    println!("meg_jit_b2b2a shape: skip leg branchy-only 16-bit, folds compile-time gated");
}

// ---------------------------------------------------------------------------
// B2b-2b — dm/dr leg byte/shape pins (origin: swp30_jit.cpp:843-889, :988,
// :1414-1496; brief §3). Pure byte-assembly checks — platform-free; the
// EXEC proofs live in the b2a mod (same split as B2b-2a).
// ---------------------------------------------------------------------------

/// Hand-derived encodings (rr()/rm() rules, jit_emit.rs:92-138) for the
/// B2b-2b helper bytes: pack24 (every 64-bit leg carries REX.W; the two
/// cmove64 carry `48 0f 44`), rnd (the LCG pair + SEED store + rol16),
/// and rnd_skip (the SAME shape with the rand_jump(n) immediates —
/// recomputed independently from the PAIRED meg::rand_jump in-test).
#[test]
fn b2b2b_pack24_rnd_byte_pins() {
    // register-plan constants the pins below bake (swp30_jit.cpp:814/:819)
    assert_eq!((SEED, P, K_MAX, K_MIN), (6u8, 13u8, 7u8, 5u8), ":814/:819 plan");

    // pack24 (:845-856): mov rcx,rax / sar rcx,63 / and ecx,7fff /
    // add rax,rcx / sar rax,15 / cmp rax,0x800000 / cmove rax,rdi /
    // cmp rax,-0x800001 / cmove rax,rbp / shl eax,8 / sar eax,8
    let mut a = Assembler::new();
    emit_pack24(&mut a);
    assert_eq!(
        a.code,
        [
            0x48, 0x8b, 0xc8, // mov rcx,rax  (:845)
            0x48, 0xc1, 0xf9, 0x3f, // sar rcx,63 (:846)
            0x81, 0xe1, 0xff, 0x7f, 0x00, 0x00, // and ecx,0x7fff (:847)
            0x48, 0x01, 0xc8, // add rax,rcx (:848)
            0x48, 0xc1, 0xf8, 0x0f, // sar rax,15 (:849)
            0x48, 0x81, 0xf8, 0x00, 0x00, 0x80, 0x00, // cmp rax,0x800000 (:851)
            0x48, 0x0f, 0x44, 0xc7, // cmove rax,rdi (:852, K_MAX)
            0x48, 0x81, 0xf8, 0xff, 0xff, 0x7f, 0xff, // cmp rax,-0x800001 (:853)
            0x48, 0x0f, 0x44, 0xc5, // cmove rax,rbp (:854, K_MIN)
            0xc1, 0xe0, 0x08, // shl eax,8 (:855)
            0xc1, 0xf8, 0x08, // sar eax,8 (:856)
        ],
        "emit_pack24 bytes"
    );

    // rnd (:860-863): imul eax,esi,1664525 / add eax,1013904223 /
    // mov esi,eax / rol eax,16
    let mut a = Assembler::new();
    emit_rnd(&mut a);
    assert_eq!(
        a.code,
        [
            0x69, 0xc6, 0x0d, 0x66, 0x19, 0x00, // imul eax,esi,1664525 (:860)
            0x81, 0xc0, 0x5f, 0xf3, 0x6e, 0x3c, // add eax,1013904223 (:861)
            0x8b, 0xf0, // mov esi,eax (:862)
            0xc1, 0xc0, 0x10, // rol eax,16 (:863)
        ],
        "emit_rnd bytes"
    );

    // rnd_skip(n) (:869-871): SAME head shape, rand_jump(n) immediates.
    // Cross-checked against the PAIRED meg::rand_jump (meg.rs:586).
    for n in [1u32, 2, 3, 9] {
        let (mul, add) = smu_swp30::meg::rand_jump(n);
        let mut want: Vec<u8> = vec![0x69, 0xc6];
        want.extend_from_slice(&mul.to_le_bytes()); // imul eax,esi,mul
        want.push(0x81);
        want.push(0xc0);
        want.extend_from_slice(&add.to_le_bytes()); // add eax,add
        want.extend_from_slice(&[0x8b, 0xf0]); // mov esi,eax (NO rol — values unused)
        let mut a = Assembler::new();
        emit_rnd_skip(&mut a, n);
        assert_eq!(a.code, want, "emit_rnd_skip({n}) bytes");
    }

    // p_packed(noise=false) (:880+pack24): mov rax,r13 then the pin above
    let mut a = Assembler::new();
    emit_p_packed(&mut a, false);
    let mut want = vec![0x49u8, 0x8b, 0xc5]; // mov rax,r13 (:880)
    want.extend_from_slice(&[
        0x48, 0x8b, 0xc8, 0x48, 0xc1, 0xf9, 0x3f, 0x81, 0xe1, 0xff, 0x7f, 0x00, 0x00, 0x48, 0x01,
        0xc8, 0x48, 0xc1, 0xf8, 0x0f, 0x48, 0x81, 0xf8, 0x00, 0x00, 0x80, 0x00, 0x48, 0x0f, 0x44,
        0xc7, 0x48, 0x81, 0xf8, 0xff, 0xff, 0x7f, 0xff, 0x48, 0x0f, 0x44, 0xc5, 0xc1, 0xe0, 0x08,
        0xc1, 0xf8, 0x08,
    ]);
    let mut a2 = Assembler::new();
    emit_pack24(&mut a2);
    assert_eq!(&a.code[..3], &[0x49u8, 0x8b, 0xc5], "p_packed head = mov rax,P (:880)");
    assert_eq!(&a.code[3..], a2.code.as_slice(), "p_packed tail == emit_pack24");
    assert_eq!(a.code, want);
    // noise=true (:876-878): rnd() + and 0x07e0 + add rax,r13 + pack24
    let mut a = Assembler::new();
    emit_p_packed(&mut a, true);
    let mut a_rnd = Assembler::new();
    emit_rnd(&mut a_rnd);
    assert_eq!(&a.code[..a_rnd.code.len()], a_rnd.code.as_slice(), "noise head = rnd (:876)");
    let rest = &a.code[a_rnd.code.len()..];
    assert_eq!(&rest[..6], &[0x81u8, 0xe0, 0xe0, 0x07, 0x00, 0x00], "and eax,0x07e0 (:877)");
    assert_eq!(&rest[6..9], &[0x4cu8, 0x01, 0xe8], "add rax,r13 (:878, REX.R for R13 src)");
    assert_eq!(&rest[9..], a2.code.as_slice(), "noise tail == pack24 (:881)");
    println!("meg_jit_b2b2b helpers: pack24/rnd/rnd_skip/p_packed byte-pinned");
}

/// dm src select / dm store / dr apply leg SHAPES (compile-time gating,
/// disp32 fixed lengths for the B2b-3 jump patching, and the two un-hoisted
/// LFO arms now dispatching to emit_lfo / the callout) — origin:
/// swp30_jit.cpp:1414-1496.
#[test]
fn b2b2b_dm_dr_leg_shape() {
    let o = offs();
    fn mk(f: impl FnOnce(&mut Assembler)) -> Assembler {
        let mut a = Assembler::new();
        f(&mut a);
        a
    }
    // A non-null sintab stand-in: enough to make the dm-src arm take the
    // "resident" branch (it is only baked as an immediate never deref'd in a
    // byte-shape pin — the exec rigs below use a real table).
    let fake_sintab: *const u16 = usize::MAX as *const u16;

    // ---- dm src (:1415-1465) ----
    // frame-slot leg (:1419): mov eax,[rsp+48] — SIB forced (base RSP),
    // fixed 7 bytes whatever the slot
    let op0 = Op { dm_src: 0, lfo: 3, dm: 2, ..Op::ZERO };
    let a = mk(|a| emit_dm_src(a, &offs(), &op0, 48, std::ptr::null(), 0));
    assert_eq!(a.code, [0x8b, 0x84, 0x24, 48, 0x00, 0x00, 0x00], ":1419 frame-slot load");
    // LFO-slot STORE twin (:988): mov [rsp+52],eax
    let a = mk(|a| emit_lfo_slot_store(a, 52));
    assert_eq!(a.code, [0x89, 0x84, 0x24, 52, 0x00, 0x00, 0x00], ":988 hoist store");
    // ram_read leg (:1453)
    let op4 = Op { dm_src: 4, dm: 1, ..Op::ZERO };
    let a = mk(|a| emit_dm_src(a, &offs(), &op4, 0, std::ptr::null(), 0));
    let mut want = vec![0x8bu8, 0x83];
    want.extend_from_slice(&o.ram_read.to_le_bytes());
    assert_eq!(a.code, want, ":1453 ram_read leg");
    // rnd-noise leg (:1456-1458) = rnd ++ shl32 8 ++ sar32 8
    let op5 = Op { dm_src: 5, dm: 1, ..Op::ZERO };
    let a = mk(|a| emit_dm_src(a, &offs(), &op5, 0, std::ptr::null(), 0));
    let mut ar = Assembler::new();
    emit_rnd(&mut ar);
    let mut want = ar.code.clone();
    want.extend_from_slice(&[0xc1, 0xe0, 0x08, 0xc1, 0xf8, 0x08]);
    assert_eq!(a.code, want, ":1456-1458 noise leg");
    // p_packed leg (:1461, no_noise=1 ⇒ noise=false)
    let op6 = Op { dm_src: 6, no_noise: 1, dm: 1, ..Op::ZERO };
    let a = mk(|a| emit_dm_src(a, &offs(), &op6, 0, std::ptr::null(), 0));
    let mut ap = Assembler::new();
    emit_p_packed(&mut ap, false);
    assert_eq!(a.code, ap.code, ":1461 p_packed(!no_noise) leg");
    // m[sm] leg (:1464, the `default` arm) — disp32 ⇒ length-independent
    // of sm (B2b-3 jump-patch stability)
    let op7 = Op { dm_src: 7, sm: 7, dm: 1, ..Op::ZERO };
    let a = mk(|a| emit_dm_src(a, &offs(), &op7, 0, std::ptr::null(), 0));
    let mut want = vec![0x8bu8, 0x83];
    want.extend_from_slice(&(o.m + 4 * 7).to_le_bytes());
    assert_eq!(a.code, want, ":1464 m[sm] leg");
    let op7b = Op { dm_src: 7, sm: 63, dm: 1, ..Op::ZERO };
    assert_eq!(
        mk(|a| emit_dm_src(a, &offs(), &op7b, 0, std::ptr::null(), 0)).code.len(),
        want.len(),
        "disp32 rm() ⇒ sm cannot change leg length"
    );
    // ---- B2b-3a: the two un-hoisted 0-3 arms now dispatch (no more
    // `unimplemented!`). Resident sintab ⇒ in-place emit_lfo (:1421); null
    // sintab ⇒ the callout (:1431-1448). Each byte-equals the direct helper.
    let want_lfo = mk(|a| emit_lfo(a, &offs(), 3, fake_sintab));
    let a = mk(|a| emit_dm_src(a, &o, &op0, 0, fake_sintab, 0x8000));
    assert_eq!(a.code, want_lfo.code, ":1421 un-hoisted + sintab ⇒ in-place emit_lfo");
    let want_call = mk(|a| emit_call_lfo(a, 3, std::ptr::null(), 0));
    let a = mk(|a| emit_dm_src(a, &o, &op0, 0, std::ptr::null(), 0));
    assert_eq!(a.code, want_call.code, ":1431-1448 no-sintab ⇒ emit_call_lfo callout");

    // ---- dm store (:1467-1475) ----
    // early + last slot (:1468 + :1470): ops[0x10].dm=5 is the only m
    // writer of class k%3==1 ⇒ both flags true
    let mut ops = zero_ops();
    ops[0x10].dm = 5;
    let an = analyze_ops(&ops, true);
    assert!(an.early_m[5] && an.last_slot_m[0x10], "precondition");
    let s3 = slot3(0, 0x10); // = 1
    let a = mk(|a| emit_dm_store(a, &offs(), 0x10, &ops[0x10], s3, &analyze_ops(&ops, true)));
    let mut want = vec![0x89u8, 0x83];
    want.extend_from_slice(&(o.m + 4 * 5).to_le_bytes()); // store m[5] (:1468)
    want.extend_from_slice(&[0x89, 0x83]);
    want.extend_from_slice(&(o.mw_value + 4 * s3 as i32).to_le_bytes()); // + ring slot (:1470)
    assert_eq!(a.code, want, "early+last: direct store THEN slot");
    // early, NOT last (a later class-1 dm steals the slot) ⇒ direct only
    ops[0x13].dm = 5;
    let an = analyze_ops(&ops, true);
    assert!(an.early_m[5] && !an.last_slot_m[0x10] && an.last_slot_m[0x13]);
    let a = mk(|a| emit_dm_store(a, &offs(), 0x10, &ops[0x10], s3, &analyze_ops(&ops, true)));
    assert_eq!(a.code.len(), 6, "early non-last: direct store only (:1468)");
    // early OFF ⇒ ring-slot store only (:1472), reg byte still absent
    let an_off = analyze_ops(&ops, false);
    assert!(!an_off.early_m[5]);
    let a = mk(|a| emit_dm_store(a, &offs(), 0x10, &ops[0x10], s3, &analyze_ops(&ops, false)));
    let mut want = vec![0x89u8, 0x83];
    want.extend_from_slice(&(o.mw_value + 4 * s3 as i32).to_le_bytes());
    assert_eq!(a.code, want, "ring-slot store (:1472)");
    // tail trio k=0x17d: ring store + the unconditional reg byte
    // (:1472-1475). The analysis already refuses early for the trio
    // (:699-702) — and the :1467 `k < 0x17d` conjunct refuses at emit
    // too; the shape below pins the EMIT-side gate.
    let mut ops2 = zero_ops();
    ops2[0x17d].dm = 6;
    let an2 = analyze_ops(&ops2, true);
    assert!(!an2.early_m[6], "tail trio is never an early candidate (:699-702)");
    let s3d = slot3(0, 0x17d); // 381%3 = 0
    let a = mk(|a| emit_dm_store(a, &offs(), 0x17d, &ops2[0x17d], s3d, &analyze_ops(&ops2, true)));
    let mut want = vec![0x89u8, 0x83];
    want.extend_from_slice(&(o.mw_value + 4 * s3d as i32).to_le_bytes());
    want.push(0xc6);
    want.push(0x83);
    want.extend_from_slice(&(o.mw_reg + s3d as i32).to_le_bytes());
    want.push(6); // imm8 = o.dm
    assert_eq!(a.code, want, "tail: slot store + store8i mw_reg byte (:1472-1475)");
    // branchy + dm==0: the byte leg fires OUTSIDE the dm gate and CLEARS
    // the slot (:1474, skipped-op eraser contract) — and nothing else
    let mut ops3 = zero_ops();
    ops3[0x20].jump = 1;
    let an3 = analyze_ops(&ops3, true);
    assert!(an3.branchy);
    let s3b = slot3(0, 5); // = 2
    let a = mk(|a| emit_dm_store(a, &offs(), 5, &Op::ZERO, s3b, &analyze_ops(&ops3, true)));
    let mut want = vec![0xc6u8, 0x83];
    want.extend_from_slice(&(o.mw_reg + s3b as i32).to_le_bytes());
    want.push(0);
    assert_eq!(a.code, want, "branchy dm==0: zero-clear byte only (:1474)");

    // ---- dr apply (:1480-1496) ----
    // rand_n head (:1481-1482) + r[sr] select (:1485) + early/last stores
    let mut ops4 = zero_ops();
    ops4[0x10].dr = 9;
    ops4[0x10].dr_from_r = 1;
    ops4[0x10].sr = 4;
    ops4[0x10].rand_n = 3;
    let an4 = analyze_ops(&ops4, true);
    assert!(an4.early_r[9] && an4.last_slot_r[0x10]);
    let a = mk(|a| emit_dr_apply(a, &offs(), 0x10, &ops4[0x10], s3, &analyze_ops(&ops4, true)));
    let mut ar = Assembler::new();
    emit_rnd_skip(&mut ar, 3);
    let mut want = ar.code.clone(); // rnd_skip FIRST (:1481-1482)
    want.extend_from_slice(&[0x8b, 0x83]);
    want.extend_from_slice(&(o.r + 4 * 4).to_le_bytes()); // mov eax,[r+4sr] (:1485)
    want.extend_from_slice(&[0x89, 0x83]);
    want.extend_from_slice(&(o.r + 4 * 9).to_le_bytes()); // mov [r+4dr],eax (:1489)
    want.extend_from_slice(&[0x89, 0x83]);
    want.extend_from_slice(&(o.rw_value + 4 * s3 as i32).to_le_bytes()); // last slot (:1491)
    assert_eq!(a.code, want, "dr: rnd_skip → r[sr] → early+last stores");
    // no rand_n + no_noise p-leg (:1487) + early OFF ring store (:1493)
    let mut ops5 = zero_ops();
    ops5[0x10].dr = 9;
    ops5[0x10].no_noise = 1;
    let a = mk(|a| emit_dr_apply(a, &offs(), 0x10, &ops5[0x10], s3, &analyze_ops(&ops5, false)));
    let mut want = vec![0x49u8, 0x8b, 0xc5]; // p_packed(noise=false) head (:1487/:880)
    want.extend_from_slice(&{
        let mut ap = Assembler::new();
        emit_pack24(&mut ap);
        ap.code
    });
    want.extend_from_slice(&[0x89, 0x83]);
    want.extend_from_slice(&(o.rw_value + 4 * s3 as i32).to_le_bytes());
    assert_eq!(a.code, want, "dr: p_packed !noise + ring store, no rnd_skip");
    // tail: rw_reg byte (:1495-1496) rides after the slot store
    let mut ops6 = zero_ops();
    ops6[0x17e].dr = 10;
    ops6[0x17e].dr_from_r = 1;
    ops6[0x17e].sr = 4;
    let s3e = slot3(0, 0x17e); // 382%3 = 1
    let a = mk(|a| emit_dr_apply(a, &offs(), 0x17e, &ops6[0x17e], s3e, &analyze_ops(&ops6, true)));
    let mut tail = Vec::new();
    tail.push(0xc6);
    tail.push(0x83);
    tail.extend_from_slice(&(o.rw_reg + s3e as i32).to_le_bytes());
    tail.push(10);
    assert_eq!(&a.code[a.code.len() - tail.len()..], tail.as_slice(), "rw_reg tail byte (:1495)");
    let opsz = a.code.len();
    let mut ops7 = zero_ops();
    ops7[0x17e].dr = 10;
    ops7[0x17e].dr_from_r = 1;
    ops7[0x17e].sr = 0x7f; // max r index — disp32 ⇒ same length
    assert_eq!(
        mk(|a| emit_dr_apply(a, &offs(), 0x17e, &ops7[0x17e], s3e, &analyze_ops(&ops7, true))).code.len(),
        opsz,
        "dr legs are disp32-fixed-length"
    );
    println!("meg_jit_b2b2b shape: dm src/store + dr apply legs compile-time gated, lengths fixed");
}

// ---------------------------------------------------------------------------
// B2b-2c — memw + ix2 index leg byte/shape pins (origin: swp30_jit.cpp:883-889
// AccFromP/ShrAcc/ShrAccTZ15 lambdas, :1501-1509 memw, :1511-1525 index;
// brief §3). Pure byte-assembly checks — platform-free; the EXEC proofs live in
// the b2a mod (same split as B2b-2a/2b).
// ---------------------------------------------------------------------------

/// Hand-derived encodings for the B2b-2c accumulator lambdas (rr()/rm()
/// rules, jit_emit.rs:92-138): AccFromP (`mov rax,r13` = the p_packed :880
/// head), ShrAcc (`sar rax,imm8`), ShrAccTZ15 (byte-identical to the first
/// five pack24 instructions — the same round-toward-zero leg), and the
/// memw/index leg shapes with the §7-B ix2 window deref (SIB base R12 with
/// REX 0x49, `49 8b e4 24`, then the store through the loaded slot pointer).
#[test]
fn b2b2c_acc_memw_index_byte_pins() {
    let o = offs();
    fn mk(f: impl FnOnce(&mut Assembler)) -> Vec<u8> {
        let mut a = Assembler::new();
        f(&mut a);
        a.code
    }

    // AccFromP (:884) = mov rax,r13 ⇒ REX 0x49 (B for r13, W not set on the
    // 8b move: reg 0 base 13 ⇒ 0x40|(13>>3)&1=0x41? no — W=true here) —
    // rr(0,W,{8b},RAX,R13): rex=0x40|8|0|1=0x49; ModRM C0|(0<<3)|5=C5.
    assert_eq!(mk(|a| emit_acc_from_p(a)), [0x49, 0x8b, 0xc5], "AccFromP (:884)");
    // ShrAcc(15+8) (:885/1514) = sar rax,23 ⇒ 48 C1 F8 17 (shift-group /7 ⇒
    // reg field 7 ⇒ ModRM C0|(7<<3)|0=F8 — NOT the /4 shl form E0).
    assert_eq!(mk(|a| emit_shr_acc(a, 15 + 8)), [0x48, 0xc1, 0xf8, 0x17], "ShrAcc 23 (:885)");
    // ShrAccTZ15 (:886-889) is EXACTLY pack24's first five instructions
    // (:845-849 — the same round-toward-zero bias+shift leg).
    let mut ap = Assembler::new();
    emit_pack24(&mut ap);
    let tz = mk(|a| emit_shr_acc_tz15(a));
    assert_eq!(tz, ap.code[..20].to_vec(), "ShrAccTZ15 == pack24 first five (:845-849)");
    assert_eq!(
        tz,
        [
            0x48, 0x8b, 0xc8, // mov rcx,rax (:887)
            0x48, 0xc1, 0xf9, 0x3f, // sar rcx,63 (:887)
            0x81, 0xe1, 0xff, 0x7f, 0x00, 0x00, // and ecx,0x7fff (:887)
            0x48, 0x01, 0xc8, // add rax,rcx (:888)
            0x48, 0xc1, 0xf8, 0x0f, // sar rax,15 (:888)
        ],
        "ShrAccTZ15 bytes"
    );

    // ---- memw leg (:1501-1509) ----
    // branchy memw=1 ⇒ both the value (AccFromP++ShrAccTZ15++store32) AND the
    // 2-ring act byte (:1508-1509) fire; the act threshold is 0x17e (2-ring),
    // so also prove the mid-k NON-branchy op drops the act byte.
    let mw = Op { memw: 1, ..Op::ZERO };
    let mut ops = zero_ops();
    ops[0x20].jump = 1;
    ops[0x20].target = 0x10;
    let anb = analyze_ops(&ops, true); // branchy
    let s2 = slot2(0, 0x10); // 16 % 2 = 0
    let want: Vec<u8> = {
        let mut w = vec![0x49u8, 0x8b, 0xc5]; // AccFromP
        w.extend_from_slice(&tz); // ShrAccTZ15
        w.extend_from_slice(&[0x89, 0x83]); // store32 memw_val+4*s2
        w.extend_from_slice(&(o.memw_val + 4 * s2 as i32).to_le_bytes());
        w.extend_from_slice(&[0xc6, 0x83]); // store8i memw_act+s2, imm 1
        w.extend_from_slice(&(o.memw_act + s2 as i32).to_le_bytes());
        w.push(1);
        w
    };
    assert_eq!(mk(|a| emit_memw_acc(a, &o, 0x10, &mw, s2, &anb)), want, "memw branchy value+act (:1501-1509)");
    // memw=0 branchy ⇒ ZERO-clear the act byte only (outside the value gate)
    let want0: Vec<u8> = {
        let mut w = vec![0xc6u8, 0x83];
        w.extend_from_slice(&(o.memw_act + s2 as i32).to_le_bytes());
        w.push(0);
        w
    };
    assert_eq!(mk(|a| emit_memw_acc(a, &o, 0x10, &Op::ZERO, s2, &anb)), want0, "memw=0 branchy act-clear (:1509)");
    // non-branchy mid-k ⇒ value leg ONLY, no act byte (:1508 gate)
    let ann = analyze_ops(&zero_ops(), true); // non-branchy
    let want_nona: Vec<u8> = {
        let mut w = vec![0x49u8, 0x8b, 0xc5];
        w.extend_from_slice(&tz);
        w.extend_from_slice(&[0x89, 0x83]);
        w.extend_from_slice(&(o.memw_val + 4 * s2 as i32).to_le_bytes());
        w
    };
    assert_eq!(mk(|a| emit_memw_acc(a, &o, 0x10, &mw, s2, &ann)), want_nona, "memw mid-k no act byte (:1508)");
    // the 2-ring tail threshold: k=0x17e fires the act byte, 0x17d does not
    let s2t = slot2(0, 0x17e);
    let tail_fire = mk(|a| emit_memw_acc(a, &o, 0x17e, &Op::ZERO, s2t, &ann));
    assert_eq!(&tail_fire[..2], &[0xc6, 0x83], "k=0x17e fires the 2-ring act byte (:1508)");
    let no_fire = mk(|a| emit_memw_acc(a, &o, 0x17d, &Op::ZERO, slot2(0, 0x17d), &ann));
    assert!(no_fire.is_empty(), "k=0x17d is NOT at the memw 2-ring tail (:1508)");

    // ---- index legs (:1511-1525) — full branchy index+index2 shape ----
    let ix = Op { index: 1, index2: 1, ..Op::ZERO };
    let s3 = slot3(0, 0x10); // 16 % 3 = 1
    let shr23 = [0x48u8, 0xc1, 0xf8, 0x17]; // sar rax,23 (/7 form)
    let want: Vec<u8> = {
        let mut w = vec![0x49u8, 0x8b, 0xc5]; // AccFromP (:1513)
        w.extend_from_slice(&shr23); // ShrAcc 23 (:1514)
        w.extend_from_slice(&[0x89, 0x83]); // store32 ix_value+4*s3 (:1515)
        w.extend_from_slice(&(o.ix_value + 4 * s3 as i32).to_le_bytes());
        w.extend_from_slice(&[0xc6, 0x83]); // store8i ix_act+s3, imm 1 (:1518)
        w.extend_from_slice(&(o.ix_act + s3 as i32).to_le_bytes());
        w.push(1);
        w.extend_from_slice(&[0x49, 0x8b, 0xc5]); // AccFromP (:1520)
        w.extend_from_slice(&shr23); // ShrAcc 23 (:1521)
        w.extend_from_slice(&[0x49, 0x8b, 0x94, 0x24]); // load64 rdx,[r12+o.ix2_value] (ModRM 80|(RDX<<3)|4=94)
        w.extend_from_slice(&o.ix2_value.to_le_bytes());
        w.extend_from_slice(&[0x89, 0x82]); // store32 [rdx+4*s3] (:1522 window)
        w.extend_from_slice(&(4 * s3 as i32).to_le_bytes());
        w.extend_from_slice(&[0x49, 0x8b, 0x94, 0x24]); // load64 rdx,[r12+o.ix2_act]
        w.extend_from_slice(&o.ix2_act.to_le_bytes());
        w.extend_from_slice(&[0xc6, 0x82]); // store8i [rdx+s3], imm 1 (:1525 window)
        w.extend_from_slice(&(s3 as i32).to_le_bytes());
        w.push(1);
        w
    };
    assert_eq!(mk(|a| emit_index_legs(a, &o, 0x10, &ix, s3, &anb)), want, "index branchy full (:1511-1525)");
    // mid-k non-branchy with index2 only ⇒ the window-deref value leg only,
    // both act bytes suppressed (:1517/:1524 gates) — fixed length across s3.
    let ix2only = Op { index2: 1, ..Op::ZERO };
    let l1 = mk(|a| emit_index_legs(a, &o, 0x10, &ix2only, slot3(0, 0x10), &ann)).len();
    let l2 = mk(|a| emit_index_legs(a, &o, 0x10, &ix2only, slot3(0, 0x11), &ann)).len();
    assert_eq!(l1, l2, "disp32 rm() ⇒ slot index cannot change ix2 leg length");
    let ix2_leg = mk(|a| emit_index_legs(a, &o, 0x10, &ix2only, s3, &ann));
    let want_ix2: Vec<u8> = {
        let mut w = vec![0x49u8, 0x8b, 0xc5]; // AccFromP (:1520)
        w.extend_from_slice(&shr23); // ShrAcc 23 (:1521)
        w.extend_from_slice(&[0x49, 0x8b, 0x94, 0x24]); // load64 window slot (RDX reg)
        w.extend_from_slice(&o.ix2_value.to_le_bytes());
        w.extend_from_slice(&[0x89, 0x82]); // store [rdx+4*s3] (:1522)
        w.extend_from_slice(&(4 * s3 as i32).to_le_bytes());
        w
    };
    assert_eq!(ix2_leg, want_ix2, "ix2 mid-k non-branchy: window value leg only");
    println!("meg_jit_b2b2c byte pins: AccFromP/ShrAcc/ShrAccTZ15 + memw/index legs + ix2 window deref");
}

// ---------------------------------------------------------------------------
// B2b-2d — t/tval leg byte/shape pins (origin: swp30_jit.cpp:1530-1561
// folded t, :1552-1557 ±0x8000 clamp pair, :1647-1675 branchy erase-twin;
// brief §3). Pure byte-assembly checks — platform-free; the EXEC proofs live
// in the b2a mod (same split as B2b-2a/2b/2c).
// ---------------------------------------------------------------------------

/// Hand-derived encodings (rr()/rm() rules, jit_emit.rs:92-138) for the
/// B2b-2d t-leg shapes: the ±0x8000 clamp pair (`48 b9` movabs immediates +
/// strict `48 39 c8` cmp / `48 0f 4c c1` cmovl / `48 0f 4f c1` cmovg), the
/// folded index form (`48 c1 e8 08` + `81 e0 ff 7f 00 00`), the t-write
/// arm select (:1533 slot load / :1535 baked imm32 / :1537 runtime konst —
/// each 16-bit `0f b7` load, `66 89` store, disp32-fixed).
#[test]
fn b2b2d_t_leg_byte_pins() {
    let o = offs();
    fn mk(f: impl FnOnce(&mut Assembler)) -> Vec<u8> {
        let mut a = Assembler::new();
        f(&mut a);
        a.code
    }

    // clamp pair (:1552-1557) — the TWO movabs immediates are the pin:
    // -0x8000 sign-extended full 64-bit, and 0x7fff (NOT ±0x7fff symmetric).
    assert_eq!(
        mk(|a| emit_tval_clamp(a)),
        [
            0x48, 0xb9, 0x00, 0x80, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // movabs rcx,-0x8000 (:1552)
            0x48, 0x39, 0xc8, // cmp rax,rcx (:1553)
            0x48, 0x0f, 0x4c, 0xc1, // cmovl rax,rcx (:1554)
            0x48, 0xb9, 0xff, 0x7f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // movabs rcx,0x7fff (:1555)
            0x48, 0x39, 0xc8, // cmp rax,rcx (:1556)
            0x48, 0x0f, 0x4f, 0xc1, // cmovg rax,rcx (:1557)
        ],
        "emit_tval_clamp bytes"
    );

    // k=0x20 publishes (ops[0x22] t_write+t_from_p forces need_tval via the
    // :664 k+2 rule); k=0x21 does NOT — both gates exercised below.
    let k = 0x20u32;
    let s2 = slot2(0, k); // 32 % 2 = 0
    let tvd = (o.t_value + 2 * s2 as i32).to_le_bytes();
    let td = (o.t + 2 * 3).to_le_bytes(); // o.t = 3
    let mut s = Swp30::new();
    s.meg.konst[k as usize] = -2; // exercised by the bake arm below
    let ms = &s.meg;
    let mut ops = zero_ops();
    ops[0x22].t_write = 1;
    ops[0x22].t_from_p = 1;
    let an = analyze_ops(&ops, true);
    assert!(an.need_tval[k as usize] && !an.need_tval[0x21] && !an.branchy, "preconditions");

    // full folded index leg: slot load BEFORE the publish (:1533/:1560 ride
    // the SAME disp) → t[3] store → AccFromP → ShrAcc(8) → and 0x7fff →
    // publish slot.
    let op_ix = Op { t_write: 1, t_from_p: 1, t: 3, index: 1, ..Op::ZERO };
    let mut want: Vec<u8> = vec![0x0f, 0xb7, 0x83]; // movzx eax,[rbx+t_value+2*s2] (:1533)
    want.extend_from_slice(&tvd);
    want.extend_from_slice(&[0x66, 0x89, 0x83]); // mov [rbx+t+6],ax (:1538)
    want.extend_from_slice(&td);
    want.extend_from_slice(&[0x49, 0x8b, 0xc5]); // AccFromP (:1541)
    want.extend_from_slice(&[0x48, 0xc1, 0xf8, 0x08]); // sar rax,8 (:1543, shift group /7)
    want.extend_from_slice(&[0x81, 0xe0, 0xff, 0x7f, 0x00, 0x00]); // and eax,0x7fff (:1544)
    want.extend_from_slice(&[0x66, 0x89, 0x83]); // mov [rbx+t_value+2*s2],ax (:1560)
    want.extend_from_slice(&tvd);
    assert_eq!(mk(|a| emit_t_leg(a, &o, ms, k, &op_ix, s2, false, &an)), want, "folded index leg (:1530-1561)");

    // clamp leg (no index flags): ShrAcc(15+8) + the pair instead (:1546-1557)
    let op_ni = Op { t_write: 1, t_from_p: 1, t: 3, ..Op::ZERO };
    let mut want2: Vec<u8> = vec![0x0f, 0xb7, 0x83];
    want2.extend_from_slice(&tvd);
    want2.extend_from_slice(&[0x66, 0x89, 0x83]);
    want2.extend_from_slice(&td);
    want2.extend_from_slice(&[0x49, 0x8b, 0xc5]);
    want2.extend_from_slice(&[0x48, 0xc1, 0xf8, 0x17]); // sar rax,23 (:1546)
    want2.extend_from_slice(&mk(|a| emit_tval_clamp(a)));
    want2.extend_from_slice(&[0x66, 0x89, 0x83]);
    want2.extend_from_slice(&tvd);
    assert_eq!(mk(|a| emit_t_leg(a, &o, ms, k, &op_ni, s2, false, &an)), want2, "folded clamp leg (:1545-1560)");

    // t-write arm select (:1532-1537): runtime konst load vs the BAKED imm32.
    // Across compile variants the size may differ (imm32 5 B vs movzx+disp32
    // 7 B — bake is fixed per code version); WITHIN one compilation the leg
    // is length-stable (disp32 rm(), t index cannot shift it) — the jump
    // patch stability B2b-3 relies on. k=0x20 also publishes (need_tval),
    // so both arms carry the SAME clamp tail — only the value source differs.
    let op_rt = Op { t_write: 1, t_from_p: 0, t: 3, ..Op::ZERO };
    let kd = (o.konst + 2 * k as i32).to_le_bytes();
    let mut pubtail: Vec<u8> = vec![0x49, 0x8b, 0xc5]; // AccFromP (:1541)
    pubtail.extend_from_slice(&[0x48, 0xc1, 0xf8, 0x17]); // sar rax,23 (:1546)
    pubtail.extend_from_slice(&mk(|a| emit_tval_clamp(a))); // :1552-1557
    pubtail.extend_from_slice(&[0x66, 0x89, 0x83]); // mov [rbx+t_value+2*s2],ax (:1560)
    pubtail.extend_from_slice(&tvd);
    let mut want_rt: Vec<u8> = vec![0x0f, 0xb7, 0x83]; // movzx eax,[rbx+konst+2k] (:1537)
    want_rt.extend_from_slice(&kd);
    want_rt.extend_from_slice(&[0x66, 0x89, 0x83]);
    want_rt.extend_from_slice(&td);
    want_rt.extend_from_slice(&pubtail);
    let bake_leg = mk(|a| emit_t_leg(a, &o, ms, k, &op_rt, s2, false, &an));
    assert_eq!(bake_leg, want_rt, "runtime konst arm (:1537)");
    let mut want_bk: Vec<u8> = vec![0xb8]; // mov eax, u16(konst[k]) — RAX<8 ⇒ no REX (:1535)
    want_bk.extend_from_slice(&((-2i16) as u16 as u32).to_le_bytes());
    want_bk.extend_from_slice(&[0x66, 0x89, 0x83]);
    want_bk.extend_from_slice(&td);
    want_bk.extend_from_slice(&pubtail);
    let baked = mk(|a| emit_t_leg(a, &o, ms, k, &op_rt, s2, true, &an));
    assert_eq!(baked, want_bk, "baked konst imm32 arm (:1535)");
    // in-compile length stability: the t register index cannot shift the
    // leg (disp32 rm() everywhere) — B2b-3 jump-patch stable
    let op_t7 = Op { t_write: 1, t_from_p: 0, t: 7, ..Op::ZERO };
    assert_eq!(
        mk(|a| emit_t_leg(a, &o, ms, k, &op_t7, s2, false, &an)).len(),
        want_rt.len(),
        "disp32 rm() ⇒ t index cannot change leg length"
    );

    // need_tval=false (k=0x21): write-only leg, ZERO publish bytes.
    let s2_21 = slot2(0, 0x21); // = 1
    let tvd21 = (o.t_value + 2 * s2_21 as i32).to_le_bytes();
    let mut want_np: Vec<u8> = vec![0x0f, 0xb7, 0x83];
    want_np.extend_from_slice(&tvd21);
    want_np.extend_from_slice(&[0x66, 0x89, 0x83]);
    want_np.extend_from_slice(&td);
    assert_eq!(mk(|a| emit_t_leg(a, &o, ms, 0x21, &op_ix, s2_21, false, &an)), want_np, "no publish when !need_tval (:1540 gate)");
    println!("meg_jit_b2b2d byte pins: clamp pair + folded index/clamp legs + t-write arms + need_tval gate");
}

/// Branchy twin (:1647-1675) shape: the jump t-write arm (NO bake arm —
/// :1652 always runtime konst), the five ring-byte erasers with the ix2 byte
/// through the §7-B window (`49 8b 94 24` slot load + `c6 82` store), and
/// the ALWAYS-clamp publish (:1662 no index branch). t_write with
/// jump==0 ⇒ the leg is erasers-only (:1647 conjunct).
#[test]
fn b2b2d_branchy_byte_pins() {
    let o = offs();
    fn mk(f: impl FnOnce(&mut Assembler)) -> Vec<u8> {
        let mut a = Assembler::new();
        f(&mut a);
        a.code
    }
    let k = 0x20u32;
    let s2 = slot2(0, k); // = 0
    let s3 = slot3(0, k); // 32 % 3 = 2
    // (no MegState needed: the branchy konst arm is the RUNTIME load — the
    // bake variant lives in the folded test; only the disp is baked here)
    let mut ops = zero_ops();
    ops[k as usize].jump = 1;
    ops[k as usize].target = 0x10;
    ops[0x22].t_write = 1; // :664 k+2 rule ⇒ need_tval[k] (not the tail at 0x20)
    ops[0x22].t_from_p = 1;
    let an = analyze_ops(&ops, true);
    assert!(an.branchy && an.need_tval[k as usize], "k=0x20: branchy + k+2-forced publish");

    let opj = Op { jump: 1, t_write: 1, t_from_p: 1, t: 3, index: 1, ..Op::ZERO };
    let tvd = (o.t_value + 2 * s2 as i32).to_le_bytes();
    let kd = (o.konst + 2 * k as i32).to_le_bytes();
    let clamp = mk(|a| emit_tval_clamp(a));
    let mut want: Vec<u8> = vec![0x0f, 0xb7, 0x83]; // movzx eax,[rbx+t_value] (:1650)
    want.extend_from_slice(&tvd);
    want.extend_from_slice(&[0x66, 0x89, 0x83]);
    want.extend_from_slice(&(o.t + 6).to_le_bytes()); // mov [rbx+t+6],ax (:1653)
    for (base, disp) in [
        (o.mw_reg + s3 as i32, 0u8), // :1655
        (o.rw_reg + s3 as i32, 0),   // :1656
        (o.memw_act + s2 as i32, 0), // :1657
        (o.ix_act + s3 as i32, 0),   // :1658
    ] {
        want.extend_from_slice(&[0xc6, 0x83]); // mov byte [rbx+disp],0
        want.extend_from_slice(&base.to_le_bytes());
        want.push(disp);
    }
    want.extend_from_slice(&[0x49, 0x8b, 0x94, 0x24]); // load64 rdx,[r12+o.ix2_act] — window (:1659)
    want.extend_from_slice(&o.ix2_act.to_le_bytes());
    want.extend_from_slice(&[0xc6, 0x82]); // mov byte [rdx+s3],0 (:1659 window store)
    want.extend_from_slice(&(s3 as i32).to_le_bytes());
    want.push(0);
    want.extend_from_slice(&[0x49, 0x8b, 0xc5]); // AccFromP (:1661)
    want.extend_from_slice(&[0x48, 0xc1, 0xf8, 0x17]); // sar rax,23 (:1662 — ALWAYS, no index branch)
    want.extend_from_slice(&clamp);
    want.extend_from_slice(&[0x66, 0x89, 0x83]); // mov [rbx+t_value+2*s2],ax (:1674)
    want.extend_from_slice(&tvd);
    assert_eq!(mk(|a| emit_t_branchy(a, &o, k, &opj, s2, s3, &an)), want, "branchy twin full (:1647-1675)");

    // t_from_p=0 ⇒ runtime konst load, NEVER the baked imm (:1651-1652):
    // the first three bytes are the 0f b7 load, not b8 (imm32).
    let opj_rt = Op { jump: 1, t_write: 1, t_from_p: 0, t: 3, ..Op::ZERO };
    let leg = mk(|a| emit_t_branchy(a, &o, k, &opj_rt, s2, s3, &an));
    assert_eq!(&leg[..3], &[0x0f, 0xb7, 0x83], "const arm stays runtime (:1652)");
    assert_eq!(&leg[3..7], &kd, "runtime konst disp (:1652)");

    // t_write with jump==0 ⇒ the :1647 conjunct keeps the leg erasers-only
    // plus publish: 4 MS erasers (7 B each) + window deref (load64 8 B +
    // store8i 7 B) + AccFromP+Sar23 (7 B) + clamp (34 B) + store16 (7 B).
    let opnt = Op { t_write: 1, t_from_p: 1, t: 3, ..Op::ZERO };
    let e = mk(|a| emit_t_branchy(a, &o, k, &opnt, s2, s3, &an));
    assert_eq!(e.len(), 4 * 7 + 8 + 7 + 7 + clamp.len() + 7, "no jump ⇒ no t-write arm (:1647)");
    assert_eq!(&e[..2], &[0xc6, 0x83], "leg opens with the first eraser (:1655)");
    println!("meg_jit_b2b2d branchy pins: jump t-arm + 5 erasers (ix2 window) + always-clamp publish");
}

// ---------------------------------------------------------------------------
// B2b-2e — memop address leg byte pins (swp30_jit.cpp:1566-1638). Pure
// byte assembly — platform-free like the B2b-2c/2d pins; the exec rigs
// live in mod b2a below.
// ---------------------------------------------------------------------------

#[test]
fn b2b2e_memop_addr_byte_pins() {
    let o = offs();
    fn mk(f: impl FnOnce(&mut Assembler)) -> Vec<u8> {
        let mut a = Assembler::new();
        f(&mut a);
        a.code
    }
    let an = analyze_ops(&zero_ops(), true);
    assert!(!an.branchy);

    // base leg (:1570-1578): offset word + ix + §7-B window deref of ix2.
    let op3 = Op {
        memop: 3,
        offset_index: 7,
        mem_use_index: 1,
        mem_use_index2: 1,
        addr_mask: 0x1fff,
        addr_base: 0x400,
        region: 2,
        ..Op::ZERO
    };
    let base = mk(|a| emit_memop_addr_base(a, &o, &op3));
    let mut want: Vec<u8> = vec![0x0f, 0xb7, 0x83]; // movzx eax,[rbx+off+14] (:1570)
    want.extend_from_slice(&(o.offset + 14).to_le_bytes());
    want.extend_from_slice(&[0x8b, 0x8b]); // mov ecx,[rbx+ram_index] (:1572)
    want.extend_from_slice(&o.ram_index.to_le_bytes());
    want.extend_from_slice(&[0x01, 0xc8]); // add eax,ecx (:1573)
    want.extend_from_slice(&[0x49, 0x8b, 0x94, 0x24]); // mov rdx,[r12+ix2slot] (:1576 window)
    want.extend_from_slice(&o.ram_index2.to_le_bytes());
    want.extend_from_slice(&[0x8b, 0x8a, 0, 0, 0, 0]); // mov ecx,[rdx] — rm() is ALWAYS disp32 (:1576 deref)
    want.extend_from_slice(&[0x01, 0xc8]); // add eax,ecx (:1577)
    assert_eq!(base, want, "base sum (:1570-1578)");
    // index arms are compile-time gated (:1571/:1575): bare op = offset load only
    let op0 = Op { memop: 2, offset_index: 0, ..Op::ZERO };
    assert_eq!(mk(|a| emit_memop_addr_base(a, &o, &op0)).len(), 7, "gated index arms (:1571/:1575)");

    // full addr (:1593-1612) memop=3: base + −SC + 1 + baked mask/base + 18-bit
    let ad3 = mk(|a| emit_memop_addr(a, &o, &op3));
    let mut want3 = base.clone();
    want3.extend_from_slice(&[0x44, 0x29, 0xf0]); // sub eax,r14d (:1606 x64 arm — NOT the dead FM spill)
    want3.extend_from_slice(&[0x81, 0xc0, 0x01, 0x00, 0x00, 0x00]); // add eax,1 (:1609)
    want3.extend_from_slice(&[0x81, 0xe0]); // and eax,addr_mask (:1610 — BAKED imm32)
    want3.extend_from_slice(&op3.addr_mask.to_le_bytes());
    want3.extend_from_slice(&[0x81, 0xc0]); // add eax,addr_base (:1611 — BAKED imm32)
    want3.extend_from_slice(&op3.addr_base.to_le_bytes());
    want3.extend_from_slice(&[0x81, 0xe0, 0xff, 0xff, 0x03, 0x00]); // and eax,0x3ffff (:1612)
    assert_eq!(ad3, want3, "region addr math, baked map legs (:1593-1612)");

    // memop=2 skips the +1 arm (:1608 gate); everything else identical
    let mut op2 = op3;
    op2.memop = 2;
    let ad2 = mk(|a| emit_memop_addr(a, &o, &op2));
    assert_eq!(ad2.len(), ad3.len() - 6, "no +1 arm for memop!=3 (:1608)");
    let mut want2 = base.clone();
    want2.extend_from_slice(&[0x44, 0x29, 0xf0]); // −SC straight from the base sum (:1606)
    want2.extend_from_slice(&[0x81, 0xe0]); // and mask (:1610)
    want2.extend_from_slice(&op2.addr_mask.to_le_bytes());
    want2.extend_from_slice(&[0x81, 0xc0]);
    want2.extend_from_slice(&op2.addr_base.to_le_bytes());
    want2.extend_from_slice(&[0x81, 0xe0, 0xff, 0xff, 0x03, 0x00]);
    assert_eq!(ad2, want2, "memop=2 addr shape (:1593-1612 without :1609)");

    // enabled WRITE leg (:1613-1626): addr + R8 parking + ram_write + PAIRED
    // encode + scale-2 store. R8 survives the encode (RENC=R11) — and the
    // 0x47 SIB below has index bits = R8&7 (0) with X=1, i.e. the store
    // really is [r15+r8*2] (:1626), NOT [r15+rax*2].
    let enc = mk(emit_revram_encode);
    let w = Op { memop: 1, offset_index: 3, addr_mask: 0x3fff, addr_base: 0x2000, region: 1, ..Op::ZERO };
    let wr = mk(|a| emit_memop(a, &o, 0x40, &w, 1, 0, &an));
    let mut wantw = mk(|a| emit_memop_addr(a, &o, &w));
    wantw.extend_from_slice(&[0x4c, 0x8b, 0xc0]); // mov r8,rax (:1623 — REX.W+R: dest in REG field)
    wantw.extend_from_slice(&[0x8b, 0x83]); // mov eax,[rbx+ram_write] (:1624)
    wantw.extend_from_slice(&o.ram_write.to_le_bytes());
    wantw.extend_from_slice(&enc);
    wantw.extend_from_slice(&[0x66, 0x43, 0x89, 0x84, 0x47, 0, 0, 0, 0]); // mov [r15+r8*2],ax (:1626)
    assert_eq!(wr, wantw, "write scale-2 leg through emit_revram_encode (:1623-1626)");
    assert_eq!(&wantw[wantw.len() - 9..], &[0x66, 0x43, 0x89, 0x84, 0x47, 0, 0, 0, 0], "store SIB index=R8 (:1626)");

    // enabled READ leg (:1628-1632): scale-2 [r15+rax*2] + PAIRED decode +
    // 2-ring value store; tail k adds the memr_act byte (:1637-1638, 2-ring
    // 0x17e gate, value 1 for reads) INSIDE the same leg.
    let dec = mk(emit_revram_decode);
    let s2 = slot2(0, 0x17e);
    let rd = mk(|a| emit_memop(a, &o, 0x17e, &op2, s2, 0, &an));
    let mut wantr = mk(|a| emit_memop_addr(a, &o, &op2));
    wantr.extend_from_slice(&[0x41, 0x0f, 0xb7, 0x84, 0x47, 0, 0, 0, 0]); // movzx eax,[r15+rax*2] (:1630)
    wantr.extend_from_slice(&dec);
    wantr.extend_from_slice(&[0x89, 0x83]); // mov [rbx+memr_val+4*s2],eax (:1632)
    wantr.extend_from_slice(&((o.memr_val + 4 * s2 as i32).to_le_bytes()));
    wantr.extend_from_slice(&[0xc6, 0x83]); // mov byte [rbx+memr_act+s2],1 (:1638)
    wantr.extend_from_slice(&((o.memr_act + s2 as i32).to_le_bytes()));
    wantr.extend_from_slice(&[0x01]);
    assert_eq!(rd, wantr, "read scale-2 leg + tail act byte (:1630-1638)");
    // non-tail read keeps NO act byte (:1637 gate)
    assert_eq!(
        mk(|a| emit_memop(a, &o, 0x40, &op2, 1, 0, &an)).len(),
        wantr.len() - 7,
        "act byte only at k>=0x17e/branchy (:1637)"
    );

    // DISABLED region (:1589 BIT set): READ = the 10-byte forced-0 store and
    // NOTHING else — no addr math, no scale-2 form, no jmp (:1590-1591).
    // (store32i = c7 83 + disp32 + imm32 = 10 — rm() disp32 fixed length)
    let dis = mk(|a| emit_memop(a, &o, 0x40, &op2, 1, 1 << op2.region, &an));
    let mut wantd: Vec<u8> = vec![0xc7, 0x83]; // mov dword [rbx+memr_val+4*1],0 (:1591)
    wantd.extend_from_slice(&((o.memr_val + 4).to_le_bytes()));
    wantd.extend_from_slice(&[0; 4]);
    assert_eq!(dis, wantd, "disabled READ = inert 0-store (:1589-1591)");
    assert!(!dis.windows(2).any(|x| x == [0x84, 0x47]) && !dis.contains(&0xe9), "disabled leg touches no RAM (:1587)");
    // DISABLED WRITE = EMPTY program (書き込みは落ちる :1588)
    assert!(mk(|a| emit_memop(a, &o, 0x40, &w, 1, 1 << w.region, &an)).is_empty(), "disabled WRITE emits nothing (:1589-1592)");
    // disabled tail read: the act byte STILL rides (:1637 is outside the gate)
    let dtail = mk(|a| emit_memop(a, &o, 0x17e, &op2, s2, 1 << op2.region, &an));
    let mut wantdt: Vec<u8> = vec![0xc7, 0x83]; // the inert 0-store at slot s2
    wantdt.extend_from_slice(&((o.memr_val + 4 * s2 as i32).to_le_bytes()));
    wantdt.extend_from_slice(&[0; 4]);
    wantdt.extend_from_slice(&[0xc6, 0x83]); // + tail act byte, value 1 (:1638)
    wantdt.extend_from_slice(&((o.memr_act + s2 as i32).to_le_bytes()));
    wantdt.push(0x01);
    assert_eq!(dtail, wantdt, "disabled tail keeps the act byte (:1637-1638)");

    // table leg (:1568-1586): absolute read + jmp around the normal path,
    // and the region gate STILL rides after it (bypassed BY the jmp):
    // patch(at) takes at = the jmp's FALL-THROUGH end (jit_emit jmp_fwd), so
    // rel = the disabled-read gate block alone = 10 B (target = gate end =
    // the patch position; the tail act byte rides AFTER the landing,
    // :1635-1638 — jumping over the gate, not over the act byte).
    let tb = Op { memop: 3, mem_table: 1, offset_index: 5, mem_use_index: 1, mem_use_index2: 1, region: 2, ..Op::ZERO };
    let tbs = mk(|a| {
        emit_memop_table(a, &o, &tb, s2);
    });
    let e9 = tbs.len() - 5;
    assert_eq!(tbs[e9], 0xe9, "jmp around the normal path (:1585)");
    let want_rel = (10u32).to_le_bytes(); // = the gate's 10 B store32i
    let full = mk(|a| emit_memop(a, &o, 0x17e, &tb, s2, 1 << tb.region, &an));
    let mut wantt = tbs.clone();
    wantt[e9 + 1..e9 + 5].copy_from_slice(&want_rel);
    let mut g: Vec<u8> = vec![0xc7, 0x83];
    g.extend_from_slice(&((o.memr_val + 4 * s2 as i32).to_le_bytes()));
    g.extend_from_slice(&[0; 4]);
    wantt.extend_from_slice(&g);
    wantt.extend_from_slice(&[0xc6, 0x83]);
    wantt.extend_from_slice(&(o.memr_act + s2 as i32).to_le_bytes());
    wantt.extend_from_slice(&[0x01]);
    assert_eq!(full, wantt, "table+jmp+patch-before-act (:1568-1585, :1635-1638)");
    // table prefix shape: base + +1 + &0x3ffff + [r15+rax*2] + decode + store
    let mut wantp: Vec<u8> = mk(|a| emit_memop_addr_base(a, &o, &tb));
    wantp.extend_from_slice(&[0x81, 0xc0, 0x01, 0x00, 0x00, 0x00]); // +1 (:1580)
    wantp.extend_from_slice(&[0x81, 0xe0, 0xff, 0xff, 0x03, 0x00]); // &0x3ffff, map-free (:1581)
    wantp.extend_from_slice(&[0x41, 0x0f, 0xb7, 0x84, 0x47, 0, 0, 0, 0]); // [r15+rax*2] (:1582)
    wantp.extend_from_slice(&dec);
    wantp.extend_from_slice(&[0x89, 0x83]);
    wantp.extend_from_slice(&((o.memr_val + 4 * s2 as i32).to_le_bytes()));
    assert_eq!(&tbs[..e9], &wantp[..], "table absolute read (:1570-1584)");

    println!("meg_jit_b2b2e byte pins: base/addr math, baked imm legs, scale-2 forms, region-gate branches, table+jmp patch");
}

// ---------------------------------------------------------------------------
// B2a — EMITTED helper selftests (C++ meg_jit_selftest :520-585 executes
// exactly these byte streams) + frame-skeleton pins + exec-buffer
// roundtrip + the PHASE_B2_EMIT_OK inertness gate. VirtualAlloc/stub-exec
// is Windows x64, so this section is cfg'd (the B1 pins above stay
// platform-free).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// B2b-3a — the LAST `unimplemented!` is gone (brief §3): meg_jit.rs must
// contain ZERO `unimplemented!`/`todo!`. This is the structural half of the
// "no half-port can sneak past" rule — the behavioral half is every exec rig
// below + the `*_build_inert` pins + the piano/boot_golden harness.
// ---------------------------------------------------------------------------
#[test]
fn meg_jit_module_has_zero_unimplemented() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/meg_jit.rs");
    let src = std::fs::read_to_string(path).expect("read meg_jit.rs");
    // Match the MACRO INVOCATION (`unimplemented!(..)` / `todo!(..)`), not the
    // prose that narrates the removal (those say "unimplemented!" in backticks
    // with no directly-following paren).
    assert!(
        !src.contains("unimplemented!("),
        "meg_jit.rs must contain ZERO `unimplemented!(..)` after B2b-3a"
    );
    assert!(!src.contains("todo!("), "meg_jit.rs must contain ZERO `todo!(..)`");
    println!("meg_jit_b2b3a: meg_jit.rs source scan — zero unimplemented!/todo! invocations");
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
mod b2a {
    use std::ffi::c_void;

    use smu_machine::meg_jit::{
        analyze_ops, emit_acc_from_p, emit_call_lfo, emit_dm_src, emit_dm_store, emit_dr_apply,
        emit_epilogue, emit_frame_skeleton, emit_index_legs, emit_lfo, emit_lfo_slot_store,
        emit_m1_expand, emit_memop, emit_memw_acc, emit_p_packed, emit_prologue, emit_revram_decode,
        emit_revram_encode, emit_ring2_folded, emit_ring2_head, emit_ring3_folded, emit_ring3_head,
        emit_shr_acc, emit_shr_acc_tz15, emit_skip_reset, emit_t_branchy, emit_t_leg,
        load_p_limits, meg_jit_call_lfo, offs, selftest_revram_decode, selftest_revram_encode,
        slot2, slot3, Assembler, Code, K_MAX, K_MIN, Mem, FRAME, MS, MegJit, PHASE_B2_EMIT_OK, P,
        RAM, R14, SC, SEED, SWP, ARG0, ARG1, ARG2, ARG3, R12, R13, R15, R9, R10, RAX, RBP, RBX,
        RCX, RDI, RDX, RSI,
    };
    use smu_swp30::meg::{meg_mem_value, meg_pack24, rand_skip, MegState, MegSwp, Op};
    use smu_swp30::mix::MegJitHook;
    use smu_swp30::voice::swp_rand;
    use smu_swp30::Swp30;
    use smu_machine::meg_jit::MegSwpDev;
    use super::zero_ops;
    use std::ptr::addr_of_mut;

    // jit.rs:49-57 pattern (C++ exec_mem.h: alloc_rw+make_executable collapse
    // to one PAGE_EXECUTE_READWRITE mapping — §7-C)
    #[link(name = "kernel32")]
    extern "system" {
        fn VirtualAlloc(p: *mut c_void, n: usize, lt: u32, prot: u32) -> *mut c_void;
        fn VirtualFree(p: *mut c_void, n: usize, ft: u32) -> i32;
    }

    struct X64Exec {
        ptr: *mut u8,
    }

    impl X64Exec {
        fn new(code: &[u8]) -> X64Exec {
            let size = (code.len() + 0xfff) & !0xfffusize;
            let p = unsafe { VirtualAlloc(std::ptr::null_mut(), size, 0x3000, 0x40) };
            assert!(!p.is_null(), "VirtualAlloc RWX failed");
            unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), p as *mut u8, code.len()) };
            X64Exec { ptr: p as *mut u8 }
        }
    }

    impl Drop for X64Exec {
        fn drop(&mut self) {
            unsafe { VirtualFree(self.ptr as *mut c_void, 0, 0x8000) };
        }
    }

    fn call_u32(x: &X64Exec, v: u32) -> u32 {
        let f: unsafe extern "system" fn(u32) -> u32 = unsafe { std::mem::transmute(x.ptr) };
        unsafe { f(v) }
    }

    fn call_i64(x: &X64Exec, v: i64) -> i64 {
        let f: unsafe extern "system" fn(i64) -> i64 = unsafe { std::mem::transmute(x.ptr) };
        unsafe { f(v) }
    }

    /// C++ selftest stub shape (:526-541 x64 arm): mov eax, ARG0; helper; ret.
    fn stub(helper: impl Fn(&mut Assembler)) -> Vec<u8> {
        let mut a = Assembler::new();
        a.mov32(RAX, ARG0);
        helper(&mut a);
        a.ret();
        a.code
    }

    /// Emitted emit_* helpers vs the PAIRED meg.rs ground truth AND the B1
    /// scalar transliterations (three-way). encode: dense mantissa band +
    /// prime-stride 27-bit walk + edges + the C++ :558 high inputs (the
    /// C++ selftest :555 full 0x8000000-call sweep is emulated by the
    /// stride for gate speed; the scalar test does the same decomposition).
    #[test]
    fn emitted_helpers_selftest() {
        let se = stub(emit_revram_encode);
        let sd = stub(emit_revram_decode);
        let sm = stub(emit_m1_expand);
        let enc = X64Exec::new(&se);
        for v in 0..0x10_0000u32 {
            let e = call_u32(&enc, v);
            assert_eq!((e & 0xffff) as u16, MegState::revram_encode(v), "emit_encode({v:#x})");
            assert_eq!(e, selftest_revram_encode(v), "emitted==scalar encode {v:#x}");
        }
        let mut v = 0u32;
        while v < 0x800_0000 {
            let e = call_u32(&enc, v);
            assert_eq!((e & 0xffff) as u16, MegState::revram_encode(v), "emit_encode stride {v:#x}");
            v = v.wrapping_add(1009);
        }
        let mut edges: Vec<u32> = vec![0, 1, 0x3ff_ffff, 0x400_0000, 0x400_0001, 0x7ff_ffff];
        for e in 0..16u32 {
            let base = 0x400u32 << e;
            for d in [0i64, -1, 1, 2] {
                edges.push((base as i64 + d) as u32 & 0x7ff_ffff);
            }
        }
        for v in edges {
            let e = call_u32(&enc, v);
            assert_eq!((e & 0xffff) as u16, MegState::revram_encode(v), "emit_encode edge {v:#x}");
        }
        for v in [0xffff_ffffu32, 0x8000_0000, 0xf800_0001] {
            // C++ :558-560 — high garbage inputs, encode still masks to 27 bits
            let e = call_u32(&enc, v);
            assert_eq!((e & 0xffff) as u16, MegState::revram_encode(v), "emit_encode high {v:#x}");
        }

        let dec = X64Exec::new(&sd);
        for v in 0..0x1_0000u32 {
            // full 16-bit domain (C++ :562-564): emitted == meg.rs ground
            // truth == the B1 scalar transliteration (three-way)
            let e = call_u32(&dec, v);
            assert_eq!(e, MegState::revram_decode(v as u16), "emit_decode({v:#x})");
            assert_eq!(e, selftest_revram_decode(v), "emitted==scalar decode {v:#x}");
        }

        let m1 = X64Exec::new(&sm);
        for v in -0x8000i64..0x8000 {
            // full s16 domain, 64-bit in/out via the loads16 sign-extended
            // convention (C++ :573-576 fn64)
            let e = call_i64(&m1, v);
            assert_eq!(e, MegState::m1_expand(v as i16) as i64, "emit_m1({v})");
        }
        println!(
            "meg_jit_b2a selftest: emitted encode/decode/m1_expand clean (three-way); stub sizes enc {} / dec {} / m1 {} bytes (incl entry+ret)",
            se.len(), sd.len(), sm.len()
        );
    }

    /// Frame/prologue/epilogue skeleton (:826-840 + :1688-1694) — byte
    /// pins: ret termination, exact Win64 callee-save push/pop order,
    /// sub/addrsp(FRAME) pair, and the rsp-aligned math at call sites.
    /// B2b-1: REAL `offs()` values now (seed legs = MegSwpDev window
    /// double-indirection, §7-B — bytes are length-stable, disp32 rm()).
    #[test]
    fn frame_skeleton_pins() {
        let o = offs();
        let mut a = Assembler::new();
        emit_frame_skeleton(&mut a, &o, false);
        let c = &a.code;
        assert!(!c.is_empty());
        // :1694 — terminates with `ret`
        assert_eq!(*c.last().unwrap(), 0xc3, "skeleton terminates with ret");
        // :827 — push rbx r12 r13 r14 r15 rsi rdi rbp (8 pushes, 12 bytes)
        assert_eq!(
            &c[..12],
            &[0x53, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x56, 0x57, 0x55],
            "prologue push order (Win64 callee-saves, S11 lesson)"
        );
        // :828/:1691 — sub rsp,FRAME .. add rsp,FRAME (FRAME=152=0x98;
        // rr() mod=11 register form: sub=C0|(5<<3)|4=EC, add=C0|(0<<3)|4=C4)
        let sub = [0x48u8, 0x81, 0xec, FRAME as u8, 0, 0, 0];
        let add = [0x48u8, 0x81, 0xc4, FRAME as u8, 0, 0, 0];
        let si = c.windows(sub.len()).position(|w| w == sub).expect("sub rsp,FRAME");
        let ai = c.windows(add.len()).position(|w| w == add).expect("add rsp,FRAME");
        assert!(si < ai, "frame opens before it closes");
        // :1692 — exact LIFO pop twin: rbp rdi rsi r15 r14 r13 r12 rbx (12 bytes)
        assert_eq!(
            &c[c.len() - 13..c.len() - 1],
            &[0x5d, 0x5f, 0x5e, 0x41, 0x5f, 0x41, 0x5e, 0x41, 0x5d, 0x41, 0x5c, 0x5b],
            "epilogue pop order (exact LIFO inverse)"
        );
        // rsp balance: entry rsp≡8 (mod 16) from the call; −64 −152 inside
        // the body ⇒ ≡0 = 16-aligned at every frame-body callout (:1443);
        // +152 +64 restores it exactly at the ret.
        assert_eq!((8 - 8 * 8 - FRAME) % 16, 0, "rsp 16-aligned at frame-body calls");
        assert_eq!(-8 * 8 - FRAME + FRAME + 8 * 8, 0, "net rsp delta across the frame is zero");
        // skeleton == emit_prologue ++ emit_epilogue (same real offsets)
        let mut pa = Assembler::new();
        emit_prologue(&mut pa, o.p, o.sample, o.seed);
        let mut ea = Assembler::new();
        emit_epilogue(&mut ea, o.p, o.seed);
        let mut both = pa.code.clone();
        both.extend_from_slice(&ea.code);
        assert_eq!(c.as_slice(), both.as_slice(), "skeleton == prologue ++ epilogue");
        println!(
            "meg_jit_b2a frame: {} bytes (prologue {} / epilogue {}), ret-terminated, rsp-balanced, calls aligned",
            c.len(), pa.code.len(), ea.code.len()
        );
    }

    /// Exec buffer scaffolding (brief §1): RWX alloc/release roundtrip and
    /// Drop, no-crash (mirrors C++ :1696-1705; the mapping is genuinely
    /// executable — write a `ret` and call it).
    #[test]
    fn exec_buffer_alloc_release() {
        let mut cd = Code::new();
        assert_eq!(cd.exec_state(), (0, 0, 0), "fresh Code: fnp/buf/size all zero");
        assert!(cd.alloc_buf(1));
        let (fnp, buf, size) = cd.exec_state();
        assert_eq!(fnp, 0, "alloc alone must never publish fnp");
        assert_ne!(buf, 0);
        assert_eq!(size, 0x1_0000, "64 KiB granularity (:1699)");
        unsafe { std::ptr::write(buf as *mut u8, 0xc3) }; // ret
        let f: unsafe extern "system" fn() -> u32 = unsafe { std::mem::transmute(buf) };
        unsafe { f() }; // no-crash execution proof
        cd.release_buf();
        assert_eq!(cd.exec_state(), (0, 0, 0), "released");
        {
            let mut cd2 = Code::new();
            assert!(cd2.alloc_buf(100));
        } // Drop -> VirtualFree (:277-283 mirror) must not fault
        // regrow keeps both sides consistent (:1696-1699): 64K+1 needs a
        // second page; the (:1699) rounding is (need+0xffff)&~0xffff.
        let mut cd3 = Code::new();
        assert!(cd3.alloc_buf(0x10));
        cd3.release_buf();
        assert!(cd3.alloc_buf(0x1_0001));
        assert_eq!(cd3.exec_state().2, 0x2_0000, "rounded growth to 64 KiB pages (:1699)");
        cd3.release_buf();
    }

    /// B2b-3c MIGRATION of `b2a_build_inert_even_with_big_ram`: emission is
    /// LIVE — past the :611-612 RAM guard `rebuild()` now SUCCEEDS (the
    /// formerly "even with big RAM" refused case), fnp publishes, and run()
    /// executes the compiled block. The RAM guard refusal (the honest
    /// remaining refusal) rides at the end of the same row.
    #[test]
    fn b2a_build_live_even_with_big_ram() {
        assert!(PHASE_B2_EMIT_OK, "B2b-3c: emission is LIVE");
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        let mut j = smu_machine::meg_jit::MegJit::new();
        assert!(j.can(), "enabled — emission is live");
        // ram_len = 0x40000: the C++ RAM guard (:611-612) is OPEN here
        assert!(j.rebuild(&s.meg, &s.meg_ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        let (fnp, buf, sz) = j.gen_code_state();
        assert_ne!(fnp, 0, "fnp published (:1712)");
        assert_eq!(fnp, buf, "fnp == the RWX buffer (:1712)");
        assert!(sz >= 0x1_0000, "64 KiB-granular RWX buffer (:1699)");
        let mut seam = smu_swp30::meg::MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        assert!(j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "run() executes the compiled block");
        // RAM-guard refusal case (KEPT): below 0x40000 nothing compiles, the
        // published gen is cleared (:882-884) and run() falls to the
        // interpreter at the :390 null guard.
        assert!(!j.rebuild(&s.meg, &s.meg_ops, 0x3_ffff, s.revram_enable, &sintab, s.meg_const_gen), "RAM guard (:611-612)");
        assert_eq!(j.gen_code_state().0, 0, "guard-closed build keeps fnp==0 (:604)");
        assert!(!j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "fnp==0 ⇒ interpreter-fall (:390)");
        println!("meg_jit_b2a: LIVE at ram_len=0x40000 (rebuild/fnp/run true); guard refuses at 0x3ffff, fnp==0");
    }

    /// B2b-3c MIGRATION of `b2b1_build_inert`: the branchy program that had
    /// to stay un-published now COMPILES (fnp != 0) and RUNS (run() true);
    /// the :614-615 ring snapshot pin is KEPT (still committed at compile).
    #[test]
    fn b2b1_build_live() {
        assert!(PHASE_B2_EMIT_OK, "B2b-3c: emission is LIVE");
        let mut s = Swp30::new();
        let sintab: Vec<u16> = vec![0; 0x8000]; // resident sintab (bake-ready)
        let mut j = smu_machine::meg_jit::MegJit::new();
        assert!(j.can(), "enabled — emission is live");
        // branchy program (jump at 0x20) + moved ring snapshot
        let mut ops = *s.meg_ops;
        ops[0x20].jump = 1;
        ops[0x20].target = 0x10;
        s.meg.delay_3 = 2;
        s.meg.delay_2 = 1;
        assert!(j.rebuild(&s.meg, &ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        let (fnp, buf, _size) = j.gen_code_state();
        assert_ne!(fnp, 0, "brief (migrated): executable entry published (:1712)");
        assert_ne!(buf, 0, "exec buffer handed off (:1696-1712)");
        assert_eq!(j.gen_ring(), (2, 1), ":614-615 snapshot committed at compile");
        let mut seam = smu_swp30::meg::MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        // run() passes the :390 fn guard, the :396 revram re-check and the
        // :423 d3/d2 re-check (just compiled from this exact state) and
        // executes the branchy block.
        assert!(j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen));
        println!("meg_jit_b2b1: branchy+big-ram build LIVE, fnp/buf published, ring committed, run() executes");
    }

    // -----------------------------------------------------------------------
    // B2b-2a — EXEC rigs for the skip reset and the ring legs. Each stub is
    // a STANDALONE helper harness (prologue-swap of (MS, SWP) + the leg(s) +
    // ret) — never a half-emitted program (brief: "without half-compiling a
    // program"). RBX/R12 are preserved (callee-saved, S11 lesson); RAX/RCX/
    // RDX are the legs' own scratch (no callout inside ring-apply).
    // -----------------------------------------------------------------------

    /// Park the real register plan on the real ARG positions (ARG0=ms,
    /// ARG1=window — the JIT fn_t shape, :266/:829-831), run `body`,
    /// return 0. The Rust caller sees a balanced stack + intact RBX/R12.
    fn rig(body: impl Fn(&mut Assembler)) -> Vec<u8> {
        let mut a = Assembler::new();
        a.push(RBX);
        a.push(R12);
        a.mov64(MS, ARG0);
        a.mov64(SWP, ARG1);
        body(&mut a);
        a.xor32(RAX, RAX);
        a.pop(R12);
        a.pop(RBX);
        a.ret();
        a.code
    }

    /// emit_skip_reset (:839-840, u16-adapted through the §7-B window):
    /// the u16 in the window's skip slot clears to 0 AND the neighbor u16
    /// survives — a 32-bit store (the C++ width) would smash the second
    /// half. RAX stays the slot pointer, so the stub returns the cleared
    /// value straight back.
    #[test]
    fn b2b2a_skip_reset_exec() {
        let o = offs();
        let mut a = Assembler::new();
        a.push(R12); // callee-saved across the Rust caller (S11 lesson)
        a.mov64(SWP, ARG0); // window base -> the real SWP reg
        emit_skip_reset(&mut a, &o);
        a.loadu16(RAX, Mem::b(RAX, 0)); // eax = post-clear skip value
        a.pop(R12);
        a.ret();
        let x = X64Exec::new(&a.code);
        let f: unsafe extern "system" fn(*mut MegSwpDev) -> u32 =
            unsafe { std::mem::transmute(x.ptr) };
        let mut buf: [u16; 2] = [0x1234, 0xbeef];
        let mut dev = MegSwpDev {
            seed: std::ptr::null_mut(),
            flag_n: std::ptr::null_mut(),
            flag_z: std::ptr::null_mut(),
            ix2_value: std::ptr::null_mut(),
            ix2_act: std::ptr::null_mut(),
            ram_index2: std::ptr::null_mut(),
            skip: buf.as_mut_ptr(),
        };
        let got = unsafe { f(&mut dev) };
        assert_eq!(got, 0, "skip slot cleared through the window (:840)");
        assert_eq!(buf, [0, 0xbeef], "16-bit store only — neighbor u16 intact");
        println!("meg_jit_b2b2a skip-reset exec: u16 cleared in place, no 32-bit smash");
    }

    /// emit_ring3_head (:1000-1029) against LIVE Swp30 state through the
    /// real register plan: slot-0 legs all apply (scale-4 m/r stores,
    /// ram_index, window-deref'd ram_index2), slot-1's zero reg/act bytes
    /// take the jz legs and must leave the poison sentinels alone.
    #[test]
    fn b2b2a_ring_head_exec() {
        let o = offs();
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        // slot 0: every leg active
        s.meg.mw_reg[0] = 3;
        s.meg.mw_value[0] = 0x77aa_55ccu32 as i32;
        s.meg.rw_reg[0] = 5;
        s.meg.rw_value[0] = 0x1122_3344;
        s.meg.index_active[0] = true;
        s.meg.index_value[0] = -7;
        s.meg_ix2_act[0] = 1;
        s.meg_ix2_value[0] = -9;
        // slot 1 poison: reg/act bytes are 0 (⇒ destination reg 0 / no-op),
        // values junk — any jz misfire lands visibly in m[0]/r[0]/…
        s.meg.m[0] = 0x5a5a_5a5au32 as i32;
        s.meg.mw_reg[1] = 0;
        s.meg.mw_value[1] = 0xdead_beefu32 as i32;
        s.meg.r[0] = 0xa5a5_a5a5u32 as i32;
        s.meg.rw_reg[1] = 0;
        s.meg.rw_value[1] = 0xc0ffee;
        s.meg.index_active[1] = false;
        s.meg.index_value[1] = 99;
        s.meg_ix2_act[1] = 0;
        s.meg_ix2_value[1] = 123;
        let ms = addr_of_mut!(s.meg);
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        let code = rig(|a| {
            emit_ring3_head(a, &o, 0);
            emit_ring3_head(a, &o, 1);
        });
        let x = X64Exec::new(&code);
        let f: unsafe extern "system" fn(*mut MegState, *const MegSwpDev) -> u32 =
            unsafe { std::mem::transmute(x.ptr) };
        unsafe { f(ms, &dev) };
        assert_eq!(s.meg.m[3], 0x77aa_55ccu32 as i32, ":1003-1008 scale-4 m store");
        assert_eq!(s.meg.r[5], 0x1122_3344, ":1010-1015 r store");
        assert_eq!(s.meg.ram_index, -7, ":1017-1022 index apply");
        assert_eq!(s.meg_ram_index2, -9, ":1024-1029 index2 apply via window");
        // jz legs: zero reg/act byte ⇒ nothing applied
        assert_eq!(s.meg.m[0], 0x5a5a_5a5au32 as i32, "reg byte 0 ⇒ no m store (:1005)");
        assert_eq!(s.meg.r[0], 0xa5a5_a5a5u32 as i32, "reg byte 0 ⇒ no r store");
        assert_eq!(s.meg.index_value[1], 99);
        assert_eq!(s.meg_ix2_value[1], 123, "act byte 0 ⇒ ix2 leg skipped (:1026)");
        println!("meg_jit_b2b2a ring3-head exec: slot-0 applies, zero-byte jz legs hold");
    }

    /// The folded legs against live state: ring3 folded (MEG_EARLY off ⇒
    /// every flagged write stores from its slot, :1030-1048), ring2 head
    /// (apply AND clear the one-shot act bytes, :1050-1065), ring2 folded
    /// (slot-0 memw/memop-3 stores, :1066-1077 — observable AFTER the head
    /// because it overwrites ram_write/ram_read with slot-0 values).
    #[test]
    fn b2b2a_ring_folded_exec() {
        let o = offs();
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        // writer w (ops[k-3] / ops[k-2] twin): all mem/index flags on
        let mut ops = zero_ops();
        ops[0x10].dm = 20;
        ops[0x10].dr = 70;
        ops[0x10].index = 1;
        ops[0x10].index2 = 1;
        ops[0x10].memw = 1;
        ops[0x10].memop = 3;
        // MEG_EARLY off ⇒ nothing folds early ⇒ every leg must fire
        let an_off = analyze_ops(&ops, false);
        assert!(an_off.early_m.iter().all(|&b| !b));
        // ring3 folded slot 0 values
        s.meg.mw_value[0] = 0x0102_0304;
        s.meg.rw_value[0] = -0x0a0b_0c0d;
        s.meg.index_value[0] = -3;
        s.meg_ix2_value[0] = -4;
        // ring2 head slot 1: one-shot act applies + clears
        s.meg.memw_active[1] = true;
        s.meg.memw_value[1] = 0x2000;
        s.meg.memr_active[1] = true;
        s.meg.memr_value[1] = 0x3000;
        // ring2 folded slot 0: these must land LAST (memw + memop==3 read)
        s.meg.memw_value[0] = 0x4000;
        s.meg.memr_value[0] = 0x5000;
        let ms = addr_of_mut!(s.meg);
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        let code = rig(|a| {
            emit_ring3_folded(a, &o, 0, &ops[0x10], &an_off);
            emit_ring2_head(a, &o, 1);
            emit_ring2_folded(a, &o, 0, &ops[0x10]);
        });
        let x = X64Exec::new(&code);
        let f: unsafe extern "system" fn(*mut MegState, *const MegSwpDev) -> u32 =
            unsafe { std::mem::transmute(x.ptr) };
        unsafe { f(ms, &dev) };
        assert_eq!(s.meg.m[20], 0x0102_0304, ":1033-1036 folded dm store");
        assert_eq!(s.meg.r[70], -0x0a0b_0c0d, ":1037-1040 folded dr store");
        assert_eq!(s.meg.ram_index, -3, ":1041-1044 folded index");
        assert_eq!(s.meg_ram_index2, -4, ":1045-1048 folded index2 via window");
        // ring2: head applied slot 1, then folded slot 0 overwrote both ports
        assert_eq!(s.meg.ram_write, 0x4000, ":1069-1072 memw fold wins last");
        assert_eq!(s.meg.ram_read, 0x5000, ":1073-1076 memop==3 memr fold");
        // head cleared the two slot-1 act bytes (:1057/:1064); slot 0's
        // were never active (the fold never touches act bytes)
        assert!(!s.meg.memw_active[1] && !s.meg.memr_active[1], "one-shot act clear (:1057/:1064)");
        assert!(!s.meg.memw_active[0] && !s.meg.memr_active[0]);
        println!("meg_jit_b2b2a ring-folded exec: dm/dr/index/ix2 + memw/memr legs + act clear pinned");
    }

    /// B2b-3c MIGRATION of `b2b2a_build_inert`: the branchy ring-flagged
    /// program now compiles AND runs; the skip leg and ring helpers' bytes
    /// are LIVE in the published buffer (the standalone exec rigs above pin
    /// them individually).
    #[test]
    fn b2b2a_build_live() {
        assert!(PHASE_B2_EMIT_OK, "B2b-3c: emission is LIVE");
        let mut s = Swp30::new();
        let sintab: Vec<u16> = vec![0; 0x8000];
        let mut j = MegJit::new();
        assert!(j.can(), "enabled — emission is live");
        let mut ops = *s.meg_ops;
        ops[0x20].jump = 1; // branchy ⇒ the skeleton emits the new skip leg
        ops[0x20].target = 0x10;
        ops[0x10].dm = 5;
        ops[0x10].dr = 9;
        ops[0x10].index = 1;
        ops[0x10].index2 = 1;
        s.meg.delay_3 = 2;
        s.meg.delay_2 = 1;
        assert!(j.rebuild(&s.meg, &ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        let (fnp, buf, _sz) = j.gen_code_state();
        assert_ne!(fnp, 0, "brief (migrated): entry published (:1712)");
        assert_ne!(buf, 0, "buffer handoff done (:1696-1712)");
        assert_eq!(j.gen_ring(), (2, 1), "ring snapshot commits at compile (:614-615)");
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        assert!(j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "run() executes the branchy skip/ring block");
        println!("meg_jit_b2b2a: branchy+ring-flagged build LIVE, gen fnp published, run() executes");
    }

    // -----------------------------------------------------------------------
    // B2b-2b — EXEC rigs for the dm/dr legs + pack24/rnd/p_packed helpers
    // (swp30_jit.cpp:843-889, :988, :1414-1496). Standalone stubs only —
    // never a half-emitted program (brief). Ground truths are the PAIRED
    // meg.rs: meg_pack24, voice::swp_rand, meg::rand_skip.
    // -----------------------------------------------------------------------

    /// dm src select legs against live state (:1415-1465): m[sm] (:1464),
    /// ram_read (:1453) and the frame-slot round-trip (:988 store written
    /// by `emit_lfo_slot_store`, :1419 load read back by `emit_dm_src`).
    #[test]
    fn b2b2b_dm_src_exec_legs() {
        let o = offs();
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        s.meg.m[7] = 0x1122_3344u32 as i32;
        s.meg.ram_read = 0xcafe_f00d;
        let ms = addr_of_mut!(s.meg);
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        type Sel = unsafe extern "system" fn(*mut MegState, *const MegSwpDev) -> u32;

        // m[sm] leg (the switch `default`, :1464)
        let op7 = Op { dm_src: 7, sm: 7, dm: 5, ..Op::ZERO };
        let mut a = Assembler::new();
        a.push(RBX);
        a.mov64(MS, ARG0);
        emit_dm_src(&mut a, &o, &op7, 0, std::ptr::null(), 0);
        a.pop(RBX);
        a.ret();
        let x = X64Exec::new(&a.code);
        assert_eq!(unsafe { std::mem::transmute::<*mut u8, Sel>(x.ptr)(ms, &dev) }, 0x1122_3344);

        // ram_read leg (:1453)
        let op4 = Op { dm_src: 4, dm: 1, ..Op::ZERO };
        let mut a = Assembler::new();
        a.push(RBX);
        a.mov64(MS, ARG0);
        emit_dm_src(&mut a, &o, &op4, 0, std::ptr::null(), 0);
        a.pop(RBX);
        a.ret();
        let x = X64Exec::new(&a.code);
        assert_eq!(unsafe { std::mem::transmute::<*mut u8, Sel>(x.ptr)(ms, &dev) }, 0xcafe_f00d);

        // frame-slot round trip: emit_lfo_slot_store (:988) parks the
        // "hoisted" value, emit_dm_src's :1419 leg reads it back. The
        // subrsp/addrsp pair stands in for the real frame (:828/:1691).
        let op0 = Op { dm_src: 0, lfo: 2, dm: 1, ..Op::ZERO };
        let mut a = Assembler::new();
        a.push(RBX);
        a.mov64(MS, ARG0);
        a.subrsp(256);
        a.imm32(RAX, 0xabcd_1234); // stand-in for the emit_lfo result (:968)
        emit_lfo_slot_store(&mut a, 60); // :988
        emit_dm_src(&mut a, &o, &op0, 60, std::ptr::null(), 0); // :1419
        a.addrsp(256);
        a.pop(RBX);
        a.ret();
        let x = X64Exec::new(&a.code);
        assert_eq!(
            unsafe { std::mem::transmute::<*mut u8, Sel>(x.ptr)(ms, &dev) },
            0xabcd_1234,
            "LFO-slot store :988 ⇄ frame-slot load :1419 round trip"
        );
        println!("meg_jit_b2b2b dm-src exec: m[sm] / ram_read / frame-slot legs proven");
    }

    /// dm_src 5 (:1456-1458) executed: the rnd leg's draw (ROL16 form)
    /// sign-extended through shl32/sar32, and SEED left exactly where the
    /// PAIRED voice::swp_rand leaves it (handoff §H — same LCG, risk 3).
    #[test]
    fn b2b2b_rnd_exec() {
        let op5 = Op { dm_src: 5, dm: 1, ..Op::ZERO };
        for seed in [0u32, 1, 0xdead_beef, 0x1234_5678, 0xffff_ffff] {
            let mut a = Assembler::new();
            a.push(RSI);
            a.mov32(SEED, ARG1); // pin the seed where the prologue keeps it (:834)
            emit_dm_src(&mut a, &offs(), &op5, 0, std::ptr::null(), 0); // rnd ++ shl8/sar8 (:1456-1458)
            a.store32(Mem::b(ARG2, 0), SEED); // seed return, epilogue shape (:1690)
            a.pop(RSI);
            a.ret();
            let x = X64Exec::new(&a.code);
            let f: unsafe extern "system" fn(*mut c_void, u32, *mut u32) -> u32 =
                unsafe { std::mem::transmute(x.ptr) };
            let mut sd = seed;
            let draw = swp_rand(&mut sd);
            let want = ((draw << 8) as i32 >> 8) as u32;
            let mut out = 0u32;
            let got = unsafe { f(std::ptr::null_mut(), seed, &mut out) };
            assert_eq!(got, want, "dm_src 5 draw seed={seed:#x}");
            assert_eq!(out, sd, "SEED advanced exactly one draw (:862)");
        }
        println!("meg_jit_b2b2b rnd exec: emitted LCG+ROL16 == voice::swp_rand, seed returned");
    }

    /// pack24 / p_packed executed against the PAIRED meg::meg_pack24 —
    /// truncation toward zero, the ONE-past-limit clamps (:851-854 exact
    /// -value hits), the 24-bit fold, and the `& 0x07e0` dither leg
    /// (:877, handoff risk 3: the mask lives ONLY here and on dm6/dr-p).
    #[test]
    fn b2b2b_packed_exec() {
        let ps: [i64; 13] = [
            0,
            32_767,
            32_768,
            -32_768,
            -32_767,
            0x80_0000i64 * 32_768, // q == 0x800000  ⇒ K_MAX clamp (:851)
            -0x80_0001i64 * 32_768, // q == -0x800001 ⇒ K_MIN clamp (:853)
            0x0123_4567i64 * 32_768 + 1, // trunc-toward-0 low bits (:844)
            -0x1000i64 * 32_768 - 1, // negative trunc leg
            0x7f_ffffi64 * 32_768,
            -0x80_0000i64 * 32_768,
            (i32::MAX as i64) * 32_768,
            -(1i64 << 40), // fold leg
        ];
        // noise=false: mov rax,P + pack24, K regs pinned prologue-style.
        // R13 is CALLEE-SAVED on Win64 (S11 lesson) — the rig parks P in
        // it for the leg window and restores it before the ret, or it
        // smashes the test harness's own live values.
        let mut a = Assembler::new();
        a.push(RBP);
        a.push(RDI);
        a.push(P); // R13 — parked for the leg window
        a.imm64(K_MAX, 0x7fff_ff); // :835 (0x7fffff)
        a.imm64(K_MIN, (-0x80_0000i64) as u64); // :836
        a.mov64(P, ARG0);
        emit_p_packed(&mut a, false); // :880+pack24
        a.pop(P);
        a.pop(RDI);
        a.pop(RBP);
        a.ret();
        let x = X64Exec::new(&a.code);
        let f: unsafe extern "system" fn(i64) -> u32 = unsafe { std::mem::transmute(x.ptr) };
        for p in ps {
            assert_eq!(unsafe { f(p) }, meg_pack24(p), "packed({p:#x})");
        }
        // noise=true: rnd + and 0x07e0 + add rax,P + pack24 (:876-881)
        let mut a = Assembler::new();
        a.push(RBP);
        a.push(RDI);
        a.push(RSI);
        a.push(P); // R13 — parked for the leg window
        a.imm64(K_MAX, 0x7fff_ff);
        a.imm64(K_MIN, (-0x80_0000i64) as u64);
        a.mov64(P, ARG0);
        a.mov32(SEED, ARG1);
        emit_p_packed(&mut a, true);
        a.store32(Mem::b(ARG2, 0), SEED);
        a.pop(P);
        a.pop(RSI);
        a.pop(RDI);
        a.pop(RBP);
        a.ret();
        let x = X64Exec::new(&a.code);
        let f: unsafe extern "system" fn(i64, u32, *mut u32) -> u32 =
            unsafe { std::mem::transmute(x.ptr) };
        for seed in [0u32, 1, 0xdead_beef] {
            let mut sd = seed;
            let draw = swp_rand(&mut sd);
            for &p in &ps {
                let want = meg_pack24(p.wrapping_add((draw & 0x07e0) as i64));
                let mut out = 0u32;
                let got = unsafe { f(p, seed, &mut out) };
                assert_eq!(got, want, "packed noise p={p:#x} seed={seed:#x}");
                assert_eq!(out, sd, "noise draw advanced SEED once (:862)");
            }
        }
        println!("meg_jit_b2b2b packed exec: pack24/p_packed == meg_pack24 (clamps + fold + dither)");
    }

    /// emit_dm_store (:1467-1475) against live state, two rigs:
    /// non-branchy (early+last direct store :1468+:1470; tail trio ring
    /// + reg byte :1472+:1474) and branchy (ring + byte at a low k; a
    /// dm==0 op CLEARS its slot byte OUTSIDE the dm gate :1474).
    #[test]
    fn b2b2b_dm_store_exec() {
        let o = offs();
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        s.meg.m[3] = 0xfeed_f00du32 as i32; // the RAX value source (sm=3)
        s.meg.m[5] = 0x5b5b_5b5bu32 as i32; // poisons: direct-store targets
        s.meg.m[6] = 0x6c6c_6c6cu32 as i32;
        s.meg.mw_reg[1] = 0x7f; // must survive a !tail !branchy op
        let mut ops = zero_ops();
        ops[0x10].dm = 5; // early + last of class 1
        ops[0x17d].dm = 6; // tail trio: ring + byte (slot 0)
        let an = analyze_ops(&ops, true);
        assert!(an.early_m[5] && an.last_slot_m[0x10] && !an.branchy);
        let ms = addr_of_mut!(s.meg);
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        type Run = unsafe extern "system" fn(*mut MegState, *const MegSwpDev) -> u32;

        // rig A — non-branchy: early+last, then the tail op
        let op7_5 = Op { dm_src: 7, sm: 3, dm: 5, ..Op::ZERO };
        let op7_6 = Op { dm_src: 7, sm: 3, dm: 6, ..Op::ZERO };
        let code = rig(|a| {
            emit_dm_src(a, &o, &op7_5, 0, std::ptr::null(), 0); // eax = m[3]
            emit_dm_store(a, &o, 0x10, &ops[0x10], slot3(0, 0x10), &an);
            emit_dm_src(a, &o, &op7_6, 0, std::ptr::null(), 0);
            emit_dm_store(a, &o, 0x17d, &ops[0x17d], slot3(0, 0x17d), &an);
        });
        let x = X64Exec::new(&code);
        unsafe { std::mem::transmute::<*mut u8, Run>(x.ptr)(ms, &dev) };
        assert_eq!(s.meg.m[5], 0xfeed_f00du32 as i32, "early DIRECT m store (:1468)");
        assert_eq!(s.meg.mw_value[1], 0xfeed_f00du32 as i32, "last-slot ring ALSO written (:1470)");
        assert_eq!(s.meg.mw_reg[1], 0x7f, "reg byte untouched at k<0x17d !branchy (:1474 gate)");
        assert_eq!(s.meg.mw_value[0], 0xfeed_f00du32 as i32, "tail trio stores the RING (:1472)");
        assert_eq!(s.meg.mw_reg[0], 6, "tail reg byte = dm (:1474-1475)");
        assert_eq!(s.meg.m[6], 0x6c6c_6c6cu32 as i32, "tail direct m store must NOT happen (:1467 k gate)");

        // rig B — branchy: ring + byte at a low k, and the dm==0 clear
        ops[0x40].jump = 1;
        ops[0x40].target = 0x10;
        let anb = analyze_ops(&ops, true);
        assert!(anb.branchy && anb.early_m.iter().all(|&b| !b));
        ops[0x11].dm = 7; // branchy slot 2 ⇒ ring store + byte
        let anb = analyze_ops(&ops, true);
        let op7_7 = Op { dm_src: 7, sm: 3, dm: 7, ..Op::ZERO };
        let code = rig(|a| {
            emit_dm_src(a, &o, &op7_7, 0, std::ptr::null(), 0);
            emit_dm_store(a, &o, 0x11, &ops[0x11], slot3(0, 0x11), &anb);
            emit_dm_store(a, &o, 0x12, &Op::ZERO, slot3(0, 0x12), &anb); // dm==0 clear (slot 0)
        });
        let x = X64Exec::new(&code);
        unsafe { std::mem::transmute::<*mut u8, Run>(x.ptr)(ms, &dev) };
        assert_eq!(s.meg.mw_value[2], 0xfeed_f00du32 as i32, "branchy low k: ring store (:1472)");
        assert_eq!(s.meg.mw_reg[2], 7, "branchy reg byte (:1474 branchy arm)");
        assert_eq!(s.meg.mw_reg[0], 0, "dm==0 clears the slot byte outside the gate (:1474)");
        assert_eq!(s.meg.mw_value[0], 0xfeed_f00du32 as i32, "dm==0 touches NO value slot (:1415 gate)");
        println!("meg_jit_b2b2b dm-store exec: early/last/tail/branchy/zero-clear legs pinned");
    }

    /// emit_dr_apply (:1480-1496) against live state + the live rand seed:
    /// the rand_n head rides rand_jump(3) exactly (== PAIRED meg::rand_skip),
    /// the r[sr] leg feeds the value that lands early+last-slot, and the
    /// tail op writes the ring + rw_reg byte.
    #[test]
    fn b2b2b_dr_exec() {
        let o = offs();
        let mut s = Swp30::new();
        s.meg.r[4] = 0x0bad_c0de; // ALU-bank source (:1485)
        s.meg.r[9] = 0x9999_9999u32 as i32; // early direct-store target poison
        s.meg.r[10] = 0xaaaa_aaaau32 as i32; // tail: ring-only, must stay
        s.meg.rw_reg[1] = 0x77; // must survive (k<0x17d, !branchy)
        let mut ops = zero_ops();
        ops[0x10].dr = 9;
        ops[0x10].dr_from_r = 1;
        ops[0x10].sr = 4;
        ops[0x10].rand_n = 3; // coalesced skipped-region draws (:1481)
        ops[0x17d].dr = 10;
        ops[0x17d].dr_from_r = 1;
        ops[0x17d].sr = 4;
        let an = analyze_ops(&ops, true);
        assert!(an.early_r[9] && an.last_slot_r[0x10] && !an.branchy);
        let ms = addr_of_mut!(s.meg);
        let mut a = Assembler::new();
        a.push(RBX);
        a.push(RSI);
        a.mov64(MS, ARG0);
        a.mov32(SEED, ARG1);
        emit_dr_apply(&mut a, &o, 0x10, &ops[0x10], slot3(0, 0x10), &an);
        emit_dr_apply(&mut a, &o, 0x17d, &ops[0x17d], slot3(0, 0x17d), &an);
        a.store32(Mem::b(ARG2, 0), SEED);
        a.pop(RSI);
        a.pop(RBX);
        a.ret();
        let x = X64Exec::new(&a.code);
        let f: unsafe extern "system" fn(*mut MegState, u32, *mut u32) -> u32 =
            unsafe { std::mem::transmute(x.ptr) };
        let mut sd = 0x1357_9bdfu32;
        rand_skip(&mut sd, 3);
        let mut out = 0u32;
        unsafe { f(ms, 0x1357_9bdf, &mut out) };
        assert_eq!(s.meg.r[9], 0x0bad_c0de, "early DIRECT r store (:1489)");
        assert_eq!(s.meg.rw_value[1], 0x0bad_c0de, "last-slot rw ring ALSO written (:1491)");
        assert_eq!(s.meg.rw_reg[1], 0x77, "rw_reg byte untouched at k<0x17d !branchy (:1495 gate)");
        assert_eq!(s.meg.rw_value[0], 0x0bad_c0de, "tail ring store (:1493)");
        assert_eq!(s.meg.rw_reg[0], 10, "tail rw_reg byte = dr (:1495-1496)");
        assert_eq!(s.meg.r[10], 0xaaaa_aaaau32 as i32, "tail dr must NOT store direct (:1488 k gate)");
        assert_eq!(out, sd, "rand_n=3 seed advance == meg::rand_skip (:1481-1482)");
        println!("meg_jit_b2b2b dr exec: rnd_skip head + r[sr] feed + early/last/tail legs pinned");
    }

    // -----------------------------------------------------------------------
    // B2b-2c — EXEC rigs for the memw + ix2 index legs (swp30_jit.cpp:883-889,
    // :1501-1525). Standalone stubs only — never a half-emitted program (brief).
    // Ground truths: meg::meg_mem_value (:578 truncate-toward-zero) vs the
    // arithmetic `(p >> 23) as i32` the interpreter uses for both index banks
    // (meg.rs:893/:899). The p accumulator is loaded through the MS offset
    // (prologue :832 shape) so the legs see a real R13.
    // -----------------------------------------------------------------------

    /// AccFromP (:884) returns the P accumulator verbatim; ShrAccTZ15
    /// (:886-889) then shifts it right 15 TOWARD ZERO (== meg::meg_mem_value,
    /// NOT an arithmetic `>>15` — the pin below proves the difference at
    /// negative p).
    #[test]
    fn b2b2c_acc_from_p_shr_tz_exec() {
        // rig: mov r13,ARG0 (p) ; AccFromP (rax=P) ; ret  -> returns p verbatim
        let mut a = Assembler::new();
        a.push(P);
        a.mov64(P, ARG0);
        emit_acc_from_p(&mut a); // :884
        a.pop(P);
        a.ret();
        let x = X64Exec::new(&a.code);
        let facc: unsafe extern "system" fn(i64) -> i64 = unsafe { std::mem::transmute(x.ptr) };
        for p in [0i64, 1, -1, 5, -5, 1 << 30, -(1 << 30), 0x2_0000_0000, -(0x2_0000_0000)] {
            assert_eq!(unsafe { facc(p) }, p, "AccFromP returns P verbatim ({p})");
        }

        // rig: mov r13,ARG0 ; AccFromP ; ShrAccTZ15 ; ret -> rax = p/32768 (TZ)
        let mut a = Assembler::new();
        a.push(P);
        a.mov64(P, ARG0);
        emit_acc_from_p(&mut a); // :1504
        emit_shr_acc_tz15(&mut a); // :1505
        a.pop(P);
        a.ret();
        let x = X64Exec::new(&a.code);
        let ftz: unsafe extern "system" fn(i64) -> i64 = unsafe { std::mem::transmute(x.ptr) };
        // p band spans plausible 42-bit accumulator values; the negatives are
        // the discriminator: arithmetic `>>15` of -1 == -1, toward-zero == 0.
        let ps: [i64; 12] = [
            0,
            1,
            -1, // TZ ⇒ 0, arith ⇒ -1 (the key contrast)
            32_767,
            -32_768,
            32_768,
            -32_769, // -1.00003 → -1 (TZ)
            1 << 30,
            -(1 << 30) - 1,
            0x1_ffff_ffff,
            -0x1_ffff_ffff,
            (1i64 << 41) - 1, // accumulator ceiling
        ];
        for p in ps {
            assert_eq!(unsafe { ftz(p) }, meg_mem_value(p), "ShrAccTZ15({p}) == meg_mem_value");
            assert_eq!(
                unsafe { ftz(p) },
                (p / 32768) as i64,
                "ShrAccTZ15 truncates toward zero ({p})"
            );
        }
        // Prove the leg is NOT the arithmetic shift the index bank uses for the
        // negatives where the two disagree (p=-1: arith -1, TZ 0).
        assert_ne!(unsafe { ftz(-1) }, (-1i64) >> 15, "TZ != arithmetic >>15 at p=-1");
        println!("meg_jit_b2b2c acc exec: AccFromP verbatim + ShrAccTZ15 == meg_mem_value (TZ)");
    }

    /// emit_memw_acc (:1501-1506) against a live MegState through the real
    /// MS base: the memw value slot gets `meg_mem_value(p)` (truncate toward
    /// ZERO, upstream.md 39), while the index/act slots stay untouched (the
    /// mid-k non-branchy op suppresses the 2-ring act byte, :1508).
    #[test]
    fn b2b2c_memw_exec_truncate_toward_zero() {
        let o = offs();
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        let mut ops = zero_ops();
        ops[0x10].memw = 1;
        let an = analyze_ops(&ops, true); // non-branchy
        assert!(!an.branchy);
        let s2 = slot2(0, 0x10); // 16 % 2 = 0
        // poisons: the value slot + a neighbor (mid-k must not touch act bytes)
        s.meg.memw_value[s2] = 0x5a5a_5a5au32 as i32;
        s.meg.memw_active[0] = false;
        s.meg.memw_active[1] = false;
        s.meg.memr_value[0] = 0xdead_beefu32 as i32; // must survive
        let ms = addr_of_mut!(s.meg);
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        // rig: MS=ARG0, SWP=ARG1, R13=P from [MS+o.p] (:832), memw leg, return.
        let mut a = Assembler::new();
        a.push(RBX);
        a.push(R12);
        a.push(P); // R13 — parked for the leg window (S11 callee-save lesson)
        a.mov64(MS, ARG0);
        a.mov64(SWP, ARG1);
        a.load64(P, Mem::b(MS, o.p)); // :832 — load the accumulator
        emit_memw_acc(&mut a, &o, 0x10, &ops[0x10], s2, &an);
        a.xor32(RAX, RAX);
        a.pop(P);
        a.pop(R12);
        a.pop(RBX);
        a.ret();
        let x = X64Exec::new(&a.code);
        let f: unsafe extern "system" fn(*mut MegState, *const MegSwpDev) =
            unsafe { std::mem::transmute(x.ptr) };
        // p values incl. negatives where truncate-toward-zero differs from `>>15`
        for p in [0i64, 65_536, -65_536, -1, 32_767, -32_769, 0x3_ffff_ffff, -(0x1 << 40)] {
            s.meg.p = p;
            unsafe { f(ms, &dev) };
            assert_eq!(s.meg.memw_value[s2], meg_mem_value(p) as i32, "memw_value at p={p}");
            assert_eq!(s.meg.memw_value[s2], (p / 32768) as i32, "memw TZ-truncates at p={p}");
            // mid-k non-branchy: NO 2-ring act byte touched (:1508 gate)
            assert!(!s.meg.memw_active[0] && !s.meg.memw_active[1], "act byte untouched mid-k (:1508)");
            assert_eq!(s.meg.memr_value[0], 0xdead_beefu32 as i32, "memr slot untouched");
        }
        println!("meg_jit_b2b2c memw exec: value == meg_mem_value (TZ), mid-k act byte suppressed");
    }

    /// emit_index_legs (:1511-1525) against a live device: both the MS-side
    /// `index_value` (direct store :1515) and the §7-B window `ix2_value`
    /// (deref store :1522) land `p >> (15+8)` (arithmetic, meg.rs:893/:899) in
    /// the SAME slot `slot3(d3,k)` the interpreter uses — the window deref
    /// reaches the live `Swp30::meg_ix2_value`, neighbor slots untouched.
    #[test]
    fn b2b2c_index_deref_matches_interpreter() {
        let o = offs();
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        let mut ops = zero_ops();
        ops[0x10].index = 1;
        ops[0x10].index2 = 1;
        let an = analyze_ops(&ops, true); // non-branchy
        let d3 = 0u32;
        let s3 = slot3(d3, 0x10); // 16 % 3 = 1
        // poisons in the two non-target slots on BOTH banks
        for i in 0..3 {
            s.meg.index_value[i] = 0x1111_1111 * (i as i32 + 1);
            s.meg_ix2_value[i] = 0x2222_2222 * (i as i32 + 1);
        }
        let ms = addr_of_mut!(s.meg);
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        let mut a = Assembler::new();
        a.push(RBX);
        a.push(R12);
        a.push(P);
        a.mov64(MS, ARG0);
        a.mov64(SWP, ARG1);
        a.load64(P, Mem::b(MS, o.p)); // :832
        emit_index_legs(&mut a, &o, 0x10, &ops[0x10], s3, &an);
        a.xor32(RAX, RAX);
        a.pop(P);
        a.pop(R12);
        a.pop(RBX);
        a.ret();
        let x = X64Exec::new(&a.code);
        let f: unsafe extern "system" fn(*mut MegState, *const MegSwpDev) =
            unsafe { std::mem::transmute(x.ptr) };
        // arithmetic `>>23` values (meg.rs:893/:899); negatives keep the sign
        let ps: [i64; 9] = [
            0,
            1i64 << 23, // = 1
            -(1i64 << 23), // = -1 (arithmetic)
            (5i64 << 23) + 0x7f_ffff, // low bits truncate → 5
            -((5i64 << 23) + 1), // → -6 (floor), unlike the memw TZ leg
            1i64 << 40,
            -(1i64 << 40),
            0x1_2345_6789i64 << 2,
            -0x1_2345_6789i64 << 2,
        ];
        for p in ps {
            s.meg.p = p;
            let want = (p >> (15 + 8)) as i32; // meg.rs:893/:899 arithmetic
            unsafe { f(ms, &dev) };
            assert_eq!(s.meg.index_value[s3], want, "index_value[s3] at p={p} (:1515)");
            assert_eq!(s.meg_ix2_value[s3], want, "ix2_value[s3] via window at p={p} (:1522)");
            // the §7-B window deref wrote EXACTLY the target slot — neighbors intact
            for i in 0..3 {
                if i != s3 {
                    assert_eq!(
                        s.meg_ix2_value[i],
                        0x2222_2222 * (i as i32 + 1),
                        "ix2 neighbor slot {i} untouched (:1522 slot-scoped)"
                    );
                    assert_eq!(s.meg.index_value[i], 0x1111_1111 * (i as i32 + 1));
                }
            }
        }
        println!("meg_jit_b2b2c index exec: index_value + §7-B ix2 window deref == interpreter (arith >>23)");
    }

    /// B2b-3c MIGRATION of `b2b2c_build_inert`: memw/index-flagged programs
    /// (the exact inputs the new legs consume) now compile AND run — the
    /// memw/index legs execute live through the published buffer.
    #[test]
    fn b2b2c_build_live() {
        assert!(PHASE_B2_EMIT_OK, "B2b-3c: emission is LIVE");
        let mut s = Swp30::new();
        let sintab: Vec<u16> = vec![0; 0x8000];
        let mut j = MegJit::new();
        assert!(j.can(), "enabled — emission is live");
        let mut ops = *s.meg_ops;
        ops[0x10].memw = 1; // memw value leg
        ops[0x10].index = 1; // MS index leg
        ops[0x10].index2 = 1; // §7-B ix2 window leg
        ops[0x20].jump = 1; // branchy ⇒ act bytes every op
        ops[0x20].target = 0x10;
        ops[0x17e].memw = 1; // 2-ring tail act byte
        ops[0x17d].index = 1; // 3-ring tail act byte
        ops[0x17d].index2 = 1;
        s.meg.delay_3 = 2;
        s.meg.delay_2 = 1;
        assert!(j.rebuild(&s.meg, &ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        let (fnp, buf, _sz) = j.gen_code_state();
        assert_ne!(fnp, 0, "brief (migrated): entry published (:1712)");
        assert_ne!(buf, 0, "buffer handoff done (:1696-1712)");
        assert_eq!(j.gen_ring(), (2, 1), "ring snapshot commits at compile (:614-615)");
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        assert!(j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "run() executes the memw/index block");
        println!("meg_jit_b2b2c: memw/index-flagged branchy build LIVE, gen fnp published, run() executes");
    }

    /// B2b-3c MIGRATION of `b2b2b_build_inert`: dm/dr-flagged programs
    /// (the exact inputs the new legs consume) now compile AND run — the
    /// dm/dr/rand legs execute live through the published buffer.
    #[test]
    fn b2b2b_build_live() {
        assert!(PHASE_B2_EMIT_OK, "B2b-3c: emission is LIVE");
        let mut s = Swp30::new();
        let sintab: Vec<u16> = vec![0; 0x8000];
        let mut j = MegJit::new();
        assert!(j.can(), "enabled — emission is live");
        let mut ops = *s.meg_ops;
        ops[0x10].dm = 5; // dm src 5 (rnd leg) + dr p-leg + coalesced draws
        ops[0x10].dm_src = 5;
        ops[0x10].dr = 9;
        ops[0x10].rand_n = 4;
        ops[0x20].jump = 1; // branchy ⇒ dm/dr tail bytes every op
        ops[0x20].target = 0x10;
        ops[0x17d].dm = 6;
        ops[0x17d].dr = 10;
        s.meg.delay_3 = 1;
        s.meg.delay_2 = 0;
        assert!(j.rebuild(&s.meg, &ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        let (fnp, buf, _sz) = j.gen_code_state();
        assert_ne!(fnp, 0, "brief (migrated): entry published (:1712)");
        assert_ne!(buf, 0, "buffer handoff done");
        assert_eq!(j.gen_ring(), (1, 0), "ring snapshot commits at compile (:614-615)");
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        assert!(j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "run() executes the dm/dr/rand block");
        println!("meg_jit_b2b2b: dm/dr-flagged branchy build LIVE, gen fnp published, run() executes");
    }

    // -----------------------------------------------------------------------
    // B2b-2d — EXEC rigs for the t/tval legs (swp30_jit.cpp:1530-1561
    // folded, :1647-1675 branchy twin). Standalone stubs only — never a
    // half-emitted program (brief). Ground truths are the interpreter legs
    // themselves: `((p >> 8) & 0x7fff) as i16` (meg.rs:4129, index form)
    // and `s16_p23_clamped` = `(p >> 23).clamp(-0x8000, 0x7fff) as i16`
    // (meg.rs:617-618 = :4027/:4130, clamp form — private, so recomputed
    // independently here). The p accumulator rides the MS offset
    // (prologue :832 shape); the jump-leg p rides ARG2 (extra stub arg).
    // -----------------------------------------------------------------------

    /// Folded index form (:1543-1544) executed: `t_value[slot2(k)]` gets
    /// `(p>>8)&0x7fff` (the interpreter index tap, meg.rs:4129) — and the
    /// `>>23` clamp leg would give a DIFFERENT value on the same inputs
    /// (non-negative 15-bit vs signed clamp: the shapes are provably
    /// distinct, not aliases). The t write still reads the OLD slot value
    /// first (:1533 before :1560, interpreter order meg.rs:1430). Both d2
    /// ring phases compile (slot alternates), neighbor slot stays poison.
    #[test]
    fn b2b2d_t_exec_index_form() {
        let o = offs();
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        let poisons: [i16; 2] = [0x1111, -0x2222];
        let mut ops = zero_ops();
        ops[0x22].t_write = 1;
        ops[0x22].t_from_p = 1; // :664 k+2 rule ⇒ need_tval[0x20]
        let an = analyze_ops(&ops, true);
        assert!(an.need_tval[0x20] && !an.branchy, "preconditions");
        let op_ix = Op { t_write: 1, t_from_p: 1, t: 3, index: 1, ..Op::ZERO };
        let ms = addr_of_mut!(s.meg);
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        // two compilations — one per d2 ring phase (s2 alternates 0/1)
        for dd2 in 0..2u32 {
            let s2 = slot2(dd2, 0x20);
            let mut a = Assembler::new();
            a.push(RBX);
            a.push(R12);
            a.push(P);
            a.mov64(MS, ARG0);
            a.mov64(SWP, ARG1);
            a.load64(P, Mem::b(MS, o.p)); // :832
            emit_t_leg(&mut a, &o, &s.meg, 0x20, &op_ix, s2, false, &an);
            a.pop(P);
            a.pop(R12);
            a.pop(RBX);
            a.ret();
            let x = X64Exec::new(&a.code);
            let f: unsafe extern "system" fn(*mut MegState, *const MegSwpDev) =
                unsafe { std::mem::transmute(x.ptr) };
            for p in [
                0i64,
                1 << 8,
                -256i64, // arithmetic >>8 = -1, &0x7fff ⇒ 0x7fff
                -1i64,  // >>8 = -1 ⇒ 0x7fff
                (1i64 << 31) - 1,
                -(1i64 << 23),
                0x1234_5678i64 << 8,
                -(0x1234_5678i64 << 8),
            ] {
                s.meg.p = p;
                s.meg.t_value = poisons;
                s.meg.t[3] = -12345;
                unsafe { f(ms, &dev) };
                let want = ((p >> 8) & 0x7fff) as i16; // meg.rs:4129
                assert_eq!(s.meg.t_value[s2], want, "index publish at p={p} s2={s2} (:1543-1544)");
                assert_eq!(s.meg.t[3], poisons[s2], "t read the OLD slot before publish (:1533/:1538)");
                assert_eq!(s.meg.t_value[1 - s2], poisons[1 - s2], "neighbor t_value slot intact");
                let clamp_want = (p >> 23).clamp(-0x8000, 0x7fff) as i16;
                if want != clamp_want {
                    assert_ne!(s.meg.t_value[s2], clamp_want, "index form is NOT the clamp leg (p={p})");
                }
            }
        }
        println!("meg_jit_b2b2d index exec: (p>>8)&0x7fff publish == interpreter, old-slot t read, neighbors intact");
    }

    /// Folded clamp form (:1546-1557) executed — the p band is built as
    /// `(shr << 23) | low3` so every case is designed around its `>>23`
    /// result: +0x8000 clamps DOWN to 0x7fff, −0x8001 clamps UP to −0x8000,
    /// the exact 0x7fff / −0x8000 boundaries pass through unchanged (strict
    /// cmovl/cmovg — s16_p23_clamped meg.rs:618), and an in-band value is
    /// untouched. On the clamped cases the emitted byte is provably NOT the
    /// `>>8` index tap (whose low-15 bits are zero for the `<<23` inputs).
    #[test]
    fn b2b2d_t_exec_clamp_form() {
        let o = offs();
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        let ops = zero_ops(); // k=0x17e publishes via the TAIL rule (:662-663)
        let an = analyze_ops(&ops, true);
        assert!(an.need_tval[0x17e], "tail publishes (:662-663)");
        let s2 = slot2(0, 0x17e); // = 0
        let k = 0x17eu32;
        let op_ni = Op { t_write: 1, t_from_p: 1, t: 3, ..Op::ZERO };
        let ms = addr_of_mut!(s.meg);
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        let mut a = Assembler::new();
        a.push(RBX);
        a.push(R12);
        a.push(P);
        a.mov64(MS, ARG0);
        a.mov64(SWP, ARG1);
        a.load64(P, Mem::b(MS, o.p)); // :832
        emit_t_leg(&mut a, &o, &s.meg, k, &op_ni, s2, false, &an);
        a.pop(P);
        a.pop(R12);
        a.pop(RBX);
        a.ret();
        let x = X64Exec::new(&a.code);
        let f: unsafe extern "system" fn(*mut MegState, *const MegSwpDev) =
            unsafe { std::mem::transmute(x.ptr) };
        let clamps: [i64; 8] = [
            0x8000i64 << 23, // >>23 = 0x8000   ⇒ clamps to 0x7fff
            0x8001i64 << 23, // >>23 = 0x8001   ⇒ clamps to 0x7fff
            -0x8001i64 << 23, // >>23 = -0x8001 ⇒ clamps to -0x8000
            -0x8002i64 << 23, // >>23 = -0x8002 ⇒ clamps to -0x8000
            0x7fffffi64 << 23, // boundary: stays 0x7fff (cmovg strict)
            -0x8000i64 << 23, // boundary: stays -0x8000 (cmovl strict)
            (0x8001i64 << 23) + 1, // >>23 = 0x8001 ⇒ clamps, low bit irrelevant
            -(0x7ffdi64 << 23) - 0x7f_ffff, // >>23 = -0x7ffe ⇒ in band, passes through
        ];
        for p in clamps {
            s.meg.p = p;
            s.meg.t_value = [0x3a3a, 0x4b4b];
            s.meg.t[3] = -12345;
            unsafe { f(ms, &dev) };
            let shr = p >> 23;
            let want = shr.clamp(-0x8000, 0x7fff) as i16; // s16_p23_clamped
            let got = s.meg.t_value[s2];
            assert_eq!(got, want, "clamp publish at p={p} (>>23={shr})");
            assert_eq!(s.meg.t[3], 0x3a3a, "t read the OLD slot value (:1533)");
            assert_eq!(s.meg.t_value[1 - s2], 0x4b4b, "neighbor intact");
            if shr < -0x8000 || shr > 0x7fff {
                assert!(got == 0x7fff || got == -0x8000, "clamped value {got:#x} at {shr}");
                // the index tap would yield 0 here (low bits of a <<23 input)
                assert_ne!(got, ((p >> 8) & 0x7fff) as i16, "clamp leg is NOT the index tap (p={p})");
            }
        }
        println!("meg_jit_b2b2d clamp exec: ±0x8000 edges clamp, boundaries pass through (== s16_p23_clamped)");
    }

    /// Branchy twin (:1647-1675) executed against the SAME live state as
    /// the folded clamp leg: same jump t-input read, same published clamp
    /// value for a fixed t input — plus the five ring-byte erasers (ix2_act
    /// through the §7-B window reaches the live `meg_ix2_act`). Two rigs,
    /// identical p/t inputs; and the index-op quirk pinned: the branchy
    /// publish stays the CLAMP form even for index ops (interpreter jump
    /// path meg.rs:1296/:4027), unlike the folded leg's `>>8&0x7fff`.
    #[test]
    fn b2b2d_branchy_exec_twin() {
        let o = offs();
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        let mut ops = zero_ops();
        ops[0x20].jump = 1;
        ops[0x20].target = 0x10;
        ops[0x22].t_write = 1; // :664 ⇒ need_tval[0x20] (both rigs publish)
        ops[0x22].t_from_p = 1;
        let an = analyze_ops(&ops, true);
        assert!(an.branchy && an.need_tval[0x20], "preconditions");
        let k = 0x20u32;
        let s2 = slot2(0, k);
        let s3 = slot3(0, k);
        let op_ni = Op { t_write: 1, t_from_p: 1, t: 3, ..Op::ZERO };
        let opj = Op { jump: 1, t_write: 1, t_from_p: 1, t: 3, ..Op::ZERO };
        let op_ix = Op { jump: 1, t_write: 1, t_from_p: 1, t: 3, index: 1, ..Op::ZERO };
        let ms = addr_of_mut!(s.meg);
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        // rig A — folded clamp leg: p through [MS+o.p] (:832 shape)
        let xa = X64Exec::new(&rig(|a| {
            a.push(P);
            a.load64(P, Mem::b(MS, o.p)); // P = s.meg.p
            emit_t_leg(a, &o, &s.meg, k, &op_ni, s2, false, &an);
            a.pop(P);
        }));
        let fa: unsafe extern "system" fn(*mut MegState, *const MegSwpDev) =
            unsafe { std::mem::transmute(xa.ptr) };
        // rig B — branchy twin: p through the spare ARG2 register (R8)
        let xb = X64Exec::new(&rig(|a| {
            a.push(P);
            a.mov64(P, ARG2);
            emit_t_branchy(a, &o, k, &opj, s2, s3, &an);
            a.pop(P);
        }));
        let fb: unsafe extern "system" fn(*mut MegState, *const MegSwpDev, i64) =
            unsafe { std::mem::transmute(xb.ptr) };
        for p in [
            0i64,
            5i64 << 23, // in band ⇒ 5
            0x8000i64 << 23, // clamps to 0x7fff
            -(0x8001i64 << 23), // clamps to -0x8000
            -256i64, // >>23 = -1 ⇒ -1
            (0x7ffdi64 << 23) + 1, // truncates toward -inf ⇒ 0x7ffd
        ] {
            s.meg.p = p;
            s.meg.t_value = [0x5c5c, -0x6d6d];
            s.meg.t[3] = 1234;
            unsafe { fa(ms, &dev) };
            let folded_t = s.meg.t[3];
            let folded_tv = s.meg.t_value[s2];
            assert_eq!(folded_t, 0x5c5c, "folded read old slot (p={p})");
            // re-arm the identical t input; branchy also poisons the rings
            s.meg.t[3] = 1234;
            s.meg.t_value[s2] = 0x5c5c;
            s.meg.mw_reg[s3] = 9;
            s.meg.rw_reg[s3] = 9;
            s.meg.memw_active[s2] = true;
            s.meg.index_active[s3] = true;
            s.meg_ix2_act[s3] = 1;
            unsafe { fb(ms, &dev, p) };
            assert_eq!(s.meg.t[3], folded_t, "twin t input == folded (p={p}) (:1650/:1653)");
            assert_eq!(s.meg.t_value[s2], folded_tv, "twin publish == folded clamp leg (p={p})");
            assert_eq!(s.meg.t_value[s2], (p >> 23).clamp(-0x8000, 0x7fff) as i16, "clamp == s16_p23_clamped (p={p})");
            assert_eq!(s.meg.t_value[1 - s2], [0x5c5c, -0x6d6d][1 - s2], "neighbor intact");
            // the five erasers (:1655-1659) — ix2_act through the §7-B window
            assert_eq!(s.meg.mw_reg[s3], 0, "mw_reg slot erased (:1655)");
            assert_eq!(s.meg.rw_reg[s3], 0, "rw_reg slot erased (:1656)");
            assert!(!s.meg.memw_active[s2], "memw_act 2-ring erased (:1657)");
            assert!(!s.meg.index_active[s3], "ix_act slot erased (:1658)");
            assert_eq!(s.meg_ix2_act[s3], 0, "ix2_act live byte erased via window (:1659)");
        }
        // index-op quirk: same k/op.index, branchy publish stays the CLAMP
        // form (interpreter jump path meg.rs:1296 = :4027), even though the
        // folded leg would tap `(p>>8)&0x7fff`.
        let xix = X64Exec::new(&rig(|a| {
            a.push(P);
            a.mov64(P, ARG2);
            emit_t_branchy(a, &o, k, &op_ix, s2, s3, &an);
            a.pop(P);
        }));
        let fix: unsafe extern "system" fn(*mut MegState, *const MegSwpDev, i64) =
            unsafe { std::mem::transmute(xix.ptr) };
        let p = 5i64 << 23;
        s.meg.p = p;
        s.meg.t_value = [0x7e7e, -0x1f1f];
        s.meg.t[3] = 1234;
        unsafe { fix(ms, &dev, p) };
        assert_eq!(s.meg.t_value[s2], 5, "branchy index-op still clamps (:1662, NOT >>8&0x7fff)");
        assert_eq!(s.meg.t[3], 0x7e7e, "old slot read first (:1650)");
        println!("meg_jit_b2b2d branchy exec: twin == folded clamp, 5 ring erasers, jump/ix2 legs pinned");
    }

    /// B2b-3c MIGRATION of `b2b2d_build_inert`: t/tval-flagged programs
    /// (the exact inputs the new legs consume) now compile AND run — the
    /// folded + branchy t legs and clamp publishes execute live.
    #[test]
    fn b2b2d_build_live() {
        assert!(PHASE_B2_EMIT_OK, "B2b-3c: emission is LIVE");
        let mut s = Swp30::new();
        let sintab: Vec<u16> = vec![0; 0x8000];
        let mut j = MegJit::new();
        assert!(j.can(), "enabled — emission is live");
        let mut ops = *s.meg_ops;
        ops[0x10].t_write = 1; // folded t write (:1531)
        ops[0x10].t_from_p = 1;
        ops[0x10].index = 1; // index-form publish (:1542-1544)
        ops[0x12].t_write = 1; // k+2 t-from-p ⇒ need_tval[0x10] (:664)
        ops[0x12].t_from_p = 1;
        ops[0x22].t_write = 1; // ⇒ need_tval[0x20] so the jump position publishes
        ops[0x22].t_from_p = 1;
        ops[0x20].jump = 1; // branchy twin inputs (:1641+)
        ops[0x20].target = 0x10;
        ops[0x20].t_write = 1; // jump-op t write (:1647)
        ops[0x20].t_from_p = 1;
        ops[0x17e].t_write = 1; // tail publish (:662-663)
        ops[0x17e].t_from_p = 1;
        let an = analyze_ops(&ops, true);
        assert!(an.branchy && an.need_tval[0x10] && an.need_tval[0x20] && an.need_tval[0x17e], "preconditions");
        s.meg.delay_3 = 2;
        s.meg.delay_2 = 1;
        assert!(j.rebuild(&s.meg, &ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        let (fnp, buf, _sz) = j.gen_code_state();
        assert_ne!(fnp, 0, "brief (migrated): entry published (:1712)");
        assert_ne!(buf, 0, "buffer handoff done (:1696-1712)");
        assert_eq!(j.gen_ring(), (2, 1), "ring snapshot commits at compile (:614-615)");
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        assert!(j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "run() executes the t/tval block");
        println!("meg_jit_b2b2d: t/tval-flagged branchy build LIVE, gen fnp published, run() executes");
    }

    // -----------------------------------------------------------------------
    // B2b-2e — EXEC rigs for the memop address leg (swp30_jit.cpp:1566-1638).
    // Standalone stubs only — never a half-emitted program (brief). The stub
    // parks the THREE runtime bases of this leg: MS=ARG0, the §7-B window
    // SWP=ARG1 and the reverb RAM RAM=ARG2 (fn_t :266/:831 — RAM is per-call,
    // never baked). RBX/R12/R15 preserved (callee-saved, S11 lesson);
    // RAX/RCX/RDX/R8 are the legs' own scratch (:1572/:1576/:1623).
    // Ground truth: the PAIRED interpreter arithmetic meg.rs:1446-1490 —
    // u32 wrapping (offset + ix + ix2 − SC, &addr_mask, +addr_base, &0x3ffff)
    // for the mapped leg, the map/SC-free absolute leg for mem_table, and
    // MegState::revram_encode/decode (meg.rs:460/:479) on the RAM side.
    // -----------------------------------------------------------------------

    /// rig() + the RAM register (R15) parked from ARG2 — the memop legs index
    /// scale-2 through it (:1582/:1626/:1630) — plus the SC register (R14)
    /// loaded through the MS offset exactly like the prologue :833 (the −SC
    /// leg :1606 reads it; leaving it at the caller's garbage value would
    /// test the subtraction against noise). RBX/R12/R14/R15 all preserved.
    fn rig3(body: impl Fn(&mut Assembler)) -> Vec<u8> {
        let o = offs();
        let mut a = Assembler::new();
        a.push(RBX);
        a.push(R12);
        a.push(R14);
        a.push(RAM);
        a.mov64(MS, ARG0);
        a.mov64(SWP, ARG1);
        a.mov64(RAM, ARG2); // :831
        a.load32(SC, Mem::b(MS, o.sample)); // :833
        body(&mut a);
        a.xor32(RAX, RAX);
        a.pop(RAM);
        a.pop(R14);
        a.pop(R12);
        a.pop(RBX);
        a.ret();
        a.code
    }

    /// The mapped address leg (:1593-1612) executed — the computed RAM
    /// address equals the interpreter `(off+ix+ix2−SC [+1] & mask)+base
    /// & 0x3ffff` u32 math (meg.rs:1468-1483) for both read memops (2 and 3
    /// — the +1 arm) across offset/ix/ix2 combos INCLUDING the negative −SC
    /// wrap (offset 0 − SC 1 → u32 0xffff_ffff, mask-clipped; and the
    /// 0xffff+0x3ffff+0x3ffff−0x7fffffff ride-through). The decoded word
    /// lands in memr_value[slot2] only — neighbors and RAM untouched.
    #[test]
    fn b2b2e_addr_exec_matches_interpreter() {
        let o = offs();
        let mut s = Swp30::new();
        s.reverb_ram.resize(0x40000, 0);
        for i in 0..s.reverb_ram.len() {
            s.reverb_ram[i] = (i as u16).wrapping_mul(7).wrapping_add(0x1234);
        }
        let sintab: Vec<u16> = Vec::new();
        let an = analyze_ops(&zero_ops(), true);
        let ms = addr_of_mut!(s.meg);
        s.revram_enable = 0; // every region ENABLED (BIT set = disabled — meg.rs:1462)
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        let ramptr = seam.reverb_ram.as_mut_ptr();
        let k = 0x20u32;
        let s2 = slot2(0, k);
        let base_op = Op {
            memop: 2,
            offset_index: 7,
            mem_use_index: 1,
            mem_use_index2: 1,
            addr_mask: 0x1fff,
            addr_base: 0x800,
            region: 3,
            ..Op::ZERO
        };
        // (offset, ix, ix2, SC) — last three combos exercise u32 wraps
        let combos = [
            (0x1234u16, 0i32, 0i32, 0u32),
            (0x1234, 0x3ff, -0x200, 0),
            (0, 0, 0, 1), // −SC wrap: 0−1 = 0xffff_ffff → &mask = 0x1fff
            (0xffff, 0x3ffff, 0x3ffff, 0x7fff_ffff),
            (0x2000, -0x1_0000, 0x222, 0x8000),
        ];
        for memop3 in [false, true] {
            let mut op = base_op;
            if memop3 {
                op.memop = 3; // the :1608-1609 +1 arm
            }
            let code = rig3(|a| emit_memop(a, &o, k, &op, s2, 0, &an));
            let x = X64Exec::new(&code);
            let f: unsafe extern "system" fn(*mut MegState, *const MegSwpDev, *mut u16) =
                unsafe { std::mem::transmute(x.ptr) };
            for (off, ix, ix2, sc) in combos {
                s.meg.offset[7] = off;
                s.meg.ram_index = ix;
                *seam.ram_index2 = ix2; // through the seam borrow (live slot)
                s.meg.sample_counter = sc;
                // interpreter leg, meg.rs:1468-1483 verbatim:
                let mut offw = (off as u32)
                    .wrapping_add(if op.mem_use_index != 0 { ix as u32 } else { 0 })
                    .wrapping_add(if op.mem_use_index2 != 0 { ix2 as u32 } else { 0 })
                    .wrapping_sub(sc);
                if op.memop == 3 {
                    offw = offw.wrapping_add(1);
                }
                let addr = ((offw & op.addr_mask).wrapping_add(op.addr_base)) & 0x3_ffff;
                let want = MegState::revram_decode(seam.reverb_ram[addr as usize]) as i32;
                for i in 0..3 {
                    s.meg.memr_value[i] = if i == s2 { -1 } else { 0x5a5a_0000 + i as i32 };
                }
                unsafe { f(ms, &dev, ramptr) };
                assert_eq!(s.meg.memr_value[s2], want, "decoded read at addr {addr:#x} (memop {})", op.memop);
                for i in 0..3 {
                    if i != s2 {
                        assert_eq!(s.meg.memr_value[i], 0x5a5a_0000 + i as i32, "memr neighbor {i}");
                    }
                }
                assert_eq!(seam.reverb_ram[addr as usize], {
                    let a = addr as usize;
                    (a as u16).wrapping_mul(7).wrapping_add(0x1234)
                }, "read never writes RAM (:1630)");
            }
        }
        println!("meg_jit_b2b2e addr exec: (off+ix+ix2−SC[+1]&mask)+base&0x3ffff == interpreter incl −SC wraps");
    }

    /// The scale-2 WRITE leg (:1623-1626) executed — RAM[address] lands
    /// EXACTLY `MegState::revram_encode(ram_write)` (the PAIRED meg.rs:1485
    /// truth; R8 holds the address across the encode, RENC=R11 — the SIB
    /// byte 0x47 pins index=R8), then a second stub reads the word back
    /// through the decode leg (:1630-1632): round-trip == decode(encode(w)),
    /// quantization and all, NEVER the raw input.
    #[test]
    fn b2b2e_revram_roundtrip_exec() {
        let o = offs();
        let mut s = Swp30::new();
        s.reverb_ram.resize(0x40000, 0);
        let sintab: Vec<u16> = Vec::new();
        let an = analyze_ops(&zero_ops(), true);
        let ms = addr_of_mut!(s.meg);
        s.revram_enable = 0;
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        let ramptr = seam.reverb_ram.as_mut_ptr();
        let k = 0x20u32;
        let s2 = slot2(0, k);
        let wop = Op { memop: 1, offset_index: 3, mem_use_index: 1, addr_mask: 0xfff, addr_base: 0x2000, region: 5, ..Op::ZERO };
        let rop = Op { memop: 2, ..wop };
        let wcode = rig3(|a| emit_memop(a, &o, k, &wop, s2, 0, &an));
        let rcode = rig3(|a| emit_memop(a, &o, k, &rop, s2, 0, &an));
        let xw = X64Exec::new(&wcode);
        let xr = X64Exec::new(&rcode);
        let fw: unsafe extern "system" fn(*mut MegState, *const MegSwpDev, *mut u16) =
            unsafe { std::mem::transmute(xw.ptr) };
        let fr: unsafe extern "system" fn(*mut MegState, *const MegSwpDev, *mut u16) =
            unsafe { std::mem::transmute(xr.ptr) };
        // (ram_write, offset, ix, SC): the last combo −SC-wraps the address
        let combos = [
            (0u32, 0x10u16, 0i32, 0u32),
            (0x7ff_ffff, 0x10, 0, 0),
            (0x1234_5678, 0x10, 0x20, 0x10),
            (0xffff_ffff, 0x0, 0, 0x1), // addr = (−1 & 0xfff)+0x2000 = 0x2fff
            (0x8000_0001, 0x333, -0x111, 0),
        ];
        for (wv, off, ix, sc) in combos {
            s.meg.offset[3] = off;
            s.meg.ram_index = ix;
            s.meg.sample_counter = sc;
            s.meg.ram_write = wv;
            // interpreter leg (meg.rs:1468-1483; memop 1 carries no +1):
            let addr = (((off as u32).wrapping_add(ix as u32).wrapping_sub(sc))
                & wop.addr_mask)
                .wrapping_add(wop.addr_base)
                & 0x3_ffff;
            let enc = MegState::revram_encode(wv);
            seam.reverb_ram[addr as usize] = 0xdead; // poison the target
            unsafe { fw(ms, &dev, ramptr) };
            assert_eq!(seam.reverb_ram[addr as usize], enc, "encode leg wrote RAM[{addr:#x}] (:1626)");
            s.meg.memr_value[s2] = 0x7fff_0001;
            unsafe { fr(ms, &dev, ramptr) };
            let dec = MegState::revram_decode(enc) as i32;
            assert_eq!(s.meg.memr_value[s2], dec, "decode leg round-trip at RAM[{addr:#x}] (:1630-1632)");
            if dec != wv as i32 {
                assert_ne!(s.meg.memr_value[s2], wv as i32, "round-trip is the QUANTIZED value, not the input");
            }
        }
        println!("meg_jit_b2b2e revram exec: scale-2 encode store + decode load round-trip == meg.rs ground truth");
    }

    /// The compile-time region gate (:1589) executed with the region
    /// DISABLED (BIT set — meg.rs:1462 polarity): the emitted READ is only
    /// the inert 0-store (memr_value ← 0 against an all-0xdead RAM that
    /// would decode non-zero; rig byte budget = harness + 12 exactly) and
    /// the emitted WRITE is NOTHING at all (empty body — the RAM snapshot
    /// compares equal, the untouched memr slot proves no stray store).
    #[test]
    fn b2b2e_disabled_region_exec_is_inert() {
        let o = offs();
        let mut s = Swp30::new();
        s.reverb_ram.resize(0x40000, 0);
        // FULL poison (resize only pads past the old len — Swp30::new may
        // already carry a zero-filled map): every word decodes non-zero.
        for w in s.reverb_ram.iter_mut() {
            *w = 0xdead;
        }
        let sintab: Vec<u16> = Vec::new();
        let an = analyze_ops(&zero_ops(), true);
        let ms = addr_of_mut!(s.meg);
        s.revram_enable = 1 << 2; // region 2 DISABLED
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        let ramptr = seam.reverb_ram.as_mut_ptr();
        let k = 0x20u32;
        let s2 = slot2(0, k);
        let op = Op { memop: 2, offset_index: 7, mem_use_index: 1, addr_mask: 0x1fff, addr_base: 0x800, region: 2, ..Op::ZERO };
        let harness = rig3(|_a| {});
        // READ (:1590-1591): +12 bytes over the harness, and ONLY the 0-store
        let rcode = rig3(|a| emit_memop(a, &o, k, &op, s2, s.revram_enable, &an));
        assert_eq!(rcode.len(), harness.len() + 10, "inert read = 10 B (:1591)");
        let x = X64Exec::new(&rcode);
        let f: unsafe extern "system" fn(*mut MegState, *const MegSwpDev, *mut u16) =
            unsafe { std::mem::transmute(x.ptr) };
        s.meg.offset[7] = 0x1234;
        s.meg.ram_index = 0x40;
        s.meg.sample_counter = 0;
        s.meg.memr_value[s2] = 0x7fff_0001; // non-zero ⇒ 0 proves the leg ran
        unsafe { f(ms, &dev, ramptr) };
        assert_eq!(s.meg.memr_value[s2], 0, "disabled read FORCED 0 (:1591 == meg.rs:1464)");
        assert!(seam.reverb_ram.iter().all(|&w| w == 0xdead), "RAM untouched by the disabled read (:1590-1591)");
        // WRITE (:1589 else-if false): the body is EMPTY — rig stays bare
        let wcode = rig3(|a| {
            let wop = Op { memop: 1, ..op };
            emit_memop(a, &o, k, &wop, s2, s.revram_enable, &an);
        });
        assert_eq!(wcode.len(), harness.len(), "disabled write emits NOTHING (:1589)");
        let xw = X64Exec::new(&wcode);
        let fw: unsafe extern "system" fn(*mut MegState, *const MegSwpDev, *mut u16) =
            unsafe { std::mem::transmute(xw.ptr) };
        s.meg.ram_write = 0x1234_5678;
        let before: Vec<u16> = seam.reverb_ram.to_vec();
        s.meg.memr_value[s2] = 0x55aa_55aa;
        unsafe { fw(ms, &dev, ramptr) };
        assert_eq!(seam.reverb_ram.to_vec(), before, "dropped write touched no RAM (:1588 書き込みは落ちる)");
        assert_eq!(s.meg.memr_value[s2], 0x55aa_55aa, "disabled write stored nothing anywhere (:1589)");
        println!("meg_jit_b2b2e region gate exec: disabled read→forced 0 / write→dropped, RAM pristine");
    }

    /// The mem_table leg (:1568-1586) executed with the region DISABLED —
    /// the absolute read BYPASSES the gate exactly like the interpreter's
    /// :1446-first test (meg.rs:1446-1459): map-free and SC-free, an
    /// impossible-but-benign sample_counter of 0x7fff_ffff would skew the
    /// address if SC were subtracted; the jmp (:1585) skips the disabled
    /// gate's 0-store, so the decoded word — not zero — lands.
    #[test]
    fn b2b2e_table_read_bypasses_region_gate_exec() {
        let o = offs();
        let mut s = Swp30::new();
        s.reverb_ram.resize(0x40000, 0);
        for i in 0..s.reverb_ram.len() {
            s.reverb_ram[i] = (i as u16).wrapping_mul(0x35).wrapping_add(0x77);
        }
        let sintab: Vec<u16> = Vec::new();
        let an = analyze_ops(&zero_ops(), true);
        let ms = addr_of_mut!(s.meg);
        s.revram_enable = 1 << 2; // region DISABLED — the table must not care
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam);
        let ramptr = seam.reverb_ram.as_mut_ptr();
        let k = 0x20u32;
        let s2 = slot2(0, k);
        let op = Op {
            memop: 3,
            mem_table: 1,
            offset_index: 5,
            mem_use_index: 1,
            mem_use_index2: 1,
            region: 2,
            ..Op::ZERO
        };
        let code = rig3(|a| emit_memop(a, &o, k, &op, s2, s.revram_enable, &an));
        let x = X64Exec::new(&code);
        let f: unsafe extern "system" fn(*mut MegState, *const MegSwpDev, *mut u16) =
            unsafe { std::mem::transmute(x.ptr) };
        // absolute (u32 wrap) address: no mask/base, NO −SC (:1579-1581)
        for (off, ix, ix2) in [(0x30u16, 0x11i32, 0i32), (0x1234, -0x222, 0x333), (0x3ff0, 0x20, -0x10)] {
            s.meg.offset[5] = off;
            s.meg.ram_index = ix;
            *seam.ram_index2 = ix2; // through the seam borrow (live slot)
            s.meg.sample_counter = 0x7fff_ffff; // ignored — proof SC is not subtracted
            let addr = (off as u32)
                .wrapping_add(ix as u32)
                .wrapping_add(ix2 as u32)
                .wrapping_add(1) // memop 3 (:1580)
                & 0x3_ffff;
            let want = MegState::revram_decode(seam.reverb_ram[addr as usize]) as i32;
            s.meg.memr_value[s2] = 0x1234_5678;
            unsafe { f(ms, &dev, ramptr) };
            assert_eq!(s.meg.memr_value[s2], want, "table absolute read at {addr:#x} (:1568-1584)");
        }
        println!("meg_jit_b2b2e table exec: absolute read (no map, no SC) rides past the disabled gate via the :1585 jmp");
    }

    /// B2b-3c MIGRATION of `b2b2e_build_inert`: memop/mem_table/region-flagged
    /// programs (half the region map disabled so the :1589 gate branches
    /// ride the compile) now compile AND run — the address leg, scale-2 RAM
    /// stores and the disabled-region inert legs execute live.
    #[test]
    fn b2b2e_build_live() {
        assert!(PHASE_B2_EMIT_OK, "B2b-3c: emission is LIVE");
        let mut s = Swp30::new();
        let sintab: Vec<u16> = vec![0; 0x8000];
        let mut j = MegJit::new();
        assert!(j.can(), "enabled — emission is live");
        let mut ops = *s.meg_ops;
        ops[0x10].memop = 2; // normal read leg
        ops[0x10].addr_mask = 0x3fff;
        ops[0x10].addr_base = 0x400;
        ops[0x20].memop = 1; // write leg (scale-2 store)
        ops[0x30].memop = 3; // +1 arm
        ops[0x30].mem_table = 1; // absolute table leg
        ops[0x40].jump = 1; // branchy ⇒ act byte every op
        ops[0x40].target = 0x10;
        ops[0x17e].memop = 2; // 2-ring tail act byte
        s.revram_enable = 0x55; // compile-time gate mixed across ops (BIT=disabled)
        s.meg.delay_3 = 2;
        s.meg.delay_2 = 1;
        assert!(j.rebuild(&s.meg, &ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        let (fnp, buf, _sz) = j.gen_code_state();
        assert_ne!(fnp, 0, "brief (migrated): entry published (:1712)");
        assert_ne!(buf, 0, "buffer handoff done (:1696-1712)");
        assert_eq!(j.gen_ring(), (2, 1), "ring snapshot commits at compile (:614-615)");
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        assert!(j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "run() executes the memop/region block");
        println!("meg_jit_b2b2e: memop-flagged branchy build LIVE, gen fnp published, run() executes");
    }

    // -----------------------------------------------------------------------
    // B2b-3a — LFO hoist head + callout (swp30_jit.cpp:897-969 emit_lfo,
    // :1431-1448 dm_src 0-3 callout + load_p_limits :1448). Emitted bytes run
    // STANDALONE (never a half-emitted program; build() still refuses at the
    // PHASE_B2_EMIT_OK gate). Ground truth is the PAIRED `meg::get_lfo`
    // (meg.rs:379) — emitted `emit_lfo` AND the `meg_jit_call_lfo` callout must
    // both equal it, byte-for-byte on the value.
    // -----------------------------------------------------------------------

    /// emit_lfo head shape (:902-904): the phase load `mov eax,[rbx+o_lfo_counter]`,
    /// `shr eax,5`, then the lfo-word load `movzx edx,word [rbx+o_lfo]`. Fixed
    /// disp32 lengths (rm() always disp32) so the future hoist jump-patching is
    /// length-stable.
    #[test]
    fn b2b3a_lfo_head_byte_pins() {
        let o = offs();
        let idx = 5u32;
        let fake: *const u16 = usize::MAX as *const u16;
        let mut a = Assembler::new();
        emit_lfo(&mut a, &o, idx, fake);
        let c = &a.code;
        // :902 load32 rax, [rbx + o_lfo_counter + 4*idx]  — 8b 83 disp32
        let mut want: Vec<u8> = vec![0x8b, 0x83];
        want.extend_from_slice(&(o.lfo_counter + 4 * idx as i32).to_le_bytes());
        // :903 shr eax,5 — c1 e8 05
        want.extend_from_slice(&[0xc1, 0xe8, 0x05]);
        // :904 loadu16 edx, [rbx + o_lfo + 2*idx] — 0f b7 93 disp32
        want.extend_from_slice(&[0x0f, 0xb7, 0x93]);
        want.extend_from_slice(&(o.lfo + 2 * idx as i32).to_le_bytes());
        assert_eq!(&c[..want.len()], want.as_slice(), "emit_lfo head (:902-904)");
        // the pitch-shift `shl eax,cl` (d3 e0, :908) and the final `shl eax,7`
        // (c1 e0 07, :968) both appear and the leg ends on the :968 shift.
        assert!(c.windows(2).any(|w| w == [0xd3, 0xe0]), "shl eax,cl (:908) present");
        assert_eq!(&c[c.len() - 3..], &[0xc1, 0xe0, 0x07], "ends shl eax,7 (:968)");
        println!("meg_jit_b2b3a emit_lfo head: phase/depth loads + shl/shl7 pinned");
    }

    /// emit_call_lfo (:1441-1448) full byte pin: ARG0=MS mov, ARG1 imm32 lfo,
    /// ARG2/ARG3 baked sintab/len (Rust §3 deviation — R8/R9), the
    /// movabs+call `call_abs` (:1443) to meg_jit_call_lfo, then the MANDATORY
    /// load_p_limits (:1448) reloading R9=R9 imm / R10 imm. No push/pop —
    /// SEED/K_MAX are Win64 callee-saved the trampoline preserves (:1434-1436).
    #[test]
    fn b2b3a_callout_byte_pins() {
        let lfo = 7u32;
        let sintab = 0x1234_5678usize as *const u16;
        let len = 0x8000usize;
        let tramp: unsafe extern "system" fn(*const MegState, u32, *const u16, usize) -> u32 =
            meg_jit_call_lfo;
        let tramp = tramp as *const () as u64;

        let mut a = Assembler::new();
        emit_call_lfo(&mut a, lfo, sintab, len);
        let mut want: Vec<u8> = Vec::new();
        want.extend_from_slice(&[0x48, 0x8b, 0xcb]); // :1441 mov rcx,rbx (ARG0=MS)
        want.push(0xba); // :1442 mov edx,imm32 (ARG1=RDX)
        want.extend_from_slice(&lfo.to_le_bytes());
        want.extend_from_slice(&[0x49, 0xb8]); // ARG2=R8 movabs sintab (§3)
        want.extend_from_slice(&(sintab as u64).to_le_bytes());
        want.extend_from_slice(&[0x49, 0xb9]); // ARG3=R9 movabs len (§3)
        want.extend_from_slice(&(len as u64).to_le_bytes());
        want.extend_from_slice(&[0x48, 0xb8]); // :1443 movabs rax, tramp
        want.extend_from_slice(&tramp.to_le_bytes());
        want.extend_from_slice(&[0xff, 0xd0]); // call rax
        // load_p_limits (:1448): R9 = 0x3fffffffff, R10 = -0x4000000000 —
        // B2b-3b FIX: disk :821-822 verbatim (2^38−1 / −2^38, the same digit
        // quirk meg.rs:1349-1355 pins; the old pin 0x3fff_ffff_ffff widened
        // both immediates and desynced the clamp arms vs the interpreter)
        want.extend_from_slice(&[0x49, 0xb9]);
        want.extend_from_slice(&0x3f_ffff_ffffu64.to_le_bytes());
        want.extend_from_slice(&[0x49, 0xba]);
        want.extend_from_slice(&((-0x40_0000_0000i64) as u64).to_le_bytes());
        assert_eq!(a.code, want, "emit_call_lfo full pin (:1441-1448)");
        // exactly one `call rax` (ff d0) in the leg.
        assert_eq!(a.code.windows(2).filter(|w| *w == [0xff, 0xd0]).count(), 1, "single call");
        println!("meg_jit_b2b3a callout: prologue/arg-loads/call/load_p_limits byte-pinned");
    }

    /// emit_lfo executed against the PAIRED meg::get_lfo across all four wave
    /// selects, the pitch-shift depth (bits 8-9), the offsets index (bits 12-15)
    /// and a lfo_step()-evolved counter sequence — and the SEED register (RSI)
    /// is left byte-for-byte untouched (handoff §H / risk 3: the LFO path draws
    /// NO rand, so the rand stream stays aligned; "the rand() seam consistency").
    #[test]
    fn b2b3a_lfo_exec_matches_get_lfo() {
        let o = offs();
        // a 0x8000-entry resident table (the sine path indexes it) filled with
        // a pattern so a wrong index is visible, not silently 0.
        let sintab: Vec<u16> = (0..0x8000u32).map(|i| (i as u16).wrapping_mul(31).wrapping_add(7)).collect();
        let sp = sintab.as_ptr();

        // rig (rebuilt per idx — the offsets/address bakes idx): the head is
        // mov64 MS,ARG0; mov32 SEED,ARG1; mov64 R10,ARG2 (stash the out ptr —
        // emit_lfo bakes R8, :915/:937, so ARG2/R8 must not stay live through
        // it); emit_lfo; store32 [R10],SEED; ret. Returns the LFO word in RAX
        // and dumps the post SEED to *out. The SEED round-trip is the rand-seam
        // assertion (emit_lfo draws nothing, handoff §H).
        for idx in 0u32..6 {
            let mut a = Assembler::new();
            a.push(RBX);
            a.push(RSI);
            a.mov64(MS, ARG0);
            a.mov32(SEED, ARG1);
            a.mov64(R10, ARG2); // stash the out ptr out of R8's way (emit_lfo bakes R8)
            emit_lfo(&mut a, &o, idx, sp); // :897-969
            a.store32(Mem::b(R10, 0), SEED); // seed return (R10 survived — emit_lfo only eats RAX/RCX/RDX/R8)
            a.pop(RSI);
            a.pop(RBX);
            a.ret();
            let x = X64Exec::new(&a.code);
            let f: unsafe extern "system" fn(*const MegState, u32, *mut u32) -> u32 =
                unsafe { std::mem::transmute(x.ptr) };

            // a device carrying this LFO; evolve the counter with the PAIRED
            // lfo_step across several samples (the hoist is loop-invariant WITHIN
            // a sample because lfo_step runs BETWEEN samples — :892-896).
            let mut s = Swp30::new();
            for wave in 0u32..4 {
                for depth in 0u32..4 {
                    for offidx in [0u16, 1, 5, 8, 15] {
                        // lfo word: offidx<<12 | wave<<10 | depth<<8
                        s.meg.lfo[idx as usize] =
                            (offidx << 12 | (wave as u16) << 10 | (depth as u16) << 8) as u16;
                        s.meg.lfo_increment[idx as usize] = 0x1234 + idx * 7 + depth;
                        s.meg.lfo_counter[idx as usize] = 0x2222_0000 + idx;
                        for _ in 0..5 {
                            let want = s.meg.get_lfo(idx as usize, &sintab);
                            let mut seed_out = 0u32;
                            let got = unsafe { f(&s.meg, 0xdead_beef, &mut seed_out) };
                            assert_eq!(got, want, "emit_lfo idx={idx} wave={wave} depth={depth} off={offidx:#x} c={:x}", s.meg.lfo_counter[idx as usize]);
                            assert_eq!(seed_out, 0xdead_beef, "emit_lfo must NOT touch SEED (rand seam aligned)");
                            // advance ONLY this LFO the way lfo_step would
                            s.meg.lfo_counter[idx as usize] =
                                (s.meg.lfo_counter[idx as usize] + s.meg.lfo_increment[idx as usize])
                                    & 0x3f_ffff;
                        }
                    }
                }
            }
        }
        println!("meg_jit_b2b3a emit_lfo exec: == meg::get_lfo across waves/depth/offsets/step, SEED intact");
    }

    /// dm_src 0-3 un-hoisted arm (no frame slot) with a resident sintab executed
    /// through emit_dm_src: it must take the in-place emit_lfo leg (:1421) and
    /// land exactly the same value as the interpreter's get_lfo.
    #[test]
    fn b2b3a_dm_src_lfo_arm_exec() {
        let o = offs();
        let sintab: Vec<u16> = (0..0x8000u32).map(|i| (i as u16).wrapping_mul(17).wrapping_add(3)).collect();
        let sp = sintab.as_ptr();
        let mut s = Swp30::new();
        let op = Op { dm_src: 2, lfo: 4, dm: 1, ..Op::ZERO };
        s.meg.lfo[4] = (1u16 << 10) | (2u16 << 8) | (3u16 << 12); // wave=1 tri, depth=2, off=3
        s.meg.lfo_counter[4] = 0x1234_5;
        let ms = addr_of_mut!(s.meg);

        let mut a = Assembler::new();
        a.push(RBX);
        a.mov64(MS, ARG0);
        emit_dm_src(&mut a, &o, &op, 0, sp, 0x8000); // un-hoisted + sintab ⇒ emit_lfo
        a.pop(RBX);
        a.ret();
        let x = X64Exec::new(&a.code);
        let f: unsafe extern "system" fn(*const MegState) -> u32 = unsafe { std::mem::transmute(x.ptr) };
        let got = unsafe { f(ms) };
        let want = unsafe { (*ms).get_lfo(op.lfo as usize, &sintab) };
        assert_eq!(got, want, "dm_src un-hoisted + sintab == get_lfo (:1421)");
        println!("meg_jit_b2b3a dm_src LFO arm exec: un-hoisted sintab ⇒ in-place emit_lfo == get_lfo");
    }

    /// THE callout exec rig (brief §3): emit_call_lfo targets the REAL
    /// meg_jit_call_lfo trampoline. It must (a) return the same word as
    /// get_lfo, (b) preserve EVERY callee-saved reg the op loop keeps live
    /// (R13/R14/R15/RSI/RDI/RBP + RBX/R12 — scrub-and-check), (c) leave
    /// P_MAX(R9)/P_MIN(R10) reloaded by load_p_limits (:1448), and (d) the
    /// stack is 16-aligned at the call target with the Win64 shadow space
    /// reserved — the frame mirrors the REAL op-loop frame: :827's 8-push
    /// prologue + `subrsp(40)` = FRAME's shadow32+align8 portion (:817/:828).
    /// The full 8 pushes ALSO restore the harness's callee-saves at ret
    /// (:1692) — the harness keeps live values in RSI/RDI/R13-R15 across the
    /// extern "system" call; the old 3-push rig handed it garbage there
    /// (the B2b-3a-FIX root cause; handoff §3 risk 5).
    #[test]
    fn b2b3a_callout_exec_abi_scrub() {
        let sintab: Vec<u16> = (0..0x8000u32).map(|i| (i as u16).wrapping_mul(45).wrapping_add(11)).collect();
        let sp = sintab.as_ptr();
        let lfo = 6u32;
        let mut s = Swp30::new();
        s.meg.lfo[6] = (3u16 << 10) | (1u16 << 8) | (2u16 << 12); // saw-down + depth + off
        s.meg.lfo_counter[6] = 0x3322_1100;
        let ms = addr_of_mut!(s.meg);
        let want = s.meg.get_lfo(lfo as usize, &sintab);

        // scrub buffer: R13,R14,R15,RSI,RDI,R9,R10,RBP post-call (8 × u64).
        let mut scrub = [0u64; 8];
        let scrubp = scrub.as_mut_ptr();

        let mut a = Assembler::new();
        a.push(RBX); // :827 exact 8-push prologue — the op loop owns ALL callee-saves
        a.push(R12); //        and the epilogue (:1692) restores them before ret; a
        a.push(R13); //        partial rig would hand garbage callee-saves back to the
        a.push(R14); //        compiled harness (which legitimately keeps values in them
        a.push(R15); //        across the extern "system" call — THE B2b-3a-FIX).
        a.push(RSI);
        a.push(RDI);
        a.push(RBP);
        a.subrsp(40); // FRAME (shadow 32 + align 8) :817/:828 —置き場16+LFO96 は ≡0 mod16
                      // omitted; rsp ≡ 0 (mod16) at the call, [rsp..rsp+32) = shadow
        a.mov64(MS, ARG0); // rbx = ms (live in the op loop)
        a.mov64(R12, ARG1); // r12 = scrub ptr (live + used to store ⇒ proves R12 survived)
        // callee-saved sentinels the op loop keeps live across the callout
        a.imm64(P, 0x1313_1313_1313_1313); // R13
        a.imm64(R14, 0x1414_1414_1414_1414);
        a.imm64(R15, 0x1515_1515_1515_1515);
        a.imm64(SEED, 0x1616_1616_1616_1616); // RSI = SEED
        a.imm64(RDI, 0x1717_1717_1717_1717); // RDI = K_MAX
        a.imm64(RBP, 0x1818_1818_1818_1818); // RBP = K_MIN
        a.imm64(R9, 0); // poison P_MAX — reload must overwrite
        a.imm64(R10, 0); // poison P_MIN
        emit_call_lfo(&mut a, lfo, sp, 0x8000); // :1441-1448
        // scrub-scan the callee-saved regs through R12 (the surviving window ptr)
        a.store64(Mem::b(R12, 0), P);
        a.store64(Mem::b(R12, 8), R14);
        a.store64(Mem::b(R12, 16), R15);
        a.store64(Mem::b(R12, 24), SEED);
        a.store64(Mem::b(R12, 32), RDI);
        a.store64(Mem::b(R12, 40), R9); // P_MAX reloaded
        a.store64(Mem::b(R12, 48), R10); // P_MIN reloaded
        a.store64(Mem::b(R12, 56), RBP);
        a.addrsp(40); // :1691 twin (FRAME portion)
        a.pop(RBP); // :1692 — exact LIFO inverse of :827; restores RSI/RDI/R13-R15
        a.pop(RDI);
        a.pop(RSI);
        a.pop(R15);
        a.pop(R14);
        a.pop(R13);
        a.pop(R12);
        a.pop(RBX);
        a.ret();
        let x = X64Exec::new(&a.code);
        let f: unsafe extern "system" fn(*const MegState, *mut u64) -> u32 =
            unsafe { std::mem::transmute(x.ptr) };
        let got = unsafe { f(ms, scrubp) };

        assert_eq!(got, want, "callout returned get_lfo (== meg_jit_call_lfo)");
        let want_scrub = [
            0x1313_1313_1313_1313u64, // P (R13)
            0x1414_1414_1414_1414,    // SC (R14)
            0x1515_1515_1515_1515,    // RAM (R15)
            0x1616_1616_1616_1616,    // SEED (RSI) :1435
            0x1717_1717_1717_1717,    // K_MAX (RDI) :1435
            0x3f_ffff_ffff,           // P_MAX (R9) RELOADED :1448 = :821 0x3fffffffff
            (-0x40_0000_0000i64) as u64, // P_MIN (R10) RELOADED :1448 = :822 -0x4000000000
            0x1818_1818_1818_1818,    // K_MIN (RBP)
        ];
        assert_eq!(scrub, want_scrub, "callee-saves preserved + P limits reloaded");
        println!("meg_jit_b2b3a callout exec: get_lfo value + all callee-saves intact + P limits reloaded");
    }

    /// B2b-3c MIGRATION of `b2b3a_build_inert`: LFO/dm_src0-3-flagged +
    /// branchy programs now compile AND run — hoist, in-place emit_lfo and
    /// the meg_jit_call_lfo callout all execute live through the published
    /// buffer (the standalone rigs above pin them byte-wise).
    #[test]
    fn b2b3a_build_live() {
        assert!(PHASE_B2_EMIT_OK, "B2b-3c: emission is LIVE");
        let mut s = Swp30::new();
        let sintab: Vec<u16> = vec![0; 0x8000]; // resident — the hoist/in-place path
        let mut j = MegJit::new();
        assert!(j.can(), "enabled — emission is live");
        let mut ops = *s.meg_ops;
        // dm_src 0-3 programs, sintab-resident so the loop hoists/in-place emits
        ops[0x10].dm_src = 0;
        ops[0x10].lfo = 3;
        ops[0x11].dm_src = 1;
        ops[0x11].lfo = 3; // used >=2x ⇒ hoisted (:983)
        ops[0x12].dm_src = 2;
        ops[0x12].lfo = 5; // used once ⇒ in-place emit_lfo (:1421)
        ops[0x20].dm_src = 3;
        ops[0x20].lfo = 0x18; // out of range ⇒ callout arm (:1422 else)
        ops[0x40].jump = 1; // branchy
        ops[0x40].target = 0x10;
        assert!(j.rebuild(&s.meg, &ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        let (fnp, buf, _sz) = j.gen_code_state();
        assert_ne!(fnp, 0, "brief (migrated): entry published (:1712)");
        assert_ne!(buf, 0, "buffer handoff done (:1696-1712)");
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        assert!(j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "run() executes the LFO/hoist/callout block");
        println!("meg_jit_b2b3a: LFO/callout-flagged branchy build LIVE, gen fnp published, run() executes");
    }
}

// ---------------------------------------------------------------------------
// B2b-3b — FULL-PROGRAM parity rigs (brief §2). `program_bytes` runs the
// SAME `emit_program` stitch build() assembles behind the inert PHASE gate
// (hoist scan :971-991 + k-loop :995-1679), so the executed bytes are the
// REAL compiled program — prologue :826-837 (exact 8 pushes + subrsp(152),
// every callee-save restored before ret — the fixed rig frame is INSIDE the
// bytes, the S12f lesson cannot regress) and epilogue :1688-1694 included.
// The rig A/Bs against the PAIRED `meg::run_program` exactly like the C++
// CHECK leg (:426-511): same start state, one compiled step vs one
// interpreter step, FULL state equality after every step (MegState + the
// device fields the program touches + the whole 0x40000 reverb RAM).
// pc/icount parity mirrors the C++ caller leg (:513-514 — the fn does NOT
// touch them; run_program subtracts its own 0x180, §5.4/CHECK :451-453).
// The device skip slot is the ONE documented asymmetry (the JIT writes it
// during a branchy sample :1099 and the prologue re-zeroes it next sample
// :840; the interpreter's skip is local meg.rs:1230) — pinned per program
// at the tail instead of per step.
// ---------------------------------------------------------------------------
#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
mod b2b3b {
    use std::ffi::c_void;
    use std::ptr::addr_of_mut;

    use smu_machine::meg_jit::{program_bytes, MegJit, MegSwpDev, PHASE_B2_EMIT_OK};
    use smu_swp30::meg::{run_program, MegState, MegSwp, Op};
    use smu_swp30::mix::MegJitHook;
    use smu_swp30::Swp30;

    // jit.rs:49-57 / b2a precedent — one RWX mapping per compiled program.
    #[link(name = "kernel32")]
    extern "system" {
        fn VirtualAlloc(p: *mut c_void, n: usize, lt: u32, prot: u32) -> *mut c_void;
        fn VirtualFree(p: *mut c_void, n: usize, ft: u32) -> i32;
    }

    struct X64Exec {
        ptr: *mut u8,
    }

    impl X64Exec {
        fn new(code: &[u8]) -> X64Exec {
            let size = (code.len() + 0xfff) & !0xfffusize;
            let p = unsafe { VirtualAlloc(std::ptr::null_mut(), size, 0x3000, 0x40) };
            assert!(!p.is_null(), "VirtualAlloc RWX failed");
            unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), p as *mut u8, code.len()) };
            X64Exec { ptr: p as *mut u8 }
        }
    }

    impl Drop for X64Exec {
        fn drop(&mut self) {
            unsafe { VirtualFree(self.ptr as *mut c_void, 0, 0x8000) };
        }
    }

    /// One complete machine image: MegState + the device fields the seam and
    /// the §7-B window point at, and the full 0x40000 reverb RAM. Built
    /// deterministically from (d3, d2) so the jit-side and interp-side rigs
    /// are bit-identical twins (Invariant 3: every field explicit).
    struct Rig {
        meg: MegState,
        flag_n: bool,
        flag_z: bool,
        ix2_value: [i32; 3],
        ix2_act: [u8; 3],
        ram_index2: i32,
        skip_to: u16,
        reverb_ram: Vec<u16>,
        seed: u32,
    }

    fn lfo_inc(i: usize) -> u32 {
        match i {
            6 => 0x0123_4567,
            1 => 0x0011_2233,
            9 => 0x1111_1111,
            2 => 0xdead_beef,
            _ => 0x1234_5677,
        }
    }

    fn new_rig(d3: u32, d2: u32) -> Rig {
        let mut meg = MegState::new();
        for i in 0..0x40usize {
            meg.m[i] = (i as i32).wrapping_mul(0x1234).wrapping_sub(0x5678);
        }
        for i in 0..0x80usize {
            meg.r[i] = (i as i32).wrapping_mul(-0x7a10).wrapping_add(0x1357);
        }
        for i in 0..8usize {
            meg.t[i] = (i as i16).wrapping_mul(3).wrapping_sub(8);
        }
        for k in 0..0x180usize {
            meg.konst[k] = (k as i16).wrapping_mul(0x51).wrapping_sub(0x2a);
        }
        meg.konst[1] = -0x350; // negative ⇒ expand(neg)==0 leg + flag_n latch
        meg.konst[0x17] = -0x2c; // branchy latch-alu operand
        for i in 0..0x20usize {
            meg.offset[i] = (0x100 + i * 0x37) as u16;
        }
        // wave coverage: tri+depth2+off3 (lfo 6, hoisted), saw-up (9),
        // saw-down (1), sine depth3 (2) — bits10-11 / bits8-9 / bits12-15
        meg.lfo[6] = (1 << 10) | (2 << 8) | (3 << 12);
        meg.lfo[9] = (2 << 10) | (1 << 8) | (5 << 12);
        meg.lfo[1] = (3 << 10) | (0 << 8) | (8 << 12);
        meg.lfo[2] = (0 << 10) | (3 << 8) | (1 << 12);
        meg.lfo_counter[6] = 0x5555_aaaa; // crosses the &0x1ffff fold
        meg.lfo_counter[9] = 0x0001_2345;
        meg.lfo_counter[1] = 0x7fff_ffff; // sine bit15/bit16 wrap band
        meg.lfo_counter[2] = 0x1000_0001;
        for i in 0..0x18usize {
            if i != 6 && i != 9 && i != 1 && i != 2 {
                meg.lfo_counter[i] = (i as u32).wrapping_mul(0x9e37_79b9);
            }
        }
        meg.p = -0x400_0000_0001; // below P_MIN ⇒ pack24 K_MIN leg + clamps
        meg.delay_3 = d3;
        meg.delay_2 = d2;
        meg.sample_counter = 0x2468;
        meg.ram_read = 0x770;
        meg.ram_write = 0x1357_9b11;
        meg.ram_index = 0x45;
        meg.mw_value = [-1, 2, -3];
        meg.mw_reg = [1, 2, 0];
        meg.rw_value = [4, -5, 6];
        meg.rw_reg = [3, 0, 5];
        meg.index_value = [-7, 8, -9];
        meg.index_active = [true, false, true];
        meg.memw_value = [0x11, 0x22, 0x33];
        meg.memr_value = [-0x44, 0x55, -0x66];
        meg.memw_active = [false, true, false];
        meg.memr_active = [true, false, true];
        meg.t_value = [-0x800, 0x7ff];
        Rig {
            meg,
            flag_n: false,
            flag_z: false,
            ix2_value: [0x111, -0x222, 0x333],
            ix2_act: [1, 0, 1],
            ram_index2: 0x9,
            skip_to: 0,
            reverb_ram: (0..0x40000u32).map(|i| (i * 7 + 0x1234) as u16).collect(),
            seed: 0xcafe_babe,
        }
    }

    fn mk_ops(list: &[(usize, Op)]) -> [Op; 0x180] {
        let mut ops = [Op::ZERO; 0x180];
        for (k, o) in list {
            ops[*k] = *o;
        }
        ops
    }

    /// Program A (non-branchy): ALU mmode 0-3 incl. m1_from_t 1/2 + expand,
    /// asel 0-4, rop 0-3, shift, clamp 0-3, latch; dm src 0-7 (lfo 6 used
    /// twice ⇒ HOISTED, 9/1/2 once ⇒ in-place), dr both arms + p-noise-off;
    /// memw + index/index2; memop write/read/table-read; bare rand_n; t both
    /// sources + the need_tval two-slots-later consumer (k3 feeds k5); the
    /// tail trio with ring bytes; baked-vs-runtime konst via the bake flag.
    /// DENSE (B2b-3b FIX): a real MU2000 program fills nearly every slot,
    /// and the C++ emits NOTHING for a zero op in folded mode except the
    /// `k >= 0x17d` ring bytes (swp30_jit.cpp:1033/:1037/:1041/:1045,
    /// :1069/:1073, :1415/:1467/:1474/:1483/:1495/:1502/:1508/:1512/:1517/
    /// :1519/:1524, :1530/:1566) — a 22-op program is ~4.3 KB faithfully,
    /// so 0x20..0x7e + 0x80..0x17b are filled with per-class work ops.
    fn prog_a() -> [Op; 0x180] {
        let mut ops = mk_ops(&[
            (0, Op { alu: 1, mmode: 1, asel: 4, dm: 1, dm_src: 6, ..Op::ZERO }),
            (1, Op { alu: 1, mmode: 2, m2_from_m: 1, sm: 1, m1_expand: 1, asel: 0, rop: 2, shift: 2, clamp: 1, latch: 1, dm: 2, dm_src: 7, dr: 3, ..Op::ZERO }),
            (2, Op { alu: 1, mmode: 3, m2_from_m: 1, sm: 3, asel: 1, sr: 2, rop: 1, clamp: 2, dm: 3, dm_src: 4, t_write: 1, t: 1, ..Op::ZERO }),
            (3, Op { alu: 1, m1_from_t: 2, m1_expand: 1, mmode: 2, m2_from_m: 1, sm: 2, asel: 3, rop: 3, clamp: 3, latch: 1, dm: 4, dm_src: 5, dr: 5, dr_from_r: 1, sr: 4, ..Op::ZERO }),
            (4, Op { alu: 1, mmode: 0, ..Op::ZERO }), // p unchanged, 42-bit wrap
            (5, Op { alu: 1, m1_from_t: 1, mmode: 1, asel: 2, sm: 2, latch: 1, t_write: 1, t_from_p: 1, t: 2, ..Op::ZERO }), // ⇒ need_tval[3]
            (6, Op { alu: 1, mmode: 1, asel: 1, sr: 1, rop: 1, dm: 6, dm_src: 0, lfo: 6, ..Op::ZERO }),
            (7, Op { dm: 7, dm_src: 0, lfo: 6, ..Op::ZERO }), // 2nd use ⇒ hoist
            (8, Op { dm: 8, dm_src: 1, lfo: 9, ..Op::ZERO }), // 1 use ⇒ in-place
            (9, Op { dm: 9, dm_src: 0, lfo: 1, ..Op::ZERO }),
            (10, Op { alu: 1, mmode: 1, index: 1, index2: 1, memw: 1, ..Op::ZERO }),
            (11, Op { memop: 1, mem_use_index: 1, mem_use_index2: 1, offset_index: 1, addr_mask: 0x3fff, addr_base: 0x1000, ..Op::ZERO }),
            (12, Op { memop: 2, mem_use_index: 1, offset_index: 2, addr_mask: 0x7ff, addr_base: 0, region: 1, ..Op::ZERO }),
            (13, Op { memop: 2, mem_table: 1, mem_use_index: 1, offset_index: 3, ..Op::ZERO }),
            (14, Op { dm: 0x14, dm_src: 4, rand_n: 25, ..Op::ZERO }), // bare seed jump
            (15, Op { dm: 0x15, dm_src: 6, no_noise: 1, dr: 0x16, ..Op::ZERO }),
            (0x16, Op { alu: 1, mmode: 2, m1_from_t: 1, sr: 0x10, asel: 2, sm: 0x10, rop: 2, shift: 4, latch: 1, ..Op::ZERO }),
            (0x1f, Op { alu: 1, mmode: 1, asel: 4, rop: 3, clamp: 1, dm: 0x20, dm_src: 6, ..Op::ZERO }),
            (0x7f, Op { memop: 1, offset_index: 0x10, addr_mask: 0x3ffff, addr_base: 0, ..Op::ZERO }),
            (0x17d, Op { alu: 1, mmode: 1, dm: 0x21, dm_src: 0, lfo: 2, dr: 0x22, index: 1, index2: 1, memw: 1, ..Op::ZERO }),
            (0x17e, Op { alu: 1, mmode: 3, m2_from_m: 1, sm: 0x20, asel: 3, rop: 1, clamp: 2, t_write: 1, t_from_p: 1, t: 3, ..Op::ZERO }),
            (0x17f, Op { dm: 0x23, dm_src: 7, sm: 0x21, dr: 0x24, dr_from_r: 1, sr: 0x22, rand_n: 3, ..Op::ZERO }),
        ]);
        // Dense fill (see doc): specials at 0x7f and 0x17c-0x17f untouched;
        // lfo field only on the 6 (hoisted) and 9/1/2 singles stay in-place.
        for k in (0x20u32..0x7f).chain(0x80..0x17c) {
            ops[k as usize] = match k % 5 {
                0 => Op {
                    alu: 1,
                    mmode: 1,
                    asel: 0,
                    rop: 0,
                    dm: 1 + ((k >> 1) & 0x1f) as u8,
                    dm_src: 0,
                    lfo: 6,
                    dr: 1 + (k & 0x3f) as u8,
                    ..Op::ZERO
                },
                1 => Op {
                    alu: 1,
                    mmode: 2,
                    m1_from_t: 1,
                    t: (k & 7) as u8,
                    m2_from_m: 1,
                    sm: (k & 0x3f) as u8,
                    asel: 1,
                    sr: (k & 0x7f) as u8,
                    rop: (k & 3) as u8,
                    shift: ((k >> 2) & 3) as u8,
                    clamp: ((k >> 1) & 3) as u8,
                    latch: (k & 1) as u8,
                    ..Op::ZERO
                },
                2 => Op {
                    dm: 1 + ((k >> 3) & 0x1f) as u8,
                    dm_src: 5,
                    rand_n: 1,
                    dr: 1 + (k & 0x3f) as u8,
                    dr_from_r: 1,
                    sr: (k & 0x7f) as u8,
                    ..Op::ZERO
                },
                3 => Op {
                    alu: 1,
                    mmode: 3,
                    m2_from_m: 1,
                    sm: (k & 0x3f) as u8,
                    asel: 3,
                    rop: 3,
                    clamp: 2,
                    memw: 1,
                    index: 1,
                    index2: (k & 1) as u8,
                    ..Op::ZERO
                },
                _ => Op {
                    alu: 1,
                    mmode: 0,
                    memop: if (k >> 1) & 1 != 0 { 2 } else { 1 },
                    region: ((k >> 1) & 1) as u8,
                    mem_use_index: 1,
                    offset_index: (k & 0x1f) as u8,
                    addr_mask: if (k >> 1) & 1 != 0 { 0x7ff } else { 0x3fff },
                    addr_base: if (k >> 1) & 1 != 0 { 0 } else { 0x1000 },
                    ..Op::ZERO
                },
            };
        }
        ops
    }

    /// Program B (branchy): ring-per-op mode; always-taken jump with a
    /// jump-op t write (upstream 31), cond n||z taken via z, cond n NOT
    /// taken (the jump op still takes the erase tail), a BACKWARD jump
    /// (target>k compile-time arm — never skips), and RAM-write /
    /// rand-drawing victims inside every skipped window (the erase path
    /// must cancel their queued ring writes AND their seed draws).
    fn prog_b() -> [Op; 0x180] {
        mk_ops(&[
            (0, Op { alu: 1, mmode: 1, asel: 4, dm: 1, dm_src: 6, ..Op::ZERO }),
            (1, Op { alu: 1, mmode: 1, asel: 4, latch: 1, ..Op::ZERO }), // konst[1]<0 ⇒ flag_n
            (2, Op { alu: 1, mmode: 0, asel: 4, rop: 3, latch: 1, ..Op::ZERO }), // 0 ⇒ flag_z
            (3, Op { jump: 1, cond: 0, target: 0x10, t_write: 1, t: 4, ..Op::ZERO }),
            (4, Op { alu: 1, mmode: 2, m2_from_m: 1, sm: 1, shift: 3, clamp: 1, dm: 0x30, dm_src: 6, memw: 1, ..Op::ZERO }),
            (5, Op { dm: 0x31, dm_src: 5, rand_n: 40, ..Op::ZERO }), // draw must be cancelled
            (6, Op { memop: 1, mem_use_index: 1, offset_index: 0x12, addr_mask: 0x3ff, addr_base: 0x2000, ..Op::ZERO }),
            (7, Op { alu: 1, mmode: 1, index: 1, index2: 1, ..Op::ZERO }),
            (8, Op { dm: 0x33, dm_src: 0, lfo: 6, ..Op::ZERO }), // hoist use (compile-time count)
            (9, Op { jump: 1, cond: 0x8 | 0x4 | 0x2, target: 0x14, t_write: 1, t_from_p: 1, t: 5, ..Op::ZERO }), // n||z via z
            (10, Op { alu: 1, mmode: 1, asel: 2, sm: 3, rop: 2, clamp: 3, dm: 0x34, dm_src: 6, latch: 1, ..Op::ZERO }),
            (11, Op { jump: 1, cond: 0x8 | 0x4, target: 0x18, ..Op::ZERO }), // n==0 ⇒ NOT taken, erase tail
            (12, Op { dm: 0x35, dm_src: 5, ..Op::ZERO }),
            (13, Op { memop: 2, mem_use_index: 1, offset_index: 0x13, addr_mask: 0x3ff, addr_base: 0x400, region: 1, ..Op::ZERO }),
            (14, Op { alu: 1, mmode: 1, asel: 4, dm: 0x36, dm_src: 6, ..Op::ZERO }), // skip-boundary op
            (15, Op { jump: 1, cond: 0, target: 4, ..Op::ZERO }), // BACKWARD — no skip, erase tail
            (16, Op { dm: 0x37, dm_src: 0, lfo: 9, ..Op::ZERO }),
            (17, Op { alu: 1, mmode: 1, asel: 4, latch: 1, dm: 0x38, dm_src: 2, lfo: 6, ..Op::ZERO }), // konst[0x17]<0 ⇒ flag_n
            (0x19, Op { jump: 1, cond: 0x8 | 0x4, target: 0x1c, ..Op::ZERO }), // n==1 ⇒ taken
            (0x1a, Op { dm: 0x39, dm_src: 5, rand_n: 5, ..Op::ZERO }),
            (0x1b, Op { alu: 1, mmode: 3, m2_from_m: 1, sm: 2, rop: 1, latch: 1, ..Op::ZERO }),
            (0x1c, Op { alu: 1, mmode: 1, rop: 2, clamp: 2, dm: 0x3a, dm_src: 6, no_noise: 1, ..Op::ZERO }),
            (0x1f, Op { jump: 1, cond: 0, target: 0x21, ..Op::ZERO }),
            (0x20, Op { dm: 0x3b, dm_src: 5, rand_n: 9, ..Op::ZERO }),
            (0x21, Op { alu: 1, mmode: 2, sr: 0x11, asel: 1, latch: 1, dm: 0x3c, dm_src: 7, sm: 0x20, ..Op::ZERO }),
            (0x80, Op { alu: 1, mmode: 3, m2_from_m: 1, sm: 1, asel: 4, memw: 1, ..Op::ZERO }),
            (0x17d, Op { dm: 0x3e, dr: 0x3f, index: 1, index2: 1, memw: 1, ..Op::ZERO }),
            (0x17e, Op { t_write: 1, t_from_p: 1, t: 6, ..Op::ZERO }),
            (0x17f, Op { rand_n: 2, ..Op::ZERO }),
        ])
    }

    fn eqf<T: PartialEq + std::fmt::Debug>(tag: &str, step: u32, name: &str, j: &T, i: &T) {
        assert!(j == i, "{tag} step {step}: {name} differs\n  jit {j:?}\n  int {i:?}");
    }

    /// FULL MegState equality (the C++ CHECK byte-compare field list :461-509,
    /// fielded). `decoded`/`program`/`map`/`lfo_increment`/`icount`-adjacent
    /// read-only state: neither engine writes them in a program step and both
    /// rigs start from the identical `new_rig` image — equality by
    /// construction; everything MUTABLE is compared below.
    fn cmp_meg(tag: &str, step: u32, j: &MegState, i: &MegState) {
        eqf(tag, step, "m", &j.m, &i.m);
        eqf(tag, step, "r", &j.r, &i.r);
        eqf(tag, step, "t", &j.t, &i.t);
        eqf(tag, step, "p", &j.p, &i.p);
        eqf(tag, step, "konst", &j.konst, &i.konst);
        eqf(tag, step, "offset", &j.offset, &i.offset);
        eqf(tag, step, "lfo", &j.lfo, &i.lfo);
        eqf(tag, step, "lfo_counter", &j.lfo_counter, &i.lfo_counter);
        eqf(tag, step, "mw_value", &j.mw_value, &i.mw_value);
        eqf(tag, step, "mw_reg", &j.mw_reg, &i.mw_reg);
        eqf(tag, step, "rw_value", &j.rw_value, &i.rw_value);
        eqf(tag, step, "rw_reg", &j.rw_reg, &i.rw_reg);
        eqf(tag, step, "index_value", &j.index_value, &i.index_value);
        eqf(tag, step, "index_active", &j.index_active, &i.index_active);
        eqf(tag, step, "memw_value", &j.memw_value, &i.memw_value);
        eqf(tag, step, "memr_value", &j.memr_value, &i.memr_value);
        eqf(tag, step, "t_value", &j.t_value, &i.t_value);
        eqf(tag, step, "memw_active", &j.memw_active, &i.memw_active);
        eqf(tag, step, "memr_active", &j.memr_active, &i.memr_active);
        eqf(tag, step, "delay_3", &j.delay_3, &i.delay_3);
        eqf(tag, step, "delay_2", &j.delay_2, &i.delay_2);
        eqf(tag, step, "ram_read", &j.ram_read, &i.ram_read);
        eqf(tag, step, "ram_write", &j.ram_write, &i.ram_write);
        eqf(tag, step, "ram_index", &j.ram_index, &i.ram_index);
        eqf(tag, step, "sample_counter", &j.sample_counter, &i.sample_counter);
        eqf(tag, step, "program_address", &j.program_address, &i.program_address);
        eqf(tag, step, "pc", &j.pc, &i.pc);
        eqf(tag, step, "icount", &j.icount, &i.icount);
        eqf(tag, step, "retval", &j.retval, &i.retval);
    }

    /// Device-side equality through the two seams (the §7-B window fields +
    /// the seed + the FULL reverb RAM). `skip_to` deliberately NOT compared
    /// per step — see the section header; pinned at the tail.
    fn cmp_dev(tag: &str, step: u32, j: &MegSwp, i: &MegSwp) {
        eqf(tag, step, "seed", j.seed, i.seed);
        eqf(tag, step, "flag_n", j.flag_n, i.flag_n);
        eqf(tag, step, "flag_z", j.flag_z, i.flag_z);
        eqf(tag, step, "ix2_value", &(*j.ix2_value), &(*i.ix2_value));
        eqf(tag, step, "ix2_act", &(*j.ix2_act), &(*i.ix2_act));
        eqf(tag, step, "ram_index2", j.ram_index2, i.ram_index2);
        let (mut bad, mut first) = (0usize, 0usize);
        for (idx, (a, b)) in j.reverb_ram.iter().zip(i.reverb_ram.iter()).enumerate() {
            if a != b {
                if bad == 0 {
                    first = idx;
                }
                bad += 1;
            }
        }
        assert_eq!(
            bad, 0,
            "{tag} step {step}: reverb_ram {bad} diffs, first @{first:#x} jit={:#x} int={:#x}",
            j.reverb_ram[first], i.reverb_ram[first]
        );
    }

    fn run_parity(tag: &str, ops: &[Op; 0x180], bake: bool, early: bool, d3: u32, d2: u32, n_steps: u32, skip_end: u16) {
        // resident sintab ⇒ the hoist/in-place legs run (the callout arm is
        // covered standalone by b2b3a's rigs; the interpreter's get_lfo
        // needs the full 0x8000 table either way)
        let sintab: Vec<u16> =
            (0..0x8000u32).map(|i| (i as u16).wrapping_mul(0x9e37).wrapping_add((i >> 3) as u16)).collect();
        let mut rj = new_rig(d3, d2);
        let mut ri = new_rig(d3, d2);
        let bytes = program_bytes(&rj.meg, ops, 0x40000, 0, bake, early, &sintab).expect("compiles");
        assert_eq!(*bytes.last().unwrap(), 0xc3, "{tag}: program ends in ret (:1694)");
        assert!(bytes.len() > 0x2000, "{tag}: full dense program, not a skeleton ({} bytes)", bytes.len());
        let x = X64Exec::new(&bytes);
        let f: unsafe extern "system" fn(*mut MegState, *mut MegSwpDev, *mut u16) =
            unsafe { std::mem::transmute(x.ptr) };
        let mut seam_j = MegSwp {
            flag_n: &mut rj.flag_n,
            flag_z: &mut rj.flag_z,
            ix2_value: &mut rj.ix2_value,
            ix2_act: &mut rj.ix2_act,
            ram_index2: &mut rj.ram_index2,
            skip_to: &mut rj.skip_to,
            revram_enable: 0,
            reverb_ram: &mut rj.reverb_ram,
            seed: &mut rj.seed,
            sintab: &sintab,
        };
        let dev = MegSwpDev::from_swp(&mut seam_j);
        let msj = addr_of_mut!(rj.meg);
        let ramj = seam_j.reverb_ram.as_mut_ptr();
        let mut seam_i = MegSwp {
            flag_n: &mut ri.flag_n,
            flag_z: &mut ri.flag_z,
            ix2_value: &mut ri.ix2_value,
            ix2_act: &mut ri.ix2_act,
            ram_index2: &mut ri.ram_index2,
            skip_to: &mut ri.skip_to,
            revram_enable: 0,
            reverb_ram: &mut ri.reverb_ram,
            seed: &mut ri.seed,
            sintab: &sintab,
        };
        for step in 0..n_steps {
            // identical external drivers between samples (lfo_step runs
            // BETWEEN samples in the real device — :892-896; both rigs get
            // the same bumps so the loop-invariant hoist stays honest)
            for i in 0..0x18usize {
                let inc = lfo_inc(i);
                rj.meg.lfo_counter[i] = rj.meg.lfo_counter[i].wrapping_add(inc);
                ri.meg.lfo_counter[i] = ri.meg.lfo_counter[i].wrapping_add(inc);
            }
            rj.meg.sample_counter = rj.meg.sample_counter.wrapping_add(1);
            ri.meg.sample_counter = ri.meg.sample_counter.wrapping_add(1);
            // the compiled sample: the REAL frame (8 pushes + subrsp(152),
            // ARG0=ms ARG1=window ARG2=RAM per :826-831 — the fixed rig
            // frame lives INSIDE these bytes)
            unsafe { f(msj, &dev as *const MegSwpDev as *mut _, ramj) };
            // caller-side parity legs (:513-514, §5.4): the fn leaves pc/icount
            // for its caller; run_program does its own (meg.rs:1505-1506)
            unsafe {
                (*msj).pc = 0;
                (*msj).icount -= 0x180;
            }
            run_program(&mut ri.meg, &mut seam_i, ops);
            cmp_meg(tag, step, &rj.meg, &ri.meg);
            cmp_dev(tag, step, &seam_j, &seam_i);
        }
        assert_eq!(
            *seam_j.skip_to, skip_end,
            "{tag}: JIT device skip residual (:1099 writes it mid-sample, :840 re-zeroes next head — never compared per step)"
        );
        println!("meg_jit_b2b3b {tag}: {n_steps} steps × full state+device+RAM equality ({} bytes compiled)", bytes.len());
    }

    #[test]
    fn b2b3b_full_program_parity() {
        // folded rings × early-off/on × bake-off/on × ring-offset variants,
        // then branchy (ring-per-op) bake-off/on. 48 samples each — the seed,
        // the LFO counters, the RAM feedback and every ring slot keep
        // evolving across sample boundaries.
        run_parity("A00", &prog_a(), false, false, 0, 0, 48, 0);
        run_parity("A11", &prog_a(), false, false, 1, 1, 48, 0);
        run_parity("Aearly", &prog_a(), false, true, 2, 1, 48, 0);
        run_parity("Abake", &prog_a(), true, false, 0, 1, 48, 0);
        run_parity("Bbranch", &prog_b(), false, false, 2, 0, 48, 0x21);
        run_parity("Bbake", &prog_b(), true, false, 1, 1, 48, 0x21);
    }

    /// B2b-3c MIGRATION of `b2b3b_build_inert`: the SAME rich programs
    /// (branchy + LFO hoist + memop + dm/dr) now compile AND publish AND
    /// run. The RAM guard (:611-612) refusal case is KEPT at the end —
    /// it is the honest remaining refusal, and it must clear fnp.
    #[test]
    fn b2b3b_build_live() {
        assert!(PHASE_B2_EMIT_OK, "B2b-3c: emission is LIVE");
        let sintab: Vec<u16> = (0..0x8000u32).map(|i| (i as u16).wrapping_mul(45).wrapping_add(11)).collect();
        let mut s = Swp30::new();
        let mut j = MegJit::new();
        assert!(j.can(), "enabled — emission is live");
        // branchy + LFO (hoist) + memop + dm/dr — every stitch leg flagged
        let ops = prog_b();
        assert!(j.rebuild(&s.meg, &ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        let (fnp, buf, _sz) = j.gen_code_state();
        assert_ne!(fnp, 0, "brief (migrated): entry published (:1712)");
        assert_ne!(buf, 0, "buffer handoff done (:1696-1712)");
        let mut seam = MegSwp {
            flag_n: &mut s.meg_flag_n,
            flag_z: &mut s.meg_flag_z,
            ix2_value: &mut s.meg_ix2_value,
            ix2_act: &mut s.meg_ix2_act,
            ram_index2: &mut s.meg_ram_index2,
            skip_to: &mut s.meg_skip_to,
            revram_enable: s.revram_enable,
            reverb_ram: &mut s.reverb_ram,
            seed: &mut s.rand_seed,
            sintab: &sintab,
        };
        assert!(j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "run() executes the full rich block");
        // the full program assembles dense and ret-terminates (rig twin):
        let bytes = program_bytes(&s.meg, &ops, 0x4_0000, s.revram_enable, false, true, &sintab)
            .expect("RAM ok, compiles");
        assert!(bytes.len() > 0x2000, "dense full program ({} bytes), never half-emitted", bytes.len());
        assert_eq!(*bytes.last().unwrap(), 0xc3);
        // RAM guard refusal case (KEPT): :611-612 closes below 0x40000,
        // build()'s :604 pre-clear + rebuild()'s :883 keep fnp dead after —
        // run() falls to the interpreter (:390).
        assert!(!j.rebuild(&s.meg, &ops, 0x100, s.revram_enable, &sintab, s.meg_const_gen), "RAM guard");
        assert_eq!(j.gen_code_state().0, 0, "guard-closed build clears fnp (:604/:883)");
        assert!(!j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen), "fnp==0 ⇒ interpreter-fall (:390)");
        println!("meg_jit_b2b3b: full {}-byte program compiled, published AND run; guard at 0x100 still refuses", bytes.len());
    }
}
