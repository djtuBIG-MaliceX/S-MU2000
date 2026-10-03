//! Row `USB host (M37640)` (M7) — the full `usb_line` wire: F80000/F80001
//! register pair, the two IRQ lines, the 10,000 byte/s byte grid, the
//! host-online command queue and the TX ring with F5 port framing.
//!
//! Disk map (mu2000.h:211-225 "USB の代役（M37640）"): the real MIDI IN
//! C・D sit on the USB-side MCU; the SH-2 sees exactly **two addresses**
//! and **two IRQ lines** (mu2000.h:213-214).
//!   - read  0xF80000 = received byte, 0xF80001 = status
//!     (bit0 = 受信あり, bit6 = command — mu2000.cpp:1006-1007/:1363)
//!   - write 0xF80000 = MIDI out,  0xF80001 = command to the M37640 (dropped)
//!   - IRQ2 (vector 66) = "TX room, hand me a byte" (:1351-1355)
//!   - IRQ3 (vector 67) = 受信あり, HELD HIGH until 0xF80000 is read
//!     (:1342-1349 — "読まれるまで上げておく": the one-shot form lost
//!     bytes while firmware parked IRQ3 at priority 0; testxg.mid)
//!   - `set_usb_host(true)` BEFORE reset (mu2000.h:218-220): HOST SELECT
//!     is only USB after the ADC4 AN4 pin says so (:1151 0x330) and the
//!     reset-time F4 03 01 01 01 host-online push fired (:1086-1099);
//!     with it ON, A・B route through USB too (DIN goes silent on hardware).
//!
//! Ownership: this line lives behind one `Rc<RefCell<UsbLine>>` shared by
//! `Machine::midi.usb` (pump/inject) and `Hub::usb` (bus decode arm
//! sh7042.rs `Dev::Usb`). The `execute_set_input` seam is the [`UsbIrq`]
//! trait: the machine adapter queues `Evt::SetInput` for `Machine::pump`
//! (sh7042.cpp:141-143 -> intc.rs:223 — the same post-instruction
//! `m_test_irq` boundary as disk; module-doc deviation note in lib.rs).
//!
//! The RECEIVER half (`usb_midi_in` F5 framing, mu2000.cpp:1302-1315) is
//! ported+paired in the `midi lines` row and stays in `midi.rs`; the
//! drop counter, `midi_pending`/`midi_idle` accounting and the state
//! stream legs (mu2000.cpp:3573-3605, incl. v10 cmd/cur_cmd) are unchanged
//! by this row. `tx`/`out_port` do NOT ride the state stream (disk
//! :3575-3604 omits them — same as the MIDI OUT ring, mu2000.h:231).

use std::collections::VecDeque;

/// origin: mu2000.cpp:43 `constexpr u64 USB_BYTE_CYCLES = 28000000 / 10000`
/// — 実機で測った USB の受けの速さ 10,000 byte/s (:1327-1330, 3x DIN).
pub const USB_BYTE_CYCLES: u64 = 28_000_000 / 10_000;

/// origin: mu2000.h:1027 `static constexpr size_t TX_SIZE = 4096` — the
/// disk constant is SHARED between the DIN MIDI OUT ring (midi.rs) and
/// the usb_w TX cap (:1376 `u.tx.size() < TX_SIZE`).
pub const USB_TX_LIMIT: usize = 4096;

/// The `m_cpu->execute_set_input(2/3, ..)` seam of `usb_step`/`usb_r`
/// (mu2000.cpp:1341/:1349/:1355/:1367 -> sh7042.cpp:141-143 ->
/// m_intc->set_input). Disk states are ASSERT_LINE(1)/CLEAR_LINE(0);
/// line 2 = IRQ2 (vec 66, TX), line 3 = IRQ3 (vec 67, RX).
pub trait UsbIrq {
    fn set_input(&mut self, line: i32, state: i8);
}

/// origin: mu2000.h:1002-1013 `struct usb_line` — the COMPLETE line.
/// (N2's `UsbIn` receiver-half stub + M5-W4's state legs grow `tx` and
/// `out_port` here; ctor values are disk's initializers verbatim, every
/// field explicit — Invariant 3.)
pub struct UsbLine {
    /// :1003 `std::deque<u8> rx` — F5-framed MIDI byte stream to firmware
    pub rx: VecDeque<u8>,
    /// :1004 `int in_port = -1` — last port queued (F5 insert decision)
    pub in_port: i32,
    /// :1005 `u64 next = 0` — next byte may be handed over at this time
    pub next: u64,
    /// :1006 `bool have = false` — handed over, firmware has not read it
    pub have: bool,
    /// :1007 `u8 cur = 0` — the byte in flight
    pub cur: u8,
    /// :1008 `std::deque<u8> cmd` — M37640 commands, flagged via status bit6
    pub cmd: VecDeque<u8>,
    /// :1009 `bool cur_cmd = false` — the byte in flight is a command
    pub cur_cmd: bool,
    /// :1010 `u64 tx_next = 0` — TX-side IRQ2 grid
    pub tx_next: u64,
    /// :1011 `std::deque<u8> tx` — firmware's MIDI bytes (F5 framing kept)
    pub tx: VecDeque<u8>,
    /// :1012 `int out_port = -1` — the port the take side is seeing
    pub out_port: i32,
}

impl UsbLine {
    /// every field explicit = mu2000.h:1003-1012 initializers
    pub fn new() -> UsbLine {
        UsbLine {
            rx: VecDeque::new(),      // :1003
            in_port: -1,              // :1004
            next: 0,                  // :1005
            have: false,              // :1006
            cur: 0,                   // :1007
            cmd: VecDeque::new(),     // :1008
            cur_cmd: false,           // :1009
            tx_next: 0,               // :1010
            tx: VecDeque::new(),      // :1011
            out_port: -1,             // :1012
        }
    }

    /// origin: mu2000.h:223 `usb_idle()`
    pub fn idle(&self) -> bool {
        self.rx.is_empty() && self.cmd.is_empty() && !self.have // :223
    }

    /// origin: mu2000.cpp:1317-1357 `usb_step`. Called at the run_cycles
    /// loop head (mu2000.cpp:1227) with the same `now` as `midi_step`.
    /// Disk has NO chunk clamp for usb (:1234-1240 is DIN-only), so a
    /// byte whose grid point falls mid-chunk is handed over at the NEXT
    /// loop head — the Rust per-instruction loop returns to the head on
    /// the same chunk boundaries, so the cadence is identical.
    pub fn step(&mut self, now: u64, host: bool, fast: bool, irq: &mut dyn UsbIrq) {
        // USB を使っていないときは何もしない。割り込みを上げると firmware の
        // USB ドライバが動き出してしまう (:1321-1324) — the guard is what
        // keeps OFF-mode runs bit-identical: untouched IRQ lines.
        if !host && self.rx.is_empty() && !self.have {
            return; // :1323
        }

        // 受信。1 バイト渡すごとに IRQ3（ベクタ 67）を上げる (:1326-1330)
        if !self.have && now >= self.next && (!self.cmd.is_empty() || !self.rx.is_empty()) {
            // :1331
            let cmd_first = !self.cmd.is_empty(); // :1333 q = cmd.empty() ? rx : cmd
            self.cur_cmd = cmd_first; // :1334
            let q = if cmd_first { &mut self.cmd } else { &mut self.rx }; // :1333
            self.cur = *q.front().unwrap(); // :1335
            self.have = true; // :1336
            q.pop_front(); // :1337
            self.next = now + if fast { 0 } else { USB_BYTE_CYCLES }; // :1338
        }
        // 送信の線を一度下ろす。下で上げ直すので、山は 1 標本ぶんになる (:1340-1341)
        irq.set_input(2, 0); // :1341
                             // **読まれるまで上げておく** — level-hold 受信あり (:1342-1349;
                             // the historical one-shot form starved testxg.mid)
        if self.have {
            irq.set_input(3, 1); // :1348-1349
        }

        // 送信。firmware は IRQ2（ベクタ 66）が来るたびに 1 バイト出す。
        // 上げないとリングが埋まり、0x437A0 の空き待ちで固まる (:1351-1352)
        if now >= self.tx_next {
            // :1353
            self.tx_next = now + USB_BYTE_CYCLES; // :1354
            irq.set_input(2, 1); // :1355
        }
    }

    /// origin: mu2000.cpp:1359-1369 `usb_r`. `sel = a - 0xf80000` AFTER
    /// the bus demotion (sh7042.rs Dev::Usb fires the r8 handler twice for
    /// a word and the word pair for a long, so `sel` can exceed 1 — disk
    /// `usb_r` only tests `a & 1`, never a bound; sel 2 behaves like 0,
    /// sel 3 like 1 — membus.h:75/:81/:96 demotion, faithful).
    pub fn r(&mut self, sel: u32, irq: &mut dyn UsbIrq) -> u8 {
        if sel & 1 != 0 {
            // bit0 = 受信あり、bit6 = コマンド (:1362-1363)
            return if self.have {
                if self.cur_cmd {
                    0x41 // :1363
                } else {
                    0x01
                }
            } else {
                0x00
            };
        }
        // 受け取られたのでその場で線を下ろす (:1364-1367 — dropping one
        // sample later re-enters the same byte twice)
        self.have = false; // :1366
        irq.set_input(3, 0); // :1367
        self.cur // :1368
    }

    /// origin: mu2000.cpp:1371-1378 `usb_w`. Odd sel = the command port
    /// (M37640 への指示) — dropped. Even sel pushes firmware MIDI; the
    /// cap is the SHARED TX_SIZE, silent drop (no drop counter on disk).
    pub fn w(&mut self, sel: u32, v: u8) {
        if sel & 1 != 0 {
            return; // :1373-1374
        }
        if self.tx.len() < USB_TX_LIMIT {
            // :1376
            self.tx.push_back(v); // :1377
        }
    }

    /// origin: mu2000.cpp:1380-1400 `usb_out_take` — one firmware TX byte
    /// with its port tag (out-bytes carry port tag: the F5-framed stream
    /// is consumed here so the take side sees bytes per 1-based `F5 nn`
    /// switches, `port = nn - 1`, mu2000.h:224 "口は 0 始まり").
    /// A trailing F5 with no number is put BACK and reads empty (:1387-1390).
    pub fn out_take(&mut self) -> Option<(u8, i32)> {
        while let Some(&b) = self.tx.front() {
            // :1383-1384
            self.tx.pop_front(); // :1385
            if b == 0xf5 {
                // :1386
                if self.tx.is_empty() {
                    // 口の番号がまだ来ていない。戻しておく (:1387-1390)
                    self.tx.push_front(b); // :1388
                    return None; // :1389
                }
                self.out_port = *self.tx.front().unwrap() as i32 - 1; // :1391
                self.tx.pop_front(); // :1392
                continue; // :1393
            }
            return Some((b, self.out_port)); // :1395-1397
        }
        None // :1399
    }
}
