//! Phase-B bit-exactness replay of the SWP30 voice engine
//! (`src/mame/sound/swp30.cpp` :1070-1366, :1517-1546, :1833-1884, :2556-2621,
//! regs :2014-2177 + keyon) against ground-truth vectors captured from
//! `%TEMP%\voicegtB\gt.cpp` — a standalone g++ harness embedding the C++
//! bodies byte-exact (incl. the streaming_block reused from the fetch row).
//! No ROMs: the 4 MiB wave buffer is LCG-synthesized identically on both sides
//! (same keys as tests/data/vectors.txt, fetch row).
//!
//! Float gates (`filter_impulse`) compare BIT PATTERNS (`to_bits()`), never
//! epsilon (Invariant 9; the divisor is a power of two).
//! The awm2 crown drives `Swp30::write16`/`keyon_w` through the REAL reg
//! dispatch, then `Channels::awm2_step` per sample.

use std::sync::OnceLock;

use smu_swp30::fetch::Wave;
use smu_swp30::regs::Swp30;
use smu_swp30::voice::{filter_impulse, lfo_pitch_trace, FilterBlock, Iir1Block};

const FILTER: &str = include_str!("data/voice2_filter.txt");
const TABLES: &str = include_str!("data/voice2_tables.txt");
const AWM2: &str = include_str!("data/voice2_awm2.txt");

/// wave LCG identical to gt.cpp main() (fetch-row generator): s = s*6364136223846793005
/// + 1442695040888963407; byte = (s >> 33) as u8, seed 0x123456789ABCDEF0.
fn wave_bytes() -> &'static [u8] {
    static W: OnceLock<Vec<u8>> = OnceLock::new();
    W.get_or_init(|| {
        let mut s: u64 = 0x123456789ABCDEF0;
        (0..(1usize << 22))
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (s >> 33) as u8
            })
            .collect()
    })
}

fn hx(s: &str) -> u32 {
    u32::from_str_radix(s, 16).unwrap_or_else(|_| panic!("bad hex {s:?}"))
}

/// input patterns from gt.cpp `pat()` — keep in lockstep with the harness.
fn pat(kind: i32, i: u32, lcg: &mut u64) -> i16 {
    match kind {
        0 => {
            if i == 0 {
                0x4000
            } else {
                0
            }
        }
        1 => {
            if (i / 16) % 2 == 1 {
                -0x2000
            } else {
                0x2000
            }
        }
        2 => {
            if i & 1 == 1 {
                -0x7fff
            } else {
                0x7fff
            }
        }
        3 => {
            *lcg = lcg
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((*lcg >> 33) as u16) as i16
        }
        _ => {
            if (i / 64) % 2 == 1 {
                -0x6000
            } else {
                0x5a5a
            }
        }
    }
}

#[test]
fn filter_scenarios_bit_exact() {
    // f.<tag>,<kind>,<f1a>,<l1>,<f2a>,<l2>,<fb>,<nev>,<at:slot:val>*|<out>*
    let mut nlines = 0;
    for line in FILTER.lines().filter(|l| l.starts_with("f.")) {
        nlines += 1;
        let (head, outs) = line.split_once('|').unwrap();
        let f: Vec<&str> = head.split(',').filter(|s| !s.is_empty()).collect();
        let kind = f[1].parse::<i32>().unwrap();
        let (f1a, l1, f2a, l2, fb) = (hx(f[2]) as u16, hx(f[3]) as u16, hx(f[4]) as u16, hx(f[5]) as u16, hx(f[6]) as u16);
        let nev: usize = f[7].parse().unwrap();
        let mut events: Vec<(u32, i32, u16)> = Vec::new();
        for e in &f[8..8 + nev] {
            let mut it = e.split(':');
            // `at` is serialized with %u (gt.cpp:169) => DECIMAL; only `val` is hex.
            events.push((it.next().unwrap().parse().unwrap(), it.next().unwrap().parse().unwrap(), hx(it.next().unwrap()) as u16));
        }
        let outs: Vec<&str> = outs.split(',').filter(|s| !s.is_empty()).collect();

        let mut fl = FilterBlock::new();
        fl.clear();
        fl.filter_1_a_w(f1a);
        fl.level_1_w(l1);
        fl.filter_2_a_w(f2a);
        fl.level_2_w(l2);
        fl.filter_b_w(fb);
        fl.keyon();
        let mut lcg = 0xfeedfacecafebeefu64;
        let mut k = 0usize;
        for (i, want) in outs.iter().enumerate() {
            while k < events.len() && events[k].0 == i as u32 {
                let (_, slot, val) = events[k];
                k += 1;
                match slot {
                    0 => fl.filter_1_a_w(val),
                    1 => fl.level_1_w(val),
                    2 => fl.filter_2_a_w(val),
                    3 => fl.level_2_w(val),
                    4 => fl.filter_b_w(val),
                    5 => fl.keyon(),
                    _ => unreachable!(),
                }
            }
            let got = fl.step(pat(kind, i as u32, &mut lcg));
            assert_eq!(hx(want), got as u32 & 0xffff_ffff, "f.{kind} step{i}");
        }
    }
    assert_eq!(nlines, 5); // 5 scen_filter calls in gt.cpp:389-401 (tags 1..5)
}

#[test]
fn iir1_scenarios_bit_exact() {
    // i.<tag>,<kind>,<a00>,<a01>,<b0>,<a10>,<a11>,<b1>,<rewrite>,<n_a00>,<n_b1>,<out>*
    let mut nlines = 0;
    for line in FILTER.lines().filter(|l| l.starts_with("i.")) {
        nlines += 1;
        let f: Vec<&str> = line.split(',').collect();
        let kind = f[1].parse::<i32>().unwrap();
        let (a00, a01, b0, a10, a11, b1) =
            (hx(f[2]) as u16, hx(f[3]) as u16, hx(f[4]) as u16, hx(f[5]) as u16, hx(f[6]) as u16, hx(f[7]) as u16);
        let (rewrite, n_a00, n_b1) = (f[8].parse::<u32>().unwrap(), hx(f[9]) as u16, hx(f[10]) as u16);
        let outs: Vec<&str> = f[11..].iter().copied().filter(|s| !s.is_empty()).collect();

        let mut g = Iir1Block::new();
        g.clear();
        g.a0_w(0, a00);
        g.a1_w(0, a01);
        g.b1_w(0, b0);
        g.a0_w(1, a10);
        g.a1_w(1, a11);
        g.b1_w(1, b1);
        g.keyon();
        let mut lcg = 0x0123456789abcdefu64;
        for (i, want) in outs.iter().enumerate() {
            if rewrite != 0 && i as u32 == rewrite {
                g.a0_w(0, n_a00);
                g.b1_w(1, n_b1);
            }
            if rewrite != 0 && i as u32 == rewrite + 137 {
                g.keyon();
            }
            let input: i32 = match kind {
                0 => {
                    if i == 0 {
                        0x400000
                    } else {
                        0
                    }
                }
                1 => {
                    if (i as u32 / 8) % 2 == 1 {
                        -0x20000
                    } else {
                        0x20000
                    }
                }
                _ => (pat(3, i as u32, &mut lcg) as i32) << 6,
            };
            assert_eq!(hx(want), g.step(input) as u32 & 0xffff_ffff, "i.{kind} step{i}");
        }
    }
    assert_eq!(nlines, 3);
}

#[test]
fn filter_impulse_float_bit_exact() {
    // t.fi,<f1a>,<l1>,<f2a>,<l2>,<fb>, then 192 f32 bit patterns (Invariant 9)
    let mut ncfg = 0;
    for line in TABLES.lines().filter(|l| l.starts_with("t.fi,")) {
        ncfg += 1;
        let f: Vec<&str> = line.split(',').collect();
        let (f1a, l1, f2a, l2, fb) = (hx(f[1]) as u16, hx(f[2]) as u16, hx(f[3]) as u16, hx(f[4]) as u16, hx(f[5]) as u16);
        let mut out = [0.0f32; 192];
        filter_impulse(f1a, l1, f2a, l2, fb, &mut out);
        for i in 0..192 {
            assert_eq!(hx(f[6 + i]), out[i].to_bits(), "t.fi cfg{ncfg} sample{i}");
        }
    }
    assert_eq!(ncfg, 14);
}

#[test]
fn lfo_pitch_trace_bit_exact() {
    // t.lpt,<tsp> then 600 i16 hex (type 3 => all zeros, :1536-1540)
    let mut nc = 0;
    for line in TABLES.lines().filter(|l| l.starts_with("t.lpt,")) {
        nc += 1;
        let f: Vec<&str> = line.split(',').collect();
        let mut out = [0i16; 600];
        lfo_pitch_trace(hx(f[1]) as u16, &mut out);
        for i in 0..600 {
            assert_eq!(hx(f[2 + i]), out[i] as u16 as u32, "t.lpt {:#x} i{i}", hx(f[1]));
        }
    }
    assert_eq!(nc, 5);
}

/// the crown: 6-channel full-pipeline scenario (driver mirror of gt.cpp
/// `scen_awm2`) driven through the REAL write16/keyon_w dispatch.
struct Cfg {
    addr: u32,
    start: u32,
    loop_: u32,
    pitch: u16,
    att: u16,
    d1: u16,
    d2: u16,
    rg: u16,
    tsp: u16,
    amp: u16,
    iir_a0: u16,
    iir_b1: u16,
    f1a: u16,
    l1: u16,
    f2a: u16,
    l2: u16,
    fb: u16,
    pegofs: u16,
    pegrade: u16,
}

const CF: [Cfg; 6] = [
    Cfg { addr: 0x0010_0000, start: 0, loop_: 0x4000, pitch: 0x0555, att: 0x2710, d1: 0x1420, d2: 0x2a30, rg: 0x1919,
          tsp: 0x4110, amp: 0x20, iir_a0: 0x2000, iir_b1: 0x0000, f1a: 0x0240, l1: 0x2a, f2a: 0x0120, l2: 0x30, fb: 0x1800,
          pegofs: 0x0000, pegrade: 0x0000 },
    Cfg { addr: 0x0011_0000, start: 0x4000_0800, loop_: 0x1800,
          pitch: 0x1000, att: 0x1f08, d1: 0x2215, d2: 0x3025, rg: 0x2404,
          tsp: 0xc111, amp: 0x40, iir_a0: 0x2000, iir_b1: 0x0000, f1a: 0x7240, l1: 0x20, f2a: 0x0120, l2: 0x40, fb: 0x0800,
          pegofs: 0x0200, pegrade: 0x4000 },
    Cfg { addr: 0x8020_0000, start: 0, loop_: 0x3000, pitch: 0x07ff, att: 0x1010, d1: 0x0a40, d2: 0x2b40, rg: 0x1b00,
          tsp: 0x0000, amp: 0x00, iir_a0: 0x1999, iir_b1: 0x1555, f1a: 0x0240, l1: 0x28, f2a: 0xa120, l2: 0x34, fb: 0x0fff,
          pegofs: 0x0000, pegrade: 0x0000 },
    Cfg { addr: 0x4030_0000, start: 0, loop_: 0x2800, pitch: 0x0320, att: 0x2018, d1: 0x1815, d2: 0x3525, rg: 0x1f0f,
          tsp: 0x4040, amp: 0x7f, iir_a0: 0x2000, iir_b1: 0x6000, f1a: 0x6140, l1: 0x3c, f2a: 0x0000, l2: 0x00, fb: 0x0400,
          pegofs: 0x0000, pegrade: 0x0000 },
    Cfg { addr: 0xc240_0000, start: 0, loop_: 0x2000, pitch: 0x3c00, att: 0x3308, d1: 0x1e15, d2: 0x3d25, rg: 0x2904,
          tsp: 0x8111, amp: 0x10, iir_a0: 0x2000, iir_b1: 0x0000, f1a: 0x0140, l1: 0x30, f2a: 0x0220, l2: 0x28, fb: 0x0c00,
          pegofs: 0x3c00, pegrade: 0x3010 },
    Cfg { addr: 0x0012_0000, start: 0, loop_: 0x2400, pitch: 0x0800, att: 0x4810, d1: 0x1030, d2: 0x3020, rg: 0x1919,
          tsp: 0x0000, amp: 0x7f, iir_a0: 0x2000, iir_b1: 0x0000, f1a: 0x0240, l1: 0x24, f2a: 0x0000, l2: 0x00, fb: 0x0000,
          pegofs: 0x0000, pegrade: 0x0000 },
];

#[test]
fn awm2_crown_via_real_dispatch() {
    // collect expected lines by key
    let aw: std::collections::HashMap<u32, Vec<&'static str>> = AWM2
        .lines()
        .filter(|l| l.starts_with("aw."))
        .map(|l| {
            let mut it = l.split(',');
            let i: u32 = it.next().unwrap()[3..].parse().unwrap();
            (i, it.collect())
        })
        .collect();
    let asx: std::collections::HashMap<(usize, u32), Vec<&'static str>> = AWM2
        .lines()
        .filter(|l| l.starts_with("as."))
        .map(|l| {
            let mut it = l.split(',');
            let tag = it.next().unwrap(); // as.<ch>.<i>
            let mut p = tag[3..].split('.');
            let ch: usize = p.next().unwrap().parse().unwrap();
            let i: u32 = p.next().unwrap().parse().unwrap();
            ((ch, i), it.collect())
        })
        .collect();

    let wave = Wave::new(wave_bytes());
    let mut swp = Swp30::new(); // == ctor + reset (swp30.cpp:1909)

    for (c, x) in CF.iter().enumerate() {
        let o = (c << 6) as u32;
        swp.write16(o | 0x12, (x.start >> 16) as u16);
        swp.write16(o | 0x13, x.start as u16);
        swp.write16(o | 0x14, (x.loop_ >> 16) as u16);
        swp.write16(o | 0x15, x.loop_ as u16);
        swp.write16(o | 0x16, (x.addr >> 16) as u16);
        swp.write16(o | 0x17, x.addr as u16);
        swp.write16(o | 0x11, x.pitch);
        swp.write16(o | 0x06, x.att);
        swp.write16(o | 0x07, x.d1);
        swp.write16(o | 0x08, x.d2);
        swp.write16(o | 0x09, x.rg);
        swp.write16(o | 0x0a, x.tsp);
        swp.write16(o | 0x05, x.amp);
        swp.write16(o | 0x00, x.f1a);
        swp.write16(o | 0x01, x.l1);
        swp.write16(o | 0x02, x.f2a);
        swp.write16(o | 0x03, x.l2);
        swp.write16(o | 0x04, x.fb);
        swp.write16(o | 0x24, x.iir_a0); // a0<0>
        swp.write16(o | 0x20, 0); // a1<0>
        swp.write16(o | 0x22, 0); // b1<0>
        swp.write16(o | 0x2a, 0x2000); // a0<1> = 1.0 passthrough
        swp.write16(o | 0x28, 0); // a1<1>
        swp.write16(o | 0x26, x.iir_b1); // b1<1> (disk order: 0x26 is b1, NOT a1!)
        swp.write16(o | 0x10, x.pegofs);
        swp.write16(o | 0x0b, x.pegrade);
    }

    // keyon ch0..4 via the real dispatch (mask low half + strobe)
    swp.write16(0x1cf, 0x001f);
    swp.write16(0x1ce, 0);
    swp.write16(0x18f, 0);
    swp.write16(0x18e, 0);
    assert_eq!(hx(&awm2_lines("a.seed,")[0][7..]), swp.rand_seed);
    swp.write16(0x20e, 0);
    // a.seed2 = seed-after, mask: 5 keyed voices (one is type3 => 2 rand)
    let s2: Vec<&str> = awm2_lines("a.seed2,")[0].split(',').collect();
    assert_eq!(hx(s2[1]), swp.rand_seed, "keyon rand consumption");
    assert_eq!(hx(s2[2]) as u64, swp.keyon_mask);

    let mut out = [0i32; 0x40];
    for i in 0..4608u32 {
        if i == 1500 {
            swp.write16((1 << 6) | 0x10, 0x3a00);
        }
        if i == 2000 {
            swp.write16((2 << 6) | 0x09, 0x80d0);
        }
        if i == 2600 {
            swp.write16(0x00, 0xb240);
            swp.write16(0x03, 0x11);
        }
        if i == 3000 {
            swp.write16((3 << 6) | 0x16, 0x0041);
            swp.write16((3 << 6) | 0x15, 0x1400);
        }
        if i == 3300 {
            swp.write16((4 << 6) | 0x11, 0x1111);
            swp.write16((4 << 6) | 0x10, 0x0c00);
        }
        if i == 3600 {
            swp.write16(0x05, 0x7f);
        }
        if i == 1024 {
            swp.write16(0x1cf, 0x0020);
            swp.write16(0x20e, 0);
            let k: Vec<String> = awm2_lines("a.keyon5,")[0]
                .split(',')
                .map(|s| s.to_string())
                .collect();
            let v = &swp.voices.voices[5];
            let st = v.streaming.state();
            assert_eq!(hx(&k[1]), swp.rand_seed, "keyon5 seed");
            assert_eq!(hx(&k[2]), st.pos as u32, "keyon5 pos");
            assert_eq!(hx(&k[3]) as u8, v.envelope.m_envelope_mode);
            assert_eq!(hx(&k[4]), v.envelope.m_envelope_level as u32 & 0xffff);
            assert_eq!(hx(&k[5]), v.lfo.m_counter);
            assert_eq!(hx(&k[6]), v.peg_cur as u32);
            assert_eq!(k[7].parse::<u32>().unwrap() as u8, v.peg_reached);
            assert_eq!(k[8].parse::<u32>().unwrap(), st.done as u32);
            // a.idle1024: harness printed m_awm_idle at the SAME point
            // (after the ch5 keyon_w :2248 wake; ch1 already idle at 1024).
            // Bits compared on the 6 CONFIGURED channels only: the harness
            // struct never runs device reset(), so its untouched ch6-63
            // envelopes sit in the ACTIVE ctor state and can never idle,
            // while the real device (and Rust `Swp30::new()`, ctor reset
            // :1969+) legitimately idles them — a provably output- and
            // rand-neutral difference (type-0 LFO draws no rand).
            let idl: Vec<&str> = awm2_lines("a.idle1024,")[0].split(',').collect();
            let want_idle = ((hx(idl[1]) as u64) << 32) | hx(idl[2]) as u64;
            assert_eq!(want_idle & 0x3f, swp.awm_idle & 0x3f, "awm_idle mask at i==1024");
            assert_ne!(swp.awm_idle & !0x3f, 0, "reset-cleared ch6+ must idle on Rust side");
        }

        let seed = &mut swp.rand_seed;
        let idle = &mut swp.awm_idle;
        let vs = &mut swp.voices;
        vs.awm2_step(&wave, seed, idle, i, &mut out, &mut None, 0, 0, 0);

        let want = aw.get(&i).unwrap_or_else(|| panic!("aw.{i} missing"));
        assert_eq!(want.len(), 7, "aw.{i} arity");
        for c in 0..6 {
            assert_eq!(hx(want[c]), out[c] as u32 & 0xffff_ffff, "aw.{i} ch{c}");
        }
        assert_eq!(hx(want[6]), swp.rand_seed, "aw.{i} seed");

        if i & 511 == 511 {
            for c in 0..6usize {
                let w = asx.get(&(c, i)).unwrap_or_else(|| panic!("as.{c}.{i} missing"));
                let v = &swp.voices.voices[c];
                let st = v.streaming.state();
                assert_eq!(hx(w[0]) as u8, v.envelope.m_envelope_mode, "as mode {c}");
                assert_eq!(hx(w[1]), v.envelope.m_envelope_level as u32 & 0xffff, "as level {c}");
                assert_eq!(hx(w[2]), v.lfo.m_counter, "as lfoctr {c}");
                assert_eq!(hx(w[3]), v.peg_cur as u32, "as peg {c}");
                assert_eq!(hx(w[4]), v.filter.m_filter_1_y0 as u32, "as y0 {c}");
                assert_eq!(w[5].parse::<u32>().unwrap() as u8, v.peg_reached, "as reached {c}");
                assert_eq!(hx(w[6]), st.pos as u32, "as pos {c}");
                assert_eq!(hx(w[7]), st.pos_dec as u32 & 0x7fff, "as dec {c}");
                assert_eq!(w[8].parse::<u32>().unwrap(), st.first as u32, "as first {c}");
                assert_eq!(w[9].parse::<u32>().unwrap(), st.finetune_active as u32, "as fine {c}");
                assert_eq!(w[10].parse::<u32>().unwrap(), st.done as u32, "as done {c}");
                assert_eq!(hx(w[11]), v.iir1.m_hy[1] as u32, "as hy1 {c}");
                assert_eq!(hx(w[12]), swp.rand_seed, "as seed {c}");
            }
        }
    }
    // final idle mask (a.idle,<hi>,<lo> — gt.cpp scen_awm2 tail; ch0-5 all
    // live again at end; ch6-63 comparison excluded, see i==1024 note)
    let idl = awm2_lines("a.idle,")[0].replace("a.idle,", "");
    let w: Vec<&str> = idl.split(',').collect();
    assert_eq!(((hx(w[0]) as u64) << 32) | hx(w[1]) as u64, swp.awm_idle & 0x3f, "final awm_idle ch0-5");
    // every access above went to a REAL handler: nothing new deferred
    assert_eq!(swp.deferred_hits, 0, "voice-path ops must not hit deferred arms");
}

fn awm2_lines(prefix: &str) -> Vec<&'static str> {
    AWM2.lines().filter(|l| l.starts_with(prefix)).collect()
}

#[test]
fn keyon_routing_across_all_64_channels() {
    // k.rw/k.p/k.m/k.v = keyon_mask_semantics + per-voice keyon effect +
    // exact rand consumption (types 0-2: 1 rand, type 3: 2)
    let mut swp = Swp30::new();
    for c in 0..64u32 {
        let o = c << 6;
        swp.write16(o | 0x12, (0x0010 + c) as u16);
        swp.write16(o | 0x13, (0x3456 + c) as u16);
        swp.write16(o | 0x14, 0);
        swp.write16(o | 0x15, (0x1000 + c) as u16);
        swp.write16(o | 0x16, 0x0012);
        swp.write16(o | 0x17, (c * 4) as u16);
        swp.write16(o | 0x11, (0x0400 + c) as u16);
        swp.write16(o | 0x06, (0x2700 + c) as u16);
        swp.write16(o | 0x0a, (0x0100 + (c & 3) * 0x1000 + 0x40) as u16);
        let po = if c < 32 { (0x200 + c) as u16 } else { (0x3e00 - c) as u16 };
        swp.write16(o | 0x10, po);
        swp.write16(o | 0x0b, (((0x40 + c) << 8) & 0xffff) as u16);
    }
    // keyon_mask half writes: Sel0=0xa5a5 Sel1=0x5a5a Sel2=0x33cc Sel3=0xcc33
    swp.write16(0x1cf, 0xa5a5);
    swp.write16(0x1ce, 0x5a5a);
    swp.write16(0x18f, 0x33cc);
    swp.write16(0x18e, 0xcc33);

    let rw = awm2_lines("k.rw,")[0].replace("k.rw,", "");
    let w: Vec<&str> = rw.split(',').collect();
    // gt.cpp prints r<0>,r<1>,r<2>,r<3>,mask_hi,mask_lo
    assert_eq!(hx(w[0]), swp.read16(0x1cf) as u32); // Sel=0
    assert_eq!(hx(w[1]), swp.read16(0x1ce) as u32); // Sel=1
    assert_eq!(hx(w[2]), swp.read16(0x18f) as u32); // Sel=2
    assert_eq!(hx(w[3]), swp.read16(0x18e) as u32); // Sel=3
    let hi = (hx(w[4]) as u64) << 32;
    assert_eq!(hi | hx(w[5]) as u64, swp.keyon_mask);

    let p = awm2_lines("k.p,")[0].replace("k.p,", "");
    let mut pw = p.split(',');
    assert_eq!(hx(pw.next().unwrap()), swp.rand_seed, "seed before keyon");
    swp.write16(0x20e, 0); // keyon_w
    assert_eq!(hx(pw.next().unwrap()), swp.rand_seed, "rand consumption over all keyed voices");

    assert_eq!(hx(&awm2_lines("k.m,")[0]["k.m,".len()..]), 0);
    assert_eq!(swp.keyon_mask, 0);

    let kv = awm2_lines("k.v,")[0]["k.v,".len()..].to_string();
    for (c, cell) in kv.split(';').filter(|s| !s.is_empty()).enumerate() {
        let mut it = cell.split(':');
        assert_eq!(c as u32, hx(it.next().unwrap()), "cell index");
        let pos = hx(it.next().unwrap());
        let mode = hx(it.next().unwrap()) as u8;
        let lvl = hx(it.next().unwrap());
        let peg = hx(it.next().unwrap());
        let reached: u8 = it.next().unwrap().parse().unwrap();
        let lfo0: u32 = it.next().unwrap().parse().unwrap();
        let v = &swp.voices.voices[c];
        let st = v.streaming.state();
        assert_eq!(pos, st.pos as u32, "ch{c} pos");
        assert_eq!(mode, v.envelope.m_envelope_mode, "ch{c} mode");
        assert_eq!(lvl, v.envelope.m_envelope_level as u32 & 0xffff, "ch{c} level");
        assert_eq!(peg, v.peg_cur as u32, "ch{c} peg_cur");
        assert_eq!(reached, v.peg_reached, "ch{c} peg_reached");
        assert_eq!(lfo0, (v.lfo.m_counter == 0) as u32, "ch{c} lfo counter zero flag");
    }
    assert_eq!(swp.deferred_hits, 0);
}
