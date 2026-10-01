//! SH-2 core + SH7042A SoC — mirror of `src/mame/cpu/sh*` interpreter path only
//! (ledger rows `sh2 core`, `sh2 device`, `sh7042`, `periph:*`).
//! The DRC half of sh.cpp (dead note at :1876, disk-verified; region-for-region
//! ported span is :1..1874 with :19-98 device_start folded into `Sh2Core::new`) and
//! the x64/arm JITs are NOT ported (M9 optional).
//! Keep `--trace-pc` / `--hash-pc` hook parity — they are the M2 boot gate.

pub mod core;
pub mod device;
pub mod periph;
pub mod sh7042;
