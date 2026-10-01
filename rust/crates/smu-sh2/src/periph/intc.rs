//! On-die SH7042 interrupt controller (ledger row `periph: intc`, M2).
//!
//! Faithful transliteration of `src/mame/cpu/sh_intc.{h,cpp}` — the disk class
//! is `sh_intc_device` (sh_intc.h:22). There is NO `sh7054_icu_device` in this
//! tree (0 rg hits); the SH7043 ICU here is exactly this `sh_intc_device`.
//!
//! HOST-INTERACTION DESIGN (same borrow-split as `sci.rs`/`mtu.rs` — the C++
//! device holds a `required_device<sh7042_device> m_cpu` back-ref and calls
//! `m_cpu->set_internal_interrupt(level, vector)` directly from `update_irq`
//! (sh_intc.cpp:92). Rust has no back-ref, so [`Sh2Intc::update_irq`] RETURNS
//! the `(level, vector)` pair that C++ would have pushed; the wiring row pushes
//! it into [`smu_sh2::sh7042::Sh7042::set_internal_interrupt`] (sh7042.cpp:390-
//! 395). The drain of the sci/mtu `irq_req` FIFOs (sci 128-135 / mtu 88-124)
//! feeds [`Sh2Intc::internal_interrupt`] (:96-98), which is the whole point of
//! this row — vectors are now arbitrated by IPR priority instead of reaching the
//! CPU straight (see sh7042.rs `route_irqs`).
//!
//! FIELD EXPLICITNESS (Invariant 3): the C++ `std::array<u32,8> m_pending` is a
//! non-initialized POD member (sh_intc.h:48) — `device_start` only `save_item`s
//! it and `device_reset` fills it (sh_intc.cpp:48,59). Rust `new()` mirrors the
//! full start-then-reset lifecycle and zeroes EVERY field explicitly.

/// origin: src/mame/cpu/sh_intc.h:46 + sh_intc.cpp:18-35 (`pribit[0x100]`).
/// Maps a vector number to its IPR "slot": `slot>>2` selects the IPR register
/// (IPRA..IPRH), `12-4*(slot&3)` selects the 4-bit priority nibble. Verbatim.
pub const PRIBIT: [u8; 256] = [
    // 0x00..0x3f — arbitration loop (sh_intc.cpp:76) never scans these
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x00
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x10
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x20
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x30
    // 0x40.. external IRQ0-7 (64-71) then first autovector group
    0, 1, 2, 3, 4, 5, 6, 7, 8, 8, 8, 8, 9, 9, 9, 9, // 0x40
    10, 10, 10, 10, 11, 11, 11, 11, 12, 12, 12, 12, 13, 13, 13, 13, // 0x50
    14, 14, 14, 14, 15, 15, 15, 15, 16, 16, 16, 16, 17, 17, 17, 17, // 0x60
    18, 18, 18, 18, 19, 19, 19, 19, 20, 20, 20, 20, 21, 21, 21, 21, // 0x70
    22, 22, 22, 22, 23, 23, 23, 23, 24, 24, 24, 24, 25, 25, 25, 25, // 0x80
    26, 26, 26, 26, 27, 27, 27, 27, 28, 28, 28, 28, 29, 29, 29, 29, // 0x90
    // 0xa0.. saturate to slot 29 (vectors >= 160 are outside the loop anyway)
    29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, // 0xa0
    29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, // 0xb0
    29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, // 0xc0
    29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, // 0xd0
    29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, // 0xe0
    29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, 29, // 0xf0
];

/// origin: src/mame/cpu/sh_intc.h:22 `class sh_intc_device`.
pub struct Sh2Intc {
    /// origin: sh_intc.h:48 `std::array<u32, 8> m_pending` — pending latch,
    /// one bit per vector (index = vector>>5, bit = vector&31).
    pub m_pending: [u32; 8],
    /// origin: sh_intc.h:49 `std::array<u16, 8> m_ipr` — IPRA..IPRH.
    pub m_ipr: [u16; 8],
    /// origin: sh_intc.h:51 `u16 m_isr` (read-only status; only bus writes set it).
    pub m_isr: u16,
    /// origin: sh_intc.h:51 `u16 m_icr` — control; bit 7-inputnum = 0 level,
    /// 1 falling-edge for external IRQn (SH7043 spec, sh_intc.cpp:112-114).
    pub m_icr: u16,
    /// origin: sh_intc.h:53 `u8 m_lines` — remembered external-line levels
    /// (S-MU2000 note sh_intc.cpp:105: without this the level/edge split and
    /// the same-value early-out at :103 both break).
    pub m_lines: u8,
    /// last `(level, vector)` handed to the CPU (`m_cpu->set_internal_interrupt`
    /// argument pair, sh_intc.cpp:92). Init -1/0 mirrors the CPU's
    /// `internal_irq_level = -1` reset value (core.rs:271 / sh2.cpp).
    pub irq_level: i32,
    pub irq_vector: u32,
}

impl Default for Sh2Intc {
    fn default() -> Self {
        Self::new()
    }
}

impl Sh2Intc {
    /// Mirror of the C++ `device_start()` (:43-55) THEN `device_reset()`
    /// (:57-60) lifecycle, with every field written explicitly (Invariant 3;
    /// the C++ `m_pending` is a raw POD member that only `device_reset` fills).
    /// origin: src/mame/cpu/sh_intc.cpp:43-60.
    pub fn new() -> Self {
        Sh2Intc {
            m_pending: [0; 8], // :59 device_reset std::fill
            m_ipr: [0; 8],     // :51 device_start std::fill(m_ipr, 0)
            m_isr: 0,          // :52
            m_icr: 0,          // :53
            m_lines: 0,        // :54
            irq_level: -1,     // CPU internal_irq_level reset (core.rs:271)
            irq_vector: 0,
        }
    }

    /// origin: src/mame/cpu/sh_intc.cpp:57-60 (`device_reset`). Clears only the
    /// pending latch (ipr/icr/isr/lines survive a reset on disk).
    pub fn device_reset(&mut self) {
        self.m_pending = [0; 8]; // :59
    }

    #[inline]
    fn bit(x: u32, n: u32) -> u32 {
        (x >> n) & 1
    }

    /// origin: src/mame/cpu/sh_intc.cpp:71-93 (`update_irq`).
    ///
    /// Loop is `for (bv = 64/32; bv != 160/32; bv++)` → bv ∈ {2,3,4}, i.e. only
    /// vectors 64..=159 are ever arbitrated (vectors 0-63 / 160-255 are latched
    /// but ignored — sh_intc.cpp:76). Inner scan iv is ASCENDING and the pick is
    /// `if (level > best_level)` (:86, STRICT): highest-priority pending source
    /// wins, and among equal-priority sources the LOWEST vector number wins
    /// (first writer at that level is never displaced because the next equal
    /// level is not `>` it). best_level starts at -1 (:73) so a priority-0 source
    /// still "wins" against an otherwise-idle arbiter — the CPU's SR.IMASK
    /// compare (sh2.cpp / core.rs:1889 `irqline <= mask` -> return) is what
    /// actually drops level-0 (priority 0 == disabled per SH7043).
    ///
    /// Returns the `(best_level, best_vector)` pair the C++ hands to
    /// `m_cpu->set_internal_interrupt` (:92) and caches it in `irq_*`.
    pub fn update_irq(&mut self) -> (i32, u32) {
        let mut best_level: i32 = -1; // :73
        let mut best_vector: u32 = 0; // :74

        let mut bv: u32 = 64 / 32; // :76 (== 2)
        while bv != 160 / 32 {
            // :76 (bv != 5)
            if self.m_pending[bv as usize] == 0 {
                // :77-78
                bv += 1;
                continue;
            }
            let mut iv: u32 = 0; // :79
            while iv != 32 {
                if Self::bit(self.m_pending[bv as usize], iv) == 0 {
                    // :80-81
                    iv += 1;
                    continue;
                }
                let vector = bv * 32 + iv; // :82
                let slot = PRIBIT[vector as usize] as u32; // :83
                let shift = 12 - 4 * (slot & 3); // :84
                let level = ((self.m_ipr[(slot >> 2) as usize] >> shift) & 15) as i32; // :85
                if level > best_level {
                    // :86-89 (strict > -> tie broken by lowest vector)
                    best_level = level;
                    best_vector = vector;
                }
                iv += 1;
            }
            bv += 1;
        }

        self.irq_level = best_level;
        self.irq_vector = best_vector;
        (best_level, best_vector) // :92 argument pair
    }

    /// origin: src/mame/cpu/sh_intc.cpp:95-99 (`internal_interrupt`). Latches
    /// the source then re-arbitrates. `|=` makes repeat raises idempotent.
    pub fn internal_interrupt(&mut self, vector: i32) -> (i32, u32) {
        let v = vector as usize;
        self.m_pending[v >> 5] |= 1 << (v & 31); // :97
        self.update_irq() // :98
    }

    /// Prompt-facing alias for [`Sh2Intc::internal_interrupt`]. The disk
    /// `internal_interrupt(vector)` has NO level argument — it only ever RAISES
    /// (:96-98); there is no disk "lower" (deassertion happens through
    /// [`Sh2Intc::set_input`] for level-sensitive external lines, or via
    /// [`Sh2Intc::interrupt_taken`] ack). So `assert == 0` is a documented
    /// no-op (returns the current winner unchanged), NOT a silent clear.
    pub fn set_source(&mut self, vector: i32, assert: bool) -> (i32, u32) {
        if assert {
            self.internal_interrupt(vector) // :96-98
        } else {
            (self.irq_level, self.irq_vector) // disk has no lower primitive here
        }
    }

    /// origin: src/mame/cpu/sh_intc.cpp:62-69 (`interrupt_taken`).
    ///
    /// `irqline` is UNUSED on disk — the clear decision is made purely from
    /// `vector` (:65-66). It clears the pending bit UNLESS the source is a
    /// level-triggered external IRQ (vector 64-71, ICR bit says level) whose
    /// line is still asserted (`m_lines` bit high) — that one stays pending so
    /// it re-fires (sh_intc.cpp:64 comment + :105-107 S-MU2000 note). Every
    /// internal (sci/mtu) vector is >= 72 so it always clears.
    pub fn interrupt_taken(&mut self, irqline: i32, vector: i32) -> (i32, u32) {
        let _ = irqline; // :62 param unused on disk (only `vector` is read)
        let v = vector as usize;
        let keep = !(v < 64 || v >= 72 || Self::bit(self.m_icr as u32, 7 - (v as u32 & 7)) != 0
            || Self::bit(self.m_lines as u32, v as u32 & 7) == 0); // :65
        if !keep {
            self.m_pending[v >> 5] &= !(1 << (v & 31)); // :66
        }
        self.update_irq() // :68
    }

    /// Prompt-facing ack: the CPU-side tail `m_intc->interrupt_taken(irqline,
    /// vector)` (sh7042.cpp:400). `irqline` is unused by disk (see above), so a
    /// vector-only ack is faithful.
    pub fn ack(&mut self, vector: i32) -> (i32, u32) {
        self.interrupt_taken(vector, vector) // sh7042.cpp:400 -> :62-69
    }

    /// origin: src/mame/cpu/sh_intc.cpp:101-126 (`set_input`). Drives one of
    /// the 8 external IRQ lines (vectors 64-71; inputnum 0-7). Same-value calls
    /// early-out (:103). Level mode (ICR bit 0) tracks the line both ways
    /// (:116-119); edge mode (:120-124) latches on the falling edge (assert)
    /// only — a 0->1 transition sets pending, a drop does not clear it.
    pub fn set_input(&mut self, inputnum: u32, state: i32) -> (i32, u32) {
        let cur = Self::bit(self.m_lines as u32, inputnum); // :103
        if cur == state as u32 {
            return (self.irq_level, self.irq_vector); // :103-104 early-out
        }
        if state != 0 {
            self.m_lines |= 1 << inputnum; // :108-109
        } else {
            self.m_lines &= !(1 << inputnum); // :110-111
        }
        if Self::bit(self.m_icr as u32, 7 - inputnum) == 0 {
            // :114 level (S-MU2000: 0=level, 1=falling — sh_intc.cpp:112-113)
            if state != 0 {
                self.m_pending[64 >> 5] |= 1 << inputnum; // :116-117
            } else {
                self.m_pending[64 >> 5] &= !(1 << inputnum); // :118-119
            }
        } else if state != 0 {
            // :120-123 edge: assert latches, deassert does nothing
            self.m_pending[64 >> 5] |= 1 << inputnum;
        }
        self.update_irq() // :125
    }

    /// Is `vector` currently latched? (test/harness helper, not on disk.)
    pub fn pending(&self, vector: usize) -> bool {
        Self::bit(self.m_pending[vector >> 5], (vector & 31) as u32) != 0
    }

    /// Given the CPU's current interrupt mask (SR.IMASK == `(sr>>4)&15`),
    /// return the vector that would be taken, or -1. Mirrors the disk split:
    /// the INTC produces `best_level`/`best_vector`, and the CPU drops it when
    /// `level <= mask` (core.rs:1889 / sh2.cpp `sh2_exception`).
    pub fn effective_vector(&self, cpu_mask: i32) -> i32 {
        if self.irq_level > cpu_mask {
            self.irq_vector as i32
        } else {
            -1
        }
    }

    // ---------------------------------------------------------------------
    // register file — bus decode mirrors sh7042_map.hxx r8/r16/w8/w16 cases.
    // `a` is the ABSOLUTE address (already gated to 0xffff8348..835b by the
    // sh7042.rs dispatcher; r16/w16 only ever see even addresses there).
    // ---------------------------------------------------------------------

    /// origin: src/mame/cpu/sh_intc.cpp:150-153 (`ipr_r`).
    #[inline]
    pub fn ipr_r(&self, offset: usize) -> u16 {
        self.m_ipr[offset] // :152
    }

    /// origin: src/mame/cpu/sh_intc.cpp:155-165 (`ipr_w`). COMBINE_DATA then
    /// RE-ARBITRATE (:159-164 S-MU2000 note: raising/lowering a priority while a
    /// source is pending must re-pick, else a parked source never re-fires — the
    /// testxg.mid USB-stall bug).
    pub fn ipr_w(&mut self, offset: usize, data: u16, mem_mask: u16) -> (i32, u32) {
        Self::combine(data, mem_mask, &mut self.m_ipr[offset]); // :157
        self.update_irq() // :164
    }

    /// origin: src/mame/cpu/sh_intc.cpp:128-131 (`icr_r`).
    #[inline]
    pub fn icr_r(&self) -> u16 {
        self.m_icr // :130
    }

    /// origin: src/mame/cpu/sh_intc.cpp:133-137 (`icr_w`). COMBINE only —
    /// NO update_irq on disk (:135-136; ICR affects only future set_input /
    /// interrupt_taken edge/level decisions, not the current pick). logerror
    /// dropped (LOG, verbose off in ground truth).
    pub fn icr_w(&mut self, data: u16, mem_mask: u16) {
        Self::combine(data, mem_mask, &mut self.m_icr); // :135
    }

    /// origin: src/mame/cpu/sh_intc.cpp:139-142 (`isr_r`).
    #[inline]
    pub fn isr_r(&self) -> u16 {
        self.m_isr // :141
    }

    /// origin: src/mame/cpu/sh_intc.cpp:144-148 (`isr_w`). COMBINE only.
    pub fn isr_w(&mut self, data: u16, mem_mask: u16) {
        Self::combine(data, mem_mask, &mut self.m_isr); // :146
    }

    /// `COMBINE_DATA(&dst)` (mamecompat): `dst = (dst & ~mask) | (data & mask)`.
    /// `data` is already shifted into place by the map caller.
    #[inline]
    fn combine(data: u16, mask: u16, dst: &mut u16) {
        *dst = (*dst & !mask) | (data & mask);
    }

    // ---- absolute-address bus decode (sh7042_map.hxx) ----

    /// origin: sh7042_map.hxx:112-131 (r8).
    pub fn r8(&self, a: u32) -> u8 {
        match a {
            0xffff8348 => (self.ipr_r(0) >> 8) as u8, // map:112
            0xffff8349 => self.ipr_r(0) as u8, // map:113
            0xffff834a => (self.ipr_r(1) >> 8) as u8, // map:114
            0xffff834b => self.ipr_r(1) as u8, // map:115
            0xffff834c => (self.ipr_r(2) >> 8) as u8, // map:116
            0xffff834d => self.ipr_r(2) as u8, // map:117
            0xffff834e => (self.ipr_r(3) >> 8) as u8, // map:118
            0xffff834f => self.ipr_r(3) as u8, // map:119
            0xffff8350 => (self.ipr_r(4) >> 8) as u8, // map:120
            0xffff8351 => self.ipr_r(4) as u8, // map:121
            0xffff8352 => (self.ipr_r(5) >> 8) as u8, // map:122
            0xffff8353 => self.ipr_r(5) as u8, // map:123
            0xffff8354 => (self.ipr_r(6) >> 8) as u8, // map:124
            0xffff8355 => self.ipr_r(6) as u8, // map:125
            0xffff8356 => (self.ipr_r(7) >> 8) as u8, // map:126
            0xffff8357 => self.ipr_r(7) as u8, // map:127
            0xffff8358 => (self.icr_r() >> 8) as u8, // map:128
            0xffff8359 => self.icr_r() as u8, // map:129
            0xffff835a => (self.isr_r() >> 8) as u8, // map:130
            0xffff835b => self.isr_r() as u8, // map:131
            _ => 0,
        }
    }

    /// origin: sh7042_map.hxx:376-385 (r16, even addresses only).
    pub fn r16(&self, a: u32) -> u16 {
        match a {
            0xffff8348 => self.ipr_r(0), // map:376
            0xffff834a => self.ipr_r(1), // map:377
            0xffff834c => self.ipr_r(2), // map:378
            0xffff834e => self.ipr_r(3), // map:379
            0xffff8350 => self.ipr_r(4), // map:380
            0xffff8352 => self.ipr_r(5), // map:381
            0xffff8354 => self.ipr_r(6), // map:382
            0xffff8356 => self.ipr_r(7), // map:383
            0xffff8358 => self.icr_r(), // map:384
            0xffff835a => self.isr_r(), // map:385
            _ => 0,
        }
    }

    /// origin: sh7042_map.hxx:615-634 (w8). Returns the arbitration delta for
    /// IPR writes (0,0 sentinel ignored by the caller when re-arbitrating).
    pub fn w8(&mut self, a: u32, v: u8) -> Option<(i32, u32)> {
        let vv = v as u16;
        match a {
            // ipr_w(index, v<<8/<<0, mask) — the two bytes of each IPRx
            0xffff8348 => Some(self.ipr_w(0, vv << 8, 0xff00)), // map:615
            0xffff8349 => Some(self.ipr_w(0, vv, 0x00ff)), // map:616
            0xffff834a => Some(self.ipr_w(1, vv << 8, 0xff00)), // map:617
            0xffff834b => Some(self.ipr_w(1, vv, 0x00ff)), // map:618
            0xffff834c => Some(self.ipr_w(2, vv << 8, 0xff00)), // map:619
            0xffff834d => Some(self.ipr_w(2, vv, 0x00ff)), // map:620
            0xffff834e => Some(self.ipr_w(3, vv << 8, 0xff00)), // map:621
            0xffff834f => Some(self.ipr_w(3, vv, 0x00ff)), // map:622
            0xffff8350 => Some(self.ipr_w(4, vv << 8, 0xff00)), // map:623
            0xffff8351 => Some(self.ipr_w(4, vv, 0x00ff)), // map:624
            0xffff8352 => Some(self.ipr_w(5, vv << 8, 0xff00)), // map:625
            0xffff8353 => Some(self.ipr_w(5, vv, 0x00ff)), // map:626
            0xffff8354 => Some(self.ipr_w(6, vv << 8, 0xff00)), // map:627
            0xffff8355 => Some(self.ipr_w(6, vv, 0x00ff)), // map:628
            0xffff8356 => Some(self.ipr_w(7, vv << 8, 0xff00)), // map:629
            0xffff8357 => Some(self.ipr_w(7, vv, 0x00ff)), // map:630
            0xffff8358 => {
                self.icr_w(vv << 8, 0xff00); // map:631
                None
            }
            0xffff8359 => {
                self.icr_w(vv, 0x00ff); // map:632
                None
            }
            0xffff835a => {
                self.isr_w(vv << 8, 0xff00); // map:633
                None
            }
            0xffff835b => {
                self.isr_w(vv, 0x00ff); // map:634
                None
            }
            _ => None,
        }
    }

    /// origin: sh7042_map.hxx:844-853 (w16, even addresses only).
    pub fn w16(&mut self, a: u32, v: u16) -> Option<(i32, u32)> {
        match a {
            0xffff8348 => Some(self.ipr_w(0, v, 0xffff)), // map:844
            0xffff834a => Some(self.ipr_w(1, v, 0xffff)), // map:845
            0xffff834c => Some(self.ipr_w(2, v, 0xffff)), // map:846
            0xffff834e => Some(self.ipr_w(3, v, 0xffff)), // map:847
            0xffff8350 => Some(self.ipr_w(4, v, 0xffff)), // map:848
            0xffff8352 => Some(self.ipr_w(5, v, 0xffff)), // map:849
            0xffff8354 => Some(self.ipr_w(6, v, 0xffff)), // map:850
            0xffff8356 => Some(self.ipr_w(7, v, 0xffff)), // map:851
            0xffff8358 => {
                self.icr_w(v, 0xffff); // map:852
                None
            }
            0xffff835a => {
                self.isr_w(v, 0xffff); // map:853
                None
            }
            _ => None,
        }
    }

    // deferred: src/mame/cpu/sh_intc.cpp:168-173 (state()) -> M5 state.rs.
    // s.tag("intc"); stdarr(m_pending); stdarr(m_ipr); v(m_isr); v(m_icr);
    // v(m_lines). All fields above are already pub + explicit for that mirror.
}
