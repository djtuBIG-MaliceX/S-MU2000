//! origin: `src/mame/sound/swp30.cpp:4706-4800` — `swp30_device::state`
//! (ledger row M5 `state serializer`, W3b). Wire format = C++ stream bytes:
//! `stdarr` of an element struct is that struct's RAW image (single
//! all-or-nothing raw block), so element serializers are explicit
//! field-order + pad-filler byte writers (`fetch.rs`/`voice.rs`/`mix.rs`/
//! `meg.rs`). Layout ground truth: g++ -std=c++20 -O3 harness
//! `%TEMP%\opencode\stategt\gt.cpp` (data members byte-copied from
//! swp30.h): sizeof streaming_block=52 filter_block=88 iir1_block=28
//! envelope_block=16 lfo_block=16 mixer_slot=12 meg_state::decoded=25
//! meg_state=14872 (m_swp@9600 — nulled on save per :4737-4741, discarded
//! on load), meg_region=32 (only `quiet` rides the v14 leg).
//!
//! Machine seam (:4709): C++ `m_machine` is the CHIP-LOCAL value member
//! (swp30.h:465), never `mu2000`'s; the Rust mirror is the owned
//! [`Swp30::machine`] (`smu_compat::timers::RunningMachine`) — pristine (seed 0x9d14abd7,
//! cycles 0, no timers) on every state path (only verify.cpp:32 ever
//! advances a chip-local seed and that binary never saves state).

use crate::mix::MixerSlot;
use crate::voice::{EnvelopeBlock, FilterBlock, Iir1Block, LfoBlock};
use crate::Swp30;
use smu_compat::StateIo;

/// `s.stdarr` of an element-struct array (state.h stdarr == one raw block of
/// `n * sizeof(T)` bytes, one bounds check, elements untouched on short
/// read). `size` is the harness sizeof; put/get must move exactly `size`
/// bytes per element.
fn stdarr<T>(
    s: &mut StateIo,
    a: &mut [T],
    size: usize,
    put: impl Fn(&T, &mut Vec<u8>),
    get: impl Fn(&mut T, &[u8]),
) {
    if s.writing() {
        let mut b: Vec<u8> = Vec::with_capacity(a.len() * size);
        for x in a.iter() {
            put(x, &mut b);
        }
        debug_assert_eq!(b.len(), a.len() * size);
        s.raw(&mut b);
    } else {
        let mut b = vec![0u8; a.len() * size];
        s.raw(&mut b);
        if s.ok() {
            for (x, c) in a.iter_mut().zip(b.chunks(size)) {
                get(x, c);
            }
        }
    }
}

/// C++ `stdarr` of a scalar array that Rust stores inside the per-voice
/// grid (m_pitch_offset/m_peg_rate/m_peg_cur/m_peg_reached, swp30.h:481-
/// 484): gather into a temp, stream the whole block with `StateIo::arr`
/// (same single raw block, same widths), scatter back only on `ok` —
/// exactly C++'s all-or-nothing.
fn voice_arr<T: smu_compat::state_io::StateScalar + Copy + Default>(
    s: &mut StateIo,
    dev: &mut Swp30,
    getv: impl Fn(&crate::voice::Voice) -> T,
    setv: impl Fn(&mut crate::voice::Voice, T),
) {
    let mut a = [T::default(); 0x40];
    if s.writing() {
        for (i, v) in dev.voices.voices.iter().enumerate() {
            a[i] = getv(v);
        }
    }
    s.arr(&mut a);
    if !s.writing() && s.ok() {
        for (i, v) in dev.voices.voices.iter_mut().enumerate() {
            setv(v, a[i]);
        }
    }
}

impl Swp30 {
    /// origin: swp30.cpp:4706-4800 `swp30_device::state(state_io &s)`.
    /// Not serialized (disk comments :4702-4705): sintab/program/wave
    /// caches (ROM-identical), `m_buf` (one-sample scratch), `m_dbg_*`,
    /// `m_meg_ops` (rebuilt — hence `meg_ops_stale=true` at :4744),
    /// `m_awm_idle` (re-run everything on load — :4778-4780 zeroes it).
    pub fn state(&mut self, s: &mut StateIo) {
        s.tag("swp30"); // :4708
        self.machine.state_sync(s); // :4709 (chip-local machine, swp30.h:465)
        s.v(&mut self.rand_seed); // :4710 (m_rand_seed_base is NOT saved)

        // :4712-4719 reverb RAM (needed to match reverb tails)
        {
            let mut n = self.reverb_ram.len() as u32; // :4714
            s.v(&mut n); // :4715
            if !s.writing() {
                // :4717 resize(n) — Vec::resize value-fills 0 like
                // std::vector::resize; :4718 then streams size()*2 bytes
                self.reverb_ram.resize(n as usize, 0);
            }
            let words = self.reverb_ram.len();
            if s.writing() {
                let mut b: Vec<u8> = Vec::with_capacity(words * 2);
                for &w in self.reverb_ram.iter() {
                    b.extend_from_slice(&w.to_le_bytes());
                }
                s.raw(&mut b); // :4718 s.mem(data, size()*sizeof(u16))
            } else {
                let mut b = vec![0u8; words * 2];
                s.raw(&mut b);
                if s.ok() {
                    for (i, w) in self.reverb_ram.iter_mut().enumerate() {
                        *w = u16::from_le_bytes([b[2 * i], b[2 * i + 1]]);
                    }
                }
            }
        }

        // :4721-4730 per-array raw-element dumps (harness sizes)
        stdarr(
            s,
            &mut self.voices.voices[..],
            crate::fetch::StreamingBlock::STATE_SIZE, // 52
            |v, b| v.streaming.state_bytes(b),
            |v, b| v.streaming.state_load(b),
        ); // :4721 stdarr(m_streaming)
        stdarr(
            s,
            &mut self.voices.voices[..],
            FilterBlock::STATE_SIZE, // 88
            |v, b| v.filter.state_bytes(b),
            |v, b| v.filter.state_load(b),
        ); // :4722
        stdarr(
            s,
            &mut self.voices.voices[..],
            Iir1Block::STATE_SIZE, // 28
            |v, b| v.iir1.state_bytes(b),
            |v, b| v.iir1.state_load(b),
        ); // :4723
        stdarr(
            s,
            &mut self.voices.voices[..],
            EnvelopeBlock::STATE_SIZE, // 16
            |v, b| v.envelope.state_bytes(b),
            |v, b| v.envelope.state_load(b),
        ); // :4724
        stdarr(
            s,
            &mut self.voices.voices[..],
            LfoBlock::STATE_SIZE, // 16
            |v, b| v.lfo.state_bytes(b),
            |v, b| v.lfo.state_load(b),
        ); // :4725
        stdarr(
            s,
            &mut self.mixer.mixer[..],
            MixerSlot::STATE_SIZE, // 12
            |x, b| x.state_bytes(b),
            |x, b| x.state_load(b),
        ); // :4726 stdarr(m_mixer)
        self.mixer.mix_dirty = [!0u64, !0u64]; // :4727 UNCONDITIONAL —
        // the taps are derived state, so after a save the dirty STAYS
        // dirty and after a load everything is rebuilt from vol/route
        s.arr(&mut self.mixer.melo); // :4728 stdarr(m_melo) s32[0x10]
        s.arr(&mut self.mixer.meli); // :4729 stdarr(m_meli) s32[0x10]
        s.arr(&mut self.mixer.adc); // :4730 stdarr(m_adc) s32[4]

        s.tag("meg"); // :4732
        self.meg.state_pod(s); // :4733-4742 (m_swp slot: zeros on save,
        // discarded on load — the Rust machine seam is MegSwp at call time)
        s.v(&mut self.meg_program_changed); // :4743
        if !s.writing() {
            self.meg_ops_stale = true; // :4744-4745 (decoded op table is
            // not saved — rebuild on first run after load)
        }

        s.v(&mut self.sample_counter); // :4747 (dead latch — see field doc)
        s.v(&mut self.wave_adr); // :4748
        s.v(&mut self.wave_size);
        s.v(&mut self.wave_val);
        s.v(&mut self.revram_adr); // :4749
        s.v(&mut self.revram_data);
        s.v(&mut self.wave_access); // :4750
        s.v(&mut self.revram_enable);
        s.v(&mut self.keyon_mask); // :4751 u64
        s.v(&mut self.internal_adr); // :4751 u16

        // v6 (:4752-4759): MEG branch flags + 2nd idx (upstream 29/32)
        if s.version() >= 6 {
            s.v(&mut self.meg_flag_n);
            s.v(&mut self.meg_flag_z);
            s.arr(&mut self.meg_ix2_value); // stdarr s32[3]
            s.arr(&mut self.meg_ix2_act); // stdarr u8[3]
            s.v(&mut self.meg_ram_index2);
        } else if !s.writing() {
            self.meg_flag_n = false;
            self.meg_flag_z = false;
            self.meg_ix2_value = [0; 3];
            self.meg_ix2_act = [0; 3];
            self.meg_ram_index2 = 0;
        }
        // v4 (:4760-4765): sampling record position
        if s.version() >= 4 {
            s.v(&mut self.rec_pos);
            s.v(&mut self.rec_ctrl);
        } else if !s.writing() {
            self.rec_pos = 0;
            self.rec_ctrl = 0;
        }
        // v3 (:4766-4777): pitch EG arrays (slots 0x0B/0x10, current, flag)
        if s.version() >= 3 {
            voice_arr(s, self, |v| v.pitch_offset, |v, x| v.pitch_offset = x);
            voice_arr(s, self, |v| v.peg_rate, |v, x| v.peg_rate = x);
            voice_arr(s, self, |v| v.peg_cur, |v, x| v.peg_cur = x);
            voice_arr(s, self, |v| v.peg_reached, |v, x| v.peg_reached = x);
        } else if !s.writing() {
            for v in self.voices.voices.iter_mut() {
                v.pitch_offset = 0; // v2 states simply have no pitch EG
                v.peg_rate = 0;
                v.peg_cur = 0;
                v.peg_reached = 0;
            }
        }
        // :4778-4780: idle-voice mask is NOT saved — spin every voice on
        // load (harmless: awm2_step just re-stamps the idle ones)
        if !s.writing() {
            self.awm_idle = 0;
        }
        // v14 (:4781-4791): MEG quiet regions (skipped sections + quiet
        // counter) — else the loader would run sections the saver slept
        if s.version() >= 14 {
            s.v(&mut self.meg_skip_mask);
            for g in self.meg_regions.iter_mut() {
                s.v(&mut g.quiet);
            }
        } else if !s.writing() {
            self.meg_skip_mask = 0;
            for g in self.meg_regions.iter_mut() {
                g.quiet = 0;
            }
        }
        // v15 (:4792-4799): rewritten instructions + map (a save taken
        // mid-rewrite must wake the same sections on load)
        if s.version() >= 15 {
            s.arr(&mut self.meg_prg_dirty); // stdarr u64[6]
            s.v(&mut self.meg_map_dirty);
        } else if !s.writing() {
            self.meg_prg_dirty = [0; 6];
            self.meg_map_dirty = true; // NOTE: true, not zeroed (:4798)
        }
    }
}
