//! Phase-B bit-exactness replay of `src/mame/sound/swp30.cpp` against
//! ground-truth vectors captured from `%TEMP%\fetchgt\gt.cpp` — a standalone
//! g++ harness embedding the C++ streaming_block methods byte-exact
//! (swp30.cpp:315-371 tables, :373-915 methods; wave cache = flat_space pow2
//! leg, mamecompat.h:160-164). Vectors: tests/data/vectors.txt.
//! No ROMs: the 4 MiB wave buffer is LCG-synthesized identically on both sides
//! (keys wave.*).

use std::collections::HashMap;
use std::sync::OnceLock;

use smu_swp30::fetch::{FetchState, StreamingBlock, Wave};

const DATA: &str = include_str!("data/vectors.txt");
const PITCH_TABLE: &str = include_str!("data/pitch_table.txt");

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

fn field(key: &str, i: usize) -> i64 {
    let v = get(key);
    v.split(',')
        .nth(i)
        .unwrap_or_else(|| panic!("key {key}: field {i} missing"))
        .parse()
        .unwrap_or_else(|e| panic!("key {key}: field {i} not i64: {e}"))
}

/// Compare rendered got-values against the CSV stored under `key`, reporting
/// the first mismatching element with full context.
fn eq_csv(key: &str, got: &[String]) {
    let want: Vec<&str> = get(key).split(',').collect();
    assert_eq!(want.len(), got.len(), "{key}: arity");
    for (i, (w, g)) in want.iter().zip(got).enumerate() {
        assert!(
            w == g,
            "{key}[{i}]: expected {w}, got {g}\n  full expected: {}\n  full got:     {:?}",
            get(key),
            got
        );
    }
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

fn wave_bytes() -> &'static Vec<u8> {
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

// --- compact SHA-1 (RFC 3174) over the ASCII hex rendering ------------------
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

/// digest of `%04x`-hex rendering of each element's raw 16 bits — matches the
/// harness `sha1_hex16` exactly.
fn hex16_sha1_i16(vals: &[i16]) -> String {
    let mut s = String::with_capacity(vals.len() * 4);
    for v in vals {
        s.push_str(&format!("{:04x}", *v as u16));
    }
    sha1(s.as_bytes())
}
fn hex16_sha1_u16(vals: &[u16]) -> String {
    let mut s = String::with_capacity(vals.len() * 4);
    for v in vals {
        s.push_str(&format!("{v:04x}"));
    }
    sha1(s.as_bytes())
}

fn spots(key: &str) -> Vec<i64> {
    // harness prints `key=` then `,%d,%d`... so the value may start with ','
    get(key)
        .trim_start_matches(',')
        .split(',')
        .map(|x| x.parse().unwrap())
        .collect()
}

#[test]
fn tables_match_cpp_ground_truth() {
    let de = StreamingBlock::dpcm_expand_tbl();
    let pb = StreamingBlock::pitch_base_tbl();
    let it = StreamingBlock::interp_tbl();
    let mv = StreamingBlock::max_value_tbl();

    assert_eq!(get("t.dpcm_expand.sha1"), hex16_sha1_i16(&de[..]));
    assert_eq!(get("t.pitch_base.sha1"), hex16_sha1_u16(&pb[..]));
    assert_eq!(get("t.interp0.sha1"), hex16_sha1_i16(&it[0][..]));
    assert_eq!(get("t.interp1.sha1"), hex16_sha1_i16(&it[1][..]));

    let spots_i16: [(&str, &[i16]); 3] = [
        ("t.dpcm_expand", &de[..]),
        ("t.interp0", &it[0][..]),
        ("t.interp1", &it[1][..]),
    ];
    for (tag, tbl) in spots_i16 {
        for pair in spots(&format!("{tag}.spot")).chunks(2) {
            assert_eq!(pair[1], tbl[pair[0] as usize] as i64, "{tag} spot idx {}", pair[0]);
        }
    }
    for pair in spots("t.pitch_base.spot").chunks(2) {
        assert_eq!(pair[1], pb[pair[0] as usize] as i64, "pitch_base spot idx {}", pair[0]);
    }
    for (i, w) in spots("t.max_value").iter().enumerate() {
        assert_eq!(*w, mv[i] as i64, "max_value[{i}]");
    }

    // pitch_base full hex vs ported table, and vs the committed integer-root
    // table from the prior phase (both must agree with the C++ pow() table).
    let hex: Vec<&str> = get("t.pitch_base.hex")
        .trim_start_matches(',')
        .split(',')
        .collect();
    assert_eq!(hex.len(), 0x400);
    let root: Vec<u32> = PITCH_TABLE
        .split(',')
        .filter_map(|t| t.trim().strip_prefix("0x"))
        .map(|h| u32::from_str_radix(h, 16).unwrap())
        .collect();
    assert_eq!(root.len(), 0x400, "pitch_table.txt entry count");
    for i in 0..0x400 {
        let p = u32::from_str_radix(hex[i], 16).unwrap();
        assert_eq!(p, pb[i] as u32, "pitch_base[{i}] vs harness hex");
        assert_eq!(p, root[i], "pitch_base[{i}] vs integer-root pitch_table.txt");
    }
}

#[test]
fn readers_16_12_8_match_cpp_ground_truth() {
    let w = Wave::new(wave_bytes());
    for tag in ["r16", "r12", "r8"] {
        for i in 0..6 {
            let key = format!("{tag}.{i}");
            let mut st = zeroed();
            st.address = field(&key, 0) as u32;
            st.pos = field(&key, 1) as i32;
            st.loop_ = field(&key, 2) as i32;
            let mut sb = StreamingBlock::NEW;
            sb.set_state(st);
            let v = sb.read_vals(&w);
            let got: Vec<String> = (0..4)
                .map(|j| v[j].to_string())
                .chain(std::iter::once(sb.state().pos.to_string()))
                .collect();
            let want: Vec<&str> = get(&key).split(',').skip(3).collect();
            assert_eq!(want.len(), got.len(), "{key}: arity");
            for (j, (wm, g)) in want.iter().zip(&got).enumerate() {
                assert!(wm == g, "{key} field {j}: expected {wm}, got {g}");
            }
        }
    }
}

#[test]
fn read_8c_dpcm_match_cpp_ground_truth() {
    let w = Wave::new(wave_bytes());
    for j in 0..12 {
        let skey = format!("r8c.{j}.seed");
        let mut st = zeroed();
        st.address = field(&skey, 0) as u32;
        st.dpcm_s0 = field(&skey, 1) as i16;
        st.dpcm_s1 = field(&skey, 2) as i16;
        st.dpcm_s2 = field(&skey, 3) as i16;
        st.dpcm_s3 = field(&skey, 4) as i16;
        st.dpcm_delta = field(&skey, 5) as i32;
        st.dpcm_pos = field(&skey, 6) as u32;
        let mut sb = StreamingBlock::NEW;
        sb.set_state(st);
        for k in 1..=256u32 {
            // one sample per call: the loop tail runs while spos != m_pos + 4
            let mut s = sb.state();
            s.pos = s.dpcm_pos as i32 - 3;
            sb.set_state(s);
            let v = sb.read_vals(&w);
            if matches!(k, 1 | 64 | 255 | 256) {
                let s = sb.state();
                eq_csv(
                    &format!("r8c.{j}.k{k}"),
                    &[
                        v[0].to_string(),
                        v[1].to_string(),
                        v[2].to_string(),
                        v[3].to_string(),
                        s.dpcm_delta.to_string(),
                        s.dpcm_pos.to_string(),
                    ],
                );
            }
        }
        // same 256 samples in a single call must land on the identical state
        let mut batch = st;
        batch.pos = st.dpcm_pos as i32 + 252;
        sb.set_state(batch);
        let v = sb.read_vals(&w);
        let s = sb.state();
        eq_csv(
            &format!("r8c.{j}.batch"),
            &[
                v[0].to_string(),
                v[1].to_string(),
                v[2].to_string(),
                v[3].to_string(),
                s.dpcm_delta.to_string(),
                s.dpcm_pos.to_string(),
            ],
        );
    }
    // reverse-flag early return (swp30.cpp:668-681): values held, state
    // untouched
    let mut st = zeroed();
    st.address = 0xE200_3400;
    st.loop_ = 0x8000_0400u32 as i32;
    st.dpcm_s0 = 0x1234;
    st.dpcm_s1 = -0x2345;
    st.dpcm_s2 = 0x0F00;
    st.dpcm_s3 = -0x111;
    st.dpcm_delta = 0x31234;
    st.dpcm_pos = 0x100;
    let mut sb = StreamingBlock::NEW;
    sb.set_state(st);
    let v = sb.read_vals(&w);
    let s = sb.state();
    eq_csv(
        "r8c.rev",
        &[
            v[0].to_string(),
            v[1].to_string(),
            v[2].to_string(),
            v[3].to_string(),
            s.dpcm_delta.to_string(),
            s.dpcm_pos.to_string(),
            s.address.to_string(),
        ],
    );
}

#[test]
fn step_match_cpp_ground_truth() {
    let w = Wave::new(wave_bytes());
    for tag in ['A', 'B', 'C'] {
        let skey = format!("st.{tag}.seed");
        let mut sb = StreamingBlock::NEW;
        sb.clear();
        sb.start_h_w(field(&skey, 0) as u16);
        sb.start_l_w(field(&skey, 1) as u16);
        sb.loop_h_w(field(&skey, 2) as u16);
        sb.loop_l_w(field(&skey, 3) as u16);
        sb.address_h_w(field(&skey, 4) as u16);
        sb.address_l_w(field(&skey, 5) as u16);
        sb.pitch_w(field(&skey, 6) as u16);
        sb.keyon();
        let pos_ovr = field(&skey, 7) as i32;
        if pos_ovr != 0x7fff_ffff {
            let posdec = field(&skey, 8) as i32;
            let mut s = sb.state();
            s.pos = pos_ovr;
            s.pos_dec = posdec;
            sb.set_state(s);
        }
        for k in 0..64 {
            let key = format!("st.{tag}.{k}");
            let lfo = field(&key, 0) as i32;
            let off = field(&key, 1) as u16;
            let (r, done_now) = sb.step(&w, lfo, off);
            let s = sb.state();
            eq_csv(
                &key,
                &[
                    lfo.to_string(),
                    off.to_string(),
                    (done_now as i32).to_string(),
                    r.to_string(),
                    s.pos.to_string(),
                    s.pos_dec.to_string(),
                    (s.first as i32).to_string(),
                    (s.finetune_active as i32).to_string(),
                    (s.done as i32).to_string(),
                ],
            );
        }
    }
}

#[test]
fn keyon_and_describe_match_cpp_ground_truth() {
    for i in 0..4 {
        let skey = format!("ko.{i}.seed");
        let mut sb = StreamingBlock::NEW;
        sb.clear();
        sb.start_h_w(field(&skey, 0) as u16);
        sb.start_l_w(field(&skey, 1) as u16);
        sb.loop_h_w(field(&skey, 2) as u16);
        sb.loop_l_w(field(&skey, 3) as u16);
        sb.address_h_w(field(&skey, 4) as u16);
        sb.address_l_w(field(&skey, 5) as u16);
        sb.pitch_w(field(&skey, 6) as u16);
        let s = sb.state();
        eq_csv(
            &format!("ko.{i}.pre"),
            &[
                s.pos.to_string(),
                s.dpcm_pos.to_string(),
                s.dpcm_delta.to_string(),
                s.loop_size.to_string(),
                (s.first as i32).to_string(),
                (s.finetune_active as i32).to_string(),
                (s.done as i32).to_string(),
                s.last.to_string(),
            ],
        );
        sb.keyon();
        let s = sb.state();
        eq_csv(
            &format!("ko.{i}.after"),
            &[
                s.pos.to_string(),
                s.dpcm_pos.to_string(),
                s.dpcm_delta.to_string(),
                s.loop_size.to_string(),
                (s.first as i32).to_string(),
                (s.finetune_active as i32).to_string(),
                (s.done as i32).to_string(),
            ],
        );
        let want = get(&format!("ko.{i}.desc"));
        let got = sb.describe();
        assert!(
            want == got,
            "ko.{i}.desc mismatch\n  expected: {want:?}\n  got:      {got:?}"
        );
    }
}
