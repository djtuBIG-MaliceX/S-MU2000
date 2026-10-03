// license:BSD-3-Clause
//
// origin: src/xg/ram.h (230 L) — where the XG values live in the firmware
// work RAM (0x400000-0x43ffff). Offsets below are RELATIVE to 0x400000,
// i.e. indexes into Machine::soc.ram (the Rust nvram() seam,
// mu2000.h:81 == Sh7042 ram : mu2000.cpp:823 add_region(0x400000,...)).
// Screen reads RAM instead of asking (doc/params.md "read from RAM").
// Address table proven byte-by-byte by xgtest (origin ram.h:9-10).

/// origin: ram.h:23 `SYSTEM` — 00 00 00-06
pub const SYSTEM: u32 = 0x226c1;
/// origin: ram.h:30 — 10ms tick mark (pc=0x12989A flips 0/1)
pub const TICK_MARK: u32 = 0x3e948;
/// origin: ram.h:36 — pan Rnd seed x=(0xB3*x+0x11)&0xFF, pan=x>>1
pub const PAN_RND: u32 = 0x3e94c;
/// origin: ram.h:45 — LCD meter source (DO NOT WRITE, ram.h:41-44)
pub const METER_SRC: u32 = 0x2dd8;
pub const SYS_TUNE: u32 = SYSTEM + 0; // ram.h:46 (00 00 00-03, 4x4-bit)
pub const SYS_VOLUME: u32 = SYSTEM + 4; // ram.h:47 (00 00 04)
/// origin: ram.h:52 — per-part gain scale 0-128 (mu writes it, 0x12A4AA)
pub const PART_GAIN: u32 = 0x12f;
pub const SYS_TRANSPOSE: u32 = SYSTEM + 6; // ram.h:53 (64 = 0 semitone)

/// origin: ram.h:59-63 — drum setup (XG `3n rr nn`), located by probing
pub const DRUM_SETUP: u32 = 0x226e1; // set 0, key 13, param 0
pub const DRUM_SETUP_PARAM: u32 = 23;
pub const DRUM_SETUP_NOTES: u32 = 79;
pub const DRUM_SETUP_NOTE0: i32 = 13;
pub const DRUM_SETUP_SETS: i32 = 4;

/// origin: ram.h:65-69
pub fn drum_setup(set: i32, note: i32, param: i32) -> u32 {
    DRUM_SETUP
        + (set as u32) * DRUM_SETUP_PARAM * DRUM_SETUP_NOTES
        + ((note - DRUM_SETUP_NOTE0) as u32) * DRUM_SETUP_PARAM
        + (param as u32)
}

/// origin: ram.h:73-87 — XG addr (3n rr pp) -> slot 0-22, else -1
pub fn drum_setup_index(addr: i32) -> i32 {
    if addr >= 0 && addr < 16 {
        return addr;
    }
    match addr {
        0x20 => 16, // ram.h:78 EQ low gain
        0x21 => 17, // :79 EQ high gain
        0x24 => 18, // :80 EQ low freq
        0x25 => 19, // :81 EQ high freq
        0x50 => 20, // :82 HPF
        0x60 => 21, // :83 vel->pitch
        0x61 => 22, // :84 vel->cut
        _ => -1, // :85
    }
}

/// origin: ram.h:89-92 — factory defaults in work RAM
pub fn drum_setup_default(index: i32) -> i32 {
    if index == 18 {
        0x0c
    } else if index == 19 {
        0x36
    } else {
        64
    }
}

/// origin: ram.h:94 — voice pull mode (1 = XG), feeds xg/voices.h lookup
pub const VOICE_MODE: u32 = 0x226bc;
/// origin: ram.h:95 — voice set selection (MU2000 kit = 1)
pub const VOICE_SET: u32 = 0x226de;
/// origin: ram.h:96-97 — effects block start (02 01 00) and length
pub const EFFECT: u32 = 0x0cad8;
pub const EFFECT_SIZE: usize = 0x16b;
/// origin: ram.h:101-102 — part blocks; NOT XG part order: per port the
/// 10th part (ch10) is slot 0, then 1-9, 11-16 (drum mode does not move it)
pub const PARTS: u32 = 0x28d64;
pub const PART_STRIDE: u32 = 0x134;

/// origin: ram.h:105-110 — XG part number (0-31) -> block head
pub fn part_base(part: i32) -> u32 {
    let port = part / 16; // :107 (port 0-1 for part 0-31; 32-63 land past
    let k = part % 16; // the table the UI copies — same math as disk)
    let slot = if k == 9 {
        0 // :108
    } else if k < 9 {
        k + 1
    } else {
        k
    };
    PARTS + ((port * 16 + slot) as u32) * PART_STRIDE // :109
}

pub const PART_XG_SIZE: u32 = 0x29; // ram.h:111 (08 pp 00-28)
/// origin: ram.h:116-118 — scale tuning 08 pp 41-4C = 12 bytes at +0x3a
pub const PART_SCALE_XG: u32 = 0x41;
pub const PART_SCALE_RAM: u32 = 0x3a;
pub const PART_SCALE_SIZE: u32 = 12;
/// origin: ram.h:122-124 — 08 pp 41-6E block at +0x3a, 0x2e bytes
pub const PART_EXT_XG: u32 = 0x41;
pub const PART_EXT_RAM: u32 = 0x3a;
pub const PART_EXT_SIZE: u32 = 0x2e;
/// origin: ram.h:126-128 — HPF cutoff: XG addr 0A pp 20, block +0x78
pub const PART_HPF_HI: u8 = 0x0a;
pub const PART_HPF_XG: u32 = 0x20;
pub const PART_HPF_RAM: u32 = 0x78;
/// origin: ram.h:129-131 — part EQ 08 pp 72-77 at +0x6a
pub const PART_EQ_XG: u32 = 0x72;
pub const PART_EQ_RAM: u32 = 0x6a;
pub const PART_EQ_SIZE: u32 = 6;
/// origin: ram.h:133-136 (6.181)
pub const PART_EQ_LGAIN: u32 = 0x6a;
pub const PART_EQ_HGAIN: u32 = 0x6b;
pub const PART_EQ_LFREQ: u32 = 0x6e;
pub const PART_EQ_HFREQ: u32 = 0x6f;

/// origin: ram.h:139-148 — in-block playing values with no XG address
pub const PART_MOD: u32 = 0x7d; // CC1
pub const PART_EXP: u32 = 0x7e; // CC11
pub const PART_COARSE: u32 = 0xc9; // RPN 2, signed semitones (6.125)
pub const PART_FINE: u32 = 0xcc; // RPN 1, 16-bit signed, 8192 = 100 cents
pub const PART_BEND: u32 = 0x80; // pitch-bend MSB half (0x20 = center)
pub const PART_HOLD: u32 = 0xd9; // CC64, 0/1
pub const PART_VOICE: u32 = 0xf8; // voice record pointer (into ROM)
pub const PART_COPY: usize = 0x100; // length copied to the screen

/// origin: ram.h:151 `struct block`
#[derive(Clone, Copy)]
pub struct Block {
    pub hi: u8,
    pub mid: u8,
    pub lo: u8,
    pub size: u32,
    pub ram: u32,
}

/// origin: ram.h:159-179 `EFFECTS` — reverb/chorus/variation are PACKED in
/// RAM (not the raw XG addresses), probed byte-by-byte; insertion 00-11 runs
/// straight, then 20-25 packed; wide (30-43) params are 16-bit at +0x18 and
/// are NOT in this table (see INS_WIDE)
pub const EFFECTS: [Block; 15] = [
    Block { hi: 0x02, mid: 0x01, lo: 0x00, size: 0x0e, ram: 0x0cad8 }, // :160 reverb
    Block { hi: 0x02, mid: 0x01, lo: 0x10, size: 0x06, ram: 0x0cae6 }, // :161
    Block { hi: 0x02, mid: 0x01, lo: 0x20, size: 0x0f, ram: 0x0caec }, // :162 chorus
    Block { hi: 0x02, mid: 0x01, lo: 0x40, size: 0x02, ram: 0x0cb02 }, // :163 variation
    Block { hi: 0x02, mid: 0x01, lo: 0x56, size: 0x0b, ram: 0x0cb18 }, // :164
    Block { hi: 0x02, mid: 0x01, lo: 0x70, size: 0x06, ram: 0x0cb23 }, // :165
    Block { hi: 0x03, mid: 0x00, lo: 0x00, size: 0x12, ram: 0x0cb7e }, // :170 insertion 1
    Block { hi: 0x03, mid: 0x00, lo: 0x20, size: 0x06, ram: 0x0cb90 }, // :171
    Block { hi: 0x03, mid: 0x01, lo: 0x00, size: 0x12, ram: 0x0cbaa }, // :172 insertion 2
    Block { hi: 0x03, mid: 0x01, lo: 0x20, size: 0x06, ram: 0x0cbbc }, // :173
    Block { hi: 0x03, mid: 0x02, lo: 0x00, size: 0x12, ram: 0x0cbd6 }, // :174 insertion 3
    Block { hi: 0x03, mid: 0x02, lo: 0x20, size: 0x06, ram: 0x0cbe8 }, // :175
    Block { hi: 0x03, mid: 0x03, lo: 0x00, size: 0x12, ram: 0x0cc02 }, // :176 insertion 4
    Block { hi: 0x03, mid: 0x03, lo: 0x20, size: 0x06, ram: 0x0cc14 }, // :177
    Block { hi: 0x02, mid: 0x40, lo: 0x00, size: 0x15, ram: 0x0cc2e }, // :178 master EQ
];

/// origin: ram.h:182 — insertion n (0-3) block heads
pub const INS_BLOCK: [u32; 4] = [0x0cb7e, 0x0cbaa, 0x0cbd6, 0x0cc02];
/// origin: ram.h:184 — "applies to part" inside the block (XG 03 0n 0C)
pub const INS_PART: u32 = 0x0c;
/// origin: ram.h:187-189 — variation block (02 01 40-5B); +0x1a connect, +0x1b part
pub const VAR_BLOCK: u32 = 0x0cb02;
pub const VAR_CONNECT: u32 = 0x1a;
pub const VAR_PART: u32 = 0x1b;
/// origin: ram.h:190 — insertion params 1-10: 16-bit MSB-first at +0x18
pub const INS_WIDE: u32 = 0x18;
/// origin: ram.h:192 — variation params 1-10 (02 01 42-55): 16-bit at +0x02
pub const VAR_WIDE: u32 = 0x02;

/// origin: ram.h:195-225 `locate` — XG address -> work-RAM offset
/// (relative to 0x400000), None when the table has no such address
pub fn locate(addr: u32) -> Option<u32> {
    let hi = (addr >> 14) as u8; // :197
    let mid = ((addr >> 7) & 0x7f) as u8;
    let lo = (addr & 0x7f) as u8;
    if hi == 0x00 && mid == 0x00 && (lo as u32) < 7 {
        // :198-201
        return Some(SYSTEM + lo as u32);
    }
    if hi == 0x08 && (mid as i32) < 32 && (lo as u32) < PART_XG_SIZE {
        return Some(part_base(mid as i32) + lo as u32); // :202-205
    }
    if hi == 0x08
        && (mid as i32) < 32
        && (lo as u32) >= PART_EXT_XG
        && (lo as u32) < PART_EXT_XG + PART_EXT_SIZE
    {
        // :206-209
        return Some(part_base(mid as i32) + PART_EXT_RAM + (lo as u32 - PART_EXT_XG));
    }
    if hi == PART_HPF_HI && (mid as i32) < 32 && lo as u32 == PART_HPF_XG {
        return Some(part_base(mid as i32) + PART_HPF_RAM); // :210-213
    }
    if hi == 0x08
        && (mid as i32) < 32
        && (lo as u32) >= PART_EQ_XG
        && (lo as u32) < PART_EQ_XG + PART_EQ_SIZE
    {
        // :214-217
        return Some(part_base(mid as i32) + PART_EQ_RAM + (lo as u32 - PART_EQ_XG));
    }
    for b in EFFECTS.iter() {
        // :218-223 — `b.lo + b.size` compares widened ints on disk; same here
        if hi == b.hi && mid == b.mid && (lo as u32) >= b.lo as u32 && (lo as u32) < b.lo as u32 + b.size {
            return Some(b.ram + (lo as u32 - b.lo as u32));
        }
    }
    None // :224
}
