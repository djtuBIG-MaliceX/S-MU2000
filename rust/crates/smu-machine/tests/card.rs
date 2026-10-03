//! M5-W3a: `smartmedia::state` (smartmedia.cpp:362-375) → `card::Card::state`.
//! Wire shape is TAGLESS (disk emits none; mu2000.cpp:3531-3533 gates it at
//! v>=5) and ends with the byte-queue leg: u32 count, then raw bytes.

use smu_compat::StateIo;
use smu_machine::card::{Card, MODE_PROGRAM};
use smu_sh2::core::Sh2Bus; // bus read_byte/read_long trait (PA/case-window seam)

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
    // M5-W3a wiring smoke: Machine::new explicit-inits the card (invariant 3).
    // W-CARD: the Machine and the Hub bus arms now share ONE Rc<RefCell>.
    let m = smu_machine::Machine::new(Vec::new());
    {
        let c = m.card.borrow();
        assert_eq!(c.ctrl, 0);
        assert_eq!(c.mode, 0); // mode::idle (smartmedia.h:83)
        assert!(c.buf.is_empty());
        assert!(!c.inserted());
    }
    let mut fresh = Card::new();
    assert_eq!(save(&mut m.card.borrow_mut()), save(&mut fresh));
}

// ===========================================================================
// W-CARD row: GT replay (harness %TEMP%\cardgt\gt.cpp, canonical
// smartmedia.cpp compiled -std=c++20 -O3, run 2026-10-03; output in
// gt.txt). PURE vectors — the bus pulses below replay the harness sequences
// bit-for-bit through control_w/data_w/data_r only.
// ===========================================================================

const PB: usize = 512 + 16; // page_bytes (smartmedia.h:69)

fn gt(p: &[u8], off: usize, n: usize) -> Vec<u8> {
    p[off..off + n].to_vec()
}

// bus pulse helpers — EXACTLY the harness cle/ale/dat/rd (CE=1 always)
fn cle(c: &mut Card, v: u8) {
    c.control_w(0x09); // CLE|CE
    c.data_w(v);
    c.control_w(0x01);
}
fn ale(c: &mut Card, v: u8) {
    c.control_w(0x05); // ALE|CE
    c.data_w(v);
    c.control_w(0x01);
}
fn dat(c: &mut Card, v: u8) {
    c.control_w(0x01);
    c.data_w(v);
}
fn rd(c: &mut Card) -> u8 {
    c.control_w(0x01);
    c.data_r()
}
fn pat(i: usize) -> u8 {
    ((i * 7 + 1) & 0xff) as u8
}
fn pat2(i: usize) -> u8 {
    // C++ int arithmetic: u8((0xff - i * 3) & 0xff) — usize wrap here
    (((0xffusize.wrapping_sub(i * 3)) & 0xff) as u8)
}
fn pat3(i: usize) -> u8 {
    ((i * 11 + 5) & 0xff) as u8
}

#[test]
fn gt_create32_cis_spare_and_geometry() {
    // GT: create32 1 / pages32 65536 / mb32 32 / ac32 3 / ins32 1 /
    // dirty32 1 / dbsz32 2048
    let mut c = Card::new();
    assert!(c.create(32));
    assert_eq!(c.pages, 65536);
    assert_eq!(c.megabytes(), 32);
    assert_eq!(c.address_cycles(), 3);
    assert!(c.inserted());
    assert!(c.dirty());
    assert_eq!(c.dirty_blocks.len(), 2048);
    assert!(c.dirty_blocks.iter().all(|&b| b == 1)); // :108 all-1 assign
    let r = c.raw();
    assert_eq!(
        gt(r, 0, 16),
        [0x01, 0x03, 0xd9, 0x01, 0xff, 0x18, 0x02, 0xdf, 0x01, 0x20, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        "create_head16"
    );
    let spare = [0xffu8, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0xff, 0xff, 0xff, 0x00, 0x00, 0xa9, 0xaa, 0xa7];
    assert_eq!(gt(r, 512, 16), spare, "create_p0_spare (ECC c1,c0,a9aa-a7)");
    assert_eq!(gt(r, PB + 512, 16), spare, "create_p1_spare");
    assert_eq!(
        gt(r, 2 * PB + 512, 16),
        [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0xff, 0xff, 0xff, 0x00, 0x00, 0xff, 0xff, 0xff],
        "create_p2_spare (addr 0000, no ECC)"
    );
    assert_eq!(gt(r, 32 * PB, 8), [0xffu8; 8], "create_blk1");
}

#[test]
fn gt_replay_bus_sequences_32mb() {
    // The harness card `c`, V2-V7 in order (pointer persistence and the
    // block-granular erase are intentionally captured):
    let mut c = Card::new();
    assert!(c.create(32));

    cle(&mut c, 0x90); // V2 id32 98 75 98 75 (smartmedia.cpp:250-253 cycling)
    let got: Vec<u8> = (0..4).map(|_| rd(&mut c)).collect();
    assert_eq!(got, [0x98, 0x75, 0x98, 0x75]);

    cle(&mut c, 0x50); // V3 rd50_16: p0 spare[8..] -> p1 spare[0..8]
    ale(&mut c, 8);
    ale(&mut c, 0);
    ale(&mut c, 0);
    let got: Vec<u8> = (0..16).map(|_| rd(&mut c)).collect();
    assert_eq!(
        got,
        [0xff, 0xff, 0xff, 0x00, 0x00, 0xa9, 0xaa, 0xa7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00]
    );

    cle(&mut c, 0x80); // V4 program page 4. QUIRK captured: pointer is STILL 0x50
    ale(&mut c, 0); // (V3) -> cpp:293 column starts at 512, the pattern lands in
    ale(&mut c, 4); // the SPARE only, data stays 0xFF. Replay = data_r ff x16.
    ale(&mut c, 0);
    for i in 0..528 {
        dat(&mut c, pat(i));
    }
    cle(&mut c, 0x10);
    cle(&mut c, 0x70);
    assert_eq!(rd(&mut c), 192, "sts_prog"); // 0xC0 not-WP|ready (:256)
    cle(&mut c, 0x00);
    ale(&mut c, 0);
    ale(&mut c, 4);
    ale(&mut c, 0);
    let got: Vec<u8> = (0..16).map(|_| rd(&mut c)).collect();
    assert_eq!(got, [0xffu8; 16], "prog16");
    assert!(c.dirty(), "dirty_after_prog");

    cle(&mut c, 0x60); // V5 erase page 4 -> BLOCK 0 (pages 0-31, CIS gone too)
    ale(&mut c, 4);
    ale(&mut c, 0);
    cle(&mut c, 0xd0);
    cle(&mut c, 0x00);
    ale(&mut c, 0);
    ale(&mut c, 4);
    ale(&mut c, 0);
    let got: Vec<u8> = (0..16).map(|_| rd(&mut c)).collect();
    assert_eq!(got, [0xffu8; 16], "erase16");
    assert!(c.dirty(), "dirty_after_erase");

    c.write_protected = true; // V6 sts_wp 64 (smartmedia.cpp:256)
    cle(&mut c, 0x70);
    assert_eq!(rd(&mut c), 64);
    c.write_protected = false;

    cle(&mut c, 0x00); // V7 rd00_8_4 — page 0 erased by V5's block erase
    ale(&mut c, 8);
    ale(&mut c, 0);
    ale(&mut c, 0);
    let got: Vec<u8> = (0..4).map(|_| rd(&mut c)).collect();
    assert_eq!(got, [0xffu8; 4]);

    cle(&mut c, 0x01); // wrap01: col 256 + 272 reads (data-hi + spare), then the
    ale(&mut c, 0); // 273rd crosses pages: pointer 01->00, col->0 (:262-268)
    ale(&mut c, 0);
    ale(&mut c, 0);
    for _ in 0..272 {
        rd(&mut c);
    }
    let got: Vec<u8> = (0..16).map(|_| rd(&mut c)).collect();
    assert_eq!(got, [0xffu8; 16], "wrap01_p1head16 (page 1 erased with block 0)");

    cle(&mut c, 0x60); // erase page 4 again, then 50-read its spare
    ale(&mut c, 4);
    ale(&mut c, 0);
    cle(&mut c, 0xd0);
    cle(&mut c, 0x50);
    ale(&mut c, 0);
    ale(&mut c, 4);
    ale(&mut c, 0);
    let got: Vec<u8> = (0..16).map(|_| rd(&mut c)).collect();
    assert_eq!(got, [0xffu8; 16], "erase_spare16 — erase covers the spare");
}

#[test]
fn gt_file_roundtrip_blocks_and_bad_loads() {
    // V8: create(16) -> take-all -> save -> program page 40 (block 1) ->
    // take-one -> write_blocks -> load -> read back
    let dir = std::env::temp_dir();
    let path = dir.join(format!("smu_card_{}.bin", std::process::id()));
    let p = path.to_str().unwrap();

    let mut f = Card::new();
    assert!(f.create(16));
    assert_eq!(f.address_cycles(), 3, "ac16");
    let mut bl: Vec<smu_machine::card::Block> = Vec::new();
    f.take_dirty_blocks(&mut bl);
    assert_eq!(bl.len(), 1024, "blocks_fresh");
    assert_eq!(bl[0].index, 0);
    assert_eq!(bl[0].bytes.len(), 16896, "blk0_len (32 pages x 528)");
    assert!(!f.dirty(), "dirty_after_take");

    let mut err = String::new();
    assert!(f.save(p, &mut err), "save: {err}");

    cle(&mut f, 0x80); // idle pointer 0 -> column 0: the FULL 528-byte page
    ale(&mut f, 0);
    ale(&mut f, 40);
    ale(&mut f, 0);
    for i in 0..528 {
        dat(&mut f, pat2(i));
    }
    cle(&mut f, 0x10);
    bl.clear();
    f.take_dirty_blocks(&mut bl);
    assert_eq!(bl.len(), 1, "blocks_after_prog");
    assert_eq!(bl[0].index, 1, "blk1_idx");
    assert_eq!(
        gt(&bl[0].bytes, 8 * PB, 16),
        [0xff, 0xfc, 0xf9, 0xf6, 0xf3, 0xf0, 0xed, 0xea, 0xe7, 0xe4, 0xe1, 0xde, 0xdb, 0xd8, 0xd5, 0xd2],
        "blk1_win16 (pat2 at page 40 = block byte 8*528)"
    );
    assert!(Card::write_blocks(p, &bl, &mut err), "write_blocks: {err}");

    let mut g = Card::new();
    assert!(g.load(p, &mut err), "load: {err}");
    assert_eq!(g.megabytes(), 16);
    cle(&mut g, 0x00);
    ale(&mut g, 0);
    ale(&mut g, 40);
    ale(&mut g, 0);
    let got: Vec<u8> = (0..16).map(|_| rd(&mut g)).collect();
    assert_eq!(got, gt(&bl[0].bytes, 8 * PB, 16), "load_p40_head16");
    cle(&mut g, 0x50);
    ale(&mut g, 0);
    ale(&mut g, 40);
    ale(&mut g, 0);
    let got: Vec<u8> = (0..16).map(|_| rd(&mut g)).collect();
    assert_eq!(
        got,
        [0xff, 0xfc, 0xf9, 0xf6, 0xf3, 0xf0, 0xed, 0xea, 0xe7, 0xe4, 0xe1, 0xde, 0xdb, 0xd8, 0xd5, 0xd2],
        "load_p40_spare16 (pat2 continues into the spare)"
    );
    assert!(!g.dirty(), "load_dirty — clear_dirty at cpp:162");
    std::fs::remove_file(p).ok();

    // bad size -> false, empty slot, idle read 0xFF (the old stub's 0 is a LIE)
    let bad = dir.join(format!("smu_card_bad_{}.bin", std::process::id()));
    std::fs::write(&bad, [0x5au8; 1234]).unwrap();
    let mut h = Card::new();
    assert!(!h.load(bad.to_str().unwrap(), &mut err), "load_bad");
    assert!(!h.inserted());
    assert_eq!(rd(&mut h), 0xff, "load_bad_r — no-card data_r = 0xFF (cpp:247)");
    std::fs::remove_file(&bad).ok();

    // exact-size but short file -> fread short-circuit -> eject (cpp:157-161)
    let short = dir.join(format!("smu_card_short_{}.bin", std::process::id()));
    std::fs::write(
        &short,
        vec![0x11u8; 16 * 1024 * 1024 / 512 * 528 / 2],
    )
    .unwrap();
    let mut s = Card::new();
    assert!(!s.load(short.to_str().unwrap(), &mut err), "load_short");
    assert!(!s.inserted(), "load_short ins — eject on short read");
    std::fs::remove_file(&short).ok();
}

#[test]
fn gt_128mb_four_address_cycles_and_other_codes() {
    // V9 + tail: create(128) pages>65536 -> 4 cycles; page 0x012345 lives
    // beyond the 3-cycle range and must program/read via the 4th byte.
    let mut b = Card::new();
    assert!(b.create(128));
    assert_eq!(b.address_cycles(), 4, "ac128");
    cle(&mut b, 0x80);
    ale(&mut b, 0);
    ale(&mut b, 0x45);
    ale(&mut b, 0x23);
    ale(&mut b, 0x01);
    for i in 0..528 {
        dat(&mut b, pat3(i));
    }
    cle(&mut b, 0x10);
    cle(&mut b, 0x00);
    ale(&mut b, 0);
    ale(&mut b, 0x45);
    ale(&mut b, 0x23);
    ale(&mut b, 0x01);
    let got: Vec<u8> = (0..16).map(|_| rd(&mut b)).collect();
    assert_eq!(
        got,
        [0x05, 0x10, 0x1b, 0x26, 0x31, 0x3c, 0x47, 0x52, 0x5d, 0x68, 0x73, 0x7e, 0x89, 0x94, 0x9f, 0xaa],
        "big128_16 (pat3 readback at page 0x012345)"
    );
    let mut bl: Vec<smu_machine::card::Block> = Vec::new();
    b.take_dirty_blocks(&mut bl);
    assert_eq!(bl.len(), 8192, "big_blocks");
    assert_eq!(bl[bl.len() - 1].index, 8191, "big_blk_idx");
    drop(b); // 138MB image back

    // device codes (smartmedia.cpp:43-49)
    for (mb, code) in [(16u32, 0x73u8), (64, 0x76), (128, 0x79)] {
        let mut x = Card::new();
        assert!(x.create(mb));
        cle(&mut x, 0x90);
        assert_eq!(rd(&mut x), 0x98, "maker {mb}");
        assert_eq!(rd(&mut x), code, "id code {mb}");
    }

    // unsupported size: false + STATE UNTOUCHED (create_bad GT)
    let mut z = Card::new();
    assert!(!z.create(8));
    assert!(!z.create(7));
    assert_eq!(z.pages, 0);
    assert!(z.raw().is_empty());

    // eject keeps the latch fields (GT eject_ctrl 1), kills the data
    let mut c = Card::new();
    assert!(c.create(16));
    c.control_w(0x01);
    let head = gt(c.raw(), 0, 16);
    c.eject();
    assert!(!c.inserted());
    assert_eq!(c.data_r(), 0xff);
    assert_eq!(c.ctrl, 1, "eject_ctrl — eject touches data only (cpp:42)");
    let mut c2 = Card::new();
    assert!(c2.create(16));
    assert_eq!(gt(c2.raw(), 0, 16), head, "create is deterministic");
}

#[test]
fn machine_panel_and_seams() {
    use smu_machine::{button_name, Button};
    let mut m = smu_machine::Machine::new(Vec::new());

    // set_button drives the shared sws rows (mu2000.cpp:537-551, active-low)
    assert!(!m.button_pressed(Button::Bass));
    m.set_button(Button::Bass, true);
    assert_eq!(m.sws.borrow()[0], 0xff & !(1 << 3));
    assert!(m.button_pressed(Button::Bass));
    m.set_button(Button::Bass, false);
    assert_eq!(m.sws.borrow()[0], 0xff);

    // row 5 bit 7 = Sampling/Mode; row 3 bit 1 = Part +
    m.set_button(Button::SamplingMode, true);
    assert_eq!(m.sws.borrow()[5], 0x7f);
    m.set_button(Button::PartPlus, true);
    assert_eq!(m.sws.borrow()[3], 0xfd);
    assert_eq!(m.sws.borrow()[0], 0xff);

    // button table (mu2000.cpp:510-524) — first/last + out-of-range ""
    assert_eq!(button_name(Button::Strings), "Strings");
    assert_eq!(button_name(Button::SamplingMode), "Sampling/Mode");
    assert_eq!(button_name(Button::ChromPerc), "Chrom. Perc.");
    assert_eq!(Button::Count as i32, 35); // static_assert mu2000.cpp:526
    assert!(Button::from_i32(35).is_none());
    assert!(Button::from_i32(-1).is_none());

    // PA19/PA20 follow the REAL card (mu2000.cpp:1121-1125): read port A
    // through the bus — PADR r32 fires the Hub pins closure (:1083-1136)
    assert_eq!(m.soc.bus.read_long(0xffff_8380) >> 19 & 1, 0, "no card, PA19 low");
    assert!(m.card.borrow_mut().create(16));
    let pins = m.soc.bus.read_long(0xffff_8380);
    assert_eq!(pins >> 19 & 1, 1, "PA19 inserted");
    assert_eq!(pins >> 20 & 1, 1, "PA20 not write-protected");
    m.card.borrow_mut().write_protected = true;
    let pins = m.soc.bus.read_long(0xffff_8380);
    assert_eq!(pins >> 19 & 1, 1);
    assert_eq!(pins >> 20 & 1, 0, "PA20 low while write-protected");

    // the bus arms drive the SAME card object (card.rs data_r is real now)
    m.card.borrow_mut().control_w(0x01);
    assert_eq!(m.soc.bus.read_byte(0x00c0_0000), 0xff, "idle read 0xFF, not stub-0");

    // lcd_ddram passthrough (hd44780 m_ddram, 128 cells)
    let dd = m.lcd_ddram();
    assert_eq!(dd.len(), 0x80);
    assert_eq!(dd, m.lcd.borrow().m_ddram);
}
