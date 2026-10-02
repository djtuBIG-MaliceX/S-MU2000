//! Voice engine — transliteration of the AWM2 voice path of
//! `src/mame/sound/swp30.cpp`:
//! phase A: `envelope_block`, `lfo_block`, device helpers `sounding_voices`
//! (:1469), `envelope_step` (:1548), `volume_apply` (:1814) — :1441-1513,
//! :1548-1653, :1697-1833.
//! phase B: `filter_block` (:1069-1275 incl. its own `volume_apply`),
//! `iir1_block` (:1300-1365), device helpers `filter_impulse` (:1516) and
//! `lfo_pitch_trace` (:1532), `peg_step` (:2578), `awm2_step` (:1833) and the
//! per-channel `Voice`/`Channels` assembly (swp30.h:189-332 + :465-468).
//!
//! Ground truth: `tests/data/voice_vectors.txt`, produced by a compiled-C++
//! harness (%TEMP%\voicegtA\gt.cpp) that byte-extracts the same line ranges
//! — every table in `voice_tests.rs` is asserted tuple-exact.
//!
//! Device seams (C++ `swp30_device &swp` parameters): disk check shows the
//! ported LFO methods touch ONLY `swp.rand()` (swp30.cpp:1715, :1721, :1738).
//! No `m_sintab`, no sample counter. In Rust the rand LCG seed is threaded
//! explicitly as `&mut u32` (`swp_rand` below), mirroring
//! `smu_compat::timers` (mamecompat.h:640-645, seed init swp30.h:451).

use crate::fetch::{StreamingBlock, Wave};

/// origin: swp30.h:262-267 envelope_block enum {ATTACK,DECAY1,DECAY2,RELEASE}
pub const EG_ATTACK: u8 = 0;
pub const EG_DECAY1: u8 = 1;
pub const EG_DECAY2: u8 = 2;
pub const EG_RELEASE: u8 = 3;

/// origin: swp30.h:451 m_rand_seed / m_rand_seed_base init
pub const RAND_SEED_INIT: u32 = 0x9d14abd7;

/// origin: swp30.h:73-78 `swp30_device::rand` — the LFO noise seam.
/// Identical recurrence to `smu_compat::timers::Machine::rand`
/// (mamecompat.h:640-645): LCG then rotate 16 (low bits are short-period).
pub fn swp_rand(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); // :75
    (*seed >> 16) | (*seed << 16) // :77
}

/// origin: swp30.h:260-297 `swp30_device::envelope_block`
#[derive(Clone, Debug)]
pub struct EnvelopeBlock {
    pub m_attack: u16,        // :269 (in-class init 0)
    pub m_decay1: u16,        // :270
    pub m_decay2: u16,        // :271
    pub m_release_glo: u16,   // :272
    pub m_envelope_level: i32, // :273
    pub m_envelope_mode: u8,  // :274
}

impl Default for EnvelopeBlock {
    /// construction == C++ in-class initializers swp30.h:269-274 (all 0);
    /// NOT the same as `clear()` (invariant 3: explicit, ordered fields).
    fn default() -> Self {
        Self::new()
    }
}

impl EnvelopeBlock {
    /// mirrors swp30.h:269-274 in-class initializers (all zero)
    pub fn new() -> Self {
        Self {
            m_attack: 0,
            m_decay1: 0,
            m_decay2: 0,
            m_release_glo: 0,
            m_envelope_level: 0,
            m_envelope_mode: 0,
        }
    }

    /// origin: swp30.cpp:1441-1449 `clear` — exact assignment order
    pub fn clear(&mut self) {
        self.m_attack = 0;            // :1443
        self.m_decay1 = 0;            // :1444
        self.m_decay2 = 0;            // :1445
        self.m_release_glo = 0;       // :1446
        self.m_envelope_level = 0x3fff; // :1447
        self.m_envelope_mode = EG_RELEASE; // :1448
    }

    /// origin: swp30.cpp:1451-1457 `keyon`
    pub fn keyon(&mut self) {
        self.m_envelope_level = ((self.m_attack & 0xff) as i32) << 6; // :1453
        if (self.m_attack & 0xff) == 0 {
            // quirk: attack level 0 keys on at 0x2000 instead (:1454-1455)
            self.m_envelope_level = 0x80 << 6; // :1455
        }
        self.m_envelope_mode = EG_ATTACK; // :1456
    }

    /// origin: swp30.cpp:1459-1462 `status`
    pub fn status(&self) -> u16 {
        // C++ (m_envelope_mode << 14) | m_envelope_level in int, narrowed to
        // u16 on return — compute in u32 and truncate the same way
        (((self.m_envelope_mode as u32) << 14) | (self.m_envelope_level as u32)) as u16 // :1461
    }

    /// origin: swp30.cpp:1464-1467 `active`
    pub fn active(&self) -> bool {
        self.m_envelope_level != 0x3fff || self.m_envelope_mode != EG_RELEASE // :1466
    }

    /// origin: swp30.cpp:1477-1513 `level_step` (static).
    /// Domain note (:1484-1486 comment): negative speeds only occur via the
    /// pitch EG and only down to -16 (k0 = 10, sh = 10); the vectors stay in
    /// that domain. C++ `>>` on signed is arithmetic == Rust `>>` on i32.
    pub fn level_step(level: i32, sample_counter: u32) -> u16 {
        if level >= 0x78 {
            return 0x7f; // :1481-1482
        }

        let k0 = level >> 3;              // :1487 (arithmetic)
        let k1 = (level as u32) & 7;      // :1488 (two's-complement low bits)

        if level >= 0x48 {
            // :1490-1496 — k0 is liveness-bounded 0..=5 here (level 0x48..=0x77)
            let k0 = k0 - 9; // :1491
            let a = (4u32 << k0) - 1; // :1492
            let b = (2u32 << k0) - 1; // :1493
            const MX: [u8; 8] = [0x00, 0x20, 0x44, 0xa2, 0x55, 0x75, 0xee, 0xfe]; // :1494
            if ((MX[k1 as usize] as u32 >> (sample_counter & 7)) & 1) != 0 {
                a as u16 // :1495
            } else {
                b as u16
            }
        } else if level >= 0x40 {
            if sample_counter & 1 != 0 {
                return 1; // :1499-1500
            }
            let s1 = (sample_counter & 0xe) >> 1; // :1501
            const MX: [u8; 8] = [0x00, 0x01, 0x22, 0xa8, 0x55, 0xab, 0x77, 0xfd]; // :1502
            ((MX[k1 as usize] as u32 >> s1) & 1) as u16 // :1503
        } else {
            let sh = (8 - k0) as u32; // :1506 ("larger than 8 for negative level")
            // :1508 util::make_bitmask<u32>(sh) = (1 << sh) - 1 (mamecompat.h:272-274)
            if sample_counter & (((1u32 << sh) - 1)) != 0 {
                return 0; // :1509
            }
            const MX: [u16; 8] = [0x5555, 0x5557, 0x5757, 0x5777, 0x7777, 0x777f, 0x7f7f, 0x7fff]; // :1511
            ((MX[k1 as usize] >> ((sample_counter >> sh) & 0xf)) & 1) as u16 // :1512
        }
    }

    /// origin: swp30.cpp:1553-1602 `step`
    pub fn step(&mut self, sample_counter: u32) -> u16 {
        // :1555 — level BEFORE the mode switch, no clamp (< 0x8000 always)
        let mut result = (self.m_envelope_level + (((self.m_release_glo & 0xff) as i32) << 6)) as u16;
        match self.m_envelope_mode {
            EG_ATTACK => {
                // :1558
                let mut level = self.m_envelope_level
                    - Self::level_step(
                        (self.m_attack >> 8) as i32 + ((self.m_envelope_level >> 9) << 2),
                        sample_counter,
                    ) as i32; // :1558 (C++ u16 return promotes to int)
                if level <= 0 {
                    // :1559-1562
                    level = 0;
                    self.m_envelope_mode = EG_DECAY1;
                }
                self.m_envelope_level = level; // :1563
                if (self.m_attack & 0xff) == 0 {
                    // quirk: level-0 attack outputs the release floor instead (:1564-1565)
                    result = (((self.m_release_glo & 0xff) as i32) << 6) as u16;
                }
            }

            EG_DECAY1 | EG_DECAY2 => {
                // :1570
                let reg = if self.m_envelope_mode == EG_DECAY1 {
                    self.m_decay1
                } else {
                    self.m_decay2
                };
                let limit = ((reg & 0xff) as i32) << 6; // :1571
                let mut level = self.m_envelope_level; // :1572
                if level < limit {
                    level += Self::level_step((reg >> 8) as i32, sample_counter) as i32; // :1574
                    if level > limit {
                        level = limit; // :1575-1576
                    }
                } else if level > limit {
                    level -= Self::level_step((reg >> 8) as i32, sample_counter) as i32; // :1578
                    if level < limit {
                        level = limit; // :1579-1580
                    }
                }
                self.m_envelope_level = level; // :1582
                if level == limit {
                    // :1583-1589
                    if self.m_envelope_mode == EG_DECAY1 {
                        self.m_envelope_mode = EG_DECAY2; // :1584-1585
                    } else if self.m_release_glo & 0x8000 != 0 {
                        // quirk: bit 15 is the "must release" mark (S-MU2000, :1587)
                        self.m_envelope_mode = EG_RELEASE; // :1588
                    }
                }
            }

            EG_RELEASE => {
                // :1594 — (u16 >> 8) ^ 0x80 in int, passed as signed speed
                let mut level = self.m_envelope_level
                    + Self::level_step(((self.m_release_glo >> 8) as i32) ^ 0x80, sample_counter) as i32;
                if level > 0x3fff {
                    level = 0x3fff; // :1595-1596
                }
                self.m_envelope_level = level; // :1597
            }

            // no default in the C++ switch (:1556) — unreachable modes return
            // the pre-switch result unchanged
            _ => {}
        }
        result // :1601
    }

    /// origin: swp30.cpp:1604-1607 `attack_r`
    pub fn attack_r(&self) -> u16 {
        self.m_attack // :1606
    }

    /// origin: swp30.cpp:1609-1612 `attack_w`
    pub fn attack_w(&mut self, data: u16) {
        self.m_attack = data; // :1611
    }

    /// origin: swp30.cpp:1614-1617 `decay1_r`
    pub fn decay1_r(&self) -> u16 {
        self.m_decay1 // :1616
    }

    /// origin: swp30.cpp:1619-1622 `decay1_w`
    pub fn decay1_w(&mut self, data: u16) {
        self.m_decay1 = data; // :1621
    }

    /// origin: swp30.cpp:1624-1627 `decay2_r`
    pub fn decay2_r(&self) -> u16 {
        self.m_decay2 // :1626
    }

    /// origin: swp30.cpp:1629-1632 `decay2_w`
    pub fn decay2_w(&mut self, data: u16) {
        self.m_decay2 = data; // :1631
    }

    /// origin: swp30.cpp:1634-1637 `release_glo_r`
    pub fn release_glo_r(&self) -> u16 {
        self.m_release_glo // :1636
    }

    /// origin: swp30.cpp:1639-1647 `release_glo_w`
    pub fn release_glo_w(&mut self, data: u16) {
        self.m_release_glo = data; // :1641
        // quirk (locked by vectors): bit 15 forces RELEASE immediately, even
        // mid-DECAY1 — firmware-only semantics, :1642-1646 comment (upstream.md 16)
        if data & 0x8000 != 0 {
            self.m_envelope_mode = EG_RELEASE; // :1646
        }
    }

    /// origin: swp30.cpp:1649-1653 `trigger_release`
    pub fn trigger_release(&mut self) {
        self.m_release_glo |= 0xff00; // :1651
        self.m_envelope_mode = EG_RELEASE; // :1652
    }
}

/// origin: swp30.cpp:1469-1475 `swp30_device::sounding_voices`.
/// Device seam: iterates the device voice array `m_envelope` — passed in as
/// an explicit slice (callers own the voice array).
pub fn sounding_voices(env: &[EnvelopeBlock]) -> i32 {
    let mut n = 0i32; // :1471
    for e in env {
        n += e.active() as i32; // :1473 (int += bool)
    }
    n // :1474
}

/// origin: swp30.cpp:1548-1551 `swp30_device::envelope_step` (screen hook)
pub fn envelope_step(speed: i32, sample_counter: u32) -> u16 {
    EnvelopeBlock::level_step(speed, sample_counter) // :1550
}

/// origin: swp30.h:299-326 `swp30_device::lfo_block`
#[derive(Clone, Debug)]
pub struct LfoBlock {
    pub m_counter: u32,             // :300
    pub m_state: u16,               // :301
    pub m_r_type_step_pitch: u16,   // :303
    pub m_r_amplitude: u16,         // :304
    pub m_type: u8,                 // :306
    pub m_step: u8,                 // :307
    pub m_amplitude: u8,            // :308
    pub m_pitch_mode: bool,         // :309
    pub m_pitch_depth: i8,          // :310
}

impl Default for LfoBlock {
    /// construction == C++ in-class initializers swp30.h:300-310
    fn default() -> Self {
        Self::new()
    }
}

impl LfoBlock {
    /// mirrors swp30.h:300-310 in-class initializers
    pub fn new() -> Self {
        Self {
            m_counter: 0,
            m_state: 0,
            m_r_type_step_pitch: 0,
            m_r_amplitude: 0,
            m_type: 0,
            m_step: 0,
            m_amplitude: 0,
            m_pitch_mode: false,
            m_pitch_depth: 0,
        }
    }

    /// origin: swp30.cpp:1697-1708 `clear` — exact assignment order
    pub fn clear(&mut self) {
        self.m_counter = 0;        // :1699
        self.m_state = 0;          // :1700
        self.m_type = 0;           // :1701
        self.m_step = 0;           // :1702
        self.m_amplitude = 0;      // :1703
        self.m_pitch_mode = false; // :1704
        self.m_pitch_depth = 0;    // :1705
        self.m_r_type_step_pitch = 0; // :1706
        self.m_r_amplitude = 0;    // :1707
    }

    /// origin: swp30.cpp:1710-1723 `keyon`.
    /// Seam: C++ `swp30_device &swp` supplies ONLY `swp.rand()` (:1715, :1721).
    /// Quirk locked by vectors: exactly one rand is consumed on EVERY keyon
    /// (real-hardware behavior, :1712-1714 comment), two on type 3.
    pub fn keyon(&mut self, rand_seed: &mut u32) {
        swp_rand(rand_seed); // :1715
        self.m_counter = 0; // :1716
        match self.m_type {
            0 => self.m_state = (self.m_counter >> 6) as u16, // :1718
            1 => self.m_state = Self::tri_state(self.m_counter) as u16, // :1719
            2 => self.m_state = if self.m_counter & 0x20000 != 0 { 0xfff } else { 0 }, // :1720
            3 => self.m_state = (swp_rand(rand_seed) & 0xfff) as u16, // :1721
            // no default in the C++ switch (:1717); m_type is 2 bits (:1787)
            _ => {}
        }
    }

    /// origin: swp30.cpp:1727-1731 `tri_state` (static).
    /// Quirk: +0x10000 phase shift — hardware starts the triangle rising at
    /// centre 0x800, not from the bottom (:1725-1726 comment).
    pub fn tri_state(counter: u32) -> u32 {
        let c = counter.wrapping_add(0x10000) & 0x3ffff; // :1729
        if c & 0x20000 != 0 {
            (!c >> 5) & 0xffe // :1730 (~u32, logical shift, == C++ unsigned)
        } else {
            (c >> 5) & 0xffe
        }
    }

    /// origin: swp30.cpp:1733-1739 `step`.
    /// Seam: `swp.rand()` (:1738) via `rand_seed`.
    pub fn step(&mut self, rand_seed: &mut u32) {
        let pc = self.advance(); // :1736
        if self.m_type == 3 && ((pc ^ self.m_counter) & 0x3fe00) != 0 {
            // :1737 — hold-and-refresh when bits 9-17 change
            self.m_state = (swp_rand(rand_seed) & 0xfff) as u16; // :1738
        }
    }

    /// origin: swp30.cpp:1741-1753 `advance` (no rand — screen-traceable half).
    /// Note: the 0x40 kick (:1746) is left UNMASKED like C++; it can never
    /// overflow 18 bits because (c & 0x3fc0)==0x2000 requires bits 6..12
    /// clear, so c <= 0x3e03f.
    pub fn advance(&mut self) -> u32 {
        let pc = self.m_counter; // :1743
        self.m_counter = (self.m_counter + self.m_step as u32) & 0x3ffff; // :1744
        if self.m_counter & 0x03fc0 == 0x02000 {
            self.m_counter += 0x40; // :1745-1746
        }
        match self.m_type {
            0 => self.m_state = (self.m_counter >> 6) as u16, // :1748
            1 => self.m_state = Self::tri_state(self.m_counter) as u16, // :1749
            2 => self.m_state = if self.m_counter & 0x20000 != 0 { 0xfff } else { 0 }, // :1750
            _ => {} // type 3 keeps state until step() refreshes it (:1747)
        }
        pc // :1752
    }

    /// origin: swp30.cpp:1755-1769 `get_amplitude`.
    /// Quirk locked by vectors: type 1 (triangle) derives the amplitude from
    /// the RAW counter (:1763-1767), a different phase than the pitch side's
    /// centred `tri_state` (:1757-1761 comment).
    pub fn get_amplitude(&self) -> u16 {
        if self.m_type == 1 {
            let c = self.m_counter; // :1764
            let st = if c & 0x20000 != 0 { (!c >> 5) & 0xffe } else { (c >> 5) & 0xffe }; // :1765
            ((st * self.m_amplitude as u32) >> 5) as u16 // :1766
        } else {
            ((self.m_state as u32 * self.m_amplitude as u32) >> 5) as u16 // :1768
        }
    }

    /// origin: swp30.cpp:1771-1782 `get_pitch`.
    /// Quirk: centred on 0x800 (not 0x400 — hardware measurement, :1773-1775).
    /// Signed `>>` on i32 is arithmetic in both languages.
    pub fn get_pitch(&self) -> i16 {
        let v = (self.m_state as i32 - 0x800) * self.m_pitch_depth as i32; // :1776
        if self.m_pitch_mode {
            (v >> 9) as i16 // :1778 (coarse)
        } else {
            (v >> 12) as i16 // :1780 (fine)
        }
    }

    /// origin: swp30.cpp:1784-1793 `type_step_pitch_w`
    pub fn type_step_pitch_w(&mut self, data: u16) {
        self.m_r_type_step_pitch = data; // :1786
        self.m_type = (data >> 14) as u8; // :1787
        // 6-bit step: bit 13 = fast-vibrato rate bit (:1788-1789 comment)
        self.m_step = ((data >> 8) & 0x3f) as u8; // :1790
        self.m_pitch_mode = data & 0x80 != 0; // :1791
        self.m_pitch_depth = (data & 0x7f) as i8; // :1792 (always 0..0x7f, fits i8)
    }

    /// origin: swp30.cpp:1795-1799 `amplitude_w`
    pub fn amplitude_w(&mut self, data: u16) {
        self.m_r_amplitude = data; // :1797
        self.m_amplitude = (data & 0x7f) as u8; // :1798
    }

    /// origin: swp30.cpp:1801-1804 `type_step_pitch_r`
    pub fn type_step_pitch_r(&self) -> u16 {
        self.m_r_type_step_pitch // :1803
    }

    /// origin: swp30.cpp:1806-1809 `amplitude_r`
    pub fn amplitude_r(&self) -> u16 {
        self.m_r_amplitude // :1808
    }
}

/// origin: swp30.cpp:1815-1831 `swp30_device::volume_apply` (upstream delta
/// bdabf16 — trunc removal). 4.10 attenuation times 16.6 sample, NO 256-grid
/// truncation anymore (:1827-1829: float-recorded HW decay noise scales with
/// the part volume and never floors to 0 — upstream.md 34, discussion #69).
/// No floats anywhere — integer only, so no FMA/contraction exposure
/// (Invariant 9: N/A in this region).
/// C++ `>> e` is UB for negative level (e < 0); firmware EG clamps level to
/// 0..=0x3fff — the vectors cover 0..0x7fff; debug builds assert the domain.
pub fn volume_apply(level: i32, sample: i32) -> i32 {
    if level >= 0x3fff {
        return 0; // :1821-1822 ("passed-in value may have overflowed")
    }
    debug_assert!(level >= 0, "volume_apply: negative level is outside the C++ domain (:1826)");

    let e = level >> 10; // :1824
    let m = level & 0x3ff; // :1825
    // :1826 int arithmetic in C++ too (fits i32), widened to s64 on assign
    let mul: i64 = ((0x4000000 - (m << 15)) >> e) as i64;
    ((sample as i64 * mul) >> 26) as i32 // :1830 (s64 product, arithmetic shift, no trunc)
}

// ===========================================================================
// Phase B — util::sext seam, filter_block, iir1_block, device helpers,
// peg + awm2_step and the per-channel Voice/Channels assembly.
// Ground truth: tests/data/voice2_filter.txt / voice2_tables.txt /
// voice2_awm2.txt (compiled-C++ harness %TEMP%\voicegtB, byte-extract).
// ===========================================================================

/// `util::sext(v, 14)` on an already-14-bit-masked value (mamecompat.h:277-282:
/// `(v << (32-14)) >> (32-14)` on the u32→s32 bit pattern). Every call site
/// here passes `x & 0x3fff`; the mask is folded in to mirror the C++ call.
#[inline(always)]
pub fn sext14(v: u32) -> i32 {
    (((v & 0x3fff) << 18) as u32 as i32) >> 18
}

/// origin: swp30.h:189-239 `swp30_device::filter_block`.
/// Field order mirrors the header (:190-218, all in-class init 0).
/// `f1_chamberlin_step` (swp30.h:224) is a DEAD declaration — 0 definitions
/// in swp30.cpp (disk rg; the Chamberlin step is inline in `step` :1136-1139),
/// so there is nothing to port for it.
#[derive(Clone, Debug)]
pub struct FilterBlock {
    pub m_filter_1_a: u16,    // :190
    pub m_level_1: u16,       // :191
    pub m_filter_2_a: u16,    // :192
    pub m_level_2: u16,       // :193
    pub m_filter_b: u16,      // :194
    pub m_filter_1_p1: i32,   // :196
    pub m_filter_2_p1: i32,   // :197
    pub m_filter_p2: i32,     // :198
    pub m_filter_1_x1: i32,   // :200
    pub m_filter_1_x2: i32,   // :201
    pub m_filter_1_y0: i32,   // :202
    pub m_filter_1_y1: i32,   // :203
    pub m_filter_1_h: i32,    // :205
    pub m_filter_1_b: i32,    // :206
    pub m_filter_1_l: i32,    // :207
    pub m_filter_1_n: i32,    // :208
    pub m_filter_2_x1: i32,   // :210
    pub m_filter_2_x2: i32,   // :211
    pub m_filter_2_y0: i32,   // :212
    pub m_filter_2_y1: i32,   // :213
    pub m_filter_2_h: i32,    // :215
    pub m_filter_2_b: i32,    // :216
    pub m_filter_2_l: i32,    // :217
    pub m_filter_2_n: i32,    // :218
}

impl Default for FilterBlock {
    /// construction == in-class initializers swp30.h:190-218 (all 0)
    fn default() -> Self {
        Self::new()
    }
}

impl FilterBlock {
    /// all-zero construction (swp30.h:190-218 in-class initializers)
    pub fn new() -> Self {
        Self {
            m_filter_1_a: 0, m_level_1: 0, m_filter_2_a: 0, m_level_2: 0, m_filter_b: 0,
            m_filter_1_p1: 0, m_filter_2_p1: 0, m_filter_p2: 0,
            m_filter_1_x1: 0, m_filter_1_x2: 0, m_filter_1_y0: 0, m_filter_1_y1: 0,
            m_filter_1_h: 0, m_filter_1_b: 0, m_filter_1_l: 0, m_filter_1_n: 0,
            m_filter_2_x1: 0, m_filter_2_x2: 0, m_filter_2_y0: 0, m_filter_2_y1: 0,
            m_filter_2_h: 0, m_filter_2_b: 0, m_filter_2_l: 0, m_filter_2_n: 0,
        }
    }

    /// origin: swp30.cpp:1069-1100 `clear` — exact assignment order
    pub fn clear(&mut self) {
        self.m_filter_1_a = 0;   // :1071
        self.m_level_1 = 0;      // :1072
        self.m_filter_2_a = 0;   // :1073
        self.m_level_2 = 0;      // :1074
        self.m_filter_b = 0;     // :1075
        self.m_filter_1_p1 = 0;  // :1077
        self.m_filter_2_p1 = 0;  // :1078
        self.m_filter_p2 = 0;    // :1079
        self.m_filter_1_y0 = 0;  // :1081
        self.m_filter_1_y1 = 0;  // :1082
        self.m_filter_1_x1 = 0;  // :1083
        self.m_filter_1_x2 = 0;  // :1084
        self.m_filter_1_h = 0;   // :1086
        self.m_filter_1_b = 0;   // :1087
        self.m_filter_1_l = 0;   // :1088
        self.m_filter_1_n = 0;   // :1089
        self.m_filter_2_y0 = 0;  // :1091
        self.m_filter_2_y1 = 0;  // :1092
        self.m_filter_2_x1 = 0;  // :1093
        self.m_filter_2_x2 = 0;  // :1094
        self.m_filter_2_h = 0;   // :1096
        self.m_filter_2_b = 0;   // :1097
        self.m_filter_2_l = 0;   // :1098
        self.m_filter_2_n = 0;   // :1099
    }

    /// origin: swp30.cpp:1102-1123 `keyon` — state history only, registers
    /// survive (exact order :1104-1122)
    pub fn keyon(&mut self) {
        self.m_filter_1_y0 = 0;
        self.m_filter_1_y1 = 0;
        self.m_filter_1_x1 = 0;
        self.m_filter_1_x2 = 0;
        self.m_filter_1_h = 0;
        self.m_filter_1_b = 0;
        self.m_filter_1_l = 0;
        self.m_filter_1_n = 0;
        self.m_filter_2_y0 = 0;
        self.m_filter_2_y1 = 0;
        self.m_filter_2_x1 = 0;
        self.m_filter_2_x2 = 0;
        self.m_filter_2_h = 0;
        self.m_filter_2_b = 0;
        self.m_filter_2_l = 0;
        self.m_filter_2_n = 0;
    }

    /// origin: swp30.cpp:1263-1275 `filter_block::volume_apply` (static).
    /// 4.4 attenuation times 16.6; `level` is the TRUNCATED low byte of the
    /// u16 register (C++ implicit u16→u8 param narrowing, :1199). 0xff is a
    /// hardcoded mute (:1269-1270). `>>` on the wrapped negative int is
    /// arithmetic in both languages; every int op wraps like x86 GCC.
    pub fn volume_apply(level: u8, sample: i32) -> i32 {
        if level == 0xff {
            return 0; // :1269-1270
        }
        let e = level >> 4;    // :1272 (0..=15)
        let m = (level & 0xf) as i32; // :1273
        (sample.wrapping_shl(5)).wrapping_sub(sample.wrapping_mul(m)) >> (e + 5) // :1274
    }

    /// origin: swp30.cpp:1125-1205 `step`.
    /// 64-bit intermediates exactly as on disk (`s64(...)` products); each
    /// assignment truncates the s64 result to s32 the way the C++ does.
    /// Resonance `b` only feeds back when the STAGE's own kind bit 12 is set
    /// (S-MU2000 fix, disk comment :1128-1131 / upstream.md 6).
    pub fn step(&mut self, input: i16) -> i32 {
        let mut y0: i32 = 0; // :1127
        // :1132-1133 BIT(x,12) = (x>>12)&1 (mamecompat.h:238)
        let p2_1: i32 = if (self.m_filter_1_a >> 12) & 1 != 0 { self.m_filter_p2 } else { 0x80 };
        let p2_2: i32 = if (self.m_filter_2_a >> 12) & 1 != 0 { self.m_filter_p2 } else { 0x80 };
        if self.m_filter_1_a & 0x7fff != 0 {
            if (self.m_filter_1_a >> 13) & 1 == 0 {
                // Chamberlin chain, stage 1 (:1136-1139). Left-associative
                // C++: `(input<<6) - l` happens in int BEFORE the s64 term.
                let a = (input as i32).wrapping_shl(6).wrapping_sub(self.m_filter_1_l);
                self.m_filter_1_h =
                    (a as i64 - ((p2_1 as i64 * self.m_filter_1_b as i64) >> 7)) as i32; // :1136
                self.m_filter_1_b = (self.m_filter_1_b as i64
                    + ((self.m_filter_1_p1 as i64 * self.m_filter_1_h as i64) >> 16))
                    as i32; // :1137
                self.m_filter_1_n = self.m_filter_1_h.wrapping_add(self.m_filter_1_l); // :1138
                self.m_filter_1_l = (self.m_filter_1_l as i64
                    + ((self.m_filter_1_p1 as i64 * self.m_filter_1_b as i64) >> 16))
                    as i32; // :1139
                match self.m_filter_1_a >> 14 {
                    // :1141-1146 (u16>>14 is 0..=3; no default on disk)
                    0x0 => y0 = self.m_filter_1_l,
                    0x1 => y0 = self.m_filter_1_b,
                    0x2 => y0 = self.m_filter_1_h,
                    0x3 => y0 = self.m_filter_1_n,
                    _ => {}
                }
            } else {
                // direct-form stage 1, 8 kinds (:1148-1157). Kinds not listed
                // (0x0,1,4,5,8,9,c,d) leave y0 = 0 exactly like the C++ switch.
                let p1 = self.m_filter_1_p1 as i64;
                let y0_ = self.m_filter_1_y0;
                let y1_ = self.m_filter_1_y1;
                let x1_ = self.m_filter_1_x1;
                let x2_ = self.m_filter_1_x2;
                let e7 = (p2_1 as i64 * (y1_.wrapping_sub(y0_)) as i64) >> 7; // :1150 tail
                let two_y0_y1 = y0_.wrapping_shl(1).wrapping_sub(y1_); // `2*y0 - y1` in int
                let in6 = (input as i32).wrapping_shl(6);
                let x1_6 = x1_.wrapping_shl(6);
                // `input - 2*x1 + x2` in int (:1153)
                let d2x = (input as i32)
                    .wrapping_sub(x1_.wrapping_shl(1))
                    .wrapping_add(x2_);
                y0 = match self.m_filter_1_a >> 12 {
                    // :1149
                    0x2 => (y0_ as i64 + ((p1 * (in6.wrapping_sub(y0_)) as i64) >> 16)) as i32,
                    // :1150
                    0x3 => (two_y0_y1 as i64
                        + ((p1 * (in6.wrapping_sub(y0_)) as i64) >> 16)
                        + e7) as i32,
                    // :1151
                    0x6 => ((input as i32).wrapping_sub(x1_).wrapping_shl(6).wrapping_add(y0_) as i64
                        + ((p1 * (0i32.wrapping_sub(y0_)) as i64) >> 16)) as i32,
                    // :1152
                    0x7 => ((input as i32).wrapping_sub(x1_).wrapping_shl(6).wrapping_add(two_y0_y1)
                        as i64
                        + ((p1 * (0i32.wrapping_sub(y0_)) as i64) >> 16)
                        + e7) as i32,
                    // :1153
                    0xa => (d2x.wrapping_shl(6).wrapping_add(y0_) as i64
                        + ((p1 * (0i32.wrapping_sub(y0_)) as i64) >> 16)) as i32,
                    // :1154
                    0xb => (d2x.wrapping_shl(6).wrapping_add(two_y0_y1) as i64
                        + ((p1 * (0i32.wrapping_sub(y0_)) as i64) >> 16)
                        + e7) as i32,
                    // :1155
                    0xe => (d2x.wrapping_shl(6).wrapping_add(y0_) as i64
                        + ((p1 * (x1_6.wrapping_sub(y0_)) as i64) >> 16)) as i32,
                    // :1156
                    0xf => (d2x.wrapping_shl(6).wrapping_add(two_y0_y1) as i64
                        + ((p1 * (x1_6.wrapping_sub(y0_)) as i64) >> 16)
                        + e7) as i32,
                    _ => y0,
                };
                self.m_filter_1_x2 = self.m_filter_1_x1; // :1159-1162
                self.m_filter_1_x1 = input as i32;
                self.m_filter_1_y1 = self.m_filter_1_y0;
                self.m_filter_1_y0 = y0;
            }

            if self.m_filter_2_a & 0x7fff != 0 {
                if (self.m_filter_2_a >> 13) & 1 == 0 {
                    // Chamberlin chain, stage 2 (:1167-1170) — input is y0
                    let a = y0.wrapping_sub(self.m_filter_2_l);
                    self.m_filter_2_h =
                        (a as i64 - ((p2_2 as i64 * self.m_filter_2_b as i64) >> 7)) as i32;
                    self.m_filter_2_b = (self.m_filter_2_b as i64
                        + ((self.m_filter_2_p1 as i64 * self.m_filter_2_h as i64) >> 16))
                        as i32;
                    self.m_filter_2_n = self.m_filter_2_h.wrapping_add(self.m_filter_2_l);
                    self.m_filter_2_l = (self.m_filter_2_l as i64
                        + ((self.m_filter_2_p1 as i64 * self.m_filter_2_b as i64) >> 16))
                        as i32;
                    match self.m_filter_2_a >> 14 {
                        // :1172-1177
                        0x0 => y0 = self.m_filter_2_l,
                        0x1 => y0 = self.m_filter_2_b,
                        0x2 => y0 = self.m_filter_2_h,
                        0x3 => y0 = self.m_filter_2_n,
                        _ => {}
                    }
                } else {
                    // direct-form stage 2 (:1180-1189). NOTE (:1183-1188): the
                    // difference terms are NOT <<6-shifted here, unlike stage 1.
                    let y0_1 = y0; // :1179
                    let p1 = self.m_filter_2_p1 as i64;
                    let y0_ = self.m_filter_2_y0;
                    let y1_ = self.m_filter_2_y1;
                    let x1_ = self.m_filter_2_x1;
                    let x2_ = self.m_filter_2_x2;
                    let e7 = (p2_2 as i64 * (y1_.wrapping_sub(y0_)) as i64) >> 7;
                    let two_y0_y1 = y0_.wrapping_shl(1).wrapping_sub(y1_);
                    let x1_6 = x1_.wrapping_shl(6);
                    let d2x = y0_1 // `y0 - 2*x1 + x2` in int (:1186)
                        .wrapping_sub(x1_.wrapping_shl(1))
                        .wrapping_add(x2_);
                    y0 = match self.m_filter_2_a >> 12 {
                        // :1181
                        0x2 => (y0_ as i64 + ((p1 * (y0_1.wrapping_sub(y0_)) as i64) >> 16)) as i32,
                        // :1182
                        0x3 => (two_y0_y1 as i64
                            + ((p1 * (y0_1.wrapping_sub(y0_)) as i64) >> 16)
                            + e7) as i32,
                        // :1183
                        0x6 => (y0_1.wrapping_sub(x1_).wrapping_add(y0_) as i64
                            + ((p1 * (0i32.wrapping_sub(y0_)) as i64) >> 16)) as i32,
                        // :1184
                        0x7 => (y0_1.wrapping_sub(x1_).wrapping_add(two_y0_y1) as i64
                            + ((p1 * (0i32.wrapping_sub(y0_)) as i64) >> 16)
                            + e7) as i32,
                        // :1185
                        0xa => (d2x.wrapping_add(y0_) as i64
                            + ((p1 * (0i32.wrapping_sub(y0_)) as i64) >> 16)) as i32,
                        // :1186
                        0xb => (d2x.wrapping_add(two_y0_y1) as i64
                            + ((p1 * (0i32.wrapping_sub(y0_)) as i64) >> 16)
                            + e7) as i32,
                        // :1187
                        0xe => (d2x.wrapping_add(y0_) as i64
                            + ((p1 * (x1_6.wrapping_sub(y0_)) as i64) >> 16)) as i32,
                        // :1188
                        0xf => (d2x.wrapping_add(two_y0_y1) as i64
                            + ((p1 * (x1_6.wrapping_sub(y0_)) as i64) >> 16)
                            + e7) as i32,
                        _ => y0,
                    };
                    self.m_filter_2_x2 = self.m_filter_2_x1; // :1191-1194
                    self.m_filter_2_x1 = y0_1;
                    self.m_filter_2_y1 = self.m_filter_2_y0;
                    self.m_filter_2_y0 = y0;
                }
            }
        }

        // :1199 — m_level_* u16 narrows to the u8 param (high byte discarded)
        let mut result = FilterBlock::volume_apply(self.m_level_1 as u8, (input as i32).wrapping_shl(6))
            .wrapping_add(FilterBlock::volume_apply(self.m_level_2 as u8, y0));
        if result < -0x400000 {
            // :1200-1203
            result = -0x400000;
        } else if result > 0x3fffff {
            result = 0x3fffff;
        }
        result // :1204
    }

    /// origin: swp30.cpp:1207-1230 read methods
    pub fn filter_1_a_r(&self) -> u16 { self.m_filter_1_a } // :1209
    pub fn level_1_r(&self) -> u16 { self.m_level_1 } // :1214
    pub fn filter_2_a_r(&self) -> u16 { self.m_filter_2_a } // :1219
    pub fn level_2_r(&self) -> u16 { self.m_level_2 } // :1224
    pub fn filter_b_r(&self) -> u16 { self.m_filter_b } // :1229

    /// origin: swp30.cpp:1232-1236 `filter_1_a_w` — p1 coefficient decode
    pub fn filter_1_a_w(&mut self, data: u16) {
        self.m_filter_1_a = data; // :1234
        self.m_filter_1_p1 =
            (((0x101 + (data & 0xff)) as u32) << ((data >> 8) & 7)) as i32; // :1235
    }

    /// origin: swp30.cpp:1238-1241 `level_1_w`
    pub fn level_1_w(&mut self, data: u16) {
        self.m_level_1 = data; // :1240
    }

    /// origin: swp30.cpp:1243-1247 `filter_2_a_w`
    pub fn filter_2_a_w(&mut self, data: u16) {
        self.m_filter_2_a = data; // :1245
        self.m_filter_2_p1 =
            (((0x101 + (data & 0xff)) as u32) << ((data >> 8) & 7)) as i32; // :1246
    }

    /// origin: swp30.cpp:1249-1252 `level_2_w`
    pub fn level_2_w(&mut self, data: u16) {
        self.m_level_2 = data; // :1251
    }

    /// origin: swp30.cpp:1254-1260 `filter_b_w` — resonance decode. Quirk
    /// locked by vectors: p2 is computed from b REGARDLESS of stage kind;
    /// `step` (:1132-1133) decides per stage whether to use it (:1257 comment).
    pub fn filter_b_w(&mut self, data: u16) {
        self.m_filter_b = data; // :1256
        let p2 = (data >> 11) + 4; // :1258 (u16 promotes to int; 4..=11)
        self.m_filter_p2 = (((0x10 - (p2 & 7)) as u32) << (4 - (p2 >> 3))) as i32; // :1259
    }
}

/// origin: swp30.h:241-258 `swp30_device::iir1_block`. Two cascaded 3-point
/// FIRs in 3.13 coefficients (block doc :1280-1298). The swp30.h:242-243
/// comment says the header inits survive reset — DISK CORRECTION: `clear()`
/// (:1300-1313) zeroes everything INCLUDING m_a/m_b, and swp30_device::reset
/// calls it (:1977-1978); the header comment is stale, disk behavior rules.
#[derive(Clone, Debug)]
pub struct Iir1Block {
    pub m_a: [[i16; 2]; 2], // :244 (in-class {})
    pub m_b: [i16; 2],      // :245
    pub m_hx: [i32; 2],     // :246
    pub m_hy: [i32; 2],     // :246
}

impl Default for Iir1Block {
    fn default() -> Self {
        Self::new()
    }
}

impl Iir1Block {
    /// zero construction (swp30.h:244-246 `{}` initializers)
    pub fn new() -> Self {
        Self { m_a: [[0; 2]; 2], m_b: [0; 2], m_hx: [0; 2], m_hy: [0; 2] }
    }

    /// origin: swp30.cpp:1300-1313 `clear` — exact assignment order
    pub fn clear(&mut self) {
        self.m_a[0][0] = 0; // :1302
        self.m_a[0][1] = 0; // :1303
        self.m_b[0] = 0; // :1304
        self.m_a[1][0] = 0; // :1305
        self.m_a[1][1] = 0; // :1306
        self.m_b[1] = 0; // :1307
        self.m_hx[0] = 0; // :1309
        self.m_hy[0] = 0; // :1310
        self.m_hx[1] = 0; // :1311
        self.m_hy[1] = 0; // :1312
    }

    /// origin: swp30.cpp:1315-1321 `keyon`
    pub fn keyon(&mut self) {
        self.m_hx[0] = 0;
        self.m_hy[0] = 0;
        self.m_hx[1] = 0;
        self.m_hy[1] = 0;
    }

    /// origin: swp30.cpp:1323-1335 `step`.
    /// `std::clamp<s32>` (:1325-1326) NARROWS the s64 shifted sum to s32
    /// FIRST (implementation-defined = modulo on GCC), then clamps — the
    /// `as i32` below reproduces that; clamping the raw i64 would differ.
    pub fn step(&mut self, input: i32) -> i32 {
        let ya = {
            let s = (self.m_a[0][0] as i64)
                .wrapping_mul(input as i64)
                .wrapping_add((self.m_a[0][1] as i64).wrapping_mul(self.m_hx[0] as i64))
                .wrapping_add((self.m_b[0] as i64).wrapping_mul(self.m_hy[0] as i64))
                >> 13; // :1325
            (s as i32).clamp(-0x800000, 0x7fffff) // :1325 std::clamp<s32>
        };
        let yb = {
            let s = (self.m_a[1][0] as i64)
                .wrapping_mul(ya as i64)
                .wrapping_add((self.m_a[1][1] as i64).wrapping_mul(self.m_hx[1] as i64))
                .wrapping_add((self.m_b[1] as i64).wrapping_mul(self.m_hy[1] as i64))
                >> 13; // :1326
            (s as i32).clamp(-0x800000, 0x7fffff)
        };
        self.m_hx[0] = input; // :1328-1332
        self.m_hy[0] = ya;
        self.m_hx[1] = ya;
        self.m_hy[1] = yb;
        yb // :1334
    }

    /// origin: swp30.cpp:1337-1350 read methods (template<u32 Filter>)
    pub fn a0_r(&self, filter: usize) -> u16 {
        self.m_a[filter][0] as u16 // :1339
    }
    pub fn a1_r(&self, filter: usize) -> u16 {
        self.m_a[filter][1] as u16 // :1344
    }
    pub fn b1_r(&self, filter: usize) -> u16 {
        self.m_b[filter] as u16 // :1349
    }

    /// origin: swp30.cpp:1352-1365 write methods
    pub fn a0_w(&mut self, filter: usize, data: u16) {
        self.m_a[filter][0] = data as i16; // :1354
    }
    pub fn a1_w(&mut self, filter: usize, data: u16) {
        self.m_a[filter][1] = data as i16; // :1359
    }
    pub fn b1_w(&mut self, filter: usize, data: u16) {
        self.m_b[filter] = data as i16; // :1364
    }
}

/// origin: swp30.cpp:1516-1528 `swp30_device::filter_impulse` (static,
/// swp30.h:36). Screen-trace seam — no device state. FLOAT AUDIT
/// (Invariant 9): plain f32 ops, no FMA; the divisor 0x400000 is an exact
/// power of two, so every quotient is a single correctly-rounded division —
/// vectors compare BIT PATTERNS (`to_bits()`), not epsilon.
pub fn filter_impulse(f1a: u16, level1: u16, f2a: u16, level2: u16, fb: u16, out: &mut [f32]) {
    let mut f = FilterBlock::new(); // :1518
    f.filter_1_a_w(f1a); // :1519
    f.level_1_w(level1); // :1520
    f.filter_2_a_w(f2a); // :1521
    f.level_2_w(level2); // :1522
    f.filter_b_w(fb); // :1523
    f.keyon(); // :1524
    const ONE: i16 = 0x4000; // :1525
    for (i, o) in out.iter_mut().enumerate() {
        // :1527 float(step) / float(s32(ONE) << 6); 0x4000<<6 == 1<<20 (exact
        // power-of-two divide, no rounding). NO mul_add, NO epsilon (Invariant 9).
        *o = f.step(if i == 0 { ONE } else { 0 }) as f32 / ((i32::from(ONE) << 6) as f32);
    }
}

/// origin: swp30.cpp:1532-1545 `swp30_device::lfo_pitch_trace` (static,
/// swp30.h:39). Screen seam: NO keyon (no rand consumed — :1534-1535 goes
/// straight from the register write to get_pitch/advance); type 3 returns
/// all-zeros (:1536-1540, "the noise type isn't drawn").
pub fn lfo_pitch_trace(type_step_pitch: u16, out: &mut [i16]) {
    let mut l = LfoBlock::new(); // :1534
    l.type_step_pitch_w(type_step_pitch); // :1535
    if l.m_type == 3 {
        for o in out.iter_mut() {
            *o = 0; // :1538
        }
        return; // :1539
    }
    for o in out.iter_mut() {
        *o = l.get_pitch(); // :1542
        l.advance(); // :1543
    }
}

/// One AWM2 voice: the per-channel block set that `swp30_device` keeps as
/// parallel arrays (m_streaming/m_filter/m_iir1/m_envelope/m_lfo +
/// m_pitch_offset/m_peg_rate/m_peg_cur/m_peg_reached, swp30.h:460-468) plus
/// the four pitch-EG scalars (:465-468). Invariant 3: every field explicit.
pub struct Voice {
    pub streaming: StreamingBlock, // swp30.h m_streaming[chan]
    pub filter: FilterBlock, // m_filter[chan]
    pub iir1: Iir1Block, // m_iir1[chan]
    pub envelope: EnvelopeBlock, // m_envelope[chan]
    pub lfo: LfoBlock, // m_lfo[chan]
    pub pitch_offset: u16, // swp30.h:465 m_pitch_offset[chan]
    pub peg_rate: u16, // swp30.h:466 m_peg_rate[chan]
    pub peg_cur: i32, // swp30.h:467 m_peg_cur[chan]
    pub peg_reached: u8, // swp30.h:468 m_peg_reached[chan]
}

impl Voice {
    /// construction == the in-class defaults of the five blocks + zero
    /// scalars (the device ctor then runs reset(), swp30.cpp:1909 — which
    /// `Channels::new` users get via `Swp30::new`/`Swp30::reset`)
    pub fn new() -> Self {
        Self {
            streaming: StreamingBlock::NEW,
            filter: FilterBlock::new(),
            iir1: Iir1Block::new(),
            envelope: EnvelopeBlock::new(),
            lfo: LfoBlock::new(),
            pitch_offset: 0,
            peg_rate: 0,
            peg_cur: 0,
            peg_reached: 0,
        }
    }

    /// origin: swp30.cpp:2578-2597 `swp30_device::peg_step` (per-channel).
    /// Pitch EG: same `level_step` table as the envelope, 16 rates slower
    /// (¼ speed, :2570-2577 comment incl. the <16 speed continuation).
    /// `sample_counter` = MEG `m_sample_counter` seam (MEG row owns it).
    pub fn peg_step(&mut self, sample_counter: u32) {
        // :2580 util::sext(u32(m_pitch_offset & 0x3fff), 14)
        let target = sext14(self.pitch_offset as u32);
        let mut cur = self.peg_cur; // :2581
        if cur == target {
            self.peg_reached = 1; // :2583
            return; // :2584
        }
        let rate = ((self.peg_rate >> 8) & 0x7f) as i32 - 16; // :2586
        // :2587 the envelope's static table via the instance (C++ static call)
        let step = EnvelopeBlock::level_step(rate, sample_counter) as i32;
        if cur < target {
            // :2588-2590
            cur += step;
            if cur > target {
                cur = target;
            }
        } else {
            // :2591-2593
            cur -= step;
            if cur < target {
                cur = target;
            }
        }
        self.peg_cur = cur; // :2595
        self.peg_reached = (cur == target) as u8; // :2596 (bool → u8)
    }
}

/// The 64-voice grid (C++ `std::array<..., 0x40>` members), owned by
/// `Swp30`. MEG/mixer state stays OUT — awm2_step is the seam boundary.
pub struct Channels {
    pub voices: [Voice; 0x40],
}

impl Channels {
    pub fn new() -> Self {
        Self { voices: std::array::from_fn(|_| Voice::new()) }
    }

    /// origin: swp30.cpp:1969-1982 inside `swp30_device::reset` — exact
    /// per-array order (streaming loop, then the four peg/pitch fills, then
    /// the four block loops). m_meg/mixer/wave/meli/melo clears (:1966-67,
    /// :1984-99) belong to the MEG/mixer rows and are absent here.
    pub fn clear_all(&mut self) {
        for v in self.voices.iter_mut() {
            v.streaming.clear(); // :1969-1970
        }
        for v in self.voices.iter_mut() {
            v.pitch_offset = 0; // :1971 m_pitch_offset.fill(0)
        }
        for v in self.voices.iter_mut() {
            v.peg_rate = 0; // :1972
        }
        for v in self.voices.iter_mut() {
            v.peg_cur = 0; // :1973
        }
        for v in self.voices.iter_mut() {
            v.peg_reached = 0; // :1974
        }
        for v in self.voices.iter_mut() {
            v.filter.clear(); // :1975-1976
        }
        for v in self.voices.iter_mut() {
            v.iir1.clear(); // :1977-1978
        }
        for v in self.voices.iter_mut() {
            v.envelope.clear(); // :1979-1980
        }
        for v in self.voices.iter_mut() {
            v.lfo.clear(); // :1981-1982
        }
    }

    /// origin: swp30.cpp:1833-1884 `swp30_device::awm2_step` — the crown
    /// (6.239 idle-voice skip). Seams: `wave` = fetch-row reader;
    /// `rand_seed` = device LFO noise (swp30.h:73-78); `awm_idle` = the
    /// swp30.h:631 idle mask (set here; CLEARED by write16 :2122 and keyon_w
    /// :2248 in regs.rs — a written/keyed voice always rejoins the loop);
    /// `sample_counter` = `m_meg->m_sample_counter`
    /// (:1853, :1860 — MEG row owns the counter). The `--dump-dac` fprintf
    /// block (:1862-1879) is stderr-only diagnostics gated by m_dbg_dac
    /// (never set by the machine) — deliberately not ported, like the
    /// WTRACE/logerror sinks in regs.rs.
    /// Order: ascending chan (countr_zero) == the old full-sweep order, so
    /// the type-3 LFO rand DRAWS are the same sequence; an idle voice is
    /// envelope-inactive with the peg arrived, and the old loop `continue`d
    /// those BEFORE lfo.step / volume anyway — skipping them changes no
    /// sample and consumes no rand (that is the whole point of 6.239).
    #[allow(clippy::too_many_arguments)]
    pub fn awm2_step(
        &mut self,
        wave: &Wave<'_>,
        rand_seed: &mut u32,
        awm_idle: &mut u64,
        sample_counter: u32,
        samples_per_chan: &mut [i32; 0x40],
        dbg_dac: &mut Option<std::fs::File>,
        dbg_chan: i32,
        dbg_from: u32,
        dbg_count: u32,
    ) {
        samples_per_chan.fill(0); // :1837 (pre-zero: skipped/idle channels stay 0)
        let mut live = !*awm_idle; // :1838
        while live != 0 {
            let chan = live.trailing_zeros() as usize; // :1839 std::countr_zero
            live &= live - 1; // :1838 (clears the LOW set bit — chan just handled)
            let v = &mut self.voices[chan];
            // :1841-1844: attached voices just stamp the reached flag
            if v.peg_cur == sext14(v.pitch_offset as u32) {
                v.peg_reached = 1; // :1842
            } else {
                v.peg_step(sample_counter); // :1844
            }
            if !v.envelope.active() {
                // :1845-1849: peg arrived too => mark idle (never iterated
                // again until write16/keyon clears the bit); sample stays 0
                if v.peg_reached != 0 && v.peg_cur == sext14(v.pitch_offset as u32) {
                    *awm_idle |= 1u64 << chan; // :1847
                }
                continue;
            }

            // :1853 (lfo.get_pitch() i16 promotes to s32; peg trimmed to 14 bits)
            let (sample1, trigger_release) = v.streaming.step(
                wave,
                v.lfo.get_pitch() as i32,
                (v.peg_cur & 0x3fff) as u16,
            );
            if trigger_release {
                v.envelope.trigger_release(); // :1854-1855
            }

            let sample2 = v.filter.step(sample1); // :1858
            let sample3 = v.iir1.step(sample2); // :1859
            // :1860 EG result u16 + LFO amplitude u16 in int, as s32 level
            let sample4 = volume_apply(
                v.envelope.step(sample_counter) as i32 + v.lfo.get_amplitude() as i32,
                sample3,
            );

            // :1862-1879 --dump-dac (swp30.h:127-129)
            if let Some(f) = dbg_dac {
                if chan as i32 == dbg_chan
                    && sample_counter >= dbg_from
                    && sample_counter < dbg_from.wrapping_add(dbg_count)
                {
                    dbg_dump(
                        f,
                        sample_counter,
                        v.streaming.m_pos,
                        sample1 as i32,
                        sample2 as i32,
                        sample3 as i32,
                        sample4 as i32,
                        &v.filter,
                        &v.iir1,
                    );
                }
            }

            v.lfo.step(rand_seed); // :1881 (type-3 refresh consumes rand)
            samples_per_chan[chan] = sample4; // :1882
        }
    }
}

/// origin: swp30.cpp:1859-1876 — per-stage voice dump for --dump-dac.
#[allow(clippy::too_many_arguments)]
pub fn dbg_dump(
    f: &mut std::fs::File,
    sample_counter: u32,
    st_pos: i32,
    sample1: i32,
    sample2: i32,
    sample3: i32,
    sample4: i32,
    fl: &FilterBlock,
    iir: &Iir1Block,
) {
    use std::io::Write;
    let _ = writeln!(
        f,
        "vox {sample_counter} pos={st_pos} wave={sample1} filt={sample2} iir={sample3} out={sample4} f1a={:04x} l1={:04x} f2a={:04x} l2={:04x} fb={:04x} h={} b={} l={} iirA={},{},{} iirB={},{},{} hx={},{} hy={},{}",
        fl.m_filter_1_a,
        fl.m_level_1,
        fl.m_filter_2_a,
        fl.m_level_2,
        fl.m_filter_b,
        fl.m_filter_1_h,
        fl.m_filter_1_b,
        fl.m_filter_1_l,
        iir.m_a[0][0],
        iir.m_a[0][1],
        iir.m_b[0],
        iir.m_a[1][0],
        iir.m_a[1][1],
        iir.m_b[1],
        iir.m_hx[0],
        iir.m_hx[1],
        iir.m_hy[0],
        iir.m_hy[1]
    );
}

/// origin: swp30.cpp:4635-4640 (merged) — `--dump-dac` MEG m/r line.
/// (Disk fprintf is text-mode `\n`→CRLF; like `dbg_dump` above the Rust
/// seam writes `\n` — parsers must accept both.)
pub fn dbg_meg_regs(f: &mut std::fs::File, counter: u32, m: &[i32; 0x40], r: &[i32; 0x80]) {
    use std::io::Write;
    let mut s = format!("{counter}");
    for i in 0..0x40 {
        s.push_str(&format!(" m{i:02x}={}", m[i])); // :4637
    }
    for i in 0..0x80 {
        s.push_str(&format!(" r{i:02x}={}", r[i])); // :4639
    }
    let _ = writeln!(f, "{s}");
}

/// origin: swp30.cpp:4653-4657 (merged) — `--dump-dac` per-voice line.
pub fn dbg_awm_chans(f: &mut std::fs::File, counter: u32, samples: &[i32; 0x40]) {
    use std::io::Write;
    let mut s = format!("awm {counter}");
    for i in 0..0x40 {
        if samples[i] != 0 {
            // :4655-4656 (only nonzero channels)
            s.push_str(&format!(" c{i:02x}={}", samples[i]));
        }
    }
    let _ = writeln!(f, "{s}");
}

/// origin: swp30.cpp:3125-3128 — `--dump-dac` send line (values =
/// `mixer_out[0x10..0x20]` = `m_m[0x20..0x30]`, see mix.rs sample_step).
pub fn dbg_send(f: &mut std::fs::File, counter: u32, send: &[i32]) {
    use std::io::Write;
    let mut s = format!("send {counter}");
    for (j, v) in send.iter().enumerate() {
        s.push_str(&format!(" s{j:02x}={v}")); // :3127
    }
    let _ = writeln!(f, "{s}");
}
