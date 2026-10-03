//! DIN MIDI wire: the 31250 bps bit machine + queue caps + F5 cable routing.
//!
//! origins (disk re-verified 2026-10-01, session N2):
//! - `src/mu2000.h:109-209` — port counts, `midi_in`, drop counter,
//!   `midi_queued` / `midi_pending` / `midi_idle` (+7 over N2-era cites,
//!   re-verified from disk 2026-10-03)
//! - `src/mu2000.h:981-1010` — `struct midi_line`, cable state arrays
//! - `src/mu2000.cpp:36-37` — `MIDI_BIT_CYCLES = 28000000 / 31250` (= 896)
//! - `src/mu2000.cpp:1402-1445` — `midi_step` (the per-loop bit pump;
//!   called from `run_cycles` mu2000.cpp:1226 — NOT from `run_sample`;
//!   there is no MIDI pump in the run_sample region). Re-cited 2026-10-03
//!   from disk: the region sits +32 over the N2-era numbers (the
//!   2026-10-02 merge shifted mu2000.cpp; tx_line at :1271-1294 is
//!   unchanged and re-confirms it)
//! - `src/mu2000.cpp:1061-1064` — cable reset inside `mu2000::reset`
//! - `src/mu2000.cpp:1302-1315` — `usb_midi_in` receiver half (F5 framing;
//!   disk re-verified 2026-10-03 — the N2-era cite :1270-1283 pre-dates the
//!   2026-10-02 merge +32 shift, same correction as the fast_midi row)
//! - `src/mu2000.h:227-246` + `src/mu2000.h:1024-1031` +
//!   `src/mu2000.cpp:1271-1294` — MIDI OUT: `midi_out_take`, the TX pin
//!   frame builder `tx_line` and the 4096-byte ring (`W-TX row`)
//! - `src/mu2000.cpp:1176-1179` — the MIDI OUT part of `mu2000::reset`
//!   (ring clear, then the `write_sci_tx<0>` bind consumed by
//!   `Machine::drain_sci_pins`, lib.rs)
//!
//! Timing math (mu2000.h:538-539, disk): 1 byte = 10 bits / 31250 baud
//! = 8960 CPU cycles @ 28 MHz = **exactly 14.112 samples** @ 44100
//! (8960 * 44100 = 395_136_000; / 28_000_000). mu2000.h:526's "14.1" is
//! that value rounded.
//!
//! Arms deliberately NOT ported here (documented skips):
//! - native engine `native_midi` (mu2000.h:139-140) — native engine is not
//!   built at all (AGENTS.md; lib.rs doc "strip native-engine").
//! - `usb_step` pump + registers + TX (mu2000.cpp:1317+) — ported in the
//!   M7 `USB host (M37640)` row as `smu_machine::usb` (UsbLine, the full
//!   mu2000.h:1002-1013 struct). This module keeps the paired N2 receiver
//!   half `usb_midi_in` (:1302-1315), the SHARED drop counter
//!   (mu2000.cpp:1306 uses the same `m_midi_dropped` as :161) and the
//!   `midi_pending`/`midi_idle` accounting; `usb` now lives behind one
//!   `Rc<RefCell<UsbLine>>` shared with the Hub bus arms and the
//!   run_cycles :1227 pump (single `m_usb`, disk mu2000.h:1021).
//! - fast_midi `set_fast_midi` (mu2000.h:120) is LIVE since the M7 row:
//!   render --fast-midi applies it at the render.cpp:377 seam and the fast
//!   arms below are REACHED (direct RDR inject mu2000.cpp:1408-1416 +
//!   the byte-in-flight accounting mu2000.h:184-207, disk re-verified
//!   2026-10-03 — the N2-era cites :1376-1384/:173-202 pre-date the
//!   2026-10-02 merge shift). Flag default OFF keeps every path below
//!   byte-identical to the paired M4 `midi lines` behavior.
//! - `logerror` byte traces (:1412-1413/:1427-1428) — the Rust machine strips
//!   logerror (wiring-row convention).
//!
//! Deviation (ledger-sanctioned pattern): `m_midi_dropped` is `std::atomic`
//! on disk (mu2000.h:991) because the C++ GUI thread calls `midi_in`. In
//! Rust, MIDI crosses threads as raw bytes only via the SPSC ring
//! (Invariant 6); `midi_in` runs on the audio thread alone, so a plain
//! `u64` is equivalent.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

/// origin: src/mu2000.h:107 (DIN = ports A/B on SCI ch0/ch1, parts 1-16/17-32)
pub const MIDI_DIN_PORTS: usize = 2;
/// origin: src/mu2000.h:108 (C/D are the USB M37640 side, parts 33-48/49-64)
pub const MIDI_PORTS: usize = 4;
/// origin: src/mu2000.h:119-125 — `size_t(1) << 22`; the issue #18 comment
/// (old 64KiW cap dropped note-offs under DAW wheel storms) rides along.
pub const MIDI_QUEUE_LIMIT: usize = 1usize << 22;
/// origin: src/mu2000.cpp:36-37 — `constexpr u64 MIDI_BIT_CYCLES = 28000000 / 31250`
pub const MIDI_BIT_CYCLES: u64 = 28_000_000 / 31_250; // = 896; mu2000.h:538: 10 bits = 8960 cyc = 14.112 smp
/// origin: src/mu2000.h:1027 — `TX_SIZE = 4096, TX_MASK = TX_SIZE - 1`
/// (mu2000.h:230: "溜めは 4096 バイトで、溢れたら捨てる")
pub const MIDI_TX_SIZE: usize = 4096;
/// origin: src/mu2000.h:1027 — `TX_MASK = TX_SIZE - 1`
pub const MIDI_TX_MASK: usize = MIDI_TX_SIZE - 1;

/// origin: src/mu2000.h:983-988 `struct midi_line`.
/// `bit`: -1 idle/waiting / 0 start / 1-8 data / 9 stop (disk comment :985).
pub struct MidiLine {
    pub queue: VecDeque<u8>, // :984 std::deque<u8>
    pub bit: i32,            // :985 = -1
    pub cur: u8,             // :986 = 0
    pub next: u64,           // :987 = 0
}

impl MidiLine {
    /// explicit init mirroring the disk in-class initializers (Invariant 3)
    pub fn new() -> MidiLine {
        MidiLine {
            queue: VecDeque::new(), // :984 empty deque
            bit: -1,                // :985
            cur: 0,                 // :986
            next: 0,                // :987
        }
    }
}

/// The full `usb_line` (mu2000.h:1002-1013) lives in `crate::usb` since
/// the M7 `USB host (M37640)` row — re-exported so the paired N2 receiver
/// half, the M5-W4 state stream (mu2000.cpp:3573-3605) and the bins keep
/// one type. N2-era note kept for provenance: `rx`/`in_port` + the two
/// fields `usb_idle()` reads (mu2000.h:223) were the RECEIVER HALF ONLY
/// until M7; `next`/`cur`/`cur_cmd`/`tx_next` joined at M5-W4 as
/// state-only legs (:3588/:3603); `tx`/`out_port` (disk :1011-1012) do NOT
/// ride the state stream (disk :3575-3604 omits them — like the MIDI OUT
/// ring, mu2000.h:231).
pub use crate::usb::UsbLine;

/// The SCI seam for the bit pump — mu2000.cpp:1407 `m_cpu->sci(port)`.
/// lib.rs implements it over `Sh2SciPair` (`sci.rs:329/338/353/592`);
/// tests record the edges instead. Host MUST sync `Sh2Sci::cpu_now` to the
/// loop-top `now` before `do_rx_w` (sci.rs:1080-1083 host note; disk reads
/// `current_cycles()` at sh_sci.cpp:475/484, and no CPU has run between
/// run_cycles :1170 and :1194 so that value IS `now`).
pub trait MidiSci {
    fn rx_can_accept(&self, port: usize) -> bool; // mu2000.cpp:1409 (fast arm)
    fn rx_byte_pending(&self, port: usize) -> bool; // mu2000.h:187/195/206 (fast arms)
    fn receive_byte(&mut self, port: usize, data: u8); // mu2000.cpp:1414 (fast inject)
    fn do_rx_w(&mut self, port: usize, state: i32); // mu2000.cpp:1429/1439/1441
}

/// The mu2000-side MIDI glue: 2 DIN lines + 4-port cable routing + USB in
/// + shared drop counter. Disk scatters this over mu2000.h inline methods
/// and mu2000.cpp; here it is one struct whose `sci` dependency is the
/// trait above.
pub struct Midi {
    pub lines: [MidiLine; MIDI_DIN_PORTS], // mu2000.h:990 m_midi
    pub dropped: u64, // :991 m_midi_dropped (plain u64 — see module doc)
    pub fast_midi: bool, // :992 m_fast_midi = false (M7 flag row stubs false)
    /// mu2000.h:1022 `m_usb_host`. LIVE since the M7 row: set through
    /// `Machine::set_usb_host` (mu2000.h:221) BEFORE reset; the Cell is
    /// SHARED with `Hub::usb_host` (the :1151 ADC4 lambda) — single truth.
    /// Default false == the paired DIN path (disk mu2000.cpp:1151 0).
    pub usb_host: Rc<Cell<bool>>,
    /// mu2000.h:1021 `m_usb` — the FULL usb_line behind one Rc (M7):
    /// receiver-half inject + pending/idle + state stream here; the Hub
    /// bus arms (mu2000.cpp:1011-1012) and the run_cycles :1227 pump share
    /// the same object.
    pub usb: Rc<RefCell<UsbLine>>, // :1021 m_usb
    /// mu2000.h:1009 `std::array<int, MIDI_PORTS> m_cable = { 0, 1, 2, 3 }`
    pub cable: [i32; MIDI_PORTS],
    /// mu2000.h:1010 `std::array<bool, MIDI_PORTS> m_cable_wait = {}`
    pub cable_wait: [bool; MIDI_PORTS],
    /// mu2000.h:1028 `u8 m_tx_buf[TX_SIZE] = {}` — MIDI OUT byte ring
    /// (SCI ch0 TX wire frame assembly; NOT saved in state, mu2000.h:231)
    pub tx_buf: [u8; MIDI_TX_SIZE],
    /// mu2000.h:1029 `size_t m_tx_r = 0` — take side
    pub tx_r: usize,
    /// mu2000.h:1029 `size_t m_tx_w = 0` — push side (tx_line stop bit)
    pub tx_w: usize,
    /// mu2000.h:1030 `int m_tx_bit = -1` — -1 waiting / 0-7 data / 8 stop.
    /// Ctor init (:1030) and reset (mu2000.cpp:1178) BOTH pin this to -1 —
    /// verified on disk, no ctor-vs-reset divergence for this field.
    pub tx_bit: i32,
    /// mu2000.h:1031 `u8 m_tx_cur = 0` — byte under construction
    pub tx_cur: u8,
}

impl Midi {
    /// every field explicit (Invariant 3)
    pub fn new() -> Midi {
        Midi {
            lines: [MidiLine::new(), MidiLine::new()], // mu2000.h:990
            dropped: 0,                                // :991 {0}
            fast_midi: false,                          // :992
            usb_host: Rc::new(Cell::new(false)),       // :1022 (shared Cell)
            usb: Rc::new(RefCell::new(UsbLine::new())), // :1021 (shared Rc)
            cable: [0, 1, 2, 3],                       // :1009
            cable_wait: [false; MIDI_PORTS],           // :1010 {}
            tx_buf: [0u8; MIDI_TX_SIZE],               // :1028 {}
            tx_r: 0,                                   // :1029
            tx_w: 0,                                   // :1029
            tx_bit: -1,                                // :1030 (= reset :1178)
            tx_cur: 0,                                 // :1031
        }
    }

    /// origin: src/mu2000.h:120 `set_fast_midi` (inline setter; M7 row).
    /// Bins call it through `Machine::set_fast_midi` (lib.rs) at the
    /// apply_engine_options seam (options.h:52; render.cpp:377 — BEFORE
    /// reset, so the flag is stable across boot). M4-era cite
    /// `mu2000.cpp:1376` was pre-merge drift (disk: mu2000.h:120).
    pub fn set_fast_midi(&mut self, fast: bool) {
        self.fast_midi = fast;
    }

    /// mu2000.h:1051-1064 (the MIDI part of `mu2000::reset`): cable
    /// routing set by F5 messages reverts on power cycle. Disk does NOT
    /// clear the line queues or `m_midi_dropped` here — queued bytes
    /// survive a reset.
    pub fn reset_cables(&mut self) {
        for p in 0..MIDI_PORTS {
            self.cable[p] = p as i32; // mu2000.cpp:1062
            self.cable_wait[p] = false; // :1063
        }
    }

    /// origin: src/mu2000.cpp:1271-1294 `tx_line` — assemble the MIDI OUT
    /// frame off the SCI ch0 TX wire. The SCI reports the line exactly once
    /// per bit (mu2000.h:1024-1025), so "0 starts, 8 bits LSB-first, 1 ends"
    /// reads without looking at time. Driven from `Machine::drain_sci_pins`
    /// (lib.rs) — the Rust shape of the disk `write_sci_tx<0>` bind
    /// (mu2000.cpp:1179). device_reset's born-high edge (sci.rs:322,
    /// sh_sci.cpp:367) lands here and no-ops at :1273-1278 (bit=-1, high).
    pub fn tx_line(&mut self, state: bool) {
        if self.tx_bit < 0 {
            // :1273-1278 — only a falling edge starts a byte; anything
            // else (incl. a mid-idle HIGH) returns untouched
            if !state {
                // スタートビット (:1274)
                self.tx_bit = 0; // :1275
                self.tx_cur = 0; // :1276
            }
            return; // :1278
        }
        if self.tx_bit < 8 {
            // :1280-1283 — LSB first; disk `u8((state?1:0) << m_tx_bit)`
            // shifts as int then truncates; bit<8 so the widths agree
            self.tx_cur |= ((state as u32) << self.tx_bit) as u8; // :1281
            self.tx_bit += 1; // :1282
            return; // :1283
        }
        // :1285-1288 — stop bit. 0 here means the framing slipped, so the
        // byte is dropped
        self.tx_bit = -1; // :1286
        if !state {
            return; // :1287
        }
        let next = (self.tx_w + 1) & MIDI_TX_MASK; // :1289
        if next == self.tx_r {
            return; // :1290-1291 — 溢れ。誰も読んでいない
        }
        self.tx_buf[self.tx_w] = self.tx_cur; // :1292
        self.tx_w = next; // :1293
    }

    /// origin: src/mu2000.h:232-246 `midi_out_take` — one firmware-emitted
    /// MIDI OUT byte, or None when the ring is empty (:241-242). Same-thread
    /// contract as `run_sample` (mu2000.h:230). Ring is NOT saved in state
    /// (mu2000.h:231 — a restore starts empty).
    pub fn midi_out_take(&mut self) -> Option<u8> {
        // :237-240 — LIVE since the M7 row: with USB active the firmware
        // answers on USB too ("firmware は返事も USB 側へ出す", mu2000.h:234-236
        // — DIN MIDI OUT goes silent), so the divert reads the usb tx ring.
        // Disk `usb_out_take(v, port)` (mu2000.cpp:1380-1400) discards the
        // port here (local `int port`), so the take side sees the bare
        // byte; the F5-framed port tag is consumed inside `out_take`
        // (`out_port` tracks the last `F5 nn`).
        if self.usb_host.get() {
            let (v, _port) = self.usb.borrow_mut().out_take()?;
            return Some(v);
        }
        if self.tx_r == self.tx_w {
            return None; // :241-242
        }
        let v = self.tx_buf[self.tx_r]; // :243
        self.tx_r = (self.tx_r + 1) & MIDI_TX_MASK; // :244
        Some(v) // :245
    }

    /// origin: src/mu2000.h:133-163 `midi_in`. Returns the port the byte
    /// was routed to, or -1 when an `F5 nn` cable message was consumed
    /// (disk :132 "戻り値はバイトを回した口。`F5 nn` を読んだときは -1").
    pub fn midi_in(&mut self, byte: u8, port: i32) -> i32 {
        // :135-136 port clamp
        let port: usize = if port < 0 || port >= MIDI_PORTS as i32 {
            0
        } else {
            port as usize
        };
        // :137-140 native arm — NOT BUILT (module doc). Disk would return
        // `port` here when native_midi consumed the byte; skipped.

        // :141-154 F5 cable routing. "リアルタイムは F5 と nn の間に挟まっても
        // よい" (:141) — but 0xF8..0xFF are NOT `byte < 0xf8`, so realtime
        // bytes SKIP this block: they are forwarded to the CURRENT cable port
        // while `cable_wait` STAYS armed for the nn that follows.
        if byte < 0xf8 {
            if self.cable_wait[port] {
                self.cable_wait[port] = false; // :143
                if byte & 0x80 == 0 {
                    // :144-148 in-range nn re-routes; out-of-range is read
                    // and discarded, port unchanged (:131 範囲外の nn は読み捨てて口を変えない)
                    if byte >= 1 && byte <= MIDI_PORTS as u8 {
                        self.cable[port] = (byte - 1) as i32; // :146
                    }
                    return -1; // :147
                }
                // high-bit byte after F5 : cancels the wait and falls
                // through to the queue with the OLD port (disk control flow)
            }
            if byte == 0xf5 {
                self.cable_wait[port] = true; // :151
                return -1; // :152 — the F5 itself never reaches firmware
                // (:129-131: real MU2000 ignores host F5; ours is consumed
                // here so the firmware USB receiver 0x042932 cannot misread it)
            }
        }
        let to = self.cable[port]; // :155
        if to >= MIDI_DIN_PORTS as i32 || self.usb_host.get() {
            self.usb_midi_in(byte, to); // :156-157 (host ON: A/B ride USB too,
                                        // mu2000.h:219-220 — DIN 実機で黙る)
        } else if self.lines[to as usize].queue.len() < MIDI_QUEUE_LIMIT {
            self.lines[to as usize].queue.push_back(byte); // :158-159
        } else {
            self.dropped += 1; // :161 (see module doc: non-atomic u64)
        }
        to // :162
    }

    /// origin: src/mu2000.cpp:1302-1315 `usb_midi_in` (receiver half;
    /// disk re-cited 2026-10-03 — N2-era :1270-1283 pre-merge). Cap
    /// overflow shares the DIN drop counter (:1306 == :168 counter).
    /// Port change inserts the `F5 <口>` frame the firmware receiver
    /// 0x042932 expects (:1298-1300; ports are 1-based on the wire).
    pub fn usb_midi_in(&mut self, byte: u8, port: i32) {
        let mut u = self.usb.borrow_mut();
        if u.rx.len() >= MIDI_QUEUE_LIMIT {
            self.dropped += 1; // :1306-1307
            return;
        }
        if port != u.in_port {
            u.rx.push_back(0xf5); // :1310
            u.rx.push_back((port + 1) as u8); // :1311 (ports are 1-based on the wire)
            u.in_port = port; // :1312
        }
        u.rx.push_back(byte); // :1314
    }

    /// origin: src/mu2000.cpp:1402-1445 `midi_step`. Called once per
    /// run_cycles loop iteration (mu2000.cpp:1226) — the Rust run_cycles
    /// reproduces the disk :1234-1240 chunk clamp verbatim (the same
    /// `if (!m_fast_midi)` gate at lib.rs), so OFF mode stops the CPU on
    /// the same instruction that crosses a bit edge, and fast mode opens
    /// the clamp exactly as disk does.
    pub fn midi_step(&mut self, now: u64, sci: &mut dyn MidiSci) {
        // A と B は別々の SCI に繋がっている。互いに待たせない (:1372)
        for port in 0..MIDI_DIN_PORTS {
            let m = &mut self.lines[port];
            if self.fast_midi {
                // :1408-1416 fast arm (disk re-verified 2026-10-03) — LIVE
                // since the M7 row: one byte per pump call straight into the
                // SCI, gated ONLY by rx_can_accept (RE on, RDRF clear, no
                // recv error — sh_sci.cpp:127-130). :1414 receive_byte is the
                // DIRECT inject: no wire time, no start/stop bits, no bit
                // machine (m.bit stays -1; the `next` grid never runs).
                if !m.queue.is_empty() && sci.rx_can_accept(port) {
                    let byte = m.queue.pop_front().unwrap(); // :1410-1411
                    sci.receive_byte(port, byte); // :1414
                }
                continue; // :1416 — the wire path is skipped entirely
            }

            if m.bit < 0 {
                // 直前のバイトのストップビットぶんは空けてから次を出す (:1420)
                if m.queue.is_empty() || now < m.next {
                    continue; // :1421-1422
                }
                m.cur = m.queue.pop_front().unwrap(); // :1423-1424
                m.bit = 0; // :1425
                m.next = now.wrapping_add(MIDI_BIT_CYCLES); // :1426
                // :1427-1428 logerror stripped (module doc)
                sci.do_rx_w(port, 0); // :1429 スタートビット
                continue; // :1430
            }

            if now < m.next {
                continue; // :1433-1434
            }

            m.bit += 1; // :1436
            m.next = now.wrapping_add(MIDI_BIT_CYCLES); // :1437
            if m.bit <= 8 {
                sci.do_rx_w(port, ((m.cur >> (m.bit - 1)) & 1) as i32); // :1439 下位ビットから
            } else {
                sci.do_rx_w(port, 1); // :1441 ストップビット
                m.bit = -1; // :1442
            }
        }
    }

    /// origin: src/mu2000.h:175-179 `midi_queued` — bytes on the wire incl.
    /// the in-flight one; the 31250bps throttle asks this. FAST-OBLIVIOUS on
    /// disk too (:177 counts `m.bit >= 0` unconditionally — with fast ON no
    /// bit is ever in flight, the byte-in-flight lives in the SCI RDR and is
    /// counted by `midi_pending` instead). NOTE the disk index
    /// `port == 1 ? 1 : 0` — every port except 1 sees line 0 (:177).
    pub fn midi_queued(&self, port: i32) -> usize {
        let m = &self.lines[if port == 1 { 1 } else { 0 }]; // :177
        m.queue.len() + if m.bit >= 0 { 1 } else { 0 } // :178
    }

    /// origin: src/mu2000.h:180-189 `midi_pending` (all lines + usb).
    /// SCI byte-in-flight rule: fast ON drops the wire-bit count (:184
    /// `!m_fast_midi` gate) and counts each DIN SCI's RDRF byte instead
    /// (:185-187) — the byte the firmware has not read yet.
    pub fn midi_pending(&self, sci: &dyn MidiSci) -> usize {
        let u = self.usb.borrow();
        let mut pending = u.rx.len() + if u.have { 1 } else { 0 }; // :182
        for m in self.lines.iter() {
            pending += m.queue.len() + ((!self.fast_midi && m.bit >= 0) as usize); // :183-184
        }
        if self.fast_midi {
            // :185-187 — LIVE since the M7 fast_midi row
            for port in 0..MIDI_DIN_PORTS {
                pending += sci.rx_byte_pending(port) as usize; // :187
            }
        }
        pending // :188
    }

    /// origin: src/mu2000.h:190-196 `midi_idle(port)`
    pub fn midi_idle(&self, port: i32, sci: &dyn MidiSci) -> bool {
        if port >= MIDI_DIN_PORTS as i32 || self.usb_host.get() {
            return self.usb.borrow().idle(); // :192-193 (usb_idle mu2000.h:223)
        }
        let m = &self.lines[port as usize];
        m.queue.is_empty()
            && if self.fast_midi {
                !sci.rx_byte_pending(port as usize) // :195 fast byte-in-flight (LIVE M7)
            } else {
                m.bit < 0 // :195 wire idle
            }
    }

    /// origin: src/mu2000.h:197-209 `midi_idle()` (all lines + usb)
    pub fn midi_idle_all(&self, sci: &dyn MidiSci) -> bool {
        if !self.usb.borrow().idle() {
            return false; // :199-200
        }
        for m in self.lines.iter() {
            if !m.queue.is_empty() || (!self.fast_midi && m.bit >= 0) {
                return false; // :201-203
            }
        }
        if self.fast_midi {
            // :204-207 — LIVE since the M7 fast_midi row
            for port in 0..MIDI_DIN_PORTS {
                if sci.rx_byte_pending(port) {
                    return false; // :206
                }
            }
        }
        true // :208
    }
}

#[cfg(test)]
mod tests {
    //! Disk-derived vectors only (mu2000.h/mu2000.cpp constants; the pump is
    //! pure integer arithmetic — no C++ harness needed for this row).
    use super::*;

    /// records every RX edge / fast inject in call order
    struct RecSci {
        log: Vec<(usize, i32)>, // (port, state) from do_rx_w
        inj: Vec<(usize, u8)>,  // receive_byte (fast arm only)
        accept: bool,
        pending: [bool; MIDI_DIN_PORTS],
    }

    impl RecSci {
        fn new() -> RecSci {
            RecSci {
                log: Vec::new(),
                inj: Vec::new(),
                accept: true,
                pending: [false; MIDI_DIN_PORTS],
            }
        }
    }

    impl MidiSci for RecSci {
        fn rx_can_accept(&self, _port: usize) -> bool {
            self.accept
        }
        fn rx_byte_pending(&self, port: usize) -> bool {
            self.pending[port]
        }
        fn receive_byte(&mut self, port: usize, data: u8) {
            self.inj.push((port, data));
        }
        fn do_rx_w(&mut self, port: usize, state: i32) {
            self.log.push((port, state));
        }
    }

    fn midi() -> Midi {
        Midi::new()
    }

    // ---- F5 routing legs (mu2000.h:141-153) ----

    #[test]
    fn f5_in_range_reroutes_and_returns_minus1() {
        let mut m = midi();
        assert_eq!(m.midi_in(0xf5, 0), -1); // :150-152 — F5 consumed, -1
        assert!(m.cable_wait[0] && !m.cable_wait[1]);
        assert_eq!(m.midi_in(0x02, 0), -1); // nn=2 -> cable[0]=1, -1 (:145-147)
        assert_eq!(m.cable, [1, 1, 2, 3]);
        assert!(!m.cable_wait[0]);
        // subsequent bytes ride line 1, return code = the port (:162)
        assert_eq!(m.midi_in(0x90, 0), 1);
        assert_eq!(m.lines[1].queue.back(), Some(&0x90));
        assert!(m.lines[0].queue.is_empty());
        // port 1 is untouched by port 0's cable
        assert_eq!(m.midi_in(0x40, 1), 1);
        assert_eq!(m.lines[1].queue.back(), Some(&0x40));
    }

    #[test]
    fn f5_out_of_range_reads_but_keeps_port() {
        let mut m = midi();
        for nn in [0x00u8, 0x05, 0x7f] {
            // :131 out-of-range nn: read, discarded, port unchanged
            assert_eq!(m.midi_in(0xf5, 0), -1);
            assert_eq!(m.midi_in(nn, 0), -1); // :147 (the if-body :145 was false)
            assert_eq!(m.cable[0], 0);
            assert!(!m.cable_wait[0]); // wait was CONSUMED (:143)
            assert_eq!(m.midi_in(0x90, 0), 0); // still line 0
            assert!(m.lines[0].queue.contains(&0x90));
        }
    }

    #[test]
    fn f5_then_highbit_byte_cancels_and_forwards_to_old_port() {
        // disk control flow: cable_wait clears at :143, the !(byte&0x80)
        // test :144 fails, byte!=0xf5, so the byte falls through to `to`
        let mut m = midi();
        assert_eq!(m.midi_in(0xf5, 0), -1);
        assert_eq!(m.midi_in(0xc0, 0), 0); // status byte -> old port 0, NOT -1
        assert_eq!(m.lines[0].queue.back(), Some(&0xc0));
        assert!(!m.cable_wait[0]);
        assert_eq!(m.midi_in(0x02, 0), 0); // nn now rides the wire normally
    }

    #[test]
    fn realtime_interleaves_between_f5_and_nn() {
        // mu2000.h:141 comment + :141 gate: 0xf8..0xff skip the cable block,
        // go to the CURRENT port, and the wait stays armed for nn.
        let mut m = midi();
        assert_eq!(m.midi_in(0xf5, 1), -1); // wait armed on port 1
        assert_eq!(m.midi_in(0xfa, 1), 1); // realtime -> line 1 (=old cable), wire flows
        assert!(m.cable_wait[1]); // still armed
        assert_eq!(m.midi_in(0xfe, 1), 1); // another realtime
        assert_eq!(m.midi_in(0x04, 1), -1); // the nn finally lands: cable[1]=3
        assert_eq!(m.cable[1], 3);
        assert_eq!(m.lines[1].queue.make_contiguous(), &[0xfa, 0xfe]); // both bytes kept order
        // port 3 target = USB (to >= MIDI_DIN_PORTS): next byte gets F5-framed
        assert_eq!(m.midi_in(0x90, 1), 3);
        assert_eq!(m.usb.borrow().rx.iter().copied().collect::<Vec<_>>().as_slice(), &[0xf5, 0x04, 0x90]); // :1277-1282
    }

    #[test]
    fn f5_to_usb_leg_frames_and_shares_drop_counter() {
        let mut m = midi();
        assert_eq!(m.midi_in(0xf5, 0), -1);
        assert_eq!(m.midi_in(0x03, 0), -1); // -> port 2 (USB C)
        assert_eq!(m.midi_in(0x90, 0), 2); // :156 to>=MIDI_DIN_PORTS
        assert_eq!(m.usb.borrow().rx.iter().copied().collect::<Vec<_>>().as_slice(), &[0xf5, 0x03, 0x90]); // :1278-1279 (:1279 = port+1)
        assert_eq!(m.midi_in(0x40, 0), 2); // same port -> NO extra F5 (:1277)
        assert_eq!(m.usb.borrow().rx.iter().copied().collect::<Vec<_>>().as_slice(), &[0xf5, 0x03, 0x90, 0x40]);
        // line queues untouched; DIN path still works on its own cable
        assert!(m.lines[0].queue.is_empty());
        // usb cap shares m_midi_dropped (:1274): fill to the limit, +1 drops
        m.usb.borrow_mut().rx.resize(MIDI_QUEUE_LIMIT, 0);
        assert_eq!(m.midi_in(0x41, 0), 2);
        assert_eq!(m.dropped, 1);
        assert_eq!(m.usb.borrow().rx.len(), MIDI_QUEUE_LIMIT); // rejected (:1275)
    }

    #[test]
    fn port_clamp_and_return_codes() {
        // mu2000.h:135-136 clamp
        let mut m = midi();
        assert_eq!(m.midi_in(0x9e, 7), 0); // >=4 -> port 0 -> cable[0]=0
        assert_eq!(m.midi_in(0x9e, -1), 0); // <0 -> port 0
        assert_eq!(m.midi_in(0xf5, 9), -1); // clamp happens BEFORE the wait
        assert!(m.cable_wait[0]);
        assert_eq!(m.midi_in(0x01, 9), -1); // nn=1 -> cable[0]=0 (identity)
        assert_eq!(m.midi_in(0x80, 3), 3); // port 3 identity cable, return = port
    }

    // ---- queue cap boundary (mu2000.h:158-161) ----

    #[test]
    fn din_queue_cap_boundary_and_drop() {
        let mut m = midi();
        for i in 0..MIDI_QUEUE_LIMIT {
            m.lines[0].queue.push_back((i & 0x7f) as u8); // fill directly (fast)
        }
        assert_eq!(m.lines[0].queue.len(), MIDI_QUEUE_LIMIT);
        assert_eq!(m.dropped, 0);
        assert_eq!(m.midi_in(0x41, 0), 0); // byte LIMIT+1 -> :161 drop
        assert_eq!(m.dropped, 1);
        assert_eq!(m.lines[0].queue.len(), MIDI_QUEUE_LIMIT);
        // one drained -> one accepted again (:158)
        m.lines[0].queue.pop_front();
        assert_eq!(m.midi_in(0x42, 0), 0);
        assert_eq!(m.dropped, 1);
        assert_eq!(m.lines[0].queue.back(), Some(&0x42));
    }

    // ---- bit machine (mu2000.cpp:36-37 + :1370-1413) ----

    #[test]
    fn bit_machine_frames_0x90_lsb_first_with_896cyc_bits() {
        let mut m = midi();
        let mut s = RecSci::new();
        m.midi_in(0x90, 0); // 0b1001_0000: LSB-first 0,0,0,0,1,0,0,1
        // t=0: gate :1389 `now < m.next` -> next=0, 0<0 false -> start bit
        m.midi_step(0, &mut s);
        assert_eq!(s.log, vec![(0, 0)]); // :1397 start low, bit=0
        assert_eq!(m.lines[0].bit, 0);
        assert_eq!(m.lines[0].next, 896); // :1394 MIDI_BIT_CYCLES
        s.log.clear();
        m.midi_step(895, &mut s); // :1401 hold
        assert!(s.log.is_empty());
        m.midi_step(896, &mut s); // bit 1 = LSB of 0x90 = 0
        assert_eq!(s.log, vec![(0, 0)]);
        // march the remaining data bits + stop
        let want: Vec<i32> = vec![0, 0, 0, 1, 0, 0, 1]; // bits 2..8 of 0x90
        for (k, w) in want.iter().enumerate() {
            s.log.clear();
            m.midi_step(896 * (2 + k as u64), &mut s);
            assert_eq!(s.log, vec![(0, *w)], "data bit {}", 2 + k);
        }
        s.log.clear();
        m.midi_step(896 * 9, &mut s); // bit 9 -> stop, :1409-1410
        assert_eq!(s.log, vec![(0, 1)]);
        assert_eq!(m.lines[0].bit, -1);
        assert_eq!(m.lines[0].next, 896 * 10);
        // 10 bit slots per byte: 8960 cycles @28MHz = 14.112 smp @44100
        // (mu2000.h:538-539 disk math, exact):
        assert_eq!(MIDI_BIT_CYCLES, 896);
        assert_eq!(MIDI_BIT_CYCLES * 10, 8960);
        assert_eq!(8960u64 * 44100 / 28_000_000, 14); // whole samples
        assert_eq!(8960u64 * 44100 * 1000 / 28_000_000, 14112); // milli
    }

    #[test]
    fn stop_bit_gap_gates_the_next_byte() {
        // :1388-1389: after the stop edge, the next start needs now >= next
        // (= stop-edge + 896), i.e. a full 10-bit cadence between starts.
        let mut m = midi();
        let mut s = RecSci::new();
        m.midi_in(0x01, 0);
        m.midi_in(0x02, 0);
        m.midi_step(0, &mut s); // start byte1 @0
        s.log.clear();
        for k in 1..=9 {
            m.midi_step(896 * k, &mut s);
        }
        assert_eq!(s.log.len(), 9); // 8 data + stop, no double start
        s.log.clear();
        m.midi_step(896 * 10 - 1, &mut s); // one cycle early: :1389 holds
        assert!(s.log.is_empty());
        m.midi_step(896 * 10, &mut s); // start byte2 exactly 10 bits later
        assert_eq!(s.log, vec![(0, 0)]);
        assert_eq!(m.lines[0].cur, 0x02);
    }

    #[test]
    fn lines_are_independent() {
        // mu2000.cpp:1372 "互いに待たせない" — port 1 runs its own clock
        let mut m = midi();
        let mut s = RecSci::new();
        m.midi_in(0x90, 1);
        for k in 0..3 {
            m.midi_step(896 * k, &mut s);
        }
        assert_eq!(s.log, vec![(1, 0), (1, 0), (1, 0)]); // 0x90 LSB: start,0,0
        assert_eq!(m.lines[0].bit, -1); // line 0 never woke up
        assert!(s.log.iter().all(|e| e.0 == 1));
    }

    // ---- queued / pending / idle semantics (mu2000.h:166-202) ----

    #[test]
    fn queued_counts_inflight_and_aliases_non_one_ports() {
        let mut m = midi();
        m.midi_in(0x90, 0);
        m.midi_in(0x40, 0);
        assert_eq!(m.midi_queued(0), 2); // two queued, nothing in flight
        let mut s = RecSci::new();
        m.midi_step(0, &mut s); // pop 0x90 into flight (bit 0)
        assert_eq!(m.midi_queued(0), 2); // 1 queued + 1 in flight (:171)
        assert_eq!(m.midi_queued(7), 2); // :170 port==1?1:0 -> any non-1 sees line 0
        assert_eq!(m.midi_queued(1), 0); // line 1 is its own
        while m.lines[0].bit >= 0 {
            m.next_edge_step(&mut s); // finish byte 1
        }
        assert_eq!(m.midi_queued(0), 1); // only 0x40 left, none in flight
    }

    impl Midi {
        /// advance to the next bit edge (mid-frame there is no other legal
        /// pump time; :1401)
        fn next_edge_step(&mut self, s: &mut dyn MidiSci) {
            let now = self
                .lines
                .iter()
                .map(|l| l.next)
                .max()
                .unwrap_or(0);
            self.midi_step(now, s);
        }
    }

    #[test]
    fn pending_and_idle_track_the_wire_fast_stub_false() {
        let mut m = midi();
        let mut s = RecSci::new();
        assert_eq!(m.midi_pending(&s), 0);
        assert!(m.midi_idle(0, &s) && m.midi_idle(1, &s) && m.midi_idle_all(&s));
        m.midi_in(0x90, 0);
        m.midi_in(0x40, 0);
        assert_eq!(m.midi_pending(&s), 2);
        assert!(!m.midi_idle_all(&s));
        m.midi_step(0, &mut s); // one byte in flight: still 2 on the wire (:177)
        assert_eq!(m.midi_pending(&s), 2);
        assert!(!m.midi_idle(0, &s));
        while m.lines[0].bit >= 0 || !m.lines[0].queue.is_empty() {
            m.next_edge_step(&mut s);
        }
        assert_eq!(m.midi_pending(&s), 0);
        assert!(m.midi_idle(0, &s) && m.midi_idle_all(&s));
        // usb arm of idle/pending: F5-route one byte to C, nothing pumps it
        assert_eq!(m.midi_in(0xf5, 0), -1);
        assert_eq!(m.midi_in(0x03, 0), -1);
        assert_eq!(m.midi_in(0x41, 0), 2);
        assert_eq!(m.midi_pending(&s), 3); // F5-framed: F5,03,41 in usb.rx (:175)
        assert!(!m.midi_idle_all(&s)); // usb_idle false (:192/:216)
        assert!(!m.midi_idle(2, &s)); // :185-186 port>=2 -> usb_idle()
        // DIN line 0 itself is still idle even with rx_byte_pending set:
        // fast_midi == false keeps the :188 rx arm dead
        s.pending = [true, true];
        assert!(m.midi_idle(0, &s));
        // all-idle stays FALSE: the parked USB byte keeps usb_idle() false
        // (:192/:216). Disk pumps it in usb_step (M7 — see module doc).
        assert!(!m.midi_idle_all(&s));
    }

    // ---- reset + fast-arm reachability guard ----

    #[test]
    fn reset_restores_cables_but_not_queues() {
        // mu2000.cpp:1061-1064 ONLY. Queues survive (disk has no clear).
        let mut m = midi();
        m.midi_in(0xf5, 0);
        m.midi_in(0x02, 0);
        m.midi_in(0x33, 0); // rides line 1
        m.reset_cables();
        assert_eq!(m.cable, [0, 1, 2, 3]); // :1009 init value (:1062)
        assert_eq!(m.cable_wait, [false; MIDI_PORTS]); // :1010 {} (:1063)
        assert_eq!(m.lines[1].queue.make_contiguous(), &[0x33]); // survives
    }

    #[test]
    fn fast_arm_inert_while_flag_false() {
        // OFF regression (default path byte-identical, M4 `midi lines`):
        // with fast_midi == false the pump ALWAYS wires bits through
        // do_rx_w, never touches receive_byte (mu2000.cpp:1414 stays
        // behind the :1408 gate) and never reads rx_byte_pending
        let mut m = midi();
        assert!(!m.fast_midi);
        m.midi_in(0x5a, 0);
        let mut s = RecSci::new();
        for k in 0..10 {
            m.midi_step(896 * k, &mut s);
        }
        assert_eq!(s.inj.len(), 0);
        assert_eq!(s.log.len(), 10); // start + 8 data + stop
        // OFF ignores the SCI byte-in-flight arm entirely (mu2000.h:184/195)
        s.pending = [true, true];
        assert!(m.midi_idle(0, &s) && m.midi_idle_all(&s));
        assert_eq!(m.midi_pending(&s), 0);
    }

    #[test]
    fn fast_arms_active_when_flag_on() {
        // M7 behavior pair of the inertness test above: with fast_midi ON
        // the pump injects queued bytes STRAIGHT into the SCI RDR
        // (mu2000.cpp:1408-1416, no wire bits) and pending/idle switch to
        // the SCI byte-in-flight rule (mu2000.h:184-207)
        let mut m = midi();
        m.set_fast_midi(true); // mu2000.h:120 (render --fast-midi path)
        assert!(m.fast_midi);
        m.midi_in(0x90, 0);
        m.midi_in(0x40, 0);
        let mut s = RecSci::new(); // accept=true, RDR clear both ports

        // one pump call injects at most ONE byte per port (disk pops once
        // per run_cycles iteration, mu2000.cpp:1410-1411) — no wire edges
        m.midi_step(0, &mut s);
        assert_eq!(s.inj, vec![(0, 0x90)]); // :1414 direct inject
        assert!(s.log.is_empty()); // no start/stop bits, no wire time
        assert_eq!(m.lines[0].bit, -1); // the bit machine never runs
        assert_eq!(m.lines[0].queue.make_contiguous(), &[0x40]);
        // rx_can_accept=false (RE off / RDRF busy / recv error,
        // sh_sci.cpp:127-130) holds the byte in the queue, no loss
        s.accept = false;
        m.midi_step(1, &mut s);
        assert_eq!(s.inj.len(), 1);
        assert_eq!(m.lines[0].queue.make_contiguous(), &[0x40]);
        s.accept = true;
        m.midi_step(2, &mut s);
        assert_eq!(s.inj, vec![(0, 0x90), (0, 0x40)]);
        assert!(m.lines[0].queue.is_empty());

        // ON semantics: a wire bit is INVISIBLE (:184 !fast gate) — idle
        // consults RDRF instead of bit<0 (:195/:206). A stuck bit cannot
        // exist in real fast mode (pump skips the wire path at :1416);
        // set it to prove the gate ignores it
        m.lines[0].bit = 4;
        m.lines[0].next = 12345;
        assert_eq!(m.midi_pending(&s), 0); // :183-184: bit not counted
        assert!(m.midi_idle(0, &s)); // :195: RDR clear -> idle
        assert!(m.midi_idle_all(&s)); // :204-207: both ports quiet
        // a byte in flight (firmware has not read RDR yet): +1 pending,
        // line busy — while the OTHER port stays idle
        s.pending = [true, false];
        assert_eq!(m.midi_pending(&s), 1); // :187
        assert!(!m.midi_idle(0, &s));
        assert!(m.midi_idle(1, &s));
        assert!(!m.midi_idle_all(&s));

        // the SAME state with the flag OFF flips the semantics: the wire
        // bit counts, the RDRF byte is invisible (:184/:195 else-arms)
        m.set_fast_midi(false);
        assert_eq!(m.midi_pending(&s), 1); // bit>=0 counted, rx arm dead
        assert!(!m.midi_idle(0, &s)); // bit 4 != -1
        assert!(!m.midi_idle_all(&s));
        m.lines[0].bit = -1;
        assert_eq!(m.midi_pending(&s), 0); // OFF never reads RDRF (:184)
        assert!(m.midi_idle_all(&s));
        // midi_queued is fast-OBtivIOUS on disk (:177-178 counts wire only)
        m.midi_in(0x41, 0);
        m.set_fast_midi(true);
        assert_eq!(m.midi_queued(0), 1);
        assert_eq!(m.midi_queued(7), 1); // :177 alias unchanged
    }
}
