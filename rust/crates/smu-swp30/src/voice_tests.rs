//! Vector tests for `voice.rs` (ledger row `voice engine`, M3 phase A).
//! `tests/data/voice_vectors.txt` was produced by a compiled-C++ harness
//! (g++ 16.1.0 -std=c++20 -O3, %TEMP%\voicegtA) over byte-extracted
//! swp30.cpp :1441-1513/:1548-1653/:1697-1833 + swp30.h :73-78/:260-297/:299-326.
//! Every (level_step, env-step, lfo, volume_apply, sounding, rand) tuple is
//! asserted exact. Vectors SHA1 75DC0F1EDFD7AC0B6EB7EE0E151E0C051CD37CC7.

use super::voice::*;

const DATA: &str = include_str!("../tests/data/voice_vectors.txt");

fn lines(prefix: &str) -> Vec<&'static str> {
    DATA.lines().filter(|l| l.starts_with(prefix)).collect()
}

// every scenario line ends with a trailing comma -> drop empty tail fields
fn fields(line: &str) -> Vec<&str> {
    line.split(',').filter(|s| !s.is_empty()).collect()
}

fn hx(s: &str) -> u32 {
    u32::from_str_radix(s, 16).unwrap_or_else(|_| panic!("bad hex {s:?}"))
}

#[test]
fn voice_gt_rand() {
    // t.rand x6 + t.seed: LCG seam (swp30.h:73-78), seed swp30.h:451
    let got: Vec<&'static str> = lines("t.rand,");
    assert_eq!(got.len(), 6);
    let mut seed = RAND_SEED_INIT;
    for l in got {
        assert_eq!(hx(l.split(',').nth(1).unwrap()), swp_rand(&mut seed));
    }
    let seed_line = fields(lines("t.seed,").pop().unwrap());
    assert_eq!(hx(seed_line[1]), seed);
}

#[test]
fn voice_gt_level_step() {
    // t.level_step: speeds -16..=128 x 20 counters (device wrapper :1548)
    const CS: [u32; 20] = [0, 1, 2, 3, 7, 8, 9, 15, 16, 17, 31, 32, 63, 64, 127, 128, 255, 256, 1023, 0x3fff];
    let line = lines("t.level_step,").pop().unwrap();
    let body = &line["t.level_step,".len()..];
    let mut n = 0usize;
    for cell in body.split(';').filter(|c| !c.is_empty()) {
        let (lv, vals) = cell.split_once(':').unwrap();
        let level = hx(lv) as i32; // two's-complement low 32 bits
        assert_eq!(vals.len(), 80);
        for (i, vi) in vals.as_bytes().chunks(4).enumerate() {
            let want = hx(std::str::from_utf8(vi).unwrap());
            assert_eq!(envelope_step(level, CS[i]), want as u16, "level_step({level:#x},{:#x})", CS[i]);
            n += 1;
        }
    }
    assert_eq!(n, 145 * 20);
}

#[test]
fn voice_gt_env_state_machine() {
    // t.env,<mode>,<att>,<d1>,<d2>,<rg>,<flags> then (mode,level,result,status,active)*
    let cases = lines("t.env,");
    assert_eq!(cases.len(), 7);
    for line in cases {
        let f = fields(line);
        let (att, d1, d2, rg, flags) = (hx(f[2]), hx(f[3]), hx(f[4]), hx(f[5]), hx(f[6]));
        let n = (f.len() - 7) / 5;
        let mut e = EnvelopeBlock::new();
        e.clear();
        e.attack_w(att as u16);
        e.decay1_w(d1 as u16);
        e.decay2_w(d2 as u16);
        if flags & 4 == 0 {
            e.release_glo_w(rg as u16);
        }
        e.keyon();
        for sc in 0..n {
            if sc == 100 && flags != 0 {
                if flags & 1 != 0 {
                    e.release_glo_w(rg as u16);
                }
                if flags & 2 != 0 {
                    e.trigger_release();
                }
                if flags & 4 != 0 {
                    e.keyon();
                }
            }
            let r = e.step(sc as u32);
            let t = 7 + sc * 5;
            assert_eq!(e.m_envelope_mode as u32, hx(f[t]), "env {att:#x} step{sc} mode");
            assert_eq!(e.m_envelope_level, hx(f[t + 1]) as i32, "env {att:#x} step{sc} level");
            assert_eq!(r as u32, hx(f[t + 2]), "env {att:#x} step{sc} result");
            assert_eq!(e.status() as u32, hx(f[t + 3]), "env {att:#x} step{sc} status");
            assert_eq!(e.active(), f[t + 4] == "1", "env {att:#x} step{sc} active");
        }
    }
}

#[test]
fn voice_gt_lfo() {
    // t.lfo,<type>,<step>,<amp>,<coarse>,<depth>,<tsp>,<drive> then (pc,counter,amp,pitch)*
    let cases = lines("t.lfo,");
    assert_eq!(cases.len(), 15);
    for line in cases {
        let f = fields(line);
        let (typ, amp, tsp, flags) = (hx(f[1]), hx(f[3]), hx(f[6]), hx(f[7]));
        let n = (f.len() - 8) / 4;
        let mut seed = RAND_SEED_INIT; // fresh swp30_device per scenario (gt.cpp scen_lfo)
        let mut l = LfoBlock::new();
        l.clear();
        l.type_step_pitch_w(tsp as u16);
        l.amplitude_w(amp as u16);
        // quirk lock: keyon consumes exactly 1 rand for types 0..2 and 2 for
        // type 3 (swp30.cpp:1715 always, :1721 only for hold-and-noise)
        let mut expect = seed;
        swp_rand(&mut expect);
        if typ == 3 {
            swp_rand(&mut expect);
        }
        l.keyon(&mut seed);
        assert_eq!(seed, expect, "keyon rand consumption type {typ}");
        for i in 0..n {
            let pc = l.m_counter;
            if flags & 0x80 != 0 {
                l.step(&mut seed);
            } else {
                assert_eq!(l.advance(), pc, "advance() must return pre-call counter ({i})");
            }
            let t = 8 + i * 4;
            assert_eq!(pc, hx(f[t]), "lfo {tsp:#x} i{i} pc");
            assert_eq!(l.m_counter, hx(f[t + 1]), "lfo {tsp:#x} i{i} counter");
            assert_eq!(l.get_amplitude() as u32, hx(f[t + 2]), "lfo {tsp:#x} i{i} amp");
            assert_eq!(l.get_pitch() as i32, hx(f[t + 3]) as i32, "lfo {tsp:#x} i{i} pitch");
        }
    }
}

#[test]
fn voice_gt_lfo_rw() {
    // t.lfo.rw,<r_tsp>,<r_amp>,<m_type>,<m_step> — the r methods echo the RAW
    // written words (:1801-1808); decoded fields cross-checked per line
    let cases = lines("t.lfo.rw,");
    assert_eq!(cases.len(), 2);
    for line in cases {
        let f = fields(line);
        let (tsp_in, amp_in) = (hx(f[1]), hx(f[2]));
        let mut l = LfoBlock::new();
        l.clear();
        l.type_step_pitch_w(tsp_in as u16);
        l.amplitude_w(amp_in as u16);
        assert_eq!(l.type_step_pitch_r() as u32, tsp_in);
        assert_eq!(l.amplitude_r() as u32, amp_in);
        assert_eq!(l.m_type as u32, hx(f[3]));
        assert_eq!(l.m_step as u32, hx(f[4]));
    }
}

#[test]
fn voice_gt_sounding() {
    // cleared envelopes are inactive (:1466); keyon makes them active (:1451)
    let f = fields(lines("t.sounding,").pop().unwrap());
    let mut env = [EnvelopeBlock::new(), EnvelopeBlock::new(), EnvelopeBlock::new(), EnvelopeBlock::new()];
    for e in env.iter_mut() {
        e.clear();
    }
    assert_eq!(sounding_voices(&env), 0); // cleared => 0 (before the first keyon)
    for (i, want) in f[1..].iter().enumerate() {
        if i < 4 {
            env[i].keyon();
        }
        assert_eq!(sounding_voices(&env), want.parse::<i32>().unwrap(), "i{i}");
    }
}

#[test]
fn voice_gt_volume() {
    // t.volume: 24 levels x 15 samples (swp30.cpp:1814-1833)
    let f = fields(lines("t.volume,").pop().unwrap());
    let lv: Vec<i32> = vec![
        0, 1, 2, 0x3ff, 0x400, 0x401, 0x7ff, 0x800, 0xc00, 0xfff, 0x1000, 0x1400, 0x1fff, 0x2000,
        0x2400, 0x2fff, 0x3000, 0x3400, 0x3bff, 0x3c00, 0x3dff, 0x3e00, 0x3fff, 0x7fff,
    ];
    let sm: Vec<i32> = vec![
        0, 1, -1, 64, -64, 0x200, -513, 0x2000, -8193, 0x20000, -131072, 1073741823, -1073741824,
        8388607, -8388608,
    ];
    assert_eq!(f.len() - 1, lv.len() * sm.len());
    let mut k = 1;
    for &l in &lv {
        for &s in &sm {
            assert_eq!(volume_apply(l, s), hx(f[k]) as i32, "volume_apply({l:#x},{s})");
            k += 1;
        }
    }
}

// ---- quirk locks: name the documented disk quirks so a future refactor
// cannot silently "fix" them (each is also covered by the gt vectors) ----

#[test]
fn quirk_release_glo_bit15_forces_release_mid_decay() {
    // swp30.cpp:1642-1646 (upstream.md 16): writing bit15 during DECAY1 jumps
    // straight to RELEASE instead of waiting for the decay2 handoff (:1587)
    let mut e = EnvelopeBlock::new();
    e.clear();
    e.attack_w(0xff10); // max attack rate: hits DECAY1 within ~8 steps
    e.decay1_w(0x1420);
    e.release_glo_w(0x0040); // bit15 clear => normal decay path
    e.keyon();
    for sc in 0..4000 {
        e.step(sc);
        if e.m_envelope_mode == EG_DECAY1 {
            break;
        }
    }
    assert_eq!(e.m_envelope_mode, EG_DECAY1);
    e.release_glo_w(0x8040);
    assert_eq!(e.m_envelope_mode, EG_RELEASE);
}

#[test]
fn quirk_keyon_zero_attack_level_and_floor_output() {
    // swp30.cpp:1454-1455 keyon lands at 0x2000, and :1564-1565 the ATTACK
    // output is the release floor, not level+release
    let mut e = EnvelopeBlock::new();
    e.clear();
    e.attack_w(0x4800); // (attack & 0xff) == 0
    e.release_glo_w(0x0019);
    e.keyon();
    assert_eq!(e.m_envelope_level, 0x2000);
    let r = e.step(0);
    assert_eq!(r, (0x19 << 6) as u16); // :1565 floor, NOT 0x2000+0x640
}

#[test]
fn quirk_tri_state_centred_vs_amplitude_phase() {
    // pitch side (tri_state, :1727-1731) starts at centre 0x800; amplitude
    // side (:1763-1766) starts at 0 — same counter, deliberately different phase
    let mut seed = RAND_SEED_INIT;
    let mut l = LfoBlock::new();
    l.clear();
    l.type_step_pitch_w(0x4000 | (1 << 8) | 0x40); // type1, step1, fine, depth 64
    l.amplitude_w(0x7f);
    l.keyon(&mut seed);
    assert_eq!(l.m_state, 0x800); // tri_state(0) == centre (the +0x10000 shift)
    assert_eq!(l.get_pitch(), 0);
    assert_eq!(l.get_amplitude(), 0); // amp uses the RAW counter (0) => silent start
}

#[test]
fn quirk_volume_grid_and_zero_clamp() {
    // :1820 clamp is >= 0x3fff; :1832 truncation lands on a multiple of 256
    assert_eq!(volume_apply(0x3fff, 0x20000), 0);
    assert_eq!(volume_apply(0x4000, -0x20000), 0);
    for level in [0x3c00i32, 0x3dff, 0x1234] {
        for s in [-8193i32, 512, 1] {
            assert_eq!(volume_apply(level, s) % 256, 0);
        }
    }
    // toward the silent side: a tiny product truncates to exactly 0 (:1830)
    assert_eq!(volume_apply(0x3e00, 1), 0);
}

// ---- phase-B quirk locks ----

#[test]
fn quirk_filter_level_hi_byte_dropped_and_ff_mutes() {
    // swp30.cpp:1199 passes the u16 register to the u8 PARAM (high byte
    // discarded); :1269-1270 hardcodes 0xff to silence
    assert_eq!(FilterBlock::volume_apply(0xff, 0x123456), 0);
    let mut a = FilterBlock::new();
    a.level_1_w(0x01ff); // u8 view == 0xff => dry leg muted
    let mut b = FilterBlock::new();
    b.level_1_w(0x00ff);
    assert_eq!(a.step(0x2000), b.step(0x2000));
    // ...and NOT equal to the u16-faithful level 0x1ff-attenuation path:
    let mut c = FilterBlock::new();
    c.level_1_w(0x01ef); // u8 view 0xef: still audible
    assert_ne!(c.step(0x2000), 0);
}

#[test]
fn quirk_keyon_seeds_peg_from_pre_written_target() {
    // swp30.cpp:2250-2252 (upstream.md 13): the pitch EG starts at the value
    // written to slot 0x10 BEFORE the keyon strobe, flagged reached;
    // a later pitch_offset_w (:2552-2558) un-arms it (voice-owned wiring)
    let mut s = crate::regs::Swp30::new();
    s.write16((7 << 6) | 0x10, 0x3c00); // -1024 in 14-bit
    s.write16((7 << 6) | 0x0b, 0x4000);
    s.write16(0x1cf, 1 << 7);
    s.write16(0x20e, 0);
    assert_eq!(s.voices.voices[7].peg_cur, -1024);
    assert_eq!(s.voices.voices[7].peg_reached, 1);
    s.write16((7 << 6) | 0x10, 0x0200);
    assert_eq!(s.voices.voices[7].peg_reached, 0);
    assert_eq!(s.deferred_hits, 0);
}
