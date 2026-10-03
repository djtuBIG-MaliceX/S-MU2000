// license:BSD-3-Clause
//
// origin: src/xg/sysfx.h (68 L) — address remap of the insertion fx_params
// table onto the system-effect addresses (issue #35). Verified on firmware
// by tools/fxsweep/sysfx_check.cpp (reverb 196 / chorus 154 / variation
// 1256 params; only param 10 Dry/Wet does not answer — sysfx.h:11-14).

use super::fx;

/// origin: sysfx.h:25 `enum class sysfx`
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sysfx {
    Reverb,
    Chorus,
    Variation,
}

/// origin: sysfx.h:28-31 — type address (after 02 01)
pub fn sysfx_type_lo(which: Sysfx) -> u8 {
    if which == Sysfx::Reverb {
        0x00
    } else if which == Sysfx::Chorus {
        0x20
    } else {
        0x40
    }
}

/// origin: sysfx.h:35-64 — one insertion-table row -> system-effect address
/// (after 02 01). `p` is the lite (addr, size) row from fx.rs. Writes the
/// byte count through `size`; -1 when the block does not carry that param.
pub fn sysfx_addr(which: Sysfx, p: (u8, u8), size: &mut i32) -> i32 {
    //  insertion address -> "parameter number" (1-16)  (:37-42)
    let mut n = 0i32;
    if p.0 >= 0x02 && p.0 <= 0x0b {
        n = p.0 as i32 - 0x02 + 1; // :39
    } else if p.0 >= 0x30 && p.0 <= 0x43 {
        n = (p.0 as i32 - 0x30) / 2 + 1; // :40
    } else if p.0 >= 0x20 && p.0 <= 0x25 {
        n = p.0 as i32 - 0x20 + 11; // :41
    } else {
        return -1; // :42
    }

    match which {
        // :45-49 — 10 is Dry/Wet: system effects mix by return level, no
        // Dry/Wet (firmware keeps it 0) -> skip
        Sysfx::Reverb => {
            if p.1 != 1 || n == 10 {
                return -1;
            }
            *size = 1;
            if n <= 10 {
                0x02 + (n - 1)
            } else {
                0x10 + (n - 11)
            }
        }
        // :50-54 — 10 Dry/Wet (none), 11-16 rejected by firmware (no RAM)
        Sysfx::Chorus => {
            if p.1 != 1 || n >= 10 {
                return -1;
            }
            *size = 1;
            0x22 + (n - 1)
        }
        // :55-61 — params 1-10 are 2 bytes (02 01 42-55), 11-16 one byte
        Sysfx::Variation => {
            if n <= 10 {
                *size = 2;
                0x42 + 2 * (n - 1)
            } else {
                *size = 1;
                0x70 + (n - 11)
            }
        }
    }
}

/// convenience: the `(addr, size)` rows of an fx type (keeps the C++
/// `def->params[i]` call shape readable at the compare() site)
pub fn def_params(kind: i32) -> &'static [(u8, u8)] {
    match fx::fx_find(kind) {
        Some(d) => d.params,
        None => &[],
    }
}
