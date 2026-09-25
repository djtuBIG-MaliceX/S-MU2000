# S-MU2000 — CMake build (alternative to the Makefile)
#
#   cmake -S . -B build-cmake -DCMAKE_BUILD_TYPE=Release
#   cmake --build build-cmake -j
#   ./build-cmake/verify && ./build-cmake/render roms song.mid out.wav
#
# The Makefile remains the primary build (it also covers Windows/macOS and the
# Audio Unit). This file covers the portable core everywhere and the full
# Linux set (live + VST3 + CLAP + probes) where ALSA is available.
cmake_minimum_required(VERSION 3.16)
project(S-MU2000 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
# Non-literal printf formats are bugs (translated strings must go through "%s").
add_compile_options(-Wformat-security)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()

# ---- Emulator core (same sources as SRCS in the Makefile) -------------------
add_library(smu2000_core STATIC
  src/compat/compat.cpp
  src/smartmedia.cpp
  src/mame/sound/swp30.cpp
  src/mame/sound/swp30_jit.cpp
  src/mame/video/hd44780.cpp
  src/mame/machine/sci4.cpp
  src/mame/cpu/sh.cpp
  src/mame/cpu/sh2.cpp
  src/mame/cpu/sh2_jit.cpp
  src/compat/a64asm.cpp
  src/mame/cpu/sh7042.cpp
  src/mame/cpu/sh_adc.cpp
  src/mame/cpu/sh_bsc.cpp
  src/mame/cpu/sh_cmt.cpp
  src/mame/cpu/sh_dmac.cpp
  src/mame/cpu/sh_intc.cpp
  src/mame/cpu/sh_mtu.cpp
  src/mame/cpu/sh_port.cpp
  src/mame/cpu/sh_sci.cpp
  src/mu2000.cpp
  src/smf.cpp
)
target_include_directories(smu2000_core PUBLIC src src/compat)
target_compile_options(smu2000_core PRIVATE -Wall -Wno-unused-variable -Wno-unused-but-set-variable)
set_target_properties(smu2000_core PROPERTIES POSITION_INDEPENDENT_CODE ON)

# ---- Offline tools (run anywhere; need no ROMs except via argv) -------------
if(EMSCRIPTEN)
  # Release alone means -O2: the SH7030 interpreter and the DSP need
  # -O3 + SIMD to hold under real-time cost in the browser. Above real
  # time, the ScriptProcessor overruns pile up and the page starves --
  # it looks frozen right after boot.
  add_compile_options(-O3 -msimd128)
  add_link_options(-O3 -msimd128)
  # The emulator's tables + ROMs blow past the 16MB default heap
  add_link_options(
    -sALLOW_MEMORY_GROWTH=1
    -sINITIAL_MEMORY=64MB
    -sMAXIMUM_MEMORY=2GB
    -sENVIRONMENT=node,web
  )
endif()
foreach(tool verify boot render panel statetest blocktime)
  if(EMSCRIPTEN AND tool STREQUAL "render")
    continue() # render-wasm below already owns the render.js output
  endif()
  add_executable(${tool} src/${tool}.cpp)
  target_link_libraries(${tool} PRIVATE smu2000_core)
  if(EMSCRIPTEN)
    # node runs these directly: NODERAWFS maps the real filesystem so the
    # ROM dir and MIDI paths in argv resolve (see doc/wasm.md).
    # _smu_meg_lfo is the MEG wasm JIT's LFO import; the wasm glue looks
    # it up on the module exports, so it must survive -O3 stripping.
    target_link_options(${tool} PRIVATE -sNODERAWFS=1
      -sEXPORTED_FUNCTIONS=_main,_malloc,_free,_smu_meg_lfo)
  endif()
endforeach()
foreach(tool xgtest samptest)
  add_executable(${tool} src/${tool}.cpp src/xg/model.cpp)
  target_link_libraries(${tool} PRIVATE smu2000_core)
  if(EMSCRIPTEN)
    target_link_options(${tool} PRIVATE -sNODERAWFS=1
      -sEXPORTED_FUNCTIONS=_main,_malloc,_free,_smu_meg_lfo)
  endif()
endforeach()

# Regression suite entry point: ctest runs what `make test` runs without ROMs.
enable_testing()
add_test(NAME verify COMMAND verify)

# ---- Linux: live + VST3 + CLAP (needs ALSA) ----------------------------------
if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  find_package(PkgConfig QUIET)
  if(PKG_CONFIG_FOUND)
    pkg_check_modules(ALSA QUIET alsa)
  endif()
  if(NOT ALSA_FOUND)
    message(STATUS "ALSA not found: building offline tools only (no live/VST3/CLAP)")
    return()
  endif()

  # Cairo + fontconfig for gdi_linux.cpp (panel drawing in gui and plug-ins),
  # SDL3 for the gui window (Phase 2, doc/porting-linux-gui.md)
  pkg_check_modules(CAIRO QUIET cairo fontconfig)
  pkg_check_modules(SDL3 QUIET sdl3)

  add_executable(live src/live.cpp
    src/ui/audio_out_linux.cpp
    src/ui/audio_in_linux.cpp
    src/ui/midi_in_linux.cpp
    src/ui/midi_out_linux.cpp)
  target_link_libraries(live PRIVATE smu2000_core ${ALSA_LIBRARIES} pthread dl)
  target_include_directories(live PRIVATE ${ALSA_INCLUDE_DIRS})

  set(VST3_SDK_SRCS
    third_party/vst3/pluginterfaces/base/funknown.cpp
    third_party/vst3/pluginterfaces/base/coreiids.cpp
    third_party/vst3/pluginterfaces/base/conststringtable.cpp
    third_party/vst3/pluginterfaces/base/ustring.cpp)
  set(IMGUI_CORE
    third_party/imgui/imgui.cpp
    third_party/imgui/imgui_draw.cpp
    third_party/imgui/imgui_tables.cpp
    third_party/imgui/imgui_widgets.cpp)
  set(LINUX_PANEL_SRCS
    src/compat/gdi_linux.cpp
    src/ui/panel.cpp src/ui/layout.cpp src/ui/svg.cpp src/ui/editor.cpp
    src/ui/effects.cpp src/xg/model.cpp
    src/ui/xg_ui.cpp src/ui/fx_help.cpp src/ui/fx_icons.cpp
    ${IMGUI_CORE})

  # VST3 bundle layout: S-MU2000.vst3/Contents/x86_64-linux/S-MU2000.so
  add_library(S-MU2000-vst3 MODULE
    src/vst3/plugin.cpp src/vst3/engine.cpp src/vst3/iids.cpp
    src/vst3/automation.cpp src/vst3/view.cpp src/vst3/plug_window_linux.cpp
    ${LINUX_PANEL_SRCS} ${VST3_SDK_SRCS})
  target_include_directories(S-MU2000-vst3 PRIVATE src src/compat
    third_party/vst3 third_party/imgui ${CAIRO_INCLUDE_DIRS})
  target_link_libraries(S-MU2000-vst3 PRIVATE smu2000_core
    ${ALSA_LIBRARIES} ${CAIRO_LIBRARIES} pthread dl)
  set_target_properties(S-MU2000-vst3 PROPERTIES
    PREFIX "" SUFFIX ".so" OUTPUT_NAME "S-MU2000"
    LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/S-MU2000.vst3/Contents/x86_64-linux")

  add_library(S-MU2000-clap MODULE
    src/clap/plugin.cpp src/vst3/engine.cpp src/vst3/iids.cpp
    src/vst3/automation.cpp src/vst3/view.cpp src/vst3/plug_window_linux.cpp
    ${LINUX_PANEL_SRCS} ${VST3_SDK_SRCS})
  target_include_directories(S-MU2000-clap PRIVATE src src/compat
    third_party/vst3 third_party/clap third_party/imgui ${CAIRO_INCLUDE_DIRS})
  target_link_libraries(S-MU2000-clap PRIVATE smu2000_core
    ${ALSA_LIBRARIES} ${CAIRO_LIBRARIES} pthread dl)
  set_target_properties(S-MU2000-clap PROPERTIES
    PREFIX "" SUFFIX ".clap" OUTPUT_NAME "S-MU2000"
    LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}")

  add_executable(vst3probe
    src/vst3/probe.cpp src/vst3/probe_host_linux.cpp
    src/vst3/iids.cpp ${VST3_SDK_SRCS})
  target_include_directories(vst3probe PRIVATE src src/compat third_party/vst3)
  target_link_libraries(vst3probe PRIVATE smu2000_core dl)

  add_executable(clapprobe src/clap/probe.cpp)
  target_include_directories(clapprobe PRIVATE src src/compat third_party/clap)
  target_link_libraries(clapprobe PRIVATE smu2000_core dl)

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

# ---- Emscripten: browser build (web/) ---------------------------------------
#
# Build with the emsdk toolchain (emcmake cmake -B build-wasm -G Ninja).
# The sound engine runs in wasm and renders through the page's Web Audio;
# MIDI IN arrives through WebMIDI (web/smu2000.js wires the port dropdown).
# The panel paints through gdi_wasm.cpp (a CPU rasterizer over the GDI shim,
# blitted to a 2D canvas), the ImGui editor windows draw into their own
# WebGL canvas in the page's reserved pane -- no dialogs, no extra windows.
if(EMSCRIPTEN)
  # (memory/ENVIRONMENT options moved above the tool list so they apply
  # there too). What only the browser front end needs:
  add_link_options(
    -sEXPORTED_RUNTIME_METHODS=ccall,cwrap,FS,UTF8ToString,stringToUTF8,lengthBytesUTF8,HEAPU8,callMain
    # the page marshals MIDI/mic bytes with Module._malloc/_free directly
    # _smu_meg_lfo: the MEG wasm JIT's LFO import (see swp30_jit.cpp)
    -sEXPORTED_FUNCTIONS=_main,_malloc,_free,_smu_meg_lfo
    -sSTACK_SIZE=8MB
  )

  set(IMGUI_CORE
    third_party/imgui/imgui.cpp
    third_party/imgui/imgui_draw.cpp
    third_party/imgui/imgui_tables.cpp
    third_party/imgui/imgui_widgets.cpp)

  # gui_wasm.cpp keeps main(); the shell (app + pump) lives in ui/app_wasm.*
  add_executable(gui-wasm src/gui_wasm.cpp
    src/compat/gdi_wasm.cpp
    src/ui/panel.cpp src/ui/layout.cpp src/ui/svg.cpp
    src/ui/editor.cpp src/ui/effects.cpp src/ui/png.cpp
    src/ui/pc_editor.cpp src/ui/overview.cpp src/ui/fx_editor.cpp
    src/ui/part_shapes.cpp src/ui/master_editor.cpp
    src/ui/player.cpp src/xg/model.cpp
    src/ui/xg_ui.cpp src/ui/fx_help.cpp src/ui/fx_icons.cpp
    src/ui/app_wasm.cpp
    src/ui/window_wasm.cpp
    src/ui/pc_window_wasm.cpp
    src/ui/audio_out_wasm.cpp
    src/ui/audio_in_wasm.cpp
    src/ui/midi_in_wasm.cpp
    src/ui/midi_out_wasm.cpp
    src/ui/js_wasm.cpp
    ${IMGUI_CORE}
    third_party/imgui/backends/imgui_impl_opengl3.cpp)
  set_target_properties(gui-wasm PROPERTIES OUTPUT_NAME "smu2000" SUFFIX ".js")
  target_include_directories(gui-wasm PRIVATE src src/compat third_party/imgui)
  target_link_libraries(gui-wasm PRIVATE smu2000_core)
  target_link_options(gui-wasm PRIVATE
    -sUSE_WEBGL2=1
    -sMIN_WEBGL_VERSION=2
    -sMAX_WEBGL_VERSION=2
    # dialog round-trips from C++ await JS promises (see ui/js_wasm.cpp)
    -sASYNCIFY
    -sASYNCIFY_STACK_SIZE=8MB
    # the pull functions run from rAF/audio callbacks even while a dialog
    # has the main stack unwound -- keep them uninstrumented
    "-sASYNCIFY_REMOVE=[\"smu_frame*\",\"smu_audio_render*\",\"smu_midi_push*\",\"smu_gl_*\",\"smu_mouse_*\",\"smu_wheel\",\"smu_hand_cursor\",\"smu_key\",\"smu_pane_*\",\"smu_panel_*\",\"smu_state\",\"smu_message\",\"smu_set_*\",\"smu_set_mic\",\"smu_audio_in_push\"]"
    # GUI.ini and the boot cache survive reloads
    -lidbfs.js
    # IDBFS is only linked here (render-wasm has NODERAWFS instead), so
    # the export list must be repeated target-scoped with it; CMake puts
    # target options last on the link line and the last -s flag wins.
    # Without exporting IDBFS, Module.IDBFS is undefined and the page's
    # FS.mount() throws -> nothing persists across reloads.
    -sEXPORTED_RUNTIME_METHODS=ccall,cwrap,FS,IDBFS,UTF8ToString,stringToUTF8,lengthBytesUTF8,HEAPU8,callMain
  )

  # Ship the page next to the binary (index.html beside smu2000.js/.wasm so
  # `python -m http.server` in the build dir just works). roms/ is fetched
  # at run time.
  add_custom_command(TARGET gui-wasm POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_directory
      ${CMAKE_CURRENT_SOURCE_DIR}/web ${CMAKE_CURRENT_BINARY_DIR}
    COMMENT "Copying web/ next to smu2000.js")

  # Offline render as wasm too: `node render.js` smoke-tests the engine.
  # NODERAWFS lets it read the ROMs / write the WAV straight off disk.
  add_executable(render-wasm src/render.cpp)
  set_target_properties(render-wasm PROPERTIES OUTPUT_NAME "render" SUFFIX ".js")
  target_link_libraries(render-wasm PRIVATE smu2000_core)
  target_link_options(render-wasm PRIVATE -sNODERAWFS=1)
endif()
