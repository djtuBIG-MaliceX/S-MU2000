//! MEG interpreter core (STATE + ADDRESSING + DECODE + IO) — mirror of
//! `src/mame/sound/swp30.cpp` `swp30_device::meg_state` (swp30.h:333-446).
//! Ledger row `MEG` phase A. Phase B (`step`/`run_program`/`build_ops`/
//! `flush_writes`, swp30.cpp:3609-4178) consumes the `decoded` program API
//! (`Decoded` + `decoded[0x180]` + `decode_program`) exposed here.
//!
//! SEAMS (device members the C++ meg_state methods touch; no bus invented):
//! - `swp.rand()` (swp30.h:73-78, used by call_rand :3516) -> `&mut u32`
//!   threaded through `call_rand` (same LCG as `voice::swp_rand`).
//! - `m_swp->m_sintab` (get_lfo :3474-3480) -> `sintab: &[u16]` param
//!   (machine supplies the ROM-loaded sintab; >= 0x8000 entries).
//! - `m_swp->m_meg_const_gen` (const_w :3354) -> `&mut u32` param.
//! - `m_reverb_ram` / `m_reverb_cache` / `m_revram_*` latches stay on
//!   `Swp30` (regs.rs); the pure codec halves live here as associated fns.
//! - `meg_jit_invalidate()` (revram_enable_w :2466) is inert in this port
//!   (the MEG JIT is never built — swp30.h:542-559 device members, no Rust
//!   counterpart); only the `m_meg_jit_wait = 1` latch (:2467) is mirrored.
//!
//! LFO OOB (locked decision): `meg_lfo_r/w<Sel>` (:3411-3424) index
//! `m_lfo[(chan*2+Sel)]` with chan up to 0x3f -> idx up to 0x7f vs an array
//! of 0x18 (C++ out-of-bounds = UB past the array). Disk logs `nolfo` on
//! writes (:3419). Probe evidence 2026-10-01 (`build/render.exe roms
//! dense -v` + `build/boot.exe roms -v`, stderr captured): `nolfo` count = 0
//! -> the firmware never writes there; reads have no probe but must be
//! deterministic. Rust therefore CLAMPS: read idx >= 0x18 yields 0, write is
//! dropped (documented deviation; revisit only if a render case diverges).

use crate::voice::swp_rand;

/// origin: swp30.h:341-350 `meg_state::decoded`. Definite assignment: every
/// field is written by `decode_program` (asel/rop/mmode are 2-bit masks,
/// session-J liveness proof — no Option/default-init question left).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub sm: u8,
    pub sr: u8,
    pub dm: u8,
    pub dr: u8,
    pub t: u8,
    pub mmode: u8,
    pub m1t: u8,
    pub asel: u8,
    pub rop: u8,
    pub shift: u8,
    pub clamp: u8,
    pub dm_src: u8,
    pub memop: u8,
    pub m1_expand: bool,
    pub m2_from_m: bool,
    pub dr_from_r: bool,
    pub no_noise: bool,
    pub memw: bool,
    pub index: bool,
    pub t_write: bool,
    pub t_from_p: bool,
    pub mem_use_index: bool,
    pub index2: bool,
    pub mem_use_index2: bool,
    pub mem_table: bool,
}

impl Decoded {
    /// zero = the `= {}` in-class default of `m_decoded` before the first
    /// decode pass (swp30.h:351).
    const ZERO: Decoded = Decoded {
        sm: 0, sr: 0, dm: 0, dr: 0, t: 0,
        mmode: 0, m1t: 0, asel: 0, rop: 0, shift: 0, clamp: 0,
        dm_src: 0, memop: 0,
        m1_expand: false, m2_from_m: false,
        dr_from_r: false, no_noise: false,
        memw: false, index: false, t_write: false, t_from_p: false,
        mem_use_index: false,
        index2: false, mem_use_index2: false,
        mem_table: false,
    };
}

/// origin: BIT(v,n)/BIT(v,n,len) — mamecompat.h:238-239.
#[inline]
fn bit(v: u64, lsb: u32, width: u32) -> u32 {
    if width == 0 {
        ((v >> lsb) & 1) as u32
    } else {
        ((v >> lsb) & ((1u64 << width) - 1)) as u32
    }
}

/// pub wrapper of the same BIT for regs.rs (revram_clear_w :2487-2488).
#[inline]
pub fn bit_of(v: u16, lsb: u32, width: u32) -> u32 {
    bit(v as u64, lsb, width)
}

/// origin: swp30.h:3332-3342 `lfo_increment_table` (constexpr lambda).
pub const LFO_INCREMENT_TABLE: [u32; 256] = {
    let dt: [u32; 8] = [0, 32, 64, 128, 256, 512, 1024, 2048];
    let sh: [u32; 8] = [0, 0, 1, 2, 3, 4, 5, 6];
    let mut inc = [0u32; 256];
    let mut i = 0usize;
    while i != 256 {
        let scale = ((i >> 5) & 7) as u32;
        inc[i] = (((i as u32) & 31) << sh[scale as usize]) + dt[scale as usize];
        i += 1;
    }
    inc
};

/// origin: swp30.h:333-446 `meg_state` (phase-A fields; every field an
/// explicit initializer per Invariant 3 — mirrors the in-class `= {}`s).
pub struct MegState {
    /// swp30.h:351 `m_decoded` (filled by decode_program only; NOT cleared by
    /// meg_state::reset on disk — kept zero at construction like `= {}`).
    pub decoded: [Decoded; 0x180],
    /// swp30.h:380 `m_program` (the program space `meg_prg_map_r` returns —
    /// no bus involved, swp30.cpp:2908-2911)
    pub program: [u64; 0x180],
    /// swp30.h:381 `m_const` (s16 on disk; const_w stores u16 bit-exactly)
    pub konst: [i16; 0x180],
    /// swp30.h:382 `m_offset`
    pub offset: [u16; 0x80],
    /// swp30.h:383-385
    pub lfo: [u16; 0x18],
    pub lfo_increment: [u32; 0x18],
    pub lfo_counter: [u32; 0x18],
    /// swp30.h:386 `m_map` (memory map; region_of/resolve_address/clear)
    pub map: [u16; 8],
    /// swp30.h:388-391
    pub m: [i32; 0x40],
    pub r: [i32; 0x80],
    pub t: [i16; 8],
    pub p: i64,
    /// swp30.h:393-405 write-aside / delay-line state (phase B consumes;
    /// part of STATE here)
    pub mw_value: [i32; 3],
    pub mw_reg: [u8; 3],
    pub rw_value: [i32; 3],
    pub rw_reg: [u8; 3],
    pub index_value: [i32; 3],
    pub index_active: [bool; 3],
    pub memw_value: [i32; 3],
    pub memr_value: [i32; 3],
    pub t_value: [i16; 2],
    pub memw_active: [bool; 3],
    pub memr_active: [bool; 3],
    pub delay_3: u32,
    pub delay_2: u32,
    /// swp30.h:407-413
    pub ram_read: u32,
    pub ram_write: u32,
    pub ram_index: i32,
    pub sample_counter: u32,
    pub program_address: u16,
    pub pc: u16,
    pub icount: i32,
    pub retval: u32,
}

impl MegState {
    /// Construction == `new meg_state` (all in-class `= {}`, swp30.h:351-413)
    /// — the ctor then calls `reset()` (swp30.cpp:1892), done by the owner.
    pub fn new() -> MegState {
        MegState {
            decoded: [Decoded::ZERO; 0x180],
            program: [0; 0x180],
            konst: [0; 0x180],
            offset: [0; 0x80],
            lfo: [0; 0x18],
            lfo_increment: [0; 0x18],
            lfo_counter: [0; 0x18],
            map: [0; 8],
            m: [0; 0x40],
            r: [0; 0x80],
            t: [0; 8],
            p: 0,
            mw_value: [0; 3],
            mw_reg: [0; 3],
            rw_value: [0; 3],
            rw_reg: [0; 3],
            index_value: [0; 3],
            index_active: [false; 3],
            memw_value: [0; 3],
            memr_value: [0; 3],
            t_value: [0; 2],
            memw_active: [false; 3],
            memr_active: [false; 3],
            delay_3: 0,
            delay_2: 0,
            ram_read: 0,
            ram_write: 0,
            ram_index: 0,
            sample_counter: 0,
            program_address: 0,
            pc: 0,
            icount: 0,
            retval: 0,
        }
    }

    /// origin: swp30.cpp:1924-1956 `meg_state::reset` — EXACT order.
    /// m_decoded / m_icount are NOT touched on disk (kept as-is).
    pub fn reset(&mut self) {
        self.program = [0; 0x180]; // :1926
        self.konst = [0; 0x180]; // :1927
        self.offset = [0; 0x80]; // :1928
        self.lfo = [0; 0x18]; // :1929
        self.lfo_increment = [0; 0x18]; // :1930
        self.lfo_counter = [0; 0x18]; // :1931
        self.map = [0; 8]; // :1932
        self.ram_read = 0; // :1933
        self.ram_write = 0; // :1934
        self.ram_index = 0; // :1935
        self.program_address = 0; // :1936
        self.pc = 0; // :1937
        self.m = [0; 0x40]; // :1938
        self.r = [0; 0x80]; // :1939
        self.t = [0; 8]; // :1940
        self.p = 0; // :1941
        self.mw_value = [0; 3]; // :1942
        self.mw_reg = [0; 3]; // :1943
        self.rw_value = [0; 3]; // :1944
        self.rw_reg = [0; 3]; // :1945
        self.index_value = [0; 3]; // :1946 (disk fills `false` — same bits)
        self.index_active = [false; 3]; // :1947
        self.memw_value = [0; 3]; // :1948 (disk fills `false`)
        self.memw_active = [false; 3]; // :1949
        self.memr_value = [0; 3]; // :1950 (disk fills `false`)
        self.memr_active = [false; 3]; // :1951
        self.delay_3 = 0; // :1952
        self.delay_2 = 0; // :1953
        self.sample_counter = 0; // :1954
        self.retval = 0; // :1955
    }

    /// origin: swp30.cpp:2262-2265 `meg_state::prg_address_r`
    pub fn prg_address_r(&self) -> u16 {
        self.program_address
    }

    /// origin: swp30.cpp:2267-2272 `prg_address_w` (>= 0x180 wraps to 0)
    pub fn prg_address_w(&mut self, data: u16) {
        self.program_address = data; // :2269
        if self.program_address >= 0x180 {
            self.program_address = 0; // :2270-2271
        }
    }

    /// origin: swp30.cpp:2274-2278 `prg_r<Sel>` (shift = 48-16*Sel)
    pub fn prg_r(&self, sel: usize) -> u16 {
        let shift = 48 - 16 * sel; // :2276
        (self.program[self.program_address as usize] >> shift) as u16 // :2277
    }

    /// origin: swp30.cpp:2280-2291 `prg_w<Sel>` (Sel==3 auto-increments,
    /// wrapping 0x180 -> 0 AFTER the write).
    pub fn prg_w(&mut self, sel: usize, data: u16) {
        let shift = 48 - 16 * sel; // :2282
        let mask = !(0xffffu64 << shift); // :2283
        let a = self.program_address as usize;
        self.program[a] = (self.program[a] & mask) | ((data as u64) << shift); // :2284
        if sel == 3 {
            // :2286-2290
            self.program_address += 1;
            if self.program_address == 0x180 {
                self.program_address = 0;
            }
        }
    }

    /// origin: swp30.cpp:2293-2296 `map_r<Sel>`
    pub fn map_r(&self, sel: usize) -> u16 {
        self.map[sel]
    }

    /// origin: swp30.cpp:2298-2301 `map_w<Sel>`
    pub fn map_w(&mut self, sel: usize, data: u16) {
        self.map[sel] = data;
    }

    /// origin: swp30.cpp:3346-3349 `const_r`
    pub fn const_r(&self, offset: usize) -> u16 {
        self.konst[offset] as u16
    }

    /// origin: swp30.cpp:3351-3356 `const_w`. Seam: `m_swp->m_meg_const_gen`
    /// (swp30.h:556, device field) passed explicitly; bumps ONLY on a value
    /// change (:3353-3354).
    pub fn const_w(&mut self, offset: usize, data: u16, const_gen: &mut u32) {
        if (self.konst[offset] as u16) != data {
            *const_gen += 1; // :3354
        }
        self.konst[offset] = data as i16; // :3355 (u16->s16, bit-exact in C++20)
    }

    /// origin: swp30.cpp:3358-3361 `offset_r`
    pub fn offset_r(&self, offset: usize) -> u16 {
        self.offset[offset]
    }

    /// origin: swp30.cpp:3363-3366 `offset_w`
    pub fn offset_w(&mut self, offset: usize, data: u16) {
        self.offset[offset] = data;
    }

    /// origin: swp30.cpp:3368-3371 `lfo_r` (idx < 0x18 callers only; the
    /// wrapper-level clamp policy is documented in the module header).
    pub fn lfo_r(&self, offset: usize) -> u16 {
        self.lfo[offset]
    }

    /// origin: swp30.cpp:3373-3380 `lfo_w` — written LFO restarts its phase
    /// (counter = 0 ONLY when the counter slot exists, :3377).
    pub fn lfo_w(&mut self, offset: usize, data: u16) {
        if offset < self.lfo_counter.len() {
            self.lfo_counter[offset] = 0; // :3377-3378
        }
        self.lfo[offset] = data; // :3379
    }

    /// origin: swp30.cpp:3382-3386 `lfo_commit_w`
    pub fn lfo_commit_w(&mut self) {
        for i in 0..24 {
            // :3384-3385
            self.lfo_increment[i] = LFO_INCREMENT_TABLE[(self.lfo[i] & 0xff) as usize];
        }
    }

    /// origin: swp30.cpp:3431-3435 `lfo_step` (per-sample advance)
    pub fn lfo_step(&mut self) {
        for i in 0..24 {
            self.lfo_counter[i] = (self.lfo_counter[i] + self.lfo_increment[i]) & 0x3f_ffff; // :3434
        }
    }

    /// origin: swp30.cpp:3437-3444 `region_of` — which of the 8 map regions
    /// a pc falls into. u16 arithmetic: `(pc/12)<<11` wraps at 16 bits,
    /// same as the C++ u16 assignment.
    pub fn region_of(&self, pc: u16) -> usize {
        let key = (((pc as u32 / 12) << 11) & 0xffff) as u16; // :3439
        for i in 0..8usize {
            // :3440-3442 (short-circuit protects map[i+1] at i==7)
            if i == 7 || self.map[i + 1] <= self.map[i] || ((self.map[i + 1] & 0xf800) > key) {
                return i;
            }
        }
        7 // :3443
    }

    /// origin: swp30.cpp:3446-3455 `resolve_address` — wrap offset into the
    /// pc's region (mask = 2^(10+map.bits8-10) - 1) and add map base << 10.
    pub fn resolve_address(&self, pc: u16, offset: i32) -> u32 {
        let key = (((pc as u32 / 12) << 11) & 0xffff) as u16; // :3448
        for i in 0..8usize {
            // :3449-3453
            if i == 7 || self.map[i + 1] <= self.map[i] || ((self.map[i + 1] & 0xf800) > key) {
                let mask = (1u32 << (10 + bit(self.map[i] as u64, 8, 3))) - 1; // :3451
                return ((offset as u32) & mask) + (bit(self.map[i] as u64, 0, 8) << 10); // :3452
            }
        }
        0xffff_ffff // :3454
    }

    /// origin: swp30.cpp:3457-3498 `get_lfo`. Seam: `m_swp->m_sintab`
    /// (:3474-3480) passed as `sintab` (>= 0x8000 entries).
    pub fn get_lfo(&self, lfo: usize, sintab: &[u16]) -> u32 {
        // :3459-3464
        const OFFSETS: [u32; 16] = [
            0x00000, 0x02aaa, 0x04000, 0x05555,
            0x08000, 0x0aaaa, 0x0c000, 0x0d555,
            0x10000, 0x12aaa, 0x14000, 0x15555,
            0x18000, 0x1aaaa, 0x1c000, 0x1d555,
        ];
        if lfo >= 0x18 {
            return 0; // wrapper policy — see module header (C++ UB, probe: never)
        }
        // :3466-3469 (Rust << on u32 discards high bits == C++ wrap; the
        // pre-mask values stay < 2^32 on every leg exactly as C++)
        let mut base = self.lfo_counter[lfo] >> 5;
        base = base << ((self.lfo[lfo] >> 8) & 3);
        base = base + OFFSETS[(self.lfo[lfo] >> 12) as usize];
        base &= 0x1_ffff;
        // :3471 — 2-bit select, all four arms present (definite assignment)
        let res: u32 = match (self.lfo[lfo] >> 10) & 3 {
            0 => {
                // sine :3472-3481
                if base < 0x8000 {
                    sintab[base as usize] as u32
                } else if base < 0x1_0000 {
                    sintab[((base & 0x7fff) ^ 0x7fff) as usize] as u32
                } else if base < 0x1_8000 {
                    (sintab[(base & 0x7fff) as usize] as u32) ^ 0xffff
                } else {
                    (sintab[((base & 0x7fff) ^ 0x7fff) as usize] as u32) ^ 0xffff
                }
            }
            1 => {
                // tri :3483-3487
                let mut r = (base + 0x8000) & 0x1_ffff;
                if r & 0x1_0000 != 0 {
                    r ^= 0x1_ffff;
                }
                r
            }
            2 => base >> 1,               // saw up :3489-3491
            _ => (base ^ 0x1_ffff) >> 1,  // saw down :3493-3495
        };
        res << 7 // :3497
    }

    /// origin: swp30.cpp:3502-3509 `m1_expand` (static; no device touch).
    /// s = v>>12 (>= 0 here since v < 0 returned early); max s = 7, so the
    /// left shift never reaches the sign bit (max 0x1fff << 2 = 0x7ffc).
    pub fn m1_expand(v: i16) -> i16 {
        if v < 0 {
            return 0; // :3504-3505
        }
        let s = (v >> 12) as u32; // :3506
        let x = (0x1000 | (v & 0xfff) as i16) as u32; // :3507
        if s == 5 {
            x as i16
        } else if s < 5 {
            (x >> (5 - s)) as i16
        } else {
            (x << (s - 5)) as i16
        }
    }

    /// origin: swp30.cpp:3513-3517 `call_rand`. Seam: the DEVICE LCG
    /// (swp30.h:73-78) via `seed` (same recurrence as `voice::swp_rand`).
    pub fn call_rand(&mut self, seed: &mut u32) {
        self.retval = swp_rand(seed); // :3516
    }

    /// origin: swp30.cpp:3519-3523 `call_revram_encode`
    pub fn call_revram_encode(&mut self) {
        self.retval = Self::revram_encode(self.retval) as u32; // :3522 (u16->u32 widen)
    }

    /// origin: swp30.cpp:3525-3529 `call_revram_decode`
    pub fn call_revram_decode(&mut self) {
        self.retval = Self::revram_decode(self.retval as u16);
    }

    /// origin: swp30.cpp:2428-2441 `revram_encode` (static). 27-bit value ->
    /// 16-bit float (e=exp-15 biased by finding the top bit at 0x400<<e).
    pub fn revram_encode(v: u32) -> u16 {
        let mut v = v & 0x7ff_ffff; // :2430
        let mut s = 0u32; // :2431
        if v & 0x400_0000 != 0 {
            // :2432-2435 (sign bit = bit 26: complement the 27-bit value)
            v ^= 0x7ff_ffff;
            s = 1;
        }
        let mut e = 15u32; // :2436
        while e != 0 && (v & (0x400 << e)) == 0 {
            e -= 1; // :2437-2438
        }
        let m = if e != 0 { (v >> (e - 1)) & 0x7ff } else { v }; // :2439
        ((e << 12) | (s << 11) | m) as u16 // :2440
    }

    /// origin: swp30.cpp:2443-2455 `revram_decode` (static). S-MU2000 fix
    /// :2449-2453 kept verbatim: e==0 negatives invert ALL 32 bits, not the
    /// upstream 0xffffffe0 partial mask.
    pub fn revram_decode(v: u16) -> u32 {
        let e = ((v >> 12) & 15) as u32; // :2445
        let s = ((v >> 11) & 1) as u32; // :2446
        let m = (v & 0x7ff) as u32; // :2447
        let mut vb = if e != 0 { (m | 0x800) << (e - 1) } else { m }; // :2448
        if s != 0 {
            // :2452-2453 (e-1 <= 14, shift stays in range both sides)
            vb ^= if e != 0 { (0xffff_ffffu32 << (e - 1)) & 0xffff_ffff } else { 0xffff_ffff };
        }
        vb
    }

    /// origin: swp30.cpp:3535-3568 `decode_program` — the ONLY writer of
    /// `decoded`; called by the run loop when the program changed (:4181-4184,
    /// phase B). Every `decoded` field assigned per pass (2-bit masks make
    /// asel/rop/mmode definite — session-J proof).
    pub fn decode_program(&mut self) {
        for pc in 0..0x180usize {
            let opcode = self.program[pc]; // :3538
            let d = &mut self.decoded[pc]; // :3539
            d.sm = bit(opcode, 0x04, 6) as u8; // :3540
            d.sr = bit(opcode, 0x0b, 7) as u8; // :3541
            d.dm = bit(opcode, 0x27, 6) as u8; // :3542
            d.dr = bit(opcode, 0x30, 7) as u8; // :3543
            d.t = bit(opcode, 0x38, 3) as u8; // :3544
            d.mmode = bit(opcode, 0x16, 2) as u8; // :3545
            d.m1t = bit(opcode, 0x14, 2) as u8; // :3546
            d.asel = bit(opcode, 0x18, 2) as u8; // :3547
            d.rop = bit(opcode, 0x1a, 2) as u8; // :3548
            d.shift = bit(opcode, 0x1c, 2) as u8; // :3549
            d.clamp = bit(opcode, 0x1e, 2) as u8; // :3550
            d.dm_src = bit(opcode, 0x2d, 3) as u8; // :3551
            d.memop = bit(opcode, 0x24, 2) as u8; // :3552
            d.m1_expand = bit(opcode, 0x13, 0) != 0; // :3553
            d.m2_from_m = bit(opcode, 0x12, 0) != 0; // :3554
            d.dr_from_r = bit(opcode, 0x37, 0) != 0; // :3555
            d.no_noise = bit(opcode, 0x0a, 0) != 0; // :3556
            // S-MU2000 :3557-3561 — idx(0x3e) & mw(0x3d) together mean a
            // SECOND index, not either operand:
            d.index = bit(opcode, 0x3e, 0) != 0 && bit(opcode, 0x3d, 0) == 0; // :3559
            d.index2 = bit(opcode, 0x3e, 0) != 0 && bit(opcode, 0x3d, 0) != 0; // :3560
            d.memw = bit(opcode, 0x3d, 0) != 0 && bit(opcode, 0x3e, 0) == 0; // :3561
            d.mem_use_index2 = bit(opcode, 0x22, 0) != 0; // :3562
            d.t_write = bit(opcode, 0x3b, 0) != 0; // :3563
            d.t_from_p = bit(opcode, 0x3c, 0) != 0; // :3564
            d.mem_use_index = bit(opcode, 0x21, 0) != 0; // :3565
            d.mem_table = bit(opcode, 0x23, 0) != 0; // :3566
        }
    }

    /// origin: swp30.cpp:3906-3927 `flush_writes` — drain the 3-deep write
    /// pipeline at a sample boundary (phase B wires this from the run loop;
    /// included here because `step` owns the pipeline it drains).
    pub fn flush_writes(&mut self, swp: &mut MegSwp) {
        for i in 0..3u32 {
            let k = (self.delay_3 + i) % 3; // :3909
            let k = k as usize;
            if self.mw_reg[k] != 0 {
                let r = self.mw_reg[k] as usize;
                self.m[r] = self.mw_value[k]; // :3911 (s32 = s32)
            }
            if self.rw_reg[k] != 0 {
                let r = self.rw_reg[k] as usize;
                self.r[r] = self.rw_value[k]; // :3912-3913
            }
            if self.index_active[k] {
                self.ram_index = self.index_value[k]; // :3914-3915
            }
            if swp.ix2_act[k] != 0 {
                *swp.ram_index2 = swp.ix2_value[k]; // :3916-3917
            }
            self.mw_reg[k] = 0; // :3918-3921
            self.rw_reg[k] = 0;
            self.index_active[k] = false;
            swp.ix2_act[k] = 0;
        }
    }
}

/// origin: swp30.cpp:3576-3584 `meg_pack24` (file static). C++ `/` truncates
/// toward zero == Rust i64 `/`. `s32(q)` truncate == `as i32`; the sext(s32,24)
/// fold keeps the ORIGINAL width (mamecompat.h:277-283 note): shl/shr 8 on i32.
#[inline]
pub fn meg_pack24(p: i64) -> u32 {
    let mut q = p / 32768; // :3580
    if q == 0x80_0000 {
        q = 0x7f_ffff; // :3581
    }
    if q == -0x80_0001 {
        q = -0x80_0000; // :3582
    }
    ((q as i32).wrapping_shl(8) >> 8) as u32 // :3583
}

/// origin: swp30.cpp:3599-3607 `meg_cond` (file static). Rules per :3594-3595.
#[inline]
pub fn meg_cond(cond: u8, n: bool, z: bool) -> bool {
    if bit(cond as u64, 3, 0) == 0 {
        return true; // :3601-3602
    }
    let mut c = if bit(cond as u64, 2, 0) != 0 { n } else { !n }; // :3603
    if bit(cond as u64, 1, 0) != 0 {
        c = c || z; // :3604-3605
    }
    c
}

/// s16(std::clamp<s64>(m_p >> (15+8), -0x8000, 0x7fff)) — the shared clamp of
/// :3645/:3671/:3833.
#[inline]
fn s16_p23_clamped(p: i64) -> i16 {
    (p >> 23).clamp(-0x8000, 0x7fff) as i16
}

/// Device-side MEG glue the C++ step()/flush_writes() reach through `m_swp->`:
/// swp30.h:534-540 (flag_n/flag_z, ix2_value/ix2_act, ram_index2, skip_to) +
/// swp30.h:453/:568 m_revram_enable + m_reverb_ram + swp30.h:73 rand() +
/// m_sintab (get_lfo). Rust passes the disjoint pieces explicitly so
/// `MegState` stays free of device back-pointers.
pub struct MegSwp<'a> {
    pub flag_n: &'a mut bool,
    pub flag_z: &'a mut bool,
    pub ix2_value: &'a mut [i32; 3],
    pub ix2_act: &'a mut [u8; 3],
    pub ram_index2: &'a mut i32,
    pub skip_to: &'a mut u16,
    pub revram_enable: u16,
    pub reverb_ram: &'a mut [u16; 0x40000],
    pub seed: &'a mut u32,
    pub sintab: &'a [u16],
}

/// origin: swp30.cpp:3609-3894 `meg_state::step`. One instruction.
///
/// QUIRKS (deliberate mirrors of the C++ build's behaviour where the standard
/// calls UB):
/// - `m1 << 23` / `m2 << 15` / `a << 15` / `r <<= {2,4}` / sext `r << 22`:
///   signed left-shift overflow is UB in C++; the GCC -O3 x86-64 binary wraps
///   like unsigned — Rust uses `wrapping_shl` (same bits). Cite :3707,:3713,
///   :3720-3721,:3743,:3748.
/// - `m1_expand(m1)` takes an s16; the C++ call silently TRUNCATES an oversized
///   s64 m1 to 16 bits first (:3697, header swp30.h:437) — mirrored via
///   `as i16`. Unreachable in practice (m_t/m_const are s16; m2_from_m never
///   routes to m1).
/// - The `m_dbg_meg` instruction trace (:3874-3879) is ported as inert
///   seam args (`dbg_meg == None` in every machine/harness run - no state
///   effect, no allocation when off; only the `--trace-meg` bin seam sets
///   it, mirroring the C++ `m_swp->m_dbg_meg` fprintf sink).
/// - Branch target math `(m_pc & ~0xff) | BIT(opc,0x10,8)` narrows to u16
///   exactly like the C++ `const u16 target` (:3659).
/// - resolve_address offset arg: the C++ expression promotes to `unsigned`
///   (s32 - u32) then converts back to the s32 param (wrap, gcc-defined) —
///   Rust mirrors with i32/u32 `wrapping_*` (:3842,:3863).
#[allow(clippy::too_many_arguments)]
pub fn meg_step(
    meg: &mut MegState,
    swp: &mut MegSwp,
    dbg_meg: &mut Option<std::fs::File>,
    dbg_pc0: u16,
    dbg_pc1: u16,
    dbg_from: u32,
    dbg_count: u32,
) {
    // :3613-3625 — register writes delayed by 3 cycles; reg 0 wired to 0.
    let d3 = meg.delay_3 as usize;
    let d2 = meg.delay_2 as usize;
    if meg.mw_reg[d3] != 0 {
        let r = meg.mw_reg[d3] as usize;
        meg.m[r] = meg.mw_value[d3]; // :3616
    }
    if meg.rw_reg[d3] != 0 {
        let r = meg.rw_reg[d3] as usize;
        meg.r[r] = meg.rw_value[d3]; // :3619
    }
    if meg.index_active[d3] {
        meg.ram_index = meg.index_value[d3]; // :3622-3623
    }
    if swp.ix2_act[d3] != 0 {
        *swp.ram_index2 = swp.ix2_value[d3]; // :3624-3625
    }

    // :3627-3635 — memory ports delayed by 2 cycles
    if meg.memw_active[d2] {
        meg.ram_write = meg.memw_value[d2] as u32; // :3629 (s32->u32 reinterpret)
        meg.memw_active[d2] = false;
    }
    if meg.memr_active[d2] {
        meg.ram_read = meg.memr_value[d2] as u32; // :3633
        meg.memr_active[d2] = false;
    }

    // :3637-3655 — skipped instructions inside a taken branch do nothing
    // (S-MU2000, doc/upstream.md 31)
    if *swp.skip_to != 0 {
        if meg.pc < *swp.skip_to {
            meg.mw_reg[d3] = 0; // :3640-3644
            meg.rw_reg[d3] = 0;
            meg.memw_active[d2] = false;
            meg.index_active[d3] = false;
            swp.ix2_act[d3] = 0;
            meg.t_value[d2] = s16_p23_clamped(meg.p); // :3645
            meg.delay_3 = if meg.delay_3 == 2 { 0 } else { meg.delay_3 + 1 }; // :3646
            meg.delay_2 ^= 1; // :3647
            meg.pc += 1; // :3648-3651
            meg.icount -= 1;
            if meg.pc == 0x180 {
                meg.pc = 0;
            }
            return;
        }
        *swp.skip_to = 0; // :3654
    }

    let pc = meg.pc as usize;

    // :3656-3679 — conditional jump (bit 0x3f), MAME had "no branches"
    if bit(meg.program[pc], 0x3f, 0) != 0 {
        let opc = meg.program[pc]; // :3657
        if meg_cond(bit(opc, 0x18, 8) as u8, *swp.flag_n, *swp.flag_z) {
            // :3659-3661 — target stays inside the same 256-block
            let target = (meg.pc & !0xff) | (bit(opc, 0x10, 8) as u16);
            if target > meg.pc {
                *swp.skip_to = target;
            }
        }
        // :3663-3665 — a branch still writes t (DYNA: magnitude -> envelope)
        let d = meg.decoded[pc];
        if d.t_write {
            meg.t[d.t as usize] = if d.t_from_p { meg.t_value[d2] } else { meg.konst[pc] };
        }
        meg.mw_reg[d3] = 0; // :3666-3670
        meg.rw_reg[d3] = 0;
        meg.memw_active[d2] = false;
        meg.index_active[d3] = false;
        swp.ix2_act[d3] = 0;
        meg.t_value[d2] = s16_p23_clamped(meg.p); // :3671
        meg.delay_3 = if meg.delay_3 == 2 { 0 } else { meg.delay_3 + 1 }; // :3672
        meg.delay_2 ^= 1; // :3673
        meg.pc += 1; // :3674-3677
        meg.icount -= 1;
        if meg.pc == 0x180 {
            meg.pc = 0;
        }
        return; // :3678
    }

    // :3681-3688 — pre-decoded form (decode_program)
    let d = meg.decoded[pc];
    let sm = d.sm as usize;
    let sr = d.sr as usize;
    let t = d.t as usize;
    let mmode = d.mmode as u32;

    // :3691-3769 — ALU; even mmode 0 runs adder/shift/sat on p (upstream 21/30)
    if mmode != 0 || d.shift != 0 || d.clamp != 0 || d.rop != 0 {
        let m1t = d.m1t as u32;
        let mut m1: i64 = if m1t == 1 {
            meg.t[t] as i64
        } else if m1t == 2 {
            // :3694-3695 — select 2: t if the last flag_n, else const (upstream 29)
            if *swp.flag_n {
                meg.t[t] as i64
            } else {
                meg.konst[pc] as i64
            }
        } else {
            meg.konst[pc] as i64
        };
        if d.m1_expand {
            m1 = MegState::m1_expand(m1 as i16) as i64; // :3696-3697 (truncation quirk)
        }
        let m2: i64 = if d.m2_from_m { meg.m[sm] as i64 } else { meg.r[sr] as i64 }; // :3699
        let m: i64 = match mmode {
            // :3702-3715 (2-bit mask -> total switch, session-J proof)
            1 => m1.wrapping_shl(8 + 15), // :3707
            2 => m1.wrapping_mul(m2),     // :3710
            3 => m2.wrapping_shl(15),     // :3713
            _ => 0,                       // :3703-3705
        };
        let a: i64 = match d.asel {
            // :3717-3723 (2-bit mask -> total switch)
            1 => {
                if sr != 0 {
                    (meg.r[sr] as i64).wrapping_shl(15)
                } else {
                    meg.p >> 15
                }
            }
            2 => {
                if sm != 0 {
                    (meg.m[sm] as i64).wrapping_shl(15)
                } else {
                    meg.p >> 15
                }
            }
            3 => 0,
            _ => meg.p,
        };
        let mut r: i64 = match d.rop {
            // :3725-3739 (2-bit mask -> total switch)
            0 => m.wrapping_add(a),                     // :3728
            1 => m.wrapping_sub(a),                     // :3731
            2 => m.wrapping_add(if a < 0 { a.wrapping_neg() } else { a }), // :3734
            _ => m & a,                                 // :3737
        };
        let shift = d.shift;
        if shift != 0 {
            r = r.wrapping_shl(if shift == 3 { 4 } else { shift as u32 }); // :3741-3743
        }
        // :3745-3748 — wrap at 42 bits (27.15) UNLESS saturating (upstream 20)
        if d.clamp == 0 {
            r = r.wrapping_shl(22) >> 22; // util::sext(r, 42), s64-fold
        }
        r = match d.clamp {
            // :3750-3762 (2-bit mask -> total switch). DISK DIGIT-COUNT quirk:
            // the literals are 10 hex digits (2^38 / 2^38-1), NOT the 42-bit
            // wrap width of sext(:3748) — saturating stops EARLIER than
            // wrapping (verified digit-by-digit on disk 2026-10-01; a
            // mis-grouped 0x3fff_ffff_ffff here was caught by meg_tests).
            1 => r.clamp(-0x40_0000_0000, 0x3f_ffff_ffff), // :3754
            2 => r.clamp(0, 0x3f_ffff_ffff),                // :3757
            3 => (if r < 0 { r.wrapping_neg() } else { r }).min(0x3f_ffff_ffff), // :3760
            _ => r,
        };
        meg.p = r; // :3764
        if bit(meg.program[pc], 0x20, 0) != 0 {
            *swp.flag_n = r < 0; // :3766-3767
            *swp.flag_z = r == 0;
        }
    }

    // :3771-3790 — delayed m-bank write source
    meg.mw_reg[d3] = d.dm;
    if d.dm != 0 {
        // C++ `u32 v;` + switch = definite over 3-bit dm_src; expression-form
        // match mirrors the dr leg below (no dummy-init warning).
        let v: u32 = match d.dm_src {
            0 | 1 | 2 | 3 => meg.get_lfo(pc >> 4, swp.sintab), // :3775-3777
            4 => meg.ram_read, // :3778
            5 => {
                let mut x = swp_rand(swp.seed) & 0xff_ffff; // :3779 (24-bit, sign-ext)
                if x & 0x0080_0000 != 0 {
                    x |= 0xff00_0000;
                }
                x
            }
            6 => {
                let mut p = meg.p; // :3781
                if !d.no_noise {
                    p = p.wrapping_add((swp_rand(swp.seed) & 0x07e0) as i64); // :3782-3783
                }
                meg_pack24(p) // :3784
            }
            _ => meg.m[sm] as u32, // :3787 (case 7; dm_src is 3-bit)
        };
        meg.mw_value[d3] = v as i32; // :3789 (u32->s32 reinterpret)
    }

    // :3792-3804 — delayed r-bank write source
    meg.rw_reg[d3] = d.dr;
    if d.dr != 0 {
        let v: i32 = if d.dr_from_r {
            meg.r[sr] // :3795-3796
        } else {
            let mut p = meg.p; // :3798
            if !d.no_noise {
                p = p.wrapping_add((swp_rand(swp.seed) & 0x07e0) as i64); // :3799-3800
            }
            meg_pack24(p) as i32 // :3801
        };
        meg.rw_value[d3] = v;
    }

    // :3806-3810 — memory write port latch
    if d.memw {
        meg.memw_active[d2] = true;
        meg.memw_value[d2] = (meg.p >> 15) as i32; // :3808 (s64->s32 truncate)
    } else {
        meg.memw_active[d2] = false;
    }

    // :3812-3819 — first index (meg_state) and second index (device, upstream 32)
    if d.index {
        meg.index_active[d3] = true;
        meg.index_value[d3] = (meg.p >> (15 + 8)) as i32; // :3814
    } else {
        meg.index_active[d3] = false;
    }
    swp.ix2_act[d3] = d.index2 as u8; // :3817
    if d.index2 {
        swp.ix2_value[d3] = (meg.p >> (15 + 8)) as i32; // :3818-3819
    }

    // :3821-3833 — t write + t delay line (index form taps p>>8 bit15 off;
    // 15-bit slice, other use per the :3829-3831 comment)
    if d.t_write {
        if d.t_from_p {
            meg.t[t] = meg.t_value[d2]; // :3824-3825
        } else {
            meg.t[t] = meg.konst[pc]; // :3826-3827
        }
    }
    meg.t_value[d2] = if d.index || d.index2 {
        ((meg.p >> 8) & 0x7fff) as i16 // :3832
    } else {
        s16_p23_clamped(meg.p) // :3833
    };

    // :3835-3871 — memory access (18-bit revram wrap; disabled region drops
    // writes / reads 0, upstream 33)
    let ix1 = if d.mem_use_index { meg.ram_index } else { 0 }; // :3842
    let ix2 = if d.mem_use_index2 { *swp.ram_index2 } else { 0 };
    match d.memop {
        1 => {
            if bit(swp.revram_enable as u64, meg.region_of(meg.pc) as u32, 0) != 0 {
                // :3840-3841 — region disabled: drop the write
            } else {
                let off = (meg.offset[pc / 3] as i32) // :3842
                    .wrapping_add(ix1)
                    .wrapping_add(ix2)
                    .wrapping_sub(meg.sample_counter as i32);
                let address = meg.resolve_address(meg.pc, off);
                if address != 0xffff_ffff {
                    // :3845 — plain array, 18-bit wrap (upstream)
                    swp.reverb_ram[(address & 0x3_ffff) as usize] =
                        MegState::revram_encode(meg.ram_write);
                }
            }
        }
        2 | 3 => {
            if d.mem_table {
                // :3849-3856 — absolute revram address (upstream 24): no map,
                // no sample-counter subtraction
                let address = (meg.offset[pc / 3] as u32)
                    .wrapping_add(ix1 as u32)
                    .wrapping_add(ix2 as u32)
                    .wrapping_add(if d.memop == 3 { 1 } else { 0 })
                    & 0x3_ffff; // :3852
                let val = swp.reverb_ram[address as usize];
                meg.memr_value[d2] = MegState::revram_decode(val) as i32; // :3853
                meg.memr_active[d2] = true;
            } else if bit(swp.revram_enable as u64, meg.region_of(meg.pc) as u32, 0) != 0 {
                // :3857-3862 — disabled region reads 0
                meg.memr_value[d2] = 0;
                meg.memr_active[d2] = true;
            } else {
                let off = (meg.offset[pc / 3] as i32) // :3863
                    .wrapping_add(ix1)
                    .wrapping_add(ix2)
                    .wrapping_sub(meg.sample_counter as i32)
                    .wrapping_add(if d.memop == 3 { 1 } else { 0 });
                let address = meg.resolve_address(meg.pc, off);
                if address != 0xffff_ffff {
                    let val = swp.reverb_ram[(address & 0x3_ffff) as usize]; // :3865
                    meg.memr_value[d2] = MegState::revram_decode(val) as i32; // :3866
                    meg.memr_active[d2] = true; // :3867
                }
            }
        }
        _ => {}
    }

    // :3873-3879 — m_dbg_meg instruction trace (inert unless --trace-meg)
    if let Some(f) = dbg_meg {
        if meg.pc >= dbg_pc0
            && meg.pc < dbg_pc1
            && meg.sample_counter >= dbg_from
            && meg.sample_counter < dbg_from.wrapping_add(dbg_count)
        {
            use std::io::Write;
            let _ = writeln!(
                f,
                "{} {:03x} p={} m{:02x}={} r{:02x}={} t{}={}",
                meg.sample_counter,
                meg.pc,
                meg.p,
                sm,
                meg.m[sm],
                sr,
                meg.r[sr],
                t,
                meg.t[t]
            );
        }
    }

    // :3881-3893 — pipeline + pc advance
    meg.delay_3 += 1;
    if meg.delay_3 == 3 {
        meg.delay_3 = 0;
    }
    meg.delay_2 += 1;
    if meg.delay_2 == 2 {
        meg.delay_2 = 0;
    }
    meg.pc += 1;
    meg.icount -= 1;
    if meg.pc == 0x180 {
        meg.pc = 0;
    }
}

/// origin: swp30.h:357-375 `meg_state::op` — pre-judged per-instruction form
/// consumed by `run_program`. NOT state (ledger: swp30.h:355-356 — lives on
/// the device, swp30.h:530; Rust mirror = `Swp30::meg_ops`). All u8/u16/u32
/// like the C++ (decoded bools widen to 0/1). Every field is assigned by
/// `build_ops` (region loop always terminates at i==7, :3968).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Op {
    pub alu: u8,
    pub mmode: u8,
    pub m1_from_t: u8,
    pub m1_expand: u8,
    pub m2_from_m: u8,
    /// 0 p / 1 r<<15 / 2 m<<15 / 3 p>>15 / 4 0 (swp30.h:361)
    pub asel: u8,
    pub rop: u8,
    pub shift: u8,
    pub clamp: u8,
    pub sm: u8,
    pub sr: u8,
    pub dm: u8,
    pub dr: u8,
    pub t: u8,
    pub dm_src: u8,
    pub no_noise: u8,
    pub dr_from_r: u8,
    pub memw: u8,
    pub index: u8,
    pub t_write: u8,
    pub t_from_p: u8,
    pub index2: u8,
    pub mem_use_index2: u8,
    pub memop: u8,
    pub mem_use_index: u8,
    pub mem_table: u8,
    pub lfo: u8,
    pub offset_index: u8,
    pub addr_mask: u32,
    pub addr_base: u32,
    pub region: u8,
    pub latch: u8,
    pub jump: u8,
    pub cond: u8,
    pub target: u16,
}

impl Op {
    /// the `o = op{};` zero-init of :3933 (every field is assigned below it
    /// in the same loop — this literal is the C++ `{}` verbatim mirror).
    pub const ZERO: Op = Op {
        alu: 0,
        mmode: 0,
        m1_from_t: 0,
        m1_expand: 0,
        m2_from_m: 0,
        asel: 0,
        rop: 0,
        shift: 0,
        clamp: 0,
        sm: 0,
        sr: 0,
        dm: 0,
        dr: 0,
        t: 0,
        dm_src: 0,
        no_noise: 0,
        dr_from_r: 0,
        memw: 0,
        index: 0,
        t_write: 0,
        t_from_p: 0,
        index2: 0,
        mem_use_index2: 0,
        memop: 0,
        mem_use_index: 0,
        mem_table: 0,
        lfo: 0,
        offset_index: 0,
        addr_mask: 0,
        addr_base: 0,
        region: 0,
        latch: 0,
        jump: 0,
        cond: 0,
        target: 0,
    };
}

/// origin: swp30.cpp:3928-3978 `build_ops` (const). Resolves everything that
/// depends only on the program + m_map (:3925-3927 comment).
pub fn build_ops(meg: &MegState, ops: &mut [Op; 0x180]) {
    for pc in 0..0x180usize {
        let d = meg.decoded[pc]; // :3931
        let mut o = Op::ZERO; // :3933 `o = op{};`
        // :3934 — mmode 0 still runs adder/shift/sat (upstream 21/30)
        o.alu = (d.mmode != 0 || d.shift != 0 || d.clamp != 0 || d.rop != 0) as u8;
        o.mmode = d.mmode;
        o.m1_from_t = if d.m1t == 1 {
            1
        } else if d.m1t == 2 {
            2 // select 2: t vs const on flag_n (upstream 29)
        } else {
            0
        };
        o.m1_expand = d.m1_expand as u8;
        o.m2_from_m = d.m2_from_m as u8;
        o.asel = match d.asel {
            // :3939-3944 (0..4 encoding: 3 == p>>15, 4 == zero)
            0 => 0,
            1 => {
                if d.sr != 0 {
                    1
                } else {
                    3
                }
            }
            2 => {
                if d.sm != 0 {
                    2
                } else {
                    3
                }
            }
            _ => 4,
        };
        o.rop = d.rop;
        o.shift = if d.shift == 3 { 4 } else { d.shift }; // :3946
        o.clamp = d.clamp;
        o.sm = d.sm;
        o.sr = d.sr;
        o.dm = d.dm;
        o.dr = d.dr;
        o.t = d.t; // :3948
        o.dm_src = d.dm_src;
        o.no_noise = d.no_noise as u8;
        o.dr_from_r = d.dr_from_r as u8;
        o.memw = d.memw as u8;
        o.index = d.index as u8;
        o.index2 = d.index2 as u8;
        o.mem_use_index2 = d.mem_use_index2 as u8;
        o.t_write = d.t_write as u8;
        o.t_from_p = d.t_from_p as u8;
        o.memop = d.memop;
        o.mem_table = d.mem_table as u8;
        o.mem_use_index = d.mem_use_index as u8;
        o.lfo = (pc >> 4) as u8; // :3961
        o.offset_index = (pc / 3) as u8; // :3962
        o.latch = bit(meg.program[pc], 0x20, 0) as u8; // :3963
        o.jump = bit(meg.program[pc], 0x3f, 0) as u8; // :3964
        o.cond = bit(meg.program[pc], 0x18, 8) as u8; // :3965
        o.target = (((pc & !0xff) | bit(meg.program[pc], 0x10, 8) as usize) & 0xffff) as u16; // :3966
                                                       // :3968-3976 — same pick as resolve_address();
                                                       // i==7 always stops, so "no region" is impossible
        let key = (((pc as u32 / 12) << 11) & 0xffff) as u16; // :3969 (u16 truncation)
        for i in 0..8usize {
            if i == 7 || meg.map[i + 1] <= meg.map[i] || ((meg.map[i + 1] & 0xf800) > key) {
                o.addr_mask = (1u32 << (10 + bit(meg.map[i] as u64, 8, 3))) - 1; // :3972
                o.addr_base = (bit(meg.map[i] as u64, 0, 8) << 10) as u32; // :3973
                o.region = i as u8; // :3974
                break;
            }
        }
        ops[pc] = o;
    }
}

/// origin: swp30.cpp:3982-4172 `run_program` — one sample (0x180
/// instructions) through the pre-judged `ops`. Same body as `meg_step`
/// (:4013 comment) with local p/d3/d2/flags; 384 is divisible by 3 and 2 so
/// the delay rings return to their starting slots (:3980-3981 comment).
///
/// QUIRKS mirrored from the C++ binary (same list as `meg_step`):
/// - signed left-shift overflow (`m1 << 23`, `m2 << 15`, `r <<= shift`,
///   sext `<<22 >>22`) wraps like GCC -O3 x86-64 — `wrapping_shl`.
/// - `r < 0 ? -r : r` uses `wrapping_neg` (:4060/:4072).
/// - `u32 off = u32(...) - sample_counter` and the address math are u32
///   wrapping (:4134/:4148/:4151) — `wrapping_*` + explicit masks.
/// - `skip_to` is LOCAL and starts 0 every sample (:3991) — a taken branch
///   whose target range is not reached before the sample ends does NOT
///   persist into the next sample (device `m_meg_skip_to` untouched; the
///   `step()`-path difference is intentional upstream behaviour).
/// - `m_mw_reg[d3] = o.dm` / `m_rw_reg[d3] = o.dr` are UNCONDITIONAL
///   assignments (:4081/:4102), like step's :3771/:3792.
pub fn run_program(meg: &mut MegState, swp: &mut MegSwp, ops: &[Op; 0x180]) {
    let mut d3 = meg.delay_3; // :3984
    let mut d2 = meg.delay_2;
    let mut p: i64 = meg.p; // :3985
    let sample_counter = meg.sample_counter; // :3986
    let mut flag_n = *swp.flag_n; // :3987
    let mut flag_z = *swp.flag_z;
    let mut skip_to: u32 = 0; // :3991 (device m_meg_skip_to NOT used)

    for pc in 0..0x180usize {
        let o = ops[pc]; // :3994
        let i3 = d3 as usize; // local ring indices (C++ `d3`/`d2`)
        let i2 = d2 as usize;

        // :3996-4011 — 3/2-cycle delayed register/memory drains (live banks)
        if meg.mw_reg[i3] != 0 {
            let r = meg.mw_reg[i3] as usize;
            meg.m[r] = meg.mw_value[i3]; // :3997
        }
        if meg.rw_reg[i3] != 0 {
            let r = meg.rw_reg[i3] as usize;
            meg.r[r] = meg.rw_value[i3]; // :3999
        }
        if meg.index_active[i3] {
            meg.ram_index = meg.index_value[i3]; // :4001
        }
        if swp.ix2_act[i3] != 0 {
            *swp.ram_index2 = swp.ix2_value[i3]; // :4002-4003
        }
        if meg.memw_active[i2] {
            meg.ram_write = meg.memw_value[i2] as u32; // :4005
            meg.memw_active[i2] = false; // :4006
        }
        if meg.memr_active[i2] {
            meg.ram_read = meg.memr_value[i2] as u32; // :4009
            meg.memr_active[i2] = false; // :4010
        }

        // :4013-4031 — branch (same as step): skipped instructions do nothing
        if skip_to != 0 && (pc as u32) >= skip_to {
            skip_to = 0; // :4014-4015
        }
        if skip_to != 0 || o.jump != 0 {
            if skip_to == 0 && meg_cond(o.cond, flag_n, flag_z) && (o.target as u32) > pc as u32 {
                skip_to = o.target as u32; // :4017-4018
            }
            if o.jump != 0 && o.t_write != 0 {
                // a taken/not-taken branch still writes t (:4019-4021)
                meg.t[o.t as usize] = if o.t_from_p != 0 {
                    meg.t_value[i2]
                } else {
                    meg.konst[pc]
                };
            }
            meg.mw_reg[i3] = 0; // :4022-4026
            meg.rw_reg[i3] = 0;
            meg.memw_active[i2] = false;
            meg.index_active[i3] = false;
            swp.ix2_act[i3] = 0;
            meg.t_value[i2] = s16_p23_clamped(p); // :4027
            d3 = if d3 == 2 { 0 } else { d3 + 1 }; // :4028
            d2 ^= 1; // :4029
            continue; // :4030
        }

        // :4033-4079 — ALU
        if o.alu != 0 {
            let mut m1: i64 = if o.m1_from_t == 1 {
                meg.t[o.t as usize] as i64
            } else if o.m1_from_t == 2 {
                if flag_n {
                    meg.t[o.t as usize] as i64
                } else {
                    meg.konst[pc] as i64
                }
            } else {
                meg.konst[pc] as i64
            };
            if o.m1_expand != 0 {
                m1 = MegState::m1_expand(m1 as i16) as i64; // :4035-4036 (s64->s16 trunc quirk)
            }
            let m2: i64 = if o.m2_from_m != 0 {
                meg.m[o.sm as usize] as i64
            } else {
                meg.r[o.sr as usize] as i64
            }; // :4037
            let m: i64 = match o.mmode {
                // :4040-4045
                0 => 0,
                1 => m1.wrapping_shl(8 + 15), // :4042
                2 => m1.wrapping_mul(m2),      // :4043
                _ => m2.wrapping_shl(15),      // :4044
            };
            let a: i64 = match o.asel {
                // :4048-4054 (5-way: build_ops folded sr/sm==0 into 3)
                0 => p,
                1 => (meg.r[o.sr as usize] as i64).wrapping_shl(15), // :4050
                2 => (meg.m[o.sm as usize] as i64).wrapping_shl(15), // :4051
                3 => p >> 15,                                         // :4052
                _ => 0,                                               // :4053
            };
            let mut r: i64 = match o.rop {
                // :4057-4062
                0 => m.wrapping_add(a),
                1 => m.wrapping_sub(a),
                2 => m.wrapping_add(if a < 0 { a.wrapping_neg() } else { a }),
                _ => m & a,
            };
            r = r.wrapping_shl(o.shift as u32); // :4064 (shift already folded 3->4)
            if o.clamp == 0 {
                r = r.wrapping_shl(22) >> 22; // :4065-4066 util::sext(r, 42)
            }
            r = match o.clamp {
                // :4068-4073 (same disk digit-count quirk as step :3750-3762:
                // saturating bounds are 2^38, NOT the 2^42 wrap width)
                0 => r,
                1 => r.clamp(-0x40_0000_0000, 0x3f_ffff_ffff),
                2 => r.clamp(0, 0x3f_ffff_ffff),
                _ => (if r < 0 { r.wrapping_neg() } else { r }).min(0x3f_ffff_ffff),
            };
            p = r; // :4074
            if o.latch != 0 {
                flag_n = r < 0; // :4075-4078
                flag_z = r == 0;
            }
        }

        // :4081-4100 — delayed m-bank write source
        meg.mw_reg[i3] = o.dm;
        if o.dm != 0 {
            let v: u32 = match o.dm_src {
                0..=3 => meg.get_lfo(o.lfo as usize, swp.sintab), // :4085-4087
                4 => meg.ram_read,                                 // :4088
                5 => {
                    let mut x = swp_rand(swp.seed) & 0xff_ffff; // :4089
                    if x & 0x0080_0000 != 0 {
                        x |= 0xff00_0000;
                    }
                    x
                }
                6 => {
                    let mut q = p; // :4091
                    if o.no_noise == 0 {
                        q = q.wrapping_add((swp_rand(swp.seed) & 0x07e0) as i64); // :4092-4093
                    }
                    meg_pack24(q) // :4094
                }
                _ => meg.m[o.sm as usize] as u32, // :4097
            };
            meg.mw_value[i3] = v as i32; // :4099 (u32->s32)
        }

        // :4102-4114 — delayed r-bank write source
        meg.rw_reg[i3] = o.dr;
        if o.dr != 0 {
            let v: u32 = if o.dr_from_r != 0 {
                meg.r[o.sr as usize] as u32 // :4106 (s32->u32)
            } else {
                let mut q = p; // :4108
                if o.no_noise == 0 {
                    q = q.wrapping_add((swp_rand(swp.seed) & 0x07e0) as i64); // :4109-4110
                }
                meg_pack24(q) // :4111
            };
            meg.rw_value[i3] = v as i32; // :4113
        }

        // :4116-4118 — memory write port latch
        meg.memw_active[i2] = o.memw != 0;
        if o.memw != 0 {
            meg.memw_value[i2] = (p >> 15) as i32; // :4118 (s64->s32 truncate)
        }

        // :4120-4125 — first index + second index
        meg.index_active[i3] = o.index != 0;
        if o.index != 0 {
            meg.index_value[i3] = (p >> (15 + 8)) as i32; // :4122
        }
        swp.ix2_act[i3] = o.index2; // :4123
        if o.index2 != 0 {
            swp.ix2_value[i3] = (p >> (15 + 8)) as i32; // :4124-4125
        }

        // :4127-4130 — t write + t delay line
        if o.t_write != 0 {
            meg.t[o.t as usize] = if o.t_from_p != 0 { meg.t_value[i2] } else { meg.konst[pc] };
        }
        meg.t_value[i2] = if o.index != 0 || o.index2 != 0 {
            ((p >> 8) & 0x7fff) as i16 // :4129
        } else {
            s16_p23_clamped(p) // :4130
        };

        // :4132-4158 — memory access (goto mem_done == if/else-if chains here)
        if o.memop >= 2 && o.mem_table != 0 {
            // :4132-4138 — absolute revram read (upstream 24): no map, no
            // sample-counter subtraction, u32 wrap then & 0x3ffff
            let address = (meg.offset[o.offset_index as usize] as u32)
                .wrapping_add(if o.mem_use_index != 0 { meg.ram_index as u32 } else { 0 })
                .wrapping_add(if o.mem_use_index2 != 0 {
                    *swp.ram_index2 as u32
                } else {
                    0
                })
                .wrapping_add(if o.memop == 3 { 1 } else { 0 })
                & 0x3_ffff; // :4134
            meg.memr_value[i2] = MegState::revram_decode(swp.reverb_ram[address as usize]) as i32; // :4135
            meg.memr_active[i2] = true; // :4136
        } else if o.memop != 0 {
            // :4139-4158 — region-disabled drops writes / reads 0 (:4140-4147)
            if bit(swp.revram_enable as u64, o.region as u32, 0) != 0 {
                if o.memop != 1 {
                    meg.memr_value[i2] = 0; // :4143
                    meg.memr_active[i2] = true; // :4144
                }
            } else {
                let mut off = (meg.offset[o.offset_index as usize] as u32)
                    .wrapping_add(if o.mem_use_index != 0 {
                        meg.ram_index as u32
                    } else {
                        0
                    })
                    .wrapping_add(if o.mem_use_index2 != 0 {
                        *swp.ram_index2 as u32
                    } else {
                        0
                    })
                    .wrapping_sub(sample_counter); // :4148
                if o.memop == 3 {
                    off = off.wrapping_add(1); // :4149-4150
                }
                let address = ((off & o.addr_mask).wrapping_add(o.addr_base)) & 0x3_ffff; // :4151
                if o.memop == 1 {
                    swp.reverb_ram[address as usize] = MegState::revram_encode(meg.ram_write); // :4153
                } else {
                    meg.memr_value[i2] =
                        MegState::revram_decode(swp.reverb_ram[address as usize]) as i32; // :4155
                    meg.memr_active[i2] = true; // :4156
                }
            }
        }

        d3 = if d3 == 2 { 0 } else { d3 + 1 }; // :4161
        d2 ^= 1; // :4162
    }

    // :4165-4171 — write back the locals; 384 % 3 == 384 % 2 == 0 so the
    // rings land back on their starting slots; pc restarts at 0
    meg.p = p;
    meg.delay_3 = d3;
    meg.delay_2 = d2;
    *swp.flag_n = flag_n;
    *swp.flag_z = flag_z;
    meg.pc = 0;
    meg.icount -= 0x180;
}
