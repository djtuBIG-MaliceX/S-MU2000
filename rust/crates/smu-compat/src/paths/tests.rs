//! Unit tests for the paths/console/compat transliteration
//! (`src/compat/paths.h`, `console.h`, `compat.cpp`).
//!
//! Two contracts pinned here that MUST NOT be "cleaned up":
//! - the join quirk (trailing-sep strip on empty rel), the exact FILETIME vs
//!   seconds mtime split, and the FNV-1a **64-bit** basis (nvram.h:39, not the
//!   32-bit native-engine one) — vectors captured from a compiled build of the
//!   exact C++ loops (nvram.h:39-44, compat.cpp:31-37/90-102).
//! - NO test touches %LOCALAPPDATA%\S-MU2000 (live NVRAM pitfall). Everything
//!   file-based runs under the OS temp dir through the `*_from` seams.

use super::*;

/// serializes everything that mutates the process-wide TRACE state and the
/// SMU2000_PCPROF env var (the C++ globals were "thread-safe" by convention;
/// cargo runs tests in parallel).
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmp(tag: &str) -> String {
    let mut p = std::env::temp_dir();
    p.push(format!("smu_compat_paths_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).expect("temp dir");
    p.to_string_lossy().into_owned()
}

const EOL_T: &str = if cfg!(windows) { "\r\n" } else { "\n" };

#[test]
fn dir_of_cuts_including_last_separator() {
    // origin paths.h:61-65; both separators count, result keeps it
    assert_eq!(detail::dir_of("C:\\a\\b.txt"), "C:\\a\\");
    assert_eq!(detail::dir_of("a/b"), "a/");
    assert_eq!(detail::dir_of("a/b/"), "a/b/"); // trailing sep IS the last one
    assert_eq!(detail::dir_of("no_sep"), "");
    assert_eq!(detail::dir_of(""), "");
}

#[test]
fn join_quirks_and_translation() {
    let sep = if cfg!(windows) { '\\' } else { '/' };
    assert_eq!(join("C:\\x\\", "roms/a.bin"), format!("C:\\x{sep}roms{sep}a.bin"));
    assert_eq!(join("C:\\x", "a/b"), format!("C:\\x{sep}a{sep}b"));
    // QUIRK (kept): empty rel strips dir's trailing separator (paths.h:166-169)
    assert_eq!(join("C:\\x\\", ""), "C:\\x");
    assert_eq!(join("C:\\x", ""), "C:\\x");
    // empty dir -> leading separator (paths.h:170)
    assert_eq!(join("", "a"), format!("{sep}a"));
}

#[test]
fn config_dir_suffix_semantics() {
    assert_eq!(
        config_dir_from("C:\\Users\\u\\AppData\\Local"),
        "C:\\Users\\u\\AppData\\Local\\S-MU2000\\"
    );
    assert_eq!(config_dir_from(""), "");
    // real config_dir() is pure string work — safe to check, creates nothing
    if cfg!(windows) {
        let d = config_dir();
        assert!(d.is_empty() || d.ends_with("\\S-MU2000\\"));
        let s = shared_config_dir();
        assert!(s.is_empty() || s.ends_with("\\S-MU2000\\"));
    }
    assert_eq!(shared_config_dir_from("D:\\pd"), "D:\\pd\\S-MU2000\\");
    assert_eq!(shared_config_dir_from(""), "");
}

#[test]
fn env_and_home_semantics() {
    // env(): unset == empty == "" (paths.h:177-181)
    assert_eq!(env("SMU_DEFINITELY_UNSET_VAR_XYZ"), "");
    #[cfg(windows)]
    if !env("USERPROFILE").is_empty() {
        assert_eq!(home_dir(), env("USERPROFILE")); // paths.h:187-188
    }
}

#[test]
fn fnv1a64_vectors_from_cpp_loop() {
    // Captured from the compiled nvram.h:37-46 loop (g++ -O2, this machine):
    //   u64 h = 0xcbf29ce484222325; for (u8 b : v) { h ^= b; h *= 0x100000001b3; }
    assert_eq!(fnv1a64(b""), 0xcbf29ce484222325); // offset basis, empty input
    assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
    assert_eq!(fnv1a64(b"abc"), 0xe71fa2190541574b);
    assert_eq!(fnv1a64(b"MU2000"), 0x541684789ecc47c5);
    assert_eq!(fnv1a64(b"S-MU2000"), 0x8c51764d2b07682d);
    let all: Vec<u8> = (0..=255u8).collect();
    assert_eq!(fnv1a64(&all), 0x4242dc5249c33625);
    assert_eq!(fnv1a64(b"abcabc"), 0xacc6223fa5a41b95);
}

#[test]
fn fnv1a_mix_is_the_same_fold() {
    // bootcache.h:88-91 mix == the nvram.h loop step; bootcache chains it onto
    // rom_key state — the streaming identity must hold exactly.
    let direct = fnv1a64(b"S-MU2000");
    let mut h = fnv1a64(b"S-MU"); // "rom_key" stand-in
    for b in b"2000" {
        h = fnv1a_mix(h, *b);
    }
    assert_eq!(h, direct);
    // u64 wrap (not 32-bit): the multiply must keep the full 64-bit width
    assert_eq!(
        fnv1a_mix(u64::MAX, 0),
        (u64::MAX ^ 0).wrapping_mul(FNV64_PRIME)
    );
}

#[test]
fn is_file_is_dir_agree_with_os() {
    let root = tmp("isfd");
    let file = join(&root, "f.bin");
    std::fs::write(&file, b"x").unwrap();
    assert!(is_file(&file) && !is_dir(&file));
    assert!(is_dir(&root) && !is_file(&root));
    let miss = join(&root, "gone.bin");
    assert!(!is_file(&miss) && !is_dir(&miss));
    assert!(!is_file("") && !is_dir("")); // early-out (paths.h:196/210)
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn make_dir_ensure_dir_levels() {
    let root = tmp("mkdir");
    let one = join(&root, "lvl1");
    assert!(make_dir(&one));
    assert!(make_dir(&one), "already existing counts as success (paths.h:267)");
    assert!(!make_dir("")); // paths.h:265
    // two levels at once — the NVRAM layout case (paths.h:273-275 comment)
    let deep = join(&join(&root, "a"), "b");
    assert!(ensure_dir(&deep));
    assert!(is_dir(&deep));
    assert!(ensure_dir(&deep), "already existing");
    assert!(!ensure_dir(""));
    // trailing separators are stripped before creating (paths.h:283-284)
    let tsep = format!("{deep}{}\\", std::path::MAIN_SEPARATOR);
    assert!(ensure_dir(&tsep));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn list_dir_names_and_mtimes_no_dirs() {
    let root = tmp("lsdir");
    std::fs::write(join(&root, "a.bin"), b"aaa").unwrap();
    std::fs::write(join(&root, "b.bin"), b"bb").unwrap();
    std::fs::create_dir(join(&root, "sub")).unwrap(); // must NOT appear
    let mut v = list_dir(&root);
    v.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["a.bin", "b.bin"]);
    assert!(v.iter().all(|e| e.mtime > 0));
    #[cfg(windows)]
    for e in &v {
        // Windows mtime = raw FILETIME (paths.h:237-239), 100-ns ticks:
        // any post-1971 time is >= 10^17. Seconds (POSIX branch) are not.
        assert!(e.mtime >= 10_000_000_000_000_000, "FILETIME units");
    }
    assert!(list_dir("").is_empty()); // paths.h:227
    assert!(list_dir(&join(&root, "nope")).is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn full_path_absolute_and_normalized() {
    assert_eq!(full_path(""), ""); // paths.h:310-311
    #[cfg(windows)]
    {
        let p = full_path("smu_probe\\..\\keep.txt");
        assert!(p.ends_with("keep.txt"), "{p}");
        assert!(!p.contains(".."), "{p}");
        assert!(p.contains(':'), "absolute: {p}");
    }
}

#[test]
fn exe_and_module_dir_agree() {
    // origin paths.h:70-99 / 327-356 (same image on Windows)
    #[cfg(windows)]
    {
        let ed = exe_dir();
        assert!(!ed.is_empty(), "exe_dir from GetModuleFileNameA");
        assert!(ed.ends_with('\\'), "trailing separator: {ed}");
        static ANCHOR: u8 = 7;
        let md = module_dir(&ANCHOR as *const u8 as *const std::ffi::c_void);
        assert_eq!(md, ed.trim_end_matches(['\\', '/']), "no trailing sep (paths.h:326)");
        assert_eq!(module_dir(std::ptr::null()), ""); // paths.h:329-330
    }
}

#[test]
fn local_time_layout() {
    // "YYYY-MM-DD HH:MM:SS", snprintf widths (paths.h:359-374)
    let s = local_time();
    #[cfg(windows)]
    {
        assert_eq!(s.len(), 19, "{s}");
        let b = s.as_bytes();
        assert_eq!((b[4], b[7], b[10], b[13], b[16]), (b'-', b'-', b' ', b':', b':'));
        assert!(b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || i == 10 || i == 13
            || i == 16 || c.is_ascii_digit()));
    }
}

#[test]
fn ensure_config_dir_from_makes_one_level() {
    // origin paths.h:377-398; Windows branch makes just the final level
    let root = tmp("cfgdir");
    let base = join(&root, "S-MU2000\\");
    let got = ensure_config_dir_from(&base);
    assert_eq!(got, base);
    assert!(is_dir(&join(&root, "S-MU2000")));
    assert_eq!(ensure_config_dir_from(""), ""); // empty config_dir -> ""
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn keyed_subdir_layout_and_name() {
    // bootcache.h:115-126 / nvram.h:54-65 shape: <base>\<boot|nvram>\<%016llx>.bin
    let root = tmp("keyed");
    let p = subdir_keyed_path(&root, "boot", 0xff);
    assert!(p.ends_with("boot\\00000000000000ff.bin") || p.ends_with("boot/00000000000000ff.bin"), "{p}");
    assert!(is_dir(&join(&root, "boot")));
    // the prune() filter (bootcache.h:257-258): 20 chars, hex, ".bin" at 16
    let name = p.rsplit(['\\', '/']).next().unwrap();
    assert_eq!(name.len(), 20);
    assert_eq!(&name[16..], ".bin");
    assert!(name[..16].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_eq!(subdir_keyed_path("", "nvram", 1), ""); // no base -> ""
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn tmp_plus_replace_atomic_write() {
    // origin nvram.h:84-100 save ("*.tmp") + bootcache.h:173 (".new") +
    // paths.h:296-304 replace_file
    let root = tmp("atomic");
    let target = join(&root, "0000000000000001.bin");
    assert!(write_file_atomic(&target, b"first", ".tmp"));
    assert_eq!(std::fs::read(&target).unwrap(), b"first");
    assert!(!std::path::Path::new(&format!("{target}.tmp")).exists(), "tmp consumed by replace");
    assert!(write_file_atomic(&target, b"second", ".tmp"), "replace over existing");
    assert_eq!(std::fs::read(&target).unwrap(), b"second");
    assert!(write_file_atomic(&target, b"snap", ".new"), "bootcache suffix");
    // failure path: no parent dir -> fopen fails, nothing written anywhere
    let bad = join(&join(&root, "missing"), "f.bin");
    assert!(!write_file_atomic(&bad, b"x", ".tmp"));
    assert!(!std::path::Path::new(&format!("{bad}.tmp")).exists());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn replace_file_overwrites_destination() {
    // origin paths.h:296-304 — MoveFileEx REPLACE_EXISTING is the whole point
    let root = tmp("repl");
    let a = join(&root, "a");
    let b = join(&root, "b");
    std::fs::write(&a, b"OLD").unwrap();
    std::fs::write(&b, b"NEW").unwrap();
    assert!(replace_file(&a, &b));
    assert_eq!(std::fs::read(&b).unwrap(), b"OLD");
    assert!(!std::path::Path::new(&a).exists());
    assert!(!replace_file(&a, &b), "renaming a missing source must fail");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn console_init_is_a_call_not_a_crash() {
    // origin console.h:27-32. SetConsoleOutputCP can legitimately fail on a
    // console-less (service/pipe) spawn, so no CP assertion — the gate is
    // that the port wires the call without panicking.
    init_console_utf8();
}

#[test]
fn atoi_matches_cpp() {
    // compat.cpp:55 uses std::atoi on SMU2000_PCPROF
    assert_eq!(atoi(""), 0);
    assert_eq!(atoi("2"), 2);
    assert_eq!(atoi(" 42abc"), 42);
    assert_eq!(atoi("-7"), -7);
    assert_eq!(atoi("+9x"), 9);
    assert_eq!(atoi("abc"), 0);
    assert_eq!(atoi("\t\n13"), 13);
}

#[test]
fn compat_verbose_roundtrip() {
    let _g = LOCK.lock().unwrap();
    assert!(!verbose(), "g_verbose born false (compat.cpp:14)");
    set_verbose(true);
    assert!(verbose());
    set_verbose(false);
}

#[test]
fn compat_pc_trace_skip_left_and_self_null() {
    // vectors from the compiled compat.cpp:90-102 run (this machine, text mode):
    //   5 calls, skip=2, left=2 -> EXACTLY two "802ABCD4 C=12345 abc\r\n" lines,
    //   sink self-nulled on the next call (g_pc_trace_after was null).
    let _g = LOCK.lock().unwrap();
    let root = tmp("pctrace");
    let f = join(&root, "trace.txt");
    assert!(open_pc_trace(&f, 2, 2));
    set_pc_cycles(12345);
    for _ in 0..5 {
        pc_trace(0x802ABCD4, " abc");
    }
    close_pc_trace();
    let want = format!("802ABCD4 C=12345 abc{EOL_T}");
    assert_eq!(std::fs::read(&f).unwrap(), want.repeat(2).as_bytes());
    assert!(!pc_trace_active(), "sink nulled once left hit 0 (compat.cpp:97)");
    assert_eq!(pc_cycles(), 12345);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn compat_pc_hash_block_lines_bit_exact() {
    // vectors from the compiled compat.cpp:31-37 run (this machine):
    // 0x20000 calls of pc_hash((u32)(i*7+3), i^0x12345678) emit
    //   "65536 98767743c0cb0000\r\n131072 5dfb329571da0000\r\n"
    let _g = LOCK.lock().unwrap();
    let root = tmp("pchash");
    let f = join(&root, "hash.txt");
    assert!(open_pc_hash(&f));
    for i in 0..0x20000u64 {
        pc_hash((i * 7 + 3) as u32, i ^ 0x12345678);
    }
    close_pc_hash();
    let want = format!(
        "65536 98767743c0cb0000{EOL_T}131072 5dfb329571da0000{EOL_T}"
    );
    assert_eq!(std::fs::read(&f).unwrap(), want.as_bytes());
    assert_eq!(pc_hash_count(), 0x20000);
    assert_eq!(pc_hash_value(), 0x5dfb329571da0000);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn compat_pc_prof_gate_report_and_null() {
    let _g = LOCK.lock().unwrap();
    std::env::remove_var("SMU2000_PCPROF");
    pc_prof_start(); // unset env -> untouched (compat.cpp:53-54)
    assert!(pc_prof_ptr().is_null());

    std::env::set_var("SMU2000_PCPROF", "2"); // top 2
    pc_prof_start();
    let p = pc_prof_ptr();
    assert!(!p.is_null());
    unsafe {
        *p.add(0x03) = 3;
        *p.add(0x10) = 5;
        *p.add(0x20) = 1;
    }
    let mut buf: Vec<u8> = Vec::new();
    pc_prof_report_to(&mut buf);
    let text = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "ブロックに入った回数 合計 9（上位 2）");
    assert_eq!(
        lines[1],
        "  JIT を抜けた訳: 遅延枠 0 / 割り込みの印 0 / 番地が外 0"
    );
    assert_eq!(lines[2], "  入り直し 0 回で 0 命令 = 1 回あたり 0.0 命令");
    assert_eq!(lines[3], "  JIT の速い道から外れたメモリ: 読み 0 / 書き 0");
    // descending by count, top 2; block addresses = idx * 0x40 (compat.cpp:86).
    // Spacings verified against the compiled C++ run: "  %08x" + "%10u"
    // (2+9 pad) + "%5.1f" (2+1 pad) + "%": 11 spaces before the count, 3 before the %.
    let tail = |addr: u32, cnt: u32, pct: &str| -> String {
        format!("  {addr:08x}{}{cnt}{}{pct}%", " ".repeat(11), " ".repeat(3))
    };
    assert_eq!(lines[4], tail(0x400, 5, "55.6")); // 100*5/9 = 55.55.. -> %.1f
    assert_eq!(lines[5], tail(0xC0, 3, "33.3"));  // 100*3/9 = 33.33..
    assert_eq!(lines.len(), 6, "top=2 caps the tail (compat.cpp:85)");
    std::env::remove_var("SMU2000_PCPROF");
}
