//! Compat layer — mirror of `src/compat/` (mamecompat timer queue, membus flat
//! big-endian bus, config paths, ROM loaders).
//!
//! Ledger rows: `compat/timers`, `compat/bus`, `compat/paths+console`, `rom loaders`.
//! Hard rule: transliterate the C++ line-for-line (see PORTING_LEDGER.md invariants).

// No width aliases: Rust's prelude u8..u64/i8..i64 already map 1:1 onto the
// C++ `u8..s64` typedefs. Use them directly (and `wrapping_*` where C++ wraps).

pub mod bus;
pub mod paths;
pub mod roms;
pub mod state_io;
pub mod timers;

pub use state_io::{state_pack, state_unpack, StateIo};
