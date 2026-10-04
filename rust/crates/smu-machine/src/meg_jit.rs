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
//! `last_slot_r/m`). `build()` STILL returns FALSE behind
//! `PHASE_B2_EMIT_OK = false` (op loop, LFO hoist and CHECK leg arrive in
//! B2b-2+); with `fnp == 0` no emitted byte is ever executed.
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
use std::sync::OnceLock;

use smu_swp30::meg::{MegState, MegSwp, Op};
use smu_swp30::mix::MegJitHook;

// Re-export for tests/meg_jit.rs (jit_emit stays a private crate module,
// lib.rs:86 — the brief's allowlist keeps lib.rs untouched). Same items are
// in scope here (a `pub use` is also an import).
pub use crate::jit_emit::{
    Assembler, Mem, NOREG, ARG0, ARG1, ARG2, R8, R9, R10, R11, R12, R13, R14, R15, RAX, RBP, RBX,
    RCX, RDI, RDX, RSI, RSP,
};

/// B2a inertness lever (brief): while false, `build()` never reaches the
/// buffer handoff, so no `fnp` is ever published and no emitted byte is
/// ever executed. Flip to true in B2b ONLY when the op loop + LFO hoist +
/// early-write analysis + offset table are all transliterated and the
/// CHECK leg is wired (handoff §8 gate sequence).
pub const PHASE_B2_EMIT_OK: bool = false;

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

        let _ = revram_enable; // :1589 per-region compile-time gate — B2b-2
        let _ = bake;          // :403-421 spec/baked variant — B2b-2

        // :730 assembler — the frame skeleton (prologue :826-837, epilogue
        // :1688-1694) with the REAL offsets; the seed legs deref the
        // MegSwpDev slot once (§7-B). tests/meg_jit.rs pins ret
        // termination + rsp alignment + exact prologue/epilogue bytes.
        // :839-840 branchy skip reset — B2b-2: Rust meg_skip_to is u16
        // (C++ u32, swp30.h:614) and jit_emit has store16i/cmp16 only as
        // reg forms; the width-adapted legs land with the branch gate.
        // :971-991 LFO hoist (emit_lfo :897-969) — B2b-2.
        // :995-1638 the per-op loop (ring apply :1000-1077, branch gate
        // :1079-1104, ALU :1107-1409, dm/dr :1414-1496, memw/index/t
        // :1501-1561, memop :1566-1638) consuming `an` + `o` — B2b-2.
        // DO NOT EMIT HALF A PROGRAM: the PHASE_B2_EMIT_OK guard below is
        // what keeps that true.
        let mut a = Assembler::new();
        emit_frame_skeleton(&mut a, &o, an.branchy);

        // THE single inertness gate (brief): unreachable until B2b flips the
        // const, so the handoff below — and fnp publication — never runs.
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
            Self::build(&mut self.gen, meg, &self.ops, ram_len, revram_enable, false, self.early);
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

        // :426-511 — CHECK A/B leg (SMU2000_MEG_JIT_CHECK, handoff §9:
        // snapshot meg_state/reverb_ram/seed/flags, JIT step, restore,
        // interpreter from the same start — `_UPTO` per-op step bisect
        // :444-448, field-wise compare per §7-E, 40-report cap :461-462).
        // PHASE B2 (needs the emission to compare against; the env polarity
        // helper `env_flag_on` + `meg_jit_upto` already landed and are
        // pinned in tests/meg_jit.rs).

        // :512 c->fn(m_meg, this, m_reverb_ram.data()) — PHASE B2-3
        // (transmute of the RWX buffer, jit.rs:380-384 pattern; ARG
        // laundering §7-B — ARG1 becomes a MegSwpDev::from_swp window
        // built from this very seam, B2b-1).
        let _ = use_spec;

        // :513-514 — the caller-side half of run_program: pc wrap + icount
        // tail (mirrors meg.rs:1000-1002; run_program subtracts the 0x180
        // ITSELF on every path, CHECK comment :451-453).
        meg.pc = 0;
        meg.icount = meg.icount.wrapping_sub(0x180);
        true // :515
    }
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
// B2a — frame: prologue/epilogue skeleton (origin: swp30_jit.cpp:826-840
// + :1688-1694, x64 arm; the x86-32 halves :737-812/:1682-1687 are DEAD).
// Handoff §3/§5.3-7/§5.3-10. tests/meg_jit.rs proves the emitted bytes
// terminate with `ret`, keep rsp balanced across entry/exit, and leave rsp
// 16-aligned at every frame-body callout position (S11 callee-save lesson:
// honor Win64 callee-saves exactly — jit.rs:397-400).
// ---------------------------------------------------------------------------

/// origin: swp30_jit.cpp:818 `FM` — a frame-slot operand (rsp-based). The
/// LFO hoist slots (LFO_SLOT_BASE+4n, :985) land in B2b through this.
#[allow(dead_code)] // first live readers arrive with the B2b LFO hoist
const fn fm(disp: i32) -> Mem {
    Mem { base: RSP, index: NOREG, scale: 1, disp }
}

/// origin: swp30_jit.cpp:820-822 `load_p_limits` — ±(2^38−1) saturation
/// limits. MANDATORY after every callout (R9/R10 are volatile on Win64;
/// :819 comment, :1448 after call_lfo).
pub fn load_p_limits(a: &mut Assembler) {
    a.imm64(P_MAX, 0x3fff_ffff_ffff);            // :821
    a.imm64(P_MIN, (-0x400_0000_0000i64) as u64); // :822 u64(s64(-0x4000000000))
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
/// (:607-610). Half-emitted op loops are never produced (brief §5; the
/// op loop :995-1638 and the branchy skip reset :839-840 arrive in
/// B2b-2 — the skip legs must be 16-bit-wide for the Rust u16
/// `meg_skip_to`, and jit_emit gains `store16i` additively there).
/// Nothing here is ever executed: build() still returns false at the
/// PHASE_B2_EMIT_OK gate, so no skeleton byte is copied into an exec
/// buffer (tests/meg_jit.rs `b2b1_build_inert` pins fnp==0).
pub fn emit_frame_skeleton(a: &mut Assembler, o: &Offs, branchy: bool) {
    emit_prologue(a, o.p, o.sample, o.seed);
    // :839-840 `if (branchy) store32i mem{SWP, o_skip}, 0` — B2b-2
    // (width-adapted store16i through the window slot; INERT for
    // non-branchy programs either way).
    let _ = branchy;
    emit_epilogue(a, o.p, o.seed);
}
