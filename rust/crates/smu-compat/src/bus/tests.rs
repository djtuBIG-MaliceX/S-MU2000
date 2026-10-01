//! Unit tests for the membus transliteration (`src/compat/membus.h`).
//!
//! These pin the QUIRKS too (inclusive `end`, unaligned-address masking,
//! width-promotion tables) - if someone "cleans up" bus.rs, a test must fail.

use super::{Device, MemBus};
use std::cell::RefCell;
use std::rc::Rc;

/// Builder for the standard test map:
///   ROM  0x000000-0x000FFF (read-only, hot_r)
///   RAM  0xFFC000-0xFFCFFF (writable, hot_w)   (matches mu2000 geometry shape)
struct Map {
    rom: Vec<u8>,
    ram: Vec<u8>,
    bus: MemBus,
}

impl Map {
    fn new() -> Self {
        let mut rom: Vec<u8> = (0..0x1000).map(|i| (i % 251) as u8).collect();
        let mut ram: Vec<u8> = vec![0u8; 0x1000];
        let mut bus = MemBus::new();
        // SAFETY: rom/ram outlive the bus by declaration order (dropped LIFO:
        // bus first, then ram, then rom). Same contract as the Machine wiring.
        unsafe {
            bus.add_region(0x000000, 0x000FFF, rom.as_mut_ptr(), false);
            bus.add_region(0xFFC000, 0xFFCFFF, ram.as_mut_ptr(), true);
        }
        Self { rom, ram, bus }
    }
}

#[test]
fn rom_reads_big_endian_and_ignores_writes() {
    let mut m = Map::new();
    m.rom[0] = 0xDE; m.rom[1] = 0xAD; m.rom[2] = 0xBE; m.rom[3] = 0xEF;
    assert_eq!(m.bus.read_byte(0), 0xDE);
    assert_eq!(m.bus.read_byte(1), 0xAD);
    assert_eq!(m.bus.read_word(0), 0xDEAD);
    assert_eq!(m.bus.read_dword(0), 0xDEADBEEF);
    // writes to read-only regions must not land (C++: find_write skips them)
    m.bus.write_byte(0, 0x00);
    m.bus.write_word(0, 0x0000);
    m.bus.write_dword(0, 0);
    assert_eq!(m.bus.read_dword(0), 0xDEADBEEF);
}

#[test]
fn ram_roundtrip_be_inclusive_end() {
    let mut m = Map::new();
    m.bus.write_dword(0xFFC000, 0x0BADC0DE);
    assert_eq!(&m.ram[0..4], &[0x0B, 0xAD, 0xC0, 0xDE]);
    // inclusive `end` (membus.h:192 `<= m_hot_w_len`): last byte is in range
    m.bus.write_byte(0xFFCFFF, 0x5A);
    assert_eq!(m.ram[0x0FFF], 0x5A);
    assert_eq!(m.bus.read_byte(0xFFCFFF), 0x5A);
}

#[test]
fn unaligned_word_and_dword_are_masked() {
    // membus.h:74/87/113/126 - a &= ~1 / ~3 BEFORE dispatch
    let mut m = Map::new();
    m.rom[8] = 0x11; m.rom[9] = 0x22;
    assert_eq!(m.bus.read_word(9), 0x1122); // reads word at 8, not 9
    m.bus.write_word(0xFFC001, 0x3344);
    assert_eq!(&m.ram[0..2], &[0x33, 0x44]);
}

/// Log all handler calls of a device to inspect the promotion table.
type Log = Rc<RefCell<Vec<(&'static str, u32, u32)>>>;

fn logging_device(start: u32, end: u32, have_r8: bool, have_r16: bool, have_r32: bool,
                  have_w8: bool, have_w16: bool, have_w32: bool, log: &Log) -> Device {
    let mut d = Device { start, end, ..Default::default() };
    if have_r8 {
        let l = log.clone();
        d.r8 = Some(Box::new(move |a| { l.borrow_mut().push(("r8", a, 0)); (a & 0xFF) as u8 }));
    }
    if have_r16 {
        let l = log.clone();
        d.r16 = Some(Box::new(move |a| { let v = a ^ 0xA5A5; l.borrow_mut().push(("r16", a, v)); v as u16 }));
    }
    if have_r32 {
        let l = log.clone();
        d.r32 = Some(Box::new(move |a| { let v = a ^ 0x5A5A5A5A; l.borrow_mut().push(("r32", a, v)); v }));
    }
    if have_w8 {
        let l = log.clone();
        d.w8 = Some(Box::new(move |a, v| l.borrow_mut().push(("w8", a, v as u32))));
    }
    if have_w16 {
        let l = log.clone();
        d.w16 = Some(Box::new(move |a, v| l.borrow_mut().push(("w16", a, v as u32))));
    }
    if have_w32 {
        let l = log.clone();
        d.w32 = Some(Box::new(move |a, v| l.borrow_mut().push(("w32", a, v))));
    }
    d
}

#[test]
fn read_promotion_table() {
    // r16-only device: byte at even addr gets HIGH byte (membus.h:66 >>8),
    // odd addr gets LOW byte. r32-only: byte picks lane (membus.h:69).
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut m = Map::new();
    m.bus.add_device(logging_device(0x100000, 0x1000FF, false, true, false, false, false, false, &log));
    let v = 0x100000u32 ^ 0xA5A5; // r16 handler returns a ^ 0xA5A5 (word value)
    // even byte: (r16(a&!1) >> 8) - membus.h:66
    assert_eq!(m.bus.read_byte(0x100000), (v >> 8) as u8);
    // odd byte: low byte
    assert_eq!(m.bus.read_byte(0x100001), v as u8);
    // r32-only device: read_dword must call r32 exactly once, no fallbacks
    let mut m2 = Map::new();
    m2.bus.add_device(logging_device(0x200000, 0x2000FF, false, false, true, false, false, false, &log));
    log.borrow_mut().clear();
    let _ = m2.bus.read_dword(0x200000);
    assert_eq!(log.borrow().as_slice(), &[("r32", 0x200000, 0x200000 ^ 0x5A5A5A5A)]);
}

#[test]
fn read_dword_falls_back_to_two_words() {
    // no r32 registered: membus.h:95-96 -> read_word(a) <<16 | read_word(a+2)
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut m = Map::new();
    m.bus.add_device(logging_device(0x110000, 0x1100FF, false, true, false, false, false, false, &log));
    let hi = (0x110000u32 ^ 0xA5A5) as u16;
    let lo = (0x110002u32 ^ 0xA5A5) as u16;
    assert_eq!(m.bus.read_dword(0x110000), ((hi as u32) << 16) | lo as u32);
}

#[test]
fn write_demotion_table() {
    // w8-only device: write_word fires w8 twice, big-endian order (membus.h:120)
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut m = Map::new();
    m.bus.add_device(logging_device(0x300000, 0x3000FF, false, false, false, true, false, false, &log));
    m.bus.write_word(0x300000, 0xABCD);
    assert_eq!(log.borrow().as_slice(), &[("w8", 0x300000, 0xAB), ("w8", 0x300001, 0xCD)]);
    log.borrow_mut().clear();
    // w16-only: write_byte lanes the value (membus.h:116: odd lane)
    let mut m2 = Map::new();
    m2.bus.add_device(logging_device(0x310000, 0x3100FF, false, false, false, false, true, false, &log));
    m2.bus.write_byte(0x310001, 0x77);
    assert_eq!(log.borrow().as_slice(), &[("w16", 0x310000, 0x0077)]);
    log.borrow_mut().clear();
    // w8-only: write_dword -> 2x write_word -> 4x w8 BE (membus.h:138-139)
    let mut m3 = Map::new();
    m3.bus.add_device(logging_device(0x320000, 0x3200FF, false, false, false, true, false, false, &log));
    m3.bus.write_dword(0x320000, 0x11223344);
    assert_eq!(log.borrow().as_slice(), &[
        ("w8", 0x320000, 0x11), ("w8", 0x320001, 0x22),
        ("w8", 0x320002, 0x33), ("w8", 0x320003, 0x44)]);
}

#[test]
fn unmapped_reads_zero_and_writes_drop() {
    let mut m = Map::new();
    assert_eq!(m.bus.read_byte(0x80000000), 0);
    assert_eq!(m.bus.read_word(0x80000000), 0);
    assert_eq!(m.bus.read_dword(0x7FFFFFFC), 0);
    m.bus.write_byte(0x80000000, 0xFF); // must not panic
}

#[test]
fn device_beats_region_gap_only() {
    // device under a region? regions win for direct reads (membus order:
    // fast -> find_read -> find_dev). Fully-mapped device never fires.
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut m = Map::new();
    m.bus.add_device(logging_device(0xFFC000, 0xFFC0FF, true, false, false, false, false, false, &log));
    let _ = m.bus.read_byte(0xFFC010);
    assert!(log.borrow().is_empty(), "RAM region must shadow the device");
}
