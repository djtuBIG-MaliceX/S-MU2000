//! W-SAMP1 gate — the sampling-RAM overlay, the wave-access windows
//! (0x08e/0x0ce/0x10e/0x10f/0x14e/0x14f/0x30f + writes), the AN0/AN2
//! ad_peak ladder, and the one-card-two-machines seam.
//!
//! C++ truth: mu2000.cpp:88-90 (ctor pins), mu2000.cpp:3490-3493 (peak),
//! swp30.cpp:2364-2446 (wave window), swp30.cpp:4673-4691 (rec),
//! mamecompat.h:153-201 (flat_space overlay read/write dispatch).
//! Everything is synthetic (no ROMs): the machine runs unbooted; the SWP
//! windows are driven through the REAL CPU bus (slave @ 0x802000, mu2000.cpp:922).

use smu_machine::Machine;
use smu_sh2::core::Sh2Bus;
use smu_swp30::fetch::{Wave, SAMPRAM_FROM};
use smu_swp30::SLAVE_BASE;

const MASTER_BASE: u32 = 0x0080_0000;

fn w(m: &mut Machine, reg: u32, v: u16) {
    m.soc.bus.write_word(SLAVE_BASE + 2 * reg, v); // reg = (a-base)>>1
}
fn r(m: &mut Machine, reg: u32) -> u16 {
    m.soc.bus.read_word(SLAVE_BASE + 2 * reg)
}
fn wm(m: &mut Machine, reg: u32, v: u16) {
    m.soc.bus.write_word(MASTER_BASE + 2 * reg, v);
}

/// word at sampling-RAM word index `i` (LE dword out of Machine::sample_ram)
fn ram_word(m: &Machine, i: usize) -> u32 {
    u32::from_le_bytes(m.sample_ram()[i * 4..i * 4 + 4].try_into().unwrap())
}

#[test]
fn overlay_download_and_readback_through_the_bus() {
    // 0x5000 commit pair -> sampram (NOT waverom), then the 0x9000
    // sequential read-back with busy ladder and adr/size bookkeeping.
    let mut m = Machine::new(Vec::new());
    assert_eq!(m.sample_ram().len(), 0x400000); // mu2000.cpp:88

    // arm the download window: access 0x5000, adr = 0x1000000, size 2 words
    w(&mut m, 0x08e, 0x0100); // wave_adr hi (swp30.cpp:2192)
    w(&mut m, 0x08f, 0x0000); // wave_adr lo  (:2193)
    w(&mut m, 0x0ce, 0x0000); // wave_size hi (:2194)
    w(&mut m, 0x0cf, 2); // wave_size lo  (:2195)
    w(&mut m, 0x10e, 0x5000); // wave_access (:2196 — NOT 0x8000/0x9000: no prefetch)
    w(&mut m, 0x14e, 0xdead); // val hi (latch only, :2434-2435)
    assert_eq!(ram_word(&m, 0), 0, "hi half must NOT commit alone");
    w(&mut m, 0x14f, 0xbeef); // val lo -> COMMIT dword (:2441)
    // hi half 0xDEAD was latched first, lo 0xBEEF -> dword 0xDEADBEEF
    assert_eq!(ram_word(&m, 0), 0xdead_beef);
    w(&mut m, 0x14e, 0x1111);
    w(&mut m, 0x14f, 0x2222);
    assert_eq!(ram_word(&m, 1), 0x1111_2222);
    assert_eq!(r(&mut m, 0x08e), 0x0100, "adr hi readback");
    assert_eq!(r(&mut m, 0x08f), 0x0002, "adr advanced by commits (:2442)");
    assert_eq!(r(&mut m, 0x0cf), 0, "size decremented (:2443)");

    // ROM-region write is DISCARDED (flat_space m_write null, mamecompat.h:200)
    w(&mut m, 0x10e, 0x5000);
    w(&mut m, 0x08e, 0x0000);
    w(&mut m, 0x08f, 0x0004); // word 1 of the (empty/null) waverom space
    w(&mut m, 0x0cf, 1);
    w(&mut m, 0x14e, 0xc0de);
    w(&mut m, 0x14f, 0xf00d);
    assert_eq!(ram_word(&m, 0), 0xdead_beef, "ROM-window write must drop");

    // 0x9000 sequential read-back of the two sampram words
    w(&mut m, 0x08e, 0x0100);
    w(&mut m, 0x08f, 0x0000);
    w(&mut m, 0x0cf, 2);
    w(&mut m, 0x10e, 0x9000); // prefetch word 0 (:2400-2402)
    assert_eq!(r(&mut m, 0x10f), 0x0001, "busy while words remain (:2415-2416)");
    assert_eq!(r(&mut m, 0x14e), 0xdead, "hi half of 0xDEADBEEF");
    assert_eq!(r(&mut m, 0x14f), 0xbeef, "lo half pops + advances (:2424-2428)");
    assert_eq!(r(&mut m, 0x14e), 0x1111);
    assert_eq!(r(&mut m, 0x14f), 0x2222);
    assert_eq!(r(&mut m, 0x10f), 0xffff, "done -> 0xffff (:2416)");
    assert_eq!(r(&mut m, 0x08f), 0x0002, "adr after two pops");
    assert_eq!(r(&mut m, 0x10e), 0x9000, "access readback (:2406-2409)");

    // cross-device: the MASTER shares the ONE sampram (mu2000.cpp:89-90) —
    // download through the master window, read back through the slave
    wm(&mut m, 0x08e, 0x0100);
    wm(&mut m, 0x08f, 0x0040);
    wm(&mut m, 0x0cf, 1);
    wm(&mut m, 0x10e, 0x5000);
    wm(&mut m, 0x14e, 0xabcd);
    wm(&mut m, 0x14f, 0xef01);
    assert_eq!(ram_word(&m, 0x40), 0xabcd_ef01, "master overlay write");
    w(&mut m, 0x08e, 0x0100);
    w(&mut m, 0x08f, 0x0040);
    w(&mut m, 0x0cf, 1);
    w(&mut m, 0x10e, 0x9000); // slave prefetch reads the MASTER's bytes
    assert_eq!(r(&mut m, 0x14e), 0xabcd, "slave sees master's write");

    // the window arms were the LAST deferred slots — nothing may defer now
    assert_eq!(m.swps.borrow().deferred_hits, 0, "slave deferred_hits");
    assert_eq!(m.swpm.borrow().deferred_hits, 0, "master deferred_hits");
}

#[test]
fn rec_window_ladder_and_clamp() {
    // 0x7000 recorder: MELI6 -> mixer out 8 left (the disk chain:
    // mu2000.cpp:3483-3488 AD lines + :4678-4681), two samples per word,
    // lo half first, size/adr ladder every second sample (:4681-4691).
    let mut m = Machine::new(Vec::new());

    // seed word0 through the download window (doubles as the 0x5000 seed)
    w(&mut m, 0x10e, 0x5000);
    w(&mut m, 0x08e, 0x0100);
    w(&mut m, 0x08f, 0x0000);
    w(&mut m, 0x0cf, 1);
    w(&mut m, 0x14e, 0xaaaa);
    w(&mut m, 0x14f, 0x5555);
    assert_eq!(ram_word(&m, 0), 0xaaaa_5555);

    // route mix 0x56 (MELI 6) raw to output 8 LEFT: mode bits per
    // mixer_rebuild (swp30.cpp:3001-3013, mode 2 = left only, NO atten);
    // chan 0x16 slots: route<0x40|j> at reg (0x16<<6)|0x3b..0x3d
    w(&mut m, (0x16 << 6) | 0x3b, 0x0000); // route[0] = 0
    w(&mut m, (0x16 << 6) | 0x3c, 0x0100); // route[1] bit8 -> out8 mode 2
    w(&mut m, (0x16 << 6) | 0x3d, 0x0000); // route[2] = 0
    m.swps.borrow_mut().set_meli(6, 0x0012_3400); // AD1 scale 16<<8

    // arm the take: address + length, then access 0x7000 (re-arms rec_pos)
    w(&mut m, 0x08e, 0x0100);
    w(&mut m, 0x08f, 0x0000);
    w(&mut m, 0x0cf, 2); // 2 words = 4 samples
    w(&mut m, 0x10e, 0x7000);
    assert_eq!(r(&mut m, 0x30f), 0, "0x7000 re-arms rec_pos (:2397-2398)");

    {
        let wv = Wave::new(&m.wave); // empty ROM -> zero fetch reads
        let s = m.swps.clone();
        for _ in 0..4 {
            s.borrow_mut().sample_step(&wv);
        }
    }
    // ladder: word0 = 0xAAAA5555 -> 0xAAAA1234 -> 0x12341234;
    //         word1 = 0x00000000 -> 0x00001234 -> 0x12341234
    assert_eq!(ram_word(&m, 0), 0x1234_1234, "word0 both halves");
    assert_eq!(ram_word(&m, 1), 0x1234_1234, "word1 both halves");
    assert_eq!(r(&mut m, 0x30f), 4, "rec_pos = 4 samples (:2083)");
    assert_eq!(r(&mut m, 0x08f), 2, "adr advanced once per WORD, 4 samples = 2 words (:4688)");
    assert_eq!(r(&mut m, 0x0cf), 0, "size exhausted (:4689)");

    // inert once size hits 0 (block skipped, no more writes)
    {
        let wv = Wave::new(&m.wave);
        let s = m.swps.clone();
        s.borrow_mut().sample_step(&wv);
    }
    assert_eq!(ram_word(&m, 1), 0x1234_1234, "size 0 = recorder silent");

    // clamp legs (:4682): +big -> 0x7FFF, -big -> 0x8000
    for (meli, want) in [(0x0080_0000i32, 0x7fffu16), (-0x0080_1000i32, 0x8000u16)] {
        m.swps.borrow_mut().set_meli(6, meli);
        w(&mut m, 0x08e, 0x0100);
        w(&mut m, 0x08f, 0x0010);
        w(&mut m, 0x0cf, 1);
        w(&mut m, 0x10e, 0x7000);
        {
            let wv = Wave::new(&m.wave);
            let s = m.swps.clone();
            s.borrow_mut().sample_step(&wv); // first sample -> lo half
        }
        assert_eq!(
            (ram_word(&m, 0x10) as u16, meli),
            (want, meli),
            "rec_bus>>8 clamp leg"
        );
    }
    assert_eq!(m.swps.borrow().deferred_hits, 0);
}

#[test]
fn ad_peak_ladder_by_hand() {
    // mu2000.cpp:3490-3493 exactly: a = min(abs(in),32767);
    // peak = a >= peak ? a : peak - ((peak>>12)+1)
    // zeros stand in for the sin-table ROM (>= 0x8000 entries, meg.rs:11 —
    // covers every masked LFO index; a fresh machine draws nothing anyway)
    static SINTAB0: [u16; 0x8000] = [0u16; 0x8000];

    let mut m = Machine::new(Vec::new());
    macro_rules! peak {
        () => {
            *m.ad_peak.borrow()
        };
    }

    assert_eq!(peak!(), [0, 0]); // mu2000.h:869 {} — ctor
    m.set_audio_input(40000, 0); // |40000| clamps to 32767 (:3491)
    m.run_sample_pair(&SINTAB0);
    assert_eq!(peak!(), [32767, 0], "latch + clamp to 32767");

    m.set_audio_input(0, 0);
    m.run_sample_pair(&SINTAB0);
    assert_eq!(peak!()[0], 32759, "decay: 32767-((32767>>12)+1)=32767-8");

    m.set_audio_input(-40000, 0);
    m.run_sample_pair(&SINTAB0);
    assert_eq!(peak!()[0], 32767, "abs() latch re-catches (32767 >= 32759)");

    m.set_audio_input(0, 0);
    m.run_sample_pair(&SINTAB0);
    assert_eq!(peak!()[0], 32759, "decay again");
    m.run_sample_pair(&SINTAB0);
    assert_eq!(peak!()[0], 32751, "decay ladder third step");

    // small-peak tail: peak=1 decays to 0 and STAYS 0 (never negative)
    *m.ad_peak.borrow_mut() = [0, 0]; // zero the meter for the tail steps
    m.set_audio_input(1, 0);
    m.run_sample_pair(&SINTAB0);
    assert_eq!(peak!()[0], 1);
    m.set_audio_input(0, 0);
    m.run_sample_pair(&SINTAB0);
    m.run_sample_pair(&SINTAB0);
    assert_eq!(peak!()[0], 0, "settles at 0 (1-1=0, no underflow)");

    // channel independence (AN0 vs AN2)
    m.set_audio_input(0, 20000);
    m.run_sample_pair(&SINTAB0);
    assert_eq!(peak!(), [0, 20000], "ad_peak[1] rides AN2 (meli7 line)");
}

#[test]
fn share_card_two_machines_and_pa19_follows() {
    // samptest seam: two machines, ONE card (`h.mu.card() = g.mu.card()`)
    let mut a = Machine::new(Vec::new());
    let mut b = Machine::new(Vec::new());
    assert!(!Rc_ptr_eq(&a.card, &b.card), "fresh machines own separate cards");

    b.share_card_from(&a);
    assert!(Rc_ptr_eq(&b.card, &a.card), "B now rides A's card");

    // insert through A, visible in B + PA19/PA20 on BOTH consoles
    assert!(a.card.borrow_mut().create(16));
    assert!(b.card.borrow().inserted(), "insert visible in B");
    let pins_a = a.soc.bus.read_long(0xffff_8380);
    let pins_b = b.soc.bus.read_long(0xffff_8380);
    assert_eq!(pins_a >> 19 & 1, 1, "PA19 follows on A");
    assert_eq!(pins_b >> 19 & 1, 1, "PA19 follows on B (Hub re-pinned)");
    assert_eq!(pins_b >> 20 & 1, 1, "PA20 open (not write-protected)");

    // mutate the other way: B write-protects, A's PA20 drops
    b.card.borrow_mut().write_protected = true;
    let pins_a = a.soc.bus.read_long(0xffff_8380);
    assert_eq!(pins_a >> 20 & 1, 0, "PA20 low while WP, seen on A");
    assert_eq!(a.card.borrow().megabytes(), 16, "card object is shared state");

    // the bus arm drives the shared card too (CE/idle read == 0xFF)
    b.card.borrow_mut().control_w(0x01);
    assert_eq!(a.soc.bus.read_byte(0x00c0_0000), 0xff, "A's bus sees B's ctrl_w");
}

fn Rc_ptr_eq<T>(a: &std::rc::Rc<T>, b: &std::rc::Rc<T>) -> bool {
    std::rc::Rc::ptr_eq(a, b)
}

#[test]
fn wave_overlay_dispatch_table_and_read_word_quirk() {
    // fetch.rs Wave overlay legs (mamecompat.h:153-166) at the boundaries:
    // below/first/last/after the overlay window, and the disk quirk that
    // read_word (16-bit) does NOT see the overlay (:145-148).
    let rom: Vec<u8> = (0..0x10u32).flat_map(|i| (0xA5A5_0000u32 + i).to_le_bytes()).collect();
    let mut ov: Vec<u8> = Vec::new();
    for i in 0..8u32 {
        ov.extend_from_slice(&(0x5A5A_0000u32 + i).to_le_bytes());
    }
    let wv = unsafe { Wave::with_overlay(&rom, ov.as_ptr(), (ov.len() >> 2) as u32) };

    assert_eq!(wv.read_dword(0), 0xA5A5_0000, "in-space read rides the ROM");
    assert_eq!(wv.read_dword(SAMPRAM_FROM - 1), 0xA5A5_0000 + 15, "one below: ROM wrap");
    assert_eq!(wv.read_dword(SAMPRAM_FROM), 0x5A5A_0000, "overlay first word");
    assert_eq!(wv.read_dword(SAMPRAM_FROM + 7), 0x5A5A_0007, "overlay last word");
    assert_eq!(wv.read_dword(SAMPRAM_FROM + 8), 0xA5A5_0000 + 8, "one past: ROM again");

    // DISK QUIRK: read_word never checks the overlay (mamecompat.h:145-148)
    assert_eq!(wv.read_word(SAMPRAM_FROM), 0x0000, "read_word = ROM low half (wrap), not overlay");

    // overlay OFF (Wave::new) => units 0, every addr rides the ROM (boot shape)
    let off = Wave::new(&rom);
    assert_eq!(off.read_dword(SAMPRAM_FROM), 0xA5A5_0000 + 0, "no overlay, pow2 wrap");

    // empty ROM + overlay ON: out-of-overlay reads return 0 (s_zero leg)
    let romless = unsafe { Wave::with_overlay(&[], ov.as_ptr(), (ov.len() >> 2) as u32) };
    assert_eq!(romless.read_dword(0x1234), 0, "empty base reads zero");
    assert_eq!(romless.read_dword(SAMPRAM_FROM + 3), 0x5A5A_0003, "overlay still live");
}
