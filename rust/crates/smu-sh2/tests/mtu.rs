//! Origin-cited unit vectors for `periph/mtu.rs` (ledger row `periph: mtu`).
//! Ground truth is the disk C++ `src/mame/cpu/sh_mtu.cpp` + `sh_mtu.h` + the
//! map `sh7042_map.hxx` + the die config `sh7042.cpp:179-223`. Hand-derived
//! from those files (no capture tool; the MTU has no ROM-free C++ driver).
//!
//! Naming: `MTU0..MTU4` here are the channel INDICES (0..4). On disk the
//! channels are mtu0..mtu4 (sh7042.cpp:53-57) — mtu3 is EVEN-byte interleaved
//! at 0xffff8200.., mtu4 ODD, in the shared 8200-822d block.

use smu_sh2::periph::mtu::*;
use smu_sh2::sh7042::Sh7042Peripherals;

// bus bases (sh7042_map.hxx arms); channel index helpers
const R8: u32 = 0; // unused marker to keep clippy calm if edits shift

fn fresh() -> Sh2Mtu {
    Sh2Mtu::new() // ctor + device_start fan-out (sh_mtu.cpp:175-200)
}

// ---------------------------------------------------------------------------
// reset matrix
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:42-50 (sh_mtu_device::device_reset)
fn reset_matrix_shared_regs() {
    let m = fresh();
    assert_eq!(m.m_tstr, 0x00); // :42
    assert_eq!(m.m_tsyr, 0x00); // :43
    assert_eq!(m.m_toer, 0xc0); // :44
    assert_eq!(m.m_tocr, 0x00); // :45
    assert_eq!(m.m_tgcr, 0x80); // :46
    assert_eq!(m.m_tcdr, 0xffff); // :47
    assert_eq!(m.m_tddr, 0xffff); // :48
    assert_eq!(m.m_tcnts, 0x0000); // :49
    assert_eq!(m.m_tcbr, 0xffff); // :50
    assert_eq!(m.m_timer_count, 5); // sh7042.cpp:178
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:202-221 (channel device_reset) + :199 (tsr)
fn reset_matrix_channel() {
    let m = fresh();
    let c = &m.ch[0];
    assert_eq!(c.m_tcr, 0x00); // :208
    assert_eq!(c.m_tmdr, 0xc0); // :209
    assert_eq!(c.m_tior, 0x00); // :210
    assert_eq!(c.m_tier, 0x40); // :211  0x40 & 0x60
    assert_eq!(c.m_tsr, 0xc0); // device_start:199 (NOT in device_reset)
    assert_eq!(c.m_tcnt, 0x0000); // :214
    assert_eq!(c.m_tgr, [0xffff; 4]); // :215
    assert_eq!(c.m_clock_type, DIV_1); // :212
    assert_eq!(c.m_clock_divider, 0); // :213
    assert_eq!(c.m_phase, 0); // :218
    assert_eq!(c.m_counter_cycle, 1); // :219 (pre-first-recalc)
    assert!(c.m_counter_incrementing); // :220
    assert!(!c.m_channel_active); // :177 device_start (device_reset never sets)
    assert_eq!(c.m_tgr_clearing, TGR_CLEAR_NONE); // :207
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:211 (tier = 0x40 & tier_mask; both masks
// 0x60 and 0x4c keep bit6 -> 0x40 for every channel on this die)
fn tier_reset_all_channels_are_0x40() {
    let m = fresh();
    for i in 0..5 {
        assert_eq!(m.ch[i].m_tier, 0x40, "ch{i}");
    }
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:177 (device_start active=false) then
// 64-68/202 (machine device_reset must NOT re-clear or set active). Here we
// just confirm new() left every channel stopped.
fn all_channels_start_stopped() {
    let m = fresh();
    for i in 0..5 {
        assert!(!m.ch[i].m_channel_active);
    }
}

// ---------------------------------------------------------------------------
// prescaler / clock-select table (TCR[2:0] -> count_types[TCR&7])
// mtu3 count_types = [DIV_1,DIV_4,DIV_16,DIV_64,DIV_256,DIV_1024,INPUT_A,INPUT_B]
// (sh7042.cpp:206-214). divider exponent = count_type - DIV_1 (sh_mtu.cpp:254)
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:251-254 (m_clock_divider = count_type - DIV_1)
// full reachable divider table for mtu3 (base 0xffff8200, even byte).
fn prescaler_divider_table_mtu3() {
    // (tcr_low3, expected_clock_type, expected_divider)
    let table: [(u8, i32, i32); 8] = [
        (0, DIV_1, 0),  // DIV_1  -> /1   (1<<0)
        (1, DIV_1, 2),  // DIV_4  -> /4   (1<<2)
        (2, DIV_1, 4),  // DIV_16 -> /16  (1<<4)
        (3, DIV_1, 6),  // DIV_64 -> /64  (1<<6)
        (4, DIV_1, 8),  // DIV_256 -> /256 (1<<8)
        (5, DIV_1, 10), // DIV_1024 -> /1024 (1<<10)
        (6, INPUT_A, 0), // INPUT_A (no divide)
        (7, INPUT_B, 0), // INPUT_B
    ];
    let mut m = fresh();
    m.set_cpu_now(0);
    for (low, ct, div) in table {
        // reset to a known DIV_1 state first, then program `low`
        m.ch[3].tcr_w(0x00);
        m.ch[3].tcr_w(low);
        assert_eq!(m.ch[3].m_clock_type, ct, "low={low}");
        assert_eq!(m.ch[3].m_clock_divider, div, "low={low}");
        // verify the actual divide factor 1<<divider matches the SH naming
        if ct == DIV_1 && div > 0 {
            let f = 1i64 << div;
            let want = match low {
                1 => 4,
                2 => 16,
                3 => 64,
                4 => 256,
                5 => 1024,
                _ => 1,
            };
            assert_eq!(f, want, "low={low}");
        }
    }
    let _ = R8;
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:259-273 (TCR[4:3] phase select). With a
// nonzero divider: 0x00 -> phase 0; 0x08 -> phase = 1<<(div-1); 0x10/0x18 ->
// phase 0 AND divider-- (the "0+180" odd-divider quirk). Uses DIV_16 (div=4).
fn prescaler_phase_select() {
    let mut m = fresh();
    m.set_cpu_now(0);
    // tcr = low3(2=DIV_16) | 0x08 -> phase 180
    m.ch[3].tcr_w(0x00);
    m.ch[3].tcr_w(2 | 0x08);
    assert_eq!(m.ch[3].m_clock_divider, 4);
    assert_eq!(m.ch[3].m_phase, 1 << 3); // :265 == 8

    // tcr = low3(2) | 0x10 -> 0+180: divider-- (4->3), phase 0
    m.ch[3].tcr_w(0x00);
    m.ch[3].tcr_w(2 | 0x10);
    assert_eq!(m.ch[3].m_clock_divider, 3); // :270
    assert_eq!(m.ch[3].m_phase, 0); // :269

    // tcr = low3(2) | 0x18 -> same as 0x10
    m.ch[3].tcr_w(0x00);
    m.ch[3].tcr_w(2 | 0x18);
    assert_eq!(m.ch[3].m_clock_divider, 3);
    assert_eq!(m.ch[3].m_phase, 0);
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:233-249 (TCR[6:5] compare-match/clear
// select + the >2-TGR bit7 offset). mtu0 has tgr_count=4 so bit7 applies.
fn tcr_clearing_select_mtu0() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.ch[0].tcr_w(0x00); // &0x60=0 -> NONE (:235)
    assert_eq!(m.ch[0].m_tgr_clearing, TGR_CLEAR_NONE);
    m.ch[0].tcr_w(0x20); // &0x60=0x20 -> (0x20&0x20)?0:1 = 0 (:239), no 0x80
    assert_eq!(m.ch[0].m_tgr_clearing, 0);
    m.ch[0].tcr_w(0x40); // &0x60=0x40 -> (0x40&0x20)?0:1 = 1 (:239)
    assert_eq!(m.ch[0].m_tgr_clearing, 1);
    m.ch[0].tcr_w(0x40 | 0x80); // clearing 1 +2 (tgr_count>2 && 0x80) = 3 (:240-241)
    assert_eq!(m.ch[0].m_tgr_clearing, 3);
    m.ch[0].tcr_w(0x60); // EXT (:246)
    assert_eq!(m.ch[0].m_tgr_clearing, TGR_CLEAR_EXT);
}

// ---------------------------------------------------------------------------
// freerun 16-bit counting / wrap (driven through a TCNT read, which is the
// only in-band advance trigger when no compare/overflow IRQ is armed —
// sh_mtu.cpp:344-347 update_counter on tcnt_r).
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:487-498 (increment + modulo counter_cycle).
// mtu0: TCR=DIV_1, TGR auto-clear none -> counter_cycle 0x10000 (recalc:430).
// Load TCNT=0xfffe, advance 3 CPU cycles by reading TCNT -> wraps to 0x0001.
fn freerun_wrap_fffe_plus3() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8260, 0x00); // mtu0 TCR DIV_1
    m.mtu_w8(0xffff8268, 0x00); // TGR0 hi
    m.mtu_w8(0xffff8269, 0x04); // TGR0=0x0004 (cmp 5, irrelevant: TIER bit0 off)
    m.mtu_w8(0xffff8240, 0x01); // TSTR bit0 -> enable ch0 (:64)
    assert!(m.ch[0].m_channel_active);
    assert_eq!(m.ch[0].m_counter_cycle, 0x1_0000); // after set_enable recalc (:430)
    // load TCNT=0xfffe
    m.mtu_w8(0xffff8266, 0xff); // hi
    m.mtu_w8(0xffff8267, 0xfe); // lo -> 0xfffe
    assert_eq!(m.mtu_r16(0xffff8266), 0xfffe);
    // advance clock by 3 and read (this is what triggers update_counter)
    m.set_cpu_now(3);
    assert_eq!(m.mtu_r16(0xffff8266), 0x0001); // 0xfffe + 3 == 0x10001 % 0x10000
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:484 (new_time==base_time -> no advance).
// Reading twice with the same cpu_now must not double count.
fn freerun_no_advance_without_clock_edge() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8260, 0x00);
    m.mtu_w8(0xffff8240, 0x01);
    m.set_cpu_now(100);
    let a = m.mtu_r16(0xffff8266);
    let b = m.mtu_r16(0xffff8266); // same cpu_now -> second read no delta
    assert_eq!(a, 100);
    assert_eq!(b, 100);
}

// ---------------------------------------------------------------------------
// compare match: sets TGRi flag + IRQ only when TIER bit enabled
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:500-518 (match loop). mtu0 TGR0=0x0004
// (cmp 5); TIER bit0 = IRQ_A enabled -> crossing sets TSR bit0 AND pushes the
// TGRA vector = irq_base(88)+0 = 88 (sh_mtu.h:76, sh7042.cpp:179).
fn compare_match_sets_flag_and_irq_when_tier_enabled() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8260, 0x00); // DIV_1
    m.mtu_w8(0xffff8268, 0x00);
    m.mtu_w8(0xffff8269, 0x04); // TGR0 = 0x0004
    m.mtu_w8(0xffff8264, 0x41); // TIER = 0x40 | IRQ_A(bit0)
    m.mtu_w8(0xffff8240, 0x01); // enable ch0
    m.set_cpu_now(5);
    assert_eq!(m.mtu_r16(0xffff8266), 5); // crossed the match
    let tsr = m.mtu_r8(0xffff8265); // :77 tsr_r
    assert_ne!(tsr & IRQ_A, 0, "TGRA flag must be set");
    let mut out = Vec::new();
    let n = m.drain_irqs(&mut out);
    assert_eq!(n, 1);
    assert_eq!(out[0], 88); // mtu0 TGRA vector
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:514-518 — same crossing but TIER bit0 CLEAR
// (default 0x40): flag still sets (:515) but no internal_interrupt (:516).
fn compare_match_sets_flag_without_irq_when_tier_disabled() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8260, 0x00);
    m.mtu_w8(0xffff8268, 0x00);
    m.mtu_w8(0xffff8269, 0x04); // TGR0=0x0004
    m.mtu_w8(0xffff8264, 0x40); // TIER bit0 OFF
    m.mtu_w8(0xffff8240, 0x01);
    m.set_cpu_now(5);
    let _ = m.mtu_r16(0xffff8266);
    assert_ne!(m.mtu_r8(0xffff8265) & IRQ_A, 0); // flag set
    let mut out = Vec::new();
    assert_eq!(m.drain_irqs(&mut out), 0); // but no IRQ
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:500-519, vectors from sh7042.cpp:188-196:
// mtu1 (tgr_count=2, base 96) can only raise TGRA=96 / TGRB=97 (slot C dead
// -1, mask 0x4c&0x04). Program TGRB, cross, expect vector 97.
fn compare_match_mtu1_vector_97() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8280, 0x00); // mtu1 TCR DIV_1
    m.mtu_w8(0xffff828a, 0x00); // TGR1 hi
    m.mtu_w8(0xffff828b, 0x02); // TGR1 = 0x0002 (cmp 3)
    m.mtu_w8(0xffff8284, 0x42); // TIER bit1 = IRQ_B (:92)
    m.mtu_w8(0xffff8240, 0x02); // TSTR bit1 -> enable ch1 (:65)
    m.set_cpu_now(3);
    let _ = m.mtu_r16(0xffff8286); // mtu1 TCNT base
    assert_ne!(m.mtu_r8(0xffff8285) & IRQ_B, 0); // :93 tsr (mtu1)
    let mut out = Vec::new();
    m.drain_irqs(&mut out);
    assert!(out.contains(&97), "expected TGRB vector 97, got {out:?}");
}

// ---------------------------------------------------------------------------
// TSR write-clear (:340) — bit set clears, bit0..6 clear, bit7 is write
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:337-342. After TGRA matched (TSR bit0=1),
// tsr_w(0x01) clears bit0; writing 0 changes nothing; bit7 is plain-write.
fn tsr_write_clear_semantics() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8260, 0x00);
    m.mtu_w8(0xffff8268, 0x00); // TGR0 hi
    m.mtu_w8(0xffff8269, 0x04); // TGR0 = 0x0004
    m.mtu_w8(0xffff8264, 0x41); // TIER IRQ_A
    m.mtu_w8(0xffff8240, 0x01);
    m.set_cpu_now(5);
    let _ = m.mtu_r16(0xffff8266);
    assert_ne!(m.mtu_r8(0xffff8265) & IRQ_A, 0); // matched
    // clear TGRA
    m.mtu_w8(0xffff8265, 0x01);
    assert_eq!(m.mtu_r8(0xffff8265) & IRQ_A, 0); // cleared
    // writing 0 must be a no-op on remaining flags (bit7 stays as loaded)
    let before = m.mtu_r8(0xffff8265);
    m.mtu_w8(0xffff8265, 0x00);
    assert_eq!(m.mtu_r8(0xffff8265), before);
    // bit7 write: (data&0x80) part of :340
    m.mtu_w8(0xffff8265, 0x80);
    assert_ne!(m.mtu_r8(0xffff8265) & 0x80, 0);
}

// ---------------------------------------------------------------------------
// start / stop gating: stopped channel does NOT count but TGR/TCNT stay
// writable (sh_mtu.cpp:383-392 set_enable + :472-474 inactive resync).
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:472-474 (inactive -> resync last_clock,
// do not count). Enable, count to 50, stop (TSTR=0), advance a lot, read ->
// frozen at 50; a TGR write still lands (register file live).
fn stopped_channel_frozen_but_tgr_writable() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8260, 0x00);
    m.mtu_w8(0xffff8240, 0x01); // enable ch0
    m.set_cpu_now(50);
    assert_eq!(m.mtu_r16(0xffff8266), 50);
    m.mtu_w8(0xffff8240, 0x00); // stop ch0 (:64 set_enable false)
    assert!(!m.ch[0].m_channel_active);
    m.set_cpu_now(1000);
    assert_eq!(m.mtu_r16(0xffff8266), 50); // frozen (:472-474)
    // TGR still writable while stopped (tgr_w just COMBINEs; :368-369)
    m.mtu_w8(0xffff8268, 0xbe);
    m.mtu_w8(0xffff8269, 0xef);
    assert_eq!(m.mtu_r16(0xffff8268), 0xbeef);
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:383-392 (set_enable). Restart resumes from
// the stored TCNT: stop, advance (frozen), re-enable at a new cpu_now, then a
// further advance counts the delta after the re-enable instant.
fn stopped_channel_restart_resumes() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8260, 0x00);
    m.mtu_w8(0xffff8240, 0x01);
    m.set_cpu_now(20);
    assert_eq!(m.mtu_r16(0xffff8266), 20);
    m.mtu_w8(0xffff8240, 0x00); // stop
    m.set_cpu_now(500); // elapsed while stopped
    assert_eq!(m.mtu_r16(0xffff8266), 20); // frozen
    m.set_cpu_now(500);
    m.mtu_w8(0xffff8240, 0x01); // re-enable: set_enable resyncs last_clock=500
    m.set_cpu_now(530);
    assert_eq!(m.mtu_r16(0xffff8266), 20 + 30); // resumed +30
}

// ---------------------------------------------------------------------------
// overflow (TOVF / IRQ_V)
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:520-524. counter_cycle=0x10000 (no
// auto-clear); TCNT=0xfffe, advance 3 -> tt crosses 0x10000 -> TSR |= IRQ_V
// (:521) + (TIER&IRQ_V) push TOVF vector 92 = irq_base(88)+4 (sh_mtu.h:80).
fn tovf_sets_flag_and_irq() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8260, 0x00); // DIV_1, clearing NONE -> cycle 0x10000
    m.mtu_w8(0xffff8264, 0x50); // TIER = 0x40 | IRQ_V(bit4)
    m.mtu_w8(0xffff8240, 0x01); // enable ch0
    m.mtu_w8(0xffff8266, 0xff); // TCNT hi
    m.mtu_w8(0xffff8267, 0xfe); // TCNT=0xfffe
    m.set_cpu_now(3);
    let _ = m.mtu_r16(0xffff8266);
    assert_ne!(m.mtu_r8(0xffff8265) & IRQ_V, 0); // :521
    let mut out = Vec::new();
    let n = m.drain_irqs(&mut out);
    assert_eq!(n, 1);
    assert_eq!(out[0], 92); // mtu0 TOVF
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:522 — same overflow, TIER bit4 CLEAR ->
// flag set (:521) but no IRQ pushed.
fn tovf_flag_without_irq_when_disabled() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8260, 0x00);
    m.mtu_w8(0xffff8264, 0x40); // IRQ_V off
    m.mtu_w8(0xffff8240, 0x01);
    m.mtu_w8(0xffff8266, 0xff);
    m.mtu_w8(0xffff8267, 0xfe);
    m.set_cpu_now(3);
    let _ = m.mtu_r16(0xffff8266);
    assert_ne!(m.mtu_r8(0xffff8265) & IRQ_V, 0);
    let mut out = Vec::new();
    assert_eq!(m.drain_irqs(&mut out), 0);
}

// ---------------------------------------------------------------------------
// internal_update event pump (scheduler seam) + sticky resched
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:394-402 (internal_update) + :404-462
// (recalc_event schedules the compare event). With TGRA armed and TCNT=0,
// recalc schedules event at cpu_now + (cmp - tcnt). The seam mtu_ch_update
// then fires exactly at that cycle, raising the flag/IRQ once.
fn internal_update_pumps_scheduled_compare() {
    let mut m = fresh();
    m.set_cpu_now(1000); // some nonzero time
    m.mtu_w8(0xffff8260, 0x00); // DIV_1
    m.mtu_w8(0xffff8268, 0x00);
    m.mtu_w8(0xffff8269, 0x10); // TGR0=0x0010 (cmp 0x11=17)
    m.mtu_w8(0xffff8264, 0x41); // TIER IRQ_A
    m.mtu_w8(0xffff8240, 0x01); // enable ch0 (recalc schedules event)
    let ev = m.ch[0].m_event_time;
    assert!(ev > 1000, "compare event must be scheduled after now");
    assert_eq!(ev - 1000, 17); // divider 0 -> event = now + (cmp - tcnt)
    // pump to just before the event -> no match yet
    assert_eq!(m.mtu_ch_update(0, ev - 1), ev);
    assert_eq!(m.mtu_r8(0xffff8265) & IRQ_A, 0);
    // pump to the event -> match fires
    m.mtu_ch_update(0, ev);
    assert_ne!(m.mtu_r8(0xffff8265) & IRQ_A, 0);
    let mut out = Vec::new();
    m.drain_irqs(&mut out);
    assert_eq!(out, vec![88]);
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:460-461 — a register write that changes the
// scheduled event while the CPU is running (update_cpu) latches a resched.
// NOTE :406-409 — an INACTIVE channel's recalc returns early (no resched), so
// the write must be armed AND the channel enabled to reach the resched site.
fn register_write_sets_resched() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8260, 0x00); // DIV_1
    m.mtu_w8(0xffff8268, 0x00); // TGR0 hi
    m.mtu_w8(0xffff8269, 0x10); // TGR0 = 0x0010 (cmp 17)
    m.mtu_w8(0xffff8264, 0x40); // TIER: no compare armed -> event_time stays 0
    m.mtu_w8(0xffff8240, 0x01); // enable ch0
    assert!(!m.take_resched()); // event unchanged (still 0)
    m.mtu_w8(0xffff8264, 0x41); // arm IRQ_A -> schedules event -> resched
    assert!(m.take_resched());
    assert!(!m.take_resched()); // sticky, cleared on take
}

// ---------------------------------------------------------------------------
// CHAIN mode never advances (no chain-tick driver on disk)
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:276-280 (CHAIN select) + :466/:414
// (non-DIV_1 -> no counting, no event). mtu1 tcr index 7 = CHAIN
// (sh7042.cpp:196, set_chain(m_mtu2)).
fn chain_clock_never_counts() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8280, 0x07); // mtu1 TCR: low3=7 -> CHAIN
    assert_eq!(m.ch[1].m_clock_type, CHAIN);
    m.mtu_w8(0xffff8240, 0x02); // enable ch1
    assert_eq!(m.ch[1].m_event_time, 0); // :415 no event
    m.set_cpu_now(1000);
    assert_eq!(m.mtu_r16(0xffff8286), 0); // frozen (:466)
}

// ---------------------------------------------------------------------------
// reg read/write holes + widths
// ---------------------------------------------------------------------------

#[test]
// origin: sh7042_map.hxx mtu arms. The bus does NOT route these, so the
// seam itself must drop writes and return 0 on reads (membus miss -> 0).
fn write_holes_dropped_read_holes_zero() {
    let mut m = fresh();
    m.set_cpu_now(0);
    // 0xffff820c is a hole (between TOCR 820b and TGCR 820d)
    m.mtu_w8(0xffff820c, 0xff);
    assert_eq!(m.mtu_r8(0xffff820c), 0);
    // 0xffff822e/822f past TSR mtu4 in the shared block -> hole
    m.mtu_w8(0xffff822e, 0x55);
    assert_eq!(m.mtu_r8(0xffff822e), 0);
    // an even-but-unmapped r16 (820c) -> 0
    assert_eq!(m.mtu_r16(0xffff820c), 0);
    // a stray mtu w16 hole (8242 is past TSYR) drops silently
    m.mtu_w16(0xffff8242, 0x1234);
    // 820a r16 is the TOER/TOCR pair (TOER reset 0xc0, TOCR 0)
    assert_eq!(m.mtu_r16(0xffff820a), 0xc000);
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:107/354-359 (TCNT byte writes are
// COMBINE_DATA — hi/lo independent, mask picks the byte).
fn tcnt_byte_write_combine() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.ch[0].m_tgr = [0xffff; 4];
    m.ch[0].tcnt_w(0xabcd, 0xff00); // set hi only
    assert_eq!(m.ch[0].m_tcnt & 0xff00, 0xab00);
    assert_eq!(m.ch[0].m_tcnt & 0x00ff, 0x0000);
    m.ch[0].tcnt_w(0x78, 0x00ff); // set lo only
    assert_eq!(m.ch[0].m_tcnt, 0xab78);
}

#[test]
// origin: sh7042_map.hxx:31-34/534-537/804-805 — TIOR is answered at TWO
// consecutive byte addresses per channel (mtu3 = 8204 AND 8205), and the r16
// pair duplicates the same register into both bytes (8204 -> hi=lo=tior3).
fn tior_double_address_quirk() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8204, 0x3a); // mtu3 TIOR via first address
    assert_eq!(m.mtu_r8(0xffff8204), 0x3a);
    assert_eq!(m.mtu_r8(0xffff8205), 0x3a); // same register, second address
    // second write via the alias address overwrites
    m.mtu_w8(0xffff8205, 0x9b);
    assert_eq!(m.mtu_r8(0xffff8204), 0x9b);
    // r16 duplicated (map:336)
    assert_eq!(m.mtu_r16(0xffff8204), 0x9b9b);
}

#[test]
// origin: sh7042_map.hxx:354 — mtu3/mtu4 TSR share one r16 word (hi=mtu3,
// lo=mtu4); w16 at 822c writes tsr3(hi) then tsr4(lo) (map:822).
fn tsr_shared_word_hi_lo() {
    let mut m = fresh();
    m.set_cpu_now(0);
    // set some flags: bit6 is preserved by tsr_w (only 0..6 write-clear)
    m.mtu_w8(0xffff822c, 0x2f); // mtu3 tsr: clear 0x2f bits 0..5, keep bit6/7
    m.mtu_w8(0xffff822d, 0x40); // mtu4 tsr
    let w = m.mtu_r16(0xffff822c);
    // tsr high byte = mtu3, low byte = mtu4
    let hi = (w >> 8) as u8;
    let lo = w as u8;
    assert_eq!(hi, m.mtu_r8(0xffff822c));
    assert_eq!(lo, m.mtu_r8(0xffff822d));
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:119-165 — TCNTS is a normal 16-bit reg
// (reset 0); write hi/lo then read full + r16.
fn shared_word_regs() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8220, 0x12); // TCNTS hi
    m.mtu_w8(0xffff8221, 0x34); // lo
    assert_eq!(m.mtu_r16(0xffff8220), 0x1234);
    // w16 full overwrite
    m.mtu_w16(0xffff8214, 0xcafe); // TCDR
    assert_eq!(m.mtu_r16(0xffff8214), 0xcafe);
    assert_eq!(m.mtu_r8(0xffff8214), 0xca);
    assert_eq!(m.mtu_r8(0xffff8215), 0xfe);
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:59-68 (tstr_w enable-bit map: ch0/ch1/ch2
// from bits 0/1/2, ch3/ch4 from bits 6/7). Verifies the routing via the bus.
fn tstr_enable_bit_map() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8240, 0xC4); // bits 2,6,7 -> ch2, ch3, ch4
    assert!(!m.ch[0].m_channel_active);
    assert!(!m.ch[1].m_channel_active);
    assert!(m.ch[2].m_channel_active); // :66 bit2
    assert!(m.ch[3].m_channel_active); // :67 bit6
    assert!(m.ch[4].m_channel_active); // :68 bit7
    // bits 3,4,5 route to nothing (no channel) — TSTR stores the raw byte;
    // the unused bits read back but drove no enable (:63 comment "To generalize")
    assert_eq!(m.mtu_r8(0xffff8240), 0xC4);
}

#[test]
// origin: sh7042_map.hxx:336/357/365/371 — per-channel TIOR r16 duplicates
// the byte into both halves (mtu0 8262, mtu1 8282, mtu2 82a2).
fn tior_dup_r16_all_channels() {
    let mut m = fresh();
    m.set_cpu_now(0);
    for (base, val) in [(0xffff8262u32, 0x11u8), (0xffff8282, 0x22), (0xffff82a2, 0x33)] {
        m.mtu_w8(base, val);
        assert_eq!(m.mtu_r16(base), (val as u16) | ((val as u16) << 8));
    }
}

#[test]
// origin: src/mame/cpu/sh_mtu.h:76-81 (interrupt slot table). Dead slots are
// -1: mtu1/mtu2 (mask 0x4c = bits 2,3,6) kill slot C (0x04) and slot D
// (0x08) — U is kept; mtu0/3/4 (mask 0x60 = bits 5,6) kill only slot U
// (0x20). TOVF slot4 is always wired (sh_mtu.h:80, unconditional base+4).
fn interrupt_slot_table() {
    let m = fresh();
    // mtu0 base 88, mask 0x60: [88,89,90,91,92,-1]
    assert_eq!(m.ch[0].m_interrupt, [88, 89, 90, 91, 92, -1]);
    // mtu1 base 96, mask 0x4c (bits 2,3,6): slot C (0x04) dead, slot D (0x08)
    // dead, slot U (0x20) clear -> base+5
    assert_eq!(m.ch[1].m_interrupt, [96, 97, -1, -1, 100, 101]);
    // mtu2 base 104, mask 0x4c
    assert_eq!(m.ch[2].m_interrupt, [104, 105, -1, -1, 108, 109]);
    // mtu3 base 112, mask 0x60
    assert_eq!(m.ch[3].m_interrupt, [112, 113, 114, 115, 116, -1]);
    // mtu4 base 120, mask 0x60
    assert_eq!(m.ch[4].m_interrupt, [120, 121, 122, 123, 124, -1]);
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:464-483 — prescaled divide: mtu3 DIV_256
// (divider 8). 512 cpu cycles == 2 tick edges -> TCNT advances by 2 (the
// >> divider at :481-482 with phase 0).
fn prescaled_divide_counts_edges() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8200, 0x04); // mtu3 TCR low3=4 -> DIV_256 (divider 8)
    assert_eq!(m.ch[3].m_clock_divider, 8);
    m.mtu_w8(0xffff8240, 0x40); // TSTR bit6 -> enable ch3 (:67)
    m.set_cpu_now(511); // floor((511)/256) = 1 edge from base 0
    assert_eq!(m.mtu_r16(0xffff8210), 1); // mtu3 TCNT base (map:340)
    m.set_cpu_now(767); // floor(767/256)=2 -> one more edge
    assert_eq!(m.mtu_r16(0xffff8210), 2);
}

#[test]
// origin: src/mame/cpu/sh_mtu.cpp:531-549 device_reset top matrix must be
// re-appliable (machine reset fan-out) after the device was dirtied.
fn device_reset_restores_matrix() {
    let mut m = fresh();
    m.set_cpu_now(0);
    m.mtu_w8(0xffff8240, 0xff); // dirtied TSTR
    m.mtu_w16(0xffff8214, 0x0000); // dirtied TCDR
    m.mtu_w8(0xffff820b, 0xaa); // dirtied TOCR
    m.device_reset();
    assert_eq!(m.m_tstr, 0); // :42 (tstr_w not re-run -> no enable change note)
    assert_eq!(m.m_tcdr, 0xffff); // :47
    assert_eq!(m.m_tocr, 0); // :45
    assert_eq!(m.m_toer, 0xc0); // :44
    assert_eq!(m.ch[0].m_tcr, 0); // :208
}
