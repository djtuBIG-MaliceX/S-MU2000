//! M9b — MEG (SWP30) x86-64 JIT, Phase B2a FRAME/PROLOGUE + EMITTER HELPERS
//! per JIT_M10_HANDOFF.md §3/§5.3/§8 (spec read 2026-10-04). C++ ground
//! truth: `src/mame/sound/swp30_jit.cpp` (2577 L); every block cites
//! `origin: swp30_jit.cpp:N`.
//!
//! STATUS: B2a landed the per-code exec buffer (Drop, 64 KiB growth), the
//! x64 `emit_revram_encode`/`emit_m1_expand`/`emit_revram_decode` helper
//! transliterations and the prologue/epilogue skeleton. B2b-1 landed the
//! §5.3-3 `offset_of!` offset table (MS side over `#[repr(C)] MegState`,
//! SWP side over the `MegSwpDev` pointer window — §7-B decision), the
//! :826-837 prologue now loading SEED through the window, and the §5.3-5/6
//! compile-time analysis (`branchy` / `need_tval` / `early_r/m` /
//! `last_slot_r/m`). B2b-2a lands the width-adapted 16-bit skip reset
//! (:839-840 through the u16 window slot) and the delay-ring apply legs
//! (:1000-1048 3-ring head/folded, :1050-1077 2-ring head/folded) plus the
//! `store16i`/`cmp16i_mem` emitter methods they need. B2b-2b lands the
//! :1414-1496 dm/dr op-loop legs (`emit_dm_src`/`emit_dm_store`/
//! `emit_dr_apply`) plus the `pack24`/`rnd`/`rnd_skip`/`p_packed` lambda
//! helpers (:843-889) they consume. B2b-2c lands the :1501-1525 memw + index
//! op-loop legs (`emit_memw_acc`/`emit_index_legs`) plus the shared
//! `AccFromP`/`ShrAcc`/`ShrAccTZ15` accumulator lambdas (:883-889) they and
//! the t leg consume. B2b-2d lands the :1530-1561/:1647-1675 t/tval op-loop
//! legs (`emit_t_leg` folded + `emit_t_branchy` erase-twin + the shared
//! `emit_tval_clamp` ±0x8000 pair) — all inert. B2b-2e lands the
//! :1566-1638 memop address leg (`emit_memop_addr_base`/`emit_memop_addr`/
//! `emit_memop_table`/`emit_memop` — offset+ix+ix2−SC math with baked
//! addr_mask/addr_base, the compile-time region-enable gate, and the scale-2
//! RAM store/load routed through the already-landed revram helpers) — also
//! inert. B2b-3a lands the LFO hoist head `emit_lfo` (:897-969 — a
//! transliteration of the PAIRED `meg::get_lfo`), the `emit_call_lfo`
//! callout (`:1431-1448`, `meg_jit_call_lfo` trampoline + the :1448
//! `load_p_limits`), and routes the two un-hoisted dm_src 0-3 arms through
//! them — the module's LAST `unimplemented!` is gone. B2b-3b lands the
//! FULL PROGRAM: the :971-991 LFO hoist SCAN, the :995-1679 per-op k-loop
//! stitch (`emit_program` — ring apply head/folded, the branchy skip gate
//! :1079-1104 with the u16-adapted skip legs, the ALU :1107-1409 incl. the
//! bake fold + pow2-strength reduction, and every landed leg emitter wired
//! at its C++ call point) and the loop-bound `meg_jit_upto()` gate leg
//! (CHECK-coupled per §4/§9 — default 0x180 ≡ the C++ x64 fixed bound
//! :995; only ever < 0x180 with `SMU2000_MEG_JIT_CHECK` set non-'0', the
//! inverted `env_flag_on` polarity :322-325). `build()` assembles the whole
//! program into the PRIVATE Assembler and STILL returns FALSE behind
//! `PHASE_B2_EMIT_OK = false`; with `fnp == 0` no emitted byte is ever
//! executed by the live path (the tests' X64Exec full-program parity rigs
//! are the only executors, vs the PAIRED `meg::run_program`).
//! With no compiled code the run guard :390-392 always bails, so `run()`
//! == C++ `meg_jit_run()==false` and every call site falls to the PAIRED
//! `meg::run_program` — BIT-IDENTICAL to the B1 stub (B2a acceptance rule:
//! while the flag is false `build()` must leave `fnp == 0`).
//!
//! Crate placement (handoff §7-A): smu-machine (deps smu-machine→smu-swp30;
//! the reverse is a cycle — the emitter jit_emit.rs lives here). The
//! device-side seam trait `MegJitHook` is defined in smu-swp30/mix.rs;
//! Machine owns the instances (C++ mirror: per-device `m_jit` unique_ptr,
//! swp30.h:598 — Rust uses TWO MegJits, `Machine::meg_jit` (master) +
//! `Machine::meg_jit_s` (slave), because gen/spec/ops belong to ONE program;
//! a single shared instance would cross the two MEG programs. Deviation
//! noted in the B1 return report.)
//!
//! Platform (handoff §7-G): emission lands cfg'd x86-64/Windows in B2. The
//! B1 module is cfg-free ON PURPOSE — the C++ keeps the struct (:265),
//! lifecycle (:359-516) and selftest scaffold (:520, which RETURNS 0 off
//! x86, :582-584) OUTSIDE the `SMU2000_MEG_JIT` macro gate (:27-37); only
//! build()'s emitter bodies are gated. With `build()==false` every target
//! behaves like the C++ non-JIT stub :591-595 (`return false`), which is
//! the same observable contract §7-G demands ("hook never runs elsewhere").

use std::ffi::c_void;
use std::mem::offset_of;
use std::ptr::addr_of;
use std::sync::OnceLock;

use smu_swp30::meg::{meg_step, rand_jump, run_program, MegState, MegSwp, Op};
use smu_swp30::mix::MegJitHook;

// Re-export for tests/meg_jit.rs (jit_emit stays a private crate module,
// lib.rs:86 — the brief's allowlist keeps lib.rs untouched). Same items are
// in scope here (a `pub use` is also an import).
pub use crate::jit_emit::{
    Assembler, Mem, NOREG, ARG0, ARG1, ARG2, ARG3, R8, R9, R10, R11, R12, R13, R14, R15, RAX, RBP,
    RBX, RCX, RDI, RDX, RSI, RSP,
};

/// B2a inertness lever (brief): while false, `build()` never reaches the
/// buffer handoff, so no `fnp` is ever published and no emitted byte is
/// ever executed. B2b-3c ENABLEMENT: flipped true — op loop + LFO hoist +
/// early-write analysis + offset table + CHECK leg all transliterated and
/// full-program parity green vs `meg::run_program` (S12g 53/53 ×2). The
/// byte-EQ battery (JIT入切 dense/piano, live wav byte-EQ, blocktime) is
/// the B2b-3c gate; revert to false on any stubborn red (S12g NEXT).
pub const PHASE_B2_EMIT_OK: bool = true;

// origin: swp30_jit.cpp:69-70 RENC/RDEC (x64 mode; the x86-32 ESI pairing
// at :66-67 is DEAD here — SMU_X64ASM_MODE==64 is the only live half).
const RENC: u8 = R11; // encode scratch
const RDEC: u8 = R8;  // decode scratch

// origin: swp30_jit.cpp:814 + :819 — the x64 register plan (handoff §3).
// RBX=meg_state*, R12=device*, R13=p accumulator (42-bit s64), R14=sample
// counter, R15=reverb RAM base (ARG2 per call, never baked :512), RSI=rand
// seed, RDI/ RBP = pack24 limits, R9/R10 = p saturation limits (clobbered
// by callouts — `load_p_limits` reloads after every call, :819/:1448).
pub const MS: u8 = RBX;
pub const SWP: u8 = R12;
pub const P: u8 = R13;
pub const SC: u8 = R14;
pub const RAM: u8 = R15;
pub const SEED: u8 = RSI;
pub const K_MAX: u8 = RDI;
pub const K_MIN: u8 = RBP;
pub const P_MAX: u8 = R9;
pub const P_MIN: u8 = R10;

// origin: compat/exec_mem.h — VirtualAlloc(MEM_COMMIT|MEM_RESERVE,
// PAGE_EXECUTE_READWRITE), free = VirtualFree(MEM_RELEASE); jit.rs:46-57
// precedent (handoff §7-C: per-code RWX mapping, direct memcpy on rebuild
// — the C++ make_writable/make_executable flips :1707-1711 are no-ops on
// an RWX mapping and are NOT ported).
#[link(name = "kernel32")]
extern "system" {
    fn VirtualAlloc(lpaddress: *mut c_void, dwsize: usize, flallocationtype: u32, flprotect: u32) -> *mut c_void;
    fn VirtualFree(lpaddress: *mut c_void, dwsize: usize, dwfreetype: u32) -> i32;
}
const MEM_COMMIT: u32 = 0x0000_1000;       // jit.rs:54
const MEM_RESERVE: u32 = 0x0000_2000;      // jit.rs:55
const PAGE_EXECUTE_READWRITE: u32 = 0x40;  // jit.rs:56
const MEM_RELEASE: u32 = 0x8000;           // jit.rs:57


// origin: swp30_jit.cpp:295 `static constexpr u32 STABLE = 8192` (BAKE)
pub const STABLE: u32 = 8192;
// origin: swp30_jit.cpp:332 `constexpr u32 MEG_OPS = 0x180`
pub const MEG_OPS: u32 = 0x180;
// origin: swp30_jit.cpp:817 FRAME = shadow 32 + 16 + LFO slots 96 + align 8
// (rsp stays 16-aligned ONLY with exactly 8 pushes — handoff §3/risk 5)
pub const FRAME: i32 = 152;
// origin: swp30_jit.cpp:816 LFO_SLOT_BASE = 48 (frame slots 48..144, §5.3-8)
pub const LFO_SLOT_BASE: i32 = 48;

/// origin: swp30_jit.cpp:898-901 `lfo_offsets[16]` — the phase table whose
/// ADDRESS is baked (`imm64 R8, u64(uintptr_t(lfo_offsets))` :915) into every
/// emitted `emit_lfo`, then indexed `load32 [R8 + RCX*4]` (:916). The byte
/// values are the PAIRED `meg::get_lfo` `OFFSETS` (meg.rs:381-386). A `static`
/// has one process-lifetime address (the rlib is statically linked into every
/// binary that bakes it — jit.rs:254-273 offsets precedent), so the emitted
/// table load reads the real bytes at run time. `addr_of!` (not a reference
/// coerce) avoids any vtable/padding reinterpretation.
static LFO_OFFSETS: [u32; 16] = [
    0x00000, 0x02aaa, 0x04000, 0x05555, 0x08000, 0x0aaaa, 0x0c000, 0x0d555, 0x10000, 0x12aaa,
    0x14000, 0x15555, 0x18000, 0x1aaaa, 0x1c000, 0x1d555,
];

/// origin: swp30_jit.cpp:616 `slot3 = (d3 + k) % 3` — compile-time ring
/// fold lambda (§6: "plain Rust arithmetic, no emitter method").
#[inline]
pub fn slot3(d3: u32, k: u32) -> usize {
    ((d3 + k) % 3) as usize
}

/// origin: swp30_jit.cpp:617 `slot2 = (d2 + k) % 2`.
#[inline]
pub fn slot2(d2: u32, k: u32) -> usize {
    ((d2 + k) % 2) as usize
}

// ---------------------------------------------------------------------------
// B2b-1 — offset table (origin: swp30_jit.cpp:620-652, §5.3-3) + the
// SWP-side runtime pointer window (§7-B "MegSwpDev box" decision).
//
// C++ computes the offsets from live addresses (`off(base, field)` :620)
// because MS/SWP arrive as per-call ARG pointers (:829-831); Rust pins
// them with `offset_of!` exactly like jit.rs:254-273 (`Sh2Core` is
// #[repr(C)] jit.rs:62, MegState became #[repr(C)] for this row).
//
// DEVIATION (§7-B, brief deliverable 2): the C++ SWP base is the
// `swp30_device` itself with the fields inline (:646-652). The Rust
// device fields live in `Swp30` (regs.rs — OUT of this row's allowlist,
// not repr(C), and `MegSwp` only carries BORROWED slices/references to
// them), so `offset_of!` cannot express "field of the device" through the
// seam. Decision: ARG1 is the base of a `#[repr(C)]` POINTER WINDOW
// (`MegSwpDev`) assembled from the `MegSwp` references at the call
// boundary; the emitter bakes the SLOT offset, then dereferences once
// (two loads where C++ had one — values land in the same live memory,
// interpreter semantics untouched; runtime args instead of offsets where
// noted below).
// ---------------------------------------------------------------------------

/// §7-B pointer window — the baked SWP base at runtime. One slot per
/// C++-side `off(&swp, ...)` entry (:646-652); slot order mirrors the C++
/// offset-table order so the two tables stay line-comparable.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MegSwpDev {
    pub seed: *mut u32,             // :646 `off(&swp, &swp.m_rand_seed)`
    pub flag_n: *mut bool,          // :647 (C++ u8; Rust bool is 1 B, setcc 0/1 stays valid)
    pub flag_z: *mut bool,          // :648
    pub ix2_value: *mut [i32; 3],   // :649 (doc/upstream.md 32 — 2nd index register)
    pub ix2_act: *mut [u8; 3],      // :650
    pub ram_index2: *mut i32,       // :651
    pub skip: *mut u16,             // :652 — C++ `m_meg_jit_skip` is u32 (swp30.h:614);
                                    // Rust `Swp30::meg_skip_to` is u16, so every skip
                                    // leg (:840 store, :1082 cmp) emits 16-bit width.
                                    // A/B truth is the Rust interpreter, not the C++ bytes.
}

// Window slots are plain pointers on x86-64: deterministic 8-byte stride.
const _: () = {
    assert!(std::mem::size_of::<MegSwpDev>() == 56);
    assert!(offset_of!(MegSwpDev, seed) == 0);
    assert!(offset_of!(MegSwpDev, flag_n) == 8);
    assert!(offset_of!(MegSwpDev, flag_z) == 16);
    assert!(offset_of!(MegSwpDev, ix2_value) == 24);
    assert!(offset_of!(MegSwpDev, ix2_act) == 32);
    assert!(offset_of!(MegSwpDev, ram_index2) == 40);
    assert!(offset_of!(MegSwpDev, skip) == 48);
};

impl MegSwpDev {
    /// Assemble the window from the call-time seam (§7-B laundering lives
    /// at the CALL boundary, single-threaded, strict nesting — jit.rs:70-73
    /// precedent). The returned window is valid only while `swp`'s
    /// borrows are alive; build the frame of one `run()` at a time.
    /// revram_enable/reverb_ram deliberately have NO slot: reverb RAM is
    /// ARG2 per call (:512, never baked) and revram_enable is gated at
    /// compile time (:1589) + re-checked in `run()` (:396-400) — a baked
    /// value would go stale exactly the way :394-400 guards against.
    pub fn from_swp(swp: &mut MegSwp<'_>) -> MegSwpDev {
        MegSwpDev {
            seed: swp.seed as *mut u32,
            flag_n: swp.flag_n as *mut bool,
            flag_z: swp.flag_z as *mut bool,
            ix2_value: swp.ix2_value as *mut [i32; 3],
            ix2_act: swp.ix2_act as *mut [u8; 3],
            ram_index2: swp.ram_index2 as *mut i32,
            skip: swp.skip_to as *mut u16,
        }
    }
}

/// origin: swp30_jit.cpp:621-652 — the baked field offsets, jit.rs:233
/// `Offs` precedent. MS side over `#[repr(C)] MegState` (:621-643); SWP
/// side over `MegSwpDev` slots (:646-652). sintab (:645) is NOT here —
/// it is captured once per compile into `MegJit::sintab` (baked at :937
/// when len >= 0x8000, else the callout fallback :1430+); the RAM base is
/// ARG2 (:611-612 RAM guard + :831 load); `m_program` needs no runtime
/// slot (read only at compile time via the ops copy, §I).
pub struct Offs {
    // :621-643 — meg_state fields, MS (RBX) base
    pub m: i32,          // :621 m_m          s32[0x40]
    pub r: i32,          // :622 m_r          s32[0x80]
    pub t: i32,          // :623 m_t          s16[8]
    pub p: i32,          // :624 m_p          s64
    pub konst: i32,      // :625 m_const      s16[0x180]
    pub offset: i32,     // :626 m_offset     u16[0x80]
    pub mw_value: i32,   // :627 m_mw_value   s32[3]
    pub mw_reg: i32,     // :628 m_mw_reg     u8[3]
    pub rw_value: i32,   // :629 m_rw_value   s32[3]
    pub rw_reg: i32,     // :630 m_rw_reg     u8[3]
    pub ix_value: i32,   // :631 m_index_value s32[3]
    pub ix_act: i32,     // :632 m_index_active (Rust bool[3], 1 B — :654 width)
    pub memw_val: i32,   // :633 m_memw_value s32[3]
    pub memr_val: i32,   // :634 m_memr_value s32[3]
    pub t_value: i32,    // :635 m_t_value    s16[2]
    pub memw_act: i32,   // :636 m_memw_active
    pub memr_act: i32,   // :637 m_memr_active
    pub ram_read: i32,   // :638 m_ram_read   u32
    pub ram_write: i32,  // :639 m_ram_write  u32
    pub ram_index: i32,  // :640 m_ram_index  s32
    pub sample: i32,     // :641 m_sample_counter u32
    pub lfo: i32,        // :642 m_lfo        u16[0x18]
    pub lfo_counter: i32, // :643 m_lfo_counter u32[0x18]
    // :646-652 — device fields, now MegSwpDev SLOTS (base = ARG1 window)
    pub seed: i32,       // :646
    pub flag_n: i32,     // :647
    pub flag_z: i32,     // :648
    pub ix2_value: i32,  // :649
    pub ix2_act: i32,    // :650
    pub ram_index2: i32, // :651
    pub skip: i32,       // :652
}

/// The compile-time table (jit.rs `offs()` precedent — offset_of! is a
/// const-folded constant read once at build time, never on the hot path).
pub fn offs() -> Offs {
    Offs {
        m: offset_of!(MegState, m) as i32,            // :621
        r: offset_of!(MegState, r) as i32,            // :622
        t: offset_of!(MegState, t) as i32,            // :623
        p: offset_of!(MegState, p) as i32,            // :624
        konst: offset_of!(MegState, konst) as i32,    // :625 m_const
        offset: offset_of!(MegState, offset) as i32,  // :626
        mw_value: offset_of!(MegState, mw_value) as i32, // :627
        mw_reg: offset_of!(MegState, mw_reg) as i32,  // :628
        rw_value: offset_of!(MegState, rw_value) as i32, // :629
        rw_reg: offset_of!(MegState, rw_reg) as i32,  // :630
        ix_value: offset_of!(MegState, index_value) as i32, // :631
        ix_act: offset_of!(MegState, index_active) as i32,  // :632
        memw_val: offset_of!(MegState, memw_value) as i32,  // :633
        memr_val: offset_of!(MegState, memr_value) as i32,  // :634
        t_value: offset_of!(MegState, t_value) as i32,      // :635
        memw_act: offset_of!(MegState, memw_active) as i32, // :636
        memr_act: offset_of!(MegState, memr_active) as i32, // :637
        ram_read: offset_of!(MegState, ram_read) as i32,    // :638
        ram_write: offset_of!(MegState, ram_write) as i32,  // :639
        ram_index: offset_of!(MegState, ram_index) as i32,  // :640
        sample: offset_of!(MegState, sample_counter) as i32, // :641
        lfo: offset_of!(MegState, lfo) as i32,              // :642
        lfo_counter: offset_of!(MegState, lfo_counter) as i32, // :643
        seed: offset_of!(MegSwpDev, seed) as i32,           // :646
        flag_n: offset_of!(MegSwpDev, flag_n) as i32,       // :647
        flag_z: offset_of!(MegSwpDev, flag_z) as i32,       // :648
        ix2_value: offset_of!(MegSwpDev, ix2_value) as i32, // :649
        ix2_act: offset_of!(MegSwpDev, ix2_act) as i32,     // :650
        ram_index2: offset_of!(MegSwpDev, ram_index2) as i32, // :651
        skip: offset_of!(MegSwpDev, skip) as i32,           // :652
    }
}

// ---------------------------------------------------------------------------
// B2b-1 — compile-time op analysis (origin: swp30_jit.cpp:607-610,
// :660-666, :668-711, :715-721; §5.3-1/5/6). Pure functions over the ops
// copy — no runtime effect while PHASE_B2_EMIT_OK keeps the op loop from
// consuming them (brief deliverable 3: "analysis scaffolding ... still no
// runtime effect while emit flag false").
// ---------------------------------------------------------------------------

/// The four analyses build() will consume per §5.3: `branchy` (:607-610)
/// selects ring-per-op mode; `need_tval` (:660-666) which ops must
/// publish their 2-ring t slot; `early_r/early_m` (:668-711, gated
/// `early_on && !branchy` :707-710) which 3-cycle writes may be stored
/// DIRECTLY at the writing op; `last_slot_r/m` (:715-721) which folded
/// writes are still the LAST for their ring slot and so must ALSO write
/// the slot ("not read, but keeps the state save identical" :713-714 —
/// the Rust invariant: m_mw_value/... are STATE, serialized).
pub struct Analysis {
    pub branchy: bool,                 // :607-610
    pub need_tval: [bool; 0x180],      // :660-666
    pub early_r: [bool; 128],          // :672/:707-710 (reg 0 never early :707)
    pub early_m: [bool; 128],          // :672/:707-710
    pub last_slot_r: [bool; 0x180],    // :715-721
    pub last_slot_m: [bool; 0x180],    // :715-721
}

pub fn analyze_ops(ops: &[Op; 0x180], early_on: bool) -> Analysis {
    // :607-610 — "programs with branches (only jump backwards) translate
    // the delay rings read+written every instruction like the interpreter"
    let branchy = ops.iter().any(|o| o.jump != 0);

    // :660-666 — ops that must publish their t_value ring slot: the last
    // two ops of the program, or any op 2 slots before a t-write-from-p
    // (the folded alternative of meg.rs:702/:737 t_value[d2] = p-clamped).
    let mut need_tval = [false; 0x180];
    for k in 0..0x180u32 {
        if k >= 0x17e {
            need_tval[k as usize] = true; // :662-663 tail
        }
        // :664-665 (ops[k+2] bounds: k+2 < 0x180, no ring wrap here)
        if k + 2 < 0x180 && ops[(k + 2) as usize].t_write != 0 && ops[(k + 2) as usize].t_from_p != 0 {
            need_tval[k as usize] = true;
        }
    }

    // :668-711 — early-write analysis. A 3-cycle write may go straight to
    // m[r/regs] at the writing op iff no read of that register sits 1 or
    // 2 ops later (window wraps across the sample boundary — the ring
    // continues), and the register is not one the tail trio 0x17d-0x17f
    // writes (those enter the ring at the NEXT sample's head, :670-671).
    let reads = |o: &Op, m: bool, x: u8| -> bool {
        if x == 0 {
            return false; // :676-677
        }
        let mut rd = false;
        // :679-680 — mmode 2/3 m2 source read ((m2_from_m != 0) == m picks bank)
        if o.alu != 0 && (o.mmode == 2 || o.mmode == 3) && (o.m2_from_m != 0) == m && if m { o.sm == x } else { o.sr == x } {
            rd = true;
        }
        // :681-682 — asel 1 reads r[x] into the ALU a-term
        if o.alu != 0 && !m && o.asel == 1 && o.sr == x {
            rd = true;
        }
        // :683-684 — asel 2 reads m[x]
        if o.alu != 0 && m && o.asel == 2 && o.sm == x {
            rd = true;
        }
        // :685-686 — dr reading r (dr_from_r)
        if !m && o.dr != 0 && o.dr_from_r != 0 && o.sr == x {
            rd = true;
        }
        // :687-688 — dm_src 7 reading m[x]
        if m && o.dm != 0 && o.dm_src == 7 && o.sm == x {
            rd = true;
        }
        rd
    };
    let mut bad_r = [false; 128];
    let mut bad_m = [false; 128];
    // :691-698 — window: for every read at j, writes at j-1 and j-2
    // ((j + 0x180 - back) % 0x180 wraps the ring across samples).
    // Faithful direct index: build_ops masks sr < 0x80 / sm < 0x40 and
    // dm/dr ride those same decoded fields (meg.rs build_ops), so the
    // 128-slot tables cannot be left of range (C++ relies on the same).
    for j in 0..0x180u32 {
        for back in 1..=2u32 {
            let w = &ops[((j + 0x180 - back) % 0x180) as usize];
            if w.dr != 0 && reads(&ops[j as usize], false, w.dr) {
                bad_r[w.dr as usize] = true; // :694-695
            }
            if w.dm != 0 && reads(&ops[j as usize], true, w.dm) {
                bad_m[w.dm as usize] = true; // :696-697
            }
        }
    }
    // :699-702 — tail trio 0x17d-0x17f writes are never early candidates
    for k in 0x17d..0x180 {
        if ops[k].dr != 0 {
            bad_r[ops[k].dr as usize] = true;
        }
        if ops[k].dm != 0 {
            bad_m[ops[k].dm as usize] = true;
        }
    }
    // :703-710 — early_on = SMU2000_MEG_EARLY static-once (C++
    // getenv :703-706) — Rust resolves it in MegJit::new (§4) and hands
    // it in; branchy programs fold NOTHING early (:707-709). Slot 0 is
    // never a candidate (:707 `x = 1; x != 128`).
    let mut early_r = [false; 128];
    let mut early_m = [false; 128];
    for x in 1..128usize {
        early_r[x] = early_on && !branchy && !bad_r[x];
        early_m[x] = early_on && !branchy && !bad_m[x];
    }

    // :715-721 — even an early (folded) write still lands in its ring
    // slot when it is the LAST writer for that slot (k%3 grouping matches
    // slot3's dependency on k mod 3): "not read, but keeps the state
    // save identical". First hit walking DOWN per class wins per slot.
    let mut last_slot_r = [false; 0x180];
    let mut last_slot_m = [false; 0x180];
    for c in 0..3usize {
        let mut k = 0x17fusize; // :717 `for (int k = 0x17f; k >= 0; k--)`
        loop {
            if k % 3 == c && ops[k].dr != 0 {
                last_slot_r[k] = true; // :718
                break;
            }
            if k == 0 {
                break;
            }
            k -= 1;
        }
        let mut k = 0x17fusize; // :719
        loop {
            if k % 3 == c && ops[k].dm != 0 {
                last_slot_m[k] = true; // :720
                break;
            }
            if k == 0 {
                break;
            }
            k -= 1;
        }
    }

    Analysis { branchy, need_tval, early_r, early_m, last_slot_r, last_slot_m }
}


// ---------------------------------------------------------------------------
// env flags — §4 table. jit.rs `env_flag` idiom (jit.rs:148-154); resolved
// once per construction (S9 precedent jit.rs:305-311; C++ reads once at
// file scope :311-317 for the same "no getenv in the audio path" reason).
// ---------------------------------------------------------------------------

/// C++ idiom `!(e && e[0]=='0')` — set AND first char '0' ⇒ off.
pub fn env_flag(name: &str, off: &str) -> bool {
    // origin: jit.rs:148-154 (mirror of swp30_jit.cpp:319-321/:350-351)
    match std::env::var_os(name) {
        Some(v) => !v.to_string_lossy().starts_with(off),
        None => true,
    }
}

/// INVERTED polarity helper (§4 row SMU2000_MEG_JIT_CHECK, §7-F): the C++
/// test is `e && e[0] != '0'` — OFF unless set with a non-'0' first char.
/// NOT expressible with `env_flag`. :322-325.
pub fn env_flag_on(name: &str) -> bool {
    // origin: swp30_jit.cpp:323-324
    std::env::var_os(name).is_some_and(|v| !v.to_string_lossy().starts_with('0'))
}

/// origin: swp30_jit.cpp:333-342 `meg_jit_upto()` — static-once. Only
/// meaningful with CHECK set (:328-331: "a stray variable must not be able
/// to silently truncate a compiled program" — without CHECK this is the
/// whole block).
pub fn meg_jit_upto() -> u32 {
    // origin: swp30_jit.cpp:335-340
    static UPTO: OnceLock<u32> = OnceLock::new();
    *UPTO.get_or_init(|| {
        if !MEG_JIT_CHECK.get_or_init(|| env_flag_on("SMU2000_MEG_JIT_CHECK")) {
            return MEG_OPS; // :336-337
        }
        match std::env::var_os("SMU2000_MEG_JIT_UPTO") {
            // atoi parity: parse the leading unsigned decimal; unparseable
            // => 0 like atoi garbage-in (the bisect knob is debug-only)
            Some(v) => v.to_string_lossy().trim().parse::<u32>().unwrap_or(0), // :339
            None => MEG_OPS,
        }
    })
}

/// shared static of the C++ file-scope g_meg_check (:322-325) — meg_jit_upto
/// must read the SAME resolved value (§4 "meaningless without CHECK" coupling).
static MEG_JIT_CHECK: OnceLock<bool> = OnceLock::new();

// ---------------------------------------------------------------------------
// code — origin: swp30_jit.cpp:269-284 `struct meg_jit::code`
// ---------------------------------------------------------------------------

/// One translated program (C++ `code`: fn/buf/buf_size/d3/d2/revram_enable).
/// `fnp == 0` mirrors `fn = nullptr` (:270). B2a landed the exec buffer:
/// `buf`/`buf_size` are the per-code RWX VirtualAlloc (§7-C, jit.rs:46-57
/// precedent), grown at 64 KiB granularity (:1699) and freed by `Drop`
/// (C++ dtor :277-283 + meg_jit_delete :304-307). "Rebuilds arrive still
/// executable" (:1706) — on the RWX mapping a rebuild is a plain memcpy.
pub struct Code {
    fnp: usize,          // :270 fn_t (:266) — 0 = nullptr
    buf: *mut u8,        // :271 — kept across rebuilds (:1706)
    buf_size: usize,     // :272
    pub d3: u32,         // :273 — saved ring snapshot, RE-CHECKED EVERY CALL :423-424
    pub d2: u32,         // :274 (same)
    revram_enable: u16,  // :274-276 — regions' enable latch at compile time
}

// Code holds a raw pointer, so it is not Send/Sync by default; MegJit lives
// per SWP device on the audio-thread side (handoff §7-H: the seed and the
// compiled code stay inside the audio thread only).
unsafe impl Send for Code {}

impl Code {
    /// C++ in-class initializers :270-276, spelled out (Invariant 3).
    pub fn new() -> Code {
        Code {
            fnp: 0,      // fn = nullptr
            buf: std::ptr::null_mut(), // buf = nullptr
            buf_size: 0,
            d3: 0,
            d2: 0,
            revram_enable: 0,
        }
    }

    /// origin: swp30_jit.cpp:1699-1700 — grow to `need` bytes rounded UP to
    /// 64 KiB granularity, fresh RWX mapping. Caller frees the old buffer
    /// first (:1697-1698). False = the alloc failed (:1701-1704).
    pub fn alloc_buf(&mut self, need: usize) -> bool {
        let size = (need + 0xffff) & !0xffffusize; // :1699
        let p = unsafe {
            VirtualAlloc(std::ptr::null_mut(), size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE)
        };
        if p.is_null() {
            return false; // :1701-1704 (caller zeroes buf_size)
        }
        self.buf = p as *mut u8;
        self.buf_size = size;
        true
    }

    /// origin: swp30_jit.cpp:1697-1698 free-before-regrow + :277-283 dtor
    /// (exec_mem::free_mem == VirtualFree(MEM_RELEASE), jit.rs precedent).
    pub fn release_buf(&mut self) {
        if !self.buf.is_null() {
            unsafe { VirtualFree(self.buf as *mut c_void, 0, MEM_RELEASE); }
            self.buf = std::ptr::null_mut();
            self.buf_size = 0;
        }
    }

    /// B2a test pin: (fnp, buf, buf_size). The B2a inertness contract is
    /// exactly `fnp == 0` after any build(); the buffer roundtrip test
    /// checks buf/size track the VirtualAlloc handoff (:1696-1712).
    pub fn exec_state(&self) -> (usize, usize, usize) {
        (self.fnp, self.buf as usize, self.buf_size)
    }
}

/// origin: swp30_jit.cpp:277-283 `~code()` + meg_jit_delete :304-307
/// (handoff §5.3-11). Compiled code is never persisted (§7-J) — the
/// mapping dies with the instance.
impl Drop for Code {
    fn drop(&mut self) {
        self.release_buf();
    }
}

// ---------------------------------------------------------------------------
// MegJit — origin: swp30_jit.cpp:265-302 `struct swp30_device::meg_jit`
// ---------------------------------------------------------------------------

pub struct MegJit {
    // origin: swp30_jit.cpp:286-289 (gen: runtime-constants version, always
    // usable; spec: baked-constants version, built after STABLE stable
    // samples, dropped the moment the const generation changes)
    gen: Code,
    spec: Code,
    /// §I deviation: C++ keeps a raw `ops` pointer rebound per rebuild
    /// (:290, :368); Rust COPIES the 0x180-entry table — safe because every
    /// ops mutation is paired with invalidate + the 64-sample interpret
    /// window before the next rebuild (§5.1-5.2).
    ops: Box<[Op; 0x180]>,
    spec_const_gen: u32, // :291
    seen_const_gen: u32, // :292
    stable: u32,         // :293
    spec_tried: bool,    // :294
    // ---- env flags (§4, resolved in new()) ----
    enabled: bool,       // SMU2000_MEG_JIT master switch, :346-357
    bake: bool,          // SMU2000_MEG_BAKE, :318-321
    // B2-only (build()/CHECK emit reads these; §4/§5.3-6/§9). Unused in B1.
    #[allow(dead_code)]
    check: bool,         // SMU2000_MEG_JIT_CHECK (INVERTED polarity, :322-325)
    early: bool,         // SMU2000_MEG_EARLY, :703-706 — read by build()'s
                         // analyze_ops gate (B2b-1; C++ static-once :703-706)
    #[allow(dead_code)]
    stats: bool,         // SMU2000_MEG_JIT_STATS, :1713-1731 (stderr at BUILD time only — never in run())
    // sintab base is BAKED at compile time (:645, :937 — handoff §3/§7-D):
    // captured once per rebuild. SAFETY: the Machine keeps the table pinned
    // (`sintab_pin`, set_sintab_pin lib.rs) for the process lifetime, and
    // every machine (re)init flips meg_program_changed -> invalidate before
    // any stale code can run (§7-D assert pin: tests/meg_jit.rs).
    sintab: *const u16,
    sintab_len: usize,
    /// CHECK-leg persistent buffers (see `CheckScratch`). Never touched
    /// when CHECK is off — zero effect on the gated (no-CHECK) path.
    scratch: std::cell::RefCell<CheckScratch>,
}

/// CHECK A/B scratch for `check_ab` (:426-511). The C++ heap-allocates
/// `ram0`/`ramj` EVERY call (:431/:436) — on this box that per-sample
/// VirtualAlloc/fault churn made a dense 1 s CHECK render take minutes
/// (2026-10-05 slow-leg hunt). The port allocates ONCE and reuses; same
/// bytes compared. Explicit construction (Invariant 3).
struct CheckScratch {
    ram0: Vec<u16>, // :431 — pre-JIT reverb RAM (restore)
    ramj: Vec<u16>, // :436 — JIT-side reverb RAM result
    jimg: Vec<u8>,  // :449 `jb` — post-JIT state byte image
    iimg: Vec<u8>,  // :450 `ib` — post-interpreter state byte image
}

unsafe impl Send for MegJit {}

impl MegJit {
    /// Construct with env flags decided once (mirrors jit.rs:305-339 `Jit::new`
    /// — "env lookups read once", C++ rationale :311-317). Every field is
    /// explicit (Invariant 3 — no Default devices).
    pub fn new() -> MegJit {
        // origin: swp30_jit.cpp:346-357 meg_jit_enabled — ON unless "0..."
        let enabled = env_flag("SMU2000_MEG_JIT", "0");
        // origin: swp30_jit.cpp:318-321 g_meg_bake — ON unless "0..."
        let bake = env_flag("SMU2000_MEG_BAKE", "0");
        // origin: swp30_jit.cpp:322-325 g_meg_check — OFF unless set non-'0'
        // (inverted polarity — §4/§7-F env_flag_on; seeds the shared OnceLock)
        let check = MEG_JIT_CHECK.get_or_init(|| env_flag_on("SMU2000_MEG_JIT_CHECK")).to_owned();
        let _ = meg_jit_upto(); // resolve the coupled static eagerly (:333-342)
        // origin: swp30_jit.cpp:703-706 SMU2000_MEG_EARLY — ON unless "0..."
        let early = env_flag("SMU2000_MEG_EARLY", "0");
        // origin: swp30_jit.cpp:1713-1731 SMU2000_MEG_JIT_STATS — OFF unless
        // set non-'0' (same inverted shape as CHECK; readers arrive with the
        // PHASE B2 build() — stderr there only, NEVER in run())
        let stats = env_flag_on("SMU2000_MEG_JIT_STATS");
        MegJit {
            gen: Code::new(),
            spec: Code::new(),
            ops: Box::new([Op::ZERO; 0x180]), // §I copy; Op::ZERO = `op{}` meg.rs:1059
            spec_const_gen: 0,
            seen_const_gen: 0,
            stable: 0,
            spec_tried: false,
            enabled,
            bake,
            check,
            early,
            stats,
            sintab: std::ptr::null(),
            sintab_len: 0,
            scratch: std::cell::RefCell::new(CheckScratch {
                ram0: Vec::new(),
                ramj: Vec::new(),
                jimg: Vec::new(),
                iimg: Vec::new(),
            }),
        }
    }

    /// Machine-level gate (jit.rs:344 `can` pattern): compile-time platform
    /// (none needed until PHASE B2 emission, see module §7-G note) × the
    /// runtime env flags. hash_on/trace_on force the caller to
    /// `force_off()` (lib.rs open_pc_hash/open_pc_trace — jit.rs:342-343).
    pub fn can(&self) -> bool {
        // origin: swp30_jit.cpp:346-357 + handoff §4 row 1
        self.enabled
    }

    /// Handoff §4 last row: trace/hash runs stay interpreter-only (no MEG
    /// CHECK analogue, jit.rs:15-18/:312-317 precedent).
    pub fn force_off(&mut self) {
        self.enabled = false;
        self.invalidate();
    }

    /// B2a test pin: the gen code's (fnp, buf, buf_size) — the brief rule
    /// "while PHASE_B2_EMIT_OK == false, build() must not leave a non-zero
    /// fnp" is asserted through this (gen/spec are private by design; C++
    /// keeps them inside meg_jit too, :286-289).
    pub fn gen_code_state(&self) -> (usize, usize, usize) {
        self.gen.exec_state()
    }

    /// B2b-1 test pin: the :614-615 ring snapshot COMMITTED to the gen
    /// code by build() (the load-bearing run-guard :423-424 reads exactly
    /// these). Proves the snapshot lands even while a refused build keeps
    /// fnp == 0 — same shape as C++'s post-guard refusals (:1703).
    pub fn gen_ring(&self) -> (u32, u32) {
        (self.gen.d3, self.gen.d2)
    }

    /// C++ `m_jit.reset()` leg of meg_jit_rebuild (:361-364): with the
    /// switch off nothing may stay compiled. The Rust instance is
    /// Machine-owned (C++ deletes the unique_ptr), so "reset" == clear the
    /// compiled state (observable difference is zero — a null `fnp` is what
    /// the delete would have produced for the run path).
    fn reset_compiled(&mut self) {
        // origin: swp30_jit.cpp:361-363 (+ :373-376 shape)
        self.gen.fnp = 0;
        self.spec.fnp = 0;
        self.spec_tried = false;
        self.spec_const_gen = 0;
        self.stable = 0;
    }

    /// origin: swp30_jit.cpp:598-1756 (x64 arms; §5.3 sequence). B2b-1
    /// scope = §5.3-1/2/3/4/5/6 (branchy + RAM guard + ring snapshot +
    /// offset table + width pins + op analysis) and the frame skeleton.
    /// The B2b-2+ TODO blocks below map the remaining §5.3 steps; while
    /// `PHASE_B2_EMIT_OK == false` the buffer handoff (:1696-1712) is
    /// unreachable, so no `fnp` is ever published — the observable contract
    /// stays the C++ non-JIT stub :591-595 ("そのほかでは build() が false を
    /// 返し、今までどおり解釈実行する", header :19): run() stays
    /// unreachable, audio BIT-IDENTICAL.
    #[allow(unreachable_code)]
    fn build(
        cd: &mut Code,
        ms: &MegState,
        ops: &[Op; 0x180],
        ram_len: usize,
        revram_enable: u16,
        bake: bool,
        early: bool,
        sintab_ptr: *const u16,
        sintab_len: usize,
    ) -> bool {
        // :604 `fn = nullptr` FIRST — and every early return below keeps it
        // null (the brief's B2a rule: build() must never leave a non-zero fnp).
        cd.fnp = 0;

        // :607-610 branchy scan + :660-666 need_tval + :668-711 early-write
        // (gated `early_on && !branchy`, MEG_EARLY static :703-706 mirrored
        // by the `early` arg resolved in MegJit::new) + :715-721 last_slot.
        // C++ interleaves these with the guards below; every part is pure,
        // so batching them cannot change any decision (only the RAM guard
        // returns early). Results feed the op loop in B2b-2 (handoff §5.3-
        // 1/5/6); while the emit flag is false NOTHING consumes them at
        // runtime (brief deliverable 3).
        let an = analyze_ops(ops, early);

        // :611-612 RAM guard — reverb_ram smaller than a full 0x40000 map
        // means the baked scale-2 addressing could leave the mapping
        // (meg.rs:628-629 reverb_ram is state-sized). Identical decision
        // point to C++; B2b still refuses afterwards either way.
        if ram_len < 0x40000 {
            return false;
        }

        // :614-617 ring snapshot COMMITTED to the code (C++ commits these
        // before any later refusal — e.g. the :1703 alloc failure leaves
        // d3/d2 written too; fn==null keeps the :423-424 guard unreachable
        // until B2 flips the flag). slot3/slot2 (:616-617) are the pub fns
        // above (handoff §6: plain Rust arithmetic).
        cd.d3 = ms.delay_3; // :614
        cd.d2 = ms.delay_2; // :615

        // :620-652 offset table — offset_of! over #[repr(C)] MegState +
        // the MegSwpDev pointer window (jit.rs:269 precedent, §7-B/D).
        // :645 sintab capture lands in MegJit.sintab at rebuild() already
        // (baked at :937 when count >= 0x8000, else callout fallback :1430+).
        // :654-657 layout static asserts — const _ pins at the file bottom
        // (now over the REAL field element types, not bare primitives).
        let o = offs();

        // :645 — sintab capture, the GATE included: machine-code LFO only when
        // the table is resident for a full 1/4 period (0x8000); otherwise the
        // dm_src 0-3 legs take the :1431-1448 callout. `sintab_ptr`/`sintab_len`
        // are the rebuild() capture (§7-D pin — the Machine keeps the table
        // process-pinned; an empty slice's dangling ptr never escapes here).
        let sintab = if sintab_len >= 0x8000 { sintab_ptr } else { std::ptr::null() };

        // :730 assembler — the WHOLE program now: prologue :826-837, the
        // branchy skip reset :839-840 (u16-adapted, B2b-2a), the :971-991
        // LFO hoist SCAN, the :995-1679 per-op k-loop (emit_program, B2b-3b)
        // and the epilogue :1688-1694. `revram_enable` feeds the :1589
        // compile-time region gate inside emit_memop; `bake` the :1111 ALU
        // const fold and the :1534 t-konst bake. The bytes live ONLY in this
        // private Assembler until the PHASE gate below flips — the observable
        // contract stays the C++ non-JIT stub :591-595 (fnp==0, run() falls
        // to the PAIRED interpreter, audio BIT-IDENTICAL). tests/meg_jit.rs
        // executes full programs through `program_bytes` (the same
        // emit_program) against `meg::run_program` — never a half-emitted
        // program and never a published fnp while the flag is false.
        let mut a = Assembler::new();
        emit_program(&mut a, &o, ms, ops, revram_enable, bake, &an, cd.d3, cd.d2, sintab, sintab_len);

        // THE single emission gate (B2b-3c flipped it true — the handoff
        // below and fnp publication are LIVE). Setting the const back to
        // false re-enters stub parity :591-595 (fnp==0, run() falls to the
        // PAIRED interpreter, audio BIT-IDENTICAL) — the standing revert
        // lever from the NEXT gate list.
        if !PHASE_B2_EMIT_OK {
            return false; // stub parity :591-595; fnp already 0 (:604)
        }

        // :1696-1712 buffer handoff (grow 64 KiB-granular :1699; §7-C RWX
        // direct memcpy, no W^X flips). Only reached when the whole program
        // has been emitted and checked.
        if a.code.len() > cd.buf_size {
            cd.release_buf(); // :1697-1698 (no-op when buf is null)
            if !cd.alloc_buf(a.code.len()) {
                cd.buf_size = 0; // :1702
                return false;    // :1703
            }
        }
        // SAFETY: RWX mapping of cd.buf_size >= a.code.len() bytes, just
        // (re)allocated or untouched since; single-threaded audio path.
        unsafe { std::ptr::copy_nonoverlapping(a.code.as_ptr(), cd.buf, a.code.len()) };
        cd.fnp = cd.buf as usize; // :1712 `fn = (fn_t)buf`
        true
    }
}

impl MegJitHook for MegJit {
    /// origin: swp30_jit.cpp:346-357 (via can())
    fn can(&self) -> bool {
        MegJit::can(self)
    }

    /// origin: swp30_jit.cpp:380-386 `meg_jit_invalidate` — "訳した物を
    /// 使えなくする（次に meg_jit_rebuild() を呼ぶまで解釈実行で回る）".
    /// Code buffers are KEPT (re-build overwrites them, :1696-1712).
    fn invalidate(&mut self) {
        if true {
            // :382 `if (m_jit)` — the Rust instance always exists (Machine
            // field); nulling both fnps is the C++ body :383-384.
            self.gen.fnp = 0;
            self.spec.fnp = 0;
        }
    }

    /// origin: swp30_jit.cpp:359-377 `meg_jit_rebuild` (handoff §5.2):
    /// rebind ops (:368 → Rust copy, §I), build(gen) (:369-370), latch
    /// gen.revram_enable (:371), drop spec (:373-374), seen_const_gen (:375),
    /// stable=0 (:376). Buffer growth at rebuild-on-the-audio-thread is the
    /// §7-C accepted deviation (C++ does the same via :1700 from
    /// swp30.cpp:4416, amortized 64 KiB steps :1699-1705).
    fn rebuild(
        &mut self,
        meg: &MegState,
        ops: &[Op; 0x180],
        ram_len: usize,
        revram_enable: u16,
        sintab: &[u16],
        const_gen: u32,
    ) -> bool {
        // :361-364 — switch off: C++ m_jit.reset(); Rust clears (:363)
        if !self.enabled {
            self.reset_compiled();
            return false;
        }
        // :368 `j.ops = m_meg_ops.data()` — §I: copy instead of rebind
        self.ops.copy_from_slice(ops);
        // §3/:645: sintab captured ONCE per compile (baked at :937 when the
        // table is resident, else callout fallback :1430+ — PHASE B2 reads
        // this pin; empty pins a dangling-but-unused ptr, set_sintab_pin
        // lib.rs convention)
        self.sintab = sintab.as_ptr();
        self.sintab_len = sintab.len();
        // :369-370 — false ⇒ gen.fn=null, interpret until the next rebuild
        let built =
            Self::build(
                &mut self.gen,
                meg,
                &self.ops,
                ram_len,
                revram_enable,
                false,
                self.early,
                self.sintab,
                self.sintab_len,
            );
        if !built {
            self.gen.fnp = 0;
        }
        // :371 latch the region-enable snapshot the gen build saw
        self.gen.revram_enable = revram_enable;
        // :373-374 プログラムか番地が変わったので、焼き込んだ版は作り直す
        self.spec.fnp = 0;
        self.spec_tried = false;
        // :375-376
        self.seen_const_gen = const_gen;
        self.stable = 0;
        built
    }

    /// origin: swp30_jit.cpp:388-516 `meg_jit_run` (handoff §5.4). Ported
    /// ORDER: null guards :390-392 → revram_enable re-check :396-400 →
    /// BAKE/spec state machine :403-421 → d3/d2 guard :423-424 → [CHECK
    /// A/B leg :426-511 — PHASE B2, see §9] → fn call :512 → the
    /// caller-side half of run_program :513-514 (run_program subtracts the
    /// 0x180 itself, CHECK comment :451-453 — the mirror of meg.rs:1000-1002).
    fn run(
        &mut self,
        meg: &mut MegState,
        swp: &mut MegSwp,
        jit_wait: &mut u32,
        const_gen: u32,
    ) -> bool {
        // :390-392 — meg_jit *j = m_jit.get(); if (!j || !j->gen.fn) return
        // false. THE reachability gate: while PHASE B2 keeps build()==false
        // this fires on every sample (== C++ meg_jit_run()==false, the only
        // disk path the MEG row gated) — mix.rs then runs meg_run_program.
        if self.gen.fnp == 0 {
            return false;
        }

        // :394-400 — belt-and-braces re-check: revram_enable moved under
        // the compiled code. Unreachable in B1; ported verbatim so B2
        // inherits it (regs.rs:827 revram_enable_w already latches
        // meg_jit_wait=1 — disk invalidates there too, :2488-2491, but the
        // regs.rs leg cannot reach a Machine-owned hook: §7-A "rely on the
        // run()-time re-check").
        if self.gen.revram_enable != swp.revram_enable {
            self.invalidate(); // :397
            *jit_wait = 1; // :398 m_meg_jit_wait = 1
            return false; // :399
        }

        // :402-421 — BAKE/spec state machine (handoff §5.2 second leg,
        // §5.4 order). cg == the device's m_meg_const_gen (const_w seam
        // meg.rs:288 bumped it on the last const write).
        let use_spec;
        if self.bake {
            let cg = const_gen; // :404
            if cg != self.seen_const_gen {
                // :405-408 — constants moved: back to gen, re-arm spec
                self.seen_const_gen = cg;
                self.stable = 0;
                self.spec_tried = false;
            } else if self.stable < STABLE {
                self.stable += 1; // :409-410
            }
            if self.spec.fnp != 0 && self.spec_const_gen == cg {
                use_spec = true; // :411-412 spec valid for the current gen
            } else if self.stable >= STABLE && !self.spec_tried {
                self.spec_tried = true; // :414
                // :415 build(spec, bake=true) — PHASE B2 (build()==false ⇒
                // this leg, like the whole run path, stays unreachable)
                if Self::build(
                    &mut self.spec,
                    meg,
                    &self.ops,
                    swp.reverb_ram.len(),
                    swp.revram_enable,
                    true,
                    self.early,
                    self.sintab,
                    self.sintab_len,
                ) {
                    self.spec_const_gen = cg; // :416
                    use_spec = true; // :417
                } else {
                    self.spec.fnp = 0; // :419
                    use_spec = false; // :418-419 fall to gen
                }
            } else {
                use_spec = false; // :402 c = &j->gen
            }
        } else {
            use_spec = false; // :402 g_meg_bake off ⇒ gen only
        }

        // :423-424 — THE load-bearing guard (handoff §5.3-2 / risk 2): a
        // d3/d2 mismatch (state reload mid-stream) ⇒ false ⇒ interpreter.
        let c = if use_spec { &self.spec } else { &self.gen };
        if c.d3 != meg.delay_3 || c.d2 != meg.delay_2 {
            return false;
        }

        // :426-511 — CHECK A/B leg (SMU2000_MEG_JIT_CHECK, handoff §9):
        // snapshot meg_state/reverb_ram/seed/flags, run the JIT, restore,
        // run the PAIRED interpreter from the same start (full block, or
        // `upto` per-op steps for the bisect :444-448), report the first
        // difference. The env polarity helper `env_flag_on` +
        // `meg_jit_upto` landed in B1; the build side already truncates the
        // k loop at the SAME static (emit_program :2361) — exactly the C++
        // "ignored unless the check above is on" coupling (:335-337/:2035).
        // NOTE (upstream design): in CHECK mode :434 runs the JIT once for
        // the A/B and :512 runs it AGAIN afterwards — debug mode doubles the
        // MEG block; audio in CHECK mode is deliberately NOT the gated path.
        if self.check {
            self.check_ab(c.fnp, meg, swp);
        }

        // :512 c->fn(m_meg, this, m_reverb_ram.data()) — PHASE B2-3 landed:
        // transmute of the RWX buffer (jit.rs:380-384 pattern; ARG laundering
        // §7-B — ARG1 becomes a MegSwpDev::from_swp window built from this
        // very seam, ARG2 the live reverb RAM, never baked :611-612/:831).
        unsafe { call_meg_fn(c.fnp, meg, swp) };

        // :513-514 — the caller-side half of run_program: pc wrap + icount
        // tail (mirrors meg.rs:1000-1002; run_program subtracts the 0x180
        // ITSELF on every path, CHECK comment :451-453).
        meg.pc = 0;
        meg.icount = meg.icount.wrapping_sub(0x180);
        true // :515
    }
}

impl MegJit {
    /// origin: swp30_jit.cpp:426-511 — the CHECK A/B leg body. Runs `fp`
    /// (the compiled block, already truncated to `meg_jit_upto()` at compile
    /// time), restores the device + state, runs the interpreter for the SAME
    /// steps, and reports the first divergence (field byte, RAM diff count +
    /// first index, seed, flags — :461-465), the full field dumps on the
    /// first hit (:466-489) and every differing field after (:490-509).
    /// The report cap is `shown < 40` (:461); the counter is one process
    /// static shared by both devices, like the C++ file-scope `static int`.
    fn check_ab(&self, fp: usize, meg: &mut MegState, swp: &mut MegSwp) {
        let mut sc = self.scratch.borrow_mut();
        let rlen = swp.reverb_ram.len();
        if sc.ram0.len() != rlen || sc.ramj.len() != rlen {
            // size only changes on a state reload (meg.rs reverb_ram is
            // state-sized) — steady state NEVER allocates here
            sc.ram0.resize(rlen, 0);
            sc.ramj.resize(rlen, 0);
        }
        // :430-433 — before snapshot (meg_state copy + RAM + seed + flags).
        // `before`/`jit` are stack structs exactly like the C++ (:430/:435);
        // the RAM copies ride the persistent scratch above.
        let before: MegState = meg.clone(); // :430
        sc.ram0.copy_from_slice(&swp.reverb_ram); // :431
        let seed0 = *swp.seed; // :432
        let (fn0, fz0) = (*swp.flag_n, *swp.flag_z); // :433

        // :434 — JIT the LIVE state
        unsafe { call_meg_fn(fp, meg, swp) };

        // :435-438 — JIT-result snapshots. `jit` carries the typed values the
        // report needs; the byte scan images the LIVE struct at both sample
        // points so padding is the same bytes on both sides (C++ compares a
        // raw struct copy — :449-450 — and reads its padding too).
        let jit: MegState = meg.clone(); // :435
        sc.ramj.copy_from_slice(&swp.reverb_ram); // :436
        let (seedj, fnj, fzj) = (*swp.seed, *swp.flag_n, *swp.flag_z);
        let sz = std::mem::size_of::<MegState>();
        sc.jimg.resize(sz, 0);
        // SAFETY: jimg is exactly sz bytes; the struct is POD (repr(C),
        // no pointers — the same imaging `state_pod` relies on, minus the
        // per-call Vec).
        unsafe {
            std::ptr::copy_nonoverlapping(meg as *const MegState as *const u8, sc.jimg.as_mut_ptr(), sz);
        }

        // :439-443 — restore
        *meg = before.clone();
        swp.reverb_ram.copy_from_slice(&sc.ram0);
        *swp.seed = seed0;
        *swp.flag_n = fn0;
        *swp.flag_z = fz0;

        // :444-448 — the interpreter from the same start, SAME step count as
        // the emitted program (the UPTO bisect: step() per op when set, else
        // the full block through run_program).
        let upto = meg_jit_upto(); // :444
        if upto != MEG_OPS {
            for _ in 0..upto {
                meg_step(meg, swp, &mut None, 0, 0, 0, 0); // :446 m_meg->step()
            }
        } else {
            run_program(meg, swp, &self.ops); // :448 run_program(m_meg_ops)
        }

        // :449-457 — byte compare through the live struct image; the icount
        // (and the retval next to it) are the caller-side halves and are
        // EXPECTED to differ — stop short of them (:451-454).
        let end = offset_of!(MegState, icount); // :454
        sc.iimg.resize(sz, 0);
        // SAFETY: as jimg above; same struct address, so the padding bytes
        // in the two images are byte-identical by construction (:449 note).
        unsafe {
            std::ptr::copy_nonoverlapping(meg as *const MegState as *const u8, sc.iimg.as_mut_ptr(), sz);
        }
        let mut first = end; // :455
        for i in 0..end {
            if sc.jimg[i] != sc.iimg[i] {
                first = i;
                break;
            }
        }
        // :458-460 — RAM diff count + first index (zip keeps the exact same
        // scan; equal lengths by the resize above)
        let mut rambad = 0usize;
        let mut ramfirst = 0usize;
        for (i, (a, b)) in sc.ramj.iter().zip(swp.reverb_ram.iter()).enumerate() {
            if a != b {
                if rambad == 0 {
                    ramfirst = i;
                }
                rambad += 1;
            }
        }
        // :461 — the report gate (40 cap)
        if first != end
            || rambad != 0
            || seedj != *swp.seed
            || fnj != *swp.flag_n
            || fzj != *swp.flag_z
        {
            let shown = MEGCHECK_SHOWN.load(std::sync::atomic::Ordering::Relaxed);
            if shown < 40 {
                MEGCHECK_SHOWN.store(shown + 1, std::sync::atomic::Ordering::Relaxed);
                // :462-465 — shown is printed POST-increment (shown++ first)
                let seedok = if seedj == *swp.seed { "ok" } else { "BAD" };
                let fnok = if fnj == *swp.flag_n { "ok" } else { "BAD" };
                let fzok = if fzj == *swp.flag_z { "ok" } else { "BAD" };
                eprintln!(
                    "MEGCHECK sample {} state@{first}/{end} ram {rambad} (first {ramfirst}) seed {seedok} flags {fnok}{fzok}",
                    shown + 1
                );
                if shown == 0 {
                    // :466-473 — m_m windows
                    for b in (0..0x40usize).step_by(8) {
                        let mut lj = String::new();
                        let mut li = String::new();
                        for i in 0..8usize {
                            lj.push_str(&format!(" {:8}", jit.m[b + i]));
                            li.push_str(&format!(" {:8}", meg.m[b + i]));
                        }
                        eprintln!("  m[{b:02x}..] jit{lj}");
                        eprintln!("         interp{li}");
                    }
                    // :474-478 — the m32/m33/m48/m49 + mw summary pair
                    eprintln!(
                        "  jit    m32 {} m33 {} m48 {} m49 {} mwv {},{},{} mwr {},{},{}",
                        jit.m[32], jit.m[33], jit.m[48], jit.m[49],
                        jit.mw_value[0], jit.mw_value[1], jit.mw_value[2],
                        jit.mw_reg[0], jit.mw_reg[1], jit.mw_reg[2]
                    );
                    eprintln!(
                        "  interp m32 {} m33 {} m48 {} m49 {} mwv {},{},{} mwr {},{},{}",
                        meg.m[32], meg.m[33], meg.m[48], meg.m[49],
                        meg.mw_value[0], meg.mw_value[1], meg.mw_value[2],
                        meg.mw_reg[0], meg.mw_reg[1], meg.mw_reg[2]
                    );
                    // :480-485 — the delayed-write ops that could produce it
                    for k in 0..MEG_OPS {
                        let op = &self.ops[k as usize];
                        if op.dm == 32 || op.dm == 33 || op.dm == 48 || op.dm == 49 {
                            eprintln!(
                                "  ops[{k}] dm={} dm_src={} mmode={} asel={} rop={} shift={} clamp={} sm={} sr={} alu={}",
                                op.dm, op.dm_src, op.mmode, op.asel, op.rop,
                                op.shift, op.clamp, op.sm, op.sr, op.alu
                            );
                        }
                    }
                    // :486-488 — the before state
                    eprintln!(
                        "  before m32 {} m33 {} m48 {} m49 {} p {} mwv {},{},{} mwr {},{},{} d3 {} d2 {}",
                        before.m[32], before.m[33], before.m[48], before.m[49], before.p,
                        before.mw_value[0], before.mw_value[1], before.mw_value[2],
                        before.mw_reg[0], before.mw_reg[1], before.mw_reg[2],
                        before.delay_3, before.delay_2
                    );
                }
                // :490-509 — every differing field, interpreter side-by-side
                for i in 0..0x40usize {
                    if jit.m[i] != meg.m[i] {
                        eprintln!("  m[{i}] jit {} interp {}", jit.m[i], meg.m[i]);
                    }
                }
                for i in 0..0x80usize {
                    if jit.r[i] != meg.r[i] {
                        eprintln!("  r[{i}] jit {} interp {}", jit.r[i], meg.r[i]);
                    }
                }
                for i in 0..8usize {
                    if jit.t[i] != meg.t[i] {
                        eprintln!("  t[{i}] jit {} interp {}", jit.t[i], meg.t[i]);
                    }
                }
                if jit.p != meg.p {
                    eprintln!("  p jit {} interp {}", jit.p, meg.p);
                }
                if jit.ram_index != meg.ram_index {
                    eprintln!("  ix jit {} interp {}", jit.ram_index, meg.ram_index);
                }
                if jit.ram_read != meg.ram_read {
                    eprintln!("  rr jit {} interp {}", jit.ram_read, meg.ram_read);
                }
                if jit.ram_write != meg.ram_write {
                    eprintln!("  rw jit {} interp {}", jit.ram_write, meg.ram_write);
                }
                if jit.delay_3 != meg.delay_3 || jit.delay_2 != meg.delay_2 {
                    eprintln!(
                        "  d3/d2 jit {},{} interp {},{}",
                        jit.delay_3, jit.delay_2, meg.delay_3, meg.delay_2
                    );
                }
                for i in 0..3usize {
                    if jit.mw_value[i] != meg.mw_value[i] || jit.mw_reg[i] != meg.mw_reg[i] {
                        eprintln!(
                            "  mw[{i}] jit {}/{} interp {}/{}",
                            jit.mw_value[i], jit.mw_reg[i], meg.mw_value[i], meg.mw_reg[i]
                        );
                    }
                    if jit.rw_value[i] != meg.rw_value[i] || jit.rw_reg[i] != meg.rw_reg[i] {
                        eprintln!(
                            "  rw[{i}] jit {}/{} interp {}/{}",
                            jit.rw_value[i], jit.rw_reg[i], meg.rw_value[i], meg.rw_reg[i]
                        );
                    }
                    if jit.index_value[i] != meg.index_value[i]
                        || jit.index_active[i] != meg.index_active[i]
                    {
                        eprintln!(
                            "  ixv[{i}] jit {}/{} interp {}/{}",
                            jit.index_value[i], jit.index_active[i] as u8,
                            meg.index_value[i], meg.index_active[i] as u8
                        );
                    }
                    if jit.memw_value[i] != meg.memw_value[i]
                        || jit.memw_active[i] != meg.memw_active[i]
                    {
                        eprintln!(
                            "  memw[{i}] jit {}/{} interp {}/{}",
                            jit.memw_value[i], jit.memw_active[i] as u8,
                            meg.memw_value[i], meg.memw_active[i] as u8
                        );
                    }
                    if jit.memr_value[i] != meg.memr_value[i]
                        || jit.memr_active[i] != meg.memr_active[i]
                    {
                        eprintln!(
                            "  memr[{i}] jit {}/{} interp {}/{}",
                            jit.memr_value[i], jit.memr_active[i] as u8,
                            meg.memr_value[i], meg.memr_active[i] as u8
                        );
                    }
                }
                for i in 0..2usize {
                    if jit.t_value[i] != meg.t_value[i] {
                        eprintln!("  tv[{i}] jit {} interp {}", jit.t_value[i], meg.t_value[i]);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// B2b-3c — the live :512 native call + CHECK support (:426-511).
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:429 `static int shown` — ONE file-scope counter
/// shared by both devices, capped at 40 reports (:461). Atomic for
/// form (master+slave run on one audio thread — C++'s static has exactly
/// the same benign-race shape in the threaded build it shipped with).
static MEGCHECK_SHOWN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// SMU_HUNT probe counter (temporary, B2b-3c dense hang hunt) — removed once
/// the divergence is fixed.
static HUNT_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// origin: swp30_jit.cpp:512 `c->fn(m_meg, this, m_reverb_ram.data())` —
/// the native entry call. §7-B ARG laundering lives here, at the call
/// boundary: ARG0 = MegState*, ARG1 = a stack `MegSwpDev` window built
/// from this very seam (jit.rs:70-73 precedent — valid for the duration
/// of the call only), ARG2 = the live reverb RAM base (per-call, never
/// baked — :611-612 RAM guard + :831 load `mov r15,rdx`). Win64
/// `extern "system"` == the Microsoft x64 C ABI the emitted prologue
/// (:826-837) speaks; the B2b-3b parity rigs exec through the SAME
/// transmute shape (tests/meg_jit.rs `run_parity`).
unsafe fn call_meg_fn(fp: usize, meg: &mut MegState, swp: &mut MegSwp) {
    let mut dev = MegSwpDev::from_swp(swp);
    let f: unsafe extern "system" fn(*mut MegState, *mut MegSwpDev, *mut u16) =
        std::mem::transmute(fp);
    f(meg as *mut MegState, &mut dev, swp.reverb_ram.as_mut_ptr());
}

// ---------------------------------------------------------------------------
// selftest transliterations — origin: swp30_jit.cpp:520-585
// `meg_jit_selftest`. The C++ assembles the three emit_* helpers (:75/:115/
// :143) into stubs and sweeps them against meg_state::revram_encode /
// revram_decode / m1_expand. Phase B1 has no assembler target yet, so the
// x64 instruction sequences are transliterated as scalar Rust (wrapping_*/
// 32-bit cl semantics) and swept against the PAIRED meg.rs ground truth in
// tests/meg_jit.rs — the same vectors the C++ selftest checks (:555-576).
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:75-112 `emit_revram_encode` (x64 mode, RENC=R11
/// :69; the 32-mode arithmetic leg :90-98 is DEAD). Input eax (u32), out
/// eax (u16); branch-free (comment :74: sign is data-dependent, a branch
/// would mispredict). Clashes with the meg.rs ground truth sweep
/// `revram_encode` (meg.rs:454, PAIRED).
pub fn selftest_revram_encode(v: u32) -> u32 {
    // :77 and eax, 0x7ffffff — only the low 27 bits matter (:555 comment)
    let mut ax = v & 0x7ff_ffff;
    // :78-80 mov edx,eax / shl edx,5 / sar edx,31 — bit 26 sign-spread
    let dx0 = ((ax << 5) as i32) >> 31; // :80 "bit 26 が立っていれば -1"
    // :81-82 mov ecx,edx / and ecx,0x7ffffff — the all-ones complement mask
    let cx = (dx0 as u32) & 0x7ff_ffff;
    // :83 xor eax,ecx — two's-complement the 27-bit value when negative
    ax ^= cx;
    // :84 and edx,1 — s
    let s = (dx0 as u32) & 1;
    // :86-89 mov ecx,eax / or ecx,0x400 / bsr ecx,ecx / sub ecx,10
    let orv = ax | 0x400; // never zero ⇒ bsr defined (0x400 folded in)
    let cx = (orv.ilog2() as u32).wrapping_sub(10); // :88-89 — e' (top bit - 10)
    // :100-104 x64 arm: xor r11 / test / setne — r11 = (e' != 0)
    let r11: u32 = if cx != 0 { 1 } else { 0 }; // :101-102 setcc 0x95 (jne)
    // :103-104 sub ecx,r11 / add r11,ecx — after this ecx=cl is the SHIFT
    // (e'-(e'!=0): e'-1 when e'>0, else 0) and r11 is the EXPONENT field e'
    // (r11+ecx = (e'!=0)+(e'-(e'!=0)) = e'). The two are DIFFERENT: the
    // mantissa shifts by ecx (cl), the exponent field uses r11 (==e'). (A
    // draft that shifted by the exponent dropped a mantissa bit at v=0x800 —
    // meg.rs revram_encode is the paired ground truth, meg.rs:454.)
    let cl = cx.wrapping_sub(r11); // :103 — shift count
    let e = r11.wrapping_add(cl); // :104 — exponent field (== e')
    // :106-107 shr eax,cl — shift count is cl & 31 (x64 cl form; cl ≤ 14)
    ax = ax >> (cl & 31);
    ax &= 0x7ff; // :107 — m
    // :108-109 shl r11,12 / or eax,r11 — exponent field
    ax |= e << 12;
    // :110-111 shl edx,11 / or eax,edx — sign bit
    ax |= s << 11;
    ax
}

/// origin: swp30_jit.cpp:115-140 `emit_m1_expand` (input = the 64-bit
/// SIGN-EXTENDED s16 the call sites pass via loads16, comment :572; output
/// used as 64-bit). Every op in the body is 32-bit, so the RAX high half is
/// the zero-extension of any written result — and on the negative path the
/// :135-136 jump target ZEROES eax (`xor eax,eax`, :136 — not a
/// passthrough). Sweep vs meg.rs m1_expand (meg.rs:421, PAIRED).
pub fn selftest_m1_expand(v: i64) -> i64 {
    // :117-118 test eax,0x8000 / jne neg — the 16-bit sign lives in the low
    // half regardless of the sign-extension above it
    if (v as u32) & 0x8000 != 0 {
        // :135 patch(neg) → :136 xor eax,eax → done1/2/3 → ret
        return 0;
    }
    // :119-121 mov ecx,eax / shr ecx,12 / and ecx,7 — s (v ≥ 0 ⇒ no sign)
    let s = ((v as u32) >> 12) & 7;
    // :122-123 and eax,0xfff / or eax,0x1000 — x = 0x1000 | (v & 0xfff)
    let x = ((v as u32) & 0xfff) | 0x1000;
    // :124-133 cmp 5 / je done1 / jb less / shl cl / neg+sub+shr cl
    let r = if s == 5 {
        x // :125 done1
    } else if s > 5 {
        x.wrapping_shl(s - 5) // :127-128 — max 0x1fff<<2 = 0x7ffc (meg.rs:432)
    } else {
        x >> (5 - s) // :130-133 shr eax,cl (x < 0x2000 ⇒ no high bits)
    };
    // every result was written by a 32-bit op ⇒ zero-extended to rax
    r as i64
}

/// origin: swp30_jit.cpp:143-163 `emit_revram_decode` (RDEC=R8, :70;
/// branch-free, :142 comment). Input eax (u16), out eax. Sweep vs
/// meg.rs revram_decode (meg.rs:473, PAIRED — incl. the S-MU2000
/// e==0 full-invert fix :2449-2453).
pub fn selftest_revram_decode(v: u32) -> u32 {
    // :145 mov r8d,eax — keep the raw input (used at :158)
    let rdec = v;
    // :146-147 mov ecx,eax / shr ecx,12 — e (callers pass u16: e ≤ 15)
    let mut e = v >> 12;
    // :148 and eax,0x7ff — m
    let mut ax = v & 0x7ff;
    // :149-151 xor edx,edx / test ecx,ecx / setne dl
    let d: u32 = if e != 0 { 1 } else { 0 }; // :151 setcc 0x95
    // :152 sub ecx,edx — e != 0 ⇒ e-1, e == 0 ⇒ 0 (shift count)
    e -= d;
    // :153-154 shl edx,11 / or eax,edx — e ? m|0x800 : m
    ax |= d << 11;
    // :155 shl eax,cl — (e_orig - 1) ≤ 14, no 32-bit overflow
    ax = ax << e;
    // :156-157 imm32 edx,-1 / shl edx,cl — the inversion mask range
    let mut dx: u32 = 0xffff_ffffu32 << e;
    // :158-160 mov ecx,r8d / shl ecx,20 / sar ecx,31 — s ? -1 : 0
    let cx = ((rdec << 20) as i32) >> 31;
    // :161-162 and edx,ecx / xor eax,edx — e==0 negatives invert ALL 32
    // bits (the S-MU2000 fix — meg.rs:470-471 note; mask = 0xffffffff<<0)
    dx &= cx as u32;
    ax ^= dx;
    ax
}

/// Compile-time layout asserts for the offsets the offset table bakes
/// (handoff §5.3-3/§7-B; the jit.rs:255-275 `offs` precedent). These are
/// the widths swp30_jit.cpp static-asserts at :654-657; the C++ refuses
/// to build (returns false) if any mismatches — here mismatch = compile
/// error. The exact struct offsets are pinned test-side (tests/meg_jit.rs
/// `b2b1_offs_table_pins`).
const _: () = {
    // :654 mw_reg[0] / index_active[0] / memw_active[0] == 1 byte
    assert!(std::mem::size_of::<[u8; 3]>() == 3);
    // Rust `bool` stands in for the C++ u8 flags (index_active/memw_/
    // memr_active :632/:636/:637, meg_flag_n/z :647-648) — the loadu8/
    // store8i/setcc legs REQUIRE 1 byte (setcc writes a legal 0/1 bool).
    assert!(std::mem::size_of::<bool>() == 1);
    // :655 t_value[0] == 2 (i16) && m_const[0] == 2 (i16) &&
    //      m_offset[0] == 2 (u16)
    assert!(std::mem::size_of::<i16>() == 2 && std::mem::size_of::<u16>() == 2);
    // :656 m_m[0] == 4 (i32) && m_r[0] == 4 (i32)
    assert!(std::mem::size_of::<i32>() == 4);
    // m_p is s64 (:832 load64 / :1689 store64); x86-64 pointers 8 B
    assert!(std::mem::size_of::<i64>() == 8 && std::mem::size_of::<usize>() == 8);
};

/// offset_of! pin used by tests/meg_jit.rs (§7-D): the Code snapshot layout
/// the run guard :423-424 reads. offset_of! requires a pinned
/// representation only for structures the EMITTER bakes — Code is host-only
/// (Rust reads it), so this asserts field presence/order for the test pin
/// rather than a C++-byte match.
pub fn code_field_offsets() -> (usize, usize, usize) {
    (
        offset_of!(Code, d3),
        offset_of!(Code, d2),
        std::mem::size_of::<Code>(),
    )
}

// ---------------------------------------------------------------------------
// B2a — machine-code emitter helpers (origin: swp30_jit.cpp:75-163, x64
// mode; the `#if SMU_X64ASM_MODE == 32` arms at :90-98 are DEAD). These
// drop into any Assembler buffer exactly like the C++ free functions
// (meg_asm = assembler :62). Semantic ground truth: the scalar
// transliterations above (pinned vs the PAIRED meg.rs in tests/meg_jit.rs,
// C++ meg_jit_selftest :520-585 does the identical sweep on the EMITTED
// bytes — emitted here, executed there).
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:75-112 `emit_revram_encode`. Input eax (u32),
/// output eax (u16); clobbers rcx rdx r11 (RENC :69). Branch-free (:74 —
/// the sign is sound-dependent, a branch would mispredict).
pub fn emit_revram_encode(a: &mut Assembler) {
    a.and32i(RAX, 0x7ff_ffff); // :77 — only the low 27 bits matter (:555)
    a.mov32(RDX, RAX);         // :78
    a.shl32(RDX, 5);           // :79
    a.sar32(RDX, 31);          // :80 — bit 26 が立っていれば -1
    a.mov32(RCX, RDX);         // :81
    a.and32i(RCX, 0x7ff_ffff); // :82 — the all-ones complement mask
    a.xor32(RAX, RCX);         // :83 — two's-complement when negative
    a.and32i(RDX, 1);          // :84 — s
    // :85 e = top set bit of bits 11..25 minus 10; none ⇒ e = 0, m = v
    a.mov32(RCX, RAX);         // :86
    a.or32ri(RCX, 0x400);      // :87 — never zero ⇒ bsr defined
    a.bsr32(RCX, RCX);         // :88
    a.sub32ri(RCX, 10);        // :89 — e'
                               // x64 arm :100-104 (32-mode arithmetic leg :90-98 DEAD):
    a.xor32(RENC, RENC);       // :100
    a.test32(RCX, RCX);        // :101
    a.setcc(0x95, RENC);       // :102 — r11 = (e' != 0) (jne)
    a.sub32(RCX, RENC);        // :103 — ecx=cl becomes the SHIFT (e'-(e'!=0))
    a.add32(RENC, RCX);        // :104 — r11 becomes the EXPONENT FIELD e'
                               // (mantissa shifts by cl, exponent uses r11 — the two
                               // differ; B1 scalar note above, meg.rs:454 ground truth)
    a.shr32cl(RAX);            // :106 — cl ≤ 14
    a.and32i(RAX, 0x7ff);      // :107 — m
    a.shl32(RENC, 12);         // :108
    a.or32(RAX, RENC);         // :109 — exponent field
    a.shl32(RDX, 11);          // :110
    a.or32(RAX, RDX);          // :111 — sign bit
}

/// origin: swp30_jit.cpp:115-140 `emit_m1_expand`. Input eax (low 16 = s16;
/// callers pass the loads16 sign-extended 64-bit value, :572), output rax
/// 0..0x7ffc; clobbers rcx. Every body op is 32-bit, so each written
/// result zero-extends into rax — and the negative path ZEROES eax
/// (`xor eax,eax` :136, not a passthrough).
pub fn emit_m1_expand(a: &mut Assembler) {
    a.test32ri(RAX, 0x8000); // :117 — the 16-bit sign lives in the low half
    let neg = a.jcc_fwd(0x85); // :118 — jne neg
    a.mov32(RCX, RAX);       // :119
    a.shr32(RCX, 12);        // :120
    a.and32i(RCX, 7);        // :121 — s (v >= 0 ⇒ no sign)
    a.and32i(RAX, 0xfff);    // :122
    a.or32ri(RAX, 0x1000);   // :123 — x = 0x1000 | (v & 0xfff)
    a.cmp32ri(RCX, 5);       // :124
    let done1 = a.jcc_fwd(0x84); // :125 — je done1 (s == 5: rax = x)
    let less = a.jcc_fwd(0x82);  // :126 — jb less  (s < 5)
    a.sub32ri(RCX, 5);       // :127
    a.shl32cl(RAX);          // :128 — max 0x1fff<<2 = 0x7ffc (meg.rs:432)
    let done2 = a.jmp_fwd(); // :129
    a.patch(less);           // :130
    a.neg32(RCX);            // :131
    a.add32ri(RCX, 5);       // :132
    a.shr32cl(RAX);          // :133 — x < 0x2000 ⇒ no high bits
    let done3 = a.jmp_fwd(); // :134
    a.patch(neg);            // :135
    a.xor32(RAX, RAX);       // :136 — negative ⇒ 0 (zero-extends to rax)
    a.patch(done1);          // :137
    a.patch(done2);          // :138
    a.patch(done3);          // :139
}

/// origin: swp30_jit.cpp:143-163 `emit_revram_decode`. Input eax (u16),
/// output eax; clobbers rcx rdx r8 (RDEC :70). Branch-free (:142) — incl.
/// the S-MU2000 e==0 full-invert fix (:2449-2453, meg.rs:470-471 note:
/// the inversion mask `0xffffffff << 0` covers ALL 32 bits when e==0).
pub fn emit_revram_decode(a: &mut Assembler) {
    a.mov32(RDEC, RAX);       // :145 — keep the raw input (used at :158)
    a.mov32(RCX, RAX);        // :146
    a.shr32(RCX, 12);         // :147 — e (callers pass u16: e <= 15)
    a.and32i(RAX, 0x7ff);     // :148 — m
    a.xor32(RDX, RDX);        // :149
    a.test32(RCX, RCX);       // :150
    a.setcc(0x95, RDX);       // :151 — dl = (e != 0) (jne)
    a.sub32(RCX, RDX);        // :152 — shift count e ? e-1 : 0
    a.shl32(RDX, 11);         // :153
    a.or32(RAX, RDX);         // :154 — e ? m|0x800 : m
    a.shl32cl(RAX);           // :155 — (e_orig - 1) <= 14, no 32-bit overflow
    a.imm32(RDX, 0xffff_ffff); // :156
    a.shl32cl(RDX);           // :157 — the inversion mask range
    a.mov32(RCX, RDEC);       // :158
    a.shl32(RCX, 20);         // :159
    a.sar32(RCX, 31);         // :160 — s ? -1 : 0
    a.and32(RDX, RCX);        // :161
    a.xor32(RAX, RDX);        // :162
}

// ---------------------------------------------------------------------------
// B2b-2a — skip reset + delay-ring legs (origin: swp30_jit.cpp:839-840,
// :1000-1048, :1050-1077). Additive scaffolding for the future op loop:
// build() still refuses at the PHASE_B2_EMIT_OK gate, so NONE of these bytes
// reaches an exec buffer or runs — same contract as the frame skeleton
// (tests/meg_jit.rs `b2b2a_build_inert` pins fnp==0; the exec rigs execute
// the helpers STANDALONE, never a half-emitted program).
// Width adaptation (brief §2): the Rust skip slot is `*mut u16`
// (`MegSwpDev.skip` ← `Swp30::meg_skip_to`, C++ u32 swp30.h:614), so the
// skip legs are 16-bit (`store16i`/`cmp16i_mem` added to jit_emit.rs in
// this slice). RDX is free scratch in every leg below: live cross-leg regs
// are RBX/R12/R13/R14/R15/RSI/RDI/RBP/R9/R10 (§3) and ring-apply contains
// no callout — RAX/RCX/RDX are the C++ legs' own scratch (:1003-1029).
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:839-840 `if (branchy) store32i mem{SWP, o_skip}, 0`
/// (prologue tail, :838). The C++ zeroes the device's u32 `m_meg_jit_skip`
/// once per sample so the per-op branch gate (:1082) starts each program
/// walk unskipped. Rust: through the §7-B window — load the `*mut u16` slot
/// into RAX, then `store16i [rax+0], 0` (width-adapted u16 `meg_skip_to`).
/// RAX is scratch at the prologue tail (live from :829-837 are the pinned
/// register-plan regs only); nothing downstream consumes it before the op loop.
pub fn emit_skip_reset(a: &mut Assembler, o: &Offs) {
    a.load64(RAX, Mem::b(SWP, o.skip)); // window slot -> live meg_skip_to
    a.store16i(Mem::b(RAX, 0), 0);      // :840 (u16 adaptation, brief §2)
}

/// origin: swp30_jit.cpp:1000-1029 — 3-ring apply HEAD (the dynamic leg the
/// op loop emits for `k < 3 || branchy`): test each ring byte at slot `s`,
/// apply the delayed write through the register index (scale-4 store :1007),
/// same for rw→r (:1010-1015), index→ram_index (:1017-1022), and index2→
/// ram_index2 (:1024-1029 — window legs: one extra deref load per field,
/// §7-B; the values land in the same live Swp30 bytes the interpreter uses).
pub fn emit_ring3_head(a: &mut Assembler, o: &Offs, s: usize) {
    let s = s as i32;
    // m (:1003-1008)
    a.loadu8(RAX, Mem::b(MS, o.mw_reg + s)); // :1003
    a.test32(RAX, RAX);                      // :1004
    let j1 = a.jz_fwd();                     // :1005
    a.load32(RCX, Mem::b(MS, o.mw_value + 4 * s)); // :1006
    a.store32(
        Mem { base: MS, index: RAX, scale: 4, disp: o.m }, // :1007 scale 4
        RCX,
    );
    a.patch(j1); // :1008
    // r (:1010-1015)
    a.loadu8(RAX, Mem::b(MS, o.rw_reg + s));
    a.test32(RAX, RAX);
    let j2 = a.jz_fwd();
    a.load32(RCX, Mem::b(MS, o.rw_value + 4 * s));
    a.store32(Mem { base: MS, index: RAX, scale: 4, disp: o.r }, RCX); // :1014
    a.patch(j2);
    // index (:1017-1022)
    a.loadu8(RAX, Mem::b(MS, o.ix_act + s));
    a.test32(RAX, RAX);
    let j3 = a.jz_fwd();
    a.load32(RCX, Mem::b(MS, o.ix_value + 4 * s));
    a.store32(Mem::b(MS, o.ram_index), RCX); // :1021
    a.patch(j3);
    // 2nd index (:1024-1029) — §7-B window legs: C++ addressed the device
    // inline; here each SWP-side field costs one deref load of its slot.
    a.load64(RDX, Mem::b(SWP, o.ix2_act)); // *mut [u8;3]
    a.loadu8(RAX, Mem::b(RDX, s));         // :1024
    a.test32(RAX, RAX);                    // :1025
    let j4 = a.jz_fwd();                   // :1026
    a.load64(RDX, Mem::b(SWP, o.ix2_value)); // *mut [i32;3]
    a.load32(RCX, Mem::b(RDX, 4 * s));     // :1027
    a.load64(RDX, Mem::b(SWP, o.ram_index2)); // *mut i32
    a.store32(Mem::b(RDX, 0), RCX);        // :1028
    a.patch(j4); // :1029
}

/// origin: swp30_jit.cpp:1030-1048 — 3-ring apply FOLDED (the `else` leg,
/// non-branchy ops with `k >= 3`): ops[k-3]'s (`w`) writes go straight from
/// the ring slot to their destination, register numbers compile-time
/// constants. Early-written regs (`early_m`/`early_r`, §5.3-6) were already
/// stored at the writing op and are skipped here (:1033/:1038); index legs
/// are always folded (:1041-1048).
pub fn emit_ring3_folded(a: &mut Assembler, o: &Offs, s: usize, w: &Op, an: &Analysis) {
    let s = s as i32;
    if w.dm != 0 && !an.early_m[w.dm as usize] {
        // :1033-1036
        a.load32(RCX, Mem::b(MS, o.mw_value + 4 * s));
        a.store32(Mem::b(MS, o.m + 4 * w.dm as i32), RCX);
    }
    if w.dr != 0 && !an.early_r[w.dr as usize] {
        // :1037-1040
        a.load32(RCX, Mem::b(MS, o.rw_value + 4 * s));
        a.store32(Mem::b(MS, o.r + 4 * w.dr as i32), RCX);
    }
    if w.index != 0 {
        // :1041-1044
        a.load32(RCX, Mem::b(MS, o.ix_value + 4 * s));
        a.store32(Mem::b(MS, o.ram_index), RCX);
    }
    if w.index2 != 0 {
        // :1045-1048 (window deref, §7-B)
        a.load64(RDX, Mem::b(SWP, o.ix2_value));
        a.load32(RCX, Mem::b(RDX, 4 * s));
        a.load64(RDX, Mem::b(SWP, o.ram_index2));
        a.store32(Mem::b(RDX, 0), RCX);
    }
}

/// origin: swp30_jit.cpp:1050-1065 — 2-ring (mem ports) HEAD for
/// `k < 2 || branchy`: apply ops' delayed memw/memr values AND clear the
/// act bytes (:1057/:1064 — the interpreter's one-shot clear; Rust bools are
/// the 1-byte flags pinned by the const asserts above, so `store8i 0` is a
/// legal `false`). All six legs are MS-side — no window deref.
pub fn emit_ring2_head(a: &mut Assembler, o: &Offs, s: usize) {
    let s = s as i32;
    a.loadu8(RAX, Mem::b(MS, o.memw_act + s)); // :1052
    a.test32(RAX, RAX);                        // :1053
    let j1 = a.jz_fwd();                       // :1054
    a.load32(RCX, Mem::b(MS, o.memw_val + 4 * s)); // :1055
    a.store32(Mem::b(MS, o.ram_write), RCX);   // :1056
    a.store8i(Mem::b(MS, o.memw_act + s), 0);  // :1057
    a.patch(j1); // :1058
    a.loadu8(RAX, Mem::b(MS, o.memr_act + s)); // :1059
    a.test32(RAX, RAX);                        // :1060
    let j2 = a.jz_fwd();                       // :1061
    a.load32(RCX, Mem::b(MS, o.memr_val + 4 * s)); // :1062
    a.store32(Mem::b(MS, o.ram_read), RCX);    // :1063
    a.store8i(Mem::b(MS, o.memr_act + s), 0);  // :1064
    a.patch(j2); // :1065
}

/// origin: swp30_jit.cpp:1066-1077 — 2-ring FOLDED (`else`, non-branchy
/// `k >= 2`): ops[k-2]'s memw/memop apply from their slots; NO act clear
/// (nothing was queued dynamically — the fold is the decision). The memr
/// leg is gated by memop 2/3 (read ports, :1073).
pub fn emit_ring2_folded(a: &mut Assembler, o: &Offs, s: usize, w: &Op) {
    let s = s as i32;
    if w.memw != 0 {
        // :1069-1072
        a.load32(RCX, Mem::b(MS, o.memw_val + 4 * s));
        a.store32(Mem::b(MS, o.ram_write), RCX);
    }
    if w.memop == 2 || w.memop == 3 {
        // :1073-1076
        a.load32(RCX, Mem::b(MS, o.memr_val + 4 * s));
        a.store32(Mem::b(MS, o.ram_read), RCX);
    }
}

// ---------------------------------------------------------------------------
// B2b-2b — dm/dr op-loop legs + the value lambdas they consume (origin:
// swp30_jit.cpp:843-889 `pack24`/`rnd`/`rnd_skip`/`p_packed` lambdas,
// :988 hoist-store, :1414-1496 dm/dr blocks). Additive scaffolding exactly
// like B2b-2a: build() still refuses at the PHASE_B2_EMIT_OK gate, so NONE
// of these bytes reaches an exec buffer or runs — tests/meg_jit.rs executes
// the helpers STANDALONE (exec rigs), never a half-emitted program.
// Register contract (from the §3 plan): RAX is the value register every
// lambda returns (:842 comment), RCX is pack24's scratch (:845), RSI=SEED
// is advanced DELIBERATELY by rnd/rnd_skip (:862/:871 — the same stream
// position the interpreter walks, handoff risk 3), R13=P is read-only here
// (:874-882 p_packed), and the K_MAX/K_MIN registers (:835-836) must be
// pre-pinned by the caller — the prologue does it at :835-836, the exec
// rigs emulate that. P_MAX/P_MIN are NOT touched (no saturation clamp in
// these legs) and no callout exists here, so load_p_limits (:1448 twin)
// stays irrelevant within this slice.
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:843-857 `pack24` lambda — ground truth
/// `meg::meg_pack24` (meg.rs:562, PAIRED). Input rax (s64 p), output eax
/// (24-bit folded, zero-extended into rax by the 32-bit tail). Truncates
/// toward ZERO (:844 comment: negative inputs get +0x7fff before the
/// arithmetic shift), clamps the ONE-past-limit values exactly to the K
/// registers (:850 — anything further over still folds 24-bit), then the
/// shl32/sar32 fold. C++ passes `u32(s32(-0x800001))` to the sign-extended
/// cmp64ri (:853) — same call here, same sext.
pub fn emit_pack24(a: &mut Assembler) {
    a.mov64(RCX, RAX); // :845
    a.sar64(RCX, 63);  // :846 — -1 when negative
    a.and32i(RCX, 0x7fff); // :847 — round-toward-zero bias
    a.add64(RAX, RCX); // :848
    a.sar64(RAX, 15);  // :849 — p / 32768 truncated toward 0
    a.cmp64ri(RAX, 0x80_0000); // :851
    a.cmove64(RAX, K_MAX);     // :852
    a.cmp64ri(RAX, (-0x80_0001i32) as u32); // :853 u32(s32(-0x800001))
    a.cmove64(RAX, K_MIN);     // :854
    a.shl32(RAX, 8);           // :855 — 24-bit fold
    a.sar32(RAX, 8);           // :856
}

/// origin: swp30_jit.cpp:859-864 `rnd` lambda — one draw of
/// swp30_device::rand (paired `voice::swp_rand`, meg.rs:28/:445 seam).
/// Constants 1664525/1013904223 + rol16 must match swp_rand bit-for-bit
/// (handoff §H). Advances SEED (RSI); output eax.
pub fn emit_rnd(a: &mut Assembler) {
    a.imul32i(RAX, SEED, 1664525);    // :860
    a.add32i(RAX, 1013904223);        // :861
    a.mov32(SEED, RAX);               // :862 — the new seed lives in RSI
    a.rol32(RAX, 16);                 // :863 — the draw is the rotated form
}

/// origin: swp30_jit.cpp:866-872 `rnd_skip(n)` lambda — advance the seed
/// exactly n draws without using values (:868 `swp30_device::rand_jump` =
/// the PAIRED `meg::rand_jump` meg.rs:586). Feeds the dr leg's coalesced
/// skipped-region draws (:1481-1482 ← swp30.cpp:4282-4298 — the seed
/// advance stays at the same stream position, handoff risk 3).
pub fn emit_rnd_skip(a: &mut Assembler, n: u32) {
    let (mul, add) = rand_jump(n); // :868 (C++ out-params -> tuple, §dev)
    a.imul32i(RAX, SEED, mul);     // :869
    a.add32i(RAX, add);            // :870
    a.mov32(SEED, RAX);            // :871
}

/// origin: swp30_jit.cpp:874-882 `p_packed(noise)` lambda — p with optional
/// dither, packed (dm src 6 :1461, dr p-leg :1487). Noise is the small
/// POSITIVE draw `& 0x07e0` (:877, handoff risk 3 — the mask appears ONLY
/// on these legs). Input P (R13), output eax.
pub fn emit_p_packed(a: &mut Assembler, noise: bool) {
    if noise {
        emit_rnd(a);              // :876
        a.and32i(RAX, 0x07e0);    // :877 — 雑音（正の小さな値）
        a.add64(RAX, P);          // :878 — p をそのまま足す
    } else {
        a.mov64(RAX, P);          // :880
    }
    emit_pack24(a);               // :881
}

/// origin: swp30_jit.cpp:988 — the LFO hoist tail: park the emit_lfo result
/// (shl32 :968, eax) in the frame slot `LFO_SLOT_BASE + 4n` (:985). `emit_lfo`
/// (:897-969) landed alongside it in B2b-3a; the hoist SCAN (:971-991) that
/// decides which numbers to hoist and calls both halves lands with the
/// B2b-3b op-loop stitch. This is the store half of the slot the dm-src frame
/// load (:1419) reads.
pub fn emit_lfo_slot_store(a: &mut Assembler, slot: i32) {
    debug_assert!(slot >= LFO_SLOT_BASE && slot < LFO_SLOT_BASE + 0x18 * 4); // :985 range
    a.store32(fm(slot), RAX); // :988
}

// ---------------------------------------------------------------------------
// B2b-3a — LFO HOIST head + callout (origin: swp30_jit.cpp:897-969 emit_lfo,
// :971-991 hoist scan, :1414-1448 dm LFO arms + call_lfo callout). Additive
// scaffolding exactly like B2b-2a..2e: build() STILL refuses at the
// PHASE_B2_EMIT_OK gate (op loop, hoist SCAN and stitch arrive in B2b-3b), so
// NONE of these bytes reaches an exec buffer or runs — tests/meg_jit.rs runs
// them STANDALONE. This closes the LAST `unimplemented!` in the module: the
// two un-hoisted dm_src 0-3 arms now emit (in-place emit_lfo, or the callout).
// Register contract (§3, handoff §5.3-8): emit_lfo is loop-invariant and
// clobbers ONLY RAX (out), RCX (shift/table scratch), RDX (lfo word/wave
// select) and R8 (baked table/sintab base) — the C++ :896 note "rcx rdx r8 を
// 壊す". It does NOT touch SEED(RSI)/P(R13)/SC(R14)/RAM(R15)/K_MAX(RDI)/
// K_MIN(RBP)/P_MAX(R9)/P_MIN(R10) — so the rand stream stays aligned (handoff
// §H: no rand draw on the LFO path; a dither draw lives only on dm_src 5 /
// dr-p, already landed). The callout's only clobber is RAX (call_abs target)
// plus the Win64-volatile set inside the trampoline; the 8 callee-saves the op
// loop keeps live survive, and load_p_limits (:1448 twin) reloads the
// caller-saved P_MAX/P_MIN after the call.
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:897-969 `emit_lfo` — the hoist head, a transliteration
/// of `meg_state::get_lfo` (the PAIRED `meg::get_lfo`, meg.rs:379). `idx` is
/// the LFO number (`o.lfo`, < 0x18 — the dm/hoist gate :978/:1418/:1420).
/// `sintab` is the compile-time-captured table base (baked at :937, §7-D pin):
/// the resident table is ≥ 0x8000 so every sine index (< 0x8000) is in range,
/// exactly the meg.rs:379 precondition. Result in RAX (the caller stores it
/// with `emit_lfo_slot_store` :988, or reads it straight in dm_src 0-3 :1421).
///
/// Legs, in C++ order: counter>>5 (:902-903 == meg.rs:392 phase); pitch shift
/// bits 8-9 via `shl32cl` (:904-908 == meg.rs:393 depth); the offsets table
/// (:915-916, the LFO_OFFSETS address + `lfo>>12` *4 == meg.rs:394); `& 0x1ffff`
/// (:919); wave select bits 10-11 (:920-921). sine (0): `& 0x7fff` folded by
/// bit15 (:926-931 == meg.rs:400-403 arm 1/2), sintab load (:939), negate on
/// bit16 via `xor 0xffff` (:941-944 == arm 3/4). tri (1): `+0x8000`, `& 0x1ffff`,
/// fold `xor 0x1ffff` on bit16 (:951-956 == meg.rs:412-415). saw up (2): `>>1`
/// (:962 == meg.rs:418). saw down (3): `xor 0x1ffff` then `>>1` (:965-966 ==
/// meg.rs:419). `<<7` final (:968 == meg.rs:421). The three forward-leg jumps
/// (not_sine/not_tri/not_up) and the `done` patch list mirror the C++ exactly.
pub fn emit_lfo(a: &mut Assembler, o: &Offs, idx: u32, sintab: *const u16) {
    // :902-903 — phase = counter[idx] >> 5 (meg.rs:392)
    a.load32(RAX, Mem::b(MS, o.lfo_counter + 4 * idx as i32)); // :902
    a.shr32(RAX, 5); // :903
    // :904-908 — base <<= (lfo[idx] >> 8) & 3  (meg.rs:393, depth)
    a.loadu16(RDX, Mem::b(MS, o.lfo + 2 * idx as i32)); // :904
    a.mov32(RCX, RDX); // :905
    a.shr32(RCX, 8); // :906
    a.and32i(RCX, 3); // :907
    a.shl32cl(RAX); // :908
    // :909-916 — base += offsets[lfo[idx] >> 12]  (meg.rs:394)
    a.mov32(RCX, RDX); // :909
    a.shr32(RCX, 12); // :910
    a.imm64(R8, addr_of!(LFO_OFFSETS) as *const u8 as u64); // :915 (baked addr)
    a.load32(RCX, Mem { base: R8, index: RCX, scale: 4, disp: 0 }); // :916
    a.add32(RAX, RCX); // :918
    a.and32i(RAX, 0x1ffff); // :919
    // :920-924 — wave select = (lfo[idx] >> 10) & 3 into RDX; dispatch
    a.shr32(RDX, 10); // :920
    a.and32i(RDX, 3); // :921
    let mut done: Vec<usize> = Vec::new(); // :922
    a.test32(RDX, RDX); // :923
    let not_sine = a.jcc_fwd(0x85); // :924 jne
    {
        // sine (:926-945)
        a.mov32(RCX, RAX); // :926 — rcx = base & 0x7fff, flipped on bit15
        a.and32i(RCX, 0x7fff); // :927
        a.test32ri(RAX, 0x8000); // :928
        let no_rev = a.jcc_fwd(0x84); // :929 je (bit15 clear ⇒ keep rcx)
        a.xor32ri(RCX, 0x7fff); // :930
        a.patch(no_rev); // :931
        a.imm64(R8, sintab as u64); // :937 — baked sintab base (§7-D)
        a.mov32(RDX, RAX); // :938 — keep base for the bit16 sign test
        a.loadu16(RAX, Mem { base: R8, index: RCX, scale: 2, disp: 0 }); // :939
        a.test32ri(RDX, 0x10000); // :941
        let no_neg = a.jcc_fwd(0x84); // :942 je
        a.xor32ri(RAX, 0xffff); // :943 — upper half is the inverted quarter
        a.patch(no_neg); // :944
        done.push(a.jmp_fwd()); // :945
    }
    a.patch(not_sine); // :947
    a.cmp32ri(RDX, 1); // :948
    let not_tri = a.jcc_fwd(0x85); // :949 jne
    {
        // tri (:951-957)
        a.add32ri(RAX, 0x8000); // :951
        a.and32i(RAX, 0x1ffff); // :952
        a.test32ri(RAX, 0x10000); // :953
        let no_fold = a.jcc_fwd(0x84); // :954 je
        a.xor32ri(RAX, 0x1ffff); // :955 — fold down across the mid line
        a.patch(no_fold); // :956
        done.push(a.jmp_fwd()); // :957
    }
    a.patch(not_tri); // :959
    a.cmp32ri(RDX, 2); // :960
    let not_up = a.jcc_fwd(0x85); // :961 jne
    a.shr32(RAX, 1); // :962 — saw up
    done.push(a.jmp_fwd()); // :963
    a.patch(not_up); // :964
    a.xor32ri(RAX, 0x1ffff); // :965 — saw down
    a.shr32(RAX, 1); // :966
    for d in done {
        a.patch(d); // :967
    }
    a.shl32(RAX, 7); // :968 — scale to the sample range (meg.rs:421)
}

/// origin: swp30_jit.cpp:1431-1448 (`#else` / Windows-x64 arm) — the dm_src
/// 0-3 CALLOUT: `sintab` is not resident (or `lfo >= 0x18`), so the value is
/// produced by the `meg_jit_call_lfo` trampoline instead of machine code.
/// Faithful to the C++ caller: `mov64 ARG0, MS` (:1441), `imm32 ARG1, o.lfo`
/// (:1442), `call_abs` (:1443), then the MANDATORY `load_p_limits` (:1448 —
/// P_MAX/P_MIN are Win64-volatile). The SysV `push SEED/K_MAX` legs
/// (:1437-1440/:1444-1447) are DEAD here (§7-G — SEED/K_MAX are callee-saved on
/// Win64, so the trampoline preserves them and nothing is pushed; the C++
/// comment :1434-1436 says exactly this).
///
/// Rust deviation (§3 note): the C++ `call_lfo` reaches the table through
/// `ms->m_sintab` (device field, swp30.h:468 — swp30_jit.cpp:645). Rust's
/// `MegState` has NO sintab field; the paired seam is
/// `meg::get_lfo(lfo, sintab: &[u16])` (meg.rs:379). So the compile-time
/// sintab base and length are baked into ARG2(R8)/ARG3(R9) — both volatile
/// (R8 scratch, R9 = P_MAX reloaded right after by load_p_limits), and it is
/// the SAME base `emit_lfo` bakes at :937, so the §7-D sintab-pin invariant
/// covers the callout path identically. rsp is 16-aligned at the `call`
/// (frame body, §3 risk 5) with the 32-byte shadow space at `[rsp..rsp+32)`
/// inside `FRAME` — below the LFO slots (:816), so the callee home writes
/// cannot touch them.
pub fn emit_call_lfo(a: &mut Assembler, lfo: u32, sintab: *const u16, sintab_len: usize) {
    a.mov64(ARG0, MS); // :1441 — ARG0 = meg_state*
    a.imm32(ARG1, lfo); // :1442 — ARG1 = LFO number
    a.imm64(ARG2, sintab as u64); // Rust dev: ARG2 = sintab base (§3)
    a.imm64(ARG3, sintab_len as u64); // Rust dev: ARG3 = sintab len (§3)
    // :1443 `&meg_jit::call_lfo` — the fn item coerces to a fn pointer
    // (jit.rs:134-145 fnptr precedent), then bakes its address.
    let tramp: unsafe extern "system" fn(*const MegState, u32, *const u16, usize) -> u32 =
        meg_jit_call_lfo;
    a.call_abs(tramp as *const () as u64); // :1443
    load_p_limits(a); // :1448
}

/// origin: swp30_jit.cpp:298 `static u32 call_lfo(meg_state *ms, u32 lfo) {
/// return ms->get_lfo(int(lfo)); }` — the runtime trampoline the callout
/// above targets. C++ reads `ms->m_sintab`; the Rust `get_lfo` seam takes the
/// table explicitly, so it arrives through ARG2/ARG3 (§3 deviation). Reaches
/// ONLY the PAIRED `meg::get_lfo` (meg.rs:379) so the callout and the
/// interpreter compute the identical word. Never executed while
/// PHASE_B2_EMIT_OK==false (the op loop that emits the callout is not stitched;
/// `fnp==0` ⇒ `run()` never enters compiled code); exercised ONLY by the
/// tests/meg_jit.rs exec rigs. All callee-saves are preserved by the
/// compiler-emitted `extern "system"` prologue (S11 lesson).
pub unsafe extern "system" fn meg_jit_call_lfo(
    ms: *const MegState,
    lfo: u32,
    sintab: *const u16,
    sintab_len: usize,
) -> u32 {
    // §3/:1443 — the table the compile baked; len is the resident count so the
    // index bound is identical to the interpreter's swp.sintab (meg.rs:1368).
    let tab = unsafe { core::slice::from_raw_parts(sintab, sintab_len) };
    unsafe { (*ms).get_lfo(lfo as usize, tab) }
}

/// origin: swp30_jit.cpp:1414-1465 — the dm VALUE SOURCE select (the switch
/// inside `if (o.dm)` :1415); RAX receives the value. `lfo_slot` mirrors the
/// C++ `lfo_slot[o.lfo]` (:973): nonzero ⇔ `sintab && lfo < 0x18 &&
/// lfo_hoist[lfo]` (:981-984), i.e. the head produced the value once through
/// `emit_lfo_slot_store` and :1419 just reads it. `sintab` is the same
/// compile-time capture (`MegJit::sintab`, null when the table is NOT resident,
/// C++ :645). The 0-3 arms now cover all three C++ sub-cases (no more
/// `unimplemented!`): hoisted → frame load (:1419); resident-but-unhoisted →
/// in-place `emit_lfo` (:1421); otherwise (no sintab, or `lfo >= 0x18`) → the
/// `emit_call_lfo` callout (:1431-1448). The hoist SCAN that fills lfo_slot
/// (:971-991) and the op-loop stitch arrive in B2b-3b — the gate keeps every
/// caller unreachable, so no half program is emitted either way.
pub fn emit_dm_src(
    a: &mut Assembler,
    o: &Offs,
    op: &Op,
    lfo_slot: i32,
    sintab: *const u16,
    sintab_len: usize,
) {
    match op.dm_src {
        0..=3 => {
            if lfo_slot != 0 {
                a.load32(RAX, fm(lfo_slot)); // :1419 頭で 1 回だけ作っておいた値
            } else if !sintab.is_null() && (op.lfo as usize) < 0x18 {
                emit_lfo(a, o, op.lfo as u32, sintab); // :1421 1 回しか使わない番号はその場で
            } else {
                emit_call_lfo(a, op.lfo as u32, sintab, sintab_len); // :1431-1448 sin 表無しは呼ぶ
            }
        }
        4 => a.load32(RAX, Mem::b(MS, o.ram_read)), // :1453 — ram_read port
        5 => {
            emit_rnd(a);       // :1456 — noise draw
            a.shl32(RAX, 8);   // :1457
            a.sar32(RAX, 8);   // :1458 — sign-extend the 24-bit draw
        }
        6 => emit_p_packed(a, op.no_noise == 0), // :1461 p_packed(!o.no_noise)
        _ => a.load32(RAX, Mem::b(MS, o.m + 4 * op.sm as i32)), // :1464 default m[sm]
    }
}

/// origin: swp30_jit.cpp:1467-1475 — the dm STORE legs (RAX → destination).
/// `k < 0x17d && early_m[o.dm]` stores DIRECTLY into m[dm] and only ALSO
/// writes the ring slot when it is the last writer for that slot
/// (:1467-1470 — "not read, but keeps the state save identical" :713-714;
/// m_mw_value is SERIALIZED state, §5.3-6 Rust invariant); otherwise the
/// value enters the ring (:1472). The `mw_reg` TAIL BYTE (:1474-1475) sits
/// OUTSIDE the dm gate: at the tail trio (`k >= 0x17d`) or in branchy mode
/// it writes `o.dm` UNCONDITIONALLY — dm==0 clears the byte, which is
/// exactly the interpreter's no-op slot (the skipped-op eraser :1655-1659
/// rides the same byte). `s3` = slot3(d3,k) (:616), caller-computed.
pub fn emit_dm_store(a: &mut Assembler, o: &Offs, k: u32, op: &Op, s3: usize, an: &Analysis) {
    if op.dm != 0 {
        if k < 0x17d && an.early_m[op.dm as usize] {
            a.store32(Mem::b(MS, o.m + 4 * op.dm as i32), RAX); // :1468
            if an.last_slot_m[k as usize] {
                a.store32(Mem::b(MS, o.mw_value + 4 * s3 as i32), RAX); // :1470
            }
        } else {
            a.store32(Mem::b(MS, o.mw_value + 4 * s3 as i32), RAX); // :1472
        }
    }
    if k >= 0x17d || an.branchy {
        a.store8i(Mem::b(MS, o.mw_reg + s3 as i32), op.dm); // :1474-1475
    }
}

/// origin: swp30_jit.cpp:1480-1496 — the complete dr APPLY block. The
/// coalesced skipped-region seed jump goes FIRST (:1481-1482 — the draws of
/// every folded-away skipped op ride here so the rand stream stays aligned,
/// swp30.cpp:4272-4273/:4282-4298 = mix.rs:845-858). Then the value:
/// the `r[sr]` bank read (:1485 — the same live r[] the interpreter and the
/// ALU a-term asel==1 / mmode reads :679-685 consume, which is why this leg
/// "feeds the ALU bank reads"), or the p_packed twin (:1487). Stores mirror
/// the dm shape on the r/rw side (:1488-1493), and the `rw_reg` tail byte
/// (:1495-1496) twins :1474-1475 including its unconditional-at-tail/
/// branchy, zero-clears-the-slot semantics.
pub fn emit_dr_apply(a: &mut Assembler, o: &Offs, k: u32, op: &Op, s3: usize, an: &Analysis) {
    if op.rand_n != 0 {
        emit_rnd_skip(a, op.rand_n as u32); // :1481-1482
    }
    if op.dr != 0 {
        if op.dr_from_r != 0 {
            a.load32(RAX, Mem::b(MS, o.r + 4 * op.sr as i32)); // :1485
        } else {
            emit_p_packed(a, op.no_noise == 0); // :1487
        }
        if k < 0x17d && an.early_r[op.dr as usize] {
            a.store32(Mem::b(MS, o.r + 4 * op.dr as i32), RAX); // :1489
            if an.last_slot_r[k as usize] {
                a.store32(Mem::b(MS, o.rw_value + 4 * s3 as i32), RAX); // :1491
            }
        } else {
            a.store32(Mem::b(MS, o.rw_value + 4 * s3 as i32), RAX); // :1493
        }
    }
    if k >= 0x17d || an.branchy {
        a.store8i(Mem::b(MS, o.rw_reg + s3 as i32), op.dr); // :1495-1496
    }
}

// ---------------------------------------------------------------------------
// B2b-2c — memw accumulator + ix2 index op-loop legs (origin:
// swp30_jit.cpp:883-889 `AccFromP`/`ShrAcc`/`ShrAccTZ15` lambdas + :1501-1509
// memw + :1511-1525 index blocks). Additive scaffolding exactly like
// B2b-2a/2b: build() still refuses at the PHASE_B2_EMIT_OK gate, so NONE of
// these bytes reaches an exec buffer or runs — tests/meg_jit.rs executes the
// helpers STANDALONE (exec rigs), never a half-emitted program.
// Register contract (from the §3 plan + the B2b-2b note): RAX is the value
// accumulator every lambda returns (:842 comment on pack24), RCX is the
// ShrAccTZ15/pack24 scratch (:887/:845), RDX is the free §7-B window-deref
// scratch (B2b-2a note :1202-1204) — the ix2 legs load the window SLOT
// through it WITHOUT touching RAX, so the value survives to the store.
// Interpreter ground truth: meg.rs:881-900 (memw_value = `meg_mem_value(p)`
// = p/32768 truncate-TOWARD-ZERO :3843/:578 — NOT a plain `>>15`, which left
// reverb feedback tails stuck below 0, upstream.md 39; index_value/ix2_value
// = `p >> (15+8)` = arithmetic `>>23` :3814/:3819). The two therefore DIFFER
// for negative p (p=-1: memw 0, index -1) — a distinction the exec rigs pin.
// P (R13) is read-only here; no callout exists in these legs, so load_p_limits
// (:1448 twin) stays irrelevant within this slice.
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:884 `AccFromP` — copy the p accumulator (R13) into
/// the RAX value register. Shared by the memw (:1504), index (:1513/:1520)
/// and t (:1541) legs. `mov rax,r13` ⇒ REX 0x49 (rm form), `49 8b c5`.
pub fn emit_acc_from_p(a: &mut Assembler) {
    a.mov64(RAX, P); // :884
}

/// origin: swp30_jit.cpp:885 `ShrAcc(n)` — arithmetic-shift the accumulator
/// right by n. index/index2 use `ShrAcc(15+8)` (= arith `>>23`, meg.rs:893/
/// :899); the t index-form leg uses `ShrAcc(8)` (:1543). Arithmetic (sar64),
/// NOT round-toward-zero — only ShrAccTZ15 (:886) is the round helper.
pub fn emit_shr_acc(a: &mut Assembler, n: u8) {
    a.sar64(RAX, n); // :885
}

/// origin: swp30_jit.cpp:886-889 `ShrAccTZ15` — shift the accumulator right
/// 15 TOWARD ZERO (upstream.md 39; the ground truth is `meg_mem_value`
/// meg.rs:578 = p/32768). Round-toward-zero bias via the sign byte (:887
/// mov64/sar64 63/and32i 0x7fff, identical to pack24 :845-847) then the
/// arithmetic shift (:888). Feeds ONLY the memw value (:1505): a plain `>>15`
/// left reverb feedback tails stuck below 0 (meg.rs:575 discussion #69).
pub fn emit_shr_acc_tz15(a: &mut Assembler) {
    a.mov64(RCX, RAX); // :887
    a.sar64(RCX, 63); // :887 — -1 when negative
    a.and32i(RCX, 0x7fff); // :887 — round-toward-zero bias
    a.add64(RAX, RCX); // :888
    a.sar64(RAX, 15); // :888 — p / 32768 truncated toward 0
}

/// origin: swp30_jit.cpp:1501-1509 — the memw VALUE block. When the op writes
/// the delay memory (`o.memw`), the value is `ShrAccTZ15(AccFromP())` =
/// `meg_mem_value(p)` (truncate toward ZERO, meg.rs:884-885/:3843 — NOT a
/// plain `>>15`, upstream.md 39) stored into `memw_value[slot2(k)]` (:1506).
/// The `memw_act` ring byte is a 2-RING byte: written ONLY at the tail
/// (`k >= 0x17e`) or in branchy mode (:1508-1509) — the folded consumer
/// (`emit_ring2_folded`, B2b-2a :1066-1077) reads the value slot directly and
/// needs no act byte for mid-program non-branchy ops. Note the tail threshold
/// is the 2-ring `0x17e`, NOT the 3-ring `0x17d` the dm/dr/index legs use.
/// The value store is gated on `o.memw` (:1502) while the act byte is the
/// gated `o.memw ? 1 : 0` copy (:1509) — interpreter twin meg.rs:883-888
/// writes both under the same branch. `s2` = slot2(d2,k) (:617), caller-fed.
pub fn emit_memw_acc(a: &mut Assembler, o: &Offs, k: u32, op: &Op, s2: usize, an: &Analysis) {
    if op.memw != 0 {
        emit_acc_from_p(a); // :1504
        emit_shr_acc_tz15(a); // :1505
        a.store32(Mem::b(MS, o.memw_val + 4 * s2 as i32), RAX); // :1506
    }
    if k >= 0x17e || an.branchy {
        // :1509 `o.memw ? 1 : 0` (Op carries the decoded flag as u8, meg.rs:1036)
        a.store8i(Mem::b(MS, o.memw_act + s2 as i32), (op.memw != 0) as u8);
    }
}

/// origin: swp30_jit.cpp:1511-1525 — the index blocks. First index
/// (:1512-1518, meg_state side): `ShrAcc(15+8)` (arithmetic `>>23`,
/// meg.rs:893) into `index_value[slot3(k)]`. Second index (:1519-1525, device
/// side): the SAME `p >> 23` value goes to `ix2_value[slot3(k)]`, but through
/// the §7-B pointer window — `o_ix2_value` is a `*mut [i32;3]` SLOT, so load
/// the slot into the free RDX scratch and store `[rdx+4*s3]` (one deref where
/// C++ :1522 had a direct device field; the value lands in the same live
/// `Swp30::meg_ix2_value` the interpreter reads at meg.rs:899/:686). The act
/// bytes (`ix_act` MS side :1518, `ix2_act` window :1525) are 3-RING bytes
/// written ONLY at the tail (`k >= 0x17d`) or branchy. Both write `?1:0`
/// unconditionally inside that window (interpreter `swp.ix2_act[d3] =
/// d.index2 as u8` meg.rs:897), and the ix2 ACT deref also rides the window
/// (`o_ix2_act` is a `*mut [u8;3]` SLOT). `s3` = slot3(d3,k) (:616), fed by
/// the caller (the future op loop). The RDX load leaves RAX intact, so the
/// shifted p survives the window deref to its store.
pub fn emit_index_legs(a: &mut Assembler, o: &Offs, k: u32, op: &Op, s3: usize, an: &Analysis) {
    // ---- first index (meg_state side) :1512-1518 ----
    if op.index != 0 {
        emit_acc_from_p(a); // :1513
        emit_shr_acc(a, 15 + 8); // :1514 — arith >>23
        a.store32(Mem::b(MS, o.ix_value + 4 * s3 as i32), RAX); // :1515
    }
    if k >= 0x17d || an.branchy {
        a.store8i(Mem::b(MS, o.ix_act + s3 as i32), (op.index != 0) as u8); // :1517-1518
    }
    // ---- second index (device side, §7-B window deref) :1519-1525 ----
    if op.index2 != 0 {
        emit_acc_from_p(a); // :1520
        emit_shr_acc(a, 15 + 8); // :1521
        a.load64(RDX, Mem::b(SWP, o.ix2_value)); // *mut [i32;3] window slot
        a.store32(Mem::b(RDX, 4 * s3 as i32), RAX); // :1522 (window-deref)
    }
    if k >= 0x17d || an.branchy {
        a.load64(RDX, Mem::b(SWP, o.ix2_act)); // *mut [u8;3] window slot
        a.store8i(Mem::b(RDX, s3 as i32), (op.index2 != 0) as u8); // :1525 (window-deref)
    }
}

// ---------------------------------------------------------------------------
// B2b-2d — t/tval op-loop legs (origin: swp30_jit.cpp:1530-1561 folded t +
// need_tval publish, :1647-1675 branchy erase-twin; brief §1). Additive
// scaffolding exactly like B2b-2a/2b/2c: build() still refuses at the
// PHASE_B2_EMIT_OK gate, so NONE of these bytes reaches an exec buffer or
// runs — tests/meg_jit.rs executes the helpers STANDALONE (exec rigs),
// never a half-emitted program.
// Register contract (from the §3 plan + the B2b-2b/2c notes): RAX is the
// value register every lambda returns (:842 comment), RCX is the ±0x8000
// clamp scratch (:845 pack24 precedent), RDX the free §7-B window scratch
// (B2b-2a note) — P (R13) is read-only here, no callout exists, so
// load_p_limits (:1448 twin) stays irrelevant within this slice.
// Interpreter ground truth: run_program's t block meg.rs:1429-1441 (folded:
// the t WRITE reads `t_value[i2]`/`konst[pc]` BEFORE the publish overwrites
// the slot — :1430-1435 vs :1438-1441, the same load-then-store order as
// C++ :1533-1538 before :1560); index form `(p >> 8) & 0x7fff` (:4129 =
// meg.rs:1439, non-negative 15-bit by construction); clamp form
// s16_p23_clamped = `(p >> 23).clamp(-0x8000, 0x7fff)` (:3671/:4130 =
// meg.rs:617-618). The publish is FOLDED behind the compile-time
// `need_tval[k]` (analysis §5.3-5, :660-666 — only the tail pair and the op
// feeding a t-read-from-p two slots later keep it; the folded consumer is
// meg.rs:1431 `t_value[i2]` for t_from_p writers).
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:1552-1557 (x64 arm; the x86-32 jlt/jgt form
/// :1549-1550 is DEAD) — the ±0x8000 clamp of the t publish, shared by the
/// folded leg (:1546-1558) and the branchy twin (:1667-1672, identical
/// bytes). Two STRICT cmp/cmov64 pairs: below −0x8000 clamps to −0x8000,
/// above 0x7fff clamps to 0x7fff, and both exact boundaries pass through
/// unchanged — the byte-for-byte mirror of
/// `s16_p23_clamped` (meg.rs:618). Clobbers RCX (the clamp scratch, :845
/// precedent); value in/out RAX.
pub fn emit_tval_clamp(a: &mut Assembler) {
    a.imm64(RCX, (-0x8000i64) as u64); // :1552 u64(s64(-0x8000))
    a.cmp64(RAX, RCX); // :1553
    a.cmovl64(RAX, RCX); // :1554
    a.imm64(RCX, 0x7fff); // :1555
    a.cmp64(RAX, RCX); // :1556
    a.cmovg64(RAX, RCX); // :1557
}

/// origin: swp30_jit.cpp:1530-1561 — the non-branchy (op-loop-body) t leg.
/// t WRITE (:1531-1539): `t_from_p` loads the CURRENT ring slot
/// `t_value[slot2(k)]` — the value op k−2 published (or this sample's head
/// seed) — BEFORE the publish below overwrites it (:1533, interpreter order
/// meg.rs:1430); a baked spec op carries the konst[k] immediate (:1535,
/// `bake` arg — the ONLY leg besides the LFO hoist that consumes it), else
/// the runtime konst load (:1537). Stored 16-bit to `t[o.t]` (:1538).
/// t PUBLISH (:1540-1560, folded behind `an.need_tval[k]`): `AccFromP`
/// (:1541) then the INDEX form `ShrAcc(8); and 0x7fff` (:1543-1544,
/// `(p>>8)&0x7fff` = meg.rs:1439) when `op.index || op.index2`, else
/// arithmetic `>>23` + `emit_tval_clamp` (:1546-1557); stored to
/// `t_value[slot2(k)]` (:1560). `s2` = slot2(d2,k) (:617), caller-computed
/// (the future op loop).
pub fn emit_t_leg(
    a: &mut Assembler,
    o: &Offs,
    ms: &MegState,
    k: u32,
    op: &Op,
    s2: usize,
    bake: bool,
    an: &Analysis,
) {
    // ---- t write (:1531-1539) ----
    if op.t_write != 0 {
        if op.t_from_p != 0 {
            a.loadu16(RAX, Mem::b(MS, o.t_value + 2 * s2 as i32)); // :1533
        } else if bake {
            // :1535 `u16(ms.m_const[k])` — konst word baked at compile time
            a.imm32(RAX, ms.konst[k as usize] as u16 as u32);
        } else {
            a.loadu16(RAX, Mem::b(MS, o.konst + 2 * k as i32)); // :1537
        }
        a.store16(Mem::b(MS, o.t + 2 * op.t as i32), RAX); // :1538
    }
    // ---- t publish (:1540-1560) ----
    if an.need_tval[k as usize] {
        emit_acc_from_p(a); // :1541
        if op.index != 0 || op.index2 != 0 {
            emit_shr_acc(a, 8); // :1543
            a.and32i(RAX, 0x7fff); // :1544
        } else {
            emit_shr_acc(a, 15 + 8); // :1546 (arith >>23)
            emit_tval_clamp(a); // :1552-1557
        }
        a.store16(Mem::b(MS, o.t_value + 2 * s2 as i32), RAX); // :1560
    }
}

/// origin: swp30_jit.cpp:1647-1675 — the branchy twin, the skipped/jump-tail
/// ERASE body. Reached ONLY through the B2b-3 branch gate routing (`ja`
/// :1082-1083 over the skip counter, or the jump-op hop :1105→:1639 into
/// :1643) — which is why this leg itself carries NO runtime skip check: by
/// construction `skip > k` holds here, or op is the jump op itself.
/// The jump op still writes t when `t_write` (:1647-1654, upstream.md 31 —
/// interpreter meg.rs:1283-1290): the same two sources as the folded leg,
/// but WITHOUT the bake arm (:1652 always loads the runtime konst). The five
/// ring-byte erasers (:1655-1659 — "the write the skipped op would have
/// queued is erased, only t kept" :606) clear mw_reg/rw_reg/ix_act on the
/// 3-ring slot and memw_act on the 2-ring slot; the ix2_act byte rides the
/// §7-B window (RDX deref — one load where C++ :1659 addressed the device
/// field inline; it zeroes the live `Swp30::meg_ix2_act` interpreter twin
/// of meg.rs:1295). Finally the need_tval publish (:1660-1674): ALWAYS the
/// clamp form — even for index ops — because the interpreter's jump/skip
/// path publishes `s16_p23_clamped(p)` unconditionally (meg.rs:1296 =
/// :4027); the index `>>8&0x7fff` form lives only in the folded leg.
pub fn emit_t_branchy(
    a: &mut Assembler,
    o: &Offs,
    k: u32,
    op: &Op,
    s2: usize,
    s3: usize,
    an: &Analysis,
) {
    // ---- jump-op t write (:1647-1654, no bake arm) ----
    if op.jump != 0 && op.t_write != 0 {
        if op.t_from_p != 0 {
            a.loadu16(RAX, Mem::b(MS, o.t_value + 2 * s2 as i32)); // :1650
        } else {
            a.loadu16(RAX, Mem::b(MS, o.konst + 2 * k as i32)); // :1652
        }
        a.store16(Mem::b(MS, o.t + 2 * op.t as i32), RAX); // :1653
    }
    // ---- ring-byte erasers (:1655-1659) ----
    a.store8i(Mem::b(MS, o.mw_reg + s3 as i32), 0); // :1655
    a.store8i(Mem::b(MS, o.rw_reg + s3 as i32), 0); // :1656
    a.store8i(Mem::b(MS, o.memw_act + s2 as i32), 0); // :1657
    a.store8i(Mem::b(MS, o.ix_act + s3 as i32), 0); // :1658
    a.load64(RDX, Mem::b(SWP, o.ix2_act)); // :1659 §7-B window slot deref
    a.store8i(Mem::b(RDX, s3 as i32), 0); // :1659 (live meg_ix2_act)
    // ---- t publish, clamp form always (:1660-1674) ----
    if an.need_tval[k as usize] {
        emit_acc_from_p(a); // :1661
        emit_shr_acc(a, 15 + 8); // :1662 (arith >>23)
        emit_tval_clamp(a); // :1667-1672
        a.store16(Mem::b(MS, o.t_value + 2 * s2 as i32), RAX); // :1674
    }
}

// ---------------------------------------------------------------------------
// B2b-2e — memop ADDRESS leg (origin: swp30_jit.cpp:1566-1638; brief §1).
// Additive scaffolding exactly like B2b-2a..2d: build() still refuses at the
// PHASE_B2_EMIT_OK gate, so NONE of these bytes reaches an exec buffer or
// runs — tests/meg_jit.rs executes the helpers STANDALONE (exec rigs),
// never a half-emitted program.
// Register contract (from the §3 plan + the B2b-2a..2d notes): RAX is the
// address/value register every leg ends in (:1582/:1613), RCX is the ix-term
// scratch (:1572/:1595), RDX the free §7-B window scratch (B2b-2a note — the
// ix2 load reads the `ram_index2` SLOT, one deref where C++ :1576/:1599
// addressed the device inline), R8 holds the write-leg address across the
// encode (:1623 — SAFE: emit_revram_encode clobbers rcx/rdx/r11 only, RENC
// :69; RDEC/R8 is touched by DECODE alone, which the write leg never runs).
// The scale-2 RAM operand `mem{RAM, RAX|R8, 2, 0}` (:1582/:1626/:1630) is the
// pub-fields Mem literal (handoff §6; emit_ring3_head set the precedent with
// scale 4). RAM (R15) is indexed, never written as a base. P/SEED untouched,
// no callout in this block — load_p_limits (:1448 twin) stays irrelevant.
// Interpreter ground truth (PAIRED meg.rs, run_program :4132-4158):
// mem_table read = offset+ix+ix2(+1) u32-wrap then &0x3ffff, NO map and NO
// sample counter (meg.rs:1446-1459, upstream.md 24); normal leg =
// (off+ix+ix2−SC(+1) & addr_mask)+addr_base & 0x3ffff (meg.rs:1468-1483) —
// all 32-bit ops, so the −SC wrap is u32 (offset 0 − SC 1 → 0xffff_ffff,
// mask-clipped); `BIT(m_revram_enable, region)` SET means the region is
// DISABLED (mamecompat.h:238 BIT; meg.rs:1462 twin): writes are dropped and
// reads forced 0 at COMPILE time (:1587-1591 — "無効な区画への出し入れは、
// 訳すときに省く"), which is why invalidate rides revram_enable_w
// (swp30.cpp:2488-2491) and run() re-checks (:396-400).
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:1570-1578 ≡ :1593-1601 — the shared address base
/// both memop paths open with: the runtime offset word (firmware rewrites
/// the table mid-song — header :12; never baked even under BAKE), plus the
/// optional first index (MS side) and second index (§7-B window deref).
/// All three legs are 32-bit (meg.rs:1468-1478 u32 wrapping_*), so the sum
/// keeps the interpreter's u32 wrap semantics. RAX out.
pub fn emit_memop_addr_base(a: &mut Assembler, o: &Offs, op: &Op) {
    a.loadu16(RAX, Mem::b(MS, o.offset + 2 * op.offset_index as i32)); // :1570/:1593
    if op.mem_use_index != 0 {
        a.load32(RCX, Mem::b(MS, o.ram_index)); // :1572/:1595
        a.add32(RAX, RCX);                       // :1573/:1596
    }
    if op.mem_use_index2 != 0 {
        a.load64(RDX, Mem::b(SWP, o.ram_index2)); // §7-B window slot (:1576/:1599)
        a.load32(RCX, Mem::b(RDX, 0));
        a.add32(RAX, RCX); // :1577/:1600
    }
}

/// origin: swp30_jit.cpp:1593-1612 — the region-mapped (normal) address leg
/// (the brief's `emit_memop_addr`): base sum, minus the sample counter
/// (:1606 x64 arm — the x86-32 `FM(F_SC)` spill :1602-1604 is DEAD), the
/// memop-3 +1 (:1608-1609), then the COMPILE-TIME baked map legs
/// `& o.addr_mask` / `+ o.addr_base` (:1610-1611, value-equivalent twin of
/// meg.rs:1483) and the 18-bit wrap (:1612). RAX in/out.
pub fn emit_memop_addr(a: &mut Assembler, o: &Offs, op: &Op) {
    emit_memop_addr_base(a, o, op); // :1593-1601
    a.sub32(RAX, SC); // :1606 — u32 wrap (meg.rs:1479 wrapping_sub)
    if op.memop == 3 {
        a.add32i(RAX, 1); // :1608-1609
    }
    a.and32i(RAX, op.addr_mask); // :1610 baked (build_ops :3972)
    a.add32i(RAX, op.addr_base); // :1611 baked (build_ops :3973)
    a.and32i(RAX, 0x3ffff); // :1612 — 18-bit wrap (meg.rs:1483)
}

/// origin: swp30_jit.cpp:1568-1586 — the mem_table ABSOLUTE read
/// (bit 0x23, upstream.md 24, meg.rs:1446-1459): no map, no addr_mask/
/// addr_base, NO sample-counter subtraction; base sum + the memop-3 +1
/// (:1579-1580) & 0x3ffff (:1581), scale-2 RAM load (:1582), the PAIRED
/// emit_revram_decode (:1583) and the 2-ring value store (:1584). Returns
/// the `table_done` jmp label (:1585) for the caller to patch at :1635-1636
/// — the C++ 0-sentinel works here unconditionally because the e9's rel32
/// field sits ≥ 7 bytes into the buffer (the offset load disp32; jit_emit
/// rm() is ALWAYS disp32), so the label is never 0 — same reasoning as
/// C++ `size_t table_done = 0` (:1567).
pub fn emit_memop_table(a: &mut Assembler, o: &Offs, op: &Op, s2: usize) -> usize {
    emit_memop_addr_base(a, o, op); // :1570-1578
    if op.memop == 3 {
        a.add32i(RAX, 1); // :1579-1580
    }
    a.and32i(RAX, 0x3ffff); // :1581 — absolute, map-free
    a.loadu16(RAX, Mem { base: RAM, index: RAX, scale: 2, disp: 0 }); // :1582
    emit_revram_decode(a); // :1583
    a.store32(Mem::b(MS, o.memr_val + 4 * s2 as i32), RAX); // :1584
    a.jmp_fwd() // :1585 — over the region gate + normal path
}

/// origin: swp30_jit.cpp:1566-1638 — the whole per-op memop block.
/// Order (C++ verbatim): table leg first (:1568-1586) — it bypasses the
/// enable gate exactly like the interpreter's :1446-first test; then the
/// COMPILE-TIME region gate :1589: `BIT(revram_enable, op.region)` set ⇒
/// region DISABLED — reads emit only the forced-0 store (:1590-1591),
/// writes emit NOTHING (dropped); RAM is never touched and the address math
/// is never emitted for that case (why revram_enable_w invalidates,
/// swp30.cpp:2488-2491, and run() re-checks the latch :396-400 — handoff
/// §5.1). ENABLED regions take `emit_memop_addr` (:1593-1612) then the
/// scale-2 store leg: write = park the address in R8 (survives the encode,
/// RENC=R11), eax=ram_write, PAIRED emit_revram_encode (:1623-1626 ≡
/// meg.rs:1485), read = scale-2 load + PAIRED emit_revram_decode + the
/// 2-ring value store (:1630-1632 ≡ meg.rs:1487-1488). The `table_done`
/// patch lands BEFORE the 2-ring `memr_act` byte (:1635-1638) — the byte
/// rides the 2-ring tail threshold `k >= 0x17e` (NOT the 3-ring 0x17d;
/// same gate note as emit_memw_acc) or branchy, and is 1 only for reads
/// (memop 2/3 ≡ the interpreter's memr_active=true legs meg.rs:1459/:1465/
/// :1489 — writes never arm it). `s2` = slot2(d2,k) (:617), caller-fed.
pub fn emit_memop(
    a: &mut Assembler,
    o: &Offs,
    k: u32,
    op: &Op,
    s2: usize,
    revram_enable: u16,
    an: &Analysis,
) {
    // :1567 — C++ 0-sentinel; see emit_memop_table's never-0 note.
    let mut table_done = 0usize;
    if op.memop >= 2 && op.mem_table != 0 {
        table_done = emit_memop_table(a, o, op, s2); // :1568-1585
    }
    // :1589 — the compile-time gate. BIT(v,n) = (v>>n)&1 (mamecompat.h:238);
    // `op.region` was resolved by build_ops (:3974 = meg.rs:1198), so this is
    // a constant decision per op. Bit SET = region disabled (meg.rs:1462).
    let region_off = (revram_enable >> op.region) & 1 != 0;
    if op.memop != 0 && region_off {
        if op.memop != 1 {
            a.store32i(Mem::b(MS, o.memr_val + 4 * s2 as i32), 0); // :1590-1591
        }
        // (memop==1: the write is simply not emitted — 書き込みは落ちる)
    } else if op.memop != 0 {
        emit_memop_addr(a, o, op); // :1593-1612
        if op.memop == 1 {
            a.mov64(R8, RAX); // :1623 — address survives the encode
            a.load32(RAX, Mem::b(MS, o.ram_write)); // :1624
            emit_revram_encode(a); // :1625
            a.store16(Mem { base: RAM, index: R8, scale: 2, disp: 0 }, RAX); // :1626
        } else {
            a.loadu16(RAX, Mem { base: RAM, index: RAX, scale: 2, disp: 0 }); // :1630
            emit_revram_decode(a); // :1631
            a.store32(Mem::b(MS, o.memr_val + 4 * s2 as i32), RAX); // :1632
        }
    }
    if table_done != 0 {
        a.patch(table_done); // :1635-1636
    }
    if k >= 0x17e || an.branchy {
        a.store8i(Mem::b(MS, o.memr_act + s2 as i32), ((op.memop == 2 || op.memop == 3) as u8)); // :1637-1638
    }
}

// ---------------------------------------------------------------------------
// B2a — frame: prologue/epilogue skeleton (origin: swp30_jit.cpp:826-840
// + :1688-1694, x64 arm; the x86-32 halves :737-812/:1682-1687 are DEAD).
// Handoff §3/§5.3-7/§5.3-10. tests/meg_jit.rs proves the emitted bytes
// terminate with `ret`, keep rsp balanced across entry/exit, and leave rsp
// 16-aligned at every frame-body callout position (S11 callee-save lesson:
// honor Win64 callee-saves exactly — jit.rs:397-400).
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:818 `FM` — a frame-slot operand (rsp-based). Live
/// readers: `emit_lfo_slot_store` (:988) and the `emit_dm_src` frame-slot
/// load (:1419) landed in B2b-2b (with the `emit_lfo` head B2b-3a); the hoist
/// SCAN (:971-991) that fills the slots arrives B2b-3b.
pub const fn fm(disp: i32) -> Mem {
    Mem { base: RSP, index: NOREG, scale: 1, disp }
}

/// origin: swp30_jit.cpp:820-822 `load_p_limits` — ±(2^38−1) saturation
/// limits. MANDATORY after every callout (R9/R10 are volatile on Win64;
/// :819 comment, :1448 after call_lfo).
pub fn load_p_limits(a: &mut Assembler) {
    // B2b-3b FIX: digit-count exact vs disk :821-822 — 0x3fffffffff is
    // 0x3f_ffff_ffff (2^38−1), NOT 0x3fff_ffff_ffff (2^46−1); the clamp
    // bounds are 2^38, the same "digit-count quirk" meg.rs:1349-1355 pins
    // for the interpreter (saturation ≠ the 42-bit wrap width).
    a.imm64(P_MAX, 0x3f_ffff_ffff); // :821
    a.imm64(P_MIN, (-0x40_0000_0000i64) as u64); // :822 u64(s64(-0x4000000000))
}

/// origin: swp30_jit.cpp:826-837 entry. ARG0=ms, ARG1=swp, ARG2=reverb RAM
/// (fn_t :266 — RAM is a per-call ARG, never baked, :512). o_p/o_sample are
/// `offset_of!` MegState constants (§5.3-3, `offs()`); o_seed is the
/// MegSwpDev SLOT offset — SWP base is the §7-B pointer window, so the
/// seed leg loads the live `*mut u32` first (two loads where C++ :834 had
/// one; values land in the same `Swp30::rand_seed`). RAX is free scratch
/// here (ARGs were consumed at :829-831, nothing else live until the op
/// loop) — jit_emit rm() is ALWAYS disp32 (jit_emit.rs:105), so these
/// bytes are offset-independent in length.
///
/// Exactly 8 pushes + FRAME=152 (handoff §3 risk 5): entry rsp≡8 (mod 16)
/// from the call → −64 −152 → rsp ≡ 0 at the frame body, which is what
/// every Win64 callout (the emit_lfo leg :1443) needs; the epilogue
/// restores the balance exactly.
pub fn emit_prologue(a: &mut Assembler, o_p: i32, o_sample: i32, o_seed: i32) {
    // :827 — RBX R12 R13 R14 R15 RSI RDI RBP
    a.push(RBX);
    a.push(R12);
    a.push(R13);
    a.push(R14);
    a.push(R15);
    a.push(RSI);
    a.push(RDI);
    a.push(RBP);
    a.subrsp(FRAME as u32);              // :828 影32+置き場16+LFO96+揃え8
    a.mov64(MS, ARG0);                   // :829
    a.mov64(SWP, ARG1);                  // :830 — SWP = MegSwpDev window base (§7-B)
    a.mov64(RAM, ARG2);                  // :831
    a.load64(P, Mem::b(MS, o_p));        // :832 — p accumulator
    a.load32(SC, Mem::b(MS, o_sample));  // :833 — sample counter
    a.load64(RAX, Mem::b(SWP, o_seed));  // §7-B: deref the seed slot ...
    a.load32(SEED, Mem::b(RAX, 0));      // :834 — ... rand seed now lives in RSI
    a.imm64(K_MAX, 0x7fffff);              // :835 — pack24 limit (no per-use imm)
    a.imm64(K_MIN, (-0x800000i64) as u64); // :836
    load_p_limits(a);                    // :837
}

/// origin: swp30_jit.cpp:1688-1694 exit. The seed returns to the device
/// HERE only (:1690 — mid-program the seed stays in RSI, handoff §5.3-10);
/// through the §7-B window (store through the live `*mut u32`). RAX is
/// dead after :1689-1690 — nothing downstream reads it before the pops.
pub fn emit_epilogue(a: &mut Assembler, o_p: i32, o_seed: i32) {
    a.store64(Mem::b(MS, o_p), P);        // :1689
    a.load64(RAX, Mem::b(SWP, o_seed));   // §7-B window slot
    a.store32(Mem::b(RAX, 0), SEED);      // :1690
    a.addrsp(FRAME as u32);              // :1691
    a.pop(RBP);                          // :1692 — exact LIFO inverse of :827
    a.pop(RDI);
    a.pop(RSI);
    a.pop(R15);
    a.pop(R14);
    a.pop(R13);
    a.pop(R12);
    a.pop(RBX);
    a.ret();                             // :1694
}

/// B2b-1: the frame skeleton WITHOUT the op loop — now with the REAL
/// offset table (`offs()` §5.3-3) and the `branchy` analysis result
/// (:607-610). KEPT for the B2b-1/B2b-2a byte-pin tests (they transmute
/// the skeleton standalone); the LIVE program is `emit_program` below,
/// which stitches the :995-1679 op loop between the same prologue and
/// epilogue (B2b-3b). B2b-2a: the :839-840 branchy skip reset lands here,
/// width-adapted 16-bit for the Rust u16 `meg_skip_to` (§7-B window slot
/// deref + `store16i`; INERT for non-branchy programs either way, exactly
/// like the C++ `if (branchy)`).
/// Nothing here is ever executed: build() still returns false at the
/// PHASE_B2_EMIT_OK gate, so no skeleton byte is copied into an exec
/// buffer (tests/meg_jit.rs `b2b1_build_inert` + `b2b2a_build_inert` pin
/// fnp==0).
pub fn emit_frame_skeleton(a: &mut Assembler, o: &Offs, branchy: bool) {
    emit_prologue(a, o.p, o.sample, o.seed);
    if branchy {
        emit_skip_reset(a, o); // :839-840 (u16-adapted, B2b-2a)
    }
    emit_epilogue(a, o.p, o.seed);
}

// ---------------------------------------------------------------------------
// B2b-3b — the FULL program: LFO hoist SCAN (:971-991) + the per-op k-loop
// (:995-1679) stitching every landed emitter at its exact C++ call point.
// Transliterated from build()'s x64 arms (:723-1694 — the x86-32 halves are
// DEAD, §5.3); §5.3-8/9/10 is the sequence map. THE PROGRAM IS STILL INERT:
// build() assembles this into its PRIVATE Assembler and refuses at the
// PHASE_B2_EMIT_OK gate, so no byte reaches an exec buffer and `fnp` stays 0
// (`b2b3b_build_inert`). The only executors are the tests/meg_jit.rs
// FULL-PROGRAM parity rigs through `program_bytes` (below), A/B against the
// PAIRED `meg::run_program` — the same CHECK idea as :426-511.
// Register contract (handoff §3 plan, :814/:819): live across the loop are
// MS(RBX) SWP(R12) P(R13) SC(R14) RAM(R15) SEED(RSI) K_MAX(RDI) K_MIN(RBP)
// P_MAX(R9) P_MIN(R10); RAX is the value register, RCX/RDX the C++ legs' own
// scratch, R8/R11 the revram/LFO scratch — R8/RDX additionally serve the §7-B
// window derefs (flag_n/flag_z/skip) inside the new legs. No callout exists
// between the prologue and the dm callout (:1443), and `load_p_limits`
// (the :1448 twin) reloads R9/R10 right after it.
// CHECK gate leg (§4/§7-F/§9): the loop bound is `meg_jit_upto()`, whose
// inverted `env_flag_on` polarity (`set && first char != '0'`, :322-325)
// makes the DEFAULT 0x180 — byte-identical to the C++ x64 fixed bound
// (:995; the aarch64 arm :2036-2037 is DEAD here). Only `SMU2000_MEG_JIT_CHECK`
// set non-'0' (plus `_UPTO`) ever truncates the emitted program — the bisect
// knob "ignored unless the check above is on" (:2035, kept coupled in
// meg_jit_upto :335-337). So no CHECK-specific byte is emitted unless the
// CHECK env is ON at compile time — faithful to C++.
// Stats NOTE: the C++ `jit_stats st` / `sz_*` counters (:724-728, :1160-1186,
// :1411, :1477/:1498/:1527/:1563) are compile-time-only bookkeeping printed
// under SMU2000_MEG_JIT_STATS (:1713-1731, a Rust TODO with the stats block —
// they emit ZERO machine-code bytes, so the transliteration omits them with
// this note; the loop bodies below are otherwise instruction-for-instruction).
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:971-991 — the LFO hoist SCAN. "Only the numbers
/// used 2+ times get built at the head" (:971 comment — once only is a net
/// loss against parking+reading). The count pass (:975-979) runs only with
/// a resident sintab (:976 gate — the GATED sintab, :645). The emit pass
/// (:981-989; the x86-32 `#if` exclusion :980 is DEAD here) walks
/// `i != 0x18`, and for `uses[i] >= 2` (:983): mark hoisted, allocate the
/// next frame slot `LFO_SLOT_BASE + 4n` (:985, FRAME/96 B = 24 slots — the
/// max n is 0x18 slots ≤ 96 B, never past FRAME), `emit_lfo(i)` (:987) and
/// store RAX into it (:988, `emit_lfo_slot_store`). The dm_src 0-3 legs
/// then read the slot (:1418-1419) or run `emit_lfo` in place (:1421) —
/// loop-invariant because `lfo_step()` runs only BETWEEN samples (:892-896;
/// Rust `lfo_step` meg.rs — do NOT move). Values out: `lfo_slot[i] != 0`
/// ⇔ hoisted (0 = not hoisted, mirroring the C++ zero-init :973).
pub fn emit_lfo_hoist_scan(
    a: &mut Assembler,
    o: &Offs,
    ops: &[Op; 0x180],
    sintab: *const u16,
) -> [i32; 0x18] {
    let mut lfo_slot = [0i32; 0x18]; // :973 (lfo_hoist[] is implicit in != 0)
    let mut uses = [0u32; 0x18]; // :975
    if !sintab.is_null() {
        // :976-979 — count uses per LFO number (dm gate first, then src≤3)
        for op in ops.iter() {
            if op.dm != 0 && op.dm_src <= 3 && (op.lfo as usize) < 0x18 {
                uses[op.lfo as usize] += 1;
            }
        }
    }
    let mut n = 0u32; // :981
    for i in 0..0x18u32 {
        if uses[i as usize] >= 2 {
            // :983-984
            lfo_slot[i as usize] = LFO_SLOT_BASE + (4 * n as i32); // :985
            n += 1;
            emit_lfo(a, o, i, sintab); // :987
            emit_lfo_slot_store(a, lfo_slot[i as usize]); // :988
        }
    }
    lfo_slot
}

/// origin: swp30_jit.cpp:971-1694 — the whole program body: the :971-991
/// hoist scan, the :995-1679 per-op loop and the epilogue (:1688-1694
/// through the same B2a helper). `d3`/`d2` are the ring snapshot COMMITTED
/// to the code (build()'s :614-615 — slot3/slot2 close over them, :616-617).
/// Per-op ORDER is C++ verbatim (:995 comment §5.3-9: ring-apply → branch
/// gate → ALU → dm → dr → memw → index → t → memop → post-branch fixups).
/// `#[allow(clippy::too_many_arguments)]` — the C++ builds read the same
/// eleven things from build()'s scope (:598-995).
#[allow(clippy::too_many_arguments)]
pub fn emit_program(
    a: &mut Assembler,
    o: &Offs,
    ms: &MegState,
    ops: &[Op; 0x180],
    revram_enable: u16,
    bake: bool,
    an: &Analysis,
    d3: u32,
    d2: u32,
    sintab: *const u16,
    sintab_len: usize,
) {
    let branchy = an.branchy; // :607-610 (already scanned — analyze_ops)

    // ---- prologue :826-837 + branchy skip reset :839-840 ----
    emit_prologue(a, o.p, o.sample, o.seed);
    if branchy {
        emit_skip_reset(a, o); // :839-840 (u16-adapted, B2b-2a)
    }

    // ---- :971-991 LFO hoist SCAN (fills the frame slots the dm legs read) ----
    let lfo_slot = emit_lfo_hoist_scan(a, o, ops, sintab);

    // ---- :995 per-op loop. CHECK gate leg: bound = meg_jit_upto() —
    // DEFAULT 0x180 (== C++ x64 fixed bound :995); < 0x180 only with
    // SMU2000_MEG_JIT_CHECK set non-'0' (inverted env_flag_on polarity,
    // :322-325 — see section note). ----
    let upto = meg_jit_upto();
    for k in 0..upto {
        let op = &ops[k as usize]; // :996
        let s3 = slot3(d3, k); // :616
        let s2 = slot2(d2, k); // :617

        // ---- 3-ring apply :1000-1049 (head dynamic, else folded k-3) ----
        if k < 3 || branchy {
            emit_ring3_head(a, o, s3); // :1000-1029
        } else {
            emit_ring3_folded(a, o, s3, &ops[k as usize - 3], an); // :1030-1048
        }
        // ---- 2-ring (mem ports) :1050-1077 ----
        if k < 2 || branchy {
            emit_ring2_head(a, o, s2); // :1050-1065
        } else {
            emit_ring2_folded(a, o, s2, &ops[k as usize - 2]); // :1066-1077
        }

        // ---- branch gate :1079-1104 (branchy only) ----
        // Skip slot read: §7-B window deref + U16 width adaptation (brief §2:
        // Rust meg_skip_to is u16; the C++ u32 legs :1082/:1099 become
        // cmp16i_mem/store16i through the *mut u16 — same decision, k < 0x180).
        let mut skip_jump = 0usize; // :1080
        let mut jump_done = 0usize; // :1080
        if branchy {
            a.load64(RDX, Mem::b(SWP, o.skip)); // §7-B deref (RDX free here)
            a.cmp16i_mem(Mem::b(RDX, 0), k as u16); // :1082 (u16-adapted)
            skip_jump = a.jcc_fwd(0x87); // :1083 ja — skip lands past this op
            if op.jump != 0 {
                // meg_cond in machine code :1085-1096. !cond&8 ⇒ always true
                // (:1087 skip; meg.rs:604-605 `meg_cond` returns true there).
                let mut no_jump = 0usize;
                if op.cond & 8 != 0 {
                    a.load64(RDX, Mem::b(SWP, o.flag_n)); // :1088 §7-B deref
                    a.loadu8(RAX, Mem::b(RDX, 0)); // :1088
                    if op.cond & 4 == 0 {
                        a.xor32ri(RAX, 1); // :1089-1090
                    }
                    if op.cond & 2 != 0 {
                        a.load64(RDX, Mem::b(SWP, o.flag_z)); // :1092 §7-B deref
                        a.loadu8(RCX, Mem::b(RDX, 0)); // :1092
                        a.or32(RAX, RCX); // :1093
                    }
                    a.test32(RAX, RAX); // :1095
                    no_jump = a.jz_fwd(); // :1096
                }
                if (op.target as u32) > k {
                    // :1098-1099 — target>k is a COMPILE-TIME constant (§5.3-9;
                    // backward jumps never set skip — meg.rs:1280 twin)
                    a.load64(RDX, Mem::b(SWP, o.skip)); // §7-B deref
                    a.store16i(Mem::b(RDX, 0), op.target); // :1099 (u16-adapted)
                }
                if no_jump != 0 {
                    a.patch(no_jump); // :1100-1101
                }
                jump_done = a.jmp_fwd(); // :1102 the jump op joins the erase tail
            }
        }

        // ---- op body, elided for the jump op itself :1105 (→ :1639) ----
        if !(branchy && op.jump != 0) {
            // ================= ALU :1107-1409 =================
            // BAKE fold :1111-1159 (only !m1_from_t && mmode!=3 :1111).
            let mut alu_skip = false; // :1110
            if op.alu != 0 && bake && op.m1_from_t == 0 && op.mmode != 3 {
                let mut c: i64 = ms.konst[k as usize] as i64; // :1112 (s16 sext)
                if op.m1_expand != 0 {
                    c = MegState::m1_expand(c as i16) as i64; // :1113-1114
                }
                let m_zero = op.mmode == 0 || c == 0; // :1115
                if m_zero
                    && op.asel == 0
                    && op.rop == 0
                    && op.shift == 0
                    && op.clamp == 0
                    && op.latch == 0
                {
                    alu_skip = true; // :1116-1117 — p already fits 42 bits
                } else if m_zero {
                    a.xor32(RAX, RAX); // :1119 (acc hi is the same reg here)
                } else if op.mmode == 1 {
                    // :1128 imm64 c<<(8+15) — wrapping_shl mirrors the
                    // interpreter's own wrap (meg.rs:1326), same in-range c
                    a.imm64(RAX, c.wrapping_shl(8 + 15) as u64);
                } else {
                    // :1138 loads32 m2 operand (32-bit read, sign-extended)
                    let m2 = if op.m2_from_m != 0 {
                        Mem::b(MS, o.m + 4 * op.sm as i32)
                    } else {
                        Mem::b(MS, o.r + 4 * op.sr as i32)
                    };
                    a.loads32(RAX, m2); // :1138
                    // :1139-1157 pow2 constant ⇒ shl (+neg) instead of imul
                    // (imul 3 cycles, shl 1 — 1〜2割 of song coefficients)
                    let ac: u64 = if c < 0 { c.unsigned_abs() } else { c as u64 }; // :1143
                    let mut sh: i32 = -1; // :1144
                    if ac != 0 && (ac & (ac - 1)) == 0 {
                        sh = 0;
                        while (1u64 << sh) != ac {
                            sh += 1; // :1145-1147
                        }
                    }
                    if sh < 0 {
                        a.imul64i(RAX, RAX, c as u32); // :1149 u32(s32(c))
                    } else {
                        if sh != 0 {
                            a.shl64(RAX, sh as u8); // :1151-1152
                        }
                        if c < 0 {
                            a.neg64(RAX); // :1153-1154
                        }
                    }
                }
            }
            // :1160-1186 st.* stats — ZERO emitted bytes (see section note)
            if op.alu != 0 && !alu_skip {
                // m1 source + mmode :1189-1260 (skipped when BAKE folded it)
                if !(bake && op.m1_from_t == 0 && op.mmode != 3) {
                    if op.m1_from_t == 2 {
                        // :1230-1236 sign-of-p select t-vs-const (the
                        // "upstream.md 29" quirk, meg.rs:1306-1311)
                        a.loads16(RAX, Mem::b(MS, o.t + 2 * op.t as i32)); // :1232
                        a.loads16(RCX, Mem::b(MS, o.konst + 2 * k as i32)); // :1233
                        a.load64(R8, Mem::b(SWP, o.flag_n)); // :1234 §7-B deref
                        a.loadu8(RDX, Mem::b(R8, 0)); // :1234
                        a.test32(RDX, RDX); // :1235
                        a.cmove64(RAX, RCX); // :1236 zf (flag_n==0) ⇒ const
                    } else if op.m1_from_t != 0 {
                        a.loads16(RAX, Mem::b(MS, o.t + 2 * op.t as i32)); // :1238
                    } else {
                        a.loads16(RAX, Mem::b(MS, o.konst + 2 * k as i32)); // :1240
                    }
                    if op.m1_expand != 0 {
                        emit_m1_expand(a); // :1241-1242 (clobbers RCX)
                    }
                    let m2 = if op.m2_from_m != 0 {
                        Mem::b(MS, o.m + 4 * op.sm as i32)
                    } else {
                        Mem::b(MS, o.r + 4 * op.sr as i32)
                    };
                    match op.mmode {
                        0 => a.xor32(RAX, RAX), // :1244-1245
                        1 => a.shl64(RAX, 8 + 15), // :1248 (meg.rs:1326 wrap)
                        2 => {
                            a.loads32(RCX, m2); // :1251
                            a.imul64(RAX, RCX); // :1252
                        }
                        _ => {
                            // :1254-1256 mmode==3: m/r value <<15, m1 unused
                            a.loads32(RAX, m2);
                            a.shl64(RAX, 15);
                        }
                    }
                }
                // asel :1261-1281 — asel==0 uses P DIRECTLY (no copy,
                // :1261-1262/:1275 — the p chain stays unbroken)
                let mut b: u8 = RCX; // :1264
                match op.asel {
                    0 => b = P, // :1275
                    1 => {
                        a.loads32(RCX, Mem::b(MS, o.r + 4 * op.sr as i32)); // :1276
                        a.shl64(RCX, 15);
                    }
                    2 => {
                        a.loads32(RCX, Mem::b(MS, o.m + 4 * op.sm as i32)); // :1277
                        a.shl64(RCX, 15);
                    }
                    3 => {
                        a.mov64(RCX, P); // :1278
                        a.sar64(RCX, 15);
                    }
                    _ => a.xor32(RCX, RCX), // :1279 (asel 4 = zero)
                }
                match op.rop {
                    0 => a.add64(RAX, b), // :1287
                    1 => a.sub64(RAX, b), // :1294
                    2 => {
                        // :1308-1311 a + |b| via neg/cmovs
                        a.mov64(RDX, b);
                        a.neg64(RDX);
                        a.cmovs64(RDX, b);
                        a.add64(RAX, RDX);
                    }
                    _ => a.and64(RAX, b), // :1318
                }
                if op.shift != 0 {
                    a.shl64(RAX, op.shift); // :1322-1326
                }
                if op.clamp == 0 {
                    // :1328-1335 — no clamp ⇒ ±2^41 wrap via shl22/sar22
                    // (the 42-bit sext from the Pitfalls list, meg.rs:1347)
                    a.shl64(RAX, 22);
                    a.sar64(RAX, 22);
                }
                match op.clamp {
                    0 => {}
                    1 => {
                        // :1351-1354 saturate to ±P_MAX/P_MIN (R9/R10 pinned)
                        a.cmp64(RAX, P_MIN);
                        a.cmovl64(RAX, P_MIN);
                        a.cmp64(RAX, P_MAX);
                        a.cmovg64(RAX, P_MAX);
                    }
                    2 => {
                        // :1366-1370 clamp to [0..P_MAX]
                        a.xor32(RCX, RCX);
                        a.cmp64(RAX, RCX);
                        a.cmovl64(RAX, RCX);
                        a.cmp64(RAX, P_MAX);
                        a.cmovg64(RAX, P_MAX);
                    }
                    _ => {
                        // :1383-1388 |acc| ≤ P_MAX
                        a.mov64(RDX, RAX);
                        a.neg64(RDX);
                        a.cmovs64(RDX, RAX);
                        a.mov64(RAX, RDX);
                        a.cmp64(RAX, P_MAX);
                        a.cmovg64(RAX, P_MAX);
                    }
                }
                a.mov64(P, RAX); // :1401 — the accumulator commits to R13
                if op.latch != 0 {
                    // :1402-1407 — flags through the §7-B window (bool slots,
                    // setl/sete write 0/1 — still valid bools; RDX/R8 free
                    // here, test64 clobbers neither)
                    a.load64(RDX, Mem::b(SWP, o.flag_n)); // :1404 §7-B deref
                    a.test64(P, P); // :1403
                    a.setl_mem(Mem::b(RDX, 0)); // :1404
                    a.load64(R8, Mem::b(SWP, o.flag_z)); // :1406 §7-B deref
                    a.test64(P, P); // :1405
                    a.sete_mem(Mem::b(R8, 0)); // :1406
                }
            }

            // ================= dm :1414-1475 =================
            if op.dm != 0 {
                // :1416-1465 — lfo_slot==0 ⇒ not hoisted (:1418 gate twin)
                let slot = if (op.lfo as usize) < 0x18 {
                    lfo_slot[op.lfo as usize]
                } else {
                    0
                };
                emit_dm_src(a, o, op, slot, sintab, sintab_len);
            }
            // :1467-1475 — early/last-slot vs ring slot + the tail/branchy
            // mw_reg byte (OUTSIDE the dm gate — dm==0 CLEARS the byte)
            emit_dm_store(a, o, k, op, s3, an);

            // ================= dr :1480-1496 =================
            emit_dr_apply(a, o, k, op, s3, an); // rnd_skip FIRST :1481-1482

            // ================= memw value :1501-1509 =================
            emit_memw_acc(a, o, k, op, s2, an);

            // ================= index / index2 :1511-1525 =================
            emit_index_legs(a, o, k, op, s3, an);

            // ================= t :1530-1561 =================
            emit_t_leg(a, o, ms, k, op, s2, bake, an);

            // ================= memop :1566-1638 =================
            emit_memop(a, o, k, op, s2, revram_enable, an);
        } // :1639 !(branchy && o.jump)

        // ---- skipped/jump-tail path :1641-1678 (branchy only) ----
        if branchy {
            let normal_done = if op.jump != 0 {
                0 // :1642 — the jump op's jmp_fwd already routes here
            } else {
                a.jmp_fwd() // :1642 body hops over the erase tail
            };
            a.patch(skip_jump); // :1643
            if jump_done != 0 {
                a.patch(jump_done); // :1644-1645
            }
            emit_t_branchy(a, o, k, op, s2, s3, an); // :1647-1675
            if normal_done != 0 {
                a.patch(normal_done); // :1676-1677
            }
        }
    }

    // ---- epilogue :1688-1694 ----
    emit_epilogue(a, o.p, o.seed);
}

/// Test seam (B2b-3b brief §2): run build()'s DECISION path (RAM guard
/// :611-612 + analysis) and return the FULL assembled program bytes —
/// WITHOUT any `Code`, without the :1696-1712 handoff, without touching
/// `fnp` (which stays 0 everywhere, `b2b3b_build_inert`). It executes the
/// SAME `emit_program` build() assembles into its private Assembler, so the
/// parity rigs test the real bytes. Never called by the live path
/// (run()/rebuild() never route here).
pub fn program_bytes(
    ms: &MegState,
    ops: &[Op; 0x180],
    ram_len: usize,
    revram_enable: u16,
    bake: bool,
    early: bool,
    sintab: &[u16],
) -> Option<Vec<u8>> {
    if ram_len < 0x40000 {
        return None; // :611-612 (same refusal point as build())
    }
    let an = analyze_ops(ops, early);
    let sintab_ptr = if sintab.len() >= 0x8000 {
        sintab.as_ptr() // :645 resident
    } else {
        std::ptr::null() // :645 — callout fallback instead
    };
    let mut a = Assembler::new();
    emit_program(
        &mut a,
        &offs(),
        ms,
        ops,
        revram_enable,
        bake,
        &an,
        ms.delay_3, // :614 (build commits cd.d3 = ms.m_delay_3)
        ms.delay_2, // :615
        sintab_ptr,
        sintab.len(),
    );
    Some(a.code)
}
