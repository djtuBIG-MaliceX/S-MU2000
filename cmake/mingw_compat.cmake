# mingw_compat.cmake — MinGW-w64 (MSYS2) compatibility layer for the SMU2000_VST2 CMake build.
# Copied verbatim from ../sw10_plug/cmake/mingw_compat.cmake (P5 owns MinGW; inert on MSVC).
#
# Included from the root CMakeLists.txt AFTER find_package(iPlug2) when the compiler is
# the Windows GNU/Clang (MinGW) toolchain. The MSVC path is untouched by everything here.
#
# Scope (see VST2_LEDGER.md Phase 5; JIT status updated 2026-09-16, CPU32_LEDGER.md Phases 1-8):
#   - Both x86_64 (MSYS2 MINGW64 shell, preset mingw-x64/mingw-clang-x64) and x86
#     (MSYS2 MINGW32 shell, preset mingw-win32) are supported: the SMU2000 engine is
#     arch-clean. Both JITs are now DUAL-MODE (sh2_jit.cpp / swp30_jit.cpp: x64 under
#     __x86_64__||_M_X64 — MSVC x64 included — and x86-32 under __i386__||_M_IX86 via
#     SMU_JIT32_PORT_SH2/MEG self-defines), so win32 MinGW compiles the JIT path too, in
#     principle: the 32-bit emitter is validated (tools/x64asm32_test.cpp + MSVC x86 A/B,
#     bit-exact) and the x64 path is GCC+Clang-proven here. No SSE is emitted in either
#     mode, so no win32 stack-alignment/SSE flags are needed or set anywhere below.
#   - DEV-BOX CAVEAT: the mingw-w64-i686 gcc on this machine is INOPERABLE (cc1plus loads
#     and silently exits; modifying anything under C:\msys64 is forbidden), so the win32
#     MinGW leg cannot be executed here — vs-win32 (MSVC amd64_x86, tools/msvc32_build.ps1
#     for native tools) is the supported Win32 plugin build. Never attempt mingw32 repairs.
#   - Default build is GRAPHICS-FREE (SMU2000_ENABLE_GUI=OFF): no IGraphics/NanoVG.
#     The GUI-ON path keeps the sw10 NanoVG/GL2 notes (nanovg.c+glad.c unity-built
#     inside IGraphicsWin.cpp; glad LoadLibrary()s opengl32.dll; no prebuilt MSVC
#     graphics libs are linked, unlike SKIA).
#
# Everything here stays in the superproject; the iPlug2 submodule gitlink is never touched.

if(NOT MINGW)
  return()
endif()

# ---------------------------------------------------------------------------
# Static runtime + COFF section fixes.
#   -static-libgcc/-static-libstdc++ : plugins must not depend on libgcc/libstdc++ DLLs
#                                      (matches the MSVC /MT self-contained CRT intent).
#   -Wa,-mbig-obj                    : the huge template TUs (sh.cpp / mu2000.cpp — the
#                                      same ones that need /bigobj on MSVC — plus
#                                      IGraphicsWin.cpp when GUI is ON) exceed the plain
#                                      COFF assembler section limit.
#   -fpermissive                     : iPlug/APP glue assigns FARPROC (GetProcAddress) to
#                                      void* implicitly — valid-permissive MSVC, hard error
#                                      under libstdc++. Downgrades it back to a warning.
#   -include <prelude>               : libstdc++ does not pull <memory>/<cmath>/... in
#                                      transitively the way MSVC's <Windows.h>+SDK headers
#                                      happen to; force-include a small STL prelude into every
#                                      C++ TU (iPlug SDK + VST3 SDK + plugin sources) so the
#                                      upstream headers that use std::unique_ptr etc. resolve.
# COMPILE_LANGUAGE guards keep windres (RC) and plain-C TUs out of the C++-only flags.
# ---------------------------------------------------------------------------
add_compile_definitions(_USE_MATH_DEFINES)          # M_PI / M_PI_2 under __STRICT_ANSI__
add_compile_options("$<$<COMPILE_LANGUAGE:C,CXX>:-Wa,-mbig-obj>")
add_compile_options("$<$<COMPILE_LANGUAGE:C,CXX>:-fpermissive>")
add_link_options(-static -static-libgcc -static-libstdc++)
string(APPEND CMAKE_CXX_FLAGS
  " -include \"${CMAKE_CURRENT_LIST_DIR}/mingw_portability_prelude.h\"")

# The GUI-ON editor (P7) is NATIVE GDI (ui::panel) — no IGraphicsWin.cpp, so NO opengl32/WGL is
# used. gdi32/comdlg32/user32 are what the child window + SmartMedia dialog need; smu2000_gui
# links them PUBLIC (MinGW maps Foo.lib->-lfoo). Kept as a global safety net for GUI-ON only;
# the graphics-free default (SMU2000_ENABLE_GUI=OFF) links NONE (hard rule #6).
if(SMU2000_ENABLE_GUI)
  link_libraries(gdi32 comdlg32 user32)
endif()

# ---------------------------------------------------------------------------
# smu2000_mingw_fixup_imported_libs() — the upstream iPlug2 INTERFACE targets
# (iPlug2::IPlug, iPlug2::APP, iPlug2::Extras::OSC) list MSVC import-library names
# (Shlwapi.lib, comctl32.lib, wininet.lib, dsound.lib, winmm.lib, ws2_32.lib). MinGW
# ships lib<name>.a and wants -l<name>, and ld will NOT find a "Foo.lib". Rewrite those
# bare *.lib entries in the imported targets' INTERFACE_LINK_LIBRARIES to plain names
# (CMake then emits -lfoo). Real file paths (skia.lib / WebView2LoaderStatic.lib) are
# left alone — those live behind backends/features the NanoVG MinGW build doesn't use.
# ---------------------------------------------------------------------------
function(smu2000_mingw_fixup_lib_names target)
  if(NOT TARGET ${target})
    return()
  endif()
  get_target_property(_libs ${target} INTERFACE_LINK_LIBRARIES)
  if(NOT _libs)
    return()
  endif()
  set(_rewritten "")
  set(_changed FALSE)
  foreach(_lib IN LISTS _libs)
    # Only bare "Name.lib" tokens (no directory, no generator expression, no path).
    if(_lib MATCHES "^([A-Za-z0-9_]+)\.lib$")
      list(APPEND _rewritten "${CMAKE_MATCH_1}")
      set(_changed TRUE)
    else()
      list(APPEND _rewritten "${_lib}")
    endif()
  endforeach()
  if(_changed)
    set_target_properties(${target} PROPERTIES INTERFACE_LINK_LIBRARIES "${_rewritten}")
    message(STATUS "SMU2000/mingw: rewrote MSVC .lib names in ${target} -> MinGW -l names")
  endif()
endfunction()

foreach(_t iPlug2::IPlug iPlug2::APP iPlug2::Extras::OSC iPlug2::Extras::Synth iPlug2::VST3 iPlug2::VST2 iPlug2::CLAP iPlug2::IGraphics)
  smu2000_mingw_fixup_lib_names(${_t})
endforeach()
