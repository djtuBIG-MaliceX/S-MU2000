//! M9 SH-2 JIT — transliteration of the x86-64 path of
//! `src/mame/cpu/sh2_jit.cpp` (SMU_X64ASM_MODE == 64 branch; the x86-32 and
//! aarch64 sections are DEAD on this MSYS2 x64 build — sh2_jit.cpp:33-61).
//! Ground truth ranges cited per block as `// origin: sh2_jit.cpp:N`.
//!
//! Register plan (sh2_jit.cpp:57-59 + JIT_S9_HANDOFF):
//!   RBX = *mut JitCtx, RBP = *mut Sh2Core, R12 = ROM base, R13 = RAM base,
//!   R14 = *mut HubNow, R15 = base (m_total + sext32(m_cycles_this_run)).
//! Scratch: RAX, RCX, RDX (address), R8 (write value = JIT_VAL). Win64
//! callee-saved RBX/RBP/R12..R15 survive the callout trampolines.
//!
//! DEVIATIONS vs C++ (all in JIT_S9_HANDOFF.md, approved):
//! - LAZYPC dropped: `jit_head` stores S_pc = at+2 eagerly before EVERY
//!   instruction (the driver's lazy/stale_pc machinery is absent).
//! - Compiled trace/hash dropped: SH2_JIT_TRACE / SH2_JIT_HASH env ⇒ the
//!   whole JIT stays off (interpreter path is trace-identical already);
//!   Machine additionally keeps the JIT off while `hash_on`/`trace_on`
//!   (--hash-pc / --trace-pc, the `smu2000::g_pc_*` check at :232).
//! - Rust batch-stop: dev_dirty is tested in the memop epilogue and in
//!   next_block (the LOAD-BEARING dirty-stop cadence of S6/S7); the C++
//!   loop has no analogue (C++ pumps at mu2000.cpp granularity).
//! - Callout args are Windows x64 only (sysv legs of the C++ are dead).

#![cfg(all(target_arch = "x86_64", target_os = "windows"))]

use std::ffi::c_void;
use std::mem::offset_of;
use std::ptr;

use smu_sh2::core::{Sh2Bus, Sh2Core, SH_M, SH_Q, SH_T};

use crate::jit_emit::{Assembler, Mem, ARG0, ARG1, ARG2, ARG3, R12, R13, R14, R15, R8, RAX, RBP, RBX, RCX, RDX};
use crate::{Ctx, HubNow, RunHook};

// Gate proof (tests/jit.rs `jit_actually_compiles`): every block published
// bumps this (relaxed atomic, once per compile — not per instruction).
pub static JIT_BLOCKS_COMPILED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
// Gate proof (tests/jit.rs `jit_actually_compiles`): every enter bumps this.
pub static JIT_ENTERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// origin: sh2_jit.cpp:78-80 (ROM_END / MAX_INSNS / BUF_SIZE)
const ROM_END: u32 = 0x400_000;
const MAX_INSNS: usize = 48;
const BUF_SIZE: usize = 16 * 1024 * 1024;

// origin: compat/exec_mem.h — VirtualAlloc(MEM_COMMIT|MEM_RESERVE,
// PAGE_EXECUTE_READWRITE); free = VirtualFree(MEM_RELEASE). Hand-declared
// like the smu-hal-win WASAPI vtable FFI (wasapi.rs:79 extern "system").
#[link(name = "kernel32")]
extern "system" {
    fn VirtualAlloc(lpaddress: *mut c_void, dwsize: usize, flallocationtype: u32, flprotect: u32) -> *mut c_void;
    fn VirtualFree(lpaddress: *mut c_void, dwsize: usize, dwfreetype: u32) -> i32;
}
const MEM_COMMIT: u32 = 0x0000_1000;
const MEM_RESERVE: u32 = 0x0000_2000;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const MEM_RELEASE: u32 = 0x8000;

// origin: JIT_S9_HANDOFF §JitCtx — the single block-context bundle. Lives
// INSIDE Jit; run_core refreshes bus/core/hn/base per batch, enter loads
// R14/R15 from offsets 16/24. `bus` is a *mut Ctx cast to c_void.
#[repr(C)]
pub struct JitCtx {
    pub bus: *mut c_void,
    pub core: *mut Sh2Core,
    pub hn: *mut HubNow,
    pub base: u64,
}

// The callout trampolines erase the Ctx lifetime ('static laundering — the
// pointer is a live &mut Ctx only while entered JIT code runs, strictly
// inside the batch borrow, single-threaded).
type Bus = Ctx<'static>;

// origin: sh2_jit.cpp:221 jit_rb — c->read_byte(a) (masked core read parity)
unsafe extern "system" fn jit_rb(ctx: *mut JitCtx, a: u32) -> u32 {
    let j = &*ctx;
    let core = &*j.core;
    let bus = &mut *(j.bus as *mut Bus);
    core.read_byte(bus, a) as u32
}
// origin: sh2_jit.cpp:222 jit_rw
unsafe extern "system" fn jit_rw(ctx: *mut JitCtx, a: u32) -> u32 {
    let j = &*ctx;
    let core = &*j.core;
    let bus = &mut *(j.bus as *mut Bus);
    core.read_word(bus, a) as u32
}
// origin: sh2_jit.cpp:223 jit_rl
unsafe extern "system" fn jit_rl(ctx: *mut JitCtx, a: u32) -> u32 {
    let j = &*ctx;
    let core = &*j.core;
    let bus = &mut *(j.bus as *mut Bus);
    core.read_long(bus, a)
}
// origin: sh2_jit.cpp:224 jit_wb — write_byte(a, u8(v))
unsafe extern "system" fn jit_wb(ctx: *mut JitCtx, a: u32, v: u32) {
    let j = &*ctx;
    let core = &mut *j.core;
    let bus = &mut *(j.bus as *mut Bus);
    core.write_byte(bus, a, v as u8);
}
// origin: sh2_jit.cpp:225 jit_ww
unsafe extern "system" fn jit_ww(ctx: *mut JitCtx, a: u32, v: u32) {
    let j = &*ctx;
    let core = &mut *j.core;
    let bus = &mut *(j.bus as *mut Bus);
    core.write_word(bus, a, v as u16);
}
// origin: sh2_jit.cpp:226 jit_wl
unsafe extern "system" fn jit_wl(ctx: *mut JitCtx, a: u32, v: u32) {
    let j = &*ctx;
    let core = &mut *j.core;
    let bus = &mut *(j.bus as *mut Bus);
    core.write_long(bus, a, v);
}
// origin: sh2_jit.cpp:153-156 jit_exec — c->execute_one(u16(opcode)) via
// the core.rs shim (rust smu-sh2 core.rs jit_exec_op)
unsafe extern "system" fn jit_exec(ctx: *mut JitCtx, op: u32) {
    let j = &*ctx;
    let core = &mut *j.core;
    let bus = &mut *(j.bus as *mut Bus);
    core.jit_exec_op(bus, op as u16);
}
// origin: sh2_jit.cpp:215-219 jit_irq — check_pending_irq; m_test_irq = 0
unsafe extern "system" fn jit_irq(ctx: *mut JitCtx) {
    let j = &*ctx;
    let core = &mut *j.core;
    let bus = &mut *(j.bus as *mut Bus);
    core.check_pending_irq(bus);
    core.m_test_irq = 0;
}

fn fnptr(f: unsafe extern "system" fn(*mut JitCtx, u32) -> u32) -> u64 {
    f as *const () as u64
}
fn fnptr2(f: unsafe extern "system" fn(*mut JitCtx, u32, u32)) -> u64 {
    f as *const () as u64
}
fn fnptr1(f: unsafe extern "system" fn(*mut JitCtx, u32)) -> u64 {
    f as *const () as u64
}
fn fnptr0(f: unsafe extern "system" fn(*mut JitCtx)) -> u64 {
    f as *const () as u64
}

// origin: JIT_S9_HANDOFF §Rust design — env flags precomputed at Jit::new.
fn env_flag(name: &str, off: &str) -> bool {
    // C++ idiom: !(e && e[0]==off[0]) — set AND first char 'off' ⇒ false
    match std::env::var_os(name) {
        Some(v) => !v.to_string_lossy().starts_with(off),
        None => true,
    }
}

// origin: sh2_jit.cpp:268 enum class kind { normal, delayed, ends }
#[derive(PartialEq, Clone, Copy)]
enum Kind {
    Normal,
    Delayed,
    Ends,
}

// origin: sh2_jit.cpp:270-312 classify — VERBATIM transliteration
fn classify(op: u16) -> Kind {
    match op >> 12 {
        0x0 => match op & 0x3f {
            0x03 | 0x0b | 0x23 | 0x2b => Kind::Delayed, // BSRF, RTS, BRAF, RTE
            // SLEEP + ILLEGAL set
            0x1b | 0x00 | 0x01 | 0x10 | 0x11 | 0x13 | 0x20 | 0x21 | 0x30 | 0x31 | 0x32 | 0x33 | 0x38 | 0x39
            | 0x3a | 0x3b => Kind::Ends,
            _ => Kind::Normal,
        },
        0x2 => {
            if op & 15 == 3 {
                Kind::Ends
            } else {
                Kind::Normal
            }
        }
        0x3 => {
            if op & 15 == 1 || op & 15 == 9 {
                Kind::Ends
            } else {
                Kind::Normal
            }
        }
        0x4 => {
            match op & 0x3f {
                0x0b | 0x2b => return Kind::Delayed, // JSR, JMP
                0x0c | 0x0d | 0x14 | 0x1c | 0x1d | 0x2c | 0x2d => return Kind::Ends,
                _ => {}
            }
            if (op & 0x3f) >= 0x30 && (op & 0x3f) != 0x3f {
                Kind::Ends
            } else {
                Kind::Normal
            }
        }
        0x8 => {
            match (op >> 8) & 15 {
                0xd | 0xf => Kind::Delayed, // BTS, BFS
                0x9 | 0xb | 0x2 | 0x3 | 0x6 | 0x7 | 0xa | 0xc | 0xe => Kind::Ends, // BT/BF + others
                _ => Kind::Normal,
            }
        }
        0xa | 0xb => Kind::Delayed, // BRA, BSR
        0xc => {
            if (op >> 8) & 15 == 3 {
                Kind::Ends // TRAPA
            } else {
                Kind::Normal
            }
        }
        0xf => Kind::Ends,
        _ => Kind::Normal,
    }
}

// origin: sh2_jit.cpp:579 enum res { none, pure, memop, delayed, ends }
#[derive(PartialEq, Clone, Copy)]
enum Res {
    None,
    Pure,
    Memop,
    Delayed,
    Ends,
}

// Compile-time struct offsets (C++ computes intptr_t(f)-intptr_t(st) at
// runtime :350; Rust proves them statically via offset_of! — same numbers,
// pinned numerically in tests/jit.rs).
struct Offs {
    pc: i32,
    pr: i32,
    sr: i32,
    mach: i32,
    macl: i32,
    r: i32,
    ea: i32,
    icount: i32,
    gbr: i32,
    vbr: i32,
    delay: i32,
    test_irq: i32,
    ctx_bus: i32,
    dev_dirty: i32,
    hn_now: i32,
    hn_in_event: i32,
    hn_pc: i32,
    hn_pre_seam: i32,
}

fn offs() -> Offs {
    Offs {
        pc: offset_of!(Sh2Core, pc) as i32,
        pr: offset_of!(Sh2Core, pr) as i32,
        sr: offset_of!(Sh2Core, sr) as i32,
        mach: offset_of!(Sh2Core, mach) as i32,
        macl: offset_of!(Sh2Core, macl) as i32,
        r: offset_of!(Sh2Core, r) as i32,
        ea: offset_of!(Sh2Core, ea) as i32,
        icount: offset_of!(Sh2Core, icount) as i32,
        gbr: offset_of!(Sh2Core, gbr) as i32,
        vbr: offset_of!(Sh2Core, vbr) as i32,
        delay: offset_of!(Sh2Core, m_delay) as i32,
        test_irq: offset_of!(Sh2Core, m_test_irq) as i32,
        ctx_bus: offset_of!(Ctx, bus) as i32,
        dev_dirty: offset_of!(smu_sh2::sh7042::Sh7042Bus, dev_dirty) as i32,
        hn_now: offset_of!(HubNow, now) as i32,
        hn_in_event: offset_of!(HubNow, in_event) as i32,
        hn_pc: offset_of!(HubNow, pc) as i32,
        hn_pre_seam: offset_of!(HubNow, pre_seam) as i32,
    }
}

pub struct Jit {
    // origin: sh2_jit.cpp:82-88 (buf/used/pages/enter/next_block/entry/base_used)
    buf: *mut u8,
    used: usize,
    base_used: usize,
    // stable-baked page table (the C++ pages.data() array — sh2_jit.cpp:409
    // bakes the pointer); pinned via Box::leak, NEVER moved.
    pages_pin: *mut usize,
    // enter jumps through this stable slot (C++ entry is a jit-member; the
    // Rust Jit lives in Machine and MOVES — pin it).
    entry_pin: *mut usize,
    next_off: usize, // offset of next_block inside buf (buf+off stable)
    // owned 0x800-entry pages (flush frees them; pin entries nulled first)
    pages: Vec<Box<[usize; 0x800]>>,
    jit_ctx: JitCtx,
    // env flags (precomputed at new — "env lookups read once", Pitfalls)
    enabled: bool,
    native_enabled: bool,
    slot_native: bool,
    // compile-window immediates (membus hot parity: ROM 0..0x3fffff, RAM
    // 0x400000..+0x40000 — checked in compile(); baked into every block)
    rom_end: u32,
    ram_start: u32,
    ram_len: u32,
}

impl Jit {
    /// Construct with env flags decided once (machine-level can_jit).
    pub fn new() -> Jit {
        // origin: sh2_jit.cpp:124-135 jit_enabled: !(e && e[0]=='0')
        let enabled = env_flag("SMU2000_SH2_JIT", "0");
        // origin: sh2_jit.cpp:138-145 native_enabled: !(e && e[0]=='1')
        let native_enabled = env_flag("SMU2000_SH2_JIT", "1");
        // origin: sh2_jit.cpp:853-856 slot_native: !(e && e[0]=='0')
        let slot_native = env_flag("SMU2000_SH2_SLOTNATIVE", "0");
        // DEVIATION (handoff): trace/hash in COMPILED code dropped — the env
        // vars (even malformed ones) force the whole JIT off; the
        // interpreter keeps the proven trace equivalence.
        let trace_off = std::env::var_os("SH2_JIT_TRACE").is_none();
        let hash_off = std::env::var_os("SH2_JIT_HASH").is_none();
        let enabled = enabled && trace_off && hash_off;
        Jit {
            buf: ptr::null_mut(), // alloc on first compile (C++ :445)
            used: 0,
            base_used: 0,
            pages_pin: Box::leak(Box::new([0usize; 0x400])).as_mut_ptr(),
            entry_pin: Box::leak(Box::new(0usize)),
            next_off: 0,
            pages: Vec::new(),
            jit_ctx: JitCtx {
                bus: ptr::null_mut(),
                core: ptr::null_mut(),
                hn: ptr::null_mut(),
                base: 0,
            },
            enabled,
            native_enabled,
            slot_native,
            rom_end: 0x3fff_ff,
            ram_start: 0x40_0000,
            ram_len: 0x4_0000,
        }
    }

    /// Machine-level gate: cfg(x86_64, windows) is compile-time (this file);
    /// runtime = the env flags. hash_on/trace_on are checked by the caller
    /// (mirror of the g_pc_trace/g_pc_hash bail at sh2_jit.cpp:232).
    pub fn can(&self) -> bool {
        self.enabled
    }

    // origin: sh2_jit.cpp:98-106 slot(pc) — lazily allocate the 0x800-entry
    // page, return the slot for pc (pin-stable data pointer).
    fn slot_ptr(&mut self, pc: u32) -> *mut usize {
        unsafe {
            let p = (pc >> 12) as usize;
            let mut q = *self.pages_pin.add(p);
            if q == 0 {
                let mut page: Box<[usize; 0x800]> = Box::new([0usize; 0x800]);
                q = page.as_mut_ptr() as usize;
                *self.pages_pin.add(p) = q;
                self.pages.push(page);
            }
            (q as *mut usize).add(((pc & 0xfff) >> 1) as usize)
        }
    }

    /// origin: sh2_jit.cpp:108-113 flush (jit_flush :147-151) — drop every
    /// translated block; the prologue/buf/pin survive (base_used).
    pub fn flush(&mut self) {
        unsafe {
            for i in 0..0x400 {
                *self.pages_pin.add(i) = 0;
            }
        }
        self.pages.clear();
        self.used = self.base_used;
    }

    /// origin: sh2_jit.cpp:334-440 init — alloc RWX buffer + emit the enter
    /// prologue and next_block at the buffer head. Returns false on alloc
    /// failure (host then interprets forever).
    fn init(&mut self) -> bool {
        // origin: sh2_jit.cpp:337-339 + exec_mem.h VirtualAlloc
        unsafe {
            self.buf = VirtualAlloc(ptr::null_mut(), BUF_SIZE, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE)
                as *mut u8;
        }
        if self.buf.is_null() {
            return false;
        }
        let o = offs();
        let mut a = Assembler::new();

        // ---- enter (origin: sh2_jit.cpp:367-376 x64 branch) ----
        // enter: rbx rbp r12 r13 を保って、entry へ飛ぶ
        a.push(RBX);
        a.push(RBP);
        a.push(R12);
        a.push(R13);
        // S11 FIX: the C++ enter pushes only rbx/rbp/r12/r13 (:367-369) —
        // its caller happens to leave r14/r15 dead. R14/R15 are STILL
        // non-volatile in Win64, and rustc keeps live values (e.g. `ramp`)
        // in callee-saved regs across the enter() call — the JIT returned
        // with r14=hn/r15=base and the caller passed BASE as the next
        // enter's RAM arg (R13=0x10000 = the S10 corruption, and the
        // 0xc0000005 byte-store on top of it). Save/restore both here;
        // 6 pushes keep rsp 16-aligned with the existing sub rsp,40.
        a.push(R14);
        a.push(R15);
        a.subrsp(40); // 影 32 + 詰め物 8。rsp は 16 の倍数になる (:369)
        a.mov64(RBX, ARG0); // rcx = *mut JitCtx
        a.mov64(RBP, ARG1); // rdx = *mut Sh2Core  (状態は rbp :371)
        a.mov64(R12, ARG2); // r8  = ROM base
        a.mov64(R13, ARG3); // r9  = RAM base
        // DEVIATION (handoff §Registers): two extra loads — R14 = JitCtx.hn,
        // R15 = JitCtx.base (jit_head's clock chain inputs).
        a.load64(R14, Mem::b(RBX, offset_of!(JitCtx, hn) as i32));
        a.load64(R15, Mem::b(RBX, offset_of!(JitCtx, base) as i32));
        a.imm64(RAX, self.entry_pin as *const usize as u64); // &entry (pinned)
        a.load64(RAX, Mem::b(RAX, 0));
        a.rr(0, false, &[0xff], 4, RAX); // jmp rax (:376)

        // ---- next_block (origin: sh2_jit.cpp:379-428 x64 branch) ----
        // jit_run が見ることを見て、次のブロックが訳してあればそこへ飛ぶ
        let next = a.code.len();
        let mut to_exit: Vec<usize> = Vec::new();
        a.cmp32i_mem(Mem::b(RBP, o.icount), 0);
        to_exit.push(a.jcc_fwd(0x8e)); // jle
        a.load32(RAX, Mem::b(RBP, o.delay));
        a.test32(RAX, RAX);
        to_exit.push(a.jcc_fwd(0x85));
        a.load32(RAX, Mem::b(RBP, o.test_irq)); // C_test via RBP (RBX=JitCtx)
        a.test32(RAX, RAX);
        to_exit.push(a.jcc_fwd(0x85));
        a.load32(RAX, Mem::b(RBP, o.pc));
        a.cmp32ri(RAX, ROM_END - 0x100);
        to_exit.push(a.jcc_fwd(0x83)); // jae
        a.test32ri(RAX, 1);
        to_exit.push(a.jcc_fwd(0x85));
        // DEVIATION (handoff §memop): Rust dirty-stop gate BEFORE the page
        // lookup — TWO derefs: mov rdx,[rbx] (JitCtx.bus → Ctx*), mov
        // rdx,[rdx+ctx_bus] (Ctx.bus → Sh7042Bus*), test byte
        // [rdx+dev_dirty], jnz. (The C++ :411 chain has ONE deref because the
        // jit object holds the bus directly — S10 bug: a single deref here
        // read heap garbage past Ctx, so the gate never fired and dirty
        // batches chained on.) (RDX instead of the handoff's RCX: RCX must
        // keep pc here.)
        a.load64(RDX, Mem::b(RBX, offset_of!(JitCtx, bus) as i32));
        a.load64(RDX, Mem::b(RDX, o.ctx_bus));
        a.loadu8(RDX, Mem::b(RDX, o.dev_dirty));
        a.test32(RDX, RDX);
        to_exit.push(a.jcc_fwd(0x85));
        a.mov32(RCX, RAX);
        a.shr32(RCX, 12);
        // pages lookup (x64 branch :409-418) — pin address baked once
        a.imm64(RDX, self.pages_pin as *const usize as u64);
        a.load64(RDX, Mem { base: RDX, index: RCX, scale: 8, disp: 0 });
        a.test64(RDX, RDX);
        to_exit.push(a.jcc_fwd(0x84));
        a.and32i(RAX, 0xfff);
        a.shr32(RAX, 1);
        a.load64(RAX, Mem { base: RDX, index: RAX, scale: 8, disp: 0 });
        a.test64(RAX, RAX);
        to_exit.push(a.jcc_fwd(0x84));
        a.rr(0, false, &[0xff], 4, RAX); // jmp rax (:418)
        for p in to_exit {
            a.patch(p);
        }
        // epilogue ret (x64 branch :425-428) — S11: mirrors the added
        // push r14/r15 above (LIFO order).
        a.addrsp(40);
        a.pop(R15);
        a.pop(R14);
        a.pop(R13);
        a.pop(R12);
        a.pop(RBP);
        a.pop(RBX);
        a.ret();

        // origin: sh2_jit.cpp:435-438 memcpy into buf; enter/next/base_used
        unsafe {
            std::ptr::copy_nonoverlapping(a.code.as_ptr(), self.buf, a.code.len());
        }
        self.next_off = next;
        self.base_used = a.code.len();
        self.used = a.code.len();
        true
    }

    /// origin: sh2_jit.cpp:98-106/245-259/1006-1014 — the jit_run driver
    /// per burst: gate, slot lookup, host-side compile on miss, enter.
    /// Fallback = `run_one` (one interpreter insn WITH the RunHook seam).
    /// Return contract mirrors core.rs run_cycles (core.rs:321-330).
    pub fn run_core(&mut self, core: &mut Sh2Core, ctx: &mut Ctx, hook: &mut RunHook, cycles: i32) -> i32 {
        // origin: core.rs:322-323 (run_cycles head — same icount/ctr setup)
        core.icount = cycles;
        core.m_cycles_this_run = cycles;
        // origin: core.rs:2430-2433 (execute_run m_cpu_off bail: icount=0,
        // run nothing — the done math below then yields 0 like run_cycles)
        if core.m_cpu_off != 0 {
            core.icount = 0;
        } else {
            let corep: *mut Sh2Core = &mut *core;
            let ctxp: *mut Ctx = &mut *ctx;
            let hookp: *mut RunHook = &mut *hook;
            // origin: sh2_jit.cpp:259 enter(this, state, hot_rom(), hot_ram())
            // — the hot pins re-read per burst; the Box/Vec never moves.
            let (romp, ramp): (*const u8, *mut u8) = unsafe {
                let bus = &mut *(*ctxp).bus;
                (bus.rom.as_ptr(), bus.ram.as_mut_ptr())
            };
            // JitCtx refresh per batch (hn ptr stable inside its RefCell)
            unsafe {
                let hn_ptr: *mut HubNow = {
                    let mut b = (*ctxp).hn.borrow_mut();
                    ptr::from_mut(&mut *b)
                };
                let base = (*corep).m_total_cycles.wrapping_add((*corep).m_cycles_this_run as i64 as u64);
                self.jit_ctx = JitCtx {
                    bus: ctxp as *mut c_void,
                    core: corep,
                    hn: hn_ptr,
                    // base = m_total + sext32(ctr) — sext32 is a Z/2^32→Z/2^64
                    // group homomorphism, so R15 − sext32(icount) ==
                    // total_cycles() (core.rs:316-318) at every jit_head.
                    base,
                };
            }
            loop {
                // origin: sh2_jit.cpp:232-242 jit_run gates (delay/test_irq/
                // pc range/parity) — a gate miss runs ONE interpreter insn
                // (the C++ execute_run continue), never a spin.
                let c = unsafe { &mut *corep };
                let do_jit = c.m_delay == 0
                    && c.m_test_irq == 0
                    && c.pc < ROM_END - 0x100
                    && c.pc & 1 == 0;
                let mut ran_jit = false;
                if do_jit {
                    let pc = c.pc;
                    let mut code = unsafe { *self.slot_ptr(pc) };
                    if code == 0 {
                        // 訳している途中で置き場が一杯になると全部捨てるので、
                        // 置き場は訳した後に引き直す (:246-251)
                        let got = self.compile(unsafe { &mut *corep }, unsafe { &mut *ctxp }, pc);
                        if got != 0 {
                            code = got;
                            unsafe {
                                *self.slot_ptr(pc) = code;
                            }
                        }
                    }
                    if code != 0 {
                        unsafe {
                            *self.entry_pin = code;
                            JIT_ENTERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            // enter(this, state, rom, ram) — see init prologue
                            type EnterT = unsafe extern "system" fn(*mut JitCtx, *mut Sh2Core, *const u8, *mut u8);
                            let f: EnterT = std::mem::transmute::<*mut u8, EnterT>(self.buf);
                            f(&mut self.jit_ctx as *mut JitCtx, corep, romp, ramp);
                        }
                        ran_jit = true;
                    }
                }
                if !ran_jit {
                    unsafe {
                        (*corep).run_one(&mut *ctxp, &mut *hookp);
                    }
                }
                // origin: core.rs:2440-2445 (execute_run do/while tail:
                // !(icount > 0) || batch_stop() ends the burst)
                unsafe {
                    if !((*corep).icount > 0) || (*ctxp).batch_stop() {
                        break;
                    }
                }
            }
        }
        // origin: core.rs:325-329 (done math, sign-extended into m_total)
        let done = core.m_cycles_this_run.wrapping_sub(core.icount);
        debug_assert!(done > 0, "jit_run_core done<=0 ctr={} ic={} pc={:08x} ti={} dl={:x} off={}", core.m_cycles_this_run, core.icount, core.pc, core.m_test_irq, core.m_delay, core.m_cpu_off);
        core.m_total_cycles += done as i64 as u64;
        core.m_cycles_this_run = 0;
        core.icount = 0;
        done
    }

    /// origin: sh2_jit.cpp:442-1015 compile — window checks (:448-470),
    /// per-opcode emission (:576-845 native + :872-960 driver loop),
    /// finish/ret tails (:966-1004), buffer publish (:1006-1014).
    /// Returns the block address (buf offset) or 0 on refusal.
    fn compile(&mut self, core: &mut Sh2Core, ctx: &mut Ctx, pc: u32) -> usize {
        // origin: sh2_jit.cpp:445-446
        if self.buf.is_null() && !self.init() {
            return 0;
        }
        // origin: sh2_jit.cpp:448-456 — 「番地を折り返さない設定のときだけ訳す」
        // (membus hot() parity). Rust concretions: m_am all-ones; ROM exactly
        // 0x400000 (→ rom_end 0x3fffff); RAM region exact 0x400000/0x40000.
        // The C++ :455 range aborts are then provably false (documented).
        if core.m_am != 0xffff_ffff {
            return 0;
        }
        if ctx.bus.rom.len() != 0x40_0000 {
            return 0;
        }
        let rom_end = self.rom_end;
        let ram_start = self.ram_start;
        let ram_len = self.ram_len;

        let o = offs();
        let mut a = Assembler::new();
        // origin: sh2_jit.cpp:464-468 S(f)/R(n)/C_test mem builders
        let s_pc = || Mem::b(RBP, o.pc);
        let s_delay = || Mem::b(RBP, o.delay);
        let s_icount = || Mem::b(RBP, o.icount);
        let s_ea = || Mem::b(RBP, o.ea);
        let s_sr = || Mem::b(RBP, o.sr);
        let s_pr = || Mem::b(RBP, o.pr);
        let s_gbr = || Mem::b(RBP, o.gbr);
        let s_vbr = || Mem::b(RBP, o.vbr);
        let s_mach = || Mem::b(RBP, o.mach);
        let s_macl = || Mem::b(RBP, o.macl);
        let s_test = || Mem::b(RBP, o.test_irq); // C_test — RBX=JitCtx here
        let r = |n: u32| Mem::b(RBP, o.r + 4 * n as i32);
        let hn_pc = || Mem::b(R14, o.hn_pc);
        let hn_now = || Mem::b(R14, o.hn_now);
        let hn_seam = || Mem::b(R14, o.hn_pre_seam);
        let hn_in_event = || Mem::b(R14, o.hn_in_event);
        // JitCtx.bus → Ctx*, then Ctx.bus → Sh7042Bus* — TWO derefs (S10 fix:
        // the C++ jit object holds the bus directly, so its chain needs only
        // one; a single load here yielded a Ctx* and every field read after it
        // was type-wrong — the dirty gate never fired and the R13 guard
        // compared RAM base against Ctx.hn).
        let load_bus = |a: &mut Assembler, reg: u8| {
            a.load64(reg, Mem::b(RBX, offset_of!(JitCtx, bus) as i32));
            a.load64(reg, Mem::b(reg, o.ctx_bus));
        };

        let mut to_finish: Vec<usize> = Vec::new();
        let mut to_ret: Vec<usize> = Vec::new();

        // origin: sh2_jit.cpp:486-490 setT — al 0/1 → SR.T
        let set_t = |a: &mut Assembler| {
            a.movzx8(RAX, RAX);
            a.and32i_mem(s_sr(), !SH_T);
            a.or32mr(s_sr(), RAX);
        };
        // origin: sh2_jit.cpp:491-528 mread(sz) — addr in edx, value eax.
        // BE loads: word = loadu16+bswap+shr16, long = load32+bswap.
        let mread = |a: &mut Assembler, sz: u32| {
            let mut to_slow: Vec<usize> = Vec::new();
            let mut to_done: Vec<usize> = Vec::new();
            if sz > 1 {
                a.test32ri(RDX, 1);
                to_slow.push(a.jcc_fwd(0x85));
            }
            a.cmp32ri(RDX, rom_end + 1 - sz);
            let not_rom = a.jcc_fwd(0x87); // ja
            let mr = Mem { base: R12, index: RDX, scale: 1, disp: 0 };
            if sz == 1 {
                a.loadu8(RAX, mr);
            } else if sz == 2 {
                a.loadu16(RAX, mr);
                a.bswap32(RAX);
                a.shr32(RAX, 16);
            } else {
                a.load32(RAX, mr);
                a.bswap32(RAX);
            }
            to_done.push(a.jmp_fwd());
            a.patch(not_rom);
            a.lea32(RAX, Mem::b(RDX, -(ram_start as i32)));
            a.cmp32ri(RAX, ram_len + 1 - sz);
            to_slow.push(a.jcc_fwd(0x87));
            let mw = Mem { base: R13, index: RAX, scale: 1, disp: 0 };
            if sz == 1 {
                a.loadu8(RAX, mw);
            } else if sz == 2 {
                a.loadu16(RAX, mw);
                a.bswap32(RAX);
                a.shr32(RAX, 16);
            } else {
                a.load32(RAX, mw);
                a.bswap32(RAX);
            }
            to_done.push(a.jmp_fwd());
            for p in to_slow {
                a.patch(p);
            }
            // callout legs (x64 non-sysv :523-525)
            a.mov64(ARG0, RBX);
            a.call_abs(if sz == 1 {
                fnptr(jit_rb)
            } else if sz == 2 {
                fnptr(jit_rw)
            } else {
                fnptr(jit_rl)
            });
            for p in to_done {
                a.patch(p);
            }
        };
        // origin: sh2_jit.cpp:531-574 mwrite(sz) — addr edx, value r8d
        // (JIT_VAL). ONLY RAM fast-writes; everything else callout
        // (membus hot_w parity :552-573).
        let mwrite = |a: &mut Assembler, sz: u32| {
            let mut to_slow: Vec<usize> = Vec::new();
            if sz > 1 {
                a.test32ri(RDX, 1);
                to_slow.push(a.jcc_fwd(0x85));
            }
            a.lea32(RAX, Mem::b(RDX, -(ram_start as i32)));
            a.cmp32ri(RAX, ram_len + 1 - sz);
            to_slow.push(a.jcc_fwd(0x87));
            let mw = Mem { base: R13, index: RAX, scale: 1, disp: 0 };
            if sz == 1 {
                a.store8(mw, R8); // R8B ⇒ REX.R forced via rm() (vector-tested)
            } else if sz == 2 {
                a.mov32(RCX, R8);
                a.bswap32(RCX);
                a.shr32(RCX, 16);
                a.store16(mw, RCX);
            } else {
                a.mov32(RCX, R8);
                a.bswap32(RCX);
                a.store32(mw, RCX);
            }
            let done = a.jmp_fwd();
            for p in to_slow {
                a.patch(p);
            }
            a.mov64(ARG0, RBX);
            a.call_abs(if sz == 1 {
                fnptr2(jit_wb)
            } else if sz == 2 {
                fnptr2(jit_ww)
            } else {
                fnptr2(jit_wl)
            });
            a.patch(done);
        };

        // origin: sh2_jit.cpp:576 sext8
        let sext8 = |v: u32| -> u32 { ((v as u8) as i8) as i32 as u32 };

        // origin: sh2_jit.cpp:580-845 native(op, at) — VERBATIM opcode
        // emission incl. per-opcycle icount charges (:604-605 branch_to,
        // :631 MUL.L, :648/:734 RTS/JSR/JMP, BT/BF via branch_to :781).
        // PC-relative literals (0x9/0xd) fold their ROM word host-side —
        // the C++ :793/:837 bus read with the same ea window checks.
        let native = |a: &mut Assembler, core: &mut Sh2Core, ctx: &mut Ctx, op: u16, at: u32| -> Res {
            let n = ((op >> 8) & 15) as u32;
            let m = ((op >> 4) & 15) as u32;
            let pcv = at + 2; // 実行中の pc (:582)
            // rd_ea / wr_ea / to_r / cmp_t / copy / branch_to (:583-606)
            let rd_ea = |a: &mut Assembler, base: u32, disp: u32, sz: u32| {
                a.load32(RDX, r(base));
                if disp != 0 {
                    a.add32ri(RDX, disp);
                }
                a.store32(s_ea(), RDX);
                mread(a, sz);
            };
            let wr_ea = |a: &mut Assembler, base: u32, disp: u32, src: u32, sz: u32| {
                a.load32(RDX, r(base));
                if disp != 0 {
                    a.add32ri(RDX, disp);
                }
                a.store32(s_ea(), RDX);
                a.load32(R8, r(src));
                mwrite(a, sz);
            };
            let to_r = |a: &mut Assembler, d: u32, sz: u32| {
                if sz == 1 {
                    a.movsx8(RAX, RAX);
                } else if sz == 2 {
                    a.movsx16(RAX, RAX);
                }
                a.store32(r(d), RAX);
            };
            let cmp_t = |a: &mut Assembler, cc: u8| -> Res {
                a.load32(RAX, r(n));
                a.cmp32rm(RAX, r(m));
                a.setcc(cc, RAX);
                set_t(a);
                Res::Pure
            };
            let copy = |a: &mut Assembler, from: Mem, to: Mem| -> Res {
                a.load32(RAX, from);
                a.store32(to, RAX);
                Res::Pure
            };
            let branch_to = |a: &mut Assembler, target: u32, delay: bool| {
                if delay {
                    a.store32i(s_delay(), target);
                    a.store32i(s_ea(), target);
                    a.sub32i_mem(s_icount(), 1);
                } else {
                    a.store32i(s_pc(), target);
                    a.store32i(s_ea(), target);
                    a.sub32i_mem(s_icount(), 2);
                }
            };

            match op >> 12 {
                0x0 => match op & 0x3f {
                    0x04 | 0x14 | 0x24 | 0x34 | 0x05 | 0x15 | 0x25 | 0x35 | 0x06 | 0x16 | 0x26 | 0x36 => {
                        // MOV.x Rm,@(R0,Rn) (:611-619)
                        let sz = 1u32 << ((op & 15) - 4);
                        a.load32(RDX, r(n));
                        a.add32rm(RDX, r(0));
                        a.store32(s_ea(), RDX);
                        a.load32(R8, r(m));
                        mwrite(a, sz);
                        Res::Memop
                    }
                    0x0c | 0x1c | 0x2c | 0x3c | 0x0d | 0x1d | 0x2d | 0x3d | 0x0e | 0x1e | 0x2e | 0x3e => {
                        // MOV.x @(R0,Rm),Rn (:620-628)
                        let sz = 1u32 << ((op & 15) - 12);
                        a.load32(RDX, r(m));
                        a.add32rm(RDX, r(0));
                        a.store32(s_ea(), RDX);
                        mread(a, sz);
                        to_r(a, n, sz);
                        Res::Memop
                    }
                    0x07 | 0x17 | 0x27 | 0x37 => {
                        // MUL.L (:629-632)
                        a.load32(RAX, r(n));
                        a.load32(RCX, r(m));
                        a.imul32(RAX, RCX);
                        a.store32(s_macl(), RAX);
                        a.sub32i_mem(s_icount(), 1); // extra charge (:631)
                        Res::Pure
                    }
                    0x08 => {
                        a.and32i_mem(s_sr(), !SH_T);
                        Res::Pure // CLRT (:633)
                    }
                    0x18 => {
                        a.or32i_mem(s_sr(), SH_T);
                        Res::Pure // SETT (:634)
                    }
                    0x19 => {
                        a.and32i_mem(s_sr(), !(SH_M | SH_Q | SH_T));
                        Res::Pure // DIV0U (:635)
                    }
                    0x09 => Res::Pure, // NOP (:636)
                    0x28 => {
                        a.store32i(s_mach(), 0);
                        a.store32i(s_macl(), 0);
                        Res::Pure // CLRMAC (:637)
                    }
                    0x02 => copy(a, s_sr(), r(n)),   // STC SR,Rn (:638)
                    0x12 => copy(a, s_gbr(), r(n)),  // (:639)
                    0x22 => copy(a, s_vbr(), r(n)),  // (:640)
                    0x0a => copy(a, s_mach(), r(n)), // STS MACH (:641)
                    0x1a => copy(a, s_macl(), r(n)), // STS MACL (:642)
                    0x2a => copy(a, s_pr(), r(n)),   // STS PR (:643)
                    0x29 => {
                        // MOVT (:644-646)
                        a.load32(RAX, s_sr());
                        a.and32i(RAX, SH_T);
                        a.store32(r(n), RAX);
                        Res::Pure
                    }
                    0x0b => {
                        // RTS (:647-649)
                        a.load32(RAX, s_pr());
                        a.store32(s_delay(), RAX);
                        a.store32(s_ea(), RAX);
                        a.sub32i_mem(s_icount(), 1);
                        Res::Delayed
                    }
                    _ => Res::None,
                },
                0x1 => {
                    // MOV.L Rm,@(disp,Rn) (:652-654)
                    wr_ea(a, n, (op & 15) as u32 * 4, m, 4);
                    Res::Memop
                }
                0x2 => match op & 15 {
                    0 | 1 | 2 => {
                        // MOV.x Rm,@Rn (:657-661)
                        let sz = 1u32 << (op & 15);
                        wr_ea(a, n, 0, m, sz);
                        Res::Memop
                    }
                    4 | 5 | 6 => {
                        // MOV.x Rm,@-Rn (:662-669) — note: NO S_ea store,
                        // faithful to C++ (the core handler's ea parity)
                        let sz = 1u32 << ((op & 15) - 4);
                        a.load32(R8, r(m));
                        a.sub32i_mem(r(n), sz);
                        a.load32(RDX, r(n));
                        mwrite(a, sz);
                        Res::Memop
                    }
                    8 => {
                        // TST (:670)
                        a.load32(RAX, r(n));
                        a.test32rm(RAX, r(m));
                        a.setcc(0x94, RAX);
                        set_t(a);
                        Res::Pure
                    }
                    9 => {
                        a.load32(RAX, r(m));
                        a.and32mr(r(n), RAX);
                        Res::Pure // AND (:671)
                    }
                    10 => {
                        a.load32(RAX, r(m));
                        a.xor32mr(r(n), RAX);
                        Res::Pure // XOR (:672)
                    }
                    11 => {
                        a.load32(RAX, r(m));
                        a.or32mr(r(n), RAX);
                        Res::Pure // OR (:673)
                    }
                    14 => {
                        // MULU (:674)
                        a.loadu16(RAX, r(n));
                        a.loadu16(RCX, r(m));
                        a.imul32(RAX, RCX);
                        a.store32(s_macl(), RAX);
                        Res::Pure
                    }
                    15 => {
                        // MULS (:675)
                        a.loads16_32(RAX, r(n));
                        a.loads16_32(RCX, r(m));
                        a.imul32(RAX, RCX);
                        a.store32(s_macl(), RAX);
                        Res::Pure
                    }
                    _ => Res::None,
                },
                0x3 => match op & 15 {
                    0 => cmp_t(a, 0x94), // CMP/EQ (:680)
                    2 => cmp_t(a, 0x93), // CMP/HS (:681)
                    3 => cmp_t(a, 0x9d), // CMP/GE (:682)
                    6 => cmp_t(a, 0x97), // CMP/HI (:683)
                    7 => cmp_t(a, 0x9f), // CMP/GT (:684)
                    8 => {
                        a.load32(RAX, r(m));
                        a.sub32mr(r(n), RAX);
                        Res::Pure // SUB (:685)
                    }
                    12 => {
                        a.load32(RAX, r(m));
                        a.add32mr(r(n), RAX);
                        Res::Pure // ADD (:686)
                    }
                    _ => Res::None,
                },
                0x4 => match op & 0x3f {
                    0x00 | 0x20 => {
                        // SHLL / SHAL (:691-694)
                        a.load32(RAX, r(n));
                        a.mov32(RCX, RAX);
                        a.shr32(RCX, 31);
                        a.shl32(RAX, 1);
                        a.store32(r(n), RAX);
                        a.and32i_mem(s_sr(), !SH_T);
                        a.or32mr(s_sr(), RCX);
                        Res::Pure
                    }
                    0x01 | 0x21 => {
                        // SHLR / SHAR (:695-700)
                        a.load32(RAX, r(n));
                        a.mov32(RCX, RAX);
                        a.and32i(RCX, 1);
                        if (op & 0x3f) == 0x01 {
                            a.shr32(RAX, 1);
                        } else {
                            a.sar32(RAX, 1);
                        }
                        a.store32(r(n), RAX);
                        a.and32i_mem(s_sr(), !SH_T);
                        a.or32mr(s_sr(), RCX);
                        Res::Pure
                    }
                    0x08 => {
                        a.shl32i_mem(r(n), 2);
                        Res::Pure // SHLL2 (:701)
                    }
                    0x18 => {
                        a.shl32i_mem(r(n), 8);
                        Res::Pure // SHLL8 (:702)
                    }
                    0x28 => {
                        a.shl32i_mem(r(n), 16);
                        Res::Pure // SHLL16 (:703)
                    }
                    0x09 => {
                        a.shr32i_mem(r(n), 2);
                        Res::Pure // SHLR2 (:704)
                    }
                    0x19 => {
                        a.shr32i_mem(r(n), 8);
                        Res::Pure // SHLR8 (:705)
                    }
                    0x29 => {
                        a.shr32i_mem(r(n), 16);
                        Res::Pure // SHLR16 (:706)
                    }
                    0x10 => {
                        // DT (:707)
                        a.sub32i_mem(r(n), 1);
                        a.setcc(0x94, RAX);
                        set_t(a);
                        Res::Pure
                    }
                    0x11 => {
                        // CMP/PZ (:708)
                        a.cmp32i_mem(r(n), 0);
                        a.setcc(0x9d, RAX);
                        set_t(a);
                        Res::Pure
                    }
                    0x15 => {
                        // CMP/PL (:709)
                        a.cmp32i_mem(r(n), 0);
                        a.setcc(0x9f, RAX);
                        set_t(a);
                        Res::Pure
                    }
                    0x0a => copy(a, r(n), s_mach()), // LDS Rn,MACH (:710)
                    0x1a => copy(a, r(n), s_macl()), // LDS Rn,MACL (:711)
                    0x2a => copy(a, r(n), s_pr()),   // LDS Rn,PR (:712)
                    0x1e => copy(a, r(n), s_gbr()),  // LDC GBR (:713)
                    0x2e => copy(a, r(n), s_vbr()),  // LDC VBR (:714)
                    0x02 | 0x12 | 0x22 => {
                        // STS.L x,@-Rn (:715-721)
                        let src = if (op & 0x3f) == 0x02 {
                            s_mach()
                        } else if (op & 0x3f) == 0x12 {
                            s_macl()
                        } else {
                            s_pr()
                        };
                        a.sub32i_mem(r(n), 4);
                        a.load32(RDX, r(n));
                        a.store32(s_ea(), RDX);
                        a.load32(R8, src);
                        mwrite(a, 4);
                        Res::Memop
                    }
                    0x06 | 0x16 | 0x26 => {
                        // LDS.L @Rn+,x (:722-729)
                        let dst = if (op & 0x3f) == 0x06 {
                            s_mach()
                        } else if (op & 0x3f) == 0x16 {
                            s_macl()
                        } else {
                            s_pr()
                        };
                        a.load32(RDX, r(n));
                        a.store32(s_ea(), RDX);
                        mread(a, 4);
                        a.store32(dst, RAX);
                        a.add32i_mem(r(n), 4);
                        Res::Memop
                    }
                    0x0b => {
                        a.store32i(s_pr(), pcv + 2); // JSR (:730-732)
                        a.load32(RAX, r(n));
                        a.store32(s_delay(), RAX);
                        a.store32(s_ea(), RAX);
                        a.sub32i_mem(s_icount(), 1);
                        Res::Delayed
                    }
                    0x2b => {
                        // JMP (:733-735)
                        a.load32(RAX, r(n));
                        a.store32(s_delay(), RAX);
                        a.store32(s_ea(), RAX);
                        a.sub32i_mem(s_icount(), 1);
                        Res::Delayed
                    }
                    _ => Res::None,
                },
                0x5 => {
                    // MOV.L @(disp,Rm),Rn (:738-741)
                    rd_ea(a, m, (op & 15) as u32 * 4, 4);
                    to_r(a, n, 4);
                    Res::Memop
                }
                0x6 => match op & 15 {
                    0 | 1 | 2 => {
                        // MOV.x @Rm,Rn (:744-749)
                        let sz = 1u32 << (op & 15);
                        rd_ea(a, m, 0, sz);
                        to_r(a, n, sz);
                        Res::Memop
                    }
                    3 => copy(a, r(m), r(n)), // MOV Rm,Rn (:750)
                    4 | 5 | 6 => {
                        // MOV.x @Rm+,Rn (:751-758)
                        let sz = 1u32 << ((op & 15) - 4);
                        a.load32(RDX, r(m));
                        mread(a, sz);
                        to_r(a, n, sz);
                        if n != m {
                            a.add32i_mem(r(m), sz);
                        }
                        Res::Memop
                    }
                    7 => {
                        // NOT (:759)
                        a.load32(RAX, r(m));
                        a.not32(RAX);
                        a.store32(r(n), RAX);
                        Res::Pure
                    }
                    11 => {
                        // NEG (:760)
                        a.load32(RAX, r(m));
                        a.neg32(RAX);
                        a.store32(r(n), RAX);
                        Res::Pure
                    }
                    12 => {
                        // MOV.B @Rm,Rn (zero) (:761)
                        a.loadu8(RAX, r(m));
                        a.store32(r(n), RAX);
                        Res::Pure
                    }
                    13 => {
                        // MOV.W @Rm,Rn (zero) (:762)
                        a.loadu16(RAX, r(m));
                        a.store32(r(n), RAX);
                        Res::Pure
                    }
                    14 => {
                        // MOV.BS (:763)
                        a.loads8_32(RAX, r(m));
                        a.store32(r(n), RAX);
                        Res::Pure
                    }
                    15 => {
                        // MOV.WS (:764)
                        a.loads16_32(RAX, r(m));
                        a.store32(r(n), RAX);
                        Res::Pure
                    }
                    _ => Res::None,
                },
                0x7 => {
                    // ADD #imm,Rn (:767-769)
                    a.add32i_mem(r(n), sext8((op & 0xff) as u32));
                    Res::Pure
                }
                0x8 => {
                    // :770-787 (BT/BF/BTS/BFS target: pcv + sext8*2 + 2)
                    let target = pcv.wrapping_add(sext8((op & 0xff) as u32).wrapping_mul(2)).wrapping_add(2);
                    match n {
                        0 => {
                            wr_ea(a, m, (op & 15) as u32, 0, 1);
                            Res::Memop // MOV.B R0,@(disp,Rm) (:773)
                        }
                        1 => {
                            wr_ea(a, m, (op & 15) as u32 * 2, 0, 2);
                            Res::Memop // MOV.W R0 (:774)
                        }
                        4 => {
                            rd_ea(a, m, (op & 15) as u32, 1);
                            to_r(a, 0, 1);
                            Res::Memop // MOV.B @(disp,Rm),R0 (:775)
                        }
                        5 => {
                            rd_ea(a, m, (op & 15) as u32 * 2, 2);
                            to_r(a, 0, 2);
                            Res::Memop // MOV.W @(disp,Rm),R0 (:776)
                        }
                        8 => {
                            // CMP/EQ #imm,R0 (:777)
                            a.cmp32i_mem(r(0), sext8((op & 0xff) as u32));
                            a.setcc(0x94, RAX);
                            set_t(a);
                            Res::Pure
                        }
                        9 | 11 | 13 | 15 => {
                            // BT / BF / BT/S / BF/S (:778-784)
                            a.test32i_mem(s_sr(), SH_T);
                            let skip = a.jcc_fwd(if n == 9 || n == 13 { 0x84 } else { 0x85 });
                            branch_to(a, target, n >= 13);
                            a.patch(skip);
                            if n >= 13 {
                                Res::Delayed
                            } else {
                                Res::Ends
                            }
                        }
                        _ => Res::None,
                    }
                }
                0x9 => {
                    // MOV.W @(disp,PC),Rn (:788-795) — host-folded literal
                    let ea = pcv + (op & 0xff) as u32 * 2 + 2;
                    if ea + 1 > rom_end {
                        return Res::None;
                    }
                    let v = core.read_word(ctx, ea);
                    a.store32i(s_ea(), ea);
                    a.store32i(r(n), ((v as i16) as i32) as u32);
                    Res::Pure
                }
                0xa => {
                    // BRA (:796-798)
                    let disp = (((op & 0xfff) as i32) << 20 >> 20) as u32;
                    branch_to(a, pcv.wrapping_add(disp.wrapping_mul(2)).wrapping_add(2), true);
                    Res::Delayed
                }
                0xb => {
                    // BSR (:799-802)
                    a.store32i(s_pr(), pcv + 2);
                    let disp = (((op & 0xfff) as i32) << 20 >> 20) as u32;
                    branch_to(a, pcv.wrapping_add(disp.wrapping_mul(2)).wrapping_add(2), true);
                    Res::Delayed
                }
                0xc => {
                    let d = (op & 0xff) as u32;
                    match n {
                        0 | 1 | 2 => {
                            // MOV.x R0,@(disp,GBR) (:806-812)
                            let sz = 1u32 << n;
                            a.load32(RDX, s_gbr());
                            a.add32ri(RDX, d * sz);
                            a.store32(s_ea(), RDX);
                            a.load32(R8, r(0));
                            mwrite(a, sz);
                            Res::Memop
                        }
                        4 | 5 | 6 => {
                            // MOV.x @(disp,GBR),R0 (:813-818)
                            let sz = 1u32 << (n - 4);
                            a.load32(RDX, s_gbr());
                            a.add32ri(RDX, d * sz);
                            a.store32(s_ea(), RDX);
                            mread(a, sz);
                            to_r(a, 0, sz);
                            Res::Memop
                        }
                        7 => {
                            // MOVA (:820-823)
                            let ea = ((pcv + 2) & !3) + d * 4;
                            a.store32i(s_ea(), ea);
                            a.store32i(r(0), ea);
                            Res::Pure
                        }
                        8 => {
                            // TST #imm,R0 (:825)
                            a.test32i_mem(r(0), d);
                            a.setcc(0x94, RAX);
                            set_t(a);
                            Res::Pure
                        }
                        9 => {
                            a.and32i_mem(r(0), d);
                            Res::Pure // AND (:826)
                        }
                        10 => {
                            a.xor32i_mem(r(0), d);
                            Res::Pure // XOR (:827)
                        }
                        11 => {
                            a.or32i_mem(r(0), d);
                            Res::Pure // OR (:828)
                        }
                        _ => Res::None,
                    }
                }
                0xd => {
                    // MOV.L @(disp,PC),Rn (:832-839) — host-folded literal
                    let ea = ((pcv + 2) & !3) + (op & 0xff) as u32 * 4;
                    if ea + 3 > rom_end {
                        return Res::None;
                    }
                    let v = core.read_long(ctx, ea);
                    a.store32i(s_ea(), ea);
                    a.store32i(r(n), v);
                    Res::Pure
                }
                0xe => {
                    // MOV #imm,Rn (:840-842)
                    a.store32i(r(n), sext8((op & 0xff) as u32));
                    Res::Pure
                }
                _ => Res::None,
            }
        };

        // ---- driver loop (origin: sh2_jit.cpp:871-960; LAZYPC/stale machinery
        // DROPPED — jit_head stores S_pc eagerly every instruction) ----
        let mut slot = false;
        let mut i: usize = 0;
        loop {
            let at = pc + 2 * i as u32;
            // opcode fetch: C++ m_decrypted_program->read_word (:874);
            // m_am == !0 and at < ROM_END ⇒ the masked read IS the ROM byte.
            let op = {
                let rom = &ctx.bus.rom;
                (u16::from(rom[at as usize]) << 8) | u16::from(rom[at as usize + 1])
            };
            let k = classify(op);

            // jit_head — the exact RunHook replication (handoff §jit_head):
            // replaces the per-instruction hook; do NOT re-sync in stubs.
            // 1. pc (driver :891-902; eager — the non-delay branch of the
            //    slot head only fires if a compiled insn skipped its
            //    m_delay store — impossible by the :604/RTS/JSR/JMP/BT
            //    coverage argument; kept as belt-and-braces)
            if slot {
                a.load32(RAX, s_delay());
                a.test32(RAX, RAX);
                let no_delay = a.jcc_fwd(0x84); // je
                a.store32(s_pc(), RAX);
                a.store32i(s_delay(), 0);
                let done = a.jmp_fwd();
                a.patch(no_delay);
                a.store32i(s_pc(), at + 2);
                a.patch(done);
                // hn.pc = [S_pc] load == interpreter pc_exec (= m_delay)
                a.load32(RDX, s_pc());
                a.store32(hn_pc(), RDX);
            } else {
                a.store32i(s_pc(), at + 2);
                a.store32i(hn_pc(), at + 2);
            }
            // 2. RAX = R15 - sext32([rbp+icount]) == total_cycles()
            //    S11 FIX: the first port emitted sub64(RAX=icount, RDX=base)
            //    — sign-flipped (hn.now went negative: total_cycles() ground
            //    truth in core.rs:316-318 is base − icount, cf. RunHook
            //    `h.now = pre` in lib.rs:2022).
            a.loads32(RDX, s_icount());
            a.mov64(RAX, R15);
            a.sub64(RAX, RDX);
            // 3. cc = hn.cpu_now() BEFORE hn.now update (lib.rs:197-205):
            //    in_event ? now : (now==0 ? 0 : now-1). RAX (total) is held
            //    across; RDX is scratch; RCX carries cc.
            a.loadu8(RDX, hn_in_event());
            a.test32(RDX, RDX);
            let use_now = a.jcc_fwd(0x85); // jnz ⇒ in_event (block emitted below)
            a.load64(RCX, hn_now());
            a.test64(RCX, RCX);
            let zero = a.jcc_fwd(0x84); // jz ⇒ RCX already 0 (cc = 0)
            a.imm64(RDX, 1);
            a.sub64(RCX, RDX); // cc = now - 1
            let cont = a.jmp_fwd();
            a.patch(use_now);
            a.load64(RCX, hn_now()); // in_event leg (HubNow::cpu_now lib.rs:199)
            a.patch(zero);
            a.patch(cont); // cc now in RCX
            a.store64(hn_seam(), RCX); // pre_seam = cc
            // NOTE (M9 debug session 2026-10-04): a head-of-block callout probe
            // was tried here and is DISABLED. call_abs clobbers the caller-saved
            // RAX/RCX/RDX, but the very next real instruction stores RAX (the
            // total) into hn.now — so the probe corrupted the clock chain and
            // produced the very AV it was chasing. The disassembly of the
            // UN-instrumented blocks (see Session log) is faithful to the C++
            // ground truth, so no head probe is needed to find the real bug.
            a.store64(hn_now(), RAX); // now = pre (this instruction's total)

            // 4. execute (driver :904-937)
            let pc_rel = (op >> 12) == 0x9 || (op >> 12) == 0xd || (op >> 8) == 0xc7;
            let mut r = Res::None;
            if self.native_enabled && (!slot || (self.slot_native && k == Kind::Normal && !pc_rel)) {
                r = native(&mut a, core, ctx, op, at);
            }
            if r == Res::None {
                // jit_exec callout leg (:912-926); pc already eager-stored
                a.mov64(ARG0, RBX);
                a.imm32(ARG1, op as u32);
                a.call_abs(fnptr1(jit_exec));
                r = match k {
                    Kind::Delayed => Res::Delayed,
                    Kind::Ends => Res::Ends,
                    Kind::Normal => Res::Memop,
                };
            }

            // 分岐した・止まる命令・遅延スロットの後はブロックを終える (:939-941)
            if slot || r == Res::Ends || (r != Res::Delayed && (i + 1 >= MAX_INSNS || at + 4 >= ROM_END)) {
                break;
            }

            // 3./4. 途中の確かめ (memop epilogue; driver :943-955 + the
            // Rust dirty gate). Callout memops MUST re-check here — only
            // callouts can set m_test_irq/dev_dirty (exceptions included).
            if r == Res::Memop {
                a.load64(RCX, s_test());
                a.test32(RCX, RCX);
                to_finish.push(a.jcc_fwd(0x85)); // jne
                load_bus(&mut a, RCX);
                a.loadu8(RCX, Mem::b(RCX, o.dev_dirty)); // byte dev_dirty
                a.test32(RCX, RCX);
                to_finish.push(a.jcc_fwd(0x85)); // jne — dev_dirty (DEVIATION)
                a.cmp32i_mem(s_pc(), at + 2);
                to_finish.push(a.jcc_fwd(0x85)); // jne — unexpected pc change
            }
            a.sub32i_mem(s_icount(), 1);
            to_ret.push(a.jcc_fwd(0x8e)); // jle

            slot = r == Res::Delayed;
            if slot && at + 4 >= ROM_END {
                // C++ :958-959 — refuse the block (host interprets one insn;
                // no spin since the ROM_END-0x100 gate bounds this at the top)
                return 0;
            }
            i += 1;
        }

        // 終わりの処理 (driver :966-997): finish + ret tails
        let finish = a.code.len();
        for p in to_finish {
            a.patch_to(p, finish);
        }
        a.load32(RAX, s_test());
        a.test32(RAX, RAX);
        let no_irq1 = a.jcc_fwd(0x84);
        a.load32(RAX, s_delay());
        a.test32(RAX, RAX);
        let no_irq2 = a.jcc_fwd(0x85);
        a.mov64(ARG0, RBX);
        a.call_abs(fnptr0(jit_irq));
        a.patch(no_irq1);
        a.patch(no_irq2);
        a.sub32i_mem(s_icount(), 1);

        let ret = a.code.len();
        for p in to_ret {
            a.patch_to(p, ret);
        }
        a.imm64(RAX, self.buf as u64 + self.next_off as u64);
        a.rr(0, false, &[0xff], 4, RAX); // jmp next_block (:995-996)

        // origin: sh2_jit.cpp:1006-1014 — buffer full ⇒ flush + refuse
        // (caller falls back to one interpreter insn; next visit recompiles)
        if self.used + a.code.len() > BUF_SIZE {
            self.flush();
            return 0;
        }
        let dst = self.buf as usize + self.used;
        unsafe {
            std::ptr::copy_nonoverlapping(a.code.as_ptr(), self.buf.add(self.used), a.code.len());
        }
        self.used += a.code.len();
        let _ = JIT_BLOCKS_COMPILED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        dst
    }
}

impl Drop for Jit {
    // origin: sh2_jit.cpp:90-96 ~jit — VirtualFree(MEM_RELEASE). The leaked
    // pins (pages table/entry slot) are 4 KiB per Machine, never freed.
    fn drop(&mut self) {
        if !self.buf.is_null() {
            unsafe {
                VirtualFree(self.buf as *mut c_void, 0, MEM_RELEASE);
            }
        }
    }
}
