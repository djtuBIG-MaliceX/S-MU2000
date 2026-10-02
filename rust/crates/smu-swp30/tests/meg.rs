//! MEG phase-A bit-exactness replay of `src/mame/sound/swp30.cpp` against
//! ground-truth vectors captured from `%TEMP%\megA\gt.cpp` — a g++
//! (-std=c++20 -O3 -mfpmath=sse -msse2) harness embedding the C++
//! meg_state/revram bodies BYTE-EXACT (swp30.h:73-78 rand; swp30.cpp
//! :1924-1956 reset, :2262-2337 prg/map, :2428-2527 revram, :2908-2911
//! prg_map_r, :3332-3342 lfo table, :3346-3435 const/offset/lfo,
//! :3437-3529 region_of/resolve/get_lfo/m1_expand/call_*, :3535-3568
//! decode_program). Vectors: tests/data/meg_*.txt (hex tuples, SHA1-pinned
//! in the ledger row).
//!
//! No ROMs: sintab (0x8000 words), program RAM and the reverb RAM fill are
//! the harness xorshift LCG (`0x243F6A8885A308D3; s^=s<<13; s^=s>>7;
//! s^=s<<17`) replayed here in the EXACT harness consumption order (see
//! Lcg call sites). Register-facing traffic goes through
//! `Swp30::read16/write16` so the dispatch (regs.rs) is gated too.

use std::sync::OnceLock;

use smu_swp30::meg::{rand_jump, rand_skip, Decoded, LFO_INCREMENT_TABLE};
use smu_swp30::regs::Swp30;

const DECODE: &str = include_str!("data/meg_decode.txt");
const TABLE: &str = include_str!("data/meg_table.txt");
const IO: &str = include_str!("data/meg_io.txt");
const LFO: &str = include_str!("data/meg_lfo.txt");
const ADDR: &str = include_str!("data/meg_addr.txt");
const REVRAM: &str = include_str!("data/meg_revram.txt");
const RAND: &str = include_str!("data/meg_rand.txt");

struct Lcg(u64);
impl Lcg {
    fn new() -> Lcg {
        Lcg(0x243F_6A88_85A3_08D3)
    }
    fn nxt(&mut self) -> u64 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        self.0 = s;
        s
    }
    fn skip(&mut self, n: usize) {
        for _ in 0..n {
            self.nxt();
        }
    }
}

/// The full harness LCG stream: (sintab, program) plus the remaining
/// consumption counts so late scenarios stay stream-aligned.
fn streams() -> (Vec<u16>, Vec<u64>) {
    let mut lcg = Lcg::new();
    let sintab: Vec<u16> = (0..0x8000).map(|_| (lcg.nxt() >> 17) as u16).collect();
    let program: Vec<u64> = (0..0x180).map(|_| lcg.nxt() >> 8).collect();
    (sintab, program)
}

#[test]
fn decode_program_full0x180() {
    let (_sintab, program) = streams();
    let mut swp = Swp30::new();
    swp.meg.program = program.clone().try_into().unwrap();
    swp.meg.decode_program();
    let mut n = 0;
    for line in DECODE.lines() {
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        assert_eq!(f[0], "d");
        let pc: usize = usize::from_str_radix(f[1], 16).unwrap();
        assert_eq!(pc, n);
        assert_eq!(u64::from_str_radix(f[2], 16).unwrap(), program[pc], "opcode pc={pc:x}");
        let d = &swp.meg.decoded[pc];
        let want = Decoded {
            sm: x(f[3]),
            sr: x(f[4]),
            dm: x(f[5]),
            dr: x(f[6]),
            t: x(f[7]),
            mmode: x(f[8]),
            m1t: x(f[9]),
            asel: x(f[10]),
            rop: x(f[11]),
            shift: x(f[12]),
            clamp: x(f[13]),
            dm_src: x(f[14]),
            memop: x(f[15]),
            m1_expand: b(f[16]),
            m2_from_m: b(f[17]),
            dr_from_r: b(f[18]),
            no_noise: b(f[19]),
            memw: b(f[20]),
            index: b(f[21]),
            t_write: b(f[22]),
            t_from_p: b(f[23]),
            mem_use_index: b(f[24]),
            index2: b(f[25]),
            mem_use_index2: b(f[26]),
            mem_table: b(f[27]),
        };
        assert_eq!(*d, want, "decoded pc={pc:x}");
        // the harness flg column: packed bool bits (cross-check of the order)
        let flg: u32 = u32::from_str_radix(f[28], 16).unwrap();
        let bits = [
            want.m1_expand, want.m2_from_m, want.dr_from_r, want.no_noise, want.memw, want.index,
            want.t_write, want.t_from_p, want.mem_use_index, want.index2, want.mem_use_index2,
            want.mem_table,
        ];
        let mut pack = 0u32;
        for (i, v) in bits.iter().enumerate() {
            if *v {
                pack |= 1 << i;
            }
        }
        assert_eq!(pack, flg, "flg pc={pc:x}");
        n += 1;
    }
    assert_eq!(n, 0x180);
}

#[test]
fn lfo_increment_table_locked() {
    let mut n = 0;
    for line in TABLE.lines() {
        let f: Vec<&str> = line.split(',').collect();
        assert_eq!(f[0], "t");
        let i: usize = usize::from_str_radix(f[1], 16).unwrap();
        assert_eq!(i, n);
        assert_eq!(LFO_INCREMENT_TABLE[i], u32::from_str_radix(f[2], 16).unwrap());
        n += 1;
    }
    assert_eq!(n, 256);
}

fn x(s: &str) -> u8 {
    u8::from_str_radix(s, 16).unwrap()
}
fn b(s: &str) -> bool {
    s.parse::<u32>().unwrap() != 0
}

/// io scenario: harness LCG consumption AFTER the program fill:
/// pw 12x1, mw 8x1, cw 24x2, ow 16x3, lfo/counter init 24x2, lw 12x2.
struct IoState {
    swp: Swp30,
    lcg: Lcg,
}

fn io_replay() -> IoState {
    let (_sintab, program) = streams();
    let mut swp = Swp30::new();
    swp.meg.program = program.try_into().unwrap();
    let mut lcg = Lcg::new();
    lcg.skip(0x8000 + 0x180); // sintab + program
    // pa: no LCG
    for line in IO.lines() {
        if line.starts_with("pa,") {
            let f: Vec<&str> = line.split(',').collect();
            let a = u16::from_str_radix(f[1], 16).unwrap();
            let want = u16::from_str_radix(f[2], 16).unwrap();
            swp.write16(0x40f, a); // meg_prg_address_w :2196
            assert_eq!(swp.read16(0x40f), want, "pa {line}");
        }
    }
    // pw: start address 0x17d (harness :prg_address_w(0x17d))
    swp.meg.prg_address_w(0x17d);
    swp.meg_program_changed = false;
    for line in IO.lines() {
        if line.starts_with("pw,") {
            let f: Vec<&str> = line.split(',').collect();
            let sel: usize = f[1].parse().unwrap();
            let v = u16::from_str_radix(f[2], 16).unwrap();
            let reread = u16::from_str_radix(f[3], 16).unwrap();
            let addr_after = u16::from_str_radix(f[4], 16).unwrap();
            let changed: u32 = f[5].parse().unwrap();
            let entry = u64::from_str_radix(f[6], 16).unwrap();
            let addr_before = swp.meg.prg_address_r();
            let reg: u32 = match sel {
                0 => 0x44e,
                1 => 0x44f,
                2 => 0x48e,
                _ => 0x48f,
            };
            swp.write16(reg, v); // meg_prg_w<Sel>
            assert_eq!(swp.read16(reg), reread, "pw reread {line}");
            assert_eq!(swp.meg.prg_address_r(), addr_after, "pw auto-inc {line}");
            assert_eq!(swp.meg_program_changed, changed != 0, "pw changed {line}");
            assert_eq!(swp.meg.program[addr_before as usize], entry, "pw entry {line}");
            lcg.nxt();
        }
    }
    let mut q = 0;
    for line in IO.lines() {
        if line.starts_with("q,") {
            let v = u64::from_str_radix(line.split(',').nth(1).unwrap(), 16).unwrap();
            let idx = [0x17fusize, 0][q];
            assert_eq!(swp.meg.program[idx], v, "q idx={idx:x}");
            q += 1;
        }
    }
    assert_eq!(q, 2);
    for line in IO.lines() {
        if line.starts_with("mw,") {
            let f: Vec<&str> = line.split(',').collect();
            let sel: usize = f[1].parse().unwrap();
            let v = u16::from_str_radix(f[2], 16).unwrap();
            let reread = u16::from_str_radix(f[3], 16).unwrap();
            swp.meg_program_changed = false;
            swp.write16(0x60e + 0x40 * (sel as u32), v); // meg_map_w<Sel> :2201-08
            assert_eq!(swp.read16(0x60e + 0x40 * (sel as u32)), reread, "mw {line}");
            assert!(swp.meg_program_changed, "mw changed {line}");
            lcg.nxt();
        }
    }
    for line in IO.lines() {
        if line.starts_with("cw,") {
            let f: Vec<&str> = line.split(',').collect();
            let off = u32::from_str_radix(f[1], 16).unwrap();
            let v = u16::from_str_radix(f[2], 16).unwrap();
            let reread = u16::from_str_radix(f[3], 16).unwrap();
            let g1 = u32::from_str_radix(f[5], 16).unwrap();
            let g2 = u32::from_str_radix(f[6], 16).unwrap();
            let chan = off >> 6;
            let sel = off % 6; // harness cwr() dispatch rule
            let reg = (chan << 6) | (0x21 + 2 * sel);
            assert_eq!(swp.meg_const_gen, u32::from_str_radix(f[4], 16).unwrap(), "cw g0 {line}");
            swp.write16(reg, v);
            assert_eq!(swp.read16(reg), reread, "cw reread {line}");
            assert_eq!(swp.meg_const_gen, g1, "cw g1 {line}");
            swp.write16(reg, v); // same value: gen must NOT move (:3353)
            assert_eq!(swp.meg_const_gen, g2, "cw g2 {line}");
            lcg.skip(2);
        }
    }
    for line in IO.lines() {
        if line.starts_with("ow,") {
            let f: Vec<&str> = line.split(',').collect();
            let off = u32::from_str_radix(f[1], 16).unwrap();
            let sel: u32 = f[2].parse().unwrap();
            let v = u16::from_str_radix(f[3], 16).unwrap();
            let reread = u16::from_str_radix(f[4], 16).unwrap();
            let reg = off | 0x30 | sel; // chan<<6 (+Sel slot 0/1)
            swp.write16(reg, v);
            assert_eq!(swp.read16(reg), reread, "ow {line}");
            lcg.skip(3);
        }
    }
    // counter/lfo random init (24x2) — values pinned later via L, stream-align only
    lcg.skip(48);
    for line in IO.lines() {
        if line.starts_with("lw,") {
            let f: Vec<&str> = line.split(',').collect();
            let off = u32::from_str_radix(f[1], 16).unwrap();
            let sel: u32 = f[2].parse().unwrap();
            let v = u16::from_str_radix(f[3], 16).unwrap();
            let reread = u16::from_str_radix(f[4], 16).unwrap();
            let counter = u32::from_str_radix(f[5], 16).unwrap();
            let reg = off | 0x3e | sel;
            swp.write16(reg, v); // meg_lfo_w<Sel>
            assert_eq!(swp.read16(reg), reread, "lw reread {line}");
            // lfo_w zeroes the phase counter (:3377-3378)
            assert_eq!(counter, 0, "lw counter {line}");
            lcg.skip(2);
        }
    }
    IoState { swp, lcg }
}

#[test]
fn prg_map_const_offset_lfo_io() {
    io_replay();
}

#[test]
fn lfo_commit_step_get() {
    let (sintab, _program) = streams();
    let mut st = io_replay();
    // wave init consumed 24x2 of LCG (values pinned by the L lines)
    st.lcg.skip(48);
    let mut lfo: [u16; 0x18] = [0; 0x18];
    let mut counter: [u32; 0x18] = [0; 0x18];
    for line in LFO.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f[0] == "L" {
            let i: usize = usize::from_str_radix(f[1], 16).unwrap();
            lfo[i] = u16::from_str_radix(f[2], 16).unwrap();
            counter[i] = u32::from_str_radix(f[3], 16).unwrap();
            let inc = u32::from_str_radix(f[4], 16).unwrap();
            st.swp.meg.lfo = lfo;
            st.swp.meg.lfo_counter = counter;
            st.swp.meg.lfo_commit_w(); // :3382-3386
            assert_eq!(st.swp.meg.lfo_increment[i], inc, "commit i={i:x}");
        }
    }
    st.swp.meg.lfo = lfo;
    st.swp.meg.lfo_counter = counter;
    st.swp.meg.lfo_commit_w();
    for line in LFO.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f[0] == "g" {
            let it: u32 = f[1].parse().unwrap();
            let i: usize = usize::from_str_radix(f[2], 16).unwrap();
            let want = u32::from_str_radix(f[3], 16).unwrap();
            // harness steps ONCE at the head of every iteration (:lfo_step)
            if i == 0 {
                st.swp.meg.lfo_step();
            }
            assert_eq!(st.swp.meg.get_lfo(i, &sintab), want, "g it={it} i={i:x}");
        }
    }
}

/// Harness map-config sweep (scaffolding inputs duplicated verbatim from
/// %TEMP%\megA\gt.cpp; the r/s vectors pin their use through the C++ bodies).
const MAPS: [[u16; 8]; 4] = [
    [0x0400, 0x2401, 0x4402, 0x6403, 0x8404, 0xa405, 0xc406, 0xe407],
    [0x0010, 0x0011, 0x0011, 0x4012, 0x8013, 0x4014, 0xc015, 0xc016],
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0xf8ff, 0x0100, 0xf900, 0x0200, 0xfa00, 0x0300, 0xfb00, 0x0400],
];

#[test]
fn region_resolve_sweep() {
    let (pc_blocks, s_lines): (Vec<(u32, u32)>, Vec<(u32, u32, u32)>) = {
        let mut p = Vec::new();
        let mut s = Vec::new();
        for line in ADDR.lines() {
            let f: Vec<&str> = line.split(',').collect();
            match f[0] {
                "r" => p.push((
                    u32::from_str_radix(f[1], 16).unwrap(),
                    u32::from_str_radix(f[2], 16).unwrap(),
                )),
                "s" => s.push((
                    u32::from_str_radix(f[1], 16).unwrap(),
                    u32::from_str_radix(f[2], 16).unwrap(),
                    u32::from_str_radix(f[3], 16).unwrap(),
                )),
                _ => {}
            }
        }
        (p, s)
    };
    let r_per_map = pc_blocks.len() / 4;
    let s_per_map = s_lines.len() / 4;
    for (mi, m) in MAPS.iter().enumerate() {
        let mut swp = Swp30::new();
        // map through the REAL dispatch (meg_map_w sets program_changed too)
        for i in 0..8 {
            swp.write16(0x60e + 0x40 * (i as u32), m[i]);
        }
        for (pc, want) in &pc_blocks[mi * r_per_map..(mi + 1) * r_per_map] {
            assert_eq!(
                swp.meg.region_of(*pc as u16) as u32, *want,
                "map{mi} region_of pc={pc:x}"
            );
        }
        for (pc, off, want) in &s_lines[mi * s_per_map..(mi + 1) * s_per_map] {
            assert_eq!(
                swp.meg.resolve_address(*pc as u16, *off as i32),
                *want,
                "map{mi} resolve pc={pc:x} off={off:x}"
            );
        }
    }
}

#[test]
fn m1_expand_and_prg_map_r() {
    for line in ADDR.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f[0] == "e" {
            let v = u16::from_str_radix(f[1], 16).unwrap() as i16;
            let want = u16::from_str_radix(f[2], 16).unwrap();
            assert_eq!(smu_swp30::meg::MegState::m1_expand(v) as u16, want, "m1 {line}");
        }
    }
    // meg_prg_map_r (:2908-2911): the q lines of the addr file are
    // program[0,1,80,17f] after the io replay (same program mutation order).
    let (mut q, addrs) = (0usize, [0usize, 1, 0x80, 0x17f]);
    let (_sintab, program) = streams();
    let mut swp = Swp30::new();
    swp.meg.program = program.clone().try_into().unwrap();
    io_replay_mut(&mut swp);
    for line in ADDR.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f[0] == "q" && f.len() == 3 {
            let a: usize = usize::from_str_radix(f[1], 16).unwrap();
            let want = u64::from_str_radix(f[2], 16).unwrap();
            assert_eq!(a, addrs[q], "q addr order");
            // the program space read = program[address]; the non-pow2
            // flat_space wraps mod 0x180 (mamecompat.h:215), NOT a bitmask
            assert_eq!(swp.meg.program[a % 0x180], want, "prg_map_r {line}");
            q += 1;
        }
    }
    assert_eq!(q, 4);
}

/// Same traffic as io_replay() but on a caller's device (program must be the
/// scenario fill first — program mutation then matches the harness byte-for-byte).
fn io_replay_mut(swp: &mut Swp30) {
    let mut lcg = Lcg::new();
    lcg.skip(0x8000 + 0x180);
    for line in IO.lines() {
        if line.starts_with("pa,") {
            let f: Vec<&str> = line.split(',').collect();
            swp.write16(0x40f, u16::from_str_radix(f[1], 16).unwrap());
        }
    }
    swp.meg.prg_address_w(0x17d);
    for line in IO.lines() {
        if line.starts_with("pw,") {
            let f: Vec<&str> = line.split(',').collect();
            let sel: usize = f[1].parse().unwrap();
            let v = u16::from_str_radix(f[2], 16).unwrap();
            let reg: u32 = match sel {
                0 => 0x44e,
                1 => 0x44f,
                2 => 0x48e,
                _ => 0x48f,
            };
            swp.write16(reg, v);
            lcg.nxt();
        }
    }
    for line in IO.lines() {
        if line.starts_with("mw,") {
            let f: Vec<&str> = line.split(',').collect();
            let sel: u32 = f[1].parse().unwrap();
            let v = u16::from_str_radix(f[2], 16).unwrap();
            swp.write16(0x60e + 0x40 * sel, v);
            lcg.nxt();
        }
        if line.starts_with("cw,") {
            let f: Vec<&str> = line.split(',').collect();
            let off = u32::from_str_radix(f[1], 16).unwrap();
            let v = u16::from_str_radix(f[2], 16).unwrap();
            let reg = (off >> 6 << 6) | (0x21 + 2 * (off % 6));
            swp.write16(reg, v);
            swp.write16(reg, v);
            lcg.skip(2);
        }
        if line.starts_with("ow,") {
            let f: Vec<&str> = line.split(',').collect();
            let off = u32::from_str_radix(f[1], 16).unwrap();
            let sel: u32 = f[2].parse().unwrap();
            let v = u16::from_str_radix(f[3], 16).unwrap();
            swp.write16(off | 0x30 | sel, v);
            lcg.skip(3);
        }
        if line.starts_with("lw,") {
            let f: Vec<&str> = line.split(',').collect();
            let off = u32::from_str_radix(f[1], 16).unwrap();
            let sel: u32 = f[2].parse().unwrap();
            let v = u16::from_str_radix(f[3], 16).unwrap();
            swp.write16(off | 0x3e | sel, v);
            lcg.skip(2);
        }
    }
}

#[test]
fn revram_codec_enable_clear_adrdata() {
    let (mut swp, mut lcg) = {
        let s = Swp30::new();
        let l = Lcg::new();
        (s, l)
    };
    // consume the stream up to the revram scenarios:
    // sintab 0x8000, program 0x180, io (12+8+48+48+48+24), lfo wave 48,
    // region scenarios 0
    lcg.skip(0x8000 + 0x180 + 12 + 8 + 48 + 48 + 48 + 24 + 48);
    // re: encode/decode round-trip lines. Harness consumed 200 LCG draws for
    // the random values; the 68 boundary-pushed lines draw nothing.
    let mut re_idx = 0usize;
    for line in REVRAM.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f[0] == "re" {
            let v = u32::from_str_radix(f[1], 16).unwrap();
            let enc = u16::from_str_radix(f[2], 16).unwrap();
            let dec = u32::from_str_radix(f[3], 16).unwrap();
            assert_eq!(smu_swp30::meg::MegState::revram_encode(v), enc, "re {line}");
            assert_eq!(smu_swp30::meg::MegState::revram_decode(enc), dec, "re dec {line}");
            if re_idx < 200 {
                lcg.nxt();
            }
            re_idx += 1;
        }
    }
    assert_eq!(re_idx, 268);
    for line in REVRAM.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f[0] == "rd" {
            let v = u16::from_str_radix(f[1], 16).unwrap();
            let want = u32::from_str_radix(f[2], 16).unwrap();
            assert_eq!(smu_swp30::meg::MegState::revram_decode(v), want, "rd {line}");
            lcg.nxt();
        }
    }
    // enable machine (fresh enable=0, jit_wait=0)
    for line in REVRAM.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f[0] == "e" {
            let d = u16::from_str_radix(f[1], 16).unwrap();
            let en = u16::from_str_radix(f[2], 16).unwrap();
            let wait = u32::from_str_radix(f[3], 16).unwrap();
            swp.write16(0x80e, d); // revram_enable_w
            assert_eq!(swp.revram_enable, en, "en {line}");
            assert_eq!(swp.meg_jit_wait, wait, "jit_wait {line}");
        }
        if f[0] == "en" {
            assert_eq!(swp.read16(0x84e), 0, "status"); // revram_status_r :2495
        }
    }
    // clear scenario: cmap literal + LCG ram fill (positioned), then the
    // harness clear sequence 0001, 0010, 0020, 00ff — each clear dumps one
    // "c" line per set bit, ascending i, AFTER clearing.
    const CMAP: [u16; 8] = [0x0400, 0x0501, 0x0602, 0x0703, 0x0004, 0xfd05, 0x0206, 0x0307];
    for i in 0..8 {
        swp.meg.map[i] = CMAP[i];
    }
    for w in swp.reverb_ram.iter_mut() {
        *w = lcg.nxt() as u16;
    }
    const CLRS: [u16; 4] = [0x0001, 0x0010, 0x0020, 0x00ff];
    let expect: Vec<(u16, usize)> = CLRS
        .iter()
        .flat_map(|c| (0..8usize).filter(move |i| c & (1 << i) != 0).map(move |i| (*c, i)))
        .collect();
    let mut clines: Vec<Vec<&str>> = REVRAM
        .lines()
        .filter(|l| l.starts_with("c,"))
        .map(|l| l.split(',').collect())
        .collect();
    assert_eq!(clines.len(), expect.len(), "c-line count");
    for ((c, i), f) in expect.iter().zip(clines.drain(..)) {
        let li = usize::from_str_radix(f[1], 16).unwrap();
        assert_eq!(li, *i, "c-line order");
        let base = usize::from_str_radix(f[2], 16).unwrap();
        let end = usize::from_str_radix(f[4], 16).unwrap();
        let outside_before = u16::from_str_radix(f[5], 16).unwrap();
        let first = u16::from_str_radix(f[6], 16).unwrap();
        let last = u16::from_str_radix(f[7], 16).unwrap();
        let outside_after = u16::from_str_radix(f[8], 16).unwrap();
        let mid = usize::from_str_radix(f[9], 16).unwrap();
        swp.write16(0x80f, *c); // revram_clear_w :2210/:2470-2493
        assert_eq!(swp.reverb_ram[base], first, "clear c={c:x} i={i} first");
        assert_eq!(swp.reverb_ram[end - 1], last, "clear c={c:x} i={i} last");
        if base > 0 {
            assert_eq!(swp.reverb_ram[base - 1], outside_before, "clear c={c:x} i={i} before");
        }
        if end < swp.reverb_ram.len() {
            assert_eq!(swp.reverb_ram[end], outside_after, "clear c={c:x} i={i} after");
        }
        assert_eq!(swp.reverb_ram[mid], 0, "clear c={c:x} i={i} mid");
    }
    // ad/dw/ar loop (self-contained per line thanks to the raw column)
    swp.meg.map[0] = 0x0400;
    let mut rep = 0;
    let _ = rep;
    for line in REVRAM.lines() {
        let f: Vec<&str> = line.split(',').collect();
        match f[0] {
            "ad" => {
                let sel: u32 = f[1].parse().unwrap();
                let d = u16::from_str_radix(f[2], 16).unwrap();
                let adr = u32::from_str_radix(f[3], 16).unwrap();
                swp.write16(if sel != 0 { 0x94e } else { 0x94f }, d);
                assert_eq!(swp.revram_adr, adr, "ad {line}");
            }
            "ar" if f[1] == "1" => {
                let want = u16::from_str_radix(f[2], 16).unwrap();
                let raw = u16::from_str_radix(f[3], 16).unwrap();
                let a = (swp.revram_adr & 0x3_ffff) as usize;
                assert_eq!(swp.reverb_ram[a], raw, "ar raw {line}");
                assert_eq!(swp.read16(0x98e), want, "ar,1 {line}");
            }
            "dw" if f[1] == "1" => {
                let d = u16::from_str_radix(f[2], 16).unwrap();
                let adr = u32::from_str_radix(f[3], 16).unwrap();
                let data = u32::from_str_radix(f[4], 16).unwrap();
                swp.write16(0x98e, d);
                assert_eq!(swp.revram_adr, adr, "dw1 adr {line}");
                assert_eq!(swp.revram_data, data, "dw1 data {line}");
            }
            "dw" => {
                let d = u16::from_str_radix(f[2], 16).unwrap();
                let data = u32::from_str_radix(f[3], 16).unwrap();
                let ramword = u16::from_str_radix(f[4], 16).unwrap();
                let reread = u16::from_str_radix(f[5], 16).unwrap();
                swp.write16(0x98f, d); // Sel=0 -> encodes into RAM (:2517-2518)
                assert_eq!(swp.revram_data, data, "dw0 data {line}");
                let a = (swp.revram_adr & 0x3_ffff) as usize;
                assert_eq!(swp.reverb_ram[a], ramword, "dw0 ram {line}");
                assert_eq!(swp.read16(0x98f), reread, "ar,0 {line}");
                rep += 1;
                if rep == 10 {
                    break;
                }
                // next iteration inputs: a1,a0,d1,d0 consumed implicitly
            }
            _ => {}
        }
    }
    assert_eq!(rep, 10);
}

#[test]
fn rand_and_revram_calls() {
    let mut swp = Swp30::new();
    swp.rand_seed = 0x9d14abd7; // harness reseed before the n-loop
    for line in RAND.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f[0] == "n" {
            swp.meg.call_rand(&mut swp.rand_seed);
            assert_eq!(swp.meg.retval, u32::from_str_radix(f[2], 16).unwrap(), "n {line}");
            assert_eq!(swp.rand_seed, u32::from_str_radix(f[3], 16).unwrap(), "n seed {line}");
        }
        // merged 6.237 rand_jump/rand_skip pins (gt_rand2.cpp j lines)
        if f[0] == "j" {
            let n = u32::from_str_radix(f[1], 16).unwrap();
            swp.rand_seed = 0x9d14abd7;
            rand_skip(&mut swp.rand_seed, n);
            assert_eq!(swp.rand_seed, u32::from_str_radix(f[2], 16).unwrap(), "j {line}");
            // the (mul,add) pair must equal n single rand() calls from ANY
            // seed (small n loop-checked here, all n pinned by the seed line)
            let (mul, add) = rand_jump(n);
            if n <= 4096 {
                let probe = 0x1357_9BDFu32.wrapping_mul(n | 1);
                let mut s = probe;
                for _ in 0..n {
                    smu_swp30::voice::swp_rand(&mut s);
                }
                assert_eq!(
                    mul.wrapping_mul(probe).wrapping_add(add),
                    s,
                    "j mul/add n={n:x}"
                );
            }
        }
        if f[0] == "x" {
            let v0 = u32::from_str_radix(f[1], 16).unwrap();
            let v1 = u32::from_str_radix(f[2], 16).unwrap();
            let v2 = u32::from_str_radix(f[3], 16).unwrap();
            swp.meg.retval = v0;
            swp.meg.call_revram_encode();
            assert_eq!(swp.meg.retval, v1, "x enc {line}");
            swp.meg.call_revram_decode();
            assert_eq!(swp.meg.retval, v2, "x dec {line}");
        }
    }
}

/// Invariant-3 / reset-order probe: a dirty device returns to the C++
/// reset-state (:1924-1956 order) and keeps reverb RAM (disk does NOT
/// re-clear it in device reset).
#[test]
fn reset_preserves_ram_and_order_state() {
    let mut swp = Swp30::new();
    assert!(swp.meg_program_changed, "ctor flag :1893");
    swp.write16(0x80f, 0x1);
    let a = (swp.revram_adr & 0x3_ffff) as usize;
    let pre = swp.reverb_ram[a];
    swp.write16(0x44e, 0x1234);
    swp.write16(0x21, 0x0f0f);
    swp.reset();
    assert!(!swp.reverb_ram.is_empty());
    assert_eq!(swp.reverb_ram[a], pre, "RAM NOT cleared by device reset");
    assert_eq!(swp.meg.program[0], 0);
    assert_eq!(swp.meg.prg_address_r(), 0);
    assert_eq!(swp.read16(0x21), 0);
    assert_eq!(swp.meg_jit_wait, 0, "never written -> stays");
    assert_eq!(swp.meg_const_gen, 1, "const_gen survives reset (no disk write)");
    assert!(swp.meg_program_changed, "program_changed survives reset (:1958-2000)");
    assert_eq!(swp.deferred_hits, 0);
    let _ = &mut swp;
}

#[allow(dead_code)]
static INIT: OnceLock<()> = OnceLock::new();
