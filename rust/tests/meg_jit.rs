//! M9b Phase B1+B2a+B2b-1 — MEG (SWP30) x86-64 JIT seam pins, selftest
//! sweeps, offset-table + op-analysis pins (JIT_M10_HANDOFF §8.B5, B2b-1
//! brief). Mounted via smu-machine `[[test]]` (like jit.rs).
//!
//! Four jobs:
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

use std::mem::size_of;
use std::ptr::addr_of_mut;

use smu_machine::meg_jit::{
    analyze_ops, code_field_offsets, env_flag, env_flag_on, offs, selftest_m1_expand,
    selftest_revram_decode, selftest_revram_encode, slot2, slot3, MegJit, MegSwpDev, Offs, FRAME,
    LFO_SLOT_BASE, MEG_OPS, STABLE,
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

/// B1 ACCEPTANCE CONTRACT: `build()` returns false ⇒ `rebuild()` false ⇒
/// `run()` never runs compiled code (returns false at the `!gen.fn` guard,
/// swp30_jit.cpp:390-392). This is what makes the whole seam bit-identical.
#[test]
fn stub_build_false_never_runs() {
    let mut s = Swp30::new();
    let sintab: Vec<u16> = Vec::new(); // empty (device reads 0) — never deref'd in B1

    let mut j = MegJit::new();
    // Default environment: JIT is *enabled* (can()==true), which is exactly
    // why the build()==false stub matters — it must refuse to compile despite
    // being enabled. (A disabled box would trivially pass; guard against that.)
    assert!(j.can(), "expected MEG JIT enabled in the default env");

    // rebuild: the C++ meg_jit_rebuild → build(gen); build()==false ⇒ returns
    // false and gen.fn stays null.
    let rebuilt = j.rebuild(
        &s.meg,
        &s.meg_ops,
        s.reverb_ram.len(),
        s.revram_enable,
        &sintab,
        s.meg_const_gen,
    );
    assert!(!rebuilt, "B1 build() must return false");

    // run: with the null fn the guard at :390-392 bails immediately, so no
    // compiled sample ever runs — the caller falls to meg_run_program.
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
    assert!(!ran, "B1 run() must never execute compiled code");

    println!("meg_jit_stub: enabled but build()/run() both false ⇒ interpreter path");
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
// B2a — EMITTED helper selftests (C++ meg_jit_selftest :520-585 executes
// exactly these byte streams) + frame-skeleton pins + exec-buffer
// roundtrip + the PHASE_B2_EMIT_OK inertness gate. VirtualAlloc/stub-exec
// is Windows x64, so this section is cfg'd (the B1 pins above stay
// platform-free).
// ---------------------------------------------------------------------------

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
mod b2a {
    use std::ffi::c_void;

    use smu_machine::meg_jit::{
        emit_epilogue, emit_frame_skeleton, emit_m1_expand, emit_prologue,
        emit_revram_decode, emit_revram_encode, offs, selftest_revram_decode,
        selftest_revram_encode, Assembler, Code, FRAME, PHASE_B2_EMIT_OK, ARG0, RAX,
    };
    use smu_swp30::meg::MegState;
    use smu_swp30::mix::MegJitHook;
    use smu_swp30::Swp30;

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

    /// THE inertness contract (brief): with PHASE_B2_EMIT_OK == false,
    /// `rebuild()` returns false EVEN with ram_len past the :611-612 guard
    /// (so the refusal is provably the flag, not the stub guards), and
    /// build() leaves fnp == 0.
    #[test]
    fn b2a_build_inert_even_with_big_ram() {
        assert!(!PHASE_B2_EMIT_OK, "B2a must land inert");
        let mut s = Swp30::new();
        let sintab: Vec<u16> = Vec::new();
        let mut j = smu_machine::meg_jit::MegJit::new();
        assert!(j.can(), "enabled — the refusal must come from PHASE_B2_EMIT_OK");
        // ram_len = 0x40000: the C++ RAM guard (:611-612) is OPEN here
        assert!(!j.rebuild(&s.meg, &s.meg_ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        assert_eq!(j.gen_code_state(), (0, 0, 0), "no fnp/buf after a refused build (:604)");
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
        assert!(!j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen));
        println!("meg_jit_b2a: build() false behind PHASE_B2_EMIT_OK even at ram_len=0x40000; fnp==0");
    }

    /// THE B2b-1 inertness contract (brief deliverable 4/5): the offset
    /// table + analysis + real-offset frame skeleton landed, yet past the
    /// RAM guard, on a BRANCHY program with the ring snapshot moving,
    /// `rebuild()` still returns false and NO executable entry is ever
    /// published (fnp==0, no buffer), and run() still falls to the
    /// interpreter. The :614-615 ring snapshot IS committed (C++-faithful:
    /// post-guard refusals like :1703 keep d3/d2 written too).
    #[test]
    fn b2b1_build_inert() {
        assert!(!PHASE_B2_EMIT_OK, "B2b-1 must land inert");
        let mut s = Swp30::new();
        let sintab: Vec<u16> = vec![0; 0x8000]; // resident sintab (bake-ready) — unused while inert
        let mut j = smu_machine::meg_jit::MegJit::new();
        assert!(j.can(), "enabled — the refusal must come from PHASE_B2_EMIT_OK");
        // branchy program (jump at 0x20) + moved ring snapshot
        let mut ops = *s.meg_ops;
        ops[0x20].jump = 1;
        ops[0x20].target = 0x10;
        s.meg.delay_3 = 2;
        s.meg.delay_2 = 1;
        assert!(!j.rebuild(&s.meg, &ops, 0x4_0000, s.revram_enable, &sintab, s.meg_const_gen));
        let (fnp, buf, _size) = j.gen_code_state();
        assert_eq!(fnp, 0, "brief: no executable entry published while PHASE_B2_EMIT_OK=false");
        assert_eq!(buf, 0, "no exec buffer handoff either (:1696-1712 unreachable)");
        assert_eq!(j.gen_ring(), (2, 1), ":614-615 snapshot committed even for a refused build");
        // spec side never compiled either (BAKE leg :415 stays unreachable)
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
        // run() bails at the :390-392 null-fn guard — never reaches the
        // d3/d2 guard, the spec build, or any compiled byte.
        assert!(!j.run(&mut s.meg, &mut seam, &mut s.meg_jit_wait, s.meg_const_gen));
        println!("meg_jit_b2b1: branchy+big-ram build refused, fnp/buf==0, ring committed, run() interpreter-falls");
    }
}
