// license:BSD-3-Clause
//
// origin: src/xg/model.cpp (481 L) + src/xg/model.h (167 L) — the
// firmware-XG parameter layer: definition table, SysEx builders, and the
// MIDI-OUT-mirror `Model`. Values are ASKED of the MU2000 firmware, never
// assumed (model.h:5-11). Table rows are the ones xgtest.exe verifies
// against the firmware (model.cpp:20-28 notes: bank applies on program,
// element_reserve total cap, key_assign 0-1 only).
//
// Deviations (disclosed):
// - `where` field renamed `where_` (Rust keyword). Same width/meaning.
// - edit/raw listeners kept as Option<Box<dyn FnMut>>; xgtest installs none.
// - model.cpp:12 `#include <cstdio>` — only format()'s snprintf needs it;
//   Rust uses format! (same output; %+d matches C %+d incl. "+0").

use std::collections::HashMap;
use std::collections::VecDeque;

/// origin: model.h:30-34 `enum class area`
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Area {
    System, // 00 00 xx
    Effect, // 02 01 xx / 03 nn xx
    Part,   // 08 pp xx
}

/// origin: model.h:36-39 `enum class coding`
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Coding {
    Byte7, // 7-bit bytes, MSB first (value = MSB*128 + LSB)
    Nibble, // 4+4 in one byte (master tune, detune)
}

/// origin: model.h:41-48 `enum class view`
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Raw,
    Plus1,
    Center,
    Pan,
    Choice,
    PartOff,
}

/// origin: model.h:50-63 `struct param`
pub struct Param {
    pub key: &'static str, // "part.volume"
    pub label: &'static str, // "Volume"
    pub where_: Area, // `where` on disk (keyword collision)
    pub hi: u8,
    pub mid: u8, // unused on part rows (part number goes there)
    pub lo: u8,
    pub size: u8, // SysEx data bytes
    pub enc: Coding,
    pub min: i32,
    pub max: i32,
    pub special: i32, // out-of-range accepted value (OFF=127), else -1
    pub def: i32, // XG doc initial value (not firmware-verified)
    pub how: View,
    pub center: i32, // for View::Center
    pub choices: Option<&'static [&'static str]>, // View::Choice, from min
}

// origin: model.cpp:12-18 choice tables
static MONO_POLY: [&str; 2] = ["MONO", "POLY"]; // :12
static KEY_ASSIGN: [&str; 2] = ["SINGLE", "MULTI"]; // :13
static PART_MODE: [&str; 6] = ["NORMAL", "DRUM", "DRUMS1", "DRUMS2", "DRUMS3", "DRUMS4"]; // :14
static CONNECT: [&str; 2] = ["INSERTION", "SYSTEM"]; // :15
static EQ_TYPE: [&str; 5] = ["FLAT", "JAZZ", "POPS", "ROCK", "CONCERT"]; // :16
static EQ_SHAPE: [&str; 2] = ["SHELF", "PEAK"]; // :17
static OFF_ON: [&str; 2] = ["OFF", "ON"]; // :18

/// origin: model.cpp:30-171 `TABLE` (params() at :193). Addresses follow the
/// XG spec; the 41-byte multipart dump was byte-matched against firmware
/// dumps (model.cpp:20-22). Transcribed verbatim.
const TABLE: &[Param] = &[
    // key, label, area, hi, mid, lo, size, coding, min, max, sp, def, view, ctr, choices
    Param { key: "system.master_tune", label: "Master Tune", where_: Area::System, hi: 0x00, mid: 0x00, lo: 0x00, size: 4, enc: Coding::Nibble, min: 0x000, max: 0x7ff, special: -1, def: 0x400, how: View::Center, center: 0x400, choices: None }, // :31
    Param { key: "system.master_volume", label: "Master Vol", where_: Area::System, hi: 0x00, mid: 0x00, lo: 0x04, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 127, how: View::Raw, center: 0, choices: None }, // :32
    Param { key: "system.transpose", label: "Transpose", where_: Area::System, hi: 0x00, mid: 0x00, lo: 0x06, size: 1, enc: Coding::Byte7, min: 0x28, max: 0x58, special: -1, def: 0x40, how: View::Center, center: 0x40, choices: None }, // :33
    // model.cpp:35 — these addresses were verified by measuring sound (doc/effects.md)
    Param { key: "reverb.type", label: "Rev Type", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x00, size: 2, enc: Coding::Byte7, min: 0, max: 0x3fff, special: -1, def: 0x80, how: View::Raw, center: 0, choices: None }, // :36
    Param { key: "reverb.return", label: "Rev Return", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x0c, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Raw, center: 0, choices: None }, // :37
    Param { key: "reverb.pan", label: "Rev Pan", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x0d, size: 1, enc: Coding::Byte7, min: 1, max: 127, special: -1, def: 64, how: View::Pan, center: 0, choices: None }, // :38
    Param { key: "chorus.type", label: "Cho Type", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x20, size: 2, enc: Coding::Byte7, min: 0, max: 0x3fff, special: -1, def: 0x2080, how: View::Raw, center: 0, choices: None }, // :39
    Param { key: "chorus.return", label: "Cho Return", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x2c, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Raw, center: 0, choices: None }, // :40
    Param { key: "chorus.pan", label: "Cho Pan", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x2d, size: 1, enc: Coding::Byte7, min: 1, max: 127, special: -1, def: 64, how: View::Pan, center: 0, choices: None }, // :41
    Param { key: "chorus.to_reverb", label: "Cho>Rev", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x2e, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :42
    Param { key: "variation.type", label: "Var Type", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x40, size: 2, enc: Coding::Byte7, min: 0, max: 0x3fff, special: -1, def: 0x280, how: View::Raw, center: 0, choices: None }, // :43
    Param { key: "variation.return", label: "Var Return", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x56, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Raw, center: 0, choices: None }, // :44
    Param { key: "variation.pan", label: "Var Pan", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x57, size: 1, enc: Coding::Byte7, min: 1, max: 127, special: -1, def: 64, how: View::Pan, center: 0, choices: None }, // :45
    Param { key: "variation.to_reverb", label: "Var>Rev", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x58, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :46
    Param { key: "variation.to_chorus", label: "Var>Cho", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x59, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :47
    Param { key: "variation.connect", label: "Var Connect", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x5a, size: 1, enc: Coding::Byte7, min: 0, max: 1, special: -1, def: 0, how: View::Choice, center: 0, choices: Some(&CONNECT) }, // :48
    Param { key: "variation.part", label: "Var Part", where_: Area::Effect, hi: 0x02, mid: 0x01, lo: 0x5b, size: 1, enc: Coding::Byte7, min: 0, max: 65, special: 127, def: 127, how: View::PartOff, center: 0, choices: None }, // :49
    Param { key: "insertion1.type", label: "Ins1 Type", where_: Area::Effect, hi: 0x03, mid: 0x00, lo: 0x00, size: 2, enc: Coding::Byte7, min: 0, max: 0x3fff, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :50
    Param { key: "insertion1.part", label: "Ins1 Part", where_: Area::Effect, hi: 0x03, mid: 0x00, lo: 0x0c, size: 1, enc: Coding::Byte7, min: 0, max: 65, special: 127, def: 127, how: View::PartOff, center: 0, choices: None }, // :51
    Param { key: "insertion2.type", label: "Ins2 Type", where_: Area::Effect, hi: 0x03, mid: 0x01, lo: 0x00, size: 2, enc: Coding::Byte7, min: 0, max: 0x3fff, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :52
    Param { key: "insertion2.part", label: "Ins2 Part", where_: Area::Effect, hi: 0x03, mid: 0x01, lo: 0x0c, size: 1, enc: Coding::Byte7, min: 0, max: 65, special: 127, def: 127, how: View::PartOff, center: 0, choices: None }, // :53
    Param { key: "insertion3.type", label: "Ins3 Type", where_: Area::Effect, hi: 0x03, mid: 0x02, lo: 0x00, size: 2, enc: Coding::Byte7, min: 0, max: 0x3fff, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :54
    Param { key: "insertion3.part", label: "Ins3 Part", where_: Area::Effect, hi: 0x03, mid: 0x02, lo: 0x0c, size: 1, enc: Coding::Byte7, min: 0, max: 65, special: 127, def: 127, how: View::PartOff, center: 0, choices: None }, // :55
    Param { key: "insertion4.type", label: "Ins4 Type", where_: Area::Effect, hi: 0x03, mid: 0x03, lo: 0x00, size: 2, enc: Coding::Byte7, min: 0, max: 0x3fff, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :56
    Param { key: "insertion4.part", label: "Ins4 Part", where_: Area::Effect, hi: 0x03, mid: 0x03, lo: 0x0c, size: 1, enc: Coding::Byte7, min: 0, max: 65, special: 127, def: 127, how: View::PartOff, center: 0, choices: None }, // :57
    // model.cpp:59-60 — master EQ 02 40 00-14: ranges written 0/127 and read
    // back; defaults are the after-boot values
    Param { key: "master_eq.type", label: "EQ Type", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x00, size: 1, enc: Coding::Byte7, min: 0, max: 4, special: -1, def: 0, how: View::Choice, center: 0, choices: Some(&EQ_TYPE) }, // :61
    Param { key: "master_eq.gain1", label: "EQ Gain1", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x01, size: 1, enc: Coding::Byte7, min: 52, max: 76, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :62
    Param { key: "master_eq.freq1", label: "EQ Freq1", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x02, size: 1, enc: Coding::Byte7, min: 4, max: 40, special: -1, def: 12, how: View::Raw, center: 0, choices: None }, // :63
    Param { key: "master_eq.q1", label: "EQ Q1", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x03, size: 1, enc: Coding::Byte7, min: 1, max: 120, special: -1, def: 7, how: View::Raw, center: 0, choices: None }, // :64
    Param { key: "master_eq.shape1", label: "EQ Shape1", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x04, size: 1, enc: Coding::Byte7, min: 0, max: 1, special: -1, def: 0, how: View::Choice, center: 0, choices: Some(&EQ_SHAPE) }, // :65
    Param { key: "master_eq.gain2", label: "EQ Gain2", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x05, size: 1, enc: Coding::Byte7, min: 52, max: 76, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :66
    Param { key: "master_eq.freq2", label: "EQ Freq2", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x06, size: 1, enc: Coding::Byte7, min: 14, max: 54, special: -1, def: 28, how: View::Raw, center: 0, choices: None }, // :67
    Param { key: "master_eq.q2", label: "EQ Q2", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x07, size: 1, enc: Coding::Byte7, min: 1, max: 120, special: -1, def: 7, how: View::Raw, center: 0, choices: None }, // :68
    Param { key: "master_eq.gain3", label: "EQ Gain3", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x09, size: 1, enc: Coding::Byte7, min: 52, max: 76, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :69
    Param { key: "master_eq.freq3", label: "EQ Freq3", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x0a, size: 1, enc: Coding::Byte7, min: 14, max: 54, special: -1, def: 34, how: View::Raw, center: 0, choices: None }, // :70
    Param { key: "master_eq.q3", label: "EQ Q3", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x0b, size: 1, enc: Coding::Byte7, min: 1, max: 120, special: -1, def: 7, how: View::Raw, center: 0, choices: None }, // :71
    Param { key: "master_eq.gain4", label: "EQ Gain4", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x0d, size: 1, enc: Coding::Byte7, min: 52, max: 76, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :72
    Param { key: "master_eq.freq4", label: "EQ Freq4", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x0e, size: 1, enc: Coding::Byte7, min: 14, max: 54, special: -1, def: 46, how: View::Raw, center: 0, choices: None }, // :73
    Param { key: "master_eq.q4", label: "EQ Q4", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x0f, size: 1, enc: Coding::Byte7, min: 1, max: 120, special: -1, def: 7, how: View::Raw, center: 0, choices: None }, // :74
    Param { key: "master_eq.gain5", label: "EQ Gain5", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x11, size: 1, enc: Coding::Byte7, min: 52, max: 76, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :75
    Param { key: "master_eq.freq5", label: "EQ Freq5", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x12, size: 1, enc: Coding::Byte7, min: 28, max: 58, special: -1, def: 52, how: View::Raw, center: 0, choices: None }, // :76
    Param { key: "master_eq.q5", label: "EQ Q5", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x13, size: 1, enc: Coding::Byte7, min: 1, max: 120, special: -1, def: 7, how: View::Raw, center: 0, choices: None }, // :77
    Param { key: "master_eq.shape5", label: "EQ Shape5", where_: Area::Effect, hi: 0x02, mid: 0x40, lo: 0x14, size: 1, enc: Coding::Byte7, min: 0, max: 1, special: -1, def: 0, how: View::Choice, center: 0, choices: Some(&EQ_SHAPE) }, // :78
    // model.cpp:80 — multiparts 08 pp 00-28 (41 bytes)
    Param { key: "part.element_reserve", label: "Elem Rsv", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x00, size: 1, enc: Coding::Byte7, min: 0, max: 32, special: -1, def: 2, how: View::Raw, center: 0, choices: None }, // :81
    Param { key: "part.bank_msb", label: "Bank MSB", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x01, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :82
    Param { key: "part.bank_lsb", label: "Bank LSB", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x02, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :83
    Param { key: "part.program", label: "Program", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x03, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Plus1, center: 0, choices: None }, // :84
    Param { key: "part.rcv_channel", label: "Rcv Ch", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x04, size: 1, enc: Coding::Byte7, min: 0, max: 63, special: 127, def: 0, how: View::Raw, center: 0, choices: None }, // :85
    Param { key: "part.mono_poly", label: "Mono/Poly", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x05, size: 1, enc: Coding::Byte7, min: 0, max: 1, special: -1, def: 1, how: View::Choice, center: 0, choices: Some(&MONO_POLY) }, // :86
    Param { key: "part.key_assign", label: "Key Assign", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x06, size: 1, enc: Coding::Byte7, min: 0, max: 1, special: -1, def: 1, how: View::Choice, center: 0, choices: Some(&KEY_ASSIGN) }, // :87
    Param { key: "part.mode", label: "Part Mode", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x07, size: 1, enc: Coding::Byte7, min: 0, max: 5, special: -1, def: 0, how: View::Choice, center: 0, choices: Some(&PART_MODE) }, // :88
    Param { key: "part.note_shift", label: "Note Shift", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x08, size: 1, enc: Coding::Byte7, min: 0x28, max: 0x58, special: -1, def: 0x40, how: View::Center, center: 0x40, choices: None }, // :89
    Param { key: "part.detune", label: "Detune", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x09, size: 2, enc: Coding::Nibble, min: 0x00, max: 0xff, special: -1, def: 0x80, how: View::Center, center: 0x80, choices: None }, // :90
    Param { key: "part.volume", label: "Volume", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x0b, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 100, how: View::Raw, center: 0, choices: None }, // :91
    Param { key: "part.vel_depth", label: "Vel Depth", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x0c, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Raw, center: 0, choices: None }, // :92
    Param { key: "part.vel_offset", label: "Vel Offset", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x0d, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Raw, center: 0, choices: None }, // :93
    Param { key: "part.pan", label: "Pan", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x0e, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Pan, center: 0, choices: None }, // :94
    Param { key: "part.note_low", label: "Note Low", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x0f, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :95
    Param { key: "part.note_high", label: "Note High", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x10, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 127, how: View::Raw, center: 0, choices: None }, // :96
    Param { key: "part.dry_level", label: "Dry Level", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x11, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 127, how: View::Raw, center: 0, choices: None }, // :97
    Param { key: "part.chorus_send", label: "Cho Send", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x12, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :98
    Param { key: "part.reverb_send", label: "Rev Send", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x13, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 40, how: View::Raw, center: 0, choices: None }, // :99
    Param { key: "part.variation_send", label: "Var Send", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x14, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :100
    Param { key: "part.vib_rate", label: "Vib Rate", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x15, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :101
    Param { key: "part.vib_depth", label: "Vib Depth", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x16, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :102
    Param { key: "part.vib_delay", label: "Vib Delay", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x17, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :103
    Param { key: "part.cutoff", label: "Cutoff", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x18, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :104
    Param { key: "part.resonance", label: "Resonance", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x19, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :105
    Param { key: "part.attack", label: "Attack", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x1a, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :106
    Param { key: "part.decay", label: "Decay", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x1b, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :107
    Param { key: "part.release", label: "Release", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x1c, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :108
    Param { key: "part.mw_pitch", label: "MW Pitch", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x1d, size: 1, enc: Coding::Byte7, min: 0x28, max: 0x58, special: -1, def: 0x40, how: View::Center, center: 0x40, choices: None }, // :109
    Param { key: "part.mw_filter", label: "MW Filter", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x1e, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :110
    Param { key: "part.mw_amp", label: "MW Amp", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x1f, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :111
    Param { key: "part.mw_lfo_pmod", label: "MW LFO PM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x20, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 10, how: View::Raw, center: 0, choices: None }, // :112
    Param { key: "part.mw_lfo_fmod", label: "MW LFO FM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x21, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :113
    Param { key: "part.mw_lfo_amod", label: "MW LFO AM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x22, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :114
    Param { key: "part.bend_pitch", label: "PB Pitch", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x23, size: 1, enc: Coding::Byte7, min: 0x28, max: 0x58, special: -1, def: 0x42, how: View::Center, center: 0x40, choices: None }, // :115
    Param { key: "part.bend_filter", label: "PB Filter", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x24, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :116
    Param { key: "part.bend_amp", label: "PB Amp", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x25, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :117
    Param { key: "part.bend_lfo_pmod", label: "PB LFO PM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x26, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :118
    Param { key: "part.bend_lfo_fmod", label: "PB LFO FM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x27, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :119
    Param { key: "part.bend_lfo_amod", label: "PB LFO AM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x28, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :120
    // model.cpp:121-124 — aftertouch 4D-58 / AC1 59-5F / AC2 60-66; firmware
    // clamps out-of-range (pitch 28-58 = +/-24 st, rest 0-127)
    Param { key: "part.cat_pitch", label: "CAT Pitch", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x4d, size: 1, enc: Coding::Byte7, min: 0x28, max: 0x58, special: -1, def: 0x40, how: View::Center, center: 0x40, choices: None }, // :125
    Param { key: "part.cat_filter", label: "CAT Filter", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x4e, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :126
    Param { key: "part.cat_amp", label: "CAT Amp", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x4f, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :127
    Param { key: "part.cat_lfo_pmod", label: "CAT LFO PM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x50, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :128
    Param { key: "part.cat_lfo_fmod", label: "CAT LFO FM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x51, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :129
    Param { key: "part.cat_lfo_amod", label: "CAT LFO AM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x52, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :130
    Param { key: "part.pat_pitch", label: "PAT Pitch", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x53, size: 1, enc: Coding::Byte7, min: 0x28, max: 0x58, special: -1, def: 0x40, how: View::Center, center: 0x40, choices: None }, // :131
    Param { key: "part.pat_filter", label: "PAT Filter", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x54, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :132
    Param { key: "part.pat_amp", label: "PAT Amp", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x55, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :133
    Param { key: "part.pat_lfo_pmod", label: "PAT LFO PM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x56, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :134
    Param { key: "part.pat_lfo_fmod", label: "PAT LFO FM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x57, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :135
    Param { key: "part.pat_lfo_amod", label: "PAT LFO AM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x58, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :136
    Param { key: "part.ac1_cc", label: "AC1 CC No", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x59, size: 1, enc: Coding::Byte7, min: 0, max: 95, special: -1, def: 16, how: View::Raw, center: 0, choices: None }, // :137
    Param { key: "part.ac1_pitch", label: "AC1 Pitch", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x5a, size: 1, enc: Coding::Byte7, min: 0x28, max: 0x58, special: -1, def: 0x40, how: View::Center, center: 0x40, choices: None }, // :138
    Param { key: "part.ac1_filter", label: "AC1 Filter", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x5b, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :139
    Param { key: "part.ac1_amp", label: "AC1 Amp", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x5c, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :140
    Param { key: "part.ac1_lfo_pmod", label: "AC1 LFO PM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x5d, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :141
    Param { key: "part.ac1_lfo_fmod", label: "AC1 LFO FM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x5e, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :142
    Param { key: "part.ac1_lfo_amod", label: "AC1 LFO AM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x5f, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :143
    Param { key: "part.ac2_cc", label: "AC2 CC No", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x60, size: 1, enc: Coding::Byte7, min: 0, max: 95, special: -1, def: 17, how: View::Raw, center: 0, choices: None }, // :144
    Param { key: "part.ac2_pitch", label: "AC2 Pitch", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x61, size: 1, enc: Coding::Byte7, min: 0x28, max: 0x58, special: -1, def: 0x40, how: View::Center, center: 0x40, choices: None }, // :145
    Param { key: "part.ac2_filter", label: "AC2 Filter", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x62, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :146
    Param { key: "part.ac2_amp", label: "AC2 Amp", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x63, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :147
    Param { key: "part.ac2_lfo_pmod", label: "AC2 LFO PM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x64, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :148
    Param { key: "part.ac2_lfo_fmod", label: "AC2 LFO FM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x65, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :149
    Param { key: "part.ac2_lfo_amod", label: "AC2 LFO AM", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x66, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :150
    // model.cpp:151 — velocity window 08 pp 6D-6E
    Param { key: "part.vel_limit_low", label: "Vel Lim Lo", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x6d, size: 1, enc: Coding::Byte7, min: 1, max: 127, special: -1, def: 1, how: View::Raw, center: 0, choices: None }, // :152
    Param { key: "part.vel_limit_high", label: "Vel Lim Hi", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x6e, size: 1, enc: Coding::Byte7, min: 1, max: 127, special: -1, def: 127, how: View::Raw, center: 0, choices: None }, // :153
    // model.cpp:154-155 — portamento & pitch EG 08 pp 67-6C (RAM +0x60)
    Param { key: "part.porta_switch", label: "Porta Sw", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x67, size: 1, enc: Coding::Byte7, min: 0, max: 1, special: -1, def: 0, how: View::Choice, center: 0, choices: Some(&OFF_ON) }, // :156
    Param { key: "part.porta_time", label: "Porta Time", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x68, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 0, how: View::Raw, center: 0, choices: None }, // :157
    Param { key: "part.peg_init_level", label: "PEG Init", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x69, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :158
    Param { key: "part.peg_attack_time", label: "PEG Attack", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x6a, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :159
    Param { key: "part.peg_rel_level", label: "PEG Rel Lv", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x6b, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :160
    Param { key: "part.peg_rel_time", label: "PEG Rel Tm", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x6c, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :161
    // model.cpp:162-163 — HPF is 0A pp 20 (not 08); RAM +0x78
    Param { key: "part.hpf_cutoff", label: "HPF Cutoff", where_: Area::Part, hi: 0x0a, mid: 0, lo: 0x20, size: 1, enc: Coding::Byte7, min: 0, max: 127, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :164
    // model.cpp:165-166 — part EQ 08 pp 72-77; gain kept to the XG +/-12dB
    Param { key: "part.eq_bass_gain", label: "EQ Bass G", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x72, size: 1, enc: Coding::Byte7, min: 52, max: 76, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :167
    Param { key: "part.eq_treble_gain", label: "EQ Treb G", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x73, size: 1, enc: Coding::Byte7, min: 52, max: 76, special: -1, def: 64, how: View::Center, center: 64, choices: None }, // :168
    Param { key: "part.eq_bass_freq", label: "EQ Bass F", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x76, size: 1, enc: Coding::Byte7, min: 4, max: 40, special: -1, def: 12, how: View::Raw, center: 0, choices: None }, // :169
    Param { key: "part.eq_treble_freq", label: "EQ Treb F", where_: Area::Part, hi: 0x08, mid: 0, lo: 0x77, size: 1, enc: Coding::Byte7, min: 28, max: 58, special: -1, def: 54, how: View::Raw, center: 0, choices: None }, // :170
];

/// origin: model.cpp:193 `params()`
pub fn params() -> &'static [Param] {
    TABLE // :193
}

/// origin: model.cpp:195-201 `find`
pub fn find(key: &str) -> Option<&'static Param> {
    for p in TABLE {
        // :197-199
        if key == p.key {
            return Some(p);
        }
    }
    None // :200
}

/// origin: model.h:69 `pack` — fold the 3 7-bit address digits into one number
#[inline]
pub fn pack(hi: u8, mid: u8, lo: u8) -> u32 {
    ((hi as u32) << 14) | ((mid as u32) << 7) | lo as u32
}

/// origin: model.cpp:203-206 `address`
pub fn address(p: &Param, part: i32) -> u32 {
    pack(p.hi, if p.where_ == Area::Part { part as u8 } else { p.mid }, p.lo) // :205
}

/// origin: model.cpp:208-211 `valid`
pub fn valid(p: &Param, value: i32) -> bool {
    (value >= p.min && value <= p.max) || (p.special >= 0 && value == p.special) // :210
}

/// origin: model.cpp:397-400 `applies_on_program` — banks take effect only
/// when the program is written (same as CC0/32 -> PC)
pub fn applies_on_program(p: &Param) -> bool {
    p.where_ == Area::Part && p.hi == 0x08 && (p.lo == 0x01 || p.lo == 0x02) // :399
}

/// origin: model.cpp:213-243 `format`
pub fn format(p: &Param, value: i32) -> String {
    match p.how {
        // :217-219 printf "%d", value+1
        View::Plus1 => format!("{}", value + 1),
        // :220-221 printf "%+d" — Rust {:+} agrees, including "+0"
        View::Center => format!("{:+}", value - p.center),
        // :223-228
        View::Pan => {
            if value == 0 {
                "Rnd".to_string() // :224
            } else if value == 64 {
                "C".to_string() // :225
            } else if value < 64 {
                format!("L{}", 64 - value) // :226
            } else {
                format!("R{}", value - 64) // :227
            }
        }
        // :229-233 — choices run from min; out of range prints the number
        View::Choice => {
            if let Some(c) = p.choices {
                if value >= p.min && value <= p.max {
                    return c[(value - p.min) as usize].to_string(); // :231
                }
            }
            format!("{}", value) // :232
        }
        // :234-237
        View::PartOff => {
            if value == p.special {
                "OFF".to_string() // :235
            } else {
                format!("Part {}", value + 1) // :236
            }
        }
        // :238-240
        View::Raw => format!("{}", value),
    }
}

/// origin: model.cpp:173-179 `checksum`
fn checksum(p: &[u8]) -> u8 {
    let mut sum: u32 = 0; // :175
    for b in p {
        sum += *b as u32; // :176-177 (u32 accum like disk)
    }
    ((0x80 - (sum & 0x7f)) & 0x7f) as u8 // :178
}

/// origin: model.cpp:181-188 `encode` — value -> SysEx data bytes
fn encode(p: &Param, value: i32, out: &mut [u8]) {
    for i in 0..p.size as usize {
        // :184 — disk shifts ints then truncates to u8; i32 shift, mask, cast
        let shift = (p.size as i32 - 1 - i as i32) * if p.enc == Coding::Nibble { 4 } else { 7 }; // :185
        out[i] = ((value >> shift) & if p.enc == Coding::Nibble { 0x0f } else { 0x7f }) as u8; // :186
    }
}

/// origin: model.cpp:246-255 `param_change` — device 0 (disk receives on the
/// firmware default "all")
pub fn param_change(p: &Param, part: i32, value: i32) -> Vec<u8> {
    let a = address(p, part); // :248
    let mut m: Vec<u8> = vec![0xf0, 0x43, 0x10, 0x4c, (a >> 14) as u8, ((a >> 7) & 0x7f) as u8, (a & 0x7f) as u8]; // :249
    let mut data = [0u8; 8]; // :250 u8 data[8] = {}
    encode(p, value, &mut data); // :251
    m.extend_from_slice(&data[..p.size as usize]); // :252 insert(data, data+size)
    m.push(0xf7); // :253
    m
}

/// origin: model.cpp:257-261 `param_request`
pub fn param_request(p: &Param, part: i32) -> Vec<u8> {
    let a = address(p, part); // :259
    vec![0xf0, 0x43, 0x30, 0x4c, (a >> 14) as u8, ((a >> 7) & 0x7f) as u8, (a & 0x7f) as u8, 0xf7] // :260
}

/// origin: model.cpp:263-266 `dump_request`
pub fn dump_request(a: u32) -> Vec<u8> {
    vec![0xf0, 0x43, 0x20, 0x4c, (a >> 14) as u8, ((a >> 7) & 0x7f) as u8, (a & 0x7f) as u8, 0xf7] // :265
}

/// origin: model.h:82 `part_dump_request`
pub fn part_dump_request(part: i32) -> Vec<u8> {
    dump_request(pack(0x08, part as u8, 0)) // :82
}

/// origin: model.h:86-163 `class model` — the MIDI-OUT mirror. Every field
/// explicitly initialized (ledger Invariant 3 == model.h:146-162 defaults).
pub struct Model {
    m_bytes: HashMap<u32, u8>, // :148 address -> 7-bit byte
    m_pinned: HashMap<u32, u64>, // :149 address -> write time (poll clock)
    m_now: u64, // :150 = 0
    m_msg: Vec<u8>, // :151
    m_in: bool, // :152 (uninitialized POD on disk; ctor pins false == feed-idle start)
    m_queue: VecDeque<u32>, // :154
    m_waiting: bool, // :155
    m_wait_addr: u32, // :156 = 0
    m_sent_ms: u64, // :157 = 0
    m_tries: i32, // :158 = 0
    m_accepted: u64, // :160 = 0
    m_rejected: u64, // :160 = 0
    m_edit: Option<Box<dyn FnMut(&Param, i32, i32)>>, // :161 (none installed)
    m_edit_raw: Option<Box<dyn FnMut(u32, i32, i32)>>, // :162 (none installed)
}

/// origin: model.h:146 PIN_MS
const PIN_MS: u64 = 500;

impl Model {
    /// explicit-init ctor (Invariant 3); mirrors model.h:148-162 defaults
    pub fn new() -> Model {
        Model {
            m_bytes: HashMap::new(),
            m_pinned: HashMap::new(),
            m_now: 0, // :150
            m_msg: Vec::new(), // :151
            m_in: false, // :152 — disk leaves it default; feed() (:281) gates on
                         // !m_in for a fresh object and forget() (:477) resets it.
                         // Pinning false here matches every observed disk state.
            m_queue: VecDeque::new(), // :154
            m_waiting: false, // :155
            m_wait_addr: 0, // :156
            m_sent_ms: 0, // :157
            m_tries: 0, // :158
            m_accepted: 0, // :160
            m_rejected: 0, // :160
            m_edit: None, // :161 std::function empty
            m_edit_raw: None, // :162
        }
    }

    /// origin: model.cpp:271-296 `feed`
    pub fn feed(&mut self, b: u8) {
        if b >= 0xf8 {
            return; // :273-274 realtime can interleave
        }
        if b == 0xf0 {
            // :275-280
            self.m_msg.clear(); // :276
            self.m_msg.push(b); // :277
            self.m_in = true; // :278
            return; // :279
        }
        if !self.m_in {
            return; // :281-282
        }
        if b & 0x80 != 0 && b != 0xf7 {
            // :283-287 another message started mid-way: drop
            self.m_in = false; // :284
            self.m_rejected += 1; // :285
            return; // :286
        }
        self.m_msg.push(b); // :288
        if b == 0xf7 {
            // :289-291
            self.m_in = false; // :290
            self.on_sysex(); // :291
        } else if self.m_msg.len() > 8192 {
            // :292-295
            self.m_in = false; // :293
            self.m_rejected += 1; // :294
        }
    }

    /// origin: model.cpp:298-328 `on_sysex`
    fn on_sysex(&mut self) {
        let m = &self.m_msg; // :300
        if m.len() < 8 || m[1] != 0x43 || m[3] != 0x4c {
            // :302-303
            return;
        }
        let kind = m[2] & 0xf0; // :304
        if kind == 0x10 {
            // :305-308 parameter change
            let a = pack(m[4], m[5], m[6]); // :307
            // :308 store(a, m.data() + 7, m.size() - 8) — the F7 terminator
            // is EXCLUDED (size-8, not to-end); phantom-byte trap fixed
            let data = m[7..m.len() - 1].to_vec(); // borrow split: store takes a slice copy
            self.store(a, &data); // :308
            self.m_accepted += 1; // :327
            return;
        } else if kind == 0x00 {
            // :309-323 bulk dump
            if m.len() < 11 {
                return; // :311-312
            }
            let count = ((m[4] as usize) << 7) | m[5] as usize; // :313
            if m.len() != count + 11 {
                // :314-317
                self.m_rejected += 1; // :315
                return; // :316
            }
            // :318 checksum over count+address+data+checksum, low 7 bits 0
            if checksum(&m[4..4 + count + 5]) != m[count + 9] {
                // :319
                self.m_rejected += 1; // :320
                return; // :321
            }
            let a = pack(m[6], m[7], m[8]); // :323
            let data = m[9..9 + count].to_vec(); // borrow split (same bytes)
            self.store(a, &data); // :323
            self.m_accepted += 1; // :327
            return;
        } else {
            // :324-326 requests etc — not ours to read. m_accepted NOT bumped
            return;
        }
    }

    /// origin: model.cpp:330-353 `store`
    fn store(&mut self, addr: u32, data: &[u8]) {
        // :333 hi/mid fixed; the lo digit walks (a dump never crosses lo 0x7f)
        let hi = (addr >> 14) as u8; // :333
        let mid = ((addr >> 7) & 0x7f) as u8; // :333
        for i in 0..data.len() {
            // :334
            let lo = (addr & 0x7f) + i as u32; // :335
            if lo > 0x7f {
                break; // :336-337
            }
            let a = pack(hi, mid, lo as u8); // :338
            if !self.m_pinned.is_empty() {
                // :339
                if let Some(pin) = self.m_pinned.get(&a) {
                    // :340-341
                    if self.m_now - pin < PIN_MS {
                        continue; // :342-343 just written: ours is newer
                    }
                }
                if self.m_pinned.contains_key(&a) {
                    // erase only past the pin window (disk erases then falls
                    // through to write; get-then-erase keeps borrowck clean
                    // with identical semantics — :344)
                    self.m_pinned.remove(&a); // :344
                }
            }
            let v = data[i] & 0x7f; // :347
            self.m_bytes.insert(a, v); // :347
        }
        if self.m_waiting && addr == self.m_wait_addr {
            // :349-352
            self.m_waiting = false; // :350
            self.m_tries = 0; // :351
        }
    }

    /// origin: model.cpp:355-367 `get` — None when never read (no hardcoded
    /// initial values, model.h:93)
    pub fn get(&self, p: &Param, part: i32) -> Option<i32> {
        let a = address(p, part); // :357
        let mut v: i32 = 0; // :358
        for i in 0..p.size as usize {
            // :359
            let it = match self.m_bytes.get(&(a + i as u32)) {
                // :360-362
                Some(v) => *v,
                None => return None, // :361
            };
            v = (v << if p.enc == Coding::Nibble { 4 } else { 7 })
                | (it & if p.enc == Coding::Nibble { 0x0f } else { 0x7f }) as i32; // :363
        }
        Some(v) // :365-366
    }

    /// origin: model.cpp:369-380 `get_raw` — addresses outside the table
    /// (per-type fx params, xg/fx_params.h)
    pub fn get_raw(&self, addr: u32, size: i32) -> Option<i32> {
        let mut v: i32 = 0; // :371
        for i in 0..size {
            // :372
            let it = match self.m_bytes.get(&(addr + i as u32)) {
                // :373-375
                Some(v) => *v,
                None => return None, // :374
            };
            v = v << 7 | (it & 0x7f) as i32; // :376
        }
        Some(v) // :378-379
    }

    /// origin: model.cpp:382-395 `set_raw`
    pub fn set_raw(&mut self, addr: u32, size: i32, value: i32) -> Vec<u8> {
        let mut m: Vec<u8> = vec![
            0xf0,
            0x43,
            0x10,
            0x4c,
            (addr >> 14) as u8,
            ((addr >> 7) & 0x7f) as u8,
            (addr & 0x7f) as u8,
        ]; // :384
        for i in 0..size {
            // :385
            let b = ((value >> (7 * (size - 1 - i))) & 0x7f) as u8; // :386
            self.m_bytes.insert(addr + i as u32, b); // :387
            self.m_pinned.insert(addr + i as u32, self.m_now); // :388
            m.push(b); // :389
        }
        m.push(0xf7); // :391
        if let Some(f) = self.m_edit_raw.as_mut() {
            f(addr, size, value); // :392-393
        }
        m
    }

    /// origin: model.cpp:402-425 `set` — write-through + pin, then the SysEx;
    /// a bank write chains the mirror's program so it lands (bank does not
    /// apply until a program change, model.cpp:414-415)
    pub fn set(&mut self, p: &Param, part: i32, value: i32) -> Vec<u8> {
        let a = address(p, part); // :404
        let mut data = [0u8; 8]; // :405 u8 data[8] = {}
        encode(p, value, &mut data); // :406
        for i in 0..p.size as usize {
            // :407
            self.m_bytes.insert(a + i as u32, data[i]); // :408
            self.m_pinned.insert(a + i as u32, self.m_now); // :409
        }
        let mut out = param_change(p, part, value); // :411
        if let Some(f) = self.m_edit.as_mut() {
            f(p, part, value); // :412-413
        }
        let mut prog: i32 = 0; // :416
        if applies_on_program(p) {
            // :417
            if let Some(pp) = find("part.program") {
                // :418-419
                if let Some(v) = self.get(pp, part) {
                    prog = v;
                    let more = param_change(pp, part, prog); // :420
                    out.extend_from_slice(&more); // :421
                }
            }
        }
        out
    }

    /// origin: model.cpp:427-433 `want_dump` — no duplicate queue entries
    pub fn want_dump(&mut self, addr: u32) {
        if self.m_waiting && self.m_wait_addr == addr {
            // :429-430
            return;
        }
        if !self.m_queue.contains(&addr) {
            // :431
            self.m_queue.push_back(addr); // :432
        }
    }

    /// origin: model.h:125 `want_part`
    pub fn want_part(&mut self, part: i32) {
        self.want_dump(pack(0x08, part as u8, 0)); // :125
    }

    /// origin: model.cpp:435-460 `poll` — at most one outstanding request;
    /// one retry after 400ms (31250 bps blocks are <20ms but the firmware
    /// may be busy). now_ms MUST be the sound-clock (model.h:128-129)
    pub fn poll(&mut self, now_ms: u64) -> Vec<u8> {
        self.m_now = now_ms; // :437
        if self.m_waiting {
            // :440-451
            if now_ms.wrapping_sub(self.m_sent_ms) < 400 {
                // :441 (disk u64 subtraction; poll clock is monotonic per rig)
                return Vec::new(); // :442
            }
            if self.m_tries >= 2 {
                self.m_waiting = false; // :443-444 give up, move on
                self.m_tries = 0; // :445
            } else {
                self.m_sent_ms = now_ms; // :447
                self.m_tries += 1; // :448
                return dump_request(self.m_wait_addr); // :449
            }
        }
        if self.m_queue.is_empty() {
            return Vec::new(); // :452-453
        }
        self.m_wait_addr = self.m_queue.pop_front().unwrap(); // :454-455
        self.m_waiting = true; // :456
        self.m_sent_ms = now_ms; // :457
        self.m_tries = 1; // :458
        dump_request(self.m_wait_addr) // :459
    }

    /// origin: model.h:131 `busy`
    pub fn busy(&self) -> bool {
        self.m_waiting || !self.m_queue.is_empty() // :131
    }

    /// origin: model.h:134 `accepted`
    pub fn accepted(&self) -> u64 {
        self.m_accepted
    }

    /// origin: model.h:135 `rejected` — checksum failures etc.
    pub fn rejected(&self) -> u64 {
        self.m_rejected
    }

    /// origin: model.cpp:462-469 `forget` (one param)
    pub fn forget(&mut self, p: &Param, part: i32) {
        let a = address(p, part); // :464
        for i in 0..p.size as usize {
            // :465
            self.m_bytes.remove(&(a + i as u32)); // :466
            self.m_pinned.remove(&(a + i as u32)); // :467
        }
    }

    /// origin: model.cpp:471-479 `forget` (all — sound card rebooted)
    pub fn forget_all(&mut self) {
        self.m_bytes.clear(); // :473
        self.m_pinned.clear(); // :474
        self.m_queue.clear(); // :475
        self.m_waiting = false; // :476
        self.m_in = false; // :477
        self.m_tries = 0; // :478
    }

    /// origin: model.h:114-121 `load` — bulk copy from work RAM (screen path,
    /// xg/ram.h). Just-written addresses resist overwrite like feed does
    pub fn load(&mut self, addr: u32, data: &[u8], now_ms: u64) {
        self.m_now = now_ms; // :118
        self.store(addr, data); // :119
        self.m_accepted += 1; // :120
    }
}
