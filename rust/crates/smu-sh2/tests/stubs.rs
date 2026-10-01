//! Origin-cited unit vectors for `periph/stubs.rs` (ledger row
//! `periph: bsc/dmac`). Ground truth = disk C++ `src/mame/cpu/sh_bsc.cpp`
//! (146) + `sh_bsc.h` (54) + `sh_dmac.cpp` (127) + `sh_dmac.h` (89) + map
//! `sh7042_map.hxx:238-319/439-479/496-511/707-788/890-961`.
//!
//! DISK NOTE: no DMA engine and no RTC countdown exist on disk — these
//! devices are masked-store registers + logerror. Tests therefore pin the
//! storage/reset/hole semantics, which IS the full disk behavior.

use smu_sh2::periph::stubs::{ShBsc, ShDmac, ShDmacChannel};
use smu_sh2::sh7042::Sh7042Peripherals;

// map bases (sh7042_map.hxx:238+ / 254+ / per-channel 16-byte blocks)
const BCR1: u32 = 0xffff8620;
const BCR2: u32 = 0xffff8622;
const WCR1: u32 = 0xffff8624;
const WCR2: u32 = 0xffff8626;
const DCR: u32 = 0xffff862a;
const RTCSR: u32 = 0xffff862c;
const RTCNT: u32 = 0xffff862e;
const RTCOR: u32 = 0xffff8630;
const DMAOR: u32 = 0xffff86b0;
fn ch_base(ch: usize) -> u32 {
    0xffff86c0 + 16 * ch as u32
} // map:256/272/288/304 (die dmac0..3, sh7042.cpp:174-177)

// ---- BSC ----

#[test]
fn bsc_reset_values_via_r16() {
    // sh_bsc.cpp:35-42 (device_reset), read back via map:439-446
    let mut b = ShBsc::new();
    assert_eq!(b.bsc_r16(BCR1), 0x200f); // :35
    assert_eq!(b.bsc_r16(BCR2), 0xffff); // :36
    assert_eq!(b.bsc_r16(WCR1), 0xffff); // :37
    assert_eq!(b.bsc_r16(WCR2), 0x000f); // :38
    assert_eq!(b.bsc_r16(DCR), 0); // :39
    assert_eq!(b.bsc_r16(RTCSR), 0); // :40
    assert_eq!(b.bsc_r16(RTCNT), 0); // :41
    assert_eq!(b.bsc_r16(RTCOR), 0); // :42
}

#[test]
fn bsc_r8_byte_split_all_addrs() {
    // map:238-253: even addr -> reg>>8, odd -> reg&0xff
    let mut b = ShBsc::new();
    let exp: [(u32, u8, u8); 8] = [
        (BCR1, 0x20, 0x0f),
        (BCR2, 0xff, 0xff),
        (WCR1, 0xff, 0xff),
        (WCR2, 0x00, 0x0f),
        (DCR, 0x00, 0x00),
        (RTCSR, 0x00, 0x00),
        (RTCNT, 0x00, 0x00),
        (RTCOR, 0x00, 0x00),
    ];
    for (a, hi, lo) in exp {
        assert_eq!(b.bsc_r8(a), hi, "hi @{a:#010x}"); // map:238/240/242/244/246/248/250/252
        assert_eq!(b.bsc_r8(a + 1), lo, "lo @{:#010x}", a + 1); // map:239..253
    }
}

#[test]
fn bsc_w16_write_readback_every_reg() {
    // map:890-897 write (mask 0xffff) -> map:439-446 read; disk stores all.
    let mut b = ShBsc::new();
    for a in [BCR1, BCR2, WCR1, WCR2, DCR, RTCSR, RTCNT, RTCOR] {
        b.bsc_w16(a, 0xa55a);
        assert_eq!(b.bsc_r16(a), 0xa55a);
        assert_eq!(b.bsc_r8(a), 0xa5);
        assert_eq!(b.bsc_r8(a + 1), 0x5a);
    }
}

#[test]
fn bsc_w8_partial_write_masks() {
    // map:707-708: w8 hi = data<<8 mask 0xff00 -> lo byte untouched.
    let mut b = ShBsc::new(); // bcr1 = 0x200f
    b.bsc_w8(BCR1, 0x12); // hi
    assert_eq!(b.bsc_r16(BCR1), 0x120f);
    b.bsc_w8(BCR1 + 1, 0x34); // lo
    assert_eq!(b.bsc_r16(BCR1), 0x1234);
}

#[test]
fn bsc_8628_hole_never_stores() {
    // disk map has NO r16/w16 case at 8628 (map:439-446 jumps 8626->862a).
    // Bus (sh7042.rs:616/849) drops it; seam defense-arm must too.
    let mut b = ShBsc::new();
    assert_eq!(b.bsc_r16(0xffff8628), 0);
    assert_eq!(b.bsc_r8(0xffff8628), 0);
    assert_eq!(b.bsc_r8(0xffff8629), 0);
    b.bsc_w16(0xffff8628, 0xbeef); // ignored
    b.bsc_w8(0xffff8628, 0xee); // ignored
    b.bsc_w8(0xffff8629, 0xef); // ignored
    assert_eq!(b.bsc_r16(BCR2), 0xffff); // neighbors intact
    assert_eq!(b.bsc_r16(DCR), 0);
}

#[test]
fn bsc_rtcsr_no_timer_no_tick() {
    // sh_bsc.cpp has NO config/update/timer — bit15 (16Hz) starts nothing;
    // RTCNT stays at the written value (ground truth: no counter logic).
    let mut b = ShBsc::new();
    b.bsc_w16(RTCNT, 0x1111);
    b.bsc_w16(RTCSR, 0x8000);
    assert_eq!(b.bsc_r16(RTCSR), 0x8000);
    assert_eq!(b.bsc_r16(RTCNT), 0x1111); // unchanged, no tick path exists
}

// ---- DMAC shared (DMAOR) ----

#[test]
fn dmaor_reset_zero_and_rwl() {
    // sh_dmac.cpp:31 reset 0; map:447/254-255 read; map:898/723-724 write.
    let mut d = ShDmac::new();
    assert_eq!(d.dmac_r16(DMAOR), 0);
    assert_eq!(d.dmac_r8(DMAOR), 0);
    assert_eq!(d.dmac_r8(DMAOR + 1), 0);
    d.dmac_w16(DMAOR, 0xabcd);
    assert_eq!(d.dmac_r16(DMAOR), 0xabcd);
    assert_eq!(d.dmac_r8(DMAOR), 0xab);
    assert_eq!(d.dmac_r8(DMAOR + 1), 0xcd);
    d.dmac_w8(DMAOR, 0x00); // map:723 hi only
    assert_eq!(d.dmac_r16(DMAOR), 0x00cd);
}

// ---- DMAC channels ----

#[test]
fn dmac_channel_reset_zeros_all_regs() {
    // sh_dmac.cpp:61-67 (device_reset).
    let c = ShDmacChannel::new();
    assert_eq!(c.m_sar, 0);
    assert_eq!(c.m_dar, 0);
    assert_eq!(c.m_dmatcr, 0);
    assert_eq!(c.m_chcr, 0);
}

#[test]
fn dmac_ch_w32_r32_all_channels_all_regs() {
    // map:946-961 (w32, mask full) + map:496-511 (r32 direct) at block
    // +0/+4/+8/+c, channels 0..3 (die dmac0..3 sh7042.cpp:174-177).
    let mut d = ShDmac::new();
    for ch in 0..4usize {
        for (off, pat) in [(0x0u32, 0x1111_0000u32 + ch as u32), (0x4, 0x2222_0000 + ch as u32), (0x8, 0x3333_0000 + ch as u32), (0xc, 0x4444_0000 + ch as u32)] {
            d.dmac_ch_w32(ch, ch_base(ch) + off, pat);
            assert_eq!(d.dmac_ch_r32(ch, ch_base(ch) + off), pat);
        }
    }
}

#[test]
fn dmac_ch_w8_each_byte_position() {
    // map:725-728 (sar): +0 v<<24, +1 v<<16, +2 v<<8, +3 v<<0.
    let mut d = ShDmac::new();
    let b = ch_base(0);
    d.dmac_ch_w8(0, b + 0, 0x11);
    d.dmac_ch_w8(0, b + 1, 0x22);
    d.dmac_ch_w8(0, b + 2, 0x33);
    d.dmac_ch_w8(0, b + 3, 0x44);
    assert_eq!(d.dmac_ch_r32(0, b), 0x1122_3344);
    // dar bytes map:729-732
    d.dmac_ch_w8(0, b + 4, 0xaa);
    d.dmac_ch_w8(0, b + 7, 0xbb);
    assert_eq!(d.dmac_ch_r32(0, b + 4), 0xaa00_00bb);
    // r8 split back (map:256-263)
    assert_eq!(d.dmac_ch_r8(0, b + 0), 0x11);
    assert_eq!(d.dmac_ch_r8(0, b + 1), 0x22);
    assert_eq!(d.dmac_ch_r8(0, b + 2), 0x33);
    assert_eq!(d.dmac_ch_r8(0, b + 3), 0x44);
    assert_eq!(d.dmac_ch_r8(0, b + 6), 0x00);
}

#[test]
fn dmac_ch_r16_half_selection() {
    // map:448-455: +0/+4/+8/+c read reg>>16, +2/+6/+a/+e read reg&0xffff.
    let mut d = ShDmac::new();
    let b = ch_base(2);
    d.dmac_ch_w32(2, b + 8, 0xdead_beef);
    assert_eq!(d.dmac_ch_r16(2, b + 8), 0xdead); // map:452
    assert_eq!(d.dmac_ch_r16(2, b + 0xa), 0xbeef); // map:453
    assert_eq!(d.dmac_ch_r8(2, b + 8), 0xde); // map:296
    assert_eq!(d.dmac_ch_r8(2, b + 0xb), 0xef); // map:299
}

#[test]
fn dmac_ch_w16_half_masks() {
    // map:899/901 (w16 high half <<16) vs :900/902 (low half).
    let mut d = ShDmac::new();
    let b = ch_base(1);
    d.dmac_ch_w16(1, b + 0, 0x1234); // sar high half
    assert_eq!(d.dmac_ch_r32(1, b), 0x1234_0000);
    d.dmac_ch_w16(1, b + 2, 0x5678); // sar low half
    assert_eq!(d.dmac_ch_r32(1, b), 0x1234_5678);
    d.dmac_ch_w16(1, b + 0xe, 0xabcd); // chcr low half (map:930)
    assert_eq!(d.dmac_ch_r32(1, b + 0xc), 0x0000_abcd);
}

#[test]
fn dmac_channels_are_independent() {
    // four SEPARATE disk devices (sh7042.cpp:48-51) — ch-block decode
    // ((a-0x86c0)>>4, sh7042.rs:550) must not alias.
    let mut d = ShDmac::new();
    d.dmac_ch_w32(1, ch_base(1) + 0, 0xcafe_babe);
    for ch in [0usize, 2, 3] {
        assert_eq!(d.dmac_ch_r32(ch, ch_base(ch)), 0);
    }
    assert_eq!(d.dmac_ch_r32(1, ch_base(1)), 0xcafe_babe);
}

#[test]
fn dmac_ch_full_block_sweep() {
    // every byte address of channel 2's block: w8 then r8/r16/r32 agree
    // (map:304-319 equivalents at 86e0-86ef; no holes in-block on disk).
    let mut d = ShDmac::new();
    let b = ch_base(2);
    for i in 0..16u32 {
        d.dmac_ch_w8(2, b + i, 0x80 + i as u8);
    }
    for i in 0..16u32 {
        assert_eq!(d.dmac_ch_r8(2, b + i), 0x80 + i as u8);
    }
    for i in (0..16u32).step_by(2) {
        let hi = 0x80 + i as u16;
        let lo = 0x81 + i as u16;
        assert_eq!(d.dmac_ch_r16(2, b + i), (hi << 8) | lo);
    }
    assert_eq!(d.dmac_ch_r32(2, b + 0), 0x8081_8283);
    assert_eq!(d.dmac_ch_r32(2, b + 4), 0x8485_8687);
    assert_eq!(d.dmac_ch_r32(2, b + 8), 0x8889_8a8b);
    assert_eq!(d.dmac_ch_r32(2, b + 0xc), 0x8c8d_8e8f);
}

#[test]
fn device_reset_restores_defaults() {
    // sh_bsc.cpp:33-43 + sh_dmac.cpp:29-32,61-67 — reset after dirt.
    let mut b = ShBsc::new();
    b.bsc_w16(BCR1, 0x0000);
    b.bsc_w16(RTCOR, 0x1234);
    b.device_reset();
    assert_eq!(b.bsc_r16(BCR1), 0x200f);
    assert_eq!(b.bsc_r16(RTCOR), 0);
    let mut d = ShDmac::new();
    d.dmac_w16(DMAOR, 0xffff);
    d.dmac_ch_w32(3, ch_base(3) + 0xc, 0xffff_ffff);
    d.device_reset();
    assert_eq!(d.dmac_r16(DMAOR), 0);
    assert_eq!(d.dmac_ch_r32(3, ch_base(3) + 0xc), 0);
}
