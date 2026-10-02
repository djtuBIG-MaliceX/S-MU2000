//! SH7042 BSC + DMAC — ledger row `periph: bsc/dmac` (session F9).
//!
//! origin: `src/mame/cpu/sh_bsc.h/.cpp` + `src/mame/cpu/sh_dmac.h/.cpp` +
//! map `src/mame/cpu/sh7042_map.hxx` + die wiring
//! `src/mame/cpu/sh7042.cpp:171-177` (`SH_BSC(m_bsc)`, `SH_DMAC(m_dmac)`,
//! `SH_DMAC_CHANNEL(m_dmac0..3)`).
//!
//! DISK BEHAVIOR SUMMARY (disk is ground truth; these are *not* invented
//! no-ops — every field write is stored and read back):
//! - `sh_bsc_device`: eight u16 regs (BCR1/BCR2/WCR1/WCR2/DCR/RTCSR/RTCNT/
//!   RTCOR). Reset (sh_bsc.cpp:35-42): BCR1=0x200f, BCR2=0xffff, WCR1=0xffff,
//!   WCR2=0x000f, rest 0. Every read returns the stored reg; every write is
//!   `COMBINE_DATA` (masked partial write). NO side effects beyond storage —
//!   the only other statement is `logerror` (stderr-only, state-free; dropped
//!   here exactly as the other periph rows).
//! - `sh_dmac_device`: ONE u16 DMAOR, reset 0 (sh_dmac.cpp:31), same
//!   store-and-COMBINE semantics. `m_cpu` is a required_device (:20) but NO
//!   handler touches it.
//! - `sh_dmac_channel_device`: four u32 regs SAR/DAR/DMATCR/CHCR, reset 0
//!   (:63-66), same semantics. `m_cpu`/`m_intc` required (:48-49) but no
//!   handler touches them. THERE IS NO DMA ENGINE ON DISK: no transfer loop,
//!   no request/ack handling, no devcb outputs at all — the wiring row gets
//!   nothing extra to hook. The "working device" here is pure register state.
//!
//! Map decode already done in `sh7042.rs` (this row adds NOTHING there):
//! r8 8620-8627|862a-8631 (sh7042.rs:545), r16/w16 skip the 8628/8629 hole
//! (:616/:849 — disk has no 8628/862a-adjacent r16 case: BCR2..WCR2 end at
//! 8626, DCR starts 862a), dmac 86b0-86b1, ch blocks 86c0+16*ch. The trait
//! defaults (0 / drop) are the not-attached miss behavior; attaching these
//! structs flips the BSC reads from 0 to the disk reset values.

use crate::sh7042::Sh7042Peripherals;

/// COMBINE_DATA (identical to sh7042.rs `combine16` / cmt.rs local).
#[inline]
fn combine16(cur: &mut u16, data: u16, mask: u16) {
    *cur = (*cur & !mask) | (data & mask);
}

/// origin: COMBINE_DATA on u32 regs (sh_dmac.cpp:77,89,101,113).
#[inline]
fn combine32(cur: &mut u32, data: u32, mask: u32) {
    *cur = (*cur & !mask) | (data & mask);
}

// ---------------------------------------------------------------------------
// BSC — origin: src/mame/cpu/sh_bsc.h:20-50, sh_bsc.cpp
// ---------------------------------------------------------------------------

/// SH Bus State Controller. Registers affect bus timings only on real硅; on
/// disk they are pure storage (no timing model). All fields pub for the M5
/// state row (`state()` sh_bsc.cpp:141-146, tag "bsc").
pub struct ShBsc {
    // origin: sh_bsc.h:46 — ctor inits ALL to 0, then device_reset overwrites.
    pub m_bcr1: u16,
    pub m_bcr2: u16,
    pub m_wcr1: u16,
    pub m_wcr2: u16,
    pub m_dcr: u16,
    pub m_rtcsr: u16,
    pub m_rtcnt: u16,
    pub m_rtcor: u16,
}

impl ShBsc {
    /// origin: sh_bsc.cpp:16-19 (ctor = base only; h:46 `= 0` inits) then
    /// device_start (:21-31, save_items only) then device_reset — every field
    /// explicit at construction (Invariant #3).
    pub fn new() -> Self {
        let mut d = Self {
            m_bcr1: 0, // h:46
            m_bcr2: 0,
            m_wcr1: 0,
            m_wcr2: 0,
            m_dcr: 0,
            m_rtcsr: 0,
            m_rtcnt: 0,
            m_rtcor: 0,
        };
        d.device_reset();
        d
    }

    /// origin: sh_bsc.cpp:33-43 (device_reset).
    pub fn device_reset(&mut self) {
        self.m_bcr1 = 0x200f; // :35
        self.m_bcr2 = 0xffff; // :36
        self.m_wcr1 = 0xffff; // :37
        self.m_wcr2 = 0x000f; // :38
        self.m_dcr = 0; // :39
        self.m_rtcsr = 0; // :40
        self.m_rtcnt = 0; // :41
        self.m_rtcor = 0; // :42
    }

    /// origin: sh_bsc.cpp:45-49 etc. — every `*_r` is `logerror + return
    /// m_x`; logerror is stderr-only, dropped here (no state effect).
    pub fn bcr1_r(&self) -> u16 {
        self.m_bcr1
    }
    /// origin: sh_bsc.cpp:57-61 (bcr2_r).
    pub fn bcr2_r(&self) -> u16 {
        self.m_bcr2
    }
    /// origin: sh_bsc.cpp:69-73 (wcr1_r).
    pub fn wcr1_r(&self) -> u16 {
        self.m_wcr1
    }
    /// origin: sh_bsc.cpp:81-85 (wcr2_r).
    pub fn wcr2_r(&self) -> u16 {
        self.m_wcr2
    }
    /// origin: sh_bsc.cpp:93-97 (dcr_r).
    pub fn dcr_r(&self) -> u16 {
        self.m_dcr
    }
    /// origin: sh_bsc.cpp:105-109 (rtcsr_r).
    pub fn rtcsr_r(&self) -> u16 {
        self.m_rtcsr
    }
    /// origin: sh_bsc.cpp:117-121 (rtcnt_r). RTCNT is NOT a timer — no tick.
    pub fn rtcnt_r(&self) -> u16 {
        self.m_rtcnt
    }
    /// origin: sh_bsc.cpp:129-133 (rtcor_r).
    pub fn rtcor_r(&self) -> u16 {
        self.m_rtcor
    }

    /// origin: sh_bsc.cpp:51-55 etc. — every `*_w` is `COMBINE_DATA + logerror`.
    pub fn bcr1_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_bcr1, data, mem_mask); // :53
    }
    /// origin: sh_bsc.cpp:63-67 (bcr2_w).
    pub fn bcr2_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_bcr2, data, mem_mask); // :65
    }
    /// origin: sh_bsc.cpp:75-79 (wcr1_w).
    pub fn wcr1_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_wcr1, data, mem_mask); // :77
    }
    /// origin: sh_bsc.cpp:87-91 (wcr2_w).
    pub fn wcr2_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_wcr2, data, mem_mask); // :89
    }
    /// origin: sh_bsc.cpp:99-103 (dcr_w).
    pub fn dcr_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_dcr, data, mem_mask); // :101
    }
    /// origin: sh_bsc.cpp:111-115 (rtcsr_w). Disk does NOT implement the
    /// 16Hz RTCNT countdown — plain store.
    pub fn rtcsr_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_rtcsr, data, mem_mask); // :113
    }
    /// origin: sh_bsc.cpp:123-127 (rtcnt_w).
    pub fn rtcnt_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_rtcnt, data, mem_mask); // :125
    }
    /// origin: sh_bsc.cpp:135-139 (rtcor_w).
    pub fn rtcor_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_rtcor, data, mem_mask); // :137
    }

    /// origin: sh_bsc.cpp:141-146 (`sh_bsc_device::state`) — eight u16 regs
    /// (sh_bsc.h:46), disk order :144-145.
    pub fn state(&mut self, s: &mut smu_compat::StateIo) {
        s.tag("bsc");           // :143
        s.v(&mut self.m_bcr1);  // :144
        s.v(&mut self.m_bcr2);  // :144
        s.v(&mut self.m_wcr1);  // :144
        s.v(&mut self.m_wcr2);  // :144
        s.v(&mut self.m_dcr);   // :145
        s.v(&mut self.m_rtcsr); // :145
        s.v(&mut self.m_rtcnt); // :145
        s.v(&mut self.m_rtcor); // :145
    }
}

impl Sh7042Peripherals for ShBsc {
    /// origin: sh7042_map.hxx:238-253 (internal_r8, BSC arms). Byte at even
    /// addr = reg>>8, odd = reg>>0 (disk calls the full u16 accessor twice).
    /// The bus never routes 8628/8629 here (sh7042.rs:545 stops at 8627 and
    /// resumes 862a) — `_` is defense-in-depth and returns the membus miss 0.
    fn bsc_r8(&mut self, a: u32) -> u8 {
        match a {
            0xffff8620 => (self.bcr1_r() >> 8) as u8,  // map:238
            0xffff8621 => self.bcr1_r() as u8,         // map:239
            0xffff8622 => (self.bcr2_r() >> 8) as u8,  // map:240
            0xffff8623 => self.bcr2_r() as u8,         // map:241
            0xffff8624 => (self.wcr1_r() >> 8) as u8,  // map:242
            0xffff8625 => self.wcr1_r() as u8,         // map:243
            0xffff8626 => (self.wcr2_r() >> 8) as u8,  // map:244
            0xffff8627 => self.wcr2_r() as u8,         // map:245
            0xffff862a => (self.dcr_r() >> 8) as u8,   // map:246
            0xffff862b => self.dcr_r() as u8,          // map:247
            0xffff862c => (self.rtcsr_r() >> 8) as u8, // map:248
            0xffff862d => self.rtcsr_r() as u8,        // map:249
            0xffff862e => (self.rtcnt_r() >> 8) as u8, // map:250
            0xffff862f => self.rtcnt_r() as u8,        // map:251
            0xffff8630 => (self.rtcor_r() >> 8) as u8, // map:252
            0xffff8631 => self.rtcor_r() as u8,        // map:253
            _ => 0,
        }
    }

    /// origin: sh7042_map.hxx:439-446 (internal_r16). Note the set: 8620,
    /// 8622, 8624, 8626 then JUMP to 862a — no 8628 case on disk.
    fn bsc_r16(&mut self, a: u32) -> u16 {
        match a {
            0xffff8620 => self.bcr1_r(),  // map:439
            0xffff8622 => self.bcr2_r(),  // map:440
            0xffff8624 => self.wcr1_r(),  // map:441
            0xffff8626 => self.wcr2_r(),  // map:442
            0xffff862a => self.dcr_r(),   // map:443
            0xffff862c => self.rtcsr_r(), // map:444
            0xffff862e => self.rtcnt_r(), // map:445
            0xffff8630 => self.rtcor_r(), // map:446
            _ => 0,
        }
    }

    /// origin: sh7042_map.hxx:707-722 (internal_w8). Disk shifts the byte
    /// into position, mask 0xff<<shift — mirror verbatim.
    fn bsc_w8(&mut self, a: u32, v: u8) {
        let vv = v as u16;
        match a {
            0xffff8620 => self.bcr1_w(vv << 8, 0xff << 8),  // map:707
            0xffff8621 => self.bcr1_w(vv, 0xff),            // map:708
            0xffff8622 => self.bcr2_w(vv << 8, 0xff << 8),  // map:709
            0xffff8623 => self.bcr2_w(vv, 0xff),            // map:710
            0xffff8624 => self.wcr1_w(vv << 8, 0xff << 8),  // map:711
            0xffff8625 => self.wcr1_w(vv, 0xff),            // map:712
            0xffff8626 => self.wcr2_w(vv << 8, 0xff << 8),  // map:713
            0xffff8627 => self.wcr2_w(vv, 0xff),            // map:714
            0xffff862a => self.dcr_w(vv << 8, 0xff << 8),   // map:715
            0xffff862b => self.dcr_w(vv, 0xff),             // map:716
            0xffff862c => self.rtcsr_w(vv << 8, 0xff << 8), // map:717
            0xffff862d => self.rtcsr_w(vv, 0xff),           // map:718
            0xffff862e => self.rtcnt_w(vv << 8, 0xff << 8), // map:719
            0xffff862f => self.rtcnt_w(vv, 0xff),           // map:720
            0xffff8630 => self.rtcor_w(vv << 8, 0xff << 8), // map:721
            0xffff8631 => self.rtcor_w(vv, 0xff),           // map:722
            _ => {}
        }
    }

    /// origin: sh7042_map.hxx:890-897 (internal_w16), mask full.
    fn bsc_w16(&mut self, a: u32, v: u16) {
        match a {
            0xffff8620 => self.bcr1_w(v, 0xffff),  // map:890
            0xffff8622 => self.bcr2_w(v, 0xffff),  // map:891
            0xffff8624 => self.wcr1_w(v, 0xffff),  // map:892
            0xffff8626 => self.wcr2_w(v, 0xffff),  // map:893
            0xffff862a => self.dcr_w(v, 0xffff),   // map:894
            0xffff862c => self.rtcsr_w(v, 0xffff), // map:895
            0xffff862e => self.rtcnt_w(v, 0xffff), // map:896
            0xffff8630 => self.rtcor_w(v, 0xffff), // map:897
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// DMAC — origin: src/mame/cpu/sh_dmac.h:24-84, sh_dmac.cpp
// ---------------------------------------------------------------------------

/// One DMA channel (disk `sh_dmac_channel_device`, sh_dmac.cpp:46-115). The
/// four u32 regs are plain masked storage; NO transfer engine exists on disk.
/// All fields pub for the M5 state row (`state()` :123-127, tag "dmach").
pub struct ShDmacChannel {
    // origin: sh_dmac.h:80 — ctor inits all 0, device_reset (:61-67) re-zeroes.
    pub m_sar: u32,
    pub m_dar: u32,
    pub m_dmatcr: u32,
    pub m_chcr: u32,
}

impl ShDmacChannel {
    /// origin: sh_dmac.cpp:46-51 (ctor; m_cpu/m_intc are required_device
    /// back-refs NO handler ever uses) + device_start (:53-59, save_items) +
    /// device_reset (:61-67).
    pub fn new() -> Self {
        let mut d = Self {
            m_sar: 0,
            m_dar: 0,
            m_dmatcr: 0,
            m_chcr: 0,
        };
        d.device_reset();
        d
    }

    /// origin: sh_dmac.cpp:61-67 (device_reset).
    pub fn device_reset(&mut self) {
        self.m_sar = 0; // :63
        self.m_dar = 0; // :64
        self.m_dmatcr = 0; // :65
        self.m_chcr = 0; // :66
    }

    /// register slot inside the 16-byte channel block: +0 SAR, +4 DAR,
    /// +8 DMATCR, +c CHCR (map:256-319 byte arms / 448-479 word / 496-511).
    #[inline]
    fn reg(&self, idx: usize) -> u32 {
        match idx {
            0 => self.sar_r(), // map:256/272/288/304 group
            1 => self.dar_r(), // map:260/276/292/308 group
            2 => self.dmatcr_r(), // map:264/280/296/312 group
            _ => self.chcr_r(), // map:268/284/300/316 group
        }
    }

    #[inline]
    fn reg_mut(&mut self, idx: usize) -> &mut u32 {
        match idx {
            0 => &mut self.m_sar,
            1 => &mut self.m_dar,
            2 => &mut self.m_dmatcr,
            _ => &mut self.m_chcr,
        }
    }

    /// origin: sh_dmac.cpp:69-73 (sar_r) — store + logerror.
    pub fn sar_r(&self) -> u32 {
        self.m_sar
    }
    /// origin: sh_dmac.cpp:75-79 (sar_w) — COMBINE_DATA + logerror.
    pub fn sar_w(&mut self, data: u32, mem_mask: u32) {
        combine32(&mut self.m_sar, data, mem_mask); // :77
    }
    /// origin: sh_dmac.cpp:81-85 (dar_r).
    pub fn dar_r(&self) -> u32 {
        self.m_dar
    }
    /// origin: sh_dmac.cpp:87-91 (dar_w).
    pub fn dar_w(&mut self, data: u32, mem_mask: u32) {
        combine32(&mut self.m_dar, data, mem_mask); // :89
    }
    /// origin: sh_dmac.cpp:93-97 (dmatcr_r). No DMA transfer ever reads it.
    pub fn dmatcr_r(&self) -> u32 {
        self.m_dmatcr
    }
    /// origin: sh_dmac.cpp:99-103 (dmatcr_w).
    pub fn dmatcr_w(&mut self, data: u32, mem_mask: u32) {
        combine32(&mut self.m_dmatcr, data, mem_mask); // :101
    }
    /// origin: sh_dmac.cpp:105-109 (chcr_r). TE/DIE/TS bits start nothing.
    pub fn chcr_r(&self) -> u32 {
        self.m_chcr
    }
    /// origin: sh_dmac.cpp:111-115 (chcr_w).
    pub fn chcr_w(&mut self, data: u32, mem_mask: u32) {
        combine32(&mut self.m_chcr, data, mem_mask); // :113
    }

    /// origin: sh_dmac.cpp:123-127 (`sh_dmac_channel_device::state`); four u32
    /// (sh_dmac.h:80 — DMATCR is u32 on disk), order :126.
    pub fn state(&mut self, s: &mut smu_compat::StateIo) {
        s.tag("dmach");          // :125
        s.v(&mut self.m_sar);    // :126
        s.v(&mut self.m_dar);    // :126
        s.v(&mut self.m_dmatcr); // :126
        s.v(&mut self.m_chcr);   // :126
    }
}

/// DMAC shared block + the four channels the die wires as `dmac:0..3`
/// (sh7042.cpp:47-51 `m_dmac0..3`, :173-177). Disk models them as separate
/// devices; the sh7042.rs seam is one object with a `ch` index, so ShDmac
/// owns the array and dispatches (same composite trick as sci.rs Sh2SciPair).
pub struct ShDmac {
    // origin: sh_dmac.h:46 — m_dmaor = 0 at ctor, reset 0 (cpp:31).
    pub m_dmaor: u16,
    /// disk side: `SH_DMAC_CHANNEL(config, m_dmac0..3, *this, m_intc)`
    /// (sh7042.cpp:174-177); seams map `ch` 0..3 to these.
    pub channels: [ShDmacChannel; 4],
}

impl ShDmac {
    /// origin: sh_dmac.cpp:18-22 (ctor) + device_start (:24-27, save_items) +
    /// device_reset (:29-32) and the four channel ctors/resets.
    pub fn new() -> Self {
        let mut d = Self {
            m_dmaor: 0,
            channels: std::array::from_fn(|_| ShDmacChannel::new()),
        };
        d.device_reset();
        d
    }

    /// origin: sh_dmac.cpp:29-32 (dmaor reset 0) + :61-67 x4 (each channel
    /// device_reset; disk resets every device — sh7042.cpp machine reset).
    pub fn device_reset(&mut self) {
        self.m_dmaor = 0; // :31
        for c in self.channels.iter_mut() {
            c.device_reset();
        }
    }

    /// origin: sh_dmac.cpp:34-38 (dmaor_r).
    pub fn dmaor_r(&self) -> u16 {
        self.m_dmaor
    }
    /// origin: sh_dmac.cpp:40-44 (dmaor_w).
    pub fn dmaor_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_dmaor, data, mem_mask); // :42
    }

    /// origin: sh_dmac.cpp:117-121 (`sh_dmac_device::state`) — the shared
    /// DMAOR only (u16, sh_dmac.h:46); the channels ride separately
    /// (`ShDmacChannel::state`, sh7042.cpp:422).
    pub fn state(&mut self, s: &mut smu_compat::StateIo) {
        s.tag("dmac");        // :119
        s.v(&mut self.m_dmaor); // :120
    }
}

impl Sh7042Peripherals for ShDmac {
    /// origin: sh7042_map.hxx:254-255 (internal_r8, DMAOR arms).
    fn dmac_r8(&mut self, a: u32) -> u8 {
        match a {
            0xffff86b0 => (self.dmaor_r() >> 8) as u8, // map:254
            0xffff86b1 => self.dmaor_r() as u8,        // map:255
            _ => 0,
        }
    }

    /// origin: sh7042_map.hxx:447 (internal_r16, DMAOR).
    fn dmac_r16(&mut self, a: u32) -> u16 {
        match a {
            0xffff86b0 => self.dmaor_r(), // map:447
            _ => 0,
        }
    }

    /// origin: sh7042_map.hxx:723-724 (internal_w8, DMAOR).
    fn dmac_w8(&mut self, a: u32, v: u8) {
        let vv = v as u16;
        match a {
            0xffff86b0 => self.dmaor_w(vv << 8, 0xff << 8), // map:723
            0xffff86b1 => self.dmaor_w(vv, 0xff),           // map:724
            _ => {}
        }
    }

    /// origin: sh7042_map.hxx:898 (internal_w16, DMAOR).
    fn dmac_w16(&mut self, a: u32, v: u16) {
        match a {
            0xffff86b0 => self.dmaor_w(v, 0xffff), // map:898
            _ => {}
        }
    }

    // ---- channel block: base 0xffff86c0 + 16*ch (sh7042.rs:549-551
    // already computed ch and routes every byte, no holes in-block) ----

    /// origin: sh7042_map.hxx:256-319 (internal_r8). Byte j of register
    /// (a>>2)&3 is at shift 24-8*j; disk invokes the full u32 accessor per
    /// byte and shifts the result.
    fn dmac_ch_r8(&mut self, ch: usize, a: u32) -> u8 {
        match self.channels.get(ch) {
            Some(c) => {
                let reg = c.reg(((a >> 2) & 3) as usize);
                (reg >> (24 - 8 * (a & 3))) as u8
            }
            None => 0, // ch>3 cannot occur (bus range 86c0-86ff -> ch 0..=3)
        }
    }

    /// origin: sh7042_map.hxx:448-479 (internal_r16). High half at
    /// offset&2==0 (>>16), low half >>0 — the disk cases pair +0>>16/+2>>0 etc.
    fn dmac_ch_r16(&mut self, ch: usize, a: u32) -> u16 {
        match self.channels.get(ch) {
            Some(c) => {
                let reg = c.reg(((a >> 2) & 3) as usize);
                if a & 2 == 0 {
                    (reg >> 16) as u16
                } else {
                    reg as u16
                }
            }
            None => 0,
        }
    }

    /// origin: sh7042_map.hxx:496-511 (internal_r32) — DIRECT whole-u32
    /// accessors, only at block+0/+4/+8/+c (sh7042.rs:638 already enforces
    /// a&3==0 and routes exactly those).
    fn dmac_ch_r32(&mut self, ch: usize, a: u32) -> u32 {
        match self.channels.get(ch) {
            Some(c) => c.reg(((a >> 2) & 3) as usize),
            None => 0,
        }
    }

    /// origin: sh7042_map.hxx:725-788 (internal_w8): data=v<<(24-8*j),
    /// mask=0xff<<(24-8*j) per byte address.
    fn dmac_ch_w8(&mut self, ch: usize, a: u32, v: u8) {
        if let Some(c) = self.channels.get_mut(ch) {
            let shift = 24 - 8 * (a & 3);
            combine32(
                c.reg_mut(((a >> 2) & 3) as usize),
                (v as u32) << shift,
                0xff << shift,
            );
        }
    }

    /// origin: sh7042_map.hxx:899-930 (internal_w16): data=v<<16 mask
    /// 0xffff<<16 at block+0/4/8/c half, v<<0 mask 0xffff at +2/6/a/e half.
    fn dmac_ch_w16(&mut self, ch: usize, a: u32, v: u16) {
        if let Some(c) = self.channels.get_mut(ch) {
            let vv = v as u32;
            if a & 2 == 0 {
                combine32(
                    c.reg_mut(((a >> 2) & 3) as usize),
                    vv << 16,
                    0xffff << 16,
                );
            } else {
                combine32(c.reg_mut(((a >> 2) & 3) as usize), vv, 0xffff);
            }
        }
    }

    /// origin: sh7042_map.hxx:946-961 (internal_w32), mask full.
    fn dmac_ch_w32(&mut self, ch: usize, a: u32, v: u32) {
        if let Some(c) = self.channels.get_mut(ch) {
            // COMBINE_DATA with mask 0xffffffff == full overwrite; keep the
            // macro verbatim (transliterate-first).
            combine32(c.reg_mut(((a >> 2) & 3) as usize), v, 0xffff_ffff);
        }
    }
}
