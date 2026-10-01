//! SH Serial Communications Interface — transliteration of
//! `src/mame/cpu/sh_sci.cpp` (800 lines) + `src/mame/cpu/sh_sci.h` (165).
//! Ledger row `periph: sci` (M2). Two instances exist on the die
//! (sh7042.cpp:237-238, `sci0`/`sci1`); `Sh2SciPair` holds both and IS the
//! `Sh7042Peripherals` seam the sh7042.rs `sci_*` case-sets dispatch into
//! (map:15-26/328-333/520-531/796-801; the 81a5/81b5 RDR read-only hole is
//! enforced by the sh7042.rs arms, NOT here — but `w8` drops it again
//! defensively).
//!
//! HOST-INTERACTION DESIGN (borrow-split replacement for the C++ `m_cpu`/
//! `m_intc` device refs; the wiring row implements these drains):
//! - `m_cpu->current_cycles()` (sh_sci.cpp:475,484) -> [`Sh2Sci::cpu_now`],
//!   host-maintained. Contract: the wiring row writes the exact cycle count
//!   (with the compat `current_cycles()` −1 write-tick semantics, M1 timers
//!   row) BEFORE any register write / `do_rx_w` / `do_clk_w` / fast-MIDI
//!   feed. `sci_update` (the scheduler seam) auto-syncs it to the chain's
//!   `current_time` — in C++ that call arrives from the timer callback at
//!   exactly that time (sh7042.cpp:244), so tick-origin `clock_start`
//!   reads (rx_done -> rx_start) match exactly.
//! - `m_cpu->internal_update()` (sh_sci.cpp:441,477,486,505) -> sticky
//!   [`Sh2Sci::resched`]; wiring drains after each instruction/tick and
//!   calls `Sh7042::internal_update()`. The :441 site (inside `internal_update`)
//!   is subsumed by `internal_update_at`'s own `recompute_timer`
//!   (sh7042.cpp:299) — the C++ recursion there re-enters the same chain
//!   with the same `current_time` and every device is already advanced, so
//!   the settle value of `m_event_cycles` is identical (abort_timeslice is
//!   latch-like; one bail == two).
//! - `m_intc->internal_interrupt(v)` (sh_sci.cpp:146,173,175,177,179,515,
//!   603,639,685,687) -> ordered FIFO [`Sh2Sci::irq_req`] of ACTUAL vector
//!   numbers (sci0: eri=128,rxi=129,txi=130,tei=131; sci1: 132..135 —
//!   sh7042.cpp:237-238). INTC's pending store is a bitmask
//!   (sh_intc.cpp:97 `|=`), so duplicates across drains are idempotent;
//!   order within a drain is preserved.
//! - `m_cpu->do_sci_clk/do_sci_tx` (many sites + device_reset:366-367) ->
//!   pin mirrors [`Sh2Sci::clk_pin`]/[`tx_pin`] plus a bounded change ring
//!   [`Sh2Sci::poll_pin`]. C++ fires the devcb on EVERY call even when the
//!   value repeats (mamecompat.h:466-470), so the ring records every call,
//!   not just edges. Unbound devcbs are no-ops on disk (mamecompat.h:468
//!   `if (m_fn)`): the build binds ONLY sci0 tx -> `mu2000::tx_line`
//!   (mu2000.cpp:1147); sci0/sci1 clk and sci1 tx fire into nothing — the
//!   wiring row reproduces that binding table, the ring stays uniform.
//! - `m_cpu->pc()`/`m_cpu->clock()` appear ONLY inside LOGMASKED format
//!   strings (:86-93 etc.): logs dropped per the M1-bus/sh7042-row
//!   deviation (stderr-only, no machine state). `m_last_clock_message`
//!   (:135,302-305) is log-throttle state: dropped with the logs.
//! - `abort()` (:611,:756) -> `panic!`; C++ `assert` (:548,:578,:741,:747)
//!   -> `debug_assert!` (release MSYS2 build defines NDEBUG).
//! - attotime `m_external_clock_period` (:74/:79) -> `ext_period_never`;
//!   nothing on the S-MU2000 path ever calls `sci_set_external_clock_period`
//!   (only MAME ymmu2000.cpp), so the EXTERNAL_RATE_* modes stay unreachable
//!   — ported verbatim anyway. The double ratio math keeps IEEE f64 (no
//!   fast-math, Invariant 9).
//! - `state()` (:789-800) -> M5 serialization row (all fields are `pub`).
//! - Survey of the brief's "DTE / semaphore / sleep / break /
//!   standard_irq_callback / update_ints": ZERO hits on disk in sh_sci.{h,cpp}
//!   (verified rg) — this SH-2 SCI has none of those; the interrupt
//!   mask/priority behavior IS the scr_w edge matrix :161-179 + rx_done/tx
//!   end paths, ported verbatim below. Invented nothing.

use crate::sh7042::Sh7042Peripherals;

// origin: src/mame/cpu/sh_sci.h:71-73 (enum ST_*)
pub const ST_IDLE: i32 = 0;
pub const ST_START: i32 = 1;
pub const ST_BIT: i32 = 2;
pub const ST_PARITY: i32 = 3;
pub const ST_STOP: i32 = 4;
pub const ST_LAST_TICK: i32 = 5;

// origin: src/mame/cpu/sh_sci.h:77-80 (enum CLK_*)
pub const CLK_TX: i32 = 1;
pub const CLK_RX: i32 = 2;

// origin: src/mame/cpu/sh_sci.h:82-90 (enum clock modes)
pub const INTERNAL_ASYNC: u32 = 0;
pub const INTERNAL_ASYNC_OUT: u32 = 1;
pub const EXTERNAL_ASYNC: u32 = 2;
pub const EXTERNAL_RATE_ASYNC: u32 = 3;
pub const INTERNAL_SYNC_OUT: u32 = 4;
pub const EXTERNAL_SYNC: u32 = 5;
pub const EXTERNAL_RATE_SYNC: u32 = 6;

// origin: src/mame/cpu/sh_sci.h:92-119 (SMR/SCR/SSR bit enums)
pub const SMR_CA: u8 = 0x80;
pub const SMR_CHR: u8 = 0x40;
pub const SMR_PE: u8 = 0x20;
pub const SMR_OE: u8 = 0x10;
pub const SMR_STOP: u8 = 0x08;
pub const SMR_MP: u8 = 0x04;
pub const SMR_CKS: u8 = 0x03;

pub const SCR_TIE: u8 = 0x80;
pub const SCR_RIE: u8 = 0x40;
pub const SCR_TE: u8 = 0x20;
pub const SCR_RE: u8 = 0x10;
pub const SCR_MPIE: u8 = 0x08;
pub const SCR_TEIE: u8 = 0x04;
pub const SCR_CKE: u8 = 0x03;
pub const SCR_CKE1: u8 = 0x02;
pub const SCR_CKE0: u8 = 0x01;

pub const SSR_TDRE: u8 = 0x80;
pub const SSR_RDRF: u8 = 0x40;
pub const SSR_ORER: u8 = 0x20;
pub const SSR_FER: u8 = 0x10;
pub const SSR_PER: u8 = 0x08;
pub const SSR_TEND: u8 = 0x04;
pub const SSR_MPB: u8 = 0x02;
pub const SSR_MPBT: u8 = 0x01;

/// ring capacity for pin-callback events; drained after every bus op by the
/// wiring row, so depth is 2 in practice (one tx bit + one clk per tick).
/// Overflow is unreachable; if it ever happened `pin_dropped` is sticky.
pub const PIN_RING: usize = 64;

/// drain value of [`Sh2Sci::poll_pin`]: low bit = level, high bit selects
/// the clk pin (bit clear) vs tx pin (bit set) — every call of
/// do_sci_tx/do_sci_clk is one entry (C++ devcb fires unconditionally).
pub const PIN_TX: u8 = 0x80;
pub const PIN_CLK: u8 = 0x00;

// ---------------------------------------------------------------------------
// Sh2Sci — one SCI unit (sh_sci_device). Every field explicit (Invariant 3).
// ---------------------------------------------------------------------------
pub struct Sh2Sci {
    // origin: src/mame/cpu/sh_sci.h:126 (m_id, m_eri_int, m_rxi_int,
    // m_txi_int, m_tei_int; values per sh7042.cpp:237-238)
    pub m_id: i32,
    pub m_eri_int: i32,
    pub m_rxi_int: i32,
    pub m_txi_int: i32,
    pub m_tei_int: i32,

    // origin: sh_sci.h:123-124. attotime::never at ctor (:74) = true here;
    // do_set_external_clock_period (:77-80) clears it. Ratios computed in
    // device_start (:311-317) — 0/0 while never.
    pub ext_period_never: bool,
    pub m_external_to_internal_ratio: f64,
    pub m_internal_to_external_ratio: f64,

    // origin: sh_sci.h:128-130
    pub m_tx_state: i32,
    pub m_rx_state: i32,
    pub m_tx_bit: i32,
    pub m_rx_bit: i32,
    pub m_clock_state: i32,
    pub m_tx_parity: i32,
    pub m_rx_parity: i32,
    pub m_tx_clock_counter: i32,
    pub m_rx_clock_counter: i32,
    pub m_clock_mode: u32,
    pub m_ext_clock_value: bool,
    pub m_rx_value: bool,

    // origin: sh_sci.h:132-133
    pub m_rdr: u8,
    pub m_tdr: u8,
    pub m_smr: u8,
    pub m_scr: u8,
    pub m_ssr: u8,
    pub m_brr: u8,
    pub m_rsr: u8,
    pub m_tsr: u8,
    pub m_clock_event: u64,
    pub m_clock_step: u64,
    pub m_divider: u64,

    // ---- host seams (see module doc) ----
    /// stand-in for `m_cpu->current_cycles()` at sh_sci.cpp:475/484
    pub cpu_now: u64,
    /// sticky "call Sh7042::internal_update()" (m_cpu->internal_update()
    /// sites :441,:477,:486,:505)
    pub resched: bool,
    /// FIFO of pending m_intc->internal_interrupt(vector) calls
    pub irq_req: [i32; 16],
    pub irq_len: usize,
    pub irq_dropped: u64,
    /// pin callback ring: (code, level); code bit7 = tx vs clk
    pub pin_ev: [(u8, bool); PIN_RING],
    pub pin_len: usize,
    pub pin_dropped: u64,
    pub tx_pin: bool,
    pub clk_pin: bool,
}

impl Sh2Sci {
    /// ctor: origin: src/mame/cpu/sh_sci.cpp:65-75 (m_clock_mode was
    /// INTERNAL_ASYNC=0, m_rx_value=true, m_ext_clock_value=false, all
    /// others 0; `m_id`/vectors come from the template ctor sh_sci.h:30-40
    /// with the values at sh7042.cpp:237-238).
    pub fn new(id: i32, eri: i32, rxi: i32, txi: i32, tei: i32) -> Self {
        Self {
            m_id: id,                    // sh_sci.h:35
            m_eri_int: eri,              // :36
            m_rxi_int: rxi,              // :37
            m_txi_int: txi,              // :38
            m_tei_int: tei,              // :39
            ext_period_never: true,      // :74 attotime::never
            m_external_to_internal_ratio: 0.0, // :69
            m_internal_to_external_ratio: 0.0, // :69
            m_tx_state: 0,               // :70
            m_rx_state: 0,               // :70
            m_tx_bit: 0,                 // :70
            m_rx_bit: 0,                 // :70
            m_clock_state: 0,            // :70
            m_tx_parity: 0,              // :70
            m_rx_parity: 0,              // :70
            m_tx_clock_counter: 0,       // :70
            m_rx_clock_counter: 0,       // :70
            m_clock_mode: INTERNAL_ASYNC, // :71
            m_ext_clock_value: false,    // :71
            m_rx_value: true,            // :71
            m_rdr: 0,                    // :72
            m_tdr: 0,                    // :72
            m_smr: 0,                    // :72
            m_scr: 0,                    // :72
            m_ssr: 0,                    // :72
            m_brr: 0,                    // :72
            m_rsr: 0,                    // :72
            m_tsr: 0,                    // :72
            m_clock_event: 0,            // :72
            m_clock_step: 0,             // sh_sci.h:133 default 0 (no ctor init on disk — 0)
            m_divider: 0,                // :72
            cpu_now: 0,
            resched: false,
            irq_req: [0; 16],
            irq_len: 0,
            irq_dropped: 0,
            pin_ev: [(0, false); PIN_RING],
            pin_len: 0,
            pin_dropped: 0,
            tx_pin: false, // devcb containers start unbound->quiet; the
            clk_pin: false, // first real fire is device_reset (:366-367)
        }
    }

    /// device_start ratios: origin: src/mame/cpu/sh_sci.cpp:309-317
    /// (save_item :319-343 = M5 serialization row — all fields already pub).
    /// `clock()` there feeds only LOG strings (:281-299): dropped.
    pub fn device_start(&mut self) {
        if self.ext_period_never {
            // sh_sci.cpp:311-313
            self.m_internal_to_external_ratio = 0.0;
            self.m_external_to_internal_ratio = 0.0;
        } else {
            // sh_sci.cpp:315-316 — attotime*clock as_double; unreachable on
            // this build (no sci_set_external_clock_period caller), kept
            // faithful: ratio == period_seconds * cpu_hz (wiring supplies the
            // product via `set_external_clock_period_ratios`).
            // (ratios already set — see setter)
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:77-80 (do_set_external_clock_period).
    /// MAME-only plumbing (sh7042.h:41-43); never called by mu2000. Ratios
    /// are passed pre-computed because the compat layer has no attotime.
    pub fn set_external_clock_period_ratios(
        &mut self,
        ext_to_int: f64,
        int_to_ext: f64,
    ) {
        self.ext_period_never = false; // period != never
        self.m_external_to_internal_ratio = ext_to_int; // :315
        self.m_internal_to_external_ratio = int_to_ext; // :316
    }

    /// origin: src/mame/cpu/sh_sci.cpp:345-368 (device_reset).
    pub fn device_reset(&mut self) {
        self.m_rdr = 0x00; // :347
        self.m_tdr = 0xff; // :348
        self.m_smr = 0x00; // :349
        self.m_scr = 0x00; // :350
        self.m_ssr = 0x84; // :351  TDRE|TEND
        self.m_brr = 0xff; // :352
        self.m_rsr = 0x00; // :353
        self.m_tsr = 0xff; // :354
        self.m_rx_bit = 0; // :355
        self.m_tx_bit = 0; // :356
        self.m_tx_state = ST_IDLE; // :357
        self.m_rx_state = ST_IDLE; // :358
        self.m_clock_state = 0; // :359
        self.m_clock_mode = INTERNAL_ASYNC; // :360
        self.m_clock_event = 0; // :361
        self.clock_update(); // :362
        self.m_ext_clock_value = true; // :363
        self.m_tx_clock_counter = 0; // :364
        self.m_rx_clock_counter = 0; // :365
        // NOTE: m_rx_value NOT reset on disk (stays ctor-true until a do_rx_w).
        self.do_sci_clk(true); // :366 m_cpu->do_sci_clk(m_id, 1)
        self.do_sci_tx(true); // :367 m_cpu->do_sci_tx(m_id, 1)
    }

    // ---- register handlers (sh7042_map.hxx sci case-sets) ----

    /// origin: src/mame/cpu/sh_sci.cpp:82-96 (smr_w; log dropped)
    pub fn smr_w(&mut self, data: u8) {
        self.m_smr = data; // :84
        self.clock_update(); // :95
    }

    /// origin: src/mame/cpu/sh_sci.cpp:98-102 (smr_r)
    pub fn smr_r(&self) -> u8 {
        self.m_smr // :101
    }

    /// origin: src/mame/cpu/sh_sci.cpp:104-109 (brr_w)
    pub fn brr_w(&mut self, data: u8) {
        self.m_brr = data; // :106
        self.clock_update(); // :108
    }

    /// origin: src/mame/cpu/sh_sci.cpp:111-115 (brr_r)
    pub fn brr_r(&self) -> u8 {
        self.m_brr // :114
    }

    /// origin: src/mame/cpu/sh_sci.cpp:117-120 (is_sync_start)
    pub fn is_sync_start(&self) -> bool {
        (self.m_smr & SMR_CA) != 0
            && (self.m_scr & (SCR_TE | SCR_RE)) == (SCR_TE | SCR_RE) // :119
    }

    /// origin: src/mame/cpu/sh_sci.cpp:122-125 (has_recv_error)
    pub fn has_recv_error(&self) -> bool {
        (self.m_ssr & (SSR_ORER | SSR_PER | SSR_FER)) != 0 // :124
    }

    /// origin: src/mame/cpu/sh_sci.cpp:127-130 (rx_can_accept)
    pub fn rx_can_accept(&self) -> bool {
        (self.m_scr & SCR_RE) != 0
            && (self.m_ssr & SSR_RDRF) == 0
            && !self.has_recv_error() // :129
    }

    /// origin: src/mame/cpu/sh_sci.cpp:132-135 (rx_byte_pending).
    /// mu2000.h:180/188/199 consumes this in fast_midi mode (midi_pending /
    /// midi_idle "byte still on the wire" accounting).
    pub fn rx_byte_pending(&self) -> bool {
        (self.m_ssr & SSR_RDRF) != 0 // :134
    }

    /// origin: src/mame/cpu/sh_sci.h:65 (`rx_enabled`, "firmware が MIDI の
    /// 受信を有効にしたか"): mu2000.h:112 `midi_ready(port)` == this.
    pub fn rx_enabled(&self) -> bool {
        (self.m_scr & SCR_RE) != 0
    }

    /// origin: src/mame/cpu/sh_sci.cpp:137-147 (receive_byte) — the
    /// fast_midi feed point: mu2000.cpp:1377-1382 pops a queued byte when
    /// `rx_can_accept()` and injects it DIRECTLY (no wire time, no start/
    /// stop bits). Actual queue/loop wiring is the later wiring row; the
    /// feed lands here.
    pub fn receive_byte(&mut self, data: u8) {
        if !self.rx_can_accept() {
            return; // :139-140
        }
        self.m_rdr = data; // :141
        self.m_ssr |= SSR_RDRF; // :142
        if (self.m_scr & SCR_RIE) != 0 {
            // :145-146
            self.internal_interrupt(self.m_rxi_int);
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:149-180 (scr_w — the interrupt
    /// mask/edge matrix: enable-bit delta AND status-bit must BOTH hold)
    pub fn scr_w(&mut self, data: u8) {
        let delta = self.m_scr ^ data; // :161
        self.m_scr = data; // :162
        self.clock_update(); // :163

        if (delta & SCR_RE) != 0 && (self.m_scr & SCR_RE) == 0 {
            // :165-168 — RE falling: kill RX
            self.m_rx_state = ST_IDLE;
            self.clock_stop(CLK_RX);
        }

        if (delta & SCR_RE) != 0
            && (self.m_scr & SCR_RE) != 0
            && self.m_rx_state == ST_IDLE
            && !self.has_recv_error()
            && !self.is_sync_start()
        {
            self.rx_start(); // :170-171
        }
        if (delta & SCR_TIE) != 0 && (self.m_scr & SCR_TIE) != 0 && (self.m_ssr & SSR_TDRE) != 0 {
            // :172-173
            self.internal_interrupt(self.m_txi_int);
        }
        if (delta & SCR_TEIE) != 0 && (self.m_scr & SCR_TEIE) != 0 && (self.m_ssr & SSR_TEND) != 0 {
            // :174-175
            self.internal_interrupt(self.m_tei_int);
        }
        if (delta & SCR_RIE) != 0 && (self.m_scr & SCR_RIE) != 0 && (self.m_ssr & SSR_RDRF) != 0 {
            // :176-177
            self.internal_interrupt(self.m_rxi_int);
        }
        if (delta & SCR_RIE) != 0 && (self.m_scr & SCR_RIE) != 0 && self.has_recv_error() {
            // :178-179
            self.internal_interrupt(self.m_eri_int);
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:182-186 (scr_r)
    pub fn scr_r(&self) -> u8 {
        self.m_scr // :185
    }

    /// origin: src/mame/cpu/sh_sci.cpp:188-199 (tdr_w; the DMA side-effect
    /// block :192-198 is `#if 0` dead — no start on write)
    pub fn tdr_w(&mut self, data: u8) {
        self.m_tdr = data; // :191
    }

    /// origin: src/mame/cpu/sh_sci.cpp:201-205 (tdr_r)
    pub fn tdr_r(&self) -> u8 {
        self.m_tdr // :203
    }

    /// origin: src/mame/cpu/sh_sci.cpp:207-223 (ssr_w — W1C-ish semantics:
    /// write-0 clears only within the :215 mask; TDRE is forced kept while
    /// TE is off; clearing TDRE also clears TEND; a 0-to-TDRE write while
    /// the tx is idle kicks tx_start)
    pub fn ssr_w(&mut self, data: u8) {
        let mut data = data; // :207 (local mutates)
        if (self.m_scr & SCR_TE) == 0 {
            // :209-212 — TE off: TDRE can never be cleared
            data |= SSR_TDRE;
            self.m_ssr |= SSR_TDRE;
        }
        if (self.m_ssr & SSR_TDRE) != 0 && (data & SSR_TDRE) == 0 {
            // :213-214
            self.m_ssr &= !SSR_TEND;
        }
        // :215 — MPBT follows the write, then mask: bits cleared in `data`
        // drop unless they are TEND/MPB/MPBT
        self.m_ssr =
            ((self.m_ssr & !SSR_MPBT) | (data & SSR_MPBT)) & (data | (SSR_TEND | SSR_MPB | SSR_MPBT));

        if self.m_tx_state == ST_IDLE && (self.m_ssr & SSR_TDRE) == 0 {
            // :218-219
            self.tx_start();
        }

        if (self.m_scr & SCR_RE) != 0
            && self.m_rx_state == ST_IDLE
            && !self.has_recv_error()
            && !self.is_sync_start()
        {
            self.rx_start(); // :221-222
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:225-229 (ssr_r)
    pub fn ssr_r(&self) -> u8 {
        self.m_ssr // :227
    }

    /// origin: src/mame/cpu/sh_sci.cpp:231-240 (rdr_r; DMA RDRF-clear block
    /// :235-238 is `#if 0` dead). Read-only from the bus (map has no 81a5/
    /// 81b5 write) — and reading does NOT clear RDRF on disk.
    pub fn rdr_r(&self) -> u8 {
        self.m_rdr // :239
    }

    /// origin: src/mame/cpu/sh_sci.cpp:242-245 (scmr_w — accepted, ignored;
    /// SCMR is not even stored on disk)
    pub fn scmr_w(&mut self, _data: u8) {}

    /// origin: src/mame/cpu/sh_sci.cpp:247-251 (scmr_r — always 0)
    pub fn scmr_r(&self) -> u8 {
        0x00 // :250
    }

    // ---- clocking ----

    /// origin: src/mame/cpu/sh_sci.cpp:253-307 (clock_update; the whole
    /// :277-306 LOG_RATE message machinery is logging — dropped, and with
    /// it m_last_clock_message)
    pub fn clock_update(&mut self) {
        // :255-256 — integer divider: (2 << (2*cks)) * (brr+1). C++ `2` is
        // int here but max = 128*256, no width risk; u64 to match m_divider.
        self.m_divider = 2u64 << (2 * (u32::from(self.m_smr & SMR_CKS))); // :255
        self.m_divider *= u64::from(self.m_brr) + 1; // :256

        // :258-270 mode select
        if (self.m_smr & SMR_CA) != 0 {
            if (self.m_scr & SCR_CKE1) != 0 {
                self.m_clock_mode = EXTERNAL_SYNC; // :260
            } else {
                self.m_clock_mode = INTERNAL_SYNC_OUT; // :262
            }
        } else if (self.m_scr & SCR_CKE1) != 0 {
            self.m_clock_mode = EXTERNAL_ASYNC; // :265
        } else if (self.m_scr & SCR_CKE0) != 0 {
            self.m_clock_mode = INTERNAL_ASYNC_OUT; // :267
        } else {
            self.m_clock_mode = INTERNAL_ASYNC; // :269
        }

        // :272-275 — external w/ a known period simulates a rate
        if self.m_clock_mode == EXTERNAL_ASYNC && !self.ext_period_never {
            self.m_clock_mode = EXTERNAL_RATE_ASYNC; // :273
        }
        if self.m_clock_mode == EXTERNAL_SYNC && !self.ext_period_never {
            self.m_clock_mode = EXTERNAL_RATE_SYNC; // :275
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:452-495 (clock_start).
    /// `current_cycles()` -> `cpu_now` (host-synced; see module doc).
    pub fn clock_start(&mut self, mode: i32) {
        if (self.m_clock_state & mode) != 0 {
            return; // :455-456 "Happens when back-to-back"
        }
        if mode == CLK_TX {
            self.m_tx_clock_counter = 15; // :458-459
        } else {
            self.m_rx_clock_counter = 15; // :460-461
        }
        self.m_clock_state |= mode; // :463
        if self.m_clock_state != mode {
            return; // :464-465 (the other direction already owns the event)
        }

        self.m_clock_step = 0; // :467
        match self.m_clock_mode {
            INTERNAL_ASYNC | INTERNAL_ASYNC_OUT | INTERNAL_SYNC_OUT => {
                // :470-479
                self.m_clock_step = self.m_divider; // :474
                let now = self.cpu_now; // :475 current_cycles()
                self.m_clock_event =
                    (now / self.m_clock_step + 1).wrapping_mul(self.m_clock_step); // :476
                self.resched = true; // :477 m_cpu->internal_update()
            }
            EXTERNAL_RATE_ASYNC | EXTERNAL_RATE_SYNC => {
                // :481-487 — IEEE f64 rounding chain kept verbatim (never
                // reached on this build: period is never)
                let now = self.cpu_now; // :484
                let a = ((now as f64) * self.m_internal_to_external_ratio + 1.0) as u64; // u64(now*ratio+1)
                self.m_clock_event = ((a as f64) * self.m_external_to_internal_ratio + 1.0) as u64; // :485
                self.resched = true; // :486
            }
            _ => {} // :490-493 EXTERNAL_ASYNC/SYNC: wait for pin edges
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:497-506 (clock_stop)
    pub fn clock_stop(&mut self, mode: i32) {
        self.m_clock_state &= !mode; // :499
        if self.m_clock_state == 0 {
            // :500-503
            self.m_clock_event = 0;
            self.m_clock_step = 0;
        }
        self.resched = true; // :505 m_cpu->internal_update()
    }

    /// origin: src/mame/cpu/sh_sci.cpp:393-413 (do_clk_w — external clock
    /// pin input; nothing drives it on this build, ported for fidelity)
    pub fn do_clk_w(&mut self, state: i32) {
        if i32::from(self.m_ext_clock_value) == state {
            return; // :395 (bool vs int: false=0/true=1 both sides)
        }
        self.m_ext_clock_value = state != 0; // :398 (int->bool narrowing)
        if self.m_clock_state == 0 {
            return; // :399-400
        }
        if self.m_clock_mode == EXTERNAL_ASYNC {
            // :402-406
            if (self.m_clock_state & CLK_TX) != 0 {
                self.tx_async_tick();
            }
            if (self.m_clock_state & CLK_RX) != 0 {
                self.rx_async_tick();
            }
        } else if self.m_clock_mode == EXTERNAL_SYNC {
            // :407-412
            if (self.m_clock_state & CLK_TX) != 0 {
                self.tx_sync_tick();
            }
            if (self.m_clock_state & CLK_RX) != 0 {
                self.rx_sync_tick();
            }
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:375-391 (do_rx_w — RX pin input).
    /// This is the 31250 bps bit-bang feed: mu2000.cpp:1397 (start bit),
    /// :1407 (data bits, LSB first), :1409 (stop). Initial level 1 is set
    /// by mu2000.cpp:1141-1142 (`sci_rx_w<0/1>(1)`).
    pub fn do_rx_w(&mut self, state: i32) {
        // :377-382 standby block is `#if 0` dead
        if state != i32::from(self.m_rx_value) && (self.m_clock_state & CLK_RX) != 0 {
            // :384-386 — re-phase on edges near a sample point
            if self.m_rx_clock_counter == 1 || self.m_rx_clock_counter == 15 {
                self.m_rx_clock_counter = 0;
            }
        }
        self.m_rx_value = state != 0; // :388
        if !self.m_rx_value && (self.m_clock_state & CLK_RX) == 0 && self.m_rx_state != ST_IDLE {
            // :389-390 — falling edge while armed -> start sampling
            self.clock_start(CLK_RX);
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:415-450 (internal_update). Consumed
    /// through the `sci_update` seam of [`Sh2SciPair`]; `current_time` also
    /// re-syncs `cpu_now` (timer-callback time, sh7042.cpp:244).
    pub fn internal_update(&mut self, current_time: u64) -> u64 {
        self.cpu_now = current_time; // host-sync (see module doc)
        if self.m_clock_event == 0 || current_time < self.m_clock_event {
            return self.m_clock_event; // :417-418
        }

        // :420-430 tick dispatch
        if self.m_clock_mode == INTERNAL_ASYNC
            || self.m_clock_mode == INTERNAL_ASYNC_OUT
            || self.m_clock_mode == EXTERNAL_RATE_ASYNC
        {
            if (self.m_clock_state & CLK_TX) != 0 {
                self.tx_async_tick();
            }
            if (self.m_clock_state & CLK_RX) != 0 {
                self.rx_async_tick();
            }
        } else if self.m_clock_mode == INTERNAL_SYNC_OUT
            || self.m_clock_mode == EXTERNAL_RATE_SYNC
        {
            if (self.m_clock_state & CLK_TX) != 0 {
                self.tx_sync_tick();
            }
            if (self.m_clock_state & CLK_RX) != 0 {
                self.rx_sync_tick();
            }
        }

        // :432-447
        if self.m_clock_state != 0 {
            if self.m_clock_step != 0 {
                self.m_clock_event = self.m_clock_event.wrapping_add(self.m_clock_step); // :434
            } else if self.m_clock_mode == EXTERNAL_RATE_ASYNC
                || self.m_clock_mode == EXTERNAL_RATE_SYNC
            {
                // :436 f64 chain (unreachable here, period never)
                let a = ((self.m_clock_event as f64) * self.m_internal_to_external_ratio + 1.0)
                    as u64;
                self.m_clock_event = ((a as f64) * self.m_external_to_internal_ratio + 1.0) as u64;
            } else {
                self.m_clock_event = 0; // :438
            }

            if self.m_clock_event != 0 {
                self.resched = true; // :440-441 (subsumed by the seam loop's
                // recompute_timer — identical settle value, see module doc)
            }
        } else {
            self.m_clock_event = 0; // :444
            if self.m_clock_mode == INTERNAL_ASYNC_OUT || self.m_clock_mode == INTERNAL_SYNC_OUT {
                self.do_sci_clk(true); // :445-446 clk idles high
            }
        }

        self.m_clock_event // :449
    }

    // ---- transmit ----

    /// origin: src/mame/cpu/sh_sci.cpp:508-526 (tx_start)
    pub fn tx_start(&mut self) {
        self.m_ssr |= SSR_TDRE; // :510
        self.m_tsr = self.m_tdr; // :511
        self.m_tx_parity = if (self.m_smr & SMR_OE) != 0 { 0 } else { 1 }; // :512
        if (self.m_scr & SCR_TIE) != 0 {
            self.internal_interrupt(self.m_txi_int); // :514-515
        }
        if (self.m_smr & SMR_CA) != 0 {
            // :516-518 sync: straight to bits
            self.m_tx_state = ST_BIT;
            self.m_tx_bit = 8;
        } else {
            self.m_tx_state = ST_START; // :520-521
            self.m_tx_bit = 1;
        }
        self.clock_start(CLK_TX); // :523
        if self.m_rx_state == ST_IDLE
            && !self.has_recv_error()
            && self.is_sync_start()
        {
            self.rx_start(); // :524-525 (sync mode pairs tx/rx)
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:528-540 (tx_async_tick)
    pub fn tx_async_tick(&mut self) {
        self.m_tx_clock_counter = (self.m_tx_clock_counter + 1) & 15; // :530
        if self.m_tx_clock_counter == 0 {
            // :532-536
            self.tx_async_step();
            if self.m_clock_mode == INTERNAL_ASYNC_OUT {
                self.do_sci_clk(false); // :536
            }
        } else if self.m_tx_clock_counter == 8 && self.m_clock_mode == INTERNAL_ASYNC_OUT {
            self.do_sci_clk(true); // :538-539
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:542-614 (tx_async_step)
    pub fn tx_async_step(&mut self) {
        match self.m_tx_state {
            ST_START => {
                // :546-551
                self.do_sci_tx(false); // start bit low
                debug_assert!(self.m_tx_bit == 1); // :548
                self.m_tx_state = ST_BIT;
                self.m_tx_bit = if (self.m_smr & SMR_CHR) != 0 { 7 } else { 8 };
            }
            ST_BIT => {
                // :553-574 — LSB first out of m_tsr
                self.m_tx_parity ^= (self.m_tsr & 1) as i32; // :554
                self.do_sci_tx((self.m_tsr & 1) != 0); // :555
                self.m_tsr >>= 1; // :556
                self.m_tx_bit -= 1; // :557
                if self.m_tx_bit == 0 {
                    // :558-573
                    if (self.m_smr & SMR_CA) != 0 {
                        if (self.m_ssr & SSR_TDRE) == 0 {
                            self.tx_start(); // :560-561 back-to-back byte
                        } else {
                            self.m_tx_state = ST_LAST_TICK; // :563-564
                            self.m_tx_bit = 0;
                        }
                    } else if (self.m_smr & SMR_PE) != 0 {
                        self.m_tx_state = ST_PARITY; // :566-568
                        self.m_tx_bit = 1;
                    } else {
                        self.m_tx_state = ST_STOP; // :570-571
                        self.m_tx_bit = if (self.m_smr & SMR_STOP) != 0 { 2 } else { 1 };
                    }
                }
            }
            ST_PARITY => {
                // :576-581
                self.do_sci_tx(self.m_tx_parity != 0); // :577
                debug_assert!(self.m_tx_bit == 1); // :578
                self.m_tx_state = ST_STOP; // :579
                self.m_tx_bit = if (self.m_smr & SMR_STOP) != 0 { 2 } else { 1 }; // :580
            }
            ST_STOP => {
                // :583-594
                self.do_sci_tx(true); // stop bit high :584
                self.m_tx_bit -= 1; // :585
                if self.m_tx_bit == 0 {
                    if (self.m_ssr & SSR_TDRE) == 0 {
                        self.tx_start(); // :587-588
                    } else {
                        self.m_tx_state = ST_LAST_TICK; // :590-591
                        self.m_tx_bit = 0;
                    }
                }
            }
            ST_LAST_TICK => {
                // :596-608
                self.m_tx_state = ST_IDLE; // :597
                self.m_tx_bit = 0; // :598
                self.clock_stop(CLK_TX); // :599
                self.do_sci_tx(true); // :600 line idles high
                self.m_ssr |= SSR_TEND; // :601
                if (self.m_scr & SCR_TEIE) != 0 {
                    self.internal_interrupt(self.m_tei_int); // :602-603
                }
                if (self.m_scr & SCR_TE) != 0 && (self.m_ssr & SSR_TDRE) == 0 {
                    self.tx_start(); // :606-607 "more to send"
                }
            }
            // :610-611 default: abort()
            _ => panic!("sci{} tx state {}", self.m_id, self.m_tx_state),
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:616-628 (tx_sync_tick)
    pub fn tx_sync_tick(&mut self) {
        self.m_tx_clock_counter = (self.m_tx_clock_counter + 1) & 1; // :618
        if self.m_tx_clock_counter == 0 {
            // :620-624
            self.tx_sync_step();
            if self.m_clock_mode == INTERNAL_SYNC_OUT && self.m_tx_state != ST_IDLE {
                self.do_sci_clk(false); // :624
            }
        } else if self.m_tx_clock_counter == 1 && self.m_clock_mode == INTERNAL_SYNC_OUT {
            self.do_sci_clk(true); // :626-627
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:630-649 (tx_sync_step)
    pub fn tx_sync_step(&mut self) {
        if self.m_tx_bit == 0 {
            // :633-643 — end of byte
            self.m_tx_state = ST_IDLE;
            self.clock_stop(CLK_TX);
            self.do_sci_tx(true);
            self.m_ssr |= SSR_TEND;
            if (self.m_scr & SCR_TEIE) != 0 {
                self.internal_interrupt(self.m_tei_int);
            }
            if (self.m_scr & SCR_TE) != 0 && (self.m_ssr & SSR_TDRE) == 0 {
                self.tx_start();
            }
        } else {
            // :644-647 LSB first
            self.do_sci_tx((self.m_tsr & 1) != 0);
            self.m_tsr >>= 1;
            self.m_tx_bit -= 1;
        }
    }

    // ---- receive ----

    /// origin: src/mame/cpu/sh_sci.cpp:651-666 (rx_start)
    pub fn rx_start(&mut self) {
        self.m_rx_parity = if (self.m_smr & SMR_OE) != 0 { 0 } else { 1 }; // :653
        self.m_rsr = 0x00; // :654
        if (self.m_smr & SMR_CA) != 0 {
            // :656-659 sync: clock immediately
            self.m_rx_state = ST_BIT;
            self.m_rx_bit = 8;
            self.clock_start(CLK_RX);
        } else {
            self.m_rx_state = ST_START; // :661-664 async: wait for the
            self.m_rx_bit = 1; // falling edge of the start bit
            if !self.m_rx_value {
                self.clock_start(CLK_RX);
            }
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:668-695 (rx_done)
    pub fn rx_done(&mut self) {
        if (self.m_ssr & SSR_FER) == 0 {
            // :670-682 — parity, then overrun, else deliver
            if (self.m_smr & SMR_PE) != 0 && self.m_rx_parity != 0 {
                self.m_ssr |= SSR_PER; // :671-673
            } else if (self.m_ssr & SSR_RDRF) != 0 {
                self.m_ssr |= SSR_ORER; // :674-676
            } else {
                self.m_ssr |= SSR_RDRF; // :678-680
                self.m_rdr = self.m_rsr; // :680
            }
        }
        if (self.m_scr & SCR_RIE) != 0 {
            // :683-688
            if self.has_recv_error() {
                self.internal_interrupt(self.m_eri_int); // :684-685
            } else {
                self.internal_interrupt(self.m_rxi_int); // :686-687
            }
        }
        if (self.m_scr & SCR_RE) != 0 && !self.has_recv_error() && !self.is_sync_start() {
            self.rx_start(); // :689-690 keep receiving
        } else {
            self.clock_stop(CLK_RX); // :692-693
            self.m_rx_state = ST_IDLE; // :693
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:697-703 (rx_async_tick)
    pub fn rx_async_tick(&mut self) {
        self.m_rx_clock_counter = (self.m_rx_clock_counter + 1) & 15; // :699
        if self.m_rx_clock_counter == 8 {
            self.rx_async_step(); // :701-702 sample mid-bit
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:705-759 (rx_async_step)
    pub fn rx_async_step(&mut self) {
        match self.m_rx_state {
            ST_START => {
                // :709-716
                if self.m_rx_value {
                    // false start: line bounced high
                    self.clock_stop(CLK_RX); // :711
                    return; // :712
                }
                self.m_rx_state = ST_BIT; // :714
                self.m_rx_bit = if (self.m_smr & SMR_CHR) != 0 { 7 } else { 8 }; // :715
            }
            ST_BIT => {
                // :718-737 — LSB first into m_rsr. NOTE the disk quirk
                // :719/^722: parity ^= value then !parity when value==1 —
                // net effect parity toggles only on ZERO data bits. Kept
                // verbatim.
                self.m_rx_parity ^= i32::from(self.m_rx_value); // :719
                self.m_rsr >>= 1; // :720
                if self.m_rx_value {
                    // :721-724
                    self.m_rx_parity = if self.m_rx_parity == 0 { 1 } else { 0 }; // !parity :722
                    self.m_rsr |= if (self.m_smr & (SMR_CA | SMR_CHR)) == SMR_CHR {
                        0x40 // 7-bit frame: fill bit6 :723
                    } else {
                        0x80
                    };
                }
                self.m_rx_bit -= 1; // :725
                if self.m_rx_bit == 0 {
                    // :726-736
                    if (self.m_smr & SMR_CA) != 0 {
                        self.rx_done(); // :727-728
                    } else if (self.m_smr & SMR_PE) != 0 {
                        self.m_rx_state = ST_PARITY; // :729-731
                        self.m_rx_bit = 1;
                    } else {
                        self.m_rx_state = ST_STOP; // :733-734
                        self.m_rx_bit = 1; // "Always 1 on rx" :734
                    }
                }
            }
            ST_PARITY => {
                // :739-744
                self.m_rx_parity ^= i32::from(self.m_rx_value); // :740
                debug_assert!(self.m_rx_bit == 1); // :741
                self.m_rx_state = ST_STOP; // :742
                self.m_rx_bit = 1; // :743
            }
            ST_STOP => {
                // :746-753
                debug_assert!(self.m_rx_bit == 1); // :747
                if !self.m_rx_value {
                    self.m_ssr |= SSR_FER; // :748-749 framing error
                } else if (self.m_smr & SMR_PE) != 0 && self.m_rx_parity != 0 {
                    self.m_ssr |= SSR_PER; // :750-751 parity error
                }
                self.rx_done(); // :752
            }
            // :755-756 default: abort()
            _ => panic!("sci{} rx state {}", self.m_id, self.m_rx_state),
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:761-775 (rx_sync_tick)
    pub fn rx_sync_tick(&mut self) {
        self.m_rx_clock_counter = (self.m_rx_clock_counter + 1) & 1; // :763
        if self.m_rx_clock_counter == 0 && self.m_clock_mode == INTERNAL_SYNC_OUT {
            self.do_sci_clk(false); // :766-767
        } else if self.m_rx_clock_counter == 1 {
            // :769-774
            if self.m_clock_mode == INTERNAL_SYNC_OUT {
                self.do_sci_clk(true); // :770-771
            }
            self.rx_sync_step(); // :773
        }
    }

    /// origin: src/mame/cpu/sh_sci.cpp:777-787 (rx_sync_step)
    pub fn rx_sync_step(&mut self) {
        self.m_rsr >>= 1; // :780
        if self.m_rx_value {
            self.m_rsr |= 0x80; // :781-782
        }
        self.m_rx_bit -= 1; // :783
        if self.m_rx_bit == 0 {
            self.rx_done(); // :785-786
        }
    }

    // ---- host-seam helpers ----

    /// `m_intc->internal_interrupt(vector)` — origin every call site listed
    /// in the module doc. Ordered FIFO (intc pending `|=` is idempotent).
    fn internal_interrupt(&mut self, vector: i32) {
        if self.irq_len < self.irq_req.len() {
            self.irq_req[self.irq_len] = vector; // sh_intc.cpp:97 latches
            self.irq_len += 1;
        } else {
            self.irq_dropped += 1; // unreachable when drained per bus op
        }
    }

    /// drain FIFO -> (vec count, vectors). Wiring row forwards to the
    /// sh_intc port `internal_interrupt(vector)` (later row).
    pub fn drain_irqs(&mut self, out: &mut Vec<i32>) -> usize {
        let n = self.irq_len;
        out.extend_from_slice(&self.irq_req[..n]);
        self.irq_len = 0;
        n
    }

    /// take+clear the sticky resched (m_cpu->internal_update() sites).
    pub fn take_resched(&mut self) -> bool {
        let r = self.resched;
        self.resched = false;
        r
    }

    /// `m_cpu->do_sci_tx(m_id, state)` — every fire is recorded (devcb
    /// fires unconditionally, mamecompat.h:468).
    fn do_sci_tx(&mut self, state: bool) {
        self.tx_pin = state; // sh7042.h:106 m_sci_tx[sci](state)
        self.push_pin(PIN_TX, state);
    }

    /// `m_cpu->do_sci_clk(m_id, state)` — sh7042.h:107; unbound in this
    /// build (devcb no-op), ring kept for uniformity.
    fn do_sci_clk(&mut self, state: bool) {
        self.clk_pin = state; // sh7042.h:107
        self.push_pin(PIN_CLK, state);
    }

    fn push_pin(&mut self, code: u8, level: bool) {
        if self.pin_len < PIN_RING {
            self.pin_ev[self.pin_len] = (code, level);
            self.pin_len += 1;
        } else {
            self.pin_dropped += 1; // unreachable (drained per bus op)
        }
    }

    /// drain pin ring -> (code, level); code: bit7 set = tx, clear = clk.
    pub fn poll_pin(&mut self, out: &mut Vec<(u8, bool)>) -> usize {
        let n = self.pin_len;
        out.extend_from_slice(&self.pin_ev[..n]);
        self.pin_len = 0;
        n
    }
}

// ---------------------------------------------------------------------------
// Sh2SciPair — the Sh7042Peripherals seam (both die SCIs + the mu2000.h
// views the machine layer needs).
// ---------------------------------------------------------------------------
pub struct Sh2SciPair {
    pub sci: [Sh2Sci; 2],
}

impl Sh2SciPair {
    /// ctor + device_start for both, vector numbers from sh7042.cpp:237-238
    /// (`SH_SCI(config, m_sci[0], 0, *this, m_intc, 128,129,130,131)` /
    /// `m_sci[1], 1, ... 132,133,134,135`).
    pub fn new() -> Self {
        let mut sci = [
            Sh2Sci::new(0, 128, 129, 130, 131), // sh7042.cpp:237
            Sh2Sci::new(1, 132, 133, 134, 135), // sh7042.cpp:238
        ];
        for s in sci.iter_mut() {
            s.device_start(); // sh_sci.cpp:309-317
        }
        Self { sci }
    }

    /// both units' device_reset (machine reset fan-out; sh7042.rs
    /// `device_reset` only walks sh2 — the compat build resets devices
    /// explicitly from mu2000.cpp, so the wiring row calls this there).
    pub fn device_reset(&mut self) {
        for s in self.sci.iter_mut() {
            s.device_reset(); // sh_sci.cpp:345-368
        }
    }

    /// mu2000.h:112 `midi_ready(port)` == `sci(port)->rx_enabled()` —
    /// RE bit set means firmware finished booting its RX; hold MIDI until
    /// then ("起動が終わるまで送りつけないための目印").
    pub fn midi_ready(&self, port: usize) -> bool {
        self.sci.get(port).is_some_and(|s| s.rx_enabled()) // sh_sci.h:65
    }

    /// mu2000.h:180/188/199 — fast_midi "a byte still on the virtual wire"
    /// accounting (midi_pending / midi_idle).
    pub fn rx_byte_pending(&self, port: usize) -> bool {
        self.sci.get(port).is_some_and(|s| s.rx_byte_pending()) // sh_sci.cpp:134
    }

    /// fast_midi feed point — mu2000.cpp:1376-1383: wiring pops a queued
    /// byte when `rx_can_accept()`, then `receive_byte`. (Queue pump loop =
    /// the wiring row.)
    pub fn receive_byte(&mut self, port: usize, data: u8) {
        if let Some(s) = self.sci.get_mut(port) {
            s.receive_byte(data); // sh_sci.cpp:137
        }
    }

    /// 31250 bps bit-bang feed — mu2000.cpp:1397/1407/1409 drives start,
    /// data (LSB first), stop; mu2000.cpp:1141-1142 idles the line high at
    /// startup. Host MUST set `sci[port].cpu_now` before edges whose
    /// falling transition starts the RX clock (do_rx_w -> clock_start), to
    /// the DISK loop-top value `current_cycles()` == `total-1` between
    /// instructions (sh7042.h:92-98), NOT the raw loop clock.
    pub fn do_rx_w(&mut self, port: usize, state: i32) {
        if let Some(s) = self.sci.get_mut(port) {
            s.do_rx_w(state); // sh_sci.cpp:375
        }
    }
}

/// The bus seam: sh7042.rs dispatches the sci case-sets here with
/// ABSOLUTE addresses (membus.h:36 convention).
impl Sh7042Peripherals for Sh2SciPair {
    /// origin: src/mame/cpu/sh7042_map.hxx:15-26 (internal_r8 sci arms)
    fn sci_r8(&mut self, sci: usize, a: u32) -> u8 {
        let off = match a.wrapping_sub(if sci == 0 { 0xffff_81a0 } else { 0xffff_81b0 }) {
            o @ 0..=5 => o,
            _ => return 0,
        };
        let s = match self.sci.get(sci) {
            Some(s) => s,
            None => return 0,
        };
        match off {
            0 => s.smr_r(), // map:15/21
            1 => s.brr_r(), // map:16/22
            2 => s.scr_r(), // map:17/23
            3 => s.tdr_r(), // map:18/24
            4 => s.ssr_r(), // map:19/25
            _ => s.rdr_r(), // map:20/26
        }
    }

    /// origin: src/mame/cpu/sh7042_map.hxx:328-333 (internal_r16; big-endian
    /// byte pairs — low reg = high byte)
    fn sci_r16(&mut self, sci: usize, a: u32) -> u16 {
        let off = match a.wrapping_sub(if sci == 0 { 0xffff_81a0 } else { 0xffff_81b0 }) {
            o @ (0 | 2 | 4) => o,
            _ => return 0,
        };
        let s = match self.sci.get(sci) {
            Some(s) => s,
            None => return 0,
        };
        match off {
            0 => (u16::from(s.smr_r()) << 8) | u16::from(s.brr_r()), // map:328/331
            2 => (u16::from(s.scr_r()) << 8) | u16::from(s.tdr_r()), // map:329/332
            _ => (u16::from(s.ssr_r()) << 8) | u16::from(s.rdr_r()), // map:330/333
        }
    }

    /// origin: src/mame/cpu/sh7042_map.hxx:520-531 (internal_w8; NO
    /// 81a5/81b5 — RDR is read-only on disk; dropped again here
    /// defensively for direct seam callers)
    fn sci_w8(&mut self, sci: usize, a: u32, v: u8) {
        let off = match a.wrapping_sub(if sci == 0 { 0xffff_81a0 } else { 0xffff_81b0 }) {
            o @ 0..=4 => o,
            _ => return,
        };
        let s = match self.sci.get_mut(sci) {
            Some(s) => s,
            None => return,
        };
        match off {
            0 => s.smr_w(v), // map:520/525
            1 => s.brr_w(v), // map:521/526
            2 => s.scr_w(v), // map:522/527
            3 => s.tdr_w(v), // map:523/528
            _ => s.ssr_w(v), // map:524/529
        }
    }

    /// origin: src/mame/cpu/sh7042_map.hxx:796-801 (internal_w16 — NOTE
    /// case 81a4 writes ONLY ssr: `m_sci[0]->ssr_w(u8(v >> 8)); return;`
    /// the low byte (rdr slot) is dropped; 81a0/81a2 are ordered
    /// high-then-low register writes)
    fn sci_w16(&mut self, sci: usize, a: u32, v: u16) {
        let off = match a.wrapping_sub(if sci == 0 { 0xffff_81a0 } else { 0xffff_81b0 }) {
            o @ (0 | 2 | 4) => o,
            _ => return,
        };
        let hi = (v >> 8) as u8;
        let lo = v as u8;
        let s = match self.sci.get_mut(sci) {
            Some(s) => s,
            None => return,
        };
        match off {
            0 => {
                s.smr_w(hi); // map:796/799
                s.brr_w(lo);
            }
            2 => {
                s.scr_w(hi); // map:797/800
                s.tdr_w(lo);
            }
            _ => s.ssr_w(hi), // map:798/801 (low byte ignored)
        }
    }

    /// origin: src/mame/cpu/sh7042.cpp:291-292 — scheduler seam; the
    /// sh7042.rs loop (`internal_update_at`) owns add_event/recompute and
    /// replaces the in-device `m_cpu->internal_update()` recursion (see
    /// module doc).
    fn sci_update(&mut self, sci: usize, current_time: u64) -> u64 {
        self.sci
            .get_mut(sci)
            .map_or(0, |s| s.internal_update(current_time))
    }
}
