//! On-die SH7042 peripherals (ledger rows `periph:*`). One submodule per
//! `src/mame/cpu/sh_*.cpp` unit as its row lands; `sci` is first (row
//! `periph: sci`, session F4).

pub mod cmt;
pub mod intc;
pub mod mtu;
pub mod port;
pub mod sci;
pub mod stubs;

pub use cmt::ShCmt;
pub use intc::Sh2Intc;
pub use mtu::{Sh2Mtu, Sh2MtuChannel};
pub use port::{ShPort16, ShPort32};
pub use sci::{Sh2Sci, Sh2SciPair};
