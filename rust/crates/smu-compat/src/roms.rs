//! ROM loaders — mirror of `mu2000::load_program` / `load_wave` / `load_sintab` /
//! `load_lcd_font` and the shared `read_file` in `src/mu2000.cpp`, plus the
//! LCD-glyph fill (`fill_missing_glyphs`, mu2000.cpp:594) and the hand-drawn
//! overlay (`src/lcdfont.h`).
//!
//! Ledger row M1 "rom loaders". Transliteration rules (PORTING_LEDGER invariants):
//! same widths, same size-mismatch behavior, same error texts, no "fixes".
//!
//! Shape of this port: the C++ members mutate `m_prog`/`m_wave`/`m_sintab`/
//! `m_lcd_font` and poke the bus/SWP/hd44780. Those devices do not exist yet,
//! so each loader here is a pure function returning the assembled buffer
//! (`Err(String)` == the `m_error` text the C++ sets), and the device pokes
//! (`set_program_rom`→`build_bus`, `set_wave_rom`→SWP, `set_cgrom`) stay with
//! the M4 wiring row / M2 smu-dev row. There is NO `load_swp30_roms` in the
//! C++ (checked mu2000.h/.cpp) — nothing to port for it.
//!
//! Hard rule: NEVER commit ROM bytes; tests carry SHA1s only and skip
//! gracefully when `roms/` is absent (hashes-only CI rule).

use std::io::{Read, Seek};

/// origin: mu2000.cpp:45-61 — "read exactly `expect` bytes" (expect != 0).
/// QUIRKS kept verbatim:
/// - fopen failure → `false` with `out` untouched (Rust: `None`).
/// - size obtained via fseek(END)+ftell → file size for regular files.
/// - `expect != 0 && size != expect` → fail BEFORE any read: a short file is
///   rejected wholesale, a longer file too — no zero-pad, no truncation.
/// - `expect == 0` accepts ANY size, including an empty file → `Some(vec![])`.
/// - short read (error mid-file) → `got != size` → `false`.
pub fn read_file(path: &str, expect: usize) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?; // :47-49 "rb"; !f → false
    let size = f.seek(std::io::SeekFrom::End(0)).ok()? as usize; // :50-51
    f.seek(std::io::SeekFrom::Start(0)).ok()?; // :52
    if expect != 0 && size != expect {
        // :53-56 fclose + false; note the expect==0 bypass
        return None;
    }
    let mut out = vec![0u8; size]; // :57
    let mut got = 0usize;
    while got < size {
        match f.read(&mut out[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    if got != out.len() {
        return None; // :58-60 got == out.size()
    }
    Some(out)
}

/// origin: mu2000.cpp:376-385 — the whole body is `read_file(path, rom,
/// 0x400000)` + error text. :383 `set_program_rom(std::move(rom))` (bus
/// rebuild) is machine glue, NOT ported (M4 wiring row).
pub fn load_program(path: &str) -> Result<Vec<u8>, String> {
    match read_file(path, 0x400000) {
        Some(rom) => Ok(rom),
        None => Err(format!("プログラム ROM を読めない（4MB でないか、見つからない）: {}", path)), // :380
    }
}

/// origin: mu2000.cpp:414-443 — four 8 MB dumps interleaved into one 32 MB
/// image as 32-bit little-endian words (comment :416-419): ic49 → word bytes
/// 0-1 (low half), ic50 → word bytes 2-3 (high half), ic53/ic54 the same from
/// word byte-offset 0x1000000. The byte pair is copied UN-swapped
/// (`dst+0 = part[j+0]`, `dst+1 = part[j+1]`, :436-437) — file order preserved.
/// `dir + "/" + names[i]` is a PLAIN slash concat (:427), not paths::join —
/// kept (a trailing separator in `dir` yields "//", which fopen accepts).
/// :441 `set_wave_rom` (SWP feed) is machine glue, NOT ported.
pub fn load_wave(dir: &str) -> Result<Vec<u8>, String> {
    const NAMES: [&str; 4] = [
        // :420-422
        "xv364a0.ic49",
        "xv365a0.ic50",
        "xw848a0.ic53",
        "xw849a0.ic54",
    ];

    let mut rom = vec![0u8; 0x2000000]; // :424 32MB zero-filled
    for i in 0..4usize {
        let part = match read_file(&format!("{}/{}", dir, NAMES[i]), 0x800000) {
            // :427-431
            Some(p) => p,
            None => {
                return Err(format!(
                    "波形 ROM を読めない（8MB でないか、見つからない）: {}/{}",
                    dir, NAMES[i]
                )) // :429
            }
        };
        let base: usize = if i >= 2 { 0x1000000 } else { 0 }; // :432
        let off: usize = if i & 1 != 0 { 2 } else { 0 }; // :433
        // :434-438 — j steps 2 over an exact 0x800000 (even) part, so
        // part[j+1] cannot index past the end (read_file's expect guarantees
        // it; an odd part would be UB in C++, panic here — never reachable).
        let mut j = 0usize;
        while j < part.len() {
            let dst = base + j * 2 + off;
            rom[dst + 0] = part[j + 0];
            rom[dst + 1] = part[j + 1];
            j += 2;
        }
    }

    Ok(rom) // :441 set_wave_rom — glue, not ported
}

/// origin: mu2000.cpp:446-465 — 64 KB raw → u16 little-endian (:455
/// `raw[i*2] | (raw[i*2+1] << 8)`), then the stand-in fixup: if the table is
/// 0x8000 words and starts below 0x4000 (0-origin fake from make_standins.py,
/// comment :456-458), rebuild it centered on 0x8000 (:459-462).
/// FLOAT (ledger invariant 9): the expression is transliterated in the exact
/// C++ evaluation order with the same literal; rustc never contracts FMA and
/// both sides use IEEE double `sin`/`round`. Bit-exactness of this cold path
/// is pinned by the real-file SHA1 in roms/tests.rs (regen IS live here:
/// the stand-in's first word is 0x0002).
/// :463 `set_sintab_rom` (SWP feed) is machine glue, NOT ported.
pub fn load_sintab(path: &str) -> Result<Vec<u16>, String> {
    let raw = match read_file(path, 0x10000) {
        Some(r) => r,
        None => {
            return Err(format!(
                "sin 表を読めない（64KB でないか、見つからない）: {}",
                path
            )) // :450
        }
    };
    let mut rom = vec![0u16; raw.len() / 2]; // :453
    for i in 0..rom.len() {
        // :454-455
        rom[i] = (raw[i * 2] as u16) | ((raw[i * 2 + 1] as u16) << 8);
    }
    if rom.len() == 0x8000 && rom[0] < 0x4000 {
        // :459
        for i in 0..rom.len() {
            // :460-461 — min AFTER round, u16() truncates (always >= 0x8000)
            let t = (i as f64 + 0.5) / 0x8000 as f64 * 3.14159265358979323846 / 2.0;
            rom[i] = ((0x8000 as f64) + t.sin() * (0x7fff as f64)).round().min(65535.0) as u16;
        }
    }
    Ok(rom) // :463 set_sintab_rom — glue, not ported
}

/// origin: mu2000.cpp:569-578 — read the exact 4 KB font. The set-time
/// clone-and-fill (:576 → set_lcd_font) is ported as [`set_lcd_font`];
/// `m_lcd.set_cgrom` (:708) stays with the M2 hd44780 row.
pub fn load_lcd_font(path: &str) -> Result<Vec<u8>, String> {
    match read_file(path, 0x1000) {
        Some(rom) => Ok(rom),
        None => Err(format!("LCD の字を読めない（4KB でないか、見つからない）: {}", path)), // :573
    }
}

/// origin: mu2000.cpp:698-706 — `set_lcd_font`'s size-gated patch: a font of
/// at least 0x1000 is CLONED and passed through [`fill_missing_glyphs`];
/// smaller (or absent) fonts are stored as-is (:705). The real patch call
/// site is :702. :707-708 `set_cgrom` is device glue, NOT ported.
/// NOTE: `fill_missing_glyphs` touches the filesystem (hand-drawn overlay,
/// mu2000.cpp:695) — same as C++.
pub fn set_lcd_font(p: Option<Vec<u8>>) -> Option<Vec<u8>> {
    if let Some(v) = &p {
        // :700 p && p->size() >= 0x1000
        if v.len() >= 0x1000 {
            let mut patched = v.clone(); // :701
            fill_missing_glyphs(&mut patched); // :702
            return Some(patched); // :703
        }
    }
    Some(p?) // :705 (None stays None)
}

/// origin: mu2000.cpp:597-604 — the `bar` lambda: 2-dot-wide left bar in
/// columns 3-4 (0x18), right bar in columns 0-1 (0x03), each grown from the
/// bottom (`y >= 8 - n`). Writes 8 rows of `code` (rows 8-15 of the 16-byte
/// cell stay whatever the ROM had).
fn bar(rom: &mut [u8], code: usize, left: i32, right: i32) {
    for y in 0..8usize {
        let mut v: u8 = 0;
        if y as i32 >= 8 - left {
            v |= 0x18;
        }
        if y as i32 >= 8 - right {
            v |= 0x03;
        }
        rom[code * 16 + y] = v;
    }
}

/// origin: mu2000.cpp:596-691 — the generated-glyph rules (the bar lambda,
/// the level-meter block, the PAN block, the two triangles) WITHOUT the
/// `overlay_default` call of :695. Deviation from C++ (logged in the ledger):
/// the C++ is one function; the split exists so tests can pin the filesystem-
/// free part. Callers must pass `rom.len() >= 0x1000` (C++ guards the same
/// way at :700; indices reach 0xff*16+15 = 0xfff).
fn fill_glyph_rules(rom: &mut [u8]) {
    // level meter, :623-628 — code = 0x7f + 9a + b (a,b in 0..=8),
    // (0,0) skipped so 0x7f stays a normal glyph (:625-626). Exactly the
    // codes 0x80..0xcf get written (0x7f+1 .. 0x7f+80 spans them all); the
    // comment at :590 about 0x80-0x88 being full-width bars is loosely
    // worded — the generated 0x80-0x88 glyphs are partial (single-column)
    // except where a == 8 / b == 8.
    for a in 0..=8i32 {
        for b in 0..=8i32 {
            if a == 0 && b == 0 {
                continue;
            }
            bar(rom, (0x7f + a * 9 + b) as usize, a, b);
        }
    }

    // PAN, :647-658 — code = 0xd0 + 7(a+1) + b with a,b in -1..=5;
    // out-of-range codes skipped, which drops exactly (a,b)=(-1,-1) → the
    // would-be 0xcf so it never overwrites the level meter's 0xcf (:639-640
    // comment, order-sensitive: the level block above runs FIRST).
    for a in -1..=5i32 {
        for b in -1..=5i32 {
            let code = 0xd0 + (a + 1) * 7 + b;
            if code < 0xd0 || code > 0xff {
                continue;
            }
            for y in 0..8i32 {
                // top row centered, bars grow DOWNWARD (y <= n) — opposite
                // of the meter glyphs (:642-644 comment)
                let mut v: u8 = 0;
                if y <= a {
                    v |= 0x18;
                }
                if y <= b {
                    v |= 0x03;
                }
                rom[code as usize * 16 + y as usize] = v;
            }
        }
    }

    // bank/program triangles, :686-691 — 0x10 filled right-pointing,
    // 0x11 hollow right-pointing (real machine; the mulcd.zip ROM has
    // both filled, :670-672 comment)
    const TRI_FILLED: [u8; 8] = [0x08, 0x0c, 0x0e, 0x0f, 0x0e, 0x0c, 0x08, 0x00]; // :686
    const TRI_HOLLOW: [u8; 8] = [0x08, 0x0c, 0x0a, 0x09, 0x0a, 0x0c, 0x08, 0x00]; // :687
    for y in 0..8usize {
        // :688-691
        rom[0x10 * 16 + y] = TRI_FILLED[y];
        rom[0x11 * 16 + y] = TRI_HOLLOW[y];
    }
}

/// origin: mu2000.cpp:594-696 — `fill_missing_glyphs`: the generated rules
/// ([`fill_glyph_rules`], :596-691) then the hand-drawn overlay on top
/// (:695, "ROM から起こした字は入れない" — the overlay is the human-drawn
/// set from art/lcdfont.txt). Filesystem access mirrors the C++ exactly.
pub fn fill_missing_glyphs(rom: &mut [u8]) {
    fill_glyph_rules(rom);
    lcdfont::overlay_default(rom); // :695
}

/// origin: src/lcdfont.h (namespace smu2000::lcdfont) — the hand-drawn
/// glyph overlay. Lives inside roms.rs to keep this ledger row to one file.
pub mod lcdfont {
    use crate::paths::{config_dir, exe_dir, join};
    use std::io::Read;

    /// origin: lcdfont.h:46-49 — one glyph: code (-1 = unset) + 8 rows
    /// (top 3 bits always 0, 5 dots). Explicit init, no Default.
    pub struct Glyph {
        pub code: i32,
        pub row: [u8; 8],
    }

    impl Glyph {
        fn new() -> Glyph {
            Glyph { code: -1, row: [0u8; 8] } // matches `int code = -1; row {}`
        }
    }

    /// C `strtol(line, &end, 16)` stand-in for lcdfont.h:96-97. Returns
    /// `None` iff NO conversion happened (C `end == line.c_str()`).
    /// Faithful edges: optional isspace run, sign, `0x` prefix consumed ONLY
    /// when followed by a hex digit — so "0xQ" parses as 0 like the C run-
    /// time, "x1" converts nothing. Overflow saturates LONG_MAX (mingw long
    /// is 64-bit); the caller's `< 256` check drops those anyway.
    pub(crate) fn strtol_hex(line: &[u8]) -> Option<i64> {
        let mut i = 0usize;
        while i < line.len() && (line[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        let neg = line.get(i) == Some(&b'-');
        if matches!(line.get(i), Some(b'+') | Some(b'-')) {
            i += 1;
        }
        if i + 2 < line.len()
            && line[i] == b'0'
            && (line[i + 1] == b'x' || line[i + 1] == b'X')
            && (line[i + 2] as char).is_ascii_hexdigit()
        {
            i += 2;
        }
        let mut mag: u64 = 0;
        let mut ovf = false;
        let mut any = false;
        while i < line.len() {
            let d = match (line[i] as char).to_digit(16) {
                Some(d) => d as u64,
                None => break,
            };
            any = true;
            i += 1;
            mag = match mag.checked_mul(16).and_then(|v| v.checked_add(d)) {
                Some(v) => v,
                None => {
                    ovf = true; // strtol errno=ERANGE
                    u64::MAX
                }
            };
        }
        if !any {
            return None;
        }
        if ovf {
            return Some(if neg { i64::MIN } else { i64::MAX });
        }
        if neg {
            if mag > (i64::MAX as u64) + 1 {
                return Some(i64::MIN);
            }
            Some((mag as i64).wrapping_neg()) // mag == 2^63 → i64::MIN exact
        } else {
            if mag > i64::MAX as u64 {
                return Some(i64::MAX);
            }
            Some(mag as i64)
        }
    }

    /// origin: lcdfont.h:52-104 — parse the `art/lcdfont.txt` format.
    /// QUIRKS kept verbatim:
    /// - the line loop runs `i <= text.size()` (:63): a trailing '\n' or an
    ///   empty text produces one extra EMPTY line → an extra flush.
    /// - trailing trim is '\r', ' ', '\t' only; leading trim ' ', '\t' only
    ///   (:67-72).
    /// - dot rows are recognized BEFORE memos: <=5 chars of '.'/'#' only
    ///   (:77-83, the `###.#`-starts-with-# lesson); ignored while no code
    ///   is open or 8 rows are full (then a `.`-row falls through to the
    ///   strtol attempt, which converts nothing — same no-op as C++).
    /// - a memo `#`-line never flushes (:92-93).
    pub fn parse(text: &[u8], out: &mut Vec<Glyph>) -> usize {
        let mut cur = Glyph::new();
        let mut nrow: usize = 0;
        let mut i = 0usize;
        while i <= text.len() {
            let e = text[i.min(text.len())..].iter().position(|&c| c == b'\n'); // :64
            let line_end = match e {
                Some(p) => i.min(text.len()) + p,
                None => text.len(),
            };
            let mut line: Vec<u8> = text[i.min(text.len())..line_end].to_vec(); // :65
            i = match e {
                Some(_) => line_end + 1,
                None => text.len() + 1, // :66
            };
            while matches!(line.last(), Some(b'\r') | Some(b' ') | Some(b'\t')) {
                line.pop(); // :67-68
            }
            let mut b = 0usize;
            while b < line.len() && (line[b] == b' ' || line[b] == b'\t') {
                b += 1; // :69-71
            }
            let line = &line[b..]; // :72
            if line.is_empty() {
                // :73-76
                if cur.code >= 0 {
                    out.push(cur);
                }
                cur = Glyph::new();
                nrow = 0;
                continue;
            }
            let dots = line.len() <= 5 && line.iter().all(|&c| c == b'.' || c == b'#'); // :80-83
            if dots && cur.code >= 0 && nrow < 8 {
                // :84-91
                let mut v: u8 = 0;
                for x in 0..(5usize.min(line.len())) {
                    if line[x] == b'#' {
                        v |= 1u8 << (4 - x);
                    }
                }
                cur.row[nrow] = v;
                nrow += 1;
                continue;
            }
            if line[0] == b'#' {
                continue; // :92-93
            }
            if let Some(code) = strtol_hex(line) {
                // :95-100 (end != start ⇔ converted; range = 0..256)
                if code >= 0 && code < 256 {
                    if cur.code >= 0 {
                        out.push(cur);
                    }
                    cur = Glyph::new();
                    nrow = 0;
                    cur.code = code as i32;
                }
            }
        }
        if cur.code >= 0 {
            out.push(cur); // :102
        }
        out.len() // :103
    }

    /// origin: lcdfont.h:106-117 — read-whole-file into bytes.
    /// QUIRK kept: `true` whenever fopen succeeded — a mid-file read error
    /// just ends the loop and still returns success with the partial text.
    pub(crate) fn read_file_text(path: &str) -> Option<Vec<u8>> {
        let mut f = std::fs::File::open(path).ok()?; // :108-110
        let mut out: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096]; // :111
        loop {
            match f.read(&mut buf) {
                // :113 while ((n = fread(...)) > 0)
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break, // fread error → 0 → loop ends, still true
            }
        }
        Some(out)
    }

    /// origin: lcdfont.h:121-133 — paint parsed glyphs onto a >=0x1000 rom
    /// (16 bytes/character). Returns the number painted (0 = nothing done).
    pub fn overlay(rom: &mut [u8], path: &str) -> usize {
        if rom.len() < 0x1000 {
            // :124
            return 0;
        }
        let text = match read_file_text(path) {
            Some(t) => t,
            None => return 0, // :124
        };
        let mut gs: Vec<Glyph> = Vec::new();
        if parse(&text, &mut gs) == 0 {
            return 0; // :127-128
        }
        for g in gs.iter() {
            // :129-131
            for y in 0..8usize {
                rom[g.code as usize * 16 + y] = g.row[y];
            }
        }
        gs.len() // :132
    }

    /// origin: lcdfont.h:146-169 — the search chain.
    /// `SMU2000_LCDFONT` (set, even to "", short-circuits the whole chain —
    /// C `if (const char *e = getenv(...))` distinguishes set-empty from
    /// unset, so this reads std::env directly instead of paths::env, which
    /// folds both to ""; deviation noted in the row report),
    /// then exe-dir + the four REL entries, then the config dir
    /// (`lcdfont.txt`, `art/lcdfont.txt`), then the same REL entries
    /// against the CURRENT DIRECTORY. First non-zero count wins.
    pub fn overlay_default(rom: &mut [u8]) -> usize {
        // :148-149
        if let Ok(e) = std::env::var("SMU2000_LCDFONT") {
            return overlay(rom, &e);
        }
        const REL: [&str; 4] = [
            // :150-153
            "art/lcdfont.txt",
            "../art/lcdfont.txt",
            "../../art/lcdfont.txt",
            "../../../art/lcdfont.txt",
        ];
        let base = exe_dir(); // :154
        if !base.is_empty() {
            for r in REL.iter() {
                // :155-158
                let n = overlay(rom, &join(&base, r));
                if n != 0 {
                    return n;
                }
            }
        }
        let cfg = config_dir(); // :159
        if !cfg.is_empty() {
            // :160-165
            let n = overlay(rom, &join(&cfg, "lcdfont.txt"));
            if n != 0 {
                return n;
            }
            let n = overlay(rom, &join(&cfg, "art/lcdfont.txt"));
            if n != 0 {
                return n;
            }
        }
        for r in REL.iter() {
            // :166-168 — plain relative names: the C runtime resolves
            // them against the process CWD, std::fs does the same.
            let n = overlay(rom, r);
            if n != 0 {
                return n;
            }
        }
        0 // :169
    }
}

#[cfg(test)]
mod tests;
