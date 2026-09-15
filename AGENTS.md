# AGENTS.md — S-MU2000 (iPlug2 port)

Yamaha MU2000 software synth (SH7043 + SWP30 firmware emulator). Being adapted into an
**iPlug2** plugin (VST2 + CLAP, Win32 + x64). **Read `VST2_LEDGER.md` first** for the full
phase plan, findings, decisions and risks, and keep its phase checklists current. The build
system mirrors `../sw10_plug` — study `../sw10_plug/AGENTS.md` + `REFACTOR_PLAN.md` before
touching the harness.

> **START HERE.** This folder is **already** a git checkout of
> `https://github.com/tarboh/S-MU2000.git` (branch `main`) — do **not** re-clone/re-init. The
> workspace root is `D:\Projects\vst`; the reference build sits at sibling `../sw10_plug`.
> **Nothing is scaffolded yet** (`iPlug2/`, `.gitmodules`, `engine/`, `SMU2000_VST2/`, `cmake/`,
> root `CMakeLists.txt`/`CMakePresets.json` do not exist). Begin by dispatching the **P0**
> subagent (see ledger wave schedule). The `cmake --preset …` commands below only work after P0.

## Phase execution rule

Each phase in `VST2_LEDGER.md` runs as its **own subagent, max 2 concurrent** (wave schedule
in the ledger). Do not do phase work inline; dispatch the named subagent (via the Task tool,
`explore` for research / `general` for build work) and honour its acceptance gate before
starting a dependent wave.

## Repo map

- `src/` — emulator. Engine core = Makefile `OBJS` + `mu2000.cpp` + `vst3/engine.cpp` (the
  reusable, GUI-free DSP wrapper `smu2000::engine`). `src/ui/*` is GUI-only — the header-only
  `bridge/driver/resampler/snapshot` (+ transitive `xg/ram.h`/`xg/model.h`) are reused; the rest
  is deferred to the GUI phase. See ledger §Findings for the exact compile/exclude list.
- `engine/` — new thin CMake target `smu2000_engine` (sources still live under `src/`).
- `SMU2000_VST2/` — iPlug2 plugin (`config.h`, `SMU2000_VST2.{h,cpp}`, `CMakeLists.txt`). VST2 +
  CLAP targets. `ui/` = the **native GDI editor** (P7): `SMU2000Editor.{h,cpp}` (port of
  `src/vst3/view.cpp`) + its own `CMakeLists.txt` (`smu2000_gui` static lib). Built only when
  `-DSMU2000_ENABLE_GUI=ON`; reuses `ui::panel` via `IEditorDelegate::OpenWindow` — **no** IGraphics.
- `iPlug2/` — **git submodule** pinned `d54f69050` (same as sw10). Never edit its tree; put
  all build shims in `cmake/` (mirrors `../sw10_plug/cmake/mingw_compat.cmake`).
- `cmake/` (`iplug2_paths.cmake`, `mingw_compat.cmake`), root `CMakeLists.txt`,
  `CMakePresets.json` — the CMake harness (VS 2026 + MSYS2 MinGW/Clang presets).
- `Makefile` — legacy g++/MSYS2 native-tools build (`live/render/gui/verify/…`). **Stays as-is**;
  the CMake path is the plugin build. Do not repurpose it.
- `third_party/` — `vst3` (MIT `pluginterfaces`) and `imgui` (existing GUI); untouched.
- `VST2_LEDGER.md` — THE plan. `README.md` — user/emulator docs (Japanese).

## Hard rules

1. **`roms/` is NEVER committed and NEVER deleted.** It holds the user's own dumped MU2000
   ROMs (`mu2000_flash.bin`, `dump/`, `standin/`); Yamaha data cannot be distributed
   (`README.md`). Already git-ignored — keep it so. Runtime lookup is handled by
   `src/vst3/engine.cpp::find_roms()`, which probes `<dllDir>\roms` first.
2. **ROM staging at build time** (mirrors sw10 `SW10_COPY_ROM`): `SMU2000_COPY_ROMS` copies
   the repo `roms/` next to each built `.dll`/`.clap` so `find_roms()` finds them. CI presets
   set it `OFF` (ship `ROM-REQUIRED.txt` instead).
3. Never commit `build-cmake/`, `aeffect*.h`, VST2 SDK, secrets. Keep ignore patterns
   **anchored** (`/build-cmake/`) so `.github/workflows/*` stay tracked.
4. Never edit `iPlug2/`; submodule bumps are dedicated commits only.
5. Resolve SDKs via `SMU2000_*` cache vars / env, never hardcoded paths. This box: VST2 SDK at
   `D:/opt/vst/vstsdk2.4`. 32-bit MSVC needs the VS x86 toolset component installed.
6. GUI is OFF by default (`-DSMU2000_ENABLE_GUI=OFF`); a default build must link no
   IGraphics/NanoVG/OpenGL/Skia. GUI-ON stays **graphics-free too** — the editor is the native
   Win32/GDI panel (`ui::panel` via `IEditorDelegate::OpenWindow`), so it links only gdi32/comdlg32.

## Build commands (CMake plugin path — being stood up; keep synced with VST2_LEDGER.md)

```powershell
# configure + build VST2 (and CLAP) on MSVC
cmake --preset vs-win32  ; cmake --build --preset vs-win32-release
cmake --preset vs-x64    ; cmake --build --preset vs-x64-release
# GUI toggle (both states graphics-free; ON = native GDI panel editor, no NanoVG/GL/Skia)
cmake --preset vs-win32 -DSMU2000_ENABLE_GUI=ON      # alias -DENABLE_GUI=1
# CI-equivalent (SDK dirs from env VST2_SDK_DIR, no ROM): ci-win32 / ci-win64
```
Outputs: `build-cmake/<api>/<arch>/<Config>/SMU2000_VST2.{dll,clap}` (`<api>`=vst2|clap,
`<arch>`=Win32|x64; MinGW appends `-mingw[-clang]`). Static `/MT` CRT everywhere.

Legacy native exes (unchanged, MSYS2 g++): `make`, `make test`, `make vst3`.

## Toolchain notes

- **MSVC is this session.** Expect first-time MSVC compile of the g++-only engine (`/std:c++20`,
  `_USE_MATH_DEFINES` for `M_PI`, `/utf-8` for Japanese comments). Keep fixes in the engine
  CMake target only.
- **Win32 = interpreter-only.** Both JITs are `#if defined(_WIN32) && defined(__x86_64__)`
  (`swp30_jit.cpp`, `sh2_jit.cpp`); MSVC never defines `__x86_64__`, so win32 (and MSVC-x64) run
  the interpreter with `x64asm.h` never compiled. MinGW-x64 turns the JIT back on. Verify in P1.
- MinGW GCC **and** Clang compatibility is its own phase (see ledger P5); zero submodule edits.

## Status

See `VST2_LEDGER.md` phase checklists. Current: **P0–P7 green.** Plugin builds VST2+CLAP on
Win32+x64, graphics-free by default; `-DSMU2000_ENABLE_GUI=ON` adds the native GDI editor (the
reused VST3 panel) — still graphics-free. GUI-ON editor host-probed on x64 VST2+CLAP and Win32.
