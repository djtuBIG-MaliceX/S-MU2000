//! Compat paths / console / trace sinks — transliteration of
//! `src/compat/paths.h`, `src/compat/console.h`, `src/compat/compat.cpp`.
//!
//! Ledger row `compat/paths+console` (M1). Transliterate first: same string
//! building, same separator handling, same Win32 calls (ANSI A-APIs, byte-for
//! byte like the C++ `c_str()`), same `tmp + replace` write shape.
//!
//! Deliberate, documented choices (see PORTING_LEDGER report for this row):
//! - `String` (UTF-8) stands in for C++ raw ACP bytes. Identical for the ASCII
//!   paths of this project; non-ASCII/legacy-Ansi-usernames are a known
//!   cross-language fidelity seam, not a behavior change on the dev machine.
//! - Windows is the parity target (the C++ ground truth build). Non-Windows
//!   `#else` branches are mirrored where they are portable; `local_time`'s
//!   POSIX branch needs `localtime_r` (a libc dep) and is stubbed empty —
//!   the macOS/Linux HAL is backlog per AGENTS.md.
//! - Trace file/stdout writers emit `\r\n` on Windows themselves: C++ text
//!   mode `fprintf`/`printf` did the translation for it (CRLF pitfall,
//!   PORTING_LEDGER 2026-09-30).
//! - `config_dir_from` / `shared_config_dir_from` / `ensure_config_dir_from` /
//!   `subdir_keyed_path` are the C++ functions with the env/root lookup pulled
//!   out as a parameter so unit tests can run under the OS temp dir without
//!   touching the live `%LOCALAPPDATA%\S-MU2000` (NVRAM pitfall). The public
//!   entry points call them with exactly what the C++ reads from the
//!   environment — no observable behavior change.

use std::ffi::c_void;
use std::io::Write;

/// C++ text-mode `\n` on this platform (trace/console writers, see pitfall).
const EOL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// c_str() equivalent: bytes up to the first NUL, NUL-terminated (paths.h
/// hands `p.c_str()` to every A-API; an embedded NUL truncates there).
#[cfg(windows)]
fn nt(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().take_while(|b| *b != 0).collect();
    v.push(0);
    v
}

#[cfg(windows)]
fn utf8_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(windows)]
mod win {
    use super::c_void;

    pub const MAX_PATH: u32 = 260;
    pub const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
    pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    pub const ERROR_ALREADY_EXISTS: u32 = 183;
    pub const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    pub const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    pub const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x1;
    pub const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;
    pub const CP_UTF8: u32 = 65001; // console.h SetConsoleOutputCP argument
    pub const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;

    // origin: paths.h:230 WIN32_FIND_DATAA fd{} — repr(C) mirror. Offsets
    // dwFileAttributes=0 / ftLastWriteTime=20 / cFileName=44 verified against
    // the MinGW build of the scratch cross-check (sizeof there = 320 because
    // MinGW omits dwReserved2/3; the Windows x64 ABI reserves them, so the
    // mirror carries both = 328 bytes, the largest the OS may write).
    #[repr(C)]
    pub struct FileTime {
        pub dw_low_date_time: u32,
        pub dw_high_date_time: u32,
    }

    #[repr(C)]
    pub struct FindDataA {
        pub dw_file_attributes: u32,
        pub ft_creation_time: FileTime,
        pub ft_last_access_time: FileTime,
        pub ft_last_write_time: FileTime,
        pub n_file_size_high: u32,
        pub n_file_size_low: u32,
        pub dw_reserved0: u32,
        pub dw_reserved1: u32,
        pub c_file_name: [u8; 260],
        pub c_alternate_file_name: [u8; 14],
        pub dw_reserved2: u32, // x64 only (MSVC headers); harmless filler
        pub dw_reserved3: u32, // under MinGW, whose sizeof is 320
    }

    // origin: paths.h:363 SYSTEMTIME t (GetLocalTime fills all 8 WORDs)
    #[repr(C)]
    pub struct SystemTime {
        pub w_year: u16,
        pub w_month: u16,
        pub w_day_of_week: u16,
        pub w_day: u16,
        pub w_hour: u16,
        pub w_minute: u16,
        pub w_second: u16,
        pub w_milliseconds: u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetModuleFileNameA(h_module: *mut c_void, buf: *mut u8, size: u32) -> u32;
        pub fn GetFileAttributesA(path: *const u8) -> u32;
        pub fn FindFirstFileA(pattern: *const u8, data: *mut FindDataA) -> *mut c_void;
        pub fn FindNextFileA(handle: *mut c_void, data: *mut FindDataA) -> i32;
        pub fn FindClose(handle: *mut c_void) -> i32;
        pub fn CreateDirectoryA(path: *const u8, sa: *const c_void) -> i32;
        pub fn GetLastError() -> u32;
        pub fn MoveFileExA(from: *const u8, to: *const u8, flags: u32) -> i32;
        pub fn GetFullPathNameA(path: *const u8, len: u32, buf: *mut u8, part: *mut *mut u8) -> u32;
        pub fn GetModuleHandleExA(flags: u32, module: *const u8, out: *mut *mut c_void) -> i32;
        pub fn GetModuleHandleA(name: *const u8) -> *mut c_void;
        pub fn GetLocalTime(st: *mut SystemTime);
        pub fn SetConsoleOutputCP(cp: u32) -> i32;
    }
}

// ---------------------------------------------------------------------------
// paths.h
// ---------------------------------------------------------------------------

/// origin: paths.h:61-65 — Everything up to and including the last separator,
/// or "" if there is none.
pub mod detail {
    pub fn dir_of(path: &str) -> String {
        match path.rfind(['\\', '/']) {
            Some(slash) => path[..slash + 1].to_string(),
            None => String::new(),
        }
    }
}

/// origin: paths.h:70-99 — directory of the running binary, trailing separator.
/// `char buf[MAX_PATH]`, `n == 0 || n >= MAX_PATH` → "" (paths.h:73-77).
pub fn exe_dir() -> String {
    #[cfg(windows)]
    {
        let mut buf = [0u8; win::MAX_PATH as usize];
        let n = unsafe { win::GetModuleFileNameA(std::ptr::null_mut(), buf.as_mut_ptr(), win::MAX_PATH) };
        if n == 0 || n >= win::MAX_PATH {
            return String::new();
        }
        detail::dir_of(&utf8_lossy(&buf[..n as usize]))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        match std::fs::read_link("/proc/self/exe") {
            Ok(p) => detail::dir_of(&p.to_string_lossy()),
            Err(_) => String::new(),
        }
    }
    #[cfg(target_os = "macos")]
    {
        match std::env::current_exe() {
            Ok(p) => detail::dir_of(&p.to_string_lossy()),
            Err(_) => String::new(),
        }
    }
}

/// origin: paths.h:103-125 — per-user settings directory, trailing separator.
/// Nothing is created by asking.
pub fn config_dir() -> String {
    #[cfg(windows)]
    {
        config_dir_from(&env("LOCALAPPDATA"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let data = env("XDG_DATA_HOME");
        if !data.is_empty() {
            return data + "/S-MU2000/";
        }
        let home = env("HOME");
        if home.is_empty() {
            return String::new();
        }
        home + "/.local/share/S-MU2000/"
    }
    #[cfg(target_os = "macos")]
    {
        let home = env("HOME");
        if home.is_empty() {
            return String::new();
        }
        home + "/Library/Application Support/S-MU2000/"
    }
}

/// origin: paths.h:106-109 (the LOCALAPPDATA-empty test + suffix), factored so
/// tests can feed a temp root instead of the live config dir.
pub fn config_dir_from(base: &str) -> String {
    if base.is_empty() {
        return String::new();
    }
    base.to_string() + "\\S-MU2000\\"
}

/// origin: paths.h:141-153 — machine-wide counterpart, read-only, trailing separator.
pub fn shared_config_dir() -> String {
    #[cfg(windows)]
    {
        shared_config_dir_from(&env("ProgramData"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "/usr/local/share/S-MU2000/".to_string()
    }
    #[cfg(target_os = "macos")]
    {
        "/Library/Application Support/S-MU2000/".to_string()
    }
}

/// origin: paths.h:144-147 (ProgramData-empty test + suffix).
pub fn shared_config_dir_from(base: &str) -> String {
    if base.is_empty() {
        return String::new();
    }
    base.to_string() + "\\S-MU2000\\"
}

/// origin: paths.h:158-174 — join a forward-slash relative name onto a dir.
/// QUIRK (kept): with an empty `rel` the trailing separator of `dir` is
/// stripped away; with an empty `dir` the platform separator is prepended.
pub fn join(dir: &str, rel: &str) -> String {
    #[cfg(windows)]
    let sep = '\\';
    #[cfg(not(windows))]
    let sep = '/';
    let mut out = dir.to_string();
    while out.ends_with('/') || out.ends_with('\\') {
        out.pop();
    }
    if rel.is_empty() {
        return out;
    }
    out.push(sep);
    for c in rel.chars() {
        out.push(if c == '/' { sep } else { c });
    }
    out
}

/// origin: paths.h:177-181 — environment variable, or "" if unset/empty.
pub fn env(name: &str) -> String {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => v,
        _ => String::new(),
    }
}

/// origin: paths.h:184-192 — user's home directory, or "".
pub fn home_dir() -> String {
    #[cfg(windows)]
    {
        let p = env("USERPROFILE");
        if p.is_empty() {
            env("HOMEDRIVE") + &env("HOMEPATH")
        } else {
            p
        }
    }
    #[cfg(not(windows))]
    {
        env("HOME")
    }
}

/// origin: paths.h:194-205
pub fn is_file(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    #[cfg(windows)]
    {
        let a = unsafe { win::GetFileAttributesA(nt(p).as_ptr()) };
        a != win::INVALID_FILE_ATTRIBUTES && (a & win::FILE_ATTRIBUTE_DIRECTORY) == 0
    }
    #[cfg(not(windows))]
    {
        std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
    }
}

/// origin: paths.h:207-218
pub fn is_dir(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    #[cfg(windows)]
    {
        let a = unsafe { win::GetFileAttributesA(nt(p).as_ptr()) };
        a != win::INVALID_FILE_ATTRIBUTES && (a & win::FILE_ATTRIBUTE_DIRECTORY) != 0
    }
    #[cfg(not(windows))]
    {
        std::fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
    }
}

/// origin: paths.h:222 `struct dir_entry { std::string name; unsigned long long mtime; }`
pub struct DirEntry {
    pub name: String,
    pub mtime: u64,
}

/// origin: paths.h:224-259 — files (never directories) inside `dir`, as
/// (name, last-write) pairs. Windows mtime is the raw FILETIME
/// (100-ns ticks since 1601, paths.h:237-239); POSIX mtime is seconds
/// (paths.h:254) — unit difference is in the C++, keep it.
pub fn list_dir(dir: &str) -> Vec<DirEntry> {
    let mut out: Vec<DirEntry> = Vec::new();
    if dir.is_empty() {
        return out;
    }
    #[cfg(windows)]
    {
        let pattern = nt(&(dir.to_string() + "\\*")); // paths.h:231 dir + "\\*"
        let mut fd: win::FindDataA = unsafe { std::mem::zeroed() }; // paths.h:230 fd{}
        let h = unsafe { win::FindFirstFileA(pattern.as_ptr(), &mut fd) };
        if h == win::INVALID_HANDLE_VALUE {
            return out;
        }
        loop {
            if (fd.dw_file_attributes & win::FILE_ATTRIBUTE_DIRECTORY) == 0 {
                let t = ((fd.ft_last_write_time.dw_high_date_time as u64) << 32)
                    | fd.ft_last_write_time.dw_low_date_time as u64;
                let nul = fd.c_file_name.iter().position(|b| *b == 0).unwrap_or(0);
                out.push(DirEntry { name: utf8_lossy(&fd.c_file_name[..nul]), mtime: t });
            }
            if unsafe { win::FindNextFileA(h, &mut fd) } == 0 {
                break;
            }
        }
        unsafe { win::FindClose(h) };
    }
    #[cfg(not(windows))]
    {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return out,
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == "." || name == ".." {
                continue;
            }
            let md = match std::fs::metadata(e.path()) {
                Ok(md) if md.is_file() => md,
                _ => continue,
            };
            let secs = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push(DirEntry { name, mtime: secs });
        }
    }
    out
}

/// origin: paths.h:262-271 — one directory level, no parents; already
/// existing counts as success (ERROR_ALREADY_EXISTS / EEXIST).
pub fn make_dir(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    #[cfg(windows)]
    {
        let ok = unsafe { win::CreateDirectoryA(nt(p).as_ptr(), std::ptr::null()) };
        ok != 0 || unsafe { win::GetLastError() } == win::ERROR_ALREADY_EXISTS
    }
    #[cfg(not(windows))]
    {
        // mkdir(p, 0755)==0 || errno==EEXIST — the 0755 mode is not
        // expressible via std::fs (deviation; Windows is the parity target)
        match std::fs::create_dir(p) {
            Ok(()) => true,
            Err(e) => e.kind() == std::io::ErrorKind::AlreadyExists,
        }
    }
}

/// origin: paths.h:276-291 — directory that has to exist, parents included
/// (the NVRAM file lives two levels under the settings directory).
pub fn ensure_dir(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    if is_dir(path) {
        return true;
    }
    let mut at = path.to_string();
    while at.ends_with('/') || at.ends_with('\\') {
        at.pop();
    }
    // byte scan exactly like the C++ (paths.h:285-289): UTF-8 multi-byte
    // sequences never contain 0x2F/0x5C, so cut offsets stay on char boundaries
    for i in 1..at.len() {
        match at.as_bytes()[i] {
            b'/' | b'\\' => {
                make_dir(&at[..i]);
            }
            _ => continue,
        }
    }
    make_dir(&at)
}

/// origin: paths.h:296-304 — rename replacing the destination. std::rename
/// does not replace on Windows; that side goes through MoveFileEx with
/// REPLACE_EXISTING | WRITE_THROUGH (0x1 | 0x8).
pub fn replace_file(from: &str, to: &str) -> bool {
    #[cfg(windows)]
    {
        let f = nt(from);
        let t = nt(to);
        unsafe {
            win::MoveFileExA(
                f.as_ptr(),
                t.as_ptr(),
                win::MOVEFILE_REPLACE_EXISTING | win::MOVEFILE_WRITE_THROUGH,
            ) != 0
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to).is_ok()
    }
}

/// origin: paths.h:308-320 — absolute + resolved "." / ".."; falls back to
/// the input (`n && n < sizeof(buf)` gate, buf is 4096 on Windows).
pub fn full_path(p: &str) -> String {
    if p.is_empty() {
        return p.to_string();
    }
    #[cfg(windows)]
    {
        let mut buf = [0u8; 4096];
        let n = unsafe {
            win::GetFullPathNameA(nt(p).as_ptr(), buf.len() as u32, buf.as_mut_ptr(), std::ptr::null_mut())
        };
        if n != 0 && (n as usize) < buf.len() {
            utf8_lossy(&buf[..n as usize])
        } else {
            p.to_string()
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::canonicalize(p).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string())
    }
}

/// DEVIATION (dev-machine quirk, found 2026-09-30 — for the ledger): on this
/// box the loader's *InMemoryOrder* module walk is broken for rustc-built
/// images — a runtime-resolved, genuine kernel32 `GetModuleHandleExA` returns
/// ERROR_MOD_NOT_FOUND even for kernel32's own exported-function address,
/// while name-based lookups (`GetModuleHandleA(NULL)`) work and the identical
/// C++ call works in MinGW-built binaries. The fallback below answers only
/// when `addr` provably lies inside the MAIN image (range read from its own
/// PE headers), which is exactly the answer the healthy API would give; the
/// healthy path is unchanged and preferred. `addr` in any other module still
/// yields "" like the C++ does when the API refuses.
#[cfg(windows)]
unsafe fn main_image_covering(addr: usize) -> *mut c_void {
    let base = win::GetModuleHandleA(std::ptr::null());
    if base.is_null() {
        return std::ptr::null_mut();
    }
    // e_lfanew @ +0x3C; SizeOfImage = PE32+ OptionalHeader offset 56 => lfanew+80
    let pe = (base as *const u8).add(0x3C).cast::<u32>().read_unaligned() as usize;
    let size_image = (base as *const u8).add(pe + 80).cast::<u32>().read_unaligned() as usize;
    let b = base as usize;
    if addr >= b && addr - b < size_image {
        base
    } else {
        std::ptr::null_mut()
    }
}

/// origin: paths.h:327-356 — directory of the image containing `addr`
/// (module handle from address, no trailing separator; "" if refused).
pub fn module_dir(addr_in_module: *const c_void) -> String {
    if addr_in_module.is_null() {
        return String::new();
    }
    #[cfg(windows)]
    {
        let mut self_h: *mut c_void = std::ptr::null_mut();
        let ok = unsafe {
            win::GetModuleHandleExA(
                win::GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | win::GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                addr_in_module as *const u8,
                &mut self_h,
            )
        };
        if ok == 0 {
            self_h = unsafe { main_image_covering(addr_in_module as usize) };
            if self_h.is_null() {
                return String::new();
            }
        }
        let mut buf = [0u8; 4096];
        let n = unsafe { win::GetModuleFileNameA(self_h, buf.as_mut_ptr(), buf.len() as u32) };
        if n == 0 || (n as usize) >= buf.len() {
            return String::new();
        }
        let s = utf8_lossy(&buf[..n as usize]);
        match s.rfind(['\\', '/']) {
            Some(slash) => s[..slash].to_string(),
            None => String::new(),
        }
    }
    #[cfg(not(windows))]
    {
        // dladdr has no std equivalent; the portable stand-in is the exe dir.
        let s = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        match s.rfind(['\\', '/']) {
            Some(slash) => s[..slash].to_string(),
            None => String::new(),
        }
    }
}

/// origin: paths.h:359-374 — "YYYY-MM-DD HH:MM:SS" local time, %04d/%02d
/// field widths identical to the snprintf.
pub fn local_time() -> String {
    #[cfg(windows)]
    {
        let mut t: win::SystemTime = unsafe { std::mem::zeroed() };
        unsafe { win::GetLocalTime(&mut t) };
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.w_year, t.w_month, t.w_day, t.w_hour, t.w_minute, t.w_second
        )
    }
    #[cfg(not(windows))]
    {
        // POSIX branch needs localtime_r; non-Windows HAL is backlog (AGENTS.md).
        String::new()
    }
}

/// origin: paths.h:377-398 — config_dir(), created if not there yet. The
/// Windows branch makes just the one level (paths.h:383-384); the POSIX
/// branch walks the separators first (paths.h:386-395).
pub fn ensure_config_dir() -> String {
    ensure_config_dir_from(&config_dir())
}

/// origin: paths.h:379-397 with the config_dir() lookup as a parameter
/// (test seam; public behavior unchanged).
pub fn ensure_config_dir_from(base: &str) -> String {
    if base.is_empty() {
        return String::new();
    }
    #[cfg(windows)]
    {
        unsafe { win::CreateDirectoryA(nt(base).as_ptr(), std::ptr::null()) }; // already existing is fine
    }
    #[cfg(not(windows))]
    {
        let mut at = base.to_string();
        while at.ends_with('/') || at.ends_with('\\') {
            at.pop();
        }
        for i in 1..at.len() {
            if at.as_bytes()[i] != b'/' {
                continue;
            }
            let _ = std::fs::create_dir(&at[..i]);
        }
        let _ = std::fs::create_dir(&at);
    }
    base.to_string()
}

/// origin: bootcache.h:115-126 / nvram.h:54-65 — the shared "<settings>\\
/// <subdir>\\<key:016x>.bin" shape (`%016llx.bin`). `base` empty → "";
/// failing ensure_dir → "". `subdir` is "boot" (bootcache) or "nvram"
/// (nvram); `key` there comes from the FNV-1a helpers below.
pub fn subdir_keyed_path(base: &str, subdir: &str, key: u64) -> String {
    if base.is_empty() {
        return String::new();
    }
    let dir = join(base, subdir);
    if !ensure_dir(&dir) {
        return String::new();
    }
    join(&dir, &format!("{key:016x}.bin"))
}

// ---------------------------------------------------------------------------
// FNV-1a (nvram.h / bootcache.h — NOT in paths.h; see session report)
// ---------------------------------------------------------------------------

/// origin: nvram.h:39 — the 64-bit offset basis. NOT the 32-bit
/// 2166136261 of the (abandoned) native engine — this loop is u64, the
/// multiply wraps at 64 bits.
pub const FNV64_OFFSET_BASIS: u64 = 0xcbf29ce484222325;

/// origin: nvram.h:43 / bootcache.h:90.
pub const FNV64_PRIME: u64 = 0x100000001b3;

/// origin: bootcache.h:88-91 `mix` lambda (xor then wrapping multiply, FNV-1a
/// step; bootcache chains it on top of `nvram::rom_key`).
pub fn fnv1a_mix(h: u64, b: u8) -> u64 {
    (h ^ b as u64).wrapping_mul(FNV64_PRIME)
}

/// origin: nvram.h:37-46 `rom_key` — the exact per-byte loop
/// (`h ^= b; h *= 0x100000001b3`), byte-for-byte. The machine-bound part of
/// `rom_key` (reading `mu.program_rom()`) lands at the M5 row; this is its
/// pure core, vectors pinned in tests against the compiled C++ loop.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = FNV64_OFFSET_BASIS;
    for b in bytes {
        // nvram.h:42-43
        h ^= *b as u64;
        h = h.wrapping_mul(FNV64_PRIME);
    }
    h
}

/// origin: nvram.h:84-100 `save` — write `data` to `p + tmp_suffix`, then
/// replace. Suffixes used by the C++: ".tmp" (nvram.h:89), ".new"
/// (bootcache.h:173). A crash mid-write leaves the old file intact.
/// (The `std::fclose(f) != 0` error gate is approximated by sync_all:
/// Rust's drop-close surfaces no error; a failed write already sets `ok=false`.)
pub fn write_file_atomic(p: &str, data: &[u8], tmp_suffix: &str) -> bool {
    let tmp = format!("{p}{tmp_suffix}"); // nvram.h:89 p + ".tmp"
    let mut f = match std::fs::File::create(&tmp) {
        // fopen(tmp, "wb") — nvram.h:90
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut ok = f.write_all(data).is_ok(); // fwrite == full size — nvram.h:94
    if f.sync_all().is_err() {
        ok = false; // fclose flush failure stands in — nvram.h:95
    }
    drop(f);
    if !ok {
        let _ = std::fs::remove_file(&tmp); // nvram.h:96
        return false;
    }
    replace_file(&tmp, p) // nvram.h:99
}

// ---------------------------------------------------------------------------
// console.h
// ---------------------------------------------------------------------------

/// origin: console.h:27-32 — UTF-8 console output. No-op off Windows
/// (macOS terminals already speak UTF-8).
pub fn init_console_utf8() {
    #[cfg(windows)]
    {
        unsafe { win::SetConsoleOutputCP(win::CP_UTF8) };
    }
}

// ---------------------------------------------------------------------------
// compat.cpp — 互換層の実体。ログと、移植の突き合わせ用の命令追跡。
// Globals become an explicitly-initialized SyncUnsafeCell state (no
// Default::default(), rule 3). Callers are the single main thread, same as C++.
// ---------------------------------------------------------------------------

/// C++ `g_verbose` (compat.cpp:14) + all `g_pc_*` / `s_*` trace state
/// (compat.cpp:21-29, 44-48).
struct TraceState {
    verbose: bool,          // compat.cpp:14
    pc_trace: Option<std::fs::File>, // g_pc_trace (compat.cpp:21)
    pc_trace_left: u64,     // g_pc_trace_left (compat.cpp:22)
    pc_skip: u64,           // g_pc_skip (compat.cpp:23)
    pc_hash_file: Option<std::fs::File>, // g_pc_hash (compat.cpp:24)
    #[allow(dead_code)] // sink wired by later rows (mu2000 port trace / boot --trace-upd)
    port_trace: Option<std::fs::File>, // g_port_trace (compat.cpp:25)
    pc_cycles: u64,         // g_pc_cycles (compat.cpp:26)
    #[allow(dead_code)] // sink wired by later rows (boot --trace-upd; timers golden already pins the schedule side)
    upd_trace: Option<std::fs::File>,    // g_upd_trace (compat.cpp:27)
    count: u64,             // s_count (compat.cpp:29)
    h: u64,                 // s_h (compat.cpp:29)
    pc_prof: Vec<u32>,      // s_pc_prof (compat.cpp:47)
    pc_prof_top: i32,       // s_pc_prof_top (compat.cpp:48)
    pc_prof_why: [u64; 5],  // g_pc_prof_why (compat.cpp:45)
    slow_mem: [u64; 2],     // g_slow_mem (compat.cpp:46)
}

/// explicit construction of every field (C++ zero-init `u64 x[5] = {}` etc.)
const TRACE_INIT: TraceState = TraceState {
    verbose: false,
    pc_trace: None,
    pc_trace_left: 0,
    pc_skip: 0,
    pc_hash_file: None,
    port_trace: None,
    pc_cycles: 0,
    upd_trace: None,
    count: 0,
    h: 0,
    pc_prof: Vec::new(),
    pc_prof_top: 0,
    pc_prof_why: [0; 5],
    slow_mem: [0; 2],
};

struct SyncCell(std::cell::UnsafeCell<TraceState>);
// SAFETY: trace state is touched from the single main thread of the CLI tools
// exactly like the C++ globals; SyncCell only re-enables static placement.
// (SyncUnsafeCell::new/get are still unstable per rust#95439 — plain
// UnsafeCell + Sync is the stable equivalent.)
unsafe impl Sync for SyncCell {}

static TRACE: SyncCell = SyncCell(std::cell::UnsafeCell::new(TRACE_INIT));

#[inline]
fn trace() -> &'static mut TraceState {
    // SAFETY: single-threaded access per SyncCell (matches C++ globals).
    unsafe { &mut *TRACE.0.get() }
}

/// origin: compat.cpp:14 g_verbose setter/getter (tools assign it from --verbose)
pub fn set_verbose(v: bool) {
    trace().verbose = v;
}
pub fn verbose() -> bool {
    trace().verbose
}

/// origin: compat.cpp:21-23 — open the raw PC trace sink. The C++ tools set
/// `g_pc_trace` (fopen "w"), `g_pc_skip`, `g_pc_trace_left` directly; this is
/// that assignment bundled. `regs` strings supplied per call by the CPU loop.
pub fn open_pc_trace(path: &str, skip: u64, left: u64) -> bool {
    let t = trace();
    match std::fs::File::create(path) {
        Ok(f) => {
            t.pc_trace = Some(f);
            t.pc_skip = skip;
            t.pc_trace_left = left;
            true
        }
        Err(_) => false, // fopen returned null
    }
}

pub fn close_pc_trace() {
    trace().pc_trace = None;
}

/// whether g_pc_trace is still a live FILE* (compat.cpp:97 nulls itself)
pub fn pc_trace_active() -> bool {
    trace().pc_trace.is_some()
}

/// True only when the next `pc_trace` call will actually emit a line: the
/// skip window is exhausted (`g_pc_skip == 0`) and the count budget is not
/// spent (`g_pc_trace_left > 0`). Callers use this to avoid building the
/// register-text argument on the (usually huge) skip tail — the C++ hot path
/// never formats once `g_pc_trace` self-nulls (compat.cpp:97); Rust's fixed
/// hook flag must, or it pays the `format!` for every skipped instruction.
pub fn pc_trace_will_emit() -> bool {
    let t = trace();
    t.pc_skip == 0 && t.pc_trace_left > 0 && t.pc_trace.is_some()
}

/// origin: compat.cpp:26 — set g_pc_cycles
pub fn set_pc_cycles(c: u64) {
    trace().pc_cycles = c;
}
pub fn pc_cycles() -> u64 {
    trace().pc_cycles
}

/// origin: compat.cpp:24 — open the block-hash sink (fopen "w" on the caller side)
pub fn open_pc_hash(path: &str) -> bool {
    let t = trace();
    t.pc_hash_file = std::fs::File::create(path).ok();
    t.pc_hash_file.is_some()
}

pub fn close_pc_hash() {
    trace().pc_hash_file = None;
}

/// origin: compat.cpp:31-37 `pc_hash` — fold every executed instruction;
/// emit a "%llu %016llx" line every 65536 (the `!(++count & 0xffff)` gate).
/// C++ UB-writes through a null g_pc_hash if called without a sink; here the
/// fold/count still happen, only the line is dropped (the real tools open the
/// sink first, e.g. sh2_jit.cpp SH2_JIT_HASH).
pub fn pc_hash(pc: u32, regs: u64) {
    let t = trace();
    // compat.cpp:33 — left-to-right: ((h * 1000003) ^ pc) * 1000003 ^ regs, u64 wrap
    t.h = t.h.wrapping_mul(1_000_003) ^ pc as u64;
    t.h = t.h.wrapping_mul(1_000_003) ^ regs;
    t.count += 1;
    if t.count & 0xffff == 0 {
        if let Some(f) = &mut t.pc_hash_file {
            // fprintf "%llu %016llx\n" — text mode CRLF on Windows (pitfall)
            let line = format!("{} {:016x}{}", t.count, t.h, EOL);
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// count folded so far (s_count, compat.cpp:29) — test/inspection hook
pub fn pc_hash_count() -> u64 {
    trace().count
}

/// folded hash so far (s_h, compat.cpp:29)
pub fn pc_hash_value() -> u64 {
    trace().h
}

/// origin: compat.cpp:90-102 `pc_trace` — skip `g_pc_skip` instructions,
/// emit `g_pc_trace_left` of "%08X C=%llu%s", then null the sink.
pub fn pc_trace(pc: u32, regs: &str) {
    let t = trace();
    if t.pc_skip != 0 {
        t.pc_skip -= 1;
        return;
    }
    if t.pc_trace_left == 0 {
        t.pc_trace = None;
        return;
    }
    t.pc_trace_left -= 1;
    if let Some(f) = &mut t.pc_trace {
        // fprintf "%08X C=%llu%s\n" (compat.cpp:101)
        let line = format!("{:08X} C={}{}{}", pc, t.pc_cycles, regs, EOL);
        let _ = f.write_all(line.as_bytes());
    }
}

/// C atoi semantics (whitespace, sign, digits, saturate): the C++ uses
/// std::atoi on SMU2000_PCPROF (compat.cpp:55).
fn atoi(c: &str) -> i32 {
    let b = c.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] == b' ' || (b[i] >= 0x09 && b[i] <= 0x0d)) {
        i += 1;
    }
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let mut v: i64 = 0;
    let mut overflow = false;
    while i < b.len() && b[i].is_ascii_digit() {
        v = v * 10 + (b[i] - b'0') as i64;
        if v > i32::MAX as i64 {
            overflow = true;
        }
        i += 1;
    }
    if overflow {
        return if neg { i32::MIN } else { i32::MAX };
    }
    if neg {
        -(v as i32)
    } else {
        v as i32
    }
}

/// origin: compat.cpp:50-60 `pc_prof_start` — SMU2000_PCPROF env, atoi,
/// `<= 0` → 30; one u32 counter per 0x40-byte block of the 4 MB ROM window.
pub fn pc_prof_start() {
    let e = env("SMU2000_PCPROF");
    if e.is_empty() {
        return;
    }
    let t = trace();
    t.pc_prof_top = atoi(&e);
    if t.pc_prof_top <= 0 {
        t.pc_prof_top = 30;
    }
    t.pc_prof = vec![0u32; 0x400000 / 0x40];
}

/// g_pc_prof raw counter array (null when not started) — compat.cpp:59 hands
/// the CPU loop this raw pointer; test hook included.
pub fn pc_prof_ptr() -> *mut u32 {
    let t = trace();
    if t.pc_prof.is_empty() {
        std::ptr::null_mut()
    } else {
        t.pc_prof.as_mut_ptr()
    }
}

/// origin: compat.cpp:62-88 `pc_prof_report` against an arbitrary sink.
/// The exact Japanese strings and printf widths are kept byte-for-byte;
/// `std::sort` (unstable in C++) maps to sort_unstable_by.
pub fn pc_prof_report_to(w: &mut dyn Write) {
    let t = trace();
    if t.pc_prof.is_empty() {
        return;
    }
    let mut idx: Vec<u32> = Vec::new();
    let mut total: u64 = 0;
    for i in 0..t.pc_prof.len() as u32 {
        if t.pc_prof[i as usize] != 0 {
            idx.push(i);
            total += t.pc_prof[i as usize] as u64;
        }
    }
    idx.sort_unstable_by(|a, b| t.pc_prof[*b as usize].cmp(&t.pc_prof[*a as usize]));
    let _ = write!(
        w,
        "ブロックに入った回数 合計 {}（上位 {}）{}",
        total, t.pc_prof_top, EOL
    );
    let _ = write!(
        w,
        "  JIT を抜けた訳: 遅延枠 {} / 割り込みの印 {} / 番地が外 {}{}",
        t.pc_prof_why[0], t.pc_prof_why[1], t.pc_prof_why[2], EOL
    );
    let per = if t.pc_prof_why[3] != 0 {
        t.pc_prof_why[4] as f64 / t.pc_prof_why[3] as f64
    } else {
        0.0
    };
    let _ = write!(
        w,
        "  入り直し {} 回で {} 命令 = 1 回あたり {:.1} 命令{}",
        t.pc_prof_why[3], t.pc_prof_why[4], per, EOL
    );
    let _ = write!(
        w,
        "  JIT の速い道から外れたメモリ: 読み {} / 書き {}{}",
        t.slow_mem[0], t.slow_mem[1], EOL
    );
    let mut k = 0usize;
    while k < t.pc_prof_top as usize && k < idx.len() {
        let _ = write!(
            w,
            "  {:08x}  {:>10}  {:>5.1}%{}",
            idx[k] * 0x40,
            t.pc_prof[idx[k] as usize],
            100.0 * t.pc_prof[idx[k] as usize] as f64 / total as f64,
            EOL
        );
        k += 1;
    }
}

/// origin: compat.cpp:62-88 — stdout flavor (C++ printf: Windows console gets
/// CRLF through the same EOL constant).
pub fn pc_prof_report() {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    pc_prof_report_to(&mut lock);
    let _ = lock.flush();
}

#[cfg(test)]
mod tests;
