//! Origin-cited unit vectors for `periph/intc.rs` (ledger row `periph: intc`,
//! M2). Every expectation cites its disk line in `src/mame/cpu/sh_intc.cpp`
//! (+ `sh7042.cpp`/`sh7042_map.hxx` for the register decode and CPU handoff).
//! Synthetic vectors only; no ROM/WAV data.

use smu_sh2::core::SH_I;
use smu_sh2::periph::intc::{Sh2Intc, PRIBIT};
use smu_sh2::sh7042::Sh7042;

// ---------------------------------------------------------------------------
// helpers: vector -> (ipr index, nibble shift) per the disk arbiter
// (sh_intc.cpp:83-85: slot = pribit[v]; reg = slot>>2; shift = 12-4*(slot&3)).
// ---------------------------------------------------------------------------
fn slot_of(v: usize) -> (usize, u32) {
    let slot = PRIBIT[v] as u32;
    ((slot >> 2) as usize, 12 - 4 * (slot & 3))
}

// set an absolute priority `lvl` (0..15) for vector `v`, leaving other nibbles.
fn set_prio(intc: &mut Sh2Intc, v: usize, lvl: u16) -> (i32, u32) {
    let (reg, sh) = slot_of(v);
    let cur = intc.m_ipr[reg];
    let nv = (cur & !(0xF << sh)) | (lvl << sh);
    intc.ipr_w(reg, nv, 0xFFFF) // sh_intc.cpp:155-165 (re-arbitrates)
}

// ---------------------------------------------------------------------------
// reset / state
// ---------------------------------------------------------------------------

#[test]
fn reset_state_all_zero() {
    // origin: device_start sh_intc.cpp:51-54 (ipr/isr/icr/lines = 0) +
    // device_reset :59 (pending = 0); IRQ level idle mirrors the CPU
    // internal_irq_level = -1 (core.rs:271).
    let c = Sh2Intc::new();
    assert_eq!(c.m_ipr, [0u16; 8]); // :51
    assert_eq!(c.m_icr, 0); // :53
    assert_eq!(c.m_isr, 0); // :52
    assert_eq!(c.m_lines, 0); // :54
    assert_eq!(c.m_pending, [0u32; 8]); // :59
    assert_eq!(c.irq_level, -1);
    assert_eq!(c.irq_vector, 0);
    for v in 0..256 {
        assert!(!c.pending(v));
    }
}

#[test]
fn device_reset_clears_pending_only() {
    // origin: device_reset sh_intc.cpp:57-60 clears only m_pending; ipr/icr/
    // isr/lines survive (no reset write to them on disk).
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 130, 5);
    c.icr_w(0x00FF, 0x00FF); // :133
    c.internal_interrupt(130); // :95
    assert!(c.pending(130));
    c.device_reset(); // :59
    assert_eq!(c.m_pending, [0u32; 8]); // cleared
    assert_eq!(c.m_ipr[5] >> 4 & 15, 5); // IPR kept
    assert_eq!(c.m_icr, 0x00FF); // ICR kept
    assert_eq!(c.m_lines, 0); // lines kept
}

#[test]
fn pribit_table_spot_checks() {
    // origin: sh_intc.cpp:18-35 literal table.
    assert_eq!(PRIBIT[64], 0); // ext IRQ0
    assert_eq!(PRIBIT[65], 1); // ext IRQ1
    assert_eq!(PRIBIT[66], 2);
    assert_eq!(PRIBIT[67], 3);
    assert_eq!(PRIBIT[71], 7); // ext IRQ7 :23
    assert_eq!(PRIBIT[88], 12); // mtu0 :24
    assert_eq!(PRIBIT[92], 13); // mtu0 TOVF :24
    assert_eq!(PRIBIT[96], 14); // mtu1 :25
    assert_eq!(PRIBIT[112], 18); // mtu3 :26
    assert_eq!(PRIBIT[120], 20); // mtu4 :26
    assert_eq!(PRIBIT[128], 22); // sci SCI0 ERRI/RXI :27
    assert_eq!(PRIBIT[135], 23); // sci1 TECI :27
    assert_eq!(PRIBIT[159], 29); // :28
    assert_eq!(PRIBIT[160], 29); // :29 saturate
}

// ---------------------------------------------------------------------------
// pending latch (internal_interrupt, sh_intc.cpp:95-99)
// ---------------------------------------------------------------------------

#[test]
fn internal_interrupt_sets_exact_pending_bit() {
    // origin: sh_intc.cpp:97 `m_pending[vector>>5] |= 1<<(vector&31)`.
    let mut c = Sh2Intc::new();
    c.internal_interrupt(130);
    assert!(c.pending(130));
    // exact slot check, index 130>>5 = 4, bit 130&31 = 2
    assert_eq!(c.m_pending[4], 1u32 << 2);
    assert_eq!(c.m_pending[2], 0);
    assert_eq!(c.m_pending[3], 0);
}

#[test]
fn internal_interrupt_is_idempotent() {
    // origin: sh_intc.cpp:97 is `|=`, so a second raise changes nothing.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 88, 6);
    let (_, v1) = c.internal_interrupt(88);
    let (l2, v2) = c.internal_interrupt(88);
    assert_eq!(v1, 88);
    assert_eq!((l2, v2), (6, 88));
    assert_eq!(c.m_pending[88 >> 5], 1u32 << (88 & 31));
    assert_eq!(
        c.set_source(88, true),
        (6, 88),
        "set_source(assert) == internal_interrupt"
    );
}

#[test]
fn set_source_false_is_noop_not_clear() {
    // deviation note: disk internal_interrupt has no "lower"; deassert goes via
    // set_input/interrupt_taken. So assert=false must NOT clear pending.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 130, 9);
    c.internal_interrupt(130);
    let (l, v) = c.set_source(130, false);
    assert_eq!((l, v), (9, 130));
    assert!(c.pending(130)); // still latched
}

// ---------------------------------------------------------------------------
// arbitration (update_irq, sh_intc.cpp:71-93)
// ---------------------------------------------------------------------------

#[test]
fn arbitration_picks_higher_level() {
    // origin: sh_intc.cpp:86 `if (level > best_level)`.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 65, 2); // ipr[0] nibble @shift8
    set_prio(&mut c, 66, 9); // ipr[0] nibble @shift4
    c.internal_interrupt(65);
    let (l, v) = c.internal_interrupt(66);
    assert_eq!((l, v), (9, 66));
    // raise order reversed: still the higher LEVEL wins, not the last-raised.
    let mut d = Sh2Intc::new();
    set_prio(&mut d, 65, 2);
    set_prio(&mut d, 66, 9);
    d.internal_interrupt(66);
    let (l2, v2) = d.internal_interrupt(65);
    assert_eq!((l2, v2), (9, 66));
}

#[test]
fn arbitration_tie_lowest_vector_wins() {
    // SCAN-ORDER QUIRK: iv ascends (:79) and the pick is STRICT `>` (:86), so
    // the FIRST (lowest-numbered) vector at the winning level keeps it. NOT the
    // highest — that scaffold assumption is wrong per disk.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 88, 3); // ipr[3] @shift12
    set_prio(&mut c, 92, 3); // ipr[3] @shift8
    c.internal_interrupt(92); // raise high first...
    let (l, v) = c.internal_interrupt(88); // ...then low
    assert_eq!((l, v), (3, 88)); // LOW vector wins the tie regardless
    // and the reverse latch order agrees (already lowest first)
    let mut d = Sh2Intc::new();
    set_prio(&mut d, 88, 3);
    set_prio(&mut d, 92, 3);
    d.internal_interrupt(88);
    assert_eq!(d.internal_interrupt(92), (3, 88));
}

#[test]
fn arbitration_three_pending_correct_winner() {
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 100, 4); // mtu1  slot15 ipr[3]@shift0
    set_prio(&mut c, 104, 11); // mtu2  slot16 ipr[4]@shift12
    set_prio(&mut c, 130, 7); // sci   slot22 ipr[5]@shift4
    c.internal_interrupt(100);
    c.internal_interrupt(130);
    let (l, v) = c.internal_interrupt(104);
    assert_eq!((l, v), (11, 104));
    assert_eq!(c.irq_level, 11);
}

#[test]
fn arbitration_level_zero_wins_idle_but_never_vectors() {
    // origin: best_level = -1 (:73) so a level-0 source "wins" the arbiter, but
    // the CPU compares it to SR.IMASK and `irqline <= mask` returns (core.rs:
    // 1889) — priority 0 is effectively disabled (SH7043). Mask is always >= 0.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 96, 0); // mtu1 priority 0
    let (l, v) = c.internal_interrupt(96);
    assert_eq!((l, v), (0, 96)); // arbiter picks it
    assert_eq!(c.effective_vector(0), -1); // but it never vectors vs mask 0
    assert_eq!(c.effective_vector(-1), 96); // only an impossible -1 mask would let it
}

#[test]
fn out_of_range_vector_latched_never_arbitrated() {
    // origin: the scan is `for (bv = 64/32; bv != 160/32)` (:76) -> vectors
    // 0-63 and 160-255 are latched by :97 but never selected. NMI (vector 11)
    // is a CORE path (core.rs:1864 pending_nmi), not an INTC source, so this
    // also documents that feeding NMI into the INTC would be ignored.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 11, 15); // (NMI vector, meaningless to INTC)
    let (l, v) = c.internal_interrupt(11);
    assert!(c.pending(11)); // :97 latched bit0 of pending[0]
    assert_eq!((l, v), (-1, 0)); // :76 never scanned
    // and a >=160 vector too:
    let mut d = Sh2Intc::new();
    let (l2, v2) = d.internal_interrupt(200);
    assert!(d.pending(200));
    assert_eq!((l2, v2), (-1, 0));
}

// ---------------------------------------------------------------------------
// mask boundary (effective_vector mirrors core.rs:1889 / sh2_exception)
// ---------------------------------------------------------------------------

#[test]
fn mask_boundary_equal_does_not_fire() {
    // origin: sh2_exception core.rs:1889 `if (irqline <= mask) return;` — equal
    // mask => suppressed.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 132, 5);
    c.internal_interrupt(132);
    assert_eq!(c.effective_vector(5), -1); // == mask -> no fire
}

#[test]
fn mask_boundary_greater_fires() {
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 132, 5);
    c.internal_interrupt(132);
    assert_eq!(c.effective_vector(4), 132); // > mask -> fire (actual vector)
    assert_eq!(c.effective_vector(0), 132);
}

// ---------------------------------------------------------------------------
// ack / interrupt_taken (sh_intc.cpp:62-69)
// ---------------------------------------------------------------------------

#[test]
fn ack_clears_and_rearbitrates() {
    // origin: interrupt_taken :66 clears the bit, :68 re-arbitrates. Internal
    // (>= 72) sources always clear (:65 `vector >= 72`).
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 88, 9); // high
    set_prio(&mut c, 90, 3); // low
    c.internal_interrupt(88);
    c.internal_interrupt(90);
    assert_eq!(c.irq_vector, 88);
    let (l, v) = c.ack(88); // :62-69 (irqline arg unused -> use vector)
    assert!(!c.pending(88));
    assert!(c.pending(90));
    assert_eq!((l, v), (3, 90)); // now the survivor wins
}

#[test]
fn ack_clears_only_named_vector() {
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 104, 5);
    set_prio(&mut c, 105, 4);
    c.internal_interrupt(104);
    c.internal_interrupt(105);
    c.ack(104);
    assert!(!c.pending(104));
    assert!(c.pending(105));
    assert_eq!(c.irq_vector, 105);
}

#[test]
fn interrupt_taken_ignores_irqline_arg() {
    // origin: sh_intc.cpp:62 signature takes irqline but the body reads ONLY
    // `vector` (:65-66). Passing a wildly different irqline must still clear the
    // named vector.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 128, 6);
    c.internal_interrupt(128);
    c.interrupt_taken(3 /* unused */, 128);
    assert!(!c.pending(128));
}

#[test]
fn interrupt_taken_keeps_level_external_still_asserted() {
    // origin: sh_intc.cpp:65 — for a LEVEL external IRQ (vector 64-71, ICR bit
    // = 0) whose line is still high, the pending bit is NOT cleared (so it
    // re-fires). ICR bit for `vector` = BIT(m_icr, 7-(vector&7)) (:65); for
    // vector 64 that is bit 7, left 0 (level).
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 64, 8);
    // icr default 0 => IRQ0 is level mode (:114).
    c.set_input(0, 1); // line asserted, level mode latches pending (64)
    assert!(c.pending(64));
    let (l, v) = c.interrupt_taken(64, 64); // take it...
    assert!(c.pending(64)); // ...but line is high -> kept
    assert_eq!((l, v), (8, 64));
}

#[test]
fn interrupt_taken_clears_edge_external_even_when_line_high() {
    // origin: same guard, but ICR bit set (edge, :114 else branch) makes the
    // `BIT(m_icr,...)` clause in :65 true => always clears.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 64, 8);
    c.icr_w(0x0080, 0x00FF); // icr bit7 = 1 -> IRQ0 edge (sh7042.cpp:114 note)
    c.set_input(0, 1); // edge latch
    assert!(c.pending(64));
    c.interrupt_taken(64, 64);
    assert!(!c.pending(64)); // cleared even though line high
}

// ---------------------------------------------------------------------------
// set_input (sh_intc.cpp:101-126)
// ---------------------------------------------------------------------------

#[test]
fn set_input_level_tracks_both_edges() {
    // origin: level mode (ICR bit 0) asserts on high (:116-117) and clears on
    // low (:118-119).
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 65, 6); // IRQ1, ICR bit6 default 0 => level
    c.set_input(1, 1);
    assert!(c.pending(65));
    assert_eq!(c.m_lines & (1 << 1), 1 << 1); // :108-109 line remembered
    c.set_input(1, 0);
    assert!(!c.pending(65)); // :118-119
    assert_eq!(c.m_lines & (1 << 1), 0);
}

#[test]
fn set_input_edge_latches_on_rising_only() {
    // origin: edge mode (ICR bit 1) latches on assert (:122-123) and does NOT
    // clear on deassert — the pending bit survives until ack.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 66, 7); // IRQ2
    c.icr_w(0x0020, 0x00FF); // icr bit5 (7-inputnum => 7-2=5) = edge
    c.set_input(2, 1); // rising -> latch
    assert!(c.pending(66));
    c.set_input(2, 0); // falling -> edge mode does nothing
    assert!(c.pending(66)); // still latched
    assert_eq!(c.m_lines & (1 << 2), 0); // line cleared (:110-111)
    c.ack(66); // now it can be cleared
    assert!(!c.pending(66));
}

#[test]
fn set_input_same_value_early_out() {
    // origin: sh_intc.cpp:103-104 — same level as remembered => no-op return.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 67, 4);
    // deassert while already deasserted: early-out, no change, no re-assert.
    let before = c.irq_level;
    c.set_input(3, 0);
    assert_eq!(c.irq_level, before);
    assert!(!c.pending(67));
    c.set_input(3, 1);
    assert!(c.pending(67));
    // second assert: same value -> early-out, still just one pending bit.
    c.set_input(3, 1);
    assert_eq!(c.m_pending[64 >> 5], 1u32 << 3);
}

// ---------------------------------------------------------------------------
// IPR re-arbitration (the testxg / USB-stall fix, sh_intc.cpp:159-164)
// ---------------------------------------------------------------------------

#[test]
fn ipr_write_rearbitrates_winner() {
    // origin: ipr_w calls update_irq (:164). Firmware drops a source's priority
    // to stop it and restores it later; without re-pick the parked source would
    // never re-fire.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 65, 6); // high-priority source
    set_prio(&mut c, 66, 2); // low
    c.internal_interrupt(65);
    c.internal_interrupt(66);
    assert_eq!(c.irq_vector, 65);
    // swap priorities in ONE ipr_w (both nibbles in ipr[0]): 65->0, 66->9.
    let (reg, _) = slot_of(65);
    assert_eq!(reg, 0);
    let nv = (0u16 << 8) | (9u16 << 4);
    let (l, v) = c.ipr_w(0, nv, 0xFFFF); // :155-164
    assert_eq!((l, v), (9, 66)); // winner switched with no new interrupt
    assert!(c.pending(65)); // still pending, just lower priority now
    assert_eq!(c.effective_vector(9), -1); // its new level (0) is disabled
}

#[test]
fn icr_write_stores_but_does_not_rearbitrate_or_clear() {
    // origin: icr_w (:133-137) only COMBINEs; no update_irq. It cannot change
    // the current winner (arbiter reads IPR+pending only), and must leave the
    // pending latch intact.
    let mut c = Sh2Intc::new();
    set_prio(&mut c, 130, 8);
    c.internal_interrupt(130);
    let before = (c.irq_level, c.irq_vector);
    c.icr_w(0x00F0, 0x00FF); // set some ICR bits
    assert_eq!(c.icr_r(), 0x00F0); // :128-130 store
    assert_eq!((c.irq_level, c.irq_vector), before); // winner unchanged
    assert!(c.pending(130)); // latch untouched
}

// ---------------------------------------------------------------------------
// register bus decode (sh7042_map.hxx)
// ---------------------------------------------------------------------------

#[test]
fn reg_r8_byte_decode() {
    // origin: map:112-131.
    let mut c = Sh2Intc::new();
    c.ipr_w(0, 0x1234, 0xFFFF);
    c.icr_w(0x00AB, 0x00FF);
    c.isr_w(0x00CD, 0x00FF);
    assert_eq!(c.r8(0xffff8348), 0x12); // ipr0 high
    assert_eq!(c.r8(0xffff8349), 0x34); // ipr0 low
    assert_eq!(c.r8(0xffff8356), 0x00); // ipr7 high (unset)
    assert_eq!(c.r8(0xffff8358), 0x00); // icr high (only low byte used)
    assert_eq!(c.r8(0xffff8359), 0xAB); // icr low
    assert_eq!(c.r8(0xffff835b), 0xCD); // isr low
}

#[test]
fn reg_r16_decode() {
    // origin: map:376-385 (even only).
    let mut c = Sh2Intc::new();
    c.ipr_w(3, 0xBEEF, 0xFFFF);
    assert_eq!(c.r16(0xffff834e), 0xBEEF); // ipr3
    assert_eq!(c.r16(0xffff8348), 0x0000); // ipr0
    assert_eq!(c.r16(0xffff8358), 0); // icr
}

#[test]
fn reg_w8_byte_merge_preserves_other_half() {
    // origin: map w8 (:615-630) does ipr_w(idx, v<<8/<<0, mask); COMBINE must
    // keep the untouched byte (sh_intc.cpp:157).
    let mut c = Sh2Intc::new();
    c.w8(0xffff8348, 0x12); // ipr0 high byte
    c.w8(0xffff8349, 0x34); // ipr0 low byte
    assert_eq!(c.ipr_r(0), 0x1234);
    c.w8(0xffff8348, 0x99); // rewrite high only
    assert_eq!(c.ipr_r(0), 0x9934); // low byte preserved
}

#[test]
fn reg_w16_write_then_rearbitrate() {
    // origin: map w16 (:844-851) full-word ipr_w -> re-arbitrate (:164).
    let mut c = Sh2Intc::new();
    c.internal_interrupt(88); // level 0 now (ipr all 0)
    assert_eq!(c.irq_level, 0);
    c.w16(0xffff834e, 0xF000); // ipr3 nibble@shift12 (vector 88 slot12) = 15
    assert_eq!(c.irq_level, 15); // re-picked
    assert_eq!(c.irq_vector, 88);
}

// ---------------------------------------------------------------------------
// end-to-end: INTC -> set_internal_interrupt -> sh2_exception (core.rs:1885)
// ---------------------------------------------------------------------------

#[test]
fn end_to_end_intc_vectors_the_core() {
    // Proves the sci/mtu drain routing: a vector arbitrated here reaches the
    // core's sh2_exception as its actual vector (core.rs:1895-1896 uses
    // m_internal_irq_vector), and the CPU drops it if SR.IMASK >= level.
    let mut rom = vec![0u8; 0x400_000];
    let target: u32 = 0x0010_2040; // arbitrary firmware-ish handler address
    // exception entry = VBR + vector*4; VBR default 0, vector 130 (sci RXI).
    rom[130 * 4..130 * 4 + 4].copy_from_slice(&target.to_be_bytes());

    let mut s = Sh7042::new(28_000_000, rom.clone());
    s.dev.core.sr = 0; // SR.IMASK = 0 -> any level > 0 vectors
    s.dev.core.vbr = 0;
    s.dev.core.pc = 0x0010_0000;
    s.dev.core.r[15] = 0x4000_ff00; // RAM stack (mu2000.cpp:85)

    let mut intc = Sh2Intc::new();
    // give SCI RXI (130) priority 8, then drain one request through the INTC
    set_prio(&mut intc, 130, 8);
    let (lvl, vec) = s.route_irqs(&mut intc, &[130]); // sh_intc.cpp:92 handoff
    assert_eq!((lvl, vec), (8, 130));
    assert_eq!(s.dev.core.internal_irq_level, 8); // sh7042.cpp:392
    assert_eq!(s.dev.core.m_internal_irq_vector, 130); // sh7042.cpp:393
    assert_eq!(s.dev.core.m_test_irq, 1); // sh7042.cpp:394

    s.dev.core.check_pending_irq(&mut s.bus); // core.rs:1863
    // exception taken: PC jumps to VBR+130*4 handler, level consumed.
    assert_eq!(s.dev.core.pc, target); // core.rs:1925 fetch
    assert_eq!(s.dev.core.internal_irq_level, -1); // core.rs:1898 consumed
    assert_eq!(s.dev.core.sr & 0xF0, 8 << 4); // core.rs:1921 IMASK=8
    assert_ne!(s.dev.core.sr & SH_I, 0); // interrupts re-armed off? (bit kept)

    // A blocked case: same source, IMASK raised above its level -> no vector.
    let mut s2 = Sh7042::new(28_000_000, rom.clone());
    s2.dev.core.sr = (9 << 4) & 0xF0; // IMASK 9 > level 8
    s2.dev.core.vbr = 0;
    s2.dev.core.pc = 0x0010_0000;
    s2.dev.core.r[15] = 0x4000_ff00;
    let mut intc2 = Sh2Intc::new();
    set_prio(&mut intc2, 130, 8);
    s2.route_irqs(&mut intc2, &[130]);
    s2.dev.core.check_pending_irq(&mut s2.bus);
    assert_eq!(s2.dev.core.pc, 0x0010_0000); // masked out (core.rs:1889)
    assert_eq!(s2.dev.core.internal_irq_level, 8); // still parked (not consumed)
}
