//! M5-W3a: `smartmedia::state` (smartmedia.cpp:362-375) → `card::Card::state`.
//! Wire shape is TAGLESS (disk emits none; mu2000.cpp:3531-3533 gates it at
//! v>=5) and ends with the byte-queue leg: u32 count, then raw bytes.

use smu_compat::StateIo;
use smu_machine::card::{Card, MODE_PROGRAM};

fn save(c: &mut Card) -> Vec<u8> {
    let mut o = Vec::new();
    {
        let mut s = StateIo::writer(&mut o);
        c.state(&mut s);
        assert!(s.ok(), "save: {}", s.error());
    }
    o
}

fn load(c: &mut Card, buf: &[u8]) {
    let mut s = StateIo::reader(buf);
    c.state(&mut s);
    assert!(s.ok(), "load: {}", s.error());
}

fn quirk_buf(i: usize) -> u8 {
    (i as u32 * 7 + 3) as u8
}

#[test]
fn card_state_roundtrip_quirky_and_offsets() {
    let mut a = Card::new();
    a.ctrl = 0x1d; // :364
    a.mode = MODE_PROGRAM; // :365-367
    a.pointer = 0x50; // :368
    a.addr_count = 3;
    a.column = 0xdead_beef; // u32 width from smartmedia.h:86
    a.page = 0xffff_ffff;
    a.id_pos = 0x8000_0001;
    a.last_cmd = 0x10;
    a.buf = (0..300usize).map(quirk_buf).collect();

    let s1 = save(&mut a);
    // 8 scalars (4 u8 + 3 u32 + 1 u8 = 17 B) + u32 count + 300 raw bytes
    assert_eq!(s1.len(), 17 + 4 + 300);
    // hand-verified offsets (smartmedia.cpp stream order, no tag):
    assert_eq!(s1[0], 0x1d); // ctrl
    assert_eq!(s1[1], MODE_PROGRAM); // mode as u8 (cpp:365 u8 md)
    assert_eq!(s1[2], 0x50); // pointer
    assert_eq!(s1[3], 3); // addr_count
    assert_eq!(&s1[4..8], &0xdead_beefu32.to_le_bytes()); // column
    assert_eq!(&s1[8..12], &0xffff_ffffu32.to_le_bytes()); // page
    assert_eq!(&s1[12..16], &0x8000_0001u32.to_le_bytes()); // id_pos
    assert_eq!(s1[16], 0x10); // last_cmd
    assert_eq!(&s1[17..21], &300u32.to_le_bytes()); // queue count
    assert_eq!(s1[21], quirk_buf(0)); // first byte
    assert_eq!(s1[320], quirk_buf(299)); // last byte

    let mut b = Card::new();
    load(&mut b, &s1);
    assert_eq!(b.ctrl, 0x1d);
    assert_eq!(b.mode, MODE_PROGRAM);
    assert_eq!(b.column, 0xdead_beef);
    assert_eq!(b.buf.len(), 300);
    assert_eq!(save(&mut b), s1, "save->load->save bytes diverge");
}

#[test]
fn card_state_queue_leg_cap_4096_quirk() {
    // cpp:371-374 read side: resize(min(n,4096)) THEN mem(size()) — a
    // >4096-byte queue on the wire is cut to 4096 (and leaves the stream
    // desynced behind it, verbatim disk quirk).
    let mut a = Card::new();
    a.mode = MODE_PROGRAM;
    a.buf = (0..5000usize).map(quirk_buf).collect();
    let s = save(&mut a);
    assert_eq!(&s[17..21], &5000u32.to_le_bytes()); // writer emits true n
    assert_eq!(s.len(), 21 + 5000);

    let mut b = Card::new();
    load(&mut b, &s);
    assert_eq!(b.buf.len(), 4096); // std::min<u32>(n, 4096) — :372
    assert_eq!(&b.buf[..4096], &a.buf[..4096]); // consumed bytes ride exact
}

#[test]
fn machine_ctor_carries_fresh_card() {
    // M5-W3a wiring smoke: Machine::new explicit-inits the card (invariant 3)
    let mut m = smu_machine::Machine::new(Vec::new());
    assert_eq!(m.card.ctrl, 0);
    assert_eq!(m.card.mode, 0); // mode::idle (smartmedia.h:83)
    assert!(m.card.buf.is_empty());
    let mut fresh = Card::new();
    assert_eq!(save(&mut m.card), save(&mut fresh));
}
