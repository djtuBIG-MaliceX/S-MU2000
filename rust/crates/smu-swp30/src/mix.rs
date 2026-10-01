//! Mixer / MELO — transliteration of `src/mame/sound/swp30.cpp:2978-3106`
//! plus the mixer state of `swp30.h:328-331/497-523/560-563`. Phase B adds
//! the per-sample device chain `run_sample` :4179-4271 / `adc_step` :4273-4277
//! / `sample_step` :4304-4375 + the vol/route/internal reg handlers
//! (:2786-2814 / :2836-2869) and the swp30.h:95-96 device accessors.
//! Ground truth: `%TEMP%\mixA` harness (mix.inc = byte-extract of :2978-3106,
//! fc-verified vs disk; g++ -std=c++20 -O3 -mfpmath=sse -msse2), vectors in
//! `tests/data/mix_vectors.txt` (synthetic LCG only, no ROMs); phase B:
//! `%TEMP%\mixB` (32 byte-extract windows, fc /B double-slice proved, real
//! MEG engine compiled in) with `tests/data/mixB_vectors.txt`.
//!
//! Deviations (all zero-observable, ledger-logged):
//! - C++ `m_native` (pointer) is modeled as `native: bool`; `m_native_mask`
//!   lives with `set_native_fx` (phase-B glue) and is not touched by the
//!   mixer functions, so it is not a Mixer field here.
//! - The `m_dbg_dac` fprintf block (swp30.cpp:3098-3105) is omitted: dead
//!   whenever `m_dbg_dac == nullptr` (always, in harness and normal runs).
//! - C++ accessor `melo(i)` (swp30.h:95) is `melo_clamped(i)` — the plain
//!   field owns the `melo` name.
//! - C++ lambdas `raw`/`att` (:2998-2999) are closures here; identical
//!   push order and bounds.
//! - Stale `mix_tap` entries beyond `mix_ntaps` keep old bytes on both sides
//!   (never read: :3074 loops `i != n`).
//! - Phase B: the sampling rec block (:4362-4372) is NOT ported —
//!   `m_wave_access` is only writable through the still-deferred wave arm
//!   (write16 0x10e; deferred_hits gate), so it stays 0 and the block is
//!   inert on every vector and the whole boot. Sampling-RAM row owns it.
//! - `run_sample` returns the DAC pair (C++ out-params :4220-4221); the
//!   meg_jit legs (:4189/4193/4209/4212) are inert here (JIT never built —
//!   same as `meg_jit_run()==false`, the only disk path the MEG row gated).

use crate::fetch::Wave;
use crate::meg::{build_ops, MegState};
use crate::regs::Swp30;

/// origin: swp30.h:94 `SERIAL_FULL_SCALE`
pub const SERIAL_FULL_SCALE: i32 = 1 << 26;

/// origin: swp30.h:328-331 `mixer_slot`
#[derive(Clone, Copy)]
pub struct MixerSlot {
    pub vol: [u16; 3],
    pub route: [u16; 3],
}

/// origin: swp30.h:500-504 `mix_tap`
#[derive(Clone, Copy)]
pub struct MixTap {
    pub dst: u8,
    pub frac: u8,
    pub shift: u8,
}

impl MixTap {
    pub const ZERO: MixTap = MixTap { dst: 0, frac: 0, shift: 0 };
}

impl MixerSlot {
    pub const ZERO: MixerSlot = MixerSlot { vol: [0; 3], route: [0; 3] };
}

/// origin: swp30.h:497-523 + 560-563 mixer members of `swp30_device`
/// (Invariant 3: every field explicitly initialized in [`Mixer::new`],
/// mirroring the in-class `= {}`s / `{ ~u64(0), ~u64(0) }` at :509).
pub struct Mixer {
    /// swp30.h:497 `m_mixer`
    pub mixer: [MixerSlot; 0x80],
    /// swp30.h:505 `m_mix_taps`
    pub mix_taps: [[MixTap; 32]; 0x60],
    /// swp30.h:506 `m_mix_ntaps`
    pub mix_ntaps: [u8; 0x60],
    /// swp30.h:507 `m_mix_active`
    pub mix_active: [u8; 0x60],
    /// swp30.h:508 `m_mix_nactive`
    pub mix_nactive: u8,
    /// swp30.h:509 `m_mix_dirty` — both words start ALL SET
    pub mix_dirty: [u64; 2],
    /// swp30.h:513 `m_melo`
    pub melo: [i32; 0x10],
    /// swp30.h:514 `m_meli`
    pub meli: [i32; 0x10],
    /// swp30.h:563 `m_rec_bus`
    pub rec_bus: i32,
    /// swp30.h:515 `m_adc` (adc_step output; DAC = [0]/[1] per :4220-4221)
    pub adc: [i32; 4],
    /// swp30.h:519 `m_native != nullptr` (deviation: bool, see module doc)
    pub native: bool,
    /// swp30.h:520 `m_native_full`
    pub native_full: bool,
    /// swp30.h:522 `m_nsend`
    pub nsend: [[i32; 2]; 4],
    /// swp30.h:523 `m_ndry`
    pub ndry: [i32; 2],
}

impl Mixer {
    /// Construction == default member init of the mixer fields
    /// (all `= {}` except `m_mix_dirty = { ~0, ~0 }`, swp30.h:509).
    pub fn new() -> Mixer {
        Mixer {
            mixer: [MixerSlot::ZERO; 0x80],
            mix_taps: [[MixTap::ZERO; 32]; 0x60],
            mix_ntaps: [0; 0x60],
            mix_active: [0; 0x60],
            mix_nactive: 0,
            mix_dirty: [!0u64, !0u64], // :509 ~u64(0), ~u64(0)
            melo: [0; 0x10],
            meli: [0; 0x10],
            rec_bus: 0,
            adc: [0; 4],           // :515 `= {}`
            native: false,   // :519 nullptr
            native_full: false, // :520
            nsend: [[0; 2]; 4], // :522
            ndry: [0; 2],       // :523
        }
    }

    /// origin: swp30.h:95 `melo(int)` — clamp to ±SERIAL_FULL_SCALE on read
    pub fn melo_clamped(&self, i: usize) -> i32 {
        self.melo[i].clamp(-SERIAL_FULL_SCALE, SERIAL_FULL_SCALE)
    }

    /// origin: swp30.h:96 `set_meli`
    pub fn set_meli(&mut self, i: usize, v: i32) {
        self.meli[i] = v;
    }

    /// origin: swp30.h:511 `mixer_mark`
    pub fn mixer_mark(&mut self, mix: i32) {
        if mix < 0x60 {
            self.mix_dirty[(mix >> 6) as usize] |= 1u64 << (mix & 63);
        }
    }

    /// origin: swp30.cpp:2978-2987 `mixer_att`
    pub fn mixer_att(sample: i32, att: i32) -> i32 {
        if att >= 0xff {
            // :2980-2981
            return 0;
        }
        // :2985 — S-MU2000: low 4 bits are 1/32 steps (1..17/32); gcc `>>`
        // on i32 is arithmetic, matching Rust's `>>`.
        (sample.wrapping_sub((sample.wrapping_mul(att & 0xf)) >> 5)) >> (att >> 4)
    }

    /// origin: swp30.cpp:2988-3049 `mixer_rebuild`
    pub fn mixer_rebuild(&mut self) {
        // :2990
        for mix in 0..0x60usize {
            // :2991-2992
            if (self.mix_dirty[mix >> 6] >> (mix & 63)) & 1 == 0 {
                continue;
            }
            // :2993
            let route = (u64::from(self.mixer[mix].route[0]) << 32)
                | (u64::from(self.mixer[mix].route[1]) << 16)
                | u64::from(self.mixer[mix].route[2]);
            let vol = self.mixer[mix].vol; // :2994
            let taps = &mut self.mix_taps[mix]; // :2995
            let mut n: usize = 0;
            // :2998 raw (no attenuation == att 0) — macro stand-in for the
            // C++ lambda (two FnMut closures cannot share the taps borrow)
            macro_rules! raw {
                ($dst:expr) => {{
                    taps[n] = MixTap { dst: $dst as u8, frac: 0, shift: 0 };
                    n += 1;
                }};
            }
            // :2999 att (>= 0xff adds nothing -> not in the list)
            macro_rules! att {
                ($dst:expr, $a:expr) => {{
                    let a: u32 = $a;
                    if a < 0xff {
                        taps[n] = MixTap {
                            dst: $dst as u8,
                            frac: (a & 0xf) as u8,
                            shift: (a >> 4) as u8,
                        };
                        n += 1;
                    }
                }};
            }
            // :3000
            for out in 0..16usize {
                // :3001
                let mode = (((route >> (out + 32 - 2)) & 4)
                    | ((route >> (out + 16 - 1)) & 2)
                    | ((route >> (out + 0 - 0)) & 1)) as usize;
                match mode {
                    0 => {} // :3003-3004 no routing
                    // :3006-3009 both channels
                    1 => {
                        raw!(out * 2);
                        raw!(out * 2 + 1);
                    }
                    2 => raw!(out * 2),           // :3011-3013 left
                    3 => raw!(out * 2 + 1),       // :3015-3017 right
                    4 => {                        // :3019-3022 slot 0
                        att!(out * 2, u32::from(vol[0] >> 8) + u32::from(vol[1] >> 8));
                        att!(out * 2 + 1, u32::from(vol[0] & 0xff) + u32::from(vol[1] >> 8));
                    }
                    5 => {                        // :3024-3027 slot 1
                        att!(out * 2, u32::from(vol[0] >> 8) + u32::from(vol[1] & 0xff));
                        att!(out * 2 + 1, u32::from(vol[0] & 0xff) + u32::from(vol[1] & 0xff));
                    }
                    6 => {                        // :3029-3032 slot 2
                        att!(out * 2, u32::from(vol[0] >> 8) + u32::from(vol[2] >> 8));
                        att!(out * 2 + 1, u32::from(vol[0] & 0xff) + u32::from(vol[2] >> 8));
                    }
                    _ => {
                        // :3034-3037 slot 3 (mode 7)
                        att!(out * 2, u32::from(vol[0] >> 8) + u32::from(vol[2] & 0xff));
                        att!(out * 2 + 1, u32::from(vol[0] & 0xff) + u32::from(vol[2] & 0xff));
                    }
                }
            }
            self.mix_ntaps[mix] = n as u8; // :3040
        }
        // :3042
        self.mix_dirty = [0, 0];
        // :3043-3047 compact the active list
        self.mix_nactive = 0;
        for mix in 0..0x60usize {
            if self.mix_ntaps[mix] != 0 {
                self.mix_active[self.mix_nactive as usize] = mix as u8;
                self.mix_nactive += 1;
            }
        }
    }

    /// origin: swp30.cpp:3050-3106 `mixer_step` (`m_meg` -> `meg` parameter;
    /// :3098-3105 debug fprintf block omitted — dead while `m_dbg_dac` is
    /// null, see module doc). NOTE: NEXT's `:3050-3331` range is stale —
    /// the body ends at :3106 on disk; there are no DAC/sample_step arms in
    /// it (`m_rec_bus` is consumed by the sample loop at :4363, phase B).
    pub fn mixer_step(&mut self, samples_per_chan: &[i32; 0x40], meg: &mut MegState) {
        // :3052-3053
        if self.mix_dirty[0] | self.mix_dirty[1] != 0 {
            self.mixer_rebuild();
        }
        // :3055-3056
        let mut mixer_out = [0i32; 0x20];
        // :3058
        for ai in 0..self.mix_nactive as usize {
            let mix = self.mix_active[ai] as usize; // :3059
            let n = self.mix_ntaps[mix] as usize; // :3060
            // :3062-3068 input selection: voice / MEG m20-2f / MELI
            let input = if mix < 0x40 {
                samples_per_chan[mix]
            } else if mix < 0x50 {
                meg.m[0x20 | (mix & 0xf)]
            } else {
                self.meli[mix & 0xf]
            };
            if input == 0 {
                continue; // :3070-3071
            }
            let t = &self.mix_taps[mix]; // :3073
            // :3074-3075 same arithmetic as mixer_att (:3075 comment)
            for i in 0..n {
                mixer_out[t[i].dst as usize] = mixer_out[t[i].dst as usize].wrapping_add(
                    (input.wrapping_sub((input.wrapping_mul(t[i].frac as i32)) >> 5))
                        >> (t[i].shift as u32),
                );
            }
        }
        // :3077 record bus = mixer output 8 left
        self.rec_bus = mixer_out[0x10];
        // :3078 mixer_out 0x00-0x0f -> MELO
        self.melo.copy_from_slice(&mixer_out[0x00..0x10]);
        // :3079 mixer_out 0x10-0x1f -> MEG m20-2f
        meg.m[0x20..0x30].copy_from_slice(&mixer_out[0x10..0x20]);
        // :3080-3097 native mode: steal the effect sends (and dry bus)
        if self.native {
            // :3083 SLOT = reverb, chorus, variation, insertion 1
            const SLOT: [usize; 4] = [0x24, 0x26, 0x2c, 0x28];
            for i in 0..4 {
                self.nsend[i][0] = meg.m[SLOT[i]]; // :3085
                self.nsend[i][1] = meg.m[SLOT[i] + 1]; // :3086
                meg.m[SLOT[i]] = 0; // :3087
                meg.m[SLOT[i] + 1] = 0;
            }
            if self.native_full {
                // :3089-3096 dry bus = m20/m21, then wipe m20-2f
                self.ndry[0] = meg.m[0x20]; // :3092
                self.ndry[1] = meg.m[0x21]; // :3093
                for i in 0x20..0x30 {
                    meg.m[i] = 0; // :3094-3095
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase B — device-side per-sample chain + vol/route/internal handlers.
// These are `swp30_device` members on disk; they live here because this row
// owns them (`regs.rs` keeps only the dispatch arms that call into them).
// `Wave`/`build_ops`/`Swp30` come from the `use` lines at the top.
// ---------------------------------------------------------------------------
impl Swp30 {
    /// origin: swp30.h:95 `melo(int)` — clamped device-side serial output
    pub fn melo(&self, i: usize) -> i32 {
        self.mixer.melo_clamped(i)
    }

    /// origin: swp30.h:96 `set_meli`
    pub fn set_meli(&mut self, i: usize, v: i32) {
        self.mixer.set_meli(i, v)
    }

    /// origin: swp30.cpp:2786-2789 `vol_r<Sel>` (offset = chan << 6 on disk)
    pub fn vol_r(&self, sel: usize, chan: usize) -> u16 {
        self.mixer.mixer[(sel & 0x40) | chan].vol[sel & 3] // :2788
    }

    /// origin: swp30.cpp:2791-2800 `vol_w<Sel>`. The no-change early return
    /// (:2796-2797) is the A/D-part 2000-writes/s quirk (:2793-2794).
    pub fn vol_w(&mut self, sel: usize, chan: usize, data: u16) {
        let idx = (sel & 0x40) | chan; // :2795
        let v = &mut self.mixer.mixer[idx].vol[sel & 3];
        if *v == data {
            return; // :2796-2797
        }
        *v = data; // :2798
        self.mixer.mixer_mark(idx as i32); // :2799
    }

    /// origin: swp30.cpp:2802-2805 `route_r<Sel>`
    pub fn route_r(&self, sel: usize, chan: usize) -> u16 {
        self.mixer.mixer[(sel & 0x40) | chan].route[sel & 3] // :2804
    }

    /// origin: swp30.cpp:2807-2814 `route_w<Sel>`
    pub fn route_w(&mut self, sel: usize, chan: usize, data: u16) {
        let idx = (sel & 0x40) | chan; // :2809
        let r = &mut self.mixer.mixer[idx].route[sel & 3];
        if *r == data {
            return; // :2810-2811
        }
        *r = data; // :2812
        self.mixer.mixer_mark(idx as i32); // :2813
    }

    /// origin: swp30.cpp:2836-2839 `internal_adr_r`
    pub fn internal_adr_r(&self) -> u16 {
        self.internal_adr // :2838
    }

    /// origin: swp30.cpp:2841-2844 `internal_adr_w`
    pub fn internal_adr_w(&mut self, data: u16) {
        self.internal_adr = data; // :2843
    }

    /// origin: swp30.cpp:2846-2869 `internal_r`. The :2866 `logerror` sink
    /// returns 0 (logerror is off in this port, voice-row precedent); the
    /// `return 0x0000` at :2860 is dead after :2856 on disk too.
    pub fn internal_r(&self) -> u16 {
        let chan = (self.internal_adr & 0x3f) as usize; // :2848
        match self.internal_adr >> 8 { // :2849
            0 => self.voices.voices[chan].envelope.status(), // :2851
            // :2853-2856: peg-reached bit 14 (firmware 0x12B81C); low 14
            // bits = current value (unobserved by firmware, still ported)
            4 => {
                let v = &self.voices.voices[chan];
                (if v.peg_reached != 0 { 0x4000 } else { 0 }) | ((v.peg_cur & 0x3fff) as u16)
            }
            6 => 0x8000, // :2862-2863
            _ => 0,      // :2866-2868 (logerror + return 0)
        }
    }

    /// origin: swp30.cpp:4273-4277 `adc_step` — DAC front-end clamp
    /// (arithmetic `>> 4` on i32 == gcc behavior, invariants hold).
    pub fn adc_step(&mut self) {
        for i in 0..4usize { // :4275
            self.mixer.adc[i] = (self.meg.m[0x30 + i] >> 4).clamp(-0x20000, 0x1ffff); // :4276
        }
    }

    /// origin: swp30.cpp:4304-4375 `sample_step`. Omitted blocks: :4313-4322
    /// + :4332-4339 `--dump-dac` fprintf (m_dbg_dac never set), :4327-4328
    /// voice_tap (unported, null), :4341-4349 g_verbose diagnostics;
    /// :4362-4372 rec block deferred to the sampling-RAM row (see module doc —
    /// wave_access is only writable through the deferred wave arm).
    pub fn sample_step(&mut self, wave: &Wave) {
        self.meg_flush_writes(); // :4306
        let mut samples = [0i32; 0x40]; // :4324
        // :4325 — REAL awm2 on a fresh device writes zeros for every inactive
        // voice (matches the harness stub bit-for-bit, mixB_vectors)
        let cnt = self.meg.sample_counter; // :1857 seam (MEG row owns the counter)
        self.voices.awm2_step(
            wave,
            &mut self.rand_seed,
            cnt,
            &mut samples,
            &mut self.dbg_dac,
            self.dbg_chan,
            self.dbg_dac_from,
            self.dbg_dac_count,
        );
        self.adc_step(); // :4350
        self.mixer.mixer_step(&samples, &mut self.meg); // :4351
        self.meg.lfo_step(); // :4352
        self.meg.sample_counter = self.meg.sample_counter.wrapping_add(1); // :4374
    }

    /// origin: swp30.cpp:4179-4271 `run_sample`. Deviations (module doc):
    /// DAC pair returned instead of out-params; JIT legs inert (only the
    /// `!meg_jit_run()` interpret path exists here — the MEG-row-paired
    /// `run_program`); :4197-4199/:4215-4216 meg_tap, :4200 native skip,
    /// :4206-4211 profile wall-clock (invariant 5) and :4261-4270 g_verbose
    /// maxima all unported/no-state; :4202-4205 dbg_meg step-leg ported
    /// (--trace-meg seam, inert unless the bin sets `dbg_meg`).
    pub fn run_sample(&mut self, sintab: &[u16], wave: &Wave) -> (i32, i32) {
        if self.meg_program_changed || self.meg_ops_stale { // :4181
            self.meg.decode_program(); // :4182
            build_ops(&self.meg, &mut self.meg_ops); // :4183
            self.meg_program_changed = false; // :4184
            self.meg_ops_stale = false; // :4185
            // :4189 meg_jit_invalidate() — JIT never built
            self.meg_jit_wait = 1; // :4190
        } else if self.meg_jit_wait != 0 {
            self.meg_jit_wait = self.meg_jit_wait.wrapping_add(1); // :4191 ++
            if self.meg_jit_wait > 64 {
                // :4192-4193 rebuild never happens (no JIT)
                self.meg_jit_wait = 0;
            }
        }
        self.sample_step(wave); // :4196
        if self.dbg_meg.is_some() {
            // :4202-4205 - dbg leg: 384 per-instruction steps so meg_step's
            // trace seam fires (upstream behaviour, incl. local-skip_to)
            for _ in 0..384 {
                self.step(sintab);
            }
        } else {
            self.meg_run_program(sintab); // :4212-4214 (meg_jit_run always false)
        }
        // :4219 DAC = first two of outputs 0-3 (scale 1<<17)
        let left = self.mixer.adc[0]; // :4220
        let right = self.mixer.adc[1]; // :4221
        (left, right)
    }

    /// origin: swp30.cpp:4281-4300 `dump_meg` (--dump-meg program/const/off/
    /// lfo/map/mix dump; text-only diagnostic, C++ `fopen "w"` failure is
    /// silent). Deviation: C++ printf summary at render.cpp:598 omitted like
    /// the other unported render stdout stats.
    pub fn dump_meg(&self, path: &str) {
        use std::io::Write;
        let Ok(mut f) = std::fs::File::create(path) else { return }; // :4284-4286
        for pc in 0..0x180u32 {
            // :4288-4289 prg %03x %016llx %04x
            let _ = writeln!(
                f,
                "prg {:03x} {:016x} {:04x}",
                pc,
                self.meg.program[pc as usize],
                self.meg.konst[pc as usize] as u16
            );
        }
        for i in 0..0x80usize {
            let _ = writeln!(f, "off {:02x} {:04x}", i, self.meg.offset[i]); // :4291
        }
        for i in 0..0x18usize {
            let _ = writeln!(f, "lfo {:02x} {:04x}", i, self.meg.lfo[i]); // :4293
        }
        for i in 0..8usize {
            let _ = writeln!(f, "map {:x} {:04x}", i, self.meg.map[i]); // :4295
        }
        for i in 0..0x60usize {
            // :4297-4300 only routed slots
            let s = &self.mixer.mixer[i];
            if s.route[0] | s.route[1] | s.route[2] != 0 {
                let _ = writeln!(
                    f,
                    "mix {:02x} route {:04x} {:04x} {:04x} vol {:04x} {:04x} {:04x}",
                    i, s.route[0], s.route[1], s.route[2], s.vol[0], s.vol[1], s.vol[2]
                );
            }
        }
    }
}
