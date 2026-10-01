// live.rs — BOOT SKELETON for the `live main` ledger row (M6). Ground truth:
// src/live.cpp:429-527. Goal of this pass: compile + smoke-run against the
// paired smu_machine Machine. Audio/MIDI/WAV HAL, NVRAM, engine options are
// NOT here yet (M4/M5/M6 HAL rows) — flags are accepted-and-ignored.
//
// Deviations (documented, no fingerprint gates this yet — no audio output):
// - sintab dir/standin/sin-table.bin: C++ loads it WARNING-ONLY
//   (live.cpp:496-497); Machine::boot skips it (M3 needs it for the SWP
//   sintab stand-in). No warning printed.
// - NVRAM load (live.cpp:503-506): skipped — smu-machine has no NVRAM (M5);
//   %LOCALAPPDATA%\S-MU2000 is never touched.
// - Boot wait: C++ checks midi_ready() per sample (live.cpp:513-517); the
//   skeleton drives Machine::run_cycles in chunks of CPU_HZ/441 cycles
//   (≈100 samples ≈ 2.27 ms) and checks midi_ready after each chunk.
// - Boot-wait TIMEOUT is a warning here, not the disk's fatal exit
//   (live.cpp:518-520): RE cannot rise until SWP30 (M3) exists — measured
//   scr0=0x00 for the full 30 s limit while the SH2 idles. M3 re-tightens.
// - No --seconds: disk runs until Ctrl+C; skeleton runs 1 s and exits 0.

use smu_compat::paths::init_console_utf8; // live.cpp:431 (paths.rs:664)
use smu_machine::{Machine, CPU_HZ};

const RATE: u64 = 44100; // live.cpp:513 RATE

fn main() {
    init_console_utf8(); // live.cpp:431

    // live.cpp:433-447 defaults; only `seconds` and `dir` are honored here.
    let mut seconds: f64 = 0.0; // live.cpp:443
    let mut dir = String::new(); // live.cpp:447

    // origin: live.cpp:449-476 arg loop. Value-taking flags consume their
    // argument so the next positional is not stolen; every unimplemented
    // flag/directive is ACCEPT-AND-IGNORE ("M6 stub"). Unknown flags: ignore.
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        let a = &raw[i];
        let takes_value = matches!(
            a.as_str(),
            // live.cpp:459-462,465,467-468 — accepted, ignored (M6 stub)
            "--midi" | "--frames" | "--buffers" | "--latency" | "--dump-dev" | "--wav"
        );
        if a == "--seconds" && i + 1 < raw.len() {
            // live.cpp:467 — the only flag with behavior in this skeleton
            seconds = raw[i + 1].parse::<f64>().unwrap_or(0.0);
            i += 2;
            continue;
        }
        if takes_value {
            i += 2; // M6 stub: skip flag + value
            continue;
        }
        match a.as_str() {
            // live.cpp:450-474 — everything else accepted, ignored (M6 stub)
            "--list" | "--nomidi" | "--waveout" | "--raw" | "--single" | "-v" => {}
            _ => {
                if dir.is_empty() && !a.starts_with('-') {
                    dir = a.clone(); // live.cpp:475 first positional = ROM dir
                }
                // no env fallback for dir (session-C finding)
            }
        }
        i += 1;
    }

    if dir.is_empty() {
        // origin: live.cpp:478-486 usage (shape mirrored; --list stub has no output yet)
        eprintln!(
            "使い方: live <rom ディレクトリ> [--midi 番号] [--latency ミリ秒] [--fast-midi]\n\
             \x20       [--exclusive]  デバイスを独り占めして待ち時間を詰める\n\
             \x20       [--factory]    覚えている設定を捨てて工場出荷状態で起動する\n\
             \x20       live <rom ディレクトリ> --waveout [--frames 数] [--buffers 数]\n\
             \x20       live --list        MIDI 入力の一覧"
        );
        std::process::exit(1);
    }

    // origin: live.cpp:490-497 — Machine::boot loads prog+wave and resets
    // (lib.rs:1682-1689). Errors print the error string like C++'s mu.error().
    let mut m = match Machine::boot(&dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // live.cpp:503-506 NVRAM: skipped (M5). live.cpp:507 mu.reset(): in boot().

    // origin: live.cpp:509-523 boot wait. Deviation: chunked run_cycles
    // instead of per-sample run_sample; pseudo-samples = cycles*RATE/CPU_HZ.
    println!("起動中..."); // live.cpp:510
    use std::io::Write;
    std::io::stdout().flush().unwrap(); // live.cpp:511

    const CHUNK: u64 = (CPU_HZ as u64) / 441; // ≈100 samples ≈ 2.27 ms per check
    let limit_cycles: u64 = 30 * CPU_HZ as u64; // live.cpp:513 (30.0 * RATE samples)
    let mut cycles_done: u64 = 0;
    let mut booted = false;
    while cycles_done < limit_cycles {
        m.run_cycles(CHUNK);
        cycles_done += CHUNK;
        if m.pair.borrow().midi_ready(0) {
            // live.cpp:516 mu.midi_ready()
            booted = true;
            let samples = cycles_done * RATE / CPU_HZ as u64;
            // live.cpp:522 " %.2f 秒"
            println!(" {:.2} 秒", samples as f64 / RATE as f64);
            break;
        }
    }
    if !booted {
        // DEVIATION (skeleton): disk exits 1 here (live.cpp:518-520 "起動しなかった").
        // In THIS build RE can never rise: SCI init sits behind the 2×SWP30 boot
        // handshake (M3 rows) — scr0 stays 0x00 through 30 s machine time while the
        // SH2 idles in its poll loop. Warn and continue so the smoke gate can run.
        println!("\n(スケルトン: 起動しなかった — RE 立たず; SWP30 は M3 未実装。続行)");
    }

    println!("live (rust skeleton): 起動 OK — audio/MIDI HAL は M6 (未実装)");

    // Deviation: disk runs until Ctrl+C; skeleton runs --seconds (default 1 s)
    // of machine time in the same chunks, then exits 0. No audio, MIDI, WAV.
    let run_secs = if seconds > 0.0 { seconds } else { 1.0 };
    let run_cycles = (run_secs * CPU_HZ as f64) as u64;
    let mut done = 0u64;
    while done < run_cycles {
        m.run_cycles(CHUNK);
        done += CHUNK;
    }
}
