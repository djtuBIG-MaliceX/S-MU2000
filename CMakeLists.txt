# SMU2000_VST2 — CMake build (Windows; mirrors ../sw10_plug). VST2 + CLAP, Win32 + x64.
# Quickstart: cmake --preset vs-win32 && cmake --build --preset vs-win32-release
# Consumes the iPlug2 submodule's own CMake support (iPlug2.cmake + Scripts/cmake/*);
# see VST2_LEDGER.md §Phase 0 for the harness design.
cmake_minimum_required(VERSION 3.25)
project(SMU2000_VST2 VERSION 0.0.1 LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()

if(MSVC)
  set(SMU2000_TOOLCHAIN "msvc")
  # Aggressive MSVC optimizer switches for EVERY config and every target defined
  # afterwards (engine, plugin MODULEs, native tools; iPlug2's compiled targets too
  # since this runs before find_package(iPlug2)): /Ob2 inline any suitable function,
  # /Ot favor fast code, /GT fiber-safe Win32 TLS, /GL whole-program opt — /LTCG on
  # the linker is what actually activates /GL at link time. Debug links trade
  # incremental linking for these (linker drops /INCREMENTAL, warning LNK4075).
  add_compile_options(/Ob2 /Ot /GT /GL)
  add_link_options(/LTCG)
elseif(MINGW)
  set(SMU2000_TOOLCHAIN "mingw")
  # Non-literal printf formats are bugs (translated strings must go through "%s").
  add_compile_options(-Wformat-security)
endif()

list(APPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/cmake")

# ---------------------------------------------------------------------------
# Options (no APP target, no VST3 target — VST2 + CLAP only, plan/ledger decisions)
# ---------------------------------------------------------------------------
option(SMU2000_BUILD_VST2 "Build the VST2 plugin (smu2000_vst2)"   ON)
option(SMU2000_BUILD_CLAP "Build the CLAP plugin (smu2000_clap)"   ON)
# GUI is ON by default now (2026-09-16); both states are graphics-free (native Win32/GDI
# panel, no IGraphics/NanoVG/GL/Skia). Disable with -DSMU2000_ENABLE_GUI=OFF (or -DENABLE_GUI=OFF).
option(SMU2000_ENABLE_GUI "Compile the native GDI editor (smu2000_gui; default ON; both ON/OFF are graphics-free: raw Win32/GDI panel, no IGraphics/NanoVG/GL/Skia). OFF removes the editor." ON)
option(SMU2000_COPY_ROMS  "Post-build: copy the repo roms/ tree next to each binary (runtime requirement)" ON)
# Native tool exes (Makefile render/live/gui). Desktop tools — imgui/D3D/WinMM allowed in
# native/ only; the plugin targets above/below stay graphics-free (hard rule #6 unaffected).
option(SMU2000_BUILD_TOOLS "Build native tool exes (gui, live, render)" ON)

# Alias: -DENABLE_GUI=ON maps to SMU2000_ENABLE_GUI (ledger GUI toggle contract).
if(DEFINED ENABLE_GUI)
  set(SMU2000_ENABLE_GUI "${ENABLE_GUI}" CACHE BOOL "GUI toggle (aliased from ENABLE_GUI)" FORCE)
endif()

# ---------------------------------------------------------------------------
# SDK / dependency resolution (SMU2000_* cache vars; VST3/CLAP optional)
# ---------------------------------------------------------------------------
include(cmake/iplug2_paths.cmake)

# ---------------------------------------------------------------------------
# iPlug2 consumption — include(iPlug2.cmake) + find_package(iPlug2) the way upstream
# Examples do, instead of add_subdirectory(iPlug2) (which would pull in Examples/Tests).
# Graphics: NanoVG/GL2 (upstream Windows default; unused either way — both GUI states are graphics-free).
# ---------------------------------------------------------------------------
set(IGRAPHICS_BACKEND "NANOVG" CACHE STRING "IGraphics drawing backend (NANOVG or SKIA)")
set_property(CACHE IGRAPHICS_BACKEND PROPERTY STRINGS "NANOVG" "SKIA")
set(IGRAPHICS_RENDERER "GL2" CACHE STRING "IGraphics renderer")

# Upstream Deploy.cmake defaults (out/-relative paths) do not match our output layout;
# SMU2000_VST2/CMakeLists.txt sets the build-cmake/<api>/<arch>/<Config>/ layout instead.
set(IPLUG_DEPLOY_PLUGINS OFF CACHE BOOL "iPlug2 auto-deploy (unused; SMU2000 out-dir helper replaces it)" FORCE)

include(${IPLUG2_DIR}/iPlug2.cmake)
find_package(iPlug2 REQUIRED)

# MinGW-w64 (MSYS2) compatibility: remap the MSVC import-library names the upstream
# iPlug2 INTERFACE targets hardcode, and add static-runtime / big-obj flags. No-op on MSVC
# (P5 owns the MinGW path; presets arrive with it).
include(mingw_compat)

# Static /MT CRT everywhere (upstream iPlug2.cmake sets the same default; re-set with
# CACHE FORCE so it also applies to any compiled SDK sources).
# MSVC-only knob — MinGW static runtime is handled in cmake/mingw_compat.cmake.
if(MSVC)
  set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>"
      CACHE STRING "static CRT for all targets" FORCE)
endif()

# GUI toggle wiring (P4). Deliberately NOT a global add_compile_definitions: the
# smu2000_engine target must never see SMU2000_ENABLE_GUI. SMU2000_VST2/CMakeLists.txt
# adds it PRIVATE on the GUI-ON plugin targets only, where config.h turns it into
# PLUG_HAS_UI 1. BOTH states (ON = default since 2026-09-16, and OFF) link no
# IGraphics/NanoVG/OpenGL/Skia (hard rule #6).

# ---------------------------------------------------------------------------
# Output layout: build-cmake/<api>/<arch>/<Config>/
# ---------------------------------------------------------------------------
if(CMAKE_SIZEOF_VOID_P EQUAL 8)
  set(SMU2000_ARCH "x64")
else()
  set(SMU2000_ARCH "Win32")
endif()
# Keep MinGW artifacts in their own <arch>-mingw dir so they never collide with the
# MSVC build in the shared build-cmake root (output dir segment only; arch logic in
# SMU2000_VST2/CMakeLists.txt keys off CMAKE_SIZEOF_VOID_P, not this variable).
# Clang gets its own -clang suffix too so the two MinGW toolchains do not clobber each other.
if(MINGW)
  set(SMU2000_ARCH "${SMU2000_ARCH}-mingw")
  if(CMAKE_CXX_COMPILER_ID MATCHES "Clang")
    string(APPEND SMU2000_ARCH "-clang")
  endif()

  # gui (Phase 4: SDL3 window + PC editor windows, doc/porting-linux-gui.md)
  if(CAIRO_FOUND AND SDL3_FOUND)
    add_executable(gui src/gui_linux.cpp
      src/compat/gdi_linux.cpp
      src/ui/panel.cpp src/ui/layout.cpp src/ui/svg.cpp
      src/ui/editor.cpp src/ui/effects.cpp src/ui/png.cpp
      src/ui/player.cpp src/xg/model.cpp
      src/ui/xg_ui.cpp src/ui/fx_help.cpp src/ui/fx_icons.cpp
      src/ui/sdl_popup.cpp
      src/ui/window_sdl.cpp
      src/ui/app_linux.cpp
      src/ui/pc_window_linux.cpp
      src/ui/pc_editor.cpp src/ui/overview.cpp src/ui/fx_editor.cpp
      src/ui/part_shapes.cpp src/ui/master_editor.cpp
      ${IMGUI_CORE}
      third_party/imgui/backends/imgui_impl_sdl3.cpp
      third_party/imgui/backends/imgui_impl_sdlrenderer3.cpp
      src/ui/audio_out_linux.cpp
      src/ui/audio_in_linux.cpp
      src/ui/midi_in_linux.cpp
      src/ui/midi_out_linux.cpp)
    target_include_directories(gui PRIVATE src src/compat
      third_party/imgui ${ALSA_INCLUDE_DIRS} ${CAIRO_INCLUDE_DIRS} ${SDL3_INCLUDE_DIRS})
    target_link_libraries(gui PRIVATE smu2000_core
      ${ALSA_LIBRARIES} ${CAIRO_LIBRARIES} ${SDL3_LIBRARIES} pthread dl)
  else()
    message(STATUS "Cairo/SDL3 not found: skipping gui (needs libcairo2-dev libsdl3-dev)")
  endif()
endif()
set(SMU2000_OUTPUT_ROOT "${CMAKE_SOURCE_DIR}/build-cmake" CACHE PATH
    "Artifact root; outputs land in <root>/<api>/${SMU2000_ARCH}/<Config>/")

add_subdirectory(engine)
add_subdirectory(SMU2000_VST2)
if(SMU2000_BUILD_TOOLS)
  add_subdirectory(native)   # render/live/gui exes (native/CMakeLists.txt; Makefile mirror)
endif()

message(STATUS "SMU2000_VST2 configure summary:")
message(STATUS "  toolchain=${SMU2000_TOOLCHAIN}  arch=${SMU2000_ARCH}  graphics=${IGRAPHICS_BACKEND}/${IGRAPHICS_RENDERER}  gui=${SMU2000_ENABLE_GUI}")
message(STATUS "  vst2=${SMU2000_BUILD_VST2} (sdk=${SMU2000_VST2_SUPPORTED}) clap=${SMU2000_BUILD_CLAP} (sdk=${SMU2000_CLAP_SUPPORTED}) (no app, no vst3)")
message(STATUS "  roms=${SMU2000_ROMS_DIR} (copy=${SMU2000_COPY_ROMS})")
message(STATUS "  tools=${SMU2000_BUILD_TOOLS} (render/live/gui exes -> ${SMU2000_OUTPUT_ROOT}/tools/${SMU2000_ARCH}/<Config>/)")
message(STATUS "  outputs -> ${SMU2000_OUTPUT_ROOT}/<api>/${SMU2000_ARCH}/<Config>/")
