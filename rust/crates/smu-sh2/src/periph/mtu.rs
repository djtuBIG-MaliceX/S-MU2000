//! SH Multifunction Timer Pulse unit — transliteration of
//! `src/mame/cpu/sh_mtu.cpp` (549 lines) + `src/mame/cpu/sh_mtu.h` (196).
//! Ledger row `periph: mtu` (M2).
//!
//! DIE SHAPE (sh7042.cpp:52-57,178-229): one `sh_mtu_device` (shared regs)
//! + five `sh_mtu_channel_device`s (mtu0..mtu4). `Sh2Mtu` owns both; it IS
//! the `Sh7042Peripherals` seam the sh7042.rs `mtu_*` case-sets dispatch into
//! (map:27-111 r8, 334-375 r16, 530-614 w8, 802-843 w16 — the odd/even holes
//! and the TIOR double-address quirk are enforced by the sh7042.rs arms AND
//! re-enforced in the case-sets below).
//!
//! mu2000.cpp has ZERO `mtu` references (disk-verified rg): no glue/firmware
//! loop drives it. The ROM running on the SH-2 programs it purely through
//! memory-mapped writes to 0xffff82xx; IRQs leave through the INTC (row
//! `periph: intc`, not yet ported). No `midi`/`boot` interplay is invented.
//!
//! HOST-INTERACTION DESIGN (borrow-split for the C++ `m_cpu`/`m_intc` device
//! refs — same pattern as `sci.rs`; the wiring row drains these):
//! - `m_cpu->current_cycles()` (sh_mtu.cpp:423,470) -> [`Sh2MtuChannel::cpu_now`],
//!   host-maintained. Contract: the wiring row calls [`Sh2Mtu::set_cpu_now`]
//!   with the exact cycle count (compat `current_cycles()` −1 write-tick
//!   semantics, M1 timers row) BEFORE any register write. `mtu_ch_update`
//!   (the scheduler seam, sh7042.cpp:286-290) re-syncs channel `ch` to the
//!   chain's `current_time` — in C++ that call comes from the timer callback
//!   at exactly that time (sh7042.cpp:244), so `internal_update`'s
//!   `update_counter(m_event_time)` / `recalc_event(m_event_time)` are fed the
//!   explicit time and never fall back to `cpu_now`.
//! - `m_cpu->internal_update()` (sh_mtu.cpp:417,461) -> sticky
//!   [`Sh2MtuChannel::resched`]; wiring drains via [`Sh2Mtu::take_resched`]
//!   after each instruction/tick and calls `Sh7042::internal_update()`. The
//!   :417/:461 sites fire only when `update_cpu` (cur_time==0, i.e. a register
//!   write), so the in-`internal_update` recursion (recalc called with the
//!   explicit event time) produces no extra resched — matching the C++ where
//!   `internal_update` recursion re-enters the same chain already advanced.
//! - `m_intc->internal_interrupt(v)` (sh_mtu.cpp:517,523) -> ordered FIFO
//!   [`Sh2MtuChannel::irq_req`] of ACTUAL vector numbers; the vectors are
//!   computed per channel from `irq_base` exactly as the sh_mtu.h template
//!   ctor (see [`Sh2MtuChannel::new`]). INTC pending is a bitmask
//!   (sh_intc.cpp:97 `|=`), so duplicates across drains are idempotent;
//!   order within a drain is preserved. [`Sh2Mtu::drain_irqs`] concatenates
//!   all five channel FIFOs (they share one INTC on disk).
//! - `exit(1)` on the decrementing-counter path (:457,:527) -> `panic!`
//!   (firmware never sets `TMDR[CRM]`=decrement here; kept as a loud trap).
//! - `logerror`/`if(V>=1) logerror` (:55,:62,:232,:236,:242,:247,:255,:262,
//!   :266,:271,:280,:286,:299,:322,:388-390,:456,:504,:511,:526): all dropped
//!   per the M1-bus / sh7042-row stderr deviation (logs carry no machine
//!   state). The `if(0) logerror` at :321-328 is dead on disk.
//! - `machine().side_effects_disabled()` (:346): this build never reads
//!   registers with side effects disabled (no such probe), so `tcnt_r` always
//!   advances the counter — faithful to every observed call site.
//! - `state()` (:531-549) -> M5 serialization row (every field is `pub`).
//! - `m_chained_timer` (CHAIN mode, mtu1->mtu2): on disk `update_counter` has
//!   NO CHAIN handling and `internal_update` never drives it; CHAIN only sets
//!   `m_clock_type=CHAIN` (:277) which then makes `update_counter`/`recalc_event`
//!   no-op (:414,:466), i.e. a channel clocked by a chain NEVER advances in
//!   this port. Reproduced verbatim — nothing is invented for chain tick propagation.

use crate::sh7042::Sh7042Peripherals;

// origin: src/mame/cpu/sh_mtu.h:29-48 (count-type enum)
pub const CHAIN: i32 = 0;
pub const INPUT_A: i32 = 1;
pub const INPUT_B: i32 = 2;
pub const INPUT_C: i32 = 3;
pub const INPUT_D: i32 = 4;
pub const DIV_1: i32 = 5;
pub const DIV_2: i32 = 6;
pub const DIV_4: i32 = 7;
pub const DIV_8: i32 = 8;
pub const DIV_16: i32 = 9;
pub const DIV_32: i32 = 10;
pub const DIV_64: i32 = 11;
pub const DIV_128: i32 = 12;
pub const DIV_256: i32 = 13;
pub const DIV_512: i32 = 14;
pub const DIV_1024: i32 = 15;
pub const DIV_2048: i32 = 16;
pub const DIV_4096: i32 = 17;

// origin: src/mame/cpu/sh_mtu.h:50-53 (TGR clear sentinels)
pub const TGR_CLEAR_NONE: i32 = -1;
pub const TGR_CLEAR_EXT: i32 = -2;

// origin: src/mame/cpu/sh_mtu.h:55-63 (TIER / TSR bit names)
pub const IRQ_A: u8 = 0x01;
pub const IRQ_B: u8 = 0x02;
pub const IRQ_C: u8 = 0x04;
pub const IRQ_D: u8 = 0x08;
pub const IRQ_V: u8 = 0x10; // TOVF (overflow)
pub const IRQ_U: u8 = 0x20;
pub const IRQ_E: u8 = 0x80;

// ---------------------------------------------------------------------------
// Sh2MtuChannel — one MTU channel (sh_mtu_channel_device). Every field is an
// explicit init (Invariant 3): config comes from `new` (template ctor), the
// rest from `device_reset` (the reset matrix).
// ---------------------------------------------------------------------------
pub struct Sh2MtuChannel {
    // ---- config (sh_mtu.h:67-91 template ctor; never touched by reset) ----
    pub m_interrupt: [i32; 6], // sh_mtu.h:120 — computed from irq_base+mask
    pub m_tier_mask: u8, // sh_mtu.h:121
    pub m_tgr_count: i32, // sh_mtu.h:123
    pub m_tbr_count: i32, // sh_mtu.h:123 (always 0 on this die)
    pub m_count_types: [i32; 8], // sh_mtu.h:133 (t0..t7 per channel)
    pub m_chained: bool, // stand-in for m_chained_timer present (mtu1)

    // ---- reset-matrix state (sh_mtu.h:124-132) ----
    pub m_tgr_clearing: i32, // sh_mtu.h:124
    pub m_tcr: u8, // sh_mtu.h:125
    pub m_tmdr: u8,
    pub m_tior: u8,
    pub m_tier: u8,
    pub m_tsr: u8,
    pub m_clock_type: i32, // sh_mtu.h:126
    pub m_clock_divider: i32,
    pub m_tcnt: u16, // sh_mtu.h:127
    pub m_tgr: [u16; 4], // sh_mtu.h:128
    pub m_last_clock_update: u64, // sh_mtu.h:129
    pub m_event_time: u64,
    pub m_phase: u32, // sh_mtu.h:130
    pub m_counter_cycle: u32,
    pub m_counter_incrementing: bool, // sh_mtu.h:131
    pub m_channel_active: bool, // sh_mtu.h:132 (device_reset does NOT touch)

    // ---- host seams (see module doc) ----
    /// stand-in for `m_cpu->current_cycles()` at :423/:470
    pub cpu_now: u64,
    /// sticky "call Sh7042::internal_update()" (:417/:461)
    pub resched: bool,
    /// FIFO of pending `m_intc->internal_interrupt(vector)` (:517/:523)
    pub irq_req: [i32; 16],
    pub irq_len: usize,
    pub irq_dropped: u64,
}

impl Sh2MtuChannel {
    /// template ctor: origin: src/mame/cpu/sh_mtu.h:67-91. `irq_base` +
    /// `tier_mask` derive the six interrupt vector slots EXACTLY as the ctor
    /// (:76-81: slot2 dead if mask&0x04, slot3 dead if mask&0x08, slot5 dead
    /// if mask&0x20) — the dead flags are computed FROM `tier_mask` here so the
    /// bool arguments cannot drift from the mask. Vectors per channel
    /// (sh7042.cpp:179-223): mtu0 base 88, mtu1 96, mtu2 104, mtu3 112,
    /// mtu4 120.
    pub fn new(tgr_count: i32, tier_mask: u8, irq_base: i32, ct: [i32; 8], chained: bool) -> Self {
        // origin: sh_mtu.h:76-81 (interrupt slot table) — dead flags from mask
        let mut m_interrupt = [0i32; 6];
        m_interrupt[0] = irq_base;
        m_interrupt[1] = irq_base + 1;
        m_interrupt[2] = if tier_mask & 0x04 != 0 { -1 } else { irq_base + 2 };
        m_interrupt[3] = if tier_mask & 0x08 != 0 { -1 } else { irq_base + 3 };
        m_interrupt[4] = irq_base + 4; // TOVF always wired (:80)
        m_interrupt[5] = if tier_mask & 0x20 != 0 { -1 } else { irq_base + 5 };
        Self {
            m_interrupt,                        // :76-81
            m_tier_mask: tier_mask,             // :74
            m_tgr_count: tgr_count,             // :72
            m_tbr_count: 0,                     // :73
            m_count_types: ct,                  // :83-90
            m_chained: chained,                 // set_chain (:94)
            // device_reset matrix defaults (overwritten by device_reset):
            m_tgr_clearing: TGR_CLEAR_NONE, // :207
            m_tcr: 0,                   // :208
            m_tmdr: 0xc0,               // :209
            m_tior: 0,                  // :210
            m_tier: 0x40 & tier_mask,   // :211
            m_tsr: 0,                   // sh_mtu.h:125 base default (device_start sets 0xc0)
            m_clock_type: DIV_1,        // :212
            m_clock_divider: 0,         // :213
            m_tcnt: 0,                  // :214
            m_tgr: [0xffff; 4],         // :215
            m_last_clock_update: 0,     // :216
            m_event_time: 0,            // :217
            m_phase: 0,                 // :218
            m_counter_cycle: 1,         // :219
            m_counter_incrementing: true, // :220
            m_channel_active: false,    // device_start:177 (not in reset)
            cpu_now: 0,
            resched: false,
            irq_req: [0; 16],
            irq_len: 0,
            irq_dropped: 0,
        }
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:175-200 (device_start). The save_item
    /// block (:180-197) is the M5 serialization row — all fields already pub.
    /// Sets active=false, runs the reset matrix, THEN forces TSR=0xc0 (:199).
    pub fn device_start(&mut self) {
        self.m_channel_active = false; // :177
        self.device_reset(); // :178
        self.m_tsr = 0xc0; // :199
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:202-221 (device_reset). NOTE :204
    /// comment — device_reset does NOT touch m_channel_active (the top device
    /// / machine owns it) and does NOT set m_tsr (TSR's 0xc0 comes only from
    /// device_start:199; afterwards it is only cleared via tsr_w write-clear).
    pub fn device_reset(&mut self) {
        self.m_tbr_count = 0; // :206
        self.m_tgr_clearing = TGR_CLEAR_NONE; // :207
        self.m_tcr = 0; // :208
        self.m_tmdr = 0xc0; // :209
        self.m_tior = 0; // :210
        self.m_tier = 0x40 & self.m_tier_mask; // :211
        self.m_clock_type = DIV_1; // :212
        self.m_clock_divider = 0; // :213
        self.m_tcnt = 0; // :214
        self.m_tgr = [0xffff; 4]; // :215
        self.m_last_clock_update = 0; // :216
        self.m_event_time = 0; // :217
        self.m_phase = 0; // :218
        self.m_counter_cycle = 1; // :219
        self.m_counter_incrementing = true; // :220
    }

    // ---- register handlers (sh_mtu.cpp) ----

    /// origin: src/mame/cpu/sh_mtu.cpp:223-226 (tcr_r)
    pub fn tcr_r(&self) -> u8 {
        self.m_tcr
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:228-289 (tcr_w). The compare-match /
    /// auto-clear select (:233-249) and the prescaler reprogram (:251-287)
    /// are transliterated line-for-line, including the QUIRK at :252 — the
    /// divisor branch is gated on the PREVIOUS `m_clock_type <= DIV_4`, so
    /// reprogramming a channel that is ALREADY on DIV_8..DIV_4096 leaves
    /// clock_type/divider/phase untouched (no branch matches a high DIV while
    /// old type > DIV_4, and it is neither CHAIN nor INPUT). Copied verbatim.
    pub fn tcr_w(&mut self, data: u8) {
        self.update_counter(0); // :230
        self.m_tcr = data; // :231
        match self.m_tcr & 0x60 {
            // :233
            0x00 => {
                self.m_tgr_clearing = TGR_CLEAR_NONE; // :235
            }
            0x20 | 0x40 => {
                // :239-241
                self.m_tgr_clearing = if self.m_tcr & 0x20 != 0 { 0 } else { 1 };
                if self.m_tgr_count > 2 && (self.m_tcr & 0x80) != 0 {
                    self.m_tgr_clearing += 2;
                }
            }
            0x60 => {
                self.m_tgr_clearing = TGR_CLEAR_EXT; // :246
            }
            _ => {}
        }

        let count_type = self.m_count_types[(self.m_tcr & 7) as usize]; // :251
        if count_type >= DIV_1 && self.m_clock_type <= DIV_4 {
            // :252 (see QUIRK note above — old clock_type)
            self.m_clock_type = DIV_1; // :253
            self.m_clock_divider = count_type - DIV_1; // :254
            if self.m_clock_divider == 0 {
                // :256
                self.m_phase = 0; // :257
            } else {
                match self.m_tcr & 0x18 {
                    // :259
                    0x00 => self.m_phase = 0, // :261
                    0x08 => self.m_phase = 1 << (self.m_clock_divider - 1), // :265
                    // :268-271 Phase 0+180: divider-- (odd divider)
                    0x10 | 0x18 => {
                        self.m_phase = 0;
                        self.m_clock_divider -= 1;
                    }
                    _ => {}
                }
            }
        } else if count_type == CHAIN {
            // :276
            self.m_clock_type = CHAIN; // :277
            self.m_clock_divider = 0; // :278
            self.m_phase = 0; // :279
        } else if count_type >= INPUT_A && count_type <= INPUT_D {
            // :282
            self.m_clock_type = count_type; // :283
            self.m_clock_divider = 0; // :284
            self.m_phase = 0; // :285
        }
        self.recalc_event(0); // :288
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:291-300 (tmdr_r / tmdr_w)
    pub fn tmdr_r(&self) -> u8 {
        self.m_tmdr
    }
    pub fn tmdr_w(&mut self, data: u8) {
        self.m_tmdr = data; // :298
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:302-310 (tior_r / tior_w)
    pub fn tior_r(&self) -> u8 {
        self.m_tior
    }
    pub fn tior_w(&mut self, data: u8) {
        self.m_tior = data; // :309
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:312-330 (tier_r / tier_w)
    pub fn tier_r(&self) -> u8 {
        self.m_tier
    }
    pub fn tier_w(&mut self, data: u8) {
        self.update_counter(0); // :319
        self.m_tier = data; // :320 (`if(0) logerror` :321 dropped)
        self.recalc_event(0); // :329
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:332-342 (tsr_r / tsr_w). Write-1-to-
    /// CLEAR semantics for bits 0..6 (:340): a set bit in `data` clears that
    /// TSR flag; bit7 (IRQA?) is a plain write; a 0 bit in `data` is a no-op.
    pub fn tsr_r(&self) -> u8 {
        self.m_tsr
    }
    pub fn tsr_w(&mut self, data: u8) {
        self.update_counter(0); // :339
        // :340  m_tsr = (data & 0x80) | (m_tsr & ~data & 0x7f);
        self.m_tsr = (data & 0x80) | (self.m_tsr & !data & 0x7f);
        self.recalc_event(0); // :341
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:344-352 (tcnt_r). Advances the counter
    /// (side_effects_disabled never set here — module doc), then the rotary
    /// phase mode (:349): if TMDR low nibble is nonzero the read returns 0.
    pub fn tcnt_r(&mut self) -> u16 {
        self.update_counter(0); // :346-347
        if self.m_tmdr & 0xf != 0 {
            // :349
            return 0;
        }
        self.m_tcnt // :351
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:354-359 (tcnt_w). COMBINE_DATA = bits
    /// set in mask take the new value (sh7042.rs `combine16`, same as C++).
    pub fn tcnt_w(&mut self, data: u16, mem_mask: u16) {
        self.update_counter(0); // :356
        self.m_tcnt = (self.m_tcnt & !mem_mask) | (data & mem_mask); // :357 COMBINE_DATA
        self.recalc_event(0); // :358
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:361-364 (tgr_r)
    pub fn tgr_r(&self, reg: usize) -> u16 {
        self.m_tgr[reg]
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:366-371 (tgr_w)
    pub fn tgr_w(&mut self, reg: usize, data: u16, mem_mask: u16) {
        self.update_counter(0); // :368
        self.m_tgr[reg] = (self.m_tgr[reg] & !mem_mask) | (data & mem_mask); // :369
        self.recalc_event(0); // :370
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:373-381 (tgrc_r / tgrc_w) — TGRC/D are
    /// just TGR[reg+2]; bounds preserved (would panic like C++ OOB if reg>=2
    /// reached a 2-TGR channel, but the map never routes that).
    pub fn tgrc_r(&self, reg: usize) -> u16 {
        self.tgr_r(reg + 2) // :375
    }
    pub fn tgrc_w(&mut self, reg: usize, data: u16, mem_mask: u16) {
        self.tgr_w(reg + 2, data, mem_mask); // :380
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:383-392 (set_enable). Start/stop
    /// gating: enable=false freezes the counter — `update_counter` keeps
    /// resyncing `m_last_clock_update` (:472-474) without advancing `m_tcnt`,
    /// and `recalc_event` clears `m_event_time` (:406-408). The channel keeps
    /// all TGR/TCNT register state (nothing is dropped on stop).
    pub fn set_enable(&mut self, enable: bool) {
        self.update_counter(0); // :385
        self.m_channel_active = enable; // :386
        self.recalc_event(0); // :391
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:394-402 (internal_update).
    pub fn internal_update(&mut self, current_time: u64) -> u64 {
        self.cpu_now = current_time; // host-sync (module doc)
        while self.m_event_time != 0 && current_time >= self.m_event_time {
            // :396
            self.update_counter(self.m_event_time); // :397
            self.recalc_event(self.m_event_time); // :398
        }
        self.m_event_time // :401
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:404-462 (recalc_event). Inactive or a
    /// non-DIV_1 clock_type schedules nothing (:406-420). All widths kept:
    /// `m_counter_cycle` is u32 (so `m_tgr+1` == 0x10000 at 0xffff, :428);
    /// `cmp` is u16 (wraps at :437); event_delay u32; event_time u64 with the
    /// divider/phase reconstruction verbatim (:452).
    pub fn recalc_event(&mut self, mut cur_time: u64) {
        if !self.m_channel_active {
            // :406
            self.m_event_time = 0; // :407
            return;
        }
        let update_cpu = cur_time == 0; // :411
        let old_event_time = self.m_event_time; // :412

        if self.m_clock_type != DIV_1 {
            // :414 (CHAIN / INPUT_x never advance)
            self.m_event_time = 0; // :415
            if old_event_time != 0 && update_cpu {
                self.resched = true; // :416-417
            }
            return;
        }

        if cur_time == 0 {
            cur_time = self.cpu_now; // :422-423 current_cycles()
        }

        if self.m_counter_incrementing {
            // :425
            let mut event_delay: u32 = 0xffff_ffff; // :426
            if self.m_tgr_clearing >= 0 {
                // :427
                self.m_counter_cycle = u32::from(self.m_tgr[self.m_tgr_clearing as usize]) + 1; // :428
            } else {
                self.m_counter_cycle = 0x1_0000; // :430
            }
            // :431-432 TOVF (V) pending?
            if (self.m_tier & IRQ_V != 0 && self.m_interrupt[4] != -1)
                && (self.m_counter_cycle == 0x1_0000 || u32::from(self.m_tcnt) >= self.m_counter_cycle)
            {
                event_delay = 0x1_0000 - u32::from(self.m_tcnt);
            }
            let mut i = 0; // :434
            while i < self.m_tgr_count {
                if (self.m_tier >> i) & 1 != 0 && self.m_interrupt[i as usize] != -1 {
                    // :435 BIT(m_tier,i)
                    let mut new_delay: u32 = 0xffff_ffff; // :436
                    let cmp: u16 = self.m_tgr[i as usize].wrapping_add(1); // :437
                    let tcnt = u32::from(self.m_tcnt);
                    if u32::from(cmp) > tcnt {
                        // :438
                        if tcnt >= self.m_counter_cycle || u32::from(cmp) <= self.m_counter_cycle {
                            // :439
                            new_delay = u32::from(cmp) - tcnt; // :440
                        }
                    } else if u32::from(cmp) <= self.m_counter_cycle {
                        // :441
                        if tcnt < self.m_counter_cycle {
                            // :442
                            new_delay = (self.m_counter_cycle - tcnt) + u32::from(cmp); // :443
                        } else {
                            new_delay = (0x1_0000 - tcnt) + u32::from(cmp); // :445
                        }
                    }
                    if event_delay > new_delay {
                        // :448
                        event_delay = new_delay;
                    }
                }
                i += 1;
            }
            if event_delay != 0xffff_ffff {
                // :451 — the u64 reconstruction, exact operator order
                let div = self.m_clock_divider as u32;
                let phase = u64::from(self.m_phase);
                let base = (cur_time
                    .wrapping_add(1u64 << div)
                    .wrapping_sub(phase))
                    >> div;
                self.m_event_time = ((base + u64::from(event_delay) - 1) << div) + phase; // :452
            } else {
                self.m_event_time = 0; // :454
            }
        } else {
            // :455-458 decrementing counter: not reachable here
            panic!("mtu decrementing counter (sh_mtu.cpp:456)");
        }

        if old_event_time != self.m_event_time && update_cpu {
            self.resched = true; // :460-461
        }
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:464-529 (update_counter). Freerun
    /// 16-bit counter derived from elapsed (post-divider) CPU cycles. Widths
    /// exact: `tt`/`delta`/`base_time`/`new_time` are u64 (:477-490), `m_tcnt`
    /// and `prev`/`cmp` are u16 with the C++ implicit u64->u16 narrowing at
    /// the modulo stores (:494/:496/:498 — reproduced with `as u16`).
    pub fn update_counter(&mut self, mut cur_time: u64) {
        if self.m_clock_type != DIV_1 {
            // :466 (CHAIN / INPUT_x: no advance at all)
            return;
        }
        if cur_time == 0 {
            cur_time = self.cpu_now; // :469-470 current_cycles()
        }
        if !self.m_channel_active {
            // :472  stopped channel: resync clock, DO NOT count (TCNT frozen,
            // but register writes still land — see set_enable doc)
            self.m_last_clock_update = cur_time; // :473
            return;
        }

        let mut base_time = self.m_last_clock_update; // :477
        self.m_last_clock_update = cur_time; // :478
        let mut new_time = cur_time; // :479
        if self.m_clock_divider != 0 {
            // :480  prescale the clock edges
            let div = self.m_clock_divider as u32;
            let phase = u64::from(self.m_phase);
            base_time = (base_time.wrapping_add(phase)) >> div; // :481
            new_time = (new_time.wrapping_add(phase)) >> div; // :482
        }
        if new_time == base_time {
            // :484  no full clock edge this window
            return;
        }

        if !self.m_counter_incrementing {
            // :525-528
            panic!("mtu decrementing counter (sh_mtu.cpp:526)");
        }

        let prev: u16 = self.m_tcnt; // :488
        let delta: u64 = new_time - base_time; // :489
        let tt: u64 = u64::from(self.m_tcnt) + delta; // :490

        // :492-498 wrap modulo the auto-clear cycle
        if u32::from(prev) >= self.m_counter_cycle {
            if tt >= 0x1_0000 {
                self.m_tcnt = ((tt - 0x1_0000) % u64::from(self.m_counter_cycle)) as u16; // :494
            } else {
                self.m_tcnt = tt as u16; // :496
            }
        } else {
            self.m_tcnt = (tt % u64::from(self.m_counter_cycle)) as u16; // :498
        }

        // :500-519 compare-match scan (TGRA/B/C/D flags + IRQ)
        let mut i = 0usize;
        while (i as i32) < self.m_tgr_count {
            let cmp: u16 = self.m_tgr[i].wrapping_add(1); // :501
            let mut match_now = self.m_tcnt == cmp || (tt == u64::from(cmp) && tt == u64::from(self.m_counter_cycle)); // :502
            if !match_now {
                // :503-511 extra skip-detection for flag-polling software
                if u32::from(prev) >= self.m_counter_cycle {
                    // :505
                    match_now = (u32::from(cmp) > u32::from(prev) && tt >= u64::from(cmp))
                        || (u32::from(cmp) <= self.m_counter_cycle
                            && u32::from(self.m_tcnt) < self.m_counter_cycle
                            && (delta - (0x1_0000 - u64::from(prev))) >= u64::from(cmp)); // :506
                } else if u32::from(cmp) <= self.m_counter_cycle {
                    // :507
                    match_now = delta >= u64::from(self.m_counter_cycle)
                        || (u32::from(prev) < u32::from(cmp) && tt >= u64::from(cmp))
                        || (u32::from(self.m_tcnt) <= u32::from(prev)
                            && u32::from(self.m_tcnt) >= u32::from(cmp)); // :508
                }
                // :510-511 `logerror unexpected TGR IRQ` dropped
            }
            if match_now {
                self.m_tsr |= 1 << i; // :515 set TGRi flag
                if (self.m_tier >> i) & 1 != 0 && self.m_interrupt[i] != -1 {
                    // :516
                    self.internal_interrupt(self.m_interrupt[i]); // :517
                }
            }
            i += 1;
        }

        // :520-524 overflow (TOVF / IRQ_V)
        if tt >= 0x1_0000
            && (self.m_counter_cycle == 0x1_0000 || u32::from(prev) >= self.m_counter_cycle)
        {
            self.m_tsr |= IRQ_V; // :521
            if self.m_tier & IRQ_V != 0 && self.m_interrupt[4] != -1 {
                // :522
                self.internal_interrupt(self.m_interrupt[4]); // :523
            }
        }
    }

    /// `m_intc->internal_interrupt(vector)` — the two call sites are
    /// :517/:523 (see update_counter). Ordered FIFO; intc pending `|=` is
    /// idempotent (sh_intc.cpp:97).
    fn internal_interrupt(&mut self, vector: i32) {
        if self.irq_len < self.irq_req.len() {
            self.irq_req[self.irq_len] = vector;
            self.irq_len += 1;
        } else {
            self.irq_dropped += 1; // unreachable when drained per bus op
        }
    }

    /// drain this channel's FIFO into `out`.
    pub fn drain_irqs(&mut self, out: &mut Vec<i32>) -> usize {
        let n = self.irq_len;
        out.extend_from_slice(&self.irq_req[..n]);
        self.irq_len = 0;
        n
    }

    /// take+clear the sticky resched (:417/:461).
    pub fn take_resched(&mut self) -> bool {
        let r = self.resched;
        self.resched = false;
        r
    }
}

// ---------------------------------------------------------------------------
// Sh2Mtu — the shared MTU device (TSTR/TOER/TOCR/TGCR/TCDR/TDDR/TCNTS/TCBR)
// plus the five channels; the `Sh7042Peripherals` seam.
// ---------------------------------------------------------------------------
pub struct Sh2Mtu {
    // origin: src/mame/cpu/sh_mtu.h:181-187 (shared device state)
    pub m_timer_count: i32, // :184 (always 5 — sh7042.cpp:178)
    pub m_tstr: u8, // :186 reset 0 (:42)
    pub m_tsyr: u8, // :43
    pub m_toer: u8, // :44 reset 0xc0
    pub m_tocr: u8, // :45
    pub m_tgcr: u8, // :46 reset 0x80
    pub m_tcdr: u16, // :47 reset 0xffff
    pub m_tddr: u16, // :48 reset 0xffff
    pub m_tcnts: u16, // :49
    pub m_tcbr: u16, // :50 reset 0xffff

    pub ch: [Sh2MtuChannel; 5], // m_timer_channel[5] (:182)
}

impl Sh2Mtu {
    /// ctor + device_start fan-out. Channel configs are copied EXACTLY from
    /// sh7042.cpp:179-223:
    ///   mtu0 tgr=4 mask=0x60 base=88  [D1,D4,D16,D64,A,B,C,D]
    ///   mtu1 tgr=2 mask=0x4c base=96  [D1,D4,D16,D64,A,B,D256,CHAIN] chain->mtu2
    ///   mtu2 tgr=2 mask=0x4c base=104 [D1,D4,D16,D64,A,B,C,D1024]
    ///   mtu3 tgr=4 mask=0x60 base=112 [D1,D4,D16,D64,D256,D1024,A,B]
    ///   mtu4 tgr=4 mask=0x60 base=120 [D1,D4,D16,D64,D256,D1024,A,B]
    /// The dead-slot flags are `mask & 0x04`(C) / `& 0x08`(D) / `& 0x20`(U).
    pub fn new() -> Self {
        let mut ch = [
            // mtu0 — sh7042.cpp:179-187, mask 0x60 (bit5 set -> U dead)
            Sh2MtuChannel::new(
                4,
                0x60,
                88,
                [DIV_1, DIV_4, DIV_16, DIV_64, INPUT_A, INPUT_B, INPUT_C, INPUT_D],
                false,
            ),
            // mtu1 — :188-196, mask 0x4c (bits2,3 -> C/D dead)
            Sh2MtuChannel::new(
                2,
                0x4c,
                96,
                [DIV_1, DIV_4, DIV_16, DIV_64, INPUT_A, INPUT_B, DIV_256, CHAIN],
                true,
            ),
            // mtu2 — :197-205, mask 0x4c
            Sh2MtuChannel::new(
                2,
                0x4c,
                104,
                [DIV_1, DIV_4, DIV_16, DIV_64, INPUT_A, INPUT_B, INPUT_C, DIV_1024],
                false,
            ),
            // mtu3 — :206-214, mask 0x60
            Sh2MtuChannel::new(
                4,
                0x60,
                112,
                [DIV_1, DIV_4, DIV_16, DIV_64, DIV_256, DIV_1024, INPUT_A, INPUT_B],
                false,
            ),
            // mtu4 — :215-223, mask 0x60
            Sh2MtuChannel::new(
                4,
                0x60,
                120,
                [DIV_1, DIV_4, DIV_16, DIV_64, DIV_256, DIV_1024, INPUT_A, INPUT_B],
                false,
            ),
        ];
        for c in ch.iter_mut() {
            c.device_start(); // sh_mtu.cpp:175-200 (active=false, reset, tsr=0xc0)
        }
        let mut mtu = Self {
            m_timer_count: 5,      // sh7042.cpp:178 SH_MTU(config, m_mtu, *this, 5)
            m_tstr: 0,             // device_reset:42 (device_start saves only)
            m_tsyr: 0,             // :43
            m_toer: 0xc0,          // :44
            m_tocr: 0,             // :45
            m_tgcr: 0x80,          // :46
            m_tcdr: 0xffff,        // :47
            m_tddr: 0xffff,        // :48
            m_tcnts: 0,            // :49
            m_tcbr: 0xffff,        // :50
            ch,
        };
        // sh_mtu_device::device_start (:27-38) is save_items only — M5 row.
        // set_channel wiring (:225-229) is already reflected by ch[].
        let _ = &mut mtu;
        mtu
    }

    /// machine reset fan-out (sh_mtu_device::device_reset :40-51 + the per-
    /// channel device_reset the compat machine calls on each device). NOTE:
    /// channel `device_reset` never touches TSR (that is device_start-only) nor
    /// `m_channel_active` (device_reset:204 comment; owned by the top/machine).
    pub fn device_reset(&mut self) {
        self.m_tstr = 0; // :42
        self.m_tsyr = 0; // :43
        self.m_toer = 0xc0; // :44
        self.m_tocr = 0; // :45
        self.m_tgcr = 0x80; // :46
        self.m_tcdr = 0xffff; // :47
        self.m_tddr = 0xffff; // :48
        self.m_tcnts = 0; // :49
        self.m_tcbr = 0xffff; // :50
        for c in self.ch.iter_mut() {
            c.device_reset(); // :202-221
        }
    }

    // ---- host-seam helpers ----

    /// The wiring row writes the exact cycle count here before bus ops
    /// (replaces `m_cpu->current_cycles()`); fans out to every channel.
    pub fn set_cpu_now(&mut self, t: u64) {
        self.m_cpu_now_all(t);
    }
    fn m_cpu_now_all(&mut self, t: u64) {
        for c in self.ch.iter_mut() {
            c.cpu_now = t;
        }
    }

    /// Concatenate every channel's pending IRQs (all share one INTC on disk).
    pub fn drain_irqs(&mut self, out: &mut Vec<i32>) -> usize {
        let mut n = 0;
        for c in self.ch.iter_mut() {
            n += c.drain_irqs(out);
        }
        n
    }

    /// OR of every channel's sticky resched (:417/:461 sites).
    pub fn take_resched(&mut self) -> bool {
        let mut r = false;
        for c in self.ch.iter_mut() {
            r |= c.take_resched();
        }
        r
    }

    // ---- shared-device register handlers (sh_mtu_device) ----

    /// origin: src/mame/cpu/sh_mtu.cpp:53-69 (tstr_r / tstr_w). The enable
    /// bit map is the To generalize comment (:63): channels 0,1,2 from
    /// TSTR bits 0,1,2 and channels 3,4 from TSTR bits 6,7 (SH-2 channel-F
    /// style layout — bits 3..5 unused on this die).
    pub fn tstr_r(&self) -> u8 {
        self.m_tstr
    }
    pub fn tstr_w(&mut self, data: u8) {
        self.m_tstr = data; // :61
        self.ch[0].set_enable((data >> 0) & 1 != 0); // :64 BIT(m_tstr,0)
        self.ch[1].set_enable((data >> 1) & 1 != 0); // :65
        self.ch[2].set_enable((data >> 2) & 1 != 0); // :66
        self.ch[3].set_enable((data >> 6) & 1 != 0); // :67 BIT(m_tstr,6)
        self.ch[4].set_enable((data >> 7) & 1 != 0); // :68 BIT(m_tstr,7)
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:71-117 (tsyr/toer/tocr/tgcr r/w — plain)
    pub fn tsyr_r(&self) -> u8 {
        self.m_tsyr
    }
    pub fn tsyr_w(&mut self, data: u8) {
        self.m_tsyr = data; // :79
    }
    pub fn toer_r(&self) -> u8 {
        self.m_toer
    }
    pub fn toer_w(&mut self, data: u8) {
        self.m_toer = data; // :91
    }
    pub fn tocr_r(&self) -> u8 {
        self.m_tocr
    }
    pub fn tocr_w(&mut self, data: u8) {
        self.m_tocr = data; // :103
    }
    pub fn tgcr_r(&self) -> u8 {
        self.m_tgcr
    }
    pub fn tgcr_w(&mut self, data: u8) {
        self.m_tgcr = data; // :115
    }

    /// origin: src/mame/cpu/sh_mtu.cpp:119-165 (tcdr/tddr/tcnts/tcbr r/w).
    /// tcXX_w uses COMBINE_DATA (bits set in mask take new value).
    pub fn tcdr_r(&self) -> u16 {
        self.m_tcdr
    }
    pub fn tcdr_w(&mut self, data: u16, mask: u16) {
        self.m_tcdr = (self.m_tcdr & !mask) | (data & mask); // :127
    }
    pub fn tddr_r(&self) -> u16 {
        self.m_tddr
    }
    pub fn tddr_w(&mut self, data: u16, mask: u16) {
        self.m_tddr = (self.m_tddr & !mask) | (data & mask); // :139
    }
    pub fn tcnts_r(&self) -> u16 {
        self.m_tcnts
    }
    pub fn tcnts_w(&mut self, data: u16, mask: u16) {
        self.m_tcnts = (self.m_tcnts & !mask) | (data & mask); // :151
    }
    pub fn tcbr_r(&self) -> u16 {
        self.m_tcbr
    }
    pub fn tcbr_w(&mut self, data: u16, mask: u16) {
        self.m_tcbr = (self.m_tcbr & !mask) | (data & mask); // :163
    }
}

// ---------------------------------------------------------------------------
// Bus case-sets — the sh7042.rs mtu arms dispatch ABSOLUTE addresses here.
// Layout recap (map.hxx): mtu3=even / mtu4=odd interleaved across the
// 8200-822d shared block; the TOER/TOCR/TGCR/TCDR/TDDR/TCNTS/TCBR shared regs
// and TSTR/TSYR live in the same block; mtu0=8260..826f, mtu1=8280..828b,
// mtu2=82a0..82ab. QUIRK (map:32-33,74-75,90-91,102-103 / 534-537,577-578,
// 592-593,605-606): a channel's TIOR byte is answered at TWO consecutive
// addresses (8204 AND 8205 -> mtu3; ...). All unlisted even r16/w16 and any
// in-range hole return 0 / drop the write, matching the map defaults
// (membus.h miss -> 0).
// ---------------------------------------------------------------------------
const MTU3: usize = 3;
const MTU4: usize = 4;
const MTU0: usize = 0;
const MTU1: usize = 1;
const MTU2: usize = 2;

impl Sh7042Peripherals for Sh2Mtu {
    /// origin: src/mame/cpu/sh7042_map.hxx:27-111 (internal_r8 mtu arms)
    fn mtu_r8(&mut self, a: u32) -> u8 {
        match a {
            // ---- shared / mtu3 / mtu4 interleaved block ----
            0xffff8200 => self.ch[MTU3].tcr_r(), // map:27
            0xffff8201 => self.ch[MTU4].tcr_r(), // map:28
            0xffff8202 => self.ch[MTU3].tmdr_r(), // map:29
            0xffff8203 => self.ch[MTU4].tmdr_r(), // map:30
            0xffff8204 | 0xffff8205 => self.ch[MTU3].tior_r(), // map:31/32 (TIOR quirk: both)
            0xffff8206 | 0xffff8207 => self.ch[MTU4].tior_r(), // map:33/34 (TIOR quirk)
            0xffff8208 => self.ch[MTU3].tier_r(), // map:35
            0xffff8209 => self.ch[MTU4].tier_r(), // map:36
            0xffff820a => self.toer_r(), // map:37
            0xffff820b => self.tocr_r(), // map:38
            0xffff820d => self.tgcr_r(), // map:39
            0xffff8210 => (self.ch[MTU3].tcnt_r() >> 8) as u8, // map:40
            0xffff8211 => self.ch[MTU3].tcnt_r() as u8, // map:41
            0xffff8212 => (self.ch[MTU4].tcnt_r() >> 8) as u8, // map:42
            0xffff8213 => self.ch[MTU4].tcnt_r() as u8, // map:43
            0xffff8214 => (self.tcdr_r() >> 8) as u8, // map:44
            0xffff8215 => self.tcdr_r() as u8, // map:45
            0xffff8216 => (self.tddr_r() >> 8) as u8, // map:46
            0xffff8217 => self.tddr_r() as u8, // map:47
            0xffff8218 => (self.ch[MTU3].tgr_r(0) >> 8) as u8, // map:48
            0xffff8219 => self.ch[MTU3].tgr_r(0) as u8, // map:49
            0xffff821a => (self.ch[MTU3].tgr_r(1) >> 8) as u8, // map:50
            0xffff821b => self.ch[MTU3].tgr_r(1) as u8, // map:51
            0xffff821c => (self.ch[MTU4].tgr_r(0) >> 8) as u8, // map:52
            0xffff821d => self.ch[MTU4].tgr_r(0) as u8, // map:53
            0xffff821e => (self.ch[MTU4].tgr_r(1) >> 8) as u8, // map:54
            0xffff821f => self.ch[MTU4].tgr_r(1) as u8, // map:55
            0xffff8220 => (self.tcnts_r() >> 8) as u8, // map:56
            0xffff8221 => self.tcnts_r() as u8, // map:57
            0xffff8222 => (self.tcbr_r() >> 8) as u8, // map:58
            0xffff8223 => self.tcbr_r() as u8, // map:59
            0xffff8224 => (self.ch[MTU3].tgrc_r(0) >> 8) as u8, // map:60 (=TGR2)
            0xffff8225 => self.ch[MTU3].tgrc_r(0) as u8, // map:61
            0xffff8226 => (self.ch[MTU3].tgrc_r(1) >> 8) as u8, // map:62 (=TGR3)
            0xffff8227 => self.ch[MTU3].tgrc_r(1) as u8, // map:63
            0xffff8228 => (self.ch[MTU4].tgrc_r(0) >> 8) as u8, // map:64
            0xffff8229 => self.ch[MTU4].tgrc_r(0) as u8, // map:65
            0xffff822a => (self.ch[MTU4].tgrc_r(1) >> 8) as u8, // map:66
            0xffff822b => self.ch[MTU4].tgrc_r(1) as u8, // map:67
            0xffff822c => self.ch[MTU3].tsr_r(), // map:68
            0xffff822d => self.ch[MTU4].tsr_r(), // map:69
            0xffff8240 => self.tstr_r(), // map:70
            0xffff8241 => self.tsyr_r(), // map:71
            // ---- mtu0 block ----
            0xffff8260 => self.ch[MTU0].tcr_r(), // map:72
            0xffff8261 => self.ch[MTU0].tmdr_r(), // map:73
            0xffff8262 | 0xffff8263 => self.ch[MTU0].tior_r(), // map:74/75 (TIOR quirk)
            0xffff8264 => self.ch[MTU0].tier_r(), // map:76
            0xffff8265 => self.ch[MTU0].tsr_r(), // map:77
            0xffff8266 => (self.ch[MTU0].tcnt_r() >> 8) as u8, // map:78
            0xffff8267 => self.ch[MTU0].tcnt_r() as u8, // map:79
            0xffff8268 => (self.ch[MTU0].tgr_r(0) >> 8) as u8, // map:80
            0xffff8269 => self.ch[MTU0].tgr_r(0) as u8, // map:81
            0xffff826a => (self.ch[MTU0].tgr_r(1) >> 8) as u8, // map:82
            0xffff826b => self.ch[MTU0].tgr_r(1) as u8, // map:83
            0xffff826c => (self.ch[MTU0].tgr_r(2) >> 8) as u8, // map:84
            0xffff826d => self.ch[MTU0].tgr_r(2) as u8, // map:85
            0xffff826e => (self.ch[MTU0].tgr_r(3) >> 8) as u8, // map:86
            0xffff826f => self.ch[MTU0].tgr_r(3) as u8, // map:87
            // ---- mtu1 block ----
            0xffff8280 => self.ch[MTU1].tcr_r(), // map:88
            0xffff8281 => self.ch[MTU1].tmdr_r(), // map:89
            0xffff8282 | 0xffff8283 => self.ch[MTU1].tior_r(), // map:90/91 (TIOR quirk)
            0xffff8284 => self.ch[MTU1].tier_r(), // map:92
            0xffff8285 => self.ch[MTU1].tsr_r(), // map:93
            0xffff8286 => (self.ch[MTU1].tcnt_r() >> 8) as u8, // map:94
            0xffff8287 => self.ch[MTU1].tcnt_r() as u8, // map:95
            0xffff8288 => (self.ch[MTU1].tgr_r(0) >> 8) as u8, // map:96
            0xffff8289 => self.ch[MTU1].tgr_r(0) as u8, // map:97
            0xffff828a => (self.ch[MTU1].tgr_r(1) >> 8) as u8, // map:98
            0xffff828b => self.ch[MTU1].tgr_r(1) as u8, // map:99
            // ---- mtu2 block ----
            0xffff82a0 => self.ch[MTU2].tcr_r(), // map:100
            0xffff82a1 => self.ch[MTU2].tmdr_r(), // map:101
            0xffff82a2 | 0xffff82a3 => self.ch[MTU2].tior_r(), // map:102/103 (TIOR quirk)
            0xffff82a4 => self.ch[MTU2].tier_r(), // map:104
            0xffff82a5 => self.ch[MTU2].tsr_r(), // map:105
            0xffff82a6 => (self.ch[MTU2].tcnt_r() >> 8) as u8, // map:106
            0xffff82a7 => self.ch[MTU2].tcnt_r() as u8, // map:107
            0xffff82a8 => (self.ch[MTU2].tgr_r(0) >> 8) as u8, // map:108
            0xffff82a9 => self.ch[MTU2].tgr_r(0) as u8, // map:109
            0xffff82aa => (self.ch[MTU2].tgr_r(1) >> 8) as u8, // map:110
            0xffff82ab => self.ch[MTU2].tgr_r(1) as u8, // map:111
            _ => 0, // bus miss / hole
        }
    }

    /// origin: src/mame/cpu/sh7042_map.hxx:334-375 (internal_r16). Big-endian
    /// byte pairs (high reg = high byte). NOTE the TIOR pairs duplicate the
    /// same register into both bytes (map:336/337/357/365/371 — mtu3/mtu4 and
    /// per-channel TIOR).
    fn mtu_r16(&mut self, a: u32) -> u16 {
        match a {
            0xffff8200 => (u16::from(self.ch[MTU3].tcr_r()) << 8) | u16::from(self.ch[MTU4].tcr_r()), // map:334
            0xffff8202 => (u16::from(self.ch[MTU3].tmdr_r()) << 8) | u16::from(self.ch[MTU4].tmdr_r()), // map:335
            0xffff8204 => (u16::from(self.ch[MTU3].tior_r()) << 8) | u16::from(self.ch[MTU3].tior_r()), // map:336 (dup)
            0xffff8206 => (u16::from(self.ch[MTU4].tior_r()) << 8) | u16::from(self.ch[MTU4].tior_r()), // map:337 (dup)
            0xffff8208 => (u16::from(self.ch[MTU3].tier_r()) << 8) | u16::from(self.ch[MTU4].tier_r()), // map:338
            0xffff820a => (u16::from(self.toer_r()) << 8) | u16::from(self.tocr_r()), // map:339
            0xffff8210 => self.ch[MTU3].tcnt_r(), // map:340
            0xffff8212 => self.ch[MTU4].tcnt_r(), // map:341
            0xffff8214 => self.tcdr_r(), // map:342
            0xffff8216 => self.tddr_r(), // map:343
            0xffff8218 => self.ch[MTU3].tgr_r(0), // map:344
            0xffff821a => self.ch[MTU3].tgr_r(1), // map:345
            0xffff821c => self.ch[MTU4].tgr_r(0), // map:346
            0xffff821e => self.ch[MTU4].tgr_r(1), // map:347
            0xffff8220 => self.tcnts_r(), // map:348
            0xffff8222 => self.tcbr_r(), // map:349
            0xffff8224 => self.ch[MTU3].tgrc_r(0), // map:350
            0xffff8226 => self.ch[MTU3].tgrc_r(1), // map:351
            0xffff8228 => self.ch[MTU4].tgrc_r(0), // map:352
            0xffff822a => self.ch[MTU4].tgrc_r(1), // map:353
            0xffff822c => (u16::from(self.ch[MTU3].tsr_r()) << 8) | u16::from(self.ch[MTU4].tsr_r()), // map:354
            0xffff8240 => (u16::from(self.tstr_r()) << 8) | u16::from(self.tsyr_r()), // map:355
            0xffff8260 => (u16::from(self.ch[MTU0].tcr_r()) << 8) | u16::from(self.ch[MTU0].tmdr_r()), // map:356
            0xffff8262 => (u16::from(self.ch[MTU0].tior_r()) << 8) | u16::from(self.ch[MTU0].tior_r()), // map:357 (dup)
            0xffff8264 => (u16::from(self.ch[MTU0].tier_r()) << 8) | u16::from(self.ch[MTU0].tsr_r()), // map:358
            0xffff8266 => self.ch[MTU0].tcnt_r(), // map:359
            0xffff8268 => self.ch[MTU0].tgr_r(0), // map:360
            0xffff826a => self.ch[MTU0].tgr_r(1), // map:361
            0xffff826c => self.ch[MTU0].tgr_r(2), // map:362
            0xffff826e => self.ch[MTU0].tgr_r(3), // map:363
            0xffff8280 => (u16::from(self.ch[MTU1].tcr_r()) << 8) | u16::from(self.ch[MTU1].tmdr_r()), // map:364
            0xffff8282 => (u16::from(self.ch[MTU1].tior_r()) << 8) | u16::from(self.ch[MTU1].tior_r()), // map:365 (dup)
            0xffff8284 => (u16::from(self.ch[MTU1].tier_r()) << 8) | u16::from(self.ch[MTU1].tsr_r()), // map:366
            0xffff8286 => self.ch[MTU1].tcnt_r(), // map:367
            0xffff8288 => self.ch[MTU1].tgr_r(0), // map:368
            0xffff828a => self.ch[MTU1].tgr_r(1), // map:369
            0xffff82a0 => (u16::from(self.ch[MTU2].tcr_r()) << 8) | u16::from(self.ch[MTU2].tmdr_r()), // map:370
            0xffff82a2 => (u16::from(self.ch[MTU2].tior_r()) << 8) | u16::from(self.ch[MTU2].tior_r()), // map:371 (dup)
            0xffff82a4 => (u16::from(self.ch[MTU2].tier_r()) << 8) | u16::from(self.ch[MTU2].tsr_r()), // map:372
            0xffff82a6 => self.ch[MTU2].tcnt_r(), // map:373
            0xffff82a8 => self.ch[MTU2].tgr_r(0), // map:374
            0xffff82aa => self.ch[MTU2].tgr_r(1), // map:375
            _ => 0,
        }
    }

    /// origin: src/mame/cpu/sh7042_map.hxx:530-614 (internal_w8). 16-bit regs
    /// are written as one byte with mask 0xff<<8 or 0xff<<0 (COMBINE_DATA).
    /// TIOR answered twice per channel (map:534/535,577/578,592/593,605/606).
    fn mtu_w8(&mut self, a: u32, v: u8) {
        match a {
            0xffff8200 => self.ch[MTU3].tcr_w(v), // map:530
            0xffff8201 => self.ch[MTU4].tcr_w(v), // map:531
            0xffff8202 => self.ch[MTU3].tmdr_w(v), // map:532
            0xffff8203 => self.ch[MTU4].tmdr_w(v), // map:533
            0xffff8204 | 0xffff8205 => self.ch[MTU3].tior_w(v), // map:534/535
            0xffff8206 | 0xffff8207 => self.ch[MTU4].tior_w(v), // map:536/537
            0xffff8208 => self.ch[MTU3].tier_w(v), // map:538
            0xffff8209 => self.ch[MTU4].tier_w(v), // map:539
            0xffff820a => self.toer_w(v), // map:540
            0xffff820b => self.tocr_w(v), // map:541
            0xffff820d => self.tgcr_w(v), // map:542
            0xffff8210 => self.ch[MTU3].tcnt_w((u16::from(v)) << 8, 0xff00), // map:543
            0xffff8211 => self.ch[MTU3].tcnt_w(u16::from(v), 0x00ff), // map:544
            0xffff8212 => self.ch[MTU4].tcnt_w((u16::from(v)) << 8, 0xff00), // map:545
            0xffff8213 => self.ch[MTU4].tcnt_w(u16::from(v), 0x00ff), // map:546
            0xffff8214 => self.tcdr_w((u16::from(v)) << 8, 0xff00), // map:547
            0xffff8215 => self.tcdr_w(u16::from(v), 0x00ff), // map:548
            0xffff8216 => self.tddr_w((u16::from(v)) << 8, 0xff00), // map:549
            0xffff8217 => self.tddr_w(u16::from(v), 0x00ff), // map:550
            0xffff8218 => self.ch[MTU3].tgr_w(0, (u16::from(v)) << 8, 0xff00), // map:551
            0xffff8219 => self.ch[MTU3].tgr_w(0, u16::from(v), 0x00ff), // map:552
            0xffff821a => self.ch[MTU3].tgr_w(1, (u16::from(v)) << 8, 0xff00), // map:553
            0xffff821b => self.ch[MTU3].tgr_w(1, u16::from(v), 0x00ff), // map:554
            0xffff821c => self.ch[MTU4].tgr_w(0, (u16::from(v)) << 8, 0xff00), // map:555
            0xffff821d => self.ch[MTU4].tgr_w(0, u16::from(v), 0x00ff), // map:556
            0xffff821e => self.ch[MTU4].tgr_w(1, (u16::from(v)) << 8, 0xff00), // map:557
            0xffff821f => self.ch[MTU4].tgr_w(1, u16::from(v), 0x00ff), // map:558
            0xffff8220 => self.tcnts_w((u16::from(v)) << 8, 0xff00), // map:559
            0xffff8221 => self.tcnts_w(u16::from(v), 0x00ff), // map:560
            0xffff8222 => self.tcbr_w((u16::from(v)) << 8, 0xff00), // map:561
            0xffff8223 => self.tcbr_w(u16::from(v), 0x00ff), // map:562
            0xffff8224 => self.ch[MTU3].tgrc_w(0, (u16::from(v)) << 8, 0xff00), // map:563
            0xffff8225 => self.ch[MTU3].tgrc_w(0, u16::from(v), 0x00ff), // map:564
            0xffff8226 => self.ch[MTU3].tgrc_w(1, (u16::from(v)) << 8, 0xff00), // map:565
            0xffff8227 => self.ch[MTU3].tgrc_w(1, u16::from(v), 0x00ff), // map:566
            0xffff8228 => self.ch[MTU4].tgrc_w(0, (u16::from(v)) << 8, 0xff00), // map:567
            0xffff8229 => self.ch[MTU4].tgrc_w(0, u16::from(v), 0x00ff), // map:568
            0xffff822a => self.ch[MTU4].tgrc_w(1, (u16::from(v)) << 8, 0xff00), // map:569
            0xffff822b => self.ch[MTU4].tgrc_w(1, u16::from(v), 0x00ff), // map:570
            0xffff822c => self.ch[MTU3].tsr_w(v), // map:571
            0xffff822d => self.ch[MTU4].tsr_w(v), // map:572
            0xffff8240 => self.tstr_w(v), // map:573
            0xffff8241 => self.tsyr_w(v), // map:574
            0xffff8260 => self.ch[MTU0].tcr_w(v), // map:575
            0xffff8261 => self.ch[MTU0].tmdr_w(v), // map:576
            0xffff8262 | 0xffff8263 => self.ch[MTU0].tior_w(v), // map:577/578
            0xffff8264 => self.ch[MTU0].tier_w(v), // map:579
            0xffff8265 => self.ch[MTU0].tsr_w(v), // map:580
            0xffff8266 => self.ch[MTU0].tcnt_w((u16::from(v)) << 8, 0xff00), // map:581
            0xffff8267 => self.ch[MTU0].tcnt_w(u16::from(v), 0x00ff), // map:582
            0xffff8268 => self.ch[MTU0].tgr_w(0, (u16::from(v)) << 8, 0xff00), // map:583
            0xffff8269 => self.ch[MTU0].tgr_w(0, u16::from(v), 0x00ff), // map:584
            0xffff826a => self.ch[MTU0].tgr_w(1, (u16::from(v)) << 8, 0xff00), // map:585
            0xffff826b => self.ch[MTU0].tgr_w(1, u16::from(v), 0x00ff), // map:586
            0xffff826c => self.ch[MTU0].tgr_w(2, (u16::from(v)) << 8, 0xff00), // map:587
            0xffff826d => self.ch[MTU0].tgr_w(2, u16::from(v), 0x00ff), // map:588
            0xffff826e => self.ch[MTU0].tgr_w(3, (u16::from(v)) << 8, 0xff00), // map:589
            0xffff826f => self.ch[MTU0].tgr_w(3, u16::from(v), 0x00ff), // map:590
            0xffff8280 => self.ch[MTU1].tcr_w(v), // map:591
            0xffff8281 => self.ch[MTU1].tmdr_w(v), // map:592
            0xffff8282 | 0xffff8283 => self.ch[MTU1].tior_w(v), // map:593/594
            0xffff8284 => self.ch[MTU1].tier_w(v), // map:595
            0xffff8285 => self.ch[MTU1].tsr_w(v), // map:596
            0xffff8286 => self.ch[MTU1].tcnt_w((u16::from(v)) << 8, 0xff00), // map:597
            0xffff8287 => self.ch[MTU1].tcnt_w(u16::from(v), 0x00ff), // map:598
            0xffff8288 => self.ch[MTU1].tgr_w(0, (u16::from(v)) << 8, 0xff00), // map:599
            0xffff8289 => self.ch[MTU1].tgr_w(0, u16::from(v), 0x00ff), // map:600
            0xffff828a => self.ch[MTU1].tgr_w(1, (u16::from(v)) << 8, 0xff00), // map:601
            0xffff828b => self.ch[MTU1].tgr_w(1, u16::from(v), 0x00ff), // map:602
            0xffff82a0 => self.ch[MTU2].tcr_w(v), // map:603
            0xffff82a1 => self.ch[MTU2].tmdr_w(v), // map:604
            0xffff82a2 | 0xffff82a3 => self.ch[MTU2].tior_w(v), // map:605/606
            0xffff82a4 => self.ch[MTU2].tier_w(v), // map:607
            0xffff82a5 => self.ch[MTU2].tsr_w(v), // map:608
            0xffff82a6 => self.ch[MTU2].tcnt_w((u16::from(v)) << 8, 0xff00), // map:609
            0xffff82a7 => self.ch[MTU2].tcnt_w(u16::from(v), 0x00ff), // map:610
            0xffff82a8 => self.ch[MTU2].tgr_w(0, (u16::from(v)) << 8, 0xff00), // map:611
            0xffff82a9 => self.ch[MTU2].tgr_w(0, u16::from(v), 0x00ff), // map:612
            0xffff82aa => self.ch[MTU2].tgr_w(1, (u16::from(v)) << 8, 0xff00), // map:613
            0xffff82ab => self.ch[MTU2].tgr_w(1, u16::from(v), 0x00ff), // map:614
            _ => {} // hole / bus miss — write dropped
        }
    }

    /// origin: src/mame/cpu/sh7042_map.hxx:802-843 (internal_w16). Composite
    /// pairs write hi-byte-then-lo-byte in that order; the TIOR pairs write
    /// the SAME register twice (map:804/805,825,833,839), and 8264/8284/82a4
    /// write TIER(hi) then TSR(lo) — preserved exactly.
    fn mtu_w16(&mut self, a: u32, v: u16) {
        let hi = (v >> 8) as u8;
        let lo = v as u8;
        match a {
            0xffff8200 => {
                self.ch[MTU3].tcr_w(hi); // map:802
                self.ch[MTU4].tcr_w(lo);
            }
            0xffff8202 => {
                self.ch[MTU3].tmdr_w(hi); // map:803
                self.ch[MTU4].tmdr_w(lo);
            }
            0xffff8204 => {
                self.ch[MTU3].tior_w(hi); // map:804 (TIOR twice)
                self.ch[MTU3].tior_w(lo);
            }
            0xffff8206 => {
                self.ch[MTU4].tior_w(hi); // map:805
                self.ch[MTU4].tior_w(lo);
            }
            0xffff8208 => {
                self.ch[MTU3].tier_w(hi); // map:806
                self.ch[MTU4].tier_w(lo);
            }
            0xffff820a => {
                self.toer_w(hi); // map:807
                self.tocr_w(lo);
            }
            0xffff8210 => self.ch[MTU3].tcnt_w(v, 0xffff), // map:808
            0xffff8212 => self.ch[MTU4].tcnt_w(v, 0xffff), // map:809
            0xffff8214 => self.tcdr_w(v, 0xffff), // map:810
            0xffff8216 => self.tddr_w(v, 0xffff), // map:811
            0xffff8218 => self.ch[MTU3].tgr_w(0, v, 0xffff), // map:812
            0xffff821a => self.ch[MTU3].tgr_w(1, v, 0xffff), // map:813
            0xffff821c => self.ch[MTU4].tgr_w(0, v, 0xffff), // map:814
            0xffff821e => self.ch[MTU4].tgr_w(1, v, 0xffff), // map:815
            0xffff8220 => self.tcnts_w(v, 0xffff), // map:816
            0xffff8222 => self.tcbr_w(v, 0xffff), // map:817
            0xffff8224 => self.ch[MTU3].tgrc_w(0, v, 0xffff), // map:818
            0xffff8226 => self.ch[MTU3].tgrc_w(1, v, 0xffff), // map:819
            0xffff8228 => self.ch[MTU4].tgrc_w(0, v, 0xffff), // map:820
            0xffff822a => self.ch[MTU4].tgrc_w(1, v, 0xffff), // map:821
            0xffff822c => {
                self.ch[MTU3].tsr_w(hi); // map:822
                self.ch[MTU4].tsr_w(lo);
            }
            0xffff8240 => {
                self.tstr_w(hi); // map:823
                self.tsyr_w(lo);
            }
            0xffff8260 => {
                self.ch[MTU0].tcr_w(hi); // map:824
                self.ch[MTU0].tmdr_w(lo);
            }
            0xffff8262 => {
                self.ch[MTU0].tior_w(hi); // map:825 (TIOR twice)
                self.ch[MTU0].tior_w(lo);
            }
            0xffff8264 => {
                self.ch[MTU0].tier_w(hi); // map:826 (TIER then TSR)
                self.ch[MTU0].tsr_w(lo);
            }
            0xffff8266 => self.ch[MTU0].tcnt_w(v, 0xffff), // map:827
            0xffff8268 => self.ch[MTU0].tgr_w(0, v, 0xffff), // map:828
            0xffff826a => self.ch[MTU0].tgr_w(1, v, 0xffff), // map:829
            0xffff826c => self.ch[MTU0].tgr_w(2, v, 0xffff), // map:830
            0xffff826e => self.ch[MTU0].tgr_w(3, v, 0xffff), // map:831
            0xffff8280 => {
                self.ch[MTU1].tcr_w(hi); // map:832
                self.ch[MTU1].tmdr_w(lo);
            }
            0xffff8282 => {
                self.ch[MTU1].tior_w(hi); // map:833 (TIOR twice)
                self.ch[MTU1].tior_w(lo);
            }
            0xffff8284 => {
                self.ch[MTU1].tier_w(hi); // map:834 (TIER then TSR)
                self.ch[MTU1].tsr_w(lo);
            }
            0xffff8286 => self.ch[MTU1].tcnt_w(v, 0xffff), // map:835
            0xffff8288 => self.ch[MTU1].tgr_w(0, v, 0xffff), // map:836
            0xffff828a => self.ch[MTU1].tgr_w(1, v, 0xffff), // map:837
            0xffff82a0 => {
                self.ch[MTU2].tcr_w(hi); // map:838
                self.ch[MTU2].tmdr_w(lo);
            }
            0xffff82a2 => {
                self.ch[MTU2].tior_w(hi); // map:839 (TIOR twice)
                self.ch[MTU2].tior_w(lo);
            }
            0xffff82a4 => {
                self.ch[MTU2].tier_w(hi); // map:840 (TIER then TSR)
                self.ch[MTU2].tsr_w(lo);
            }
            0xffff82a6 => self.ch[MTU2].tcnt_w(v, 0xffff), // map:841
            0xffff82a8 => self.ch[MTU2].tgr_w(0, v, 0xffff), // map:842
            0xffff82aa => self.ch[MTU2].tgr_w(1, v, 0xffff), // map:843
            _ => {}
        }
    }

    /// origin: src/mame/cpu/sh7042.cpp:286-290 — scheduler seam (mtu0..mtu4).
    /// The sh7042.rs `internal_update_at` loop owns add_event/recompute, which
    /// replaces the C++ `m_cpu->internal_update()` recursion (module doc).
    fn mtu_ch_update(&mut self, ch: usize, current_time: u64) -> u64 {
        self.ch
            .get_mut(ch)
            .map_or(0, |c| c.internal_update(current_time)) // sh_mtu.cpp:394
    }
}
