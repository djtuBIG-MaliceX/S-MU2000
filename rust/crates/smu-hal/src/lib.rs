//! Platform traits for the live binary: MIDI input (SPSC producer), audio output
//! (event-driven, shared/exclusive, starvation stats), graceful quit.
//! Filled in with the M6 row; the Windows impl lives in smu-hal-win.
