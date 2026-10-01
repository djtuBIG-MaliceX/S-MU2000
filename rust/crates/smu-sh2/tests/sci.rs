//! Hand-derived vectors for `src/mame/cpu/sh_sci.cpp` (ledger row
//! `periph: sci`, session F4). Every expectation cites its disk origin —
//! reset values (device_reset :345-368), integer divider math (:255-256),
//! clock-mode matrix (:258-275), SCR enable-bit x SSR status-bit interrupt
//! matrix (:161-179), ssr_w mask semantics (:207-223), async RX/TX bit
//! timing (16 events/bit, mid-bit sampling :697-759/:528-614), TE/RE
//! gating (:209,:139/:389), error paths (FER/PER/ORER :668-695), sync
//! pairing (:516-525/:656-659), and the bus seam maps (:15-26/328-333/
//! 520-531/796-801).
//!
//! MIDI-relevant: `midi_ready`==RE (mu2000.h:112 <- sh_sci.h:65),
//! `rx_byte_pending`==RDRF (mu2000.h:180 <- sh_sci.cpp:134), fast_midi
//! direct feed = receive_byte (mu2000.cpp:1377-1382), 31250 bps bit-bang =
//! do_rx_w (mu2000.cpp:1397-1409). MIDI timing: 28 MHz / (16*31250) ->
//! divider 56 = brr 27, cks 0 (:255-256).

use smu_sh2::periph::sci::*;
use smu_sh2::sh7042::Sh7042Peripherals;

/// ctor + device_start + device_reset for both die SCIs (sh7042.cpp:237-238
/// vector wiring). Reset fires do_sci_clk/do_sci_tx(:366-367): pin/irq
/// rings are drained here so each test starts from a clean slate.
fn fresh() -> Sh2SciPair {
    let mut p = Sh2SciPair::new();
    p.device_reset();
    let mut pins: Vec<(u8, bool)> = Vec::new();
    let mut irqs: Vec<i32> = Vec::new();
    for s in p.sci.iter_mut() {
        s.poll_pin(&mut pins);
        s.drain_irqs(&mut irqs);
    }
    p
}

// origin: sh_sci.cpp:345-368 (device_reset) + ctor :65-75
#[test]
fn t01_reset_values() {
    let p = fresh();
    let s = &p.sci[0];
    assert_eq!(s.m_rdr, 0x00); // :347
    assert_eq!(s.m_tdr, 0xff); // :348
    assert_eq!(s.m_smr, 0x00); // :349
    assert_eq!(s.m_scr, 0x00); // :350
    assert_eq!(s.m_ssr, 0x84); // :351 TDRE|TEND
    assert_eq!(s.m_brr, 0xff); // :352
    assert_eq!(s.m_rsr, 0x00); // :353
    assert_eq!(s.m_tsr, 0xff); // :354
    assert_eq!(s.m_tx_state, ST_IDLE); // :357
    assert_eq!(s.m_rx_state, ST_IDLE); // :358
    assert_eq!(s.m_clock_state, 0); // :359
    assert_eq!(s.m_clock_mode, INTERNAL_ASYNC); // :360
    assert_eq!(s.m_clock_event, 0); // :361
    assert!(s.m_ext_clock_value); // :363
    assert!(s.m_rx_value); // ctor :71 (device_reset does NOT reset it)
    assert_eq!(s.m_divider, 512); // :255-256 2<<(0) * (0xff+1)
    assert_eq!(s.m_id, 0); // sh7042.cpp:237
    assert_eq!((s.m_eri_int, s.m_rxi_int, s.m_txi_int, s.m_tei_int), (128, 129, 130, 131)); // :237
    let s1 = &p.sci[1];
    assert_eq!((s1.m_id, s1.m_eri_int, s1.m_rxi_int, s1.m_txi_int, s1.m_tei_int), (1, 132, 133, 134, 135)); // sh7042.cpp:238
    // device_reset fired clk+tx high (:366-367):
    assert!(s.tx_pin && s.clk_pin);
}

// origin: sh7042_map.hxx:15-26/520-524 (r8/w8 case-sets), scmr (:242-251)
#[test]
fn t02_r8_w8_seam_maps() {
    let mut p = fresh();
    // reset reads through the seam
    assert_eq!(p.sci_r8(0, 0xffff_81a0), 0x00); // smr
    assert_eq!(p.sci_r8(0, 0xffff_81a1), 0xff); // brr
    assert_eq!(p.sci_r8(0, 0xffff_81a2), 0x00); // scr
    assert_eq!(p.sci_r8(0, 0xffff_81a3), 0xff); // tdr
    assert_eq!(p.sci_r8(0, 0xffff_81a4), 0x84); // ssr
    assert_eq!(p.sci_r8(0, 0xffff_81a5), 0x00); // rdr
    assert_eq!(p.sci_r8(1, 0xffff_81b4), 0x84); // sci1 (map:25)
    p.sci_w8(0, 0xffff_81a3, 0x5a); // tdr_w
    assert_eq!(p.sci_r8(0, 0xffff_81a3), 0x5a);
    // RDR hole: 81a5 has NO w8 arm on disk (map:520-524 stops at 81a4)
    p.sci_w8(0, 0xffff_81a5, 0xee);
    assert_eq!(p.sci_r8(0, 0xffff_81a5), 0x00);
    // scmr: accepted, never stored, reads 0 (sh_sci.cpp:242-250)
    p.sci[0].scmr_w(0xa5);
    assert_eq!(p.sci[0].scmr_r(), 0x00); // :250
    // r16 byte pair (map:329)
    p.sci_w8(0, 0xffff_81a3, 0x33);
    assert_eq!(p.sci_r16(0, 0xffff_81a2), (0x00 << 8) | 0x33);
}

// origin: sh_sci.cpp:255-256 — divider = (2 << (2*cks)) * (brr+1), integer
#[test]
fn t03_brr_divider_vectors() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    for (cks, base) in [(0u8, 2u64), (1, 8), (2, 32), (3, 128)] {
        s.smr_w(cks); // :84 then clock_update :95
        s.brr_w(0); // :256 *1
        assert_eq!(s.m_divider, base); // :255
        s.brr_w(27); // MIDI: 56 = 28e6/(16*31250)
        assert_eq!(s.m_divider, base * 28);
    }
    s.smr_w(3);
    s.brr_w(0xff);
    assert_eq!(s.m_divider, 128 * 256); // :255-256 max frame
    s.smr_w(0);
    s.brr_w(27);
    assert_eq!(s.m_divider, 56); // the MIDI divider used downstream
}

// origin: sh_sci.cpp:258-270 (mode select matrix)
#[test]
fn t04_clock_mode_matrix() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.smr_w(0);
    s.scr_w(0);
    assert_eq!(s.m_clock_mode, INTERNAL_ASYNC); // :269
    s.scr_w(SCR_CKE0);
    assert_eq!(s.m_clock_mode, INTERNAL_ASYNC_OUT); // :267
    s.scr_w(SCR_CKE1);
    assert_eq!(s.m_clock_mode, EXTERNAL_ASYNC); // :265
    s.smr_w(SMR_CA);
    s.scr_w(0);
    assert_eq!(s.m_clock_mode, INTERNAL_SYNC_OUT); // :262
    s.scr_w(SCR_CKE1);
    assert_eq!(s.m_clock_mode, EXTERNAL_SYNC); // :260
    s.smr_w(0);
    s.scr_w(SCR_CKE0 | SCR_CKE1); // CKE=3 takes the CKE1 branch :264
    assert_eq!(s.m_clock_mode, EXTERNAL_ASYNC);
}

// origin: sh_sci.cpp:272-275 (external + known period -> RATE variants)
#[test]
fn t05_clock_mode_rate_variants() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    // period never on this build -> RATE unreachable (:272/274 false)
    s.smr_w(0);
    s.scr_w(SCR_CKE1);
    assert_eq!(s.m_clock_mode, EXTERNAL_ASYNC); // stays
    s.set_external_clock_period_ratios(1.0 / 2.0, 2.0); // :77-80 + :315-316
    assert!(!s.ext_period_never);
    s.scr_w(SCR_CKE1); // clock_update re-run via scr_w :163
    assert_eq!(s.m_clock_mode, EXTERNAL_RATE_ASYNC); // :273
    s.smr_w(SMR_CA);
    s.scr_w(SCR_CKE1);
    assert_eq!(s.m_clock_mode, EXTERNAL_RATE_SYNC); // :275
    s.smr_w(0);
    s.scr_w(0);
    assert_eq!(s.m_clock_mode, INTERNAL_ASYNC); // internal unaffected :269
}

// origin: sh_sci.cpp:165-168 (RE falling -> rx idle + clock_stop)
#[test]
fn t06_scr_re_off_kills_rx() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    s.smr_w(0);
    s.brr_w(27);
    s.scr_w(SCR_RE); // rx_start -> ST_START (line high: no clock :661-664)
    s.do_rx_w(0); // :389-390 start sampling
    assert_eq!(s.m_clock_state, CLK_RX);
    assert_eq!(s.m_clock_event, 56); // (0/56+1)*56 :476
    let mut irqs: Vec<i32> = Vec::new();
    s.drain_irqs(&mut irqs);
    s.take_resched();
    s.scr_w(0x00); // RE falling
    assert_eq!(s.m_rx_state, ST_IDLE); // :166
    assert_eq!(s.m_clock_state, 0); // clock_stop :499-503
    assert_eq!(s.m_clock_event, 0); // :501
    assert_eq!(s.m_clock_step, 0); // :502
    assert!(s.take_resched()); // :505
}

// origin: sh_sci.cpp:170-171 + 651-666 (RE rising -> rx_start, async)
#[test]
fn t07_scr_re_on_starts_st_start() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.smr_w(0);
    s.brr_w(27);
    s.scr_w(SCR_RE);
    assert_eq!(s.m_rx_state, ST_START); // :661
    assert_eq!(s.m_rx_bit, 1); // :662
    assert_eq!(s.m_clock_state, 0); // line high -> no clock :663
    assert_eq!(s.m_rx_parity, 1); // OE off :653
    assert_eq!(s.m_rsr, 0); // :654
    assert!(p.midi_ready(0)); // mu2000.h:112 / sh_sci.h:65
    assert!(!p.midi_ready(1)); // sci1 still RE off
}

// origin: sh_sci.cpp:149-180 — the enable-bit-delta x status-bit matrix
#[test]
fn t08_scr_irq_matrix() {
    let mut irqs: Vec<i32> = Vec::new();
    // TIE rising + TDRE (reset ssr has it) -> txi (sci0 = 130)
    let mut p = fresh();
    p.sci[0].scr_w(SCR_TIE);
    assert_eq!(p.sci[0].drain_irqs(&mut irqs), 1); // :172-173
    assert_eq!(irqs[0], 130);
    // TIE again with no delta -> nothing
    p.sci[0].scr_w(SCR_TIE);
    assert_eq!(p.sci[0].drain_irqs(&mut irqs), 0); // delta==0 :172
    // TEIE rising + TEND -> tei 131
    irqs.clear();
    let mut p = fresh();
    p.sci[0].scr_w(SCR_TEIE);
    p.sci[0].drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![131]); // :174-175
    // RIE rising + RDRF -> rxi 129 (fill RDR first with RIE still off)
    irqs.clear();
    let mut p = fresh();
    p.sci[0].scr_w(SCR_RE);
    p.sci[0].receive_byte(0x77); // RIE off -> no irq (:145)
    assert_eq!(p.sci[0].drain_irqs(&mut irqs), 0);
    p.sci[0].scr_w(SCR_RE | SCR_RIE);
    p.sci[0].drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![129]); // :176-177
    // RIE rising + recv error -> eri 128 (:178-179; FER set directly —
    // natural path covered by t16)
    irqs.clear();
    let mut p = fresh();
    p.sci[0].scr_w(SCR_RE);
    p.sci[0].m_ssr |= SSR_FER;
    p.sci[0].scr_w(SCR_RE | SCR_RIE);
    p.sci[0].drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![128]);
    // sci1 vectors (sh7042.cpp:238)
    irqs.clear();
    let mut p = fresh();
    p.sci[1].scr_w(SCR_TIE);
    p.sci[1].drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![134]); // txi sci1
}

// origin: sh_sci.cpp:207-223 — TE-off keeps TDRE (TX can never start)
#[test]
fn t09_ssr_te_off_keeps_tdre() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.scr_w(0x00); // TE off
    s.ssr_w(0x00); // try to clear TDRE
    assert!(s.m_ssr & SSR_TDRE != 0); // :209-211 forced set
    assert_eq!(s.m_tx_state, ST_IDLE); // no tx_start (:218 guard fails)
    assert_eq!(s.m_clock_state, 0);
    assert_eq!(s.m_clock_event, 0);
    assert!(s.m_ssr & SSR_TEND != 0); // :213 not cleared (data keeps TDRE)
}

// origin: sh_sci.cpp:207-223 + 508-526 — TE on, clear TDRE -> tx_start
#[test]
fn t10_ssr_kick_tx_start() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    s.smr_w(0);
    s.brr_w(27); // step 56
    s.scr_w(SCR_TE);
    s.tdr_w(0x3c);
    let mut irqs: Vec<i32> = Vec::new();
    s.drain_irqs(&mut irqs);
    s.ssr_w(0x00); // clear TDRE (TE on -> allowed)
    // :213-214 clears TEND; :215 mask drops TDRE; :510 re-sets TDRE only
    assert_eq!(s.m_ssr, SSR_TDRE); // TEND gone (0x84 -> 0x80)
    assert_eq!(s.m_tx_state, ST_START); // :520
    assert_eq!(s.m_tx_bit, 1); // :521
    assert_eq!(s.m_tsr, 0x3c); // :511 shadow copy
    assert_eq!(s.m_tx_parity, 1); // OE off :512
    assert_eq!(s.m_clock_state, CLK_TX); // :523
    assert_eq!(s.m_tx_clock_counter, 15); // :459
    assert_eq!(s.m_clock_step, 56); // :474
    assert_eq!(s.m_clock_event, 56); // :476 (0/56+1)*56
    assert!(s.take_resched()); // :477
    assert_eq!(s.drain_irqs(&mut irqs), 0); // TIE off -> no txi :514
    let mut pins: Vec<(u8, bool)> = Vec::new();
    s.poll_pin(&mut pins);
    assert!(pins.is_empty()); // start bit only fires on first tick
}

// origin: sh_sci.cpp:215 — ssr_w mask: MPBT follows the write; bits
// cleared in data drop unless the data|TEND|MPB|MPBT mask keeps them
#[test]
fn t11_ssr_mask_semantics() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    // ssr=0x84. write 0x85 (TDRE kept, MPBT=1):
    // ((0x84 & ~0x01)|0x01) & (0x85|0x07) = 0x85 & 0x87 = 0x85
    s.ssr_w(SSR_TDRE | SSR_MPBT);
    assert_eq!(s.m_ssr, 0x85);
    // write 0x84: MPBT follows to 0; TEND survives via the OR mask:
    // ((0x85&~1)|0)&(0x84|7) = 0x84 & 0x87 = 0x84
    s.ssr_w(SSR_TDRE);
    assert_eq!(s.m_ssr, 0x84);
    // clearing RDRF works (bit6 is NOT in the keep set):
    // data=0x80: ((0x84&~1)|0)&(0x80|7)=0x84&0x87=0x84 -> wait RDRF(0x40)
    // is dropped by the mask -> 0x84 & ~0x40? recompute exactly:
    // (0x84 & !0x01) = 0x84; | (0x80 & 0x01)=0 -> 0x84; & (0x80|0x07)=0x87
    // -> 0x84. RDRF was never set here; set it first, then clear:
    s.m_ssr |= SSR_RDRF; // 0xC4
    s.ssr_w(SSR_TDRE); // mask 0x87 -> RDRF cleared by :215
    assert_eq!(s.m_ssr, 0x84);
    assert_eq!(s.m_ssr & SSR_RDRF, 0);
}

// origin: sh_sci.cpp:127-130 (rx_can_accept truth table)
#[test]
fn t12_rx_can_accept_matrix() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    assert!(!s.rx_can_accept()); // RE off :129
    s.scr_w(SCR_RE);
    assert!(s.rx_can_accept());
    s.m_ssr |= SSR_RDRF;
    assert!(!s.rx_can_accept()); // RDRF
    s.m_ssr &= !SSR_RDRF;
    for e in [SSR_ORER, SSR_PER, SSR_FER] {
        s.m_ssr |= e;
        assert!(!s.rx_can_accept()); // has_recv_error :122-125
        s.m_ssr &= !e;
    }
    assert!(s.rx_can_accept());
}

// origin: sh_sci.cpp:137-147 — RE off (and RDRF-full) blocks the feed
#[test]
fn t13_receive_byte_blocked() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.receive_byte(0x11); // RE off
    assert_eq!(s.m_rdr, 0); // :139-140 drop
    assert!(!s.rx_byte_pending()); // :134
    s.scr_w(SCR_RE);
    s.receive_byte(0x11);
    assert_eq!(s.m_rdr, 0x11); // :141
    assert!(s.rx_byte_pending()); // mu2000.h:180 view
    p.receive_byte(0, 0x22); // RDRF set -> drop
    assert_eq!(p.sci[0].m_rdr, 0x11);
    // pair-level mu2000.h views forward by port:
    assert!(p.midi_ready(0)); // :112
    assert!(p.rx_byte_pending(0)); // :180/188/199
    assert!(!p.midi_ready(1)); // sci1 untouched
    assert!(!p.rx_byte_pending(1));
    assert!(!p.rx_byte_pending(9)); // out-of-range port reads false
    p.do_rx_w(0, 1); // pair-level bit-bang forward (:1397/1407/1409 shape)
    assert!(p.sci[0].m_rx_value);
}

// origin: sh_sci.cpp:137-147 + sh_sci.h:65 — fast_midi feed
// (mu2000.cpp:1377-1382): byte direct to RDR + RDRF + rxi when RIE
#[test]
fn t14_receive_byte_fast_midi() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    let mut irqs: Vec<i32> = Vec::new();
    s.scr_w(SCR_RE | SCR_RIE); // both at once: delta RIE with RDRF clear
    // -> no spurious rxi (:176; t08 proves the firing case)
    assert_eq!(s.drain_irqs(&mut irqs), 0);
    s.receive_byte(0x99);
    assert_eq!(s.m_rdr, 0x99); // :141
    assert_eq!(s.m_ssr & SSR_RDRF, SSR_RDRF); // :142
    assert_eq!(s.drain_irqs(&mut irqs), 1); // :145-146
    assert_eq!(irqs, vec![129]); // rxi = sci0 RXI (sh7042.cpp:237)
    assert!(s.rx_enabled()); // RE -> midi_ready (mu2000.h:112)
    assert!(s.rx_byte_pending()); // mu2000.h:188/199
}

/// shared async-RX bit driver. Line idles high; do_rx_w(0) supplies the
/// start bit and kicks the clock (:389-390); 16 ticks/bit (cks0/brr27 ->
/// step 56), sample at counter==8 = tick 9+16k (:699-702). The level of
/// wire slot k (k=0 is the FIRST DATA slot, LSB-first; then optional
/// parity, then stop) is applied just AFTER tick 16+16k, i.e. at ticks
/// 17/33/49... — post-tick counter is 0 there, so the :385 re-phase
/// (fires at counter 1/15) is never tripped. Slot k is sampled at tick
/// 9+16*(k+1). Returns ticks run.
fn drive_rx(s: &mut Sh2Sci, slots: &[bool], stop_on: impl Fn(&Sh2Sci) -> bool) -> u64 {
    s.do_rx_w(0); // start bit low (:397 mu2000 equivalent)
    let mut n: u64 = 0;
    loop {
        let ev = s.m_clock_event;
        assert_ne!(ev, 0, "rx clock died at tick {n}");
        let e = s.internal_update(ev); // seam also funnels current_time here
        n += 1;
        assert_eq!(e, s.m_clock_event);
        if n >= 17 && (n - 1) % 16 == 0 {
            let k = ((n - 1) / 16 - 1) as usize; // wire slot starting now
            if k < slots.len() {
                s.do_rx_w(i32::from(slots[k]));
            }
        }
        if stop_on(s) {
            return n;
        }
        assert!(n < 400, "rx watchdog");
    }
}

// golden: full 8N1 byte in (data 0x65) -> RDRF + RDR + rxi; reading RDR
// does NOT clear RDRF on disk (:231-240); RE stays armed (:689-690) and a
// high line in ST_START ends in the false-start clock_stop (:709-712)
#[test]
fn t15_rx_full_byte_async() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    s.smr_w(0); // 8N1, cks0
    s.brr_w(27); // step 56
    s.scr_w(SCR_RE | SCR_RIE);
    assert_eq!(s.m_clock_state, 0); // waiting high
    let data: u8 = 0x65;
    let mut slots: Vec<bool> = Vec::new();
    for i in 0..8 {
        slots.push((data >> i) & 1 != 0);
    }
    slots.push(true); // stop
    let n = drive_rx(s, &slots, |s| s.m_ssr & SSR_RDRF != 0);
    assert_eq!(n, 153); // stop sampled at tick 9+16*9 (:701 on the :476 grid)
    assert_eq!(s.m_rdr, 0x65); // :680 (LSB-first :719-723 reconstructs byte)
    assert!(s.m_ssr & SSR_RDRF != 0); // :678
    let mut irqs: Vec<i32> = Vec::new();
    assert_eq!(s.drain_irqs(&mut irqs), 1);
    assert_eq!(irqs, vec![129]); // rxi via rx_done :687
    assert!(s.take_resched()); // :441 fired on every tick
    assert_eq!(s.rdr_r(), 0x65); // :239 — read keeps RDRF (no DMA path)
    assert!(s.m_ssr & SSR_RDRF != 0);
    assert_eq!(s.m_rx_state, ST_START); // re-armed :661 (back-to-back:
    // clock_start early-return :455-456 kept the old event running)
    // line still high -> the next sample point (tick 169) false-stops:
    let mut guard = 0;
    while s.m_clock_state & CLK_RX != 0 {
        let ev = s.m_clock_event;
        assert_ne!(ev, 0);
        s.internal_update(ev);
        guard += 1;
        assert!(guard < 40);
    }
    assert_eq!(guard, 16); // exactly to tick 169 (the ST_STOP-grid sample)
    assert_eq!(s.m_clock_event, 0); // :711 clock_stop
    assert_eq!(s.m_rx_state, ST_START); // :711-712 (IDLE only via scr_w)
}

// framing error: stop bit low -> FER, NO RDRF / no RDR update, eri; RX
// stops (error blocks rx_start :689). Disk has NO explicit "break" feature
// (rg 0 hits) — a held-low stop is the wire-level stand-in.
#[test]
fn t16_rx_framing_error_break_like() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    s.smr_w(0);
    s.brr_w(27);
    s.scr_w(SCR_RE | SCR_RIE);
    let data: u8 = 0x3c;
    let mut slots: Vec<bool> = Vec::new();
    for i in 0..8 {
        slots.push((data >> i) & 1 != 0);
    }
    slots.push(false); // stop held LOW
    let n = drive_rx(s, &slots, |s| s.m_clock_state & CLK_RX == 0);
    assert_eq!(n, 153); // stop sample -> :748 -> rx_done -> :692 stops
    assert!(s.m_ssr & SSR_FER != 0); // :749
    assert_eq!(s.m_ssr & SSR_RDRF, 0); // :670 skips delivery
    assert_eq!(s.m_rdr, 0); // :347 reset value stands
    assert_eq!(s.m_rx_state, ST_IDLE); // :693
    let mut irqs: Vec<i32> = Vec::new();
    s.drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![128]); // eri only (:684-685)
    assert_eq!(s.m_clock_event, 0); // clock_stop :501
}

// parity error (PE set, OE odd): with the disk :719/^722 quirk data bits
// never flip parity, so a ZERO parity bit leaves parity=1 -> PER at the
// stop sample (:750-751), eri, no byte
#[test]
fn t17_rx_parity_error() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    s.smr_w(SMR_PE); // parity enabled, OE off (:653 inits parity 1)
    s.brr_w(27);
    s.scr_w(SCR_RE | SCR_RIE);
    // start + eight 0 data bits + parity 0 (:719/^722 keep parity=1) + stop
    let slots = vec![false, false, false, false, false, false, false, false, false, true];
    let n = drive_rx(s, &slots, |s| s.m_clock_state & CLK_RX == 0);
    assert_eq!(n, 169); // stop sample 9+16*10 (parity slot inserted)
    assert!(s.m_ssr & SSR_PER != 0); // :751
    assert_eq!(s.m_ssr & SSR_RDRF, 0); // :671-673 error wins
    assert_eq!(s.m_rx_state, ST_IDLE); // :693
    let mut irqs: Vec<i32> = Vec::new();
    s.drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![128]); // eri :684-685
}

// overrun: RDRF already full when a second byte lands -> ORER, RDR keeps
// byte 1, rxi (byte 1) then eri (rx_done error path) (:674-676, :683-688)
#[test]
fn t18_rx_overrun() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    s.smr_w(0);
    s.brr_w(27);
    s.scr_w(SCR_RE | SCR_RIE);
    s.receive_byte(0xab); // fills RDR -> rxi now (:145)
    let mut irqs: Vec<i32> = Vec::new();
    s.drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![129]);
    let data: u8 = 0x12;
    let mut slots: Vec<bool> = Vec::new();
    for i in 0..8 {
        slots.push((data >> i) & 1 != 0);
    }
    slots.push(true);
    let n = drive_rx(s, &slots, |s| s.m_clock_state & CLK_RX == 0);
    assert_eq!(n, 153); // rx_done -> error -> :692 stops (RE on, error)
    assert!(s.m_ssr & SSR_ORER != 0); // :675
    assert!(s.m_ssr & SSR_RDRF != 0);
    assert_eq!(s.m_rdr, 0xab); // :674-676 — old byte wins
    let mut irqs: Vec<i32> = Vec::new();
    s.drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![128]); // eri (:684) — rxi suppressed on error
}

// golden: full 8N1 byte out (tdr 0x3c). Pin callback sequence per
// :542-614 — start low, 8 LSB-first, stop high, idle high on LAST_TICK.
// INTERNAL_ASYNC -> no clk output (:535/:538 need INTERNAL_ASYNC_OUT).
#[test]
fn t19_tx_full_byte_async() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    s.smr_w(0);
    s.brr_w(27);
    s.scr_w(SCR_TE);
    s.tdr_w(0x3c);
    s.ssr_w(0x00); // kick
    let mut pins: Vec<(u8, bool)> = Vec::new();
    let mut n: u64 = 0;
    while s.m_clock_state & CLK_TX != 0 {
        let ev = s.m_clock_event;
        assert_ne!(ev, 0, "tx clock died at tick {n}");
        s.internal_update(ev);
        n += 1;
        assert!(n < 200, "tx watchdog");
    }
    s.poll_pin(&mut pins);
    // steps on ticks 1,17,...: start(0) d0..d7 = 0,0,1,1,1,1,0,0 stop(1),
    // then LAST_TICK idle-high (:596-608)
    let levels: Vec<bool> = pins.iter().map(|(_, v)| *v).collect();
    assert_eq!(
        levels,
        vec![false, false, false, true, true, true, true, false, false, true, true]
    );
    assert!(pins.iter().all(|(c, _)| c & PIN_TX != 0)); // tx only, no clk
    assert_eq!(n, 161); // last step = stop(145) + LAST_TICK(161)
    assert_eq!(s.m_tx_state, ST_IDLE); // :597
    assert_eq!(s.m_ssr, SSR_TDRE | SSR_TEND); // :510 + :601 (TEND was
    // cleared by the :213 kick, re-set at :601)
    assert!(s.take_resched()); // clock_stop :505
    let mut irqs: Vec<i32> = Vec::new();
    assert_eq!(s.drain_irqs(&mut irqs), 0); // no TIE/TEIE set
}

// TIE: fires on scr_w rising (t08) AND every tx_start (:514-515); TEIE
// rising fires once TEND exists (:174-175)
#[test]
fn t20_tx_irq_matrix() {
    let mut irqs: Vec<i32> = Vec::new();
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    s.smr_w(0);
    s.brr_w(27);
    s.scr_w(SCR_TE | SCR_TIE); // TIE rising with TDRE set -> txi (:172)
    s.drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![130]);
    s.ssr_w(0x00); // kick -> tx_start fires txi again (:515)
    irqs.clear();
    s.drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![130]);
    while s.m_clock_state & CLK_TX != 0 {
        let ev = s.m_clock_event;
        s.internal_update(ev);
    }
    // enable TEIE after the fact: TEND now set, so the rising edge fires
    irqs.clear();
    s.scr_w(SCR_TE | SCR_TIE | SCR_TEIE);
    s.drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![131]); // :174-175 rising + TEND
}

// TE gating at :606 — TDRE cleared while ST_LAST_TICK is pending (write
// happens while TE still on, :210 would forbid it otherwise) + TE turned
// off: the LAST_TICK path must NOT re-arm the transmitter.
#[test]
fn t21_te_off_blocks_restart() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    s.smr_w(0);
    s.brr_w(27);
    s.scr_w(SCR_TE);
    s.tdr_w(0xff);
    s.ssr_w(0x00); // kick (TE on)
    let mut n: u64 = 0;
    let mut wrote = false;
    while s.m_clock_state & CLK_TX != 0 {
        let ev = s.m_clock_event;
        s.internal_update(ev);
        n += 1;
        assert!(n < 200);
        if !wrote && n == 145 && s.m_tx_state == ST_LAST_TICK {
            s.ssr_w(0x00); // TE still on -> TDRE clearable; state is
            // ST_LAST_TICK so the :218 kick guard (IDLE only) does not fire
            s.scr_w(0x00); // now TE off
            wrote = true;
        }
    }
    assert!(wrote);
    assert_eq!(n, 161); // one byte only — :606 (TE off) blocked rearm
    assert_eq!(s.m_tx_state, ST_IDLE); // :597
    assert_eq!(s.m_clock_state, 0);
    assert_eq!(s.m_ssr, SSR_TEND); // :601 set TEND; TDRE stays cleared
    // (TE off only forces TDRE inside ssr_w :210-211, not here)
}

// back-to-back chaining at :587-588 — clearing TDRE before the stop-bit
// step chains a SECOND byte without any new tdr write; second byte uses
// the same tdr (0xff) and runs to a clean LAST_TICK
#[test]
fn t21b_tx_back_to_back_chain() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    s.smr_w(0);
    s.brr_w(27);
    s.scr_w(SCR_TE);
    s.tdr_w(0xff);
    s.ssr_w(0x00); // kick byte 1
    let mut n: u64 = 0;
    let mut cleared = false;
    while s.m_clock_state & CLK_TX != 0 {
        let ev = s.m_clock_event;
        if n == 144 {
            // between d7-step (129) and stop-step (145): state ST_STOP
            assert_eq!(s.m_tx_state, ST_STOP);
            s.ssr_w(0x00); // clear TDRE (TE on; :218 no-kick: not IDLE)
            cleared = true;
        }
        s.internal_update(ev);
        n += 1;
        assert!(n < 400);
    }
    assert!(cleared);
    assert_eq!(n, 321); // 161 + byte 2 (start 161, bits..289, stop 305,
    // LAST_TICK 321; :587 fired tx_start at 145, :606 does NOT fire at 321
    // because :510 re-set TDRE for byte 2)
    assert_eq!(s.m_tx_state, ST_IDLE);
    assert_eq!(s.m_clock_state, 0);
    assert_eq!(s.m_ssr, SSR_TDRE | SSR_TEND); // :510 + :601
    let mut pins: Vec<(u8, bool)> = Vec::new();
    s.poll_pin(&mut pins);
    // byte 1 has NO idle-high event (:587 chained before LAST_TICK's :600):
    // start(0) + 8x1 + stop(1) = 10, then byte 2 complete = 11 -> 21
    assert_eq!(pins.len(), 21);
    assert_eq!(pins[0], (PIN_TX, false)); // byte 1 start
    assert!(pins[1..10].iter().all(|(_, v)| *v)); // data + stop high
    assert_eq!(pins[10], (PIN_TX, false)); // byte 2 start (:588 chain)
    assert!(pins[11..21].iter().all(|(_, v)| *v)); // data+stop+idle high
}

// RE gating: RE off -> do_rx_w never arms a clock (:389 needs
// rx_state != IDLE, and scr_w RE-off forces IDLE :165-168)
#[test]
fn t22_re_off_blocks_rx() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.do_rx_w(0); // line low but RE off -> rx_state IDLE
    assert_eq!(s.m_clock_state, 0);
    assert!(!s.m_rx_value); // :388 still tracks the pin
    assert!(!s.rx_enabled()); // RE clear
    assert_eq!(s.m_rx_state, ST_IDLE);
}

// sync mode (CA + TE + RE): is_sync_start (:117-120) blocks the solo
// rx_start (:170/:221); tx_start pairs rx_start (:524-525 -> :656-659,
// sharing the TX event via :464). Byte 1 (input high) -> 0xff + rxi;
// rx_done does NOT re-arm in sync mode (:689 !is_sync_start fails ->
// :692-693) — sync RX chains only from the next tx_start
#[test]
fn t23_sync_mode_pairing() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    let mut irqs: Vec<i32> = Vec::new();
    s.cpu_now = 0;
    s.smr_w(SMR_CA); // CKE=0 -> INTERNAL_SYNC_OUT :262
    assert_eq!(s.m_clock_mode, INTERNAL_SYNC_OUT);
    s.brr_w(27);
    s.scr_w(SCR_TE | SCR_RE | SCR_RIE);
    assert!(s.is_sync_start()); // :117-120
    assert_eq!(s.m_rx_state, ST_IDLE); // RE rising refused (:170)
    s.ssr_w(SSR_TDRE); // TDRE stays -> no kick (:218)
    assert_eq!(s.m_clock_state, 0);
    s.ssr_w(0x00); // kick tx
    assert_eq!(s.m_tx_state, ST_BIT); // :517 sync: straight to bits
    assert_eq!(s.m_tx_bit, 8);
    assert_eq!(s.m_rx_state, ST_BIT); // :657 from the tx_start pairing
    assert_eq!(s.m_rx_bit, 8);
    assert_eq!(s.m_clock_state, CLK_TX | CLK_RX); // shared clock (:464)
    assert_eq!(s.m_clock_event, 56);
    // run until the clock dies (ORER stops both at the 2nd byte)
    let mut n: u64 = 0;
    while s.m_clock_state != 0 {
        let ev = s.m_clock_event;
        assert_ne!(ev, 0);
        s.internal_update(ev);
        n += 1;
        assert!(n < 80, "sync watchdog");
    }
    // both counters start at 15 (:459/:461) and toggle every tick, so TX
    // steps on odd ticks (1..15 = 8 bits) and RX samples on EVEN ticks
    // (:769). Byte 1 completes at tick 16 -> rx_done: the :689 re-arm test
    // contains !is_sync_start() which is FALSE here -> else :692-693:
    // RX clock stops + IDLE (sync RX chains ONLY off the next tx_start
    // :524-525; TDRE is set so :642-643 does not re-kick TX either).
    // TX then ends at its 9th step (tick 17, :633 branch) -> clock 0.
    assert_eq!(n, 17);
    assert_eq!(s.m_rdr, 0xff); // :780-782 all-high input line
    assert_eq!(s.m_rx_state, ST_IDLE); // :693
    assert!(s.m_ssr & SSR_RDRF != 0); // :678
    assert_eq!(s.m_ssr, SSR_TDRE | SSR_RDRF | SSR_TEND); // :510+:678+:637
    s.drain_irqs(&mut irqs);
    assert_eq!(irqs, vec![129]); // single rxi (:687) — no second byte
    // TX bits (tsr=0xff) are all high repeats; clk outputs fire every half
    // tick in INTERNAL_SYNC_OUT (:624/:627 and :766-771 — BOTH active
    // directions fire the devcb, mamecompat.h:468)
    let mut pins: Vec<(u8, bool)> = Vec::new();
    s.poll_pin(&mut pins);
    let txev: Vec<bool> = pins.iter().filter(|(c, _)| c & PIN_TX != 0).map(|(_, v)| *v).collect();
    let cknev = pins.iter().any(|(c, _)| c & PIN_TX == 0);
    assert!(cknev); // sync-out clock toggles were recorded
    assert!(txev.len() >= 9); // 8 bit repeats + idle-high (:645, :636)
    assert!(txev.iter().all(|v| *v)); // all high (line was idle high)
    assert!(!txev.contains(&false));
}

// bus seam: 16-bit big-endian pairs (map:328-333/796-801) — high reg is
// the high byte; w16@81a4 hits ssr ONLY (rdr slot ignored); sci1 base
#[test]
fn t24_seam_16bit_pairing() {
    let mut p = fresh();
    p.sci_w16(0, 0xffff_81a0, (0x00 << 8) | 27); // smr=0, brr=27 (map:796)
    assert_eq!(p.sci[0].m_smr, 0x00);
    assert_eq!(p.sci[0].m_brr, 27);
    assert_eq!(p.sci[0].m_divider, 56); // two clock_update calls (:95,:108)
    p.sci_w16(0, 0xffff_81a2, (u16::from(SCR_RE) << 8) | 0x77); // scr+tdr :797
    assert_eq!(p.sci[0].m_scr, SCR_RE);
    assert_eq!(p.sci[0].m_tdr, 0x77);
    assert_eq!(p.sci_r16(0, 0xffff_81a0), 27); // map:328
    assert_eq!(p.sci_r16(0, 0xffff_81a2), (u16::from(SCR_RE) << 8) | 0x77); // map:329
    // ssr write with TE OFF (scr=RE only here): :209-211 force TDRE back
    // into `data`, so :213 never clears TEND and :215 keeps everything ->
    // 0x84 unchanged; low byte is the RDR hole (map:798)
    p.sci_w16(0, 0xffff_81a4, 0x00ff);
    assert_eq!(p.sci[0].m_ssr, SSR_TDRE | SSR_TEND); // :210 forced keep
    assert_eq!(p.sci[0].m_rdr, 0x00); // hole: 0xff written to rdr ignored
    assert_eq!(p.sci_r16(0, 0xffff_81a4), 0x8400); // (ssr<<8)|rdr map:330
    // sci1 base (map:331-333/799-801)
    p.sci_w16(1, 0xffff_81b0, (0x02 << 8) | 0x07);
    assert_eq!(p.sci[1].m_smr, 0x02);
    assert_eq!(p.sci[1].m_brr, 0x07);
    assert_eq!(p.sci[1].m_divider, 32 * 8); // :255-256 cks=2 (0x02&3), brr=7
    assert_eq!(p.sci_r16(1, 0xffff_81b0), (0x02 << 8) | 0x07);
}

// resched sticky flag: set at clock_start (:477), clock_stop (:505) and
// every tick with a pending event (:441); NOT set by plain reg writes while
// idle (smr_w/brr_w/scr_w only clock_update :95/:108/:163)
#[test]
fn t25_resched_contract() {
    let mut p = fresh();
    let s = &mut p.sci[0];
    s.cpu_now = 0;
    assert!(!s.take_resched());
    s.smr_w(0);
    s.brr_w(27);
    s.scr_w(SCR_TE);
    s.tdr_w(0x00);
    assert!(!s.take_resched()); // disk: no internal_update in those writers
    s.ssr_w(0x00); // clock_start -> resched (:477)
    assert!(s.take_resched());
    assert!(!s.take_resched()); // sticky-once
    s.internal_update(s.m_clock_event);
    assert!(s.take_resched()); // :441 event still pending
    while s.m_clock_state & CLK_TX != 0 {
        let ev = s.m_clock_event;
        s.internal_update(ev);
    }
    assert!(s.take_resched()); // :441 last tick or clock_stop :505
    assert!(!s.take_resched());
    assert_eq!(s.m_clock_event, 0); // no seam event after stop (:444)
}

// pin ring: EVERY do_sci_tx/do_sci_clk call is an entry even when the
// level repeats (mamecompat.h:466-470 fires the devcb unconditionally —
// mu2000::tx_line, mu2000.cpp:1239-1255, edge-detects itself); reset fires
// clk(1) then tx(1) (:366-367) from ctor-low pins
#[test]
fn t26_pin_ring_contract() {
    let mut p = Sh2SciPair::new(); // no device_reset yet: ctor pins false
    let mut pins: Vec<(u8, bool)> = Vec::new();
    assert_eq!(p.sci[0].poll_pin(&mut pins), 0); // nothing fired yet
    assert!(!p.sci[0].tx_pin && !p.sci[0].clk_pin); // ctor :71
    p.device_reset(); // :366-367 fire clk(1) then tx(1)
    assert_eq!(p.sci[0].poll_pin(&mut pins), 2);
    assert_eq!(pins, vec![(PIN_CLK, true), (PIN_TX, true)]); // :366 then :367
    pins.clear();
    // repeat-level fires: TX with tdr=0xff -> start(0), eight 1s, stop(1),
    // idle(1) — the last two high repeats both land (:584 + :600)
    p.sci[0].cpu_now = 0;
    p.sci[0].smr_w(0);
    p.sci[0].brr_w(27);
    p.sci[0].scr_w(SCR_TE);
    p.sci[0].tdr_w(0xff);
    p.sci[0].ssr_w(0x00);
    while p.sci[0].m_clock_state & CLK_TX != 0 {
        let ev = p.sci[0].m_clock_event;
        p.sci[0].internal_update(ev);
    }
    pins.clear();
    p.sci[0].poll_pin(&mut pins);
    let lv: Vec<bool> = pins.iter().map(|(_, v)| *v).collect();
    assert_eq!(
        lv,
        vec![false, true, true, true, true, true, true, true, true, true, true]
    ); // 11 events, no clk (INTERNAL_ASYNC)
    // drain/overflow counters start and stay clean
    assert_eq!(p.sci[0].irq_dropped, 0);
    assert_eq!(p.sci[0].pin_dropped, 0);
}
