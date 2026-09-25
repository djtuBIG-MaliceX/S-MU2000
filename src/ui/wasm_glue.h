// license:BSD-3-Clause
//
// The wires between the Emscripten back-ends (midi_in_wasm.cpp,
// midi_out_wasm.cpp, audio_out_wasm.cpp, audio_in_wasm.cpp), the frame
// loop (gui_wasm.cpp / app_wasm.h) and the JS bridge (js_wasm.cpp).
//
// The browser is single threaded and the page owns the clock: WebMIDI
// callbacks and the ScriptProcessor both run on the main thread, so every
// "thread" in this port is really the rAF/rpc callback that happens to be
// running. Nothing here needs a lock for that reason -- except that
// push_midi may also be called from a plain JS event callback between
// frames, which is the same thread, so even that is fine.

#ifndef S_MU2000_UI_WASM_GLUE_H
#define S_MU2000_UI_WASM_GLUE_H

#pragma once

#include "compat/mamecompat.h"

#include <functional>
#include <string>
#include <vector>

namespace ui {

// ---- WebMIDI port names, registered by the page -------------------------
// The dropdowns live in HTML; whatever the page has selected it tells us
// here and the menu list() calls answer with the same names, so the menu
// and the dropdown cannot disagree. Index == device number.
void wasm_set_midi_in_ports(std::vector<std::string> names);
void wasm_set_midi_out_ports(std::vector<std::string> names);
std::vector<std::string> wasm_midi_in_ports();
std::vector<std::string> wasm_midi_out_ports();

// The one microphone the page opened ("" = none). rate is the AudioContext
// rate the pushed frames come at (audio_in_wasm resamples to 44100).
void wasm_set_audio_in_device(std::string name, double rate);

// ---- page -> engine ------------------------------------------------------

// Bytes off a WebMIDI input, already a whole message. port is 0..3 (A-D);
// app_wasm routes them into the same ui::midi_in rings the desktop
// back-ends fill, and engine::fill() drains those exactly as before.
void wasm_midi_push(int port, const u8 *p, size_t n);

// ---- engine -> page ------------------------------------------------------

// One assembled THRU/MIDI-OUT message. js_wasm.cpp forwards it to JS,
// which routes it to whichever output the dropdown picked (called from
// midi_out_wasm's assembler, on the main thread).
void wasm_midi_out_bytes(const u8 *p, size_t n);

// ---- audio ---------------------------------------------------------------

// The running audio_out registers its fill here; the page pulls blocks
// from the ScriptProcessor through smu_audio_render (js_wasm.cpp).
void wasm_audio_register(std::function<void(s16 *, u32)> fill, std::string name);
void wasm_audio_unregister();
// False when nothing is registered (silent block filled)
bool wasm_audio_pull(s16 *out, u32 frames);

// Mic frames from the page, at the device's own rate, s16 interleaved.
// audio_in_wasm resamples to 44100 with ui::resampler.
void wasm_audio_in_push(const s16 *frames, u32 frames_count);

} // namespace ui

#endif // S_MU2000_UI_WASM_GLUE_H
