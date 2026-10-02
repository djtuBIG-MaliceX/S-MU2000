// license:BSD-3-Clause
//
// Row `nvram` (M5/M6) integration gate: the DISK-shape API (path/load/save
// with no `*_from` seam) driven under a redirected LOCALAPPDATA. paths.rs:164-168
// reads env LOCALAPPDATA on Windows (== paths.h:106 std::getenv), so pointing
// it at a private %TEMP% dir exercises the real %LOCALAPPDATA%\S-MU2000\nvram\
// layout shape without EVER touching the live config dir (the paths row's
// isolation precedent; the live dir is never even resolved here).
// env mutation is process-wide: this file owns it, serialized by a mutex
// (cargo builds every integration file as its own process; the lib unit
// tests in src/nvram.rs use the `*_from` seams and ignore env entirely).

use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use smu_machine::nvram;
use smu_machine::Machine;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("smu_nvram_it_{}_{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ram_pattern(seed: u8) -> Vec<u8> {
    // deterministic synthetic stand-in (no ROM bytes, Invariant-3 content)
    (0..0x40000usize)
        .map(|i| (((i as u32).wrapping_mul(2654435761)) >> 13) as u8 ^ seed)
        .collect()
}

#[test]
fn disk_shape_roundtrip_under_redirected_localappdata() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let la = scratch("disk");
    let prev = std::env::var_os("LOCALAPPDATA");
    std::env::set_var("LOCALAPPDATA", &la);

    let r = std::panic::catch_unwind(|| {
        let mut mu = Machine::new(vec![0x5au8; 4096]);
        let pat = ram_pattern(0);
        mu.soc.bus.ram.copy_from_slice(&pat);

        // nvram.h:54-65 path(): <la>\S-MU2000\nvram\<%016llx>.bin, dir made
        let p = nvram::path(&mu);
        let key = format!("{:016x}", nvram::rom_key(&mu));
        let expect = Path::new(&la).join("S-MU2000").join("nvram").join(format!("{key}.bin"));
        assert_eq!(Path::new(&p), &expect, "disk path shape");
        assert!(expect.parent().unwrap().is_dir(), "nvram dir created");

        // nvram.h:84-100 save(): raw 256KB, p+".tmp" then replace, no residue
        assert!(nvram::save(&mu));
        assert_eq!(std::fs::read(&expect).unwrap(), pat);
        assert!(!Path::new(&format!("{p}.tmp")).exists(), "no .tmp residue");

        // nvram.h:68-80 load(): factory-zero machine comes back byte-exact
        let mut back = Machine::new(vec![0x5au8; 4096]);
        assert!(nvram::load(&mut back));
        assert_eq!(&back.soc.bus.ram[..], &pat[..]);

        // different program ROM -> different key -> clean miss, fail-open
        // (nvram.h:11-13 the ROM hash IS the identity; :67 工場出荷で起動)
        let mut other = Machine::new(vec![0x5bu8; 4096]);
        assert!(!nvram::load(&mut other));
        assert!(other.soc.bus.ram.iter().all(|&b| b == 0), "miss leaves RAM zero");

        // one byte over -> the buf.size()+1 gate fails the load, RAM kept
        // (nvram.h:76-79)
        let mut over = pat.clone();
        over.push(0xff);
        std::fs::write(&expect, &over).unwrap();
        assert!(!nvram::load(&mut back));
        assert_eq!(&back.soc.bus.ram[..], &pat[..], "bad file leaves RAM");
    });

    match prev {
        Some(v) => std::env::set_var("LOCALAPPDATA", v),
        None => std::env::remove_var("LOCALAPPDATA"),
    }
    let _ = std::fs::remove_dir_all(&la);
    assert!(r.is_ok(), "disk-shape roundtrip panicked (see stdout)");
}
