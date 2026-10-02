//! M5 `state io` gate — `src/state.h:27-171` byte-level behaviour.

use super::*;

fn hex(v: &[u8]) -> String {
    v.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------- state_pack / state_unpack ----------------

#[test]
fn pack_plain_bytes() {
    let inp = [0x41u8, 0x42, 0x43, 0xff];
    let p = state_pack(&inp);
    assert_eq!(p, inp.to_vec()); // no run ≥4, no zero → verbatim
    assert_eq!(state_unpack(&p).unwrap(), inp.to_vec());
}

#[test]
fn pack_zero_run_boundaries() {
    // run≥4 → 00 (run-4) 00; run caps at 255 (`run < 255` guard :131)
    let cases: [(usize, &str); 5] = [
        (4, "000000"),                       // first RLE form, cnt=0
        (5, "000100"),
        (254, "00fa00"),
        (255, "00fb00"),                     // cap: 255-4 = 0xfb
        (256, concat!("00fb00", "00fc00")), // 255 RLE + lone-zero escape
    ];
    for (n, want) in cases {
        let inp = vec![0u8; n];
        let p = state_pack(&inp);
        assert_eq!(hex(&p), want, "pack {} zeros", n);
        assert_eq!(state_unpack(&p).unwrap(), inp, "unpack {} zeros", n);
    }
}

#[test]
fn pack_nonzero_run() {
    let inp = [0x41u8; 5];
    let p = state_pack(&inp);
    assert_eq!(hex(&p), "000141"); // RLE is not zero-only (value field :136)
    assert_eq!(state_unpack(&p).unwrap(), inp.to_vec());
}

#[test]
fn pack_short_zero_runs_escape() {
    // runs of 1-3 zeros → one 00 fc 00 escape each (:138-142)
    for n in 1..=3 {
        let inp = vec![0u8; n];
        let p = state_pack(&inp);
        assert_eq!(hex(&p), "00fc00".repeat(n), "pack {} zeros", n);
        assert_eq!(state_unpack(&p).unwrap(), inp);
    }
    let inp = [0xffu8, 0x00, 0x00, 0xff];
    let p = state_pack(&inp);
    assert_eq!(hex(&p), "ff00fc0000fc00ff");
    assert_eq!(state_unpack(&p).unwrap(), inp.to_vec());
}

#[test]
fn unpack_truncated_escape_fails() {
    assert_eq!(state_unpack(&[0x41, 0x00]), Err(())); // 00 + 1 byte
    assert_eq!(state_unpack(&[0x00, 0x02]), Err(())); // 00 + 2 bytes
    assert_eq!(state_unpack(&[0x00]), Err(())); // 00 alone
    assert_eq!(state_unpack(&[]), Ok(vec![]));
    // prefix is emitted before the failure point, C++ clears out then bails;
    // Rust returns Err and the caller discards — only Err matters here.
}

#[test]
fn pack_roundtrip_lcg_buffers() {
    // MAME-style LCG bytes with natural short runs + forced long runs
    let mut seed = 0x1234_5678u32;
    let mut buf = Vec::new();
    for i in 0..2048 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        buf.push((seed >> 16) as u8);
        if i % 128 == 0 {
            buf.extend_from_slice(&[0u8; 300]);
            buf.extend_from_slice(&[0x5au8; 9]);
        }
    }
    let p = state_pack(&buf);
    assert!(p.len() < buf.len() / 2, "RLE must shrink the zeroes");
    assert_eq!(state_unpack(&p).unwrap(), buf);
}

// ---------------- StateIo ----------------

#[test]
fn scalar_roundtrip_all_widths() {
    let mut buf = Vec::new();
    {
        let mut w = StateIo::writer(&mut buf);
        assert!(w.writing());
        let mut x: u8 = 0xfe;
        w.v(&mut x);
        let mut x: i8 = -3;
        w.v(&mut x);
        let mut x: u16 = 0xbeef;
        w.v(&mut x);
        let mut x: i16 = -300;
        w.v(&mut x);
        let mut x: u32 = 0xdead_beef;
        w.v(&mut x);
        let mut x: i32 = -70_000;
        w.v(&mut x);
        let mut x: u64 = 0x0123_4567_89ab_cdef;
        w.v(&mut x);
        let mut x: i64 = -1;
        w.v(&mut x);
        let mut x: f32 = -1.5;
        w.v(&mut x);
        let mut x: f64 = 1e10;
        w.v(&mut x);
        let mut x: bool = true;
        w.v(&mut x);
        assert!(w.ok());
    }
    assert_eq!(buf.len(), 1 + 1 + 2 + 2 + 4 + 4 + 8 + 8 + 4 + 8 + 1);
    assert_eq!(hex(&buf[..2]), "fefd"); // LE confirmed
    let mut r = StateIo::reader(&buf);
    assert!(!r.writing());
    let mut x: u8 = 0;
    r.v(&mut x);
    assert_eq!(x, 0xfe);
    let mut x: i8 = 0;
    r.v(&mut x);
    assert_eq!(x, -3);
    let mut x: u16 = 0;
    r.v(&mut x);
    assert_eq!(x, 0xbeef);
    let mut x: i16 = 0;
    r.v(&mut x);
    assert_eq!(x, -300);
    let mut x: u32 = 0;
    r.v(&mut x);
    assert_eq!(x, 0xdead_beef);
    let mut x: i32 = 0;
    r.v(&mut x);
    assert_eq!(x, -70_000);
    let mut x: u64 = 0;
    r.v(&mut x);
    assert_eq!(x, 0x0123_4567_89ab_cdef);
    let mut x: i64 = 0;
    r.v(&mut x);
    assert_eq!(x, -1);
    let mut x: f32 = 0.0;
    r.v(&mut x);
    assert_eq!(x, -1.5f32);
    let mut x: f64 = 0.0;
    r.v(&mut x);
    assert_eq!(x, 1e10f64);
    let mut x: bool = false;
    r.v(&mut x);
    assert!(x);
    assert!(r.ok());
    // bool travels as u8 0x01
    assert_eq!(buf[42], 0x01);
}

#[test]
fn tag_and_arr_and_mem_roundtrip() {
    let mut buf = Vec::new();
    {
        let mut w = StateIo::writer(&mut buf);
        w.set_version(3);
        assert_eq!(w.version(), 3);
        w.tag("mach");
        let mut a = [1u16, 2, 3, 0x0405];
        w.arr(&mut a);
        let mut m = [7u8, 8, 9];
        w.mem(&mut m);
    }
    // 8-byte NUL-padded tag, 7-char truncation
    assert_eq!(hex(&buf[..8]), "6d61636800000000");
    let mut r = StateIo::reader(&buf);
    r.set_version(3);
    r.tag("mach");
    assert!(r.ok(), "{}", r.error());
    let mut a = [0u16; 4];
    r.arr(&mut a);
    assert_eq!(a, [1u16, 2, 3, 0x0405]);
    let mut m = [0u8; 3];
    r.mem(&mut m);
    assert_eq!(m, [7u8, 8, 9]);
    assert!(r.ok());
    assert_eq!(r.version(), 3);

    // long name truncates to 7 bytes on both sides → still matches
    let mut buf2 = Vec::new();
    StateIo::writer(&mut buf2).tag("0123456789abcdef");
    assert_eq!(buf2, b"0123456\0".to_vec()); // :87 strncpy caps at 7 bytes
    let mut r = StateIo::reader(&buf2);
    r.tag("01234567zzzz");
    assert!(r.ok(), "{}", r.error());
}

#[test]
fn tag_mismatch_exact_error() {
    let mut buf = Vec::new();
    StateIo::writer(&mut buf).tag("mach");
    let mut r = StateIo::reader(&buf);
    r.tag("booth");
    assert!(!r.ok());
    // C++ :93 snprintf format — exact bytes, naming the EXPECTED tag
    assert_eq!(r.error(), "目印が違う（booth のところ）");
    assert_eq!(
        r.error().as_bytes(),
        "目印が違う（booth のところ）".as_bytes()
    );
}

#[test]
fn short_read_fails_sticky_first_error_wins() {
    let mut buf = Vec::new();
    {
        let mut w = StateIo::writer(&mut buf);
        w.tag("mach");
        let mut x: u32 = 9;
        w.v(&mut x);
    }
    let short = buf[..10].to_vec(); // tag fits, v misses 2 bytes
    let mut r = StateIo::reader(&short);
    r.tag("mach");
    assert!(r.ok());
    let mut x: u32 = 0xa5a5_a5a5;
    r.v(&mut x);
    assert!(!r.ok());
    assert_eq!(r.error(), "足りない");
    assert_eq!(x, 0xa5a5_a5a5); // failed read leaves the value untouched
    // sticky: everything after !ok is a no-op, first error kept
    let mut y: u32 = 0;
    r.v(&mut y);
    assert_eq!(y, 0);
    r.tag("mach"); // would also mismatch bytes… but ok() already false
    assert_eq!(r.error(), "足りない");
    let mut z = [0u8; 4];
    r.mem(&mut z);
    assert_eq!(z, [0u8; 4]);
    assert!(!r.ok());
}

#[test]
fn arr_bounds_check_is_all_or_nothing() {
    // C++ single raw() call checks the FULL byte count (state.h:51)
    let mut buf = Vec::new();
    let mut a = [1u32, 2, 3];
    StateIo::writer(&mut buf).arr(&mut a);
    let short = buf[..11].to_vec(); // 12 needed
    let mut r = StateIo::reader(&short);
    let mut got = [0u32; 3];
    r.arr(&mut got);
    assert!(!r.ok());
    assert_eq!(r.error(), "足りない");
    assert_eq!(got, [0u32; 3]);
    // at did not advance past the failure gate (still 0: whole call rejected)
    assert_eq!(r.at, 0);
}
