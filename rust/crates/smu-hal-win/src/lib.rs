//! Windows HAL — WASAPI (event-driven shared/exclusive + own sinc resampler +
//! GetCurrentPadding starvation stats), waveOut fallback, winmm MIDI-in SPSC ring
//! (65536, 4×8192 SysEx requeue), MMCSS "Pro Audio", Ctrl+C graceful shutdown.
//! Ledger rows in `smu-hal / smu-hal-win / bins`; lands at M6.
//!
//! Landed this row (`live main` + `midi in` + the winmm half of
//! `waveout fallback`): `sys` (kernel32/avrt FFI: QPC, Ctrl+C, MMCSS, events),
//! `midi` (ui/midi_in.cpp winmm MIDI-in as the g_midi global), `waveout`
//! (winmm waveOut declarations; the live.cpp order lives in bins/live.rs).
//! Landed this row (`audio out`, M6b): `wasapi` (ui/audio_out.cpp +
//! ui/resampler.h — event-driven shared/exclusive, own sinc resampler,
//! GetCurrentPadding starvation stats, hand-rolled COM vtables).
//! Landed this row (M6b `midisend`): `midi_out` (winmm MIDI-OUT +
//! timeBeginPeriod/timeGetTime FFI for bins/midisend.rs, src/midisend.cpp).

pub mod midi;
pub mod midi_out;
pub mod sys;
pub mod wasapi;
pub mod waveout;
