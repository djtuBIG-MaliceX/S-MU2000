//! Row gate `USB host (M37640)` (M7): `UsbLine` offline vectors — the
//! bus-byte semantics of `usb_r`/`usb_w` (mu2000.cpp:1359-1378 incl. the
//! `a & 1` only rule the membus.h demotion relies on), the `usb_step`
//! byte grid / level-hold 受信あり / IRQ2 pulse order (:1317-1357) and
//! the F5-framed TX ring `usb_out_take` (:1380-1400). Constants are
//! disk-derived (USB_BYTE_CYCLES = 2800, mu2000.cpp:43); the recorder
//! plays `m_cpu->execute_set_input` exactly as `QIrq` does on the machine.
//!
//! Mounted via smu-machine/Cargo.toml `[[test]] path =
//! "../../tests/usb.rs"` — the boot_golden/midi_out pattern.

use smu_machine::usb::{UsbIrq, UsbLine, USB_BYTE_CYCLES, USB_TX_LIMIT};

#[derive(Default)]
struct IrqRec {
    calls: Vec<(i32, i8)>,
}
impl UsbIrq for IrqRec {
    fn set_input(&mut self, line: i32, state: i8) {
        self.calls.push((line, state));
    }
}
impl IrqRec {
    fn take(&mut self) -> Vec<(i32, i8)> {
        std::mem::take(&mut self.calls)
    }
}

// (0) the byte grid: 28 MHz / 10,000 byte/s = 2800 cycles exactly
#[test]
fn byte_grid_is_2800_cycles() {
    assert_eq!(USB_BYTE_CYCLES, 2800); // mu2000.cpp:43
    assert_eq!(USB_TX_LIMIT, 4096); // mu2000.h:1027 (shared TX_SIZE)
}

// (1) disk guard :1323 — OFF + empty rx + !have: NOTHING happens. This is
// the OFF-regression keystone: with no USB traffic neither IRQ line is
// touched and the TX grid never advances.
#[test]
fn guard_quiet_when_off_and_empty() {
    let mut u = UsbLine::new();
    let mut rec = IrqRec::default();
    u.step(1_000_000, false, false, &mut rec);
    assert!(rec.calls.is_empty()); // no execute_set_input at all
    assert_eq!(u.next, 0); // :1005 untouched
    assert_eq!(u.tx_next, 0); // :1010 NOT advanced (guard returns first)
    assert!(u.idle()); // mu2000.h:223
}

// (2) host ON, everything empty: TX pump still ticks — IRQ2 (vec 66)
// drops then rises once per grid (:1341 then :1353-1355); the 1-sample
// pulse is exactly the "山は 1 標本ぶん" shape (:1340 comment).
#[test]
fn tx_grid_pulses_irq2_per_2800() {
    let mut u = UsbLine::new();
    let mut rec = IrqRec::default();
    u.step(0, true, false, &mut rec);
    assert_eq!(rec.take(), vec![(2, 0), (2, 1)]); // :1341 + :1355
    assert_eq!(u.tx_next, 2800); // :1354
    u.step(1, true, false, &mut rec); // 1 < tx_next: drop only
    assert_eq!(rec.take(), vec![(2, 0)]);
    assert_eq!(u.tx_next, 2800);
    u.step(2800, true, false, &mut rec); // grid point: re-arm + raise
    assert_eq!(rec.take(), vec![(2, 0), (2, 1)]);
    assert_eq!(u.tx_next, 5600);
}

// (3) RX: the host-online command hands over on the grid, IRQ3 is HELD
// (:1342-1349 受信あり「読まれるまで上げておく」) — set while have is
// true, cleared ONLY by the 0xF80000 read (:1366-1367).
#[test]
fn rx_level_hold_until_read() {
    let mut u = UsbLine::new();
    // the reset push (mu2000.cpp:1097-1099): F4 03 01 01 01
    for b in [0xf4u8, 0x03, 0x01, 0x01, 0x01] {
        u.cmd.push_back(b);
    }
    let mut rec = IrqRec::default();
    u.step(0, true, false, &mut rec);
    assert_eq!(u.cur, 0xf4); // :1335
    assert!(u.cur_cmd); // :1334 — cmd queue first (:1333)
    assert!(u.have); // :1336
    assert_eq!(u.next, 2800); // :1338
    assert_eq!(u.cmd.len(), 4); // :1337 popped exactly one
    assert_eq!(rec.take(), vec![(2, 0), (3, 1), (2, 1)]); // :1341,:1349,:1355
    // status while in flight: bit0+bit6 = 0x41 (:1363)
    assert_eq!(u.r(1, &mut rec), 0x41);
    assert_eq!(rec.calls.len(), 0); // odd read never touches the line
    // read the byte: cur out, have down, IRQ3 drops (:1366-1367)
    assert_eq!(u.r(0, &mut rec), 0xf4);
    assert_eq!(rec.take(), vec![(3, 0)]);
    assert!(!u.have);
    assert_eq!(u.r(1, &mut rec), 0x00); // :1363 have=false leg
    // the grid keeps serving the rest; second byte is DATA-flagged only
    // after rx (all five here are cmd, so 0x41 until cmd empties)
    u.step(2800, true, false, &mut rec);
    assert_eq!(u.cur, 0x03);
    assert_eq!(rec.take(), vec![(2, 0), (3, 1), (2, 1)]);
}

// (4) 2799 ticks early: no second byte while the first sits unread AND no
// re-delivery while have (disk :1331 `!u.have && now >= u.next`)
#[test]
fn rx_next_grid_and_have_lock() {
    let mut u = UsbLine::new();
    u.rx.push_back(0x90);
    u.rx.push_back(0x40);
    let mut rec = IrqRec::default();
    u.step(10, true, false, &mut rec);
    assert_eq!(u.cur, 0x90);
    assert_eq!(u.next, 2810); // now + 2800 (:1338, not a global grid)
    u.step(2809, true, false, &mut rec); // grid NOT reached
    assert!(u.have); // still 0x90 in flight
    assert_eq!(u.cur, 0x90);
    u.r(0, &mut rec); // firmware reads it
    u.step(2809, true, false, &mut rec); // have now, still before next
    assert_eq!(u.cur, 0x90); // NOT re-delivered early
    u.step(2810, true, false, &mut rec); // grid point: 0x40
    assert_eq!(u.cur, 0x40);
    assert!(!u.cur_cmd); // rx byte — status would read 0x01 (:1363)
    assert_eq!(u.r(1, &mut rec), 0x01);
}

// (5) command BEFORE data (:1333 ternary), and the OFF-with-parked-rx
// disk case: guard passes on `have`, so a parked byte is still handed
// over and IRQ3 rises even with host OFF (disk pumps :1291-era truth).
#[test]
fn cmd_first_and_parked_rx_fires_off() {
    let mut u = UsbLine::new();
    u.cmd.push_back(0xf4);
    u.rx.push_back(0x90);
    let mut rec = IrqRec::default();
    u.step(0, true, false, &mut rec);
    assert_eq!(u.cur, 0xf4);
    assert!(u.cur_cmd);
    u.r(0, &mut rec);
    u.step(2800, true, false, &mut rec);
    assert_eq!(u.cur, 0x90); // cmd emptied -> rx (:1333)
    assert!(!u.cur_cmd);
    // OFF + parked: guard lets it through, IRQ2 TX grid also runs
    let mut v = UsbLine::new();
    v.rx.push_back(0x80); // N2-era parked byte
    let mut rec2 = IrqRec::default();
    v.step(0, false, false, &mut rec2);
    assert_eq!(v.cur, 0x80);
    assert!(v.have);
    assert_eq!(rec2.take(), vec![(2, 0), (3, 1), (2, 1)]); // disk-faithful
}

// (6) fast_midi collapses the RX grid (:1338 `now + 0`) — DIN-side
// fast rule mirrored here; the TX grid stays at 2800 (:1354 unguarded).
#[test]
fn fast_collapses_rx_grid_only() {
    let mut u = UsbLine::new();
    u.rx.push_back(0x90);
    u.rx.push_back(0x40);
    u.rx.push_back(0x41);
    let mut rec = IrqRec::default();
    u.step(100, true, true, &mut rec);
    assert_eq!(u.cur, 0x90);
    assert_eq!(u.next, 100); // :1338 fast leg
    u.r(0, &mut rec);
    u.step(100, true, true, &mut rec); // SAME now -> next byte
    assert_eq!(u.cur, 0x40);
    u.r(0, &mut rec);
    u.step(101, true, true, &mut rec);
    assert_eq!(u.cur, 0x41);
    // TX grid: 101 < 2800 -> still no re-raise after the first drop
    let calls = rec.take(); // [(2,0),(2,1) @100] etc — just check no hang
    assert!(calls.contains(&(2, 1)) || calls.contains(&(2, 0)));
}

// (7) bus-byte semantics of usb_w (:1371-1378): even = MIDI into tx,
// odd = command port DROPPED; cap = TX_SIZE silent drop. Word demotion
// (membus.h via sh7042.rs Dev::Usb) fires w8(even,hi) THEN w8(odd,lo) —
// a word write pushes ONLY the hi byte; the long pair pushes hi-of-lo
// word... exercised through the same even/odd rule below.
#[test]
fn w_even_odd_and_cap() {
    let mut u = UsbLine::new();
    u.w(1, 0xee); // odd: M37640 への指示、捨てる (:1373-1374)
    assert!(u.tx.is_empty());
    u.w(0, 0xf0);
    u.w(2, 0x55); // sel 2 (long demotion past window end, disk tests a&1 only)
    assert_eq!(u.tx.iter().copied().collect::<Vec<_>>(), vec![0xf0, 0x55]);
    // word write (v>>8)@even + v@odd -> hi lands, lo dropped:
    let (hi, lo) = (0xabu8, 0xcdu8); // sh7042.rs :1289-1295 shape
    u.w(0, hi);
    u.w(1, lo);
    assert_eq!(u.tx.iter().copied().collect::<Vec<_>>(), vec![0xf0, 0x55, 0xab]);
    // cap: TX_SIZE total, further even writes silently dropped (:1376)
    while u.tx.len() < USB_TX_LIMIT {
        u.w(0, 0x7e);
    }
    u.w(0, 0x11);
    assert_eq!(u.tx.len(), USB_TX_LIMIT); // 4096 (mu2000.h:1027)
    assert_eq!(u.tx.back(), Some(&0x7e));
}

// (8) word-read pair semantics (sh7042.rs Dev::Usb r8 pair): hi=sel0
// FIRST (clears have, drops IRQ3), lo=sel1 — so the status half of a
// word read after a byte read returns 0, exactly like the disk lambda pair.
#[test]
fn word_read_pair_order() {
    let mut u = UsbLine::new();
    u.cur = 0x2a;
    u.have = true;
    u.cur_cmd = false;
    let mut rec = IrqRec::default();
    let hi = u.r(0, &mut rec); // sel 0 first (BE hi byte, membus.h:81)
    let lo = u.r(1, &mut rec); // sel 1 second: have ALREADY cleared
    assert_eq!((hi, lo), (0x2a, 0x00));
    assert_eq!(rec.take(), vec![(3, 0)]); // one drop, from the sel-0 call
}

// (9) out_take F5 framing (:1380-1400) — port tag per out-byte rule:
// 1-based nn -> 0-based port; a trailing bare F5 is put BACK (:1388).
#[test]
fn out_take_frames_ports_and_holds_trailing_f5() {
    let mut u = UsbLine::new();
    assert_eq!(u.out_take(), None); // :1399
    u.tx.extend([0xf5, 0x04, 0xf0, 0x43, 0xf7]); // D, then same-port bytes
    assert_eq!(u.out_take(), Some((0xf0, 3))); // F5 pair consumed (:1386-93)
    assert_eq!(u.out_port, 3); // 1-based 4 -> 0-based 3 (:1391)
    assert_eq!(u.out_take(), Some((0x43, 3))); // port held
    assert_eq!(u.out_take(), Some((0xf7, 3)));
    assert_eq!(u.out_take(), None);
    u.tx.push_back(0xf5); // trailing F5, number not arrived yet
    assert_eq!(u.out_take(), None); // :1387-1390
    assert_eq!(u.tx.iter().copied().collect::<Vec<_>>(), vec![0xf5]); // back
    u.tx.push_back(0x02); // the number shows up -> C (1-based 2 -> 0)
    u.tx.push_back(0xf0); // ... and a byte to go with it
    assert_eq!(u.out_take(), Some((0xf0, 1))); // pair consumed, C port
    // (the None-with-putback above guarantees NO byte is ever eaten)
}

// (10) idle() three conditions (:223) — matches the parked-state facts
// the pending/idle tests in midi.rs (N2 row) already pin.
#[test]
fn idle_three_conditions() {
    let mut u = UsbLine::new();
    assert!(u.idle());
    u.rx.push_back(1);
    assert!(!u.idle()); // :223 rx
    u.rx.clear();
    u.cmd.push_back(0xf4);
    assert!(!u.idle()); // cmd
    u.cmd.clear();
    u.have = true;
    assert!(!u.idle()); // in-flight
    u.have = false;
    assert!(u.idle());
    // tx does NOT make the line busy (disk usb_idle reads rx/cmd/have only)
    u.tx.push_back(0x80);
    assert!(u.idle());
}
