# VST2_LEDGER.md — S-MU2000 on iPlug2 (VST2/CLAP, Win32 + x64)

Ledger for adapting the S-MU2000 MU2000 emulator (`github.com/tarboh/S-MU2000`, already
checked out here) into an **iPlug2** plugin so it compiles as a **VST2** (and **CLAP**)
plugin, mirroring how `../sw10_plug` is built. **Sound engine first; GUI later.**

Reference build system to copy: `../sw10_plug` (iPlug2 submodule + CMake harness +
`SW10_PLUG/CMakeLists.txt` API targets). Read `../sw10_plug/AGENTS.md` and
`../sw10_plug/REFACTOR_PLAN.md` for the patterns being mirrored. **Update the phase
checklists here as work completes.**

## Current repo state (cold start — read first)

- This folder **is already a git checkout** of `https://github.com/tarboh/S-MU2000.git`
  (branch `main`). **Do NOT re-clone or re-init.** It is the upstream emulator repo; we are
  layering the iPlug2 plugin build on top of it in place.
- Present now: `src/`, `Makefile`, `third_party/` (vst3+imgui), `doc/`, `art/`, `tests/`,
  `tools/`, `roms/` (user ROMs — git-ignored, do not touch), `README.md`, `AGENTS.md`.
- **NOT created yet (P0 makes them):** `iPlug2/` (submodule), `.gitmodules`, `engine/`,
  `SMU2000_VST2/`, `cmake/`, root `CMakeLists.txt`, `CMakePresets.json`. So the
  `cmake --preset …` commands in `AGENTS.md` do **not** work until P0 lands. Start at P0.
- Verified facts a cold agent can rely on (checked against the tree): sw10's iPlug2 submodule
  pin is `d54f69050f517e43b941d88c2a170f0a840b9ee4`; `roms/` + `roms/mu2000_flash.bin` are
  confirmed git-ignored; `engine.cpp`/`mu2000.cpp`/`driver.h`/`bridge.h` do **not** ODR-use any
  `xg::model.cpp` symbol, so `xg/model.cpp` is genuinely excluded from the engine link.

---

## Execution protocol (subagents)

- **Every phase below is executed as a dedicated subagent.** Do not do phase work inline.
- **At most 2 subagents run concurrently.** Honour the wave schedule; never exceed 2 at once.
- A subagent owns its phase's acceptance gate and reports back done/blocked with evidence
  (build log tail, artifact paths). The next wave starts only when its dependencies pass.
- Subagents keep edits surgical: engine sources under `src/`, plugin under `SMU2000_VST2/`,
  harness under `cmake/` + root files. **Never edit the `iPlug2/` submodule tree** (changes
  are lost on checkout); put all shims in `cmake/` like `../sw10_plug/cmake/mingw_compat.cmake`.

### Wave schedule (≤2 concurrent)

| Wave | Subagents (parallel) | Depends on |
|---|---|---|
| 1 | `P0-scaffold` | — |
| 2 | `P1-engine-lib` | P0 |
| 3 | `P2-vst2-wrapper` ‖ `P3-clap-wrapper` | P1 |
| 4 | `P4-gui-toggle` ‖ `P5-mingw-clang` | P2, P3 |
| 5 | `P6-ci-github` | P2, P3, P4, P5 |
| 6 | `P7-gui-editor` | P2, P3, P4 |

(Phases 0–7 all green. P7 shipped the native GDI editor (reused from the VST3 GUI), not the
originally-sketched IGraphics rewrite — see §Phase 7.)

---

## Locked decisions

1. **iPlug2 = git submodule**, pinned to `d54f69050` (same gitlink as `../sw10_plug`).
2. **Architectures: Win32 + x64** VST2 and CLAP (both). MSVC is this session's toolchain.
3. **VST2 session state wired in pass 1**: `PLUG_DOES_STATE_CHUNKS 1`, map
   `SerializeState`/`UnserializeState` → `engine.save_state()/load_state()`.
4. **ROM discovery = reuse `engine.cpp::find_roms()`** (no new IPlug2 settings-dir code).
   Post-build **stages the `roms/` folder next to each built DLL/CLAP**, exactly like
   sw10 stages `ROMSXGM.BIN` (`SW10_COPY_ROM`) — see §ROM policy.
5. **CLAP built the same way as sw10** (`sw10_clap MODULE` → `.clap` via `iplug_configure_target(... CLAP)`).
6. **GUI is a build-time toggle**: `-DSMU2000_ENABLE_GUI=ON` (also accepted alias `-DENABLE_GUI=1`).
   Default **OFF** → engine-only plugin, `PLUG_HAS_UI 0`, no IGraphics/NanoVG/Skia/OpenGL linked.
7. **MSYS2 MinGW GCC + Clang compatibility is its own phase** (P5), mirroring sw10's
   `mingw_compat.cmake` / `mingw_portability_prelude.h` shims. Zero submodule edits.
8. **GitHub Actions CI is its own phase** (P6), mirroring `../sw10_plug/.github/workflows/build-native.yml`.
9. **The `roms/` folder is NEVER committed and NEVER deleted.** It is already git-ignored.

---

## Findings (from codebase recon — drives the whole design)

- **The engine is already GUI-free and VST-free.** `smu2000::engine`
  (`src/vst3/engine.{h,cpp}`) includes only `engine.h`, `mu2000.h`, `nvram.h`,
  `smartmedia.h`, `ui/bridge.h`, `ui/driver.h`, `ui/resampler.h`. Those three `ui/`
  headers are **header-only** (no `.cpp` to compile). Only `src/vst3/plugin.cpp` pulls in
  Steinberg `pluginterfaces/*` — so the engine compiles into a plain static lib unchanged.
  (The `smu2000::vst3` namespace name is cosmetic; keep it to avoid churn.)

- **Engine API a plugin needs (all present):**
  - `engine::start()` → finds ROM + boots firmware on a **background thread**; returns immediately.
  - `engine::set_output_rate(double)` → windowed-sinc resample from native 44100; bypass at 44.1k.
  - `engine::midi(const uint8_t* bytes, size_t n, int port=0)` — raw MIDI bytes (port 0 = MIDI IN A / parts 1-16, port 1 = B / 17-32). Queued safely if still booting.
  - `engine::fill(float* L, float* R, int n, const float* in_l, const float* in_r)` → emits stereo float; **silence until `status::ready`** (`engine.cpp:463`). Also pumps buttons/MIDI/wheel internally.
  - `engine::save_state()` / `load_state(ptr,n)` → whole-machine blob. `save_state()` defers onto the audio thread via `serve_state()` (`engine.cpp:543`), so main-thread chunk calls are safe.
  - `engine::all_notes_off()`, `latency_samples()`, `panel()` (bridge, for later GUI).

- **Engine core sources to compile** = the Makefile `OBJS`
  (`src/compat/compat.cpp`, `src/smartmedia.cpp`, `src/mame/sound/swp30{,_jit}.cpp`,
  `src/mame/video/hd44780.cpp`, `src/mame/machine/sci4.cpp`,
  `src/mame/cpu/{sh,sh2,sh2_jit,sh7042,sh_adc,sh_bsc,sh_cmt,sh_dmac,sh_intc,sh_mtu,sh_port,sh_sci}.cpp`)
  **+ `src/mu2000.cpp` + `src/vst3/engine.cpp`**.
  **Header-only (no `.cpp` to compile, pulled in via `engine.h`):** `ui/bridge.h`,
  `ui/driver.h`, `ui/resampler.h`, `ui/snapshot.h`, and — transitively through `driver.h` —
  `xg/ram.h` (all `constexpr`/inline) and `xg/model.h` (types/enums only).
  **Excluded (GUI/tool-only):** all other `src/ui/*.cpp`; all `src/vst3/*.cpp` except
  `engine.cpp` (plugin/view/iids/probe); `src/xg/model.cpp`; `third_party/imgui`;
  every `src/*.cpp` tool main (`render/live/gui/boot/rec/verify/…`).
  *P1 link safety:* engine path was verified not to call `xg/model.cpp`; if a link error names
  an `xg::` symbol, add `src/xg/model.cpp` to the `smu2000_engine` source list (22 KB, harmless).

- **32-bit JIT is already safe.** Both JITs are gated `#if defined(_WIN32) && defined(__x86_64__)`
  (`swp30_jit.cpp:22`, `sh2_jit.cpp:27`), with interpreter fallbacks
  (`swp30_jit.cpp:293` returns false when disabled). MSVC-win32 does **not** define
  `__x86_64__` → `SMU2000_MEG_JIT=0`, `jit_enabled()=false`, `x64asm.h` never included.
  **Verify** `sh2_jit.cpp` has the matching interpreter stub for `jit_enabled()/jit_run()`
  when its guard is false (mirror of the swp30 pattern). Engine runs interpreter-only on both archs under MSVC.
  - *Optional x64-only perf follow-on:* the JITs emit the Microsoft x64 ABI (rcx/rdx/r8/r9),
    so widening the guard to `(defined(__x86_64__) || defined(_M_X64))` could re-enable JIT on
    MSVC-x64. **Default OFF** for pass 1; gate behind `SMU2000_ENABLE_JIT` (x64-only) once sound is proven.
  - **SUPERSEDED 2026-09-16 (CPU32_LEDGER.md Phases 1–8):** both JITs are now dual-mode —
    `__x86_64__ || _M_X64` (MSVC-x64 gets the JIT) **and** `__i386__ || _M_IX86`
    (x86-32, self-defined `SMU_JIT32_PORT_SH2/MEG`). Win32 is **no longer interpreter-only**;
    MSVC-win32/x64 both run the JIT by default (`SMU2000_{MEG,SH2}_JIT=0` kill-switches).

- **MSVC portability looks low-risk.** Scan of all 71 core files found **no** `__attribute__`,
  `__builtin`, `typeof`, `__int128`, GCC statement-exprs, or SIMD intrinsics. Known MSVC items:
  `std::rotl/rotr` (`<bit>`, fine under `/std:c++20`) and `M_PI` in `engine.cpp` → needs
  `_USE_MATH_DEFINES`. This codebase is currently g++-only, so expect first-pass `/W-level`
  diagnostics; isolate all fixes in the engine CMake target, not in iPlug2.

---

## ROM policy (hard rule)

- `roms/` exists and contains `mu2000_flash.bin` (4MB program ROM), `dump/` (wave ROMs),
  `standin/` (MEG sin-table). It is **already in `.gitignore`** (`/roms/` style entry present).
- **Never `git add` anything under `roms/`. Never delete it.** Yamaha ROMs cannot be
  distributed (see `README.md`); the user dumps them from their own MU2000.
- Keep the existing unanchored `roms/` ignore, but **verify it stays tracked-correct**: do not
  let a future `build-*` style ignore swallow `.github/workflows/`. Add `/build-cmake/` (anchored).
- **Runtime location:** `engine.cpp::find_roms()` probes, in order: env `S_MU2000_ROMS` →
  `<dllDir>\..\Resources\roms` → `<dllDir>\roms` → `<dllDir>` → `roms.txt` pointers →
  `%LOCALAPPDATA%\S-MU2000\roms` → `%USERPROFILE%\Documents\S-MU2000\roms`.
- **Build-time staging (mirror sw10 `SW10_COPY_ROM`):** a `SMU2000_COPY_ROMS` post-build step
  copies the repo `roms/` tree next to each produced binary so `find_roms()` hits `<dllDir>\roms`.
  CI (`ci-*` presets) sets `SMU2000_COPY_ROMS=OFF` (no ROM by design; ship a `ROM-REQUIRED.txt`).

---

## Graphics-free plugin recipe (P2/P3/P4 — critical, satisfies hard rule #6)

Verified against the pinned iPlug2 CMake (`Scripts/cmake/`):
- `iplug_configure_target(<t> VST2 <proj>)` (in `FindiPlug2.cmake`) takes ONLY 3 args (no UI switch),
  includes `VST2.cmake`, and links `iPlug2::VST2` whose INTERFACE **forces `IPLUG_EDITOR=1`**
  (`VST2.cmake:53`). The editor code in the IPlug core compiles only under `#if IPLUG_EDITOR` and
  pulls IGraphics → so this path canNOT be graphics-free.
- **`iPlug2::IPlug` (the DSP core) does NOT define `IPLUG_EDITOR` and pulls NO IGraphics**
  (`IPlug.cmake:71-88`; it links only system libs Shlwapi/comctl32/wininet). Core sources
  (`IPlugAPIBase.cpp` etc.) are editor-free when `IPLUG_EDITOR` is unset.
- ⇒ **GUI OFF (default) recipe:** build the plugin MODULE WITHOUT `iplug_configure_target`/
  `iPlug2::VST2`. Instead: `add_library(... MODULE)`; add the plugin sources **plus**
  `${IPLUG_DIR}/VST2/IPlugVST2.cpp` (CLAP: the CLAP glue is via `IPlug_include_in_plug_src.h`
  with `CLAP_API` + CLAP SDK/HELPERS includes); include `${IPLUG_DIR}/VST2` + the VST2 SDK stub
  dir `${IPLUG2_DIR}/Dependencies/IPlug/VST2_SDK` (aeffect headers, populated in P0); define
  `VST2_API VST_FORCE_DEPRECATED IPLUG_DSP=1` and **NOT** `IPLUG_EDITOR`; link
  `smu2000_engine iPlug2::IPlug` (**never `${IGRAPHICS_LIB}`**); set `/MT` (root already does);
  the plugin `.h` includes only `IPlug_include_in_plug_hdr.h` (NOT `IControls.h`) and guards every
  editor member under `#if IPLUG_EDITOR`. `PLUG_HAS_UI 0`. Verify with `dumpbin /DEPENDENTS` → no
  opengl32/nanovg/skia/IGraphics imports.
- **GUI ON (P7 — native, still graphics-free):** the SAME manual recipe as GUI OFF (add
  `IPlugVST2.cpp`/`IPlugCLAP.cpp` by hand, `iPlug2::IPlug`, keep `NO_IGRAPHICS`) PLUS
  `-DSMU2000_ENABLE_GUI` (→ `PLUG_HAS_UI 1`) + link `smu2000_gui` + `gdi32/comdlg32/user32`. NO
  `iplug_configure_target`, NO `${IGRAPHICS_LIB}`, NO `IPLUG_EDITOR`. `NO_IGRAPHICS` keeps
  `EDITOR_DELEGATE_CLASS = iplug::IEditorDelegate`, whose native `OpenWindow/CloseWindow` the plugin
  overrides to host `ui::panel` in a child `HWND`. `/DEPENDENTS` gains only GDI32+COMDLG32 (the
  VST3 view's GDI + SmartMedia file dialog), never OpenGL/NanoVG/Skia. Toggle = which two recipes
  the target CMakeLists takes (both graphics-free; ON just adds the editor lib + the UI define).

---

## GUI toggle
- `SMU2000_ENABLE_GUI` cache var (alias `-DENABLE_GUI=1`), **default OFF**. **Both states are
  graphics-free** (no IGraphics/NanoVG/OpenGL/Skia) — the GUI is the native Win32/GDI panel.
  - OFF → `PLUG_HAS_UI 0`; no editor sources; pure instrument plugin (VST2/CLAP load, MIDI-in,
    audio-out, state chunks). `/DEPENDENTS` = KERNEL32/USER32/api-ms.
  - ON → `PLUG_HAS_UI 1`; compile+link `smu2000_gui` = `SMU2000_VST2/ui/SMU2000Editor.cpp`
    (port of `src/vst3/view.cpp`) + `src/ui/{panel,editor,effects,layout,svg}.cpp` + `src/xg/model.cpp`
    (Makefile `VST3_SRCS` GUI set). `NO_IGRAPHICS` stays ON. Plugin overrides
    `IEditorDelegate::OpenWindow/CloseWindow` (VST2 `effEditOpen/Close`, CLAP `guiSetParent/Destroy`).
  Toggle flows: root option → `SMU2000_VST2/CMakeLists.txt` conditional `add_subdirectory(ui)` +
  GUI-ON MODULE recipe → per-target `-DSMU2000_ENABLE_GUI` → `config.h` derives `PLUG_HAS_UI`.


---

## Target matrix (after P3)

| Target | Kind | Output | Notes |
|---|---|---|---|
| `smu2000_engine` | STATIC lib | `…/.lib` | OBJS + `mu2000.cpp` + `engine.cpp`; shared by all API targets |
| `smu2000_vst2` | MODULE | `SMU2000_VST2.dll` (`vst2/<arch>/<Config>/`) | VST2 entry `VSTPluginMain`; Win32 + x64 |
| `smu2000_clap` | MODULE | `SMU2000_VST2.clap` (`clap/<arch>/<Config>/`) | CLAP entry via `CLAP_EXPORT`; Win32 + x64 |
| (`smu2000_app`) | — | (optional later) | Standalone not required; engine already runs via existing `live/gui` exes |

Output layout: `build-cmake/<api>/<arch>/<Config>/` (identical to sw10; `<arch>` = `Win32`|`x64`,
MinGW appends `-mingw`/`-mingw-clang`). Static `/MT` CRT everywhere.

---

## Phases

### Phase 0 — Scaffold (`P0-scaffold`, Wave 1) — subagent
- Add `iPlug2` submodule pinned `d54f69050`; create `.gitmodules`. Do NOT add SDKs/prebuilt libs as commits.
- Create `SMU2000_VST2/` (config.h, empty `SMU2000_VST2.{h,cpp}` stub, `ui/` placeholder), `engine/`
  (thin CMake only — sources stay in `src/`), `cmake/` (`iplug2_paths.cmake`, `mingw_compat.cmake` inert),
  root `CMakeLists.txt`, `CMakePresets.json`.
- `.gitignore`: add `/build-cmake/`, `aeffect.h`, `aeffectx.h` (keep `roms/` intact).
- Port `../sw10_plug/cmake/iplug2_paths.cmake` → `SMU2000_*` knobs: keep VST2 SDK resolution
  (`SMU2000_VST2_SDK_DIR` → env `VST2_SDK_DIR` → stub `aeffect.h/aeffectx.h` copy; this box:
  `D:/opt/vst/vstsdk2.4`), CLAP (`SMU2000_CLAP_DIR` default `iPlug2/Dependencies/IPlug`),
  optional VST3 block default OFF. **Drop** the ROMSXGM `SW10_ROM_PATH` block; replace with
  `SMU2000_ROMS_DIR` (default `${CMAKE_SOURCE_DIR}/roms`) used only for staging (P2), never FATAL if absent.
- Presets: `vs-win32`, `vs-x64`, `ci-win32`, `ci-win64` (VS 2026 gen, `-A Win32|x64`),
  `smu2000_clap`/`smu2000_vst2` toggles via options; VST2 SDK gated on header presence.
- **Accept:** `cmake --preset vs-win32` and `vs-x64` **configure** clean (no compile yet).

### Phase 1 — Engine static lib (`P1-engine-lib`, Wave 2) — subagent
- `engine/CMakeLists.txt`: `add_library(smu2000_engine STATIC)` with the source list from §Findings,
  include dirs `src` + `src/compat`. `/std:c++20 /utf-8 /MT`, defs
  `_USE_MATH_DEFINES;NOMINMAX;_CRT_SECURE_NO_WARNINGS;WIN32`.
- Confirm `x64asm.h` is NOT compiled (guarded out on win32) and `sh2_jit`/`swp30_jit` compile to
  interpreter-only stubs. *(Historic as of 2026-09-16: x86-32 JIT port landed — win32 now
  compiles `x64asm.h` in 32-bit mode with real JITs; see `CPU32_LEDGER.md` Phases 1–8.)*
- **Accept:** `smu2000_engine.lib` builds on Win32 **and** x64 MSVC with sound code intact
  (link a throwaway that calls `engine::start()`+`fill()` if needed to prove symbols).

### Phase 2 — VST2 wrapper (`P2-vst2-wrapper`, Wave 3a) — subagent
- `SMU2000_VST2/config.h`: `PLUG_TYPE 1`, `PLUG_DOES_MIDI_IN/OUT 1`, `PLUG_DOES_STATE_CHUNKS 1`,
  `PLUG_HAS_UI` from GUI toggle (OFF default), `PLUG_CHANNEL_IO "0-2"`, `PLUG_UNIQUE_ID 'SMU2'`,
  `PLUG_LATENCY 0`, CLAP/VST3 metadata macros (shared with P3).
- `SMU2000_VST2.{h,cpp}`: `class SMU2000_VST2 final : public Plugin` holding
  `std::unique_ptr<smu2000::engine> m_engine`.
  - ctor: `m_engine->start(); m_engine->set_output_rate(GetSampleRate());`
  - `OnReset()`: `m_engine->set_output_rate(GetSampleRate())`.
  - `ProcessBlock(in,out,n)`: cast `sample**`→float, `m_engine->fill(fL,fR,n, fInL?, fInA?)`.
  - `ProcessMidiMsg` → `m_engine->midi(msg.mMsg, msg.mNumBytes, 0)`; `ProcessSysEx` → `m_engine->midi(msg.mData,msg.mSize,0)`.
  - `SerializeState/UnserializeState` → engine blob (chunk). Optional MIDI-out pump via `SendMidiMsg`.
- `SMU2000_VST2/CMakeLists.txt`: mirror sw10 `sw10_vst2` — `add_library(smu2000_vst2 MODULE)` +
  `iplug_configure_target(smu2000_vst2 VST2 SMU2000_VST2)`; link `smu2000_engine` (+ `${IGRAPHICS_LIB}` only if GUI ON);
  `PREFIX ""`; out dirs `build-cmake/vst2/<arch>/<Config>/SMU2000_VST2.dll`; **post-build ROM staging**
  (`SMU2000_COPY_ROMS`) copying `roms/` → `$<TARGET_FILE_DIR>/roms`.
- **Accept:** Win32 + x64 `SMU2000_VST2.dll` build; exports `VSTPluginMain`; with ROMs staged,
  audio boots (silence→ready) and MIDI sounds in a 32-bit host; project save/recall round-trips.
- **DONE (Wave 3a finish).** Both archs build clean with the existing graphics-free recipe — zero
  code/CMake fixes were needed; `NO_IGRAPHICS` alone keeps the editor out (no `IPLUG_EDITOR=0`
  required at this pin). `/DEPENDENTS` = KERNEL32+USER32+api-ms synch only (no GL/Skia/png/CRT DLLs).
  `VSTPluginMain`+`main` exported both archs; roms staged both archs. Load probes (native C++ +
  P/Invoke, x64 pwsh / SysWOW64 PS5.1): magic=0x56737450, uniqueID=0x534D5532, effOpen/effClose
  return 0, effGetChunk(23)/effSetChunk(24) round-trip 6,096,753-byte chunk, processReplacing OK.
  GOTCHA for probes: this SDK's aeffect.h has NO `index` field — effOpen=0/effClose=1,
  effGetChunk=23/effSetChunk=24 (calling the 34/35 slots = effGetVendorString writes 64 bytes
  through your `void**` — instant AV). Engine boots async (~2-4 s with staged ROMs); poll
   `state()==ready` before chunk/audio calls. Sound-out unverifiable: no DAW/host installed.
- **P2-FIX (2026-09-16) — sample-accurate MIDI (was block-boundary quantised).** The original
  `ProcessMidiMsg` → `m_engine->midi(bytes, n, 0)` applied *every* event in a block at its start
  (the `0` is the engine **port**, not a time). `m_engine->midi()` has no time arg — it just
  clock-queues bytes onto the emulated 31250 bps serial line — so note on/off onset was jittered
  by up to one host block (≈23 ms @1024/44.1k), worse at large buffers while audio stayed steady.
  Hosts *do* pass the intra-block offset (`IMidiMsg::mOffset`/`ISysEx::mOffset`, set from VST2
  `deltaFrames` / CLAP `time`); we were dropping it. Fix: park stamped events in an audio-thread
  `m_midi_q` in `ProcessMidiMsg`/`ProcessSysEx`, `stable_sort` by offset in `ProcessBlock`, then
  interleave — `fill()` up to each offset (writing `outputs+produced`), inject that event's bytes
  via `midi()`, continue. Each `fill()` releases `m_machine` on return so `midi()` between calls
  is lock-safe; the serial model then spaces bytes to the right sample. Empty-block fast path kept.
  Resampler path (`m_pos`/`m_written`/ring are member state) and `driver` pumps (`apply_buttons`
  state-based, `pump_midi`/`wheel`/`out` drain-while, `publish`/`advance_clock` sum to nFrames) all
  verified safe across split `fill()` calls. No latency change: `engine::latency_samples()` is
  intentionally 0 (resampler synthesises lookahead). Rebuilt VST2+CLAP Win32+x64 clean (MSVC).

### Phase 3 — CLAP wrapper (`P3-clap-wrapper`, Wave 3b) — subagent (parallel with P2)
- Reuse the **same** `SMU2000_VST2.{h,cpp}` plugin class + `smu2000_engine`. Add CLAP target in
  `SMU2000_VST2/CMakeLists.txt` mirroring sw10 `sw10_clap`: `add_library(smu2000_clap MODULE)` +
  `iplug_configure_target(smu2000_clap CLAP SMU2000_VST2)`, suffix `.clap`, link `smu2000_engine`
  (+iPlug2::CLAP from `SMU2000_CLAP_DIR`); ROM staging like P2. CLAP metadata already in config.h.
- If CLAP SDK/helpers absent, print the sw10-style fix message (download via Git-Bash) and gate the target.
- **Accept:** `SMU2000_VST2.clap` builds Win32 + x64; CLAP entry point present; same sound as VST2.
- **DONE (Wave 3b finish).** CLAP_SDK/CLAP_HELPERS staged from sw10 (untracked, same policy as VST2 SDK).
  Graphics-free CLAP target mirrors P2 recipe (no `iplug_configure_target`/`iPlug2::CLAP`; manual
  `IPlugCLAP.cpp` + SDK/HELPERS includes; link `smu2000_engine iPlug2::IPlug`; `CLAP_API IPLUG_DSP=1
  NO_IGRAPHICS SAMPLE_TYPE_FLOAT`). GOTCHA fixed: ctor mem-init must be `: iplug::Plugin(info, ...)`
  (qualified — CLAP's base `clap::helpers::Plugin` injects the name `Plugin` into class scope, C2614;
  upstream Examples use the same spelling). `/DEPENDENTS` both archs = KERNEL32+USER32+api-ms-synch
  only. `clap_entry` exported both archs. Load probes (native, both archs): init(path)=true,
  get_factory("clap.plugin-factory") non-null, deinit clean. GOTCHAS: this pin's `clap_init` does
  `gPluginPath = pluginPath` unguarded — null path AVs (pass the DSO path like real hosts);
  `clap_version_t` is 3×u32 (+pad on x64) so entry pointers sit at +16/+24/+32 (x64), +12/+16/+20 (x86).
  Sound-out unverifiable (no CLAP host installed).

### Phase 4 — GUI toggle (`P4-gui-toggle`, Wave 4a) — subagent
- Wire `SMU2000_ENABLE_GUI` (alias `ENABLE_GUI`) end-to-end: root option → configure-time define
  → `config.h` `PLUG_HAS_UI` → conditional sources/libs in P2/P3 targets.
- Default OFF must produce a **graphics-free** build (verify no NanoVG/GL/OpenGL/Skia linked).
- OFF→ON with GUI sources still empty should configure/link with IGraphics wired (placeholder editor),
  proving the toggle for Phase 7.
- **Accept:** `-DSMU2000_ENABLE_GUI=OFF` and `=ON` both configure+build the VST2 target; OFF binary
  has no graphics imports.
- **DONE (Wave 4a finish).** OFF = existing recipe untouched (re-verified: both archs VST2+CLAP,
  `/DEPENDENTS` = KERNEL32+USER32+api-ms-synch only). ON = sw10-style `iplug_configure_target`
  + `${IGRAPHICS_LIB}`; VST2 x64/Win32 + CLAP x64 build; `/DEPENDENTS` now includes OPENGL32
  (NanoVG/GL2 wired) + ole32/GDI32/COMCTL32/WININET (editor libs); `VSTPluginMain` exported.
  `SMU2000_ENABLE_GUI` is a PER-TARGET define (root global `add_compile_definitions` removed;
  engine target never sees it). GOTCHA (cost hours — document!): at this pin `IGraphics` lives in
  **nested** `iplug::igraphics`; a global `using namespace igraphics;` BEFORE
  `using namespace iplug;` fails C2871. sw10's header order (iplug first, igraphics second) is
  mandatory — `SMU2000_VST2.h` now follows it inside `#if IPLUG_EDITOR`. Also `ITextControl`
  ctor is `(rect, str, IText, ...)` here (str before text). Editor = placeholder
  (panel bg + label "SMU2000 — GUI (editor pending Phase 7)"); no fonts/resources required.

### Phase 5 — MinGW/Clang compatibility (`P5-mingw-clang`, Wave 4b) — subagent
- Port `../sw10_plug/cmake/mingw_compat.cmake` + `mingw_portability_prelude.h` (rename `SW10_*`→`SMU2000_*`).
- Add presets `mingw-win32`, `mingw-x64`, `mingw-clang-x64`, `mingw-ci-*` (Ninja + gcc/clang, MSYS2 MINGW32/MINGW64 shell).
- On MinGW **win64**, `__x86_64__` IS defined → JIT path turns ON; verify `x64asm.h` assembles under
  GCC **and** Clang. On MinGW **win32** JIT stays off (interpreter), like MSVC-win32.
  *(2026-09-16: JIT support is now compiled-in for MinGW-win32 too (dual-mode JITs), but the
  i686 toolchain is broken on this box — use `vs-win32`; see `CPU32_LEDGER.md`.)*
- Import-lib name remaps + static runtime handled in `mingw_compat.cmake` (submodule untouched).
- **Accept:** VST2 + CLAP build under **MSYS2 GCC and Clang** (x64, and win32 where the toolchain exists),
  self-contained (system DLL imports only), entry exports present.

### Phase 6 — CI GitHub Actions (`P6-ci-github`, Wave 5) — subagent
- `.github/workflows/build-native.yml` modeled on sw10: `windows-latest`, matrix
  `win32`/`x64` (`ci-win32`/`ci-win64` presets), MSVC. VST2 SDK is **proprietary** → gate OFF unless a
  secret/cached SDK is staged; always build engine + (VST3 optional) + CLAP (headers via
  `iPlug2/Dependencies/download-clap-sdks.sh`). `SMU2000_COPY_ROMS=OFF`, ship `ROM-REQUIRED.txt`.
- Job also runs `cmake --preset vs-win32 && cmake --build --preset vs-win32-release` to keep the
  first-party path honest. Add a MinGW job (MSYS2 setup) once P5 is green (optional).
- **Never** cache/commit `roms/`, `aeffect*.h`, VST2 SDK. Keep ignore patterns anchored so
  `.github/workflows/*` stay tracked.
- **Accept:** CI workflow YAML valid (`actionlint`-clean), matrix configures; green on the pieces CI can build.

### Phase 7 — GUI editor (`P7-gui-editor`) — **DONE 2026-09-15** — native GDI, NOT IGraphics
- **Decision (supersedes the P4 placeholder / sw10 NanoVG plan):** the existing VST3 GUI is *not*
  IGraphics — `src/vst3/view.cpp` draws the front panel with raw **GDI** (`src/ui/draw.h`,
  `CreateFontA`, `BitBlt`) on a child `HWND`, driving `ui::panel` from `engine::panel()`. So we
  **reuse it verbatim** instead of reimplementing in NanoVG. Result: GUI-ON is *still* graphics-free
  (hard rule #6 holds for **both** states; no NanoVG/OpenGL/Skia/imgui/D3D anywhere).
- **Mechanism.** iPlug2's `IEditorDelegate` (selected via `NO_IGRAPHICS` in
  `IPlugDelegate_select.h`) ships an empty native path `OpenWindow(void* parent)`/`CloseWindow()`;
  VST2 routes `effEditOpen`/`effEditClose` (IPlugVST2.cpp:466/483) and CLAP routes
  `guiSetParent`/`guiDestroy`/`guiShow` (IPlugCLAP.cpp:951/882/893) straight to it. So the plugin
  class just overrides those two: `OpenWindow` spawns the child `HWND`, `CloseWindow` tears it down.
  Host is the real parent; the child's own `WndProc` owns all input + a 33 ms `WM_TIMER` (VST3 parity,
  host-agnostic — no `effEditIdle` needed).
- **New files.** `SMU2000_VST2/ui/SMU2000Editor.{h,cpp}` — `smu2000::editor`, a near-verbatim port
  of `src/vst3/view.cpp` (paint, press/drag/release/wheel, `key_to_button`, SmartMedia card menu,
  `WM_GETMINMAXINFO` 2.5:1 track). `SMU2000_VST2/ui/CMakeLists.txt` → STATIC `smu2000_gui`: the exact
  Makefile `VST3_SRCS` GUI set (`src/ui/{panel,editor,effects,layout,svg}.cpp` + `src/xg/model.cpp`)
  + `SMU2000Editor.cpp`; carries its own `src`+`src/compat` PUBLIC includes + `NOMINMAX`/`NO_IGRAPHICS`.
- **Plugin.** `SMU2000_VST2.h`: `std::unique_ptr<smu2000::editor> m_editor` + `OpenWindow`/
  `CloseWindow`/`OnParentWindowResize` overrides, all `#if PLUG_HAS_UI`. `.cpp`: builds the editor in
  ctor (replaces the placeholder `mMakeGraphicsFunc`/`mLayoutFunc`). `config.h`: `PLUG_WIDTH/HEIGHT`
  = 1250×500 (logical 1000×400 = 2.5:1).
- **CMake.** GUI-ON VST2 **and** CLAP now use the *same* manual graphics-free recipe as GUI-OFF
  (hand-added `IPlugVST2.cpp`/`IPlugCLAP.cpp`, `iPlug2::IPlug`, `NO_IGRAPHICS`) + `SMU2000_ENABLE_GUI`
  (per-target → `PLUG_HAS_UI 1`) + `smu2000_gui` + `gdi32/comdlg32/user32` (MSVC). NO
  `iplug_configure_target`/`${IGRAPHICS_LIB}` on either path — dropped the sw10 NanoVG wiring entirely.
- **Gotchas learned.** (1) `svg.cpp` includes `<windows.h>` before `<algorithm>` → MSVC `min/max`
  macros corrupt `std::min/max` (C2589); fixed by `NOMINMAX` PUBLIC on `smu2000_gui` (engine had it,
  the standalone lib must too). (2) `SMU2000Editor.cpp` uses `PLUG_WIDTH/HEIGHT` → `#include
  "../config.h"` (only macro defines, no iPlug headers). (3) GUI-ON must keep `NO_IGRAPHICS` so
  `EDITOR_DELEGATE_CLASS` stays plain `IEditorDelegate` (not `IGEditorDelegate`) and `OpenWindow`
  resolves to *our* override, not the graphics `final`. (4) `static_assert` in the plugin `.cpp`
  guards `SMU2000_ENABLE_GUI ⇒ PLUG_HAS_UI==1` (catches config desync at compile time).
- **Accept — DONE, host-probed (no DAW on box).** Native C++ hosts `tools/vst2_host_probe.cpp` +
  `tools/clap_host_probe.cpp` (this SDK's real enums/ABI):
  - x64 **VST2** GUI-ON: `effEditGetRect`→ERect 1250×500, `effEditOpen`→ child panel HWND
    (`SMU2000IPlugView`) visible 1250×500, 370 idle/timer pumps (paint+tick+engine boot) no crash,
    `effEditClose` destroys it. **PASS.**
  - x64 **CLAP** GUI-ON: `clap.gui` `is_api_supported(win32)`, `create`, `set_parent`, `show` → child
    panel visible 1250×500, 366 pumps alive, hide/destroy clean. **PASS.**
  - **Win32** VST2 GUI-ON (32-bit host): child panel attaches/paints/detaches 1250×500. **PASS.**
  - GUI-OFF default (x64): `/DEPENDENTS` = KERNEL32/USER32/api-ms only (no editor on either API). **PASS.**
  - GUI-ON `/DEPENDENTS` = GDI32/COMDLG32/USER32/KERNEL32/api-ms — **no OPENGL32/NanoVG/Skia**.
  - Opcode ABI note: this SDK keeps deprecated entries under `VST_FORCE_DEPRECATED`, so
    `effEditGetRect=15, effEditOpen=13, effEditClose=14, effEditIdle=18` (NOT the compact 11/12/13/19).
    AEffect on x64: `dispatcher@8`, `uniqueID@112` (`'SMU2'`). MinGW x64 GUI-ON not re-run here
    (needs MSYS2 shell); the `smu2000_gui` lib reuses the engine's already-MinGW-proven flags.


---

## Status

- [x] Recon complete; decisions locked; this ledger + `AGENTS.md` written.
- [x] P0 scaffold — **DONE 2026-09-15.** iPlug2 submodule @ `d54f69050` + `.gitmodules`;
      `cmake/{iplug2_paths,mingw_compat}.cmake`, root `CMakeLists.txt` + `CMakePresets.json`
      (`vs-win32`/`vs-x64`/`ci-*`), `engine/` INTERFACE placeholder, `SMU2000_VST2/` scaffold
      (config.h, stub sources, `CMakeLists.txt` with `smu2000_set_out_dirs`/`smu2000_stage_roms`
      live, `SMU2000_HAS_PLUGIN_SOURCES=FALSE`). `cmake --preset vs-win32`/`vs-x64` configure
      CLEAN (verified). Divergences: CLAP + VST3 SDK non-fatal/optional; `SMU2000_ROMS_DIR`
      non-fatal; VST2 env fallback probes `D:/opt/vst/vstsdk2.4`; upstream IGraphics.cmake
      downloads WebView2 at configure (needs network). Nothing committed.
- [x] P1 engine lib — **DONE 2026-09-15.** `engine/CMakeLists.txt` = STATIC `smu2000_engine`
      (exact 20-TU list: Makefile OBJS + `mu2000.cpp` + `vst3/engine.cpp`; PUBLIC includes
      `src`+`src/compat`; `/std:c++20 /utf-8 /bigobj`; defs `_USE_MATH_DEFINES NOMINMAX
      _CRT_SECURE_NO/STDC NO_WARNINGS WIN32`; CRT left to root `/MT`). Builds **Win32 + x64**
      (verified `.lib`s; dumpbin shows engine API symbols). JIT interpreter-only confirmed on
      both (`x64asm.h` never compiled; `sh2_jit`+`swp30_jit` fallbacks complete). Only src edit:
      `src/mame/cpu/sh.cpp` +#include <bit>. `xg/model.cpp` NOT needed. No winmm/avrt/ole32 deps.
      Namespace `smu2000::vst3::engine`.
- [x] P2 vst2 wrapper — **DONE.** Win32+x64 green, graphics-free (3 system DLLs), `VSTPluginMain`,
      load-probed: magic `VstP`/uniqueID `'SMU2'`/effOpen/effClose + effGetChunk/SetChunk ~6 MB round-trip.
- [x] P3 clap wrapper — **DONE.** CLAP SDK+HELPERS staged (copied from sibling sw10, untracked);
      Win32+x64 `.clap` green, graphics-free, `clap_entry`+plugin-factory probed; VST2 unregressed.
      One src tweak: ctor `: iplug::Plugin(...)` (CLAP helper base hides the `Plugin` alias).
- [x] P4 gui toggle — **DONE.** `SMU2000_ENABLE_GUI` (alias `ENABLE_GUI`) end-to-end. GUI-OFF default:
      both archs stay graphics-free (regression checked: KERNEL32/USER32/api-ms only). GUI-ON
      (`-DSMU2000_ENABLE_GUI=ON`, per-target define → `config.h PLUG_HAS_UI 1` + `IPLUG_EDITOR`):
      sw10-style `iplug_configure_target` + `${IGRAPHICS_LIB}` (NanoVG/GL2) + placeholder editor
       (`#if IPLUG_EDITOR` block; `using namespace igraphics` AFTER `iplug`). VST2 ON imports OPENGL32
       (toggle proven). Tree left configured GUI-OFF.
       > **SUPERSEDED by P7 (2026-09-15):** the placeholder `mMakeGraphicsFunc`/`mLayoutFunc` and the
       > NanoVG/`IPLUG_EDITOR` GUI-ON wiring were removed; GUI-ON is now the native GDI editor
       > (`NO_IGRAPHICS` + `smu2000_gui`) and no longer imports OPENGL32. The GUI-OFF half is unchanged.
- [x] P5 mingw/clang - **DONE 2026-09-15 (win32 leg env-BLOCKED).** GCC 16.2 + Clang 22 (MSYS2 mingw64) x64: VST2+CLAP all green, presets `mingw-{x64,clang-x64,win32,ci-x64}`. Self-contained (imports only ADVAPI32/comdlg32/GDI32/KERNEL32/msvcrt/SHELL32/SHLWAPI/USER32 - libstdc++/libgcc/winpthread static, no opengl); exports `VSTPluginMain`+`main` / `clap_entry`; JIT ON on x64 (`meg_jit`/x64asm symbols in both jit objs; win32 stays interpreter-only). Shims: `cmake/mingw_compat.cmake` (x86 FATAL removed - arch-clean engine; opengl32/gdi32 now GUI-ON-only), new `cmake/mingw_portability_prelude.h`, CLAP `CLAP_EXPORT extern` guard in `SMU2000_VST2.cpp`, single-config Release out-dir fix in the helper. Two Clang-only src fixes (mamecompat `timer_alloc` defined after `running_machine` completes; swp30_jit.cpp:515 explicit `s32()` narrowing) - MSVC/GCC behavior unchanged. MSVC vs-x64 re-verified green + graphics-free. win32: this box's i686 toolchain is BROKEN (cc1.exe dies STATUS_ENTRYPOINT_NOT_FOUND unless run from /mingw32/bin; partially-upgraded 2023-2026 package set); repair = `pacman -Syu mingw-w64-i686-gcc` (network - forbidden here); preset+harness proven working up to the compiler self-test. No submodule edits, no commit, roms untouched.
- [x] P6 ci github actions — **DONE.** `.github/workflows/build-native.yml` (untracked, not committed):
      `build-windows` (matrix x64/Win32 → `ci-win64`/`ci-win32`, MSVC, VST2 gated **OFF** — proprietary;
      CLAP+engine only; no app/vst3/Skia; ROM-REQUIRED.txt stub; anchored clap-only uploads) +
      independent `build-mingw` (MSYS2 MINGW64 gcc/ninja/cmake → `mingw-ci-x64`). YAML valid
      (PyYAML `safe_load`; actionlint absent). CI path emulated locally: `ci-win64 -DSMU2000_BUILD_VST2=OFF`
      configures (vst2 OFF / clap TRUE) + builds `.clap`. No `roms/`/SDK in any path.
- [x] P7 gui editor — **DONE 2026-09-15 (native GDI, superseded the IGraphics/NanoVG plan).** The
      VST3 GUI (`src/vst3/view.cpp` + `src/ui/*`) is raw Win32/GDI, so it's reused *verbatim* rather
      than ported to NanoVG. `SMU2000_VST2/ui/SMU2000Editor.{h,cpp}` (port of `view.cpp`) + STATIC
      `smu2000_gui` (Makefile `VST3_SRCS` GUI set) host `ui::panel` in a child `HWND` via
      `IEditorDelegate::OpenWindow/CloseWindow` — VST2 `effEditOpen/Close`, CLAP `guiSetParent/Destroy`.
      GUI-ON stays **graphics-free** (same manual recipe as GUI-OFF + `SMU2000_ENABLE_GUI` +
      `smu2000_gui` + gdi32/comdlg32/user32; **no** `iplug_configure_target`/`${IGRAPHICS_LIB}`).
      Host-probed (native C++ hosts in `tools/`): x64 VST2 + CLAP + Win32 VST2 all attach/paint/
      detach the 1250×500 panel through the real message loop; GUI-OFF default stays editor-free/
      graphics-free. See §Phase 7 for the full write-up + ABI/opcode gotchas. Tree left configured
       GUI-OFF (x64 rebuilt last). Nothing committed.
- [x] CPU32 x86-32 JIT port — **DONE 2026-09-16** (full record: `CPU32_LEDGER.md` Phases 1–8).
      Dual-mode `x64asm.h` + SH-2 + MEG JITs ported to x86-32; guards widened so **MSVC-x64 also
      gets the JIT**; Win32 JIT default-ON, bit-exact (35/35 render matrix + soak + traces). Host
      win32 CPU win 2.06x (native 4.74x); win32-JIT 1.60xRT vs interpreter 0.78xRT. Tooling:
      `tools/msvc32_build.ps1` (MSVC amd64_x86 harness — mingw32 cc1plus broken on this box, do
      not repair), `tools/x64asm32_test.cpp` (encoding gate). Supersedes every "Win32 =
      interpreter-only" note above (Findings §32-bit JIT, P1, P5, P0-P7 status entries).

### Wave schedule — COMPLETE (P0–P7 green)

