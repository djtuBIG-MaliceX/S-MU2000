//! Sample-fetch seam of the SWP30 voice path — transliteration of
//! `src/mame/sound/swp30.cpp:315-915` (`streaming_block`) plus the wave-ROM
//! reader `memory_access<25, 2, -2, ENDIANNESS_LITTLE>::cache`, whose real
//! definition is `flat_space<AddrBits, DataWidth, AddrShift>` in
//! `src/compat/mamecompat.h:130-233` (aliased to `memory_access::cache` at
//! `src/compat/mamecompat.h:746-749`). NOT the CPU bus in `src/compat/membus.h`.
//!
//! AddrShift = -2: address 1 advances one 32-bit word (4 bytes). Little-endian
//! dword loads. Off-space wraps: pow2-sized spaces mask, others take
//! `byte % bytes`, always aligned down to the element size (mamecompat.h:203-216).
//! Empty space points at a static 8-byte zero buffer (mamecompat.h:136-143).

/// Wave-ROM reader — `flat_space<25, 2, -2>` restricted to the read seam the
/// streaming block actually uses (`read_dword`; `read_word` ported for parity).
/// W-SAMP1: the sampling-RAM overlay (`set_overlay`, mamecompat.h:155-159,
/// 170-175) is wired — `read_dword` checks the overlay FIRST (disk order).
/// `read_word` keeps the disk quirk: NO overlay check (mamecompat.h:145-148
/// never consults `m_ov`) — transliterated, not "fixed".
/// origin: src/compat/mamecompat.h:130-233 (flat_space)
#[derive(Debug, Clone, Copy)]
pub struct Wave<'a> {
    /// m_base / m_bytes (mamecompat.h:225, 227). Empty → 8 static zero bytes.
    bytes: &'a [u8],
    /// m_mask = bytes - 1 (pow2 spaces only; mamecompat.h:141)
    mask: usize,
    /// m_pow2 (mamecompat.h:142)
    pow2: bool,
    /// m_ov (mamecompat.h:230) — sampling-RAM window base, raw-shared with
    /// the Machine (set_sample_ram, swp30.h:53). Null when unset.
    ov: *const u8,
    /// m_ov_units (mamecompat.h:232) — window size in 32-bit words; 0 = off
    /// (set_overlay folds the null-base case into units, :174)
    ov_units: u32,
}

const WAVE_ZERO: [u8; 8] = [0u8; 8];

/// Overlay origin from `set_sample_ram` (swp30.h:53: `set_overlay(base,
/// 0x1000000, bytes >> 2)`) — word address where the sampling RAM starts.
pub const SAMPRAM_FROM: u32 = 0x0100_0000;

impl<'a> Wave<'a> {
    /// Mirrors `set(base, bytes)` (mamecompat.h:134-143): a null/empty base
    /// reads as zeros instead of requiring an emptiness check at read sites.
    /// Overlay OFF (mamecompat.h:229-232 member defaults m_ov=null, units 0).
    pub fn new(bytes: &'a [u8]) -> Self {
        // origin: mamecompat.h:138-142
        let (bytes, mask, pow2) = if bytes.is_empty() {
            (&WAVE_ZERO[..], 0usize, false) // base=s_zero, m_bytes=0 → wrap() → offset 0
        } else {
            let n = bytes.len();
            (bytes, n - 1, n & (n - 1) == 0)
        };
        Self { bytes, mask, pow2, ov: std::ptr::null(), ov_units: 0 }
    }

    /// `set(base, bytes)` + `set_overlay(base, SAMPRAM_FROM, words)`
    /// (mamecompat.h:134-143 + 170-175, called by swp30.h:53 `set_sample_ram`
    /// on BOTH devices from mu2000.cpp:89-90).
    ///
    /// SAFETY: `ov` must stay valid and readable for `'a` (the Machine owns
    /// `sampram: Vec<u8>`; the buffer never reallocs — only in-place
    /// `StateIo::mem` restore, state.rs:55 / mu2000.cpp:3530), and the window
    /// must be exactly `words * 4` bytes. Raw-pointer aliasing matches disk
    /// (`u8 *m_ov`): the write side is the device-side `wave_write_dword`
    /// (regs of the same or the OTHER swp instance — exactly the C++
    /// shared-sampram race the master/slave handshake serializes).
    pub unsafe fn with_overlay(bytes: &'a [u8], ov: *const u8, words: u32) -> Self {
        // origin: mamecompat.h:170-175 — m_ov_units = base ? units : 0
        let mut w = Self::new(bytes);
        w.ov = ov;
        w.ov_units = if ov.is_null() { 0 } else { words };
        w
    }

    /// AddrShift byte offset with element-aligned wrap.
    /// origin: mamecompat.h:205-216 (offset_of + wrap, AddrShift = -2)
    #[inline]
    fn offset_of(&self, addr: u32, elem: usize) -> usize {
        let byte = (addr as usize) << 2; // << (-AddrShift), AddrShift = -2
        if self.bytes.is_empty() {
            return 0; // mamecompat.h:213 (`if (!m_bytes) return 0;`)
        }
        if self.pow2 {
            return byte & self.mask & !(elem - 1); // mamecompat.h:214
        }
        (byte % self.bytes.len()) & !(elem - 1) // mamecompat.h:215
    }

    /// The hottest read (2-3 per sample). pow2 fast path is branch-free;
    /// little-endian dword, addr 1 = one dword (AddrShift = -2).
    /// Overlay check comes FIRST — same order and same single-compare cost
    /// as disk (mamecompat.h:155-159); with the overlay off (units 0, the
    /// `Wave::new` default and every boot path) the branch folds away.
    /// origin: mamecompat.h:153-166 (read_dword; overlay leg :155-159,
    /// pow2 leg :160-164)
    #[inline]
    pub fn read_dword(&self, addr: u32) -> u32 {
        if addr.wrapping_sub(SAMPRAM_FROM) < self.ov_units {
            // origin: mamecompat.h:156-158 — memcpy(&v, m_ov + (addr-from)<<2, 4)
            unsafe {
                let p = self.ov.add(((addr - SAMPRAM_FROM) as usize) << 2);
                u32::from_le_bytes([*p, *p.add(1), *p.add(2), *p.add(3)])
            }
        } else if self.pow2 {
            let off = ((addr as usize) << 2) & self.mask & !3usize;
            // SAFETY (S8): pow2 leg only runs for a non-empty power-of-two
            // space (WAVE_ZERO/empty take the non-pow2 leg — `new` sets
            // pow2=false there), and every real wave ROM is ≥ 4 dwords, so
            // len % 4 == 0 and `off = x & (len-1) & !3` gives off + 3 ≤ len-1.
            // Disk reads the same 4 bytes via memcpy (mamecompat.h:160-163);
            // unchecked slice read lets LLVM fuse the four byte loads into one
            // aligned 32-bit mov (bit-identical value, one load instead of
            // four bounds-checked ones).
            unsafe {
                let p = self.bytes.as_ptr().add(off);
                u32::from_le_bytes([*p, *p.add(1), *p.add(2), *p.add(3)])
            }
        } else {
            let off = self.offset_of(addr, 4);
            u32::from_le_bytes([
                self.bytes[off],
                self.bytes[off + 1],
                self.bytes[off + 2],
                self.bytes[off + 3],
            ])
        }
    }

    /// origin: mamecompat.h:145-148 (read_word — slow path only in C++ too;
    /// DISK QUIRK kept: `read_word` does NOT check the overlay, only
    /// `read_dword` does — mamecompat.h:145-148 vs :153-159)
    #[inline]
    pub fn read_word(&self, addr: u32) -> u16 {
        let off = self.offset_of(addr, 2);
        u16::from_le_bytes([self.bytes[off], self.bytes[off + 1]])
    }
}

// ---------------------------------------------------------------------------
// Static tables — origin: swp30.cpp:315-371 (lambda-generated statics)
// ---------------------------------------------------------------------------

/// DPCM step-expansion table. origin: swp30.cpp:315-326
/// offset[] at :317, main loop :318-323, hardware quirk :324.
const fn gen_dpcm_expand() -> [i16; 256] {
    // origin: swp30.cpp:317
    let offset: [i16; 4] = [0, 0x20, 0x60, 0xe0];
    let mut deltas = [0i16; 256];
    let mut i: u32 = 0;
    while i != 128 {
        // origin: swp30.cpp:319-322
        let e = i >> 5;
        let base = (((i & 0x1f) << e) as i16).wrapping_add(offset[e as usize]);
        deltas[i as usize] = base;
        deltas[i as usize + 128] = base.wrapping_neg();
        i += 1;
    }
    // origin: swp30.cpp:324 — unused by samples, tested on hardware
    deltas[0x80] = 0x88;
    deltas
}

/// Pitch conversion table, 2**(i/1024) as 1.12. origin: swp30.cpp:328-334.
///
/// C++ computes pow(2, i/1024.0) * 4096 and truncates to u16 (:332).
/// Deviation from the previous draft of this file (whose f64-bit-reassembly
/// scheme contradicted its own data and had short arrays): the table below is
/// now stored literally. Each entry is loor(2**((i+12288)/1024)), computed
/// exactly with integer 1024th roots (binary search over Python bignums), and
/// cross-checked against the IEEE double evaluation of the C++ expression
/// (int(pow(2.0, i/1024.0) * 4096.0)): all 1024 entries agree, and for
/// i >= 1 every value sits >= 3.7e-4 away from a truncation boundary, so no
/// libm misround (approx. 1 ulp around 4096-8192, i.e. approx. 1e-12) can flip
/// a truncation. i=0 is exactly 4096 (pow(2, 0.0) is exact).
const fn gen_pitch_base() -> [u16; 0x400] {
    [
        0x1000, 0x1002, 0x1005, 0x1008, 0x100b, 0x100d, 0x1010, 0x1013, 0x1016, 0x1019, 0x101b, 0x101e, 0x1021, 0x1024, 0x1027, 0x1029,
        0x102c, 0x102f, 0x1032, 0x1035, 0x1037, 0x103a, 0x103d, 0x1040, 0x1043, 0x1045, 0x1048, 0x104b, 0x104e, 0x1051, 0x1054, 0x1056,
        0x1059, 0x105c, 0x105f, 0x1062, 0x1065, 0x1067, 0x106a, 0x106d, 0x1070, 0x1073, 0x1076, 0x1078, 0x107b, 0x107e, 0x1081, 0x1084,
        0x1087, 0x108a, 0x108d, 0x108f, 0x1092, 0x1095, 0x1098, 0x109b, 0x109e, 0x10a1, 0x10a4, 0x10a6, 0x10a9, 0x10ac, 0x10af, 0x10b2,
        0x10b5, 0x10b8, 0x10bb, 0x10be, 0x10c0, 0x10c3, 0x10c6, 0x10c9, 0x10cc, 0x10cf, 0x10d2, 0x10d5, 0x10d8, 0x10db, 0x10de, 0x10e0,
        0x10e3, 0x10e6, 0x10e9, 0x10ec, 0x10ef, 0x10f2, 0x10f5, 0x10f8, 0x10fb, 0x10fe, 0x1101, 0x1104, 0x1107, 0x110a, 0x110d, 0x1110,
        0x1113, 0x1115, 0x1118, 0x111b, 0x111e, 0x1121, 0x1124, 0x1127, 0x112a, 0x112d, 0x1130, 0x1133, 0x1136, 0x1139, 0x113c, 0x113f,
        0x1142, 0x1145, 0x1148, 0x114b, 0x114e, 0x1151, 0x1154, 0x1157, 0x115a, 0x115d, 0x1160, 0x1163, 0x1166, 0x1169, 0x116c, 0x116f,
        0x1172, 0x1175, 0x1178, 0x117b, 0x117e, 0x1181, 0x1184, 0x1187, 0x118a, 0x118e, 0x1191, 0x1194, 0x1197, 0x119a, 0x119d, 0x11a0,
        0x11a3, 0x11a6, 0x11a9, 0x11ac, 0x11af, 0x11b2, 0x11b5, 0x11b8, 0x11bb, 0x11be, 0x11c2, 0x11c5, 0x11c8, 0x11cb, 0x11ce, 0x11d1,
        0x11d4, 0x11d7, 0x11da, 0x11dd, 0x11e0, 0x11e4, 0x11e7, 0x11ea, 0x11ed, 0x11f0, 0x11f3, 0x11f6, 0x11f9, 0x11fc, 0x11ff, 0x1203,
        0x1206, 0x1209, 0x120c, 0x120f, 0x1212, 0x1215, 0x1219, 0x121c, 0x121f, 0x1222, 0x1225, 0x1228, 0x122b, 0x122f, 0x1232, 0x1235,
        0x1238, 0x123b, 0x123e, 0x1241, 0x1245, 0x1248, 0x124b, 0x124e, 0x1251, 0x1254, 0x1258, 0x125b, 0x125e, 0x1261, 0x1264, 0x1268,
        0x126b, 0x126e, 0x1271, 0x1274, 0x1278, 0x127b, 0x127e, 0x1281, 0x1284, 0x1288, 0x128b, 0x128e, 0x1291, 0x1294, 0x1298, 0x129b,
        0x129e, 0x12a1, 0x12a5, 0x12a8, 0x12ab, 0x12ae, 0x12b2, 0x12b5, 0x12b8, 0x12bb, 0x12be, 0x12c2, 0x12c5, 0x12c8, 0x12cc, 0x12cf,
        0x12d2, 0x12d5, 0x12d9, 0x12dc, 0x12df, 0x12e2, 0x12e6, 0x12e9, 0x12ec, 0x12ef, 0x12f3, 0x12f6, 0x12f9, 0x12fd, 0x1300, 0x1303,
        0x1306, 0x130a, 0x130d, 0x1310, 0x1314, 0x1317, 0x131a, 0x131e, 0x1321, 0x1324, 0x1328, 0x132b, 0x132e, 0x1332, 0x1335, 0x1338,
        0x133c, 0x133f, 0x1342, 0x1346, 0x1349, 0x134c, 0x1350, 0x1353, 0x1356, 0x135a, 0x135d, 0x1360, 0x1364, 0x1367, 0x136a, 0x136e,
        0x1371, 0x1375, 0x1378, 0x137b, 0x137f, 0x1382, 0x1385, 0x1389, 0x138c, 0x1390, 0x1393, 0x1396, 0x139a, 0x139d, 0x13a1, 0x13a4,
        0x13a7, 0x13ab, 0x13ae, 0x13b2, 0x13b5, 0x13b8, 0x13bc, 0x13bf, 0x13c3, 0x13c6, 0x13ca, 0x13cd, 0x13d0, 0x13d4, 0x13d7, 0x13db,
        0x13de, 0x13e2, 0x13e5, 0x13e8, 0x13ec, 0x13ef, 0x13f3, 0x13f6, 0x13fa, 0x13fd, 0x1401, 0x1404, 0x1408, 0x140b, 0x140f, 0x1412,
        0x1416, 0x1419, 0x141d, 0x1420, 0x1423, 0x1427, 0x142a, 0x142e, 0x1431, 0x1435, 0x1438, 0x143c, 0x143f, 0x1443, 0x1447, 0x144a,
        0x144e, 0x1451, 0x1455, 0x1458, 0x145c, 0x145f, 0x1463, 0x1466, 0x146a, 0x146d, 0x1471, 0x1474, 0x1478, 0x147b, 0x147f, 0x1483,
        0x1486, 0x148a, 0x148d, 0x1491, 0x1494, 0x1498, 0x149c, 0x149f, 0x14a3, 0x14a6, 0x14aa, 0x14ad, 0x14b1, 0x14b5, 0x14b8, 0x14bc,
        0x14bf, 0x14c3, 0x14c7, 0x14ca, 0x14ce, 0x14d1, 0x14d5, 0x14d9, 0x14dc, 0x14e0, 0x14e3, 0x14e7, 0x14eb, 0x14ee, 0x14f2, 0x14f6,
        0x14f9, 0x14fd, 0x1500, 0x1504, 0x1508, 0x150b, 0x150f, 0x1513, 0x1516, 0x151a, 0x151e, 0x1521, 0x1525, 0x1529, 0x152c, 0x1530,
        0x1534, 0x1537, 0x153b, 0x153f, 0x1542, 0x1546, 0x154a, 0x154d, 0x1551, 0x1555, 0x1559, 0x155c, 0x1560, 0x1564, 0x1567, 0x156b,
        0x156f, 0x1572, 0x1576, 0x157a, 0x157e, 0x1581, 0x1585, 0x1589, 0x158d, 0x1590, 0x1594, 0x1598, 0x159c, 0x159f, 0x15a3, 0x15a7,
        0x15ab, 0x15ae, 0x15b2, 0x15b6, 0x15ba, 0x15bd, 0x15c1, 0x15c5, 0x15c9, 0x15cc, 0x15d0, 0x15d4, 0x15d8, 0x15dc, 0x15df, 0x15e3,
        0x15e7, 0x15eb, 0x15ef, 0x15f2, 0x15f6, 0x15fa, 0x15fe, 0x1602, 0x1605, 0x1609, 0x160d, 0x1611, 0x1615, 0x1618, 0x161c, 0x1620,
        0x1624, 0x1628, 0x162c, 0x1630, 0x1633, 0x1637, 0x163b, 0x163f, 0x1643, 0x1647, 0x164a, 0x164e, 0x1652, 0x1656, 0x165a, 0x165e,
        0x1662, 0x1666, 0x1669, 0x166d, 0x1671, 0x1675, 0x1679, 0x167d, 0x1681, 0x1685, 0x1689, 0x168d, 0x1690, 0x1694, 0x1698, 0x169c,
        0x16a0, 0x16a4, 0x16a8, 0x16ac, 0x16b0, 0x16b4, 0x16b8, 0x16bc, 0x16c0, 0x16c4, 0x16c7, 0x16cb, 0x16cf, 0x16d3, 0x16d7, 0x16db,
        0x16df, 0x16e3, 0x16e7, 0x16eb, 0x16ef, 0x16f3, 0x16f7, 0x16fb, 0x16ff, 0x1703, 0x1707, 0x170b, 0x170f, 0x1713, 0x1717, 0x171b,
        0x171f, 0x1723, 0x1727, 0x172b, 0x172f, 0x1733, 0x1737, 0x173b, 0x173f, 0x1743, 0x1747, 0x174b, 0x174f, 0x1753, 0x1757, 0x175b,
        0x175f, 0x1763, 0x1768, 0x176c, 0x1770, 0x1774, 0x1778, 0x177c, 0x1780, 0x1784, 0x1788, 0x178c, 0x1790, 0x1794, 0x1798, 0x179c,
        0x17a1, 0x17a5, 0x17a9, 0x17ad, 0x17b1, 0x17b5, 0x17b9, 0x17bd, 0x17c1, 0x17c6, 0x17ca, 0x17ce, 0x17d2, 0x17d6, 0x17da, 0x17de,
        0x17e2, 0x17e7, 0x17eb, 0x17ef, 0x17f3, 0x17f7, 0x17fb, 0x17ff, 0x1804, 0x1808, 0x180c, 0x1810, 0x1814, 0x1818, 0x181d, 0x1821,
        0x1825, 0x1829, 0x182d, 0x1832, 0x1836, 0x183a, 0x183e, 0x1842, 0x1847, 0x184b, 0x184f, 0x1853, 0x1857, 0x185c, 0x1860, 0x1864,
        0x1868, 0x186d, 0x1871, 0x1875, 0x1879, 0x187e, 0x1882, 0x1886, 0x188a, 0x188f, 0x1893, 0x1897, 0x189b, 0x18a0, 0x18a4, 0x18a8,
        0x18ac, 0x18b1, 0x18b5, 0x18b9, 0x18be, 0x18c2, 0x18c6, 0x18ca, 0x18cf, 0x18d3, 0x18d7, 0x18dc, 0x18e0, 0x18e4, 0x18e9, 0x18ed,
        0x18f1, 0x18f6, 0x18fa, 0x18fe, 0x1902, 0x1907, 0x190b, 0x1910, 0x1914, 0x1918, 0x191d, 0x1921, 0x1925, 0x192a, 0x192e, 0x1932,
        0x1937, 0x193b, 0x193f, 0x1944, 0x1948, 0x194d, 0x1951, 0x1955, 0x195a, 0x195e, 0x1963, 0x1967, 0x196b, 0x1970, 0x1974, 0x1979,
        0x197d, 0x1981, 0x1986, 0x198a, 0x198f, 0x1993, 0x1998, 0x199c, 0x19a0, 0x19a5, 0x19a9, 0x19ae, 0x19b2, 0x19b7, 0x19bb, 0x19c0,
        0x19c4, 0x19c9, 0x19cd, 0x19d1, 0x19d6, 0x19da, 0x19df, 0x19e3, 0x19e8, 0x19ec, 0x19f1, 0x19f5, 0x19fa, 0x19fe, 0x1a03, 0x1a07,
        0x1a0c, 0x1a10, 0x1a15, 0x1a19, 0x1a1e, 0x1a23, 0x1a27, 0x1a2c, 0x1a30, 0x1a35, 0x1a39, 0x1a3e, 0x1a42, 0x1a47, 0x1a4b, 0x1a50,
        0x1a55, 0x1a59, 0x1a5e, 0x1a62, 0x1a67, 0x1a6b, 0x1a70, 0x1a75, 0x1a79, 0x1a7e, 0x1a82, 0x1a87, 0x1a8b, 0x1a90, 0x1a95, 0x1a99,
        0x1a9e, 0x1aa3, 0x1aa7, 0x1aac, 0x1ab0, 0x1ab5, 0x1aba, 0x1abe, 0x1ac3, 0x1ac8, 0x1acc, 0x1ad1, 0x1ad5, 0x1ada, 0x1adf, 0x1ae3,
        0x1ae8, 0x1aed, 0x1af1, 0x1af6, 0x1afb, 0x1aff, 0x1b04, 0x1b09, 0x1b0e, 0x1b12, 0x1b17, 0x1b1c, 0x1b20, 0x1b25, 0x1b2a, 0x1b2e,
        0x1b33, 0x1b38, 0x1b3d, 0x1b41, 0x1b46, 0x1b4b, 0x1b4f, 0x1b54, 0x1b59, 0x1b5e, 0x1b62, 0x1b67, 0x1b6c, 0x1b71, 0x1b75, 0x1b7a,
        0x1b7f, 0x1b84, 0x1b89, 0x1b8d, 0x1b92, 0x1b97, 0x1b9c, 0x1ba0, 0x1ba5, 0x1baa, 0x1baf, 0x1bb4, 0x1bb8, 0x1bbd, 0x1bc2, 0x1bc7,
        0x1bcc, 0x1bd0, 0x1bd5, 0x1bda, 0x1bdf, 0x1be4, 0x1be9, 0x1bed, 0x1bf2, 0x1bf7, 0x1bfc, 0x1c01, 0x1c06, 0x1c0b, 0x1c0f, 0x1c14,
        0x1c19, 0x1c1e, 0x1c23, 0x1c28, 0x1c2d, 0x1c31, 0x1c36, 0x1c3b, 0x1c40, 0x1c45, 0x1c4a, 0x1c4f, 0x1c54, 0x1c59, 0x1c5e, 0x1c63,
        0x1c67, 0x1c6c, 0x1c71, 0x1c76, 0x1c7b, 0x1c80, 0x1c85, 0x1c8a, 0x1c8f, 0x1c94, 0x1c99, 0x1c9e, 0x1ca3, 0x1ca8, 0x1cad, 0x1cb2,
        0x1cb7, 0x1cbc, 0x1cc1, 0x1cc6, 0x1ccb, 0x1cd0, 0x1cd5, 0x1cda, 0x1cdf, 0x1ce4, 0x1ce9, 0x1cee, 0x1cf3, 0x1cf8, 0x1cfd, 0x1d02,
        0x1d07, 0x1d0c, 0x1d11, 0x1d16, 0x1d1b, 0x1d20, 0x1d25, 0x1d2a, 0x1d2f, 0x1d34, 0x1d39, 0x1d3e, 0x1d43, 0x1d48, 0x1d4d, 0x1d53,
        0x1d58, 0x1d5d, 0x1d62, 0x1d67, 0x1d6c, 0x1d71, 0x1d76, 0x1d7b, 0x1d80, 0x1d86, 0x1d8b, 0x1d90, 0x1d95, 0x1d9a, 0x1d9f, 0x1da4,
        0x1da9, 0x1daf, 0x1db4, 0x1db9, 0x1dbe, 0x1dc3, 0x1dc8, 0x1dcd, 0x1dd3, 0x1dd8, 0x1ddd, 0x1de2, 0x1de7, 0x1ded, 0x1df2, 0x1df7,
        0x1dfc, 0x1e01, 0x1e06, 0x1e0c, 0x1e11, 0x1e16, 0x1e1b, 0x1e21, 0x1e26, 0x1e2b, 0x1e30, 0x1e35, 0x1e3b, 0x1e40, 0x1e45, 0x1e4a,
        0x1e50, 0x1e55, 0x1e5a, 0x1e5f, 0x1e65, 0x1e6a, 0x1e6f, 0x1e75, 0x1e7a, 0x1e7f, 0x1e84, 0x1e8a, 0x1e8f, 0x1e94, 0x1e9a, 0x1e9f,
        0x1ea4, 0x1ea9, 0x1eaf, 0x1eb4, 0x1eb9, 0x1ebf, 0x1ec4, 0x1ec9, 0x1ecf, 0x1ed4, 0x1ed9, 0x1edf, 0x1ee4, 0x1eea, 0x1eef, 0x1ef4,
        0x1efa, 0x1eff, 0x1f04, 0x1f0a, 0x1f0f, 0x1f14, 0x1f1a, 0x1f1f, 0x1f25, 0x1f2a, 0x1f2f, 0x1f35, 0x1f3a, 0x1f40, 0x1f45, 0x1f4b,
        0x1f50, 0x1f55, 0x1f5b, 0x1f60, 0x1f66, 0x1f6b, 0x1f71, 0x1f76, 0x1f7b, 0x1f81, 0x1f86, 0x1f8c, 0x1f91, 0x1f97, 0x1f9c, 0x1fa2,
        0x1fa7, 0x1fad, 0x1fb2, 0x1fb8, 0x1fbd, 0x1fc3, 0x1fc8, 0x1fce, 0x1fd3, 0x1fd9, 0x1fde, 0x1fe4, 0x1fe9, 0x1fef, 0x1ff4, 0x1ffa,
    ]
}

/// Sample interpolation functions f0 and f1 (all-integer math).
/// origin: swp30.cpp:337-367
const fn gen_interpolation_table() -> [[i16; 0x800]; 2] {
    let mut result = [[0i16; 0x800]; 2];
    // origin: swp30.cpp:344-348
    let mut i: u32 = 1;
    while i != 1024 {
        let f0 = ((((i << 20) - i * i * i) / 6) >> 20) as i16;
        result[0][(2 * i - 1) as usize] = f0;
        result[0][(2 * i) as usize] = f0;
        i += 1;
    }
    // origin: swp30.cpp:349-353
    i = 1;
    while i != 513 {
        let f1 = (i + ((((i * i) << 10) - i * i * i) >> 21)) as i16;
        result[1][(2 * i - 1) as usize] = f1;
        result[1][(2 * i) as usize] = f1;
        i += 1;
    }
    // origin: swp30.cpp:354-361 — second half of f1 adjusted so f0+f1 pairs sum to 0x400
    i = 513;
    while i != 1024 {
        let i1 = 2 * i;
        let i2 = 2047 ^ i1;
        let f1 = (0x400i32
            + result[0][i1 as usize] as i32
            + result[0][i2 as usize] as i32
            - result[1][i2 as usize] as i32) as i16;
        result[1][(2 * i - 1) as usize] = f1;
        result[1][(2 * i) as usize] = f1;
        i += 1;
    }
    // origin: swp30.cpp:362-365
    result[0][0] = 0x000;
    result[0][0x7ff] = 0x000;
    result[1][0] = 0x000;
    result[1][0x7ff] = 0x400;
    result
}

/// Clamp ceilings per scale (index = scale). origin: swp30.cpp:369-371
const MAX_VALUE: [i32; 8] = [0x7fff, 0x7ffe, 0x7ffc, 0x7ff8, 0x7ff0, 0x7fe0, 0x7fc0, 0x7f80];

// Static table instances (C++ declares them as static members at
// swp30.h:136-139; generated by the lambdas at swp30.cpp:315-367).
static PITCH_BASE: [u16; 0x400] = gen_pitch_base(); // origin: swp30.h:136
static DPCM_EXPAND: [i16; 256] = gen_dpcm_expand(); // origin: swp30.h:137
static INTERP_TABLE: [[i16; 0x800]; 2] = gen_interpolation_table(); // origin: swp30.h:138

/// Truncate a u32 expression to s16 the way the C++ implicit conversion does
/// (low 16 bits, two's complement).
#[inline(always)]
fn s16_of(v: u32) -> i16 {
    v as u16 as i16
}

/// One AWM2 sample-fetch block. origin: swp30.h:135-187, swp30.cpp:373-909.
/// Field order and types mirror swp30.h:141-154 exactly; every field is
/// initialized explicitly (no Default) per the project hard rules.
pub struct StreamingBlock {
    m_start: i32,          // origin: swp30.h:141
    m_loop: i32,           // origin: swp30.h:142
    m_address: u32,        // origin: swp30.h:143
    m_pitch: u16,          // origin: swp30.h:144
    m_loop_size: i32,      // origin: swp30.h:146
    pub m_pos: i32,        // origin: swp30.h:147 (pub only for --dump-dac)
    m_pos_dec: i32,        // origin: swp30.h:148
    m_dpcm_s0: i16,        // origin: swp30.h:149
    m_dpcm_s1: i16,        // origin: swp30.h:149
    m_dpcm_s2: i16,        // origin: swp30.h:149
    m_dpcm_s3: i16,        // origin: swp30.h:149
    m_dpcm_pos: u32,       // origin: swp30.h:150
    m_dpcm_delta: i32,     // origin: swp30.h:151
    m_first: bool,         // origin: swp30.h:153
    m_finetune_active: bool, // origin: swp30.h:153
    m_done: bool,          // origin: swp30.h:153
    m_last: i16,           // origin: swp30.h:154
}

impl StreamingBlock {
    /// Header default initializers, swp30.h:141-154 (all zero/false).
    pub const NEW: Self = Self {
        m_start: 0,
        m_loop: 0,
        m_address: 0,
        m_pitch: 0,
        m_loop_size: 0,
        m_pos: 0,
        m_pos_dec: 0,
        m_dpcm_s0: 0,
        m_dpcm_s1: 0,
        m_dpcm_s2: 0,
        m_dpcm_s3: 0,
        m_dpcm_pos: 0,
        m_dpcm_delta: 0,
        m_first: false,
        m_finetune_active: false,
        m_done: false,
        m_last: 0,
    };

    /// origin: swp30.cpp:373-388 (clear). Note: does NOT touch
    /// m_finetune_active, exactly as the C++ listing.
    pub fn clear(&mut self) {
        self.m_start = 0;
        self.m_loop = 0;
        self.m_address = 0;
        self.m_pitch = 0;
        self.m_loop_size = 0x400;
        self.m_pos = 0;
        self.m_pos_dec = 0;
        self.m_dpcm_s0 = 0;
        self.m_dpcm_s1 = 0;
        self.m_dpcm_s2 = 0;
        self.m_dpcm_s3 = 0;
        self.m_dpcm_pos = 0;
        self.m_dpcm_delta = 0;
        self.m_first = false;
        self.m_done = false;
        self.m_last = 0;
    }

    /// origin: swp30.cpp:390-400 (keyon)
    pub fn keyon(&mut self) {
        self.m_pos = -((self.m_start & 0xffffff) as i32) - 1;
        self.m_pos_dec = 0;
        self.m_dpcm_s0 = 0;
        self.m_dpcm_s1 = 0;
        self.m_dpcm_s2 = 0;
        self.m_dpcm_s3 = 0;
        self.m_dpcm_pos = (self.m_pos + 1) as u32;
        self.m_dpcm_delta = 0;
        self.m_first = true;
        self.m_finetune_active = false;
        self.m_done = false;
    }

    /// origin: swp30.cpp:402-410 (scale_and_clamp_one)
    #[inline]
    fn scale_and_clamp_one(val: &mut i16, scale: u32, limit: i32) {
        // s16 promotes to int, then << scale (|val|<<7 < 2^23: no overflow)
        let mut sval = (*val as i32) << scale;
        if sval < -0x8000 {
            sval = -0x8000;
        } else if sval > limit {
            sval = limit;
        }
        *val = sval as i16; // post-clamp value always fits s16
    }

    /// origin: swp30.cpp:412-422 (scale_and_clamp)
    #[inline]
    fn scale_and_clamp(&self, val0: &mut i16, val1: &mut i16, val2: &mut i16, val3: &mut i16) {
        let scale = (self.m_address >> 27) & 7;
        if scale == 0 {
            return;
        }
        let limit = MAX_VALUE[scale as usize];
        Self::scale_and_clamp_one(val0, scale, limit);
        Self::scale_and_clamp_one(val1, scale, limit);
        Self::scale_and_clamp_one(val2, scale, limit);
        Self::scale_and_clamp_one(val3, scale, limit);
    }

    /// Reverse-direction flag bit 31 of m_loop. origin: e.g. swp30.cpp:427
    #[inline(always)]
    fn reverse(&self) -> bool {
        (self.m_loop as u32) & 0x8000_0000 != 0
    }

    /// origin: swp30.cpp:424-458 (read_16)
    fn read_16(
        &self,
        wave: &Wave<'_>,
        val0: &mut i16,
        val1: &mut i16,
        val2: &mut i16,
        val3: &mut i16,
    ) {
        // S-MU2000: 逆向きのときは 1 つ手前から読み、step() で並びを裏返す（doc/upstream.md の 35）
        // origin: swp30.cpp:427
        let spos: i32 = if self.reverse() {
            self.m_pos.wrapping_neg().wrapping_sub(1)
        } else {
            self.m_pos
        };
        let base_address = self.m_address & 0x1ff_ffff; // origin: swp30.cpp:428
        let adr = base_address.wrapping_add((spos >> 1) as u32); // origin: swp30.cpp:429
        match spos & 1 {
            // origin: swp30.cpp:431-443 (case 0)
            0 => {
                let l0 = wave.read_dword(adr);
                // S-MU2000: MAME は l1 も adr から読んでいて、後ろの 2 つが前の 2 つの写しになっていた
                // （doc/upstream.md の 10）  origin: swp30.cpp:437
                let l1 = wave.read_dword(adr + 1);
                *val0 = s16_of(l0);
                *val1 = s16_of(l0 >> 16);
                *val2 = s16_of(l1);
                *val3 = s16_of(l1 >> 16);
            }
            // origin: swp30.cpp:444-455 (case 1)
            _ => {
                let l0 = wave.read_dword(adr);
                let l1 = wave.read_dword(adr + 1);
                let l2 = wave.read_dword(adr + 2);
                *val0 = s16_of(l0 >> 16);
                *val1 = s16_of(l1);
                *val2 = s16_of(l1 >> 16);
                *val3 = s16_of(l2);
            }
        }
        self.scale_and_clamp(val0, val1, val2, val3); // origin: swp30.cpp:457
    }

    /// origin: swp30.cpp:460-560 (read_12)
    fn read_12(
        &self,
        wave: &Wave<'_>,
        val0: &mut i16,
        val1: &mut i16,
        val2: &mut i16,
        val3: &mut i16,
    ) {
        // S-MU2000: 逆向きのときは 1 つ手前から読み、step() で並びを裏返す（doc/upstream.md の 35）
        // origin: swp30.cpp:463
        let spos: i32 = if self.reverse() {
            self.m_pos.wrapping_neg().wrapping_sub(1)
        } else {
            self.m_pos
        };
        let base_address = self.m_address & 0x1ff_ffff; // origin: swp30.cpp:464
        let adr = base_address.wrapping_add(((spos >> 3).wrapping_mul(3)) as u32); // origin: swp30.cpp:465
        match spos & 7 {
            // origin: swp30.cpp:467-477 (case 0)
            0 => {
                let l0 = wave.read_dword(adr);
                let l1 = wave.read_dword(adr + 1);
                *val0 = s16_of((l0 & 0x0000_0fff) << 4);
                *val1 = s16_of((l0 & 0x00ff_f000) >> 8);
                *val2 = s16_of(((l0 & 0xff00_0000) >> 20) | ((l1 & 0x0000_000f) << 12));
                *val3 = s16_of(l1 & 0x0000_fff0);
            }
            // origin: swp30.cpp:478-488 (case 1)
            1 => {
                let l0 = wave.read_dword(adr);
                let l1 = wave.read_dword(adr + 1);
                *val0 = s16_of((l0 & 0x00ff_f000) >> 8);
                *val1 = s16_of(((l0 & 0xff00_0000) >> 20) | ((l1 & 0x0000_000f) << 12));
                *val2 = s16_of(l1 & 0x0000_fff0);
                *val3 = s16_of((l1 & 0x0fff_0000) >> 12);
            }
            // origin: swp30.cpp:489-500 (case 2)
            2 => {
                let l0 = wave.read_dword(adr);
                let l1 = wave.read_dword(adr + 1);
                let l2 = wave.read_dword(adr + 2);
                *val0 = s16_of(((l0 & 0xff00_0000) >> 20) | ((l1 & 0x0000_000f) << 12));
                *val1 = s16_of(l1 & 0x0000_fff0);
                *val2 = s16_of((l1 & 0x0fff_0000) >> 12);
                *val3 = s16_of(((l1 & 0xf000_0000) >> 24) | ((l2 & 0x0000_00ff) << 8));
            }
            // origin: swp30.cpp:501-511 (case 3)
            3 => {
                let l1 = wave.read_dword(adr + 1);
                let l2 = wave.read_dword(adr + 2);
                *val0 = s16_of(l1 & 0x0000_fff0);
                *val1 = s16_of((l1 & 0x0fff_0000) >> 12);
                *val2 = s16_of(((l1 & 0xf000_0000) >> 24) | ((l2 & 0x0000_00ff) << 8));
                *val3 = s16_of((l2 & 0x000f_ff00) >> 4);
            }
            // origin: swp30.cpp:512-522 (case 4)
            4 => {
                let l1 = wave.read_dword(adr + 1);
                let l2 = wave.read_dword(adr + 2);
                *val0 = s16_of((l1 & 0x0fff_0000) >> 12);
                *val1 = s16_of(((l1 & 0xf000_0000) >> 24) | ((l2 & 0x0000_00ff) << 8));
                *val2 = s16_of((l2 & 0x000f_ff00) >> 4);
                *val3 = s16_of((l2 & 0xfff0_0000) >> 16);
            }
            // origin: swp30.cpp:523-534 (case 5)
            5 => {
                let l1 = wave.read_dword(adr + 1);
                let l2 = wave.read_dword(adr + 2);
                let l3 = wave.read_dword(adr + 3);
                *val0 = s16_of(((l1 & 0xf000_0000) >> 24) | ((l2 & 0x0000_00ff) << 8));
                *val1 = s16_of((l2 & 0x000f_ff00) >> 4);
                *val2 = s16_of((l2 & 0xfff0_0000) >> 16);
                *val3 = s16_of((l3 & 0x0000_0fff) << 4);
            }
            // origin: swp30.cpp:535-545 (case 6)
            6 => {
                let l2 = wave.read_dword(adr + 2);
                let l3 = wave.read_dword(adr + 3);
                *val0 = s16_of((l2 & 0x000f_ff00) >> 4);
                *val1 = s16_of((l2 & 0xfff0_0000) >> 16);
                *val2 = s16_of((l3 & 0x0000_0fff) << 4);
                *val3 = s16_of((l3 & 0x00ff_f000) >> 8);
            }
            // origin: swp30.cpp:546-557 (case 7)
            _ => {
                let l2 = wave.read_dword(adr + 2);
                let l3 = wave.read_dword(adr + 3);
                let l4 = wave.read_dword(adr + 4);
                *val0 = s16_of((l2 & 0xfff0_0000) >> 16);
                *val1 = s16_of((l3 & 0x0000_0fff) << 4);
                *val2 = s16_of((l3 & 0x00ff_f000) >> 8);
                *val3 = s16_of(((l3 & 0xff00_0000) >> 20) | ((l4 & 0x0000_000f) << 12));
            }
        }
        self.scale_and_clamp(val0, val1, val2, val3); // origin: swp30.cpp:559
    }

    /// origin: swp30.cpp:562-614 (read_8)
    fn read_8(
        &self,
        wave: &Wave<'_>,
        val0: &mut i16,
        val1: &mut i16,
        val2: &mut i16,
        val3: &mut i16,
    ) {
        // S-MU2000: 逆向きのときは 1 つ手前から読み、step() で並びを裏返す（doc/upstream.md の 35）
        // origin: swp30.cpp:565
        let spos: i32 = if self.reverse() {
            self.m_pos.wrapping_neg().wrapping_sub(1)
        } else {
            self.m_pos
        };
        let base_address = self.m_address & 0x1ff_ffff; // origin: swp30.cpp:566
        let adr = base_address.wrapping_add((spos >> 2) as u32); // origin: swp30.cpp:567
        match spos & 3 {
            // origin: swp30.cpp:569-578 (case 0)
            0 => {
                let l0 = wave.read_dword(adr);
                *val0 = s16_of((l0 & 0x0000_00ff) << 8);
                *val1 = s16_of(l0 & 0x0000_ff00);
                *val2 = s16_of((l0 & 0x00ff_0000) >> 8);
                *val3 = s16_of((l0 & 0xff00_0000) >> 16);
            }
            // origin: swp30.cpp:579-589 (case 1)
            1 => {
                let l0 = wave.read_dword(adr);
                let l1 = wave.read_dword(adr + 1);
                *val0 = s16_of(l0 & 0x0000_ff00);
                *val1 = s16_of((l0 & 0x00ff_0000) >> 8);
                *val2 = s16_of((l0 & 0xff00_0000) >> 16);
                *val3 = s16_of((l1 & 0x0000_00ff) << 8);
            }
            // origin: swp30.cpp:590-600 (case 2)
            2 => {
                let l0 = wave.read_dword(adr);
                let l1 = wave.read_dword(adr + 1);
                *val0 = s16_of((l0 & 0x00ff_0000) >> 8);
                *val1 = s16_of((l0 & 0xff00_0000) >> 16);
                *val2 = s16_of((l1 & 0x0000_00ff) << 8);
                *val3 = s16_of(l1 & 0x0000_ff00);
            }
            // origin: swp30.cpp:601-611 (case 3)
            _ => {
                let l0 = wave.read_dword(adr);
                let l1 = wave.read_dword(adr + 1);
                *val0 = s16_of((l0 & 0xff00_0000) >> 16);
                *val1 = s16_of((l1 & 0x0000_00ff) << 8);
                *val2 = s16_of(l1 & 0x0000_ff00);
                *val3 = s16_of((l1 & 0x00ff_0000) >> 8);
            }
        }
        self.scale_and_clamp(val0, val1, val2, val3); // origin: swp30.cpp:613
    }

    /// One DPCM expansion step. origin: swp30.cpp:618-663 (dpcm_step).
    /// mode/scale/limit are hoisted by the caller (read_8c), per the comment
    /// at swp30.cpp:616-617.
    fn dpcm_step(&mut self, input: u8, mode: u32, scale: u32, limit: i32) {
        self.m_dpcm_s0 = self.m_dpcm_s1;
        self.m_dpcm_s1 = self.m_dpcm_s2;
        self.m_dpcm_s2 = self.m_dpcm_s3;
        // origin: swp30.cpp:644-646. m_dpcm_delta & 7 is bitwise two's-complement
        // in both languages for negative deltas; >> 8 is arithmetic (sext) in both.
        let acc = (self.m_dpcm_s3 as i32) >> scale;
        let rem = -(self.m_dpcm_delta & 7);
        let mut delta = (self.m_dpcm_delta >> 8) + DPCM_EXPAND[input as usize] as i32;

        // origin: swp30.cpp:648-653. |acc + delta| < 2^16 after clamp history;
        // << scale stays inside i32, matching the C++ int arithmetic.
        let mut sample = (acc + delta) << scale;
        if sample < -0x8000 {
            sample = -0x8000;
        } else if sample > limit {
            sample = limit;
        }
        self.m_dpcm_s3 = sample as i16; // clamped value fits s16
        delta = (sample >> scale) - acc;

        // origin: swp30.cpp:656-662
        match mode {
            0 => {
                let y = delta * 7 + rem;
                self.m_dpcm_delta = (y >> 3) * 256 + if y & 7 != 0 { 8 - (y & 7) } else { 0 };
            }
            1 => {
                let y = delta * 3 + rem;
                self.m_dpcm_delta = (y >> 2) * 256 + if y & 3 != 0 { 4 - (y & 3) } else { 0 };
            }
            2 => {
                let y = delta + rem;
                self.m_dpcm_delta = (y >> 1) * 256 + if y & 1 != 0 { 2 - (y & 1) } else { 0 };
            }
            // origin: swp30.cpp:661 (default: mode 3 — next delta is 0)
            _ => self.m_dpcm_delta = 0,
        }
    }

    /// origin: swp30.cpp:665-704 (read_8c)
    fn read_8c(
        &mut self,
        wave: &Wave<'_>,
        val0: &mut i16,
        val1: &mut i16,
        val2: &mut i16,
        val3: &mut i16,
    ) {
        let mut base_address = self.m_address & 0x1ff_ffff; // origin: swp30.cpp:667
        if self.reverse() {
            // S-MU2000: MAME はここで abort() して落ちる（MU100 でも同じ。doc/upstream.md の 7）。
            // 逆向き圧縮は発生し得ない過渡状態（発音枠の取り合い中の 1 サンプル）なので、
            // 直前の値を保つ。origin: swp30.cpp:668-680
            *val0 = self.m_dpcm_s0;
            *val1 = self.m_dpcm_s1;
            *val2 = self.m_dpcm_s2;
            *val3 = self.m_dpcm_s3;
            return;
        } else {
            // origin: swp30.cpp:682-697
            let mode = (self.m_address >> 25) & 3;
            let scale = (self.m_address >> 27) & 7;
            let limit = MAX_VALUE[scale as usize];
            let mut spos = self.m_dpcm_pos as i32;
            base_address = base_address.wrapping_add((spos >> 2) as u32);
            let mut cv = wave.read_dword(base_address);
            while spos != self.m_pos.wrapping_add(4) {
                let input = (cv >> ((spos & 3) << 3)) as u8;
                self.dpcm_step(input, mode, scale, limit);
                spos += 1;
                if (spos & 3) == 0 {
                    base_address += 1;
                    cv = wave.read_dword(base_address);
                }
            }
            self.m_dpcm_pos = spos as u32;
        }

        // origin: swp30.cpp:700-703
        *val0 = self.m_dpcm_s0;
        *val1 = self.m_dpcm_s1;
        *val2 = self.m_dpcm_s2;
        *val3 = self.m_dpcm_s3;
    }

    /// One interpolated output sample. origin: swp30.cpp:706-794 (step).
    /// Returns `(sample, done)` == `std::pair<s16, bool>`.
    pub fn step(&mut self, wave: &Wave<'_>, pitch_lfo: i32, pitch_offset: u16) -> (i16, bool) {
        if self.m_done {
            return (self.m_last, false); // origin: swp30.cpp:708-709
        }

        let mut val0: i16 = 0;
        let mut val1: i16 = 0;
        let mut val2: i16 = 0;
        let mut val3: i16 = 0;

        // origin: swp30.cpp:713-718 (m_address >> 30 selects the format; the
        // 2-bit shift makes the switch exhaustive)
        match self.m_address >> 30 {
            0 => self.read_16(wave, &mut val0, &mut val1, &mut val2, &mut val3),
            1 => self.read_12(wave, &mut val0, &mut val1, &mut val2, &mut val3),
            2 => self.read_8(wave, &mut val0, &mut val1, &mut val2, &mut val3),
            _ => self.read_8c(wave, &mut val0, &mut val1, &mut val2, &mut val3),
        }
        // S-MU2000: 逆向きに鳴らすサンプルは、読んだ 4 つが番地の順（再生の順とは逆）に並ぶ。
        // 裏返して前・今・次・その次の順にする（doc/upstream.md の 35）
        // origin: swp30.cpp:719-726
        if self.reverse() && (self.m_address >> 30) != 3 {
            std::mem::swap(&mut val0, &mut val3);
            std::mem::swap(&mut val1, &mut val2);
        }
        if self.m_first {
            val0 = 0; // origin: swp30.cpp:727-728
        }

        // origin: swp30.cpp:730-732 ("not perfectly exact" comment preserved below)
        // Not perfectly exact, there are some rounding-like issues from
        // time to time
        let index = ((self.m_pos_dec >> 4) & 2047) as usize;
        // The 4-tap cubic interpolation can overshoot past full scale near loud
        // peaks; saturate to s16 instead of letting the store wrap (a full-scale
        // sign flip = an audible click on loud notes).
        // origin: swp30.cpp:733-741
        let racc = (-(INTERP_TABLE[0][index ^ 2047] as i32) * val0 as i32
            + INTERP_TABLE[1][index ^ 2047] as i32 * val1 as i32
            + INTERP_TABLE[1][index] as i32 * val2 as i32
            - INTERP_TABLE[0][index] as i32 * val3 as i32)
            >> 10;
        let result = racc.clamp(-0x8000, 0x7fff) as i16; // origin: swp30.cpp:742

        // Bit 14 is set by the firmware on compressed samples but is not part
        // of the pitch.  Left in, it trips the 0x4000 clamp below as soon as
        // finetune becomes active (i.e. once the loop point is crossed), and
        // the note jumps to the maximum pitch for the rest of its life.
        // S-MU2000: ピッチ EG の今の値を、14bit で回り込ませて足す（doc/upstream.md の 13）
        // S-MU2000: ピッチは 14bit の符号付き（1 オクターブ 1024、0x3eef = -0x111）。下の e の計算は
        // 符号付きのまま成り立つ。MAME はループ点を越えたあとの範囲の制限だけ符号なし（0〜0x3fff）で
        // かけていたので、ビブラートが片側だけになっていた（doc/upstream.md の 14）
        // origin: swp30.cpp:744-753 — util::sext(x,14) via shl/shr 18 on the 0x3fff-masked value
        let masked = ((self.m_pitch as i32).wrapping_add(pitch_offset as i32) & 0x3fff) as u32;
        let mut sp = ((masked << 18) as i32) >> 18; // origin: swp30.cpp:753 (util::sext(x, 14))
        sp = sp.wrapping_add(pitch_lfo);
        if self.m_finetune_active {
            // origin: swp30.cpp:754-759
            let mut ft = (self.m_loop >> 24) & 0x7f;
            if ft & 0x40 != 0 {
                ft -= 0x80;
            }
            sp = sp.wrapping_add(ft).clamp(-0x2000, 0x1fff);
        }
        let pitch = sp as u32; // origin: swp30.cpp:760

        // origin: swp30.cpp:762-764 — pitch_base[m] promotes to int in C++;
        // (u16 << 10) < 2^26 so u32 arithmetic is bit-identical.
        let e = ((pitch >> 10) + 8) & 15;
        let m = pitch & 0x3ff;
        let st = ((PITCH_BASE[m as usize] as u32) << 10) >> (15 - e);

        // origin: swp30.cpp:766 — m_pos_dec (s32) += step (u32): C++ converts
        // the sum to unsigned; reproduce with the u32 round-trip.
        self.m_pos_dec = ((self.m_pos_dec as u32).wrapping_add(st)) as i32;
        if self.m_pos_dec >= 0x8000 {
            self.m_first = false; // origin: swp30.cpp:768
            self.m_pos += self.m_pos_dec >> 15; // origin: swp30.cpp:769
            if !self.m_finetune_active && self.m_pos >= 0 {
                self.m_finetune_active = true; // origin: swp30.cpp:770-771
            }

            self.m_pos_dec &= 0x7fff; // origin: swp30.cpp:773
            if self.m_pos >= self.m_loop_size {
                if !(self.reverse() || (self.m_start as u32 & 0x4000_0000) != 0) {
                    // origin: swp30.cpp:776-785
                    self.m_pos -= self.m_loop_size;
                    self.m_pos_dec += (self.m_start >> 15) & 0x7e00;
                    if self.m_pos_dec >= 0x8000 {
                        self.m_pos += 1;
                    }
                    self.m_pos_dec &= 0x7fff;
                    // S-MU2000: MAME は 3 に決め打ちしていた（doc/upstream.md の 12）。
                    // 済んだ所の続きから展開する。origin: swp30.cpp:781-785
                    self.m_dpcm_pos = self.m_dpcm_pos.wrapping_sub(self.m_loop_size as u32);
                } else {
                    // origin: swp30.cpp:786-790
                    self.m_done = true;
                    self.m_last = result;
                    return (self.m_last, true);
                }
            }
        }
        (result, false) // origin: swp30.cpp:793
    }

    /// origin: swp30.cpp:796-806 (update_loop_size)
    fn update_loop_size(&mut self) {
        // The loop size is 24 bits; bits 24-30 are the finetune, which is read
        // back as (m_loop >> 24) & 0x7f just below in step().  Taking 26 bits
        // here folds the bottom two finetune bits into the size, so any sample
        // with a finetune >= 4 gets a loop size tens of millions of samples
        // long and never loops.
        self.m_loop_size = self.m_loop & 0xff_ffff;
        if self.m_loop_size == 0 && !(self.reverse() || (self.m_start as u32 & 0x4000_0000) != 0) {
            self.m_loop_size = 0x400;
        }
    }

    /// origin: swp30.cpp:808-812 (start_h_w)
    pub fn start_h_w(&mut self, data: u16) {
        self.m_start = (self.m_start & 0x0000_ffff) | ((data as i32) << 16);
        self.update_loop_size();
    }

    /// origin: swp30.cpp:814-817 (start_l_w)
    pub fn start_l_w(&mut self, data: u16) {
        // C++ `m_start & 0xffff0000`: literal is unsigned int; do the mask in u32.
        self.m_start = ((self.m_start as u32 & 0xffff_0000) | (data as u32)) as i32;
    }

    /// origin: swp30.cpp:819-823 (loop_h_w)
    pub fn loop_h_w(&mut self, data: u16) {
        self.m_loop = (self.m_loop & 0x0000_ffff) | ((data as i32) << 16);
        self.update_loop_size();
    }

    /// origin: swp30.cpp:825-829 (loop_l_w)
    pub fn loop_l_w(&mut self, data: u16) {
        // C++ `m_loop & 0xffff0000`: literal is unsigned int; do the mask in u32.
        self.m_loop = ((self.m_loop as u32 & 0xffff_0000) | (data as u32)) as i32;
        self.update_loop_size();
    }

    /// origin: swp30.cpp:831-834 (address_h_w)
    pub fn address_h_w(&mut self, data: u16) {
        self.m_address = (self.m_address & 0x0000_ffff) | ((data as u32) << 16);
    }

    /// origin: swp30.cpp:836-839 (address_l_w)
    pub fn address_l_w(&mut self, data: u16) {
        self.m_address = (self.m_address & 0xffff_0000) | (data as u32);
    }

    /// origin: swp30.cpp:841-844 (pitch_w)
    pub fn pitch_w(&mut self, data: u16) {
        self.m_pitch = data;
    }

    /// origin: swp30.cpp:846-849 (start_h_r)
    pub fn start_h_r(&self) -> u16 {
        (self.m_start >> 16) as u16
    }

    /// origin: swp30.cpp:851-854 (start_l_r)
    pub fn start_l_r(&self) -> u16 {
        self.m_start as u16
    }

    /// origin: swp30.cpp:856-859 (loop_h_r)
    pub fn loop_h_r(&self) -> u16 {
        (self.m_loop >> 16) as u16
    }

    /// origin: swp30.cpp:861-864 (loop_l_r)
    pub fn loop_l_r(&self) -> u16 {
        self.m_loop as u16
    }

    /// origin: swp30.cpp:866-869 (address_h_r)
    pub fn address_h_r(&self) -> u16 {
        (self.m_address >> 16) as u16
    }

    /// origin: swp30.cpp:871-874 (address_l_r)
    pub fn address_l_r(&self) -> u16 {
        self.m_address as u16
    }

    /// origin: swp30.cpp:876-879 (pitch_r)
    pub fn pitch_r(&self) -> u16 {
        self.m_pitch
    }

    /// origin: swp30.cpp:881-909 (describe). m_start/m_loop print their raw
    /// two's-complement bits under %x, hence the u32 casts.
    pub fn describe(&self) -> String {
        let mut desc = String::new();
        use std::fmt::Write as _;
        let _ = write!(
            desc,
            "[{:04x} {:08x} {:08x} {:08x}] ",
            self.m_pitch, self.m_start as u32, self.m_loop as u32, self.m_address
        );
        let _ = write!(
            desc,
            "sample {:06x}-{:06x} @ {:07x} ",
            (self.m_start & 0xff_ffff) as u32,
            (self.m_loop & 0xff_ffff) as u32,
            self.m_address & 0x1ff_ffff
        );
        match self.m_address >> 30 {
            0 => desc.push_str("16"),
            1 => desc.push_str("12"),
            2 => desc.push_str("8 "),
            _ => {
                let _ = write!(desc, "c{:x}", (self.m_address >> 25) & 3);
            }
        }
        if self.m_address & 0x3800_0000 != 0 {
            let _ = write!(desc, " scale {:x}", (self.m_address >> 27) & 7);
        }
        if self.reverse() {
            desc.push_str(" back");
        } else if (self.m_start as u32 & 0x4000_0000) != 0 {
            desc.push_str(" fwd ");
        } else {
            desc.push_str(" loop");
        }
        if (self.m_start as u32) & 0x3f00_0000 != 0 {
            let _ = write!(desc, " loop-adjust {:02x}", (self.m_start >> 24) & 0x3f);
        }
        if (self.m_loop as u32) & 0x7f00_0000 != 0 {
            if (self.m_loop as u32) & 0x4000_0000 != 0 {
                let _ = write!(
                    desc,
                    " loop-tune -{:02x}",
                    0x40 - ((self.m_loop >> 24) & 0x3f)
                );
            } else {
                let _ = write!(desc, " loop-tune +{:02x}", (self.m_loop >> 24) & 0x3f);
            }
        }
        // Deviation (fixed Phase B): this pitch branch of the C++ describe()
        // was missing from the port; the ground-truth harness (%TEMP%\fetchgt)
        // proved the mismatch on seeds with m_pitch & 0x3fff != 0.
        // origin: swp30.cpp:908-912
        if self.m_pitch & 0x2000 != 0 {
            let p = 0x4000u32 - (self.m_pitch as u32 & 0x3fff);
            let _ = write!(desc, " pitch -{:x}.{:03x}", p >> 10, p & 0x3ff);
        } else if self.m_pitch & 0x3fff != 0 {
            let _ = write!(
                desc,
                " pitch +{:x}.{:03x}",
                (self.m_pitch >> 10) & 7,
                self.m_pitch & 0x3ff
            );
        }
        desc
    }
}

// ---------------------------------------------------------------------------
// Test-only seams (Phase B ground-truth replay, tests/fetch.rs). These expose
// raw state so committed C++ harness vectors can seed/verify without changing
// any emulation behaviour. Not part of the device-facing API.
// ---------------------------------------------------------------------------

/// Raw snapshot of every `streaming_block` field (swp30.h:141-154).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub struct FetchState {
    pub start: i32,
    pub loop_: i32,
    pub address: u32,
    pub pitch: u16,
    pub loop_size: i32,
    pub pos: i32,
    pub pos_dec: i32,
    pub dpcm_s0: i16,
    pub dpcm_s1: i16,
    pub dpcm_s2: i16,
    pub dpcm_s3: i16,
    pub dpcm_pos: u32,
    pub dpcm_delta: i32,
    pub first: bool,
    pub finetune_active: bool,
    pub done: bool,
    pub last: i16,
}

impl StreamingBlock {
    #[doc(hidden)]
    pub fn state(&self) -> FetchState {
        FetchState {
            start: self.m_start,
            loop_: self.m_loop,
            address: self.m_address,
            pitch: self.m_pitch,
            loop_size: self.m_loop_size,
            pos: self.m_pos,
            pos_dec: self.m_pos_dec,
            dpcm_s0: self.m_dpcm_s0,
            dpcm_s1: self.m_dpcm_s1,
            dpcm_s2: self.m_dpcm_s2,
            dpcm_s3: self.m_dpcm_s3,
            dpcm_pos: self.m_dpcm_pos,
            dpcm_delta: self.m_dpcm_delta,
            first: self.m_first,
            finetune_active: self.m_finetune_active,
            done: self.m_done,
            last: self.m_last,
        }
    }

    /// Seed raw state directly (harness driver sets fields the same way).
    #[doc(hidden)]
    pub fn set_state(&mut self, s: FetchState) {
        self.m_start = s.start;
        self.m_loop = s.loop_;
        self.m_address = s.address;
        self.m_pitch = s.pitch;
        self.m_loop_size = s.loop_size;
        self.m_pos = s.pos;
        self.m_pos_dec = s.pos_dec;
        self.m_dpcm_s0 = s.dpcm_s0;
        self.m_dpcm_s1 = s.dpcm_s1;
        self.m_dpcm_s2 = s.dpcm_s2;
        self.m_dpcm_s3 = s.dpcm_s3;
        self.m_dpcm_pos = s.dpcm_pos;
        self.m_dpcm_delta = s.dpcm_delta;
        self.m_first = s.first;
        self.m_finetune_active = s.finetune_active;
        self.m_done = s.done;
        self.m_last = s.last;
    }

    /// Call the mode reader selected by `m_address >> 30` without advancing
    /// position — same dispatch as the top of step().
    /// origin: swp30.cpp:713-718
    #[doc(hidden)]
    pub fn read_vals(&mut self, wave: &Wave<'_>) -> [i16; 4] {
        let (mut v0, mut v1, mut v2, mut v3) = (0i16, 0i16, 0i16, 0i16);
        match self.m_address >> 30 {
            0 => self.read_16(wave, &mut v0, &mut v1, &mut v2, &mut v3),
            1 => self.read_12(wave, &mut v0, &mut v1, &mut v2, &mut v3),
            2 => self.read_8(wave, &mut v0, &mut v1, &mut v2, &mut v3),
            _ => self.read_8c(wave, &mut v0, &mut v1, &mut v2, &mut v3),
        }
        [v0, v1, v2, v3]
    }

    #[doc(hidden)]
    pub fn dpcm_expand_tbl() -> &'static [i16; 256] {
        &DPCM_EXPAND
    }
    #[doc(hidden)]
    pub fn pitch_base_tbl() -> &'static [u16; 0x400] {
        &PITCH_BASE
    }
    #[doc(hidden)]
    pub fn interp_tbl() -> &'static [[i16; 0x800]; 2] {
        &INTERP_TABLE
    }
    #[doc(hidden)]
    pub fn max_value_tbl() -> &'static [i32; 8] {
        &MAX_VALUE
    }
}

// ---- M5-W3b state serializer (origin: swp30.cpp:4721 s.stdarr(m_streaming)) ----
// Element stream = RAW C++ struct bytes (stdarr memcpy). Layout ground truth:
// g++ -std=c++20 -O3 (x86-64) harness %TEMP%\opencode\stategt\gt.cpp
// (data members byte-copied from swp30.h:156-169): sizeof==52, offsets below.
impl StreamingBlock {
    /// harness `sizeof(streaming_block)` == 52 (pad @14-15 after m_pitch,
    /// pad @47 after m_done, tail pad @50-51 for align-4).
    pub const STATE_SIZE: usize = 52;

    /// append this block's 52 wire bytes (C++ declaration order, explicit
    /// LE widths, bool = 1 byte, explicit pad fillers)
    pub fn state_bytes(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.m_start.to_le_bytes()); // swp30.h:156 @0
        out.extend_from_slice(&self.m_loop.to_le_bytes()); // :157 @4
        out.extend_from_slice(&self.m_address.to_le_bytes()); // :158 @8
        out.extend_from_slice(&self.m_pitch.to_le_bytes()); // :159 @12
        out.extend_from_slice(&[0u8; 2]); // pad @14
        out.extend_from_slice(&self.m_loop_size.to_le_bytes()); // :161 @16
        out.extend_from_slice(&self.m_pos.to_le_bytes()); // :162 @20
        out.extend_from_slice(&self.m_pos_dec.to_le_bytes()); // :163 @24
        out.extend_from_slice(&self.m_dpcm_s0.to_le_bytes()); // :164 @28
        out.extend_from_slice(&self.m_dpcm_s1.to_le_bytes()); // :164 @30
        out.extend_from_slice(&self.m_dpcm_s2.to_le_bytes()); // :164 @32
        out.extend_from_slice(&self.m_dpcm_s3.to_le_bytes()); // :164 @34
        out.extend_from_slice(&self.m_dpcm_pos.to_le_bytes()); // :165 @36
        out.extend_from_slice(&self.m_dpcm_delta.to_le_bytes()); // :166 @40
        out.push(self.m_first as u8); // :168 @44 (bool = 1 byte)
        out.push(self.m_finetune_active as u8); // :168 @45
        out.push(self.m_done as u8); // :168 @46
        out.push(0u8); // pad @47
        out.extend_from_slice(&self.m_last.to_le_bytes()); // :169 @48
        out.extend_from_slice(&[0u8; 2]); // tail pad @50-51
    }

    /// fill from 52 wire bytes (pads ignored; bool follows != 0, same
    /// decode rule as StateIo::bool — no repr reading)
    pub fn state_load(&mut self, b: &[u8]) {
        let g = |o: usize, n: usize| &b[o..o + n];
        self.m_start = i32::from_le_bytes(g(0, 4).try_into().unwrap());
        self.m_loop = i32::from_le_bytes(g(4, 4).try_into().unwrap());
        self.m_address = u32::from_le_bytes(g(8, 4).try_into().unwrap());
        self.m_pitch = u16::from_le_bytes(g(12, 2).try_into().unwrap());
        self.m_loop_size = i32::from_le_bytes(g(16, 4).try_into().unwrap());
        self.m_pos = i32::from_le_bytes(g(20, 4).try_into().unwrap());
        self.m_pos_dec = i32::from_le_bytes(g(24, 4).try_into().unwrap());
        self.m_dpcm_s0 = i16::from_le_bytes(g(28, 2).try_into().unwrap());
        self.m_dpcm_s1 = i16::from_le_bytes(g(30, 2).try_into().unwrap());
        self.m_dpcm_s2 = i16::from_le_bytes(g(32, 2).try_into().unwrap());
        self.m_dpcm_s3 = i16::from_le_bytes(g(34, 2).try_into().unwrap());
        self.m_dpcm_pos = u32::from_le_bytes(g(36, 4).try_into().unwrap());
        self.m_dpcm_delta = i32::from_le_bytes(g(40, 4).try_into().unwrap());
        self.m_first = b[44] != 0;
        self.m_finetune_active = b[45] != 0;
        self.m_done = b[46] != 0;
        self.m_last = i16::from_le_bytes(g(48, 2).try_into().unwrap());
    }
}

#[cfg(test)]
mod state_layout_tests {
    use super::*;

    /// Every field byte-group at its g++-harness offset (layout.txt:
    /// m_start 0, m_loop 4, m_address 8, m_pitch 12, pad 14, m_loop_size
    /// 16, m_pos 20, m_pos_dec 24, s0 28, s1 30, s2 32, s3 34, dpcm_pos
    /// 36, dpcm_delta 40, bools 44/45/46, pad 47, m_last 48, tail 50).
    #[test]
    fn streaming_state_image_offsets_match_harness() {
        let mut b = StreamingBlock::NEW;
        b.m_start = -0x0102_0304;
        b.m_loop = 0x1112_1314;
        b.m_address = 0x2122_2324;
        b.m_pitch = 0x3132;
        b.m_loop_size = -0x4142_4344;
        b.m_pos = 0x5152_5354;
        b.m_pos_dec = -0x6162_6364;
        b.m_dpcm_s0 = -0x7172;
        b.m_dpcm_s1 = 0x7273;
        b.m_dpcm_s2 = -0x7374;
        b.m_dpcm_s3 = 0x7475;
        b.m_dpcm_pos = 0x8182_8384;
        b.m_dpcm_delta = -0x1192_9394;
        b.m_first = true;
        b.m_finetune_active = true;
        b.m_done = false;
        b.m_last = -0x1234;
        let mut out = Vec::new();
        b.state_bytes(&mut out);
        assert_eq!(out.len(), StreamingBlock::STATE_SIZE);
        let i32le = |v: i32| v.to_le_bytes();
        assert_eq!(&out[0..4], &i32le(-0x0102_0304));
        assert_eq!(&out[4..8], &i32le(0x1112_1314));
        assert_eq!(&out[8..12], &0x2122_2324u32.to_le_bytes());
        assert_eq!(&out[12..14], &0x3132u16.to_le_bytes());
        assert_eq!(&out[14..16], &[0, 0], "pad after m_pitch");
        assert_eq!(&out[16..20], &i32le(-0x4142_4344));
        assert_eq!(&out[20..24], &i32le(0x5152_5354));
        assert_eq!(&out[24..28], &i32le(-0x6162_6364));
        assert_eq!(&out[28..30], &(-0x7172i16).to_le_bytes());
        assert_eq!(&out[30..32], &0x7273i16.to_le_bytes());
        assert_eq!(&out[32..34], &(-0x7374i16).to_le_bytes());
        assert_eq!(&out[34..36], &0x7475i16.to_le_bytes());
        assert_eq!(&out[36..40], &0x8182_8384u32.to_le_bytes());
        assert_eq!(&out[40..44], &i32le(-0x1192_9394));
        assert_eq!(&out[44..47], &[1, 1, 0], "bools m_first/finetune/done");
        assert_eq!(&out[47..48], &[0], "pad before m_last");
        assert_eq!(&out[48..50], &(-0x1234i16).to_le_bytes());
        assert_eq!(&out[50..52], &[0, 0], "tail pad");
        let mut c = StreamingBlock::NEW;
        c.state_load(&out);
        let mut out2 = Vec::new();
        c.state_bytes(&mut out2);
        assert_eq!(out, out2, "save/load/save stability");
        assert!(c.m_first && c.m_finetune_active && !c.m_done);
    }
}
