# SMU2000_VST2/ui/CMakeLists.txt — smu2000_gui STATIC (Phase 7 + P8). Native Win32/GDI editor
# + the five ImGui/D3D11 PC windows (一覧・エディタ・fx・part_shapes・master).
# Sources under src/ are the unchanged upstream GUI code; the host bridge
# is SMU2000Editor.{h,cpp} (a port of src/vst3/view.cpp + view_win.cpp).
# P8 amendment (user-approved 2026-09-20, ledger §Phase 8): **ImGui + Direct3D11 PC windows
# are ALLOWED in this GUI-ON lib** — the panel itself stays raw GDI (src/ui/draw.h, system
# fonts) and only the optional PC windows pull the imgui/DX11 stack (verbatim native/
# smu2000_gui_app recipe, Makefile IMGUI_SRCS+PC_SRCS). IGraphics / NanoVG / OpenGL / Skia
# remain FORBIDDEN in every state; GUI-OFF excludes this lib entirely and stays
# byte-for-byte graphics-free. CRT (/MT) comes from the root; this standalone target carries
# its own src/src-compat includes + NOMINMAX PUBLIC.

add_library(smu2000_gui STATIC)

target_sources(smu2000_gui PRIVATE
  # --- VST3 GUI panel (the exact set Makefile VST3_SRCS pulls, minus the Steinberg glue)
  ${CMAKE_SOURCE_DIR}/src/ui/panel.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/editor.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/effects.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/layout.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/svg.cpp
  ${CMAKE_SOURCE_DIR}/src/xg/model.cpp
  # --- iPlug2 host: wraps ui::panel in a child HWND + WndProc (port of src/vst3/view.cpp)
  ${CMAKE_CURRENT_SOURCE_DIR}/SMU2000Editor.cpp
  ${CMAKE_CURRENT_SOURCE_DIR}/SMU2000Editor.h
  # --- P8: the five PC windows (一覧・エディタ・fx_editor・part_shapes・master_editor),
  # Makefile PC_SRCS verbatim (doc/pc-editor.md). png.cpp is NOT here: nothing in this set
  # references it (only src/ui/png.cpp itself includes png.h; gui.exe carries it via UI_SRCS).
  ${CMAKE_SOURCE_DIR}/src/ui/pc_editor.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/pc_window.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/xg_ui.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/overview.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/fx_editor.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/fx_help.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/part_shapes.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/master_editor.cpp
  ${CMAKE_SOURCE_DIR}/src/ui/fx_icons.cpp
  # --- P8: vendored imgui (third_party/ read-only; native/ smu2000_gui_app list verbatim)
  ${CMAKE_SOURCE_DIR}/third_party/imgui/imgui.cpp
  ${CMAKE_SOURCE_DIR}/third_party/imgui/imgui_draw.cpp
  ${CMAKE_SOURCE_DIR}/third_party/imgui/imgui_tables.cpp
  ${CMAKE_SOURCE_DIR}/third_party/imgui/imgui_widgets.cpp
  ${CMAKE_SOURCE_DIR}/third_party/imgui/backends/imgui_impl_win32.cpp
  ${CMAKE_SOURCE_DIR}/third_party/imgui/backends/imgui_impl_dx11.cpp
)

# GUI-only sources (src/**) and the host bridge don't link the engine, so they need the
# engine's PUBLIC include roots directly. PUBLIC so the plugin target inherits them too
# (its SMU2000_VST2.cpp reaches engine.h / ui/*.h via the same roots).
target_include_directories(smu2000_gui PUBLIC
  ${CMAKE_SOURCE_DIR}/src
  ${CMAKE_SOURCE_DIR}/src/compat
  ${CMAKE_SOURCE_DIR}/third_party/imgui   # P8: fx_editor.h includes imgui.h; PUBLIC so the
                                          # plugin MODULE's editor TU sees it via SMU2000Editor.h
  ${CMAKE_CURRENT_SOURCE_DIR})   # SMU2000_VST2/ui → plugin `#include "ui/SMU2000Editor.h"`

target_compile_features(smu2000_gui PUBLIC cxx_std_20)

# NOMINMAX is REQUIRED here, not just on the engine: svg.cpp includes <windows.h> (via
# svg.h) BEFORE <algorithm>, so without it MSVC's windows.h min/max macros corrupt
# std::min/std::max (C2589). Engine has it; this target is standalone so it must carry its
# own. PUBLIC keeps dependents consistent.
target_compile_definitions(smu2000_gui PUBLIC
  NOMINMAX
  _CRT_SECURE_NO_WARNINGS
  _CRT_NONSTDC_NO_WARNINGS
  WIN32
  NO_IGRAPHICS)   # native editor is IGraphics-free by construction (SMU2000Editor.h guard)

# P8: imgui_impl_win32 without gamepad → no xinput dependency (Makefile IMGUI_FLAGS, and
# native/smu2000_gui_app verbatim). PRIVATE: only the imgui backend TUs compiled here care.
target_compile_definitions(smu2000_gui PRIVATE IMGUI_IMPL_WIN32_DISABLE_GAMEPAD)

# GUI-ON editor: the Win32 libs the child window needs (gdi32 fonts/blit, comdlg32 SmartMedia
# file dialog, user32 child window/menus). PUBLIC so both the lib and the plugin MODULE link them.
# MSVC resolves the .lib names directly; MinGW maps them to -lgdi32/-lcomdlg32/-luser32.
# (opengl32 is NOT needed — the native editor has no GL; unlike the old IGraphicsWin path.)
# P8 PC windows add the DX11 half of the Makefile gui/VST3 link line: d3d11+dxgi (swap chain),
# d3dcompiler (imgui FX shader), dwmapi+imm32 (imgui_impl_win32), shell32 (pc_window.cpp's
# DragAcceptFiles/DragQueryFileW file-drop). NO xinput — IMGUI_IMPL_WIN32_DISABLE_GAMEPAD above.
target_link_libraries(smu2000_gui PUBLIC
  $<$<CXX_COMPILER_ID:MSVC>:gdi32 comdlg32 user32 d3d11 dxgi d3dcompiler dwmapi imm32 shell32>
  $<$<CXX_COMPILER_ID:GNU>:gdi32 comdlg32 user32 d3d11 dxgi d3dcompiler dwmapi imm32 shell32>)

# Japanese-commented UTF-8 sources; MSVC needs /utf-8 (mirrors engine + plugin targets),
# MinGW passes the UTF-8 bytes through as-is but needs big-obj (svg/layout) + permissive like
# the engine. PUBLIC so the plugin MODULE's editor TU (SMU2000_VST2.cpp) matches under MinGW.
target_compile_options(smu2000_gui PUBLIC
  $<$<CXX_COMPILER_ID:MSVC>:/utf-8>
  $<$<CXX_COMPILER_ID:MSVC>:/bigobj>
  $<$<CXX_COMPILER_ID:GNU>:-Wa,-mbig-obj -fpermissive>
)
