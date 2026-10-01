//! Windows HAL — WASAPI (event-driven shared/exclusive + own sinc resampler +
//! GetCurrentPadding starvation stats), waveOut fallback, winmm MIDI-in SPSC ring
//! (65536, 4×8192 SysEx requeue), MMCSS "Pro Audio", Ctrl+C graceful shutdown.
//! Ledger rows in `smu-hal / smu-hal-win / bins`; lands at M6.
