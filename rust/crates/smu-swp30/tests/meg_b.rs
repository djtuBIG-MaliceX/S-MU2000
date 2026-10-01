//! MEG phase-B bit-exactness replay of `src/mame/sound/swp30.cpp` against
//! ground-truth vectors captured from `%TEMP%\megB2\gt.cpp` — a g++
//! (-std=c++20 -O3 -mfpmath=sse -msse2) harness embedding the C++ bodies
//! BYTE-EXACT and fc-proved vs disk: reset :1924-1956, prg/map :2262-2337,
//! revram :2428-2527, lfo table :3332-3342, const/offset/lfo + lfo_step
//! :3346-3435, addr helpers :3437-3529, decode_program :3535-3568,
//! meg_pack24/meg_cond :3570-3607, step :3609-3905, flush_writes
//! :3906-3927, build_ops :3928-3978, run_program :3982-4172 (+sext, rand).
//!
//! ALU/STP scenarios: one 0x180 bank per mmode (A=0..D=3, all four
//! :3702-3714 arms); every (asel,rop) x (shift,clamp) triple plus a
//! m1t/m1x/m2m/drfr/latch/tw/tfp/nn field pass; STP* drives the same
//! banks via meg_step (skip_to/ring carry across the flush).
//!
//! Per-sample driver mirrors `run_sample` (:4196 sample_step ->
//! sample_step:4306 flush_writes, then :4204 step×384 / :4214 run_program).
//! STP*/STPF scenarios drive the step() path (device-side skip_to), the rest
//! drive run_program. Vectors: tests/data/meg_b_*.txt (hex only, SHA1-pinned
//! in the ledger row). No ROMs: sintab/reverb-RAM/banks are xorshift LCG
//! fills pinned through the C++ bodies.
//!
//! CHAOS reprograms every 128 samples (seed 0xABCD12349876EF01, harness
//! gt.cpp :375). R,off/R,prog lines pin every draw; R,konst prints
//! nonzero-only, so the konst draws are replayed here in stream order and
//! cross-checked against every emitted line. The harness re-copies ONLY
//! m_program at regen (gt.cpp :135) — meg offset/konst stay frozen for
//! the whole run; the replay mirrors that exactly.

use std::collections::BTreeMap;

use smu_swp30::meg::{build_ops, meg_step, run_program, MegSwp, Op};
use smu_swp30::regs::Swp30;

const ALU: &str = include_str!("data/meg_b_alu.txt");
const FLOW: &str = include_str!("data/meg_b_flow.txt");
const MEM: &str = include_str!("data/meg_b_mem.txt");
const LFO: &str = include_str!("data/meg_b_lfo.txt");
const CHAOS: &str = include_str!("data/meg_b_chaos.txt");

/// harness xs() (gt.cpp:25) — same primitive as the phase-A Lcg, any seed.
struct Xs(u64);
impl Xs {
    fn nxt(&mut self) -> u64 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        self.0 = s;
        s
    }
}

/// harness fnv() (gt.cpp:28): FNV-1a over every u16 as two LE bytes.
fn fnv(ram: &[u16]) -> u64 {
    #![allow(clippy::needless_range_loop)]
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &w in ram {
        for b in 0..2u32 {
            h ^= ((w >> (8 * b)) & 0xff) as u64;
            h = h.wrapping_mul(0x1_0000_0001_b3);
        }
    }
    h
}

fn hx(s: &str) -> u64 {
    u64::from_str_radix(s, 16).unwrap()
}
fn bank(s: &str, n: usize) -> Vec<u32> {
    assert_eq!(s.len(), n * 8, "bank width {n}");
    (0..n).map(|i| hx(&s[i * 8..i * 8 + 8]) as u32).collect()
}

/// sintab: harness main() stream 0x243F6A8885A308D3, >>17 (meg.rs test Lcg).
fn sintab() -> Vec<u16> {
    let mut x = Xs(0x243F_6A88_85A3_08D3);
    (0..0x8000).map(|_| (x.nxt() >> 17) as u16).collect()
}

struct Ctl {
    via_step: bool,
    regen: Option<(u64, usize)>, // (seed, cadence)
}

const RUN: Ctl = Ctl { via_step: false, regen: None };
const STEP: Ctl = Ctl { via_step: true, regen: None };

/// build the device-seam bundle over disjoint Swp30 fields. A macro (not a
/// fn) so the field-path borrows stay disjoint under NLL.
macro_rules! mkswp {
    ($swp:expr, $en:expr, $st:expr) => {
        MegSwp {
            flag_n: &mut $swp.meg_flag_n,
            flag_z: &mut $swp.meg_flag_z,
            ix2_value: &mut $swp.meg_ix2_value,
            ix2_act: &mut $swp.meg_ix2_act,
            ram_index2: &mut $swp.meg_ram_index2,
            skip_to: &mut $swp.meg_skip_to,
            revram_enable: $en,
            reverb_ram: &mut $swp.reverb_ram,
            seed: &mut $swp.rand_seed,
            sintab: $st,
        }
    };
}

fn replay(text: &str, scn: &str, ctl: &Ctl, st: &[u16]) {
    let mut swp = Swp30::new();
    let lines: Vec<Vec<&str>> = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| -> Vec<&str> { l.split(',').collect() })
        .filter(|f| f[1] == scn)
        .collect();

    let mut prog = [0u64; 0x180];
    let mut map = [0u16; 8];
    let mut off = [0u16; 0x80];
    let mut konst = [0i16; 0x180];
    let mut lfo = [0u16; 0x18];
    let mut cnt = [0u32; 0x18];
    let mut m = [0i32; 0x40];
    let mut r = [0i32; 0x80];
    let mut t = [0i16; 8];
    let (mut seed, mut ramseed): (u32, u64) = (0, 0);
    let (mut p0, mut ri, mut ri2, mut en): (i64, i32, i32, u16) = (0, 0, 0, 0);
    let (mut sstep, mut lfostep): (u32, bool) = (0, false);
    let mut lw: Option<(u32, usize, u16)> = None;
    let mut lc: Option<u32> = None;
    let mut samples: Vec<Vec<&str>> = Vec::new();
    let mut regens: BTreeMap<u32, Vec<Vec<&str>>> = BTreeMap::new();
    let mut fin: Option<Vec<&str>> = None;

    for f in lines {
        match f[0] {
            "P" => prog[hx(f[2]) as usize] = hx(f[3]),
            "X" => match f[2] {
                "map" => map[f[3].parse::<usize>().unwrap()] = hx(f[4]) as u16,
                "off" => off[hx(f[3]) as usize] = hx(f[4]) as u16,
                "konst" => konst[hx(f[3]) as usize] = hx(f[4]) as u16 as i16,
                "lfo" => lfo[hx(f[3]) as usize] = hx(f[4]) as u16,
                "lc" => cnt[hx(f[3]) as usize] = hx(f[4]) as u32,
                "en" => en = hx(f[3]) as u16,
                "seed" => seed = hx(f[3]) as u32,
                "ramseed" => ramseed = hx(f[3]),
                "p" => p0 = hx(f[3]) as i64,
                "ri" => ri = hx(f[3]) as u32 as i32,
                "ri2" => ri2 = hx(f[3]) as u32 as i32,
                "st" => sstep = hx(f[3]) as u32,
                "lfs" => lfostep = f[3].parse::<u32>().unwrap() != 0,
                o => panic!("unknown X kind {o}"),
            },
            "B" => {
                if f[2] == "m" {
                    for (i, v) in bank(f[3], 0x40).into_iter().enumerate() {
                        m[i] = v as i32;
                    }
                } else if f[2] == "r" {
                    for (i, v) in bank(f[3], 0x80).into_iter().enumerate() {
                        r[i] = v as i32;
                    }
                } else {
                    // t<i>, %04x (index decimal)
                    let i: usize = f[2][1..].parse().unwrap();
                    t[i] = hx(f[3]) as u16 as i16;
                }
            }
            "W" => lw = Some((f[2].parse().unwrap(), f[3].parse().unwrap(), hx(f[4]) as u16)),
            "M" => lc = Some(f[2].parse().unwrap()),
            "R" => {
                let k: u32 = f[2].parse().unwrap();
                regens.entry(k).or_default().push(f.clone());
            }
            "S" | "C" => {
                let k: usize = f[2].parse().unwrap();
                if samples.len() <= k {
                    samples.resize(k + 1, Vec::new());
                }
                samples[k] = f;
            }
            "F" => fin = Some(f),
            o => panic!("unknown line kind {o}"),
        }
    }
    assert!(!samples.is_empty() && fin.is_some(), "scenario {scn} empty");

    // --- device init (harness run() :78-114 order) ---
    let mut xs = Xs(ramseed);
    for w in swp.reverb_ram.iter_mut() {
        *w = (xs.nxt() & 0xffff) as u16;
    }
    swp.rand_seed = seed;
    swp.revram_enable = en;
    swp.meg.program = prog;
    swp.meg.map = map;
    swp.meg.offset = off;
    swp.meg.konst = konst;
    swp.meg.lfo = lfo;
    swp.meg.lfo_counter = cnt;
    swp.meg.m = m;
    swp.meg.r = r;
    swp.meg.t = t;
    swp.meg.p = p0;
    swp.meg.ram_index = ri;
    swp.meg_ram_index2 = ri2;

    let mut ops = Box::new([Op::ZERO; 0x180]);
    swp.meg.decode_program();
    swp.meg.lfo_commit_w();
    build_ops(&swp.meg, &mut ops);

    let mut regen_xs = ctl.regen.map(|(s, _)| Xs(s));
    for (k, s) in samples.iter().enumerate() {
        if s.is_empty() {
            panic!("missing sample {k} of {scn}");
        }
        let ku = k as u32;
        // sample_step() :4306 flush FIRST (disk-faithful; multi-write tails
        // reorder otherwise — proven with per-write traces, session megB2)
        {
            let mut sw = mkswp!(swp, en, st);
            swp.meg.flush_writes(&mut sw);
        }
        if k > 0 && sstep != 0 {
            swp.meg.sample_counter += sstep;
        }
        if let Some((wk, wi, wv)) = lw {
            if wk == ku {
                swp.meg.lfo_w(wi, wv);
            }
        }
        if lc == Some(ku) {
            swp.meg.lfo_commit_w();
        }
        if let (Some((_rs, cad)), Some(rlines)) = (ctl.regen, regens.get(&ku)) {
            assert_eq!(k % cad, 0, "unscheduled regen at k={k} {scn}");
            let rx = regen_xs.as_mut().expect("regen seed");
            let ov: Vec<u16> = (0..0x80).map(|_| rx.nxt() as u16).collect();
            let kv: Vec<i16> = (0..0x180).map(|_| rx.nxt() as i16).collect();
            let pv: Vec<u64> = (0..0x180).map(|_| rx.nxt() & !(1u64 << 63)).collect();
            let (mut oi, mut ki, mut pi) = (0usize, 0usize, 0usize);
            for f in rlines {
                let idx = hx(f[4]) as usize;
                let v = hx(f[5]);
                match f[3] {
                    "off" => {
                        assert_eq!((oi, idx), (idx, idx), "R off order {scn} k={k}");
                        assert_eq!(v, ov[idx] as u64, "R off {scn} k={k} i={idx:x}");
                        oi += 1;
                    }
                    "konst" => {
                        // skip zero-draws (harness printed nonzero only)
                        while kv[ki] == 0 {
                            ki += 1;
                        }
                        assert_eq!((ki, idx), (idx, idx), "R konst order {scn} k={k}");
                        assert_eq!(v, kv[idx] as u16 as u64, "R konst {scn} k={k} i={idx:x}");
                        ki += 1;
                    }
                    "prog" => {
                        assert_eq!((pi, idx), (idx, idx), "R prog order {scn} k={k}");
                        assert_eq!(v, pv[idx], "R prog {scn} k={k} i={idx:x}");
                        pi += 1;
                    }
                    o => panic!("unknown R kind {o}"),
                }
            }
            assert_eq!(oi, 0x80);
            assert_eq!(ki, kv.iter().filter(|v| **v != 0).count(), "R konst stream {scn} k={k}");
            assert_eq!(pi, 0x180);
            // harness gt.cpp re-copies ONLY m_program at regen (:135);
            // meg.m_offset/m_const stay at their initial prefill for the
            // whole CH run (R,off/R,konst lines pin the LCG stream only)
            swp.meg.program.copy_from_slice(&pv);
            swp.meg.decode_program();
            build_ops(&swp.meg, &mut ops);
        }
        if ctl.via_step {
            for _ in 0..384 {
                // run_sample :4204-4205 (--dump-meg path, per-instruction)
                let mut sw = mkswp!(swp, en, st);
                meg_step(&mut swp.meg, &mut sw, &mut None, 0, 0x180, 0, 0);
            }
        } else {
            let mut sw = mkswp!(swp, en, st);
            run_program(&mut swp.meg, &mut sw, &ops);
        }
        if lfostep {
            swp.meg.lfo_step();
        }
        if s[0] == "S" {
            assert_eq!(swp.meg.p as u64, hx(s[3]), "S p {scn} k={k}");
            let fz = ((swp.meg_flag_n as u64) << 1) | swp.meg_flag_z as u64;
            assert_eq!(fz, hx(s[4]), "S f/z {scn} k={k}");
            assert_eq!(swp.meg.delay_3, hx(s[5]) as u32, "S d3 {scn} k={k}");
            assert_eq!(swp.meg.delay_2, hx(s[6]) as u32, "S d2 {scn} k={k}");
            assert_eq!(swp.meg.ram_read, hx(s[7]) as u32, "S rr {scn} k={k}");
            assert_eq!(swp.meg.ram_write, hx(s[8]) as u32, "S rw {scn} k={k}");
            assert_eq!(swp.meg.ram_index as u32, hx(s[9]) as u32, "S ri {scn} k={k}");
            assert_eq!(swp.meg_ram_index2 as u32, hx(s[10]) as u32, "S ri2 {scn} k={k}");
            assert_eq!(swp.meg.sample_counter, hx(s[11]) as u32, "S st {scn} k={k}");
            for i in 0..8 {
                assert_eq!(
                    swp.meg.t[i] as u16,
                    hx(&s[12][i * 4..i * 4 + 4]) as u16,
                    "S t{i} {scn} k={k}"
                );
            }
            let tv: Vec<&str> = s[13].split('_').collect();
            assert_eq!(swp.meg.t_value[0] as u16, hx(tv[0]) as u16, "S tv0 {scn} k={k}");
            assert_eq!(swp.meg.t_value[1] as u16, hx(tv[1]) as u16, "S tv1 {scn} k={k}");
            for (i, v) in bank(s[14], 0x40).into_iter().enumerate() {
                assert_eq!(swp.meg.m[i] as u32, v, "S m{i:02x} {scn} k={k}");
            }
            for (i, v) in bank(s[15], 0x80).into_iter().enumerate() {
                assert_eq!(swp.meg.r[i] as u32, v, "S r{i:02x} {scn} k={k}");
            }
            assert_eq!(fnv(&swp.reverb_ram[..]), hx(s[16]), "S ram {scn} k={k}");
        } else {
            assert_eq!(swp.meg.p as u64, hx(s[3]), "C p {scn} k={k}");
            let fz = ((swp.meg_flag_n as u64) << 1) | swp.meg_flag_z as u64;
            assert_eq!(fz, hx(s[4]), "C f/z {scn} k={k}");
            for (j, i) in (0x30..0x34).enumerate() {
                assert_eq!(swp.meg.m[i] as u32, hx(s[5 + j]) as u32, "C m{i:x} {scn} k={k}");
            }
            assert_eq!(swp.meg.ram_write, hx(s[9]) as u32, "C rw {scn} k={k}");
            assert_eq!(fnv(&swp.reverb_ram[..]), hx(s[10]), "C ram {scn} k={k}");
        }
    }

    let f = fin.unwrap();
    assert_eq!(swp.meg.icount as u32, hx(f[2]) as u32, "F icount {scn}");
    assert_eq!(swp.meg.pc, hx(f[3]) as u16, "F pc {scn}");
    assert_eq!(swp.meg.p as u64, hx(f[4]), "F p {scn}");
    assert_eq!(fnv(&swp.reverb_ram[..]), hx(f[5]), "F ram {scn}");
    assert_eq!(swp.meg.delay_3, hx(f[6]) as u32, "F d3 {scn}");
    assert_eq!(swp.meg.delay_2, hx(f[7]) as u32, "F d2 {scn}");
    assert_eq!(swp.rand_seed, hx(f[8]) as u32, "F seed {scn}");
}

fn go(text: &str, scn: &str, ctl: &Ctl, st: &[u16]) {
    replay(text, scn, ctl, st);
}

#[test]
fn b_alu_a() {
    go(ALU, "ALUA", &RUN, &sintab());
}
#[test]
fn b_alu_b() {
    go(ALU, "ALUB", &RUN, &sintab());
}
#[test]
fn b_step_alu_a() {
    go(ALU, "STPA", &STEP, &sintab());
}
#[test]
fn b_alu_c() {
    go(ALU, "ALUC", &RUN, &sintab());
}
#[test]
fn b_alu_d() {
    go(ALU, "ALUD", &RUN, &sintab());
}
#[test]
fn b_step_alu_b() {
    go(ALU, "STPB", &STEP, &sintab());
}
#[test]
fn b_step_alu_c() {
    go(ALU, "STPC", &STEP, &sintab());
}
#[test]
fn b_step_alu_d() {
    go(ALU, "STPD", &STEP, &sintab());
}
#[test]
fn b_flow() {
    go(FLOW, "FLOW", &RUN, &sintab());
}
#[test]
fn b_step_flow() {
    go(FLOW, "STPF", &STEP, &sintab());
}
#[test]
fn b_flow2() {
    go(FLOW, "FLOW2", &RUN, &sintab());
}
#[test]
fn b_mem_enabled() {
    go(MEM, "MEM1", &RUN, &sintab());
}
#[test]
fn b_mem_disabled() {
    go(MEM, "MEM2", &RUN, &sintab());
}
#[test]
fn b_lfo_waves_and_midflight() {
    go(LFO, "LFO", &RUN, &sintab());
}
#[test]
fn b_chaos_regen() {
    go(
        CHAOS,
        "CH",
        &Ctl { via_step: false, regen: Some((0xABCD_1234_9876_EF01, 128)) },
        &sintab(),
    );
}
