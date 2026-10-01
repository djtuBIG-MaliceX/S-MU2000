//! Device-layer vectors for `smu_sh2::device` — every expectation hand-derived
//! from the C++ ON DISK (`src/mame/cpu/sh2.cpp` / `sh2.h` / `mamecompat.h`), with
//! the origin line cited per test. The fake bus records the exact bus-traffic
//! sequence so reset order, exception push/fetch order, and deferral semantics are
//! proven, not assumed.
//!
//! NOTE (session F2): masked-vs-entered hinges on `sh2.cpp:354`
//! (`irqline <= IPL` → return). Post-reset SR = 0xF0 (IPL 15) masks IRQ0..15;
//! every entering vector below first lowers IPL. The internal-vector path
//! (`:358/:360`) is ONLY reachable with `internal_irq_level >= 0` and NO external
//! bit at that level (external tie wins the pick at `:167-170`), so it is driven
//! through `check_pending_irq` directly, as the peripheral rows will.

use smu_sh2::core::{InstructionHook, Sh2Bus, Sh2Core, SH_I};
use smu_sh2::device::{
    Sh2Device, ASSERT_LINE, CLEAR_LINE, INPUT_LINE_NMI, USE_JIT,
};

const AM: u32 = 0x1F_FFFF;

struct RecBus {
    mem: Vec<u8>,
    reads: Vec<(u32, u32)>,  // (address-as-seen, width-bits)
    writes: Vec<(u32, u32)>,
}

impl RecBus {
    fn new() -> Self {
        RecBus { mem: vec![0; 0x40_0000], reads: vec![], writes: vec![] }
    }
    fn idx(a: u32) -> usize { (a as usize) & 0x3F_FFFF }
    fn bs(&mut self, a: u32, v: u16) {
        let i = Self::idx(a);
        self.mem[i] = (v >> 8) as u8;
        self.mem[i + 1] = v as u8;
    }
    fn bl(&mut self, a: u32, v: u32) { self.bs(a, (v >> 16) as u16); self.bs(a + 2, v as u16) }
    fn hl(&self, a: u32) -> u16 {
        let i = Self::idx(a);
        ((self.mem[i] as u16) << 8) | self.mem[i + 1] as u16
    }
    fn wl(&self, a: u32) -> u32 { ((self.hl(a) as u32) << 16) | self.hl(a + 2) as u32 }
    fn code(&mut self, pc: u32, ops: &[u16]) {
        for (i, o) in ops.iter().enumerate() { self.bs(pc + 2 * i as u32, *o) }
    }
    fn clear_logs(&mut self) { self.reads.clear(); self.writes.clear() }
}

impl Sh2Bus for RecBus {
    fn read_byte(&mut self, o: u32) -> u8 { self.reads.push((o, 8)); self.mem[Self::idx(o)] }
    fn read_word(&mut self, o: u32) -> u16 { self.reads.push((o, 16)); self.hl(o) }
    fn read_long(&mut self, o: u32) -> u32 { self.reads.push((o, 32)); self.wl(o) }
    fn write_byte(&mut self, o: u32, v: u8) { self.writes.push((o, 8)); let i = Self::idx(o); self.mem[i] = v }
    fn write_word(&mut self, o: u32, v: u16) { self.writes.push((o, 16)); self.bs(o, v) }
    fn write_long(&mut self, o: u32, v: u32) { self.writes.push((o, 32)); self.bl(o, v) }
}

/// Hook recorder: pc at the `debugger_instruction_hook` site (sh2.cpp:268).
/// Hook-before-fetch ordering is proven via the reads log (recorded address vs
/// hook pc) in `scripted_loop_device_core_hook_order`.
struct HookLog {
    log: Vec<u32>,
}
impl HookLog {
    fn new() -> Self { HookLog { log: vec![] } }
}
impl InstructionHook for HookLog {
    fn instruction(&mut self, core: &Sh2Core) {
        self.log.push(core.pc);
    }
}

fn dev() -> (Sh2Device, RecBus) {
    (Sh2Device::new(28_000_000, 0, AM), RecBus::new())
}

// ---------------------------------------------------------------- ctor/start

#[test]
fn ctor_start_explicit_init() {
    // origin: sh2.cpp:29-36 (ctor: clock/cpu_type/am only) + sh2.cpp:42-61
    // (device_start: zero-fill folded into Sh2Core::new from
    // sh_common_execution::device_start sh.cpp:45-63; :59 zeroes m_nmi_line_state).
    let (d, _b) = dev();
    assert_eq!(d.clock, 28_000_000);   // sh2.cpp:33
    assert_eq!(d.cpu_type, 0);         // sh2.cpp:34
    assert_eq!(d.core.m_am, AM);       // sh2.cpp:35
    assert_eq!(d.m_cache_dirty, false);
    let c = &d.core;
    assert_eq!(c.r, [0u32; 16]);
    assert_eq!((c.pc, c.pr, c.sr, c.gbr, c.vbr, c.mach, c.macl), (0, 0, 0, 0, 0, 0, 0));
    assert_eq!(c.internal_irq_level, 0); // 0 here; -1 only after device_reset (:75)
    assert_eq!(c.m_nmi_line_state, 0);   // sh2.h:67 default, re-zeroed sh2.cpp:59
    assert_eq!(c.m_irq_line_state, [0i8; 17]); // sh2.h:91 {}/sh2.cpp:67
    assert_eq!(c.pending_irq, 0);
    assert_eq!(c.pending_nmi, 0);
}

// ---------------------------------------------------------------- reset

#[test]
fn reset_full_state_and_read_order() {
    // origin: sh2.cpp:63-84 — exact init order; bus must see EXACTLY
    // read_long(0) then read_long(4) (:77-78), nothing else.
    let (mut d, mut b) = dev();
    d.device_start(); // sh2.cpp:42-61
    b.bl(0, 0x0000_8100); // reset vector
    b.bl(4, 0x0010_0000); // initial SP
    d.device_reset(&mut b);
    assert_eq!(b.reads, vec![(0u32, 32u32), (4, 32)]); // :77-78 only, in order
    assert_eq!(b.writes, vec![]);
    let c = &d.core;
    assert_eq!(c.sr, SH_I);            // :76 (0xF0: IPL bits 7-4 = 15, T/S/Q/M=0)
    assert_eq!(c.pc, 0x0000_8100);     // :77
    assert_eq!(c.r[15], 0x0010_0000);  // :78
    for i in 0..15 {
        assert_eq!(c.r[i], 0);         // :66/:69
    }
    assert_eq!((c.pr, c.gbr, c.vbr, c.mach, c.macl), (0, 0, 0, 0, 0)); // :69
    assert_eq!((c.evec, c.irqsr), (0, 0)); // :70
    assert_eq!((c.ea, c.m_delay), (0, 0)); // :71
    assert_eq!((c.pending_irq, c.pending_nmi, c.sleep_mode), (0, 0, 0)); // :72-74
    assert_eq!(c.internal_irq_level, -1);  // :75
    assert_eq!((c.m_test_irq, c.m_cpu_off, c.m_internal_irq_vector), (0, 0, 0)); // :80-82
    assert_eq!(d.m_cache_dirty, true);     // :83
    assert_eq!(c.m_nmi_line_state, 0);
}

// ---------------------------------------------------------------- IRQ entry

fn armed(irq: i32, sr: u32) -> (Sh2Device, RecBus) {
    // post-reset machine with VBR/SP/pc placed and a known vector table
    let (mut d, mut b) = dev();
    d.device_start();
    d.device_reset(&mut b);
    {
        let c = &mut d.core;
        c.vbr = 0x2000;      // :397 fetch base
        c.pc = 0x0000_1236;
        c.r[15] = 0x1000;    // stack top
        c.sr = sr;           // caller-controlled mask state
    }
    b.bl(0x2000 + (64 + irq / 2) as u32 * 4, 0x280); // autovector handler
    b.bl(0x2000 + 44, 0x21C);                         // NMI vector 11
    b.bl(0x2000 + 16, 0x1234_5678);                   // ILLEGAL vector 4
    b.clear_logs();
    (d, b)
}

#[test]
fn irq_entry_bus_sequence_exact() {
    // origin: sh2.cpp:297-345 (assert → :342 check_pending_irq) → :154-178
    // (external irq pick) → :347-379 (mask pass :354, autovector 64+irq/2 :368) →
    // :381-401 (push SR @0xFFC? NO — :385 decrements to 0xFFC first? see run:
    // ground truth = push order SR then PC, first slot = 0x1000-4? The C++ on
    // disk (:385-388) decrements BEFORE each write: 0xFFC first. The core
    // mirrors it; verified against core-test push convention core.rs:157).
    let (mut d, mut b) = armed(10, 0x0000_0010); // IPL 1 → irq10 passes :354
    d.core.sleep_mode = 1;
    d.execute_set_input(&mut b, 10, ASSERT_LINE);
    assert_eq!(b.writes, vec![(0xFFCu32, 32u32), (0xFF8u32, 32u32)]); // :385-388
    assert_eq!(b.wl(0xFFC), 0x10);       // SR pushed — :386
    assert_eq!(b.wl(0xFF8), 0x1236);     // PC pushed — :388
    assert_eq!(d.core.r[15], 0x0FF8);    // two pushes — :385,:387
    assert_eq!(b.reads, vec![(0x2114u32, 32u32)]); // vbr+69*4, single fetch — :397
    assert_eq!(d.core.pc, 0x280 & AM);   // :397 (& m_am)
    assert_eq!(d.core.sr, 0xA0);         // :394 — (0x10 & ~0xF0) | (10<<4)
    assert_eq!(d.core.sleep_mode, 2);    // :399-400
    assert_eq!(d.core.pending_irq, 1 << 10); // level stays asserted (:337 set, no clear)
    assert_eq!(d.core.m_irq_line_state[10], ASSERT_LINE); // :327
}

#[test]
fn masked_irq_noop_returns_early() {
    // origin: sh2.cpp:354 — `irqline <= ((sr>>4)&15)` → return before ANY bus
    // touch. Post-reset SR=0xF0 (IPL 15) masks IRQ2; the line still latches
    // (:327/:337) so it will fire when a later LDC SR un-masks (:192-196).
    let (mut d, mut b) = armed(2, SH_I); // SR = 0xF0 exactly
    d.execute_set_input(&mut b, 2, ASSERT_LINE);
    assert!(b.reads.is_empty() && b.writes.is_empty()); // :354 early return
    assert_eq!(d.core.pending_irq, 1 << 2);             // :337 still recorded
    assert_eq!(d.core.m_irq_line_state[2], ASSERT_LINE); // :327
    assert_eq!(d.core.pc, 0x1236);                      // unchanged
    assert_eq!(d.core.sr, SH_I);                        // untouched
}

#[test]
fn unmask_then_check_enters_autovector() {
    // origin: the run-loop gate (:284-288) is what calls check_pending_irq
    // (:286) after LDC SR set m_test_irq (:195). Direct call stands in for the
    // gate here (the scripted loop test covers the gate itself).
    let (mut d, mut b) = armed(2, SH_I);
    b.bl(0x2104, 0x1C8); // autovector 65 handler
    b.clear_logs();
    assert_eq!(b.wl(0x2104), 0x1C8);
    d.execute_set_input(&mut b, 2, ASSERT_LINE); // masked latch
    d.core.sr = 0x0000_0010; // LDC SR side effect (:194)
    d.core.m_test_irq = 1;   // :195
    d.core.check_pending_irq(&mut b); // the gate's call (:286)
    assert_eq!(b.writes, vec![(0xFFCu32, 32u32), (0xFF8u32, 32u32)]);
    assert_eq!(d.core.pc, 0x1C8);          // vector 64+2/2=65 → 0x2000+0x104
    assert_eq!(d.core.sr, 0x20);           // IPL ← 2 (:394)
    let _ = CLEAR_LINE;
}

#[test]
fn irq_retrigger_same_level_noop() {
    // origin: sh2.cpp:324-325 — second assert at the SAME level bails before
    // touching pending/check; only a level CHANGE acts.
    let (mut d, mut b) = armed(10, 0x0000_0010);
    d.execute_set_input(&mut b, 10, ASSERT_LINE);
    let n = b.writes.len();
    assert_eq!(n, 2); // entered once
    d.execute_set_input(&mut b, 10, ASSERT_LINE); // :324 bail
    assert_eq!(b.writes.len(), n);
    assert_eq!(b.reads.len(), 1); // no second vector fetch
}

#[test]
fn irq_line_state_set_and_clear() {
    // origin: sh2.cpp:324-343 — unchanged level bails (:324); set ORs the bit
    // (:337), clear AND-NOTs it (:332) and fires no exception on clear (:329-332).
    let (mut d, mut b) = armed(3, SH_I); // mask everything
    d.execute_set_input(&mut b, 3, ASSERT_LINE);
    assert_eq!(d.core.pending_irq, 1 << 3);
    d.execute_set_input(&mut b, 5, ASSERT_LINE);
    assert_eq!(d.core.pending_irq, (1 << 3) | (1 << 5));
    assert_eq!(d.core.m_irq_line_state[3], ASSERT_LINE);
    d.execute_set_input(&mut b, 5, CLEAR_LINE); // :332
    assert_eq!(d.core.pending_irq, 1 << 3);
    assert_eq!(d.core.m_irq_line_state[5], CLEAR_LINE);
    d.execute_set_input(&mut b, 3, CLEAR_LINE);
    assert_eq!(d.core.pending_irq, 0);
    assert!(b.reads.is_empty() && b.writes.is_empty()); // never entered
}

// ---------------------------------------------------------------- NMI

#[test]
fn nmi_entry_ignores_mask_vector11() {
    // origin: sh2.cpp:314 assert → :319 check → :156-159 NMI branch →
    // :352/372-375 vector=11 (mask check :354 SKIPPED for irqline==16) →
    // :391-392 `irqline > SH2_INT_15` → sr |= SH_I (all masked), :397 pc fetch.
    let (mut d, mut b) = armed(0, SH_I);
    d.execute_set_input(&mut b, INPUT_LINE_NMI, ASSERT_LINE);
    assert_eq!(b.writes, vec![(0xFFCu32, 32u32), (0xFF8u32, 32u32)]); // :385-388
    assert_eq!(b.wl(0xFFC), SH_I);       // SR pushed (0xF0) — :386
    assert_eq!(b.wl(0xFF8), 0x1236);     // PC pushed — :388
    assert_eq!(b.reads, vec![(0x202Cu32, 32u32)]); // vbr + 11*4 — :397
    assert_eq!(d.core.pc, 0x21C & AM);
    assert_eq!(d.core.sr, 0xF0);        // :392 — SH_I set wholesale, NOT (16<<4)
    assert_eq!(d.core.pending_nmi, 0);   // :159 cleared after entry
    assert_eq!(d.core.m_nmi_line_state, ASSERT_LINE); // :304
}

#[test]
fn nmi_edge_only_retrigger() {
    // origin: sh2.cpp:299-321 — NMI is edge-triggered (sh2.h:52): a second assert
    // at the same level bails (:301-302); only CLEAR_LINE (:306) re-arms.
    let (mut d, mut b) = armed(0, SH_I);
    d.execute_set_input(&mut b, INPUT_LINE_NMI, ASSERT_LINE);
    assert_eq!(d.core.pc, 0x21C & AM); // entered
    let n = b.reads.len();
    d.execute_set_input(&mut b, INPUT_LINE_NMI, ASSERT_LINE); // :301 bail
    assert_eq!(b.reads.len(), n);
    assert_eq!(d.core.pending_nmi, 0); // no re-arm while line held
    d.execute_set_input(&mut b, INPUT_LINE_NMI, CLEAR_LINE); // :306-309: no action
    assert_eq!(b.reads.len(), n);
    d.core.pc = 0x0000_2000; // simulate handler return point
    d.execute_set_input(&mut b, INPUT_LINE_NMI, ASSERT_LINE); // second edge
    assert_eq!(d.core.pending_nmi, 0); // :159 — entered immediately (m_delay==0)
    assert_eq!(d.core.pc, 0x21C & AM);
    assert_eq!(b.reads.len(), n + 1);
}

// ---------------------------------------------------------------- delay-slot deferral

#[test]
fn irq_assert_deferred_by_delay_slot() {
    // origin: sh2.cpp:339-340 (m_delay armed → ONLY m_test_irq=1, no check) and
    // the run-loop gate :284-288 (`m_test_irq && !m_delay` → check at :286,
    // clear :287). NOTE the gate clears m_test_irq even when nothing is pending,
    // so the assert must land AFTER the loop would have consumed the LDC flag —
    // i.e. the peripheral row raises lines mid-slot, not mid-loop.
    // Script: 0:NOP, 2:BRA +1 (arms m_delay=0xA = pc(4)+2*1+2 — the core's bra
    // encodes target=pc+2*disp+2, disk sh.cpp bra / core test 0xA001 pattern),
    // 4:NOP(slot), pad 6/8/A; SR lowered by hand to IPL 1 (== a completed
    // LDC SR, :194). Entry fires INSIDE the slot step: fetch@4, pc:=0xA,
    // m_delay=0, then the :284 gate opens and :286 enters, PC pushed = 0xA.
    let (mut d, mut b) = dev();
    d.device_start();
    d.device_reset(&mut b);
    d.core.vbr = 0x2000;
    d.core.r[15] = 0x1000; // stack for the entry pushes (:385-388)
    b.code(0, &[0x0009, 0xA001, 0x0009, 0x0009, 0x0009]);
    b.bl(0x2000 + 69 * 4, 0x280);
    b.clear_logs();
    let mut hk = HookLog::new();
    d.core.run_one(&mut b, &mut hk);        // NOP, pc 0→2
    d.core.run_one(&mut b, &mut hk);        // BRA +1 → m_delay=0xA (sh.cpp bra)
    assert_eq!(d.core.m_delay, 0x8);
    d.core.sr = 0x0000_0010;                // as-if LDC R1,SR (:194), flag consumed
    d.execute_set_input(&mut b, 10, ASSERT_LINE); // IRQ lands inside the slot
    assert_eq!(d.core.pc, 4);               // NO entry now - deferred (:339-340)
    assert!(b.writes.is_empty());
    assert_eq!(d.core.m_test_irq, 1);       // :340
    d.core.run_one(&mut b, &mut hk);        // slot NOP: fetch@4, pc:=m_delay=0xA,
    //                                        m_delay=0 → gate :284 opens → :286
    assert_eq!(d.core.pc, 0x280 & AM);
    assert_eq!(b.writes, vec![(0xFFCu32, 32u32), (0xFF8u32, 32u32)]);
    assert_eq!(b.wl(0xFF8), 0x8);           // PC pushed == post-slot pc (:388)
    assert_eq!(d.core.sr, 0xA0);            // IPL←10 (:394)
    assert_eq!(d.core.m_test_irq, 0);       // :287
    // hook order: hooks fire BEFORE each fetch (sh2.cpp:268 vs :272)
    assert_eq!(hk.log, vec![0u32, 2, 4]);
}

// ---------------------------------------------------------------- internal irq

#[test]
fn internal_irq_uses_device_vector() {
    // origin: sh2.cpp:357-363 - internal_irq_level == irqline -> vector =
    // m_internal_irq_vector (sh2.h:66), :362 internal_irq_level := -1. NOTE the
    // vector is scaled like every other fetch: :397 `vbr + vector*4`
    // (disk-verified) -> 0x2000 + 0x300*4 = 0x2C00. Driven via check_pending_irq
    // (the intc/sci peripheral path), no pending_irq bit set.
    let (mut d, mut b) = armed(2, 0x0000_0010); // IPL 1 - irq2 passes :354
    d.core.internal_irq_level = 2;
    d.core.m_internal_irq_vector = 0x300;
    b.bl(0x2C00, 0x4000_0100); // vbr + 0x300*4 = 0x2C00
    b.clear_logs();
    d.core.check_pending_irq(&mut b); // internal-only pick (:168)
    assert_eq!(b.reads, vec![(0x2C00u32, 32u32)]); // vbr + 0x300*4 = 0x2C00 - :397
    assert_eq!(d.core.pc, 0x0000_0100);            // & m_am (:397)
    assert_eq!(d.core.internal_irq_level, -1);     // :362
    assert_eq!(d.core.sr, 0x20);                   // :394
}

#[test]
fn external_at_internal_level_redirects_to_internal_vector() {
    // origin: sh2.cpp:166-170 - with an external bit AT the internal level,
    // external_irq(2) >= irq(2) wins the PICK, but sh2_exception :358 only
    // compares `internal_irq_level == irqline` - the pick's origin is invisible
    // there - so the INTERNAL vector is taken anyway and :362 clears the level.
    // Disk quirk kept verbatim (canonical MAME behaves identically).
    let (mut d, mut b) = armed(2, 0x0000_0010);
    d.core.internal_irq_level = 2;
    d.core.m_internal_irq_vector = 0x300;
    b.bl(0x2000 + 65 * 4, 0x150);  // autovector 65 - written but must NOT be used
    b.bl(0x2C00, 0x4000_0100); // vbr + 0x300*4 = 0x2C00
    b.clear_logs();
    d.execute_set_input(&mut b, 2, ASSERT_LINE); // sets pending bit 2 (:337)
    assert_eq!(b.reads, vec![(0x2C00u32, 32u32)]); // internal vector wins (:360)
    assert_eq!(d.core.pc, 0x100);
    assert_eq!(d.core.internal_irq_level, -1); // cleared by the :358 branch (:362)
}
// ---------------------------------------------------------------- bus helpers

#[test]
fn bus_helper_widths_and_mask_boundary() {
    // origin: sh2.cpp:89-152 — below 0x4000_0000 AND with m_am, at/above pass
    // through; device methods DELEGATE to core (no double mask).
    let (mut d, mut b) = dev();
    d.write_byte(&mut b, 0x3FFF_FFFF, 0xAB); // masked → bus sees 0x1F_FFFF
    assert_eq!(b.writes.last(), Some(&(0x1F_FFFFu32, 8u32)));
    d.write_word(&mut b, 0x4000_0000, 0x1234); // unmasked
    assert_eq!(b.writes.last(), Some(&(0x4000_0000u32, 16u32)));
    d.write_long(&mut b, 0x800, 0xDEAD_BEEF);
    assert_eq!(b.writes.last(), Some(&(0x800u32, 32u32)));
    assert_eq!(d.read_byte(&mut b, 0x3FFF_FFFF), 0xAB);
    assert_eq!(d.read_word(&mut b, 0x4000_0000), 0x1234);
    assert_eq!(d.read_long(&mut b, 0x800), 0xDEAD_BEEF);
    // origin: sh2.h:73 / sh2.cpp:118 — decrypted fetch is straight passthrough
    b.bs(0x3FFF_FFFE, 0xBEEF);
    assert_eq!(d.decrypted_read_word(&mut b, 0x3FFF_FFFE), 0xBEEF); // NOT masked
    assert_eq!(*b.reads.last().unwrap(), (0x3FFF_FFFE, 16));
}

// ---------------------------------------------------------------- scripted loop

#[test]
fn scripted_loop_device_core_hook_order() {
    // Proves device→core delegation end-to-end + hook-before-fetch-before-execute
    // (sh2.cpp:268 → :272 → :282 → :284-288 → :289). MOVI+LDC R1,SR lower IPL to
    // 1 (REG_N form, core test core.rs:1216-1217); IRQ10 asserted after the LDC
    // step → immediate entry (:342, m_delay==0); execute_run with icount==0 runs
    // exactly ONE instruction (do/while :262-290).
    let (mut d, mut b) = dev();
    d.device_start();
    d.device_reset(&mut b);
    d.core.vbr = 0x2000;
    d.core.r[15] = 0x3000; // stack above code/vectors: pushes @0x2FFC/0x2FF8
    b.code(0, &[0xE110, 0x410E, 0x0009]); // MOVI #0x10,R1; LDC R1,SR; NOP
    b.code(0x280, &[0x0009, 0x0009]);      // handler landing pad (NOPs)
    b.bl(0x2000 + 69 * 4, 0x280);
    b.clear_logs();
    let mut hk = HookLog::new();
    d.core.run_one(&mut b, &mut hk); // MOVI: hook@0, fetch@0 (R1=0x10)
    assert_eq!(d.core.r[1], 0x10);
    d.core.run_one(&mut b, &mut hk); // LDC R1,SR: sr=0x10, flag set+cleared (:287)
    assert_eq!(d.core.sr, 0x10);
    d.execute_set_input(&mut b, 10, ASSERT_LINE); // immediate entry (m_delay==0)
    assert_eq!(d.core.pc, 0x280);
    assert_eq!(b.writes, vec![(0x2FFCu32, 32u32), (0x2FF8u32, 32u32)]);
    assert_eq!(b.wl(0x2FFC), 0x10); // the un-masked SR is what gets pushed (:386)
    let nreads = b.reads.len(); // includes the just-done vector fetch
    d.execute_run(&mut b, &mut hk); // icount==0 → do/while runs ONE instruction
    assert_eq!(d.core.icount, -3);  // icount -2 at call (one -1 per run_one), execute_run's single step (:289)
    assert_eq!(hk.log, vec![0u32, 2, 0x280]); // entry (:342/:397) fires no hook
    assert_eq!(b.reads[2], (0x2114, 32)); // vector fetch at entry (:397), no step in between
    assert_eq!(b.reads.len(), nreads + 1); // then exactly one opcode fetch
    assert_eq!(b.reads[nreads], (0x280, 16)); // opcode fetch AFTER the hook (:272)
    assert_eq!(d.clock, 28_000_000);
}

// ---------------------------------------------------------------- jit-off / misc

#[test]
fn cpu_off_bail() {
    // origin: sh2.cpp:254-259 — m_cpu_off: bail via debugger_wait_hook (no-op,
    // mamecompat.h:81), icount := 0, no fetch, no hook.
    let (mut d, mut b) = dev();
    d.device_start();
    d.device_reset(&mut b);
    b.code(0, &[0x0009]);
    b.clear_logs();
    d.core.m_cpu_off = 1;
    let mut hk = HookLog::new();
    d.execute_run(&mut b, &mut hk);
    assert!(b.reads.is_empty() && b.writes.is_empty());
    assert!(hk.log.is_empty());
    assert_eq!(d.core.icount, 0);
    assert_eq!(d.core.pc, 0);
}

#[test]
fn jit_off_constants_and_trivial_devcb() {
    // origin: row decision const use_jit=false (sh2.cpp:261); jit_flush no-op
    // (sh2.h:108); min/max cycles 1/4, default vector 0 (sh2.h:49-51);
    // edge-triggered NMI only (sh2.h:52); set_frt_input no-op (sh2.h:34).
    assert!(!USE_JIT);
    assert!(!Sh2Device::jit_enabled());
    assert!(!Sh2Device::jit_trace_on());
    Sh2Device::jit_flush(); // no-op — must not panic / mutate
    let (mut d, _b) = dev();
    Sh2Device::jit_flush();
    assert!(!d.m_cache_dirty); // plain no-op; device_reset is what sets it (:83)
    assert_eq!(Sh2Device::execute_min_cycles(), 1);
    assert_eq!(Sh2Device::execute_max_cycles(), 4);
    assert_eq!(Sh2Device::execute_default_irq_vector(3), 0);
    assert!(Sh2Device::execute_input_edge_triggered(INPUT_LINE_NMI));
    assert!(!Sh2Device::execute_input_edge_triggered(0));
    d.set_frt_input(1); // sh2.h:34 — empty
    assert_eq!(d.clock, 28_000_000);
}

#[test]
fn sleep_mode_untouched_without_entry() {
    // origin: sh2.cpp:329-332 / :306-309 — CLEAR_LINE changes only the line
    // state / pending bit; sleep_mode is touched ONLY at :399-400 (entry).
    let (mut d, mut b) = armed(4, SH_I);
    d.core.sleep_mode = 1;
    d.execute_set_input(&mut b, 4, ASSERT_LINE);  // masked → no entry
    d.execute_set_input(&mut b, 4, CLEAR_LINE);   // :332
    d.execute_set_input(&mut b, INPUT_LINE_NMI, CLEAR_LINE); // :306-309, never set
    assert_eq!(d.core.sleep_mode, 1);
    assert_eq!(d.core.pending_irq, 0);
    assert_eq!(d.core.pending_nmi, 0);
    assert!(b.reads.is_empty() && b.writes.is_empty());
}
