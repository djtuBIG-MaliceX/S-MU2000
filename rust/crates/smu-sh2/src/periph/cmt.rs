//! SH Compare-Match Timer subsystem — transliteration of
//! `src/mame/cpu/sh_cmt.cpp` (232 lines) + `src/mame/cpu/sh_cmt.h` (83).
//! Ledger row `periph: cmt` (M2).
//!
//! DIE SHAPE — DISK GROUND TRUTH (sh_cmt.h:63-66, sh7042.cpp:172): this die's
//! CMT is TWO 16-bit channels sharing one flat device (m_str + m_csr[2] +
//! m_cnt[2] + m_cor[2] + m_next_event[2]), NOT the upstream-MAME CMCR/CMCOR/
//! T1MA..T4MB/T32 family. The ledger NEXT row's field list (CMCR, TCOR,
//! T1MA-T4MB, T32) describes upstream sh_cmt.h; NONE of it exists on disk
//! (`rg` on `src/mame/cpu/sh_cmt.*`: no CMCOR, no T32, no MCOR). We transliterate
//! the DISK 2-channel device verbatim — that is the bit-exact ground truth
//! (Invariants #1/#2). `ShCmt` == `sh_cmt_device`.
//!
//! REGISTERS (map sh7042_map.hxx:186-199 r8, 413-419 r16, 687-700 w8,
//! 880-886 w16; base 0xffff83d0, range 83d0-83dd, all 14 bytes decoded, no
//! internal holes; no r32/w32 seam exists for CMT on disk):
//!   83d0/1 CMSTR  83d2/3 CMCSR0  83d4/5 CMCNT0  83d6/7 CMCOR0
//!   83d8/9 CMCSR1  83da/b CMCNT1  83dc/d CMCOR1
//! CMCSR bits (sh_cmt.cpp): 1:0 = CKS clock select, 6 = CMIE (CMI enable),
//! 7 = CMI (compare-match flag, 0x80).
//!
//! PRESCALER (disk-faithful, NO separate CMCOR): the count-clock shift is
//! `3 + 2*BIT(m_csr[clk],0,2)` (sh_cmt.cpp:202,217,222) -> divisor
//! `1 << (3 + 2*cks)` = {8,32,128,512} for CKS {0,1,2,3}. Matches the real
//! SH/SH-2 CMT (P'CLK/8,/32,/128,/512); no buggy bits on this die.
//!
//! IRQ ROUTING (intc seam, per `periph: mtu`/`periph: sci` pattern):
//! `m_intc->internal_interrupt(m_intc_vector[i])` (sh_cmt.cpp:55) -> ordered
//! FIFO [`ShCmt::irq_req`] of ACTUAL vector numbers. Vectors come from the die
//! config `SH_CMT(config, m_cmt, *this, m_intc, 144, 148)` (sh7042.cpp:172):
//! ch0 CMI = 144, ch1 CMI = 148. Both land in intc `pending[4]` (144>>5=4
//! bit16, 148>>5=4 bit20, sh_intc.cpp:97) inside the 64..=159 arbitrated
//! window. [`ShCmt::drain_irqs`] hands the FIFO to the wiring row, which calls
//! `Sh2Intc::internal_interrupt` per entry; intc pending `|=` makes
//! duplicates idempotent, order within a drain preserved.
//!
//! HOST SEAMS (un-ported-device contract, same as mtu.rs/sci.rs):
//! - `m_cpu->current_cycles()` (sh_cmt.cpp:91,97,98,109,115,116,157,158,171,
//!   186,203) -> [`ShCmt::cpu_now`], host-maintained via [`ShCmt::set_cpu_now`]
//!   (the wiring row writes the exact cycle count — compat `current_cycles()`
//!   −1 write-tick semantics, M1 timers row — before any bus op).
//! - `m_cpu->internal_update()` (sh_cmt.cpp:166,180,190) -> sticky
//!   [`ShCmt::resched`]; the wiring row drains via [`ShCmt::take_resched`] and
//!   calls `Sh7042::internal_update()`. NOTE the disk `internal_update`
//!   (:60-81) never reads `current_cycles()` — it drives `catch_up` with the
//!   explicit `current_time`, so [`ShCmt::cmt_update`] does NOT touch `cpu_now`
//!   (faithful: unlike mtu.rs, whose `internal_update` falls back to `cpu_now`,
//!   this device only needs `cpu_now` for the *register* read/write paths).
//! - `logerror` (the commented-out `//logerror` at :196) -> dropped (no state).
//! - `state()` (:226-232, `s.tag("cmt")`) -> M5 serialization row; every field
//!   is `pub` so the serializer can walk it.

use crate::sh7042::Sh7042Peripherals;

/// COMBINE_DATA (sh7042.rs `combine16`, identical to the C++ macro): bits SET
/// in the mask take the new value.
#[inline]
fn combine16(cur: &mut u16, data: u16, mask: u16) {
    *cur = (*cur & !mask) | (data & mask);
}

/// MAME `BIT(value, offset, width)` for the two widths used here.
/// origin: emu.h BIT — `(value >> offset) & ((1 << width) - 1)`.
#[inline]
fn bit(v: u16, off: u32, width: u32) -> u32 {
    ((v as u32 >> off) & ((1u32 << width) - 1)) as u32
}

// ---------------------------------------------------------------------------
// ShCmt — the whole sh_cmt_device (2 channels, flat arrays as on disk).
// ---------------------------------------------------------------------------
pub struct ShCmt {
    // origin: src/mame/cpu/sh_cmt.h:61-66 (device state)
    pub m_next_event: [u64; 2], // :61 (ctor leaves indeterminate -> device_reset:42 zeroes; explicit init per Invariant #3)
    pub m_intc_vector: [i32; 2], // :62 (ctor {0,0}; die passes {144,148} via template ctor — NOT touched by device_reset)
    pub m_str: u16,             // :63 CMSTR, reset 0
    pub m_csr: [u16; 2],        // :64 CMCSR0/1, reset 0
    pub m_cnt: [u16; 2],        // :65 CMCNT0/1, reset 0
    pub m_cor: [u16; 2],        // :66 CMCOR0/1, reset 0xffff

    // ---- host seams (see module doc) ----
    /// stand-in for `m_cpu->current_cycles()` (reads/writes)
    pub cpu_now: u64,
    /// sticky `m_cpu->internal_update()` (:166,180,190)
    pub resched: bool,
    /// FIFO of pending `m_intc->internal_interrupt(vector)` (:55)
    pub irq_req: [i32; 16],
    pub irq_len: usize,
    pub irq_dropped: u64,
}

impl ShCmt {
    /// origin: src/mame/cpu/sh_cmt.h:31-39 (template ctor) +
    /// src/mame/cpu/sh_cmt.cpp:18-28 (base ctor). `vect0`/`vect1` are the two
    /// CMI vector numbers from the die config (`SH_CMT(..., 144, 148)`,
    /// sh7042.cpp:172). device_start (:30-38) is save_items only; the reset
    /// matrix is applied here so every field is explicit (Invariant #3).
    pub fn new(vect0: i32, vect1: i32) -> Self {
        // sh_cmt.cpp:22 m_intc_vector{0,0}, :23 m_str(0), :24 m_csr{0,0},
        // :25 m_cnt{0,0}, :26 m_cor{0xffff,0xffff}; the template ctor then
        // overwrites m_intc_vector with the passed vectors (:37-38).
        let mut d = Self {
            m_next_event: [0, 0], // device_reset:42 (ctor leaves it; we zero it)
            m_intc_vector: [vect0, vect1], // template ctor :37-38
            m_str: 0,
            m_csr: [0, 0],
            m_cnt: [0, 0],
            m_cor: [0xffff, 0xffff],
            cpu_now: 0,
            resched: false,
            irq_req: [0; 16],
            irq_len: 0,
            irq_dropped: 0,
        };
        d.device_reset(); // sh_cmt.cpp:40-47 (idempotent with the above)
        d
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:40-47 (device_reset). Touches ONLY the
    /// five state arrays; never m_intc_vector (that is ctor-only, sh_cmt.h:62).
    pub fn device_reset(&mut self) {
        self.m_next_event = [0, 0]; // :42
        self.m_str = 0; // :43
        self.m_csr = [0, 0]; // :44
        self.m_cnt = [0, 0]; // :45
        self.m_cor = [0xffff, 0xffff]; // :46
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:226-232 (`sh_cmt_device::state`).
    /// Widths from sh_cmt.h:61-66: next_event std::array<u64,2>, str u16,
    /// csr/cnt/cor std::array<u16,2>, disk order :229-231. m_intc_vector is
    /// ctor config — NOT serialized on disk :226-232.
    pub fn state(&mut self, s: &mut smu_compat::StateIo) {
        s.tag("cmt");                   // :228
        s.arr(&mut self.m_next_event);  // :229 stdarr u64 x2
        s.v(&mut self.m_str);           // :230 (h:63 u16)
        s.arr(&mut self.m_csr);         // :231 stdarr u16 x2
        s.arr(&mut self.m_cnt);         // :231
        s.arr(&mut self.m_cor);         // :231
    }

    // ---- host-seam helpers ----

    /// Wiring row writes the exact cycle count here before bus ops (replaces
    /// `m_cpu->current_cycles()`).
    pub fn set_cpu_now(&mut self, t: u64) {
        self.cpu_now = t;
    }

    /// `m_intc->internal_interrupt(vector)` (sh_cmt.cpp:55) — ordered FIFO.
    fn internal_interrupt(&mut self, vector: i32) {
        if self.irq_len < self.irq_req.len() {
            self.irq_req[self.irq_len] = vector;
            self.irq_len += 1;
        } else {
            self.irq_dropped += 1; // unreachable when drained per bus op
        }
    }

    /// Drain this device's pending IRQs into `out` (wiring row -> intc).
    pub fn drain_irqs(&mut self, out: &mut Vec<i32>) -> usize {
        let n = self.irq_len;
        out.extend_from_slice(&self.irq_req[..n]);
        self.irq_len = 0;
        n
    }

    /// Take+clear the sticky resched (:166,180,190).
    pub fn take_resched(&mut self) -> bool {
        let r = self.resched;
        self.resched = false;
        r
    }

    // ---- catch-up engine (S-MU2000's on-demand model) ----

    /// origin: src/mame/cpu/sh_cmt.cpp:50-58 (catch_up). "S-MU2000: 追いつか
    /// せる" — if the read time has passed the scheduled compare match, raise
    /// the CMI flag (:53), fire the CMI IRQ only when CMIE (bit6) is set
    /// (:54-55), then recompute the counter position (:56).
    pub fn catch_up(&mut self, i: usize, current_time: u64) {
        // :52 — m_next_event[i] && current_time >= m_next_event[i]
        if self.m_next_event[i] != 0 && current_time >= self.m_next_event[i] {
            self.m_csr[i] |= 0x80; // :53 CMI flag
            if bit(self.m_csr[i], 6, 1) != 0 {
                // :54 CMIE gated
                self.internal_interrupt(self.m_intc_vector[i]); // :55
            }
            self.cnt_update(i, current_time); // :56
        }
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:60-81 (internal_update). Scheduler seam
    /// (sh7042.cpp:285). Drives catch_up for both channels, then advertises the
    /// next event — but ONLY for channels with CMIE set (bit6). The S-MU2000
    /// comment (:66-75): MU2000 firmware runs CMT at COR=1, CKS=/8 with IRQs
    /// OFF, reading CMCNT as a fine time ruler; scheduling those would stop the
    /// CPU every 16 cycles. Since catch_up reproduces the value on read, an
    /// IRQ-less channel is deliberately left unscheduled. Faithful: does NOT
    /// read current_cycles()/cpu_now (takes explicit current_time).
    pub fn internal_update(&mut self, current_time: u64) -> u64 {
        let mut next: u64 = 0; // :62
        for i in 0..2 {
            self.catch_up(i, current_time); // :64
            if bit(self.m_csr[i], 6, 1) != 0 {
                // :76 only CMIE-enabled channels are scheduled
                if next == 0 || (self.m_next_event[i] != 0 && self.m_next_event[i] < next) {
                    // :77
                    next = self.m_next_event[i]; // :78
                }
            }
        }
        next // :80
    }

    // ---- register accessors (sh_cmt.cpp cmstr/cmcsr/cmcnt/cmcor) ----

    /// origin: src/mame/cpu/sh_cmt.cpp:84-87 (cmstr_r).
    pub fn cmstr_r(&self) -> u16 {
        self.m_str // :86
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:89-93 (cmcsr0_r). "読むときに追いつか
    /// せる" — catch_up at the current cycle, then return CSR0.
    pub fn cmcsr0_r(&mut self) -> u16 {
        let now = self.cpu_now; // :91 m_cpu->current_cycles()
        self.catch_up(0, now); // :91
        self.m_csr[0] // :92
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:95-100 (cmcnt0_r). catch_up then
    /// cnt_update, then return CNT0.
    pub fn cmcnt0_r(&mut self) -> u16 {
        let now = self.cpu_now; // :97
        self.catch_up(0, now); // :97
        self.cnt_update(0, now); // :98
        self.m_cnt[0] // :99
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:102-105 (cmcor0_r).
    pub fn cmcor0_r(&self) -> u16 {
        self.m_cor[0] // :104
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:107-111 (cmcsr1_r).
    pub fn cmcsr1_r(&mut self) -> u16 {
        let now = self.cpu_now; // :109
        self.catch_up(1, now); // :109
        self.m_csr[1] // :110
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:113-118 (cmcnt1_r).
    pub fn cmcnt1_r(&mut self) -> u16 {
        let now = self.cpu_now; // :115
        self.catch_up(1, now); // :115
        self.cnt_update(1, now); // :116
        self.m_cnt[1] // :117
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:120-123 (cmcor1_r).
    pub fn cmcor1_r(&self) -> u16 {
        self.m_cor[1] // :122
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:155-167 (cmstr_w). First catch both
    /// counters up to now (:157-158), then COMBINE (:160); for each channel
    /// that RISES 0->1 start its clock (:162-163), else (bit off) drop its
    /// scheduled event (:164-165); finally request a resched (:166).
    pub fn cmstr_w(&mut self, data: u16, mem_mask: u16) {
        let now = self.cpu_now;
        self.cnt_update(0, now); // :157
        self.cnt_update(1, now); // :158
        let old = self.m_str; // :159
        combine16(&mut self.m_str, data, mem_mask); // :160 COMBINE_DATA
        for i in 0..2 {
            // :161-165
            if bit(old, i as u32, 1) == 0 && bit(self.m_str, i as u32, 1) != 0 {
                self.clock_start(i); // :163
            } else if bit(self.m_str, i as u32, 1) == 0 {
                self.m_next_event[i] = 0; // :165
            }
        }
        self.resched = true; // :166 m_cpu->internal_update()
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:169-173 (csr_w). Catch up first (so the
    /// flag reflects elapsed time before the write), then COMBINE — a write
    /// with bit7 clear in the masked data clears CMI.
    pub fn csr_w(&mut self, reg: usize, data: u16, mem_mask: u16) {
        let now = self.cpu_now; // :171
        self.cnt_update(reg, now); // :171
        combine16(&mut self.m_csr[reg], data, mem_mask); // :172
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:175-182 (cnt_w). NO catch/cnt_update
    /// here (unlike csr_w/cor_w): the count is written raw (:177); only if the
    /// channel is running does it recompute the schedule + resched (:178-181).
    pub fn cnt_w(&mut self, reg: usize, data: u16, mem_mask: u16) {
        combine16(&mut self.m_cnt[reg], data, mem_mask); // :177
        if bit(self.m_str, reg as u32, 1) != 0 {
            // :178
            self.compute_next_event(reg); // :179
            self.resched = true; // :180 m_cpu->internal_update()
        }
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:184-192 (cor_w). Catch up first (:186)
    /// then COMBINE (:187); recompute + resched only when running (:188-191).
    pub fn cor_w(&mut self, reg: usize, data: u16, mem_mask: u16) {
        let now = self.cpu_now; // :186
        self.cnt_update(reg, now); // :186
        combine16(&mut self.m_cor[reg], data, mem_mask); // :187
        if bit(self.m_str, reg as u32, 1) != 0 {
            // :188
            self.compute_next_event(reg); // :189
            self.resched = true; // :190
        }
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:194-198 (clock_start). The logerror
    /// (:196) is commented out on disk; the body is just compute_next_event.
    pub fn clock_start(&mut self, clk: usize) {
        self.compute_next_event(clk); // :197
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:200-211 (compute_next_event). Align the
    /// now-time UP to the count-clock boundary, then add (COR-CNT+1) clock
    /// periods. Prescaler shift = 3 + 2*CKS (:202). `counts` is s32 and wraps
    /// into [0,0x10000) by adding 0x10000 when negative (:206-208).
    pub fn compute_next_event(&mut self, clk: usize) {
        let shift = 3 + 2 * bit(self.m_csr[clk], 0, 2); // :202 (3 + 2*BIT(csr,0,2))
        let step1: u64 = 1u64 << shift; // :202
        let mut time = self.cpu_now; // :203 m_cpu->current_cycles()
        if time & (step1 - 1) != 0 {
            // :204 round up to the next count-clock boundary
            time = (time | (step1 - 1)) + 1; // :205
        }
        let mut counts: i32 = self.m_cor[clk] as i32 + 1 - self.m_cnt[clk] as i32; // :206 (s32)
        if counts < 0 {
            counts += 0x10000; // :207-208
        }
        time += step1 * (counts as u64); // :209
        self.m_next_event[clk] = time; // :210
    }

    /// origin: src/mame/cpu/sh_cmt.cpp:213-224 (cnt_update). If stopped, no-op
    /// (:215-216). Advance the scheduled event past `current_time` one whole
    /// match period at a time (:218-220), then back-compute the live count
    /// from the remaining distance (:221-222). Count-clock shift = 3+2*CKS
    /// (:217,:222).
    pub fn cnt_update(&mut self, clk: usize, current_time: u64) {
        if bit(self.m_str, clk as u32, 1) == 0 {
            return; // :215-216
        }
        let shift = 3 + 2 * bit(self.m_csr[clk], 0, 2); // :217
        let step: u64 = ((self.m_cor[clk] as u64) + 1) << shift; // :217 (cor+1) << shift
        if self.m_next_event[clk] != 0 {
            // :218
            while current_time >= self.m_next_event[clk] {
                self.m_next_event[clk] += step; // :219-220
            }
            let delta = self.m_next_event[clk] - current_time; // :221 (>0)
            // :222 m_cnt = m_cor - ((delta-1) >> shift); u16 result (disk is
            // u16 - u64 assigned back to u16 -> wrap to 16 bits).
            self.m_cnt[clk] =
                ((self.m_cor[clk] as u64).wrapping_sub((delta - 1) >> shift)) as u16; // :222
        }
    }
}

// ---------------------------------------------------------------------------
// Sh7042Peripherals seam — the map case-sets (absolute addresses, as the C++
// bus hands them, membus.h:36). Byte writes carry the shifted byte + mask
// exactly as sh7042_map.hxx builds them (e.g. w8 high: data=v<<8, mask=0xff00).
// ---------------------------------------------------------------------------
impl Sh7042Peripherals for ShCmt {
    /// origin: src/mame/cpu/sh7042_map.hxx:186-199 (internal_r8, CMT arms).
    /// Each byte arm invokes the full disk accessor (so a byte read of CMCNT
    /// runs catch_up+cnt_update, exactly as the map calling cmcntN_r()).
    fn cmt_r8(&mut self, a: u32) -> u8 {
        match a {
            0xffff83d0 => (self.cmstr_r() >> 8) as u8, // map:186
            0xffff83d1 => self.cmstr_r() as u8, // map:187
            0xffff83d2 => (self.cmcsr0_r() >> 8) as u8, // map:188
            0xffff83d3 => self.cmcsr0_r() as u8, // map:189
            0xffff83d4 => (self.cmcnt0_r() >> 8) as u8, // map:190
            0xffff83d5 => self.cmcnt0_r() as u8, // map:191
            0xffff83d6 => (self.cmcor0_r() >> 8) as u8, // map:192
            0xffff83d7 => self.cmcor0_r() as u8, // map:193
            0xffff83d8 => (self.cmcsr1_r() >> 8) as u8, // map:194
            0xffff83d9 => self.cmcsr1_r() as u8, // map:195
            0xffff83da => (self.cmcnt1_r() >> 8) as u8, // map:196
            0xffff83db => self.cmcnt1_r() as u8, // map:197
            0xffff83dc => (self.cmcor1_r() >> 8) as u8, // map:198
            0xffff83dd => self.cmcor1_r() as u8, // map:199
            _ => 0, // bus miss / hole (sh7042.rs range is 83d0-83dd)
        }
    }

    /// origin: src/mame/cpu/sh7042_map.hxx:413-419 (internal_r16, CMT arms).
    /// Even-word reads of the 16-bit registers directly.
    fn cmt_r16(&mut self, a: u32) -> u16 {
        match a {
            0xffff83d0 => self.cmstr_r(), // map:413
            0xffff83d2 => self.cmcsr0_r(), // map:414
            0xffff83d4 => self.cmcnt0_r(), // map:415
            0xffff83d6 => self.cmcor0_r(), // map:416
            0xffff83d8 => self.cmcsr1_r(), // map:417
            0xffff83da => self.cmcnt1_r(), // map:418
            0xffff83dc => self.cmcor1_r(), // map:419
            _ => 0,
        }
    }

    /// origin: src/mame/cpu/sh7042_map.hxx:687-700 (internal_w8, CMT arms).
    /// The map shifts the byte into position and masks it; mirror verbatim.
    fn cmt_w8(&mut self, a: u32, v: u8) {
        let vv = v as u16;
        match a {
            0xffff83d0 => self.cmstr_w(vv << 8, 0xff << 8), // map:687
            0xffff83d1 => self.cmstr_w(vv, 0xff), // map:688
            0xffff83d2 => self.csr_w(0, vv << 8, 0xff << 8), // map:689 cmcsr0_w
            0xffff83d3 => self.csr_w(0, vv, 0xff), // map:690
            0xffff83d4 => self.cnt_w(0, vv << 8, 0xff << 8), // map:691 cmcnt0_w
            0xffff83d5 => self.cnt_w(0, vv, 0xff), // map:692
            0xffff83d6 => self.cor_w(0, vv << 8, 0xff << 8), // map:693 cmcor0_w
            0xffff83d7 => self.cor_w(0, vv, 0xff), // map:694
            0xffff83d8 => self.csr_w(1, vv << 8, 0xff << 8), // map:695 cmcsr1_w
            0xffff83d9 => self.csr_w(1, vv, 0xff), // map:696
            0xffff83da => self.cnt_w(1, vv << 8, 0xff << 8), // map:697 cmcnt1_w
            0xffff83db => self.cnt_w(1, vv, 0xff), // map:698
            0xffff83dc => self.cor_w(1, vv << 8, 0xff << 8), // map:699 cmcor1_w
            0xffff83dd => self.cor_w(1, vv, 0xff), // map:700
            _ => {}
        }
    }

    /// origin: src/mame/cpu/sh7042_map.hxx:880-886 (internal_w16, CMT arms).
    fn cmt_w16(&mut self, a: u32, v: u16) {
        match a {
            0xffff83d0 => self.cmstr_w(v, 0xffff), // map:880
            0xffff83d2 => self.csr_w(0, v, 0xffff), // map:881
            0xffff83d4 => self.cnt_w(0, v, 0xffff), // map:882
            0xffff83d6 => self.cor_w(0, v, 0xffff), // map:883
            0xffff83d8 => self.csr_w(1, v, 0xffff), // map:884
            0xffff83da => self.cnt_w(1, v, 0xffff), // map:885
            0xffff83dc => self.cor_w(1, v, 0xffff), // map:886
            _ => {}
        }
    }

    /// origin: src/mame/cpu/sh7042.cpp:285 — scheduler seam `add_event(
    /// event_time, m_cmt->internal_update(current_time))`.
    fn cmt_update(&mut self, current_time: u64) -> u64 {
        self.internal_update(current_time) // sh_cmt.cpp:60
    }
}
