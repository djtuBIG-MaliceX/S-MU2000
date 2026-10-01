//! Cross-checks against C++ ground truth captured from `src/smf.cpp` (cap.exe, 2026-09-30).
//! Dump format is byte-identical to `cap_main.cpp::dump_one` (see %TEMP%\smfcap).

use super::*;
mod testdata {
    include!("testdata.rs");
}

/// SHA-1 (RFC 3174), std-only, for golden comparison.
fn sha1(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let ml = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());
    for blk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([blk[4 * i], blk[4 * i + 1], blk[4 * i + 2], blk[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for i in 0..80 {
            let (f, k) = if i < 20 {
                ((b & c) | ((!b) & d), 0x5A827999u32)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ED9EBA1)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8F1BBCDC)
            } else {
                (b ^ c ^ d, 0xCA62C1D6)
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h = [
            h[0].wrapping_add(a),
            h[1].wrapping_add(b),
            h[2].wrapping_add(c),
            h[3].wrapping_add(d),
            h[4].wrapping_add(e),
        ];
    }
    let mut s = String::new();
    for x in h {
        s += &format!("{x:08x}");
    }
    s
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
fn sha1_selftest() {
    assert_eq!(sha1(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(sha1(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    assert_eq!(fnv1a64(b"abc"), 0xe71fa2190541574b); // ledger-locked FNV vector
}

/// Build the canonical dump for an OK result (cap_main.cpp format).
fn dump_ok(label: &str, evs: &[Event]) -> Vec<u8> {
    let mut s = format!("S {label}\nOK {}\n", evs.len());
    for (i, e) in evs.iter().enumerate() {
        s += &format!("{} {:016x} {}", i, e.time.to_bits(), e.port);
        for b in &e.bytes {
            s += &format!(" {b:02x}");
        }
        s += "\n";
    }
    s.into_bytes()
}

fn dump_err(label: &str, err: &str) -> Vec<u8> {
    format!("S {label}\nERR {err}\n").into_bytes()
}

/// Writes scratch bytes to temp, loads, checks the golden dump SHA1.
fn check_scratch(name: &str, bytes: &[u8], fnv: u64, want_sha1: &str) {
    assert_eq!(fnv1a64(bytes), fnv, "{name}: embedded bytes drifted");
    let path = std::env::temp_dir().join(format!("smu_smf_{}_{}", std::process::id(), name));
    std::fs::write(&path, bytes).unwrap();
    let mut evs: Vec<Event> = Vec::new();
    let mut err = String::new();
    let dump = if load(&path.to_string_lossy(), &mut evs, &mut err) {
        dump_ok(name, &evs)
    } else {
        dump_err(name, &err)
    };
    let got = sha1(&dump);
    if std::env::var("SMU_SMF_DUMP").is_ok() {
        std::fs::write(
            std::env::temp_dir().join(format!("smu_smf_dump_{name}.txt")),
            &dump,
        )
        .unwrap();
    }
    let _ = std::fs::remove_file(&path);
    assert_eq!(got, want_sha1, "{name} dump diverged:\n{}", String::from_utf8_lossy(&dump));
}

#[test]
fn scratch_inputs_match_cpp() {
    let cases: &[(&str, &[u8], u64)] = &[
        ("scr_biglen", testdata::SCR_BIGLEN_MID.1, testdata::SCR_BIGLEN_MID.2),
        ("scr_chunk", testdata::SCR_CHUNK_MID.1, testdata::SCR_CHUNK_MID.2),
        ("scr_div0", testdata::SCR_DIV0_MID.1, testdata::SCR_DIV0_MID.2),
        ("scr_empty", testdata::SCR_EMPTY_MID.1, testdata::SCR_EMPTY_MID.2),
        ("scr_hdrlen", testdata::SCR_HDRLEN_MID.1, testdata::SCR_HDRLEN_MID.2),
        ("scr_latetempo", testdata::SCR_LATETEMPO_MID.1, testdata::SCR_LATETEMPO_MID.2),
        ("scr_names", testdata::SCR_NAMES_MID.1, testdata::SCR_NAMES_MID.2),
        ("scr_nothd", testdata::SCR_NOTHD_MID.1, testdata::SCR_NOTHD_MID.2),
        ("scr_overread", testdata::SCR_OVERREAD_MID.1, testdata::SCR_OVERREAD_MID.2),
        ("scr_smpte", testdata::SCR_SMPTE_MID.1, testdata::SCR_SMPTE_MID.2),
        ("scr_tie", testdata::SCR_TIE_MID.1, testdata::SCR_TIE_MID.2),
        ("scr_tiny", testdata::SCR_TINY_MID.1, testdata::SCR_TINY_MID.2),
        ("scr_trunc", testdata::SCR_TRUNC_MID.1, testdata::SCR_TRUNC_MID.2),
        ("scr_vlq", testdata::SCR_VLQ_MID.1, testdata::SCR_VLQ_MID.2),
    ];
    for (name, bytes, fnv) in cases {
        let want = testdata::SCRATCH_SHA1
            .iter()
            .find(|(l, _)| l == name)
            .unwrap_or_else(|| panic!("no golden for {name}"))
            .1;
        check_scratch(name, bytes, *fnv, want);
    }
}

/// Repo fixtures (tools/make_test_midi.py output). Search $SMU_SMF_FIXTURES,
/// then the repo `--help` artifacts, then build/tests. Skip gracefully if absent.
fn fixture_path(name: &str) -> Option<std::path::PathBuf> {
    let file = format!("{name}.mid");
    let roots: Vec<std::path::PathBuf> = match std::env::var_os("SMU_SMF_FIXTURES") {
        Some(d) => vec![std::path::PathBuf::from(d)],
        None => {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("..");
            vec![repo.join("--help"), repo.join("build").join("tests")]
        }
    };
    roots.into_iter().map(|r| r.join(&file)).find(|p| p.is_file())
}

#[test]
fn repo_fixtures_match_cpp() {
    let mut seen = 0;
    for (label, want) in testdata::FIXTURE_SHA1 {
        let path = match fixture_path(label) {
            Some(p) => p,
            None => continue, // fixture absent on this checkout — skip gracefully
        };
        let _bytes = std::fs::read(&path).unwrap();
        let mut evs: Vec<Event> = Vec::new();
        let mut err = String::new();
        let ok = load(&path.to_string_lossy(), &mut evs, &mut err);
        assert!(ok, "fixture {label} failed to load: {err}");
        let dump = dump_ok(label, &evs);
        assert_eq!(
            sha1(&dump),
            *want,
            "fixture {label} diverged ({} events)",
            evs.len()
        );
        seen += 1;
    }
    if seen == 0 {
        eprintln!("smu-smf: no repo fixtures found — skipped all fixture golden checks");
    }
}

#[test]
fn port_name_vectors_match_cpp() {
    for (name, want) in testdata::NAME_VECTORS {
        // load() passes a raw byte slice (no C-string semantics involved)
        assert_eq!(port_from_track_name(name.as_bytes()), *want, "name {name:?}");
    }
}

#[test]
fn port_from_track_name_nul_and_nonascii() {
    // NUL skip + bare-letter rejection (locked via scr_names t9: "A\0" -> -1)
    assert_eq!(port_from_track_name(b"A\0"), -1);
    assert_eq!(port_from_track_name(b"part\0a"), 0);
    // non-ASCII bytes are never lowered in the C locale
    assert_eq!(port_from_track_name(b"\xc3a"), -1);
    assert_eq!(port_from_track_name(b""), -1);
    assert_eq!(port_from_track_name(b"   "), -1);
    // trailing tab (smf.cpp:20 only trims ' ' and '\t')
    assert_eq!(port_from_track_name(b"PartB\t"), 1);
}

#[test]
fn mu_port_table() {
    // DIN 2-port default (usb=false): A/B pass, C/D fold A/B or mute (smf.h:36-39)
    assert_eq!(mu_port_d(0, false), 0);
    assert_eq!(mu_port_d(1, false), 1);
    assert_eq!(mu_port_d(2, false), -1);
    assert_eq!(mu_port_d(3, false), -1);
    assert_eq!(mu_port_d(0, true), 0);
    assert_eq!(mu_port_d(1, true), 1);
    assert_eq!(mu_port_d(2, true), 0);
    assert_eq!(mu_port_d(3, true), 1);
    assert_eq!(mu_port_d(4, true), 0);
    assert_eq!(mu_port_d(5, true), 1);
    assert_eq!(mu_port_d(255, true), 1);
    // USB 4-port (render.cpp:545 3-arg form)
    assert_eq!(mu_port(3, false, true), 3);
    assert_eq!(mu_port(4, false, true), -1);
    assert_eq!(mu_port(4, true, true), 0);
    assert_eq!(mu_port(7, true, true), 3);
}

#[test]
fn open_error_text_and_append_semantics() {
    // C++ load() appends to `out` (never clears) — mirror that, and pin the
    // Japanese open-failure text with the caller path (smf.cpp:64).
    let missing = std::env::temp_dir().join("smu_smf_definitely_missing_9f3.mid");
    let mut evs: Vec<Event> = vec![Event::new(12.5, vec![0x90, 0x3c, 0x64])];
    let mut err = String::new();
    assert!(!load(missing.to_string_lossy().as_ref(), &mut evs, &mut err));
    assert_eq!(err, format!("MIDI ファイルを開けない: {}", missing.to_string_lossy()));
    assert_eq!(evs.len(), 1); // untouched
    assert_eq!(evs[0].port, 0); // smf.h:27 default
}

#[test]
fn err_strings_are_the_captured_utf8() {
    // error payloads are goldened end-to-end via scr_smpte/scr_nothd/scr_tiny/scr_empty
    // (their dumps carry the exact C++ UTF-8 bytes). Direct re-asserts here:
    let cases = [
        ("scr_smpte", "SMPTE 単位の MIDI には未対応"),
        ("scr_nothd", "MThd がない。標準 MIDI ファイルではないらしい"),
    ];
    for (label, want) in cases {
        let (_name, bytes, fnv) = match label {
            "scr_smpte" => testdata::SCR_SMPTE_MID,
            _ => testdata::SCR_NOTHD_MID,
        };
        assert_eq!(fnv1a64(bytes), fnv);
        let path = std::env::temp_dir().join(format!("smu_smf_{}_{}", std::process::id(), label));
        std::fs::write(&path, bytes).unwrap();
        let mut evs = Vec::new();
        let mut err = String::new();
        assert!(!load(&path.to_string_lossy(), &mut evs, &mut err));
        assert_eq!(err, want);
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn event_struct_defaults_explicit() {
    let e = Event::new(0.0, Vec::new());
    assert_eq!(e.time, 0.0);
    assert!(e.bytes.is_empty());
    assert_eq!(e.port, 0); // smf.h:19-27
}
