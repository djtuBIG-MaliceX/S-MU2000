//! SH7042/SH7043 SoC layer — transliteration of `src/mame/cpu/sh7042.cpp` +
//! `sh7042.h` and the address decode that the disk splits between
//! `src/mame/cpu/sh7042_map.hxx` (internal registers) and
//! `src/mu2000.cpp::build_bus` (top-level spaces). Disk OVERRIDES the
//! orchestrator's "expected" note: DRAM, IRAM, D80 and SmartMedia are also
//! decoded (mu2000.cpp:825-828, 939-963); the internal space is
//! 0xffff8000-0xffff9fff (mu2000.cpp:984-995), not "cache regs".
//!
//! DESIGN (recorded per ledger Invariant 2):
//! - Split ownership: `Sh7042 { dev, bus, ... }`. `Sh7042Bus` owns ROM/RAM/
//!   DRAM/IRAM + pcf_* + the `Option<Box<dyn Sh7042Peripherals>>` seam and IS
//!   the mem_bus port (membus.h). `Sh2Bus for Sh7042` forwards to `bus`
//!   (mu2000.cpp:997 `set_program_bus(&m_bus)` glue). The split lets
//!   `device_reset` hand the bus to `Sh2Device::device_reset` with disjoint
//!   field borrows (C++ passes the machine-supplied `mem_bus` ref, so the
//!   device never aliases its own fields — faithful).
//! - Peripheral seam: trait objects, one read/write per width PER MAP SPACE,
//!   handlers receive the ABSOLUTE address exactly like `mem_bus::device`
//!   ("周辺のハンドラには絶対番地がそのまま渡る", membus.h:36). Absent
//!   (`None`) peripheral: reads 0 / writes dropped — mirrors the mem_bus
//!   miss default (membus.h:70,83,96 read->0; :101-140 fall through = drop).
//!   MAME's `logerror` lines (map defaults :321/481/790/932, sh7042.cpp:90-115)
//!   are stderr-only diagnostics with no machine state: NOT emitted (same
//!   deviation as the M1 bus row).
//! - Width fallback (missing-width composition) lives in the bus methods
//!   VERBATIM per membus.h:61-140; the internal space registers all six
//!   widths on the bus device (mu2000.cpp:988-993) so its composition rules
//!   instead live inside `internal_r32`/`internal_w32` (map:514, 963-965).
//! - DEVIATION (documented, unreachable by firmware): C++ indexes the program
//!   ROM vector past its end (UB) if the loader ever attached a short vector;
//!   Rust returns 0 there.

use crate::core::Sh2Bus;
use crate::device::Sh2Device;
// origin: src/state.h — layout engine (smu_compat re-export, M5-W1)
use smu_compat::StateIo;

// origin: src/mame/cpu/sh.h:54 (#define CPU_TYPE_SH2 (1)) — passed at sh7042.cpp:41
pub const CPU_TYPE_SH2: i32 = 1;

// ---------------------------------------------------------------------------
// Peripheral seam — un-ported devices (ledger rows periph:* and machine
// wiring) implement this. All addresses are ABSOLUTE, as on the C++ bus
// (membus.h:36). Unoverridden methods return 0 / do nothing: that IS the
// mem_bus miss behavior (membus.h:70/83/96 -> 0, writes drop).
// ---------------------------------------------------------------------------
#[allow(unused_variables)]
pub trait Sh7042Peripherals {
    /// Downcast seam for machine-level handle swaps over the boxed arena
    /// (W-SAMP1 `Machine::share_card_from`): the concrete owner (smu-machine
    /// `Hub`) returns itself; plain impls return None.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }

    // ---- internal SCI0/1, 0xffff81a0-81a5 / 81b0-81b5 (map:15-26) ----
    fn sci_r8(&mut self, sci: usize, a: u32) -> u8 {
        0
    } // map:15-20 (smr/brr/scr/tdr/ssr/rdr_r)
    fn sci_r16(&mut self, sci: usize, a: u32) -> u16 {
        0
    } // map:328-333 (byte pairs)
    fn sci_w8(&mut self, sci: usize, a: u32, v: u8) {} // map:520-525; NO 81a5/81b5 (RDR ro)
    fn sci_w16(&mut self, sci: usize, a: u32, v: u16) {} // map w16 81a0/81a2/81a4

    // ---- internal MTU (shared + channels 0..4 interleaved), 8200-82ab ----
    fn mtu_r8(&mut self, a: u32) -> u8 {
        0
    } // map:27-74+ (mtu3/mtu4/mtu/mtu0/1/2 interleaved)
    fn mtu_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn mtu_w8(&mut self, a: u32, v: u8) {}
    fn mtu_w16(&mut self, a: u32, v: u16) {}

    // ---- INTC 8348-835b ----
    fn intc_r8(&mut self, a: u32) -> u8 {
        0
    }
    fn intc_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn intc_w8(&mut self, a: u32, v: u8) {}
    fn intc_w16(&mut self, a: u32, v: u16) {}

    /// origin: src/mame/cpu/sh7042.cpp:141-144 (execute_set_input -> m_intc->set_input)
    fn intc_set_input(&mut self, irqline: i32, state: i8) {}
    /// origin: src/mame/cpu/sh7042.cpp:397-401 (sh2_exception_internal -> m_intc->interrupt_taken)
    fn intc_interrupt_taken(&mut self, irqline: i32, vector: u32) {}

    // ---- CMT 83d0-83dd ----
    fn cmt_r8(&mut self, a: u32) -> u8 {
        0
    }
    fn cmt_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn cmt_w8(&mut self, a: u32, v: u8) {}
    fn cmt_w16(&mut self, a: u32, v: u16) {}

    // ---- BSC 8620-8627 / 862a-8631 (w16/r16 skip the 8628 hole) ----
    fn bsc_r8(&mut self, a: u32) -> u8 {
        0
    }
    fn bsc_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn bsc_w8(&mut self, a: u32, v: u8) {}
    fn bsc_w16(&mut self, a: u32, v: u16) {}

    // ---- DMAC shared 86b0-86b1; channels 0..3 at 86c0+16*ch..+15 ----
    fn dmac_r8(&mut self, a: u32) -> u8 {
        0
    }
    fn dmac_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn dmac_w8(&mut self, a: u32, v: u8) {}
    fn dmac_w16(&mut self, a: u32, v: u16) {}
    fn dmac_ch_r8(&mut self, ch: usize, a: u32) -> u8 {
        0
    }
    fn dmac_ch_r16(&mut self, ch: usize, a: u32) -> u16 {
        0
    }
    fn dmac_ch_r32(&mut self, ch: usize, a: u32) -> u32 {
        0
    } // map:496-511 (sar/dar/dmatcr/chcr)
    fn dmac_ch_w8(&mut self, ch: usize, a: u32, v: u8) {}
    fn dmac_ch_w16(&mut self, ch: usize, a: u32, v: u16) {}
    fn dmac_ch_w32(&mut self, ch: usize, a: u32, v: u32) {} // map:946-961

    // ---- PORT A / D (32-bit): dr 8380/83a0, io 8384/83a4 runs ----
    fn porta_r8(&mut self, a: u32) -> u8 {
        0
    }
    fn porta_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn porta_r32(&mut self, a: u32) -> u32 {
        0
    } // map:488-489 (dr_r/io_r)
    fn porta_w8(&mut self, a: u32, v: u8) {}
    fn porta_w16(&mut self, a: u32, v: u16) {}
    fn porta_w32(&mut self, a: u32, v: u32) {} // map:938-939
    fn portd_r8(&mut self, a: u32) -> u8 {
        0
    }
    fn portd_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn portd_r32(&mut self, a: u32) -> u32 {
        0
    } // map:492-493
    fn portd_w8(&mut self, a: u32, v: u8) {}
    fn portd_w16(&mut self, a: u32, v: u16) {}
    fn portd_w32(&mut self, a: u32, v: u32) {} // map:942-943

    // ---- PORT B/C/E (16-bit), F read-only (no portf write in the map) ----
    fn portb_r8(&mut self, a: u32) -> u8 {
        0
    }
    fn portb_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn portb_w8(&mut self, a: u32, v: u8) {}
    fn portb_w16(&mut self, a: u32, v: u16) {}
    fn portc_r8(&mut self, a: u32) -> u8 {
        0
    }
    fn portc_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn portc_w8(&mut self, a: u32, v: u8) {}
    fn portc_w16(&mut self, a: u32, v: u16) {}
    fn porte_r8(&mut self, a: u32) -> u8 {
        0
    } // the LCD/hd44780 attach seam: mu2000.cpp:1001-1024 drives the LCD
      // through PORT E bits — the port row (periph:port) wires it.
    fn porte_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn porte_w8(&mut self, a: u32, v: u8) {}
    fn porte_w16(&mut self, a: u32, v: u16) {}
    fn portf_r8(&mut self, a: u32) -> u8 {
        0
    } // map r8 83b2-83b3 only — PORTF has NO write handler on disk
    fn portf_r16(&mut self, a: u32) -> u16 {
        0
    } // map r16 83b2; no w16/w32 on disk

    // ---- ADC0; ADC1 present only on die-A (sh7042.cpp:166-170) ----
    fn adc0_r8(&mut self, a: u32) -> u8 {
        0
    } // map: 83e0-83e1,83f0-8407,8410,8412
    fn adc0_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn adc0_w8(&mut self, a: u32, v: u8) {} // map w8: 83e0-83e1,8410,8412 only
    fn adc0_w16(&mut self, a: u32, v: u16) {} // map w16: 83e0,8410,8412 only
    fn adc1_r8(&mut self, a: u32) -> u8 {
        0
    } // map: 8408-840f,8411,8413
    fn adc1_r16(&mut self, a: u32) -> u16 {
        0
    }
    fn adc1_w8(&mut self, a: u32, v: u8) {} // map w8: 8411,8413 ONLY
                                            // (adc1 has NO w16/w32 on disk)

    // ---- scheduler seams: per-device internal_update, returns next event
    // cycle (0 = none). Call ORDER fixed by sh7042.cpp:282-292 ----
    fn adc0_update(&mut self, current_time: u64) -> u64 {
        0
    } // sh7042.cpp:282
    fn adc1_update(&mut self, current_time: u64) -> u64 {
        0
    } // sh7042.cpp:283-284 (only when die_a — m_adc1 optional)
    fn cmt_update(&mut self, current_time: u64) -> u64 {
        0
    } // sh7042.cpp:285
    fn mtu_ch_update(&mut self, ch: usize, current_time: u64) -> u64 {
        0
    } // sh7042.cpp:286-290 (mtu0..mtu4)
    fn sci_update(&mut self, sci: usize, current_time: u64) -> u64 {
        0
    } // sh7042.cpp:291-292 (sci0, sci1)

    // ---- M5-W2 state seams (sh7042.cpp:412-430). `Sh7042::state` calls these
    // in the EXACT disk order; each device emits its own tag. Defaults are
    // silent (no bytes): a fake/test periph that does not carry the device
    // contributes nothing — the real Hub always routes to the real devices.
    fn intc_state(&mut self, _s: &mut StateIo) {} // sh7042.cpp:412
    fn adc0_state(&mut self, _s: &mut StateIo) {} // :413
    fn adc1_state(&mut self, _s: &mut StateIo) {} // :417-418 (die-A, version>=8)
    fn bsc_state(&mut self, _s: &mut StateIo) {} // :419
    fn cmt_state(&mut self, _s: &mut StateIo) {} // :420
    fn dmac_state(&mut self, _s: &mut StateIo) {} // :421 (shared DMAOR)
    fn dmac_ch_state(&mut self, _ch: usize, _s: &mut StateIo) {} // :422 ch0..3
    fn mtu_state(&mut self, _s: &mut StateIo) {} // :423 (shared TSTR..)
    fn mtu_ch_state(&mut self, _ch: usize, _s: &mut StateIo) {} // :424-425 ch0..4
    /// porta..portf, index 0..5 (sh7042.cpp:426-427). Each port knows its own
    /// width/tag: a/d are sh_port32 ("port32"), b/c/e/f are sh_port16
    /// ("port16") — sh_port.cpp:124-134, creation sh7042.cpp:231-236.
    fn port_state(&mut self, _port: usize, _s: &mut StateIo) {}
    fn sci_state(&mut self, _sci: usize, _s: &mut StateIo) {} // :428-430 (m_sci[i].lookup())

    // ---- bus SWP30 windows (handlers get absolute addr; reg == (a-base)>>1).
    // NO swp r32 exists on disk (mu2000.cpp:858-918 registers r8/r16/w8/w16/w32) ----
    fn swp_r8(&mut self, master: bool, a: u32) -> u8 {
        0
    } // mu2000.cpp:877-880
    fn swp_r16(&mut self, master: bool, a: u32) -> u16 {
        0
    } // mu2000.cpp:861-866
    fn swp_w8(&mut self, master: bool, a: u32, v: u8) {} // mu2000.cpp:869-876 (16-bit reg RMW, lane=(a&1))
    fn swp_w16(&mut self, master: bool, a: u32, v: u16) {} // mu2000.cpp:906-918
    fn swp_w32(&mut self, master: bool, a: u32, v: u32) {} // mu2000.cpp:881-905 (two 16-bit writes)

    // ---- SCI4 0xf00000-0xf0003f (8-bit only, mu2000.cpp:965-972) ----
    fn sci4_r8(&mut self, a: u32) -> u8 {
        0
    } // handler gets OFFSET a-0xf00000 (mu2000.cpp:969 subtracts)
    fn sci4_w8(&mut self, a: u32, v: u8) {} // offset too (mu2000.cpp:970)

    // ---- USB M37640 0xf80000/0xf80001 (8-bit only, mu2000.cpp:974-982):
    // sel 0 = RX byte / MIDI write, sel 1 = status / command ----
    fn usb_r8(&mut self, sel: u32) -> u8 {
        0
    } // mu2000.cpp:979 (usb_r(a-0xf80000))
    fn usb_w8(&mut self, sel: u32, v: u8) {} // mu2000.cpp:980

    // ---- LED latch + switch scan: single-byte devices ----
    fn ledsw_r8(&mut self) -> u8 {
        0
    } // mu2000.cpp:929 (ledsw_r())
    fn ledsw1_w8(&mut self, v: u8) {} // mu2000.cpp:930 (m_ledsw1 = v)
    fn ledsw2_w8(&mut self, v: u8) {} // mu2000.cpp:936 (m_ledsw2 = v; NO read handler — mu2000.cpp:933-937)

    // ---- D80 contrast/misc latch 0xd80000 (mu2000.cpp:939-947) ----
    fn d80_r8(&mut self) -> u8 {
        0
    } // mu2000.cpp:942 (m_d80)
    fn d80_w8(&mut self, v: u8) {} // mu2000.cpp:945

    // ---- SmartMedia: data 0xc00000-0xc7ffff, control 0xd00000-0xd7ffff
    // (mu2000.cpp:949-963). Control READ is a hardcoded 0xff — owned by the
    // bus, NOT this trait (mu2000.cpp:960 `return 0xff`) ----
    fn card_data_r8(&mut self) -> u8 {
        0
    } // mu2000.cpp:953 (m_card.data_r())
    fn card_data_w8(&mut self, v: u8) {} // mu2000.cpp:954
    fn card_ctrl_w8(&mut self, v: u8) {} // mu2000.cpp:961
}

// ---------------------------------------------------------------------------
// COMBINE_DATA (as used at sh7042.cpp:309 etc.) — bits SET in the mask take
// the new value.
// ---------------------------------------------------------------------------
#[inline]
fn combine16(cur: &mut u16, data: u16, mask: u16) {
    *cur = (*cur & !mask) | (data & mask);
}
#[inline]
fn combine32(cur: &mut u32, data: u32, mask: u32) {
    *cur = (*cur & !mask) | (data & mask);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dev {
    Swpm,
    Swps,
    Led,
    Panel,
    D80,
    CardData,
    CardCtrl,
    Sci4,
    Usb,
    Internal,
}

// ---------------------------------------------------------------------------
// The bus (mem_bus port): regions + device spaces + internal register map.
// ---------------------------------------------------------------------------
pub struct Sh7042Bus {
    // origin: src/mu2000.cpp:740 add_region(0x000000,0x3fffff, m_prog, false) —
    // present only when m_prog non-empty (mu2000.cpp:718/740). Sized 4MB by the
    // loader (read_file exact 0x400000, mu2000.cpp:379); DEVIATION: C++ indexes
    // a short vector out-of-bounds (UB) — Rust returns 0 past the end.
    pub rom: Vec<u8>,
    // origin: src/mu2000.cpp:823 add_region(0x400000,0x43ffff, m_ram, true);
    // RAM init: mu2000.cpp:85 m_ram.assign(0x40000, 0) — zero-filled (Invariant 3)
    pub ram: Box<[u8; 0x40000]>,
    // origin: src/mu2000.cpp:826 add_region(0x1000000,0x107ffff, m_dram, true);
    // init mu2000.cpp:86 m_dram.assign(0x80000, 0)
    pub dram: Box<[u8; 0x80000]>,
    // origin: src/mu2000.cpp:828 add_region(0xfffff000,0xffffffff, m_iram, true);
    // init mu2000.cpp:87 m_iram.assign(0x1000, 0). NOTE: this — not 0xffff8000 —
    // is the disk's "CPU internal RAM" region; 0xffff8000-9fff is the register
    // DEVICE space (mu2000.cpp:984-995), fully decoded below.
    pub iram: Box<[u8; 0x1000]>,
    // origin: src/mame/cpu/sh7042.h:176-183 — pcf_* pin-control regs; explicit
    // zeros at device_start (sh7042.cpp:131-138) mirrored (ctor + start).
    pub m_pcf_ah: u16,
    pub m_pcf_al: u32,
    pub m_pcf_b: u32,
    pub m_pcf_c: u16,
    pub m_pcf_dh: u32,
    pub m_pcf_dl: u16,
    pub m_pcf_e: u32,
    pub m_pcf_if: u16,
    // un-ported peripheral seam (None until the wiring row attaches a machine)
    pub periph: Option<Box<dyn Sh7042Peripherals>>,
}

impl Sh7042Bus {
    /// Every field explicitly initialized (ledger Invariant 3 — no Default).
    // origin: sh7042.cpp:38-85 (ctor) + device_start zeros sh7042.cpp:131-138 +
    // mu2000.cpp:85-87 (RAM arrays) + mu2000.cpp:718-741 (ROM attach)
    pub fn new(rom: Vec<u8>) -> Self {
        Sh7042Bus {
            rom,
            ram: Box::new([0u8; 0x40000]),  // mu2000.cpp:85 assign(...,0)
            dram: Box::new([0u8; 0x80000]), // mu2000.cpp:86 assign(...,0)
            iram: Box::new([0u8; 0x1000]),  // mu2000.cpp:87 assign(...,0)
            m_pcf_ah: 0,
            m_pcf_al: 0,
            m_pcf_b: 0,
            m_pcf_c: 0,
            m_pcf_dh: 0,
            m_pcf_dl: 0,
            m_pcf_e: 0,
            m_pcf_if: 0,
            periph: None, // required_device wiring deferred to the machine row
        }
    }

    #[inline]
    fn rom_present(&self) -> bool {
        !self.rom.is_empty() // mu2000.cpp:718/740 `m_prog && !m_prog->empty()`
    }

    // ---- mem_bus hot fast path (membus.h:181-195). hot_r = the start-0 ROM
    // region, hot_w = the first writable region (RAM) ----
    #[inline]
    fn fast_read(&self, a: u32) -> Option<u8> {
        if self.rom_present() && a <= 0x3fffff {
            // membus.h:183-184 `a <= m_hot_r_end`; get() = stand-in for the
            // documented short-vector UB
            Some(self.rom.get(a as usize).copied().unwrap_or(0))
        } else if a.wrapping_sub(0x400000) <= 0x3ffff {
            // membus.h:185-186 `u32(a - hot_w_start) <= hot_w_len`
            // (hot_w_len = 0x43ffff-0x400000 = 0x3ffff); wrapping mirrors u32
            Some(self.ram[a.wrapping_sub(0x400000) as usize])
        } else {
            None
        }
    }

    // ---- mem_bus::find_read region scan, add order ROM,RAM,DRAM,IRAM
    // (membus.h:197-202 inclusive end; mu2000.cpp:740/823/826/828 order) ----
    fn find_read(&self, a: u32) -> Option<u8> {
        if self.rom_present() && a <= 0x3fffff {
            Some(self.rom.get(a as usize).copied().unwrap_or(0))
        } else if (0x400000..=0x43ffff).contains(&a) {
            Some(self.ram[(a - 0x400000) as usize])
        } else if (0x1000000..=0x107ffff).contains(&a) {
            Some(self.dram[(a - 0x1000000) as usize])
        } else if a >= 0xfffff000 {
            Some(self.iram[(a - 0xfffff000) as usize])
        } else {
            None
        }
    }

    /// hot-or-region byte (membus.h:63-64 / :76-77 / :89-92 flat pointers).
    /// BE assembly for aligned accesses never straddles a region end here
    /// (all regions are even-sized except the ROM's odd 0x3fffff end, which
    /// only an unaligned word could straddle — SH-2 never issues those).
    #[inline]
    fn region_byte(&self, a: u32) -> Option<u8> {
        self.fast_read(a).or_else(|| self.find_read(a))
    }
    #[inline]
    fn region_word(&self, a: u32) -> Option<u16> {
        // membus.h:76-77: p[0]<<8|p[1] from the SAME region pointer
        let b0 = self.region_byte(a)?;
        let b1 = self.region_byte(a + 1)?;
        Some((u16::from(b0) << 8) | u16::from(b1))
    }
    #[inline]
    fn region_long(&self, a: u32) -> Option<u32> {
        // membus.h:89-92
        let b0 = u32::from(self.region_byte(a)?);
        let b1 = u32::from(self.region_byte(a + 1)?);
        let b2 = u32::from(self.region_byte(a + 2)?);
        let b3 = u32::from(self.region_byte(a + 3)?);
        Some((b0 << 24) | (b1 << 16) | (b2 << 8) | b3)
    }

    // ---- mem_bus::find_write: WRITABLE regions only (membus.h:204-209) —
    // a write to the read-only ROM region therefore DROPS (membus.h:101-110
    // falls off the end). RAM/DRAM/IRAM writable. ----
    fn write_byte_region(&mut self, a: u32, v: u8) -> bool {
        if (0x400000..=0x43ffff).contains(&a) {
            self.ram[(a - 0x400000) as usize] = v; // membus.h:103-104
            true
        } else if (0x1000000..=0x107ffff).contains(&a) {
            self.dram[(a - 0x1000000) as usize] = v;
            true
        } else if a >= 0xfffff000 {
            self.iram[(a - 0xfffff000) as usize] = v;
            true
        } else {
            false // ROM hit (read-only, membus.h:207 skips) or unmapped
        }
    }

    // ---- device-space hit (membus.h:211-216, FIRST match in add order,
    // inclusive). Ranges verbatim mu2000.cpp ----
    fn find_dev(&self, a: u32) -> Option<Dev> {
        if (0x800000..=0x801fff).contains(&a) {
            // mu2000.cpp:858-860 d.start=base d.end=base+0x1fff; :921 base 0x800000
            Some(Dev::Swpm)
        } else if (0x802000..=0x803fff).contains(&a) {
            // mu2000.cpp:922 base 0x802000
            Some(Dev::Swps)
        } else if a == 0xc80000 {
            // mu2000.cpp:928 d.start = d.end = 0xc80000 (single byte)
            Some(Dev::Led)
        } else if a == 0xe00000 {
            // mu2000.cpp:935 single byte, WRITE ONLY (no r8/r16/r32 registered)
            Some(Dev::Panel)
        } else if a == 0xd80000 {
            // mu2000.cpp:941
            Some(Dev::D80)
        } else if (0xc00000..=0xc7ffff).contains(&a) {
            // mu2000.cpp:952
            Some(Dev::CardData)
        } else if (0xd00000..=0xd7ffff).contains(&a) {
            // mu2000.cpp:959
            Some(Dev::CardCtrl)
        } else if (0xf00000..=0xf0003f).contains(&a) {
            // mu2000.cpp:968
            Some(Dev::Sci4)
        } else if a == 0xf80000 || a == 0xf80001 {
            // mu2000.cpp:978 d.start=0xf80000 d.end=0xf80001
            Some(Dev::Usb)
        } else if (0xffff8000..=0xffff9fff).contains(&a) {
            // mu2000.cpp:987 internal register space
            Some(Dev::Internal)
        } else {
            None
        }
    }

    // =====================================================================
    // internal register dispatch — sh7042_map.hxx transliteration. The arms
    // are the exact per-device case sets of the generated switches (verified
    // by extracting every `case:` label into address runs).
    // =====================================================================

    // origin: src/mame/cpu/sh7042_map.hxx:12-323 (internal_r8)
    pub fn internal_r8(&mut self, a: u32) -> u8 {
        match a {
            // map:15-26 sci0 81a0-81a5 / sci1 81b0-81b5
            0xffff81a0..=0xffff81a5 => self.periph.as_mut().map_or(0, |x| x.sci_r8(0, a)),
            0xffff81b0..=0xffff81b5 => self.periph.as_mut().map_or(0, |x| x.sci_r8(1, a)),
            // map:27-~100 mtu family (mtu3/mtu4/mtu/mtu0/1/2 interleaved)
            0xffff8200..=0xffff820b
            | 0xffff820d
            | 0xffff8210..=0xffff822d
            | 0xffff8240..=0xffff8241
            | 0xffff8260..=0xffff826f
            | 0xffff8280..=0xffff828b
            | 0xffff82a0..=0xffff82ab => self.periph.as_mut().map_or(0, |x| x.mtu_r8(a)),
            // intc 8348-835b
            0xffff8348..=0xffff835b => self.periph.as_mut().map_or(0, |x| x.intc_r8(a)),
            // porta dr/io 8380-8387
            0xffff8380..=0xffff8387 => self.periph.as_mut().map_or(0, |x| x.porta_r8(a)),
            // pcf_* owned bytes (map:140-185)
            0xffff8388 => (self.pcf_ah_r() >> 8) as u8,  // map:140
            0xffff8389 => self.pcf_ah_r() as u8,         // map:141
            0xffff838c => (self.pcf_al_r() >> 24) as u8, // map:142
            0xffff838d => (self.pcf_al_r() >> 16) as u8, // map:143
            0xffff838e => (self.pcf_al_r() >> 8) as u8,  // map:144
            0xffff838f => self.pcf_al_r() as u8,         // map:145
            // portb 8390-8391,8394-8395 / portc 8392-8393,8396-8397
            0xffff8390..=0xffff8391 | 0xffff8394..=0xffff8395 => {
                self.periph.as_mut().map_or(0, |x| x.portb_r8(a))
            }
            0xffff8392..=0xffff8393 | 0xffff8396..=0xffff8397 => {
                self.periph.as_mut().map_or(0, |x| x.portc_r8(a))
            }
            // pcf_b 8398-839b, pcf_c 839c-839d (map:154-159)
            0xffff8398 => (self.pcf_b_r() >> 24) as u8,
            0xffff8399 => (self.pcf_b_r() >> 16) as u8,
            0xffff839a => (self.pcf_b_r() >> 8) as u8,
            0xffff839b => self.pcf_b_r() as u8,
            0xffff839c => (self.pcf_c_r() >> 8) as u8,
            0xffff839d => self.pcf_c_r() as u8,
            // portd 83a0-83a7
            0xffff83a0..=0xffff83a7 => self.periph.as_mut().map_or(0, |x| x.portd_r8(a)),
            // pcf_dh 83a8-83ab, pcf_dl 83ac-83ad (map:168-173)
            0xffff83a8 => (self.pcf_dh_r() >> 24) as u8,
            0xffff83a9 => (self.pcf_dh_r() >> 16) as u8,
            0xffff83aa => (self.pcf_dh_r() >> 8) as u8,
            0xffff83ab => self.pcf_dh_r() as u8,
            0xffff83ac => (self.pcf_dl_r() >> 8) as u8,
            0xffff83ad => self.pcf_dl_r() as u8,
            // porte 83b0-83b1,83b4-83b5 / portf 83b2-83b3 (READ ONLY)
            0xffff83b0..=0xffff83b1 | 0xffff83b4..=0xffff83b5 => {
                self.periph.as_mut().map_or(0, |x| x.porte_r8(a))
            }
            0xffff83b2..=0xffff83b3 => self.periph.as_mut().map_or(0, |x| x.portf_r8(a)),
            // pcf_e 83b8-83bb, pcf_if 83c8-83c9 (map:180-185)
            0xffff83b8 => (self.pcf_e_r() >> 24) as u8,
            0xffff83b9 => (self.pcf_e_r() >> 16) as u8,
            0xffff83ba => (self.pcf_e_r() >> 8) as u8,
            0xffff83bb => self.pcf_e_r() as u8,
            0xffff83c8 => (self.pcf_if_r() >> 8) as u8,
            0xffff83c9 => self.pcf_if_r() as u8,
            // cmt 83d0-83dd
            0xffff83d0..=0xffff83dd => self.periph.as_mut().map_or(0, |x| x.cmt_r8(a)),
            // adc0 83e0-83e1,83f0-8407,8410,8412 / adc1 8408-840f,8411,8413
            0xffff83e0..=0xffff83e1 | 0xffff83f0..=0xffff8407 | 0xffff8410 | 0xffff8412 => {
                self.periph.as_mut().map_or(0, |x| x.adc0_r8(a))
            }
            0xffff8408..=0xffff840f | 0xffff8411 | 0xffff8413 => {
                self.periph.as_mut().map_or(0, |x| x.adc1_r8(a))
            }
            // bsc 8620-8627,862a-8631 / dmac 86b0-86b1 / dmac0-3 16-byte blocks
            0xffff8620..=0xffff8627 | 0xffff862a..=0xffff8631 => {
                self.periph.as_mut().map_or(0, |x| x.bsc_r8(a))
            }
            0xffff86b0..=0xffff86b1 => self.periph.as_mut().map_or(0, |x| x.dmac_r8(a)),
            0xffff86c0..=0xffff86ff => {
                let ch = ((a - 0xffff_86c0) >> 4) as usize; // map:~680-788 per-channel blocks
                self.periph.as_mut().map_or(0, |x| x.dmac_ch_r8(ch, a))
            }
            // map:321-322 default: logerror (stderr-only) + return 0
            _ => 0,
        }
    }

    // origin: src/mame/cpu/sh7042_map.hxx:325-483 (internal_r16). Disk cases
    // are exact EVEN addresses; `even && in-run` reproduces each case set
    // (holes 0xffff820c..., bsc 0xffff8628 stay misses; odd always -> miss).
    pub fn internal_r16(&mut self, a: u32) -> u16 {
        match a {
            0xffff81a0 | 0xffff81a2 | 0xffff81a4 => {
                self.periph.as_mut().map_or(0, |x| x.sci_r16(0, a))
            } // map:328-330
            0xffff81b0 | 0xffff81b2 | 0xffff81b4 => {
                self.periph.as_mut().map_or(0, |x| x.sci_r16(1, a))
            } // map:331-333
            0xffff8200..=0xffff820a
            | 0xffff8210..=0xffff822c
            | 0xffff8240
            | 0xffff8260..=0xffff826e
            | 0xffff8280..=0xffff828a
            | 0xffff82a0..=0xffff82aa
                if a & 1 == 0 =>
            {
                self.periph.as_mut().map_or(0, |x| x.mtu_r16(a)) // mtu family union
            }
            (0xffff8348..=0xffff835a) if a & 1 == 0 => {
                self.periph.as_mut().map_or(0, |x| x.intc_r16(a))
            }
            0xffff8380 | 0xffff8382 | 0xffff8384 | 0xffff8386 => {
                self.periph.as_mut().map_or(0, |x| x.porta_r16(a))
            }
            // pcf owned r16 (map:390-401+): ah 8388, al 838c/838e, b 8398/839a,
            // c 839c, dh 83a8/83aa, dl 83ac, e 83b8/83ba, if 83c8
            0xffff8388 => self.pcf_ah_r(),
            0xffff838c => (self.pcf_al_r() >> 16) as u16,
            0xffff838e => self.pcf_al_r() as u16,
            0xffff8390 | 0xffff8394 => self.periph.as_mut().map_or(0, |x| x.portb_r16(a)),
            0xffff8392 | 0xffff8396 => self.periph.as_mut().map_or(0, |x| x.portc_r16(a)),
            0xffff8398 => (self.pcf_b_r() >> 16) as u16,
            0xffff839a => self.pcf_b_r() as u16,
            0xffff839c => self.pcf_c_r(),
            0xffff83a0 | 0xffff83a2 | 0xffff83a4 | 0xffff83a6 => {
                self.periph.as_mut().map_or(0, |x| x.portd_r16(a))
            }
            0xffff83a8 => (self.pcf_dh_r() >> 16) as u16,
            0xffff83aa => self.pcf_dh_r() as u16,
            0xffff83ac => self.pcf_dl_r(),
            0xffff83b0 | 0xffff83b4 => self.periph.as_mut().map_or(0, |x| x.porte_r16(a)),
            0xffff83b2 => self.periph.as_mut().map_or(0, |x| x.portf_r16(a)),
            0xffff83b8 => (self.pcf_e_r() >> 16) as u16,
            0xffff83ba => self.pcf_e_r() as u16,
            0xffff83c8 => self.pcf_if_r(),
            (0xffff83d0..=0xffff83dc) if a & 1 == 0 => {
                self.periph.as_mut().map_or(0, |x| x.cmt_r16(a))
            }
            0xffff83e0 | 0xffff83f0..=0xffff8406 | 0xffff8410 | 0xffff8412 if a & 1 == 0 => {
                self.periph.as_mut().map_or(0, |x| x.adc0_r16(a))
            }
            (0xffff8408..=0xffff840e) if a & 1 == 0 => {
                self.periph.as_mut().map_or(0, |x| x.adc1_r16(a))
            }
            // bsc r16 cases SKIP 8628 (disk set: 8620,8622,8624,8626,862a,..)
            0xffff8620..=0xffff8626 | 0xffff862a..=0xffff8630 if a & 1 == 0 => {
                self.periph.as_mut().map_or(0, |x| x.bsc_r16(a))
            }
            0xffff86b0 => self.periph.as_mut().map_or(0, |x| x.dmac_r16(a)),
            (0xffff86c0..=0xffff86fe) if a & 1 == 0 => {
                let ch = ((a - 0xffff_86c0) >> 4) as usize;
                self.periph.as_mut().map_or(0, |x| x.dmac_ch_r16(ch, a))
            }
            // map:481-482 default: logerror + return 0
            _ => 0,
        }
    }

    // origin: src/mame/cpu/sh7042_map.hxx:485-515 (internal_r32)
    pub fn internal_r32(&mut self, a: u32) -> u32 {
        match a {
            0xffff8380 | 0xffff8384 => self.periph.as_mut().map_or(0, |x| x.porta_r32(a)), // map:488-489
            0xffff838c => self.pcf_al_r(), // map:490
            0xffff8398 => self.pcf_b_r(),  // map:491
            0xffff83a0 | 0xffff83a4 => self.periph.as_mut().map_or(0, |x| x.portd_r32(a)), // map:492-493
            0xffff83a8 => self.pcf_dh_r(), // map:494
            0xffff83b8 => self.pcf_e_r(),  // map:495
            (0xffff86c0..=0xffff86fc) if a & 3 == 0 => {
                // map:496-511 dmac ch sar/dar/dmatcr/chcr at block+0,+4,+8,+c
                let ch = ((a - 0xffff_86c0) >> 4) as usize;
                self.periph.as_mut().map_or(0, |x| x.dmac_ch_r32(ch, a))
            }
            // map:513-514 "32bit レジスタでないところは 16bit を 2 回"
            _ => (u32::from(self.internal_r16(a)) << 16) | u32::from(self.internal_r16(a + 2)),
        }
    }

    // origin: src/mame/cpu/sh7042_map.hxx:517-791 (internal_w8)
    pub fn internal_w8(&mut self, a: u32, v: u8) {
        match a {
            // map:520-531 sci — NOTE no 81a5/81b5 (RDR read-only on disk)
            0xffff81a0..=0xffff81a4 => {
                if let Some(p) = self.periph.as_mut() {
                    p.sci_w8(0, a, v)
                }
            }
            0xffff81b0..=0xffff81b4 => {
                if let Some(p) = self.periph.as_mut() {
                    p.sci_w8(1, a, v)
                }
            }
            // w8 runs = r8 runs for the mtu family
            0xffff8200..=0xffff820b
            | 0xffff820d
            | 0xffff8210..=0xffff822d
            | 0xffff8240..=0xffff8241
            | 0xffff8260..=0xffff826f
            | 0xffff8280..=0xffff828b
            | 0xffff82a0..=0xffff82ab => {
                if let Some(p) = self.periph.as_mut() {
                    p.mtu_w8(a, v)
                }
            }
            0xffff8348..=0xffff835b => {
                if let Some(p) = self.periph.as_mut() {
                    p.intc_w8(a, v)
                }
            }
            0xffff8380..=0xffff8387 => {
                if let Some(p) = self.periph.as_mut() {
                    p.porta_w8(a, v)
                }
            }
            // pcf byte writes (map:643-686) — COMBINE_DATA with the byte mask
            0xffff8388 => self.pcf_ah_w(u16::from(v) << 8, 0xff00),             // map:643
            0xffff8389 => self.pcf_ah_w(u16::from(v), 0x00ff),                  // map:644
            0xffff838c => self.pcf_al_w(u32::from(v) << 24, 0xff00_0000),       // map:645
            0xffff838d => self.pcf_al_w(u32::from(v) << 16, 0x00ff_0000),       // map:646
            0xffff838e => self.pcf_al_w(u32::from(v) << 8, 0x0000_ff00),        // map:647
            0xffff838f => self.pcf_al_w(u32::from(v), 0x0000_00ff),             // map:648
            0xffff8390..=0xffff8391 | 0xffff8394..=0xffff8395 => {
                if let Some(p) = self.periph.as_mut() {
                    p.portb_w8(a, v)
                }
            }
            0xffff8392..=0xffff8393 | 0xffff8396..=0xffff8397 => {
                if let Some(p) = self.periph.as_mut() {
                    p.portc_w8(a, v)
                }
            }
            0xffff8398 => self.pcf_b_w(u32::from(v) << 24, 0xff00_0000), // map:657
            0xffff8399 => self.pcf_b_w(u32::from(v) << 16, 0x00ff_0000), // map:658
            0xffff839a => self.pcf_b_w(u32::from(v) << 8, 0x0000_ff00),  // map:659
            0xffff839b => self.pcf_b_w(u32::from(v), 0x0000_00ff),       // map:660
            0xffff839c => self.pcf_c_w(u16::from(v) << 8, 0xff00),       // map:661
            0xffff839d => self.pcf_c_w(u16::from(v), 0x00ff),            // map:662
            0xffff83a0..=0xffff83a7 => {
                if let Some(p) = self.periph.as_mut() {
                    p.portd_w8(a, v)
                }
            }
            0xffff83a8 => self.pcf_dh_w(u32::from(v) << 24, 0xff00_0000), // map:671
            0xffff83a9 => self.pcf_dh_w(u32::from(v) << 16, 0x00ff_0000), // map:672
            0xffff83aa => self.pcf_dh_w(u32::from(v) << 8, 0x0000_ff00),  // map:673
            0xffff83ab => self.pcf_dh_w(u32::from(v), 0x0000_00ff),       // map:674
            0xffff83ac => self.pcf_dl_w(u16::from(v) << 8, 0xff00),       // map:675
            0xffff83ad => self.pcf_dl_w(u16::from(v), 0x00ff),            // map:676
            0xffff83b0..=0xffff83b1 | 0xffff83b4..=0xffff83b5 => {
                if let Some(p) = self.periph.as_mut() {
                    p.porte_w8(a, v)
                }
            }
            // PORTF has NO w8 on disk (83b2-83b3 absent from the w8 table) ->
            // falls to the default drop. map:790 logerror only.
            0xffff83b8 => self.pcf_e_w(u32::from(v) << 24, 0xff00_0000), // map:681
            0xffff83b9 => self.pcf_e_w(u32::from(v) << 16, 0x00ff_0000), // map:682
            0xffff83ba => self.pcf_e_w(u32::from(v) << 8, 0x0000_ff00),  // map:683
            0xffff83bb => self.pcf_e_w(u32::from(v), 0x0000_00ff),       // map:684
            0xffff83c8 => self.pcf_if_w(u16::from(v) << 8, 0xff00),      // map:685
            0xffff83c9 => self.pcf_if_w(u16::from(v), 0x00ff),           // map:686
            0xffff83d0..=0xffff83dd => {
                if let Some(p) = self.periph.as_mut() {
                    p.cmt_w8(a, v)
                }
            }
            // adc0 byte writes ONLY 83e0-83e1,8410,8412 (ADCDR ro)
            0xffff83e0..=0xffff83e1 | 0xffff8410 | 0xffff8412 => {
                if let Some(p) = self.periph.as_mut() {
                    p.adc0_w8(a, v)
                }
            }
            // adc1 byte writes ONLY the odd 8411/8413
            0xffff8411 | 0xffff8413 => {
                if let Some(p) = self.periph.as_mut() {
                    p.adc1_w8(a, v)
                }
            }
            0xffff8620..=0xffff8627 | 0xffff862a..=0xffff8631 => {
                if let Some(p) = self.periph.as_mut() {
                    p.bsc_w8(a, v)
                }
            }
            0xffff86b0..=0xffff86b1 => {
                if let Some(p) = self.periph.as_mut() {
                    p.dmac_w8(a, v)
                }
            }
            0xffff86c0..=0xffff86ff => {
                let ch = ((a - 0xffff_86c0) >> 4) as usize;
                if let Some(p) = self.periph.as_mut() {
                    p.dmac_ch_w8(ch, a, v)
                }
            }
            // map:790 default: logerror, write discarded
            _ => {}
        }
    }

    // origin: src/mame/cpu/sh7042_map.hxx:793-933 (internal_w16)
    pub fn internal_w16(&mut self, a: u32, v: u16) {
        match a {
            0xffff81a0 | 0xffff81a2 | 0xffff81a4 => {
                if let Some(p) = self.periph.as_mut() {
                    p.sci_w16(0, a, v)
                }
            }
            0xffff81b0 | 0xffff81b2 | 0xffff81b4 => {
                if let Some(p) = self.periph.as_mut() {
                    p.sci_w16(1, a, v)
                }
            }
            0xffff8200..=0xffff820a
            | 0xffff8210..=0xffff822c
            | 0xffff8240
            | 0xffff8260..=0xffff826e
            | 0xffff8280..=0xffff828a
            | 0xffff82a0..=0xffff82aa
                if a & 1 == 0 =>
            {
                if let Some(p) = self.periph.as_mut() {
                    p.mtu_w16(a, v)
                }
            }
            (0xffff8348..=0xffff835a) if a & 1 == 0 => {
                if let Some(p) = self.periph.as_mut() {
                    p.intc_w16(a, v)
                }
            }
            0xffff8380 | 0xffff8382 | 0xffff8384 | 0xffff8386 => {
                if let Some(p) = self.periph.as_mut() {
                    p.porta_w16(a, v)
                }
            }
            // pcf w16 (map:858-879)
            0xffff8388 => self.pcf_ah_w(v, 0xffff),                      // map:858
            0xffff838c => self.pcf_al_w(u32::from(v) << 16, 0xffff_0000), // map:859
            0xffff838e => self.pcf_al_w(u32::from(v), 0x0000_ffff),       // map:860
            0xffff8390 | 0xffff8394 => {
                if let Some(p) = self.periph.as_mut() {
                    p.portb_w16(a, v)
                }
            }
            0xffff8392 | 0xffff8396 => {
                if let Some(p) = self.periph.as_mut() {
                    p.portc_w16(a, v)
                }
            }
            0xffff8398 => self.pcf_b_w(u32::from(v) << 16, 0xffff_0000), // map:865
            0xffff839a => self.pcf_b_w(u32::from(v), 0x0000_ffff),       // map:866
            0xffff839c => self.pcf_c_w(v, 0xffff),                       // map:867
            0xffff83a0 | 0xffff83a2 | 0xffff83a4 | 0xffff83a6 => {
                if let Some(p) = self.periph.as_mut() {
                    p.portd_w16(a, v)
                }
            }
            0xffff83a8 => self.pcf_dh_w(u32::from(v) << 16, 0xffff_0000), // map:872
            0xffff83aa => self.pcf_dh_w(u32::from(v), 0x0000_ffff),       // map:873
            0xffff83ac => self.pcf_dl_w(v, 0xffff),                       // map:874
            0xffff83b0 | 0xffff83b4 => {
                if let Some(p) = self.periph.as_mut() {
                    p.porte_w16(a, v)
                }
            }
            // PORTF / ADC1 have NO w16 on disk; ADC0 w16 only 83e0,8410,8412
            0xffff83b8 => self.pcf_e_w(u32::from(v) << 16, 0xffff_0000), // map:877
            0xffff83ba => self.pcf_e_w(u32::from(v), 0x0000_ffff),       // map:878
            0xffff83c8 => self.pcf_if_w(v, 0xffff),                      // map:879
            (0xffff83d0..=0xffff83dc) if a & 1 == 0 => {
                if let Some(p) = self.periph.as_mut() {
                    p.cmt_w16(a, v)
                }
            }
            0xffff83e0 | 0xffff8410 | 0xffff8412 => {
                if let Some(p) = self.periph.as_mut() {
                    p.adc0_w16(a, v)
                }
            }
            // bsc skips the 8628 hole exactly like r16
            0xffff8620..=0xffff8626 | 0xffff862a..=0xffff8630 if a & 1 == 0 => {
                if let Some(p) = self.periph.as_mut() {
                    p.bsc_w16(a, v)
                }
            }
            0xffff86b0 => {
                if let Some(p) = self.periph.as_mut() {
                    p.dmac_w16(a, v)
                }
            }
            (0xffff86c0..=0xffff86fe) if a & 1 == 0 => {
                let ch = ((a - 0xffff_86c0) >> 4) as usize;
                if let Some(p) = self.periph.as_mut() {
                    p.dmac_ch_w16(ch, a, v)
                }
            }
            // map:932 default: logerror, write discarded
            _ => {}
        }
    }

    // origin: src/mame/cpu/sh7042_map.hxx:935-966 (internal_w32)
    pub fn internal_w32(&mut self, a: u32, v: u32) {
        match a {
            0xffff8380 | 0xffff8384 => {
                // map:938-939 porta->dr_w / io_w, mask full
                if let Some(p) = self.periph.as_mut() {
                    p.porta_w32(a, v)
                }
            }
            0xffff838c => self.pcf_al_w(v, 0xffff_ffff), // map:940
            0xffff8398 => self.pcf_b_w(v, 0xffff_ffff),  // map:941
            0xffff83a0 | 0xffff83a4 => {
                // map:942-943 portd
                if let Some(p) = self.periph.as_mut() {
                    p.portd_w32(a, v)
                }
            }
            0xffff83a8 => self.pcf_dh_w(v, 0xffff_ffff), // map:944
            0xffff83b8 => self.pcf_e_w(v, 0xffff_ffff),  // map:945
            (0xffff86c0..=0xffff86fc) if a & 3 == 0 => {
                // map:946-961 dmac sar/dar/dmatcr/chcr
                let ch = ((a - 0xffff_86c0) >> 4) as usize;
                if let Some(p) = self.periph.as_mut() {
                    p.dmac_ch_w32(ch, a, v)
                }
            }
            // map:963-965 "32bit レジスタでないところは 16bit を 2 回"
            _ => {
                self.internal_w16(a, (v >> 16) as u16);
                self.internal_w16(a + 2, (v & 0xffff) as u16);
            }
        }
    }

    // ---- pcf register pairs (sh7042.cpp:302-388), COMBINE_DATA at :309 etc. ----
    // origin: src/mame/cpu/sh7042.cpp:302-305 pcf_ah_r
    #[inline]
    pub fn pcf_ah_r(&self) -> u16 {
        self.m_pcf_ah
    }
    // origin: src/mame/cpu/sh7042.cpp:307-311 pcf_ah_w (COMBINE_DATA :309)
    #[inline]
    pub fn pcf_ah_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_pcf_ah, data, mem_mask);
    }
    // origin: src/mame/cpu/sh7042.cpp:313-316
    #[inline]
    pub fn pcf_al_r(&self) -> u32 {
        self.m_pcf_al
    }
    // origin: src/mame/cpu/sh7042.cpp:318-322 (:320)
    #[inline]
    pub fn pcf_al_w(&mut self, data: u32, mem_mask: u32) {
        combine32(&mut self.m_pcf_al, data, mem_mask);
    }
    // origin: src/mame/cpu/sh7042.cpp:324-327
    #[inline]
    pub fn pcf_b_r(&self) -> u32 {
        self.m_pcf_b
    }
    // origin: src/mame/cpu/sh7042.cpp:329-333 (:331)
    #[inline]
    pub fn pcf_b_w(&mut self, data: u32, mem_mask: u32) {
        combine32(&mut self.m_pcf_b, data, mem_mask);
    }
    // origin: src/mame/cpu/sh7042.cpp:335-338
    #[inline]
    pub fn pcf_c_r(&self) -> u16 {
        self.m_pcf_c
    }
    // origin: src/mame/cpu/sh7042.cpp:340-344 (:342)
    #[inline]
    pub fn pcf_c_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_pcf_c, data, mem_mask);
    }
    // origin: src/mame/cpu/sh7042.cpp:346-349
    #[inline]
    pub fn pcf_dh_r(&self) -> u32 {
        self.m_pcf_dh
    }
    // origin: src/mame/cpu/sh7042.cpp:351-355 (:353)
    #[inline]
    pub fn pcf_dh_w(&mut self, data: u32, mem_mask: u32) {
        combine32(&mut self.m_pcf_dh, data, mem_mask);
    }
    // origin: src/mame/cpu/sh7042.cpp:357-360
    #[inline]
    pub fn pcf_dl_r(&self) -> u16 {
        self.m_pcf_dl
    }
    // origin: src/mame/cpu/sh7042.cpp:362-366 (:364)
    #[inline]
    pub fn pcf_dl_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_pcf_dl, data, mem_mask);
    }
    // origin: src/mame/cpu/sh7042.cpp:368-371
    #[inline]
    pub fn pcf_e_r(&self) -> u32 {
        self.m_pcf_e
    }
    // origin: src/mame/cpu/sh7042.cpp:373-377 (:375)
    #[inline]
    pub fn pcf_e_w(&mut self, data: u32, mem_mask: u32) {
        combine32(&mut self.m_pcf_e, data, mem_mask);
    }
    // origin: src/mame/cpu/sh7042.cpp:379-382
    #[inline]
    pub fn pcf_if_r(&self) -> u16 {
        self.m_pcf_if
    }
    // origin: src/mame/cpu/sh7042.cpp:384-388 (:386)
    #[inline]
    pub fn pcf_if_w(&mut self, data: u16, mem_mask: u16) {
        combine16(&mut self.m_pcf_if, data, mem_mask);
    }
}

// ---------------------------------------------------------------------------
// mem_bus::read_byte / read_word / read_dword — membus.h:61-97 verbatim
// (BE assembly, width-fallback order, unmapped -> 0).
// ---------------------------------------------------------------------------
impl Sh2Bus for Sh7042Bus {
    // origin: src/compat/membus.h:61-71
    fn read_byte(&mut self, a: u32) -> u8 {
        if let Some(v) = self.fast_read(a) {
            return v; // membus.h:63 hot ROM/RAM
        }
        if let Some(v) = self.find_read(a) {
            return v; // membus.h:64 DRAM/IRAM regions
        }
        if let Some(d) = self.find_dev(a) {
            // membus.h:65-68. Every disk device registers r8 EXCEPT Panel
            // (mu2000.cpp:933-937: no read handler at all), which also lacks
            // r16/r32 — so the :67-68 fallback legs are dead for this device
            // set (kept here as the faithful-order comment):
            //   if (d->r8)  return d->r8(a);
            //   if (d->r16) return d->r16(a & ~1) >> ((a & 1) ? 0 : 8);
            //   if (d->r32) return d->r32(a & ~3) >> ((3 - (a & 3)) * 8);
            match d {
                Dev::Swpm | Dev::Swps => {
                    return self
                        .periph
                        .as_mut()
                        .map_or(0, |p| p.swp_r8(d == Dev::Swpm, a))
                }
                Dev::Led => return self.periph.as_mut().map_or(0, |p| p.ledsw_r8()),
                Dev::Panel => {} // NO handlers: falls to 0 (membus.h:70)
                Dev::D80 => return self.periph.as_mut().map_or(0, |p| p.d80_r8()),
                Dev::CardData => return self.periph.as_mut().map_or(0, |p| p.card_data_r8()),
                Dev::CardCtrl => return 0xff, // mu2000.cpp:960 `return 0xff` literal
                Dev::Sci4 => {
                    return self
                        .periph
                        .as_mut()
                        .map_or(0, |p| p.sci4_r8(a - 0xf00000)) // mu2000.cpp:969
                }
                Dev::Usb => {
                    return self
                        .periph
                        .as_mut()
                        .map_or(0, |p| p.usb_r8(a - 0xf80000)) // mu2000.cpp:979
                }
                Dev::Internal => return self.internal_r8(a), // mu2000.cpp:988
            }
        }
        0 // membus.h:70 unmapped read -> 0
    }

    // origin: src/compat/membus.h:73-84
    fn read_word(&mut self, a: u32) -> u16 {
        let a = a & !1u32; // membus.h:75
        if let Some(v) = self.region_word(a) {
            return v; // membus.h:76-77 BE from the region pointer
        }
        if let Some(d) = self.find_dev(a) {
            match d {
                Dev::Swpm | Dev::Swps => {
                    // membus.h:79 d->r16 exists for the SWP (mu2000.cpp:861)
                    return self
                        .periph
                        .as_mut()
                        .map_or(0, |p| p.swp_r16(d == Dev::Swpm, a))
                }
                Dev::Led => {
                    // no r16 -> membus.h:81 r8 pair ON THE SAME HANDLER
                    // (ledsw_r() ignores the address; two real scans)
                    let p = self.periph.as_mut();
                    if let Some(p) = p {
                        let hi = p.ledsw_r8();
                        let lo = p.ledsw_r8();
                        return (u16::from(hi) << 8) | u16::from(lo);
                    }
                    return 0;
                }
                Dev::Panel => return 0, // no handlers at all (membus.h:83)
                Dev::D80 => {
                    let p = self.periph.as_mut();
                    if let Some(p) = p {
                        let hi = p.d80_r8();
                        let lo = p.d80_r8();
                        return (u16::from(hi) << 8) | u16::from(lo);
                    }
                    return 0;
                }
                Dev::CardData => {
                    let p = self.periph.as_mut();
                    if let Some(p) = p {
                        let hi = p.card_data_r8();
                        let lo = p.card_data_r8();
                        return (u16::from(hi) << 8) | u16::from(lo);
                    }
                    return 0;
                }
                Dev::CardCtrl => return 0xffff, // 0xff<<8|0xff (mu2000.cpp:960 twice)
                Dev::Sci4 => {
                    // membus.h:81 r8 pair; offsets per mu2000.cpp:969 rule,
                    // TWO real device accesses (state may change per read)
                    let p = self.periph.as_mut();
                    if let Some(p) = p {
                        let hi = p.sci4_r8(a - 0xf00000);
                        let lo = p.sci4_r8(a + 1 - 0xf00000);
                        return (u16::from(hi) << 8) | u16::from(lo);
                    }
                    return 0;
                }
                Dev::Usb => {
                    let p = self.periph.as_mut();
                    if let Some(p) = p {
                        let hi = p.usb_r8(a - 0xf80000);
                        let lo = p.usb_r8(a + 1 - 0xf80000);
                        return (u16::from(hi) << 8) | u16::from(lo);
                    }
                    return 0;
                }
                Dev::Internal => return self.internal_r16(a), // mu2000.cpp:989
            }
        }
        0 // membus.h:83 unmapped -> 0
    }

    // origin: src/compat/membus.h:86-97
    fn read_long(&mut self, a: u32) -> u32 {
        let a = a & !3u32; // membus.h:88
        if let Some(v) = self.region_long(a) {
            return v; // membus.h:89-92
        }
        if let Some(d) = self.find_dev(a) {
            if d == Dev::Internal {
                // membus.h:93-95: d->r32 present only for the internal device
                // (mu2000.cpp:990). SWP has NO r32 -> word pair (membus.h:96).
                return self.internal_r32(a);
            }
        }
        // membus.h:96: (read_word(a) << 16) | read_word(a + 2) — full re-dispatch
        let hi = u32::from(self.read_word(a));
        let lo = u32::from(self.read_word(a + 2));
        (hi << 16) | lo
    }

    // origin: src/compat/membus.h:101-110
    fn write_byte(&mut self, a: u32, v: u8) {
        if self.write_byte_region(a, v) {
            return; // membus.h:103-104 (hot-w ⊂ writable regions)
        }
        if let Some(d) = self.find_dev(a) {
            // membus.h:106 d->w8; fallback legs :107-108 (w16/w32 without w8)
            // are dead: every disk write handler registers w8 except none.
            match d {
                Dev::Swpm | Dev::Swps => {
                    // mu2000.cpp:869-876 — 16-bit reg RMW, byte lane = (a&1)
                    // resolved INSIDE the handler (trait keeps absolute a)
                    if let Some(p) = self.periph.as_mut() {
                        p.swp_w8(d == Dev::Swpm, a, v)
                    }
                }
                Dev::Led => {
                    if let Some(p) = self.periph.as_mut() {
                        p.ledsw1_w8(v) // mu2000.cpp:930
                    }
                }
                Dev::Panel => {
                    if let Some(p) = self.periph.as_mut() {
                        p.ledsw2_w8(v) // mu2000.cpp:936
                    }
                }
                Dev::D80 => {
                    if let Some(p) = self.periph.as_mut() {
                        p.d80_w8(v) // mu2000.cpp:945
                    }
                }
                Dev::CardData => {
                    if let Some(p) = self.periph.as_mut() {
                        p.card_data_w8(v) // mu2000.cpp:954
                    }
                }
                Dev::CardCtrl => {
                    if let Some(p) = self.periph.as_mut() {
                        p.card_ctrl_w8(v) // mu2000.cpp:961
                    }
                }
                Dev::Sci4 => {
                    if let Some(p) = self.periph.as_mut() {
                        p.sci4_w8(a - 0xf00000, v) // mu2000.cpp:970
                    }
                }
                Dev::Usb => {
                    if let Some(p) = self.periph.as_mut() {
                        p.usb_w8(a - 0xf80000, v) // mu2000.cpp:980
                    }
                }
                Dev::Internal => self.internal_w8(a, v), // mu2000.cpp:991
            }
        }
        // unmapped or read-only-ROM write: dropped (membus.h:105-109 falls off)
    }

    // origin: src/compat/membus.h:112-122
    fn write_word(&mut self, a: u32, v: u16) {
        let a = a & !1u32; // membus.h:114
        // membus.h:115-116 p[0]=hi p[1]=lo BE. Regions are even-sized/aligned
        // so both bytes stay in-region for aligned a (SH-2 rule).
        if (0x400000..=0x43ffff).contains(&a) {
            let i = (a - 0x400000) as usize;
            self.ram[i] = (v >> 8) as u8;
            self.ram[i + 1] = v as u8;
            return;
        } else if (0x1000000..=0x107fffe).contains(&a) {
            let i = (a - 0x1000000) as usize;
            self.dram[i] = (v >> 8) as u8;
            self.dram[i + 1] = v as u8;
            return;
        } else if (0xfffff000..=0xfffffffe).contains(&a) {
            let i = (a - 0xfffff000) as usize;
            self.iram[i] = (v >> 8) as u8;
            self.iram[i + 1] = v as u8;
            return;
        }
        if let Some(d) = self.find_dev(a) {
            match d {
                Dev::Swpm | Dev::Swps => {
                    // mu2000.cpp:906-918 single 16-bit reg write
                    if let Some(p) = self.periph.as_mut() {
                        p.swp_w16(d == Dev::Swpm, a, v)
                    }
                }
                Dev::Internal => self.internal_w16(a, v), // mu2000.cpp:992
                _ => {
                    // membus.h:120 r8-pair fallback on the SAME handler:
                    // d->w8(a, v>>8); d->w8(a+1, v)
                    match d {
                        Dev::Led => {
                            if let Some(p) = self.periph.as_mut() {
                                p.ledsw1_w8((v >> 8) as u8)
                            }
                            if let Some(p) = self.periph.as_mut() {
                                p.ledsw1_w8(v as u8)
                            }
                        }
                        Dev::Panel => {
                            if let Some(p) = self.periph.as_mut() {
                                p.ledsw2_w8((v >> 8) as u8)
                            }
                            if let Some(p) = self.periph.as_mut() {
                                p.ledsw2_w8(v as u8)
                            }
                        }
                        Dev::D80 => {
                            if let Some(p) = self.periph.as_mut() {
                                p.d80_w8((v >> 8) as u8)
                            }
                            if let Some(p) = self.periph.as_mut() {
                                p.d80_w8(v as u8)
                            }
                        }
                        Dev::CardData => {
                            if let Some(p) = self.periph.as_mut() {
                                p.card_data_w8((v >> 8) as u8)
                            }
                            if let Some(p) = self.periph.as_mut() {
                                p.card_data_w8(v as u8)
                            }
                        }
                        Dev::CardCtrl => {
                            if let Some(p) = self.periph.as_mut() {
                                p.card_ctrl_w8((v >> 8) as u8)
                            }
                            if let Some(p) = self.periph.as_mut() {
                                p.card_ctrl_w8(v as u8)
                            }
                        }
                        Dev::Sci4 => {
                            if let Some(p) = self.periph.as_mut() {
                                p.sci4_w8(a - 0xf00000, (v >> 8) as u8)
                            }
                            if let Some(p) = self.periph.as_mut() {
                                p.sci4_w8(a + 1 - 0xf00000, v as u8)
                            }
                        }
                        Dev::Usb => {
                            if let Some(p) = self.periph.as_mut() {
                                p.usb_w8(a - 0xf80000, (v >> 8) as u8)
                            }
                            if let Some(p) = self.periph.as_mut() {
                                p.usb_w8(a + 1 - 0xf80000, v as u8)
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        // unmapped/ROM write dropped (membus.h:117-121)
    }

    // origin: src/compat/membus.h:124-140
    fn write_long(&mut self, a: u32, v: u32) {
        let a = a & !3u32; // membus.h:126
        // membus.h:127-130 four BE bytes; even-sized regions (see write_word)
        if (0x400000..=0x43fffc).contains(&a) {
            let i = (a - 0x400000) as usize;
            self.ram[i] = (v >> 24) as u8;
            self.ram[i + 1] = (v >> 16) as u8;
            self.ram[i + 2] = (v >> 8) as u8;
            self.ram[i + 3] = v as u8;
            return;
        } else if (0x1000000..=0x107fffc).contains(&a) {
            let i = (a - 0x1000000) as usize;
            self.dram[i] = (v >> 24) as u8;
            self.dram[i + 1] = (v >> 16) as u8;
            self.dram[i + 2] = (v >> 8) as u8;
            self.dram[i + 3] = v as u8;
            return;
        } else if (0xfffff000..=0xfffffffc).contains(&a) {
            let i = (a - 0xfffff000) as usize;
            self.iram[i] = (v >> 24) as u8;
            self.iram[i + 1] = (v >> 16) as u8;
            self.iram[i + 2] = (v >> 8) as u8;
            self.iram[i + 3] = v as u8;
            return;
        }
        if let Some(d) = self.find_dev(a) {
            match d {
                // mu2000.cpp:993 internal (its w32 has the w16-pair tail)
                Dev::Internal => {
                    self.internal_w32(a, v);
                    return;
                }
                // membus.h:136 d->w32 exists for the SWP devices
                // (mu2000.cpp:881-905): ONE 32-bit handler does two reg writes
                Dev::Swpm | Dev::Swps => {
                    if let Some(p) = self.periph.as_mut() {
                        p.swp_w32(d == Dev::Swpm, a, v)
                    }
                    return;
                }
                // membus.h:138-139 else write_word(a,hi); write_word(a+2,lo) —
                // full re-dispatch (USB/Sci4/card/led take the byte-pair legs)
                _ => {}
            }
        }
        let hi = (v >> 16) as u16;
        let lo = (v & 0xffff) as u16;
        self.write_word(a, hi);
        self.write_word(a + 2, lo);
    }
}

// ---------------------------------------------------------------------------
// The SoC device.
// ---------------------------------------------------------------------------
pub struct Sh7042 {
    // origin: src/mame/cpu/sh7042.cpp:41 sh2_device base ctor (CPU_TYPE_SH2,
    // 32 address lines, mask 0xffffffff)
    pub dev: Sh2Device,
    pub bus: Sh7042Bus,
    // origin: src/mame/cpu/sh7042.h:173 m_event_cycles = 0
    pub m_event_cycles: u64,
    // origin: src/mame/cpu/sh7042.h:174 m_in_event = false
    pub m_in_event: bool,
    // origin: src/mame/cpu/sh7042.h:129 m_die_a = false; set true by the 'a'
    // variants (sh7042.cpp:17/23/29/35)
    pub m_die_a: bool,
}

impl Sh7042 {
    /// SH7042/SH7043 (non-'a' die). Every field explicit (Invariant 3).
    /// ROM arrives at construction — the disk attaches the program bus at
    /// build time (mu2000.cpp:383/389-393 + set_program_bus :997), i.e.
    /// BEFORE any reset reads the 0x00/0x04 vectors.
    // origin: src/mame/cpu/sh7042.cpp:14-18 + :38-85
    pub fn new(clock: u32, rom: Vec<u8>) -> Self {
        Sh7042 {
            dev: Sh2Device::new(clock, CPU_TYPE_SH2, 0xffff_ffff), // sh7042.cpp:41
            bus: Sh7042Bus::new(rom),
            m_event_cycles: 0, // sh7042.h:173
            m_in_event: false, // sh7042.h:174
            m_die_a: false,    // sh7042.cpp:17 (SH7042) / :29 (SH7043)
        }
    }

    /// SH7042A/SH7043A variant: m_die_a = true (two ADCs, sh7042.cpp:166-168).
    /// S-MU2000 runs the SH7043A — die_a = true.
    // origin: src/mame/cpu/sh7042.cpp:20-24 + :32-36
    pub fn new_a(clock: u32, rom: Vec<u8>) -> Self {
        Sh7042 {
            m_die_a: true, // sh7042.cpp:23 (SH7042A) / :35 (SH7043A)
            ..Sh7042::new(clock, rom)
        }
    }

    /// MAME clock stand-in: total-1 OUTSIDE event callbacks, exact tick inside
    /// (the -1 rounding from the cycle<->attotime round trip; sh7042.h:85-91).
    // origin: src/mame/cpu/sh7042.h:92-98
    #[inline]
    pub fn current_cycles(&self) -> u64 {
        let c = self.dev.core.total_cycles(); // sh7042.h:94
        if self.m_in_event {
            return c; // sh7042.h:95-96
        }
        if c != 0 {
            c - 1 // sh7042.h:97 (`c ? c - 1 : 0`)
        } else {
            0
        }
    }

    /// device_start: sh2 start first, then the pcf zeros. save_item (:122-129)
    /// is the M5 state row; the explicit zeros mirror the disk position.
    // origin: src/mame/cpu/sh7042.cpp:118-139
    pub fn device_start(&mut self) {
        self.dev.device_start(); // sh7042.cpp:120
        self.bus.m_pcf_ah = 0; // sh7042.cpp:131
        self.bus.m_pcf_al = 0; // sh7042.cpp:132
        self.bus.m_pcf_b = 0; // sh7042.cpp:133
        self.bus.m_pcf_c = 0; // sh7042.cpp:134
        self.bus.m_pcf_dh = 0; // sh7042.cpp:135
        self.bus.m_pcf_dl = 0; // sh7042.cpp:136
        self.bus.m_pcf_e = 0; // sh7042.cpp:137
        self.bus.m_pcf_if = 0; // sh7042.cpp:138
    }

    /// device_reset: just the sh2 reset (sh7042.cpp:146-149). ROM attach order
    /// is enforced by construction (bus.rom exists before any reset; the disk
    /// equivalent is set_program_bus at build time, mu2000.cpp:997).
    // origin: src/mame/cpu/sh7042.cpp:146-149
    pub fn device_reset(&mut self) {
        // disjoint field borrows (bus as bus, dev as dev) — mirrors the C++
        // device_reset reaching the machine-supplied program bus
        let mut ctx = BusCtx { bus: &mut self.bus };
        self.dev.device_reset(&mut ctx); // sh7042.cpp:148
    }

    /// origin: src/mame/cpu/sh7042.cpp:403-431. Tag, then the sh2 part (:406),
    /// the event bookkeeping (:407) and the eight PCF pin regs (:408-409,
    /// widths per sh7042.h:176-183), then EVERY internal peripheral in birth
    /// order (:412-430). The adc1 leg (:414-418, issue #18): version>=8 AND
    /// present — C++ gates on the optional device pointer `m_adc1`
    /// (created only on die-A, sh7042.cpp:166-170; mirrored by `m_die_a`).
    /// The sci loop (:428-430) skips absent devices via `lookup()`; here the
    /// seam method is always routed (the Hub pair always exists).
    pub fn state(&mut self, s: &mut StateIo) {
        s.tag("sh7042");                     // :405
        self.dev.state(s);                   // :406 sh2_device::state
        s.v(&mut self.m_event_cycles);       // :407 (sh7042.h:173 u64)
        s.v(&mut self.m_in_event);           // :407 (sh7042.h:174 bool)
        s.v(&mut self.bus.m_pcf_ah);         // :408 (h:176 u16)
        s.v(&mut self.bus.m_pcf_al);         // :408 (h:177 u32)
        s.v(&mut self.bus.m_pcf_b);          // :408 (h:178 u32)
        s.v(&mut self.bus.m_pcf_c);          // :408 (h:179 u16)
        s.v(&mut self.bus.m_pcf_dh);         // :409 (h:180 u32)
        s.v(&mut self.bus.m_pcf_dl);         // :409 (h:181 u16)
        s.v(&mut self.bus.m_pcf_e);          // :409 (h:182 u32)
        s.v(&mut self.bus.m_pcf_if);         // :409 (h:183 u16)
        // :411 "内蔵の周辺。生まれた順にたどる" — birth order, not address order
        if let Some(p) = self.bus.periph.as_mut() {
            p.intc_state(s); // :412
            p.adc0_state(s); // :413
            if s.version() >= 8 && self.m_die_a {
                // :417-418 (s.version()>=8 && m_adc1)
                p.adc1_state(s);
            }
            p.bsc_state(s);  // :419
            p.cmt_state(s);  // :420
            p.dmac_state(s); // :421
            // :422 m_dmac0..3 — ShDmac owns the four channel structs
            for ch in 0..4usize {
                p.dmac_ch_state(ch, s);
            }
            p.mtu_state(s); // :423
            // :424-425 m_mtu0..4
            for ch in 0..5usize {
                p.mtu_ch_state(ch, s);
            }
            // :426-427 porta..portf (a/d 32-bit, b/c/e/f 16-bit)
            for port in 0..6usize {
                p.port_state(port, s);
            }
            // :428-430 sci0, sci1 (m_sci[i].lookup())
            for sci in 0..2usize {
                p.sci_state(sci, s);
            }
        }
    }

    // origin: src/mame/cpu/sh7042.cpp:242-245 (internal_update() -> timed)
    pub fn internal_update(&mut self) {
        let t = self.current_cycles(); // sh7042.cpp:244
        self.internal_update_at(t);
    }

    /// origin: src/mame/cpu/sh7042.cpp:278-300. Order per :282-292: adc0,
    /// adc1 (only die-A — m_adc1 optional_device), cmt, mtu0..4, sci0, sci1.
    /// The g_upd_trace line (:295-297) is the wiring-row sink — deferred at
    /// this layer. Absent peripherals contribute 0 and add_event skips 0
    /// (:256-257), matching "no event scheduled".
    /// Returns TRUE when `recompute_timer` changed `m_event_cycles` — the
    /// `abort_timeslice()` effect (sh7042.cpp:267-269): the mu2000 run-loop
    /// must cut its CPU burst at this instruction boundary.
    pub fn internal_update_at(&mut self, current_time: u64) -> bool {
        let mut event_time = 0u64; // sh7042.cpp:280
        if let Some(p) = self.bus.periph.as_mut() {
            let e = p.adc0_update(current_time); // sh7042.cpp:282
            Self::add_event(&mut event_time, e);
            if self.m_die_a {
                let e = p.adc1_update(current_time); // sh7042.cpp:283-284
                Self::add_event(&mut event_time, e);
            }
            let e = p.cmt_update(current_time); // sh7042.cpp:285
            Self::add_event(&mut event_time, e);
            for ch in 0..5usize {
                // sh7042.cpp:286-290 mtu0..mtu4
                let e = p.mtu_ch_update(ch, current_time);
                Self::add_event(&mut event_time, e);
            }
            for sci in 0..2usize {
                // sh7042.cpp:291-292 sci0, sci1
                let e = p.sci_update(sci, current_time);
                Self::add_event(&mut event_time, e);
            }
        }
        // sh7042.cpp:295-297 g_upd_trace — DEFERRED to the wiring row (sink).
        self.recompute_timer(event_time) // sh7042.cpp:299
    }

    // origin: src/mame/cpu/sh7042.cpp:254-260
    #[inline]
    fn add_event(event_time: &mut u64, new_event: u64) {
        if new_event == 0 {
            return; // sh7042.cpp:256-257
        }
        if *event_time == 0 || *event_time > new_event {
            // sh7042.cpp:258-259
            *event_time = new_event;
        }
    }

    /// origin: src/mame/cpu/sh7042.cpp:262-271. abort_timeslice -> the core's
    /// m_cpu_off bail (core.rs:242-243 / :2272).
    pub fn recompute_timer(&mut self, event_time: u64) -> bool {
        if self.m_event_cycles != event_time {
            // sh7042.cpp:267-270
            self.m_event_cycles = event_time;
            self.dev.core.abort_timeslice(); // sh7042.cpp:269
            true
        } else {
            false
        }
    }

    // origin: src/mame/cpu/sh7042.cpp:247-252
    pub fn event_tick(&mut self) {
        self.m_in_event = true; // sh7042.cpp:249
        let t = self.current_cycles(); // sh7042.cpp:250
        self.internal_update_at(t);
        self.m_in_event = false; // sh7042.cpp:251
    }

    // origin: src/mame/cpu/sh7042.h:76 (u64 event_cycles() const)
    #[inline]
    pub fn event_cycles(&self) -> u64 {
        self.m_event_cycles
    }

    // origin: src/mame/cpu/sh7042.cpp:390-395 (set_internal_interrupt)
    pub fn set_internal_interrupt(&mut self, level: i32, vector: u32) {
        self.dev.core.internal_irq_level = level; // sh7042.cpp:392
        self.dev.core.m_internal_irq_vector = vector as i32; // sh7042.cpp:393
        self.dev.core.m_test_irq = 1; // sh7042.cpp:394
    }

    /// Wiring-row drain seam (row `periph: intc`). Feeds a batch of freshly
    /// drained peripheral IRQ vectors — the sci `irq_req` FIFO (vectors 128-135,
    /// sh_sci.cpp:146..687) and the mtu `irq_req` FIFOs (mtu0 88-92 / mtu1 96-
    /// 101 / mtu2 104-109 / mtu3 112-116 / mtu4 120-124, sh_mtu.cpp:517/523) —
    /// THROUGH the INTC arbiter, then hands each arbitration winner to
    /// [`set_internal_interrupt`] exactly as the disk device does at
    /// sh_intc.cpp:92 (`m_cpu->set_internal_interrupt`). Previously (row
    /// `periph: intc` = todo) these real vectors reached the CPU with no IPR
    /// arbitration; routing here restores priority order without touching the
    /// drain order/timing: `internal_interrupt` calls `update_irq` synchronously
    /// after each latch (:97-98), matching C++, and no CPU instruction runs
    /// mid-drain, so pushing after each vector is bit-identical to the C++
    /// per-call `set_internal_interrupt`.
    ///
    /// Returns the final `(level, vector)` still latched after the batch.
    pub fn route_irqs(&mut self, intc: &mut crate::periph::intc::Sh2Intc, vectors: &[i32]) -> (i32, u32) {
        let mut last = (intc.irq_level, intc.irq_vector);
        for &v in vectors {
            last = intc.internal_interrupt(v); // sh_intc.cpp:96-98
            self.set_internal_interrupt(last.0, last.1); // sh_intc.cpp:92
        }
        last
    }

    /// origin: src/mame/cpu/sh7042.cpp:141-144 (execute_set_input -> intc).
    /// The disk routes the line to the INTC, which re-asserts the CPU input
    /// line through its own wiring (row periph:intc); with no intc attached the
    /// C++ required_device would crash, so `None` here is a dead wire.
    pub fn execute_set_input(&mut self, irqline: i32, state: i8) {
        if let Some(p) = self.bus.periph.as_mut() {
            p.intc_set_input(irqline, state); // sh7042.cpp:143
        }
    }

    /// origin: src/mame/cpu/sh7042.cpp:397-401 (sh2_exception_internal tail).
    /// The CPU-side exception bookkeeping is already on `Sh2Core` (sh2 device
    /// row); this adds the INTC acknowledge the disk performs after it.
    pub fn exception_internal_done(&mut self, irqline: i32, vector: u32) {
        if let Some(p) = self.bus.periph.as_mut() {
            p.intc_interrupt_taken(irqline, vector); // sh7042.cpp:400
        }
    }
}

/// Field-split bus view used by `device_reset` (disjoint &mut self.bus / &mut
/// self.dev; the C++ device receives the program bus as a separate ref).
struct BusCtx<'a> {
    bus: &'a mut Sh7042Bus,
}
impl Sh2Bus for BusCtx<'_> {
    #[inline]
    fn read_byte(&mut self, offset: u32) -> u8 {
        self.bus.read_byte(offset)
    }
    #[inline]
    fn read_word(&mut self, offset: u32) -> u16 {
        self.bus.read_word(offset)
    }
    #[inline]
    fn read_long(&mut self, offset: u32) -> u32 {
        self.bus.read_long(offset)
    }
    #[inline]
    fn write_byte(&mut self, offset: u32, data: u8) {
        self.bus.write_byte(offset, data)
    }
    #[inline]
    fn write_word(&mut self, offset: u32, data: u16) {
        self.bus.write_word(offset, data)
    }
    #[inline]
    fn write_long(&mut self, offset: u32, data: u32) {
        self.bus.write_long(offset, data)
    }
}

// The SoC itself is the program bus (mu2000.cpp:997 set_program_bus(&m_bus)) —
// forwards so the machine row keeps one bus handle.
impl Sh2Bus for Sh7042 {
    #[inline]
    fn read_byte(&mut self, offset: u32) -> u8 {
        self.bus.read_byte(offset)
    }
    #[inline]
    fn read_word(&mut self, offset: u32) -> u16 {
        self.bus.read_word(offset)
    }
    #[inline]
    fn read_long(&mut self, offset: u32) -> u32 {
        self.bus.read_long(offset)
    }
    #[inline]
    fn write_byte(&mut self, offset: u32, data: u8) {
        self.bus.write_byte(offset, data)
    }
    #[inline]
    fn write_word(&mut self, offset: u32, data: u16) {
        self.bus.write_word(offset, data)
    }
    #[inline]
    fn write_long(&mut self, offset: u32, data: u32) {
        self.bus.write_long(offset, data)
    }
}
