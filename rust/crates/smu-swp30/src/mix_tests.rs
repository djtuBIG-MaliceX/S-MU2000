//! Replay of `src/mame/sound/swp30.cpp:2978-3106` (mixer_att / mixer_rebuild
//! / mixer_step) against `tests/data/mix_vectors.txt` (hex lines, captured
//! from the `%TEMP%\mixA` g++ -std=c++20 -O3 -mfpmath=sse -msse2 harness with
//! byte-extracted bodies; synthetic LCG inputs only, inputs are dumped so
//! every test is a pure replay).

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::meg::MegState;
use crate::mix::{Mixer, MixerSlot};

const DATA: &str = include_str!("../tests/data/mix_vectors.txt");

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

fn eq_str(key: &str, got: String) {
    let want = get(key);
    assert!(want == got, "{key} mismatch\n  expected: {want}\n  got:      {got}");
}

fn hx(t: &str) -> u32 {
    u32::from_str_radix(t, 16).unwrap()
}
fn sx(t: &str) -> i32 {
    hx(t) as i32
}
fn toks(v: &str) -> Vec<&str> {
    v.split(' ').filter(|s| !s.is_empty()).collect()
}

/// hex dump in harness shape: phex prints " %08x" (leading space per token)
fn hexs(vals: impl Iterator<Item = i32>) -> String {
    vals.map(|v| format!(" {:08x}", v as u32)).collect()
}

/// harness dump_taps shape: "{ntaps_dec}[ {dst}{frac}{shift}...]"
fn tap_dump(m: &Mixer, mix: usize) -> String {
    let n = m.mix_ntaps[mix] as usize;
    let mut s = format!("{n}");
    for t in &m.mix_taps[mix][..n] {
        s += &format!(" {:02x}{:02x}{:02x}", t.dst, t.frac, t.shift);
    }
    s
}

/// harness dump_active shape (`.nactive` key + `.act` key)
fn active_keys(m: &Mixer, tag: &str) {
    eq_str(&format!("{tag}.nactive"), m.mix_nactive.to_string());
    eq_str(
        &format!("{tag}.act"),
        m.mix_active[..m.mix_nactive as usize]
            .iter()
            .map(|a| format!(" {a:02x}"))
            .collect(),
    );
}

fn set_slot(m: &mut Mixer, mix: usize, vol: [u16; 3], route: [u16; 3]) {
    m.mixer[mix] = MixerSlot { vol, route };
}

fn u16s(s: &str) -> [u16; 3] {
    assert!(s.len() == 12);
    [
        u16::from_str_radix(&s[0..4], 16).unwrap(),
        u16::from_str_radix(&s[4..8], 16).unwrap(),
        u16::from_str_radix(&s[8..12], 16).unwrap(),
    ]
}

#[test]
fn att_cross() {
    let ss: Vec<i32> = toks(get("att.s")).into_iter().map(sx).collect();
    let as_: Vec<i32> = toks(get("att.a")).into_iter().map(sx).collect();
    let out: Vec<u32> = toks(get("att.out")).into_iter().map(hx).collect();
    assert!(ss.len() == 9 && as_.len() == 13 && out.len() == 9 * 13);
    let mut k = 0;
    for s in &ss {
        for a in &as_ {
            let got = Mixer::mixer_att(*s, *a);
            assert!(got as u32 == out[k], "mixer_att({s:#x},{a:#x}) -> {got:#x}, want {:x}", out[k]);
            k += 1;
        }
    }
}

#[test]
fn rebuild_raw_all() {
    let mut m = Mixer::new();
    set_slot(&mut m, 7, [0, 0, 0], [0xffff, 0, 0]);
    m.mix_dirty = [0, 0];
    m.mixer_mark(7);
    m.mixer_rebuild();
    active_keys(&m, "rbl1");
    eq_str("rbl1.m.07", tap_dump(&m, 7));
    eq_str("rbl1.m.08", tap_dump(&m, 8));
}

#[test]
fn rebuild_lcg_routes() {
    let mut m = Mixer::new();
    m.mix_dirty = [0, 0];
    let mut mixes: Vec<usize> = Vec::new();
    for (k, v) in vectors() {
        if let Some(mx) = k.strip_prefix("rbl2.cfg") {
            let mut parts = v.split(',');
            let vol = u16s(parts.next().unwrap());
            let route = u16s(parts.next().unwrap());
            let mx = usize::from_str_radix(mx, 16).unwrap();
            set_slot(&mut m, mx, vol, route);
            m.mixer_mark(mx as i32);
            mixes.push(mx);
        }
    }
    mixes.sort();
    assert!(mixes.len() == 8);
    m.mixer_rebuild();
    active_keys(&m, "rbl2");
    for mx in mixes {
        eq_str(&format!("rbl2.m.{mx:02x}"), tap_dump(&m, mx));
    }
}

#[test]
fn rebuild_dirty_semantics() {
    let mut m = Mixer::new();
    set_slot(&mut m, 3, [0, 0, 0], [0xffff, 0, 0]);
    set_slot(&mut m, 4, [0x8040, 0x7ebf, 0x8080], [0, 0xffff, 0]);
    m.mix_dirty = [0, 0];
    m.mixer_mark(3);
    m.mixer_rebuild();
    active_keys(&m, "rbl3a");
    eq_str("rbl3a.m.03", tap_dump(&m, 3));
    eq_str("rbl3a.m.04", tap_dump(&m, 4));
    set_slot(&mut m, 4, [0, 0, 0], [0, 0, 0xffff]);
    m.mix_dirty = [0, 0];
    m.mixer_mark(4);
    m.mixer_rebuild();
    active_keys(&m, "rbl3b");
    eq_str("rbl3b.m.03", tap_dump(&m, 3));
    eq_str("rbl3b.m.04", tap_dump(&m, 4));
    set_slot(&mut m, 3, [0xffff, 0xffff, 0xffff], [0, 0, 0]); // changed, NOT marked
    m.mixer_rebuild();
    active_keys(&m, "rbl3c");
    eq_str("rbl3c.m.03", tap_dump(&m, 3));
    eq_str("rbl3c.m.04", tap_dump(&m, 4));
}

#[test]
fn rebuild_all_modes() {
    let mut m = Mixer::new();
    let (mut r0, mut r1, mut r2) = (0u16, 0u16, 0u16);
    for out in 0..16 {
        let mode = out & 7;
        if mode & 1 != 0 {
            r2 |= 1 << out;
        }
        if mode & 2 != 0 {
            r1 |= 1 << out;
        }
        if mode & 4 != 0 {
            r0 |= 1 << out;
        }
    }
    set_slot(&mut m, 0x2a, [0x8040, 0x7ebf, 0x8080], [r0, r1, r2]);
    m.mix_dirty = [0, 0];
    m.mixer_mark(0x2a);
    m.mixer_rebuild();
    active_keys(&m, "rbl4");
    eq_str("rbl4.m.2a", tap_dump(&m, 0x2a));
}

/// config shared by stp1 (all three input families)
fn stp1_config(m: &mut Mixer) {
    m.mix_dirty = [0, 0];
    set_slot(m, 0x03, [0, 0, 0], [0xffff, 0, 0]);                    // raw all
    set_slot(m, 0x0f, [0x0f40, 0x1f, 0x0f70], [0xffff, 0, 0]);       // att s0/s1
    set_slot(m, 0x20, [0x2000, 0x0340, 0x007f], [0, 0xffff, 0]);     // att s2/s3
    set_slot(m, 0x3f, [0, 0, 0], [0, 0, 0x00f0]);                    // raw 4-7
    for mx in [3usize, 0xf, 0x20, 0x3f] {
        m.mixer_mark(mx as i32);
    }
    set_slot(m, 0x41, [0x0180, 0x00c5, 0], [0xffff, 0xffff, 0]);     // MEG in
    m.mixer_mark(0x41);
    set_slot(m, 0x4e, [0, 0, 0], [0, 0, 0x0003]);
    m.mixer_mark(0x4e);
    set_slot(m, 0x50, [0, 0, 0], [0xf0f0, 0, 0]);                    // MELI raw
    m.mixer_mark(0x50);
    set_slot(m, 0x5c, [0x0021, 0x0f00, 0], [0, 0, 0xffff]);          // MELI att
    m.mixer_mark(0x5c);
    m.mixer_rebuild();
    active_keys(m, "stp1");
}

#[test]
fn step_chain_meli_melo() {
    let mut m = Mixer::new();
    let mut meg = MegState::new();
    stp1_config(&mut m);
    for it in 0..16 {
        let t = toks(get(&format!("stp1.in{it}")));
        assert!(t.len() == 0x40 + 0x10 + 0x10);
        let mut spc = [0i32; 0x40];
        for i in 0..0x40 {
            spc[i] = sx(t[i]);
        }
        for i in 0..0x10 {
            m.meli[i] = sx(t[0x40 + i]);
        }
        for i in 0..0x10 {
            meg.m[0x20 + i] = sx(t[0x50 + i]);
        }
        m.mixer_step(&spc, &mut meg);
        eq_str(
            &format!("stp1.melo{it}"),
            hexs(m.melo.into_iter().chain([m.rec_bus])),
        );
        eq_str(
            &format!("stp1.meg{it}"),
            hexs(meg.m[0x20..0x30].iter().copied()),
        );
    }
}

#[test]
fn step_native_sends() {
    let mut m = Mixer::new();
    let mut meg = MegState::new();
    m.native = true; // m_native != nullptr
    m.mix_dirty = [0, 0];
    set_slot(&mut m, 0x01, [0, 0, 0], [0x01f0, 0, 0]);
    m.mixer_mark(0x01);
    set_slot(&mut m, 0x31, [0x0033, 0x0f44, 0], [0, 0, 0x00ff]);
    m.mixer_mark(0x31);
    m.mixer_rebuild();
    for it in 0..8 {
        let t = toks(get(&format!("stp2.in{it}")));
        let mut spc = [0i32; 0x40];
        for i in 0..0x40 {
            spc[i] = sx(t[i]);
        }
        m.mixer_step(&spc, &mut meg);
        eq_str(
            &format!("stp2.nsend{it}"),
            hexs(m.nsend.iter().flat_map(|p| p.iter().copied())),
        );
        eq_str(
            &format!("stp2.meg{it}"),
            hexs(meg.m[0x20..0x30].iter().copied()),
        );
        eq_str(&format!("stp2.melo{it}"), hexs(m.melo.into_iter()));
    }
}

#[test]
fn step_native_full_dry() {
    let mut m = Mixer::new();
    let mut meg = MegState::new();
    m.native = true;
    m.native_full = true;
    m.mix_dirty = [0, 0];
    set_slot(&mut m, 0x02, [0, 0, 0], [0x03ff, 0, 0]);
    m.mixer_mark(0x02);
    m.mixer_rebuild();
    for it in 0..8 {
        let t = toks(get(&format!("stp3.in{it}")));
        let mut spc = [0i32; 0x40];
        for i in 0..0x40 {
            spc[i] = sx(t[i]);
        }
        m.mixer_step(&spc, &mut meg);
        eq_str(&format!("stp3.ndry{it}"), hexs(m.ndry.into_iter()));
        eq_str(
            &format!("stp3.nsend{it}"),
            hexs(m.nsend.iter().flat_map(|p| p.iter().copied())),
        );
        eq_str(
            &format!("stp3.meg{it}"),
            hexs(meg.m[0x20..0x30].iter().copied()),
        );
    }
}

#[test]
fn step_auto_rebuild() {
    let mut m = Mixer::new(); // mix_dirty still {~0,~0}
    let mut meg = MegState::new();
    set_slot(&mut m, 0x11, [0x0011, 0x0ef0, 0x0707], [0xffff, 0xffff, 0xffff]);
    let t = toks(get("stp4.in"));
    assert!(t.len() == 0x40 + 0x10);
    let mut spc = [0i32; 0x40];
    for i in 0..0x40 {
        spc[i] = sx(t[i]);
    }
    for i in 0..0x10 {
        m.meli[i] = sx(t[0x40 + i]);
    }
    m.mixer_step(&spc, &mut meg); // rebuild happens inside :3052-3053
    active_keys(&m, "stp4");
    eq_str("stp4.m.11", tap_dump(&m, 0x11));
    eq_str("stp4.melo", hexs(m.melo.into_iter().chain([m.rec_bus])));
}

#[test]
fn melo_clamp_and_set_meli() {
    let mut m = Mixer::new();
    let in_: Vec<i32> = toks(get("clmp.in")).into_iter().map(sx).collect();
    let out: Vec<u32> = toks(get("clmp.out")).into_iter().map(hx).collect();
    assert!(in_.len() == 8 && out.len() == 8);
    for i in 0..8 {
        m.melo[i] = in_[i];
        let got = m.melo_clamped(i);
        assert!(got as u32 == out[i], "melo_clamped({i}) with {} -> {got}, want {:x}", in_[i], out[i]);
    }
    let ins: Vec<i32> = toks(get("sme.in")).into_iter().map(sx).collect();
    let outs: Vec<u32> = toks(get("sme.out")).into_iter().map(hx).collect();
    for i in 0..4 {
        m.set_meli(i, ins[i]);
    }
    for i in 0..4 {
        assert!(m.meli[i] as u32 == outs[i], "set_meli({i}) readback mismatch");
    }
}
