//! Row gate "MIDI OUT ring" (W-TX): the mu2000 SCI-ch0 TX capture —
//! `tx_line` frame builder (mu2000.cpp:1271-1294) + `midi_out_take`
//! ring (mu2000.h:232-246, mu2000.h:1027-1031). Pure bit arithmetic, so
//! disk-derived vectors only (same convention as midi.rs `mod tests`).
//!
//! Mounted via smu-machine/Cargo.toml `[[test]] path =
//! "../../tests/midi_out.rs"` — the boot_golden pattern.

use smu_machine::midi::{Midi, MIDI_TX_MASK, MIDI_TX_SIZE};

/// origin: mu2000.h:1024-1025 — one UART frame on the TX wire: idle high,
/// start 0, 8 data bits LSB-first, stop 1. Every level change is one
/// tx_line call (the SCI reports exactly once per bit).
fn send_byte(m: &mut Midi, b: u8) {
    m.tx_line(false); // start bit
    for i in 0..8 {
        // origin: mu2000.cpp:1280-1283 — cur |= (state?1:0) << bit
        m.tx_line((b >> i) & 1 != 0);
    }
    m.tx_line(true); // stop bit
}

// (1) idle-hi -> start -> LSB-first 8 bits -> stop pushes the exact byte
#[test]
fn frame_pushes_exact_byte_lsb_first() {
    let mut m = Midi::new();
    m.tx_line(true); // idle high (:1273 guard — bit=-1, high: no-op)
    assert_eq!(m.midi_out_take(), None);
    send_byte(&mut m, 0xf0); // LSB-first: 0x0F would have the FIRST 4 bits high
    assert_eq!(m.midi_out_take(), Some(0xf0)); // :1292 pushed as assembled
    assert_eq!(m.tx_bit, -1); // :1286 back to waiting
    // two frames ride the ring in order (:243-244 FIFO)
    send_byte(&mut m, 0x80);
    send_byte(&mut m, 0x01);
    assert_eq!(m.midi_out_take(), Some(0x80));
    assert_eq!(m.midi_out_take(), Some(0x01));
    assert_eq!(m.midi_out_take(), None);
}

// (2) stop-bit 0 drops the byte (:1285-1288 "枠がずれているので捨てる")
#[test]
fn stop_bit_zero_drops_byte() {
    let mut m = Midi::new();
    m.tx_line(false); // start
    for _ in 0..8 {
        m.tx_line(true); // all-ones data
    }
    m.tx_line(false); // stop 0 -> :1287 return without push
    assert_eq!(m.midi_out_take(), None);
    assert_eq!(m.tx_bit, -1); // :1286 still reset the frame
    assert_eq!(m.tx_w, 0); // ring untouched
}

// (3) a falling edge mid-byte is a DATA bit, never a restart (:1273 guard
// only fires at bit<0). 9 post-start edges here: consumed correctly the
// frame ends on the stop; a (wrong) mid-byte restart would leave the
// machine mid-frame with a different byte / no push at all.
#[test]
fn start_edge_midbyte_ignored_as_data_bit() {
    let mut m = Midi::new();
    m.tx_line(false); // start (bit 0)
    m.tx_line(false); // fake "start" while mid-byte -> data bit0 = 0
    m.tx_line(true); // data bit1 = 1  -> LSB-first value 0x02
    for _ in 2..8 {
        m.tx_line(false); // data bits 2..7 = 0
    }
    m.tx_line(true); // stop
    assert_eq!(m.midi_out_take(), Some(0x02));
    assert_eq!(m.tx_bit, -1);
}

// (4) ring-full drop (:1289-1291) and slot-frees-on-take (:241-244)
#[test]
fn ring_full_drops_then_take_frees_slot() {
    let mut m = Midi::new();
    for i in 0..(MIDI_TX_SIZE - 1) as u64 {
        send_byte(&mut m, (i & 0xff) as u8); // 4095 bytes = full
    }
    assert_eq!(m.tx_r, 0);
    assert_eq!(m.tx_w, MIDI_TX_SIZE - 1);
    send_byte(&mut m, 0xee); // 4096th: next(=0)==r -> DROP (:1290)
    assert_eq!(m.tx_w, MIDI_TX_SIZE - 1); // w unchanged — byte discarded
    assert_eq!(m.midi_out_take(), Some(0x00)); // :243-244 frees a slot
    assert_eq!(m.tx_r, 1);
    send_byte(&mut m, 0xee); // now there is room again
    assert_eq!(m.tx_w, 0); // wrapped
    for i in 1..(MIDI_TX_SIZE - 1) as u64 {
        assert_eq!(m.midi_out_take(), Some((i & 0xff) as u8));
    }
    assert_eq!(m.midi_out_take(), Some(0xee)); // the retried byte
    assert_eq!(m.midi_out_take(), None);
}

// (5) r wraps 4095 -> 0 on take (:244 `(m_tx_r + 1) & TX_MASK`)
#[test]
fn take_wraps_r_to_zero() {
    let mut m = Midi::new();
    m.tx_buf[MIDI_TX_MASK] = 0x5a; // park one byte at the wrap cell
    m.tx_r = MIDI_TX_MASK;
    m.tx_w = 0; // r..w non-empty: exactly the wrap cell
    assert_eq!(m.midi_out_take(), Some(0x5a));
    assert_eq!(m.tx_r, 0);
    assert_eq!(m.midi_out_take(), None); // r==w -> empty (:241)
}

// (6) empty ring -> None (:241-242)
#[test]
fn empty_ring_returns_none() {
    let mut m = Midi::new();
    assert_eq!(m.midi_out_take(), None);
    assert_eq!(m.tx_r, 0);
    assert_eq!(m.tx_w, 0);
}

// (7) USB divert LIVE (mu2000.h:237-240, M7 `USB host` row): with
// usb_host=true take reads the USB tx ring instead of the DIN ring —
// and the DIN ring bytes SURVIVE untouched (the disk tx_line wire path
// is USB-oblivious; mu2000.cpp:1179 stays bound either way).
#[test]
fn usb_divert_live_reads_tx_ring_ring_intact() {
    let mut m = Midi::new();
    assert!(!m.usb_host.get()); // mu2000.h:1022 ctor default
    send_byte(&mut m, 0xf7); // DIN ring: one byte waiting
    m.usb_host.set(true); // disk diverts to usb_out_take (:237-240)
    assert_eq!(m.midi_out_take(), None); // usb tx ring EMPTY -> None
    m.usb.borrow_mut().w(0, 0xf5); // usb_w(:1371-1377): F5 framing kept
    m.usb.borrow_mut().w(0, 0x04); // (usb_midi_in-side framing, mu2000.cpp)
    m.usb.borrow_mut().w(0, 0xf0); // a SysEx start on port D (3)
    assert_eq!(m.midi_out_take(), Some(0xf0)); // F5 pair consumed (:1386-1393)
    assert_eq!(m.usb.borrow().out_port, 3); // 1-based 4 -> 0-based 3 (:1391)
    m.usb_host.set(false);
    assert_eq!(m.midi_out_take(), Some(0xf7)); // DIN ring untouched by the arm
}

// (8) boot-neutral: Machine::reset (:1177-1178 equivalent) + the
// device_reset born-high edges (sh_sci.cpp:367 via sci.rs:322, drained by
// Machine::drain_sci_pins) must all no-op and leave the pump pristine.
#[test]
fn device_reset_born_high_edges_noop() {
    let mut m = Midi::new();
    send_byte(&mut m, 0x90); // dirty the ring, like a session before reset
    assert_eq!(m.midi_out_take(), Some(0x90));
    // origin: mu2000.cpp:1177-1178 (Machine::reset seam, lib.rs :1145-1147)
    m.tx_r = 0;
    m.tx_w = 0;
    m.tx_bit = -1;
    // origin: sh_sci.cpp:367 -> sci.rs:322 do_sci_tx(true) per device_reset,
    // both channels' edges funnel through the ch0 bind (:1179)
    for _ in 0..4 {
        m.tx_line(true); // :1273-1278 — high at bit=-1: no start, no push
    }
    assert_eq!(m.tx_bit, -1);
    assert_eq!(m.tx_r, 0);
    assert_eq!(m.tx_w, 0);
    assert_eq!(m.midi_out_take(), None); // nothing crossed the wire
    send_byte(&mut m, 0xf5); // pump still alive after the reset edges
    assert_eq!(m.midi_out_take(), Some(0xf5));
}
