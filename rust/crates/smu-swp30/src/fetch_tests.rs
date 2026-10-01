//! Pass-4 gap-closing replay of `src/mame/sound/swp30.cpp:315-915` against the
//! extended ground-truth vectors (`tests/data/vectors.txt`, `x.*` keys),
//! captured from the `%TEMP%\swpcap` g++ -O3 harness (byte-exact extraction;
//! flat_space stand-in now covers pow2 + `%`-wrap + empty legs of
//! mamecompat.h:130-233, legacy keys regenerate byte-identical).
//! Covers: exhaustive reader case x scale sweeps, non-pow2 and empty wave
//! spaces, 4096-call compressed runs per (mode, scale), and 4096-step step()
//! runs over every dispatch/pitch/loop/done/finetune path.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::fetch::{FetchState, StreamingBlock, Wave};

const DATA: &str = include_str!("../tests/data/vectors.txt");

fn vectors() -> &'static HashMap<&'static str, &'static str> {
    static V: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    V.get_or_init(|| {
        let mut m = HashMap::new();
        for line in DATA.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line.split_once('=').expect("vector line KEY=CSV");
            m.insert(k, v);
        }
        m
    })
}

fn get(key: &str) -> &'static str {
    vectors()
        .get(key)
        .unwrap_or_else(|| panic!("vector key missing: {key}"))
}

fn eq_str(key: &str, got: &str) {
    let want = get(key);
    assert!(want == got, "{key} mismatch\n  expected: {want}\n  got:      {got}");
}

fn zeroed() -> FetchState {
    FetchState {
        start: 0,
        loop_: 0,
        address: 0,
        pitch: 0,
        loop_size: 0,
        pos: 0,
        pos_dec: 0,
        dpcm_s0: 0,
        dpcm_s1: 0,
        dpcm_s2: 0,
        dpcm_s3: 0,
        dpcm_pos: 0,
        dpcm_delta: 0,
        first: false,
        finetune_active: false,
        done: false,
        last: 0,
    }
}

/// Same LCG as the harness `main()` and the pow2 vectors (keys wave.*).
fn wave_bytes() -> &'static [u8] {
    static W: OnceLock<Vec<u8>> = OnceLock::new();
    W.get_or_init(|| {
        let mut s: u64 = get("wave.seed").parse().unwrap();
        let a: u64 = get("wave.a").parse().unwrap();
        let c: u64 = get("wave.c").parse().unwrap();
        let bits: u32 = get("wave.bits").parse().unwrap();
        assert_eq!(bits, 22);
        let mut v = vec![0u8; 1usize << bits];
        for b in v.iter_mut() {
            s = s.wrapping_mul(a).wrapping_add(c);
            *b = (s >> 33) as u8;
        }
        v
    })
}

// --- compact SHA-1 (RFC 3174), identical to the harness SHA1 and the
//     integration-test copy in tests/fetch.rs ---------------------------------
fn sha1(msg: &[u8]) -> String {
    let mut h: [u32; 5] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
    let len_bits = (msg.len() as u64) << 3;
    let mut data = msg.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&len_bits.to_be_bytes());
    for chunk in data.chunks(64) {
        let mut w = [0u32; 80];
        for (t, item) in w.iter_mut().enumerate().take(16) {
            *item = u32::from_be_bytes([
                chunk[4 * t],
                chunk[4 * t + 1],
                chunk[4 * t + 2],
                chunk[4 * t + 3],
            ]);
        }
        for t in 16..80 {
            w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (t, wt) in w.iter().enumerate() {
            let (f, k) = match t {
                0..=19 => ((b & c) | (!b & d), 0x5a827999u32),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wt);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(v);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// Case x scale sweep per format (harness run_sweep): legs r16 {0,1},
/// r12 {0..7}, r8 {0..3} x scale {0..7}, read_vals via the pub test seam.
#[test]
fn ext_reader_case_scale_sweep() {
    let w = Wave::new(wave_bytes());
    let cases = [2usize, 8, 4];
    for mode in 0..3u32 {
        let mut s = String::new();
        for cs in 0..cases[mode as usize] {
            for scale in 0..8u32 {
                let mut st = zeroed();
                st.address = (mode << 30) | (scale << 27) | 0x0123_4560;
                st.loop_ = 0x0000_4000;
                st.pos = 512 + cs as i32;
                let mut sb = StreamingBlock::NEW;
                sb.set_state(st);
                let v = sb.read_vals(&w);
                // C++ "%d,%d,%d,%d,%d;" over (v0..v3, m_pos)
                s.push_str(&format!("{},{},{},{},{};", v[0], v[1], v[2], v[3], sb.state().pos));
            }
        }
        eq_str(&format!("x.sweep.{mode}.sha"), &sha1(s.as_bytes()));
    }
}

/// Non-pow2 wave space (`% m_bytes` leg, mamecompat.h:215): all four formats,
/// even/odd pos, both directions, scales 0..3 (harness run_np2).
#[test]
fn ext_wave_space_nonpow2() {
    let bytes = &wave_bytes()[..4_194_301]; // 4 MiB - 3, non-pow2
    let w = Wave::new(bytes);
    for mode in 0..4 {
        for i in 0..8i32 {
            let rev = (i & 1) != 0;
            let addr: u32 =
                (mode << 30) | (((i >> 1) as u32 & 7) << 27) | (0x01ff_f00 + i as u32 * 0x110);
            let loop_: i32 = if rev { 0x8000_0400u32 as i32 } else { 0x0000_0400 };
            let mut st = zeroed();
            st.address = addr;
            st.loop_ = loop_;
            st.pos = 1000 * (i + 1) + (i & 3);
            st.dpcm_s0 = 0x123;
            st.dpcm_s1 = -0x100;
            st.dpcm_s2 = 0x33;
            st.dpcm_s3 = -7;
            st.dpcm_delta = 0x1234;
            st.dpcm_pos = st.pos as u32;
            let mut sb = StreamingBlock::NEW;
            sb.set_state(st);
            let v = sb.read_vals(&w);
            let s = sb.state();
            eq_str(
                &format!("x.np2.{mode}.{i}"),
                &format!(
                    "{addr},{loop_},{},{},{},{},{},{},{}",
                    st.pos, v[0], v[1], v[2], v[3], s.dpcm_delta, s.dpcm_pos
                ),
            );
        }
    }
}

/// Empty wave space: `set(nullptr, 0)` reads the static zero buffer
/// (mamecompat.h:136-143, :213). All formats, both directions (harness run_empty).
#[test]
fn ext_wave_space_empty() {
    let w = Wave::new(&[]);
    for mode in 0..4 {
        for i in 0..2i32 {
            let rev = (i & 1) != 0;
            let mut st = zeroed();
            st.address = (mode << 30) | (0x3 << 27) | 0x1000; // scale 3
            st.loop_ = if rev { 0x8000_0400u32 as i32 } else { 0x400 };
            st.pos = 64 + i;
            st.dpcm_s0 = 0x11;
            st.dpcm_s1 = 0x22;
            st.dpcm_s2 = -0x33;
            st.dpcm_s3 = 0x44;
            st.dpcm_delta = -0x5678;
            st.dpcm_pos = 64;
            let mut sb = StreamingBlock::NEW;
            sb.set_state(st);
            let v = sb.read_vals(&w);
            let s = sb.state();
            eq_str(
                &format!("x.empty.{mode}.{i}"),
                &format!("{},{},{},{},{},{}", v[0], v[1], v[2], v[3], s.dpcm_pos, s.dpcm_delta),
            );
        }
    }
}

/// 4096 one-sample compressed calls per (mode, scale) so the hidden delta
/// remainder accumulates (harness run_8c_long; known-buggy expander kept
/// verbatim — this locks its actual bit behaviour, quirk included).
#[test]
fn ext_dpcm_long_runs_all_modes_scales() {
    let w = Wave::new(wave_bytes());
    for mode in 0..4i32 {
        for scale in 0..8i32 {
            let mut st = zeroed();
            st.address = 0xC000_0000 | ((mode as u32) << 25) | ((scale as u32) << 27) | 0x1234;
            st.dpcm_s0 = (0x1234 + mode) as i16;
            st.dpcm_s1 = (-0x2345 + scale) as i16;
            st.dpcm_s2 = (0x0F00 ^ mode as i32) as i16;
            st.dpcm_s3 = (-0x111 - scale) as i16;
            st.dpcm_delta = 0x31234 + (mode << 8) + scale;
            st.dpcm_pos = 0x1000;
            let mut sb = StreamingBlock::NEW;
            sb.set_state(st);
            let mut s = String::new();
            for _k in 0..4096 {
                let mut cur = sb.state();
                cur.pos = cur.dpcm_pos as i32 - 3;
                sb.set_state(cur);
                let v = sb.read_vals(&w);
                s.push_str(&format!("{},{},{},{};", v[0], v[1], v[2], v[3]));
            }
            eq_str(&format!("x.8cL.{mode}.{scale}.sha"), &sha1(s.as_bytes()));
            let f = sb.state();
            eq_str(
                &format!("x.8cL.{mode}.{scale}.st"),
                &format!(
                    "{},{},{},{},{},{}",
                    f.dpcm_delta, f.dpcm_s0, f.dpcm_s1, f.dpcm_s2, f.dpcm_s3, f.dpcm_pos
                ),
            );
        }
    }
}

/// 4096-step step() runs per scenario: forward loop, reverse loop, no-loop
/// done path (then the m_last hold path), loop-tune finetune clamp, and the
/// r12 / r8 / r8c dispatch legs inside step(). LFO and offset sweep the whole
/// pitch range (all pitch_base e-regions and m indices). Harness
/// run_step_long: same seeds, same lfo/off formulas.
#[test]
fn ext_step_long_runs() {
    let w = Wave::new(wave_bytes());
    // (tag, start_h, start_l, loop_h, loop_l, addr_h, addr_l, pitch)
    let seeds: [(char, u16, u16, u16, u16, u16, u16, u16); 7] = [
        ('F', 0x0001, 0x0000, 0x0000, 0x1000, 0x0000, 0x0000, 0x0800),
        ('R', 0x0000, 0x5000, 0x8010, 0x0000, 0x0000, 0x0000, 0x0700),
        ('D', 0x4001, 0x0000, 0x0000, 0x0800, 0x0000, 0x0000, 0x0800),
        ('T', 0x0001, 0x0000, 0x4010, 0x0000, 0x0000, 0x0000, 0x07FF),
        ('2', 0x0001, 0x0000, 0x0000, 0x1000, 0x4000, 0x2000, 0x0800),
        ('8', 0x0001, 0x0000, 0x0000, 0x1000, 0x8000, 0x3000, 0x0900),
        ('C', 0x0000, 0x0000, 0x0000, 0x0400, 0xC000, 0x3456, 0x0800),
    ];
    for (tag, sh, sl, lh, ll, ah, al, pitch) in seeds {
        let mut sb = StreamingBlock::NEW;
        sb.clear();
        sb.start_h_w(sh);
        sb.start_l_w(sl);
        sb.loop_h_w(lh);
        sb.loop_l_w(ll);
        sb.address_h_w(ah);
        sb.address_l_w(al);
        sb.pitch_w(pitch);
        sb.keyon();
        let mut s = String::new();
        let mut done_at: i32 = -1;
        for k in 0..4096i32 {
            let lfo = ((k as u32 * 7919) % 4096) as i32 - 2048;
            let off = (k * 13) as u16;
            let (r, done_now) = sb.step(&w, lfo, off);
            if done_now && done_at < 0 {
                done_at = k;
            }
            s.push_str(&format!("{:04x},{};", r as u16, done_now as u8));
        }
        eq_str(&format!("x.stp.{tag}.sha"), &sha1(s.as_bytes()));
        let f = sb.state();
        eq_str(
            &format!("x.stp.{tag}.st"),
            &format!(
                "{},{},{},{},{},{},{},{},{}",
                f.pos,
                f.pos_dec,
                f.dpcm_pos as i32,
                f.dpcm_delta,
                f.dpcm_s0,
                f.dpcm_s1,
                f.dpcm_s2,
                f.dpcm_s3,
                done_at
            ),
        );
    }
}
