//! Row gate for `wiring/bus map` (M4-M5 smu-machine): boot the REAL
//! `Machine` (all paired devices wired) for the C++ boot.cpp default of
//! 1 s = 28 MHz cycles and replay the `--trace-upd` stream against the
//! golden `rust/tests/golden/trace_upd_boot.txt` BYTE-IDENTICAL.
//!
//! origin: src/boot.cpp:26 `u64 cycles = 28000000; // 既定で 1 秒ぶん`
//!
//! Run in RELEASE — the replay is millions of instructions (dev profile
//! will not finish):
//!   cargo test --release --test boot_golden    (from `rust\`)

use std::path::{Path, PathBuf};

/// origin: boot.cpp:26 — default boot length in cycles (1 s at 28 MHz).
const TOTAL: u64 = 28_000_000;
/// Chunk per `run_cycles` call to exercise the :1158-1161 overrun seam at
/// every call boundary (C++ chunks at cycles/10, boot.cpp:101; observably
/// equivalent — the loop steps one instruction per iteration either way).
const CHUNK: u64 = 65_536;

/// Walk up from the package dir (crates/smu-machine; this file is mounted
/// via `[[test]] path = "../../tests/boot_golden.rs"`) to the repo root,
/// i.e. the ancestor holding `roms/` and `rust/tests/golden/`.
fn repo_root() -> Option<PathBuf> {
    let mut d: Option<&Path> = Some(Path::new(env!("CARGO_MANIFEST_DIR")));
    while let Some(dir) = d {
        if dir.join("roms").join("mu2000_flash.bin").exists()
            && dir.join("rust").join("tests").join("golden").is_dir()
        {
            return Some(dir.to_path_buf());
        }
        d = dir.parent();
    }
    None
}

#[test]
fn boot_golden_replay() {
    let root = match repo_root() {
        Some(r) => r,
        None => {
            eprintln!("SKIP boot_golden: repo root with roms/mu2000_flash.bin not found");
            return;
        }
    };
    let golden_path = root
        .join("rust")
        .join("tests")
        .join("golden")
        .join("trace_upd_boot.txt");
    let golden = match std::fs::read(&golden_path) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("SKIP boot_golden: golden fixture unreadable {golden_path:?}: {e}");
            return;
        }
    };
    let rom_dir = root.join("roms");
    let mut m = match smu_machine::Machine::boot(&rom_dir.display().to_string()) {
        Ok(m) => m,
        Err(e) => {
            // ROM/wave absent on this box -> graceful skip (never fake state).
            eprintln!("SKIP boot_golden: Machine::boot({rom_dir:?}) failed: {e}");
            return;
        }
    };

    let tmp = std::env::temp_dir().join("smu_boot_golden_upd.txt");
    assert!(
        m.set_upd_trace(&tmp.display().to_string()),
        "set_upd_trace({tmp:?}) failed"
    );

    let mut done = 0u64;
    while done < TOTAL {
        let want = CHUNK.min(TOTAL - done); // exact 28 M budget, no tail overshoot
        m.run_cycles(want);
        done += want;
    }
    let total = m.total_cycles();
    let pc = m.pc();
    drop(m); // File writes are unbuffered; drop closes the handle anyway.

    let mine = std::fs::read(&tmp).expect("read back upd trace");
    if mine != golden {
        let gl: Vec<&[u8]> = golden.split(|b| *b == b'\n').collect();
        let ml: Vec<&[u8]> = mine.split(|b| *b == b'\n').collect();
        for i in 0..gl.len().max(ml.len()) {
            let g = gl.get(i).copied().unwrap_or(b"<no line>");
            let r = ml.get(i).copied().unwrap_or(b"<no line>");
            if g != r {
                panic!(
                    "upd golden DIVERGES at line {i} (of {} golden / {} mine):\n  golden: {}\n  mine:   {}\n  machine end pc={pc:08x} total_cycles={total}",
                    gl.len(),
                    ml.len(),
                    String::from_utf8_lossy(g),
                    String::from_utf8_lossy(r),
                );
            }
        }
        panic!("byte mismatch with identical line splits (trailing bytes); golden {} vs mine {}", golden.len(), mine.len());
    }
    eprintln!(
        "boot_golden: PASS byte-identical ({} B, {} upd lines; cycles requested={done} executed={total}, end pc={pc:08x})",
        golden.len(),
        golden.iter().filter(|b| **b == b'\n').count()
    );
}
