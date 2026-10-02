//! Timer queue + machine clock — transliteration of `src/compat/mamecompat.h`.
//!
//! Ledger row M1 `compat/timers`: attotime (:503-535), emu_timer (:542-563 and
//! its out-of-line `adjust` :730-737), and the clock/rand/timer subset of
//! `running_machine` (:637-719). `state_sync` (:556, :701-712) lands with the
//! M5 state row once the serializer interface exists in `smu-machine`.
//!
//! Quirks copied verbatim (do NOT "fix"):
//! - `adjust` truncates the double product toward zero (:736). With 28 MHz,
//!   `from_ticks(1, hz)` round-trips to **0**, not 1 — the documented MAME
//!   cycle→attotime→cycle −1 class (sh7042.h:85-98 comment). Bit-exact: both
//!   toolchains use IEEE-754 double, no fast-math (ledger invariant 9).
//! - `enable(false)` clears the schedule; `enable(true)` does **nothing** (:549).
//!   Only `adjust` may re-arm. Reproduced exactly.
//! - `run_timers` picks the due timer by FIRST-BORN order on ties (strict `<`
//!   scan, :688-691), sets `m_cycles` to the due time BEFORE firing (:694),
//!   and bails silently after 64 iterations (:686).
//! - `rand()` is MAME's LCG, seed 0x9d14abd7, rotl16 out (:637-647). SWP30 ×2
//!   carry separate seeds via `set_rand_seed` (mu2000.cpp:1135-1136) — that
//!   setter lands with the M3 SWP port.

/// origin: mamecompat.h:503-532 `struct attotime`. Internally "seconds" as f64.
#[derive(Clone, Copy, Debug)]
pub struct Attotime {
    t: f64,
    m_never: bool,
}

impl Attotime {
    /// origin: :512,534 `attotime::never`
    pub const NEVER: Attotime = Attotime { t: 0.0, m_never: true };
    /// origin: :513,535 `attotime::zero`
    pub const ZERO: Attotime = Attotime { t: 0.0, m_never: false };

    /// origin: :509 `from_ticks`
    pub fn from_ticks(n: u64, hz: u32) -> Attotime {
        Attotime { t: n as f64 / hz as f64, m_never: false }
    }

    /// origin: :515 `as_ticks`. C++ `u64(double)` truncates toward zero; Rust
    /// `as u64` truncates identically for finite non-negative values, which is
    /// all that ever reaches here (t only comes from unsigned ticks/hz).
    pub fn as_ticks(&self, hz: u32) -> u64 {
        (self.t * hz as f64) as u64
    }

    /// origin: :516 `as_double`
    pub fn as_double(&self) -> f64 {
        self.t
    }

    /// origin: :517 `is_never`
    pub fn is_never(&self) -> bool {
        self.m_never
    }

    /// origin: :518 `is_zero`
    pub fn is_zero(&self) -> bool {
        !self.m_never && self.t == 0.0
    }

    /// origin: :527 `operator+`
    pub fn add(&self, o: &Attotime) -> Attotime {
        Attotime { t: self.t + o.t, m_never: false }
    }

    /// origin: :528 `operator-`
    pub fn sub(&self, o: &Attotime) -> Attotime {
        Attotime { t: self.t - o.t, m_never: false }
    }

    /// origin: :530 `operator*` — "period × frequency" ratio helper
    pub fn mul(&self, k: f64) -> Attotime {
        Attotime { t: self.t * k, m_never: false }
    }

    /// origin: :531 `operator/`
    pub fn div(&self, k: f64) -> Attotime {
        Attotime { t: self.t / k, m_never: false }
    }
}

/// origin: :520-521 `operator==` / `operator!=`
impl PartialEq for Attotime {
    fn eq(&self, o: &Attotime) -> bool {
        self.m_never == o.m_never && self.t == o.t
    }
}
impl Eq for Attotime {}

/// origin: :522-526 `operator<` — `never` sorts after every finite time.
impl PartialOrd for Attotime {
    fn partial_cmp(&self, o: &Attotime) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Attotime {
    fn cmp(&self, o: &Attotime) -> std::cmp::Ordering {
        if self.m_never || o.m_never {
            if !self.m_never && o.m_never {
                std::cmp::Ordering::Less
            } else if self.m_never && !o.m_never {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        } else {
            self.t.partial_cmp(&o.t).unwrap()
        }
    }
}

/// origin: mamecompat.h:542-563 `class emu_timer`.
///
/// DEVIATION (documented in PORTING_LEDGER Pitfalls/Deviations): C++ callbacks
/// are `std::function<void(s32)>` grabbing the machine through a hidden
/// `m_machine` back-pointer; Rust passes `&mut RunningMachine` into the
/// callback instead (the machine is mutably borrowed while firing). Fire
/// order and the machine clock visible inside a callback are unchanged, so
/// bit-exactness is unaffected.
pub type TimerCb = Box<dyn FnMut(&mut RunningMachine, i32)>;

pub struct EmuTimer {
    cb: Option<TimerCb>,
    /// ~0 = not scheduled (origin: :561 `m_expire = ~u64(0)`)
    expire: u64,
    param: i32, // origin: :562
}

impl EmuTimer {
    /// origin: :551 `scheduled`
    pub fn scheduled(&self) -> bool {
        self.expire != u64::MAX
    }

    /// origin: :552 `expire_cycles`
    pub fn expire_cycles(&self) -> u64 {
        self.expire
    }
}

/// Timer handle. C++ handed out raw `emu_timer *`; here it is an index into
/// `RunningMachine::timers`. Timers are only ever created during device wiring
/// (never removed), so indices are stable for the machine lifetime — same
/// stability guarantee the C++ raw pointers had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerId(pub usize);

/// origin: mamecompat.h:637-719 `class running_machine` (clock + rand + timer
/// queue half). `describe_context`/`side_effects_disabled` (:650-653) are
/// trivially `""`/`false` and fold into call sites at M2.
pub struct RunningMachine {
    rand_seed: u32, // :715, init 0x9d14abd7 — explicit, ledger invariant 3
    hz: u32,        // :716
    cycles: u64,    // :717
    timers: Vec<EmuTimer>, // :718 — birth order matters (tie-break scan)
}

impl RunningMachine {
    /// Every field explicit (ledger invariant 3). Defaults match :715-718.
    pub fn new() -> Self {
        RunningMachine {
            rand_seed: 0x9d14abd7,
            hz: 28_000_000,
            cycles: 0,
            timers: Vec::new(),
        }
    }

    /// origin: :640-645 `rand`. MAME's LCG — the SWP30 noise/LFO waveforms
    /// depend on this exact sequence.
    pub fn rand(&mut self) -> u32 {
        self.rand_seed = self
            .rand_seed
            .wrapping_mul(1664525)
            .wrapping_add(1013904223);
        // low bits are short-period, so rotate 16 out (MAME comment :643)
        (self.rand_seed >> 16) | (self.rand_seed << 16)
    }

    /// origin: :647 `reset_seed`
    pub fn reset_seed(&mut self) {
        self.rand_seed = 0x9d14abd7;
    }

    /// origin: :657 `set_clock_hz`
    pub fn set_clock_hz(&mut self, hz: u32) {
        self.hz = hz;
    }

    /// origin: :658 `clock_hz`
    pub fn clock_hz(&self) -> u32 {
        self.hz
    }

    /// origin: :659 `set_cycles`
    pub fn set_cycles(&mut self, c: u64) {
        self.cycles = c;
    }

    /// origin: :660 `cycles`
    pub fn cycles(&self) -> u64 {
        self.cycles
    }

    /// origin: :661 `time`
    pub fn time(&self) -> Attotime {
        Attotime { t: self.cycles as f64 / self.hz as f64, m_never: false }
    }

    /// origin: :663-667 `make_timer` (+ the out-of-line `device_t::timer_alloc`
    /// glue at :723-728, which in Rust is just the caller passing its closure).
    pub fn make_timer<F>(&mut self, cb: F) -> TimerId
    where
        F: FnMut(&mut RunningMachine, i32) + 'static,
    {
        self.timers.push(EmuTimer {
            cb: Some(Box::new(cb)),
            expire: u64::MAX,
            param: 0,
        });
        TimerId(self.timers.len() - 1)
    }

    /// origin: :730-737 `emu_timer::adjust`. Truncating product — see the
    /// module-header quirk note.
    pub fn timer_adjust(&mut self, id: TimerId, when: &Attotime, param: i32) {
        let t = &mut self.timers[id.0];
        t.param = param;
        if when.is_never() {
            t.expire = u64::MAX;
        } else {
            t.expire = self.cycles + (when.as_double() * self.hz as f64) as u64;
        }
    }

    /// origin: :549 `enable`. Quirk kept: `on == true` is a no-op.
    pub fn timer_enable(&mut self, id: TimerId, on: bool) {
        if !on {
            self.timers[id.0].expire = u64::MAX;
        }
    }

    /// origin: :551 via machine — `scheduled()` for a handle.
    pub fn timer_scheduled(&self, id: TimerId) -> bool {
        self.timers[id.0].scheduled()
    }

    /// origin: :552 via machine — `expire_cycles()` for a handle.
    pub fn timer_expire(&self, id: TimerId) -> u64 {
        self.timers[id.0].expire_cycles()
    }

    /// origin: :669-680 `next_timer_cycles`. Re-scans every call on purpose
    /// (C++ comment :670-672: a cached variant changed the audio — never
    /// "optimize" this).
    pub fn next_timer_cycles(&self) -> u64 {
        let mut best = u64::MAX;
        for t in &self.timers {
            if t.scheduled() && t.expire_cycles() < best {
                best = t.expire_cycles();
            }
        }
        best
    }

    /// origin: :683-697 `run_timers`. Fire-and-re-arm happens live: the loop
    /// re-scans after every fire, so a callback that re-adjusts itself (sci4
    /// tx/rx ticks) is picked up in the same call, bounded by the 64 guard.
    pub fn run_timers(&mut self, now: u64) {
        for _guard in 0..64 {
            let mut due: Option<usize> = None;
            for (i, t) in self.timers.iter().enumerate() {
                if t.scheduled()
                    && t.expire_cycles() <= now
                    && due.map_or(true, |d| t.expire_cycles() < self.timers[d].expire_cycles())
                {
                    due = Some(i);
                }
            }
            let d = match due {
                Some(d) => d,
                None => return,
            };
            // machine clock rides the event time during the callback (:694)
            self.cycles = self.timers[d].expire_cycles();
            // fire(): clear schedule first, then call (:553)
            self.timers[d].expire = u64::MAX;
            let param = self.timers[d].param;
            let mut cb = self.timers[d].cb.take();
            if let Some(cb) = cb.as_mut() {
                cb(self, param);
            }
            self.timers[d].cb = cb;
        }
    }
}

#[cfg(test)]
mod tests;

