//! Device layer — transliteration of `src/mame/cpu/sh2.cpp` (+ `sh2.h`) device-level
//! logic. The core row already folded the per-instruction device pieces into
//! `Sh2Core` and they STAY there (single home = no drift): the masked bus accessors
//! (sh2.cpp:89-152), `LDCMSR`/`LDCSR`/`RTE`/`TRAPA`/`ILLEGAL` (:181-250),
//! `check_pending_irq` (:154-178), `sh2_exception`(:347-379)/`sh2_exception_internal`
//! (:381-401), the `execute_run` loop with hook ordering (:252-291), and
//! `device_reset` (:63-84, `Sh2Core::reset`). This module adds what sh2_device owns
//! in C++: the ctor fields, `device_start`, the input-line state machine
//! (`execute_set_input`), the jit-off stubs, and the bus seam the sh7042/wiring rows
//! will plug into.
//!
//! DESIGN (recorded per ledger Invariant 2):
//! - Ownership: C++ `sh2_device` shares one `internal_sh2_state` with the execution
//!   side via `m_sh2_state`; here `Sh2Device` OWNS the single `Sh2Core` — same
//!   single-source-of-truth shape, no second copy of any register.
//! - Bus seam: GENERIC `B: Sh2Bus` passed per call (like the core), NOT a stored
//!   `dyn` object — bit-exact math must not depend on dyn layout, and the C++
//!   `mem_bus` ref is likewise supplied by the assembler (mu2000 glue). The later
//!   `sh7042` row implements `Sh2Bus` on the real map; tests use a fake vector bus.
//! - CRITICAL anti-double-mask trap: `Sh2Core::read_*/write_*` ARE the C++
//!   `sh2_device::read_*/write_*` (sh2.cpp:89-152). Device methods MUST delegate to
//!   them and MUST NOT re-apply `& m_am`, or the 0x40000000 boundary logic breaks.

use crate::core::{InstructionHook, Sh2Bus, Sh2Core};

// origin: src/compat/mamecompat.h:566 (INPUT_LINE_NMI = -1, INPUT_LINE_IRQ0 = 0)
pub const INPUT_LINE_NMI: i32 = -1;
pub const INPUT_LINE_IRQ0: i32 = 0;
// origin: src/compat/mamecompat.h:622 (line_state: CLEAR_LINE = 0, ASSERT_LINE, HOLD_LINE)
pub const CLEAR_LINE: i8 = 0;
pub const ASSERT_LINE: i8 = 1;
// origin: src/mame/cpu/sh2.cpp:27 (constexpr int SH2_INT_15 = 15)
pub const SH2_INT_15: i32 = 15;

/// JIT is never enabled in this port (ledger row note: "jit hooks → const
/// `use_jit=false` path"). The C++ call sites sh2.cpp:261-270 and the `jit_run`
/// continue at :265-266 are therefore statically not-taken in `Sh2Core::execute_run`.
// origin: src/mame/cpu/sh2.cpp:261 (const bool use_jit = jit_enabled();)
pub const USE_JIT: bool = false;

/// Device-side line/aux state (sh2.h members NOT already mirrored in `Sh2Core`:
/// 65-67/90-91 live on the core because the ported loop reads them there).
pub struct Sh2Device {
    pub core: Sh2Core,
    // origin: src/mame/cpu/sh2.cpp:33 (set_clock(clock)) — clock is kept for the
    // later timers/wiring rows; no wall clock is ever read here (Invariant 5).
    pub clock: u32,
    // origin: src/mame/cpu/sh2.cpp:34 (m_cpu_type = cpu_type)
    pub cpu_type: i32,
    // origin: src/mame/cpu/sh.h:466 (m_cache_dirty — cache-flush flag; DRC/cache is
    // not ported, so it only ever records staleness; set at device_reset :83)
    pub m_cache_dirty: bool,
}

impl Sh2Device {
    /// Every field explicitly initialized (ledger Invariant 3 — no Default).
    // origin: src/mame/cpu/sh2.cpp:29-36 (ctor: set_clock, m_cpu_type, m_am; the
    // S-MU2000 comment at :31-32 says the address-space shape is built by the
    // assembler via set_program_bus — mirrored here by the generic bus parameter)
    pub fn new(clock: u32, cpu_type: i32, address_mask: u32) -> Self {
        Sh2Device {
            core: Sh2Core::new(address_mask), // sh2.cpp:35 (m_am = address_mask)
            clock,                            // sh2.cpp:33
            cpu_type,                         // sh2.cpp:34
            m_cache_dirty: false, // no in-class init in C++; device_start leaves it,
                                  // device_reset (:83) sets true — explicit here
        }
    }

    /// Device start. The C++ body is: sh_common_execution::device_start() (state
    /// zero-fill — done in `Sh2Core::new`, core.rs:143-177), save_item registrations
    /// (:50-54 — deferred to the M5 state row), STATE_GENPC debug entries (:56-57 —
    /// debugger-only, this port has none), then `m_nmi_line_state = 0` (:59).
    /// `sh2_device::state(state_io&)` (sh2.cpp:407-413) intentionally NOT ported
    /// here — owned by the M5 `state serializer` row.
    // origin: src/mame/cpu/sh2.cpp:42-61
    // deferred: src/mame/cpu/sh2.cpp:407-413 -> M5 state.rs
    pub fn device_start(&mut self) {
        // origin: src/mame/cpu/sh2.cpp:59
        self.core.m_nmi_line_state = 0;
    }

    /// Device reset. `jit_flush()` (sh2.cpp:65) is a DRC no-op in this port
    /// (row note: "jit_flush call sites become cache-invalidate no-ops in v1");
    /// the whole core-register init sequence incl. `sr = SH_I`, `pc = read_long(0)`,
    /// `r[15] = read_long(4)` lives in `Sh2Core::reset` (origin sh2.cpp:66-82) and
    /// is delegated to verbatim — call order preserved.
    // origin: src/mame/cpu/sh2.cpp:63-84
    pub fn device_reset<B: Sh2Bus>(&mut self, bus: &mut B) {
        Self::jit_flush(); // sh2.cpp:65 — no-op (USE_JIT == false)
        self.core.reset(bus); // sh2.cpp:66-82
        // sh2.cpp:80-82 (m_test_irq/m_cpu_off/m_internal_irq_vector = 0) are done
        // inside core.reset in the same positions (core.rs:275-277).
        self.m_cache_dirty = true; // sh2.cpp:83
    }

    /// Run until icount exhausts (≥1 instruction always runs). Hook ordering lives
    /// in `Sh2Core::execute_run`/`step` (fetch @:272 → delay-slot apply @:274-280 →
    /// execute_one @:282 → deferred IRQ test @:284-288 → icount-- @:289), fired at
    /// the `debugger_instruction_hook(pc)` site (:268) — unchanged by this row.
    // origin: src/mame/cpu/sh2.cpp:252-291 (use_jit==false :261-266 folded const)
    pub fn execute_run<B: Sh2Bus, H: InstructionHook>(&mut self, bus: &mut B, hook: &mut H) {
        self.core.execute_run(bus, hook);
    }

    /// Input-line state machine — the one device behavior NOT already on the core.
    /// NMI is edge-triggered only: `execute_input_edge_triggered` returns true for
    /// INPUT_LINE_NMI (sh2.h:52), and both branches bail on unchanged level (:301-302,
    /// :324-325). While a delay slot is armed (`m_delay != 0`) the test is deferred
    /// via `m_test_irq = 1` (:316-317, :339-340), exactly like the run-loop test at
    /// :284-288. LOG() is compiled out (//#define VERBOSE, sh2.cpp:24).
    ///
    /// Line index domain: NMI = -1 (`m_nmi_line_state`, sh2.h:67); IRQ0..16 map to
    /// `m_irq_line_state[0..17]` (sh2.h:91). `pending_irq`/`pending_nmi` live on the
    /// core (sh.h internal_sh2_state).
    // origin: src/mame/cpu/sh2.cpp:297-345 (execute_set_input)
    pub fn execute_set_input<B: Sh2Bus>(&mut self, bus: &mut B, irqline: i32, state: i8) {
        if irqline == INPUT_LINE_NMI {
            // sh2.cpp:299
            if self.core.m_nmi_line_state == state {
                // sh2.cpp:301-302
                return;
            }
            self.core.m_nmi_line_state = state; // sh2.cpp:304
            if state == CLEAR_LINE {
                // sh2.cpp:306-309 (LOG only — no state change on clear)
            } else {
                // sh2.cpp:311-320
                self.core.pending_nmi = 1; // sh2.cpp:314
                if self.core.m_delay != 0 {
                    self.core.m_test_irq = 1; // sh2.cpp:316-317
                } else {
                    self.core.check_pending_irq(bus); // sh2.cpp:319
                }
            }
        } else {
            // sh2.cpp:322
            let idx = irqline as usize; // irqline >= 0 here (INPUT_LINE_IRQ0..)
            if self.core.m_irq_line_state[idx] == state {
                // sh2.cpp:324-325
                return;
            }
            self.core.m_irq_line_state[idx] = state; // sh2.cpp:327
            if state == CLEAR_LINE {
                // sh2.cpp:329-333
                self.core.pending_irq &= !(1u32 << irqline as u32); // sh2.cpp:332
            } else {
                // sh2.cpp:334-343
                self.core.pending_irq |= 1u32 << irqline as u32; // sh2.cpp:337
                if self.core.m_delay != 0 {
                    self.core.m_test_irq = 1; // sh2.cpp:339-340
                } else {
                    self.core.check_pending_irq(bus); // sh2.cpp:342
                }
            }
        }
    }

    // ---- jit stubs (all DRC, never live: row decision const use_jit=false) ----

    // origin: src/mame/cpu/sh2.h:96 (static bool jit_enabled()) — const path
    #[inline(always)]
    pub fn jit_enabled() -> bool {
        USE_JIT // sh2.cpp:261
    }

    // origin: src/mame/cpu/sh2.h:99 (static bool jit_trace_on()) — DRC-only
    #[inline(always)]
    pub fn jit_trace_on() -> bool {
        false
    }

    /// Cache/JIT invalidate — a no-op in v1 (row note). The C++ `jit_flush` runs at
    /// device_reset (sh2.cpp:65); the device records the invalidate as
    /// `m_cache_dirty = true` there instead (sh.h:466 flag, set at :83).
    // origin: src/mame/cpu/sh2.h:108 (void jit_flush())
    #[inline(always)]
    pub fn jit_flush() {}

    // origin: src/mame/cpu/sh2.h:49-51 (execute_min/max_cycles, default irq vector)
    #[inline(always)]
    pub fn execute_min_cycles() -> u32 {
        1
    }
    #[inline(always)]
    pub fn execute_max_cycles() -> u32 {
        4
    }
    #[inline(always)]
    pub fn execute_default_irq_vector(_inputnum: u32) -> u32 {
        0
    }

    // origin: src/mame/cpu/sh2.h:52 (execute_input_edge_triggered — NMI only)
    #[inline(always)]
    pub fn execute_input_edge_triggered(inputnum: i32) -> bool {
        inputnum == INPUT_LINE_NMI
    }

    // origin: src/mame/cpu/sh2.h:34 (set_frt_input — base class does nothing)
    #[inline(always)]
    pub fn set_frt_input(&mut self, _state: i32) {}

    // ---- masked bus access: DELEGATION ONLY (see anti-double-mask note above) ----
    // These are the C++ sh2_device::read_byte/read_word/read_long/write_* virtuals
    // (sh2.h:70-76, sh2.cpp:89-152) already transliterated on the core; kept here
    // as the device-level API the core mem-trait plugs into / tests drive.

    // origin: src/mame/cpu/sh2.cpp:89-95 (via core.rs:285)
    #[inline(always)]
    pub fn read_byte<B: Sh2Bus>(&self, bus: &mut B, offset: u32) -> u8 {
        self.core.read_byte(bus, offset)
    }
    // origin: src/mame/cpu/sh2.cpp:97-103 (via core.rs:295)
    #[inline(always)]
    pub fn read_word<B: Sh2Bus>(&self, bus: &mut B, offset: u32) -> u16 {
        self.core.read_word(bus, offset)
    }
    // origin: src/mame/cpu/sh2.cpp:105-114 (via core.rs:305)
    #[inline(always)]
    pub fn read_long<B: Sh2Bus>(&self, bus: &mut B, offset: u32) -> u32 {
        self.core.read_long(bus, offset)
    }
    // origin: src/mame/cpu/sh2.cpp:121-129 (via core.rs:315)
    #[inline(always)]
    pub fn write_byte<B: Sh2Bus>(&mut self, bus: &mut B, offset: u32, data: u8) {
        self.core.write_byte(bus, offset, data)
    }
    // origin: src/mame/cpu/sh2.cpp:132-141 (via core.rs:325)
    #[inline(always)]
    pub fn write_word<B: Sh2Bus>(&mut self, bus: &mut B, offset: u32, data: u16) {
        self.core.write_word(bus, offset, data)
    }
    // origin: src/mame/cpu/sh2.cpp:143-152 (via core.rs:335)
    #[inline(always)]
    pub fn write_long<B: Sh2Bus>(&mut self, bus: &mut B, offset: u32, data: u32) {
        self.core.write_long(bus, offset, data)
    }
    // origin: src/mame/cpu/sh2.h:73 (decrypted_read_word) — no decryption exists in
    // this port; m_decrypted_program == the program bus (sh.h:460-462), so the
    // UNMASKED fetch is the honest mirror (matches core.rs:346 fetch_word rule).
    #[inline(always)]
    pub fn decrypted_read_word<B: Sh2Bus>(&self, bus: &mut B, offset: u32) -> u16 {
        bus.read_word(offset) // sh2.cpp:118 — straight passthrough, no & m_am
    }
}
