//! SWP30 tone generator — mirror of `src/mame/sound/swp30.cpp` (AWM2 voices +
//! MEG interpreter + mixer), instantiated ×2 by smu-machine.
//! Ledger rows `reg dispatch`, `sample fetch`, `voice engine`, `MEG`, `mixer / MELO`.
//! The known-buggy 8-bit compressed expander is reproduced verbatim (see ledger pitfalls;
//! GCC warns its `a`/`r` may read uninit at swp30.cpp:3717–3743 — settle that question
//! with a trace before choosing Rust initializers).

pub mod fetch;
pub mod meg;
pub mod mix;
pub mod regs;
pub mod voice;

#[cfg(test)]
mod fetch_tests;

#[cfg(test)]
mod meg_tests;

#[cfg(test)]
mod mix_tests;

#[cfg(test)]
mod mixb_tests;

#[cfg(test)]
mod voice_tests;

pub use fetch::StreamingBlock;
pub use regs::{Swp30, MASTER_BASE, SLAVE_BASE};
