//! SmartMedia card — STATE half of `src/smartmedia.{h,cpp}` (M5-W3a row of
//! `state serializer`). Fields = EXACTLY the set `smartmedia::state`
//! (smartmedia.cpp:362-375) touches; every one explicitly initialized in
//! [`Card::new`] from the smartmedia.h:82-90 member initializers (ledger
//! invariant 3 — the class has no ctor body and no reset(); construction IS
//! the reset state, `eject()` changes card data, not these).
//!
//! Bus behavior (control_w/data_w/data_r, NAND commands) is NOT wired here —
//! that is the `smartmedia stub` row. The empty-slot pins (PA18/19/20) keep
//! living in `Machine::card_inserted`; mu2000.cpp:3531-3533 calls
//! `m_card.state(s)` for version >= 5 INSIDE `mu2000::state` (W4 owns the
//! call ordering). NOTE: no `tag` — disk emits none (the v5 gate and
//! position are the mu2000::state legs).
//!
//! Quirk kept verbatim (smartmedia.cpp:371-374): the READ side caps the
//! queue length at 4096 *before* the resize, then `s.mem` consumes
//! `m_buf.size()` — so a >4096-byte queue from the wire desyncs the stream
//! (writer wrote n bytes, reader ate 4096). Never "fixed".

use smu_compat::StateIo;

/// origin: smartmedia.h:67 `enum class mode : u8 { idle, read, read_id,
/// status, program, erase }` — kept as the raw u8 it serializes as
/// (smartmedia.cpp:365-367 round-trips `u8` ⇄ `mode`, and GCC's cast of an
/// out-of-range byte keeps the byte; a Rust enum could not).
pub const MODE_IDLE: u8 = 0;
/// origin: smartmedia.h:67 `read`
pub const MODE_READ: u8 = 1;
/// origin: smartmedia.h:67 `read_id`
pub const MODE_READ_ID: u8 = 2;
/// origin: smartmedia.h:67 `status`
pub const MODE_STATUS: u8 = 3;
/// origin: smartmedia.h:67 `program`
pub const MODE_PROGRAM: u8 = 4;
/// origin: smartmedia.h:67 `erase`
pub const MODE_ERASE: u8 = 5;

/// origin: smartmedia.h:32-91 `class smartmedia` (state-touched fields only).
pub struct Card {
    /// origin: smartmedia.h:82 `u8 m_ctrl = 0` — CLE/ALE/CE/WE latch.
    pub ctrl: u8,
    /// origin: smartmedia.h:83 `mode m_mode = mode::idle` — u8 carrier
    /// (see [`MODE_IDLE`]).
    pub mode: u8,
    /// origin: smartmedia.h:84 `u8 m_pointer = 0` — 00/01/50 column phase.
    pub pointer: u8,
    /// origin: smartmedia.h:85 `u8 m_addr_count = 0`.
    pub addr_count: u8,
    /// origin: smartmedia.h:86 `u32 m_column = 0` (0-527 within a page).
    pub column: u32,
    /// origin: smartmedia.h:87 `u32 m_page = 0`.
    pub page: u32,
    /// origin: smartmedia.h:88 `u32 m_id_pos = 0`.
    pub id_pos: u32,
    /// origin: smartmedia.h:89 `u8 m_last_cmd = 0`.
    pub last_cmd: u8,
    /// origin: smartmedia.h:90 `std::vector<u8> m_buf` — the 528-byte
    /// program-staging queue (state queue leg = u32 count + raw bytes).
    pub buf: Vec<u8>,
}

impl Card {
    /// Construction == the smartmedia.h:82-90 member initializers verbatim
    /// (invariant 3: every field explicit; no Default::default()).
    pub fn new() -> Card {
        Card {
            ctrl: 0,        // :82 `= 0`
            mode: MODE_IDLE, // :83 `= mode::idle`
            pointer: 0,     // :84 `= 0`
            addr_count: 0,  // :85 `= 0`
            column: 0,      // :86 `= 0`
            page: 0,        // :87 `= 0`
            id_pos: 0,      // :88 `= 0`
            last_cmd: 0,    // :89 `= 0`
            buf: Vec::new(), // :90 default-empty vector
        }
    }

    /// origin: smartmedia.cpp:362-375 `state(state_io &s)` (M5-W3a). No
    /// card DATA (smartmedia.h:61 「カードの中身は入れない」), no tag.
    pub fn state(&mut self, s: &mut StateIo) {
        s.v(&mut self.ctrl); // :364 u8
        let mut md = self.mode; // :365 `u8 md = u8(m_mode)`
        s.v(&mut md); // :366
        self.mode = md; // :367 `m_mode = mode(md)` — u8 carrier, see MODE_IDLE
        s.v(&mut self.pointer); // :368 u8
        s.v(&mut self.addr_count); // :368 u8
        s.v(&mut self.column); // :368 u32
        s.v(&mut self.page); // :368 u32
        s.v(&mut self.id_pos); // :368 u32
        s.v(&mut self.last_cmd); // :368 u8
        let mut n = self.buf.len() as u32; // :369 `u32 n = u32(m_buf.size())`
        s.v(&mut n); // :370
        if !s.writing() {
            // :371-372 read side: resize(min(n,4096)) — value-init 0 bytes,
            // all overwritten by the mem leg below (or a sticky io failure)
            self.buf.resize(std::cmp::min(n, 4096u32) as usize, 0);
        }
        if !self.buf.is_empty() {
            // :373-374 mem(m_buf.data(), m_buf.size()) — size(), NOT n:
            // the >4096 read-side desync quirk, verbatim (module doc)
            let len = self.buf.len();
            s.mem(&mut self.buf[..len]);
        }
    }
}
