// license:BSD-3-Clause
//
// origin: src/ui/xg_state.h (308 L) — glue around the work-RAM mirror.
// Moved from ui/ into the xg module for the Rust port (deviation: ui/ stays
// frozen; nothing here touches Windows or the screen — xg_state.h:11).
//
// Deviations (disclosed):
// - XgSnapshot carries only the three arrays this file reads/writes
//   (system/effect/parts). The C++ ui::xg_snapshot (ui/snapshot.h:58-75)
//   also carries kit/notes/velocity/bend/note_ons — those belong to the
//   panel/screenshot rows and every function ported here ignores them.
//   Defaults `= {}` == zeroed here and there.

use super::fx;
use super::model;
use super::model::Model;
use super::ram;

/// origin: ui/snapshot.h:51-54 (the sizes xg_state.h pulls via `using`)
pub const XG_PARTS: usize = 64; // :51 ports A-D (C/D are USB on real gear)
pub const XG_PART_COPY: usize = 0x100; // :52 == ram::PART_COPY
pub const XG_SYSTEM_SIZE: usize = 7; // :53
pub const XG_EFFECT_SIZE: usize = 0x16b; // :54 == ram::EFFECT_SIZE

/// origin: ui/snapshot.h:58-63 (subset — see header deviations)
pub struct XgSnapshot {
    pub system: [u8; XG_SYSTEM_SIZE], // :60
    pub effect: [u8; XG_EFFECT_SIZE], // :62 02 01 00 .. master EQ end
    pub parts: [[u8; XG_PART_COPY]; XG_PARTS], // :63 reordered into XG order
}

impl XgSnapshot {
    /// C++ `= {}` member defaults (snapshot.h:60-63)
    pub fn new() -> XgSnapshot {
        XgSnapshot {
            system: [0; XG_SYSTEM_SIZE], // :60
            effect: [0; XG_EFFECT_SIZE], // :62
            parts: [[0; XG_PART_COPY]; XG_PARTS], // :63
        }
    }
}

/// origin: xg_state.h:29-37 — insertion params 1-10 are 16-bit in RAM;
/// rebuild the XG 03 0n 30-43 7-bit pairs
pub fn ins_wide_bytes(ram_: &XgSnapshot, n: usize, bytes: &mut [u8; 20]) {
    let at = (ram::INS_BLOCK[n] - ram::EFFECT + ram::INS_WIDE) as usize; // :31
    let w = &ram_.effect[at..at + 20]; // :31
    for i in 0..10 {
        // :32
        let v = (w[2 * i] as i32) << 8 | w[2 * i + 1] as i32; // :33
        bytes[2 * i] = ((v >> 7) & 0x7f) as u8; // :34
        bytes[2 * i + 1] = (v & 0x7f) as u8; // :35
    }
}

/// origin: xg_state.h:40-48 — variation params 1-10 (02 01 42-55), same trick
pub fn var_wide_bytes(ram_: &XgSnapshot, bytes: &mut [u8; 20]) {
    let at = (ram::VAR_BLOCK - ram::EFFECT + ram::VAR_WIDE) as usize; // :42
    let w = &ram_.effect[at..at + 20]; // :42
    for i in 0..10 {
        // :43
        let v = (w[2 * i] as i32) << 8 | w[2 * i + 1] as i32; // :44
        bytes[2 * i] = ((v >> 7) & 0x7f) as u8; // :45
        bytes[2 * i + 1] = (v & 0x7f) as u8; // :46
    }
}

/// origin: xg_state.h:52-62 — does insertion n's type use 2-byte params?
/// (When yes, the 1-byte 02-0B are the firmware's 8-bit mirrors and MIDI
/// would clip them — xg_state.h:50-51)
pub fn ins_is_wide(ram_: &XgSnapshot, n: usize) -> bool {
    let blk = ram::INS_BLOCK[n] - ram::EFFECT; // :54
    let kind = (ram_.effect[blk as usize] as i32) << 7 | ram_.effect[blk as usize + 1] as i32; // :55
    let def = match fx::fx_find(kind) {
        Some(d) => d, // :56
        None => return false, // :57-58 (C++ `if (!def) return false`)
    };
    for i in 0..def.params.len() {
        // :58 — def->count == slice length (fx.rs regen asserts it)
        if def.params[i].1 == 2 {
            return true; // :59-60
        }
    }
    false // :61
}

/// origin: xg_state.h:64-84 `load_model` — the screen's way to fill the
/// mirror from work RAM (no MIDI traffic)
pub fn load_model(m: &mut Model, ram_: &XgSnapshot, now_ms: u64) {
    m.load(model::pack(0x00, 0x00, 0x00), &ram_.system, now_ms); // :66
    for blk in ram::EFFECTS.iter() {
        // :67-68
        let off = (blk.ram - ram::EFFECT) as usize;
        m.load(
            model::pack(blk.hi, blk.mid, blk.lo),
            &ram_.effect[off..off + blk.size as usize],
            now_ms,
        );
    }
    for p in 0..XG_PARTS {
        // :69-75
        m.load(
            model::pack(0x08, p as u8, 0x00),
            &ram_.parts[p][..ram::PART_XG_SIZE as usize],
            now_ms,
        );
        m.load(
            model::pack(0x08, p as u8, ram::PART_EQ_XG as u8),
            &ram_.parts[p][ram::PART_EQ_RAM as usize
                ..ram::PART_EQ_RAM as usize + ram::PART_EQ_SIZE as usize],
            now_ms,
        );
        m.load(
            model::pack(0x08, p as u8, ram::PART_EXT_XG as u8),
            &ram_.parts[p][ram::PART_EXT_RAM as usize
                ..ram::PART_EXT_RAM as usize + ram::PART_EXT_SIZE as usize],
            now_ms,
        );
        m.load(
            model::pack(ram::PART_HPF_HI, p as u8, ram::PART_HPF_XG as u8),
            &ram_.parts[p][ram::PART_HPF_RAM as usize..ram::PART_HPF_RAM as usize + 1],
            now_ms,
        );
    }
    for n in 0..4 {
        // :76-80
        let mut bytes = [0u8; 20];
        ins_wide_bytes(ram_, n, &mut bytes); // :78
        m.load(model::pack(0x03, n as u8, 0x30), &bytes, now_ms); // :79
    }
    let mut wide = [0u8; 20]; // :81-83
    var_wide_bytes(ram_, &mut wide); // :82
    m.load(model::pack(0x02, 0x01, 0x42), &wide, now_ms); // :83
}

/// origin: xg_state.h:88-107 `locate_byte` — one byte of the mirror; the
/// 16-bit 03 0n 30-43 addresses are not handled here (:87)
pub fn locate_byte(ram_: &XgSnapshot, hi: u8, mid: u8, lo: u8) -> Option<&u8> {
    if hi == 0x00 && mid == 0x00 && (lo as usize) < XG_SYSTEM_SIZE {
        return Some(&ram_.system[lo as usize]); // :90-91
    }
    if hi == 0x08 && (mid as usize) < XG_PARTS {
        // :92-99
        if (lo as u32) < ram::PART_XG_SIZE {
            return Some(&ram_.parts[mid as usize][lo as usize]); // :93-94
        }
        if (lo as u32) >= ram::PART_EQ_XG && (lo as u32) < ram::PART_EQ_XG + ram::PART_EQ_SIZE {
            // :95-96
            return Some(
                &ram_.parts[mid as usize]
                    [(ram::PART_EQ_RAM + (lo as u32 - ram::PART_EQ_XG)) as usize],
            );
        }
        if (lo as u32) >= ram::PART_EXT_XG
            && (lo as u32) < ram::PART_EXT_XG + ram::PART_EXT_SIZE
        {
            // :97-98
            return Some(
                &ram_.parts[mid as usize]
                    [(ram::PART_EXT_RAM + (lo as u32 - ram::PART_EXT_XG)) as usize],
            );
        }
        return None; // :99
    }
    if hi == ram::PART_HPF_HI && (mid as usize) < XG_PARTS && lo as u32 == ram::PART_HPF_XG {
        return Some(&ram_.parts[mid as usize][ram::PART_HPF_RAM as usize]); // :101-102
    }
    for b in ram::EFFECTS.iter() {
        // :103-105
        if hi == b.hi && mid == b.mid && (lo as u32) >= b.lo as u32 && (lo as u32) < b.lo as u32 + b.size {
            return Some(
                &ram_.effect
                    [(b.ram - ram::EFFECT + (lo as u32 - b.lo as u32)) as usize],
            );
        }
    }
    None // :106
}

/// origin: xg_state.h:110-122 `read_value`
pub fn read_value(p: &model::Param, part: i32, ram_: &XgSnapshot) -> Option<i32> {
    let mid = if p.where_ == model::Area::Part {
        part as u8 // :112
    } else {
        p.mid
    };
    let mut v: i32 = 0; // :113
    for i in 0..p.size as usize {
        // :114
        let b = match locate_byte(ram_, p.hi, mid, (p.lo as usize + i) as u8) {
            // :115
            Some(b) => *b,
            None => return None, // :116-117 unreadable address
        };
        v = if p.enc == model::Coding::Nibble {
            (v << 4) | (b & 0x0f) as i32 // :118
        } else {
            (v << 7) | (b & 0x7f) as i32
        };
    }
    Some(v) // :120-121
}

/// origin: xg_state.h:135-194 `setup_messages` — mirror -> SysEx that rebuilds
/// the same setup on another machine (the plugin's "XG values only" state and
/// the .syx export). Firmware rules proven on disk (xg_state.h:126-134):
/// effect/master-EQ type bytes go first as parameter changes (writing a type
/// resets the block to that type's defaults), part EQ frequencies go one by
/// one (bulk dumps skip them), 74/75 have no address.
pub fn setup_messages(ram_: &XgSnapshot) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(8192); // :138 reserve
    {
        // :139-150 bulk — F0 43 0n 4C cntH cntL hi mid lo data.. sum F7
        let bulk = |out: &mut Vec<u8>, hi: u8, mid: u8, lo: u8, data: &[u8], n: usize| {
            let at = out.len(); // :140 block base; the sum window is at+4.. (F0 43 0n 4C excluded)
            let head: [u8; 9] = [
                0xf0,
                0x43,
                0x00,
                0x4c,
                ((n >> 7) & 0x7f) as u8, // :141
                (n & 0x7f) as u8,
                hi,
                mid,
                lo,
            ];
            out.extend_from_slice(&head); // :142
            for b in data.iter().take(n) {
                out.push(b & 0x7f); // :143-144
            }
            let mut sum: u32 = 0; // :145
            for i in (at + 4)..out.len() {
                // :146 from at+4 == this block's head[4] (cntH) onward; F0 43
                // 0n 4C excluded (the dump checksum covers count/address/data)
                sum += out[i] as u32;
            }
            out.push(((0x80 - (sum & 0x7f)) & 0x7f) as u8); // :148
            out.push(0xf7); // :149
        };
        // :151-157 change — F0 43 10 4C hi mid lo data.. F7
        let change = |out: &mut Vec<u8>, hi: u8, mid: u8, lo: u8, data: &[u8], n: usize| {
            let head: [u8; 7] = [0xf0, 0x43, 0x10, 0x4c, hi, mid, lo]; // :152
            out.extend_from_slice(&head); // :153
            for b in data.iter().take(n) {
                out.push(b & 0x7f); // :154-155
            }
            out.push(0xf7); // :156
        };

        bulk(&mut out, 0x00, 0x00, 0x00, &ram_.system, XG_SYSTEM_SIZE); // :159
        for b in ram::EFFECTS.iter() {
            // :160-173
            let data = &ram_.effect[(b.ram - ram::EFFECT) as usize..]; // :161
            let fx = (b.hi == 0x02 && b.mid == 0x01 && (b.lo == 0x00 || b.lo == 0x20 || b.lo == 0x40))
                || (b.hi == 0x03 && b.lo == 0x00); // :162-163
            let meq = b.hi == 0x02 && b.mid == 0x40 && b.lo == 0x00; // :164
            let mut head: usize = if fx {
                2 // :165 — 2-byte type
            } else if meq {
                1 // master EQ type is 1 byte
            } else {
                0
            };
            if head != 0 {
                change(&mut out, b.hi, b.mid, b.lo, data, head); // :166-167
            }
            // :168-170 — wide insertions skip the 1-byte 02-0B (ins_is_wide)
            if b.hi == 0x03 && b.lo == 0x00 && ins_is_wide(ram_, b.mid as usize) {
                head = 0x0c; // :169-170
            }
            if (b.size as usize) > head {
                // :171-172
                bulk(
                    &mut out,
                    b.hi,
                    b.mid,
                    (b.lo + head as u8) as u8,
                    &data[head..],
                    b.size as usize - head,
                );
            }
        }
        for n in 0..4 {
            // :174-178
            let mut bytes = [0u8; 20];
            ins_wide_bytes(ram_, n, &mut bytes); // :176
            bulk(&mut out, 0x03, n as u8, 0x30, &bytes, 20); // :177
        }
        {
            // :179-183
            let mut bytes = [0u8; 20];
            var_wide_bytes(ram_, &mut bytes); // :181
            bulk(&mut out, 0x02, 0x01, 0x42, &bytes, 20); // :182
        }
        for p in 0..XG_PARTS {
            // :184-192
            bulk(
                &mut out,
                0x08,
                p as u8,
                0x00,
                &ram_.parts[p],
                ram::PART_XG_SIZE as usize,
            ); // :185
            let eq = &ram_.parts[p][ram::PART_EQ_RAM as usize..]; // :186
            for k in [0usize, 1, 4, 5] {
                // :187-188 — 74/75 have no address, do not send
                change(&mut out, 0x08, p as u8, (ram::PART_EQ_XG as usize + k) as u8, &eq[k..], 1);
            }
            // :189-191 41-6E rides the block dump; HPF is one byte
            bulk(
                &mut out,
                0x08,
                p as u8,
                ram::PART_EXT_XG as u8,
                &ram_.parts[p][ram::PART_EXT_RAM as usize..],
                ram::PART_EXT_SIZE as usize,
            );
            change(
                &mut out,
                ram::PART_HPF_HI,
                p as u8,
                ram::PART_HPF_XG as u8,
                &ram_.parts[p][ram::PART_HPF_RAM as usize..],
                1,
            );
        }
    }
    out // :193
}

/// origin: xg_state.h:202-304 `setup_diff_messages` — only what differs from
/// `base` (the XG-System-On mirror), as single parameter changes (the .syx
/// "differences from default only"). Rules at xg_state.h:196-201: effect
/// type changed -> write the type, then the whole block (writing a type
/// resets the block); bank/program move as an MSB-LSB-PC triple; master tune
/// and detune are single values across their byte groups.
pub fn setup_diff_messages(ram_: &XgSnapshot, base: &XgSnapshot) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    // :205-211 change
    fn change(out: &mut Vec<u8>, hi: u8, mid: u8, lo: u8, data: &[u8], n: usize) {
        let head: [u8; 7] = [0xf0, 0x43, 0x10, 0x4c, hi, mid, lo]; // :206
        out.extend_from_slice(&head); // :207
        for b in data.iter().take(n) {
            out.push(b & 0x7f); // :208-209
        }
        out.push(0xf7); // :210
    }
    fn differs(a: &[u8], b: &[u8], n: usize) -> bool {
        a[..n] != b[..n] // :212 memcmp(a, b, n) != 0
    }

    // :214-219 system 00 00 00-06; 00-03 is the one master-tune value
    if differs(&ram_.system, &base.system, 4) {
        change(&mut out, 0x00, 0x00, 0x00, &ram_.system, 4); // :215-216
    }
    for i in 4..XG_SYSTEM_SIZE {
        // :217-219
        if ram_.system[i] != base.system[i] {
            change(&mut out, 0x00, 0x00, i as u8, &ram_.system[i..], 1);
        }
    }

    // :221-227 effect families; type address groups the blocks
    // (wide: 0 none, 1 variation, 2+n insertion n)
    const FAMILIES: [(u8, u8, u8, usize, i32); 8] = [
        (0x02, 0x01, 0x00, 2, 0),
        (0x02, 0x01, 0x20, 2, 0),
        (0x02, 0x01, 0x40, 2, 1),
        (0x03, 0x00, 0x00, 2, 2),
        (0x03, 0x01, 0x00, 2, 3),
        (0x03, 0x02, 0x00, 2, 4),
        (0x03, 0x03, 0x00, 2, 5),
        (0x02, 0x40, 0x00, 1, 0),
    ];
    fn in_family(f: (u8, u8, u8, usize, i32), b: &ram::Block) -> bool {
        // :228-234
        if b.hi != f.0 || b.mid != f.1 {
            return false; // :229-230
        }
        if f.0 == 0x02 && f.1 == 0x01 {
            // :231-232 — 02 01: 00-1F / 20-3F / 40-7F
            return if f.2 == 0x40 {
                b.lo >= 0x40
            } else {
                (b.lo & 0x60) == f.2
            };
        }
        true // :233
    }
    for f in FAMILIES.iter() {
        // :235
        let mut first: Option<&ram::Block> = None; // :236
        for b in ram::EFFECTS.iter() {
            // :237-238
            if in_family(*f, b) && b.lo == f.2 {
                first = Some(b);
                break;
            }
        }
        let first = match first {
            Some(b) => b,
            None => continue, // :239-240
        };
        let type_now = &ram_.effect[(first.ram - ram::EFFECT) as usize..]; // :241
        let type_base = &base.effect[(first.ram - ram::EFFECT) as usize..]; // :242
        let new_type = differs(type_now, type_base, f.3); // :243
        if new_type {
            change(&mut out, f.0, f.1, f.2, type_now, f.3); // :244-245
        }
        // :246 — type changed: the comparison partner reset, send everything
        for b in ram::EFFECTS.iter() {
            // :247
            if !in_family(*f, b) {
                continue; // :248-249
            }
            let now = &ram_.effect[(b.ram - ram::EFFECT) as usize..]; // :250
            let was = &base.effect[(b.ram - ram::EFFECT) as usize..]; // :251
            let skip_narrow =
                f.4 >= 2 && b.lo == 0x00 && ins_is_wide(ram_, (f.4 - 2) as usize); // :252
            let start = if b.lo == f.2 { f.3 } else { 0 }; // :253
            for i in start..b.size as usize {
                // :254-255
                if !(skip_narrow && (0x02..=0x0b).contains(&i)) && (new_type || now[i] != was[i]) {
                    change(&mut out, b.hi, b.mid, (b.lo as u32 + i as u32) as u8, &now[i..], 1);
                }
            }
        }
        if f.4 != 0 {
            // :257-270
            let mut now = [0u8; 20];
            let mut was = [0u8; 20];
            if f.4 == 1 {
                // :259-261
                var_wide_bytes(ram_, &mut now);
                var_wide_bytes(base, &mut was);
            } else {
                // :262-264
                ins_wide_bytes(ram_, (f.4 - 2) as usize, &mut now);
                ins_wide_bytes(base, (f.4 - 2) as usize, &mut was);
            }
            let lo0: u8 = if f.4 == 1 { 0x42 } else { 0x30 }; // :266
            for i in 0..10 {
                // :267-269
                if new_type || differs(&now[2 * i..], &was[2 * i..], 2) {
                    change(
                        &mut out,
                        f.0,
                        f.1,
                        (lo0 as usize + 2 * i) as u8,
                        &now[2 * i..],
                        2,
                    );
                }
            }
        }
    }

    for p in 0..XG_PARTS {
        // :273
        let now = &ram_.parts[p]; // :274
        let was = &base.parts[p]; // :274
        if now[0] != was[0] {
            change(&mut out, 0x08, p as u8, 0x00, now, 1); // :275-276
        }
        if differs(&now[1..], &was[1..], 3) {
            // :277-279 — bank/program move together (program applies them)
            for lo in [1u8, 2, 3] {
                change(&mut out, 0x08, p as u8, lo, &now[lo as usize..], 1);
            }
        }
        let mut lo = 4usize; // :280
        while lo < ram::PART_XG_SIZE as usize {
            if lo == 0x09 {
                // :281-285 — detune (09-0A one value)
                if differs(&now[9..], &was[9..], 2) {
                    change(&mut out, 0x08, p as u8, 0x09, &now[9..], 2);
                }
                lo += 1; // :283 lo++ (then the loop's lo++ skips 0x0A)
                lo += 1;
                continue;
            }
            if now[lo] != was[lo] {
                change(&mut out, 0x08, p as u8, lo as u8, &now[lo..], 1); // :287-288
            }
            lo += 1;
        }
        for k in 0..ram::PART_EXT_SIZE {
            // :290-294
            let at = (ram::PART_EXT_RAM + k) as usize;
            if now[at] != was[at] {
                change(&mut out, 0x08, p as u8, (ram::PART_EXT_XG + k) as u8, &now[at..], 1);
            }
        }
        if now[ram::PART_HPF_RAM as usize] != was[ram::PART_HPF_RAM as usize] {
            // :295-296
            change(
                &mut out,
                ram::PART_HPF_HI,
                p as u8,
                ram::PART_HPF_XG as u8,
                &now[ram::PART_HPF_RAM as usize..],
                1,
            );
        }
        for k in [0u32, 1, 4, 5] {
            // :297-301
            let at = (ram::PART_EQ_RAM + k) as usize;
            if now[at] != was[at] {
                change(
                    &mut out,
                    0x08,
                    p as u8,
                    (ram::PART_EQ_XG + k) as u8,
                    &now[at..],
                    1,
                );
            }
        }
    }
    out // :303
}
