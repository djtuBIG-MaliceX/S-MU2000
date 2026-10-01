//! Replay of the mixer-row PHASE B chain — `run_sample` :4179-4271,
//! `sample_step` :4304-4375, `adc_step` :4273-4277, vol/route :2786-2814 +
//! dispatch blocks :2049-2060/:2163-2174, internal :2836-2869, melo/set_meli
//! swp30.h:95-96 — against `tests/data/mixB_vectors.txt` captured from the
//! `%TEMP%\mixB` g++ -std=c++20 -O3 -mfpmath=sse -msse2 harness (32 windows
//! byte-extracted + fc /B double-slice proved; the REAL MEG engine compiled
//! in, awm2/taps stubbed to fresh-device behaviour). Inputs are dumped per
//! sample so every test is a pure replay. No ROMs.

use std::sync::OnceLock;

use crate::fetch::Wave;
use crate::mix::SERIAL_FULL_SCALE;
use crate::regs::Swp30;

const DATA: &str = include_str!("../tests/data/mixB_vectors.txt");

struct Vecs {
    lines: Vec<(&'static str, &'static str)>,
}

fn data() -> &'static Vecs {
    static V: OnceLock<Vecs> = OnceLock::new();
    V.get_or_init(|| {
        let raw: &'static str = Box::leak(DATA.to_string().into_boxed_str());
        let mut lines = Vec::new();
        for line in raw.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line.split_once('=').expect("vector line KEY=V");
            lines.push((k, v));
        }
        Vecs { lines }
    })
}

/// all (key, value) pairs in file order (keys repeat by design: s1.wr etc.)
fn pairs(prefix: &str) -> Vec<(&'static str, &'static str)> {
    data()
        .lines
        .iter()
        .filter(|(k, _)| k.starts_with(prefix))
        .map(|(k, v)| (*k, *v))
        .collect()
}

fn get(key: &str) -> &'static str {
    data()
        .lines
        .iter()
        .find(|(k, _)| *k == key)
        .unwrap_or_else(|| panic!("vector key missing: {key}"))
        .1
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

/// harness phex shape: " %08x" leading space per token
fn hexs(vals: impl Iterator<Item = i32>) -> String {
    vals.map(|v| format!(" {:08x}", v as u32)).collect()
}

fn sintab0() -> Vec<u16> {
    vec![0u16; 0x8000] // harness d.m_sintab zeros (gt.cpp)
}

#[test]
fn grid_writes_reads() {
    // s1.wr/s1.rd: every write through Swp30::write16 (dispatch under test),
    // every read back through Swp30::read16 — cross-gates the addr->Sel
    // mapping against the harness's verbatim :2049-2060/:2163-2174 blocks.
    let mut s = Swp30::new();
    for (k, v) in pairs("s1.") {
        if let Some(a) = k.strip_prefix("s1.wr") {
            let mut p = v.split(',');
            let addr = hx(p.next().unwrap());
            let data = hx(p.next().unwrap()) as u16;
            s.write16(addr, data);
        } else if let Some(_) = k.strip_prefix("s1.rd") {
            let mut p = v.split(',');
            let addr = hx(p.next().unwrap());
            let want = hx(p.next().unwrap());
            let got = s.read16(addr) as u32;
            assert!(got == want, "read16({addr:#x}) -> {got:#06x}, want {want:#06x}");
        }
    }
    // the whole grid family must now be REAL: zero deferred traffic
    assert_eq!(s.deferred_hits, 0, "vol/route must not defer any more");
    // every bank-0x40 mix 0x40..0x5f got routes written -> dirty pending
    assert_ne!(s.mixer.mix_dirty[1], 0, "mixer_mark fired for bank 0x40");
}

#[test]
fn chain_run_sample() {
    // full run_sample replay: fresh Swp30 (zero program -> the harness's),
    // per-sample meg.m + meli injection exactly as the harness dumped them.
    let mut s = Swp30::new();
    for (k, v) in pairs("s1.wr") {
        let mut p = v.split(',');
        s.write16(hx(p.next().unwrap()), hx(p.next().unwrap()) as u16);
    }
    let wave = Wave::new(&[]); // no ROM: fresh voices never read the wave
    let st = sintab0();
    for k in 0..100usize {
        let m20: Vec<i32> = toks(get(&format!("s1.m20{k}"))).into_iter().map(sx).collect();
        let m30: Vec<i32> = toks(get(&format!("s1.m30{k}"))).into_iter().map(sx).collect();
        let li: Vec<i32> = toks(get(&format!("s1.li{k}"))).into_iter().map(sx).collect();
        assert!(m20.len() == 0x10 && m30.len() == 4 && li.len() == 0x10);
        for (i, v) in m20.iter().enumerate() {
            s.meg.m[0x20 + i] = *v; // harness injected via meg.m_m
        }
        for (i, v) in m30.iter().enumerate() {
            s.meg.m[0x30 + i] = *v;
        }
        for (i, v) in li.iter().enumerate() {
            s.set_meli(i, *v); // harness d.set_meli
        }
        if let Some(jw) = data().lines.iter().find(|(kk, _)| *kk == format!("s1.jw{k}")) {
            assert_eq!(
                s.meg_jit_wait as u32,
                hx(jw.1),
                "meg_jit_wait protocol mismatch at sample {k} (:4181-4194)"
            );
        }
        let (l, r) = s.run_sample(&st, &wave);
        let want = get(&format!("s1.o{k}"));
        let got: String = hexs(
            (0..16)
                .map(|i| s.mixer.melo[i])
                .chain(std::iter::once(s.mixer.rec_bus))
                .chain((0..4).map(|i| s.mixer.adc[i]))
                .chain(std::iter::once(l))
                .chain(std::iter::once(r))
                .chain(std::iter::once(s.meg.sample_counter as i32)),
        )
        .trim()
            .to_string();
        let wantt = want.trim().to_string();
        assert!(wantt == got, "sample {k}:\n want:{wantt}\n  got:{got}");
    }
    assert_eq!(s.deferred_hits, 0);
}

#[test]
fn run_sample_end_protocol() {
    // s1.end: rebuild flags consumed, jit-wait counter position (the :4191
    // >64 reset arm sat mid-count at sample 100: wait == 99+1 re-arm legs)
    let mut s = Swp30::new();
    for (k, v) in pairs("s1.wr") {
        let mut p = v.split(',');
        s.write16(hx(p.next().unwrap()), hx(p.next().unwrap()) as u16);
    }
    let wave = Wave::new(&[]);
    let st = sintab0();
    for k in 0..100usize {
        for (i, v) in toks(get(&format!("s1.m20{k}"))).into_iter().enumerate() {
            s.meg.m[0x20 + i] = sx(v);
        }
        for (i, v) in toks(get(&format!("s1.m30{k}"))).into_iter().enumerate() {
            s.meg.m[0x30 + i] = sx(v);
        }
        for (i, v) in toks(get(&format!("s1.li{k}"))).into_iter().enumerate() {
            s.set_meli(i, sx(v));
        }
        let _ = s.run_sample(&st, &wave);
    }
    let mut p = get("s1.end").split(',');
    let changed = hx(p.next().unwrap()) != 0;
    let stale = hx(p.next().unwrap()) != 0;
    let jw = hx(p.next().unwrap());
    let nact = hx(p.next().unwrap());
    assert_eq!(s.meg_program_changed, changed, ":4184");
    assert_eq!(s.meg_ops_stale, stale, ":4185");
    assert_eq!(s.meg_jit_wait, jw, ":4190/4192 wait counter");
    assert_eq!(s.mixer.mix_nactive as u32, nact);
}

#[test]
fn internal_regs() {
    // s2.pk pokes voice state directly (as the harness stand-in did);
    // s2.r replays internal_adr_w + internal_r THROUGH write16/read16
    // (0x04e/0x04f — dispatch arms + envelope/peg legs + log->0 leg).
    let mut s = Swp30::new();
    for (k, v) in pairs("s2.") {
        if let Some(_) = k.strip_prefix("s2.pk") {
            let mut p = v.split(',');
            let chan = hx(p.next().unwrap()) as usize;
            let mode = hx(p.next().unwrap()) as u8;
            let level = sx(p.next().unwrap());
            let peg = sx(p.next().unwrap());
            let reach = hx(p.next().unwrap()) as u8;
            let vo = &mut s.voices.voices[chan];
            vo.envelope.m_envelope_mode = mode;
            vo.envelope.m_envelope_level = level;
            vo.peg_cur = peg;
            vo.peg_reached = reach;
        } else if let Some(_) = k.strip_prefix("s2.r") {
            let mut p = v.split(',');
            let adr = hx(p.next().unwrap());
            let want = hx(p.next().unwrap());
            s.write16(0x04e, adr as u16);
            let got = s.read16(0x04f) as u32;
            assert!(got == want, "internal_r(adr={adr:#x}) -> {got:#06x}, want {want:#06x}");
        } else if let Some(_) = k.strip_prefix("s2.ad") {
            let want = hx(v);
            assert_eq!(s.read16(0x04e) as u32, want, "internal_adr_r");
        }
    }
    assert_eq!(s.deferred_hits, 0, "internal arm must not defer");
}

#[test]
fn melo_clamp_and_set_meli() {
    // swp30.h:95-96 via the device accessors
    let mut s = Swp30::new();
    let ins: Vec<i32> = toks(get("s3.in")).into_iter().map(sx).collect();
    let outs: Vec<i32> = toks(get("s3.out")).into_iter().map(sx).collect();
    assert!(ins.len() == 8 && outs.len() == 8);
    for i in 0..8 {
        s.mixer.melo[i] = ins[i];
        let got = s.melo(i);
        assert!(got == outs[i], "melo({i}) raw {:#x} -> {:#x}, want {:#x}", ins[i], got, outs[i]);
    }
    assert_eq!(outs[3], SERIAL_FULL_SCALE);
    assert_eq!(outs[4], -SERIAL_FULL_SCALE);
    let sme: Vec<i32> = toks(get("s3.sme")).into_iter().map(sx).collect();
    let out2: Vec<i32> = toks(get("s3.out2")).into_iter().map(sx).collect();
    for i in 0..4 {
        s.set_meli(i, sme[i]);
        assert_eq!(s.mixer.meli[i], out2[i], "set_meli({i})");
    }
}
