// license:BSD-3-Clause
//
// origin: src/boot.cpp (whole file, 115 L) — 起動の確認, transliterated onto
// smu_machine::Machine.
//
// Deviations (reported to ledger author, do not "fix" silently):
// - Disk fuses nothing here: this bin loads prog/wave (:60-67), warns on the
//   sin-table (:68-69), opens `--trace-swp` + `set_swp_trace` (:71-79), THEN
//   resets (:81) — the trace sink is live before reset exactly as disk orders.
//   `Machine::boot` (fused load+reset) is NOT used here for that reason.
// - `--trace-swp`/`--reads` are wired (reg-dispatch row, M3): the machine emits
//   the `R ` / `W ` boot-trace from the SWP bus window (mu2000.cpp:861-918),
//   byte-for-byte with the C++ `--trace-swp` sink. `--trace-port` (g_port_trace,
//   :92-93) stays accept-ignored (sink unported; fopen failure ignored on disk).
// - fclose hf/pf/tf (:108-113) = Rust Drop at end of main; per-sink bytes
//   unaffected (separate sinks, all flushed before process exit).
// - stdout/stderr written with CRLF on Windows: MSVCRT opens all three std
//   streams text-mode, so the C++ fprintf("\n") emits CRLF even through a
//   pipe (CRLF pitfall, doc/testing.md).

use smu_compat::{paths, roms};
use smu_machine::Machine;

const EOL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// origin: boot.cpp:48/50/56 `std::strtoull(s, nullptr, 0)` — base 0:
/// leading ws, optional sign, 0x/0X hex prefix, leading-0 octal, else
/// decimal; suffix after the first invalid char is ignored (endptr). C99:
/// empty subject -> 0; '-' negates in the return space; overflow ->
/// ULLONG_MAX (ERANGE).
fn strtoull0(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1; // strtoul leading isspace
    }
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let mut radix = 10u32;
    if i + 1 < b.len() && b[i] == b'0' && (b[i + 1] | 0x20) == b'x' {
        radix = 16;
        i += 2;
    } else if i < b.len() && b[i] == b'0' {
        radix = 8;
        i += 1;
    }
    let digit = |c: u8, radix: u32| -> Option<u32> {
        let d = match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'f' => (c - b'a') as u32 + 10,
            b'A'..=b'F' => (c - b'A') as u32 + 10,
            _ => 255,
        };
        if d < radix {
            Some(d)
        } else {
            None
        }
    };
    let mut val: u64 = 0;
    let mut any = false;
    let mut overflow = false;
    while i < b.len() {
        if let Some(d) = digit(b[i], radix) {
            // ULLONG_MAX saturate on overflow (C strtoull ERANGE path)
            match val.checked_mul(radix as u64) {
                Some(v) => match v.checked_add(d as u64) {
                    Some(v2) => val = v2,
                    None => overflow = true,
                },
                None => overflow = true,
            }
            any = true;
            i += 1;
        } else {
            break; // endptr stops here; rest of the token ignored
        }
    }
    if !any {
        return 0; // empty subject sequence
    }
    if overflow {
        return u64::MAX;
    }
    if neg {
        0u64.wrapping_sub(val)
    } else {
        val
    }
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.is_empty() {
        // origin: boot.cpp:19-23 usage (exact string)
        eprint!(
            "使い方: boot <rom ディレクトリ> [サイクル数] [--trace-swp <出力先>] [-v]{EOL}"
        );
        std::process::exit(1);
    }

    let dir = raw[0].clone(); // :25
    let mut cycles: u64 = 28_000_000; // :26 既定で 1 秒ぶん
    let mut trace: Option<String> = None; // :27
    let mut with_reads = false; // :28
    let mut pctrace: Option<String> = None; // :29
    let mut pchash: Option<String> = None; // :30
    let mut porttrace: Option<String> = None; // :31
    let mut updtrace: Option<String> = None; // :32
    let mut pcskip: u64 = 0; // :33
    let mut pccount: u64 = 2_000_000; // :34

    // origin: boot.cpp:36-57 flag loop (raw[k] == argv[k+1])
    let mut i = 1usize;
    while i < raw.len() {
        let a = raw[i].as_str();
        if a == "--trace-swp" && i + 1 < raw.len() {
            trace = Some(raw[i + 1].clone());
            i += 2;
        } else if a == "--trace-pc" && i + 1 < raw.len() {
            pctrace = Some(raw[i + 1].clone());
            i += 2;
        } else if a == "--hash-pc" && i + 1 < raw.len() {
            pchash = Some(raw[i + 1].clone());
            i += 2;
        } else if a == "--trace-upd" && i + 1 < raw.len() {
            updtrace = Some(raw[i + 1].clone());
            i += 2;
        } else if a == "--trace-port" && i + 1 < raw.len() {
            porttrace = Some(raw[i + 1].clone());
            i += 2;
        } else if a == "--pc-skip" && i + 1 < raw.len() {
            pcskip = strtoull0(&raw[i + 1]);
            i += 2;
        } else if a == "--pc-count" && i + 1 < raw.len() {
            pccount = strtoull0(&raw[i + 1]);
            i += 2;
        } else if a == "--reads" {
            with_reads = true; // :51-52 (swp sink is M3; recorded, unused)
            i += 1;
        } else if a == "-v" {
            paths::set_verbose(true); // :53-54 g_verbose
            i += 1;
        } else {
            cycles = strtoull0(a); // :55-56 positional
            i += 1;
        }
    }

    // origin: boot.cpp:59-81 — faithful order (load prog, load wave, sin-table
    // warning, trace-open + set_swp_trace, THEN reset). Machine::boot fuses
    // reset at :81, so this bin inlines the steps to keep the trace sink open
    // BEFORE reset as disk does (:78 set_swp_trace precedes :81 reset). The
    // SWP register window is live from the first instruction after reset.
    let prog = match roms::load_program(&format!("{dir}/mu2000_flash.bin")) {
        Ok(p) => p,
        Err(e) => {
            eprint!("{e}{EOL}"); // :61 mu.error()
            std::process::exit(1); // :62
        }
    };
    let wave = match roms::load_wave(&format!("{dir}/dump")) {
        Ok(w) => w,
        Err(e) => {
            eprint!("{e}{EOL}"); // :65 mu.error()
            std::process::exit(1); // :66
        }
    };
    let mut m = Machine::new(prog);
    m.set_wave_rom(wave); // :64 (W-SAMP1: stashes + pins both devices, mu2000.cpp:395-411)

    // :68-69 sin-table: WARNING only, exit code unaffected.
    if let Err(e) = roms::load_sintab(&format!("{dir}/standin/sin-table.bin")) {
        eprint!("警告: {e}{EOL}"); // :69
    }

    // :71-79 --trace-swp: fopen failure is fatal ("書けない"); the handle is
    // then handed to the machine (set_swp_trace, mu2000.h:353-354) BEFORE reset
    // (:81), exactly as the C++ passes the just-opened FILE *tf.
    if let Some(t) = &trace {
        match std::fs::File::create(t) {
            Ok(f) => m.set_swp_trace(f, with_reads), // :78
            Err(_) => {
                eprint!("書けない: {t}{EOL}"); // :75
                std::process::exit(1); // :76
            }
        }
    }

    m.reset(); // :81 — AFTER the trace sink is installed

    // origin: boot.cpp:83-97 — AFTER reset: pf/skip/left, upd, port, hash.
    // Writers (paths::open_pc_trace / open_pc_hash, Machine upd sink) already
    // emit CRLF on Windows — no re-wrap here.
    if let Some(p) = &pctrace {
        m.set_trace_pc(p, pcskip, pccount); // :84-89
    }
    if let Some(p) = &updtrace {
        m.set_upd_trace(p); // :90-91
    }
    let portf = match &porttrace {
        // :92-93 g_port_trace: fopen, failure ignored on disk; sink unported
        Some(p) => std::fs::File::create(p).ok(),
        None => None,
    };
    if let Some(p) = &pchash {
        m.set_hash_pc(p); // :94-97
    }

    // :98 TWO spaces before PC
    print!("リセット後  PC={:08x}{EOL}", m.pc());

    // origin: boot.cpp:101-106 少しずつ走らせて、進んでいるか見る
    let step = if cycles / 10 != 0 { cycles / 10 } else { cycles }; // :101
    let mut done: u64 = 0;
    while done < cycles {
        m.run_cycles(step);
        print!("{:>10} サイクル  PC={:08x}{EOL}", m.total_cycles(), m.pc()); // :104-105
        done = done.wrapping_add(step); // u64 wrap = C++ u64 +=
    }

    // Row gate (M3 reg dispatch): no "deferred" SWP slot may be touched during
    // boot. A non-zero total means a not-yet-ported handler was hit -> that
    // handler becomes this row's problem. Silent when 0 (keeps stderr identical
    // to the C++ golden); only a real violation prints.
    let dh = m.swp_deferred_total();
    if dh != 0 {
        eprintln!("DEFERRED_HITS={dh}");
    }

    // :108-113 fclose(hf), fclose(pf), fclose(tf) — Rust: drops below / the swp
    // sink rides in the machine; all bytes were already flushed per line, so
    // ordering is unobservable.
    drop(portf);
    drop(m);
}
