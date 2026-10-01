//! Support devices — HD44780 LCD registers (busy-flag timing is boot-critical) and
//! SCI4 PLG serial. Mirrors `src/mame/video/hd44780.cpp`, `src/mame/machine/sci4.cpp`.

pub mod hd44780;
pub mod sci4;
