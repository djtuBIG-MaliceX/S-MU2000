// license:BSD-3-Clause
//
// origin: src/render.cpp (689 L) — MIDI を食わせて WAV に書き出す.
// Ledger row `render` (M6 bins / M3 closer), phase R-A: argv parse (all flags),
// ROM load + bus glue (wave + sintab into the SWP30), smf::load -> events,
// boot (fixed via --boot, or boot-wait on midi_ready), the per-sample loop
// (mu2000::run_sample cycle-debt -> run_cycles -> MIDI feed -> run_sample_pair
// -> s16 clamp -> WAV). Ground truth: run_tests.py:230 invokes
//   render <roms> <midi> <wav> <seconds> --boot 8.000 -v
// so duration_given=true and boot is FIXED (the midi_ready boot-wait loop is
// NOT exercised by the gate — it is transliterated for the no---boot path).
//
// Deviations (reported to ledger author, do NOT "fix" silently):
// - Unimplemented flags are ACCEPTED and warn-ignored on stderr (one line each).
//   Disk truth for --usb :356 (--bootcache is LIVE since M5-W5a —
//   bootcache.rs; disk seams render.cpp:194/272-273/380/391-398/413-414;
//   --state-at :252/534-540 is LIVE since M5-W4), --dump-dac/-meg
//   :338-352/594-599, --trace-meg :346-352 (--lcd-at/--lcd-every LIVE since
//   M5-LCD — disk seams render.cpp:233-236/493-533),
//   --voices-every :477-490, --part-rms :475/480-487, --adc-in :289-290/559-563
//   (A/D capture row), --card :302/642-649, --replay-swp :304-332/566-570,
//   --midi-block :533-538, --native-* / engine options (render.cpp:243
//   ui::consume_engine_option). None affect the firmware-path PCM.
// - --single (:235) is a no-op: this build is single-threaded (the machine has
//   no slave thread — mu2000.cpp:3413-3416 else-arm == Machine::run_sample_pair).
// - --fast-midi (:243 -> set_fast_midi) is warn-ignored (M7); leaving
//   Machine::midi.fast_midi=false keeps the DIN bit-pump path (M4 `midi lines`).
// - --boot is HONORED (fixed boot), unlike the boot-wait default. The harness
//   passes "8.000" POSITIONAL as <seconds> AND --boot 8.000; DISK CHECK: yes,
//   argv[4]="5.000" -> seconds positional (:271), argv[5..6] -> --boot (:210).
// - stdout/stderr use LF (Rust byte-verbatim); the fingerprint gate reads only
//   the WAV body (tools/fingerprint.py:131 `sha1(raw[cut*2:])`, cut=boot*ch),
//   never stdout, so stdout byte shape is not gated. stderr (.log) feeds only
//   the non-gated keyon list. The one diagnostic line printed is `[re] ...`
//   which the KEYON regex does not match.
// - SMU2000_RAMSNAP / SMU2000_RAMWATCH env probes (:428-465) and the verbose
//   per-device note histograms (:618-640) are omitted: not part of the firmware
//   audio path and not gated (the machine does not expose m_dbg_notes fields).

use smu_compat::{paths, roms};
use smu_machine::{bootcache, Machine};
use smu_smf::{self, Event};

use std::io::Write;

// origin: render.cpp:361 (const u32 rate = 44100) / mu2000.h:252/108
const RATE: u32 = 44100;
const DAC_FULL_SCALE: i32 = 1 << 17; // :574 "DAC の全振幅は 1<<17"
const MIDI_PORTS: i32 = 4; // mu2000.h:108 (render.cpp:542 clamp upper bound)

// origin: render.cpp:37-51 — 出来た WAV を 16bit stereo で書き出す
fn write_wav(path: &str, pcm: &[i16], rate: u32) {
    // :39-40 fopen "wb"; failure -> silent return (unlike boot.cpp's 書けない)
    let f = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let mut w = std::io::BufWriter::new(f);
    let bytes = (pcm.len() as u32).wrapping_mul(2); // :41 u32(pcm.size()*2)
    // :42-43 u32w / :44 u16w — both little-endian
    let u32w = |w: &mut std::io::BufWriter<std::fs::File>, v: u32| {
        let _ = w.write_all(&v.to_le_bytes());
    };
    let u16w = |w: &mut std::io::BufWriter<std::fs::File>, v: u16| {
        let _ = w.write_all(&v.to_le_bytes());
    };
    // :45 RIFF / size / WAVE
    let _ = w.write_all(b"RIFF");
    u32w(&mut w, 36u32.wrapping_add(bytes));
    let _ = w.write_all(b"WAVE");
    // :46-47 fmt  (PCM=1, stereo=2, rate, rate*4, block=4, bits=16)
    let _ = w.write_all(b"fmt ");
    u32w(&mut w, 16);
    u16w(&mut w, 1);
    u16w(&mut w, 2);
    u32w(&mut w, rate);
    u32w(&mut w, rate.wrapping_mul(4));
    u16w(&mut w, 4);
    u16w(&mut w, 16);
    // :48 data + length
    let _ = w.write_all(b"data");
    u32w(&mut w, bytes);
    // :49 fwrite(pcm.data(), 1, bytes, f) — interleaved s16 little-endian
    let mut buf: Vec<u8> = Vec::with_capacity(bytes as usize);
    for &v in pcm {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    let _ = w.write_all(&buf);
    // :50 fclose -> BufWriter flush on drop
}

// origin: render.cpp:91-107 — SysEx リセットの判別
fn reset_name(b: &[u8]) -> Option<&'static str> {
    if b.len() == 6 && b[0] == 0xf0 && b[1] == 0x7e && b[3] == 0x09 && b[5] == 0xf7 {
        if b[4] == 0x01 {
            return Some("GM System On");
        }
        if b[4] == 0x03 {
            return Some("GM2 System On");
        }
    }
    if b.len() >= 11
        && b[0] == 0xf0
        && b[1] == 0x41
        && (b[2] & 0xf0) == 0x10
        && b[3] == 0x42
        && b[4] == 0x12
        && b[5] == 0x40
        && b[6] == 0x00
        && b[7] == 0x7f
        && b[8] == 0x00
    {
        return Some("GS Reset");
    }
    if b.len() == 9
        && b[0] == 0xf0
        && b[1] == 0x43
        && (b[2] & 0xf0) == 0x10
        && b[3] == 0x4c
        && b[4] == 0x00
        && b[5] == 0x00
        && b[6] == 0x7e
        && b[7] == 0x00
        && b[8] == 0xf7
    {
        return Some("XG System On");
    }
    None
}

// origin: render.cpp:109-116
fn reset_bytes(mode: &str) -> Vec<u8> {
    match mode {
        "gm" => vec![0xf0, 0x7e, 0x7f, 0x09, 0x01, 0xf7], // :111-112
        "xg" => vec![0xf0, 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7e, 0x00, 0xf7], // :113-114
        _ => vec![0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7f, 0x00, 0x41, 0xf7], // :115
    }
}

// origin: render.cpp:118-133 — 先頭にリセットを差し込む（F5 のみは飛ばして数える）
fn insert_reset(events: &mut Vec<Event>, mode: &str) {
    // :120 first = events.empty() ? 0.05 : events.front().time
    let mut first = if events.is_empty() { 0.05 } else { events[0].time };
    // :121-126
    for ev in events.iter() {
        if !(ev.bytes.len() == 2 && ev.bytes[0] == 0xf5) {
            first = ev.time;
            break;
        }
    }
    // :127-131
    if first < 0.05 {
        let shift = 0.05 - first;
        for ev in events.iter_mut() {
            ev.time += shift;
        }
    }
    // :132 events.insert(begin, { 0.0, reset_bytes(mode), 0 })
    events.insert(0, Event { time: 0.0, bytes: reset_bytes(mode), port: 0 });
}

// origin: render.cpp:135-148
fn event_name(b: &[u8]) -> &'static str {
    if b.is_empty() {
        return "empty";
    }
    match b[0] & 0xf0 {
        0x80 => "note-off",
        // :140 bytes.size()>2 && bytes[2] ? note-on : note-off
        0x90 => {
            if b.len() > 2 && b[2] != 0 {
                "note-on"
            } else {
                "note-off"
            }
        }
        0xa0 => "poly-pressure",
        0xb0 => "control-change",
        0xc0 => "program-change",
        0xd0 => "channel-pressure",
        0xe0 => "pitch-bend",
        // :146 sysex / system
        _ => {
            if b[0] == 0xf0 {
                "sysex"
            } else {
                "system"
            }
        }
    }
}

// origin: render.cpp:150-157 — trace_midi 限定 (stdout; not gated)
fn trace_event(index: usize, ev: &Event, port: i32) {
    print!("MIDI event {index}: {:.6} s, port {}, {}:", ev.time, port + 1, event_name(&ev.bytes));
    for byte in &ev.bytes {
        print!(" {byte:02X}");
    }
    println!();
}

// origin: render.cpp:31-35 getenv_or (only used by the omitted RAMSNAP probes)
// — kept for parity of behavior: unused in R-A.

// origin: render.cpp:271/210 std::atof — leading ws, [sign], digits, '.', exp;
// trailing garbage ignored. Empty/garbage -> 0.0 (C strtod returns 0).
fn parse_u32_base0(s: &str) -> u32 {
    let t = s.trim();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t),
    };
    let (radix, d) = if let Some(r) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        (16, r)
    } else {
        (10, digits)
    };
    let v = u32::from_str_radix(d, radix).unwrap_or(u32::MAX);
    if neg {
        v.wrapping_neg()
    } else {
        v
    }
}

fn parse_u64_base0(s: &str) -> u64 {
    let t = s.trim();
    let (radix, d) = if let Some(r) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (16, r)
    } else {
        (10, t)
    };
    u64::from_str_radix(d, radix).unwrap_or(u64::MAX)
}

fn atof(s: &str) -> f64 {
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let start = i;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut seen_digit = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        seen_digit = true;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            seen_digit = true;
        }
    }
    if seen_digit && i < b.len() && (b[i] | 0x20) == b'e' {
        let save = i;
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        if i < b.len() && b[i].is_ascii_digit() {
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            i = save; // not a valid exponent; strtod stops before 'e'
        }
    }
    if let Ok(v) = s[start..i].parse::<f64>() {
        v
    } else {
        0.0
    }
}

// origin: mu2000.cpp:3186-3189 (cycle-debt) + :3380-3388 (run_cpu->run_cycles)
// + :3401-3453 (run_sample_pair). native engine OFF + not threaded, so the
// native/scope/threading arms of mu2000::run_sample (:3172-3379, :3404-3412)
// are inert on this path. Firmware is always run (m_cpu_enabled default true).
fn run_sample(m: &mut Machine, sintab: &[u16], debt: &mut u64) -> (i32, i32) {
    *debt = debt.wrapping_add(28_000_000); // :3187 (28 MHz)
    let cycles = *debt / 44100; // :3188
    *debt -= cycles.wrapping_mul(44100); // :3189
    m.run_cycles(cycles); // :3388 — the SH-2 + DIN bit pump
    m.run_sample_pair(sintab) // :3414-3415 + interconnect + master DAC (:3450-3453)
}

// origin: render.cpp:605-608 (and the boot-wait push :405-406). GCC -O3 FOLDS
// `l * 32768 / (1<<17)` into `l / 4` (idiv, trunc toward zero) — the product
// NEVER overflows the s32 intermediate there, so the old "wrapping two's
// complement" transliteration was wrong for every |l| >= 65536 (calshort:
// master DAC peak 86813 > 2^16, first divergence frame 421775, sign-flipped
// samples). Exhaustive ground truth over the full legal ADC domain
// [-131072, 131071] (build\render semantics, Makefile-canonical g++ flags,
// %TEMP%\opencode\s16gt): result == trunc(l/4) for ALL 262144 values.
// Rust '/' on i32 truncates toward zero identically.
#[inline]
fn to_s16(l: i32) -> i16 {
    let s = l / 4; // == l * 32768 / DAC_FULL_SCALE as the C++ binary folds it
    s.clamp(-32768, 32767) as i16
}

fn main() {
    paths::init_console_utf8();

    // origin: render.cpp:162-169 argc<4 -> usage exit 1. raw skips argv[0], so
    // raw[0]=roms argv[1], raw[1]=midi argv[2], raw[2]=wav argv[3].
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.len() < 3 {
        eprint!(
            "使い方: render <rom ディレクトリ> <MIDI ファイル> <出力 wav> [秒数] \
             [--trace-midi] [--reset gm|gs|xg] [--fast-midi]\n"
        ); // :165-166
        std::process::exit(1);
    }
    let dir = raw[0].clone(); // :169
    let mid = raw[1].clone(); // :169
    let wav = raw[2].clone(); // :169

    // :170-204 option state (only the firmware-path subset is honored)
    let mut seconds = 0.0f64; // :170
    let mut duration_given = false; // :171
    let mut trace_midi = false; // :172
    let usb_host = false; // :174 (parsed, warn-ignored — M7; firmware path is DIN-only)
    let mut forced_reset: Option<String> = None; // :177
    let mut swptrace: Option<String> = None; // :178
    let mut mu_dac_path: Option<String> = None; // :189-190
    let mut mu_dac_from: u32 = 0;
    let mut mu_dac_count: u32 = 0;
    let mut meg_path: Option<String> = None; // :191
    let mut meg_trace: Option<String> = None; // :192
    let mut meg_tr_from: u32 = 0; // :230-233
    let mut meg_tr_count: u32 = 0;
    let mut meg_tr_pc0: u32 = 0;
    let mut meg_tr_pc1: u32 = 0;
    let _single = false; // :179 (parsed; no-op — single-threaded build)
    let mut boot = -1.0f64; // :180 (<0 -> boot-wait on midi_ready)
    let mut use_bootcache = false; // :194 --bootcache。起動後の写しから始める（確かめ用）
    // DEV pc-trace window (boot->RE timers hunt; mirrors boot.rs:123-140 /
    // boot.cpp:39-50+84-89). No flag = byte-identical default behavior.
    let mut pctrace: Option<String> = None;
    let mut pcskip: u64 = 0;
    let mut pccount: u64 = 0;
    // origin: render.cpp:195 --state-at（確かめ用）— M5-W4 LIVE
    let mut state_at: Option<String> = None; // :195 const char *state_at
    let mut state_sample: usize = 0; // :195 size_t state_sample
    // origin: render.cpp:200-202 --lcd-at 秒 / --lcd-every 秒 — LIVE (M5-LCD)
    let mut lcd_at: f64 = -1.0; // :200 double lcd_at
    let mut lcd_every: f64 = 0.0; // :202 double lcd_every
    let mut lcd_next: f64 = 0.0; // :207 double lcd_next

    // origin: render.cpp:205-274 flag loop, raw[i] == argv[i+1]; loop starts at
    // argv[4] -> raw index 3.
    let mut i = 3usize;
    while i < raw.len() {
        let a = raw[i].as_str();
        let nxt = |k: usize| raw[k].clone();
        if a == "--trace-swp" && i + 1 < raw.len() {
            swptrace = Some(nxt(i + 1)); // :206-207 (M3 sink live)
            i += 2;
        } else if a == "--trace-pc" && i + 1 < raw.len() {
            pctrace = Some(nxt(i + 1)); // DEV (boot.rs:123-125 seam)
            i += 2;
        } else if a == "--pc-skip" && i + 1 < raw.len() {
            pcskip = parse_u64_base0(&nxt(i + 1)); // DEV (boot.rs:135-137)
            i += 2;
        } else if a == "--pc-count" && i + 1 < raw.len() {
            pccount = parse_u64_base0(&nxt(i + 1)); // DEV (boot.rs:138-140)
            i += 2;
        } else if a == "--replay-swp" && i + 1 < raw.len() {
            eprintln!("(rust render: --replay-swp は未実装 — 無視)"); // :208-209 M7
            i += 2;
        } else if a == "--boot" && i + 1 < raw.len() {
            boot = atof(&nxt(i + 1)); // :210-211 HONORED
            i += 2;
        } else if a == "--lcd-at" && i + 1 < raw.len() {
            lcd_at = atof(&nxt(i + 1)); // :233-234 HONORED
            i += 2;
        } else if a == "--lcd-every" && i + 1 < raw.len() {
            lcd_every = atof(&nxt(i + 1)); // :235-236 HONORED
            i += 2;
        } else if a == "--voices-every" && i + 1 < raw.len() {
            eprintln!("(rust render: --voices-every は未実装 — 無視)"); // :217-218
            i += 2;
        } else if a == "--part-rms" && i + 1 < raw.len() {
            eprintln!("(rust render: --part-rms は未実装 — 無視)"); // :219-220
            i += 2;
        } else if a == "--dump-dac" && i + 3 < raw.len() {
            // :221-225 — LIVE (debug seam ported: Swp30::dbg_* + awm2_step)
            mu_dac_path = Some(nxt(i + 1));
            mu_dac_from = parse_u32_base0(&nxt(i + 2)); // strtoul base 0 :223-224
            mu_dac_count = parse_u32_base0(&nxt(i + 3));
            i += 4;
        } else if a == "--dump-meg" && i + 1 < raw.len() {
            meg_path = Some(nxt(i + 1)); // :226-227 - LIVE (dump_meg seam ported)
            i += 2;
        } else if a == "--trace-meg" && i + 5 < raw.len() {
            // :228-234 - LIVE (debug seam ported: Swp30::dbg_meg* + meg_step)
            meg_trace = Some(nxt(i + 1));
            meg_tr_from = parse_u32_base0(&nxt(i + 2));  // :230
            meg_tr_count = parse_u32_base0(&nxt(i + 3)); // :231
            meg_tr_pc0 = parse_u32_base0(&nxt(i + 4));   // :232
            meg_tr_pc1 = parse_u32_base0(&nxt(i + 5));   // :233
            i += 6;
        } else if a == "--single" {
            // :235-236 no-op (single-threaded build)
            i += 1;
        } else if a == "--adc-in" && i + 1 < raw.len() {
            eprintln!("(rust render: --adc-in は未実装 — 無視)"); // :237-238 A/D row
            i += 2;
        } else if a == "--card" && i + 1 < raw.len() {
            eprintln!("(rust render: --card は未実装 — 無視)"); // :239-240 SmartMedia
            i += 2;
        } else if a == "--trace-midi" {
            trace_midi = true; // :241-242
            i += 1;
        } else if matches!(
            a,
            "--fast-midi" | "--native-fx" | "--native-fx-full" | "--native-engine"
                | "--cal" | "--nocal" | "--voicecache" | "--no-voicecache"
        ) {
            // :243 ui::consume_engine_option (all no-value flags). --fast-midi
            // -> set_fast_midi is M7; native engine not built (AGENTS). Ignore.
            if a == "--fast-midi" {
                eprintln!("(rust render: --fast-midi は未実装 — 無視)");
            }
            i += 1;
        } else if a == "--usb" {
            eprintln!("(rust render: --usb は未実装 — 無視)"); // :244-245 M7
            i += 1;
        } else if a == "--native-off" && i + 1 < raw.len() {
            eprintln!("(rust render: --native-off は未実装 — 無視)"); // :246-247
            i += 2;
        } else if a == "--midi-block" && i + 1 < raw.len() {
            eprintln!("(rust render: --midi-block は未実装 — 無視)"); // :248-249
            i += 2;
        } else if a == "--bootcache" {
            use_bootcache = true; // :272-273 — LIVE (M5-W5a, bootcache.rs)
            i += 1;
        } else if a == "--state-at" && i + 2 < raw.len() {
            // :252-255 — LIVE (M5-W4): --state-at <sample> <file>
            state_sample = parse_u64_base0(&nxt(i + 1)) as usize; // :275 strtoull base 0
            state_at = Some(nxt(i + 2)); // :276
            i += 3;
        } else if a == "--reset" {
            // :256-267 validate
            if i + 1 >= raw.len() {
                eprintln!("--reset requires gm, gs, or xg"); // :258
                std::process::exit(1); // :259
            }
            let mode = nxt(i + 1);
            if mode != "gm" && mode != "gs" && mode != "xg" {
                eprintln!("unknown reset mode: {mode} (expected gm, gs, or xg)"); // :264
                std::process::exit(1); // :265
            }
            forced_reset = Some(mode); // :261
            i += 2;
        } else if a == "-v" {
            paths::set_verbose(true); // :268-269 g_verbose (print-only)
            i += 1;
        } else {
            seconds = atof(a); // :271-272 seconds positional
            duration_given = true; // :272
            i += 1;
        }
    }
    let _ = usb_host; // parsed but ignored (M7); firmware path uses DIN only

    // origin: render.cpp:276-278 smf::load
    let mut events: Vec<Event> = Vec::new();
    let mut err = String::new();
    if !smu_smf::load(&mid, &mut events, &mut err) {
        eprintln!("{err}"); // :278
        std::process::exit(1);
    }
    // :279-285 forced reset: erase existing resets, insert the forced one
    if let Some(mode) = &forced_reset {
        events.retain(|ev| reset_name(&ev.bytes).is_none()); // :280-282
        insert_reset(&mut events, mode); // :283
        let nm = events.first().and_then(|e| reset_name(&e.bytes)).unwrap_or("");
        println!("MIDI reset forced: {nm}"); // :284
    }
    let last_time = events.last().map(|e| e.time).unwrap_or(0.0); // :286-287
    println!("MIDI: {} イベント、最後は {:.2} 秒", events.len(), last_time);

    // origin: render.cpp:292-300 — load prog + wave (fatal), sintab (warning
    // only). Machine::new(prog) is attach-before-reset (mu2000.cpp:383-393/997);
    // wave is parked on the Machine and consumed by run_sample_pair (Wave::new).
    let prog = match roms::load_program(&format!("{dir}/mu2000_flash.bin")) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}"); // :294
            std::process::exit(1);
        }
    };
    let wave = match roms::load_wave(&format!("{dir}/dump")) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}"); // :297
            std::process::exit(1);
        }
    };
    let mut m = Machine::new(prog);
    m.wave = wave; // render-side wave bus glue (mu2000.cpp:395-402, M4 deferral)
    // :299-300 sintab: WARNING only, does not change exit code. The SWP30 sin
    // stand-in is fed to run_sample_pair (the deferred set_sintab_rom glue).
    let sintab: Vec<u16> = match roms::load_sintab(&format!("{dir}/standin/sin-table.bin")) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("警告: {e}"); // :300
            Vec::new()
        }
    };

    // origin: render.cpp:334-336 --trace-swp BEFORE reset. fopen failure here is
    // SILENT on disk (`if (tf) set...`), unlike boot.cpp's 書けない exit.
    if let Some(t) = &swptrace {
        if let Ok(f) = std::fs::File::create(t) {
            m.set_swp_trace(f, true); // :336 (with_reads = true)
        }
    }

    // origin: render.cpp:338-343 --dump-dac (master only; SWP30_CHAN env picks
    // the voice, same seam as C++). fopen failure = silent (disk has no check).
    if let Some(t) = &mu_dac_path {
        if let Ok(f) = std::fs::File::create(t) {
            let mut d = m.swpm.borrow_mut();
            d.dbg_dac = Some(f);
            d.dbg_dac_from = mu_dac_from;
            d.dbg_dac_count = mu_dac_count;
            if let Ok(c) = std::env::var("SWP30_CHAN") {
                d.dbg_chan = c.trim().parse::<i64>().unwrap_or(-1) as i32; // :343
            }
        }
    }

    // origin: render.cpp:345-352 --trace-meg (master only); fopen failure
    // silent like the dump-dac seam (no check on disk).
    if let Some(t) = &meg_trace {
        if let Ok(f) = std::fs::File::create(t) {
            let mut d = m.swpm.borrow_mut();
            d.dbg_meg = Some(f);
            d.dbg_meg_from = meg_tr_from;  // :348
            d.dbg_meg_count = meg_tr_count; // :349
            d.dbg_meg_pc0 = meg_tr_pc0 as u16; // :350 u16 truncation
            d.dbg_meg_pc1 = meg_tr_pc1 as u16; // :351
        }
    }

    // render.cpp:376-377 set_threaded(!single) / apply_engine_options:
    // single-threaded (no slave thread), firmware path (native off) — no-ops.
    // :378 set_usb_host — M7: --usb warn-ignored above, so this stays false
    // (behavior AND the key input below stay the consistent DIN-path pair).
    // :379-380 鍵は起動に使うワーク RAM も混ぜるので reset() の前に作る
    let boot_key = if use_bootcache { bootcache::key(&m) } else { 0 }; // :380
    // :381 reset AFTER the trace sink is installed.
    m.reset();

    // DEV: AFTER reset, exactly boot.rs:198-200 (boot.cpp:83-89).
    if let Some(p) = &pctrace {
        m.set_trace_pc(p, pcskip, pccount);
    }

    let mut pcm: Vec<i16> = Vec::new();

    // origin: render.cpp:383-398 — 起動後の写しから始める（--bootcache）。
    // **確かめ用**で既定では使わない (:391-392). Try-cache BEFORE the boot
    // wait; a hit skips boot entirely (boot = 0.0, :396).
    if use_bootcache && boot < 0.0 {
        // :393 condition use_bootcache && boot < 0.0 (the harness passes
        // --boot, so this arm — like the whole boot-wait — is not exercised
        // by the fingerprint gate)
        if bootcache::load(&mut m, boot_key) {
            // :395 printf text-mode -> CRLF on Windows (ledger CRLF pitfall)
            print!("起動: 前の写しから\r\n");
            boot = 0.0; // :396
        }
    }

    // mu2000.h:918 m_cycle_debt — the machine owns the debt (render.cpp's
    // loop calls mu.run_sample, same member). 0 after reset; a snapshot load
    // restores it (state leg mu2000.cpp:3545). Read AFTER the load attempt.
    let mut debt: u64 = m.cycle_debt;

    // origin: render.cpp:399-419 boot-wait (boot < 0). NOT used by the gate
    // (harness passes --boot). Faithful transliteration for the no---boot path.
    if boot < 0.0 {
        let limit = (30.0 * RATE as f64) as usize; // :400
        let mut n = 0usize;
        while n < limit && !m.midi_ready(0) {
            // :402 midi_ready() -> mu2000.h:112 port default 0
            let (l, r) = run_sample(&mut m, &sintab, &mut debt); // :404 (member debt)
            pcm.push(to_s16(l)); // :405
            pcm.push(to_s16(r)); // :406
            n += 1;
        }
        boot = n as f64 / RATE as f64; // :412
        if use_bootcache && n < limit {
            bootcache::save(&mut m, boot_key); // :413-414 起動に成功したときだけ
        }
        if n >= limit {
            eprintln!("起動を待ったが MIDI 受信が有効にならなかった"); // :416
            std::process::exit(1); // :417
        }
        println!("起動に {boot:.6} 秒。ここから MIDI を流す"); // :419
    }

    // origin: render.cpp:395-398
    let boot_samples = (boot * RATE as f64 + 0.5) as usize; // :395
    let estimated_seconds = if duration_given {
        seconds
    } else if events.is_empty() {
        3.0
    } else {
        events.last().unwrap().time + 3.0
    };
    pcm.reserve(((boot + estimated_seconds) * RATE as f64) as usize * 2); // :398

    // origin: render.cpp:400-411
    let mut port: i32 = -1; // :404 (-1 -> follow SMF port)
    let mut next = 0usize; // :405
    let mut scheduled_events = 0usize; // :406
    let mut scheduled_bytes = 0usize; // :406
    let mut tail_start = usize::MAX; // :407 (size_t(-1))
    let hard_stop = if duration_given {
        ((boot + seconds) * RATE as f64) as usize // :408
    } else {
        usize::MAX
    };
    // debt: declared above (:556-581) — continuous member debt through the
    // boot-wait / snapshot load into this loop (mu2000.h:918).
    let mut re_seen = false; // diagnostic: proof RE (midi_ready) rises in-session
    let mut tmr_seen = false; // diagnostic: first finite emu_timer due-time (sci4 enable)

    // origin: render.cpp:412-581 the sample loop (for (i = pcm.size()/2;; i++))
    let mut i = pcm.len() / 2;
    loop {
        if duration_given && i >= hard_stop {
            break; // :413-414
        }
        // :415-423 no explicit duration: stop 3 s after the MIDI goes idle
        if !duration_given && next == events.len() && m.midi_idle_all() {
            if tail_start == usize::MAX {
                tail_start = i;
                println!(
                    "MIDI queue drained at {:.3} s; rendering 3.0 s tail",
                    i as f64 / RATE as f64 - boot
                );
            }
            if i >= tail_start + (3.0 * RATE as f64) as usize {
                break;
            }
        }

        // diagnostic (not gated): first sample where SCI RE is up = M3 alive
        if !re_seen && m.midi_ready(0) {
            re_seen = true;
            eprintln!(
                "[re] midi_ready(RISE) sample {i} ({:.5} s)",
                i as f64 / RATE as f64
            );
        }
        // diagnostic (not gated): first finite emu_timer due-time = sci4 enabled
        if !tmr_seen {
            let nt = m.rm.next_timer_cycles();
            if nt != u64::MAX {
                tmr_seen = true;
                eprintln!("[diag] first finite next_timer_cycles={nt} at sample {i}");
            }
        }

        // origin: render.cpp:493-501 --lcd-at one-shot LCD dump (M5-LCD). The
        // dump sits at the disk seam :493 — BEFORE t (:557) and MIDI delivery
        // (:566). ddram = m.lcd (mu2000.h m_lcd; hd44780.rs:74-75 m_ddram);
        // borrow ends in the block, never held across run_sample.
        if lcd_at >= 0.0 && i >= ((boot + lcd_at) * RATE as f64) as usize {
            lcd_at = -1.0; // :494
            let dd = { m.lcd.borrow().m_ddram }; // :495 const u8 *dd = mu.lcd().ddram()
            // printf text mode on Windows emits CRLF (ledger CRLF pitfall; cf. 起動:)
            print!("LCDHEX"); // :496
            for line in 0..2 {
                // :497-499
                for pos in 0..24 {
                    print!(" {:02x}", dd[line * 0x40 + pos]); // :499
                }
            }
            print!("\r\n"); // :500 "\n" text mode
        }
        // :502-503 --part-rms: ignored (unimplemented, warn at parse)
        // :504-517 --voices-every: ignored (unimplemented, warn at parse)
        // origin: render.cpp:518-533 --lcd-every periodic LCD+CG dump (M5-LCD)
        if lcd_every > 0.0 && i >= ((boot + lcd_next) * RATE as f64) as usize {
            let now = lcd_next; // :519 const double now
            lcd_next += lcd_every; // :520
            let dd = { m.lcd.borrow().m_ddram }; // :521
            print!("LCD {now:.3}"); // :522 printf "LCD %.3f"
            for line in 0..2 {
                // :523-525
                for pos in 0..24 {
                    print!(" {:02x}", dd[line * 0x40 + pos]); // :525
                }
            }
            print!("\r\n"); // :526
            // 外字（音色の絵）も出す。1 文字 8 バイト × 8 文字
            let cg = { m.lcd.borrow().m_cgram }; // :528 (hd44780.rs:88-89 m_cgram)
            print!("CG {now:.3}"); // :529 printf "CG %.3f"
            for k in 0..64 {
                print!(" {:02x}", cg[k]); // :531
            }
            print!("\r\n"); // :532
        }

        // origin: render.cpp:530 — 起動ぶんは整数で引く (no cancellation drift)
        let t = (i as f64 - boot_samples as f64) / RATE as f64;
        // :533-538 --midi-block: ignored (midi_block=0) — no block jump

        // origin: render.cpp:539-556 — feed events whose time has come
        while next < events.len() && events[next].time <= t {
            let ev = &events[next];
            if ev.bytes.len() == 2 && ev.bytes[0] == 0xf5 {
                // :541-542 F5 nn -> port switch, NOT forwarded to firmware
                port = (ev.bytes[1] as i32 - 1).clamp(0, MIDI_PORTS - 1);
            } else {
                // :544-545 route: explicit port, else SMF track port folded to DIN
                let to = if port >= 0 {
                    port
                } else {
                    smu_smf::mu_port(ev.port, true, false) // usb_host ignored -> false
                };
                if trace_midi {
                    trace_event(next, ev, to); // :546-547
                }
                if let Some(rn) = reset_name(&ev.bytes) {
                    // :548-549
                    println!("MIDI reset: {:.6} s, port {}, {}", ev.time, to + 1, rn);
                }
                // :550-551 mu.midi_in(b, to) — one received byte on `to`
                for &b in ev.bytes.iter() {
                    m.midi_in(b, to);
                }
                scheduled_events += 1; // :552
                scheduled_bytes += ev.bytes.len(); // :553
            }
            next += 1; // :555
        }
        // :559-563 --adc-in: empty (ignored); :564-570 --replay-swp: empty (ignored)

        // origin: render.cpp:534-540 --state-at（確かめ用, M5-W4）— absolute
        // sample i == size_t(boot*rate) + state_sample. NOTE disk :534 uses
        // `size_t(boot * rate)` TRUNCATION, not the +0.5-rounded
        // boot_samples of :395/:574 — transliterated as written. fopen "wb"
        // failure is SILENT on disk (`if (FILE *sf = ...)`)
        if let Some(path) = &state_at {
            if i == (boot * RATE as f64) as usize + state_sample {
                let st = smu_machine::state::save_state(&mut m); // :535
                if let Ok(mut f) = std::fs::File::create(path) {
                    // :536-538 fwrite(st.data(),1,size) — silent on fail
                    let _ = std::io::Write::write_all(&mut f, &st);
                }
            }
        }

        // origin: render.cpp:571-577 — run_sample + DAC scale + clamp + push
        let (l, r) = run_sample(&mut m, &sintab, &mut debt);
        pcm.push(to_s16(l));
        pcm.push(to_s16(r));

        // :579-580 progress (stdout; not gated)
        if i % (RATE as usize * 5) == 0 {
            println!("  {:5.1} 秒  PC={:08x}", i as f64 / RATE as f64 - boot, m.pc());
        }
        i += 1;
    }

    // origin: render.cpp:583-589 scheduled/pending/dropped report (stdout)
    let total = pcm.len() / 2;
    println!(
        "MIDI scheduled: {} events, {} bytes; pending: {}; dropped: {}",
        scheduled_events,
        scheduled_bytes,
        m.midi_pending(),
        m.midi_dropped()
    );
    if next != events.len() {
        println!(
            "MIDI unscheduled: {} events (explicit render duration reached)",
            events.len() - next
        ); // :587-589
    }

    // origin: render.cpp:594-599 --dump-meg (master ".m" + slave ".s"; the
    // C++ stdout summary printf is omitted like the other render stats)
    if let Some(p) = &meg_path {
        m.swpm.borrow().dump_meg(&format!("{p}.m")); // :596
        m.swps.borrow().dump_meg(&format!("{p}.s")); // :597
    }

    // :591 fclose(tf) -> sink dropped with the machine
    // DIAG (R-A): did the firmware ever leave its SWP-init poll? deferred hits?
    eprintln!(
        "[diag] re_rise_seen={} deferred_hits={} timer_fires={} event_fires={} loops={} next_timer_cycles={} total_cycles={}",
        re_seen,
        m.swp_deferred_total(),
        m.timer_fires,
        m.event_fires,
        m.loops,
        m.rm.next_timer_cycles(),
        m.total_cycles()
    );

    // :594-599 --dump-meg / :601-606 verbose maxima / :616 print_swp_widths /
    // :618-640 verbose note histograms: stdout-only, machine fields not exposed
    // (not gated). Omitted.

    // origin: render.cpp:607-614 CPU + loop accounting (stdout)
    println!(
        "CPU {} サイクル / {} サンプル = {:.3}（あるべき値 {:.3}）",
        m.total_cycles(),
        total,
        m.total_cycles() as f64 / total as f64,
        28000000.0 / RATE as f64
    );
    println!(
        "実行ループ {} 周（1 サンプルあたり {:.2} 周）、タイマ {} 回、周辺イベント {} 回",
        m.loops,
        m.loops as f64 / total as f64,
        m.timer_fires,
        m.event_fires
    );

    // :642-649 --card writeback (ignored). :651 write_wav
    write_wav(&wav, &pcm, RATE);

    // :652-686 native-engine accounting: skipped (native off). :687 final line.
    println!("書き出した: {wav}（{:.1} 秒）", total as f64 / RATE as f64);
    let _ = _single;
    // exit 0
}
