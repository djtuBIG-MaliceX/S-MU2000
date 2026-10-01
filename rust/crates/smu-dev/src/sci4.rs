//! Yamaha SCI4 / XV833A00 — 7-line serial, 4 channels multiplexed on chan 3
//! (origin: src/mame/machine/sci4.cpp, sci4.h:1-83). PLG board serial.
//!
//! Boot-critical (AGENTS.md pitfall + ledger row `sci4`): the firmware waits
//! on this IRQ or the main loop never starts. The disk asserts the IRQ line as
//! a LEVEL (held until serviced), see `tx_enabled` (sci4.cpp:277-285) and the
//! clear paths in `fifo_w`/`fifo_r`/`reset_r`/`enable_w`.
//!
//! Timers: the disk uses the compat emu_timer queue, NOT cycle math —
//! `timer_alloc` ×8 in device_start (sci4.cpp:76-79, birth order
//! tx0,rx0,tx1,rx1,... which is the compat `run_timers` tie-break order,
//! timers.rs:273) and `adjust` inside `wait` (:313-318). `clock()` here is the
//! DEVICE clock 8 MHz (sci4.h:21 default; mu2000.cpp:78 constructs without a
//! clock arg), while the machine runs at 28 MHz (mu2000.cpp:81), so every
//! delay lands as `u64((n as f64 / 8e6) * 28e6)` machine cycles — the double
//! truncation of mamecompat.h:736, reproduced verbatim by
//! `RunningMachine::timer_adjust` (timers.rs:227-235). `Attotime::NEVER`
//! mirrors `attotime::never` (mamecompat.h:512,534).
//!
//! The disk timer closure captures the raw `self` pointer (mamecompat.h:727).
//! Rust mirror: `device_start` registers `Rc<RefCell<Core>>` clones as
//! `RunningMachine::make_timer` callbacks (timers.rs:213-223) — same shared
//! pump, same fire order, machine clock still rides the event time inside the
//! callback (timers.rs:286) exactly like disk `run_timers` :694.
//!
//! devcb seams (sci4.h:39-40) become recorded line states the wiring row
//! polls: `Core::tx_line` (7 lines: 0-2 = chans 0-2, 3-6 = mux targets,
//! sci4.h:25) and `Core::irq_line` (4 lines). Disk wiring on the S-MU2000
//! machine (mu2000.cpp:1128-1132): irq0|irq1 ORed into CPU IRQ0 pin → INTC
//! vector 64; irq3 → IRQ1 pin → vector 66 (sh7042.cpp:141-143 →
//! intc.rs:206-232, external lines 0-7 = vectors 64-71); irq2 is unbound and
//! every tx line is unbound — no board is plugged (mu2000.cpp:77).
//!
//! PLG peer feed: disk feeds rx through `rx_w<30..32>` from the PLG1X0
//! connector `midi_tx` (ymmu2000.cpp:407-425). The S-MU2000 machine NEVER
//! calls it (mu2000.cpp wires no rx_w — board absent); the boot IRQ therefore
//! comes from the enable/tx path only. The seam is [`Sci4::rx_w`] with the
//! disk port numbering (0..2 direct, 30..33 → chan 3..6, sci4.h:23-24); the
//! wiring row must NOT invent board bytes.
//!
//! `state()` (sci4.cpp:363-374) is deferred to M5 row `state serializer`;
//! field order there is: sci4 tag, rx, enable, status, datamode, div, cur_rx,
//! tdr, tsr, tdr_full, tx_step, tx_active, rdr, rsr, rdr_full, rx_step,
//! rx_active, targets. `save_item` in device_start (sci4.cpp:57-74) is the
//! same list.
//!
//! Dropped vs disk: `logerror`/`chan_id` (sci4.cpp:125,130,150-152,287-294,
//! 304-306,347-349) — gated behind `g_verbose` on disk (mamecompat.h:113-117)
//! with zero state effect; the parity gates are pcm_sha1 / trace-pc, not
//! stderr. Also disk quirk: `m_rdr_full` is never assigned 1 anywhere in
//! sci4.cpp (only 0 at :97,:251; read at :352) — so the rx overrun branch
//! `status = 6` in rx_tick is dead unless a state load injects it. Kept
//! verbatim, never "fixed".

use std::cell::RefCell;
use std::rc::Rc;

use smu_compat::timers::{Attotime, RunningMachine, TimerId};

/// origin: sci4.h:15-79 `class sci4_device`. The Rc/RefCell is the Rust form
/// of the disk pointer capture in `timer_alloc` (mamecompat.h:723-728); all
/// fields explicit (ledger invariant 3) with the `device_reset` values
/// (sci4.cpp:84-101) so a constructed device equals a freshly reset one.
pub struct Sci4 {
    core: Rc<RefCell<Core>>,
}

/// Device state half of `sci4_device` (sci4.h:45-49). `pub` so the wiring row
/// and tests observe the devcb line records without a borrow dance.
pub struct Core {
    /// origin: device_t `m_clock` (mamecompat.h:320-324,352) — part of the
    /// disk device object, read by `wait` via `clock()` (:317). Disk default
    /// 8 MHz (sci4.h:21); mu2000.cpp:78 passes no override.
    pub clock: u32,
    /// origin: sci4.h:45 `std::array<u8, 7> m_rx` — raw rx line states.
    pub rx: [u8; 7],
    /// origin: sci4.h:46 `m_enable, m_status, m_datamode, m_div, m_cur_rx`.
    pub enable: [u8; 4],
    pub status: [u8; 4],
    pub datamode: [u8; 4],
    pub div: [u8; 4],
    pub cur_rx: [u8; 4],
    /// origin: sci4.h:47 `m_tdr, m_tsr, m_tdr_full, m_tx_step, m_tx_active`.
    pub tdr: [u8; 4],
    pub tsr: [u8; 4],
    pub tdr_full: [u8; 4],
    pub tx_step: [u8; 4],
    pub tx_active: [u8; 4],
    /// origin: sci4.h:48 `m_rdr, m_rsr, m_rdr_full, m_rx_step, m_rx_active`.
    pub rdr: [u8; 4],
    pub rsr: [u8; 4],
    pub rdr_full: [u8; 4],
    pub rx_step: [u8; 4],
    pub rx_active: [u8; 4],
    /// origin: sci4.h:49 `u8 m_targets = 0`.
    pub targets: u8,
    /// devcb seam for `m_tx` (sci4.h:39 `devcb_write_line::array<7>`): last
    /// state driven per line (0-2 direct, 3-6 mux targets, sci4.h:25).
    /// Idle-high init mirrors the rx idle reset value (sci4.cpp:84); disk
    /// devcb itself holds no state and is UNBOUND on the S-MU2000 machine
    /// (mu2000.cpp wires no sci4 tx).
    pub tx_line: [i8; 7],
    /// devcb seam for `m_irq` (sci4.h:40 `devcb_write_line::array<4>`): last
    /// state per line. Init 0 matches mu2000.h:898 `m_sci_irq[2] = {0, 0}`.
    /// Wiring (mu2000.cpp:1130-1132): line0|line1 → CPU IRQ0 pin (INTC
    /// vector 64), line3 → IRQ1 pin (vector 66), line2 unbound.
    pub irq_line: [i8; 4],
    /// origin: sci4.h:42-43 `emu_timer *m_tx_timer[4] / m_rx_timer[4]`.
    /// Placeholders until `device_start`; `device_start` MUST run before any
    /// write (disk would crash on a null emu_timer too).
    tx_ids: [TimerId; 4],
    rx_ids: [TimerId; 4],
}

impl Core {
    /// origin: sci4_device ctor empty body (sci4.cpp:8-13) + device_reset
    /// (:82-102) fused so no field is ever uninitialized (invariant 3).
    fn new_core(clock: u32) -> Core {
        Core {
            clock, // :8-13 (ctor; device_t m_clock)
            rx: [1; 7],          // :84 std::fill 1
            enable: [0; 4],      // :85
            status: [0; 4],      // :86
            datamode: [0; 4],    // :87
            div: [0; 4],         // :88
            cur_rx: [1; 4],      // :89
            tdr: [0; 4],         // :90
            tsr: [0; 4],         // :91
            tdr_full: [0; 4],    // :92
            tx_step: [0; 4],     // :93
            tx_active: [0; 4],   // :94
            rdr: [0; 4],         // :95
            rsr: [0; 4],         // :96
            rdr_full: [0; 4],    // :97
            rx_step: [0; 4],     // :98
            rx_active: [0; 4],   // :99
            targets: 0,          // :101
            tx_line: [1; 7],
            irq_line: [0; 4],
            tx_ids: [TimerId(0); 4],
            rx_ids: [TimerId(0); 4],
        }
    }

    /// origin: sci4.cpp:24-36 `read8`. Offsets ≥0x20 and holes 4/6/7 (<0x20)
    /// fall to `default_r` → 0 (:128-132, log dropped). Read of the reset
    /// register (offset&7==5) has the disk side effect (disk has no debugger;
    /// `side_effects_disabled` is false on disk, mamecompat.h:653).
    pub fn read8(&mut self, m: &mut RunningMachine, offset: u32) -> u8 {
        if offset < 0x20 {
            match offset & 7 {
                0 => return self.data_r(m, offset),
                1 => return self.enable_r(offset),
                2 => return self.status_r(offset),
                3 => return self.datamode_r(offset),
                5 => return self.reset_r(m, offset),
                _ => {}
            }
        }
        self.default_r(offset)
    }

    /// origin: sci4.cpp:38-52 `write8`. Note the disk quirk: writes to
    /// offset&7==2 (status) and ==5 (reset) are NOT register writes — they
    /// fall through to `default_w` (:51). Only 0x20, and <0x20 with
    /// &7 ∈ {0,1,3}, are decoded.
    pub fn write8(&mut self, m: &mut RunningMachine, offset: u32, data: u8) {
        if offset == 0x20 {
            self.target_w(m, data); // :40-42
            return;
        }
        if offset < 0x20 {
            match offset & 7 {
                0 => {
                    self.data_w(m, offset, data); // :46
                    return;
                }
                1 => {
                    self.enable_w(m, offset, data); // :47
                    return;
                }
                3 => {
                    self.datamode_w(offset, data); // :48
                    return;
                }
                _ => {}
            }
        }
        self.default_w(offset, data); // :51
    }

    /// origin: sci4.cpp:104-121 `do_rx_w` (the body of `rx_w<Sci>`,
    /// sci4.h:24). Port numbering 0..2 and 30..33 (sci4.h:23); ≥30 maps to
    /// chan 3..6 (:106-107) and only feeds the mux aggregate for chan 3
    /// (:116-120) — per-chan edge detect is chans 0-2 only (:110-115).
    pub fn do_rx_w(&mut self, m: &mut RunningMachine, mut sci: usize, state: i8) {
        if sci >= 30 {
            sci = sci - 30 + 3; // :106-107
        }
        self.rx[sci] = state as u8; // :109 (int → u8 line state)
        if sci < 3 {
            if state as u8 != self.cur_rx[sci] {
                // :111-114
                self.cur_rx[sci] = state as u8;
                self.rx_changed(m, sci);
            }
        }
        // :116 — mux aggregate: selected lines (targets>>4) all high ⇒ idle.
        // i32 mirrors C++ int promotion; mask order `~(targets >> 4)` verbatim.
        let rx: u8 = if ((((self.rx[6] as i32) << 3)
            | ((self.rx[5] as i32) << 2)
            | ((self.rx[4] as i32) << 1)
            | (self.rx[3] as i32))
            | !((self.targets as i32) >> 4))
            & 0xf
            == 0xf
        {
            1
        } else {
            0
        };
        if rx != self.cur_rx[3] {
            // :117-120
            self.cur_rx[3] = rx;
            self.rx_changed(m, 3);
        }
    }

    /// origin: sci4.cpp:123-126 `default_w` — write hole, log dropped.
    pub fn default_w(&mut self, _offset: u32, _data: u8) {}

    /// origin: sci4.cpp:128-132 `default_r` — read hole returns 0.
    pub fn default_r(&mut self, _offset: u32) -> u8 {
        0
    }

    /// origin: sci4.cpp:134-137 `datamode_w` (slot passed raw, >>3 here).
    pub fn datamode_w(&mut self, slot: u32, data: u8) {
        self.datamode[(slot >> 3) as usize] = data;
    }

    /// origin: sci4.cpp:139-142 `datamode_r`.
    pub fn datamode_r(&self, slot: u32) -> u8 {
        self.datamode[(slot >> 3) as usize]
    }

    /// origin: sci4.cpp:144-155 `data_w`. 0x80 = baud-divisor access mode,
    /// bit1 = FIFO write mode; anything else is ignored (no write hole log).
    pub fn data_w(&mut self, m: &mut RunningMachine, slot: u32, data: u8) {
        let slot = (slot >> 3) as usize; // :146
        if self.datamode[slot] == 0x80 {
            self.div[slot] = data; // :148 (baud log dropped :149-152)
        } else if self.datamode[slot] & 2 != 0 {
            self.fifo_w(m, slot, data); // :153-154
        }
    }

    /// origin: sci4.cpp:157-166 `data_r`. 0x80 → div; bit1 → rx FIFO read
    /// (side effect: clears status 4 / IRQ); else 0.
    pub fn data_r(&mut self, m: &mut RunningMachine, slot: u32) -> u8 {
        let sloti = (slot >> 3) as usize; // :159
        if self.datamode[sloti] == 0x80 {
            self.div[sloti] // :161
        } else if self.datamode[sloti] & 1 != 0 {
            self.fifo_r(sloti) // :163
        } else {
            0 // :165
        }
    }

    /// origin: sci4.cpp:168-182 `enable_w`. bit2 rising edge → `tx_enabled`
    /// (the boot IRQ assert, :277-285); bit2 falling edge with status==2
    /// deasserts the IRQ (level-clear path #1).
    pub fn enable_w(&mut self, m: &mut RunningMachine, slot: u32, data: u8) {
        let slot = (slot >> 3) as usize; // :170
        let old = self.enable[slot]; // :171
        self.enable[slot] = data; // :172
        if data & 2 != 0 && old & 2 == 0 {
            self.tx_enabled(m, slot); // :173-174
        } else if data & 2 == 0 && old & 2 != 0 {
            if self.status[slot] == 2 {
                // :176-181
                self.status[slot] = 0;
                self.irq(slot, 0);
            }
        }
        let _ = m;
    }

    /// origin: sci4.cpp:184-187 `enable_r`.
    pub fn enable_r(&self, slot: u32) -> u8 {
        self.enable[(slot >> 3) as usize]
    }

    /// origin: sci4.cpp:189-192 `status_r` — pure read, does NOT clear
    /// (level IRQ stays asserted until data read / reset / disable).
    pub fn status_r(&self, slot: u32) -> u8 {
        self.status[(slot >> 3) as usize]
    }

    /// origin: sci4.cpp:194-204 `reset_r` — REGISTER RESET, read-triggered
    /// (:32). Clears status + IRQ, cancels/re-idles the tx side. The `adjust`
    /// at :198 uses the default param (0) on disk — mirrored.
    pub fn reset_r(&mut self, m: &mut RunningMachine, slot: u32) -> u8 {
        let slot = (slot >> 3) as usize; // :196
        self.status[slot] = 0; // :197
        m.timer_adjust(self.tx_ids[slot], &Attotime::NEVER, 0); // :198
        self.tx_active[slot] = 0; // :199
        self.tx_set(slot, 1); // :200
        self.tdr_full[slot] = 0; // :201
        self.irq(slot, 0); // :202
        0 // :203
    }

    /// origin: sci4.cpp:207-218 `target_w` — mux select. Recomputes the chan3
    /// aggregate (:210-214) and forces unselected mux tx lines idle-high
    /// (:215-217). `>>4` is u8-shift, `~` promoted — see do_rx_w note.
    pub fn target_w(&mut self, m: &mut RunningMachine, data: u8) {
        self.targets = data; // :209
        let rx: u8 = if ((((self.rx[6] as i32) << 3)
            | ((self.rx[5] as i32) << 2)
            | ((self.rx[4] as i32) << 1)
            | (self.rx[3] as i32))
            | !((self.targets as i32) >> 4))
            & 0xf
            == 0xf
        {
            1
        } else {
            0
        };
        if rx != self.cur_rx[3] {
            // :211-214
            self.cur_rx[3] = rx;
            self.rx_changed(m, 3);
        }
        for i in 0..4 {
            // :215-217 (u32 i on disk)
            if self.targets & (1 << i) == 0 {
                self.tx_line[3 + i] = 1; // m_tx[i+3](1)
            }
        }
    }

    /// origin: sci4.cpp:220-229 `tx_set`. Chan <3 drives its own line; chan 3
    /// (mux) drives every selected target line together.
    pub fn tx_set(&mut self, chan: usize, state: i8) {
        if chan < 3 {
            self.tx_line[chan] = state; // :222-223
        } else {
            for i in 0..4 {
                // :225-227
                if self.targets & (1 << i) != 0 {
                    self.tx_line[3 + i] = state;
                }
            }
        }
    }

    /// origin: sci4.cpp:231-247 `fifo_w` — tx byte. Overrun (full + enable
    /// bit4... bit2 IRQ-enable mask is actually `(m_enable & 4)`, disk) sets
    /// status=6 + IRQ; accept clears a pending status-2 IRQ (level-clear path
    /// #2) and kicks `tx_start` when idle.
    pub fn fifo_w(&mut self, m: &mut RunningMachine, chan: usize, data: u8) {
        if self.tdr_full[chan] != 0 && (self.enable[chan] & 4) != 0 {
            self.status[chan] = 6; // :234
            self.irq(chan, 1); // :235
        } else {
            self.tdr[chan] = data; // :238
            self.tdr_full[chan] = 1; // :239
            if self.status[chan] == 2 {
                // :240-243
                self.status[chan] = 0;
                self.irq(chan, 0);
            }
            if self.tx_active[chan] == 0 && (self.enable[chan] & 2) != 0 {
                self.tx_start(m, chan); // :244-245
            }
        }
    }

    /// origin: sci4.cpp:249-257 `fifo_r` — rx byte read. Clears rdr_full and
    /// a pending status-4 IRQ (level-clear path #3); returns the latched byte.
    pub fn fifo_r(&mut self, chan: usize) -> u8 {
        self.rdr_full[chan] = 0; // :251
        if self.status[chan] == 4 {
            // :252-255
            self.status[chan] = 0;
            self.irq(chan, 0);
        }
        self.rdr[chan] // :256
    }

    /// origin: sci4.cpp:259-275 `rx_changed`. Falling start-bit edge arms the
    /// receiver at a HALF period (wait full=0 → div*8); a line return-to-high
    /// before the sample point (step==0) aborts (:267-270, `adjust(never)`
    /// with disk default param 0), otherwise force a half-period resync
    /// (:273).
    pub fn rx_changed(&mut self, m: &mut RunningMachine, chan: usize) {
        if self.rx_active[chan] == 0 && self.cur_rx[chan] == 0 && (self.enable[chan] & 1) != 0 {
            self.rx_active[chan] = 1; // :261-262
            self.rx_step[chan] = 0; // :263
            self.rsr[chan] = 0; // :264
            self.wait(m, 1, 0, chan); // :265
        } else if self.rx_active[chan] != 0 {
            if self.rx_step[chan] == 0 {
                self.rx_active[chan] = 0; // :269
                m.timer_adjust(self.rx_ids[chan], &Attotime::NEVER, 0); // :270
            } else {
                self.wait(m, 1, 0, chan); // :273
            }
        }
    }

    /// origin: sci4.cpp:277-285 `tx_enabled`. BOOT-CRITICAL ASSERT: enabling
    /// tx (enable bit2 ↑) with an empty data register immediately
    /// `status |= 2` and asserts the IRQ line — no peer byte required
    /// (mu2000.cpp:77 documents the board is absent; this is the IRQ the
    /// firmware gets at boot).
    pub fn tx_enabled(&mut self, m: &mut RunningMachine, chan: usize) {
        if self.tdr_full[chan] != 0 {
            self.tx_start(m, chan); // :279-280
        } else {
            self.status[chan] |= 2; // :282
            self.irq(chan, 1); // :283
        }
    }

    /// origin: sci4.cpp:296-311 `tx_start`. Loads tsr, keeps status bit1 high
    /// (IRQ stays/re-asserts), drives the start bit low, schedules the first
    /// bit tick.
    pub fn tx_start(&mut self, m: &mut RunningMachine, chan: usize) {
        self.tx_active[chan] = 1; // :298
        self.tsr[chan] = self.tdr[chan]; // :299
        self.tdr_full[chan] = 0; // :300
        self.status[chan] |= 2; // :301
        self.irq(chan, 1); // :302 (log dropped :304-306)
        self.tx_set(chan, 0); // :308 start bit low
        self.tx_step[chan] = 0; // :309
        self.wait(m, 0, 1, chan); // :310
    }

    /// origin: sci4.cpp:313-318 `wait`. div 0 ⇒ 0x100 (wrapped to 0 as u8 on
    /// disk? no — `u32 div = m_div ? m_div : 0x100`, u8 widened, :315).
    /// cycles = div × 16 (full) or div × 8 (half, rx resync), timed at the
    /// DEVICE clock (8 MHz, :317 `clock()`) then truncated by the machine's
    /// `adjust` into 28 MHz cycles (mamecompat.h:730-737).
    pub fn wait(&mut self, m: &mut RunningMachine, timer: usize, full: usize, chan: usize) {
        let d = self.div[chan];
        let div: u32 = if d != 0 { d as u32 } else { 0x100 }; // :315
        let cycles: u32 = div * if full != 0 { 16 } else { 8 }; // :316
        let when = Attotime::from_ticks(cycles as u64, self.clock); // :317
        if timer != 0 {
            m.timer_adjust(self.rx_ids[chan], &when, chan as i32); // :317
        } else {
            m.timer_adjust(self.tx_ids[chan], &when, chan as i32); // :317
        }
    }

    /// origin: sci4.cpp:320-333 `tx_tick`. Post-increment step retained even
    /// after the frame ends (:322). Frames: start bit driven low by
    /// `tx_start`, ticks 0-7 = data bits LSB-first, tick 8 = stop (high),
    /// tick 9 = finish (auto-continue if enabled + full, :328-329).
    pub fn tx_tick(&mut self, m: &mut RunningMachine, param: i32) {
        let p = param as usize;
        let step = self.tx_step[p]; // :322 (post-inc mirrors C++ `u32 step = m_tx_step[param]++`)
        self.tx_step[p] = self.tx_step[p].wrapping_add(1);
        if step < 9 {
            // :323-325
            let state = if step == 8 { 1 } else { ((self.tsr[p] >> step) & 1) as i8 };
            self.tx_set(p, state);
            self.wait(m, 0, 1, p);
        } else {
            // :327-332
            if (self.enable[p] & 2) != 0 && self.tdr_full[p] != 0 {
                self.tx_start(m, p);
            } else {
                self.tx_active[p] = 0;
            }
        }
    }

    /// origin: sci4.cpp:335-359 `rx_tick`. Tick 0 (half period after the
    /// start edge) re-arms at full period; ticks 1-8 sample cur_rx into rsr
    /// (LSB first); tick 9 = stop: framing error (line low) only logs and
    /// LEAVES rx_active set with no timer armed (disk quirk, :346-347 — the
    /// next `rx_changed` then takes the "force a precise resync" branch),
    /// else latch byte, status 4 (or dead-quirk 6, see module header), and
    /// ASSERT the IRQ (level — cleared later by `fifo_r`/`reset_r`/disable).
    pub fn rx_tick(&mut self, m: &mut RunningMachine, param: i32) {
        let p = param as usize;
        let step = self.rx_step[p]; // :337 post-inc, kept
        self.rx_step[p] = self.rx_step[p].wrapping_add(1);
        if step == 0 {
            self.wait(m, 1, 1, p); // :338-339
        } else if step < 9 {
            // :340-343
            if self.cur_rx[p] != 0 {
                self.rsr[p] |= 1 << (step - 1);
            }
            self.wait(m, 1, 1, p);
        } else {
            if self.rx[p] == 0 {
                // :346-347 framing error/break — log dropped, rx_active stays 1
            } else {
                // :348-357
                self.rx_active[p] = 0;
                self.rdr[p] = self.rsr[p];
                if self.rdr_full[p] != 0 && (self.enable[p] & 4) != 0 {
                    self.status[p] = 6; // :353 (dead: rdr_full never set, see header)
                } else {
                    self.status[p] = 4; // :355
                }
                self.irq(p, 1); // :356
            }
        }
    }

    /// devcb call seam for `m_irq[chan](state)` (sci4.h:40): record the level
    /// (disk devcb is stateless; the S-MU2000 wiring ORs 0/1→IRQ0 and
    /// routes 3→IRQ1 and re-issues `execute_set_input` per change,
    /// mu2000.cpp:1128-1132 — duplicate calls early-out in intc.rs:213-214).
    fn irq(&mut self, chan: usize, state: i8) {
        self.irq_line[chan] = state;
    }
}

impl Sci4 {
    /// origin: sci4.cpp:8-13 ctor. `clock` default at the call site is
    /// 8_000_000 (sci4.h:21; mu2000.cpp:78 uses the default). Every device
    /// field is explicitly initialized here with its device_reset value
    /// (invariant 3).
    pub fn new(clock: u32) -> Sci4 {
        Sci4 { core: Rc::new(RefCell::new(Core::new_core(clock))) }
    }

    /// devcb/observability handle for the wiring row and tests.
    pub fn core(&self) -> &Rc<RefCell<Core>> {
        &self.core
    }

    /// Device clock (mamecompat.h:324 `device_t::clock`).
    pub fn clock(&self) -> u32 {
        self.core.borrow().clock
    }

    /// origin: sci4.cpp:55-80 `device_start`. `save_item` :57-74 → M5 row
    /// `state serializer` (field list in the module header). The eight
    /// `timer_alloc` calls (:76-79) become `make_timer` registrations on the
    /// SHARED compat queue in the disk birth order tx0,rx0,tx1,rx1,...
    /// (compat tie-break scans birth order, timers.rs:273). The closure's
    /// `Rc` clone is the pointer-capture mirror of mamecompat.h:727.
    pub fn device_start(&mut self, m: &mut RunningMachine) {
        let rc = Rc::clone(&self.core);
        let mut c = rc.borrow_mut();
        for i in 0..4 {
            let a = Rc::clone(&rc);
            c.tx_ids[i] = m.make_timer(move |mm, p| a.borrow_mut().tx_tick(mm, p)); // :77
            let b = Rc::clone(&rc);
            c.rx_ids[i] = m.make_timer(move |mm, p| b.borrow_mut().rx_tick(mm, p)); // :78
        }
    }

    /// origin: sci4.cpp:82-102 `device_reset` — re-run of the explicit init.
    pub fn device_reset(&mut self) {
        let mut c = self.core.borrow_mut();
        // Exactly the fills at :84-101; the emu_timers (and their handles)
        // survive reset on disk — timers are device_start-owned objects, the
        // disk reset never touches m_tx_timer/m_rx_timer.
        let (tx_ids, rx_ids, clock) = (c.tx_ids, c.rx_ids, c.clock);
        *c = Core::new_core(clock);
        c.tx_ids = tx_ids;
        c.rx_ids = rx_ids;
    }

    /// origin: mu2000.cpp:969 (bus r8 at 0xf00000-0xf0003f, `a - 0xf00000`).
    pub fn read8(&mut self, m: &mut RunningMachine, offset: u32) -> u8 {
        self.core.borrow_mut().read8(m, offset)
    }

    /// origin: mu2000.cpp:970 (bus w8 at 0xf00000-0xf0003f, `a - 0xf00000`).
    pub fn write8(&mut self, m: &mut RunningMachine, offset: u32, data: u8) {
        self.core.borrow_mut().write8(m, offset, data)
    }

    /// origin: sci4.h:24 `rx_w<Sci>` → sci4.cpp:104-121. THE PLG peer seam:
    /// ymmu2000.cpp:407-425 wires PLG1X0 connector `midi_tx` → `rx_w<30/31/32>`
    /// (direct lines 3-6 via the ≥30 remap). The S-MU2000 machine calls this
    /// NOTHING (mu2000.cpp:77 "board not plugged") — the wiring row feeds it
    /// only if/when a PLG emulation row exists. Boot must not depend on it:
    /// the boot IRQ asserts from `enable_w` bit2 ↑ → `tx_enabled`
    /// (sci4.cpp:168-174 → :277-285) and clears via data read / disable /
    /// register reset (sci4.cpp:176-181, :240-243, :251-255, :194-204).
    pub fn rx_w(&mut self, m: &mut RunningMachine, sci: usize, state: i8) {
        self.core.borrow_mut().do_rx_w(m, sci, state)
    }
}
