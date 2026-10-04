# JIT_M10_HANDOFF.md — M9b: port the MEG (SWP30) x86-64 JIT to Rust (Phase A design)

Single-writer Phase A deliverable, per PORTING_LEDGER.md `## NEXT` (S11, lines 17-43):
read `swp30_jit.cpp` in ranges, map the driver/emission/register plan + emitter gap
vs `jit_emit.rs`, write this file, touch nothing else. Phase B (the code) resumes
from §8. C++ side FROZEN (ledger NEXT:41).

All cites are `swp30_jit.cpp:N` / `swp30.cpp:N` / `swp30.h:N` / `meg.rs:N` /
`mix.rs:N` / `jit.rs:N` / `jit_emit.rs:N`. Anything not read from disk this
session is marked **UNVERIFIED**.

---

## 1. Mission + contract

Port `swp30_device::meg_jit` (swp30_jit.cpp, 2577 L) — the x86-64 backend only
(`build()` at :598-1756; the x86-32 arms inside it are `#if SMU_X64ASM_MODE==32`
and dead on this MSYS2 x64 build, the arm64 `build()` at :1763+ is dead — macro
gate :27-37 sets `SMU2000_MEG_JIT=1` for `__x86_64__`/`_M_X64` at :30-31).

**Contract** (file header :3-19): the emitted machine code is **bit-identical** to
`meg_state::run_program()` (swp30.cpp:3982-4172 = Rust `meg::run_program`
meg.rs:1217, PAIRED):

- per-op decisions (ALU kind, read/write sources, memop) are resolved at
  compile time (:8);
- the 3-cycle write ring, 2-cycle memory-port ring and `t` values are FOLDED to
  their ring slots at compile time; only the sample-crossing head (first 3/2
  ops) and tail (last 3/2 ops) keep the real ring (:9-11 → emission
  :1000-1077, ring slots `slot3/slot2` :616-617);
- constants, address-offset table and LFO are read at RUNTIME (firmware writes
  them mid-song) (:12) — except the BAKE pass (:286-288, :403-421) which fuses
  them after 8192 stable samples (:295);
- the dither rand stream is drawn in the SAME ORDER THE SAME NUMBER OF TIMES
  (:13) — seed lives in RSI across the program (:814, :859-864), skipped-region
  draws collapse to `rand_jump` multiplies (`rand_n`, :1481-1482 ←
  swp30.cpp:4274-4298; Rust `rand_jump`/`rand_skip` meg.rs:580-592);
- branchy programs (LO-FI/DYNA, jump bit 0x3f) are compiled with the ring read/
  written every op, plus a runtime skip-counter (:15 → :839-840, :1079-1104,
  :1639-1678);
- compiled code is never persisted (disk-clean) (:16).

Goal (ledger NEXT:21-24): Rust live idle CPU 44.3% → ≤ ~25% (C++ bar 11.7%);
SWP30 master 9737 ns/sample is the whole remaining gap. Every gate stays green.

## 2. File map — C++ line ranges → planned Rust pieces

Crate layout note: the caller suggested `smu-swp30/src/meg_jit.rs`, but
`smu-swp30` deps = `smu-compat` only and `smu-machine` deps = `smu-swp30`
(rust\crates\{smu-machine,smu-swp30}\Cargo.toml, read this session). Putting the
JIT under smu-swp30 would require jit_emit.rs there (cycle) — SO:

| Planned Rust file | Contents | C++ origin |
|---|---|---|
| **`rust/crates/smu-machine/src/meg_jit.rs`** (NEW) | `MegJit` struct, `build()`, `run()`, prologue/epilogue, op emission loop, `emit_lfo`, `pack24`/`rnd`/`p_packed` lambdas, `emit_revram_encode/decode`, `emit_m1_expand`, `meg_jit_selftest` port | struct meg_jit swp30_jit.cpp:265-302; helpers :59-178; env :311-357; rebuild/invalidate/run :359-516; selftest :520-585; x64 build() :598-1756 |
| `rust/crates/smu-swp30/src/mix.rs` (EDIT, Phase B) | add `pub trait MegJitHook` + wire the 4 inert JIT legs: invalidate :675, rebuild :677-683, run :722-726, skip wake/quiet :916-919/:980-982; revram-enable leg | swp30.cpp:4412, 4414-4416, 4441/4444, 4332-4336, 4371-4373, 2482-2492 |
| `rust/crates/smu-machine/src/lib.rs` (EDIT, Phase B) | `Machine.meg_jit: MegJit` field; `run_sample_pair`→`swp.run_sample_jit(sintab,&w,jit)` at the 3 call sites (lib.rs:3048/3092/3101 — disjoint `&mut` fields, borrowck-safe; mirrors jit.rs `run_core(&mut self, core, ctx, hook, ...)` idiom jit.rs:491) | device member `m_jit` swp30.h:598; `m_meg_jit_wait` swp30.h:622 |
| `rust/crates/smu-swp30/src/regs.rs` (EDIT, Phase B, optional) | revram_enable_w leg: C++ invalidates at swp30.cpp:2490-2491; Rust today mirrors only the wait latch (meg.rs:15-17). Rust def-line line number **UNVERIFIED** (not read). The run()-time re-check (swp30_jit.cpp:396-400) makes this belt-and-braces. | swp30.cpp:2482-2492, port case 0x80e :2219 |
| `rust/crates/smu-machine/src/jit_emit.rs` | **NO additions needed** (§6). SH-2 JIT keeps using it; any Phase-B comfort-adds (e.g. `Mem::bis` with scale) are additive-only. | x64asm.h:37-213 (mode-64 class) |
| `rust/crates/smu-swp30/src/meg.rs` | ground-truth interpreter + `Op`/`MegSwp` types (Op meg.rs:1011-1054; MegSwp meg.rs:620-632); `#[repr(C)]`+offset pins may be owed (see §7-D) | Op = swp30.h:372 struct |
| `tests/meg_jit.rs` (NEW, Phase B) | offset pins (pattern: tests/jit.rs mounted via smu-machine [[test]], Cargo.toml), selftest sweep, CHECK harness vectors | selftest swp30_jit.cpp:520-585 |

Dead code — DO NOT PORT: x86-32 arms (`#if SMU_X64ASM_MODE == 32`: :731-812,
:1120-1136, :1190-1228, :1267-1273, :1284-1316 partial, :1340-1349, :1358-1364,
:1374-1381, :1392-1399, :1423-1429, :1547-1550, :1602-1604, :1615-1621,
:1663-1665, :1682-1687, plus JIT32 debug envs :1732-1754); arm64 section
(:1758-2577 incl. a64 `emit_*` :180/:209/:241); non-JIT build stub :591-595.

## 3. Register / ABI plan (Windows x64; SysV arms dead like jit.rs:22)

| Reg | Role | C++ cite | Rust note |
|---|---|---|---|
| RBX | `meg_state*` (MS) | swp30_jit.cpp:814 | callee-saved; jit_emit `RBX=3` jit_emit.rs:14 |
| R12 | `swp30_device*` (SWP) — seed/flags/ix2/skip live here | :814, offsets :646-652 | |
| R13 | `p` accumulator (42-bit s64) | :814, :832 | |
| R14 | `sample_counter` (SC) | :814, :833 | |
| R15 | reverb RAM base | :814, :831 | ARG2 per call — NOT baked (:512) |
| RSI | rand SEED | :814, :834 | **callee-saved on Win64 / SysV-caller-saved** — push/pop only on SysV call legs (:1437-1440/:1444-1447), dead for our Windows-only port |
| RDI | K_MAX = 0x7fffff (pack24 limit) | :814, :835 | |
| RBP | K_MIN = −0x800000 | :814, :836 | |
| R9 / R10 | P_MAX=+2^38−1 / P_MIN=−2^38 (saturation limits) | :814, :819-822 | **clobbered by callouts — reload via `load_p_limits()` after every call** (:819 comment, :1448 after `call_lfo` at :1443) |
| RAX RCX RDX R8 | scratch (R8 = addr temp in revram encode, :1623-1626; sintab ptr in emit_lfo, :937) | passim | RENC/R11 split only matters in 32-mode (:69) — in x64 R11 fully free |
| frame | `FRAME = 152` = shadow 32 + 16 + **LFO slots 96** + align 8; `rsp` 16-aligned at calls | :817, :828 | FM(disp) = rsp slots; `LFO_SLOT_BASE = 48` (:816, slots 48..144) |
| entry | `fn(meg_state*, swp30_device*, u16* ram)` in ARG0/1/2 | fn_t :266, prologue :826-837 | Rust: `unsafe extern "system" fn` transmute from exec buf (jit.rs:380-384 buf pattern) |
| prologue/epilogue | push RBX,R12,R13,R14,R15,RSI,RDI,RBP; subrsp 152 / store64 P→m_p; store32 SEED→swp; addrsp 152; pop×8; ret | :827-837, :1688-1694 | S11 lesson: honor Win64 callee-saves exactly (jit.rs:397-400) |

Callout: the ONLY call site is `call_lfo` (C++ `meg_jit::call_lfo(ms,lfo)` =
`ms->get_lfo(lfo)`, :298) at :1443, only when `sintab==null` (:645, :1430-1448).
Rust: `unsafe extern "system"` trampoline like jit.rs:76-116 (pointer laundering
jit.rs:70-73) reaching `MegState::get_lfo(lfo, sintab)` (meg.rs:373) — but note
the C++ trampoline calls `ms->get_lfo` without a sintab arg because the DEVICE
holds m_sintab (:645 reads `swp.m_sintab`); the Rust seam takes
`sintab: &[u16]` (meg.rs:373), so the trampoline must stash the sintab pointer
in MegJit (Phase B). Post-call: `load_p_limits()` (:1448) MANDATORY; on Windows
SEED/K_MAX need no push (callee-saved, comment :1435-1436).

## 4. Env flags (read-once semantics; mirror jit.rs env_flag idiom)

C++ file-scope/`static`-once reads (rationale: per-sample call cost, :311-317);
Rust mirror = resolve in `MegJit::new()` via `env_flag(name, off)` (jit.rs:148-154).

| Flag | C++ cite | Default | Semantics | Rust mapping |
|---|---|---|---|---|
| `SMU2000_MEG_JIT` | :346-357 | **ON** (`!(e && e[0]=='0')`) | master switch; off ⇒ `m_jit.reset()` (:361-363) | `env_flag("SMU2000_MEG_JIT","0")` in `MegJit::new` (jit.rs:307 pattern); gate = `can()` (jit.rs:344) |
| `SMU2000_MEG_BAKE` | :318-321 | **ON** | enables the spec/baked-constant second code version (:403-421) | `env_flag("SMU2000_MEG_BAKE","0")` |
| `SMU2000_MEG_JIT_CHECK` | :322-325 | **OFF** (`e && e[0]!='0'`) | A/B verify each compiled sample vs interpreter (:426-511) | **NOT expressible with `env_flag` (inverted polarity)** — add tiny helper `env_flag_on(name)` = `var_os(name).is_some_and(|v| !v.starts_with('0'))`. Phase B addition inside meg_jit.rs only. |
| `SMU2000_MEG_JIT_UPTO` | :327-342 | `0x180` (=MEG_OPS :332); only read when CHECK set (:336) | bisect: emit/step only the first N ops — meaningless without CHECK (:328-331) | `OnceLock<u32>` in meg_jit.rs; drives both the compile loop bound AND the CHECK interpreter leg (:444-448) |
| `SMU2000_MEG_EARLY` | :703-706 | **ON** | enables the early-write analysis (:668-711) | `env_flag("SMU2000_MEG_EARLY","0")` in `MegJit::new`, stored as bool field |
| `SMU2000_MEG_JIT_STATS` | :1713-1731 | OFF | per-build stderr breakdown | port behind same env (stderr writes only at build time — never in `run`) |
| JIT32 (`…_LOG/_DUMP/_DUMPSAMPLE`, macro SMU2000_MEG_JIT32) | :1732-1754, :32-34 | — | x86-32 debug | **DEAD — do not port** |
| `SH2_JIT_TRACE/HASH` analogue | — | — | no MEG analogue; trace/hash = interpreter only (jit.rs:15-18, :312-317) | Machine keeps `meg_jit` OFF while `hash_on`/`trace_on` (jit.rs:342-343 pattern) |

## 5. Compile / enter lifecycle — where bit-exactness lives

### 5.1 Invalidation points (all set `m_meg_jit_wait = 1` too)

| Trigger | C++ cite | Rust today (inert leg to wire) |
|---|---|---|
| program/map/ops changed | swp30.cpp:4412-4413 | mix.rs:675-676 |
| skip-region wake (sound arrives) | swp30.cpp:4332-4336 (`meg_ops_rebuild` FIRST, then invalidate) | mix.rs:916-919 |
| region goes quiet | swp30.cpp:4371-4373 | mix.rs:980-982 |
| revram_enable_w (port 0x80e) | swp30.cpp:2482-2492 (change-test :2485) | regs.rs leg UNVERIFIED; meg.rs:15-17 (inert) |
| (belt-and-braces) run-time re-check of baked revram_enable | swp30_jit.cpp:396-400 → invalidate + wait + return false | Phase B: `MegJitHook::run` compares vs `MegSwp.revram_enable` (meg.rs:627) |

`invalidate` = `gen.fn = spec.fn = nullptr` (swp30_jit.cpp:380-386) ⇒ next
`run()` returns false ⇒ mix.rs falls to `meg_run_program` (mix.rs:725,
swp30.cpp:4444-4446). Code buffers are KEPT (re-`build` overwrites them —
swp30_jit.cpp:1696-1712, "rebuilds arrive still executable" :1706).

### 5.2 Per-program compile trigger from the sample loop (the answer to the dispatch question)

**swp30.cpp:4409-4416**: after `run_sample` sees no change, the latch
`m_meg_jit_wait` counts settled samples (`m_meg_jit_wait && ++m_meg_jit_wait >
64` at :4414); at `> 64` → `meg_jit_rebuild()` (:4416) → `build(gen)`
(swp30_jit.cpp:369). The comment :4409-4411: firmware writes programs over
hundreds of samples, recompiling every sample would dominate — so: invalidate
(:4412) → interpret ≥64 samples → rebuild (:4416→swp30_jit.cpp:369). Rust
mirror already latches the counter (mix.rs:677-683) — Phase B replaces the
"never happens" arm (mix.rs:680-683) with `jit.rebuild(&self.meg,
&self.meg_ops, …)`.
Second (lazy) compile arm INSIDE the run path: the BAKE/spec build at
swp30_jit.cpp:413-420, reached from swp30.cpp:4441/4444 every sample.

`rebuild` steps (swp30_jit.cpp:359-377): rebind `j.ops = m_meg_ops.data()`
(:368), `build(gen)` (:369; false ⇒ gen.fn=null, interpret until next rebuild
:370), latch `gen.revram_enable` (:371), drop spec (:373-374),
`seen_const_gen = m_meg_const_gen` (:375), `stable = 0` (:376).
`m_meg_const_gen` bumps on every `m_const` write (swp30.cpp:3378; Rust seam
`const_w(.., const_gen)` meg.rs:288) — `m_meg_off_gen` (:3391) is NOT a JIT
input (offset table read at runtime, header :12; it only feeds native-FX
:4490, dead).

### 5.3 `build()` compile sequence (swp30_jit.cpp:598-1756, x64 arms)

1. **branchy scan** :607-610 — any `ops[pc].jump` ⇒ whole program compiles in
   ring-per-op mode (:15). **RAM guard** :611-612 — `reverb_ram < 0x40000` ⇒
   false (interpreter; Rust: MegSwp.reverb_ram len check meg.rs:628-629).
2. **ring snapshot**: `d3=ms.m_delay_3`, `d2=ms.m_delay_2` :614-615;
   `slot3(k)=(d3+k)%3`, `slot2(k)=(d2+k)%2` :616-617. **Saved per code as
   `code.d3/d2`** (:273, :603) and RE-CHECKED EVERY CALL in
   `meg_jit_run` :423-424 — a d3/d2 mismatch (state reload mid-stream) ⇒
   return false ⇒ interpreter. THE load-bearing guard; port verbatim.
3. **field offsets** :620-652 via `off(base,field)` (works because MS/SWP arrive
   as ARG pointers :829-831 — the JIT bakes OFFSETS, not device addresses;
   RAM is ARG2 :831, fresh per call :512). Rust: `offset_of!` table (jit.rs:269
   precedent) over `#[repr(C)]` MegState/Swp30 — see deviation §7-B/D.
   `sintab` captured ONCE per compile (:645, baked at :937 when
   `count ≥ 0x8000`, else callout fallback :1430+).
4. **layout static asserts** :654-657 (widths of mw_reg/act/flags/t_value/
   m_const/m_offset/m_r — Rust: `assert_eq!(size_of)` compile-time asserts).
5. **`need_tval[0x180]`** :660-666 — op k must publish its `t_value` ring slot
   if k≥0x17e (tail) or op k+2 writes t-from-p. Folded-alternative of
   meg.rs:702/:737 `t_value[d2] = s16_p23_clamped(p)` (meg.rs:611-613).
6. **early-write analysis** :668-711 (gated `!branchy && MEG_EARLY` :707-710):
   a 3-cycle write may be stored DIRECTLY to `m[r/regs]` at the writing op iff
   no read of that reg occurs 1 or 2 ops later (`reads()` predicate :675-690,
   window :691-698) and it isn't written by the tail trio 0x17d-0x17f
   (:699-702). `last_slot_*` :715-721: even folded writes still land in the
   ring slot when they are LAST for that slot — "not read, but keeps the state
   save identical" :713-714. Emission: :1467-1472 (dm), :1488-1493 (dr).
   **Rust invariant**: `m_mw_value/m_mw_reg`… are STATE (serialized) — any
   fold that skips a ring-slot write breaks byte-EQ saves; copy the
   last-slot rule exactly.
7. **prologue** :826-837 + branchy skip reset `store32i [SWP+o_skip],0`
   :839-840 (device var `m_meg_jit_skip` swp30.h:614; Rust MegSwp analogue =
   `skip_to` meg.rs:626 — check width/ownership Phase B).
8. **LFO hoist** :971-991: count uses per LFO index (`dm_src≤3`, `lfo<0x18`,
   only when sintab :976-979); indexes used ≥2× get `emit_lfo(i)` at the head,
   stored to frame slot `LFO_SLOT_BASE+4n` (:985, :816, FRAME 152 :817);
   `lfo_slot[0x18]` maps idx→frame offset (0 = not hoisted). `emit_lfo`
   :897-969 is the transliterated `get_lfo` (meg.rs:373-…): counter>>5
   (:902-903 = meg.rs:386), pitch shift bits8-9 via `shl32cl` (:904-908 =
   meg.rs:387), offsets table (:898-901 = meg.rs:375-380), wave select
   bits10-11 (:920-923): sine = fold idx over 0x8000 (:926-931) + sintab load
   (:937-939) + sign fold bit16 (:941-944); tri = +0x8000, &0x1ffff, fold
   :951-956; saw up = >>1 :962; saw down = ^0x1ffff >>1 :965-966; result
   `<<7` :968. Hoisted values are loop-invariant **because `lfo_step()` runs
   only between samples** (:892-896; Rust `lfo_step` meg.rs:337 — do NOT move).
9. **per-op loop k=0..0x180** (:995). Keep the EXACT ordering within each op:
   ring-apply → branch gate → ALU → dm → dr → memw → index → t → memop →
   post-branch fixups:
   - **3-ring apply** :1000-1049. Head (`k<3||branchy`) :1000-1029: dynamic —
     load `mw_reg[s]`, test, `m[reg]=mw_value[s]` (index store with scale 4,
     :1007); same for rw→r :1010-1015; `ix_act[s]`→`ram_index` :1017-1022;
     ix2→SWP `ram_index2` :1024-1029. Folded path :1030-1048: straight
     `load32 [slot]→store [reg]` for ops[k-3]'s dm/dr UNLESS early_m/early_r
     :1033-1040; index/index2 always folded :1041-1048.
   - **2-ring (mem ports)** :1050-1077: head clears `memw_act/memr_act` slots
     after applying (:1057, :1064); folded from ops[k-2] :1066-1077
     (`memop==2||3` ⇒ ram_read :1073-1076).
   - **branch gate (branchy only)** :1079-1104: `cmp32i_mem [SWP+o_skip],k` +
     `ja` → body-skipped (:1082-1083); `meg_cond` in machine code :1087-1096
     (cond&8 ⇒ test flag_n via loadu8+xor1, cond&2 or's z, `jz no_jump`);
     taken jump sets skip=target when target>k :1098-1099; jump op itself
     falls into the skipped post-amble via `jmp_done` :1102. Body elided when
     `branchy && o.jump` (:1105 → :1639). **Skipped/jump-tail path**
     :1641-1678: zero the ring bytes `mw_reg/rw_reg (slot3), memw_act (slot2),
     ix_act, ix2_act` (:1655-1659) — the write the skipped op "would have
     queued is erased, only t kept" (:606, header :15); jump op still writes t
     when `t_write` (:1647-1654); `need_tval` publish :1660-1675 (same p>>23
     ±0x8000 clamp as :1540-1560). `target` math already resolved by
     build_ops (Rust Op.target meg.rs:1047; target>k compare is a COMPILE-time
     constant :1098).
   - **ALU** :1107-1409. Bake fold :1111-1159 (only `!m1_from_t && mmode!=3`
     :1111): `alu_skip` when m≡0 and asel==0 and rop==0 and shift==0 and
     clamp==0 and !latch — "p already fits 42 bits" :1115-1117; m≡0 other ⇒
     `xor32` acc :1118-1123; mmode1 ⇒ `imm64 c<<(8+15)` :1128; else loads +
     **pow2 constant ⇒ shl/neg instead of imul** :1138-1157 (imul 3 cycles →
     shl 1, comment :1139-1141; neg :1154). Runtime: m1 source :1190-1259 —
     `m1_from_t==2` = sign-of-p select t-vs-const (`loadu8 flag_n; test;
     cmove64` :1230-1236, the "upstream.md 29" quirk, meg.rs header);
     `m1_expand` via `emit_m1_expand` (:1241-1242, helper :115; ground truth
     meg.rs:421); mmode switch 0/1/2/3 :1243-1258 (`loads32` + `imul64`; 3 =
     load + `shl64 15`); asel :1266-1281 (**asel==0 uses the P register
     directly, no copy** :1261-1262/:1275; 1/2 = `loads32 r/m<<15`
     :1276-1277; 3 = `sar64 15` :1278); rop :1282-1321 (`add64` :1287,
     `sub64` :1294, |b| via `mov64/neg64/cmovs64/add64` :1308-1311, `and64`
     :1318); `shl64 o.shift` :1322-1327; **clamp==0 wraps ±2^41 via
     `shl64 22 / sar64 22`** :1328-1336; clamp1 saturate `cmp64 P_MIN/
     cmovl64; cmp64 P_MAX/cmovg64` :1351-1354; clamp2 [0..P_MAX]
     `xor32;cmp64;cmovl64, cmp64 P_MAX;cmovg64` :1366-1371; clamp3 |acc| ≤
     P_MAX :1383-1388; `mov64 P,RAX` + latch `test64/setl_mem flag_n/
     sete_mem flag_z` :1401-1407 (42-bit limits ±(2^38−1) = P_MAX/P_MIN
     :820-822; sibling ground-truth `s16_p23_clamped` meg.rs:611).
   - **dm** :1414-1475: src 0-3 LFO (frame slot :1419 / in-place emit_lfo
     :1421 / callout :1430-1448); 4 = `ram_read` :1453; 5 = `rnd();shl8;sar8`
     :1455-1459; 6 = `p_packed(!no_noise)` :1460-1462; 7 = `m[sm]` :1464.
     Store :1467-1472 (early+last_slot vs ring slot). Tail/branchy reg byte
     `store8i mw_reg[slot3], o.dm` :1474-1475.
   - **dr** :1480-1496: `rand_n` ⇒ `rnd_skip` FIRST (:1481-1482 —
     rand_jump(n) seed multiply, meg.rs:580-592; n comes from
     meg_ops_rebuild's carry coalescing swp30.cpp:4282-4298 / mix.rs:845-858,
     which folds consecutive skipped regions' draws into the LAST skipped op
     so the seed advance stays at the same stream position, :4272-4273
     comment); value = `r[sr]` (dr_from_r) or `p_packed(!no_noise)`
     :1484-1487; store early/ring :1488-1493; rw_reg byte :1495-1496.
   - **memw** :1501-1509: `AccFromP; ShrAccTZ15` (= `meg_mem_value`
     round-toward-zero, meg.rs:572) → `memw_val[slot2]`; act byte :1508-1509.
   - **index** :1511-1525: `p>>(15+8)` → `ix_value[slot3]` :1513-1515 / ix2
     (SWP) :1519-1523; act bytes :1517-1518/:1524-1525.
   - **t** :1530-1561 (+ branchy twin :1647-1675): `t_write` reads
     `t_value[slot2]` (from-p) or const (baked imm :1535, runtime load :1537)
     → `store16 t[...]` :1538; `need_tval` publish: index-ops `>>8 &0x7fff`
     :1542-1544 else `>>23` saturated ±0x8000 via cmp/cmov pairs :1545-1558 →
     `store16 t_value[slot2]` :1560 (ground truth meg.rs:702/:737).
   - **memop** :1566-1638: `mem_table` (bit 0x23 absolute read, upstream.md
     24) :1568-1586: offset word + `ram_index` + `ram_index2` (+1 if memop3)
     `&0x3ffff` → RAM loadu16 → `emit_revram_decode` → `memr_val[slot2]`;
     jmp around the normal path :1585/:1635-1636. Per-region enable gate
     `BIT(swp.m_revram_enable, o.region)` resolved AT COMPILE TIME :1589
     (disabled: write dropped / read forced 0 :1590-1591 — why invalidate
     lives in revram_enable_w :2488-2491, double-covered :394-400); addr =
     (offset + ix + ix2 − SC :1593-1606, memop3 +1 :1608) `& addr_mask
     + addr_base &0x3ffff` :1610-1612 (mask/base baked :1610-1611); write:
     R8=addr, `emit_revram_encode(ram_write)` → `store16 [RAM+R8*2]`
     :1623-1626 (scale-2 `mem{RAM,R8,2,0}` — Rust: `Mem{base,index,scale:2,
     disp}` literal, fields pub jit_emit.rs:38-43); read: loadu16 +
     `emit_revram_decode` :1630-1632. `memr_act` byte :1637-1638.
10. **epilogue** :1688-1694: `store64 m_p←P`, `store32 swp.m_rand_seed←SEED`
    (:1690 — ONLY place the seed returns; mid-program seed stays in RSI),
    `addrsp 152`, pop×8, `ret`.
11. **buffer handoff** :1696-1712: grow at 64 KiB granularity (:1699),
    `exec_mem::alloc_rw` (:1700), **make_writable → memcpy → make_executable**
    (:1707-1711 — rebuild overwrites an already-exec buffer), `fn = buf`
    (:1712). `code` dtor frees via `exec_mem::free_mem` (:277-283);
    `meg_jit_delete` (:304-307) = Rust `Drop`.

### 5.4 Per-sample enter (`meg_jit_run` swp30_jit.cpp:388-516 ← swp30.cpp:4444)

Order (port verbatim): null-guards :390-392 → revram_enable re-check :396-400 →
pick code: **BAKE/spec state machine** :403-421 (const-gen changed ⇒
`stable=0, spec_tried=false` :404-408; else `stable++` up to `STABLE=8192`
(:295); spec valid for current gen ⇒ use spec :411-412; just-stable ⇒ build
spec ONCE :413-415, on success use it :416-417, else gen :418-419) → **d3/d2
guard** :423-424 → [CHECK A/B leg :426-511, §9] → `c->fn(m_meg, this,
m_reverb_ram.data())` :512 → **then the caller-side half of run_program**:
`m_pc = 0; m_icount -= 0x180` :513-514 (mirrors meg.rs:1000-1002 pc wrap + the
run_program icount tail; the CHECK comment :451-453 states run_program
subtracts 0x180 itself). Rust: hook returns bool; mix.rs:722-726 becomes
`if !jit.run(..) { self.meg_run_program(sintab) }`; the pc/icount post-leg
inside the hook's success path, :513-514 ordering (fn call first).

## 6. Emitter gap table (Rust `jit_emit.rs` vs methods called by LIVE x64 code)

Method set extracted from swp30_jit.cpp:598-1756, the live x64 helpers :59-178,
and selftest :520-585, gated through `#if SMU_X64ASM_MODE==32` /
`#ifdef __aarch64__` (script this session; x64-only result). **67 distinct
methods. Result: ZERO missing, ZERO renames** — `jit_emit.rs` is a 1:1
transliteration of x64asm.h:37-213 mode-64 (jit_emit.rs:1-4 header). C++1st =
first x64-mode use in swp30_jit.cpp (helper lines where helper-only); Rust =
jit_emit.rs definition line.

| method | C++1st | Rust | method | C++1st | Rust |
|---|---|---|---|---|---|
| add32 | :918/:104 | :213 | load32 | :833 | :152 |
| add32i | :861 | :231 | load64 | :832 | :146 |
| add32ri | :951/:132 | :442 | loads32 | :1138 | :158 |
| add64 | :848 | :192 | loads16 | :1232 | :161 |
| addrsp | :1691 | :297 | loadu16 | :904 | :164 |
| and32 | :161 | :219 | loadu8 | :1003 | :167 |
| and32i | :847/:77 | :234 | mov32 | :862 | :369 |
| and64 | :1318 | :198 | mov64 | :829 | :143 |
| bsr32 | :88 | :493 | neg32 | :131 | :457 |
| call_abs | :1443 | :320 | neg64 | :1154 | :252 |
| cmove64 | :852 | :264 | or32 | :1093 | :478 |
| cmovg64 | :1354 | :258 | or32ri | :87 | :480 |
| cmovl64 | :1352 | :255 | patch | :931 | :313 |
| cmovs64 | :1310 | :261 | pop | :1445/:1692 | :287 |
| cmp32i_mem | :1082 | :331 | push | :827 | :279 |
| cmp32ri | :948/:124 | :448 | ret | :1694 | :301 |
| cmp64 | :1351 | :201 | rol32 | :863 | :249 |
| cmp64ri | :851 | :267 | sar32 | :856 | :246 |
| imm32 | :156/:1442 | :183 | sar64 | :846 | :240 |
| imul32i | :860 | :225 | setcc | :102 | :472 |
| imul64 | :1252 | :222 | sete_mem | :1406 | :273 |
| imul64i | :1149 | :228 | setl_mem | :1404 | :270 |
| jcc_fwd | :924 | :337 | shl32 | :855 | :243 |
| jmp_fwd | :945 | :345 | shl32cl | :908 | :487 |
| jz_fwd | :1005 | :304 | shl64 | :1152 | :237 |
| shr32 | :903 | :454 | shr32cl | :106 | :490 |
| store16 | :1538 | :170 | store32 | :988 | :155 |
| store32i | :840 | :328 | store64 | :1689 | :149 |
| store8i | :1057 | :173 | sub32 | :1606/:103 | :216 |
| sub32ri | :89 | :445 | sub64 | :1294 | :195 |
| subrsp | :828 | :295 | test32 | :923/:101 | :207 |
| test32ri | :928/:117 | :451 | test64 | :1403 | :204 |
| xor32 | :1119/:83 | :210 | xor32ri | :930 | :484 |

Explicitly NOT needed (ledger NEXT:32-33 asked about these): `cmovcc64`,
`cmp64i`, 4-arg `add64/sub64` acc-pair forms, `jlt64i/jgt64i`,
`push(ESI)`-cdecl, table-base-in-imm32 legs — all live ONLY in the x64asm.h
**x86-32** class (x64asm.h:232+, DEAD here); x64 mode uses the named
`cmovl64/cmovg64/cmovs64/cmove64` (:1352-1354/:1310/:852), `cmp64ri` (:851),
`sar64` (:846). `slot3/slot2` (:616-617) are compile-time lambdas — plain Rust
arithmetic, no emitter method. `call_reg` (:276) only behind `call_abs` (:320).
Scale addressing `Mem{base,index,2|4,disp}` (:1007, :1582, :1630): `rm()`
already encodes scales 1/2/4/8 (jit_emit.rs:125-133); construct via struct
literal (pub fields jit_emit.rs:38-43) — additive `Mem::bis_scale` optional.
`patch_to` (:362) unused by MEG (all `patch(at)` end-target jumps,
swp30_jit.cpp:931/x64asm.h:159).

## 7. Deviations required (Rust rules vs C++)

**A. Crate placement** — `meg_jit.rs` in **smu-machine** (deps
smu-machine→smu-swp30; reverse = cycle), NOT smu-swp30 as the dispatch
suggested. Device-side seam = `pub trait MegJitHook` defined in mix.rs:
`run(&mut self, meg, &mut MegSwp-parts…) -> bool`, `invalidate(&mut self)`,
`rebuild(&mut self, …) -> bool`, `can() -> bool`. Machine owns `meg_jit` and
hands it into a new `Swp30::run_sample_jit(sintab, wave, &mut dyn MegJitHook)`
(disjoint-field borrows at lib.rs:3048/3092/3101; jit.rs `run_core` carries the
precedent jit.rs:491). Harness/vectors keep `run_sample` → hook never runs
(behaves as today's inert legs mix.rs:675/680/724-725 — byte-EQ by
construction).

**B. Borrowck / pointer laundering** — compiled code mutates MegState, Swp30
parts and RAM through baked offsets while Rust holds `&mut` at the call
boundary. Launder exactly like jit.rs:70-73 ('static erasure inside the enter
only, single-threaded, strict nesting). `offset_of!` pins require `#[repr(C)]`
on MegState (partly pinned for state.rs save already? UNVERIFIED) and on
whatever Swp30-part bundle is baked (alternative: a small `#[repr(C)]`
MegSwpDev box with seed/flags/ix2/ram_index2/skip so SWP-base offsets are
pinned ONCE; decide in Phase B; keep the C++ two-base regime: MS offsets + SWP
offsets, swp30_jit.cpp:621-652).

**C. Exec memory** — C++ per-code `exec_mem::alloc_rw` + W^X flips (:545-552,
:1696-1712, :277-283). Rust precedent = RWX `VirtualAlloc(PAGE_EXECUTE_READWRITE)`
(jit.rs:46-57, :380-384). Deviation: per-code RWX VirtualAlloc, direct memcpy on
rebuild (skip make_writable/make_executable). Buffer growth happens at
rebuild-on-the-audio-thread (C++ does the same, :1700 reached via
swp30.cpp:4416 — the "zero allocations in the audio callback" invariant
already carries this from ground truth, amortized at 64 KiB steps :1699-1705);
document in ledger, no gate change.

**D. Offsets vs runtime pointers** — RAM/MS/SWP arrive per call as ARGs
(:512, :829-831) so state reloads, `reverb_ram` Vec resizes (meg.rs:628-629 ←
swp30.cpp:4717) and RefCell moves are safe. But **sintab base is baked at
compile time** (:937). If the sintab slice can be re-allocated (machine
rebuild / ROM reload) the code MUST be invalidated — natural today because
machine (re)init also flips `meg_program_changed`→invalidate
(state.rs:169-171 / :4743-4745 pattern). Pin an assert + ledger note.

**E. MEG_JIT_CHECK struct-byte compare** — C++ memcmps `meg_state` bytes up to
`m_icount`'s offset (:449-457, icount+neighbor retval exempt :451-453). Rust
MegState is not guaranteed byte-comparable (Decoded has explicit STATE_SIZE
save, meg.rs:1569). Deviation: CHECK compares (a) field-wise m_m/m_r/m_t/p/
rings/ram_index/ram_read/ram_write + (b) seed `*swp.seed` (meg.rs:630) +
(c) `flag_n/flag_z` (meg.rs:621-622) + (d) `reverb_ram` — the exact checklist
C++ prints :461-509. Keep the 40-report cap idea (:461-462).

**F. `env_flag` polarity** — CHECK/UPTO need the inverted helper (§4).

**G. Windows-only** — `#![cfg(all(target_arch="x86_64", target_os="windows"))]`
like jit.rs:24; `SYSV_ABI=false` (jit_emit.rs:34) — drop the SysV SEED/K_MAX
push-pop legs (:1437-1440/:1444-1447) as jit.rs dropped SysV call arms
(jit.rs:22). Other targets: hook never runs (C++ = build() never entered;
same observable behavior).

**H. rand/SEED thread-safety** — seed stays inside the audio thread only
(meg.rs:630; C++ `m_rand_seed` baked as SWP offset :646). No shared state
added; SPSC ring rules untouched. `rnd`/`rnd_skip` LCG constants 1664525/
1013904223 + `rol16` (:859-864) must match `swp_rand` (meg.rs:28, call_rand
meg.rs:438-439) bit-for-bit — CHECK gate §9 covers.

**I. Ops table ownership** — C++ JIT holds `j.ops` raw pointer rebound per
rebuild (:290, :368). Rust: **copy** the `Op` array (0x180) into MegJit at
rebuild — sound because every ops mutation is paired with invalidate + the
64-sample interpret window before the next rebuild (§5.1-5.2), so compiled
code never reads a mutated table. Removes a `&'static Op` laundering headache.
Deviation noted.

**J. state.rs** — compiled code is NOT state (mirror: `m_jit` is a device
unique_ptr outside the save stream, swp30.h:598; the OPS table is already
handled as a non-state rebuild-artifact — meg.rs:1005-1009, and state load
forces `meg_ops_stale=true`, state.rs:82/169-171 ← swp30.cpp:4743-4745 ⇒
run_sample :4383 leg ⇒ invalidate :4412). State save/load needs NO new
fields; requirement: every Rust loader goes through `run_sample_impl`'s
:638-639 gate (it does, mix.rs:637-638). Phase B: verify every loader sets
`meg_program_changed`/`meg_ops_stale` exactly like state.rs:169-171; any that
doesn't must call `jit.invalidate()`.

## 8. Phase B / C split and gates

**Phase B (same subagent session, per ledger NEXT:35-38) — code lands:**
1. `smu-machine/src/meg_jit.rs`: `MegJit` struct (gen/spec code structs
   :269-284/:286-295), env flags (§4), `rebuild/invalidate/run` (§5.1-5.4),
   `build()` transliteration of :598-1756 x64 arms ONLY (helpers
   `emit_revram_encode/decode`, `emit_m1_expand` :75-178; `emit_lfo` :897-969;
   `pack24/rnd/rnd_skip/p_packed/AccFromP/ShrAcc/ShrAccTZ15` :843-889; op
   loop §5.3; prologue/epilogue §3). `// origin: swp30_jit.cpp:N` per block.
2. `mix.rs`: `MegJitHook` trait + wire mix.rs:675, 677-683, 722-726, 916-919,
   980-982 (and regs.rs revram leg if a hook handle reaches; else rely on the
   run-time re-check :396-400). `run_sample_jit` variant.
3. `lib.rs`: Machine field + swap the 3 run_sample call sites (lib.rs:3048/
   3092/3101); hash/trace keep JIT off (jit.rs:342-343 pattern).
4. `meg.rs`/`state.rs`: `#[repr(C)]` + asserts only where §7-B/D demands; no
   logic edits.
5. `tests/meg_jit.rs` (Cargo [[test]] like tests/jit.rs): offset pins; port
   `meg_jit_selftest` (:520-585 — encode sweep 0..0x8000000 + edge values
   :555-560, decode 0..0x10000 :562-564, m1_expand −0x8000..0x8000 :573-576
   vs meg.rs:421/454/473); CHECK leg (`SMU2000_MEG_JIT_CHECK=1` + `_UPTO`
   bisect support). **Zero `jit_emit.rs` edits expected** (§6) — any comfort
   ctor is additive-only (SH-2 JIT shares the file, jit.rs:32).

**Phase C (follow-up session if B blows budget):** perf polish (STATS-guided,
:723-728/:1713-1731), spec/BAKE ratio, JIT32 stays dead, ledger M9b row
close-out + commit (NEXT owes "ws re-run + commit by M9b closer", ledger M9
row end).

**Gate sequence (ledger NEXT:38-40, all in-session, same box/window):**
1. `cargo build --release` green (build from `rust\`; refresh flat exes after
   EVERY build — NEXT:42-43 stale-exe trap).
2. `tests/meg_jit.rs` green; selftest = 0 mismatches.
3. `python tools/run_tests.py` (ws, SMU_BUILD=build-rust) 562/562.
4. `--only piano` + dense: 合 (peak/rms fingerprints).
5. `boot_golden` green (JIT engages under boot; revram_enable flips mid-boot
   stress the :396-400 + :2490 invalidations; d3/d2 guard :423-424 stress =
   state reload paths).
6. `SMU2000_MEG_JIT_CHECK=1` live run ≥ boot window: ZERO `MEGCHECK` lines
   (§9); optionally one `_UPTO` bisect drill to prove tooling.
7. live wav BYTE-EQ vs C++ idle ×2 + live idle CPU ≤ ~25% (blocktime dense;
   bar C++ 11.7%).
8. Full suite + samptest STILL skipped (user directive, NEXT:39-40).
Ledger last action: M9 row evidence + `## NEXT` rewrite.

## 9. MEG_JIT_CHECK as the built-in A/B (reuse plan)

C++ protocol (:426-511), per compiled sample: snapshot `meg_state` COPY +
`reverb_ram` + seed + flags (:430-433) → run JIT (:434) → copy JIT-side
results (:435-438) → restore snapshot (:439-443) → run the interpreter from
the SAME starting state — `_UPTO<N` per-op `step()`s else full `run_program`
(:444-448) → byte-compare state struct up to `&m_icount` (icount + neighbor
retval exempt, :449-457) → RAM diff count/first (:458-460) → seed +
flag_n/flag_z (:461) → on mismatch, first 40 samples print m[]/r[]/t[]/p/ix/
rr/rw/d3-d2/mw/rw/ixv/memw/memr/tv dumps + the ops[] lines touching m32/33/48/
49 (:466-509). Semantics: CHECK never alters audible output (the interpreter
result is restored as truth, :439-448) — pure self-verification ⇒ ideal Rust
test seam: run live + vector cases with `SMU2000_MEG_JIT_CHECK=1`, require
zero `MEGCHECK` lines; CHECK+`_UPTO` bisects any divergence to an op index
(:328-331 — UPTO is meaningless without CHECK, keep that coupling). Rust
adaptation per §7-E.

## 10. Top risks (ranked)

1. **Seam/borrowck plumbing** — RefCell device + &mut MegJit + re-forming
   MegSwp-shaped disjoint `&mut`s from `&mut Swp30` at every run() entry;
   expect churn in mix.rs run_sample_impl (precedents lib.rs:3048/3092/3101,
   jit.rs:491, MegSwp meg.rs:620-632).
2. **Ring/t fold edge semantics** — head k<3/k<2 dynamic path (:1000-1077),
   tail reg-bytes (:1474/:1495/:1508/:1517/:1524/:1637), `need_tval` publish
   (:1540-1560, :1660-1675), skipped-op ring erasure (:1655-1659) and the
   d3/d2 re-check (:423-424) — one wrong slot mapping = silent audio drift,
   caught only by CHECK / wav-EQ.
3. **rand stream parity** — RSI-only seed across the program (:834/:1690),
   `rol16` draw shape (:859-864), `rand_n` coalesced skips (:1481-1482 ←
   swp30.cpp:4282-4298 / mix.rs:845-858), noise `and 0x07e0` only on dm6/
   dr-p legs (:874-882) — ordering slips are invisible except as dither
   drift.
4. **Saturation/42-bit edges** — pack24 off-by-one clamps (:843-857),
   P_MAX/P_MIN via R9/R10 RELOADED after the single callout (:819, :1448;
   S11 callee-save trap jit.rs:397-400), clamp==0 `shl22/sar22` wrap
   (:1328-1336), t ±0x8000 cmov pairs (:1545-1558).
5. **ABI/frame** — FRAME=152 keeps rsp 16-aligned ONLY with exactly 8 pushes
   (:817/:827-828); the call_lfo leg needs shadow space + `load_p_limits()`
   after (:1430-1448); sintab base baked (:937) needs the machine-rebuild
   flush tie (§7-D).

— Phase A end. Phase B starts from §8 step 1 with this file + §5 as spec.
