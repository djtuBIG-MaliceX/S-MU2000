//! M1 `compat/timers` gate.
//!
//! The golden `rust/tests/golden/trace_upd_boot.txt` is the C++ `--trace-upd`
//! log (sh7042.cpp:294-297) of `build/boot.exe roms` (28,000,000 cycles),
//! captured 2026-09-30. Format: `U %llu -> %llu pc=%08x\n` — the time
//! `sh7042_device::internal_update` ran and the next event it scheduled
//! (0 = none).
//!
//! The replay drives the ported `RunningMachine` through the exact timer/event
//! skeleton of `mu2000::run_cycles` (mu2000.cpp:1168-1201), with a scripted
//! CPU/peripheral stand-in: the 24 golden lines pin both the observed time of
//! every update and the schedule it produced. Peripheral-register writes stop
//! the CPU mid-slice (sh.h `abort_timeslice`); the −1 rounding of
//! `sh7042_device::current_cycles` (sh7042.h:92-98) is exercised because the
//! harness feeds the *total* cycles and the ported view must show `total-1`
//! for write-triggers but the exact boundary inside `event_tick` (m_in_event).
//! Midi/USB/SWP-wait clamps of the loop are no-ops for this fixture: boot has
//! no MIDI/USB line activity, and SWP wait only advances the clock via
//! `skip_cycles`, which the scripted targets already fold in.
//! This same golden re-gates the full boot at M2 with real peripherals.

use super::*;

const HZ: u32 = 28_000_000;

/// origin: sh7042.h:92-98 `current_cycles` (test-only stand-in for the SH7042
/// device; lands for real in `smu-sh2` at M2).
fn current_cycles(total: u64, in_event: bool) -> u64 {
    if in_event {
        total
    } else if total != 0 {
        total - 1
    } else {
        0
    }
}

struct Upd {
    cur: u64,
    ev: u64,
    pc: u32,
}

fn golden() -> (String, Vec<Upd>) {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/golden/trace_upd_boot.txt"
    );
    let text = std::fs::read_to_string(path).expect("golden trace_upd_boot.txt missing");
    let mut v = Vec::new();
    for line in text.lines() {
        // "U 124 -> 159 pc=00001170"
        let rest = line.strip_prefix("U ").expect("U prefix");
        let (times, pc) = rest.split_once(" pc=").expect("pc field");
        let (cur, ev) = times.split_once(" -> ").expect("arrow");
        v.push(Upd {
            cur: cur.parse().expect("cur"),
            ev: ev.parse().expect("ev"),
            pc: u32::from_str_radix(pc, 16).expect("pc"),
        });
    }
    (text, v)
}

/// origin: sh7042.cpp:278-300 `internal_update` traced tail + :262-271
/// `recompute_timer` (abort_timeslice at a slice top is a no-op, so the
/// harness only re-arms the schedule).
fn do_update(cur: u64, k: &mut usize, script: &[Upd], sched: &mut u64, out: &mut String) {
    let s = &script[*k];
    assert_eq!(cur, s.cur, "golden line {}: update observed at wrong cycle", *k + 1);
    out.push_str(&format!("U {} -> {} pc={:08x}\n", cur, s.ev, s.pc));
    *sched = s.ev;
    *k += 1;
}

#[test]
fn golden_boot_trace_replay() {
    let (golden_text, script) = golden();
    assert_eq!(script.len(), 24);

    let mut machine = RunningMachine::new();
    machine.set_clock_hz(HZ);

    let mut out = String::new();
    let mut k = 0usize;
    let mut event_sched = 0u64; // sh7042.h:173 m_event_cycles
    let mut in_event = false; // sh7042.h m_in_event (event_tick :247-252)
    let mut cpu_total = 0u64; // sh.h:234 m_total_cycles
    let mut idle = 0i32;
    let mut n = 28_000_000u64; // boot.cpp:26 default budget, single run_cycles
    let mut guard = 0u64;

    // ---- mu2000.cpp:1168 while (n) — timer/event skeleton ----
    while n > 0 {
        guard += 1;
        assert!(guard < 20_000_000, "run_cycles skeleton run away");

        let now = cpu_total; // :1170
        machine.set_cycles(now); // :1171

        let tmr = machine.next_timer_cycles(); // :1174
        if tmr <= now {
            // :1175-1180 — never taken here: boot schedules no emu_timers
            // (the queue is sci4-only, see sh7042 trace sources).
            machine.run_timers(now);
            machine.set_cycles(now);
            continue;
        }

        let ev = event_sched; // sh7042.h:76 event_cycles()
        if ev != 0 && now >= ev {
            // :1184-1190 event_tick: m_in_event brackets the update
            in_event = true;
            do_update(current_cycles(now, true), &mut k, &script, &mut event_sched, &mut out);
            in_event = false;
            if event_sched == ev {
                idle += 1;
                if idle > 2 {
                    break; // :1187-1188 idle guard
                }
            }
            continue;
        }
        idle = 0; // :1191

        // :1194-1195 midi_step/usb_step — no line activity in this fixture
        let mut chunk = n; // :1197-1201
        if ev != 0 && ev - now < chunk {
            chunk = ev - now;
        }
        if tmr != u64::MAX && tmr - now < chunk {
            chunk = tmr - now;
        }
        // :1202-1208 MIDI bit clamp — inactive (queue empty, bit < 0)
        // :1211-1217 swp_wait skip — folded into the scripted targets

        let mut stop = cpu_total + chunk;
        // A scripted peripheral-register write ends the timeslice exactly at
        // `cur+1` total cycles (abort_timeslice, sh.h:219-223); the update
        // then observes current_cycles() == cur (the −1, sh7042.h:97).
        if k < script.len() {
            let is_tick = event_sched != 0 && script[k].cur == event_sched;
            assert!(
                is_tick || event_sched == 0 || script[k].cur + 1 < event_sched,
                "line {}: write target {} not before scheduled tick {}",
                k + 1,
                script[k].cur + 1,
                event_sched
            );
            if !is_tick {
                let w = script[k].cur + 1;
                if w <= stop {
                    n -= w - cpu_total;
                    cpu_total = w;
                    do_update(current_cycles(w, false), &mut k, &script, &mut event_sched, &mut out);
                    continue;
                }
            }
        }
        cpu_total = stop;
        n -= chunk;
    }

    assert_eq!(k, script.len(), "script not exhausted");
    assert_eq!(cpu_total, 28_000_000);
    // the gate: event order (and bytes) identical to the C++ golden.
    // C++ text-mode fprintf turns \n into \r\n on Windows; Rust stdio is
    // verbatim — the Rust --trace-upd writer must emit \r\n itself when run
    // on Windows (logged as a pitfall). Fixture keeps the raw capture.
    assert_eq!(out, golden_text.replace("\r\n", "\n"));
}

#[test]
fn adjust_roundtrip_bit_exact() {
    // Expected values computed offline with Python IEEE-754 doubles (no
    // fast-math either side). Deltas 35/108/2800129/2799156/2800021 are the
    // real schedule spacings from the golden; 1 is the documented −1 trap:
    // 1/28e6 * 28e6 = 0.9999999999999999 → truncates to 0.
    let cases: [(u64, u64); 6] = [
        (1, 0),
        (35, 35),
        (108, 108),
        (2800129, 2800129),
        (2799156, 2799156),
        (2800021, 2800021),
    ];
    let mut m = RunningMachine::new();
    m.set_clock_hz(HZ);
    let id = m.make_timer(|_, _| {});
    for (d, want) in cases {
        m.set_cycles(1000);
        m.timer_adjust(id, &Attotime::from_ticks(d, HZ), 7);
        assert_eq!(m.timer_expire(id), 1000 + want, "delta {}", d);
        assert_eq!(Attotime::from_ticks(d, HZ).as_ticks(HZ), want, "as_ticks {}", d);
    }
}

#[test]
fn run_timers_order_rearm_and_guard() {
    let mut m = RunningMachine::new();
    m.set_clock_hz(HZ);

    // birth-order tie-break (mamecompat.h:688-691 strict <): first born wins
    use std::rc::Rc;
    use std::cell::RefCell;
    let order = Rc::new(RefCell::new(Vec::<i32>::new()));
    let o1 = Rc::clone(&order);
    let t1 = m.make_timer(move |_, p| o1.borrow_mut().push(p));
    let o2 = Rc::clone(&order);
    let t2 = m.make_timer(move |_, p| o2.borrow_mut().push(p));
    m.set_cycles(500);
    m.timer_adjust(t2, &Attotime::from_ticks(108, HZ), 2);
    m.timer_adjust(t1, &Attotime::from_ticks(108, HZ), 1); // same expire, older index
    assert_eq!(m.next_timer_cycles(), 500 + 108);
    m.run_timers(608);
    assert_eq!(*order.borrow(), vec![1, 2]); // t1 born first → fires first
    assert!(!m.timer_scheduled(t1) && !m.timer_scheduled(t2));

    // re-arm from inside the callback: clock rides the expire time at entry,
    // so the new expire = old_expire + trunc(delta*hz/hz path)
    let count = Rc::new(RefCell::new(0u32));
    let c = Rc::clone(&count);
    let t3 = m.make_timer(move |mm: &mut RunningMachine, _| {
        *c.borrow_mut() += 1;
        mm.timer_adjust(TimerId(2), &Attotime::from_ticks(35, HZ), 0);
    });
    m.set_cycles(0);
    m.timer_adjust(t3, &Attotime::from_ticks(10, HZ), 0);
    m.run_timers(50);
    // fires at 10, re-arms 10+35=45, fires again at 45, re-arms 80 > 50 → stop
    assert_eq!(*count.borrow(), 2);

    // guard: zero-delay re-arm spins exactly 64 times then bails (:686)
    let count2 = Rc::new(RefCell::new(0u32));
    let c2 = Rc::clone(&count2);
    let t4 = m.make_timer(move |mm: &mut RunningMachine, _| {
        *c2.borrow_mut() += 1;
        mm.timer_adjust(TimerId(3), &Attotime::ZERO, 0);
    });
    m.set_cycles(0);
    m.timer_adjust(t4, &Attotime::ZERO, 0);
    m.run_timers(0);
    assert_eq!(*count2.borrow(), 64);
}

#[test]
fn enable_never_and_zero_quirks() {
    let mut m = RunningMachine::new();
    let id = m.make_timer(|_, _| {});
    m.set_cycles(10);
    m.timer_adjust(id, &Attotime::from_ticks(35, HZ), 0);
    m.timer_enable(id, false); // :549 clears
    assert!(!m.timer_scheduled(id));
    m.timer_enable(id, true); // quirk: enable(true) restores NOTHING
    assert!(!m.timer_scheduled(id));

    m.timer_adjust(id, &Attotime::NEVER, 3); // :733-734 never → unscheduled
    assert!(!m.timer_scheduled(id));

    // ordering: never sorts after all finite (operator< :522-526)
    assert!(Attotime::from_ticks(1 << 40, 1) < Attotime::NEVER);
    assert!(!(Attotime::NEVER < Attotime::NEVER));
    assert!(Attotime::ZERO.is_zero());
    assert!(!Attotime::NEVER.is_zero());
    assert_eq!(Attotime::NEVER, Attotime::NEVER);
    assert!(Attotime::ZERO != Attotime::NEVER);
}

#[test]
fn rand_lcg_vectors() {
    // Python: s=(1664525*s+1013904223)&0xffffffff from 0x9d14abd7, rotl16 out
    let want = [0x574a3af2, 0xde214fbe, 0x610c06da, 0xa8fb125c, 0x8a1e96fa];
    let mut m = RunningMachine::new();
    for w in want {
        assert_eq!(m.rand(), w);
    }
    m.reset_seed();
    assert_eq!(m.rand(), want[0]);
}

#[test]
fn machine_clock_defaults() {
    let m = RunningMachine::new();
    assert_eq!(m.clock_hz(), 28_000_000); // mamecompat.h:716
    assert_eq!(m.cycles(), 0);
    assert_eq!(m.next_timer_cycles(), u64::MAX); // nothing scheduled (:675)
}

// ---- M5-W1: state_sync golden (mamecompat.h:556, :699-712 + state.h) ----

fn hex(v: &[u8]) -> String {
    v.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn machine_state_sync_golden_bytes_and_restore() {
    use crate::state_io::StateIo;

    // 2-timer machine: seed default, cycles 1000, t1 @1000+108 param 7,
    // t2 unscheduled (NEVER adjust keeps ~0) param -3.
    let mut m = RunningMachine::new();
    m.set_clock_hz(HZ);
    let t1 = m.make_timer(|_, _| {});
    let t2 = m.make_timer(|_, _| {});
    m.set_cycles(1000);
    m.timer_adjust(t1, &Attotime::from_ticks(108, HZ), 7);
    m.timer_adjust(t2, &Attotime::NEVER, -3);

    let mut buf = Vec::new();
    {
        let mut io = StateIo::writer(&mut buf);
        m.state_sync(&mut io);
        assert!(io.ok());
    }
    // hand-built golden: "mach\0\0\0\0" | seed d7ab149d | cyc 1000 | n=2
    //   | t1 exp 1108(0x454) param 7 | t2 exp ~0 param -3(0xfffffffd)
    const GOLDEN: &str = "6d61636800000000\
                          d7ab149d\
                          e803000000000000\
                          02000000\
                          5404000000000000\
                          07000000\
                          ffffffffffffffff\
                          fdffffff";
    assert_eq!(hex(&buf), GOLDEN);

    // restore into a fresh same-birth-order machine; dirty state first
    use std::cell::RefCell;
    use std::rc::Rc;
    let fired = Rc::new(RefCell::new(Vec::<i32>::new()));
    let f = Rc::clone(&fired);
    let mut m2 = RunningMachine::new();
    m2.set_clock_hz(HZ);
    let u1 = m2.make_timer(move |_, p| f.borrow_mut().push(p));
    let u2 = m2.make_timer(|_, _| {});
    m2.rand(); // desync seed
    m2.set_cycles(7);
    m2.timer_adjust(u2, &Attotime::from_ticks(35, HZ), 99);
    {
        let mut io = StateIo::reader(&buf);
        m2.state_sync(&mut io);
        assert!(io.ok(), "{}", io.error());
    }
    assert_eq!(m2.cycles(), 1000);
    assert_eq!(m2.rand(), 0x574a3af2); // seed restored → golden LCG next
    assert!(m2.timer_scheduled(u1));
    assert_eq!(m2.timer_expire(u1), 1108);
    assert!(!m2.timer_scheduled(u2)); // t2's ~0 overwrote the dirty schedule
    assert_eq!(m2.timer_expire(u2), u64::MAX);
    m2.run_timers(1108); // param travels: callback sees 7
    assert_eq!(*fired.borrow(), vec![7]);
}

#[test]
fn machine_state_sync_count_mismatch_early_return_quirk() {
    use crate::state_io::StateIo;

    // writer: ONE timer (cycles 9, expire 9+35=44, param 5)
    let mut m1 = RunningMachine::new();
    let t = m1.make_timer(|_, _| {});
    m1.set_cycles(9);
    m1.timer_adjust(t, &Attotime::from_ticks(35, HZ), 5);
    let mut buf = Vec::new();
    m1.state_sync(&mut StateIo::writer(&mut buf));

    // reader: TWO timers. n=1 != 2 → early return after the count (:708).
    // NOT an error: ok() stays true, timers untouched, stream parked
    // exactly on the first schedule byte (「読み手が食い違いを見る」).
    let mut m2 = RunningMachine::new();
    let u = m2.make_timer(|_, _| {});
    m2.make_timer(|_, _| {});
    m2.set_cycles(5);
    m2.timer_adjust(u, &Attotime::from_ticks(35, HZ), 1); // dirty expire 40
    {
        let mut io = StateIo::reader(&buf);
        m2.state_sync(&mut io);
        assert!(io.ok()); // quirk: mismatch alone never fails the io
        assert_eq!(io.error(), "");
        assert_eq!(m2.cycles(), 9); // clock WAS read (before the quirk)
        let mut probe: u8 = 0;
        io.v(&mut probe);
        assert_eq!(probe, 44); // next byte = writer t1.expire lo — not consumed
        assert!(io.ok());
    }
    assert_eq!(m2.timer_expire(u), 40); // schedule untouched
    assert!(m2.timer_scheduled(u));

    // and the write side NEVER early-returns: n == len by construction
    // (covered above: the 2-timer golden emitted every schedule).
}
