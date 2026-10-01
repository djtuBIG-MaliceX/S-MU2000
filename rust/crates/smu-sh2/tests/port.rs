//! Origin-cited unit vectors for `periph/port.rs` (ledger row `periph: port`,
//! M2). Every expectation cites its disk line in `src/mame/cpu/sh_port.cpp`
//! (+ `sh_port.h` fields, `sh7042.cpp:231-236` port set, `sh7042_map.hxx`
//! register decode, `mu2000.cpp:1004-1105` LCD/encoder glue). Synthetic only;
//! no ROM/WAV data.

use smu_sh2::periph::port::{ShPort16, ShPort32};

// ---------------------------------------------------------------------------
// reset / initial state (device_start writes m_io; device_reset is a NO-OP)
// ---------------------------------------------------------------------------

#[test]
fn reset_defaults_porta() {
    // origin: device_start sh_port.cpp:86 (m_io = m_default_io) ; m_dr stays 0
    // (sh_port.h:79). Disk constants sh7042.cpp:231 (index 0, mask 0xff000000).
    let p = ShPort32::porta();
    assert_eq!(p.m_index, 0);
    assert_eq!(p.m_default_io, 0x0000_0000);
    assert_eq!(p.m_mask, 0xff00_0000);
    assert_eq!(p.m_io, 0); // start wrote default_io (== 0)
    assert_eq!(p.m_dr, 0); // never initialized off its header default
}

#[test]
fn reset_defaults_portb() {
    // origin: sh_port.cpp:27 (m_io = m_default_io) + sh7042.cpp:232 (mask 0xfc00).
    let p = ShPort16::portb();
    assert_eq!(p.m_index, 0);
    assert_eq!(p.m_mask, 0xfc00);
    assert_eq!(p.m_io, 0);
    assert_eq!(p.m_dr, 0);
}

#[test]
fn device_reset_is_noop_16() {
    // origin: sh_port.cpp:32-34 device_reset() { } — must NOT clear dr/io.
    let mut p = ShPort16::portc();
    p.io_w(0x00ff, 0xffff);
    p.dr_w(0xabcd, 0xffff);
    let (io, dr) = (p.m_io, p.m_dr);
    p.device_reset();
    assert_eq!(p.m_io, io);
    assert_eq!(p.m_dr, dr);
}

#[test]
fn device_reset_is_noop_32() {
    // origin: sh_port.cpp:91-93 device_reset() { } (32-bit).
    let mut p = ShPort32::portd();
    p.io_w(0x0000_ffff, 0xffff_ffff);
    p.dr_w(0x1234_5678, 0xffff_ffff);
    let (io, dr) = (p.m_io, p.m_dr);
    p.device_reset();
    assert_eq!(p.m_io, io);
    assert_eq!(p.m_dr, dr);
}

// ---------------------------------------------------------------------------
// PDR (data register) write: COMBINE_DATA + protected-bit clear
// ---------------------------------------------------------------------------

#[test]
fn dr_w_full16_updates_latch() {
    // origin: sh_port.cpp:53 COMBINE_DATA + :57 m_dr &= ~m_mask (mask 0 here).
    let mut p = ShPort16::portc();
    let fired = p.dr_w(0xabcd, 0xffff);
    assert_eq!(p.m_dr, 0xabcd);
    assert!(fired.is_none()); // io == 0 (sh_port.cpp:58) -> no drive
}

#[test]
fn dr_w_byte_lanes() {
    // origin: map w8 hands (v<<shift, 0xff<<shift) e.g. sh7042_map.hxx:649-652;
    // COMBINE_DATA (mamecompat.h:617) writes only mask-set bits.
    let mut p = ShPort16::portc();
    p.dr_w(0x12 << 8, 0xff << 8); // high lane
    p.dr_w(0x34 << 0, 0xff << 0); // low lane
    assert_eq!(p.m_dr, 0x1234);
}

#[test]
fn dr_w_mask_clears_protected_bits() {
    // origin: sh_port.cpp:57 m_dr &= ~m_mask ; PORT B mask 0xfc00 (sh7042.cpp:232).
    let mut p = ShPort16::portb();
    p.dr_w(0xffff, 0xffff);
    assert_eq!(p.m_dr, 0x03ff); // bits 10..15 forced to 0
}

#[test]
fn porta_dr_w_w32_masks_high_byte() {
    // origin: sh_port.cpp:105 m_dr &= ~m_mask ; PORT A mask 0xff000000 (:231).
    let mut p = ShPort32::porta();
    p.dr_w(0xffff_ffff, 0xffff_ffff);
    assert_eq!(p.m_dr, 0x00ff_ffff); // high byte (reserved I/O) cleared
}

#[test]
fn portd_full_width_no_mask() {
    // origin: sh_port.cpp:102-108, PORT D mask 0 (sh7042.cpp:234); io==0 -> no fire.
    let mut p = ShPort32::portd();
    let fired = p.dr_w(0xdead_beef, 0xffff_ffff);
    assert_eq!(p.m_dr, 0xdead_beef);
    assert!(fired.is_none());
}

// ---------------------------------------------------------------------------
// PDDR (direction) write
// ---------------------------------------------------------------------------

#[test]
fn io_r_returns_direction() {
    // origin: sh_port.cpp:62-65 io_r -> m_io ; PORT B io &= ~0xfc00 (:72).
    let mut p = ShPort16::portb();
    p.io_w(0x00ff, 0xffff);
    assert_eq!(p.io_r(), 0x00ff);
}

#[test]
fn porta_io_now_matches_io() {
    // origin: sh_port.h:70 io_now() ; used by porta_io() sh7042.h:82. PORT A
    // io &= ~0xff000000 keeps only the low byte as settable direction (:118).
    let mut p = ShPort32::porta();
    p.io_w(0x0000_ff00, 0xffff_ffff);
    assert_eq!(p.io_now(), 0x0000_ff00);
    assert_eq!(p.io_now(), p.io_r());
}

// ---------------------------------------------------------------------------
// output-drive callback seam: value carried, and the fire-every-write quirk
// ---------------------------------------------------------------------------

#[test]
fn output_write_fires_with_driven_bits() {
    // origin: sh_port.cpp:58-59 do_write_port16(port, m_dr & m_io, m_io).
    // Only the OUTPUT bits (m_dr & m_io) are handed to the delegate.
    let mut p = ShPort16::portc();
    p.io_w(0x00ff, 0xffff); // make low byte output
    let fired = p.dr_w(0x1234, 0xffff).expect("io != 0 must fire");
    assert_eq!(fired.0, 0x0034); // data = 0x1234 & 0x00ff
    assert_eq!(fired.1, 0x00ff); // ddr = m_io
}

#[test]
fn write_same_value_fires_again() {
    // DISK QUIRK: no change detection. sh_port.cpp:58 gates ONLY on `if (m_io)`.
    let mut p = ShPort16::portc();
    p.io_w(0xffff, 0xffff);
    let a = p.dr_w(0x1111, 0xffff);
    let b = p.dr_w(0x1111, 0xffff); // identical value ...
    assert_eq!(a, Some((0x1111, 0xffff)));
    assert_eq!(b, Some((0x1111, 0xffff))); // ... still fires (not suppressed)
}

#[test]
fn write_with_io_zero_does_not_fire() {
    // origin: sh_port.cpp:58 `if (m_io)` — no output bits -> do_write skipped.
    let mut p = ShPort16::portc(); // io == 0
    assert!(p.dr_w(0x5555, 0xffff).is_none());
}

#[test]
fn io_w_fires_with_new_direction() {
    // origin: sh_port.cpp:73-74 after direction change re-drive (m_dr & m_io, m_io).
    let mut p = ShPort16::portc();
    p.dr_w(0x00ff, 0xffff); // latch first (io==0, no fire)
    let fired = p.io_w(0x00ff, 0xffff).expect("io became nonzero");
    assert_eq!(fired.0, 0x00ff); // m_dr & m_io
    assert_eq!(fired.1, 0x00ff); // m_io
}

#[test]
fn io_w_to_zero_does_not_fire() {
    // origin: sh_port.cpp:72 io &= ~mask then :73 `if (m_io)` — all-input -> no fire.
    let mut p = ShPort16::portc();
    assert!(p.io_w(0xffff, 0xffff).is_some());
    assert!(p.io_w(0x0000, 0xffff).is_none());
}

// ---------------------------------------------------------------------------
// input read: direction mask + Port A encoder + Port F button row
// ---------------------------------------------------------------------------

#[test]
fn input_read_masked_by_direction() {
    // origin: sh_port.cpp:46-48 (dr & io) | (ext & ~io). bit0=output (latch),
    // bit1=input (pin). The pin's bit0 must be masked OUT (output selects latch).
    let mut p = ShPort16::portc();
    p.io_w(0x0001, 0xffff); // bit0 output, others input
    p.dr_w(0x0001, 0xffff); // latch bit0 = 1
    let v = p.dr_r(|| 0x00fe); // pin bit0=0, bit1..8=1
    assert_eq!(v, 0x00ff); // bit0 from latch(1), bit1 from pin(1)
}

#[test]
fn read_pins_not_called_when_all_inputs_masked() {
    // origin: sh_port.cpp:46 condition `~io & ~mask`. PORT B mask 0xfc00; with
    // io=0x03ff the only input-eligible bits (0..9) are all output, so the
    // delegate MUST NOT fire (encoder-style side effect safety).
    let mut p = ShPort16::portb();
    p.io_w(0x03ff, 0xffff); // -> io &= ~0xfc00 = 0x03ff
    let mut reads = 0u32;
    let v = p.dr_r(|| {
        reads += 1;
        0x5555
    });
    assert_eq!(reads, 0); // branch not taken
    assert_eq!(v, p.m_dr); // pure latch returned
}

#[test]
fn porta_encoder_bits_16_17_input_read() {
    // origin: sh_port.cpp:97-99 (32-bit dr_r). Encoder rea/reb = PORT A bits
    // 17/16 (mu2000.cpp:1073-1077), read as INPUT (io bit clear) via read_porta
    // (mu2000.cpp:1083-1104 -> read_pins here).
    let mut p = ShPort32::porta();
    let v = p.dr_r(|| 0x0003_0000); // bits 16/17 set on the pins
    assert_eq!(v & 0x0003_0000, 0x0003_0000);
}

#[test]
fn porta_encoder_bits_return_latch_when_output() {
    // origin: sh_port.cpp:97-99 once direction bits 16/17 are output the read
    // takes them from m_dr, NOT from the pins (pins = 0 here).
    let mut p = ShPort32::porta();
    p.io_w(0x0003_0000, 0xffff_ffff); // bits 16/17 -> output
    p.dr_w(0x0003_0000, 0xffff_ffff); // latch them high
    let v = p.dr_r(|| 0x0000_0000); // pins low
    assert_eq!(v & 0x0003_0000, 0x0003_0000); // latch wins
}

#[test]
fn portf_readonly_button_row_reads_pins() {
    // origin: PORT F map has NO write case (sh7042_map.hxx:176-177 read only),
    // so io stays 0 and reads reflect the active-low button matrix (pressed = 0,
    // mu2000.cpp:511-515). dr_r returns the pin value (sh_port.cpp:46-48).
    let mut p = ShPort16::portf();
    let v = p.dr_r(|| 0xff7f); // bit7 pressed (low), rest released
    assert_eq!(v, 0xff7f);
}

#[test]
fn portf_read_passes_high_byte_despite_mask() {
    // origin: sh_port.cpp:46-48 mask (0xff00) only gates the BRANCH, not the
    // returned bits; a read-only port still returns the full pin word.
    let mut p = ShPort16::portf();
    let v = p.dr_r(|| 0x1234);
    assert_eq!(v, 0x1234);
}

// ---------------------------------------------------------------------------
// Port E -> LCD seam: writing output bits drives the delegate with the lines
// ---------------------------------------------------------------------------

#[test]
fn porte_write_drives_lcd_data_lines() {
    // origin: sh_port.cpp:58-59. LCD glue (mu2000.cpp:1015-1036) reads /E=bit4,
    // R/W=bit0, RS=bit2 and the 8 data lines from bits 8..15 of the driven
    // value; those must survive the `m_dr & m_io` mask to the delegate.
    let mut p = ShPort16::porte();
    // make bits 0,2,4 (control) + 8..15 (data) outputs
    let ddr = 0xff15; // bit0|bit2|bit4 | 0xff00
    p.io_w(ddr, 0xffff);
    // strobe: high data 0x55 with /E high, R/W low, RS high (a data write edge)
    let f1 = p.dr_w(0x5500 | (1 << 4), 0xffff).expect("E outputs drive LCD");
    assert_eq!(f1.1, ddr); // ddr carried
    assert_eq!(f1.0, (0x5500 | (1 << 4)) & ddr); // data lines carried through
    // falling /E edge (bit4 cleared) - fires again, delivering the write value
    let f2 = p.dr_w(0x5500, 0xffff).expect("second write fires (no change gate)");
    assert_eq!(f2.0 & (1 << 4), 0); // /E now low
}
