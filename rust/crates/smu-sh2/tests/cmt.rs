//! Origin-cited unit vectors for `periph/cmt.rs` (ledger row `periph: cmt`).
//! Ground truth = disk C++ `src/mame/cpu/sh_cmt.cpp` (232) + `sh_cmt.h` (83)
//! + map `sh7042_map.hxx:186-199/413-419/687-700/880-886` + die config
//! `sh7042.cpp:172` (`SH_CMT(..., 144, 148)`). Hand-derived from those files.
//!
//! DISK NOTE: this die's CMT is TWO 16-bit channels (CMSTR + CMCSR/CMCNT/CMCOR
//! ×2), NOT the upstream T1MA..T4MB/T32 family — so there are no T32/CMCOR
//! tests here (they don't exist on disk; see cmt.rs module doc).

use smu_sh2::periph::cmt::ShCmt;
use smu_sh2::sh7042::Sh7042Peripherals;

// map bases (sh7042_map.hxx)
const CMSTR: u32 = 0xffff83d0;
const CMCSR0: u32 = 0xffff83d2;
const CMCNT0: u32 = 0xffff83d4;
const CMCOR0: u32 = 0xffff83d6;
const CMCSR1: u32 = 0xffff83d8;
const CMCNT1: u32 = 0xffff83da;
const CMCOR1: u32 = 0xffff83dc;

fn fresh() -> ShCmt {
    // die vectors 144/148 (sh7042.cpp:172)
    ShCmt::new(144, 148)
}

// start ch0 with a given COR and CKS (csr low 2 bits), from cpu_now=0.
fn start_ch0(d: &mut ShCmt, cor: u16, cks: u16) {
    d.set_cpu_now(0);
    d.cmt_w16(CMCSR0, cks); // csr0 = cks (str0 still off -> cnt_update no-op)
    d.cmt_w16(CMCOR0, cor);
    d.cmt_w16(CMSTR, 1); // str bit0 0->1 -> clock_start(0)
    let _ = d.take_resched(); // cmstr_w always rescheds (:166)
}

fn irq(d: &mut ShCmt) -> Vec<i32> {
    let mut v = Vec::new();
    d.drain_irqs(&mut v);
    v
}

// ---------------------------------------------------------------------------
// reset matrix / vectors
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:40-47 (device_reset) + :18-28 (ctor)
fn reset_defaults() {
    let d = fresh();
    assert_eq!(d.m_str, 0x0000); // :43
    assert_eq!(d.m_csr, [0x0000, 0x0000]); // :44
    assert_eq!(d.m_cnt, [0x0000, 0x0000]); // :45
    assert_eq!(d.m_cor, [0xffff, 0xffff]); // :46
    assert_eq!(d.m_next_event, [0, 0]); // :42
    assert_eq!(d.cpu_now, 0);
    assert!(!d.resched);
    assert_eq!(d.irq_len, 0);
}

#[test]
// origin: src/mame/cpu/sh_cmt.h:37-38 (template ctor stores vectors) +
// sh7042.cpp:172 SH_CMT(config, m_cmt, *this, m_intc, 144, 148). NOT reset.
fn intc_vectors_are_144_148() {
    let d = fresh();
    assert_eq!(d.m_intc_vector, [144, 148]);
}

// ---------------------------------------------------------------------------
// prescaler: shift = 3 + 2*CKS (sh_cmt.cpp:202) -> divisor 1<<shift
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:202 (step1 = 1 << (3 + 2*BIT(csr,0,2))).
// With COR=0,CNT=0 the whole next-event offset IS the divisor.
fn prescaler_divisor_table() {
    let want = [8u64, 32, 128, 512]; // CKS 0,1,2,3
    for cks in 0..4u16 {
        let mut d = fresh();
        d.set_cpu_now(0);
        d.cmt_w16(CMCSR0, cks);
        d.cmt_w16(CMCOR0, 0); // counts = 0+1-0 = 1
        d.cmt_w16(CMSTR, 1); // clock_start -> compute_next_event
        assert_eq!(d.m_next_event[0], want[cks as usize], "cks={cks}");
    }
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:217,222 (cnt_update step + back-compute).
// CKS=0 -> one count per 8 cpu cycles.
fn tcnt_increments_cks0() {
    let mut d = fresh();
    start_ch0(&mut d, 0x00ff, 0); // COR=255, shift 3
    d.set_cpu_now(0);
    assert_eq!(d.cmcnt0_r(), 0);
    d.set_cpu_now(8);
    assert_eq!(d.cmcnt0_r(), 1);
    d.set_cpu_now(16);
    assert_eq!(d.cmcnt0_r(), 2);
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:217 (same, CKS=1 -> shift 5 -> /32).
fn tcnt_increments_cks1_slower() {
    let mut d = fresh();
    start_ch0(&mut d, 0x00ff, 1); // COR=255, shift 5
    d.set_cpu_now(0);
    assert_eq!(d.cmcnt0_r(), 0);
    d.set_cpu_now(16); // still within first /32 slot
    assert_eq!(d.cmcnt0_r(), 0);
    d.set_cpu_now(32);
    assert_eq!(d.cmcnt0_r(), 1);
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:50-58 (catch_up) + :213-224 (cnt_update).
// At one full period (256*8=2048) CMI sets and the count wraps back to 0.
fn tcnt_wraps_at_match() {
    let mut d = fresh();
    start_ch0(&mut d, 0x00ff, 0);
    d.set_cpu_now(2048); // full period
    assert_eq!(d.cmcnt0_r(), 0); // wrapped (255 -> 0)
    assert_ne!(d.m_csr[0] & 0x80, 0); // CMI flag up (:53)
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:218-220 — repeated whole-period advance
// leaves the ruler monotonic: 2056 == one tick past the wrap.
fn tcnt_advances_after_wrap() {
    let mut d = fresh();
    start_ch0(&mut d, 0x00ff, 0);
    d.set_cpu_now(2056); // 2048 + 8
    assert_eq!(d.cmcnt0_r(), 1);
}

// ---------------------------------------------------------------------------
// compare-match flag + IRQ gating (intc seam via drain_irqs)
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:52-56 — flag rises even with CMIE(bit6) off
// but NO interrupt reaches the INTC.
fn cmp_match_flag_but_no_irq_when_ie_off() {
    let mut d = fresh();
    start_ch0(&mut d, 1, 0); // COR=1 -> period 16, CMIE off (csr0=0)
    assert_eq!(d.cmt_update(16), 0); // catch_up fires; also 0 scheduled (:77)
    assert_ne!(d.m_csr[0] & 0x80, 0); // CMI up (:53)
    assert_eq!(irq(&mut d), Vec::<i32>::new()); // bit6=0 -> no fire (:54)
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:54-55 (BIT(csr,6) -> internal_interrupt) +
// sh7042.cpp:172 ch0 vector 144.
fn cmp_match_fires_irq_144_when_ie_on() {
    let mut d = fresh();
    d.set_cpu_now(0);
    d.cmt_w16(CMCSR0, 0x40); // CKS=0 + CMIE(bit6)
    d.cmt_w16(CMCOR0, 1);
    d.cmt_w16(CMSTR, 1);
    d.take_resched();
    // catch_up fired the match at 16 and advanced next_event to 32; internal_update
    // advertises the NEXT period (sh_cmt.cpp:78), and ch0 (CMIE on) is scheduled.
    assert_eq!(d.cmt_update(16), 32);
    assert_eq!(irq(&mut d), vec![144]);
}

#[test]
// origin: sh7042.cpp:172 ch1 vector 148 (m_intc_vector[1]).
fn ch1_fires_irq_148() {
    let mut d = fresh();
    d.set_cpu_now(0);
    d.cmt_w16(CMCSR1, 0x40); // csr1 CMIE
    d.cmt_w16(CMCOR1, 1);
    d.cmt_w16(CMSTR, 2); // str bit1 -> clock_start(1)
    d.take_resched();
    d.cmt_update(16);
    assert_eq!(irq(&mut d), vec![148]);
}

#[test]
// origin: catch_up sets 0x80 (:53); nothing clears it but csr_w (:172).
// With next_event already past, a second read keeps the flag up.
fn cmi_flag_is_sticky() {
    let mut d = fresh();
    start_ch0(&mut d, 1, 0);
    d.set_cpu_now(16);
    assert_ne!(d.cmcsr0_r() & 0x80, 0);
    assert_ne!(d.cmcsr0_r() & 0x80, 0); // still up
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:169-173 (csr_w COMBINE clears bit7 when the
// masked data has bit7=0; bit6/CKS preserved).
fn cmi_flag_clear_via_csr() {
    let mut d = fresh();
    start_ch0(&mut d, 1, 0);
    d.set_cpu_now(16);
    d.cmcsr0_r(); // raise flag
    d.csr_w(0, 0x0000, 0x80); // clear bit7 only
    assert_eq!(d.cmcsr0_r() & 0x80, 0); // (:172)
}

// ---------------------------------------------------------------------------
// CMSTR start/stop semantics (clock_start / next_event clear)
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:161-163 — a 0->1 CMSTR bit calls
// clock_start -> compute_next_event (next_event becomes non-zero).
fn str_rising_starts_clock() {
    let mut d = fresh();
    start_ch0(&mut d, 0x00ff, 0);
    assert_ne!(d.m_next_event[0], 0);
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:164-165 — a bit held 0 (else branch) zeroes
// that channel's next_event.
fn str_falling_clears_next_event() {
    let mut d = fresh();
    start_ch0(&mut d, 0x00ff, 0);
    assert_ne!(d.m_next_event[0], 0);
    d.set_cpu_now(100);
    d.cmt_w16(CMSTR, 0); // bit0 -> 0
    assert_eq!(d.m_next_event[0], 0);
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:161-165 — re-writing a bit that is ALREADY 1
// takes neither branch (no clock_start, no clear): schedule is preserved.
fn str_stays_on_keeps_schedule() {
    let mut d = fresh();
    start_ch0(&mut d, 0x00ff, 0);
    let ev = d.m_next_event[0];
    d.set_cpu_now(0);
    d.cmt_w16(CMSTR, 1); // still 1
    assert_eq!(d.m_next_event[0], ev);
}

// ---------------------------------------------------------------------------
// write-during-count quirks: cnt_w writes raw (NO cnt_update), unlike cor_w
// ---------------------------------------------------------------------------

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:175-182 (cnt_w): COMBINE only, then recompute
// +resched when running. Mask honoured (partial write).
fn cnt_w_partial_raw_mask() {
    let mut d = fresh();
    d.set_cpu_now(0);
    d.cmt_w16(CMCNT0, 0xabcd); // stopped -> raw combine (no cnt_update, :177)
    assert_eq!(d.m_cnt[0], 0xabcd);
    d.cmt_w8(CMCNT0, 0x12); // high byte only (map:691, mask 0xff00)
    assert_eq!(d.m_cnt[0], 0x12cd); // low byte preserved
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:178-180 — running cnt_w recomputes the
// schedule AND raises the sticky resched (m_cpu->internal_update()).
fn cnt_w_running_sets_resched() {
    let mut d = fresh();
    start_ch0(&mut d, 0x00ff, 0);
    let _ = d.take_resched();
    d.set_cpu_now(0);
    d.cmt_w16(CMCNT0, 0x0000); // str0 on -> recompute (:179) + resched (:180)
    assert!(d.take_resched());
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:188-190 — cor_w recomputes +resched only when
// the channel runs; a stopped cor_w is a plain register write.
fn cor_w_running_recomputes_stopped_does_not() {
    let mut d = fresh();
    // stopped path
    let _ = d.take_resched();
    d.cmt_w16(CMCOR0, 0x0010); // str0 off -> no resched (:188 false)
    assert_eq!(d.m_cor[0], 0x0010);
    assert!(!d.take_resched());
    // running path
    start_ch0(&mut d, 0x00ff, 0); // str0 on, next=2048
    let _ = d.take_resched();
    d.set_cpu_now(0);
    d.cmt_w16(CMCOR0, 1); // running -> next = (1+1)*8 = 16 (:209)
    assert_eq!(d.m_next_event[0], 16);
    assert!(d.take_resched());
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:166 — cmstr_w ALWAYS requests a resched,
// regardless of whether any bit actually changed.
fn cmstr_w_always_rescheds() {
    let mut d = fresh();
    let _ = d.take_resched();
    d.cmt_w16(CMSTR, 0x0000); // no bits set, still rescheds (:166)
    assert!(d.take_resched());
}

// ---------------------------------------------------------------------------
// map decode / width holes / idempotence
// ---------------------------------------------------------------------------

#[test]
// origin: sh7042_map.hxx r8/r16 pair (186-199 vs 413-419): the two CMCOR0 byte
// reads reassemble the r16 word (CMCOR is read-only, no side effects).
fn r16_matches_byte_pair() {
    let mut d = fresh();
    d.cmt_w16(CMCOR0, 0x1234);
    assert_eq!(d.cmt_r16(CMCOR0), 0x1234);
    assert_eq!(d.cmt_r8(CMCOR0), 0x12); // map:192 high
    assert_eq!(d.cmt_r8(CMCOR0 + 1), 0x34); // map:193 low
}

#[test]
// origin: sh7042_map.hxx w8 arms (687-700): high/low byte writes are split by
// mask and combine without clobbering the other byte.
fn w8_bytes_combine_without_clobber() {
    let mut d = fresh();
    d.cmt_w8(CMCSR0, 0xab); // high byte (map:689)
    assert_eq!(d.cmt_r16(CMCSR0), 0xab00);
    d.cmt_w8(CMCSR0 + 1, 0xcd); // low byte (map:690)
    assert_eq!(d.cmt_r16(CMCSR0), 0xabcd);
}

#[test]
// origin: sh7042.rs range is 83d0-83dd; the case-set default arm returns 0 for
// anything else (bus-miss behavior, membus.h:70).
fn case_set_miss_returns_zero() {
    let mut d = fresh();
    assert_eq!(d.cmt_r8(0xffff83de), 0);
    assert_eq!(d.cmt_r16(0xffff83de), 0);
}

#[test]
// origin: src/mame/cpu/sh_cmt.cpp:95-100 — cmcnt0_r runs catch_up THEN
// cnt_update at the SAME cycle; the ruler is idempotent within one read time.
fn cmcnt_double_read_same_time_is_idempotent() {
    let mut d = fresh();
    start_ch0(&mut d, 0x00ff, 0);
    d.set_cpu_now(100);
    let a = d.cmcnt0_r();
    let b = d.cmcnt0_r();
    assert_eq!(a, b);
    assert_eq!(a, 12); // 255 - ((2048-100-1)>>3)
}
