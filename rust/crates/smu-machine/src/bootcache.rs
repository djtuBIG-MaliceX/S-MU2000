// license:BSD-3-Clause
//
// origin: src/bootcache.h (274 L) — 起動後の状態を取っておいて、次からはそれを読み込む.
// Ledger row `bootcache` (M5). The snapshot file is: 12-byte "S2BC" envelope +
// mu2000::save_state() verbatim (state.rs, "S2MU"). A bad/stale snapshot is
// NEVER fatal — it is dropped and the next run boots fresh (bootcache.h:8
// comment: the restored state is bit-identical to a full boot, proven by
// statetest; anything that cannot be proven settled gets rebuilt).
//
// Layout note (no padding surprises): the C++ envelope is three u32 in a
// plain array (`const u32 h[3]`, bootcache.h:56) written as raw bytes —
// little-endian on x86. Rust writes each field with explicit LE byte arrays;
// byte-identical, no repr tricks.
//
// DEVIATIONS (disclosed):
// - refresh() takes `sintab: &[u16]`: C++ owns the sin table in the machine
//   (set_sintab_rom); the Rust machine feeds it per call (run_sample_pair),
//   same seam as render.rs's run_sample helper.
// - key()/load()/save() read the machine's fields directly (soc.bus.rom =
//   m_prog mu2000.h:49, soc.bus.ram = m_ram mu2000.h:81/870, wave = m_wave
//   :50, midi.usb_host = m_usb_host :1022 — pinned false until M7).
// - save()/refresh() take &mut where disk is `const` (state.rs's disclosed
//   save_state const_cast deviation propagates).
// - The read-error line uses the ja text table (texts_ja.h:35) and emits CRLF
//   like the C++ text-mode fprintf (bootcache.h:154 UI_TEXT).

use crate::state;
use crate::Machine;
use smu_compat::paths;

/// origin: bootcache.h:49 — "S2BC" envelope magic.
pub const K_ENV_MAGIC: u32 = 0x43423253;
/// origin: bootcache.h:50 — bump when snapshot semantics change; older
/// generations are ignored AND removed, rebuilt by one full boot (:44-48).
pub const K_ENV_VERSION: u32 = 1;
/// origin: bootcache.h:51 — LCD past the mid-boot transient.
pub const K_ENV_SETTLED: u32 = 1;
/// origin: bootcache.h:52 — three u32.
pub const K_ENV_SIZE: usize = 12;

/// origin: bootcache.h:54-59 — append {magic, version, flags} as raw LE bytes.
fn write_envelope(out: &mut Vec<u8>, flags: u32) {
    // :56 const u32 h[3] = { kEnvMagic, kEnvVersion, flags };
    out.extend_from_slice(&K_ENV_MAGIC.to_le_bytes());
    out.extend_from_slice(&K_ENV_VERSION.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes()); // :58 insert(b, b+kEnvSize)
}

/// origin: bootcache.h:69-82 — true for a CURRENT, SETTLED snapshot only.
/// Anything else (older generation, raw blob, corruption, other save-format
/// version) must be rebuilt, never loaded (:62-68). The inner blob header is
/// mu2000.cpp's "S2MU" magic then version (:77-81).
pub fn check_envelope(data: &[u8]) -> bool {
    if data.len() < K_ENV_SIZE + 8 {
        // :71 n < kEnvSize + 8
        return false;
    }
    let rd = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
    // :73-74 u32 h[3]; memcpy(h, data, kEnvSize) — x86 LE
    if rd(0) != K_ENV_MAGIC || rd(4) != K_ENV_VERSION || (rd(8) & K_ENV_SETTLED) == 0 {
        // :75
        return false;
    }
    // :77-81 magic == "S2MU" && version == mu2000::state_version()
    rd(K_ENV_SIZE) == 0x554d3253 && rd(K_ENV_SIZE + 4) == state::state_version()
}

/// origin: bootcache.h:86-112 — 鍵。プログラム ROM・ワーク RAM・波形 ROM・
/// 状態の版から作る。**reset() の前に、起動に使う RAM が入った状態で呼ぶこと** (:85).
/// The mix lambda (:88-91) is the M1-pinned `paths::fnv1a_mix`; the program-ROM
/// part is nvram::rom_key (nvram.h:37-46) == `paths::fnv1a64` (both pinned in
/// paths.rs / paths/tests.rs — NOT re-derived here).
pub fn key(mu: &Machine) -> u64 {
    let mix = |h: u64, b: u8| paths::fnv1a_mix(h, b); // :88-91
    let mut h = paths::fnv1a64(&mu.soc.bus.rom); // :92 プログラム ROM 4MB (rom_key)
    for b in mu.soc.bus.ram.iter() {
        // :93-95 起動に使ったワーク RAM (m_ram, mu2000.h:81)
        h = mix(h, *b);
    }
    // :98 if (const auto wave = mu.wave_rom()) — DEVIATION: empty Vec stands
    // for the null view (a loaded wave is always non-empty, 32MB).
    if !mu.wave.is_empty() {
        let n = mu.wave.len() as u64; // :99 n = wave->size()
        for k in 0..8 {
            // :100-101 大きさだけ見る (8 LE bytes of size_t)
            h = mix(h, (n >> (k * 8)) as u8);
        }
        // :102 頭・真ん中・終わりの 4KB ずつ
        let spots = [0usize, mu.wave.len() / 2, if mu.wave.len() > 4096 { mu.wave.len() - 4096 } else { 0 }];
        for at in spots {
            let mut i = 0usize;
            while i < 4096 && at + i < mu.wave.len() {
                // :103-105
                h = mix(h, mu.wave[at + i]);
                i += 1;
            }
        }
    }
    let ver = state::state_version(); // :107-108 状態の形の版 (4 LE bytes)
    for k in 0..4 {
        h = mix(h, (ver >> (k * 8)) as u8);
    }
    // :110 HOST SELECT（USB か MIDI か）でも起動後の姿が変わる
    h = mix(h, if mu.midi.usb_host { 1 } else { 0 });
    h
}

/// origin: bootcache.h:115-126 — 置き場。作れなければ空. Shares the M1
/// `paths::subdir_keyed_path` ("<settings>\\boot\\<%016llx>.bin").
pub fn path(k: u64) -> String {
    let base = paths::ensure_config_dir(); // :117
    if base.is_empty() {
        // :118-119
        return String::new();
    }
    paths::subdir_keyed_path(&base, "boot", k) // :120-125 (join + ensure_dir + name)
}

/// origin: bootcache.h:130-159 — 起動後の状態を読む。**reset() の代わりに呼ぶ**.
/// Fail-open everywhere: any problem = miss + (for a corrupt file) removal,
/// never fatal (:146-157; a bad cache just means booting fresh).
pub fn load(mu: &mut Machine, k: u64) -> bool {
    let p = path(k); // :132
    if p.is_empty() {
        // :133-134
        return false;
    }
    // :135-143 fopen "rb" + seek/tell + full fread. fs::read covers
    // open/size/read in one; Err stands for any of fopen-null / short-read.
    let buf = match std::fs::read(&p) {
        Ok(b) => b,
        Err(_) => return false, // :136-137 (!f) / :142-145 (!read_ok — empty or short)
    };
    if buf.is_empty() {
        // :141-142 size<=0 -> buf empty -> read_ok false (file NOT removed)
        return false;
    }
    // :148-151 not a current settled snapshot: drop it so a full boot rebuilds
    if !check_envelope(&buf) {
        let _ = std::fs::remove_file(&p);
        return false;
    }
    // :152-157 load_state over the bytes after the envelope
    let mut err = String::new();
    if !state::load_state(mu, &buf[K_ENV_SIZE..], &mut err) {
        // bootcache.h:154 UI_TEXT(bootcache_read_error_fmt) — texts_ja.h:35;
        // CRLF mirrors the C++ text-mode fprintf.
        print!("起動の写しを読めない: {err}\r\n");
        let _ = std::fs::remove_file(&p);
        return false;
    }
    true
}

/// origin: bootcache.h:162-184 — 起動し切った所で残す。**起動に成功したときだけ呼ぶこと** (:161).
/// Envelope(kEnvSettled) + save_state, via tmp+replace (".new" suffix per :173)
/// so a crash mid-write never leaves a broken copy (:172).
pub fn save(mu: &mut Machine, k: u64) -> bool {
    let p = path(k); // :164
    if p.is_empty() {
        // :165-166
        return false;
    }
    let st = state::save_state(mu); // :167 (const mu2000& — see state.rs deviation)
    let mut out: Vec<u8> = Vec::with_capacity(K_ENV_SIZE + st.len()); // :168-169
    write_envelope(&mut out, K_ENV_SETTLED); // :170
    out.extend_from_slice(&st); // :171
    // :173-183 fopen(tmp,"wb") + fwrite(size) + fclose, remove on failure,
    // then replace — paths::write_file_atomic transliterates that nvram.h shape.
    paths::write_file_atomic(&p, &out, ".new") // :177/:183
}

/// origin: bootcache.h:192-242 — 設定（ワーク RAM）を書き戻したあとに呼ぶ。
/// The live machine's RAM boots a fresh machine up to the settled point and
/// saves, keeping the next boot fast (:186-191). `sintab`: DEVIATION (see doc
/// header) — the caller carries the sin table the C++ machine owns.
pub fn refresh(live: &Machine, sintab: &[u16]) -> bool {
    // :194-196 `ram.empty()` is structurally false in Rust (sized Box); the
    // program_rom guard below is the live check (mu2000.h:49).
    if live.soc.bus.rom.is_empty() {
        return false; // :195 !live.program_rom()
    }

    // :198-204 fresh.set_program_rom / set_wave_rom / set_sintab_rom /
    // set_usb_host / set_nvram (size mismatch -> false; sizes match by type).
    let mut fresh = Machine::new(live.soc.bus.rom.clone()); // :199
    fresh.wave = live.wave.clone(); // :200
    fresh.midi.usb_host = live.midi.usb_host; // :202 (sintab :201 = `sintab` param)
    fresh.soc.bus.ram.copy_from_slice(&live.soc.bus.ram[..]); // :203 set_nvram

    let k = key(&fresh); // :206
    // :207-224 a current settled snapshot already there means nothing to do;
    // anything else (missing, older generation, unsettled) gets rebuilt.
    let p = path(k); // :210
    if p.is_empty() {
        // :211-212
        return false;
    }
    if let Ok(bytes) = std::fs::read(&p) {
        // :213-221 fopen + head[kEnvSize+8] read + check_envelope(head)
        if bytes.len() >= K_ENV_SIZE + 8 && check_envelope(&bytes[..K_ENV_SIZE + 8]) {
            // :222-223 current -> false (no rebuild)
            return false;
        }
    }

    // :226-234 boot until midi_ready, cap 30 s (run_sample == the machine
    // cycle-debt helper shape, mu2000.cpp:3186-3189/3388/3401-3453 — same as
    // render.rs's run_sample; a fresh machine has m_cycle_debt = 0, :925).
    fresh.reset(); // :226
    let limit: u64 = 30 * 44100; // :227
    let mut debt: u64 = 0;
    let mut i: u64 = 0; // :228
    while i < limit && !fresh.midi_ready(0) {
        // :229-232
        run_sample(&mut fresh, sintab, &mut debt);
        i += 1;
    }
    if i >= limit {
        // :233-234 boot never completed — leave nothing behind
        return false;
    }
    // :236-240 settle past the mid-boot LCD transient before saving (the
    // snapshot keeps whatever frame is up)
    for _ in 0..2 * 44100 {
        run_sample(&mut fresh, sintab, &mut debt);
    }
    save(&mut fresh, k) // :241
}

/// origin: mu2000.cpp:3186-3189 (cycle-debt) + :3388 (run_cycles) +
/// :3401-3453 (run_sample_pair) — the boot loop of refresh() needs the
/// machine's run_sample; same transliteration as render.rs's helper.
fn run_sample(m: &mut Machine, sintab: &[u16], debt: &mut u64) -> (i32, i32) {
    *debt = debt.wrapping_add(28_000_000); // :3187 (28 MHz)
    let cycles = *debt / 44100; // :3188
    *debt -= cycles.wrapping_mul(44100); // :3189
    m.run_cycles(cycles); // :3388
    m.run_sample_pair(sintab) // :3414-3415 + interconnect + master DAC
}

/// origin: bootcache.h:246-269 — 写しは 1 つ 6MB ほどある…新しいほうから
/// keep 個だけ残す (default `keep = 4`, :246 — callers pass 4).
pub fn prune(keep: usize) {
    let base = paths::config_dir(); // :248 config_dir (NOT ensure_config_dir)
    if base.is_empty() {
        // :249-250
        return;
    }
    let dir = paths::join(&base, "boot"); // :251
    if !paths::is_dir(&dir) {
        // :252-253
        return;
    }
    // :254-260 鍵の名前のものだけ。人が置いた物は触らない —
    // 20 chars, [0..16] lowercase hex, ".bin" at 16 (find_first_not_of == 16:
    // '.' at 16 is the first non-hex, guaranteed by the ".bin" test).
    let is_key = |name: &str| {
        name.len() == 20
            && &name[16..20] == ".bin"
            && name[..16].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    let mut files: Vec<paths::DirEntry> =
        paths::list_dir(&dir).into_iter().filter(|e| is_key(&e.name)).collect();
    if files.len() <= keep {
        // :261-262 int(files.size()) <= keep
        return;
    }
    // :263-266 sort newest-first (C++ std::sort — unstable, mirrored)
    files.sort_unstable_by(|a, b| b.mtime.cmp(&a.mtime));
    for e in &files[keep..] {
        // :267-268 remove the stale ones
        let _ = std::fs::remove_file(paths::join(&dir, &e.name));
    }
}
