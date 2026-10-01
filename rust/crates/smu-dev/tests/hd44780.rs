//! HD44780 row tests — origin-cited against src/mame/video/hd44780.{h,cpp}
//! and src/mu2000.cpp. Busy math per hd44780.h:106-109 (CPU cycles, not
//! emu_timer — hd44780.h:14-16): cpu 28 MHz / lcd 270 kHz (hd44780.h:33).

use smu_compat::roms::lcdfont::{parse, Glyph};
use smu_dev::hd44780::{CGRAM, DDRAM, Hd44780, RENDER_SIZE};

/// origin: mu2000.cpp:1126 `m_lcd.reset()` right after wiring; ctor
/// defaults hd44780.h:33 (28 MHz / 270 kHz).
fn dev() -> Hd44780 {
    let mut d = Hd44780::new(28_000_000, 270_000);
    d.set_now(0);
    d.reset();
    d
}

/// set_busy(10) in CPU cycles: 10 * 28_000_000 / 270_000 = 1037
/// (hd44780.h:108; ~37 us, hd44780.h:15).
const BUSY_CMD: u64 = 10 * 28_000_000 / 270_000;
/// set_busy(410) in CPU cycles: 410 * 28_000_000 / 270_000 = 42518
/// (the .5 truncates; ~1.5185 ms, hd44780.h:16).
const BUSY_LONG: u64 = 410 * 28_000_000 / 270_000;

#[test]
fn reset_state() {
    // origin: hd44780.cpp:11-32
    let mut d = Hd44780::new(28_000_000, 270_000);
    d.m_pe = 0x1234;
    d.set_now(0);
    d.reset();
    assert_eq!(BUSY_CMD, 1037);
    assert_eq!(BUSY_LONG, 42518);
    assert!(d.m_ddram.iter().all(|&v| v == 0x20)); // :13
    assert!(d.m_cgram.iter().all(|&v| v == 0)); // :14
    assert_eq!(d.m_fw, d.m_ddram); // :29
    assert_eq!(d.m_cg_fw, d.m_cgram); // :30
    assert_eq!((d.m_ac, d.m_direction, d.m_disp_shift), (0, 1, 0)); // :15-18
    assert_eq!(d.m_active_ram, DDRAM); // :16
    assert_eq!((d.m_num_line, d.m_char_size, d.m_data_len), (1, 8, 8)); // :19-21
    assert!(!(d.m_shift_on || d.m_display_on || d.m_cursor_on || d.m_blink_on || d.m_nibble)); // :22-24
    assert_eq!((d.m_ir, d.m_dr), (0, 0)); // :25
    assert_eq!(d.m_pe, 0x1234); // reset does NOT touch m_pe (cpp:11-32)
    assert_eq!(d.m_now, 0); // reset does NOT touch m_now (cpp:11-32)
}

#[test]
fn reset_busy_window() {
    // origin: hd44780.cpp:26-31 — busy_until = m_now + 410 ticks, boundary
    // is EXCLUSIVE (hd44780.h:46 `m_now < m_busy_until`).
    let mut d = Hd44780::new(28_000_000, 270_000);
    d.set_now(500_000);
    d.reset();
    assert_eq!(d.m_busy_until, 500_000 + 42_518);
    d.set_now(500_000 + 42_517);
    assert!(d.busy());
    d.set_now(500_000 + 42_518);
    assert!(!d.busy());
}

#[test]
fn instruction_busy_math_10_cycles() {
    // origin: hd44780.cpp:156 set_busy(10) → hd44780.h:108 math; boundary
    // now < busy_until (hd44780.h:46).
    let mut d = dev();
    d.set_now(1_000_000);
    d.control_w(0x0C); // display on, cpp:152-156
    assert_eq!(d.m_busy_until, 1_000_000 + BUSY_CMD);
    d.set_now(1_000_000 + BUSY_CMD - 1);
    assert!(d.busy());
    d.set_now(1_000_000 + BUSY_CMD);
    assert!(!d.busy());
}

#[test]
fn ddram_addr_set_1line_wrap() {
    // origin: hd44780.cpp:121-126 + correct_ac 84-86 (1-line max 0x4f).
    let mut d = dev();
    d.control_w(0x85); // :123 ac = ir & 0x7f
    assert_eq!((d.m_ac, d.m_active_ram), (5, DDRAM));
    assert_eq!(d.m_busy_until, BUSY_CMD); // :125
    d.control_w(0xC5); // 0x45 <= 0x4f stays
    assert_eq!(d.m_ac, 0x45);
    d.control_w(0xE0); // 0x60 > 0x4f wraps: 0x60 - 0x50 (cpp:86)
    assert_eq!(d.m_ac, 0x10);
}

#[test]
fn ddram_addr_set_2line_mirror_and_wrap() {
    // origin: hd44780.cpp:89-90 mirror 0x28..0x3f -> 0x40..; :84-86 wrap
    // at max 0x67 for 2 lines.
    let mut d = dev();
    d.control_w(0x38); // function set, cpp:134-143
    assert_eq!((d.m_num_line, d.m_char_size), (2, 8)); // :140/:135-136
    d.control_w(0xB0); // addr 0x30 -> 0x40 + (0x30-0x28) (cpp:90)
    assert_eq!(d.m_ac, 0x48);
    d.control_w(0xE8); // addr 0x68 > 0x67 -> -(0x67+1) (cpp:86)
    assert_eq!(d.m_ac, 0);
}

#[test]
fn cgram_addr_set() {
    // origin: hd44780.cpp:128-132 — CGRAM select masks 0x3f, sets busy(10).
    let mut d = dev();
    d.control_w(0x5F); // 0x40 | 0x1F
    assert_eq!((d.m_ac, d.m_active_ram), (0x1F, CGRAM));
    d.control_w(0x7B); // 0x40 | 0x3B
    assert_eq!((d.m_ac, d.m_active_ram), (0x3B, CGRAM));
    assert_eq!(d.m_busy_until, BUSY_CMD); // :131
}

#[test]
fn function_set_8bit_2line() {
    // origin: hd44780.cpp:134-143 — DL(bit4)=8, N(bit3)=2, 5x10 denied at
    // 2 lines (:135-136) even though bit2=1 (0x3C).
    let mut d = dev();
    d.control_w(0x3C);
    assert_eq!((d.m_data_len, d.m_num_line, d.m_char_size), (8, 2, 8));
    assert_eq!(d.line_size(), 40); // hd44780.h:101 80/2
}

#[test]
fn function_set_4bit_5x10_and_nibble_quirk() {
    // QUIRK (disk-faithful): m_nibble never goes true (cpp:24/:295), so the
    // live 4-bit arm is `m_ir |= high nibble` (:117) executed EVERY write —
    // and the stale 0x24 bit5 drags the next command back into the
    // FUNCTION-SET arm: 0x24 | 0x0E = 0x2E -> bit5 live -> re-decoded.
    let mut d = dev();
    d.control_w(0x24);
    assert_eq!((d.m_data_len, d.m_char_size, d.m_num_line), (4, 10, 1));
    d.control_w(0xE0);
    assert_eq!(d.m_ir, 0x2E); // :117 OR quirk
    // 0x2E via :134-141: bit5 function set; bit3=1 -> char 8 (5x10 denied)
    // + 2 lines; bit4=0 -> DL stays 4 (the quirk re-runs forever: any
    // command's high nibble ORs onto the stale bit5). No shift, no write.
    assert_eq!(d.m_data_len, 4);
    assert_eq!(d.m_ir, 0x2E);
    assert_eq!((d.m_num_line, d.m_char_size), (2, 8));
    assert_eq!(d.m_disp_shift, 0);
}

#[test]
fn entry_mode_inc() {
    // origin: hd44780.cpp:157-160 (0x06: I/D=+1 :158, S=0 :159).
    let mut d = dev();
    d.control_w(0x06);
    assert_eq!((d.m_direction, d.m_shift_on), (1, false));
    assert_eq!(d.m_busy_until, BUSY_CMD); // :160
}

#[test]
fn entry_mode_dec_shift() {
    // origin: hd44780.cpp:158-159 (0x05: I/D=-1, S=1).
    let mut d = dev();
    d.control_w(0x05);
    assert_eq!((d.m_direction, d.m_shift_on), (-1, true));
}

#[test]
fn display_fn_flags() {
    // origin: hd44780.cpp:152-156 — 0x0F all on, 0x08 all off.
    let mut d = dev();
    d.control_w(0x0F);
    assert!(d.m_display_on && d.m_cursor_on && d.m_blink_on);
    assert!(d.display_on()); // hd44780.h:103
    d.control_w(0x08);
    assert!(!(d.m_display_on || d.m_cursor_on || d.m_blink_on));
}

#[test]
fn cursor_shift_directions() {
    // origin: hd44780.cpp:145-150 S(bit3)=0 -> update_ac; dir from bit2.
    // 0x4f wrap from cpp:87-88 (ac<0 -> max_ac).
    let mut d = dev();
    d.control_w(0x14); // shift cursor right (:146 dir=+1, :150 update_ac)
    assert_eq!(d.m_ac, 1);
    d.control_w(0x10); // left
    assert_eq!(d.m_ac, 0);
    d.control_w(0x10); // left again -> wraps to max_ac 0x4f
    assert_eq!(d.m_ac, 0x4F);
}

#[test]
fn display_shift_wrap_ring() {
    // origin: hd44780.cpp:145-148 + shift_display 104-111 ring 0..0x4f.
    let mut d = dev();
    d.control_w(0x1D); // display shift right (bit3=1, bit2=1)
    assert_eq!(d.m_disp_shift, 1);
    d.control_w(0x19); // left -> 0
    assert_eq!(d.m_disp_shift, 0);
    d.control_w(0x19); // left -> -1 -> 0x4f (:109-110)
    assert_eq!(d.m_disp_shift, 0x4F);
}

#[test]
fn home() {
    // origin: hd44780.cpp:161-166 — ac/shift zeroed, dir forced +1, CGRAM
    // selection drops to DDRAM, busy 410.
    let mut d = dev();
    d.control_w(0x05); // dir=-1 shift
    d.control_w(0x5F); // CGRAM addr 0x1F
    d.control_w(0x02); // home
    assert_eq!(d.m_ac, 0); // :162
    assert_eq!(d.m_active_ram, DDRAM); // :163
    assert_eq!((d.m_direction, d.m_disp_shift), (1, 0)); // :164-165
    assert_eq!(d.m_busy_until, BUSY_LONG); // :166
    d.set_now(BUSY_LONG - 1);
    assert!(d.busy());
}

#[test]
fn clear_display() {
    // origin: hd44780.cpp:167-176 — memset ddram+fw 0x20 (:172/:174),
    // same registers as home, busy 410.
    let mut d = dev();
    d.control_w(0x80);
    d.data_w(b'X');
    assert_eq!(d.m_ddram[0], b'X');
    d.set_now(10_000);
    d.control_w(0x01);
    assert!(d.m_ddram.iter().all(|&v| v == 0x20));
    assert!(d.m_fw.iter().all(|&v| v == 0x20));
    assert_eq!((d.m_ac, d.m_direction, d.m_disp_shift, d.m_active_ram), (0, 1, 0, DDRAM));
    assert_eq!(d.m_busy_until, 10_000 + BUSY_LONG);
}

#[test]
fn ddram_write_increments_and_busy() {
    // origin: hd44780.cpp:190-218 — DDRAM/CGRAM write busy = set_busy(10)
    // (:215), ac += direction (:216).
    let mut d = dev();
    d.set_now(2_000_000);
    d.data_w(b'A');
    assert_eq!(d.m_ddram[0], b'A'); // :204
    assert_eq!(d.m_fw[0], b'A'); // :200
    assert_eq!(d.m_ac, 1); // :216 with entry++ (cpp:158 default)
    assert_eq!(d.m_busy_until, 2_000_000 + BUSY_CMD); // :215
}

#[test]
fn ddram_write_2line_backward_wrap() {
    // origin: hd44780.cpp:97-98 — 2-line, dir=-1, ac==0x40 -> 0x27.
    let mut d = dev();
    d.control_w(0x38); // 2 lines
    d.control_w(0x04); // entry decrement (cpp:158)
    d.control_w(0xC0); // addr 0x40
    assert_eq!(d.m_ac, 0x40);
    d.data_w(b'Y');
    assert_eq!(d.m_ddram[0x40], b'Y');
    assert_eq!(d.m_ac, 0x27); // cpp:97-98
}

#[test]
fn entry_shift_moves_display() {
    // origin: hd44780.cpp:217-218 — S=1: every data write shifts display.
    let mut d = dev();
    d.control_w(0x07); // entry++ with shift (cpp:158-159)
    d.data_w(b'A');
    assert_eq!(d.m_ac, 1);
    assert_eq!(d.m_disp_shift, 1); // shift_display(+1), cpp:106
}

#[test]
fn cgram_write_readback_wrap() {
    // origin: hd44780.cpp:198-213 (CGRAM branch :206-212) + correct_ac
    // CGRAM wrap `& 0x3f` (:92); data_r reads the fw shadow (:224).
    let mut d = dev();
    d.control_w(0x78); // CGRAM addr 0x38 (cpp:130)
    for i in 0..8u8 {
        d.data_w(0x10 + i);
    }
    assert_eq!(d.m_ac, 0x00); // 0x38 + 8 = 0x40 wraps `& 0x3f` (cpp:92)
    assert_eq!(d.m_cgram[0x38], 0x10); // :211
    assert_eq!(d.m_cgram[0x3F], 0x17);
    d.control_w(0x78);
    assert_eq!(d.data_r(), 0x10); // :224 cg_fw shadow
    assert_eq!(d.m_ac, 0x39); // :233
}

#[test]
fn data_r_returns_fw_and_sets_busy() {
    // origin: hd44780.cpp:221-234 — DDRAM read path, busy(10) :232,
    // ac += direction :233.
    let mut d = dev();
    d.control_w(0x83);
    d.data_w(b'B'); // ddram[3], ac -> 4
    d.control_w(0x83);
    d.set_now(50_000);
    assert_eq!(d.data_r(), b'B'); // :224
    assert_eq!(d.m_ac, 4); // :233
    assert_eq!(d.m_busy_until, 50_000 + BUSY_CMD); // :232
}

#[test]
fn control_r_busy_then_free() {
    // origin: hd44780.cpp:180-188 — 8-bit status = busy<<7 | addr&0x7f
    // (:187), busy flips at the EXACT cycle boundary (hd44780.h:46).
    let mut d = dev();
    d.set_now(0);
    d.control_w(0x85); // addr 5, busy 10
    assert_eq!(d.control_r(), 0x85); // busy|0x05
    d.set_now(BUSY_CMD);
    assert_eq!(d.control_r(), 0x05);
}

#[test]
fn control_r_4bit_low_nibble_no_busy() {
    // origin: hd44780.cpp:182-185 — 4-bit, nibble==false (the only live
    // arm): returns (ac << 4) & 0xf0 and DROPS the busy bit. Disk quirk.
    let mut d = dev();
    d.control_w(0xC5); // ac = 0x45 (8-bit mode)
    d.set_now(100);
    d.control_w(0x24); // switch to 4-bit (cpp:139); busy now
    assert_eq!(d.m_data_len, 4);
    assert_eq!(d.control_r(), 0x50); // (0x45 << 4) & 0xf0, no 0x80
}

#[test]
fn port_w_edge_strobe_fires_every_write() {
    // origin: mu2000.cpp:1015-1036 — /E bit4, RS bit2, R/W bit0, data
    // bits 8-15; fires on EVERY callback (port.rs quirk) and edge-detects
    // HERE (:1018): high->low writes, anything else is latch-only.
    let mut d = Hd44780::new(28_000_000, 270_000);
    d.set_now(0);
    d.reset();
    // protocol: RS/RW are DECODED FROM THE FALLING WORD (mu2000.cpp:1020
    // reads bit2/bit0 of `data` at the edge), so they must be held through
    // the whole E pulse.
    // command strobe: addr 0 (RS=0 both phases)
    d.lcd_port_w(10, (0x80u16 << 8) | 0x10); // rising, no fire (:1018)
    d.lcd_port_w(20, 0x80u16 << 8); // falling -> control_w(0x80)
    // data strobe: 'A', RS=1 held (:1020 -> data_w, :1021)
    d.lcd_port_w(30, (b'A' as u16) << 8 | 0x14);
    d.lcd_port_w(40, (b'A' as u16) << 8 | 0x04); // falling -> data_w('A')
    assert_eq!((d.m_ddram[0], d.m_ac), (b'A', 1));
    // SAME word again without an E phase: callback fires, no edge, no op
    d.lcd_port_w(50, (b'A' as u16) << 8 | 0x04);
    assert_eq!(d.m_ac, 1); // ddram[1] untouched (would be 'A' if fired)
    assert_eq!(d.m_ddram[1], 0x20);
    // R/W=1 falling edge must NOT write (:1019)
    d.lcd_port_w(60, (b'B' as u16) << 8 | 0x15); // E high, RS=1, R/W=1
    d.lcd_port_w(70, (b'B' as u16) << 8 | 0x01); // falling, R/W=1 -> latch only
    assert_eq!(d.m_ac, 1);
    assert_eq!(d.m_pe, (b'B' as u16) << 8 | 0x01); // :1035
}

#[test]
fn port_r_busy_addr_through_porte_bit15() {
    // origin: mu2000.cpp:1004-1013 — with /E high & R/W=1 the byte RS
    // selects returns in bits 8-15; busy = bit15 of the port word (bit7 of
    // hd44780.cpp:187 shifted <<8 at mu2000.cpp:1009). R/W=0 -> 0 (:1010),
    // /E low -> 0 (:1012).
    let mut d = Hd44780::new(28_000_000, 270_000);
    d.set_now(0);
    d.reset();
    d.lcd_port_w(100, (0x85u16 << 8) | 0x10);
    d.lcd_port_w(200, 0x85u16 << 8); // addr 5; busy_until = 200 + 1037
    d.lcd_port_w(300, 0x11); // hold E high, R/W=1, RS=0 (rising, no fire)
    assert_eq!(d.lcd_port_r(300 + 500), 0x85 << 8); // busy | 0x05 -> bit15 set
    assert_eq!(d.lcd_port_r(300 + 1037), 0x05 << 8); // free
    d.lcd_port_w(2000, 0x15); // RS=1 (rising, no fire); ddram[5] = 0x20
    assert_eq!(d.lcd_port_r(2000), 0x20 << 8);
    assert_eq!(d.m_ac, 6); // data_r advanced the pointer (cpp:233)
    d.lcd_port_w(3000, 0x14); // E high, R/W=0 (rising, no fire)
    assert_eq!(d.lcd_port_r(3000), 0x0000); // :1010
    // E falling with R/W=1: latch only, NEVER touches the device (:1019)
    d.lcd_port_w(4000, 0x01);
    assert_eq!(d.m_ac, 6); // no stray data_w/control_w from the falling edge
    assert_eq!(d.lcd_port_r(4000), 0); // :1012 /E low
}

#[test]
fn render_glyph_and_cursor_via_lcdfont() {
    // Font bytes come from the REUSED roms::lcdfont parser (smu-compat
    // M1 row): parse -> paint like lcdfont::overlay (roms.rs:433-439) ->
    // set_cgrom (mu2000.cpp:708). Fetch math: hd44780.cpp:252-266.
    let text = b"0x41\n#####\n#...#\n#...#\n#####\n.....\n#...#\n#...#\n.....\n";
    let mut glyphs: Vec<Glyph> = Vec::new();
    assert_eq!(parse(text, &mut glyphs), 1); // roms.rs:348
    let g = glyphs.iter().find(|g| g.code == 0x41).unwrap();
    // roms.rs:385-387 maps x -> bit (4 - x): '#' at x=0 AND x=4 -> 0x11
    assert_eq!(g.row, [0x1F, 0x11, 0x11, 0x1F, 0x00, 0x11, 0x11, 0x00]);

    let mut rom = vec![0u8; 0x1000];
    for r in 0..8 {
        rom[0x41 * 16 + r] = g.row[r]; // paint mirror (lcdfont.h:121-133)
    }
    rom[0x20 * 16] = 0x15; // 'space' for the cursor cell check

    let mut d = dev();
    d.set_cgrom(&rom); // hd44780.h:91-92 gate
    d.control_w(0x06); // entry++
    d.control_w(0x80);
    d.data_w(0x41); // 'A' -> cell 0, ac -> 1
    d.control_w(0x0E); // display + cursor on (cell 1 = 0x20 space)
    let img = d.render();
    assert_eq!(&img[0..8], &g.row[..]); // cpp:259 CGROM c*0x10, :263 copy 8
    assert_eq!(img[8], 0); // cpp:242 zero base
    assert_eq!(img[16], 0x15); // cell 1 from CGROM
    assert_eq!(img[16 + 7], 0x1F); // cursor last row (cpp:265-266)
    assert_eq!(img.len(), RENDER_SIZE); // hd44780.h:97 80*16
}

#[test]
fn render_gates_small_rom_and_display_off() {
    // origin: hd44780.h:91-92 (size gate -> nullptr) and hd44780.cpp:243
    // (!display_on || !cgrom -> all zeros).
    let mut d = dev();
    d.control_w(0x0C);
    let small = vec![0xAAu8; 0x100]; // < 0x1000: gate fails
    d.set_cgrom(&small);
    assert!(d.render().iter().all(|&v| v == 0));
    let full = vec![0xAAu8; 0x1000];
    d.set_cgrom(&full);
    d.control_w(0x80);
    d.data_w(b'A');
    assert_eq!(d.render()[0], 0xAA); // live now
    d.control_w(0x08); // display off (:153)
    assert!(d.render().iter().all(|&v| v == 0));
}
