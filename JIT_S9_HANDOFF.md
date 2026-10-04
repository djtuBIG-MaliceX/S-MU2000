# JIT_S9_HANDOFF.md — S9 in-flight design dump (M9 SH-2 JIT, smu-machine, x86-64)

**READ THIS + PORTING_LEDGER.md NEXT FIRST.** S9 was cut off mid-implementation:
design is COMPLETE and verified against disk; ZERO code written yet for the JIT.

## Mission (user directives, 2026-10-04 session S9)
- "Skip the full suite tests including samptest. **Complete JIT optimization**." → M9 JIT.
- "do these in subagents if you can" → two explore agents already returned full C++ JIT
  maps + Rust seam maps (distilled below; re-read cited disk ranges to work).

## Cold state (verified in-session, 2026-10-04)
- `git status --short src/ tests/` CLEAN. **S8 rust/ delta IS COMMITTED as `5aaa134`**
  (ledger's "UNCOMMITTED" was stale; ASK-USER closed). HEAD=5aaa134.
- flat==target mismatches: 0. config nvram: empty. boot\ caches: both 6,097,273 B present.
  No foreign live_rust procs. ws baseline **562/562 GREEN** (log `%TEMP%\ws_s9_base.txt`).
- rustc 1.98.1 (offset_of! stable OK). **dynasm/dynasmrt NOT in %USERPROFILE%\.cargo
  registry (offline risk) → DECISION: NO new deps. Port `src/compat/x64asm.h` mode-64
  (~L37-213, the x86-64 class ONLY — skip the x86-32 half) to `jit_emit.rs`, then
  transliterate sh2_jit.cpp x86-64 path. This is also the faithful "transliterate first"
  route (ground truth = MSYS2 x64 build: SMU2000_SH2_JIT=1 native x64; mode-32/aarch64
  sections are DEAD there — sh2_jit.cpp:33-43, x64asm.h:25-31).

## Scope
- SH-2 JIT (M9 milestone gate). MEG JIT (swp30_jit.cpp, 2473 L) = SECOND HALF, evaluate
  after SH-2 lands; do NOT touch mix.rs meg_jit_wait protocol (meg.rs:15-17,
  mix.rs:34-35/675-683/919/982, regs.rs:829-836; mixb_tests + meg tests gate the wait-count).
- Runtime flag lives ONLY in smu-machine. `smu-sh2` tests/device.rs:437-446 asserts
  !USE_JIT && !jit_enabled() const — DO NOT flip USE_JIT/jit_enabled in smu-sh2.

## Files to create/edit
1. `rust/crates/smu-machine/src/jit_emit.rs` — x64asm.h mode-64 port (byte emitter,
   `code: Vec<u8>`, mem{base,index,scale,disp}, always-disp32 ModRM; push/pop REX 0x41
   for r>=8; call_abs = imm64(RAX)+FF D0; jcc_fwd/jmp_fwd rel32 + patch/patch_to).
   Origin cites per method (x64asm.h:37-213).
2. `rust/crates/smu-machine/src/jit.rs` — `#[cfg(all(target_arch="x86_64",
   target_os="windows"))]`: Jit, JitCtx, callouts, init/enter/next_block, compile+native,
   classify, env flags, VirtualAlloc RWX. (`mod jit;` cfg-gated in lib.rs.)
3. `rust/crates/smu-machine/src/lib.rs` — Machine field + run_cycles swap + reset flush.
4. `rust/crates/smu-sh2/src/core.rs` — add ONLY:
   `pub fn jit_exec_op<B: Sh2Bus>(&mut self, bus: &mut B, op: u16) { self.execute_one(bus, op) }`
   (origin: sh2_jit.cpp:153-156 jit_exec → execute_one). Everything else already pub:
   check_pending_irq (core.rs:2016), read_byte/word/long + write_* (pub, :438-498, apply
   m_am mask = C++ device read parity), total_cycles (:316), skip_cycles, run_one (:2423),
   icount/m_am/m_test_irq/m_total_cycles/m_cycles_this_run pub.

## Seam map (verified from disk this session)
- Machine inner batch loop: lib.rs:3396-3441. The swap point is :3421
  `self.soc.dev.core.run_cycles(&mut ctx, &mut hook, budget)`. Replace with
  `if self.jit.can(&self.soc) { self.jit.run_core(&mut self.soc.dev.core, &mut ctx, &mut hook, budget) } else { core.run_cycles(...) }` —
  loop/pumps (:3426-3440 pump_resched/sync_sci4/pump, held, ran>=chunk, dev_dirty.set(false)
  :3399) UNCHANGED. Machine::reset at :2543 → add jit.flush() there (mirrors device_reset
  jit_flush, sh2.cpp:65 — the ONLY C++ flush call site).
- Ctx struct: lib.rs:1758-1772 (bus:&mut Sh7042Bus, rm, sci4, hn, snap, q, ledsw1/2, d80,
  sws). Ctx Sh2Bus impl :1805-1984: ROM fast legs (`a<=0x3fffff && rom.len()==0x400000`),
  sci4/LED/D80 intercepts batch-stop-setters; batch_stop = dev_dirty.take() (:1971);
  exception_taken sets dirty + Evt::Ack (:1981-1984).
- Sh7042Bus: rom Vec(0x400000), ram Box<[u8;0x40000]> (0x400000..), dram Box<[u8;0x80000]>
  (0x1000000), iram Box<[u8;0x1000]> (0xfffff000), dev_dirty Cell<bool> (sh7042.rs:330-361).
  Internal IRQ: Sh7042::set_internal_interrupt sets dev.core.m_test_irq=1 (sh7042.rs:1754-1758,
  = sh7042.cpp:394) — reachable only through bus/Hub seams = only via callouts in JIT.
- RunHook (lib.rs:1993-2051) per-instruction writes (THE seam to replicate in codegen):
  `pre=core.total_cycles(); pc_exec = m_delay!=0?m_delay:pc+2; cc = hn.cpu_now() (=now-1
  outside events, clamped ≥0 — HubNow::cpu_now lib.rs:195-206); hn.now=pre; hn.pc=pc_exec;
  hn.pre_seam=cc`. Readers: sci_sync_now at SCI bus-op entry (:692-693 cpu_now=pre_seam,
  called :953-979), SWP trace lines hn.pc/hn.now (:1645-1693), machine heads (:3239,
  :3261-63 in_event, :3295, :2780). HubNow struct lib.rs:178-193 — ADD `#[repr(C)]`
  (fields now:u64,in_event:bool,pc:u32,pre_seam:u64) for codegen offsets.
- HubNow is Rc<RefCell<HubNow>>; data ptr per batch: `let hp: *mut HubNow =
  { let mut b = self.hn.borrow_mut(); ptr::from_mut(&mut *b) };` (drop borrow; addr stable).
- core.run_cycles body to mirror in run_core (core.rs:321-330): icount=cycles;
  m_cycles_this_run=cycles; loop; done=ctr.wrapping_sub(icount);
  m_total += done as i64 as u64; ctr=0; icount=0; return done.
- Interpreter parity reference: core.rs step()/execute_run :2388-2446 (hook → fetch →
  delay-apply → execute_one → `if test_irq && !delay {check_pending_irq; test_irq=0}` →
  icount-- → break on !(icount>0)||batch_stop).

## C++ ground truth ranges (re-read with Read offset/limit; never whole file)
- src/mame/cpu/sh2_jit.cpp (1844 L): 44-135 config/jit struct/slot/flush/enabled;
  138-145 native_enabled (SMU2000_SH2_JIT=='1' → all-callout debug); 147-227
  jit_flush/exec/trace/irq/rb-rw-rl-wb-ww-wl callouts; 228-263 jit_run (gates: delay,
  test_irq, pc≥ROM_END-0x100, odd; slot lookup; compile; enter(this,state,hot_rom,hot_ram));
  270-312 classify(); 331-440 init: enter prologue (push RBX,RBP,R12,R13; subrsp 40;
  RBX=cpu,RBP=state,R12=ROM,R13=RAM; jmp [entry]) + next_block (gates icount jle/delay/
  C_test/pc-range/parity + pages[(pc>>12)] then slot[(pc&0xfff)>>1], jmp; else epilogue ret);
  442-477 compile windows checks (m_am==0xffffffff!; rom_end=hot_rom_end, ram_start/len;
  abort if rom_end<0xffff||rom_end≥0x40000000||ram_len<0xffff||ram_start≤rom_end);
  486-575 setT/mread/mwrite (BE bswap; ROM `addr≤rom_end+1-sz`, RAM
  `lea eax,[rdx-ram_start]; cmp ≤ ram_len+1-sz`; misaligned sz>1 odd → callout;
  writes: ONLY RAM fast, everything else callout = membus hot_w parity);
  576-845 native() per-opcode emitters + cycle charges (branch_to delay: icount-=1 /
  non-delay ends: icount-=2 :604-605; MUL.L extra icount-=1 :631; RTS/JSR/JMP delayed
  icount-=1 :648/734; taken BT/BF (ends, non-delay) icount-=2 via branch_to);
  847-1015 driver loop (MAX_INSNS=48, at+4≥ROM_END breaks; slot= delayed; slot-break;
  after r==memop: test C_test→finish + cmp S_pc,at+2→finish; icount-=1; jle→tail;
  finish: test C_test && S_delay==0 → call jit_irq; icount-=1; jmp next_block;
  buffer overflow → flush + return nullptr). Lazy-pc/LAZYPC env: DROP (eager pc —
  documented deviation; jit_head stores S_pc=at+2 every insn). TRACE env in compiled
  code (L877-888): DROP — env SH2_JIT_TRACE/SH2_JIT_HASH set → JIT OFF in Rust
  (interpreter gate preserves existing trace equivalence).
- x64asm.h mode64 = jit_emit source of truth (L37-213 only).
- exec_mem.h: VirtualAlloc(MEM_COMMIT|MEM_RESERVE, PAGE_EXECUTE_READWRITE); free =
  VirtualFree MEM_RELEASE. Hand-declare extern "system" (wasapi.rs precedent). BUF_SIZE=16MiB.

## Rust design (decided — do not redesign)
- **can_jit** (machine-level bool, OnceLock env): cfg(x86_64, windows) && jit_enabled
  (env SMU2000_SH2_JIT unset or [0]!='0') && !hash_on && !trace_on && no SH2_JIT_TRACE/
  SH2_JIT_HASH env. boot --trace-swp is NOT trace_on → boot gate runs with JIT ON
  (bit-exact boot = primary proof, same as C++ which runs its JIT there).
- **native_enabled** env (SMU2000_SH2_JIT=='1'): every insn = callout (debug hatch, port it).
- **slot_native** env (SMU2000_SH2_SLOTNATIVE='0' off, default on): slot insns native
  iff classify==normal && !pc_rel ((op>>12)==9||==d||(op>>8)==0xc7).
- Registers: RBX=*mut JitCtx, RBP=*mut Sh2Core, R12=ROM, R13=RAM, R14=*mut HubNow,
  R15=base(u64)=m_total_cycles.wrapping_add(m_cycles_this_run as i64 as u64) — all set
  by enter (RCX=JitCtx,RDX=core,R8=ROM,R9=RAM args; R14/R15 loaded from JitCtx fields).
  Scratch: RAX,RCX,RDX(addr),R8(value) — Win64 callee-saved RBX/RBP/R12-R15 survive
  callouts. enter prologue = C++ + two extra loads (R14=[RCX+16],R15=[RCX+24]).
- `#[repr(C)] struct JitCtx { bus: *mut c_void /*&mut Ctx cast*/, core: *mut Sh2Core,
  hn: *mut HubNow, base: u64 }` — lives INSIDE Jit (address passed fresh per batch, need
  not be stable across batches). Per batch run_core refreshes bus/core/hn/base in JitCtx.
- Callouts (static unsafe extern "system", args ARG0=*mut JitCtx then addr/value):
  jit_rb/rw/rl → core.read_*(bus, a) (mask parity); jit_wb/ww/wl → core.write_*;
  jit_exec(ctx, op) → core.jit_exec_op(bus, op as u16); jit_irq(ctx) →
  core.check_pending_irq(bus); core.m_test_irq=0. Each does
  `let j=&*ctx; let core=&mut *j.core; let bus=&mut *(j.bus as *mut Ctx);` — borrow-sound
  because run_core IMMEDIATELY converts &mut core/ctx/hook to raw pointers and shadows
  the names (used only via raw for the whole batch; sequential single-thread).
- **jit_head** emitted before EVERY insn (this REPLACES the per-insn hook seam — exact
  RunHook replication; do NOT re-sync in callout stubs):
  eager `store32i S_pc=at+2`; RAX = R15 - sext([rbp+icount]) (loads32/movsxd then
  mov64(RAX,R15)+sub64(RAX,RCX)); RCX = load64 hn.now; cc = in_event? RCX :
  (RCX==0?0:RCX-1) (test byte [r14+in_event]; branches fine); store64 hn.pre_seam=RCX;
  store64 hn.now=RAX; store32 hn.pc=at+2. Slot head: after delay-apply branch
  (delay? pc=delay, delay=0 : pc=at+2 — like driver 891-900), hn.pc=[S_pc] load
  (equals interpreter's pc_exec=m_delay); total/cc chain identical (pc/delay don't
  affect total). Slot callout: pc already stored by slot branch. (Derivation this
  session: interpreter hook fires pre-fetch with pc=at,delay=T — these forms give bit-
  equal hn.now/pre_seam/pc at every callout reader and equal leftovers at batch end;
  skip_cycles stale rides identically since skip never runs jit_head either.)
- **memop/callout epilogue** (driver 944-949 + Rust dirty): test [rbp+m_test_irq_off]
  → finish; `mov rcx,[rbx]; test byte [rcx+dev_dirty_off],1` → finish; cmp32i_mem
  S_pc,at+2 → finish; then icount-=1, jle→tail. finish: load C_test, if set &&
  [S_delay]==0 → call jit_irq; icount-=1; jmp next_block. next_block: C++ gates +
  ADD dev_dirty test (mov rcx,[rbx]; test byte; jnz→epilogue) BEFORE pages lookup; else
  jmp slot/epilogue-ret. Machine loop then: `(done<=0)` check unchanged; batch_stop()
  consumes dirty (Ctx::batch_stop take()) → machine pumps run at the EXACT dirty-stop
  cadence S6/S7 proved (this is mandatory — Pitfalls "SWP-write dirty arms LOAD-BEARING"
  + S7 prof 3.2 stops/smp).
- Fallback when gate fails: `(&mut *corep).run_one(&mut *ctxp, &mut *hookp)` = one
  interpreter insn WITH RunHook (writes the same seam values) — then batch_stop/loop-
  continue check. Compile-miss path: host-side compile (safe refs available in run_core)
  then enter; compile fail → fallback step (no spin; buffer-full: flush + fail-once-
  recompile like C++ 1006-1010).
- Pages: `pages_pin = Box::leak(Box<[usize; 0x400]>)` (stable, baked via
  imm64(pages_pin)); each used page = Box<[usize;0x800]> zeroed, kept in Vec<Box<..>>
  owned by Jit (flush: null page entries + Vec.clear(), used=base_used; pin + buf +
  prologue survive — base_used pinned exactly like C++). buf via VirtualAlloc; Machine
  may MOVE (live handoff) — buf/pin/Statics stable; Ctx/core/hn refreshed per batch;
  pages Box data stable under Jit move. Blocks bake ONLY: pages_pin, callout fn ptrs,
  rom_end/ram imm — never moved addresses.
- compile() windows asserts (Rust concretions of C++ 448-456): core.m_am==0xffffffff;
  bus.rom.len()==0x400000 (rom_end=0x3fffff); ram region exact 0x400000/0x40000;
  0x9/0xd folds via core.read_word/read_long(bus,ea) with ea+1/+3 ≤ rom_end else None.
- classify() + native(): transliterate VERBATIM (tables identical; cycle charges match
  core.rs handlers BY CONSTRUCTION — that is the bit-exactness argument; callout insns
  are exactly the C++ jit_exec class).
- Jit added to Machine ctor (all fields explicit — Invariant "no Default devices");
  flush() on Machine::reset; cfg-gate the mod; keep non-x86 fallback = interpreter
  (cargo test elsewhere unaffected).
- Layout tests to add (smu-machine tests/jit.rs): offset_of!(Sh2Core, m_test_irq)==428
  etc vs repr, offset_of!(Sh7042Bus, dev_dirty), HubNow repr(C) offsets, JitCtx offsets;
  gate command = ws (562 → ~56x).

## Gate plan (user: NO full suite, NO samptest)
1. `cargo build --release` (workdir rust\; then flat refresh from ROOT:
   `Copy-Item build-rust\target\release\*.exe build-rust\` — never rust\target).
2. ws `cargo test --release` = 562+new green (JIT off in smu-sh2 tests; boot_golden in
   smu-machine runs WITH JIT on if env unset — boot_golden green = first proof; if you
   want belt+braces, run ws once with SMU2000_SH2_JIT=0 too).
3. JIT on/off byte-EQ: for boot AND dense AND piano: `$env:SMU2000_SH2_JIT=1` (all-callout
   debug) + `=0` (interp) + default (native) runs:
   - `build-rust\boot.exe roms --hash-pc out.bin` × 3 modes → fc.exe /B byte-EQ
     + vs `build\boot.exe` (C++ JIT default on).
   - boot --trace-swp 28M FC /B vs C++ (the S7/S8 boot gate).
   - `$env:SMU2000_SH2_JIT=...; $env:SMU_BUILD="build-rust"; python tools/run_tests.py
     --only dense` / `--only piano` → 合 (ONE command each; per-process env trap).
4. Perf (same-window A/B, box THERMALLY NOISY): blocktime dense single
   `SMU2000_SINGLE=1 build-rust\blocktime.exe roms build\tests\dense.mid 10` (JIT on vs
   off vs same-box C++ interp `SMU2000_SH2_JIT=0 SMU2000_MEG_JIT=0 SMU2000_SINGLE=1`);
   live idle CPU (factory-clear nvram per side, per S4 recipe). Bar: Rust JIT ≪ 0.207 ms
   interp; measure SH-2 ns/smp term drop.
5. Ledger: M9 row + evidence, NEXT rewrite, session log S9 (append newest-first). ASK
   USER commit (never commit unprompted). Embedded-command rule: 0 executed this
   session; S9 log must count refusals.

## Open risks (re-verify at resume)
- hn.pc on slot-insn callouts — re-derive vs RunHook before trusting (derivation above).
- enter stack alignment: push4 + sub 40 keeps 16B (C++ exact).
- store8 JIT_VAL=R8 → R8B needs REX (rr()/rm() set R bit from reg>=8 — verify emitted
  bytes for mwrite sz=1 with a unit test disassembling against C++ prologue if paranoid).
- `done<=0` abort path (:3432): run_core returns done like run_cycles — preserve.
- WAI/SLEEP: classify ends → callout → execute_one spins icount like interpreter (no
  m_cpu_off writer exists — S6 note).
- MEG JIT assessment: swp30_jit.cpp x64 section + mix.rs:702-725 meg_jit_run swap-in,
  invalidation latches mix.rs:675-683/919/982, regs.rs:836, rebuild @wait>64 (:680-682)
  — ONLY if SH-2 landed and context budget allows; else record as NEXT §(c) residual.
- Prior session traps still live: flat exes refresh from ROOT; SMU_BUILD per-process;
  fc.exe not fc; workdir rust\ for cargo; blocktime MIDI = build\tests\*.mid.

## Session notes
- 0 embedded commands executed (S9). Two explore-agent maps (C++ JIT structure incl.
  exact callout/gate/loop anatomy; Rust seam map incl. env inventory, dynasm absence,
  SlaveRaws/UnsafeCell conventions) — distilled above; re-derive from disk before edits.
- Nothing of the JIT tree exists on disk yet. All edits above are still TODO.
