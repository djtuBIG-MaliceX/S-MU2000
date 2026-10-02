// license:BSD-3-Clause
//
// origin: src/nvram.h (105 L) — ワーク RAM（NVRAM）をファイルに残す。
// Ledger row `nvram` (M5/M6).
//
// 実機の 0x400000-0x43ffff（256KB）は電池で保持されていて、MAME も NVRAM と
// して保存している。置いておく場は %LOCALAPPDATA%\S-MU2000\nvram\
// <プログラム ROM のハッシュ>.bin で、中身は 256KB をそのまま。
// 使い方: reset() の前に load()、止めたあとに save()。**起動に成功したとき
// だけ save() すること。** render や make test は使わない。
//
// DEVIATIONS (disclosed):
// - `*_from(base)` variants take the config dir as a parameter (test seam,
//   same precedent as paths::config_dir_from): the disk shape calls
//   paths::ensure_config_dir() (nvram.h:56) and forwards. The unit tests and
//   the %TEMP% interchange harness NEVER touch the real config dir.
// - mu.program_rom() is a shared_ptr (mu2000.h:47-49); an empty Vec is the
//   Rust stand-in for the null view — the exact convention bootcache.rs:80
//   already discloses for the same field.
// - fopen/fread become File::open + a read loop; the `size + 1` buffer gate
//   (nvram.h:76) is kept EXACTLY: `got` is the number of bytes read up to
//   size+1, so `got == size` (nvram.h:79) holds only for a file of exactly
//   `size` bytes. An I/O error mid-read stops the loop with a short `got`
//   exactly like fread's partial return.
// - save()'s fopen("wb")/fwrite/fclose/remove/replace chain is the already
//   pinned paths::write_file_atomic with the nvram.h:89 ".tmp" suffix
//   (its fclose-error ≈ sync_all note is disclosed at paths.rs:634-648).

use std::io::Read;

use crate::Machine;
use smu_compat::paths;

/// origin: nvram.h:37-46 `rom_key` — プログラム ROM の FNV-1a 64bit。4MB で数ミリ秒。
/// :40 `if (const auto prog = mu.program_rom())` — no ROM (null view / empty
/// Vec) skips the loop and returns the offset basis; :42-43 are the pinned
/// `paths::fnv1a64` loop (M1 row: vectors vs the compiled C++ loop).
pub fn rom_key(mu: &Machine) -> u64 {
    paths::fnv1a64(&mu.soc.bus.rom) // :41-44 (mu.program_rom() == soc.bus.rom, mu2000.h:49)
}

/// origin: nvram.h:54-65 `path` — 置き場。作れなければ空。
/// Disk shape: ensure_config_dir() then the nvram.h:54-65 body (`*_from`).
pub fn path(mu: &Machine) -> String {
    path_from(mu, &paths::ensure_config_dir()) // :56 (the LOCALAPPDATA lookup)
}

/// origin: nvram.h:55-65 with the base directory as a parameter (see
/// DEVIATIONS). :57-58 empty base -> {}; :59-61 join(base,"nvram") +
/// ensure_dir; :62-63 snprintf("%016llx.bin") of rom_key; :64 join.
/// paths::subdir_keyed_path is that exact shape (paths.rs:587-600, the
/// "<settings>\\nvram\\<%016llx>.bin" comment there names nvram.h:54-65).
pub fn path_from(mu: &Machine, base: &str) -> String {
    paths::subdir_keyed_path(base, "nvram", rom_key(mu)) // :59-64
}

/// origin: mu2000.h:82-88 `set_nvram` — 入れるのは reset() の前。
/// 大きさが違えば false (:84-85), otherwise memcpy (:86).
fn set_nvram(mu: &mut Machine, p: &[u8]) -> bool {
    if p.len() != mu.soc.bus.ram.len() {
        return false; // mu2000.h:84-85
    }
    mu.soc.bus.ram.copy_from_slice(p); // mu2000.h:86 memcpy(m_ram.data(), p, n)
    true // mu2000.h:87
}

/// origin: nvram.h:68-80 `load` — reset() の前に呼ぶ。無い・大きさが違うときは
/// 何もせず false（工場出荷状態で起動する）。Disk shape via ensure_config_dir().
pub fn load(mu: &mut Machine) -> bool {
    load_from(mu, &paths::ensure_config_dir())
}

/// origin: nvram.h:69-80 with the config base injected (see DEVIATIONS).
pub fn load_from(mu: &mut Machine, base: &str) -> bool {
    let p = path_from(mu, base); // :70
    if p.is_empty() {
        return false; // :71-72
    }
    let mut f = match std::fs::File::open(&p) {
        // :73-75 fopen(p, "rb"); if (!f) return false
        Ok(f) => f,
        Err(_) => return false,
    };
    // :76 std::vector<u8> buf(mu.nvram().size() + 1) — the +1 is the
    // oversized-file detector (a 0x40001-byte file reads `got == size + 1`
    // and fails the :79 gate); never "simplify" it away.
    let size = mu.soc.bus.ram.len(); // :76/:79 mu.nvram().size() == 0x40000 (mu2000.cpp:85)
    let mut buf = vec![0u8; size + 1];
    let mut got = 0usize;
    // :77 fread(buf.data(), 1, buf.size(), f) — loop until full or EOF;
    // a hard error leaves the short count (fread partial-return parity).
    while got < buf.len() {
        match f.read(&mut buf[got..]) {
            Ok(0) => break,                // EOF
            Ok(n) => got += n,             // :77 accumulated
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,               // short read == fread error
        }
    }
    // :78 fclose(f) — Rust drops f here
    if got != size {
        return false; // :79 first half: 大きさが違うときは何もせず false
    }
    set_nvram(mu, &buf[..got]) // :79 second half (size check inside never trips here)
}

/// origin: nvram.h:84-100 `save` — 機械が止まっているときに呼ぶ。一時ファイルに
/// 書いてから置き換えるので、途中で落ちても前の NVRAM は壊れない。
/// Disk shape via ensure_config_dir().
pub fn save(mu: &Machine) -> bool {
    save_from(mu, &paths::ensure_config_dir())
}

/// origin: nvram.h:85-100 with the config base injected (see DEVIATIONS).
pub fn save_from(mu: &Machine, base: &str) -> bool {
    let p = path_from(mu, base); // :86
    if p.is_empty() {
        return false; // :87-88
    }
    // :89-99 p + ".tmp" / fopen("wb") / fwrite(ram) / fclose-error ||
    // short-write -> remove(tmp) / replace_file(tmp, p) — exactly
    // paths::write_file_atomic with the caller's ".tmp" suffix.
    paths::write_file_atomic(&p, &mu.soc.bus.ram[..], ".tmp") // :89/:90-94/:95-98/:99
}

#[cfg(test)]
mod tests {
    use super::*;

    // Deterministic synthetic content only (LCG, no ROMs — harness precedent
    // from sessions M/K/Q). Must stay byte-identical to the C++ interchange
    // harness gt.cpp (l cg constants there).
    const PROG_BYTES: usize = 4 << 20; // 4MB program ROM stand-in

    fn lcg(s: &mut u64) -> u64 {
        *s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *s >> 33
    }

    fn fake_prog() -> Vec<u8> {
        let mut s = 0x20261002u64; // gt.cpp seed
        (0..PROG_BYTES).map(|_| lcg(&mut s) as u8).collect()
    }

    fn pattern_ram() -> Vec<u8> {
        let mut s = 0x5bd1e997ba57087u64; // gt.cpp RAM seed
        (0..0x40000usize).map(|_| lcg(&mut s) as u8).collect()
    }

    /// rom_key of the 4MB fake ROM, minted by the REAL compiled C++
    /// nvram::rom_key in the %TEMP% interchange harness (nvramgt/gt.cpp,
    /// linked against build/src/mu2000.o, 2026-10-02): key
    /// `95e194267f637e3b`, file SHA1 `8EF4A086…` (262144 B).
    const GT_PROG_KEY: &str = "95e194267f637e3b";

    fn scratch(tag: &str) -> String {
        // NEVER the real config dir: a private %TEMP% base (paths::config_dir
        // would say <base>\S-MU2000\, so the harness/`*_from` callers below
        // feed this through config_dir_from + ensure_config_dir_from)
        let d = std::env::temp_dir().join(format!("smu_nvram_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    fn base_of(la: &str) -> String {
        // `<la>\S-MU2000\`, created — same composition as disk
        // ensure_config_dir() == ensure_config_dir_from(config_dir())
        // (paths.rs:556-557/:193-198 append the S-MU2000 layer; bare
        // ensure_config_dir_from does NOT — audit fix W5b: the first
        // harness run missed the C++ file under la\nvram\ for this)
        paths::ensure_config_dir_from(&paths::config_dir_from(la)) // :56
    }

    #[test]
    fn rom_key_empty_rom_is_offset_basis() {
        // nvram.h:40 — null program_rom skips the loop (:39 basis returned)
        let mu = Machine::new(Vec::new());
        assert_eq!(rom_key(&mu), 0xcbf29ce484222325);
    }

    #[test]
    fn rom_key_matches_pinned_cpp_value() {
        if GT_PROG_KEY == "PLACEHOLDER" {
            return; // harness not run in this session
        }
        let mu = Machine::new(fake_prog());
        assert_eq!(rom_key(&mu), u64::from_str_radix(GT_PROG_KEY, 16).unwrap());
    }

    #[test]
    fn path_names_nvram_subdir_and_creates_it() {
        // nvram.h:59-64: join(base,"nvram") + ensure_dir + "%016llx.bin"
        let la = scratch("path");
        let mu = Machine::new(vec![0x5au8; 1024]);
        let base = base_of(&la);
        let p = path_from(&mu, &base);
        let key = format!("{:016x}", rom_key(&mu));
        assert!(p.ends_with(&format!("\\nvram\\{key}.bin")), "path={p}");
        assert!(paths::is_dir(&paths::join(&base, "nvram")));
        // empty base -> "" (nvram.h:57-58)
        assert_eq!(path_from(&mu, ""), "");
        let _ = std::fs::remove_dir_all(la);
    }

    #[test]
    fn load_exact_size_accepts_everything_else_leaves_ram() {
        let la = scratch("load");
        let base = base_of(&la);
        let pat = pattern_ram();
        let mut mu = Machine::new(vec![0x5au8; 64]);
        let dir = paths::join(&base, "nvram");
        let p = path_from(&mu, &base);

        // missing file -> false (nvram.h:73-75)
        assert!(!load_from(&mut mu, &base));

        // one byte short -> false, RAM untouched (nvram.h:77-79)
        std::fs::write(&p, &pat[..pat.len() - 1]).unwrap();
        assert!(!load_from(&mut mu, &base));
        assert_eq!(&mu.soc.bus.ram[..], &vec![0u8; 0x40000][..]);

        // exactly 0x40000 -> true, RAM == file (nvram.h:79 set_nvram)
        std::fs::write(&p, &pat).unwrap();
        assert!(load_from(&mut mu, &base));
        assert_eq!(&mu.soc.bus.ram[..], &pat[..]);

        // one byte over -> false AND RAM keeps the last good state
        // (buf.size()+1 gate, nvram.h:76/:79)
        let mut over = pat.clone();
        over.push(0xff);
        std::fs::write(&p, &over).unwrap();
        assert!(!load_from(&mut mu, &base));
        assert_eq!(&mu.soc.bus.ram[..], &pat[..]);

        // empty base -> false (nvram.h:71-72)
        assert!(!load_from(&mut mu, ""));
        let _ = std::fs::remove_dir_all(la);
        let _ = dir;
    }

    #[test]
    fn save_writes_raw_ram_and_replaces_atomically() {
        let la = scratch("save");
        let base = base_of(&la);
        let pat = pattern_ram();
        let mut mu = Machine::new(vec![0x5au8; 64]);
        mu.soc.bus.ram.copy_from_slice(&pat);

        assert!(save_from(&mu, &base));
        let p = path_from(&mu, &base);
        let on = std::fs::read(&p).unwrap();
        assert_eq!(on, pat); // 中身は 256KB をそのまま (nvram.h:93-94)
        assert!(!std::path::Path::new(&format!("{p}.tmp")).exists()); // tmp replaced/removed
        assert!(!save_from(&mu, "")); // :87-88 empty base -> false

        // a second save over a full file replaces it wholesale (replace_file)
        let mut pat2 = pat.clone();
        pat2[0] ^= 0xa5;
        mu.soc.bus.ram.copy_from_slice(&pat2);
        assert!(save_from(&mu, &base));
        assert_eq!(std::fs::read(&p).unwrap(), pat2);

        // round-trip into a zeroed machine (load before reset, nvram.h:15)
        let mut back = Machine::new(vec![0x5au8; 64]);
        assert!(load_from(&mut back, &base));
        assert_eq!(&back.soc.bus.ram[..], &pat2[..]);
        let _ = std::fs::remove_dir_all(la);
    }

    // ---- %TEMP% interchange gate (M5-W5b): driven by the launcher, NOT by
    // the real config dir. Harness (gt.cpp, REAL nvram.h) writes -> Rust
    // loads; Rust writes -> C++ loads. ----

    #[test]
    fn gt_cpp_file_into_rust() {
        // SMU_NVRAM_GT = the scratch LOCALAPPDATA the C++ `save` pass used.
        let la = match std::env::var("SMU_NVRAM_GT") {
            Ok(v) => v,
            Err(_) => return,
        };
        if GT_PROG_KEY == "PLACEHOLDER" {
            panic!("SMU_NVRAM_GT set but no pinned rom_key vector");
        }
        let base = base_of(&la);
        let mut mu = Machine::new(fake_prog());
        assert_eq!(format!("{:016x}", rom_key(&mu)), GT_PROG_KEY);
        let p = path_from(&mu, &base);
        let ok = load_from(&mut mu, &base);
        assert!(
            ok,
            "Rust load of C++-written nvram failed: p={p} exists={} len={:?}",
            std::path::Path::new(&p).exists(),
            std::fs::metadata(&p).map(|m| m.len()),
        );
        assert_eq!(&mu.soc.bus.ram[..], &pattern_ram()[..]); // byte-identical RAM
        let _ = std::fs::remove_dir_all(&la);
    }

    #[test]
    fn rust_writes_for_cpp_reader() {
        // SMU_NVRAM_OUT = scratch LOCALAPPDATA for the Rust `save` pass;
        // SMU_NVRAM_REF = where to drop the expected RAM bytes for C++ cmp.
        let la = match std::env::var("SMU_NVRAM_OUT") {
            Ok(v) => v,
            Err(_) => return,
        };
        let base = base_of(&la);
        let pat = pattern_ram();
        let mut mu = Machine::new(fake_prog());
        mu.soc.bus.ram.copy_from_slice(&pat);
        assert!(save_from(&mu, &base));
        let p = path_from(&mu, &base);
        assert_eq!(std::fs::read(&p).unwrap(), pat);
        assert!(!std::path::Path::new(&format!("{p}.tmp")).exists());
        if let Ok(r) = std::env::var("SMU_NVRAM_REF") {
            std::fs::write(r, &pat).unwrap();
        }
    }
}
