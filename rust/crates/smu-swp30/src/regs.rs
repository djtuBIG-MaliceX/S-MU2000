//! origin: src/mame/sound/swp30.cpp — reg-dispatch core (`read16` :2007-2106,
//! `write16` :2108-2218) + the trivial control/status latches boot touches
//! (`revram_status_r` :2495, `revram_enable_w` :2458, `keyon_r` :2231,
//! `snd_w` :2874). Ledger row `reg dispatch` (M3, smu-swp30).
//!
//! DEFERRING DISCIPLINE (the MEG-empty-array killer): the full slot/channel
//! grid is ROUTED COMPLETE (both switches, every case). Since the
//! `voice engine` row the voice-owned slots (filter/lfo/EG/peg/pitch/
//! start/loop/address/iir1 + keyon_w + keyon_mask) and since the
//! `mixer/MELO phase B` row the vol/route/internal slots are ported FOR REAL;
//! every remaining slot is MEG/wave territory — a `deferred`
//! fall-through that bumps `deferred_hits` and records `last_deferred_*` so
//! a stray boot access becomes that row's problem instead of silently
//! corrupting audio. The `--trace-swp` boot gate requires
//! `deferred_hits == 0` over 28M cycles.
//!
//! Disk-corrected facts vs the ledger row hint:
//! - slot = `addr & 0x3f`, chan = `(addr >> 6) & 0x3f` (swp30.cpp:2010-2011)
//!   — CONFIRMED.
//! - The hint formula `rctrl = 0x40*(idx>>1)|0xe|(idx&1)` has **0 hits on
//!   disk** (rg over swp30.cpp): PHANTOM. The real slot/chan addressing is
//!   the grid above; every boot access to slot 0x0e/0x0f falls through both
//!   switches to `snd_w` (write) / `return 0` basin (read), except the one
//!   read of addr 0x084e -> `revram_status_r()`.

/// origin: mu2000.cpp:861-866 (base passed to the r16/w16/w32 lambdas).
pub const MASTER_BASE: u32 = 0x0080_0000;
/// origin: mu2000.cpp:922 (slave base).
pub const SLAVE_BASE: u32 = 0x0080_2000;

use crate::meg::MegState;
use crate::mix::{Mixer, MixerSlot};
use crate::voice::{sext14, Channels, RAND_SEED_INIT};
use smu_compat::timers::RunningMachine;

/// SWP30 register-dispatch device. Voice state lives in `voices` (row
/// `voice engine`); MEG and mixer/serial state live in `meg`/`mixer`
/// (MEG + mixer rows). Invariant 3: every field is explicitly initialised and
/// `reset()` mirrors the swp30_device::reset :1958-2000 order for the
/// fields present here.
pub struct Swp30 {
    /// origin: swp30.h:451 m_rand_seed / m_rand_seed_base (0x9d14abd7)
    pub rand_seed: u32,
    pub rand_seed_base: u32,
    /// origin: swp30.h:465 `running_machine m_machine` — the CHIP-LOCAL
    /// compat machine (value member, NOT mu2000's; h:62-78 "chip per
    /// sequence"). Only ever touched at swp30.cpp:4709 (`state_sync`) and
    /// verify.cpp:32 (`machine().rand`, verify binary only — never saves
    /// state). On every state path it is pristine: seed 0x9d14abd7,
    /// cycles 0, zero timers (mamecompat.h:715-717 — the defaults
    /// `RunningMachine::new` reproduces). The chip LCG is `rand_seed`
    /// above, never this machine's.
    pub machine: RunningMachine,
    /// the 64-voice AWM2 grid (m_streaming/m_filter/m_iir1/m_envelope/m_lfo +
    /// m_pitch_offset/m_peg_rate/m_peg_cur/m_peg_reached, swp30.h:460-468)
    pub voices: Channels,
    /// origin: swp30.cpp:1992 `m_revram_enable = 0` (read16 case 0x80e).
    pub revram_enable: u16,
    /// origin: swp30.cpp:1986-1991 wave/revram address+data latches (reset 0).
    /// Ported as plain latches so the deferred write arms that only store can
    /// stay honest; boot never writes them (verified against golden).
    pub revram_data: u32, // :1991 m_revram_data
    pub rec_ctrl: u16,    // write16 case 0x30e `m_rec_ctrl = data` (:2187)
    /// origin: swp30.h:619 `m_rec_pos` (v4 state leg :4762). No writer in
    /// this port yet — the rec block (`m_rec_pos++` :4686) is deferred to
    /// the sampling-RAM row (mix.rs module doc). NOT cleared by device
    /// reset (no :1958-2000 write on disk).
    pub rec_pos: u32,
    /// origin: swp30.h:624 `m_sample_counter`. DEAD since the merge: no
    /// write anywhere post-construction (device reset :1965-2009 omits it;
    /// the :1961 zero is meg_state::reset touching MEG's own counter);
    /// every live user reads `m_meg->m_sample_counter` (:1860/:2611 — Rust
    /// `meg.sample_counter`). Materialised anyway because the state stream
    /// carries this slot (:4747) — keeps bytes cross-loadable with C++.
    pub sample_counter: u32,
    /// origin: swp30.h:625-626 `m_wave_adr/m_wave_size/m_wave_val/
    /// m_wave_access`. The wave-access arms are still deferred (wave-access
    /// row) so these latches stay 0 (boot golden writes none); reset zeroes
    /// them :1986-1989; state legs :4748-4750.
    pub wave_adr: u32,
    pub wave_size: u32,
    pub wave_val: u32,
    pub wave_access: u16,
    pub keyon_mask: u64,  // keyon_mask_w r/w64 (:2221-2229), reset 0 (:1968)
    /// origin: swp30.h:629-631 `m_awm_idle` (6.239 idle-voice skip): voices
    /// that are envelope-inactive AND peg-arrived; `awm2_step` (:1838) does
    /// not iterate them. Cleared per-channel by EVERY `write16` (:2122, incl.
    /// the snd_w basin) and by `keyon_w` for the whole mask (:2248);
    /// reset zeroes all (:1969). State load ALSO zeroes it (disk :4778-4780)
    /// — deferred M5 (state.rs unported; re-running idle voices on load is
    /// a proven no-op, awm2_step just re-stamps them).
    pub awm_idle: u64,

    // ---- MEG row (phase A) ----
    /// the meg_state core (swp30.h:333-446; swp30.h:526-527 unique_ptr+ptr
    /// collapse to an owned field)
    pub meg: MegState,
    /// origin: swp30.h:528 m_meg_program_changed. ctor sets it TRUE (:1893);
    /// meg_prg_w (:2323) / meg_map_w (:2335) set it; the run loop clears it
    /// (phase B, :4181-4184). NOT written by device reset (:1958-2000).
    pub meg_program_changed: bool,
    /// origin: swp30.h:556 m_meg_const_gen (default 0; no reset write —
    /// consts deliberately SURVIVE reset, mirrors disk). Bumped by const_w.
    pub meg_const_gen: u32,
    /// origin: swp30.h:608 m_meg_off_gen (merged; default 0, no reset write;
    /// bumped change-gated by offset_w :3390, read only by the unported
    /// native-FX block :4490 — dead here, ported for fidelity).
    pub meg_off_gen: u32,
    /// origin: swp30.h:567 m_meg_jit_wait; revram_enable_w sets 1 (:2467).
    /// The JIT rebuild consumer is inert in this port (JIT never built),
    /// kept as an explicit latch so the write side stays transliterated.
    pub meg_jit_wait: u32,
    /// origin: swp30.h:469 `std::vector<u16> m_reverb_ram` (18bit space).
    /// 1<<18 words, zeroed at device_start (:1902 `assign`); device reset
    /// does NOT clear it (:1958-2000) — only revram_clear_w regions (:2484).
    /// Vec (not Box<[u16;N]>) so the state load can mirror `resize(n)`
    /// verbatim (swp30.cpp:4717).
    pub reverb_ram: Vec<u16>,
    pub revram_adr: u32,

    // ---- MEG row (phase B) — device-side meg glue, swp30.h:534-540 ----
    /// origin: swp30.h:534 m_meg_flag_n / m_meg_flag_z (step :3766-3767
    /// writes, :3658/:3695 reads; reset :1962).
    pub meg_flag_n: bool,
    pub meg_flag_z: bool,
    /// origin: swp30.h:537-538 m_meg_ix2_value / m_meg_ix2_act (second index
    /// delay line, upstream 32; step :3624-3625/:3817-3819; reset :1963).
    pub meg_ix2_value: [i32; 3],
    pub meg_ix2_act: [u8; 3],
    /// origin: swp30.h:539 m_meg_ram_index2 (reset :1963).
    pub meg_ram_index2: i32,
    /// origin: swp30.h:540 m_meg_skip_to. In-class 0; NOT written by device
    /// reset :1958-2000 (disk-verified) — survives reset on disk too.
    pub meg_skip_to: u16,
    /// origin: swp30.h:497-523 + 560-563 mixer/serial state (mixer row;
    /// meli/melo/adc/rec_bus/taps/dirty live in [`Mixer`])
    pub mixer: Mixer,
    /// origin: swp30.h:574 m_internal_adr (in-class 0; NO reset write on disk
    /// :1958-2000 — survives device reset, like C++)
    pub internal_adr: u16,
    /// origin: swp30.h:530 m_meg_ops (build_ops output; NOT state per
    /// swp30.h:355-356). `= {}` in-class zeros -> Op::ZERO pattern.
    pub meg_ops: Box<[crate::meg::Op; 0x180]>,
    /// origin: swp30.h:531 m_meg_ops_stale = true. Cleared by the run loop
    /// after decode+build (:4408 merged; :4185 pre-merge); set again on state
    /// load (:4743 region — deferred to M5 state.rs).
    pub meg_ops_stale: bool,

    // ---- MEG idle-region skip row (merged 6.237/6.238) ----
    /// origin: swp30.h:561 m_meg_regions `= {}` (struct `MegRegion`
    /// swp30.h:554-560, mirrored in [`crate::meg::MegRegion`])
    pub meg_regions: [crate::meg::MegRegion; 8],
    /// origin: swp30.h:562 m_meg_skip_mask = 0 (bit = sleeping region)
    pub meg_skip_mask: u32,
    /// origin: swp30.h:563 m_meg_skip_on = true; ctor reads
    /// SMU2000_MEG_SKIP EXACTLY ONCE (:1898-1899, `e[0] != '0'`).
    pub meg_skip_on: bool,
    /// origin: swp30.h:564 m_meg_skip_debug (ctor :1900
    /// SMU2000_MEG_SKIP_DEBUG != nullptr). Only stderr prints gated.
    pub meg_skip_debug: bool,
    /// origin: swp30.h:567 m_meg_prg_dirty `= {}` (bit = program address,
    /// 6 words x 64 = 0x180). Set by meg_prg_w (:2343), drained by the
    /// run_sample rebuild block (:4393-4400). NOT reset by device reset.
    pub meg_prg_dirty: [u64; 6],
    /// origin: swp30.h:568 m_meg_map_dirty = TRUE (in-class default; a
    /// fresh device's first rebuild wakes ALL regions :4390-4391)
    pub meg_map_dirty: bool,
    /// origin: swp30.h:571 m_meg_idle_all (set by meg_ops_rebuild :4300-4308)
    pub meg_idle_all: bool,
    /// origin: swp30.h:572 m_meg_idle_primed (run_sample :4428/4449; an
    /// all-empty sample was run through, from now on only the seed moves)
    pub meg_idle_primed: bool,
    /// origin: swp30.h:573 m_meg_idle_rand (draws one idle sample owes the
    /// seed; run_sample :4430-4431 rand_skip's exactly this)
    pub meg_idle_rand: u32,

    // ---- debug seam (render.cpp --dump-dac, swp30.h:127-129) ----
    /// origin: swp30.h:127 `std::FILE *m_dbg_dac` — per-stage voice dump sink
    /// (awm2_step :1859-1876 + mixer_step :3098-3105); None = off.
    pub dbg_dac: Option<std::fs::File>,
    /// origin: swp30.h:128 `int m_dbg_chan = -1`
    pub dbg_chan: i32,
    /// origin: swp30.h:129 `m_dbg_dac_from / m_dbg_dac_count`
    pub dbg_dac_from: u32,
    pub dbg_dac_count: u32,
    /// origin: swp30.h:110-112 `m_dbg_meg / m_dbg_meg_from / _count / _pc0 /
    /// _pc1` — per-instruction MEG trace sink (render.cpp --trace-meg,
    /// meg_step :3873-3879 + run_sample step-leg :4202-4205); None = off.
    pub dbg_meg: Option<std::fs::File>,
    pub dbg_meg_from: u32,
    pub dbg_meg_count: u32,
    pub dbg_meg_pc0: u16,
    pub dbg_meg_pc1: u16,

    // ---- deferral diagnostics (row gate) ----
    /// Count of accesses routed into a not-yet-ported handler. MUST be 0 after
    /// the 28M-cycle boot (row gate). Never reset by traffic; only `reset()`.
    pub deferred_hits: u64,
    /// addr & 0xfff of the most recent deferred slot (0 = none yet).
    pub last_deferred_addr: u32,
    /// true if the last deferred access was a write.
    pub last_deferred_write: bool,
}

impl Swp30 {
    /// Construction mirrors the C++ ctor swp30.cpp:1887-1910: default-init
    /// members, then `reset()` (:1909) — so a fresh device already carries
    /// the cleared voice state (streaming loop_size=0x400, envelope 0x3fff/
    /// RELEASE, ...). Every field explicit (invariant 3).
    pub fn new() -> Swp30 {
        let mut s = Swp30 {
            rand_seed: RAND_SEED_INIT, // swp30.h:451
            rand_seed_base: RAND_SEED_INIT, // swp30.h:451
            machine: RunningMachine::new(), // swp30.h:465 (chip-local value member; mamecompat :715-717 defaults — never touched on machine paths)
            voices: Channels::new(), // in-class zeros; reset() below applies clear()
            revram_enable: 0, // swp30.cpp:1992 (reset) — ctor also 0 (no init = zero)
            revram_data: 0,   // :1991
            rec_ctrl: 0,      // m_rec_ctrl default 0
            rec_pos: 0,       // swp30.h:619 in-class 0
            sample_counter: 0, // swp30.h:624 in-class 0 (dead latch, see field doc)
            wave_adr: 0,      // swp30.h:625
            wave_size: 0,     // swp30.h:625
            wave_val: 0,      // swp30.h:625
            wave_access: 0,   // swp30.h:626
            keyon_mask: 0,    // :1968
            awm_idle: 0,      // swp30.h:631 in-class 0 + reset :1969
            meg: MegState::new(), // :1889-1892 (in-class zeros; reset() below)
            meg_program_changed: false, // ctor true (:1893) — set AFTER reset()
            meg_const_gen: 0, // swp30.h:556 default 0
            meg_off_gen: 0,   // swp30.h:608 default 0 (merged)
            meg_jit_wait: 0,  // swp30.h:567 default 0
            reverb_ram: vec![0u16; 0x40000], // :1902 assign(1<<18, 0)
            revram_adr: 0,    // :1990 (ctor default 0)
            meg_flag_n: false, // swp30.h:534 in-class false
            meg_flag_z: false, // :534
            meg_ix2_value: [0; 3], // swp30.h:537 `= {}`
            meg_ix2_act: [0; 3], // swp30.h:538 `= {}`
            meg_ram_index2: 0, // swp30.h:539
            meg_skip_to: 0, // swp30.h:540
            meg_ops: Box::new([crate::meg::Op::ZERO; 0x180]), // swp30.h:546 `= {}`
            meg_ops_stale: true, // swp30.h:547
            meg_regions: [crate::meg::MegRegion::ZERO; 8], // swp30.h:561 `= {}`
            meg_skip_mask: 0,    // swp30.h:562
            meg_skip_on: true,   // swp30.h:563 (env override below, ctor :1898-1899)
            meg_skip_debug: false, // swp30.h:564 (env override below, :1900)
            meg_prg_dirty: [0; 6], // swp30.h:567 `= {}`
            meg_map_dirty: true, // swp30.h:568 in-class TRUE
            meg_idle_all: false, // swp30.h:571
            meg_idle_primed: false, // swp30.h:572
            meg_idle_rand: 0,    // swp30.h:573
            mixer: Mixer::new(), // swp30.h:497-523/560-563 `= {}` (dirty ~0)
            internal_adr: 0,     // swp30.h:574
            dbg_dac: None,       // swp30.h:127 nullptr
            dbg_chan: -1,        // swp30.h:128
            dbg_dac_from: 0,     // swp30.h:129
            dbg_dac_count: 0,    // swp30.h:129
            dbg_meg: None,       // swp30.h:110 nullptr
            dbg_meg_from: 0,     // swp30.h:111
            dbg_meg_count: 0,    // swp30.h:111
            dbg_meg_pc0: 0,      // swp30.h:112
            dbg_meg_pc1: 0x180,  // swp30.h:112
            deferred_hits: 0,
            last_deferred_addr: 0,
            last_deferred_write: false,
        };
        // merged 6.237 (ctor :1897-1900): the idle-region skip env is read
        // EXACTLY here, never per sample ("e[0] != '0'": first byte; an
        // empty string is true). Env names EXACT: SMU2000_MEG_SKIP and
        // SMU2000_MEG_SKIP_DEBUG (invariant 1: identical env on both sides).
        if let Ok(e) = std::env::var("SMU2000_MEG_SKIP") {
            // :1898-1899
            s.meg_skip_on = e.as_bytes().first().map_or(true, |c| *c != b'0');
        }
        s.meg_skip_debug = std::env::var("SMU2000_MEG_SKIP_DEBUG").is_ok(); // :1900
        s.reset(); // swp30.cpp:1909 (ctor calls reset())
        s.meg_program_changed = true; // :1896 (meg->reset() at :1892, flag after)
        s
    }

    /// origin: swp30_device::reset :1958-2000 — exact order for the fields
    /// this device models. m_meg_flag/ix2 (:1962-63) land here (MEG phase B);
    /// mixer+mix_dirty (:1966-67) and nsend/meli/melo/adc (:1994-99) land
    /// here with mixer row B. Wave latches (:1994-97) ARE zeroed here
    /// (storage only — the write arms stay deferred, wave-access row).
    pub fn reset(&mut self) {
        self.rand_seed = self.rand_seed_base; // :1967
        self.keyon_mask = 0; // :1968
        self.awm_idle = 0; // :1969 (merged: every voice re-runs after reset)
        self.meg_flag_n = false; // :1962
        self.meg_flag_z = false; // :1962
        self.meg_ix2_value = [0; 3]; // :1963 fill(0)
        self.meg_ix2_act = [0; 3]; // :1963 fill(0)
        self.meg_ram_index2 = 0; // :1963
        // (m_meg_skip_to has NO reset write on disk — survives, like C++)
        self.mixer.mixer = [MixerSlot::ZERO; 0x80]; // :1966 fill(mixer_slot())
        self.mixer.mix_dirty = [!0u64, !0u64]; // :1967
        self.voices.clear_all(); // :1969-1982 (exact per-array order)
        self.meg.reset(); // :1992 (MEG row — exact order meg_state::reset :1924-1963)
        self.wave_adr = 0; // :1994 (arms deferred, but reset zeroes for real)
        self.wave_size = 0; // :1995
        self.wave_access = 0; // :1996
        self.wave_val = 0; // :1997
        self.revram_adr = 0; // :1998
        self.revram_data = 0; // :1999
        self.revram_enable = 0; // :2000
        for s in self.mixer.nsend.iter_mut() {
            s[0] = 0; // :1994-1995
            s[1] = 0;
        }
        self.mixer.meli = [0; 0x10]; // :1997
        self.mixer.melo = [0; 0x10]; // :1998
        self.mixer.adc = [0; 4]; // :1999
        // m_rec_bus: NO reset write on disk (:1958-2000) — survives, like C++
        // m_rec_ctrl (:2187 write latch) has NO reset write on disk — left as-is.
        // m_meg_program_changed/const_gen/jit_wait: NO reset writes on disk.
        // MERGED-verified (scan of :1958-2000 after upstream 6.237/6.238):
        // meg_regions/meg_skip_mask/meg_prg_dirty/meg_map_dirty/meg_idle_*
        // also have NO reset writes — they SURVIVE device reset on disk.
        // m_reverb_ram: NOT cleared by reset (only revram_clear_w regions :2484).
        self.deferred_hits = 0;
        self.last_deferred_addr = 0;
        self.last_deferred_write = false;
    }

    /// origin: meg_state::step (swp30.cpp:3609) as reached from the device —
    /// builds the `MegSwp` seam from disjoint device fields (`m_swp->` on
    /// disk) and runs one instruction. Phase B row.
    pub fn step(&mut self, sintab: &[u16]) {
        let mut seam = crate::meg::MegSwp {
            flag_n: &mut self.meg_flag_n,
            flag_z: &mut self.meg_flag_z,
            ix2_value: &mut self.meg_ix2_value,
            ix2_act: &mut self.meg_ix2_act,
            ram_index2: &mut self.meg_ram_index2,
            skip_to: &mut self.meg_skip_to,
            revram_enable: self.revram_enable,
            reverb_ram: &mut self.reverb_ram,
            seed: &mut self.rand_seed,
            sintab,
        };
        crate::meg::meg_step(
            &mut self.meg,
            &mut seam,
            &mut self.dbg_meg,
            self.dbg_meg_pc0,
            self.dbg_meg_pc1,
            self.dbg_meg_from,
            self.dbg_meg_count,
        );
    }

    /// origin: meg_state::flush_writes (swp30.cpp:3906) device shim.
    pub fn meg_flush_writes(&mut self) {
        let mut seam = crate::meg::MegSwp {
            flag_n: &mut self.meg_flag_n,
            flag_z: &mut self.meg_flag_z,
            ix2_value: &mut self.meg_ix2_value,
            ix2_act: &mut self.meg_ix2_act,
            ram_index2: &mut self.meg_ram_index2,
            skip_to: &mut self.meg_skip_to,
            revram_enable: self.revram_enable,
            reverb_ram: &mut self.reverb_ram,
            seed: &mut self.rand_seed,
            sintab: &[],
        };
        self.meg.flush_writes(&mut seam);
    }

    /// origin: meg_state::run_program (swp30.cpp:3982) device shim — the
    /// `m_swp->` seams assembled like `step()` above; runs one sample over
    /// `self.meg_ops` (caller keeps it fresh: decode_program + build_ops,
    /// the :4181-4184 run-loop protocol belongs to the wiring row).
    pub fn meg_run_program(&mut self, sintab: &[u16]) {
        let mut seam = crate::meg::MegSwp {
            flag_n: &mut self.meg_flag_n,
            flag_z: &mut self.meg_flag_z,
            ix2_value: &mut self.meg_ix2_value,
            ix2_act: &mut self.meg_ix2_act,
            ram_index2: &mut self.meg_ram_index2,
            skip_to: &mut self.meg_skip_to,
            revram_enable: self.revram_enable,
            reverb_ram: &mut self.reverb_ram,
            seed: &mut self.rand_seed,
            sintab,
        };
        let ops = &*self.meg_ops;
        // split-borrow shim: the seam above holds disjoint &mut fields; ops
        // is the read-only ops table (C++ passes m_meg_ops.data() :4442/4446)
        let meg = &mut self.meg;
        crate::meg::run_program(meg, &mut seam, ops);
    }

    /// origin: swp30_device::meg_prg_w<Sel> :2331-2345 (merged 6.237/6.238):
    /// **only a real content change** schedules a rebuild, and only the
    /// dirty addresses become per-address dirty bits (:4393-4395 wakes just
    /// those regions). C++ template Sel -> runtime `sel` (dispatch parity
    /// with the write16 arms; the shift/mask inside `prg_w` are Sel-only).
    pub fn meg_prg_w(&mut self, sel: usize, data: u16) {
        let a = self.meg.program_address; // :2335
        let before = if (a as usize) < 0x180 {
            self.meg.program[a as usize] // :2336
        } else {
            0
        };
        self.meg.prg_w(sel, data); // :2337
        if a >= 0x180 {
            // :2338-2340. NOTE: C++ `prg_w` (:2291-2302) would index
            // m_program[>=0x180] (UB) on this leg before the check; Rust
            // would panic. UNREACHABLE on both sides: prg_address_w clamps
            // >=0x180 to 0 (disk :2278-2283) and Sel==3 auto-increment
            // wraps (disk :2297-2301). Branch kept transliterated.
            self.meg_program_changed = true; // :2339
            self.meg_map_dirty = true; // :2340
        } else if self.meg.program[a as usize] != before {
            self.meg_program_changed = true; // :2342
            self.meg_prg_dirty[(a >> 6) as usize] |= 1u64 << (a & 63); // :2343
        }
    }

    /// origin: swp30_device::meg_map_w<Sel> :2353-2361 (merged: the address
    /// decoding is baked into the ops table, so a map change rebuilds — but
    /// only when the value actually changed; :4390 then wakes ALL regions).
    pub fn meg_map_w(&mut self, sel: usize, data: u16) {
        if self.meg.map_r(sel) != data {
            // :2356-2359
            self.meg_program_changed = true;
            self.meg_map_dirty = true;
        }
        self.meg.map_w(sel, data); // :2360
    }

    /// origin: swp30_device::rand_skip :88-93 (merged 6.237) — the device
    /// seed leg used by the run_sample idle-primed path (:4430-4431).
    pub fn rand_skip(&mut self, n: u32) {
        crate::meg::rand_skip(&mut self.rand_seed, n);
    }

    /// A not-yet-ported slot was touched: count it and record the address.
    #[inline]
    fn defer(&mut self, addr: u32, is_write: bool) {
        self.deferred_hits = self.deferred_hits.wrapping_add(1);
        self.last_deferred_addr = addr;
        self.last_deferred_write = is_write;
    }

    /// origin: swp30_device::read16 :2007-2106. `reg` = (bus_addr - base) >> 1.
    /// Voice slots return real state (voice-engine row); the remaining
    /// deferred slots are MEG/wave territory only (mixer row B wired the rest).
    pub fn read16(&mut self, reg: u32) -> u16 {
        let addr = reg & 0xfff; // :2009
        let slot = addr & 0x3f; // :2010
        let chan = ((addr >> 6) & 0x3f) as usize; // :2011

        // --- channel grid (all 64ch share; offset = chan<<6) :2014-2063 ---
        match slot {
            // filter_block reads (:2015-2019 -> trampolines :2661-2709)
            0x00 => return self.voices.voices[chan].filter.filter_1_a_r(),
            0x01 => return self.voices.voices[chan].filter.level_1_r(),
            0x02 => return self.voices.voices[chan].filter.filter_2_a_r(),
            0x03 => return self.voices.voices[chan].filter.level_2_r(),
            0x04 => return self.voices.voices[chan].filter.filter_b_r(),
            // lfo reads (:2020, :2025 -> :2816-2834)
            0x05 => return self.voices.voices[chan].lfo.amplitude_r(),
            // envelope reads (:2021-2024 -> :2743-2781)
            0x06 => return self.voices.voices[chan].envelope.attack_r(),
            0x07 => return self.voices.voices[chan].envelope.decay1_r(),
            0x08 => return self.voices.voices[chan].envelope.decay2_r(),
            0x09 => return self.voices.voices[chan].envelope.release_glo_r(),
            0x0a => return self.voices.voices[chan].lfo.type_step_pitch_r(),
            // peg rate (:2026 -> :2560-2563)
            0x0b => return self.voices.voices[chan].peg_rate,
            // pitch-EG target (:2027 -> :2547-2550)
            0x10 => return self.voices.voices[chan].pitch_offset,
            // streaming reads (:2028-2034 -> fetch.rs pitch/start/loop/address_r)
            0x11 => return self.voices.voices[chan].streaming.pitch_r(),
            0x12 => return self.voices.voices[chan].streaming.start_h_r(),
            0x13 => return self.voices.voices[chan].streaming.start_l_r(),
            0x14 => return self.voices.voices[chan].streaming.loop_h_r(),
            0x15 => return self.voices.voices[chan].streaming.loop_l_r(),
            0x16 => return self.voices.voices[chan].streaming.address_h_r(),
            0x17 => return self.voices.voices[chan].streaming.address_l_r(),
            // iir1 reads (:2035/0x22/0x24/0x26/0x28/0x2a; template Filter 0/1)
            0x20 => return self.voices.voices[chan].iir1.a1_r(0),
            0x22 => return self.voices.voices[chan].iir1.b1_r(0),
            0x24 => return self.voices.voices[chan].iir1.a0_r(0),
            0x26 => return self.voices.voices[chan].iir1.b1_r(1),
            0x28 => return self.voices.voices[chan].iir1.a1_r(1),
            0x2a => return self.voices.voices[chan].iir1.a0_r(1),
            // meg_const_r<Sel> (:2036-0x2b -> :3389-3392, idx=(offset>>6)*6+Sel)
            0x21 | 0x23 | 0x25 | 0x27 | 0x29 | 0x2b => {
                return self.meg.const_r(chan * 6 + ((slot - 0x21) / 2) as usize)
            }
            // meg_offset_r<Sel> (:2047-0x31 -> :3400-3403, idx=chan*2+Sel)
            0x30 | 0x31 => return self.meg.offset_r(chan * 2 + (slot - 0x30) as usize),
            // meg_lfo_r<Sel> (:2061-62 -> :3411-3414). idx >= 0x18 is UB in
            // C++ (past m_lfo); meg.rs header locks read->0 (probe: never hit).
            0x3e | 0x3f => {
                let idx = chan * 2 + (slot - 0x3e) as usize;
                return if idx < 0x18 { self.meg.lfo_r(idx) } else { 0 };
            }
            // vol/route reads (:2049-2060 — handlers in mix.rs, mixer row B;
            // template Sel passed as the on-disk `0x00|j` / `0x40|j` value)
            0x32 => return self.vol_r(0x00, chan), // :2049 vol_r<0x00|0>
            0x33 => return self.vol_r(0x01, chan), // :2050 vol_r<0x00|1>
            0x34 => return self.vol_r(0x02, chan), // :2051 vol_r<0x00|2>
            0x35 => return self.route_r(0x00, chan), // :2052 route_r<0x00|0>
            0x36 => return self.route_r(0x01, chan), // :2053 route_r<0x00|1>
            0x37 => return self.route_r(0x02, chan), // :2054 route_r<0x00|2>
            0x38 => return self.vol_r(0x40, chan), // :2055 vol_r<0x40|0>
            0x39 => return self.vol_r(0x41, chan), // :2056 vol_r<0x40|1>
            0x3a => return self.vol_r(0x42, chan), // :2057 vol_r<0x40|2>
            0x3b => return self.route_r(0x40, chan), // :2058 route_r<0x40|0>
            0x3c => return self.route_r(0x41, chan), // :2059 route_r<0x40|1>
            0x3d => return self.route_r(0x42, chan), // :2060 route_r<0x40|2>
            _ => {}
        }

        // --- control registers (single-shot at channel positions) :2066-2102 ---
        match addr {
            // REAL (boot-reachable / trivially safe reset-state reads):
            0x20e => 0,                          // keyon_r() :2231 -> 0 (real)
            // internal register window (mixer row B; handlers in mix.rs)
            0x04e => self.internal_adr_r(),      // :2067 internal_adr_r()
            0x04f => self.internal_r(),          // :2068 internal_r()
            // keyon_mask readback (:2078-81, keyon_mask_r<Sel> :2221-2224)
            0x18e => (self.keyon_mask >> 48) as u16, // Sel=3
            0x18f => (self.keyon_mask >> 32) as u16, // Sel=2
            0x1ce => (self.keyon_mask >> 16) as u16, // Sel=1
            0x1cf => self.keyon_mask as u16,         // Sel=0
            0x80e => self.revram_enable,         // :2098 -> m_revram_enable (latch)
            0x84e => 0,                          // revram_status_r() :2495 -> 0
            // MEG control reads (MEG row):
            0x40f => self.meg.prg_address_r(),   // meg_prg_address_r :2083/:2305
            0x44e => self.meg.prg_r(0),          // meg_prg_r<0> :2084/:2315
            0x44f => self.meg.prg_r(1),          // :2085
            0x48e => self.meg.prg_r(2),          // :2086
            0x48f => self.meg.prg_r(3),          // :2087
            0x60e => self.meg.map_r(0),          // meg_map_r<0..7> :2088-95/:2327
            0x64e => self.meg.map_r(1),
            0x68e => self.meg.map_r(2),
            0x6ce => self.meg.map_r(3),
            0x70e => self.meg.map_r(4),
            0x74e => self.meg.map_r(5),
            0x78e => self.meg.map_r(6),
            0x7ce => self.meg.map_r(7),
            // revram_data_r<Sel> :2100-01/:2521-27 (Sel=1 RELOADS from RAM):
            0x98e => self.revram_data_r(1),
            0x98f => self.revram_data_r(0),
            // DEFERRED control reads (not touched by the 28M-cycle boot):
            0x08e | 0x08f | 0x0ce | 0x0cf | 0x10e | 0x10f
            | 0x30f | 0x14e | 0x14f => {
                // deferred: swp30.cpp:2069-2077 (wave addr/size/access/val
                // + rec_pos) -> wave/internal rows.
                self.defer(addr, false);
                0
            }
            // snd_r basin — "書き込み専用の受け皿しかない" :2104 -> 0 (real).
            _ => 0, // :2105
        }
    }

    /// origin: swp30_device::write16 :2108-2218. `reg` = (bus_addr - base) >> 1.
    /// Boot routes every access through to the `snd_w` basin (no state change);
    /// the named grid/control arms are deferred so a stray boot access trips
    /// the `deferred_hits == 0` gate rather than being silently dropped.
    pub fn write16(&mut self, reg: u32, data: u16) {
        let addr = reg & 0xfff; // :2110
        let slot = addr & 0x3f; // :2111
        // const chan = (addr >> 6) & 0x3f; // :2112

        // WTRACE stderr dump :2116-2121 needs m_meg->m_sample_counter (MEG row)
        // — inert on the boot path (WTRACE unset), so not reproduced here.

        // --- channel grid writes :2124-2177 — voice slots REAL (voice-engine
        // row); meg/mixer slots stay deferred (MEG-row territory only) ---
        let chan = (addr >> 6) & 0x3f;
        // S-MU2000 (merged :2122): any write to a voice wakes it from idle —
        // ANY register, and this fires on the WHOLE write16 dispatch (global
        // regs compute a spurious chan too; disk calls that harmless, and the
        // snd_w basin below is included — clear BEFORE the match arms).
        self.awm_idle &= !(1u64 << chan); // :2122
        if matches!(slot, 0x21 | 0x23 | 0x25 | 0x27 | 0x29 | 0x2b) {
            // meg_const_w<Sel> (:2146-2160 -> :3394-3398, idx=(offset>>6)*6+Sel)
            let idx = (chan as usize) * 6 + ((slot - 0x21) / 2) as usize;
            self.meg.const_w(idx, data, &mut self.meg_const_gen); // seam: m_meg_const_gen
            return;
        }
        if matches!(slot, 0x30 | 0x31) {
            self.meg.offset_w((chan as usize) * 2 + (slot - 0x30) as usize, data, &mut self.meg_off_gen); // :3413-3419 seam: m_meg_off_gen (merged)
            return;
        }
        if matches!(slot, 0x3e | 0x3f) {
            // meg_lfo_w<Sel> :3416-3424 — idx >= 0x18 logs "nolfo" and in
            // C++ writes out of bounds (UB); probe (meg.rs header) proves the
            // firmware never does: Rust DROPS such writes (idx<0x18 only).
            let idx = (chan as usize) * 2 + (slot - 0x3e) as usize;
            if idx < 0x18 {
                self.meg.lfo_w(idx, data);
            }
            return;
        }
        if matches!(slot, 0x32..=0x3d) {
            // vol/route writes (:2163-2174 — handlers in mix.rs, mixer row B)
            let c = chan as usize;
            match slot {
                0x32 => self.vol_w(0x00, c, data), // :2163 vol_w<0x00|0>
                0x33 => self.vol_w(0x01, c, data), // :2164 vol_w<0x00|1>
                0x34 => self.vol_w(0x02, c, data), // :2165 vol_w<0x00|2>
                0x35 => self.route_w(0x00, c, data), // :2166 route_w<0x00|0>
                0x36 => self.route_w(0x01, c, data), // :2167 route_w<0x00|1>
                0x37 => self.route_w(0x02, c, data), // :2168 route_w<0x00|2>
                0x38 => self.vol_w(0x40, c, data), // :2169 vol_w<0x40|0>
                0x39 => self.vol_w(0x41, c, data), // :2170 vol_w<0x40|1>
                0x3a => self.vol_w(0x42, c, data), // :2171 vol_w<0x40|2>
                0x3b => self.route_w(0x40, c, data), // :2172 route_w<0x40|0>
                0x3c => self.route_w(0x41, c, data), // :2173 route_w<0x40|1>
                _ => self.route_w(0x42, c, data), // :2174 route_w<0x40|2>
            }
            return;
        }
        let v = &mut self.voices.voices[chan as usize];
        match slot {
            // filter (:2125-2129 -> :2661-2709)
            0x00 => { v.filter.filter_1_a_w(data); return; }
            0x01 => { v.filter.level_1_w(data); return; }
            0x02 => { v.filter.filter_2_a_w(data); return; }
            0x03 => { v.filter.level_2_w(data); return; }
            0x04 => { v.filter.filter_b_w(data); return; }
            // lfo (:2130, :2135 -> :2816-2834)
            0x05 => { v.lfo.amplitude_w(data); return; }
            // envelope (:2131-2134 -> :2743-2781)
            0x06 => { v.envelope.attack_w(data); return; }
            0x07 => { v.envelope.decay1_w(data); return; }
            0x08 => { v.envelope.decay2_w(data); return; }
            0x09 => { v.envelope.release_glo_w(data); return; }
            0x0a => { v.lfo.type_step_pitch_w(data); return; }
            // peg rate — plain store (:2136 -> :2565-2568)
            0x0b => { v.peg_rate = data; return; }
            // pitch-EG target (:2137 -> :2552-2558; un-arrives the peg)
            0x10 => {
                v.pitch_offset = data; // :2555
                if v.peg_cur != sext14(data as u32) {
                    v.peg_reached = 0; // :2556-2557
                }
                return;
            }
            // streaming (:2138-2144 -> fetch.rs pitch/start/loop/address_w)
            0x11 => { v.streaming.pitch_w(data); return; }
            0x12 => { v.streaming.start_h_w(data); return; }
            0x13 => { v.streaming.start_l_w(data); return; }
            0x14 => { v.streaming.loop_h_w(data); return; }
            0x15 => { v.streaming.loop_l_w(data); return; }
            0x16 => { v.streaming.address_h_w(data); return; }
            0x17 => { v.streaming.address_l_w(data); return; }
            // iir1 — NOTE :2151-2154 (S-MU2000): the 2nd filter's FIRST
            // coefficient slot is b1, not a1; confusing them makes the
            // feedback blow up. Slot order per disk :2145-2160.
            0x20 => { v.iir1.a1_w(0, data); return; }
            0x22 => { v.iir1.b1_w(0, data); return; }
            0x24 => { v.iir1.a0_w(0, data); return; }
            0x26 => { v.iir1.b1_w(1, data); return; }
            0x28 => { v.iir1.a1_w(1, data); return; }
            0x2a => { v.iir1.a0_w(1, data); return; }
            _ => {} // meg/mixer slots were caught by the deferred pre-check
        }

        // --- control register writes :2180-2215 ---
        match addr {
            // internal register window (mixer row B; handlers in mix.rs)
            0x04e => {
                self.internal_adr_w(data); // :2181 internal_adr_w
                return;
            }
            // Trivial latches (real; NOT hit by the boot golden, so the gate
            // stays deferred_hits==0, but they need no deferred subsystem):
            0x30e => {
                self.rec_ctrl = data; // :2187 m_rec_ctrl = data
                return;
            }
            // keyon_mask halves (:2190-93, keyon_mask_w<Sel> :2226-2229)
            0x18e => {
                self.keyon_mask =
                    (self.keyon_mask & !(0xffffu64 << 48)) | ((data as u64) << 48); // Sel=3
                return;
            }
            0x18f => {
                self.keyon_mask =
                    (self.keyon_mask & !(0xffffu64 << 32)) | ((data as u64) << 32); // Sel=2
                return;
            }
            0x1ce => {
                self.keyon_mask =
                    (self.keyon_mask & !(0xffffu64 << 16)) | ((data as u64) << 16); // Sel=1
                return;
            }
            0x1cf => {
                self.keyon_mask = (self.keyon_mask & !0xffffu64) | (data as u64); // Sel=0
                return;
            }
            // the key-on strobe (:2194)
            0x20e => {
                self.keyon_w(data);
                return;
            }
            // MEG control writes (MEG row; trampolines :2305-2337/:3394-3428):
            0x40e => {
                self.meg.lfo_commit_w(); // meg_lfo_commit_w :2195/:3426-3429 (data ignored)
                return;
            }
            0x40f => {
                self.meg.prg_address_w(data); // :2196/:2310-2313
                return;
            }
            // meg_prg_w<Sel> — MERGED :2207-2210 -> :2331-2345: change-gated
            // rebuild + per-address dirty bitmap (helper below)
            0x44e => { self.meg_prg_w(0, data); return; } // :2207
            0x44f => { self.meg_prg_w(1, data); return; } // :2208
            0x48e => { self.meg_prg_w(2, data); return; } // :2209
            0x48f => { self.meg_prg_w(3, data); return; } // :2210
            // meg_map_w<Sel> — MERGED :2211-2218 -> :2353-2361: change-gated
            0x60e => { self.meg_map_w(0, data); return; } // :2211
            0x64e => { self.meg_map_w(1, data); return; } // :2212
            0x68e => { self.meg_map_w(2, data); return; } // :2213
            0x6ce => { self.meg_map_w(3, data); return; } // :2214
            0x70e => { self.meg_map_w(4, data); return; } // :2215
            0x74e => { self.meg_map_w(5, data); return; } // :2216
            0x78e => { self.meg_map_w(6, data); return; } // :2217
            0x7ce => { self.meg_map_w(7, data); return; } // :2218
            0x80e => { self.revram_enable_w(data); return; } // :2209/:2458-2468
            0x80f => { self.revram_clear_w(data); return; }  // :2210/:2470-2493
            0x94e => { self.revram_adr_w(1, data); return; } // :2211/:2500-2506
            0x94f => { self.revram_adr_w(0, data); return; } // :2212
            0x98e => { self.revram_data_w(1, data); return; } // :2213/:2508-2519 (Sel=1 hi half only)
            0x98f => { self.revram_data_w(0, data); return; } // :2214 (Sel=0 latches AND stores to RAM)
            // DEFERRED control writes (not touched by the 28M-cycle boot):
            0x08e | 0x08f | 0x0ce | 0x0cf | 0x10e | 0x14e | 0x14f => {
                // deferred: swp30.cpp:2182-2189 (wave addr+size+access+val)
                // -> wave/internal rows.
                self.defer(addr, true);
                return;
            }
            _ => {}
        }

        // fall-through basin: snd_w :2117/2217.
        self.snd_w(addr, data);
    }

    /// origin: swp30_device::keyon_w :2246-2270 (merged). The data argument is
    /// IGNORED on disk (:2246 `keyon_w(u16)`); the keyon comes from
    /// `keyon_mask`. :2248 wakes every keyed voice from idle first.
    /// Exact per-channel order :2256-2263 (streaming/filter/iir1/envelope/
    /// lfo/peg). The g_verbose dbg_notes block (:2252-2255) and the
    /// `logerror[%08d] keyon` trace (:2265-2266) are debug sinks with no
    /// state effect (logerror is off in this port like WTRACE/snd_w) —
    /// not reproduced; `m_streaming[].describe()` is therefore not called.
    pub fn keyon_w(&mut self, _data: u16) {
        self.awm_idle &= !self.keyon_mask; // :2248 (merged)
        // disjoint field borrows (machine seed + voice grid)
        let keyon_mask = &mut self.keyon_mask;
        let voices = &mut self.voices;
        let seed = &mut self.rand_seed;
        for chan in 0..64usize {
            let mask = 1u64 << chan; // :2239
            if *keyon_mask & mask == 0 {
                continue; // :2240
            }
            let v = &mut voices.voices[chan];
            v.streaming.keyon(); // :2245
            v.filter.keyon(); // :2246
            v.iir1.keyon(); // :2247
            v.envelope.keyon(); // :2248
            v.lfo.keyon(seed); // :2249 (device rand seam)
            // :2250-2251: pitch EG starts from the level written BEFORE keyon
            v.peg_cur = sext14(v.pitch_offset as u32);
            v.peg_reached = 1; // :2252
        }
        *keyon_mask = 0; // :2258
    }

    /// origin: swp30_device::snd_w :2874-2899 — a pure `logerror` sink with NO
    /// register-file side effect (verified disk: only formats a debug string and
    /// returns). With `g_verbose` off (the boot path) it is a genuine no-op; the
    /// slot-based `preg` naming is stderr-only diagnostics, deferred to the
    /// verbose/logging row. Kept as a distinct method so every boot write
    /// routes here instead of a silent drop.
    fn snd_w(&mut self, _offset: u32, _data: u16) {
        // :2879-2880 slot==0x0b early return; :2882-2898 logerror only.
        // No state mutation on the disk path.
    }

    /// origin: swp30_device::revram_enable_w :2458-2468. SAME-value writes
    /// are ignored (:2461-2462 — dense probe: `enable = 00f0` repeats, only
    /// changes latch). `meg_jit_invalidate()` (:2466) is inert here (JIT
    /// never built); the `m_meg_jit_wait = 1` latch (:2467) is kept.
    fn revram_enable_w(&mut self, data: u16) {
        if data == self.revram_enable {
            return; // :2461-2462
        }
        self.revram_enable = data; // :2463
        self.meg_jit_wait = 1; // :2467
    }

    /// origin: revram_clear_w :2470-2493. Bit i of data zeroes map region i:
    /// base = map[i].bits0-7 << 10, size = 1 << (10 + map[i].bits8-10),
    /// clamped to the 1<<18 RAM (:2487-2491).
    fn revram_clear_w(&mut self, data: u16) {
        for i in 0..8usize {
            if data & (1 << i) == 0 {
                continue; // :2485-2486
            }
            let base = (crate::meg::bit_of(self.meg.map[i], 0, 8) << 10) as usize; // :2487
            let size = 1usize << (10 + crate::meg::bit_of(self.meg.map[i], 8, 3)); // :2488
            let end = (base + size).min(self.reverb_ram.len()); // :2489
            if base < end {
                self.reverb_ram[base..end].fill(0); // :2490-2491
            }
        }
    }

    /// origin: revram_adr_w<Sel> :2500-2506 (half-latch into m_revram_adr)
    fn revram_adr_w(&mut self, sel: i32, data: u16) {
        if sel != 0 {
            self.revram_adr = (self.revram_adr & 0x0000_ffff) | ((data as u32) << 16); // :2503
        } else {
            self.revram_adr = (self.revram_adr & 0xffff_0000) | (data as u32); // :2505
        }
    }

    /// origin: revram_data_w<Sel> :2508-2519. Sel=0 additionally encodes
    /// `u32(s32(m_revram_data) >> 8)` into the RAM (:2517-2518) — the
    /// S-MU2000 >>8 vs MAME >>5 scale quirk locked verbatim. RAM address
    /// wraps `flat_space<18,1,-1>` pow2-style (mamecompat.h:205-216):
    /// word index = addr & (0x40000-1).
    fn revram_data_w(&mut self, sel: i32, data: u16) {
        if sel != 0 {
            self.revram_data = (self.revram_data & 0x0000_ffff) | ((data as u32) << 16); // :2511
        } else {
            self.revram_data = (self.revram_data & 0xffff_0000) | (data as u32); // :2513
        }
        if sel == 0 {
            let enc = MegState::revram_encode((self.revram_data as i32 >> 8) as u32); // :2518
            let a = (self.revram_adr & 0x3_ffff) as usize;
            self.reverb_ram[a] = enc;
        }
    }

    /// origin: revram_data_r<Sel> :2521-2527. Sel=1 RELOADS the latch from
    /// the RAM word and reconstructs (decode <<5 then <<3 after s32), then
    /// returns the hi half; Sel=0 returns the lo half of the latch.
    fn revram_data_r(&mut self, sel: i32) -> u16 {
        if sel != 0 {
            let a = (self.revram_adr & 0x3_ffff) as usize;
            let dec = MegState::revram_decode(self.reverb_ram[a]); // :2524
            // C++: u32(s32(u32 << 5) << 3) — bit-identical u32 wraps both sides
            self.revram_data = (((dec << 5) as i32) << 3) as u32; // :2524
            (self.revram_data >> 16) as u16 // :2526 hi
        } else {
            self.revram_data as u16 // :2526 lo
        }
    }
}
