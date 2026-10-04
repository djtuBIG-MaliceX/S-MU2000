//! SH-2 interpreter core — transliteration of `src/mame/cpu/sh.cpp` (live interpreter
//! region, lines 1..1875 plus the `state()` serializer at :1881, ledger row M5-W2;
//! the dead DRC note at :1876 is not
//! ported) plus the SH-2 virtual overrides and the
//! fetch/dispatch loop of `src/mame/cpu/sh2.cpp` that the interpreter needs to run:
//! masked bus wrappers (:89-152), check_pending_irq (:154-178), LDCMSR/LDCSR/RTE/
//! TRAPA/ILLEGAL (:180-245), execute_one_f000 (:247-250), execute_run (:252-291),
//! sh2_exception (:347-379), sh2_exception_internal (:381-401), device_reset (:63-84),
//! and the clock/run helpers of `src/mame/cpu/sh.h` (:164-246).
//!
//! std-only, no external dependencies. Deviations (all justified in the session report):
//! - The x64/arm JIT paths of sh2.cpp (use_jit, :261-266) are const-false here (M9).
//! - BUSY_LOOP_HACKS is 0 (sh.h:36) — the hacked regions inside BRA (sh.cpp:233-243)
//!   and DT (sh.cpp:652-667) are dead code and not ported.
//! - debugger_* hooks expand to no-ops in mamecompat.h (:73-84); the real trace/hash
//!   hook fires at the `debugger_instruction_hook` call site sh2.cpp:268 — reproduced
//!   here as `InstructionHook::instruction` at identical granularity (pre-fetch,
//!   pre-delay-slot, register state before the instruction).
//! - LOG() calls in sh2_exception are no-ops in the ground-truth build
//!   (`//#define VERBOSE 1`, sh2.cpp:24) and are omitted — zero state effect.
//! - C++ signed overflow that is UB on x86 (wraps in practice: DMULS `0 - INT_MIN`,
//!   MAC_W `int32 *=`) is rendered `wrapping_*`; bit-identical on the MinGW baseline.

// origin: src/state.h — layout engine (smu_compat re-export, M5-W1)
use smu_compat::StateIo;

// origin: src/mame/cpu/sh.h:62-66 (Bits in SR)
pub const SH_T: u32 = 0x0000_0001;
pub const SH_S: u32 = 0x0000_0002;
pub const SH_I: u32 = 0x0000_00f0;
pub const SH_Q: u32 = 0x0000_0100;
pub const SH_M: u32 = 0x0000_0200;
// origin: src/mame/cpu/sh.h:68 (SH_FLAGS)
pub const SH_FLAGS: u32 = SH_M | SH_Q | SH_I | SH_S | SH_T;

// origin: src/mame/cpu/sh.h:58-59 (REG_N / REG_M)
#[inline(always)]
pub fn reg_n(opcode: u16) -> usize {
    ((opcode >> 8) & 15) as usize
}
#[inline(always)]
pub fn reg_m(opcode: u16) -> usize {
    ((opcode >> 4) & 15) as usize
}

// origin: MAME util::sext as used throughout sh.cpp — left-shift out the high bits,
// arithmetic right shift back. Rendered with explicit u32 shl (never UB in Rust) to
// dodge the documented `util::sext` high-bit-drop bug class (ledger Pitfalls).
#[inline(always)]
fn sext(v: u32, width: u32) -> i32 {
    ((v << (32 - width)) as i32) >> (32 - width)
}

// origin: BIT(x, n) as used throughout sh.cpp — (x >> n) & 1
#[inline(always)]
fn bit(x: u32, n: u32) -> u32 {
    (x >> n) & 1
}

/// Raw bus seam. The device row wires this to the real mem_bus map; masking of the
/// 0x40000000 boundary lives in `Sh2Core::read_*` (mirroring sh2_device::read_*,
/// sh2.cpp:89-152) so this trait is the unmapped bus, exactly like C++ `mem_bus`.
pub trait Sh2Bus {
    // origin: mem_bus::read_byte / read_word / read_dword (src/compat/membus.h)
    fn read_byte(&mut self, offset: u32) -> u8;
    fn read_word(&mut self, offset: u32) -> u16;
    fn read_long(&mut self, offset: u32) -> u32;
    fn write_byte(&mut self, offset: u32, data: u8);
    fn write_word(&mut self, offset: u32, data: u16);
    fn write_long(&mut self, offset: u32, data: u32);
    /// origin: src/mame/cpu/sh7042.cpp:397-401 — sh7042_device's override of
    /// sh2_exception_internal calls `m_intc->interrupt_taken(irqline, vector)`
    /// AFTER the base exception (pushes + vector fetch + sleep) completes.
    /// The ack re-arbitrates the INTC; internal vectors (e.g. MTU 88) clear
    /// their pending bit there — without this call internal IRQs latch forever.
    /// Default no-op matches the CPU-only test buses.
    fn exception_taken(&mut self, _vector: u32) {}
    /// M9 (2026-10-03): batch-stop predicate. `execute_run` breaks its
    /// do/while after any instruction where this returns true, handing the
    /// machine its per-instruction pump sequence at the same instruction
    /// boundary the 1-instruction loop used. Default false = test buses
    /// (and any bus without device side effects) run the whole chunk,
    /// exactly like the C++ `m_cpu->run_cycles(chunk)` (:1219).
    fn batch_stop(&self) -> bool {
        false
    }
}

/// Per-instruction observation hook. Fired exactly at the C++
/// `debugger_instruction_hook(m_sh2_state->pc)` site (sh2.cpp:268): AFTER the
/// m_cpu_off bail-out, BEFORE the opcode fetch (sh2.cpp:272), BEFORE delay-slot
/// application (sh2.cpp:274-280) and BEFORE execute_one (:282). A later row
/// reproduces mamecompat.h:75-79 inside this callback:
///   g_pc_hash  -> pc_hash(pc, core.regs_hash())
///   g_pc_trace -> g_pc_cycles = core.total_cycles(); pc_trace(pc, &core.regs_text())
/// with a CRLF-owning writer (ledger CRLF pitfall).
pub trait InstructionHook {
    fn instruction(&mut self, core: &Sh2Core);
}

/// A hook that observes nothing (used by unit tests that only want the machine).
pub struct NoHook;
// origin: absence of g_pc_hash/g_pc_trace sinks (both nullptr, boot.cpp:83-97)
impl InstructionHook for NoHook {
    #[inline(always)]
    fn instruction(&mut self, _core: &Sh2Core) {}
}

/// Execution state — mirrors `sh_common_execution::internal_sh2_state`
/// (sh.h:106-159, the POD that sh.cpp:1884 `state()` serializes whole) plus the
/// sh2_device members the ported loop touches (sh2.h:65-67,90-91) and the clock
/// members of sh.h:234-235. The SH3/4 DRC "near" block sh.h:132-158 (m_ppc..m_fpu_pr)
/// is DRC-only and never written by the interpreter path, but sh.cpp:1884 dumps it
/// as part of the POD, so M5-W2 mirrors it here as inert zero-init fields
/// (grep sh*.cpp: the only writers were the deleted DRC half, sh.cpp:1876+).
/// `repr(C)` + C++ field order make offsets 0..424 byte-identical to the
/// g++ ground-truth layout (tests/state.rs golden; sizeof=424, align=8).
/// DEVIATION (names): the struct members sh.h:140/142 collide with the device
/// mirrors already here, so their DRC slots are `m_cpu_off_drc`/`m_test_irq_drc`
/// (always 0 on disk — zero-filled at device_start, no interpreter writer).
#[repr(C)]
pub struct Sh2Core {
    // ---- internal_sh2_state, EXACT C++ order (sh.h:108-158, offs 0..424) ----
    pub pc: u32,          // sh.h:108 (offs 0)
    pub pr: u32,          // :109 (4)
    pub sr: u32,          // :110 (8)
    pub mach: u32,        // :111 (12)
    pub macl: u32,        // :112 (16)
    pub r: [u32; 16],     // :113 (20..84)
    pub ea: u32,          // :114 (84)
    pub pending_irq: u32, // :116 (88)
    pub pending_nmi: u32, // :117 (92)
    pub irqline: i32,     // :118 (96)
    pub evec: u32,        // :119 (100) exception vector for DRC
    pub irqsr: u32,       // :120 (104) IRQ-time old SR for DRC
    // Branch/delay state, exactly as C++ has it: `m_delay` (sh.h:130) is BOTH the
    // in-delay-slot flag (0 = none) and the target PC. BRAF/BSRF/JMP/JSR/RTS/RTE/
    // BRA/BSR/BTS/BFS write it; execute_run consumes it on the NEXT instruction
    // (sh2.cpp:274-278). There are no separate B/S/T delay-slot flags in this file.
    // `target` (sh.h:121) exists only for the DRC front end; never read here.
    pub target: u32,      // :121 (108)
    pub internal_irq_level: i32, // :122 (112)
    pub icount: i32,      // :123 (116)
    pub sleep_mode: u8,   // :124 (120)
    /// mirror of the C++ ABI tail padding after sleep_mode (offs 121..123);
    /// zero at construction (C++ device_start zero-fills the whole POD),
    /// serialized verbatim so the stream is byte-identical to memcpy(424).
    pub pad_sleep: [u8; 3],
    pub arg0: u32,        // :125 (124) print_debug argument 1
    pub arg1: u32,        // :126 (128)
    pub gbr: u32,         // :127 (132)
    pub vbr: u32,         // :128 (136)
    pub m_delay: u32,     // :130 (140)
    // SH3/4 DRC "near" block (sh.h:132-145) — inert here, serialized verbatim
    pub m_ppc: u32,       // :133 (144)
    pub m_spc: u32,       // :134 (148)
    pub m_ssr: u32,       // :135 (152)
    /// sh.h:136 `uint32_t m_rbnk[2][8]` — [2][8] flattened; same 64 LE bytes.
    pub m_rbnk: [u32; 16], // :136 (156..220)
    pub m_sgr: u32,       // :137 (220)
    pub m_fr: [u32; 16],  // :138 (224..288)
    pub m_xf: [u32; 16],  // :139 (288..352)
    pub m_cpu_off_drc: u32, // :140 (352) — see DEVIATION note above
    pub m_pending_irq: u32, // :141 (356)
    pub m_test_irq_drc: u32, // :142 (360) — see DEVIATION note above
    pub m_fpscr: u32,     // :143 (364)
    pub m_fpul: u32,      // :144 (368)
    pub m_dbr: u32,       // :145 (372)
    // FP constants the deleted DRC code referred to by address (sh.h:147-158).
    // Interpreter path: never assigned (grep sh*.cpp — only in-class `= 0`
    // defaults), so exact init value is 0/zero-bytes; still serialized with
    // exact width, round-tripped verbatim.
    pub m_ftrc_dmin: f64, // :148 (376) = 0
    pub m_ftrc_dmax: f64, // :149 (384) = 0
    pub m_ftrc_smin: f32, // :150 (392) = 0
    pub m_ftrc_smax: f32, // :151 (396) = 0
    pub m_fzero: f32,     // :152 (400) = 0
    pub m_fone: f32,      // :153 (404) = 0
    pub m_fpmode: [u8; 4], // :154 (408) = {}
    pub m_frt_input: i32, // :156 (412) = 0
    pub m_fpu_sz: i32,    // :157 (416) = 0
    pub m_fpu_pr: i32,    // :158 (420) = 0
    // ---- sh2_device / sh_common_execution members used by the ported path ----
    // (NOT part of the 424-byte POD dump above)
    pub m_am: u32,               // address mask (sh.h:482, set from ctor sh2.cpp:35)
    pub m_test_irq: u32,         // sh2.h:65 (device-level — sh2.cpp:411 slot)
    pub m_internal_irq_vector: i32, // sh2.h:66
    pub m_nmi_line_state: i8,    // sh2.h:67
    pub m_cpu_off: u32,          // sh2.h:90 (device-level — sh2.cpp:412 slot)
    pub m_irq_line_state: [i8; 17], // sh2.h:91 — written by device row (execute_set_input)
    pub m_total_cycles: u64,     // sh.h:234
    pub m_cycles_this_run: i32,  // sh.h:235
    pub m_pcfsel: i32,           // sh.h:452 — serialized by state() sh.cpp:1885
}

impl Sh2Core {
    /// Every field explicitly initialized (ledger Invariant 3 — no Default).
    /// Mirrors sh_common_execution::device_start (sh.cpp:45-63, which zeroes all of
    /// internal_sh2_state incl. arg0 and the DRC block; arg1 = 0 comes from the
    /// sh.h:126 in-class default) plus the sh2_device in-class defaults
    /// (sh2.h:65-67,90-91) and the ctor's m_am store (sh2.cpp:35). NOTE: C++
    /// device_reset (sh2.cpp:63-84) then re-initializes and sets
    /// internal_irq_level = -1 and sr = SH_I — call `reset()` for the machine's
    /// post-reset state. The POD defaults below are EXACTLY the in-class `= 0` /
    /// `= {}` values of sh.h:108-158 (all the FP constants too — grep shows the
    /// only assignments lived in the deleted DRC half).
    pub fn new(am: u32) -> Self {
        // origin: src/mame/cpu/sh.cpp:45-63 (device_start zero-fill)
        Sh2Core {
            pc: 0,          // sh.h:108
            pr: 0,          // :109
            sr: 0,          // :110
            mach: 0,        // :111
            macl: 0,        // :112
            r: [0; 16],     // :113
            ea: 0,          // :114
            pending_irq: 0, // :116
            pending_nmi: 0, // :117
            irqline: 0,     // :118
            evec: 0,        // :119
            irqsr: 0,       // :120
            target: 0,      // :121
            internal_irq_level: 0, // :122
            icount: 0,      // :123
            sleep_mode: 0,  // :124
            pad_sleep: [0; 3], // padding mirror (C++ device_start zero-fills)
            arg0: 0,        // :125
            arg1: 0,        // :126
            gbr: 0,         // :127
            vbr: 0,         // :128
            m_delay: 0,     // :130
            m_ppc: 0,       // :133
            m_spc: 0,       // :134
            m_ssr: 0,       // :135
            m_rbnk: [0; 16], // :136
            m_sgr: 0,       // :137
            m_fr: [0; 16],  // :138
            m_xf: [0; 16],  // :139
            m_cpu_off_drc: 0,   // :140
            m_pending_irq: 0,   // :141
            m_test_irq_drc: 0,  // :142
            m_fpscr: 0,     // :143
            m_fpul: 0,      // :144
            m_dbr: 0,       // :145
            m_ftrc_dmin: 0.0, // :148 (= 0)
            m_ftrc_dmax: 0.0, // :149
            m_ftrc_smin: 0.0, // :150
            m_ftrc_smax: 0.0, // :151
            m_fzero: 0.0,   // :152
            m_fone: 0.0,    // :153
            m_fpmode: [0; 4], // :154 (= {})
            m_frt_input: 0, // :156
            m_fpu_sz: 0,    // :157
            m_fpu_pr: 0,    // :158
            m_am: am,
            m_test_irq: 0,
            m_internal_irq_vector: 0,
            m_nmi_line_state: 0,
            m_cpu_off: 0,
            m_irq_line_state: [0; 17],
            m_total_cycles: 0,
            m_cycles_this_run: 0,
            m_pcfsel: 0,
        }
    }

    // origin: src/mame/cpu/sh.h:164 (pc()) and :166 (pr())
    #[inline(always)]
    pub fn pc(&self) -> u32 {
        self.pc
    }
    #[inline(always)]
    pub fn pr(&self) -> u32 {
        self.pr
    }

    // origin: src/mame/cpu/sh.h:169-179 (regs_hash) — u64 multiply wraps mod 2^64
    pub fn regs_hash(&self) -> u64 {
        let mut h: u64 = 0;
        for i in 0..16 {
            h = h.wrapping_mul(1_000_003) ^ (self.r[i] as u64);
        }
        h = h.wrapping_mul(1_000_003) ^ (self.sr as u64);
        h = h.wrapping_mul(1_000_003) ^ (self.pr as u64);
        h = h.wrapping_mul(1_000_003) ^ (self.gbr as u64);
        h = h.wrapping_mul(1_000_003) ^ (self.mach as u64);
        h = h.wrapping_mul(1_000_003) ^ (self.macl as u64);
        h
    }

    // origin: src/mame/cpu/sh.h:183-192 (regs_text) — 16x" %08X" then the SR/PR/
    // MACH/MACL tail. 196 chars total, buffer is 256: no truncation, so a Rust
    // String is byte-identical to the C++ static buffer (deviation: return type).
    pub fn regs_text(&self) -> String {
        let mut s = String::with_capacity(256);
        for i in 0..16 {
            s.push_str(&format!(" {:08X}", self.r[i]));
        }
        s.push_str(&format!(
            " SR={:08X} PR={:08X} MACH={:08X} MACL={:08X}",
            self.sr, self.pr, self.mach, self.macl
        ));
        s
    }

    // origin: src/mame/cpu/sh.h:226 (skip_cycles)
    pub fn skip_cycles(&mut self, n: u64) {
        self.m_total_cycles += n;
    }

    // origin: src/mame/cpu/sh.h:229-232 (total_cycles) — wrapping: same mod-2^32/64
    // math C++ does at -O3 (run_cycles(0) executes one instruction and can drive
    // icount negative; the mu2000 glue never asks for 0, but do not panic either).
    // sh.h:231 computes in `int` and converts the INT result to u64 (sign-extends),
    // so the Rust widening is `as i64 as u64`, NOT `as u32 as u64` (P6 2026-10-02).
    pub fn total_cycles(&self) -> u64 {
        self.m_total_cycles.wrapping_add((self.m_cycles_this_run.wrapping_sub(self.icount)) as i64 as u64)
    }

    // origin: src/mame/cpu/sh.h:205-215 (run_cycles)
    pub fn run_cycles<B: Sh2Bus, H: InstructionHook>(&mut self, bus: &mut B, hook: &mut H, cycles: i32) -> i32 {
        self.icount = cycles;
        self.m_cycles_this_run = cycles;
        self.execute_run(bus, hook);
        let done = self.m_cycles_this_run.wrapping_sub(self.icount);
        self.m_total_cycles += done as i64 as u64; // sh.h:210-211 int done sign-extends into u64
        self.m_cycles_this_run = 0;
        self.icount = 0;
        done
    }

    // origin: src/mame/cpu/sh.h:219-223 (abort_timeslice)
    pub fn abort_timeslice(&mut self) {
        self.m_cycles_this_run = self.m_cycles_this_run.wrapping_sub(self.icount);
        self.icount = 0;
    }

    // origin: src/mame/cpu/sh2.cpp:63-84 (device_reset). jit_flush() (:65) is a DRC
    // no-op in this port (M9); m_cache_dirty=true stays a device-row field.
    pub fn reset<B: Sh2Bus>(&mut self, bus: &mut B) {
        for i in 0..16 {
            self.r[i] = 0;
        }
        for i in 0..17 {
            self.m_irq_line_state[i] = 0;
        }
        self.pc = 0;
        self.pr = 0;
        self.sr = 0;
        self.gbr = 0;
        self.vbr = 0;
        self.mach = 0;
        self.macl = 0;
        self.evec = 0;
        self.irqsr = 0;
        self.ea = 0;
        self.m_delay = 0;
        self.pending_irq = 0;
        self.pending_nmi = 0;
        self.sleep_mode = 0;
        self.internal_irq_level = -1;
        self.sr = SH_I;
        self.pc = self.read_long(bus, 0);
        self.r[15] = self.read_long(bus, 4);
        self.m_test_irq = 0;
        self.m_cpu_off = 0;
        self.m_internal_irq_vector = 0;
    }

    /// origin: src/mame/cpu/sh.cpp:1881-1890 (`sh_common_execution::state`).
    /// C++ dumps the POD whole — `s.v(*m_sh2_state)` (:1884) is one memcpy of
    /// `sizeof(internal_sh2_state)` = 424 bytes (g++ -std=c++20 -O3 ground-truth
    /// harness, tests/state.rs golden). Mirrored field-by-field in C++ offset
    /// order, same widths; the ONLY non-field bytes are `pad_sleep` (offs
    /// 121..123, the tail pad after `sleep_mode`), serialized verbatim like the
    /// memcpy carries it. Tag first (:1883); the three tail scalars follow in
    /// disk order :1885/:1888/:1889 (pcfsel i32, total_cycles u64,
    /// cycles_this_run i32). Wire total: 8 + 424 + 4 + 8 + 4 = 448 bytes.
    pub fn state(&mut self, s: &mut StateIo) {
        s.tag("shcore");                    // sh.cpp:1883
        s.v(&mut self.pc);                  // sh.h:108 (0)
        s.v(&mut self.pr);                  // :109 (4)
        s.v(&mut self.sr);                  // :110 (8)
        s.v(&mut self.mach);                // :111 (12)
        s.v(&mut self.macl);                // :112 (16)
        s.arr(&mut self.r);                 // :113 (20..84) 16 x u32
        s.v(&mut self.ea);                  // :114 (84)
        s.v(&mut self.pending_irq);         // :116 (88)
        s.v(&mut self.pending_nmi);         // :117 (92)
        s.v(&mut self.irqline);             // :118 (96) int32
        s.v(&mut self.evec);                // :119 (100)
        s.v(&mut self.irqsr);               // :120 (104)
        s.v(&mut self.target);              // :121 (108)
        s.v(&mut self.internal_irq_level);  // :122 (112) int
        s.v(&mut self.icount);              // :123 (116) int
        s.v(&mut self.sleep_mode);          // :124 (120) uint8
        s.raw(&mut self.pad_sleep);         // (121..123) memcpy'd tail pad
        s.v(&mut self.arg0);                // :125 (124)
        s.v(&mut self.arg1);                // :126 (128)
        s.v(&mut self.gbr);                 // :127 (132)
        s.v(&mut self.vbr);                 // :128 (136)
        s.v(&mut self.m_delay);             // :130 (140)
        s.v(&mut self.m_ppc);               // :133 (144)
        s.v(&mut self.m_spc);               // :134 (148)
        s.v(&mut self.m_ssr);               // :135 (152)
        s.arr(&mut self.m_rbnk);            // :136 (156..220) [2][8] flat = 64B
        s.v(&mut self.m_sgr);               // :137 (220)
        s.arr(&mut self.m_fr);              // :138 (224..288)
        s.arr(&mut self.m_xf);              // :139 (288..352)
        s.v(&mut self.m_cpu_off_drc);       // :140 (352)
        s.v(&mut self.m_pending_irq);       // :141 (356)
        s.v(&mut self.m_test_irq_drc);      // :142 (360)
        s.v(&mut self.m_fpscr);             // :143 (364)
        s.v(&mut self.m_fpul);              // :144 (368)
        s.v(&mut self.m_dbr);               // :145 (372)
        s.v(&mut self.m_ftrc_dmin);         // :148 (376) double
        s.v(&mut self.m_ftrc_dmax);         // :149 (384) double
        s.v(&mut self.m_ftrc_smin);         // :150 (392) float
        s.v(&mut self.m_ftrc_smax);         // :151 (396) float
        s.v(&mut self.m_fzero);             // :152 (400) float
        s.v(&mut self.m_fone);              // :153 (404) float
        s.arr(&mut self.m_fpmode);          // :154 (408) uint8 x4
        s.v(&mut self.m_frt_input);         // :156 (412) int
        s.v(&mut self.m_fpu_sz);            // :157 (416) int
        s.v(&mut self.m_fpu_pr);            // :158 (420) int
        s.v(&mut self.m_pcfsel);            // sh.cpp:1885 (sh.h:452 int)
        // sh.cpp:1886-1887: peripherals read m_total_cycles as "now" — the
        // timers' schedules die without this field (the :1888 comment).
        s.v(&mut self.m_total_cycles);      // sh.cpp:1888 (sh.h:234 u64)
        s.v(&mut self.m_cycles_this_run);   // sh.cpp:1889 (sh.h:235 int)
    }

    // ---- masked bus access (origin: src/mame/cpu/sh2.cpp:89-152) ----
    // The 0x40000000 cache-through window boundary is mirrored exactly.

    // origin: src/mame/cpu/sh2.cpp:89-95
    #[inline(always)]
    pub fn read_byte<B: Sh2Bus>(&self, bus: &mut B, offset: u32) -> u8 {
        if offset < 0x4000_0000 {
            bus.read_byte(offset & self.m_am)
        } else {
            bus.read_byte(offset)
        }
    }

    // origin: src/mame/cpu/sh2.cpp:97-103
    #[inline(always)]
    pub fn read_word<B: Sh2Bus>(&self, bus: &mut B, offset: u32) -> u16 {
        if offset < 0x4000_0000 {
            bus.read_word(offset & self.m_am)
        } else {
            bus.read_word(offset)
        }
    }

    // origin: src/mame/cpu/sh2.cpp:105-114 (read_dword on the C++ bus == read_long here)
    #[inline(always)]
    pub fn read_long<B: Sh2Bus>(&self, bus: &mut B, offset: u32) -> u32 {
        if offset < 0x4000_0000 {
            bus.read_long(offset & self.m_am)
        } else {
            bus.read_long(offset)
        }
    }

    // origin: src/mame/cpu/sh2.cpp:121-129
    #[inline(always)]
    pub fn write_byte<B: Sh2Bus>(&mut self, bus: &mut B, offset: u32, data: u8) {
        if offset < 0x4000_0000 {
            bus.write_byte(offset & self.m_am, data);
            return;
        }
        bus.write_byte(offset, data);
    }

    // origin: src/mame/cpu/sh2.cpp:132-141
    #[inline(always)]
    pub fn write_word<B: Sh2Bus>(&mut self, bus: &mut B, offset: u32, data: u16) {
        if offset < 0x4000_0000 {
            bus.write_word(offset & self.m_am, data);
            return;
        }
        bus.write_word(offset, data);
    }

    // origin: src/mame/cpu/sh2.cpp:143-152
    #[inline(always)]
    pub fn write_long<B: Sh2Bus>(&mut self, bus: &mut B, offset: u32, data: u32) {
        if offset < 0x4000_0000 {
            bus.write_long(offset & self.m_am, data);
            return;
        }
        bus.write_long(offset, data);
    }

    // origin: src/mame/cpu/sh2.cpp:272 — fetch through m_decrypted_program (== the
    // program bus here, sh.h:460-462) with the same inline mask rule.
    #[inline(always)]
    fn fetch_word<B: Sh2Bus>(&self, bus: &mut B, pc: u32) -> u16 {
        if pc >= 0x4000_0000 {
            bus.read_word(pc)
        } else {
            bus.read_word(pc & self.m_am)
        }
    }

    // =====================================================================
    // Opcode helpers — transliterated region-for-region from sh.cpp
    // =====================================================================

    /*  code                 cycles  t-bit
     *  0011 nnnn mmmm 1100  1       -
     *  ADD     Rm,Rn
     */
    // origin: src/mame/cpu/sh.cpp:108-111
    fn add(&mut self, m: usize, n: usize) {
        self.r[n] = self.r[n].wrapping_add(self.r[m]);
    }

    /*  0111 nnnn iiii iiii  1  ADD #imm,Rn */
    // origin: src/mame/cpu/sh.cpp:117-120
    fn addi(&mut self, i: u32, n: usize) {
        self.r[n] = self.r[n].wrapping_add(sext(i, 8) as u32);
    }

    /*  0011 nnnn mmmm 1110  1  ADDC Rm,Rn (carry) */
    // origin: src/mame/cpu/sh.cpp:126-139 — verbatim, incl. the two-step carry
    // (this is NOT the textbook addc; MAME's specific tmp0/tmp1 compare order)
    fn addc(&mut self, m: usize, n: usize) {
        let tmp1 = self.r[n].wrapping_add(self.r[m]);
        let tmp0 = self.r[n];
        self.r[n] = tmp1.wrapping_add(self.sr & SH_T);
        if tmp0 > tmp1 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
        if tmp1 > self.r[n] {
            self.sr |= SH_T;
        }
    }

    /*  0011 nnnn mmmm 1111  1  ADDV Rm,Rn (overflow) */
    // origin: src/mame/cpu/sh.cpp:145-165. NOTE: the `if (ans == 1)...else` has NO
    // braces in C++ — the else binds to `if (ans == 1)` (disk :158-161); when
    // src != 1, T is ALWAYS written (else clears). src/ans are sums of two 0/1 bits.
    fn addv(&mut self, m: usize, n: usize) {
        let dest = bit(self.r[n], 31) as i32;
        let mut src = bit(self.r[m], 31) as i32;
        src += dest;

        self.r[n] = self.r[n].wrapping_add(self.r[m]);

        let mut ans = bit(self.r[n], 31) as i32;
        ans += dest;

        if src != 1 {
            if ans == 1 {
                self.sr |= SH_T;
            } else {
                self.sr &= !SH_T;
            }
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  0010 nnnn mmmm 1001  1  AND Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:171-174
    fn and(&mut self, m: usize, n: usize) {
        self.r[n] &= self.r[m];
    }

    /*  1100 1001 iiii iiii  1  AND #imm,R0 */
    // origin: src/mame/cpu/sh.cpp:180-183
    fn andi(&mut self, i: u32) {
        self.r[0] &= i;
    }

    /*  1100 1101 iiii iiii  1  AND.B #imm,@(R0,GBR) */
    // origin: src/mame/cpu/sh.cpp:189-195
    fn andm<B: Sh2Bus>(&mut self, bus: &mut B, i: u32) {
        self.ea = self.gbr.wrapping_add(self.r[0]);
        let temp = i & (self.read_byte(bus, self.ea) as u32);
        self.write_byte(bus, self.ea, temp as u8);
        self.icount = self.icount.wrapping_sub(2);
    }

    /*  1000 1011 dddd dddd  3/1  BF disp8 */
    // origin: src/mame/cpu/sh.cpp:201-209 — BF/BT take the branch IMMEDIATELY
    // (direct pc update, NO delay slot); only the S variants (BFS/BTS) use m_delay
    fn bf(&mut self, d: u32) {
        if (self.sr & SH_T) == 0 {
            let disp = sext(d, 8);
            let t = self.pc.wrapping_add((disp.wrapping_mul(2)) as u32).wrapping_add(2);
            self.ea = t;
            self.pc = t;
            self.icount = self.icount.wrapping_sub(2);
        }
    }

    /*  1000 1111 dddd dddd  3/1  BFS disp8 */
    // origin: src/mame/cpu/sh.cpp:215-223
    fn bfs(&mut self, d: u32) {
        if (self.sr & SH_T) == 0 {
            let disp = sext(d, 8);
            let t = self.pc.wrapping_add((disp.wrapping_mul(2)) as u32).wrapping_add(2);
            self.ea = t;
            self.m_delay = t;
            self.icount = self.icount.wrapping_sub(1);
        }
    }

    /*  1010 dddd dddd dddd  2  BRA disp12 */
    // origin: src/mame/cpu/sh.cpp:229-246 — BUSY_LOOP_HACKS==0 (sh.h:36): the
    // :233-243 hack block is dead code, not ported
    fn bra(&mut self, d: u32) {
        let disp = sext(d, 12);
        let t = self.pc.wrapping_add((disp.wrapping_mul(2)) as u32).wrapping_add(2);
        self.ea = t;
        self.m_delay = t;
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  0000 mmmm 0010 0011  2  BRAF Rm */
    // origin: src/mame/cpu/sh.cpp:252-256 — note: does NOT write ea (disk :254)
    fn braf(&mut self, m: usize) {
        self.m_delay = self.pc.wrapping_add(self.r[m]).wrapping_add(2);
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  1011 dddd dddd dddd  2  BSR disp12 */
    // origin: src/mame/cpu/sh.cpp:262-269
    fn bsr(&mut self, d: u32) {
        let disp = sext(d, 12);
        self.pr = self.pc.wrapping_add(2);
        let t = self.pc.wrapping_add((disp.wrapping_mul(2)) as u32).wrapping_add(2);
        self.ea = t;
        self.m_delay = t;
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  0000 mmmm 0000 0011  2  BSRF Rm */
    // origin: src/mame/cpu/sh.cpp:275-280 — note: does NOT write ea (disk :278)
    fn bsrf(&mut self, m: usize) {
        self.pr = self.pc.wrapping_add(2);
        self.m_delay = self.pc.wrapping_add(self.r[m]).wrapping_add(2);
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  1000 1001 dddd dddd  3/1  BT disp8 */
    // origin: src/mame/cpu/sh.cpp:286-294 — direct pc, no delay slot
    fn bt(&mut self, d: u32) {
        if (self.sr & SH_T) != 0 {
            let disp = sext(d, 8);
            let t = self.pc.wrapping_add((disp.wrapping_mul(2)) as u32).wrapping_add(2);
            self.ea = t;
            self.pc = t;
            self.icount = self.icount.wrapping_sub(2);
        }
    }

    /*  1000 1101 dddd dddd  2/1  BTS disp8 */
    // origin: src/mame/cpu/sh.cpp:300-308
    fn bts(&mut self, d: u32) {
        if (self.sr & SH_T) != 0 {
            let disp = sext(d, 8);
            let t = self.pc.wrapping_add((disp.wrapping_mul(2)) as u32).wrapping_add(2);
            self.ea = t;
            self.m_delay = t;
            self.icount = self.icount.wrapping_sub(1);
        }
    }

    /*  0000 0000 0010 1000  1  CLRMAC */
    // origin: src/mame/cpu/sh.cpp:314-318
    fn clrmac(&mut self) {
        self.mach = 0;
        self.macl = 0;
    }

    /*  0000 0000 0000 1000  1  CLRT */
    // origin: src/mame/cpu/sh.cpp:324-327
    fn clrt(&mut self) {
        self.sr &= !SH_T;
    }

    /*  0011 nnnn mmmm 0000  1  CMP_EQ Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:333-339
    fn cmpeq(&mut self, m: usize, n: usize) {
        if self.r[n] == self.r[m] {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  0011 nnnn mmmm 0011  1  CMP_GE Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:345-351
    fn cmpge(&mut self, m: usize, n: usize) {
        if (self.r[n] as i32) >= (self.r[m] as i32) {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  0011 nnnn mmmm 0111  1  CMP_GT Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:357-363
    fn cmpgt(&mut self, m: usize, n: usize) {
        if (self.r[n] as i32) > (self.r[m] as i32) {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  0011 nnnn mmmm 0110  1  CMP_HI Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:369-375
    fn cmphi(&mut self, m: usize, n: usize) {
        if self.r[n] > self.r[m] {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  0011 nnnn mmmm 0010  1  CMP_HS Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:381-387
    fn cmphs(&mut self, m: usize, n: usize) {
        if self.r[n] >= self.r[m] {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  0100 nnnn 0001 0101  1  CMP_PL Rn */
    // origin: src/mame/cpu/sh.cpp:393-399 — strictly > 0 (zero clears T)
    fn cmppl(&mut self, n: usize) {
        if (self.r[n] as i32) > 0 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  0100 nnnn 0001 0001  1  CMP_PZ Rn */
    // origin: src/mame/cpu/sh.cpp:405-411
    fn cmppz(&mut self, n: usize) {
        if (self.r[n] as i32) >= 0 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  0010 nnnn mmmm 1100  1  CMP_STR Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:417-428
    fn cmpstr(&mut self, m: usize, n: usize) {
        let temp = self.r[n] ^ self.r[m];
        let upper_byte = ((temp >> 24) & 0xff) as u8;
        let mid_upper_byte = ((temp >> 16) & 0xff) as u8;
        let mid_lower_byte = ((temp >> 8) & 0xff) as u8;
        let lower_byte = (temp & 0xff) as u8;
        if upper_byte != 0 && mid_upper_byte != 0 && mid_lower_byte != 0 && lower_byte != 0 {
            self.sr &= !SH_T;
        } else {
            self.sr |= SH_T;
        }
    }

    /*  1000 1000 iiii iiii  1  CMP/EQ #imm,R0 */
    // origin: src/mame/cpu/sh.cpp:434-442
    fn cmpim(&mut self, i: u32) {
        let imm = sext(i, 8) as u32;
        if self.r[0] == imm {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  0010 nnnn mmmm 0111  1  DIV0S Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:448-464
    fn div0s(&mut self, m: usize, n: usize) {
        if bit(self.r[n], 31) == 0 {
            self.sr &= !SH_Q;
        } else {
            self.sr |= SH_Q;
        }

        if bit(self.r[m], 31) == 0 {
            self.sr &= !SH_M;
        } else {
            self.sr |= SH_M;
        }

        if bit(self.r[m] ^ self.r[n], 31) != 0 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  0000 0000 0001 1001  1  DIV0U */
    // origin: src/mame/cpu/sh.cpp:470-473
    fn div0u(&mut self) {
        self.sr &= !(SH_M | SH_Q | SH_T);
    }

    /*  0011 nnnn mmmm 0100  1  DIV1 Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:479-565 — the four-way Q/M update tree verbatim.
    // Braceless else-ifs verified from disk bytes (tab depth): the else always binds
    // to its own `if (r[n] >/ < tmp)`, never to the outer `if (!(sr & SH_Q))`.
    fn div1(&mut self, m: usize, n: usize) {
        let old_q = self.sr & SH_Q;
        if 0x8000_0000 & self.r[n] != 0 {
            self.sr |= SH_Q;
        } else {
            self.sr &= !SH_Q;
        }

        self.r[n] = (self.r[n] << 1) | (self.sr & SH_T);

        if old_q == 0 {
            if self.sr & SH_M == 0 {
                let tmp = self.r[n];
                self.r[n] = self.r[n].wrapping_sub(self.r[m]);
                if self.sr & SH_Q == 0 {
                    if self.r[n] > tmp {
                        self.sr |= SH_Q;
                    } else {
                        self.sr &= !SH_Q;
                    }
                } else {
                    if self.r[n] > tmp {
                        self.sr &= !SH_Q;
                    } else {
                        self.sr |= SH_Q;
                    }
                }
            } else {
                let tmp = self.r[n];
                self.r[n] = self.r[n].wrapping_add(self.r[m]);
                if self.sr & SH_Q == 0 {
                    if self.r[n] < tmp {
                        self.sr &= !SH_Q;
                    } else {
                        self.sr |= SH_Q;
                    }
                } else {
                    if self.r[n] < tmp {
                        self.sr |= SH_Q;
                    } else {
                        self.sr &= !SH_Q;
                    }
                }
            }
        } else {
            if self.sr & SH_M == 0 {
                let tmp = self.r[n];
                self.r[n] = self.r[n].wrapping_add(self.r[m]);
                if self.sr & SH_Q == 0 {
                    if self.r[n] < tmp {
                        self.sr |= SH_Q;
                    } else {
                        self.sr &= !SH_Q;
                    }
                } else {
                    if self.r[n] < tmp {
                        self.sr &= !SH_Q;
                    } else {
                        self.sr |= SH_Q;
                    }
                }
            } else {
                let tmp = self.r[n];
                self.r[n] = self.r[n].wrapping_sub(self.r[m]);
                if self.sr & SH_Q == 0 {
                    if self.r[n] > tmp {
                        self.sr &= !SH_Q;
                    } else {
                        self.sr |= SH_Q;
                    }
                } else {
                    if self.r[n] > tmp {
                        self.sr |= SH_Q;
                    } else {
                        self.sr &= !SH_Q;
                    }
                }
            }
        }

        let tmp = self.sr & (SH_Q | SH_M);
        if tmp == 0 || tmp == 0x300 {
            // if Q == M set T else clear T
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  DMULS.L Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:568-612 — 0 - INT_MIN is UB in C++ (wraps to
    // INT_MIN on the MinGW baseline); rendered wrapping_neg (deviation note)
    fn dmul_s(&mut self, m: usize, n: usize) {
        let mut tempn = self.r[n] as i32;
        let mut tempm = self.r[m] as i32;
        let fnlml = bit((tempn as u32) ^ (tempm as u32), 31) != 0;

        if tempn < 0 {
            tempn = tempn.wrapping_neg();
        }
        if tempm < 0 {
            tempm = tempm.wrapping_neg();
        }

        let rn_l = (tempn as u32) & 0x0000_ffff;
        let rn_h = (tempn as u32) >> 16;
        let rm_l = (tempm as u32) & 0x0000_ffff;
        let rm_h = (tempm as u32) >> 16;

        let temp0 = rm_l.wrapping_mul(rn_l);
        let temp1 = rm_h.wrapping_mul(rn_l);
        let temp2 = rm_l.wrapping_mul(rn_h);
        let temp3 = rm_h.wrapping_mul(rn_h);

        let mut res2: u32 = 0;
        let res1 = temp1.wrapping_add(temp2);
        if res1 < temp1 {
            res2 = res2.wrapping_add(0x0001_0000);
        }
        let temp1 = res1 << 16;

        let mut res0 = temp0.wrapping_add(temp1);
        if res0 < temp0 {
            res2 = res2.wrapping_add(1);
        }
        res2 = res2.wrapping_add(res1 >> 16).wrapping_add(temp3);

        if fnlml {
            res2 = !res2;
            if res0 == 0 {
                res2 = res2.wrapping_add(1);
            } else {
                res0 = (!res0).wrapping_add(1);
            }
        }

        self.mach = res2;
        self.macl = res0;
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  DMULU.L Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:615-642
    fn dmul_u(&mut self, m: usize, n: usize) {
        let rn_l = self.r[n] & 0x0000_ffff;
        let rn_h = self.r[n] >> 16;
        let rm_l = self.r[m] & 0x0000_ffff;
        let rm_h = self.r[m] >> 16;

        let temp0 = rm_l.wrapping_mul(rn_l);
        let temp1 = rm_h.wrapping_mul(rn_l);
        let temp2 = rm_l.wrapping_mul(rn_h);
        let temp3 = rm_h.wrapping_mul(rn_h);

        let mut res2: u32 = 0;
        let res1 = temp1.wrapping_add(temp2);
        if res1 < temp1 {
            res2 = res2.wrapping_add(0x0001_0000);
        }

        let temp1 = res1 << 16;
        let res0 = temp0.wrapping_add(temp1);
        if res0 < temp0 {
            res2 = res2.wrapping_add(1);
        }

        res2 = res2.wrapping_add(res1 >> 16).wrapping_add(temp3);

        self.mach = res2;
        self.macl = res0;
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  DT      Rn */
    // origin: src/mame/cpu/sh.cpp:645-668 — BUSY_LOOP_HACKS==0: :652-667 dead,
    // not ported. r[n]-- at 0 wraps (wrapping_sub — dev-profile bare - would panic)
    fn dt(&mut self, n: usize) {
        self.r[n] = self.r[n].wrapping_sub(1);
        if self.r[n] == 0 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  EXTS.B  Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:671-674
    fn extsb(&mut self, m: usize, n: usize) {
        self.r[n] = sext(self.r[m], 8) as u32;
    }

    /*  EXTS.W  Rm,Rn — the ledger's found bug class: exact 16-bit sign extend */
    // origin: src/mame/cpu/sh.cpp:677-680
    fn extsw(&mut self, m: usize, n: usize) {
        self.r[n] = sext(self.r[m], 16) as u32;
    }

    /*  EXTU.B  Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:683-686
    fn extub(&mut self, m: usize, n: usize) {
        self.r[n] = self.r[m] & 0x0000_00ff;
    }

    /*  EXTU.W  Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:689-692
    fn extuw(&mut self, m: usize, n: usize) {
        self.r[n] = self.r[m] & 0x0000_ffff;
    }

    /*  JMP     @Rm */
    // origin: src/mame/cpu/sh.cpp:695-699
    fn jmp(&mut self, m: usize) {
        self.ea = self.r[m];
        self.m_delay = self.ea;
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  JSR     @Rm */
    // origin: src/mame/cpu/sh.cpp:702-707
    fn jsr(&mut self, m: usize) {
        self.pr = self.pc.wrapping_add(2);
        self.ea = self.r[m];
        self.m_delay = self.ea;
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  LDC     Rm,GBR — dispatch passes REG_N (disk :1813); C++ param name is m */
    // origin: src/mame/cpu/sh.cpp:710-713
    fn ldcgbr(&mut self, m: usize) {
        self.gbr = self.r[m];
    }

    /*  LDC     Rm,VBR */
    // origin: src/mame/cpu/sh.cpp:716-719
    fn ldcvbr(&mut self, m: usize) {
        self.vbr = self.r[m];
    }

    /*  LDC.L   @Rm+,GBR */
    // origin: src/mame/cpu/sh.cpp:722-728
    fn ldcmgbr<B: Sh2Bus>(&mut self, bus: &mut B, m: usize) {
        self.ea = self.r[m];
        self.gbr = self.read_long(bus, self.ea);
        self.r[m] = self.r[m].wrapping_add(4);
        self.icount = self.icount.wrapping_sub(2);
    }

    /*  LDC.L   @Rm+,VBR */
    // origin: src/mame/cpu/sh.cpp:731-737
    fn ldcmvbr<B: Sh2Bus>(&mut self, bus: &mut B, m: usize) {
        self.ea = self.r[m];
        self.vbr = self.read_long(bus, self.ea);
        self.r[m] = self.r[m].wrapping_add(4);
        self.icount = self.icount.wrapping_sub(2);
    }

    /*  LDS     Rm,MACH */
    // origin: src/mame/cpu/sh.cpp:740-743
    fn ldsmach(&mut self, m: usize) {
        self.mach = self.r[m];
    }

    /*  LDS     Rm,MACL */
    // origin: src/mame/cpu/sh.cpp:746-749
    fn ldsmacl(&mut self, m: usize) {
        self.macl = self.r[m];
    }

    /*  LDS     Rm,PR */
    // origin: src/mame/cpu/sh.cpp:752-755
    fn ldspr(&mut self, m: usize) {
        self.pr = self.r[m];
    }

    /*  LDS.L   @Rm+,MACH — no icount adjustment in this file (disk :758-763) */
    // origin: src/mame/cpu/sh.cpp:758-763
    fn ldsmmach<B: Sh2Bus>(&mut self, bus: &mut B, m: usize) {
        self.ea = self.r[m];
        self.mach = self.read_long(bus, self.ea);
        self.r[m] = self.r[m].wrapping_add(4);
    }

    /*  LDS.L   @Rm+,MACL */
    // origin: src/mame/cpu/sh.cpp:766-771
    fn ldsmmacl<B: Sh2Bus>(&mut self, bus: &mut B, m: usize) {
        self.ea = self.r[m];
        self.macl = self.read_long(bus, self.ea);
        self.r[m] = self.r[m].wrapping_add(4);
    }

    /*  LDS.L   @Rm+,PR */
    // origin: src/mame/cpu/sh.cpp:774-779
    fn ldsmpr<B: Sh2Bus>(&mut self, bus: &mut B, m: usize) {
        self.ea = self.r[m];
        self.pr = self.read_long(bus, self.ea);
        self.r[m] = self.r[m].wrapping_add(4);
    }

    /*  MAC.L   @Rm+,@Rn+ */
    // origin: src/mame/cpu/sh.cpp:782-848 — order is n-first (read n, bump n,
    // THEN read m): observable when m == n. S-flag path saturates the 64-bit sum
    // to the 48-bit range with sign from the product; non-S carries into MACH.
    fn mac_l<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        let mut tempn = self.read_long(bus, self.r[n]) as i32;
        self.r[n] = self.r[n].wrapping_add(4);

        let mut tempm = self.read_long(bus, self.r[m]) as i32;
        self.r[m] = self.r[m].wrapping_add(4);

        let fnlml = bit((tempn as u32) ^ (tempm as u32), 31) != 0;

        if tempn < 0 {
            tempn = tempn.wrapping_neg();
        }
        if tempm < 0 {
            tempm = tempm.wrapping_neg();
        }

        let rn_l = (tempn as u32) & 0x0000_ffff;
        let rn_h = (tempn as u32) >> 16;
        let rm_l = (tempm as u32) & 0x0000_ffff;
        let rm_h = (tempm as u32) >> 16;

        let temp0 = rm_l.wrapping_mul(rn_l);
        let temp1 = rm_h.wrapping_mul(rn_l);
        let temp2 = rm_l.wrapping_mul(rn_h);
        let temp3 = rm_h.wrapping_mul(rn_h);

        let mut res2: u32 = 0;
        let res1 = temp1.wrapping_add(temp2);
        if res1 < temp1 {
            res2 = res2.wrapping_add(0x0001_0000);
        }
        let temp1 = res1 << 16;

        let mut res0 = temp0.wrapping_add(temp1);
        if res0 < temp0 {
            res2 = res2.wrapping_add(1);
        }
        res2 = res2.wrapping_add(res1 >> 16).wrapping_add(temp3);

        if fnlml {
            res2 = !res2;
            if res0 == 0 {
                res2 = res2.wrapping_add(1);
            } else {
                res0 = (!res0).wrapping_add(1);
            }
        }

        if self.sr & SH_S != 0 {
            // full 64-bit accumulate, saturated to the 48-bit range with the sign
            // taken from the product (origin: sh.cpp:829-836)
            let mut sum = (((self.mach as u64) << 32) | self.macl as u64)
                .wrapping_add(((res2 as u64) << 32) | res0 as u64);

            if sum > 0x0000_7fff_ffff_ffff && sum < 0xffff_8000_0000_0000 {
                sum = if fnlml {
                    0xffff_8000_0000_0000
                } else {
                    0x0000_7fff_ffff_ffff
                };
            }

            self.mach = (sum >> 32) as u32;
            self.macl = sum as u32;
        } else {
            res0 = self.macl.wrapping_add(res0);
            if self.macl > res0 {
                res2 = res2.wrapping_add(1);
            }
            res2 = res2.wrapping_add(self.mach);
            self.mach = res2;
            self.macl = res0;
        }
        self.icount = self.icount.wrapping_sub(2);
    }

    /*  MAC.W   @Rm+,@Rn+ */
    // origin: src/mame/cpu/sh.cpp:851-894 — n-first again; int32 *= is UB on
    // overflow, wraps on the baseline (wrapping_mul deviation note)
    fn mac_w<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        let mut tempn = self.read_word(bus, self.r[n]) as i16 as i32;
        self.r[n] = self.r[n].wrapping_add(2);

        let mut tempm = self.read_word(bus, self.r[m]) as i16 as i32;
        self.r[m] = self.r[m].wrapping_add(2);

        let templ = self.macl;
        tempm = tempm.wrapping_mul(tempn);

        let dest = bit(self.macl, 31) as i32;
        let src = bit(tempm as u32, 31) as i32 + dest;
        tempn = if bit(tempm as u32, 31) != 0 { -1 } else { 0 };

        self.macl = self.macl.wrapping_add(tempm as u32);

        let ans = bit(self.macl, 31) as i32 + dest;

        if self.sr & SH_S != 0 {
            if ans == 1 {
                // src 0 or 2 means both addends had the same sign, so the sign
                // change is an overflow (origin: sh.cpp:873-884)
                if src == 0 {
                    self.macl = 0x7fff_ffff;
                    self.mach |= 1;
                } else if src == 2 {
                    self.macl = 0x8000_0000;
                    self.mach |= 1;
                }
            }
        } else {
            self.mach = self.mach.wrapping_add(tempn as u32);
            if templ > self.macl {
                self.mach = self.mach.wrapping_add(1);
            }
        }
        self.icount = self.icount.wrapping_sub(2);
    }

    /*  MOV     Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:897-900
    fn mov(&mut self, m: usize, n: usize) {
        self.r[n] = self.r[m];
    }

    /*  MOV.B   Rm,@Rn */
    // origin: src/mame/cpu/sh.cpp:903-907
    fn movbs<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[n];
        self.write_byte(bus, self.ea, (self.r[m] & 0x0000_00ff) as u8);
    }

    /*  MOV.W   Rm,@Rn */
    // origin: src/mame/cpu/sh.cpp:910-914
    fn movws<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[n];
        self.write_word(bus, self.ea, (self.r[m] & 0x0000_ffff) as u16);
    }

    /*  MOV.L   Rm,@Rn */
    // origin: src/mame/cpu/sh.cpp:917-921
    fn movls<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[n];
        self.write_long(bus, self.ea, self.r[m]);
    }

    /*  MOV.B   @Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:924-928
    fn movbl<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[m];
        self.r[n] = sext(self.read_byte(bus, self.ea) as u32, 8) as u32;
    }

    /*  MOV.W   @Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:931-935
    fn movwl<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[m];
        self.r[n] = sext(self.read_word(bus, self.ea) as u32, 16) as u32;
    }

    /*  MOV.L   @Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:938-942
    fn movll<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[m];
        self.r[n] = self.read_long(bus, self.ea);
    }

    /*  MOV.B   Rm,@-Rn */
    // origin: src/mame/cpu/sh.cpp:945-951
    fn movbm<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        let data = (self.r[m] & 0xff) as u8;

        self.r[n] = self.r[n].wrapping_sub(1);
        self.write_byte(bus, self.r[n], data);
    }

    /*  MOV.W   Rm,@-Rn */
    // origin: src/mame/cpu/sh.cpp:954-960
    fn movwm<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        let data = (self.r[m] & 0xffff) as u16;

        self.r[n] = self.r[n].wrapping_sub(2);
        self.write_word(bus, self.r[n], data);
    }

    /*  MOV.L   Rm,@-Rn */
    // origin: src/mame/cpu/sh.cpp:963-969
    fn movlm<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        let data = self.r[m];

        self.r[n] = self.r[n].wrapping_sub(4);
        self.write_long(bus, self.r[n], data);
    }

    /*  MOV.B   @Rm+,Rn — SH quirk: NO increment when m == n (disk :975-976) */
    // origin: src/mame/cpu/sh.cpp:972-977
    fn movbp<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.r[n] = sext(self.read_byte(bus, self.r[m]) as u32, 8) as u32;
        if n != m {
            self.r[m] = self.r[m].wrapping_add(1);
        }
    }

    /*  MOV.W   @Rm+,Rn */
    // origin: src/mame/cpu/sh.cpp:980-985
    fn movwp<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.r[n] = sext(self.read_word(bus, self.r[m]) as u32, 16) as u32;
        if n != m {
            self.r[m] = self.r[m].wrapping_add(2);
        }
    }

    /*  MOV.L   @Rm+,Rn */
    // origin: src/mame/cpu/sh.cpp:988-993
    fn movlp<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.r[n] = self.read_long(bus, self.r[m]);
        if n != m {
            self.r[m] = self.r[m].wrapping_add(4);
        }
    }

    /*  MOV.B   Rm,@(R0,Rn) */
    // origin: src/mame/cpu/sh.cpp:996-1000
    fn movbs0<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[n].wrapping_add(self.r[0]);
        self.write_byte(bus, self.ea, self.r[m] as u8);
    }

    /*  MOV.W   Rm,@(R0,Rn) */
    // origin: src/mame/cpu/sh.cpp:1003-1007
    fn movws0<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[n].wrapping_add(self.r[0]);
        self.write_word(bus, self.ea, self.r[m] as u16);
    }

    /*  MOV.L   Rm,@(R0,Rn) */
    // origin: src/mame/cpu/sh.cpp:1010-1014
    fn movls0<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[n].wrapping_add(self.r[0]);
        self.write_long(bus, self.ea, self.r[m]);
    }

    /*  MOV.B   @(R0,Rm),Rn */
    // origin: src/mame/cpu/sh.cpp:1017-1021
    fn movbl0<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[m].wrapping_add(self.r[0]);
        self.r[n] = sext(self.read_byte(bus, self.ea) as u32, 8) as u32;
    }

    /*  MOV.W   @(R0,Rm),Rn */
    // origin: src/mame/cpu/sh.cpp:1024-1028
    fn movwl0<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[m].wrapping_add(self.r[0]);
        self.r[n] = sext(self.read_word(bus, self.ea) as u32, 16) as u32;
    }

    /*  MOV.L   @(R0,Rm),Rn */
    // origin: src/mame/cpu/sh.cpp:1031-1035
    fn movll0<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, n: usize) {
        self.ea = self.r[m].wrapping_add(self.r[0]);
        self.r[n] = self.read_long(bus, self.ea);
    }

    /*  MOV     #imm,Rn */
    // origin: src/mame/cpu/sh.cpp:1038-1041
    fn movi(&mut self, i: u32, n: usize) {
        self.r[n] = sext(i, 8) as u32;
    }

    /*  MOV.W   @(disp8,PC),Rn */
    // origin: src/mame/cpu/sh.cpp:1044-1049
    fn movwi<B: Sh2Bus>(&mut self, bus: &mut B, d: u32, n: usize) {
        let disp = d & 0xff;
        self.ea = self.pc.wrapping_add(disp.wrapping_mul(2)).wrapping_add(2);
        self.r[n] = sext(self.read_word(bus, self.ea) as u32, 16) as u32;
    }

    /*  MOV.L   @(disp8,PC),Rn — aligned PC base: (pc+2) & ~3 */
    // origin: src/mame/cpu/sh.cpp:1052-1057
    fn movli<B: Sh2Bus>(&mut self, bus: &mut B, d: u32, n: usize) {
        let disp = d & 0xff;
        self.ea = (self.pc.wrapping_add(2) & !3u32).wrapping_add(disp.wrapping_mul(4));
        self.r[n] = self.read_long(bus, self.ea);
    }

    /*  MOV.B   @(disp8,GBR),R0 */
    // origin: src/mame/cpu/sh.cpp:1060-1065
    fn movblg<B: Sh2Bus>(&mut self, bus: &mut B, d: u32) {
        let disp = d & 0xff;
        self.ea = self.gbr.wrapping_add(disp);
        self.r[0] = sext(self.read_byte(bus, self.ea) as u32, 8) as u32;
    }

    /*  MOV.W   @(disp8,GBR),R0 */
    // origin: src/mame/cpu/sh.cpp:1068-1073
    fn movwlg<B: Sh2Bus>(&mut self, bus: &mut B, d: u32) {
        let disp = d & 0xff;
        self.ea = self.gbr.wrapping_add(disp.wrapping_mul(2));
        self.r[0] = sext(self.read_word(bus, self.ea) as u32, 16) as u32;
    }

    /*  MOV.L   @(disp8,GBR),R0 */
    // origin: src/mame/cpu/sh.cpp:1076-1081
    fn movllg<B: Sh2Bus>(&mut self, bus: &mut B, d: u32) {
        let disp = d & 0xff;
        self.ea = self.gbr.wrapping_add(disp.wrapping_mul(4));
        self.r[0] = self.read_long(bus, self.ea);
    }

    /*  MOV.B   R0,@(disp8,GBR) */
    // origin: src/mame/cpu/sh.cpp:1084-1089
    fn movbsg<B: Sh2Bus>(&mut self, bus: &mut B, d: u32) {
        let disp = d & 0xff;
        self.ea = self.gbr.wrapping_add(disp);
        self.write_byte(bus, self.ea, self.r[0] as u8);
    }

    /*  MOV.W   R0,@(disp8,GBR) */
    // origin: src/mame/cpu/sh.cpp:1092-1097
    fn movwsg<B: Sh2Bus>(&mut self, bus: &mut B, d: u32) {
        let disp = d & 0xff;
        self.ea = self.gbr.wrapping_add(disp.wrapping_mul(2));
        self.write_word(bus, self.ea, self.r[0] as u16);
    }

    /*  MOV.L   R0,@(disp8,GBR) */
    // origin: src/mame/cpu/sh.cpp:1100-1105
    fn movlsg<B: Sh2Bus>(&mut self, bus: &mut B, d: u32) {
        let disp = d & 0xff;
        self.ea = self.gbr.wrapping_add(disp.wrapping_mul(4));
        self.write_long(bus, self.ea, self.r[0]);
    }

    /*  MOV.B   R0,@(disp4,Rn) */
    // origin: src/mame/cpu/sh.cpp:1108-1113
    fn movbs4<B: Sh2Bus>(&mut self, bus: &mut B, d: u32, n: usize) {
        let disp = d & 0x0f;
        self.ea = self.r[n].wrapping_add(disp);
        self.write_byte(bus, self.ea, self.r[0] as u8);
    }

    /*  MOV.W   R0,@(disp4,Rn) */
    // origin: src/mame/cpu/sh.cpp:1116-1121
    fn movws4<B: Sh2Bus>(&mut self, bus: &mut B, d: u32, n: usize) {
        let disp = d & 0x0f;
        self.ea = self.r[n].wrapping_add(disp.wrapping_mul(2));
        self.write_word(bus, self.ea, self.r[0] as u16);
    }

    /*  MOV.L Rm,@(disp4,Rn) */
    // origin: src/mame/cpu/sh.cpp:1124-1129
    fn movls4<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, d: u32, n: usize) {
        let disp = d & 0x0f;
        self.ea = self.r[n].wrapping_add(disp.wrapping_mul(4));
        self.write_long(bus, self.ea, self.r[m]);
    }

    /*  MOV.B   @(disp4,Rm),R0 */
    // origin: src/mame/cpu/sh.cpp:1132-1137
    fn movbl4<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, d: u32) {
        let disp = d & 0x0f;
        self.ea = self.r[m].wrapping_add(disp);
        self.r[0] = sext(self.read_byte(bus, self.ea) as u32, 8) as u32;
    }

    /*  MOV.W   @(disp4,Rm),R0 */
    // origin: src/mame/cpu/sh.cpp:1140-1145
    fn movwl4<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, d: u32) {
        let disp = d & 0x0f;
        self.ea = self.r[m].wrapping_add(disp.wrapping_mul(2));
        self.r[0] = sext(self.read_word(bus, self.ea) as u32, 16) as u32;
    }

    /*  MOV.L   @(disp4,Rm),Rn */
    // origin: src/mame/cpu/sh.cpp:1148-1153
    fn movll4<B: Sh2Bus>(&mut self, bus: &mut B, m: usize, d: u32, n: usize) {
        let disp = d & 0x0f;
        self.ea = self.r[m].wrapping_add(disp.wrapping_mul(4));
        self.r[n] = self.read_long(bus, self.ea);
    }

    /*  MOVA    @(disp8,PC),R0 */
    // origin: src/mame/cpu/sh.cpp:1156-1161
    fn mova<B: Sh2Bus>(&mut self, _bus: &mut B, d: u32) {
        let disp = d & 0xff;
        self.ea = (self.pc.wrapping_add(2) & !3u32).wrapping_add(disp.wrapping_mul(4));
        self.r[0] = self.ea;
    }

    /*  MOVT    Rn */
    // origin: src/mame/cpu/sh.cpp:1164-1167
    fn movt(&mut self, n: usize) {
        self.r[n] = self.sr & SH_T;
    }

    /*  MUL.L   Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1170-1174
    fn mull(&mut self, m: usize, n: usize) {
        self.macl = self.r[n].wrapping_mul(self.r[m]);
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  MULS    Rm,Rn — both operands sign-extended int16, product sign-extends */
    // origin: src/mame/cpu/sh.cpp:1177-1180
    fn muls(&mut self, m: usize, n: usize) {
        self.macl = ((self.r[n] as u16) as i16 as i32).wrapping_mul((self.r[m] as u16) as i16 as i32) as u32;
    }

    /*  MULU    Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1183-1186
    fn mulu(&mut self, m: usize, n: usize) {
        self.macl = (self.r[n] as u16 as u32).wrapping_mul(self.r[m] as u16 as u32);
    }

    /*  NEG     Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1189-1192
    fn neg(&mut self, m: usize, n: usize) {
        self.r[n] = 0u32.wrapping_sub(self.r[m]);
    }

    /*  NEGC    Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1195-1203
    fn negc(&mut self, m: usize, n: usize) {
        let temp = self.r[m];
        self.r[n] = temp.wrapping_neg().wrapping_sub(self.sr & SH_T);
        if temp != 0 || (self.sr & SH_T) != 0 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  NOP */
    // origin: src/mame/cpu/sh.cpp:1206-1208
    fn nop(&mut self) {}

    /*  NOT     Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1211-1214
    fn not(&mut self, m: usize, n: usize) {
        self.r[n] = !self.r[m];
    }

    /*  OR      Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1217-1220
    fn or(&mut self, m: usize, n: usize) {
        self.r[n] |= self.r[m];
    }

    /*  OR      #imm,R0 */
    // origin: src/mame/cpu/sh.cpp:1223-1226
    fn ori(&mut self, i: u32) {
        self.r[0] |= i;
    }

    /*  OR.B    #imm,@(R0,GBR) */
    // origin: src/mame/cpu/sh.cpp:1229-1234
    fn orm<B: Sh2Bus>(&mut self, bus: &mut B, i: u32) {
        self.ea = self.gbr.wrapping_add(self.r[0]);
        let v = self.read_byte(bus, self.ea) | (i as u8);
        self.write_byte(bus, self.ea, v);
        self.icount = self.icount.wrapping_sub(2);
    }

    /*  ROTCL   Rn — rotate through T: T is an EXTRA bit (carry-into-0, MSB-into-T) */
    // origin: src/mame/cpu/sh.cpp:1237-1242
    fn rotcl(&mut self, n: usize) {
        let temp = (self.r[n] >> 31) & SH_T;
        self.r[n] = (self.r[n] << 1) | (self.sr & SH_T);
        self.sr = (self.sr & !SH_T) | temp;
    }

    /*  ROTCR   Rn */
    // origin: src/mame/cpu/sh.cpp:1245-1253
    fn rotcr(&mut self, n: usize) {
        let temp = (self.sr & SH_T) << 31;
        if self.r[n] & SH_T != 0 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
        self.r[n] = (self.r[n] >> 1) | temp;
    }

    /*  ROTL    Rn — T gets the OLD bit31 (sh.cpp:1258 reads r[n] before rotating) */
    // origin: src/mame/cpu/sh.cpp:1256-1260
    fn rotl(&mut self, n: usize) {
        self.sr = (self.sr & !SH_T) | ((self.r[n] >> 31) & SH_T);
        self.r[n] = self.r[n].rotate_left(1);
    }

    /*  ROTR    Rn — T gets the OLD bit0 */
    // origin: src/mame/cpu/sh.cpp:1263-1267
    fn rotr(&mut self, n: usize) {
        self.sr = (self.sr & !SH_T) | (self.r[n] & SH_T);
        self.r[n] = self.r[n].rotate_right(1);
    }

    /*  RTS */
    // origin: src/mame/cpu/sh.cpp:1270-1274
    fn rts(&mut self) {
        self.ea = self.pr;
        self.m_delay = self.ea;
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  SETT */
    // origin: src/mame/cpu/sh.cpp:1277-1280
    fn sett(&mut self) {
        self.sr |= SH_T;
    }

    /*  SHAL    Rn      (same as SHLL) */
    // origin: src/mame/cpu/sh.cpp:1283-1287
    fn shal(&mut self, n: usize) {
        self.sr = (self.sr & !SH_T) | ((self.r[n] >> 31) & SH_T);
        self.r[n] <<= 1;
    }

    /*  SHAR    Rn — arithmetic right: T gets bit0 BEFORE the shift */
    // origin: src/mame/cpu/sh.cpp:1290-1294
    fn shar(&mut self, n: usize) {
        self.sr = (self.sr & !SH_T) | (self.r[n] & SH_T);
        self.r[n] = ((self.r[n] as i32) >> 1) as u32;
    }

    /*  SHLL    Rn      (same as SHAL) */
    // origin: src/mame/cpu/sh.cpp:1297-1301
    fn shll(&mut self, n: usize) {
        self.sr = (self.sr & !SH_T) | ((self.r[n] >> 31) & SH_T);
        self.r[n] <<= 1;
    }

    /*  SHLL2   Rn */
    // origin: src/mame/cpu/sh.cpp:1304-1307
    fn shll2(&mut self, n: usize) {
        self.r[n] <<= 2;
    }

    /*  SHLL8   Rn */
    // origin: src/mame/cpu/sh.cpp:1310-1313
    fn shll8(&mut self, n: usize) {
        self.r[n] <<= 8;
    }

    /*  SHLL16  Rn */
    // origin: src/mame/cpu/sh.cpp:1316-1319
    fn shll16(&mut self, n: usize) {
        self.r[n] <<= 16;
    }

    /*  SHLR    Rn */
    // origin: src/mame/cpu/sh.cpp:1322-1326
    fn shlr(&mut self, n: usize) {
        self.sr = (self.sr & !SH_T) | (self.r[n] & SH_T);
        self.r[n] >>= 1;
    }

    /*  SHLR2   Rn */
    // origin: src/mame/cpu/sh.cpp:1329-1332
    fn shlr2(&mut self, n: usize) {
        self.r[n] >>= 2;
    }

    /*  SHLR8   Rn */
    // origin: src/mame/cpu/sh.cpp:1335-1338
    fn shlr8(&mut self, n: usize) {
        self.r[n] >>= 8;
    }

    /*  SHLR16  Rn */
    // origin: src/mame/cpu/sh.cpp:1341-1344
    fn shlr16(&mut self, n: usize) {
        self.r[n] >>= 16;
    }

    /*  STC     SR,Rn */
    // origin: src/mame/cpu/sh.cpp:1348-1351
    fn stcsr(&mut self, n: usize) {
        self.r[n] = self.sr;
    }

    /*  STC     GBR,Rn */
    // origin: src/mame/cpu/sh.cpp:1354-1357
    fn stcgbr(&mut self, n: usize) {
        self.r[n] = self.gbr;
    }

    /*  STC     VBR,Rn */
    // origin: src/mame/cpu/sh.cpp:1360-1363
    fn stcvbr(&mut self, n: usize) {
        self.r[n] = self.vbr;
    }

    /*  STC.L   SR,@-Rn */
    // origin: src/mame/cpu/sh.cpp:1366-1372
    fn stcmsr<B: Sh2Bus>(&mut self, bus: &mut B, n: usize) {
        self.r[n] = self.r[n].wrapping_sub(4);
        self.ea = self.r[n];
        self.write_long(bus, self.ea, self.sr);
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  STC.L   GBR,@-Rn */
    // origin: src/mame/cpu/sh.cpp:1375-1381
    fn stcmgbr<B: Sh2Bus>(&mut self, bus: &mut B, n: usize) {
        self.r[n] = self.r[n].wrapping_sub(4);
        self.ea = self.r[n];
        self.write_long(bus, self.ea, self.gbr);
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  STC.L   VBR,@-Rn */
    // origin: src/mame/cpu/sh.cpp:1384-1390
    fn stcmvbr<B: Sh2Bus>(&mut self, bus: &mut B, n: usize) {
        self.r[n] = self.r[n].wrapping_sub(4);
        self.ea = self.r[n];
        self.write_long(bus, self.ea, self.vbr);
        self.icount = self.icount.wrapping_sub(1);
    }

    /*  STS     MACH,Rn */
    // origin: src/mame/cpu/sh.cpp:1393-1396
    fn stsmach(&mut self, n: usize) {
        self.r[n] = self.mach;
    }

    /*  STS     MACL,Rn */
    // origin: src/mame/cpu/sh.cpp:1399-1402
    fn stsmacl(&mut self, n: usize) {
        self.r[n] = self.macl;
    }

    /*  STS     PR,Rn */
    // origin: src/mame/cpu/sh.cpp:1405-1408
    fn stspr(&mut self, n: usize) {
        self.r[n] = self.pr;
    }

    /*  STS.L   MACH,@-Rn — no icount adjustment (disk :1411-1416) */
    // origin: src/mame/cpu/sh.cpp:1411-1416
    fn stsmmach<B: Sh2Bus>(&mut self, bus: &mut B, n: usize) {
        self.r[n] = self.r[n].wrapping_sub(4);
        self.ea = self.r[n];
        self.write_long(bus, self.ea, self.mach);
    }

    /*  STS.L   MACL,@-Rn */
    // origin: src/mame/cpu/sh.cpp:1419-1424
    fn stsmmacl<B: Sh2Bus>(&mut self, bus: &mut B, n: usize) {
        self.r[n] = self.r[n].wrapping_sub(4);
        self.ea = self.r[n];
        self.write_long(bus, self.ea, self.macl);
    }

    /*  STS.L   PR,@-Rn */
    // origin: src/mame/cpu/sh.cpp:1427-1432
    fn stsmpr<B: Sh2Bus>(&mut self, bus: &mut B, n: usize) {
        self.r[n] = self.r[n].wrapping_sub(4);
        self.ea = self.r[n];
        self.write_long(bus, self.ea, self.pr);
    }

    /*  SUB     Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1435-1438
    fn sub(&mut self, m: usize, n: usize) {
        self.r[n] = self.r[n].wrapping_sub(self.r[m]);
    }

    /*  SUBC    Rm,Rn — two-step borrow, mirror of ADDC's compare order */
    // origin: src/mame/cpu/sh.cpp:1441-1452
    fn subc(&mut self, m: usize, n: usize) {
        let tmp1 = self.r[n].wrapping_sub(self.r[m]);
        let tmp0 = self.r[n];
        self.r[n] = tmp1.wrapping_sub(self.sr & SH_T);
        if tmp0 < tmp1 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
        if tmp1 < self.r[n] {
            self.sr |= SH_T;
        }
    }

    /*  SUBV    Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1455-1470
    fn subv(&mut self, m: usize, n: usize) {
        let dest = bit(self.r[n], 31) as i32;
        let mut src = bit(self.r[m], 31) as i32;
        src += dest;

        self.r[n] = self.r[n].wrapping_sub(self.r[m]);

        let mut ans = bit(self.r[n], 31) as i32;
        ans += dest;

        if src == 1 && ans == 1 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  SWAP.B  Rm,Rn — halves-kept verbatim: temp first, then r[n] write */
    // origin: src/mame/cpu/sh.cpp:1473-1479
    fn swapb(&mut self, m: usize, n: usize) {
        let mut temp = self.r[m] & 0xffff_0000;
        temp |= (self.r[m] & 0x0000_00ff) << 8;
        self.r[n] = ((self.r[m] >> 8) as u8) as u32;
        self.r[n] = self.r[n] | temp;
    }

    /*  SWAP.W  Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1482-1486
    fn swapw(&mut self, m: usize, n: usize) {
        let temp = self.r[m] >> 16;
        self.r[n] = (self.r[m] << 16) | temp;
    }

    /*  TAS.B   @Rn */
    // origin: src/mame/cpu/sh.cpp:1489-1503
    fn tas<B: Sh2Bus>(&mut self, bus: &mut B, n: usize) {
        self.ea = self.r[n];

        /* Bus Lock enable */
        let mut temp = self.read_byte(bus, self.ea) as u32;
        if temp == 0 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
        temp |= 0x80;
        /* Bus Lock disable */
        self.write_byte(bus, self.ea, temp as u8);
        self.icount = self.icount.wrapping_sub(3);
    }

    /*  TST     Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1506-1512
    fn tst(&mut self, m: usize, n: usize) {
        if (self.r[n] & self.r[m]) == 0 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  TST     #imm,R0 */
    // origin: src/mame/cpu/sh.cpp:1515-1523
    fn tsti(&mut self, i: u32) {
        let imm = i & 0xff;

        if (imm & self.r[0]) == 0 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
    }

    /*  TST.B   #imm,@(R0,GBR) */
    // origin: src/mame/cpu/sh.cpp:1526-1536
    fn tstm<B: Sh2Bus>(&mut self, bus: &mut B, i: u32) {
        let imm = i & 0xff;

        self.ea = self.gbr.wrapping_add(self.r[0]);
        if (imm & (self.read_byte(bus, self.ea) as u32)) == 0 {
            self.sr |= SH_T;
        } else {
            self.sr &= !SH_T;
        }
        self.icount = self.icount.wrapping_sub(2);
    }

    /*  XOR     Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1539-1542
    fn xor(&mut self, m: usize, n: usize) {
        self.r[n] ^= self.r[m];
    }

    /*  XOR     #imm,R0 */
    // origin: src/mame/cpu/sh.cpp:1545-1548
    fn xori(&mut self, i: u32) {
        self.r[0] ^= i & 0x0000_00ff;
    }

    /*  XOR.B   #imm,@(R0,GBR) */
    // origin: src/mame/cpu/sh.cpp:1551-1556
    fn xorm<B: Sh2Bus>(&mut self, bus: &mut B, i: u32) {
        self.ea = self.gbr.wrapping_add(self.r[0]);
        let v = self.read_byte(bus, self.ea) ^ (i as u8);
        self.write_byte(bus, self.ea, v);
        self.icount = self.icount.wrapping_sub(2);
    }

    /*  XTRCT   Rm,Rn */
    // origin: src/mame/cpu/sh.cpp:1559-1562
    fn xtrct(&mut self, m: usize, n: usize) {
        self.r[n] = (self.r[n] >> 16) | (self.r[m] << 16);
    }

    /*  SLEEP */
    // origin: src/mame/cpu/sh.cpp:1565-1578 — mode 0 -> 1, 2 -> 0, 1 -> 1 (the
    // else-if chain leaves 1 untouched); pc re-points at itself unless mode == 2
    fn sleep(&mut self) {
        /* 0 = normal mode */
        /* 1 = enters into power-down mode */
        /* 2 = go out the power-down mode after an exception */
        if self.sleep_mode != 2 {
            self.pc = self.pc.wrapping_sub(2);
        }
        self.icount = self.icount.wrapping_sub(2);
        /* Wait_for_exception; */
        if self.sleep_mode == 0 {
            self.sleep_mode = 1;
        } else if self.sleep_mode == 2 {
            self.sleep_mode = 0;
        }
    }

    // =====================================================================
    // SH-2 overrides (origin: src/mame/cpu/sh2.cpp — pure virtuals in sh.h)
    // =====================================================================

    /*  LDC.L   @Rm+,SR */
    // origin: src/mame/cpu/sh2.cpp:181-189 (REG_N despite the Rm-looking mnemonic;
    // dispatch passes the whole opcode, disk sh.cpp:1789)
    fn ldcmsr<B: Sh2Bus>(&mut self, bus: &mut B, opcode: u16) {
        let rn = reg_n(opcode);
        self.ea = self.r[rn];
        self.sr = self.read_long(bus, self.ea) & SH_FLAGS;
        self.r[rn] = self.r[rn].wrapping_add(4);
        self.icount = self.icount.wrapping_sub(2);
        self.m_test_irq = 1;
    }

    /*  LDC     Rm,SR */
    // origin: src/mame/cpu/sh2.cpp:192-196
    fn ldcsr<B: Sh2Bus>(&mut self, _bus: &mut B, opcode: u16) {
        self.sr = self.r[reg_n(opcode)] & SH_FLAGS;
        self.m_test_irq = 1;
    }

    /*  RTE */
    // origin: src/mame/cpu/sh2.cpp:199-209 — RTE's return is a DELAYED branch
    // (m_delay), then SR reload from the stack
    fn rte<B: Sh2Bus>(&mut self, bus: &mut B) {
        self.ea = self.r[15];
        self.m_delay = self.read_long(bus, self.ea);
        self.r[15] = self.r[15].wrapping_add(4);
        self.ea = self.r[15];
        self.sr = self.read_long(bus, self.ea) & SH_FLAGS;
        self.r[15] = self.r[15].wrapping_add(4);
        self.icount = self.icount.wrapping_sub(3);
        self.m_test_irq = 1;
    }

    /*  TRAPA   #imm */
    // origin: src/mame/cpu/sh2.cpp:212-227 — debugger_exception_hook is a no-op
    // (mamecompat.h:80). Pushes SR then pc (post-fetch pc = instruction+2). NOTE:
    // vector fetch is NOT & m_am'd here (disk :224), unlike ILLEGAL (:241)
    fn trapa<B: Sh2Bus>(&mut self, bus: &mut B, i: u32) {
        let imm = i & 0xff;

        self.ea = self.vbr.wrapping_add(imm.wrapping_mul(4));

        self.r[15] = self.r[15].wrapping_sub(4);
        self.write_long(bus, self.r[15], self.sr);
        self.r[15] = self.r[15].wrapping_sub(4);
        self.write_long(bus, self.r[15], self.pc);

        self.pc = self.read_long(bus, self.ea);

        self.icount = self.icount.wrapping_sub(7);
    }

    /*  ILLEGAL */
    // origin: src/mame/cpu/sh2.cpp:230-245 — pushes SR then (pc-2) (the faulting
    // instruction's own address), vector 16 = VBR+0x10, masked fetch
    fn illegal<B: Sh2Bus>(&mut self, bus: &mut B) {
        self.r[15] = self.r[15].wrapping_sub(4);
        self.write_long(bus, self.r[15], self.sr); /* push SR onto stack */
        self.r[15] = self.r[15].wrapping_sub(4);
        self.write_long(bus, self.r[15], self.pc.wrapping_sub(2)); /* push PC onto stack */

        /* fetch PC */
        self.pc = self.read_long(bus, self.vbr.wrapping_add(4 * 4)) & self.m_am;

        /* TODO: timing is a guess */
        self.icount = self.icount.wrapping_sub(5);
    }

    // origin: src/mame/cpu/sh2.cpp:247-250 — on SH-2 the entire f000 region is ILLEGAL
    fn execute_one_f000<B: Sh2Bus>(&mut self, bus: &mut B, _opcode: u16) {
        self.illegal(bus);
    }

    // origin: src/mame/cpu/sh2.cpp:154-178 (check_pending_irq). NMI wins; otherwise
    // highest pending external line (std::bit_width-1 == 31-leading_zeros) unless
    // the internal level is higher. `message` dropped (LOG-only in ground truth).
    pub fn check_pending_irq<B: Sh2Bus>(&mut self, bus: &mut B) {
        if self.pending_nmi != 0 {
            self.sh2_exception(bus, 16);
            self.pending_nmi = 0;
        } else {
            let mut irq = self.internal_irq_level;
            if self.pending_irq != 0 {
                let external_irq = 31 - self.pending_irq.leading_zeros() as i32;
                if external_irq >= irq {
                    irq = external_irq;
                }
            }

            if irq >= 0 {
                self.sh2_exception(bus, irq);
            }
        }
    }

    // origin: src/mame/cpu/sh2.cpp:347-379 (sh2_exception). standard_irq_callback
    // is a no-op returning 0 (mamecompat.h:84); LOG() is compiled out
    // (`//#define VERBOSE 1`, sh2.cpp:24). message dropped.
    fn sh2_exception<B: Sh2Bus>(&mut self, bus: &mut B, irqline: i32) {
        let vector: i32;

        if irqline != 16 {
            if irqline <= ((self.sr >> 4) & 15) as i32 {
                // If the cpu forbids this interrupt
                return;
            }

            // if this is an sh2 internal irq, use its vector
            if self.internal_irq_level == irqline {
                vector = self.m_internal_irq_vector;
                /* avoid spurious irqs with this (TODO: needs a better fix) */
                self.internal_irq_level = -1;
            } else {
                vector = 64 + irqline / 2;
            }
        } else {
            vector = 11;
        }

        self.sh2_exception_internal(bus, irqline, vector);
    }

    // origin: src/mame/cpu/sh2.cpp:381-401 (sh2_exception_internal)
    fn sh2_exception_internal<B: Sh2Bus>(&mut self, bus: &mut B, irqline: i32, vector: i32) {
        self.r[15] = self.r[15].wrapping_sub(4);
        self.write_long(bus, self.r[15], self.sr); /* push SR onto stack */
        self.r[15] = self.r[15].wrapping_sub(4);
        self.write_long(bus, self.r[15], self.pc); /* push PC onto stack */

        /* set I flags in SR */
        if irqline > 15 {
            // SH2_INT_15 (sh2.cpp:27)
            self.sr = self.sr | SH_I;
        } else {
            self.sr = (self.sr & !SH_I) | ((irqline as u32) << 4);
        }

        /* fetch PC */
        self.pc = self.read_long(bus, self.vbr.wrapping_add((vector as u32) * 4)) & self.m_am;

        if self.sleep_mode == 1 {
            self.sleep_mode = 2;
        }

        // origin: src/mame/cpu/sh7042.cpp:400 (override tail, AFTER the base
        // sh2_device::sh2_exception_internal above — disk order exact).
        bus.exception_taken(vector as u32);
    }

    // =====================================================================
    // Dispatch — transliterated region-for-region (origin: sh.cpp:1580-1874)
    // =====================================================================

    /* Common dispatch */

    // origin: src/mame/cpu/sh.cpp:1582-1603
    fn op0010<B: Sh2Bus>(&mut self, bus: &mut B, opcode: u16) {
        match opcode & 15 {
            0 => self.movbs(bus, reg_m(opcode), reg_n(opcode)),
            1 => self.movws(bus, reg_m(opcode), reg_n(opcode)),
            2 => self.movls(bus, reg_m(opcode), reg_n(opcode)),
            3 => self.illegal(bus),
            4 => self.movbm(bus, reg_m(opcode), reg_n(opcode)),
            5 => self.movwm(bus, reg_m(opcode), reg_n(opcode)),
            6 => self.movlm(bus, reg_m(opcode), reg_n(opcode)),
            7 => self.div0s(reg_m(opcode), reg_n(opcode)),
            8 => self.tst(reg_m(opcode), reg_n(opcode)),
            9 => self.and(reg_m(opcode), reg_n(opcode)),
            10 => self.xor(reg_m(opcode), reg_n(opcode)),
            11 => self.or(reg_m(opcode), reg_n(opcode)),
            12 => self.cmpstr(reg_m(opcode), reg_n(opcode)),
            13 => self.xtrct(reg_m(opcode), reg_n(opcode)),
            14 => self.mulu(reg_m(opcode), reg_n(opcode)),
            _ => self.muls(reg_m(opcode), reg_n(opcode)), // 15
        }
    }

    // origin: src/mame/cpu/sh.cpp:1605-1626
    fn op0011<B: Sh2Bus>(&mut self, bus: &mut B, opcode: u16) {
        match opcode & 15 {
            0 => self.cmpeq(reg_m(opcode), reg_n(opcode)),
            1 => self.illegal(bus),
            2 => self.cmphs(reg_m(opcode), reg_n(opcode)),
            3 => self.cmpge(reg_m(opcode), reg_n(opcode)),
            4 => self.div1(reg_m(opcode), reg_n(opcode)),
            5 => self.dmul_u(reg_m(opcode), reg_n(opcode)),
            6 => self.cmphi(reg_m(opcode), reg_n(opcode)),
            7 => self.cmpgt(reg_m(opcode), reg_n(opcode)),
            8 => self.sub(reg_m(opcode), reg_n(opcode)),
            9 => self.illegal(bus),
            10 => self.subc(reg_m(opcode), reg_n(opcode)),
            11 => self.subv(reg_m(opcode), reg_n(opcode)),
            12 => self.add(reg_m(opcode), reg_n(opcode)),
            13 => self.dmul_s(reg_m(opcode), reg_n(opcode)),
            14 => self.addc(reg_m(opcode), reg_n(opcode)),
            _ => self.addv(reg_m(opcode), reg_n(opcode)), // 15
        }
    }

    // origin: src/mame/cpu/sh.cpp:1628-1649
    fn op0110<B: Sh2Bus>(&mut self, bus: &mut B, opcode: u16) {
        match opcode & 15 {
            0 => self.movbl(bus, reg_m(opcode), reg_n(opcode)),
            1 => self.movwl(bus, reg_m(opcode), reg_n(opcode)),
            2 => self.movll(bus, reg_m(opcode), reg_n(opcode)),
            3 => self.mov(reg_m(opcode), reg_n(opcode)),
            4 => self.movbp(bus, reg_m(opcode), reg_n(opcode)),
            5 => self.movwp(bus, reg_m(opcode), reg_n(opcode)),
            6 => self.movlp(bus, reg_m(opcode), reg_n(opcode)),
            7 => self.not(reg_m(opcode), reg_n(opcode)),
            8 => self.swapb(reg_m(opcode), reg_n(opcode)),
            9 => self.swapw(reg_m(opcode), reg_n(opcode)),
            10 => self.negc(reg_m(opcode), reg_n(opcode)),
            11 => self.neg(reg_m(opcode), reg_n(opcode)),
            12 => self.extub(reg_m(opcode), reg_n(opcode)),
            13 => self.extuw(reg_m(opcode), reg_n(opcode)),
            14 => self.extsb(reg_m(opcode), reg_n(opcode)),
            _ => self.extsw(reg_m(opcode), reg_n(opcode)), // 15
        }
    }

    // origin: src/mame/cpu/sh.cpp:1651-1672
    fn op1000<B: Sh2Bus>(&mut self, bus: &mut B, opcode: u16) {
        match (opcode >> 8) & 15 {
            0 => self.movbs4(bus, (opcode & 0x0f) as u32, reg_m(opcode)),
            1 => self.movws4(bus, (opcode & 0x0f) as u32, reg_m(opcode)),
            2 => self.illegal(bus),
            3 => self.illegal(bus),
            4 => self.movbl4(bus, reg_m(opcode), (opcode & 0x0f) as u32),
            5 => self.movwl4(bus, reg_m(opcode), (opcode & 0x0f) as u32),
            6 => self.illegal(bus),
            7 => self.illegal(bus),
            8 => self.cmpim((opcode & 0xff) as u32),
            9 => self.bt((opcode & 0xff) as u32),
            10 => self.illegal(bus),
            11 => self.bf((opcode & 0xff) as u32),
            12 => self.illegal(bus),
            13 => self.bts((opcode & 0xff) as u32),
            14 => self.illegal(bus),
            _ => self.bfs((opcode & 0xff) as u32), // 15
        }
    }

    // origin: src/mame/cpu/sh.cpp:1675-1696
    fn op1100<B: Sh2Bus>(&mut self, bus: &mut B, opcode: u16) {
        match (opcode >> 8) & 15 {
            0 => self.movbsg(bus, (opcode & 0xff) as u32),
            1 => self.movwsg(bus, (opcode & 0xff) as u32),
            2 => self.movlsg(bus, (opcode & 0xff) as u32),
            3 => self.trapa(bus, (opcode & 0xff) as u32), // sh2/4 differ
            4 => self.movblg(bus, (opcode & 0xff) as u32),
            5 => self.movwlg(bus, (opcode & 0xff) as u32),
            6 => self.movllg(bus, (opcode & 0xff) as u32),
            7 => self.mova(bus, (opcode & 0xff) as u32),
            8 => self.tsti((opcode & 0xff) as u32),
            9 => self.andi((opcode & 0xff) as u32),
            10 => self.xori((opcode & 0xff) as u32),
            11 => self.ori((opcode & 0xff) as u32),
            12 => self.tstm(bus, (opcode & 0xff) as u32),
            13 => self.andm(bus, (opcode & 0xff) as u32),
            14 => self.xorm(bus, (opcode & 0xff) as u32),
            _ => self.orm(bus, (opcode & 0xff) as u32), // 15
        }
    }

    // SH4 cases fall through to here too
    // origin: src/mame/cpu/sh.cpp:1699-1773
    fn execute_one_0000<B: Sh2Bus>(&mut self, bus: &mut B, opcode: u16) {
        // 04,05,06,07 always the same, 0c,0d,0e,0f always the same, other change
        // based on upper bits (comment is disk :1701)
        match opcode & 0x3f {
            0x00 => self.illegal(bus),
            0x01 => self.illegal(bus),
            0x02 => self.stcsr(reg_n(opcode)),
            0x03 => self.bsrf(reg_n(opcode)),
            0x04 => self.movbs0(bus, reg_m(opcode), reg_n(opcode)),
            0x05 => self.movws0(bus, reg_m(opcode), reg_n(opcode)),
            0x06 => self.movls0(bus, reg_m(opcode), reg_n(opcode)),
            0x07 => self.mull(reg_m(opcode), reg_n(opcode)),
            0x08 => self.clrt(),
            0x09 => self.nop(),
            0x0a => self.stsmach(reg_n(opcode)),
            0x0b => self.rts(),
            0x0c => self.movbl0(bus, reg_m(opcode), reg_n(opcode)),
            0x0d => self.movwl0(bus, reg_m(opcode), reg_n(opcode)),
            0x0e => self.movll0(bus, reg_m(opcode), reg_n(opcode)),
            0x0f => self.mac_l(bus, reg_m(opcode), reg_n(opcode)),

            0x10 => self.illegal(bus),
            0x11 => self.illegal(bus),
            0x12 => self.stcgbr(reg_n(opcode)),
            0x13 => self.illegal(bus),
            0x14 => self.movbs0(bus, reg_m(opcode), reg_n(opcode)),
            0x15 => self.movws0(bus, reg_m(opcode), reg_n(opcode)),
            0x16 => self.movls0(bus, reg_m(opcode), reg_n(opcode)),
            0x17 => self.mull(reg_m(opcode), reg_n(opcode)),
            0x18 => self.sett(),
            0x19 => self.div0u(),
            0x1a => self.stsmacl(reg_n(opcode)),
            0x1b => self.sleep(),
            0x1c => self.movbl0(bus, reg_m(opcode), reg_n(opcode)),
            0x1d => self.movwl0(bus, reg_m(opcode), reg_n(opcode)),
            0x1e => self.movll0(bus, reg_m(opcode), reg_n(opcode)),
            0x1f => self.mac_l(bus, reg_m(opcode), reg_n(opcode)),

            0x20 => self.illegal(bus),
            0x21 => self.illegal(bus),
            0x22 => self.stcvbr(reg_n(opcode)),
            0x23 => self.braf(reg_n(opcode)),
            0x24 => self.movbs0(bus, reg_m(opcode), reg_n(opcode)),
            0x25 => self.movws0(bus, reg_m(opcode), reg_n(opcode)),
            0x26 => self.movls0(bus, reg_m(opcode), reg_n(opcode)),
            0x27 => self.mull(reg_m(opcode), reg_n(opcode)),
            0x28 => self.clrmac(),
            0x29 => self.movt(reg_n(opcode)),
            0x2a => self.stspr(reg_n(opcode)),
            0x2b => self.rte(bus),
            0x2c => self.movbl0(bus, reg_m(opcode), reg_n(opcode)),
            0x2d => self.movwl0(bus, reg_m(opcode), reg_n(opcode)),
            0x2e => self.movll0(bus, reg_m(opcode), reg_n(opcode)),
            0x2f => self.mac_l(bus, reg_m(opcode), reg_n(opcode)),

            0x30 => self.illegal(bus),
            0x31 => self.illegal(bus),
            0x32 => self.illegal(bus),
            0x33 => self.illegal(bus),
            0x34 => self.movbs0(bus, reg_m(opcode), reg_n(opcode)),
            0x35 => self.movws0(bus, reg_m(opcode), reg_n(opcode)),
            0x36 => self.movls0(bus, reg_m(opcode), reg_n(opcode)),
            0x37 => self.mull(reg_m(opcode), reg_n(opcode)),
            0x38 => self.illegal(bus),
            0x39 => self.illegal(bus),
            0x3a => self.illegal(bus),
            0x3b => self.illegal(bus),
            0x3c => self.movbl0(bus, reg_m(opcode), reg_n(opcode)),
            0x3d => self.movwl0(bus, reg_m(opcode), reg_n(opcode)),
            0x3e => self.movll0(bus, reg_m(opcode), reg_n(opcode)),
            _ => self.mac_l(bus, reg_m(opcode), reg_n(opcode)), // 0x3f
        }
    }

    // SH4 cases fall through to here too
    // origin: src/mame/cpu/sh.cpp:1776-1851
    fn execute_one_4000<B: Sh2Bus>(&mut self, bus: &mut B, opcode: u16) {
        // 0f always the same, others differ (comment is disk :1778)
        match opcode & 0x3f {
            0x00 => self.shll(reg_n(opcode)),
            0x01 => self.shlr(reg_n(opcode)),
            0x02 => self.stsmmach(bus, reg_n(opcode)),
            0x03 => self.stcmsr(bus, reg_n(opcode)),
            0x04 => self.rotl(reg_n(opcode)),
            0x05 => self.rotr(reg_n(opcode)),
            0x06 => self.ldsmmach(bus, reg_n(opcode)),
            0x07 => self.ldcmsr(bus, opcode),
            0x08 => self.shll2(reg_n(opcode)),
            0x09 => self.shlr2(reg_n(opcode)),
            0x0a => self.ldsmach(reg_n(opcode)),
            0x0b => self.jsr(reg_n(opcode)),
            0x0c => self.illegal(bus),
            0x0d => self.illegal(bus),
            0x0e => self.ldcsr(bus, opcode),
            0x0f => self.mac_w(bus, reg_m(opcode), reg_n(opcode)),

            0x10 => self.dt(reg_n(opcode)),
            0x11 => self.cmppz(reg_n(opcode)),
            0x12 => self.stsmmacl(bus, reg_n(opcode)),
            0x13 => self.stcmgbr(bus, reg_n(opcode)),
            0x14 => self.illegal(bus),
            0x15 => self.cmppl(reg_n(opcode)),
            0x16 => self.ldsmmacl(bus, reg_n(opcode)),
            0x17 => self.ldcmgbr(bus, reg_n(opcode)),
            0x18 => self.shll8(reg_n(opcode)),
            0x19 => self.shlr8(reg_n(opcode)),
            0x1a => self.ldsmacl(reg_n(opcode)),
            0x1b => self.tas(bus, reg_n(opcode)),
            0x1c => self.illegal(bus),
            0x1d => self.illegal(bus),
            0x1e => self.ldcgbr(reg_n(opcode)),
            0x1f => self.mac_w(bus, reg_m(opcode), reg_n(opcode)),

            0x20 => self.shal(reg_n(opcode)),
            0x21 => self.shar(reg_n(opcode)),
            0x22 => self.stsmpr(bus, reg_n(opcode)),
            0x23 => self.stcmvbr(bus, reg_n(opcode)),
            0x24 => self.rotcl(reg_n(opcode)),
            0x25 => self.rotcr(reg_n(opcode)),
            0x26 => self.ldsmpr(bus, reg_n(opcode)),
            0x27 => self.ldcmvbr(bus, reg_n(opcode)),
            0x28 => self.shll16(reg_n(opcode)),
            0x29 => self.shlr16(reg_n(opcode)),
            0x2a => self.ldspr(reg_n(opcode)),
            0x2b => self.jmp(reg_n(opcode)),
            0x2c => self.illegal(bus),
            0x2d => self.illegal(bus),
            0x2e => self.ldcvbr(reg_n(opcode)),
            0x2f => self.mac_w(bus, reg_m(opcode), reg_n(opcode)),

            0x30 => self.illegal(bus),
            0x31 => self.illegal(bus),
            0x32 => self.illegal(bus),
            0x33 => self.illegal(bus),
            0x34 => self.illegal(bus),
            0x35 => self.illegal(bus),
            0x36 => self.illegal(bus),
            0x37 => self.illegal(bus),
            0x38 => self.illegal(bus),
            0x39 => self.illegal(bus),
            0x3a => self.illegal(bus),
            0x3b => self.illegal(bus),
            0x3c => self.illegal(bus),
            0x3d => self.illegal(bus),
            0x3e => self.illegal(bus),
            _ => self.mac_w(bus, reg_m(opcode), reg_n(opcode)), // 0x3f
        }
    }

    // origin: src/mame/cpu/sh.cpp:1853-1874
    fn execute_one<B: Sh2Bus>(&mut self, bus: &mut B, opcode: u16) {
        match (opcode >> 12) & 15 {
            0 => self.execute_one_0000(bus, opcode),
            1 => self.movls4(bus, reg_m(opcode), (opcode & 0xf) as u32, reg_n(opcode)),
            2 => self.op0010(bus, opcode),
            3 => self.op0011(bus, opcode),
            4 => self.execute_one_4000(bus, opcode),
            5 => self.movll4(bus, reg_m(opcode), (opcode & 0x0f) as u32, reg_n(opcode)),
            6 => self.op0110(bus, opcode),
            7 => self.addi((opcode & 0xff) as u32, reg_n(opcode)),
            8 => self.op1000(bus, opcode),
            9 => self.movwi(bus, (opcode & 0xff) as u32, reg_n(opcode)),
            10 => self.bra((opcode & 0xfff) as u32),
            11 => self.bsr((opcode & 0xfff) as u32),
            12 => self.op1100(bus, opcode),
            13 => self.movli(bus, (opcode & 0xff) as u32, reg_n(opcode)),
            14 => self.movi((opcode & 0xff) as u32, reg_n(opcode)),
            _ => self.execute_one_f000(bus, opcode), // 15
        }
    }

    /// The interpreter loop — transliteration of sh2_device::execute_run
    /// (origin: src/mame/cpu/sh2.cpp:252-291) minus the DRC: use_jit is const-false
    /// (JIT is M9; the device row pins `use_jit = false` per ledger row
    /// `sh2 device`), so jit_enabled()/jit_run() (:261-266) vanish.
    /// Hook ordering mirrors C++ exactly: instruction() fires at the
    /// debugger_instruction_hook site (:268) BEFORE the fetch (:272), BEFORE the
    /// delay-slot application (:274-280), and BEFORE execute_one (:282).
    /// One fetch/dispatch iteration — the body of the C++ do/while loop. Shared by
    /// `execute_run` and the `run_one` test lever so they can never drift.
    // origin: src/mame/cpu/sh2.cpp:268-290 (execute_run loop body)
    fn step<B: Sh2Bus, H: InstructionHook>(&mut self, bus: &mut B, hook: &mut H) {
        // origin: src/mame/cpu/sh2.cpp:268 (debugger_instruction_hook — the
        // real trace/hash sink per mamecompat.h:75-79, see InstructionHook)
        hook.instruction(self);

        // origin: src/mame/cpu/sh2.cpp:270-271 (jit_trace — DRC-only, absent)
        // origin: src/mame/cpu/sh2.cpp:272 (opcode fetch)
        let opcode = self.fetch_word(bus, self.pc);

        // origin: src/mame/cpu/sh2.cpp:274-280 (delay slot: m_delay != 0 means
        // "this instruction is in the slot"; pc := target now, flag cleared)
        if self.m_delay != 0 {
            self.pc = self.m_delay;
            self.m_delay = 0;
        } else {
            self.pc = self.pc.wrapping_add(2);
        }

        // origin: src/mame/cpu/sh2.cpp:282
        self.execute_one(bus, opcode);

        // origin: src/mame/cpu/sh2.cpp:284-288 (IRQ test deferred while a
        // delay slot is armed) — the ONE in-loop check; P6 seam removed
        // 2026-10-02, no machine-side bypass exists anymore.
        if self.m_test_irq != 0 && self.m_delay == 0 {
            self.check_pending_irq(bus);
            self.m_test_irq = 0;
        }

        // origin: src/mame/cpu/sh2.cpp:289 (icount--)
        self.icount = self.icount.wrapping_sub(1);
    }

    /// Single instruction step with identical code path to `execute_run` (test lever;
    /// skips the m_cpu_off bail so a halted CPU can still be single-stepped).
    pub fn run_one<B: Sh2Bus, H: InstructionHook>(&mut self, bus: &mut B, hook: &mut H) {
        self.step(bus, hook);
    }

    /// M9 JIT callout seam (origin: src/mame/cpu/sh2_jit.cpp:153-156
    /// `jit_exec` — `c->execute_one(u16(opcode))`; the smu-machine JIT
    /// callouts reach the private dispatcher through this shim ONLY).
    pub fn jit_exec_op<B: Sh2Bus>(&mut self, bus: &mut B, op: u16) {
        self.execute_one(bus, op)
    }

    pub fn execute_run<B: Sh2Bus, H: InstructionHook>(&mut self, bus: &mut B, hook: &mut H) {
        // origin: src/mame/cpu/sh2.cpp:254-259 (m_cpu_off bail; debugger_wait_hook
        // is a no-op, mamecompat.h:81)
        if self.m_cpu_off != 0 {
            self.icount = 0;
            return;
        }

        // origin: src/mame/cpu/sh2.cpp:261-266 (use_jit == false in this port)
        // origin: src/mame/cpu/sh2.cpp:290 (do/while icount > 0 — at least one
        // instruction always executes). M9 2026-10-03: batch_stop() ends the
        // burst at an instruction boundary where the machine must run its
        // pumps (device side effect, exception ack, SWP hold) — see Sh2Bus doc.
        loop {
            self.step(bus, hook);
            if !(self.icount > 0) || bus.batch_stop() {
                break;
            }
        }
    }
}
