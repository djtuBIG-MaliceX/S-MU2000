// license:BSD-3-Clause
//
// S4/W-XG row: offline vectors for the firmware-XG param layer (no ROM).
// Round-trips for encode/decode, checksum, pin window, locate/sysfx_addr
// tables, fx_find fallback, wide-byte rebuild, and the setup_messages dump
// shape. Cites are the C++ ground truth: src/xg/model.cpp / ram.h /
// sysfx.h / xg_state.h / fx_params.h.

use smu_machine::xg::{fx, model, ram, state, sysfx};

fn ask_model_sysex(p: &model::Param, part: i32, v: i32) -> model::Model {
    // rebuild the param_change as a feed()'d reply (param change == the
    // firmware answering our ask, model.cpp:305-308)
    let mut m = model::Model::new();
    let msg = model::param_change(p, part, v);
    // param_change builds F0 43 10 4C .. F7 already — feed it whole
    for b in msg {
        m.feed(b);
    }
    m
}

#[test]
fn pack_and_address_roundtrip() {
    // origin: model.h:69 pack; model.cpp:203-206 address (part digit wins)
    assert_eq!(model::pack(0x08, 19, 0x0b), (8 << 14) | (19 << 7) | 0x0b);
    let vol = model::find("part.volume").unwrap();
    assert_eq!(model::address(vol, 19), model::pack(0x08, 19, 0x0b)); // :205
    let tune = model::find("system.master_tune").unwrap();
    assert_eq!(model::address(tune, 0), model::pack(0x00, 0x00, 0x00)); // mid kept (not part)
    let hp = model::find("part.hpf_cutoff").unwrap();
    assert_eq!(model::address(hp, 31), model::pack(0x0a, 31, 0x20)); // hi IS 0A (:164)
    assert_eq!(model::part_dump_request(7)[4..7], [0x08, 7, 0x00]); // model.h:82
    // find miss (model.cpp:200)
    assert!(model::find("nope.nope").is_none());
}

#[test]
fn param_change_encodes_and_model_decodes() {
    // nibble 4-byte master tune 0x417 == four 4-bit digits, high first (:185)
    let tune = model::find("system.master_tune").unwrap();
    let m = ask_model_sysex(tune, 0, 0x417);
    assert_eq!(m.get(tune, 0), Some(0x417)); // :363 fold nibble
    // nibble 2-byte detune 0xa5
    let det = model::find("part.detune").unwrap();
    let msg = model::param_change(det, 18, 0xa5);
    assert_eq!(&msg[4..7], &[0x08, 18, 0x09]); // address (nibble bytes at 09-0A)
    let m2 = ask_model_sysex(det, 18, 0xa5);
    assert_eq!(m2.get(det, 18), Some(0xa5));
    // byte7 round trips incl. 2-byte MSB-first. HALL2 = type 01/01 = the
    // bytes 01 01 = value (1<<7|1) = 0x81, NOT 0x0101 (xgtest set line
    // 02 01 00 01 01); a value of 0x101 would encode to [0x02, 0x01].
    let rev = model::find("reverb.type").unwrap();
    let m3 = ask_model_sysex(rev, 0, 0x0101);
    assert_eq!(m3.get(rev, 0), Some(0x0101));
    assert_eq!(model::param_change(rev, 0, 0x0101), vec![0xf0, 0x43, 0x10, 0x4c, 0x02, 0x01, 0x00, 0x02, 0x01, 0xf7]); // :246-255
    let m3b = ask_model_sysex(rev, 0, 0x81);
    assert_eq!(m3b.get(rev, 0), Some(0x81));
    assert_eq!(model::param_change(rev, 0, 0x81), vec![0xf0, 0x43, 0x10, 0x4c, 0x02, 0x01, 0x00, 0x01, 0x01, 0xf7]);
    // high-bit mid-SysEx is DROPPED by design (:283-287 "another message
    // started; discard") — firmware never emits one (data is 7-bit), so the
    // ask simply stays unanswered (model.h:93 no hardcoded initial value)
    let mut m4 = model::Model::new();
    for b in [0xf0u8, 0x43, 0x10, 0x4c, 0x08, 0x00, 0x0b, 0xff, 0xf7] {
        m4.feed(b);
    }
    let vol = model::find("part.volume").unwrap();
    assert_eq!(m4.get(vol, 0), None); // 0xff never even reaches the message
    assert_eq!(m4.rejected(), 0); // :273 realtime guard swallows it silently
    // but the fold still masks any 7-bit garbage it does accept (:363)
    let mut m5 = model::Model::new();
    for b in [0xf0u8, 0x43, 0x10, 0x4c, 0x08, 0x00, 0x0b, 0x77, 0xf7] {
        m5.feed(b);
    }
    assert_eq!(m5.get(vol, 0), Some(0x77));
    // a high-bit byte BELOW 0xf8 mid-message: rejected++, message dropped
    // (:283-287 — the only path that bumps rejected inside feed)
    let mut m6 = model::Model::new();
    for b in [0xf0u8, 0x43, 0x10, 0x4c, 0x08, 0x00, 0x0b, 0x80, 0x10, 0xf7] {
        m6.feed(b);
    }
    assert_eq!(m6.get(vol, 0), None);
    assert_eq!(m6.rejected(), 1);
    assert_eq!(m6.accepted(), 0); // F7 arrives with m_in==false (:281-282)
    // valid + special (model.cpp:208-211)
    let vpart = model::find("variation.part").unwrap();
    assert!(model::valid(vpart, 127)); // special OFF
    assert!(model::valid(vpart, 0));
    assert!(!model::valid(vpart, 66));
}

#[test]
fn format_views_match_cpp_sprintf() {
    // %+d folds to Rust {:+} "+0" for center-zero (model.cpp:220-221)
    let ns = model::find("part.note_shift").unwrap();
    assert_eq!(model::format(ns, 0x40), "+0");
    assert_eq!(model::format(ns, 0x43), "+3");
    assert_eq!(model::format(ns, 0x28), "-24");
    // pan (model.cpp:223-228)
    let pan = model::find("part.pan").unwrap();
    assert_eq!(model::format(pan, 0), "Rnd");
    assert_eq!(model::format(pan, 64), "C");
    assert_eq!(model::format(pan, 63), "L1");
    assert_eq!(model::format(pan, 127), "R63");
    // plus1 (model.cpp:217-219) — 0x30 shows as 49 (the xgtest comment)
    let prog = model::find("part.program").unwrap();
    assert_eq!(model::format(prog, 0x30), "49");
    // choice / part_off (model.cpp:229-237)
    let mode = model::find("part.mode").unwrap();
    assert_eq!(model::format(mode, 4), "DRUMS3"); // index value-min (:231)
    let vp = model::find("variation.part").unwrap();
    assert_eq!(model::format(vp, 127), "OFF"); // :235
    assert_eq!(model::format(vp, 0), "Part 1"); // :236
}

#[test]
fn dump_checksum_accept_and_reject() {
    // bulk dump shape model.cpp:309-323. Address 02 01 0c, one byte 0x33.
    let mut msg: Vec<u8> = vec![0xf0, 0x43, 0x00, 0x4c, 0x00, 1, 0x02, 0x01, 0x0c, 0x33];
    let mut sum: u32 = 0;
    for b in &msg[4..] {
        sum += *b as u32; // :319 covers count+addr+data (:146 setup side)
    }
    msg.push(((0x80 - (sum & 0x7f)) & 0x7f) as u8);
    msg.push(0xf7);
    let mut m = model::Model::new();
    for b in msg.clone() {
        m.feed(b);
    }
    let ret = model::find("reverb.return").unwrap();
    assert_eq!(m.get(ret, 0), Some(0x33));
    assert_eq!(m.accepted(), 1);
    assert_eq!(m.rejected(), 0);
    // corrupt checksum -> rejected, value NOT stored (:319-321)
    let mut bad = msg.clone();
    let n = bad.len();
    bad[n - 2] ^= 0x01;
    let mut m2 = model::Model::new();
    for b in bad {
        m2.feed(b);
    }
    assert_eq!(m2.get(ret, 0), None); // model.h:93 no hardcoded initial value
    assert_eq!(m2.rejected(), 1);
    assert_eq!(m2.accepted(), 0);
}

#[test]
fn pin_window_protects_written_value() {
    // origin: model.cpp:402-425 set + :330-353 store PIN_MS (model.h:146)
    let pan = model::find("part.pan").unwrap();
    let mut m = model::Model::new();
    let out = m.set(pan, 0, 20);
    assert_eq!(out[0..7], [0xf0, 0x43, 0x10, 0x4c, 0x08, 0x00, 0x0e]); // :411
    assert_eq!(m.get(pan, 0), Some(20)); // :408 write-through
    // a reply arriving <500ms later must NOT overwrite (:342-343)
    m.load(model::pack(0x08, 0, 0x0e), &[64], 100); // :114-121 load(now=100)
    assert_eq!(m.get(pan, 0), Some(20)); // still pinned
    // past the window the reply wins (:344 then :347)
    m.load(model::pack(0x08, 0, 0x0e), &[64], 700);
    assert_eq!(m.get(pan, 0), Some(64));
    // bank set chains the mirror program (model.cpp:414-423)
    let msb = model::find("part.bank_msb").unwrap();
    let mut m2 = model::Model::new();
    let out = m2.set(msb, 0, 64);
    assert_eq!(out.len(), 9); // 7 head + 1 data + F7, bank only (no program yet)
    assert_eq!(out[0..7], [0xf0, 0x43, 0x10, 0x4c, 0x08, 0x00, 0x01]);
    let prog = model::find("part.program").unwrap();
    let mut m3 = ask_model_sysex(prog, 0, 5); // program now in the mirror
    let out = m3.set(msb, 0, 64);
    assert_eq!(out.len(), 9 + 9); // bank change + chained program change
    assert_eq!(&out[9..16], &[0xf0, 0x43, 0x10, 0x4c, 0x08, 0x00, 0x03]);
}

#[test]
fn applies_on_program_and_encode_edges() {
    // model.cpp:397-400 — only part bank_msb/lsb
    for key in ["part.bank_msb", "part.bank_lsb"] {
        assert!(model::applies_on_program(model::find(key).unwrap()));
    }
    for key in ["part.program", "part.volume", "system.master_tune"] {
        assert!(!model::applies_on_program(model::find(key).unwrap()));
    }
    // encode shifts: 2-byte 7-bit MSB-first (reverb.type 0x2080 chorus default)
    let cho = model::find("chorus.type").unwrap();
    let m = ask_model_sysex(cho, 0, 0x2080);
    assert_eq!(m.get(cho, 0), Some(0x2080));
}

#[test]
fn ram_table_vectors() {
    // origin: ram.h:195-225 locate; vectors from the probing comments
    // (ram.h:24-33, :56-58, :121, :162-177)
    assert_eq!(ram::locate(0x00_00_04), Some(ram::SYSTEM + 4)); // :47 master volume
    assert_eq!(ram::locate(0x00_00_06), Some(ram::SYS_TRANSPOSE)); // :53
    // part 10 (XG index 9) is the port head slot 0 (ram.h:98-108)
    assert_eq!(ram::locate(model::pack(0x08, 9, 0x00)), Some(ram::PARTS));
    // part 0 is slot 1 -> +0x134
    assert_eq!(
        ram::locate(model::pack(0x08, 0, 0x0b)),
        Some(ram::PARTS + 0x134 + 0x0b)
    );
    // part EQ 72 = +0x6a (ram.h:129-130), part 16 -> port B head
    assert_eq!(
        ram::locate(model::pack(0x08, 16, 0x72)),
        Some(ram::part_base(16) + 0x6a)
    );
    // port B, k=0 -> slot k+1=1 -> (16+1)==17th stride from PARTS head
    assert_eq!(ram::part_base(16), ram::PARTS + 17 * 0x134); // ram.h:107-109
    // HPF: hi 0A = part_base + 0x78 (ram.h:125-128)
    assert_eq!(
        ram::locate(model::pack(0x0a, 9, 0x20)),
        Some(ram::part_base(9) + 0x78)
    );
    // ext 41 = +0x3a (ram.h:119-124)
    assert_eq!(
        ram::locate(model::pack(0x08, 25, 0x41)),
        Some(ram::part_base(25) + 0x3a)
    );
    // effects: reverb head, packed 10-15 block, insertion 3, master EQ
    assert_eq!(ram::locate(model::pack(0x02, 0x01, 0x00)), Some(0x0cad8));
    assert_eq!(ram::locate(model::pack(0x02, 0x01, 0x10)), Some(0x0cae6)); // :161
    assert_eq!(ram::locate(model::pack(0x03, 0x02, 0x0c)), Some(0x0cbd6 + 0x0c)); // :174
    assert_eq!(ram::locate(model::pack(0x02, 0x40, 0x00)), Some(0x0cc2e)); // :178
    // misses: part 32 (RAM table is 0-31), unknown hi, lo past a block
    assert_eq!(ram::locate(model::pack(0x08, 32, 0x00)), None); // mid<32 gate
    assert_eq!(ram::locate(model::pack(0x01, 0x00, 0x00)), None);
    assert_eq!(ram::locate(model::pack(0x02, 0x01, 0x0e)), None); // :219 lo+b.size
    // drum setup math (ram.h:56-58 vectors: 0x30 24 00 -> 4228F2)
    assert_eq!(ram::drum_setup(0, 0x24, 0), 0x4228f2 - 0x400000);
    assert_eq!(ram::drum_setup(0, 0x25, 2), 0x42290b - 0x400000); // +23
    assert_eq!(ram::drum_setup(1, 0x24, 2), 0x42300d - 0x400000); // +1817=23*79
    assert_eq!(ram::drum_setup_index(0x24), 18); // :80
    assert_eq!(ram::drum_setup_index(0x61), 22); // :84
    assert_eq!(ram::drum_setup_index(0x23), -1); // :85
    assert_eq!(ram::drum_setup_default(18), 0x0c); // :91
    assert_eq!(ram::drum_setup_default(0), 64);
}

#[test]
fn sysfx_addr_vectors() {
    // origin: sysfx.h:35-64 (ranges probed by sysfx_check.cpp)
    let mut sz: i32 = 0;
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Reverb, (0x02, 1), &mut sz), 0x02); // p1, sz1
    assert_eq!(sz, 1);
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Reverb, (0x03, 1), &mut sz), 0x03); // p2
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Reverb, (0x0b, 1), &mut sz), -1); // p10 Dry/Wet none
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Reverb, (0x20, 1), &mut sz), 0x10); // p11 -> 02 01 10
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Reverb, (0x25, 1), &mut sz), 0x15); // p16
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Chorus, (0x02, 1), &mut sz), 0x22); // p1 -> 02 01 22
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Chorus, (0x09, 1), &mut sz), 0x29); // p9
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Chorus, (0x0a, 1), &mut sz), 0x2a); // p10 0x2A... 
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Chorus, (0x0b, 1), &mut sz), -1); // wait: p addr 0x0b -> n=10 -> rejected
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Chorus, (0x20, 1), &mut sz), -1); // 11-16 rejected
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Variation, (0x30, 2), &mut sz), 0x42); // p1 2-byte
    assert_eq!(sz, 2);
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Variation, (0x42, 2), &mut sz), 0x54); // p10 (0x42/0x43 share the pair)
    // variation has NO size gate on the 1-10 window (sysfx.h:55-61): even a
    // 1-byte insertion row remaps to the 2-byte variation address
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Variation, (0x02, 1), &mut sz), 0x42);
    assert_eq!(sz, 2);
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Variation, (0x25, 1), &mut sz), 0x75); // p16 -> 02 01 75
    // variation p10 (0x43 wide) -> 0x42+18 = 0x54
    assert_eq!(sysfx::sysfx_addr(sysfx::Sysfx::Variation, (0x43, 2), &mut sz), 0x54);
}

#[test]
fn fx_find_table_and_fallback() {
    // origin: fx_params.h:1687-1697 — exact hit first, LSB-0 fallback
    let d = fx::fx_find((0x05 << 7) | 0x00).unwrap(); // DELAY LCR
    assert_eq!((d.msb, d.lsb), (0x05, 0x00));
    assert_eq!(d.params[0], (0x30, 2)); // :730
    assert_eq!(d.params.len(), 12); // count from FX_DEFS :1582
    let d = fx::fx_find((0x01 << 7) | 0x06).unwrap(); // explicit lsb!=0 row :1570
    assert_eq!((d.msb, d.lsb), (0x01, 0x06));
    let d = fx::fx_find((0x05 << 7) | 0x07).unwrap(); // no 05/07 -> lsb0 fallback
    assert_eq!((d.msb, d.lsb), (0x05, 0x00));
    assert!(fx::fx_find(0x7f).is_none()); // msb 0 unknown -> None :1696
    // table count sanity == FX_DEFS rows from the header (116, :1567-1684)
    assert_eq!(fx::fx_def_count(), 116);
}

#[test]
fn wide_bytes_and_snapshot_shape() {
    // origin: xg_state.h:29-48 — 16-bit -> 7-bit pairs; 1234 == 0x04D2
    let mut snap = state::XgSnapshot::new();
    let at = (ram::INS_BLOCK[2] - ram::EFFECT + ram::INS_WIDE) as usize;
    snap.effect[at] = 0x04; // 1234 >> 8
    snap.effect[at + 1] = 0xd2; // 1234 & 0xff
    let mut bytes = [0u8; 20];
    state::ins_wide_bytes(&snap, 2, &mut bytes);
    assert_eq!(&bytes[0..2], &[9, 82]); // 1234>>7 = 9, 1234&0x7f = 82
    let va = (ram::VAR_BLOCK - ram::EFFECT + ram::VAR_WIDE) as usize;
    snap.effect[va] = 0x01;
    snap.effect[va + 1] = 0x02;
    state::var_wide_bytes(&snap, &mut bytes);
    assert_eq!(&bytes[0..2], &[2, 2]); // 258 -> 2,2
    // ins_is_wide: DELAY LCR (05 00) has size-2 params (xg_state.h:52-62)
    let mut s2 = state::XgSnapshot::new();
    let b = (ram::INS_BLOCK[0] - ram::EFFECT) as usize;
    s2.effect[b] = 0x05; // type 05/00 via (msb<<7|lsb): 0x05<<7
    s2.effect[b + 1] = 0x00;
    // the type read is blk[0]<<7|blk[1]; msb 05 needs blk[0]=0x05
    assert!(state::ins_is_wide(&s2, 0));
    // HALL-like 01 00: all 1-byte params -> not wide
    s2.effect[b] = 0x01;
    assert!(!state::ins_is_wide(&s2, 0));
    // unknown type -> false (:56-58)
    s2.effect[b] = 0x7f;
    assert!(!state::ins_is_wide(&s2, 0));
}

#[test]
fn setup_messages_dump_shape_and_checksum() {
    // origin: xg_state.h:139-158 — every bulk must end with a checksum whose
    // running sum (count..sum) has low 7 bits zero, exactly what
    // model::on_sysex checks (model.cpp:318-321): the dump must round-trip
    // through the mirror without a reject.
    let snap = state::XgSnapshot::new(); // booted-factory-like zeros
    let msgs = state::setup_messages(&snap);
    let mut n_bulk = 0;
    let mut i = 0;
    while i < msgs.len() {
        assert_eq!(msgs[i], 0xf0);
        let kind = msgs[i + 2];
        if kind == 0x00 {
            n_bulk += 1;
            let count = ((msgs[i + 4] as usize) << 7) | msgs[i + 5] as usize;
            let end = i + 9 + count + 2; // ..sum F7
            let mut sum: u32 = 0;
            for b in &msgs[i + 4..end - 1] {
                sum += *b as u32;
            }
            assert_eq!(
                sum & 0x7f,
                0,
                "bulk at {i} bad checksum: {:02x?}",
                &msgs[i..(i + 24).min(msgs.len())]
            );
            assert_eq!(msgs[end - 1], 0xf7);
            i = end;
        } else {
            let e = msgs[i..].iter().position(|b| *b == 0xf7).unwrap() + i;
            i = e + 1;
        }
    }
    // system(1) + EFFECTS-loop bulks (14: every block head except the
    // variation 40/42-41 head, whose size==head skips the bulk) + variation
    // 56/70 heads ride inside that 14) + ins-wide(4) + var-wide(1)
    // + 64 part-XG + 64 part-ext  ==  1 + 14 + 4 + 1 + 64 + 64
    assert_eq!(n_bulk, 148);
    // feed the whole stream into a fresh mirror: zero rejects (all bulks
    // checksum-green, changes ride the <8192 window)
    let mut m = model::Model::new();
    for b in msgs {
        m.feed(b);
    }
    assert_eq!(m.rejected(), 0);
    let tune = model::find("system.master_tune").unwrap();
    assert_eq!(m.get(tune, 0), Some(0)); // system zeroed and read back
    let prog = model::find("part.program").unwrap();
    assert_eq!(m.get(prog, 40), Some(0)); // part 41 in the mirror
}

#[test]
fn setup_diff_moves_bank_program_as_triple() {
    // origin: xg_state.h:277-279 — any of bank/program differing -> MSB, LSB,
    // PC all three, in order
    let mut a = state::XgSnapshot::new();
    let b = state::XgSnapshot::new();
    a.parts[9][3] = 0x30; // program differs on part 10
    let msgs = state::setup_diff_messages(&a, &b);
    // count changes at 08 09 (part 10) with lo 1,2,3
    let mut seen = Vec::new();
    let mut i = 0;
    while i < msgs.len() {
        assert_eq!(msgs[i], 0xf0);
        let kind = msgs[i + 2];
        assert_eq!(kind, 0x10); // diff path is single changes only
        let e = msgs[i..].iter().position(|x| *x == 0xf7).unwrap() + i;
        if kind == 0x10 && msgs[i + 4] == 0x08 && msgs[i + 5] == 9 && (1..=3).contains(&msgs[i + 6])
        {
            seen.push(msgs[i + 6]);
        }
        i = e + 1;
    }
    assert_eq!(seen, vec![1, 2, 3]); // MSB, LSB, PC in order, each exactly once
}
