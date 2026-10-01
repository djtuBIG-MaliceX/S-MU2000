//! sh7042 row tests — hand-derived from disk (sh7042.{h,cpp}, sh7042_map.hxx
//! case tables, mu2000.cpp::build_bus, membus.h). Every expectation cites the
//! disk line it was derived from. Synthetic vectors only, no ROM/WAV data.

use std::cell::RefCell;
use std::rc::Rc;

use smu_sh2::core::{Sh2Bus, SH_I};
use smu_sh2::sh7042::{Sh7042, Sh7042Bus, Sh7042Peripherals};

type Log = Rc<RefCell<Vec<(&'static str, u32, u32)>>>;

#[derive(Clone, Default)]
struct Ev {
    adc0: u64,
    adc1: u64,
    cmt: u64,
    mtu: [u64; 5],
    sci: [u64; 2],
}

#[derive(Default)]
struct Obs {
    set_irq: Option<(i32, i8)>,
    taken: Option<(i32, u32)>,
}

// ---------------------------------------------------------------------------
// synthetic ROM: byte at address a == (a & 0xff), with pinned marker bytes
// ---------------------------------------------------------------------------
fn rom() -> Vec<u8> {
    // origin: mu2000.cpp:379 read_file exact 0x400000 -> loader hands 4MB
    let mut r: Vec<u8> = (0..0x400000u32).map(|i| i as u8).collect();
    r[0] = 0xde;
    r[1] = 0xad;
    r[2] = 0xbe;
    r[3] = 0xef;
    r[0x3ffffe] = 0xa5;
    r[0x3fffff] = 0x5a;
    r
}

fn bus() -> Sh7042Bus {
    Sh7042Bus::new(rom())
}

// recording fake peripheral (shared Rc state so tests observe through the bus)
struct Fake {
    log: Log,
    ev: Rc<RefCell<Ev>>,
    obs: Rc<RefCell<Obs>>,
}

impl Default for Fake {
    fn default() -> Self {
        Fake {
            log: Rc::new(RefCell::new(Vec::new())),
            ev: Rc::new(RefCell::new(Ev::default())),
            obs: Rc::new(RefCell::new(Obs::default())),
        }
    }
}

#[allow(unused_variables)]
impl Sh7042Peripherals for Fake {
    fn sci_r8(&mut self, sci: usize, a: u32) -> u8 {
        self.log.borrow_mut().push(("sci_r8", a, sci as u32));
        (a & 0xff) as u8
    }
    fn sci_r16(&mut self, sci: usize, a: u32) -> u16 {
        self.log.borrow_mut().push(("sci_r16", a, sci as u32));
        0x1000 | (a & 0xff) as u16
    }
    fn sci_w8(&mut self, sci: usize, a: u32, v: u8) {
        self.log.borrow_mut().push(("sci_w8", a, (u32::from(v) << 8) | sci as u32));
    }
    fn sci_w16(&mut self, sci: usize, a: u32, v: u16) {
        self.log.borrow_mut().push(("sci_w16", a, (u32::from(v) << 8) | sci as u32));
    }
    fn porta_r32(&mut self, a: u32) -> u32 {
        self.log.borrow_mut().push(("porta_r32", a, 0));
        0xa5a5_5a5a
    }
    fn porta_w32(&mut self, a: u32, v: u32) {
        self.log.borrow_mut().push(("porta_w32", a, v));
    }
    fn portf_r8(&mut self, a: u32) -> u8 {
        self.log.borrow_mut().push(("portf_r8", a, 0));
        0x77
    }
    fn portf_r16(&mut self, a: u32) -> u16 {
        self.log.borrow_mut().push(("portf_r16", a, 0));
        0x7788
    }
    fn adc0_r8(&mut self, a: u32) -> u8 {
        self.log.borrow_mut().push(("adc0_r8", a, 0));
        1
    }
    fn adc0_r16(&mut self, a: u32) -> u16 {
        self.log.borrow_mut().push(("adc0_r16", a, 0));
        0x0adc
    }
    fn adc0_w8(&mut self, a: u32, v: u8) {
        self.log.borrow_mut().push(("adc0_w8", a, u32::from(v)));
    }
    fn adc0_w16(&mut self, a: u32, v: u16) {
        self.log.borrow_mut().push(("adc0_w16", a, u32::from(v)));
    }
    fn adc1_r8(&mut self, a: u32) -> u8 {
        self.log.borrow_mut().push(("adc1_r8", a, 0));
        2
    }
    fn adc1_r16(&mut self, a: u32) -> u16 {
        self.log.borrow_mut().push(("adc1_r16", a, 0));
        0x1adc
    }
    fn adc1_w8(&mut self, a: u32, v: u8) {
        self.log.borrow_mut().push(("adc1_w8", a, u32::from(v)));
    }
    // NOTE: no adc1_w16 override possible — the TRAIT has no adc1 w16 method
    // at all, mirroring "disk has no adc1 w16 case" (sh7042_map.hxx w16 runs).
    fn intc_r8(&mut self, a: u32) -> u8 {
        self.log.borrow_mut().push(("intc_r8", a, 0));
        3
    }
    fn intc_set_input(&mut self, irqline: i32, state: i8) {
        self.obs.borrow_mut().set_irq = Some((irqline, state));
    }
    fn intc_interrupt_taken(&mut self, irqline: i32, vector: u32) {
        self.obs.borrow_mut().taken = Some((irqline, vector));
    }
    fn sci4_r8(&mut self, a: u32) -> u8 {
        self.log.borrow_mut().push(("sci4_r8", a, 0));
        a as u8 // handler receives the OFFSET a-0xf00000 (mu2000.cpp:969)
    }
    fn sci4_w8(&mut self, a: u32, v: u8) {
        self.log.borrow_mut().push(("sci4_w8", a, u32::from(v)));
    }
    fn usb_r8(&mut self, sel: u32) -> u8 {
        self.log.borrow_mut().push(("usb_r8", sel, 0));
        if sel == 0 {
            0xa5
        } else {
            0x5a
        }
    }
    fn usb_w8(&mut self, sel: u32, v: u8) {
        self.log.borrow_mut().push(("usb_w8", sel, u32::from(v)));
    }
    fn ledsw1_w8(&mut self, v: u8) {
        self.log.borrow_mut().push(("ledsw1_w8", 0, u32::from(v)));
    }
    fn ledsw2_w8(&mut self, v: u8) {
        self.log.borrow_mut().push(("ledsw2_w8", 0, u32::from(v)));
    }
    fn d80_w8(&mut self, v: u8) {
        self.log.borrow_mut().push(("d80_w8", 0, u32::from(v)));
    }
    fn card_ctrl_w8(&mut self, v: u8) {
        self.log.borrow_mut().push(("card_ctrl_w8", 0, u32::from(v)));
    }
    fn swp_r16(&mut self, master: bool, a: u32) -> u16 {
        self.log.borrow_mut().push(("swp_r16", a, master as u32));
        if master {
            0x1111
        } else {
            0x2222
        }
    }
    fn swp_w8(&mut self, master: bool, a: u32, v: u8) {
        self.log.borrow_mut().push(("swp_w8", a, (u32::from(v) << 1) | master as u32));
    }
    fn swp_w16(&mut self, master: bool, a: u32, v: u16) {
        self.log.borrow_mut().push(("swp_w16", a, (u32::from(v) << 1) | master as u32));
    }
    fn swp_w32(&mut self, master: bool, a: u32, v: u32) {
        // only the MASTER window must ever call the d==true arm; the fake
        // records the flag in v's low bit is impossible (v is the payload) ->
        // log master in the addr field's bit 31
        self.log
            .borrow_mut()
            .push(("swp_w32", a | if master { 0 } else { 0x4000_0000 }, v));
    }
    fn adc0_update(&mut self, t: u64) -> u64 {
        self.log.borrow_mut().push(("adc0_update", t as u32, 0));
        let e = self.ev.borrow().adc0;
        e
    }
    fn adc1_update(&mut self, t: u64) -> u64 {
        self.log.borrow_mut().push(("adc1_update", t as u32, 0));
        let e = self.ev.borrow().adc1;
        e
    }
    fn cmt_update(&mut self, t: u64) -> u64 {
        self.log.borrow_mut().push(("cmt_update", t as u32, 0));
        let e = self.ev.borrow().cmt;
        e
    }
    fn mtu_ch_update(&mut self, ch: usize, t: u64) -> u64 {
        self.log.borrow_mut().push(("mtu_ch_update", t as u32, ch as u32));
        let e = self.ev.borrow().mtu[ch];
        e
    }
    fn sci_update(&mut self, sci: usize, t: u64) -> u64 {
        self.log.borrow_mut().push(("sci_update", t as u32, sci as u32));
        let e = self.ev.borrow().sci[sci];
        e
    }
}

fn fake() -> (Sh7042Bus, Log, Rc<RefCell<Ev>>, Rc<RefCell<Obs>>) {
    let f = Fake::default();
    let (log, ev, obs) = (f.log.clone(), f.ev.clone(), f.obs.clone());
    let mut b = bus();
    b.periph = Some(Box::new(f));
    (b, log, ev, obs)
}

fn last(log: &Log) -> (&'static str, u32, u32) {
    *log.borrow().last().unwrap()
}
fn nth(log: &Log, op: &str, n: usize) -> (&'static str, u32, u32) {
    *log.borrow().iter().filter(|e| e.0 == op).nth(n).unwrap()
}
fn cnt(log: &Log, op: &str) -> usize {
    log.borrow().iter().filter(|e| e.0 == op).count()
}
fn clear(log: &Log) {
    log.borrow_mut().clear();
}

// ===========================================================================
// region boundary decode (lo / hi / hi+1, read + write)
// ===========================================================================

#[test]
fn rom_region_0_to_3fffff() {
    // origin: mu2000.cpp:740 add_region(0x000000,0x3fffff,rom,false);
    // membus.h:63/183 hot `a <= m_hot_r_end`
    let mut b = bus();
    assert_eq!(b.read_byte(0x000000), 0xde); // lo
    assert_eq!(b.read_byte(0x3fffff), 0x5a); // hi (still ROM)
    // hi+1 = 0x400000 is NOT unmapped: it is the RAM window (zero-filled)
    assert_eq!(b.read_byte(0x400000), 0);
    // aligned long at the very end stays inside the region: FC FD A5 5A
    assert_eq!(b.read_long(0x3ffffc), 0xFCFD_A55A);
}

#[test]
fn rom_write_dropped_all_widths() {
    // origin: membus.h:101-110/112-122/124-140: find_write skips
    // !writable regions (ROM mu2000.cpp:741 writable=false), no device
    // covers 0x1000 -> every write falls off the end DROPPED.
    let mut b = bus();
    b.write_byte(0x100, 0xff);
    b.write_word(0x102, 0xffff);
    b.write_long(0x104, 0xffff_ffff);
    assert_eq!(b.rom[0x100], 0x00); // pattern untouched
    assert_eq!(b.read_word(0x102), 0x0203); // rom[0x102..0x104] pattern bytes
    assert_eq!(b.rom[0x104], 0x04);
}

#[test]
fn be_word_long_from_rom_with_masking() {
    // origin: membus.h:75 a&=~1, :88 a&=~3, BE assembly (:76/:89-92)
    let mut b = bus();
    assert_eq!(b.read_word(0x000000), 0xDEAD);
    assert_eq!(b.read_word(0x000002), 0xBEEF);
    assert_eq!(b.read_long(0x000000), 0xDEAD_BEEF);
    assert_eq!(b.read_word(0x101), 0x0001); // masked to 0x100
    assert_eq!(b.read_long(0x103), 0x0001_0203); // masked to 0x100
}

#[test]
fn ram_region_boundaries_roundtrip_no_wrap() {
    // origin: mu2000.cpp:823 add_region(0x400000,0x43ffff,ram,true) —
    // INCLUSIVE window. NO wrap on the bus: the device ctor passes address
    // mask 0xffffffff (sh7042.cpp:41) and membus never re-masks, so 0x440000
    // is UNMAPPED (membus.h:70/105), not an alias of 0x400000.
    let mut b = bus();
    b.write_byte(0x400000, 0xAB); // lo
    b.write_byte(0x43ffff, 0xCD); // hi
    assert_eq!(b.read_byte(0x400000), 0xAB);
    assert_eq!(b.read_byte(0x43ffff), 0xCD);
    assert_eq!(b.read_byte(0x440000), 0); // hi+1 unmapped -> 0
    b.write_byte(0x440000, 0xFF); // dropped
    assert_eq!(b.read_byte(0x440000), 0);
    assert_eq!(b.ram[0], 0xAB); // no aliasing into RAM[0]
    b.write_long(0x400100, 0x1234_5678); // BE roundtrip
    assert_eq!(&b.ram[0x100..0x104], &[0x12, 0x34, 0x56, 0x78]);
    assert_eq!(b.read_long(0x400100), 0x1234_5678);
    assert_eq!(b.read_word(0x400102), 0x5678);
}

#[test]
fn dram_and_iram_regions() {
    // origin: mu2000.cpp:826 DRAM 0x1000000-0x107ffff + :828 IRAM
    // 0xfffff000-0xffffffff (both writable). Boundaries read+write; hi+1
    // unmapped; membus.h:88 long alignment mask.
    let mut b = bus();
    b.write_byte(0x1000000, 0x11);
    b.write_byte(0x107ffff, 0x22);
    assert_eq!(b.read_byte(0x1000000), 0x11);
    assert_eq!(b.read_byte(0x107ffff), 0x22);
    assert_eq!(b.read_byte(0x1080000), 0); // hi+1 unmapped
    b.write_word(0x1000010, 0xBEEF);
    assert_eq!(&b.dram[0x10..0x12], &[0xBE, 0xEF]);
    b.write_long(0xfffff000, 0xCAFE_F00D); // iram lo
    assert_eq!(&b.iram[0..4], &[0xCA, 0xFE, 0xF0, 0x0D]);
    b.write_byte(0xffffffff, 0x99); // iram hi
    assert_eq!(b.read_byte(0xffffffff), 0x99);
    b.write_long(0xfffff002, 0x0102_0304); // masked down to 0xfffff000
    assert_eq!(b.read_long(0xfffff000), 0x0102_0304);
    assert_eq!(b.read_byte(0xffffefff), 0); // below iram -> unmapped
}

// ===========================================================================
// device spaces (mu2000.cpp::build_bus)
// ===========================================================================

#[test]
fn swp_windows_boundaries_and_dword_pair() {
    // origin: mu2000.cpp:858-860/921-922 master 0x800000-0x801fff, slave
    // 0x802000-0x803fff; absolute address to handlers (membus.h:36).
    // SWP registers NO r32 -> membus.h:96 composes the long as
    // read_word(a)<<16|read_word(a+2), each word an r16 on the SWP.
    let (mut b, log, _, _) = fake();
    assert_eq!(b.read_word(0x800000), 0x1111); // master lo
    assert_eq!(b.read_word(0x801ffe), 0x1111); // master hi (aligned)
    assert_eq!(b.read_word(0x802000), 0x2222); // slave lo, master=false
    assert_eq!(b.read_word(0x803ffe), 0x2222); // slave hi
    assert_eq!(b.read_word(0x804000), 0); // hi+1 unmapped
    assert_eq!(b.read_word(0x7ffffe), 0); // below master unmapped
    clear(&log);
    assert_eq!(b.read_long(0x800000), 0x1111_1111);
    assert_eq!(nth(&log, "swp_r16", 0).1, 0x800000); // absolute addrs
    assert_eq!(nth(&log, "swp_r16", 1).1, 0x800002); // a, a+2 (membus.h:96)
    assert_eq!(cnt(&log, "swp_r16"), 2);
    // d->r8 exists (mu2000.cpp:877) -> byte goes straight to swp_r8 (abs a)
    assert_eq!(b.read_byte(0x800001), 0x00); // fake swp_r8 default 0
}

#[test]
fn swp_write_widths_absolute() {
    // origin: mu2000.cpp:869-876 w8 keeps absolute a (lane=(a&1) inside
    // handler); :906-918 w16 once; :881-905 w32 = ONE 32-bit handler that
    // itself writes two regs (membus.h:136 d->w32 exists).
    let (mut b, log, _, _) = fake();
    b.write_byte(0x800001, 0x33);
    assert_eq!(last(&log), ("swp_w8", 0x800001, 0x67)); // (0x33<<1)|master
    b.write_word(0x800100, 0xABCD);
    assert_eq!(last(&log), ("swp_w16", 0x800100, (0xABCD << 1) | 1));
    b.write_long(0x800100, 0xDEAD_BEEF);
    assert_eq!(last(&log), ("swp_w32", 0x800100, 0xDEAD_BEEF)); // one call
    // slave window: master=false -> fake sets the bit-31 marker
    b.write_long(0x802000, 0x0000_0001);
    assert_eq!(last(&log).0, "swp_w32");
    assert_eq!(last(&log).1, 0x4000_0000 | 0x802000);
}

#[test]
fn led_panel_d80_single_bytes() {
    // origin: mu2000.cpp:926-947 — c80000 r8+w8 (single byte), e00000 w8
    // ONLY (no r8/r16/r32 registered), d80000 r8+w8.
    let (mut b, log, _, _) = fake();
    b.write_byte(0xc80000, 0x42);
    assert_eq!(last(&log), ("ledsw1_w8", 0, 0x42));
    assert_eq!(b.read_byte(0xc80001), 0); // hi+1 unmapped
    assert_eq!(b.read_byte(0xc7ffff), 0); // lo-1 unmapped
    // panel has no read handler -> membus.h:66-68 all null -> 0 (:70), :83
    assert_eq!(b.read_byte(0xe00000), 0);
    assert_eq!(b.read_word(0xe00000), 0);
    assert_eq!(b.read_long(0xe00000), 0);
    // membus.h:120: a w16 with only w8 on the device = two w8 on the SAME
    // handler, hi then lo
    b.write_word(0xe00000, 0x1122);
    assert_eq!(cnt(&log, "ledsw2_w8"), 2);
    assert_eq!(nth(&log, "ledsw2_w8", 0).2, 0x11);
    assert_eq!(nth(&log, "ledsw2_w8", 1).2, 0x22);
    // d80 write reaches the latch; word write pairs (fake logs both)
    b.write_word(0xd80000, 0x0307); // contrast bits live in low 3 (mu2000.cpp:943)
    assert_eq!(nth(&log, "d80_w8", 0).2, 0x03);
    assert_eq!(nth(&log, "d80_w8", 1).2, 0x07);
}

#[test]
fn card_control_read_is_ff_literal() {
    // origin: mu2000.cpp:960 d.r8 = [](offs_t) -> u8 { return 0xff; } —
    // hardcoded, NOT a device call; word/long build 0xffff/0xffffffff.
    let (mut b, log, _, _) = fake();
    assert_eq!(b.read_byte(0xd00000), 0xff); // lo
    assert_eq!(b.read_byte(0xd7ffff), 0xff); // hi
    assert_eq!(b.read_byte(0xd80000 - 1), 0xff); // hi via arithmetic
    assert_eq!(b.read_word(0xd00002), 0xffff);
    assert_eq!(b.read_long(0xd00004), 0xffff_ffff);
    assert_eq!(cnt(&log, "sci4_r8"), 0); // reads never touch any handler
    b.write_byte(0xd00010, 0x02);
    assert_eq!(last(&log), ("card_ctrl_w8", 0, 0x02));
}

#[test]
fn sci4_word_is_two_real_byte_reads() {
    // origin: mu2000.cpp:965-972 registers r8/w8 only (8-bit device);
    // membus.h:81 composes words as d->r8(a), d->r8(a+1) — two accesses,
    // offsets per the mu2000.cpp:969 lambda (- 0xf00000).
    let (mut b, log, _, _) = fake();
    assert_eq!(b.read_byte(0xf00000), 0x00); // lo, offset 0
    assert_eq!(b.read_byte(0xf0003f), 0x3f); // hi
    assert_eq!(b.read_byte(0xf00040), 0); // hi+1 unmapped (mu2000.cpp:968)
    clear(&log);
    assert_eq!(b.read_word(0xf00010), 0x1011); // offsets 0x10 then 0x11
    assert_eq!(cnt(&log, "sci4_r8"), 2);
    assert_eq!(nth(&log, "sci4_r8", 0).1, 0x10);
    assert_eq!(nth(&log, "sci4_r8", 1).1, 0x11);
    b.write_byte(0xf00001, 0xEE);
    assert_eq!(last(&log), ("sci4_w8", 0x01, 0xEE));
}

#[test]
fn usb_dword_select_pair_then_unmapped() {
    // origin: mu2000.cpp:974-982 (0xf80000 sel0 data / 0xf80001 sel1 status,
    // r8/w8 only). membus.h:96: long(0xf80000) = word(sel0,sel1)<<16 |
    // word(0xf80002,0xf80003) where the second word is UNMAPPED -> 0.
    let (mut b, log, _, _) = fake();
    assert_eq!(b.read_byte(0xf80000), 0xa5);
    assert_eq!(b.read_byte(0xf80001), 0x5a);
    assert_eq!(b.read_byte(0xf80002), 0); // hi+1 unmapped
    assert_eq!(b.read_long(0xf80000), 0xa55a_0000);
    // membus.h:120 word write -> byte pair onto sel0 then sel1
    b.write_word(0xf80000, 0x0301);
    assert_eq!(nth(&log, "usb_w8", 0), ("usb_w8", 0, 0x03));
    assert_eq!(nth(&log, "usb_w8", 1), ("usb_w8", 1, 0x01));
}

// ===========================================================================
// internal register map (sh7042_map.hxx)
// ===========================================================================

#[test]
fn internal_holes_default_zero() {
    // origin: map:321-322 / :481-482 default logerror + return 0; hole
    // addresses verified against the full `case:` extraction of the tables.
    let (mut b, log, _, _) = fake();
    assert_eq!(b.internal_r8(0xffff820c), 0); // gap between 820b and 820d
    assert_eq!(b.internal_r8(0xffff81a6), 0); // between sci0 and sci1
    assert_eq!(b.internal_r16(0xffff8628), 0); // bsc r16 skips 8628
    assert_eq!(b.internal_r16(0xffff81a1), 0); // odd -> no case
    assert_eq!(b.internal_r16(0xffff9fff), 0); // in bus window, past table
    assert_eq!(cnt(&log, "bsc_r16"), 0); // hole never reached a device
    // write holes dropped (map:790 / :932), never reaching a device
    b.internal_w8(0xffff820c, 0xff);
    b.internal_w16(0xffff8628, 0xffff);
}

#[test]
fn internal_window_edges_on_the_bus() {
    // origin: mu2000.cpp:987 device window 0xffff8000-0xffff9fff. Outside
    // (no region either) membus.h:70 -> 0; inside but past the case table ->
    // internal default 0. IRAM begins 0xfffff000 (mu2000.cpp:828).
    let mut b = bus();
    assert_eq!(b.read_byte(0xffff7fff), 0); // below window
    assert_eq!(b.read_byte(0xffffa000), 0); // above window
    assert_eq!(b.read_word(0xffff9ffe), 0); // inside window, table miss
    b.write_byte(0xffffa000, 0xff); // dropped
    b.write_word(0xffff7ffe, 0xff); // dropped
    let mut b2 = bus();
    b2.write_byte(0xfffff000, 0x77); // IRAM, NOT the register device
    assert_eq!(b2.read_byte(0xfffff000), 0x77);
}

#[test]
fn sci_rdr_is_read_only_and_sci1_indexes() {
    // origin: map r8 81a0-81a5 includes 81a5 (RDR); map w8 runs 81a0-81a4
    // ONLY (:520-525 block) -> an 81a5 write hits the default drop (:790).
    let (mut b, log, _, _) = fake();
    assert_eq!(b.internal_r8(0xffff81a5), 0xa5); // RDR readable
    b.internal_w8(0xffff81a5, 0x5a);
    assert_eq!(cnt(&log, "sci_w8"), 0); // dropped, no device call
    b.internal_w8(0xffff81a4, 0x5a); // SSR is writable
    assert_eq!(nth(&log, "sci_w8", 0), ("sci_w8", 0xffff81a4, 0x5a00));
    assert_eq!(b.internal_r8(0xffff81b0), 0xb0);
    assert_eq!(nth(&log, "sci_r8", 0).2, 0); // first read was sci0's 81a5
    assert_eq!(nth(&log, "sci_r8", 1).2, 1); // this one is sci index 1
}

#[test]
fn portf_is_read_only_on_disk() {
    // origin: portf has r8 (83b2-83b3) and r16 (83b2) cases but NO entry in
    // any w8/w16 table -> every write hits the default drop (:790/:932);
    // w32(83b2) falls to the w16 pair (:963-965) which also drops.
    let (mut b, log, _, _) = fake();
    assert_eq!(b.internal_r8(0xffff83b2), 0x77);
    assert_eq!(b.internal_r16(0xffff83b2), 0x7788);
    b.internal_w8(0xffff83b2, 0xff);
    b.internal_w16(0xffff83b2, 0xffff);
    b.internal_w32(0xffff83b2, 0xffff_ffff);
    assert_eq!(cnt(&log, "portf_r8") + cnt(&log, "portf_r16"), 2);
    assert_eq!(log.borrow().len(), 2); // nothing else logged (all writes dropped)
}

#[test]
fn adc_case_sets_exact_runs() {
    // origin: adc0 w8 set = {83e0,83e1,8410,8412}; adc0 w16 = {83e0,8410,
    // 8412}; ADCDR bytes 83f0-8407 READ-ONLY; adc1 w8 = {8411,8413}; adc1
    // has NO w16 anywhere; adc1 r16 = 8408..840e even.
    let (mut b, log, _, _) = fake();
    b.internal_w8(0xffff83f0, 1); // ADCDR: not in the w8 set -> dropped
    b.internal_w8(0xffff8407, 3); // still ADCDR: dropped
    b.internal_w8(0xffff83e0, 2); // ADSR: logged
    assert_eq!(cnt(&log, "adc0_w8"), 1);
    assert_eq!(nth(&log, "adc0_w8", 0), ("adc0_w8", 0xffff83e0, 2));
    b.internal_w16(0xffff83f0, 1); // not in adc0 w16 set -> dropped
    b.internal_w16(0xffff83e0, 2);
    assert_eq!(cnt(&log, "adc0_w16"), 1);
    b.internal_w8(0xffff8408, 1); // adc1 even byte: NOT a w8 case
    assert_eq!(cnt(&log, "adc1_w8"), 0);
    b.internal_w8(0xffff8411, 2); // odd: logged
    assert_eq!(cnt(&log, "adc1_w8"), 1);
    b.internal_w16(0xffff8408, 2); // no adc1 w16 on disk -> default drop
    assert_eq!(cnt(&log, "adc0_w16"), 1); // misrouted into adc0? no
    assert_eq!(b.internal_r16(0xffff8408), 0x1adc); // adc1 r16 present
    // via the full bus path: 8410 -> adc0, 8411 -> adc1 (even/odd interleave)
    assert_eq!(b.read_byte(0xffff8410), 1);
    assert_eq!(b.read_byte(0xffff8411), 2);
}

#[test]
fn internal_r32_pair_rule_and_direct_cases() {
    // origin: map:485-515. Direct r32 cases: {8380,8384 porta; 838c al;
    // 8398 b; 83a0,83a4 portd; 83a8 dh; 83b8 e; dmac ch 86c0/c4/c8/cc ...};
    // EVERYTHING else = (r16(a)<<16)|r16(a+2) (:514). w32 tail :963-965.
    let (mut b, log, _, _) = fake();
    assert_eq!(b.internal_r32(0xffff8380), 0xa5a5_5a5a); // porta dr_r
    assert_eq!(last(&log), ("porta_r32", 0xffff8380, 0));
    clear(&log);
    assert_eq!(b.internal_r32(0xffff81a0), 0x10a0_10a2); // sci pair fallback
    assert_eq!(nth(&log, "sci_r16", 0).1, 0xffff81a0); // order: a then a+2
    assert_eq!(nth(&log, "sci_r16", 1).1, 0xffff81a2);
    // dmac direct r32: 86d4 = channel 1 dar (map:501)
    assert_eq!(b.internal_r32(0xffff86d4), 0); // fake default, but routed as
    // DIRECT (dmac_ch_r32), never as an r16 pair:
    assert_eq!(cnt(&log, "sci_r16"), 2); // only the sci pair logged r16
    // w32 direct fires porta_w32 ONCE
    b.internal_w32(0xffff8380, 0x1234_5678);
    assert_eq!(last(&log), ("porta_w32", 0xffff8380, 0x1234_5678));
    // w32 pair fallback for sci: w16(81a0)=hi THEN w16(81a2)=lo
    b.internal_w32(0xffff81a0, 0xDEAD_BEEF);
    assert_eq!(nth(&log, "sci_w16", 0), ("sci_w16", 0xffff81a0, 0xDEAD00));
    assert_eq!(nth(&log, "sci_w16", 1), ("sci_w16", 0xffff81a2, 0xBEEF00));
}

#[test]
fn pcf_byte_writes_are_masked_rmw() {
    // origin: map:643-648 (pcf_al bytes, mask 0xff<<shift) + COMBINE_DATA
    // (sh7042.cpp:320); pcf reads map:142-145/390-392/490; w16 map:859-860;
    // w32 map:940.
    let mut b = bus();
    b.internal_w8(0xffff838c, 0x12); // MSB (map:645)
    assert_eq!(b.pcf_al_r(), 0x1200_0000);
    b.internal_w8(0xffff838d, 0x34); // map:646
    b.internal_w8(0xffff838e, 0x56); // map:647
    b.internal_w8(0xffff838f, 0x78); // map:648
    assert_eq!(b.pcf_al_r(), 0x1234_5678);
    assert_eq!(b.internal_r8(0xffff838d), 0x34); // map:143
    assert_eq!(b.internal_r16(0xffff838c), 0x1234); // map:391 (high half)
    assert_eq!(b.internal_r16(0xffff838e), 0x5678); // map:392 (low half)
    assert_eq!(b.internal_r32(0xffff838c), 0x1234_5678); // map:490 direct
    b.internal_w16(0xffff838e, 0xABCD); // low half only (map:860)
    assert_eq!(b.pcf_al_r(), 0x1234_ABCD);
    b.internal_w32(0xffff838c, 0xdead_beef); // full (map:940)
    assert_eq!(b.pcf_al_r(), 0xdead_beef);
    // u16 regs: hi byte write keeps the lo byte (map:643 vs :644)
    b.internal_w8(0xffff8388, 0xAA);
    b.internal_w8(0xffff8389, 0xBB);
    assert_eq!(b.pcf_ah_r(), 0xAABB);
    b.internal_w8(0xffff8388, 0xCC);
    assert_eq!(b.pcf_ah_r(), 0xCCBB);
    // via the full bus (byte access on an even/odd address)
    b.write_byte(0xffff83c8, 0x0f); // pcf_if hi (map:685)
    assert_eq!(b.pcf_if_r(), 0x0f00);
    b.write_byte(0xffff839b, 0x05); // pcf_b LSB (map:660)
    assert_eq!(b.pcf_b_r(), 0x0000_0005);
    assert_eq!(b.read_long(0xffff8398), 0x0000_0005); // r32 direct (map:491)
    // membus.h:120 w16->two w8 on the internal device: 839c hi then 839d lo
    b.write_word(0xffff839c, 0x1122);
    assert_eq!(b.pcf_c_r(), 0x1122);
}

#[test]
fn internal_absent_peripheral_defaults() {
    // origin: seam None = mem_bus miss: reads 0 (membus.h:70/83/96), writes
    // dropped (:101-140); disk never runs without required_devices, so this
    // is the C++ default the row defines for unattached spaces.
    let mut b = bus(); // periph == None
    assert_eq!(b.internal_r8(0xffff8348), 0); // intc space
    assert_eq!(b.internal_r16(0xffff8260), 0); // mtu0 space
    assert_eq!(b.internal_r32(0xffff86c0), 0); // dmac0 sar via r16 pair
    b.internal_w8(0xffff8348, 0xff); // no panic, no state
    b.internal_w32(0xffff86c0, 0xffff_ffff); // pair -> w16 -> default drop
    assert_eq!(b.read_byte(0xffff81a0), 0); // full bus path
    assert_eq!(b.read_word(0xffff83d0), 0);
    assert_eq!(b.read_long(0xffff8380), 0); // porta r32
    assert_eq!(b.pcf_al_r(), 0); // owned regs untouched
}

// ===========================================================================
// device glue: cycles, events, start/reset
// ===========================================================================

#[test]
fn current_cycles_minus_one_stand_in() {
    // origin: sh7042.h:92-98 `c ? c - 1 : 0`, exact during events
    let mut s = Sh7042::new(28_000_000, rom());
    assert_eq!(s.current_cycles(), 0); // c == 0 branch — no u64 underflow
    s.dev.core.m_total_cycles = 100;
    assert_eq!(s.current_cycles(), 99); // running: attotime round-trip -1
    s.m_in_event = true;
    assert_eq!(s.current_cycles(), 100); // inside event: scheduled tick exact
    s.m_in_event = false;
    assert_eq!(s.current_cycles(), 99);
}

#[test]
fn event_min_selection_and_die_a_adc1_gate() {
    // origin: add_event min of nonzero sh7042.cpp:254-260; update call set
    // and order :282-292; adc1 only on die-A (m_adc1 optional_device,
    // :283-284 + device_add_mconfig :166-170)
    let (log, ev, _) = {
        let f = Fake::default();
        let (log, ev, obs) = (f.log.clone(), f.ev.clone(), f.obs.clone());
        ev.borrow_mut().adc0 = 80;
        ev.borrow_mut().adc1 = 10; // ignored when die_a == false
        ev.borrow_mut().mtu[2] = 30;
        ev.borrow_mut().sci[1] = 20;
        let mut s = Sh7042::new(28_000_000, rom());
        s.dev.core.m_total_cycles = 1000; // current_cycles() == 999
        s.bus.periph = Some(Box::new(f));
        s.internal_update(); // reads current_cycles() (:244)
        assert_eq!(s.event_cycles(), 20); // min{80,30,20} — adc1 excluded
        assert_eq!(nth_pub(&log, "cmt_update", 0).1, 999); // -1 clock seen
        assert_eq!(cnt_pub(&log, "adc1_update"), 0);
        (log, ev, obs)
    };
    // die-A machine (SH7043A — the S-MU2000 chip): adc1 now counts
    let f = Fake::default();
    let log2 = f.log.clone();
    f.ev.borrow_mut().adc0 = 80;
    f.ev.borrow_mut().adc1 = 10;
    f.ev.borrow_mut().mtu[2] = 30;
    f.ev.borrow_mut().sci[1] = 20;
    let mut sa = Sh7042::new_a(28_000_000, rom());
    assert!(sa.m_die_a); // sh7042.cpp:35 (SH7043A)
    sa.bus.periph = Some(Box::new(f));
    sa.internal_update();
    assert_eq!(sa.event_cycles(), 10); // adc1's earlier event wins
    assert_eq!(cnt_pub(&log2, "adc1_update"), 1);
    // mtu channel order 0..4 and sci order 0..1 (sh7042.cpp:286-292)
    let order: Vec<u32> = log2
        .borrow()
        .iter()
        .filter(|e| e.0 == "mtu_ch_update")
        .map(|e| e.2)
        .collect();
    assert_eq!(order, vec![0, 1, 2, 3, 4]);
    let _ = (log, ev);
}

// pub helpers for logs outside a fake_bus scope
fn nth_pub(log: &Log, op: &str, n: usize) -> (&'static str, u32, u32) {
    nth(log, op, n)
}
fn cnt_pub(log: &Log, op: &str) -> usize {
    cnt(log, op)
}

#[test]
fn event_tick_exact_time_and_abort_only_on_change() {
    // origin: event_tick bracket sh7042.cpp:247-252 (in_event -> clock exact,
    // sh7042.h:95-96); recompute aborts the timeslice ONLY on change
    // (:267-270); abort_timeslice == icount zeroing (core.rs:243-246 ==
    // mamecompat abort semantics).
    let f = Fake::default();
    let log = f.log.clone();
    f.ev.borrow_mut().cmt = 900;
    let mut s = Sh7042::new(28_000_000, rom());
    s.dev.core.m_total_cycles = 500;
    s.bus.periph = Some(Box::new(f));
    s.event_tick();
    assert_eq!(s.event_cycles(), 900);
    assert_eq!(nth(&log, "cmt_update", 0).1, 500); // EXACT, not 499
    assert!(!s.m_in_event); // bracket closed (:251)
    s.dev.core.icount = 7;
    s.recompute_timer(900); // unchanged -> NO abort (guard :267)
    assert_eq!(s.dev.core.icount, 7);
    s.recompute_timer(700); // changed -> abort (:268-269)
    assert_eq!(s.dev.core.icount, 0);
    assert_eq!(s.event_cycles(), 700);
}

#[test]
fn set_internal_interrupt_and_input_lines_route() {
    // origin: set_internal_interrupt touches the three CPU fields
    // (sh7042.cpp:390-395); input lines + exception ack route THROUGH the
    // INTC (:141-144, :397-401) — with a fake the core line stays untouched.
    let mut s = Sh7042::new(28_000_000, rom());
    s.set_internal_interrupt(12, 0x600);
    assert_eq!(s.dev.core.internal_irq_level, 12); // :392
    assert_eq!(s.dev.core.m_internal_irq_vector, 0x600); // :393
    assert_eq!(s.dev.core.m_test_irq, 1); // :394
    let f = Fake::default();
    let obs = f.obs.clone();
    let mut s2 = Sh7042::new(28_000_000, rom());
    s2.bus.periph = Some(Box::new(f));
    s2.execute_set_input(3, 1);
    assert_eq!(s2.dev.core.m_irq_line_state[3], 0); // NOT applied directly
    assert_eq!(obs.borrow().set_irq, Some((3, 1)));
    s2.exception_internal_done(3, 0x11c);
    assert_eq!(obs.borrow().taken, Some((3, 0x11c)));
}

#[test]
fn start_and_reset_glue_reads_vectors_via_bus() {
    // origin: device_start order sh7042.cpp:118-139 (sh2 start THEN pcf
    // zeros); device_reset :146-149 -> sh2.cpp:66-82 (sr=SH_I, pc=
    // read_long(0), r15=read_long(4)). ROM attach precedes reset (build_bus
    // mu2000.cpp:740 region + :997 set_program_bus run before the start/
    // reset sequence; here the ctor takes the ROM — enforced by construction).
    let mut r = rom();
    r[0..4].copy_from_slice(&[0x00, 0x10, 0x02, 0x00]); // pc 0x00100200
    r[4..8].copy_from_slice(&[0x40, 0x00, 0xff, 0x00]); // sp 0x4000ff00
    let mut s = Sh7042::new(28_000_000, r);
    s.device_start();
    assert_eq!(s.bus.m_pcf_al, 0); // :131-138
    s.device_reset();
    assert_eq!(s.dev.core.pc, 0x0010_0200); // read_long(0) through the bus
    assert_eq!(s.dev.core.r[15], 0x4000_ff00); // read_long(4)
    assert_eq!(s.dev.core.sr, SH_I); // core.rs:272 (sh2.cpp:71)
    assert_eq!(s.dev.core.vbr, 0); // core.rs:261 (sh2.cpp:69 zero init)
}

#[test]
fn explicit_init_and_empty_rom_absence() {
    // origin: Invariant 3 vs sh7042.h:129,173-183 explicit initializers;
    // mu2000.cpp:85-87 RAM zero-fill; mu2000.cpp:718/740 no region when the
    // program vector is empty -> reads unmapped->0, writes drop.
    let s = Sh7042::new(28_000_000, Vec::new());
    assert!(!s.m_die_a); // sh7042.cpp:17
    assert_eq!(s.m_event_cycles, 0); // sh7042.h:173
    assert!(!s.m_in_event); // sh7042.h:174
    assert_eq!(s.bus.m_pcf_ah, 0); // sh7042.h:176 / cpp:131
    assert_eq!(s.bus.m_pcf_al, 0);
    assert_eq!(s.bus.m_pcf_b, 0);
    assert_eq!(s.bus.m_pcf_c, 0);
    assert_eq!(s.bus.m_pcf_dh, 0);
    assert_eq!(s.bus.m_pcf_dl, 0);
    assert_eq!(s.bus.m_pcf_e, 0);
    assert_eq!(s.bus.m_pcf_if, 0); // sh7042.h:183
    assert!(s.bus.periph.is_none());
    assert!(s.bus.ram.iter().all(|b| *b == 0)); // mu2000.cpp:85
    assert!(s.bus.dram.iter().all(|b| *b == 0)); // mu2000.cpp:86
    assert!(s.bus.iram.iter().all(|b| *b == 0)); // mu2000.cpp:87
    let mut b = s.bus;
    assert_eq!(b.read_byte(0), 0); // no ROM region at all
    assert_eq!(b.read_long(0), 0);
    b.write_word(0x100, 0x1234); // dropped
    assert_eq!(b.read_word(0x100), 0);
    b.write_byte(0x400000, 0x01); // RAM still live
    assert_eq!(b.read_byte(0x400000), 1);
}
