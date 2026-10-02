//! SH7042A general-purpose I/O ports (ledger row `periph: port`, M2).
//!
//! Faithful transliteration of `src/mame/cpu/sh_port.{h,cpp}`. The disk has
//! exactly TWO device classes (sh_port.h:22 / :54):
//!   * `sh_port16_device`  -> [`ShPort16`]  (u16 registers)
//!   * `sh_port32_device`  -> [`ShPort32`]  (u32 registers)
//! Ports A..F are NOT distinct classes; they are instances of those two
//! classes built in `sh7042.cpp:231-236` with per-port `index`/`default_io`/
//! `mask` (all `default_io = 0`). The disk set, verbatim:
//!
//! ```text
//!   A = SH_PORT32, index 0, default_io 0x00000000, mask 0xff000000  (sh7042.cpp:231)
//!   B = SH_PORT16, index 0, default_io 0x0000,     mask 0xfc00      (:232)
//!   C = SH_PORT16, index 1, default_io 0x0000,     mask 0x0000      (:233)
//!   D = SH_PORT32, index 1, default_io 0x0000,     mask 0x0000      (:234)
//!   E = SH_PORT16, index 2, default_io 0x0000,     mask 0x0000      (:235)  LCD
//!   F = SH_PORT16, index 3, default_io 0x0000,     mask 0xff00      (:236)  read-only
//! ```
//!
//! There is NO separate ODR register handler on disk: each port exposes only
//! PDR (`dr_r`/`dr_w`, offset +0) and PDDR (`io_r`/`io_w`, offset +4). The map
//! decodes exactly those two windows per port (sh7042_map.hxx; already folded
//! into the paired `sh7042.rs` register case-sets, e.g. PORT A DR @0xffff8380,
//! DDR @0xffff8384; PORT E DR @0xffff83b0, DDR @0xffff83b4; PORT F DR @0xffff83b2
//! READ-ONLY with no DDR and no write case anywhere in the map).
//!
//! ── HOST-INTERACTION SEAM ───────────────────────────────────────────────────
//! The C++ device holds a `required_device<sh7042_device> m_cpu` back-ref and
//! reaches the machine through `m_cpu->do_read_portN(index)` (input pins) and
//! `m_cpu->do_write_portN(index, m_dr & m_io, m_io)` (output drive)
//! (sh_port.cpp:40/47/59/74 and 98/107/120; sh7042.h:78-83). Those fan out to
//! per-index devcb arrays `m_read_port16/32`/`m_write_port16/32` (sh7042.cpp:68-
//! 83). Rust has no back-ref, so this row exposes the seam as:
//!
//!   * READ  — `dr_r` takes a `read_pins: FnMut() -> uN` closure and calls it
//!     *only inside the `(~io & ~mask) != 0` branch* (sh_port.cpp:46-48). The
//!     call-count match is load-bearing: mu2000's Port-A reader (mu2000.cpp:1083-
//!     1105) has SIDE EFFECTS — it advances the rotary encoder one detent per
//!     A-phase edge — so the pin delegate must fire exactly when C++ fires it.
//!     Wiring row fills this from the pin state: **encoder A/B live on Port A
//!     bits 17/16** (rea/reb, mu2000.cpp:1073-1077), and the active-low button
//!     matrix `m_sws[row]` (mu2000.cpp:475-528) reaches the read side too.
//!   * WRITE — `dr_w`/`io_w` return `Some((data, ddr))` = the two arguments C++
//!     hands to `do_write_portN(port, data, ddr)` (data = `m_dr & m_io`, only
//!     the driven output bits; ddr = `m_io`), or `None` when `m_io == 0`
//!     (sh_port.cpp:58-59/73-74). The wiring row dispatches the `Some` to the
//!     bound delegate. For **Port E that delegate is the LCD** (mu2000.cpp:
//!     1123-1124): `read_porte -> lcd_port_r`, `write_porte -> lcd_port_w(v)`,
//!     and the compat devcb hands the single-arg lambda the *middle* `data`
//!     argument (mamecompat.h:455-456), i.e. `v = m_dr & m_io`. So Port E's
//!     `Some((data, _))` is what `lcd_port_w` receives; the LCD reads R/W from
//!     bit0, RS from bit2, /E from bit4 and the 8 data lines from bits 8..15
//!     (mu2000.cpp:1004-1036). hd44780 itself is NOT ported here (later row).
//!
//! ── WRITE-CALLBACK QUIRK (disk-proven) ──────────────────────────────────────
//! The output callback fires on **EVERY** `dr_w`/`io_w` while `m_io != 0` —
//! there is NO change detection (no compare against the previous latch). Writing
//! the same value twice fires twice. It is gated only by `if (m_io)`. The LCD
//! consumer relies on this: `lcd_port_w` (mu2000.cpp:1018) edge-detects the /E
//! falling edge between successive calls, so a suppressed-on-change callback
//! would lose strobes. Locked by tests.
//!
//! FIELD EXPLICITNESS (Invariant 3): `new()` mirrors the full device_start
//! lifecycle — `m_io = m_default_io` (sh_port.cpp:27/86) and `m_dr` staying at
//! its sh_port.h:48/79 initializer `0`. `device_reset` is a genuine NO-OP on
//! disk (sh_port.cpp:32-34/91-93) and does NOT clear either register here.

/// origin: sh_port.h:22 `class sh_port16_device`.
#[derive(Debug, Clone)]
pub struct ShPort16 {
    /// origin: sh_port.h:44 `int m_index` — which `m_read/write_port16[]` slot.
    pub m_index: usize,
    /// origin: sh_port.h:45 `u16 m_default_io` — direction written at start.
    pub m_default_io: u16,
    /// origin: sh_port.h:45 `u16 m_mask` — protected bits (writes clear them).
    pub m_mask: u16,
    /// origin: sh_port.h:48 `u16 m_dr = 0` — PDR data latch.
    pub m_dr: u16,
    /// origin: sh_port.h:48 `u16 m_io = 0` — PDDR direction (bit set = output).
    pub m_io: u16,
    /// origin: `::smu2000::g_port_trace` (sh_port.cpp:39/54/70). Default false;
    /// when true AND `m_index == 2` (Port E) the three Port-E methods echo the
    /// same lines the C++ `fprintf` writes. Never set on the audio path.
    pub trace: bool,
}

/// origin: sh_port.h:54 `class sh_port32_device`.
#[derive(Debug, Clone)]
pub struct ShPort32 {
    /// origin: sh_port.h:77 `int m_index`.
    pub m_index: usize,
    /// origin: sh_port.h:78 `u32 m_default_io`.
    pub m_default_io: u32,
    /// origin: sh_port.h:78 `u32 m_mask`.
    pub m_mask: u32,
    /// origin: sh_port.h:79 `u32 m_dr = 0`.
    pub m_dr: u32,
    /// origin: sh_port.h:79 `u32 m_io = 0`.
    pub m_io: u32,
}

impl ShPort16 {
    /// origin: ctor sh_port.cpp:19-23 + device_start :25-30. `m_dr` stays 0
    /// (sh_port.h:48), `m_io = m_default_io` (sh_port.cpp:27). Every field set.
    pub fn new(index: usize, default_io: u16, mask: u16) -> Self {
        Self { m_index: index, m_default_io: default_io, m_mask: mask, m_dr: 0, m_io: default_io, trace: false }
    }

    // Disk port set (config-only helpers so the wiring row cannot mis-set the
    // per-port constants). Cites are the sh7042.cpp SH_PORT16(...) args.

    /// origin: sh7042.cpp:232 `SH_PORT16(m_portb, *this, 0, 0x0000, 0xfc00)`.
    pub fn portb() -> Self { Self::new(0, 0x0000, 0xfc00) }
    /// origin: sh7042.cpp:233 `SH_PORT16(m_portc, *this, 1, 0x0000, 0x0000)`.
    pub fn portc() -> Self { Self::new(1, 0x0000, 0x0000) }
    /// origin: sh7042.cpp:235 `SH_PORT16(m_porte, *this, 2, 0x0000, 0x0000)` — LCD.
    pub fn porte() -> Self { Self::new(2, 0x0000, 0x0000) }
    /// origin: sh7042.cpp:236 `SH_PORT16(m_portf, *this, 3, 0x0000, 0xff00)` — read-only (map has no write case).
    pub fn portf() -> Self { Self::new(3, 0x0000, 0xff00) }

    /// origin: sh_port.cpp:32-34 `device_reset()` is empty. Does NOT touch
    /// `m_dr`/`m_io` — direction and latch survive a reset unchanged.
    pub fn device_reset(&mut self) {}

    /// origin: sh_port.cpp:124-128 (`sh_port16_device::state`); tag "port16",
    /// m_dr/m_io u16 (sh_port.h:48). Config members (m_index/m_default_io/
    /// m_mask) are ctor constants — NOT serialized on disk :124-128.
    pub fn state(&mut self, s: &mut smu_compat::StateIo) {
        s.tag("port16");       // :126
        s.v(&mut self.m_dr);   // :127
        s.v(&mut self.m_io);   // :127
    }

    /// origin: sh_port.cpp:36-49 `dr_r`.
    /// Reads the pin delegate only when `(~m_io & ~m_mask) != 0` (bits that are
    /// input AND unmasked exist). Return = latched outputs OR'd with the pin
    /// value on the input bits; when the branch is not taken the pure latch is
    /// returned and the delegate is NOT called (encoder side-effect safety).
    pub fn dr_r<F: FnMut() -> u16>(&mut self, mut read_pins: F) -> u16 {
        if self.trace && self.m_index == 2 {
            // origin: sh_port.cpp:39-45 (mirror of the same computation; one read)
            let ext = read_pins();
            let v = if (!self.m_io & !self.m_mask) != 0 {
                (self.m_dr & self.m_io) | (ext & !self.m_io)
            } else {
                self.m_dr
            };
            eprintln!(
                "R dr={:04x} io={:04x} mask={:04x} ext={:04x} -> {:04x}",
                self.m_dr, self.m_io, self.m_mask, ext, v
            );
            return v;
        }
        // origin: sh_port.cpp:46-48
        if (!self.m_io & !self.m_mask) != 0 {
            (self.m_dr & self.m_io) | (read_pins() & !self.m_io)
        } else {
            self.m_dr
        }
    }

    /// origin: sh_port.cpp:51-60 `dr_w`.
    /// `COMBINE_DATA` (mamecompat.h:617) writes only the `mem_mask`-set bits,
    /// then `m_dr &= ~m_mask` forces protected bits to 0 (:57), then — if any
    /// bit is an output (`m_io != 0`) — returns the `do_write_port16` args
    /// `(m_dr & m_io, m_io)`. Fires on EVERY write (no change detection,
    /// sh_port.cpp:58-59); `None` when `m_io == 0`.
    pub fn dr_w(&mut self, data: u16, mem_mask: u16) -> Option<(u16, u16)> {
        // origin: sh_port.cpp:53 COMBINE_DATA(&m_dr)
        self.m_dr = (self.m_dr & !mem_mask) | (data & mem_mask);
        // origin: sh_port.cpp:54-56 trace AFTER combine, BEFORE mask-clear
        if self.trace && self.m_index == 2 {
            eprintln!(
                "W dr={:04x} io={:04x} data={:04x} mask={:04x}",
                self.m_dr, self.m_io, data, mem_mask
            );
        }
        // origin: sh_port.cpp:57
        self.m_dr &= !self.m_mask;
        // origin: sh_port.cpp:58-59
        if self.m_io != 0 {
            Some((self.m_dr & self.m_io, self.m_io))
        } else {
            None
        }
    }

    /// origin: sh_port.cpp:62-65 `io_r` — returns the raw direction.
    pub fn io_r(&self) -> u16 {
        self.m_io
    }

    /// origin: sh_port.cpp:67-75 `io_w`.
    /// COMBINE the new direction (:69), `m_io &= ~m_mask` forces protected bits
    /// to INPUT (:72), then if `m_io != 0` re-drives the (post-mask) outputs via
    /// `do_write_port16(port, m_dr & m_io, m_io)` (:73-74). Returns the args.
    pub fn io_w(&mut self, data: u16, mem_mask: u16) -> Option<(u16, u16)> {
        // origin: sh_port.cpp:69 COMBINE_DATA(&m_io)
        self.m_io = (self.m_io & !mem_mask) | (data & mem_mask);
        // origin: sh_port.cpp:70-71 trace AFTER combine, BEFORE mask-clear
        if self.trace && self.m_index == 2 {
            eprintln!("IO io={:04x} data={:04x}", self.m_io, data);
        }
        // origin: sh_port.cpp:72
        self.m_io &= !self.m_mask;
        // origin: sh_port.cpp:73-74
        if self.m_io != 0 {
            Some((self.m_dr & self.m_io, self.m_io))
        } else {
            None
        }
    }
}

impl ShPort32 {
    /// origin: ctor sh_port.cpp:78-82 + device_start :84-89. `m_dr` stays 0
    /// (sh_port.h:79), `m_io = m_default_io` (sh_port.cpp:86).
    pub fn new(index: usize, default_io: u32, mask: u32) -> Self {
        Self { m_index: index, m_default_io: default_io, m_mask: mask, m_dr: 0, m_io: default_io }
    }

    /// origin: sh7042.cpp:231 `SH_PORT32(m_porta, *this, 0, 0x00000000, 0xff000000)` — encoder on bits 16/17.
    pub fn porta() -> Self { Self::new(0, 0x0000_0000, 0xff00_0000) }
    /// origin: sh7042.cpp:234 `SH_PORT32(m_portd, *this, 1, 0x0000, 0x0000)`.
    pub fn portd() -> Self { Self::new(1, 0x0000_0000, 0x0000_0000) }

    /// origin: sh_port.cpp:91-93 `device_reset()` is empty (NO-OP).
    pub fn device_reset(&mut self) {}

    /// origin: sh_port.cpp:130-134 (`sh_port32_device::state`); tag "port32",
    /// m_dr/m_io u32 (sh_port.h:79).
    pub fn state(&mut self, s: &mut smu_compat::StateIo) {
        s.tag("port32");       // :132
        s.v(&mut self.m_dr);   // :133
        s.v(&mut self.m_io);   // :133
    }

    /// origin: sh_port.h:70 `io_now()` (used by `sh7042_device::porta_io`,
    /// sh7042.h:82, to peek the dial direction from outside). 16-bit class has
    /// no analogue — kept off `ShPort16` deliberately.
    pub fn io_now(&self) -> u32 {
        self.m_io
    }

    /// origin: sh_port.cpp:95-100 `dr_r` (32-bit). No trace hook (disk has none).
    pub fn dr_r<F: FnMut() -> u32>(&mut self, mut read_pins: F) -> u32 {
        // origin: sh_port.cpp:97-99
        if (!self.m_io & !self.m_mask) != 0 {
            (self.m_dr & self.m_io) | (read_pins() & !self.m_io)
        } else {
            self.m_dr
        }
    }

    /// origin: sh_port.cpp:102-108 `dr_w` (32-bit). Same fire-on-every-write
    /// semantics; `mem_mask`/widths are 32-bit.
    pub fn dr_w(&mut self, data: u32, mem_mask: u32) -> Option<(u32, u32)> {
        // origin: sh_port.cpp:104 COMBINE_DATA(&m_dr)
        self.m_dr = (self.m_dr & !mem_mask) | (data & mem_mask);
        // origin: sh_port.cpp:105
        self.m_dr &= !self.m_mask;
        // origin: sh_port.cpp:106-107
        if self.m_io != 0 {
            Some((self.m_dr & self.m_io, self.m_io))
        } else {
            None
        }
    }

    /// origin: sh_port.cpp:110-113 `io_r`.
    pub fn io_r(&self) -> u32 {
        self.m_io
    }

    /// origin: sh_port.cpp:115-121 `io_w` (32-bit).
    pub fn io_w(&mut self, data: u32, mem_mask: u32) -> Option<(u32, u32)> {
        // origin: sh_port.cpp:117 COMBINE_DATA(&m_io)
        self.m_io = (self.m_io & !mem_mask) | (data & mem_mask);
        // origin: sh_port.cpp:118
        self.m_io &= !self.m_mask;
        // origin: sh_port.cpp:119-120
        if self.m_io != 0 {
            Some((self.m_dr & self.m_io, self.m_io))
        } else {
            None
        }
    }
}
