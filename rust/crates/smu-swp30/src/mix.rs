//! Mixer / MELO — transliteration of `src/mame/sound/swp30.cpp:2978-3106`
//! plus the mixer state of `swp30.h:328-331/497-523/560-563`. Phase B adds
//! the per-sample device chain `run_sample` :4381-4459 (merged 6.237/6.238:
//! idle-region skip legs :4214-4374 in this file) / `adc_step` :4308-4312
//! / `sample_step` :4339-4410 + the vol/route/internal reg handlers
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
use crate::voice::{dbg_awm_chans, dbg_meg_regs, dbg_send};

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

    /// origin: swp30.cpp:4623-4690 (merged) `sample_step`. Omitted blocks:
    /// voice_tap (unported, null), g_verbose diagnostics;
    /// rec block deferred to the sampling-RAM row (see module doc —
    /// wave_access is only writable through the deferred wave arm).
    /// The three `--dump-dac` fprintf blocks (m/r :4632-4641, awm :4651-4658,
    /// send :3123-3129 — the send dump fires inside mixer_step on disk; here
    /// it reads `meg.m[0x20..0x30]` AFTER `mixer_step`, identical to the disk
    /// local `mixer_out[0x10..0x20]` because :3103 copies that exact window
    /// and the `m_native` steal :3106-3121 never runs in this port) are
    /// ported for the calshort hunt (read-only diagnostics, gated by
    /// `dbg_dac`; stderr-clean on every normal path).
    pub fn sample_step(&mut self, wave: &Wave) {
        self.meg_flush_writes(); // :4625
        // :4632-4641 (merged) --dump-dac: MEG m/r right after flush_writes,
        // BEFORE mixer_step overwrites m[0x20..0x2f] — the effect exit
        if let Some(f) = &mut self.dbg_dac {
            let c = self.meg.sample_counter;
            if c >= self.dbg_dac_from && c < self.dbg_dac_from.wrapping_add(self.dbg_dac_count) {
                dbg_meg_regs(f, c, &self.meg.m, &self.meg.r);
            }
        }
        let mut samples = [0i32; 0x40]; // :4643
        // :4644 — REAL awm2 on a fresh device writes zeros for every inactive
        // voice (matches the harness stub bit-for-bit, mixB_vectors)
        let cnt = self.meg.sample_counter; // :1857 seam (MEG row owns the counter)
        self.voices.awm2_step(
            wave,
            &mut self.rand_seed,
            &mut self.awm_idle,
            cnt,
            &mut samples,
            &mut self.dbg_dac,
            self.dbg_chan,
            self.dbg_dac_from,
            self.dbg_dac_count,
        );
        // :4651-4658 (merged) --dump-dac: per-voice outputs (who is alive)
        if let Some(f) = &mut self.dbg_dac {
            let c = self.meg.sample_counter;
            if c >= self.dbg_dac_from && c < self.dbg_dac_from.wrapping_add(self.dbg_dac_count) {
                dbg_awm_chans(f, c, &samples);
            }
        }
        self.adc_step(); // :4669
        self.mixer.mixer_step(&samples, &mut self.meg); // :4670
        // :3123-3129 --dump-dac send (see module doc: values == disk
        // mixer_out[0x10..0x20]; native steal never runs here)
        if let Some(f) = &mut self.dbg_dac {
            let c = self.meg.sample_counter;
            if c >= self.dbg_dac_from && c < self.dbg_dac_from.wrapping_add(self.dbg_dac_count) {
                let m = &self.meg.m;
                dbg_send(f, c, &m[0x20..0x30]);
            }
        }
        self.meg.lfo_step(); // :4671
        self.meg.sample_counter = self.meg.sample_counter.wrapping_add(1); // :4693
    }

    /// origin: swp30.cpp:4381-4459 `run_sample` (merged 6.237/6.238 chain:
    /// per-region dirty wake, `meg_regions_rebuild(true)` +
    /// `meg_ops_rebuild()`, idle-primed seed skip, `meg_skip_after`).
    /// Deviations (module doc): DAC pair returned instead of out-params;
    /// JIT legs inert (only the `!meg_jit_run()` interpret path exists here
    /// — the MEG-row-paired `run_program`); :4423-4425/:4453-4454 meg_tap,
    /// :4426-4427 native skip, :4438-4443 profile wall-clock (invariant 5),
    /// :4407 `m_mfx_gen++` (native-FX identify — native engine unported,
    /// dead in this port, kept as a comment) and the :4461+ native-FX block
    /// all unported/no-state; :4434-4437 dbg_meg step-leg ported
    /// (--trace-meg seam, inert unless the bin sets `dbg_meg`).
    /// `run_sample_impl` carries the body; the `wave: Option` seam is the
    /// harness leg (meg_b replay drives the real chain without voices/mixer).
    pub fn run_sample(&mut self, sintab: &[u16], wave: &Wave) -> (i32, i32) {
        self.run_sample_impl(sintab, Some(wave), 0, false)
    }

    /// harness entry (MEG row): body EXACTLY as `run_sample`, :4419's
    /// `sample_step(wave)` replaced by its disk-inside legs that the C++
    /// harness stubs the same way (`sample_step` :4306 flush_writes,
    /// :4374 `m_sample_counter += sstep` — the legacy `k && sstep` step
    /// cadence of the pre-merge harness, pinned through `sstep`).
    pub fn run_sample_harness(&mut self, sintab: &[u16], sstep: u32, lfostep: bool) -> (i32, i32) {
        self.run_sample_impl(sintab, None, sstep, lfostep)
    }

    fn run_sample_impl(&mut self, sintab: &[u16], wave: Option<&Wave>, sstep: u32, lfostep: bool) -> (i32, i32) {
        if self.meg_program_changed || self.meg_ops_stale { // :4383
            self.meg.decode_program(); // :4384
            // :4385-4402 (merged 6.238): a program change wakes ONLY the
            // regions holding written words (map change wakes all :4390-91)
            if self.meg_program_changed {
                let mut dirty: u32 = 0; // :4389
                if self.meg_map_dirty {
                    dirty = 0xff; // :4390-4391
                } else {
                    for w in 0..6u32 {
                        // :4393-4395
                        let mut b = self.meg_prg_dirty[w as usize];
                        while b != 0 {
                            let pc = (w * 64 + b.trailing_zeros()) as u16;
                            dirty |= 1 << (self.meg.region_of(pc) as u32 & 7);
                            b &= b - 1;
                        }
                    }
                }
                self.meg_skip_mask &= !dirty; // :4396
                for k in 0..8usize {
                    // :4397-4399
                    if (dirty >> k) & 1 != 0 {
                        self.meg_regions[k].quiet = 0;
                    }
                }
                self.meg_prg_dirty = [0; 6]; // :4400
                self.meg_map_dirty = false; // :4401
            }
            self.meg_regions_rebuild(true); // :4403 (build_ops inside, :4216)
            self.meg_ops_rebuild(); // :4404 (build_ops again, :4263)
            self.meg_program_changed = false; // :4405
            // :4407 `m_mfx_gen++` — native-FX shape re-identify generation.
            // The native engine (src/dsp, meg_fx.h) is NOT ported (module
            // doc / AGENTS.md ignore-list), so the counter has no readers:
            // ported as a no-op citation (dead on every Rust path).
            self.meg_ops_stale = false; // :4408
            // :4412 meg_jit_invalidate() — JIT never built
            self.meg_jit_wait = 1; // :4413
        } else if self.meg_jit_wait != 0 {
            // :4414 `m_meg_jit_wait && ++m_meg_jit_wait > 64` (pre-:4415)
            self.meg_jit_wait = self.meg_jit_wait.wrapping_add(1);
            if self.meg_jit_wait > 64 {
                // :4415-4416 meg_jit_rebuild never happens (no JIT)
                self.meg_jit_wait = 0;
            }
        }
        match wave {
            Some(w) => self.sample_step(w), // :4419
            // harness seam (see run_sample_harness): the three sample_step
            // legs the C++ harness stub keeps (:4341 flush, :4387 lfo_step
            // behind the scenario gate, :4410 counter via scenario cadence)
            None => {
                self.meg_flush_writes();
                if lfostep {
                    self.meg.lfo_step();
                }
                if sstep != 0 {
                    self.meg.sample_counter = self.meg.sample_counter.wrapping_add(sstep);
                }
            }
        }
        // :4420-4422 (merged): sound arrived at an idle region's entrance —
        // restore it BEFORE this sample's MEG run
        if self.meg_skip_mask != 0 {
            self.meg_skip_before();
        }
        // :4423-4425 meg_tap pre-copy — tap unported (always null here)
        let dbg = self.dbg_meg.is_some();
        if self.meg_idle_primed && !dbg {
            // :4428-4433 (merged 6.238): all regions empty and one empty
            // sample was already run — state cannot move anymore; advance
            // only the seed the empty ops would have drawn
            if self.meg_idle_rand != 0 {
                self.rand_skip(self.meg_idle_rand); // :4430-4431
            }
            self.meg.pc = 0; // :4432
            self.meg.icount -= 0x180; // :4433
        } else if dbg {
            // :4434-4437 - dbg leg: 384 per-instruction steps so meg_step's
            // trace seam fires (upstream behaviour, incl. local-skip_to)
            for _ in 0..384 {
                self.step(sintab);
            }
        } else {
            // :4438-4443 profile leg unported (wall-clock, invariant 5);
            // :4444 meg_jit_run always false
            self.meg_run_program(sintab); // :4444-4446
        }
        // :4447-4449 (merged): after one all-empty sample has been run,
        // the following samples take the seed-skip path above
        if !dbg {
            self.meg_idle_primed = self.meg_idle_all;
        }
        // :4450-4452 (merged): count the quieted regions (only when the MEG
        // ran this sample)
        if self.meg_skip_on && !dbg {
            self.meg_skip_after();
        }
        // :4453-4454 meg_tap post-call — unported (null)
        // :4456-4459 DAC = first two of outputs 0-3 (scale 1<<17)
        let left = self.mixer.adc[0]; // :4458
        let right = self.mixer.adc[1]; // :4459
        (left, right)
    }

    /// origin: swp30.cpp:4214-4256 `meg_regions_rebuild` (merged 6.237).
    /// Rebuild the ops table, reset the region bookkeeping (carrying `quiet`
    /// across a state reload of an UNCHANGED program, :4217-4222 —
    /// keep_quiet=TRUE on every merged run_sample call, :4403), then scan
    /// the program for each region's entrances (m20-m2f it reads before it
    /// writes them, :4232-4241) and exits (m20 and above it writes).
    pub fn meg_regions_rebuild(&mut self, keep_quiet: bool) {
        build_ops(&self.meg, &mut self.meg_ops); // :4216
        // :4218-4222 — `g = meg_region{}` then restore quiet if asked
        for g in self.meg_regions.iter_mut() {
            let quiet = if keep_quiet { g.quiet } else { 0 };
            *g = crate::meg::MegRegion::ZERO;
            g.quiet = quiet;
        }
        let mut written = [0u64; 8]; // :4223
        for pc in 0..0x180usize {
            let o = self.meg_ops[pc]; // :4225
            let k = (o.region & 7) as usize; // :4226
            // :4228 — "real" = touches any state beyond the rand seed
            let real = o.alu != 0
                || o.dm != 0
                || o.dr != 0
                || o.memw != 0
                || o.memop != 0
                || o.index != 0
                || o.index2 != 0
                || o.t_write != 0
                || o.jump != 0;
            if !real {
                continue; // :4229-4230
            }
            let g = &mut self.meg_regions[k]; // :4227
            g.used = true; // :4231
            // :4232-4235 — read_m: m20-m2f entrance not yet written by this
            // region itself (lambda over `written[k]` + `g` in C++)
            let read_m = |x: u32, g: &mut crate::meg::MegRegion, written: &[u64; 8]| {
                if x >= 0x20 && x < 0x30 && (written[k] & (1u64 << x)) == 0 {
                    g.in_mask |= 1 << (x - 0x20);
                }
            };
            if o.alu != 0 && (o.mmode == 2 || o.mmode == 3) && o.m2_from_m != 0 && o.sm != 0 {
                // :4236-4237
                read_m(o.sm as u32, g, &written);
            }
            if o.alu != 0 && o.asel == 2 && o.sm != 0 {
                // :4238-4239
                read_m(o.sm as u32, g, &written);
            }
            if o.dm != 0 && o.dm_src == 7 && o.sm != 0 {
                // :4240-4241
                read_m(o.sm as u32, g, &written);
            }
            if o.dm != 0 {
                // :4242-4246
                written[k] |= 1u64 << o.dm;
                if o.dm >= 0x20 {
                    g.out_mask |= 1u64 << o.dm;
                }
            }
        }
        // :4248-4255 — silence before sleep = the region's delay window
        // (2^(10+map size bits)) plus 0.1 s of slack; the
        // SMU2000_MEG_SKIP_DEBUG fprintf (:4252-4254) is a stderr sink,
        // not ported (debug-seam precedent)
        for k in 0..8usize {
            let size = 1u32 << (10 + crate::meg::bit_of(self.meg.map[k], 8, 3)); // :4250
            self.meg_regions[k].hold = size + 4410; // :4251
        }
    }

    /// origin: swp30.cpp:4261-4310 `meg_ops_rebuild` (merged 6.237/6.238).
    /// Rebuild ops, replace sleeping regions' instructions with do-nothing
    /// ops that keep `region` + their rand-draw count (:4271-4281), merge a
    /// run of draws onto the run's LAST op so the seed advances in one
    /// rand_jump (:4282-4298), then scan for all-empty (:4299-4309).
    pub fn meg_ops_rebuild(&mut self) {
        build_ops(&self.meg, &mut self.meg_ops); // :4263
        self.meg_idle_primed = false; // :4264
        if self.meg_skip_mask == 0 {
            // :4265-4268 (nothing sleeps: never all-idle with mask empty)
            self.meg_idle_all = false;
            return;
        }
        for pc in 0..0x180usize {
            // :4270-4281 — the dither draws stay in place and at the same
            // count, so RUNNING regions' streams stay bit-identical
            let o = &mut self.meg_ops[pc];
            if (self.meg_skip_mask >> (o.region & 7)) & 1 != 0 {
                let n = (o.dm != 0 && (o.dm_src == 5 || (o.dm_src == 6 && o.no_noise == 0)))
                    as u8
                    + (o.dr != 0 && o.dr_from_r == 0 && o.no_noise == 0) as u8; // :4274-4275
                let region = o.region; // :4276
                *o = crate::meg::Op::ZERO; // :4277
                o.region = region; // :4278
                o.rand_n = n as u16; // :4279 (C++ u8 widens into u16 rand_n)
            }
        }
        // :4282-4298 — consecutive skips' draws stack onto the run's last
        // op (one mul-chain instead of many; no draws between them anyway)
        let mut carry: u32 = 0; // :4284
        for pc in 0..0x180usize {
            let o = self.meg_ops[pc]; // :4286
            let skipped = (self.meg_skip_mask >> (o.region & 7)) & 1 != 0; // :4287
            if !skipped {
                if carry != 0 {
                    self.meg_ops[pc - 1].rand_n = carry as u16; // :4289-4291
                }
                carry = 0;
                continue;
            }
            carry = carry.wrapping_add(o.rand_n as u32); // :4294
            self.meg_ops[pc].rand_n = 0; // :4295
        }
        if carry != 0 {
            self.meg_ops[0x17f].rand_n = carry as u16; // :4297-4298
        }
        // :4299-4309 — anything real left besides the sleeping regions?
        let mut idle_all = true; // :4300
        let mut idle_rand: u32 = 0; // :4301
        for o in self.meg_ops.iter() {
            if o.alu != 0
                || o.dm != 0
                || o.dr != 0
                || o.memw != 0
                || o.memop != 0
                || o.index != 0
                || o.index2 != 0
                || o.t_write != 0
                || o.jump != 0
            {
                // :4303-4306
                idle_all = false;
                break;
            }
            idle_rand = idle_rand.wrapping_add(o.rand_n as u32); // :4307
        }
        self.meg_idle_all = idle_all;
        self.meg_idle_rand = idle_rand;
        self.meg_idle_primed = false; // :4309
    }

    /// origin: swp30.cpp:4312-4337 `meg_skip_before` (merged 6.237).
    /// After sample_step, before the MEG run: sound reached a sleeping
    /// region's entrance -> wake it from THIS sample.
    pub fn meg_skip_before(&mut self) {
        let mut wake: u32 = 0; // :4314
        for k in 0..8usize {
            if (self.meg_skip_mask >> k) & 1 == 0 {
                continue; // :4316-4317
            }
            let mut inp = self.meg_regions[k].in_mask; // :4318
            while inp != 0 {
                if self.meg.m[0x20 + inp.trailing_zeros() as usize] != 0 {
                    // :4319-4322
                    wake |= 1 << k;
                    break;
                }
                inp &= inp - 1;
            }
        }
        if wake == 0 {
            return; // :4324-4325
        }
        self.meg_skip_mask &= !wake; // :4326
        // :4327-4328 meg-skip wake stderr sink (SMU2000_MEG_SKIP_DEBUG) —
        // debug sink, not ported (stderr, no state)
        for k in 0..8usize {
            // :4329-4331
            if (wake >> k) & 1 != 0 {
                self.meg_regions[k].quiet = 0;
            }
        }
        self.meg_ops_rebuild(); // :4332
        // :4333-4335 meg_jit_invalidate() — JIT never built (interpret runs
        // until writes settle, same result as the ops table)
        self.meg_jit_wait = 1; // :4336
    }

    /// origin: swp30.cpp:4339-4374 `meg_skip_after` (merged 6.237).
    /// After the MEG run: count samples with entrance+exit (and the
    /// still-pending 3-cycle delayed writes, :4353-4358) all zero.
    pub fn meg_skip_after(&mut self) {
        let mut add: u32 = 0; // :4341
        for k in 0..8usize {
            // :4342-4345 — only USED regions WITH entrances, not sleeping
            if !self.meg_regions[k].used
                || self.meg_regions[k].in_mask == 0
                || (self.meg_skip_mask >> k) & 1 != 0
            {
                continue;
            }
            let mut silent = true; // :4346
            let mut inp = self.meg_regions[k].in_mask; // :4347
            while inp != 0 && silent {
                if self.meg.m[0x20 + inp.trailing_zeros() as usize] != 0 {
                    silent = false;
                }
                inp &= inp - 1;
            }
            let mut out = self.meg_regions[k].out_mask; // :4350
            while out != 0 && silent {
                if self.meg.m[out.trailing_zeros() as usize] != 0 {
                    silent = false;
                }
                out &= out - 1;
            }
            // :4353-4358 — writes still in the 3-instruction delay ring
            for d in 0..3usize {
                if !silent {
                    break;
                }
                let x = self.meg.mw_reg[d];
                if x != 0
                    && (self.meg_regions[k].out_mask >> x) & 1 != 0
                    && self.meg.mw_value[d] != 0
                {
                    silent = false;
                }
            }
            if !silent {
                // :4359-4362
                self.meg_regions[k].quiet = 0;
                continue;
            }
            // :4363-4364 — `++g.quiet >= g.hold` (u32 wrap-safe; quiet
            // stops being counted the sample after the region sleeps)
            self.meg_regions[k].quiet = self.meg_regions[k].quiet.wrapping_add(1);
            if self.meg_regions[k].quiet >= self.meg_regions[k].hold {
                add |= 1 << k;
            }
        }
        if add == 0 {
            return; // :4366-4367
        }
        self.meg_skip_mask |= add; // :4368
        // :4369-4370 meg-skip quiet stderr sink (debug) — not ported
        self.meg_ops_rebuild(); // :4371
        // :4372 meg_jit_invalidate() — JIT never built
        self.meg_jit_wait = 1; // :4373
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

// ---- M5-W3b state serializer (origin: swp30.cpp:4726 s.stdarr(m_mixer)) ----
// Harness ground truth %TEMP%\opencode\stategt\gt.cpp (swp30.h:344-345
// byte-copied): sizeof(mixer_slot)==12, vol@0 route@6 — no padding.
impl MixerSlot {
    /// harness `sizeof(mixer_slot)` == 12
    pub const STATE_SIZE: usize = 12;

    pub fn state_bytes(&self, out: &mut Vec<u8>) {
        for x in self.vol {
            out.extend_from_slice(&x.to_le_bytes()); // std::array<u16,3> vol @0
        }
        for x in self.route {
            out.extend_from_slice(&x.to_le_bytes()); // std::array<u16,3> route @6
        }
    }

    pub fn state_load(&mut self, b: &[u8]) {
        for i in 0..3 {
            self.vol[i] = u16::from_le_bytes(b[i * 2..][..2].try_into().unwrap());
        }
        for i in 0..3 {
            self.route[i] = u16::from_le_bytes(b[6 + i * 2..][..2].try_into().unwrap());
        }
    }
}
