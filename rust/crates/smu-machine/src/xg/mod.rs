// license:BSD-3-Clause
//
// Firmware-side XG parameter layer (doc/params.md) — NOT the native engine
// (src/xg/native_driver.h / native_voice.h are dead per AGENTS and untouched).
// Params are asked of the MU2000 firmware over SysEx; the firmware answers on
// MIDI OUT and `Model` keeps the mirror (origin: src/xg/model.h:1-16).
//
// Port map for the S4/W-XG row:
//   model.rs  <- src/xg/model.h + src/xg/model.cpp (481 L)
//   ram.rs    <- src/xg/ram.h (230 L)
//   sysfx.rs  <- src/xg/sysfx.h (68 L)
//   fx.rs     <- src/xg/fx_params.h (1701 L) generated subset (addr,size per
//                param + msb/lsb; display fields omitted) — regen script was
//                the temp gen_fx_rs.py this session
//   state.rs  <- src/ui/xg_state.h (308 L) — firmware-param glue only, moved
//                under xg/ (deviation; ui/ stays frozen)

pub mod fx;
pub mod model;
pub mod ram;
pub mod state;
pub mod sysfx;
