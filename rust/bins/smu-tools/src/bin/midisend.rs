// license:BSD-3-Clause
//
// origin: src/midisend.cpp (117 L) — MIDI ファイルを Windows の MIDI 出力へ
// 実時間で流す (:3-9). winmm calls ride smu_hal_win::midi_out (waveout.rs
// style); SMF load rides smu_smf (row `smf`). Same argv semantics (:45-49),
// same sleep-pacing (:82-88), no OS tempo converter anywhere (the pacing
// IS the clock: timeGetTime vs event.time, timeBeginPeriod(1) at :74).
// Deviations:
// - stdout/stderr write EOL = CRLF on Windows (boot.rs:24 precedent —
//   mirrors the CRT text-mode \n->\r\n of the C++ printf, byte-exact to a
//   redirected handle).
// - caps.szPname printed as RAW ANSI bytes via write_all (midi_out.rs note)
//   = C++ printf("%s", caps.szPname) byte-for-byte (midisend.cpp:31/72).
// - atoi is the same C99-shape helper family as boot.rs strtoull (skip ws,
//   sign, digits, stop at first non-digit, wrapping accumulation).
// - smf::load past-EOF leniencies already settled in the `smf` row
//   (Rust yields 0 where C++ reads in-buffer garbage).

use std::io::Write;

use smu_hal_win::midi_out as mo;
use smu_hal_win::sys;
use smu_smf::Event;

const EOL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// origin: midisend.cpp:24-35 list_outputs — midiOutGetNumDevs + CapsA per
/// device; "  （なし）" when the count is zero (:33-34).
fn list_outputs() {
    let n = unsafe { mo::midiOutGetNumDevs() }; // :26
    print!("MIDI 出力:{EOL}"); // :27
    for i in 0..n {
        // MIDIOUTCAPSA caps{}; — zeroed every iteration (:29)
        let mut caps: mo::MidiOutCapsA = unsafe { std::mem::zeroed() };
        let r = unsafe {
            mo::midiOutGetDevCapsA(i, &mut caps, std::mem::size_of::<mo::MidiOutCapsA>() as u32)
        };
        if r == mo::MMSYSERR_NOERROR {
            // printf("  %u: %s\n", i, caps.szPname) — raw A bytes (:30-31)
            let so = std::io::stdout();
            let mut so = so.lock();
            let _ = so.write_all(format!("  {i}: ").as_bytes());
            let _ = so.write_all(&mo::pname_bytes(&caps));
            let _ = so.write_all(EOL.as_bytes());
        }
    }
    if n == 0 {
        print!("  （なし）{EOL}"); // :34
    }
}

/// atoi (midisend.cpp:47) — C99-shaped: leading ws, sign, digits only,
/// stop at first non-digit; wrapping accumulation (UB range saturates in
/// C++ GCC the same loose way; never reachable with sane --port values).
fn c_atoi(s: &str) -> i32 {
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() && (b[i] == b' ' || (b[i]..=b'\r').contains(&b[i])) {
        i += 1; // isspace family
    }
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let mut v: i32 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        v = v.wrapping_mul(10).wrapping_add((b[i] - b'0') as i32);
        i += 1;
    }
    if neg {
        -v
    } else {
        v
    }
}

fn main() -> std::process::ExitCode {
    // origin: midisend.cpp:40-49 — argv scan. --list anywhere short-circuits;
    // --port takes the NEXT arg only when one exists, else "--port" itself
    // becomes the path (:47 fallthrough); first other arg = path (:48).
    let mut path = String::new(); // :42
    let mut port: i32 = 0; // :43

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0usize;
    while i < argv.len() {
        let a = argv[i].as_str();
        if a == "--list" {
            list_outputs(); // :46
            return std::process::ExitCode::SUCCESS;
        } else if a == "--port" && i + 1 < argv.len() {
            port = c_atoi(&argv[i + 1]); // :47 atoi(argv[++i])
            i += 1;
        } else if path.is_empty() {
            path = a.to_string(); // :48
        }
        i += 1;
    }
    if path.is_empty() {
        // :50-54 fprintf(stderr, 2-segment literal) — NOTE: a Rust trailing-`\`
        // continuation would eat the 8-space indent that C++ string-literal
        // concatenation keeps (found by usage byte-diff); keep one literal.
        eprint!("使い方: midisend <MIDI ファイル> [--port 番号]{EOL}        midisend --list{EOL}");
        return std::process::ExitCode::FAILURE;
    }

    // origin: :56-61 smf::load — err printed bare + exit 1
    let mut events: Vec<Event> = Vec::new();
    let mut err = String::new();
    if !smu_smf::load(&path, &mut events, &mut err) {
        eprint!("{err}{EOL}"); // :59
        return std::process::ExitCode::FAILURE;
    }
    // :62-63 printf("%zu イベント、最後は %.2f 秒\n", size, back().time or 0)
    print!(
        "{} イベント、最後は {:.2} 秒{EOL}",
        events.len(),
        events.last().map_or(0.0, |e| e.time)
    );

    // origin: :65-69 midiOutOpen(CALLBACK_NULL) — UINT(port): a negative
    // --port wraps into UINT exactly like the C++ cast, and fails to open
    let mut out: *mut std::ffi::c_void = std::ptr::null_mut();
    let r = unsafe { mo::midiOutOpen(&mut out, port as u32, 0, 0, mo::CALLBACK_NULL) }; // :66
    if r != mo::MMSYSERR_NOERROR {
        eprint!("MIDI 出力 {port} を開けない{EOL}"); // :67 (%d = signed port)
        return std::process::ExitCode::FAILURE;
    }
    // origin: :70-72 caps for the name line; result IGNORED on disk —
    // zeroed caps print an empty name on failure, same here
    let mut caps: mo::MidiOutCapsA = unsafe { std::mem::zeroed() }; // :70
    unsafe {
        mo::midiOutGetDevCapsA(
            port as u32,
            &mut caps,
            std::mem::size_of::<mo::MidiOutCapsA>() as u32,
        )
    }; // :71
    {
        // printf("送り先: %d: %s\n") — raw A bytes (:72)
        let so = std::io::stdout();
        let mut so = so.lock();
        let _ = so.write_all(format!("送り先: {port}: ").as_bytes());
        let _ = so.write_all(&mo::pname_bytes(&caps));
        let _ = so.write_all(EOL.as_bytes());
    }

    // origin: :74-75 1 ms timer granularity, then the millisecond clock
    unsafe { mo::timeBeginPeriod(1) }; // :74
    let start = unsafe { mo::timeGetTime() }; // :75

    // origin: :77-78 scratch sysex buffer + ONE MIDIHDR reused per sysex
    // (disk declares both with their default-init here; the sysex branch
    // re-inits both before first use, hence the allow)
    #[allow(unused_assignments)]
    let mut sysex: Vec<u8> = Vec::new();
    #[allow(unused_assignments)]
    let mut hdr: mo::MidiOutHdr = unsafe { std::mem::zeroed() };

    // origin: :80-109 event loop
    for e in &events {
        // :82-88 予定の時刻まで待つ — poll the wrapped DWORD clock; sleep
        // floor((left-0.003)*1000) ms above 5 ms, else Sleep(0) spin.
        loop {
            // (timeGetTime() - start) is DWORD math = wrapping mod 2^32
            let now =
                unsafe { mo::timeGetTime() }.wrapping_sub(start) as f64 / 1000.0; // :83
            if now >= e.time {
                break; // :84-85
            }
            let left = e.time - now; // :86
            let ms = if left > 0.005 {
                ((left - 0.003) * 1000.0) as u32 // :87 DWORD((left-0.003)*1000.0)
            } else {
                0
            };
            sys::sleep_ms(ms); // :87 Sleep
        }

        if e.bytes.is_empty() {
            continue; // :90-91
        }

        if e.bytes[0] == 0xf0 {
            // origin: :93-102 システムエクスクルーシブ — copy, prepare, send,
            // spin on MHDR_DONE with Sleep(0), unprepare. A failed
            // midiOutLongMsg without DONE spins forever on disk too
            // (faithful — no extra error arm).
            sysex = e.bytes.clone(); // :94
            hdr = unsafe { std::mem::zeroed() }; // :95 hdr = {}
            unsafe {
                std::ptr::addr_of_mut!(hdr.lp_data).write_unaligned(sysex.as_mut_ptr()); // :96
                std::ptr::addr_of_mut!(hdr.dw_buffer_length)
                    .write_unaligned(sysex.len() as u32); // :97
                mo::midiOutPrepareHeader(out, &mut hdr, std::mem::size_of::<mo::MidiOutHdr>() as u32); // :98
                mo::midiOutLongMsg(out, &mut hdr, std::mem::size_of::<mo::MidiOutHdr>() as u32); // :99
            }
            while !unsafe { mo::hdr_done(&hdr) } {
                sys::sleep_ms(0); // :100-101
            }
            unsafe {
                mo::midiOutUnprepareHeader(out, &mut hdr, std::mem::size_of::<mo::MidiOutHdr>() as u32); // :102
            }
        } else {
            // origin: :103-108 short message: b0|b1<<8|b2<<16 (midi_out::short_msg)
            unsafe { mo::midiOutShortMsg(out, mo::short_msg(&e.bytes)) }; // :107
        }
    }

    // origin: :111-116 tail — 500 ms drain, undo timer period, reset+close
    sys::sleep_ms(500); // :111
    unsafe { mo::timeEndPeriod(1) }; // :112
    unsafe { mo::midiOutReset(out) }; // :113
    unsafe { mo::midiOutClose(out) }; // :114
    print!("送信終了{EOL}"); // :115
    std::process::ExitCode::SUCCESS
}
