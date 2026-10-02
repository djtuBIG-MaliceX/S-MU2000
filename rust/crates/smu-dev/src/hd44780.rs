//! HD44780 LCD controller — port of `src/mame/video/hd44780.{h,cpp}` (270 LOC).
//!
//! **The busy flag is boot-critical** (hd44780.h:7-9): the MU2000 firmware
//! watches busy after every command; answering "always free" derails boot.
//! Disk dropped MAME's `emu_timer` in favour of CPU-cycle counting
//! (hd44780.h:14-16): LCD oscillator 270 kHz, instructions 10 LCD cycles
//! (~37 us @ 28 MHz), clear/home 410 LCD cycles (~1.5185 ms). The math is
//! copied EXACTLY in [`Hd44780::set_busy`] (hd44780.h:106-109); `smu_compat`
//! timers are NOT used here — the clock arrives via [`Hd44780::set_now`]
//! (hd44780.h:39), driven from `m_cpu->total_cycles()` at every port access
//! (mu2000.cpp:1006/1017).
//!
//! Font: the CGROM is the 4 KB HD44780U B04 image produced by
//! `smu_compat::roms::{load_lcd_font,set_lcd_font}` (M1 rom-loaders row,
//! mu2000.cpp:569-578/698-708 — that chain already applies
//! `roms::fill_missing_glyphs` + the `lcdfont` overlay before handing bytes
//! here); glyphs 0x00-0x0f come from CGRAM (hd44780.h:11-12, cpp:252-260).
//! This module reuses those byte buffers, it does NOT re-implement glyphs.
//!
//! Port-E seam (mu2000.cpp:1001-1036): bit4 /E (falling-edge strobe),
//! bit2 RS, bit0 R/W, data in bits 8-15. [`Hd44780::lcd_port_w`] is the exact
//! delegate the M2 wiring row binds to the `port.rs` `dr_w`/`io_w` seam — the
//! callback fires on EVERY write (port.rs doc 55-61) and this method
//! edge-detects /E exactly like disk (mu2000.cpp:1018). Busy reads back
//! through [`Hd44780::lcd_port_r`]: `(busy<<7|addr) << 8`, i.e. Port E data
//! bit 15 (mu2000.cpp:1004-1013 ← `read_porte` mu2000.cpp:1123).
//!
//! Deviations (all logged in the session report per Invariant 2):
//! * `m_pe` latch lives INSIDE the device (disk keeps it in `mu2000.h:891`);
//!   locality only — decode/edge semantics identical.
//! * `set_cgrom` stores an owned `Vec<u8>` copy behind the same `>= 0x1000`
//!   gate instead of a raw pointer (hd44780.h:91-92); the font is immutable
//!   after `set_lcd_font` (mu2000.cpp:701-708), one copy at boot.
//! * native-engine surface — `poke_ddram`/`poke_cgram`, `set_owned`/
//!   `set_cg_owned`/`clear_*_owned` (hd44780.h:53-88, cpp:34-79) and the
//!   `changes` log — is NOT ported (AGENTS.md ignores the native engine).
//!   `m_fw`/`m_cg_fw` ARE kept and written unconditionally, which is disk
//!   behaviour with `owned()==false` (cpp:200-212), so read-back semantics
//!   (cpp:223-224) are bit-identical for the firmware path.
//! * `state()` (cpp:273-296) ported with M5-W3a. `m_owned`/`m_cg_owned`
//!   (hd44780.h:123/125) have no native reader in this port, but the fields
//!   are kept so the state stream stays byte-identical to C++ in BOTH
//!   directions (the v>=11/12 legs ride their u64s verbatim; the v<11/12
//!   legs zero them exactly like disk). Firmware path: owned is always 0
//!   (no native engine built — wiring row), so the bytes match on both sides.
//! * panel-hold in `lcd_port_w` (mu2000.cpp:1024-1032) is native-engine
//!   plumbing; omitted.

use smu_compat::StateIo;

/// origin: hd44780.h:114 `enum { DDRAM, CGRAM }` (`m_active_ram`).
pub const DDRAM: i32 = 0;
/// origin: hd44780.h:114 — CGRAM member of `m_active_ram`.
pub const CGRAM: i32 = 1;

/// origin: hd44780.h:97 `static constexpr int RENDER_SIZE = 80 * 16`.
pub const RENDER_SIZE: usize = 80 * 16;

/// origin: hd44780.h:26-145 `class hd44780_device`.
#[derive(Debug, Clone)]
pub struct Hd44780 {
    /// origin: hd44780.h:116 `u32 m_cpu_hz, m_lcd_hz` (ctor defaults
    /// hd44780.h:33: 28 MHz / 270 kHz).
    pub m_cpu_hz: u32,
    pub m_lcd_hz: u32,
    /// origin: hd44780.h:117 `u64 m_now, m_busy_until`.
    pub m_now: u64,
    pub m_busy_until: u64,
    /// origin: hd44780.h:119 `const u8 *m_cgrom` — owned copy here
    /// (module-doc deviation); `None` = gate failed, render emits zeros.
    m_cgrom: Option<Vec<u8>>,
    /// origin: hd44780.h:120 `u8 m_render_buf[RENDER_SIZE]`.
    m_render_buf: [u8; RENDER_SIZE],
    /// origin: hd44780.h:121 `u8 m_ddram[0x80]` (128 cells — NOT 80).
    pub m_ddram: [u8; 0x80],
    /// origin: hd44780.h:122 `u8 m_fw[0x80]` — the screen the firmware
    /// believes is displayed (6.188); what `data_r` returns (cpp:224).
    pub m_fw: [u8; 0x80],
    /// origin: hd44780.h:123 `u64 m_owned[2]` — native ownership bitmaps.
    /// State-fidelity only here (module doc): the firmware path never sets
    /// them, but `state()` (cpp:281/284) serializes/zeroes them verbatim.
    pub m_owned: [u64; 2],
    /// origin: hd44780.h:124 `u8 m_cg_fw[0x40]` (6.190 counterpart).
    pub m_cg_fw: [u8; 0x40],
    /// origin: hd44780.h:125 `u64 m_cg_owned` — CGRAM counterpart, same
    /// state-fidelity note as [`Hd44780::m_owned`].
    pub m_cg_owned: u64,
    /// origin: hd44780.h:126 `u8 m_cgram[0x40]` (64 bytes, NOT 8).
    pub m_cgram: [u8; 0x40],
    /// origin: hd44780.h:127-133 — signed `int` on disk; keep i32 so the
    /// `m_ac < 0` / `m_disp_shift == -1` arms of correct_ac/shift stay live.
    pub m_ac: i32,
    pub m_active_ram: i32,
    pub m_direction: i32,
    pub m_disp_shift: i32,
    pub m_num_line: i32,
    pub m_char_size: i32,
    pub m_data_len: i32,
    /// origin: hd44780.h:134-137. `m_nibble` is ASSIGNED ONLY at reset
    /// (cpp:24) and state-load (cpp:295) on disk — the `m_nibble` arms of
    /// control_w/data_w/render NEVER execute at runtime; transliterated.
    pub m_shift_on: bool,
    pub m_display_on: bool,
    pub m_cursor_on: bool,
    pub m_blink_on: bool,
    pub m_nibble: bool,
    pub m_ir: u8,
    pub m_dr: u8,
    /// origin: mu2000.h:891 `u16 m_pe` — Port E control latch
    /// (mu2000.cpp:1035). Deviation: hoisted into the device (module doc).
    pub m_pe: u16,
}

/// origin: the `BIT(x, n)` macro used all over hd44780.cpp / mu2000.cpp.
fn bit(v: u16, n: u32) -> bool {
    (v >> n) & 1 != 0
}

impl Hd44780 {
    /// origin: hd44780.h:33-34 ctor `hd44780_device(u32 cpu_hz = 28000000,
    /// u32 lcd_hz = 270000)`. Invariant 3: every field explicit — mirrors the
    /// `= 0` / `= {}` header initializers (hd44780.h:116-139), then the
    /// mu2000.cpp:1126 caller immediately `reset()`s.
    pub fn new(cpu_hz: u32, lcd_hz: u32) -> Hd44780 {
        Hd44780 {
            m_cpu_hz: cpu_hz,           // hd44780.h:116 (ctor arg)
            m_lcd_hz: lcd_hz,           // hd44780.h:116 (ctor arg)
            m_now: 0,                   // hd44780.h:117 `= 0`
            m_busy_until: 0,            // hd44780.h:117 `= 0`
            m_cgrom: None,              // hd44780.h:119 `= nullptr`
            m_render_buf: [0u8; RENDER_SIZE], // hd44780.h:120 `= {}`
            m_ddram: [0u8; 0x80],       // hd44780.h:121 `= {}`
            m_fw: [0u8; 0x80],          // hd44780.h:122 `= {}`
            m_owned: [0u64; 2],         // hd44780.h:123 `= {}`
            m_cg_fw: [0u8; 0x40],       // hd44780.h:124 `= {}`
            m_cg_owned: 0,              // hd44780.h:125 `= 0`
            m_cgram: [0u8; 0x40],       // hd44780.h:126 `= {}`
            m_ac: 0,                    // hd44780.h:127
            m_active_ram: DDRAM,        // hd44780.h:128 `= DDRAM`
            m_direction: 1,             // hd44780.h:129 `= 1`
            m_disp_shift: 0,            // hd44780.h:130
            m_num_line: 1,              // hd44780.h:131 `= 1`
            m_char_size: 8,             // hd44780.h:132 `= 8`
            m_data_len: 8,              // hd44780.h:133 `= 8`
            m_shift_on: false,          // hd44780.h:134
            m_display_on: false,        // hd44780.h:135
            m_cursor_on: false,         // hd44780.h:135
            m_blink_on: false,          // hd44780.h:135
            m_nibble: false,            // hd44780.h:136
            m_ir: 0,                    // hd44780.h:137
            m_dr: 0,                    // hd44780.h:137
            m_pe: 0,                    // mu2000.h:891 `= 0`
        }
    }

    /// origin: hd44780.h:39 `set_now` — "the current CPU cycle; set before
    /// every read or write". This is the clock seam; no emu_timer (disk).
    pub fn set_now(&mut self, cycles: u64) {
        self.m_now = cycles;
    }

    /// origin: hd44780.h:46 `bool busy() const { return m_now < m_busy_until; }`.
    pub fn busy(&self) -> bool {
        self.m_now < self.m_busy_until
    }

    /// origin: hd44780.h:106-109 `set_busy(u16 lcd_cycles)`:
    /// `m_busy_until = m_now + u64(lcd_cycles) * m_cpu_hz / m_lcd_hz`.
    /// Integer division, truncation-toward-zero, EXACT (boot-critical):
    /// 10 * 28_000_000 / 270_000 = 1037 cycles; 410 * 28_000_000 /
    /// 270_000 = 42518 cycles (the .5 truncates).
    fn set_busy(&mut self, lcd_cycles: u16) {
        self.m_busy_until =
            self.m_now + (lcd_cycles as u64) * (self.m_cpu_hz as u64) / (self.m_lcd_hz as u64);
    }

    /// origin: hd44780.cpp:11-32 `reset()`. Does NOT touch `m_now`
    /// (busy_until builds on whatever the caller last latched) and does NOT
    /// touch `m_pe` (mu2000 never resets it either).
    pub fn reset(&mut self) {
        self.m_ddram = [0x20; 0x80]; // :13 memset 0x20
        self.m_cgram = [0u8; 0x40]; // :14
        self.m_ac = 0; // :15
        self.m_active_ram = DDRAM; // :16
        self.m_direction = 1; // :17
        self.m_disp_shift = 0; // :18
        self.m_num_line = 1; // :19
        self.m_char_size = 8; // :20
        self.m_data_len = 8; // :21
        self.m_shift_on = false; // :22
        self.m_display_on = false; // :23
        self.m_cursor_on = false; // :23
        self.m_blink_on = false; // :23
        self.m_nibble = false; // :24
        self.m_ir = 0; // :25
        self.m_dr = 0; // :25
        self.m_busy_until = 0; // :26
        self.m_fw.memcpy_bytes(&self.m_ddram); // :29
        self.m_cg_fw.memcpy_bytes(&self.m_cgram); // :30
        self.set_busy(410); // :31 power-on busy ~1.5185 ms
    }

    /// origin: hd44780.cpp:81-93 `correct_ac()` — the DDRAM wrap rules:
    /// 1-line max 0x4f, 2-line max 0x67, `ac -= max_ac + 1` down-wrap,
    /// negative → max_ac, 2-line 0x28..0x3f mirrors to 0x40..; CGRAM `& 0x3f`.
    fn correct_ac(&mut self) {
        if self.m_active_ram == DDRAM {
            let max_ac: i32 = if self.m_num_line == 1 { 0x4f } else { 0x67 }; // :84
            if self.m_ac > max_ac {
                // :85-86
                self.m_ac -= max_ac + 1;
            } else if self.m_ac < 0 {
                // :87-88
                self.m_ac = max_ac;
            } else if self.m_num_line == 2 && self.m_ac > 0x27 && self.m_ac < 0x40 {
                // :89-90
                self.m_ac = 0x40 + (self.m_ac - 0x28);
            }
        } else {
            self.m_ac &= 0x3f; // :92
        }
    }

    /// origin: hd44780.cpp:95-102 `update_ac(int direction)` — 2-line
    /// 0x40→0x27 backward jump, else plain add, then correct_ac.
    fn update_ac(&mut self, direction: i32) {
        if self.m_active_ram == DDRAM && self.m_num_line == 2 && direction == -1 && self.m_ac == 0x40
        {
            self.m_ac = 0x27; // :97-98
        } else {
            self.m_ac += direction; // :100
        }
        self.correct_ac(); // :101
    }

    /// origin: hd44780.cpp:104-111 `shift_display(int direction)` — ring
    /// 0..0x4f: 0x50 → 0, -1 → 0x4f.
    fn shift_display(&mut self, direction: i32) {
        self.m_disp_shift += direction; // :106
        if self.m_disp_shift == 0x50 {
            self.m_disp_shift = 0; // :107-108
        } else if self.m_disp_shift == -1 {
            self.m_disp_shift = 0x4f; // :109-110
        }
    }

    /// origin: hd44780.cpp:113-178 `control_w(u8 data)` — the command
    /// decode. Command busy values from disk: every address/function/shift/
    /// display/entry op `set_busy(10)` (~37 us), home/clear `set_busy(410)`
    /// (~1.5185 ms) — hd44780.h:15-16.
    pub fn control_w(&mut self, data: u8) {
        let data = data as u16;
        if self.m_data_len == 4 {
            // :115
            if self.m_nibble {
                self.m_ir = ((data & 0xf0) & 0xff) as u8; // :116 (dead path, see field doc)
                return;
            }
            self.m_ir |= ((data >> 4) & 0x0f) as u8; // :117 OR-into-stale-nibble quirk
        } else {
            self.m_ir = data as u8; // :119
        }
        let ir = self.m_ir as u16;

        if bit(ir, 7) {
            // :121 DDRAM address set
            self.m_active_ram = DDRAM; // :122
            self.m_ac = (ir & 0x7f) as i32; // :123
            self.correct_ac(); // :124
            self.set_busy(10); // :125
            return; // :126
        }
        if bit(ir, 6) {
            // :128 CGRAM address set
            self.m_active_ram = CGRAM; // :129
            self.m_ac = (ir & 0x3f) as i32; // :130
            self.set_busy(10); // :131
            return; // :132
        }
        if bit(ir, 5) {
            // :134 function set
            if bit(ir, 3) {
                self.m_char_size = 8; // :135-136 5x10 unusable at 2 lines
            } else {
                self.m_char_size = if bit(ir, 2) { 10 } else { 8 }; // :138
            }
            self.m_data_len = if bit(ir, 4) { 8 } else { 4 }; // :139
            self.m_num_line = bit(ir, 3) as i32 + 1; // :140
            self.correct_ac(); // :141
            self.set_busy(10); // :142
            return; // :143
        }
        if bit(ir, 4) {
            // :145 cursor/display shift
            let direction: i32 = if bit(ir, 2) { 1 } else { -1 }; // :146
            if bit(ir, 3) {
                self.shift_display(direction); // :147-148
            } else {
                self.update_ac(direction); // :149-150
            }
            self.set_busy(10); // :151
        } else if bit(ir, 3) {
            // :152 display on/off
            self.m_display_on = bit(ir, 2); // :153
            self.m_cursor_on = bit(ir, 1); // :154
            self.m_blink_on = bit(ir, 0); // :155
            self.set_busy(10); // :156
        } else if bit(ir, 2) {
            // :157 entry mode
            self.m_direction = if bit(ir, 1) { 1 } else { -1 }; // :158
            self.m_shift_on = bit(ir, 0); // :159
            self.set_busy(10); // :160
        } else if bit(ir, 1) {
            // :161 home
            self.m_ac = 0; // :162
            self.m_active_ram = DDRAM; // :163
            self.m_direction = 1; // :164
            self.m_disp_shift = 0; // :165
            self.set_busy(410); // :166
        } else if bit(ir, 0) {
            // :167 display clear
            self.m_ac = 0; // :168
            self.m_active_ram = DDRAM; // :169
            self.m_direction = 1; // :170
            self.m_disp_shift = 0; // :171
            self.m_ddram = [0x20; 0x80]; // :172
            self.m_fw = [0x20; 0x80]; // :174 (unconditional here = disk owned() clear, :175)
            self.set_busy(410); // :176
        }
    }

    /// origin: hd44780.cpp:180-188 `control_r() const` — the status byte:
    /// 8-bit: busy(bit7) | addr(bits0-6); 4-bit low nibble: addr<<4, NO busy
    /// bit (disk quirk, :185). Reached via `lcd_port_r` RS=0/RW=1.
    pub fn control_r(&self) -> u8 {
        if self.m_data_len == 4 {
            // :182
            if self.m_nibble {
                // :183-184 (dead path)
                return (((self.busy() as u16) << 7) | ((self.m_ac as u16) & 0x70)) as u8;
            }
            return (((self.m_ac << 4) & 0xf0) as u16 & 0xff) as u8; // :185
        }
        (((self.busy() as u16) << 7) | ((self.m_ac as u16) & 0x7f)) as u8 // :187
    }

    /// origin: hd44780.cpp:190-219 `data_w(u8 data)` — DDRAM/CGRAM write.
    /// `m_fw` written unconditionally: disk-equivalent of `owned()==false`
    /// (cpp:200-205 minus the native owned/changes branch, module doc).
    pub fn data_w(&mut self, data: u8) {
        let data = data as u16;
        if self.m_data_len == 4 {
            // :192
            if self.m_nibble {
                self.m_dr = ((data & 0xf0) & 0xff) as u8; // :193 (dead path)
                return;
            }
            self.m_dr |= ((data >> 4) & 0x0f) as u8; // :194 quirk, mirrors :117
        } else {
            self.m_dr = data as u8; // :196
        }

        let ac = self.m_ac as usize;
        if self.m_active_ram == DDRAM {
            // :198
            self.m_fw[ac] = self.m_dr; // :200
            self.m_ddram[ac] = self.m_dr; // :204
        } else {
            self.m_cg_fw[ac] = self.m_dr; // :207
            self.m_cgram[ac] = self.m_dr; // :211
        }

        self.set_busy(10); // :215 data write busy = 10 LCD cycles (~37 us)
        self.update_ac(self.m_direction); // :216
        if self.m_shift_on {
            self.shift_display(self.m_direction); // :217-218
        }
    }

    /// origin: hd44780.cpp:221-235 `data_r()` — firmware reads back what IT
    /// wrote (cpp:223-224 → `m_fw`/`m_cg_fw`); then busy(10) + ac update.
    /// Reached via `lcd_port_r` RS=1/RW=1.
    pub fn data_r(&mut self) -> u8 {
        let ac = self.m_ac as usize;
        let mut data = if self.m_active_ram == DDRAM {
            // :224
            self.m_fw[ac]
        } else {
            self.m_cg_fw[ac]
        };

        if self.m_data_len == 4 {
            // :226
            if self.m_nibble {
                return data & 0xf0; // :227-228 (dead path)
            }
            data = (data << 4) & 0xf0; // :229
        }

        self.set_busy(10); // :232
        self.update_ac(self.m_direction); // :233
        data // :234
    }

    /// origin: hd44780.h:91-92 `set_cgrom` — same gate (`rom && size >=
    /// 0x1000` → usable, else nullptr/None). Deviation: owned copy (module
    /// doc). Host: `smu_compat::roms::set_lcd_font` output, mu2000.cpp:708.
    pub fn set_cgrom(&mut self, rom: &[u8]) {
        self.m_cgrom = if !rom.is_empty() && rom.len() >= 0x1000 {
            Some(rom.to_vec())
        } else {
            None
        };
    }

    /// origin: hd44780.cpp:240-270 `render()` — 80 cells x 16 bytes,
    /// CGRAM for codes < 0x10 (5x8: `(c&7)*8`; 5x10: `((c>>1)&3)*16`),
    /// CGROM `c * 0x10` otherwise (the reused `roms::lcdfont`-patched font);
    /// cursor paints 0x1f into the last glyph row.
    pub fn render(&mut self) -> &[u8] {
        self.m_render_buf = [0u8; RENDER_SIZE]; // :242
        if !self.m_display_on || self.m_cgrom.is_none() {
            // :243
            return &self.m_render_buf;
        }
        let cgrom = self.m_cgrom.clone().unwrap(); // module doc: owned copy
        let line_size: i32 = 80 / self.m_num_line; // :246
        for line in 0..self.m_num_line {
            // :247
            for pos in 0..line_size {
                // :248
                let char_pos: usize =
                    (line * 0x40 + (pos + self.m_disp_shift) % line_size) as usize; // :249

                let src: &[u8]; // :251
                if self.m_ddram[char_pos] < 0x10 {
                    // :252 user chars from CGRAM
                    if self.m_char_size == 8 {
                        src = &self.m_cgram[((self.m_ddram[char_pos] as usize) & 0x07) * 8..]; // :255
                    } else {
                        src = &self.m_cgram[(((self.m_ddram[char_pos] as usize) >> 1) & 0x03) * 16..]; // :257
                    }
                } else {
                    src = &cgrom[self.m_ddram[char_pos] as usize * 0x10..]; // :259
                }

                let base = 16 * (line * line_size + pos) as usize; // :262
                let n = self.m_char_size as usize; // :263 memcpy m_char_size bytes
                self.m_render_buf[base..base + n].copy_from_slice(&src[..n]);

                if char_pos == self.m_ac as usize && self.m_cursor_on {
                    // :265
                    self.m_render_buf[base + self.m_char_size as usize - 1] = 0x1f; // :266
                }
            }
        }
        &self.m_render_buf
    }

    /// origin: hd44780.h:100-103 accessors `lines/line_size/char_size/display_on`.
    pub fn lines(&self) -> i32 {
        self.m_num_line
    }
    pub fn line_size(&self) -> i32 {
        80 / self.m_num_line
    }
    pub fn char_size(&self) -> i32 {
        self.m_char_size
    }
    pub fn display_on(&self) -> bool {
        self.m_display_on
    }

    /// origin: mu2000.cpp:1015-1036 `lcd_port_w(u16 data)` — the Port E
    /// write delegate. `data` is EXACTLY what the port.rs `dr_w`/`io_w` seam
    /// hands back as `Some((data, ddr)).0` (= `m_dr & m_io`, port.rs doc
    /// 43-51; `ddr` is IGNORED by disk's single-arg lambda, mamecompat.h:455
    /// — so this seam takes only `data`). set_now first (mu2000.cpp:1017);
    /// falling /E edge between successive calls (:1018) — the callback fires
    /// on EVERY write (port.rs quirk), so edge-detect here is load-bearing;
    /// R/W=0 (:1019), RS selects data vs control (:1020-1023).
    /// `now` = `m_cpu->total_cycles()` (mu2000.cpp:1017). The panel-hold
    /// branch (:1031-1032) is native-engine plumbing, omitted (module doc).
    pub fn lcd_port_w(&mut self, now: u64, data: u16) {
        self.set_now(now); // :1017
        if bit(self.m_pe, 4) && !bit(data, 4) {
            // :1018 E falling edge
            if !bit(data, 0) {
                // :1019 R/W = write
                if bit(data, 2) {
                    self.data_w((data >> 8) as u8); // :1021
                } else {
                    self.control_w((data >> 8) as u8); // :1023
                }
            }
        }
        self.m_pe = data; // :1035
    }

    /// origin: mu2000.cpp:1004-1013 `lcd_port_r()` — the Port E read
    /// delegate (wiring: it is the `read_pins` closure for the port.rs
    /// `dr_r` seam, bound via `read_porte` mu2000.cpp:1123). With /E held
    /// high and R/W=1, returns the byte RS selects, in bits 8-15 —
    /// **busy reaches the CPU as Port E data bit 15** (control_r <<8,
    /// mu2000.cpp:1009 + hd44780.cpp:187). E high & R/W=0 → 0 (:1010);
    /// E low → 0 (:1012).
    pub fn lcd_port_r(&mut self, now: u64) -> u16 {
        self.set_now(now); // :1006
        if bit(self.m_pe, 4) {
            // :1007
            if bit(self.m_pe, 0) {
                // :1008
                return u16::from(if bit(self.m_pe, 2) {
                    self.data_r() // :1009 RS=1
                } else {
                    self.control_r() // :1009 RS=0
                }) << 8;
            }
            return 0x0000; // :1010
        }
        0 // :1012
    }

    /// origin: hd44780.cpp:273-296 `state(state_io &s)` (M5-W3a). The glyph
    /// pictures `m_cgrom` are ROM — NOT saved (cpp:272 comment); the host
    /// re-loads them. v<11/12 fallback legs (cpp:283-284/289-290) rebuild
    /// `m_fw`/`m_cg_fw` from the live RAM and zero the owned bitmaps,
    /// exactly like the disk for snapshots that predate 6.188/6.190.
    pub fn state(&mut self, s: &mut StateIo) {
        s.tag("lcd"); // :275
        s.v(&mut self.m_now); // :276 u64
        s.v(&mut self.m_busy_until); // :276 u64
        s.arr(&mut self.m_render_buf); // :277 RENDER_SIZE=1280 raw bytes
        s.arr(&mut self.m_ddram); // :277 0x80 raw bytes
        s.arr(&mut self.m_cgram); // :277 0x40 raw bytes
        // :278-279 **native の持ち物**（版 11 から。6.188）— absent in old
        // records: 「誰も持っていない」(owned=0) restore leg below.
        if s.version() >= 11 {
            // :280
            s.arr(&mut self.m_fw); // :281
            s.v(&mut self.m_owned[0]); // :281 u64
            s.v(&mut self.m_owned[1]); // :281 u64
        } else {
            self.m_fw.copy_from_slice(&self.m_ddram); // :283 memcpy(m_fw, m_ddram)
            self.m_owned[0] = 0; // :284
            self.m_owned[1] = 0; // :284
        }
        if s.version() >= 12 {
            // :286
            s.arr(&mut self.m_cg_fw); // :287
            s.v(&mut self.m_cg_owned); // :287 u64
        } else {
            self.m_cg_fw.copy_from_slice(&self.m_cgram); // :289 memcpy(m_cg_fw, m_cgram)
            self.m_cg_owned = 0; // :290
        }
        s.v(&mut self.m_ac); // :292 int == i32 (hd44780.h:127)
        s.v(&mut self.m_active_ram); // :292 int
        s.v(&mut self.m_direction); // :292 int
        s.v(&mut self.m_disp_shift); // :292 int
        s.v(&mut self.m_num_line); // :293 int
        s.v(&mut self.m_char_size); // :293 int
        s.v(&mut self.m_data_len); // :293 int
        s.v(&mut self.m_shift_on); // :294 bool = 1 byte
        s.v(&mut self.m_display_on); // :294 bool
        s.v(&mut self.m_cursor_on); // :294 bool
        s.v(&mut self.m_blink_on); // :294 bool
        s.v(&mut self.m_nibble); // :295 bool
        s.v(&mut self.m_ir); // :295 u8
        s.v(&mut self.m_dr); // :295 u8
    }
}

/// std-less `memcpy(&mut dst, &src)` for equal-size arrays
/// (std::memcpy calls at hd44780.cpp:29-30/172-174/242/263).
trait MemcpyBytes<const N: usize> {
    fn memcpy_bytes(&mut self, src: &[u8; N]);
}
impl<const N: usize> MemcpyBytes<N> for [u8; N] {
    fn memcpy_bytes(&mut self, src: &[u8; N]) {
        self.copy_from_slice(src);
    }
}
