// license:BSD-3-Clause
//
// Make the MU2000 firmware re-read the parameter definition table.
//
//   xgtest <rom directory> [-v]
//
// origin: src/xgtest.cpp (586 L) — transliterated verbatim: same sections,
// same order, same stdout strings (CJK byte-verbatim), same counts. For every
// definition-table row, write the range ends and the middle through a
// parameter change, ask the same address back, and compare; also cross-check
// bulk-dump slices against single asks, values changed by non-XG paths
// (program change / CC7 / CC7-on-port-B), the work-RAM address table
// (xg/ram.h), and the XG-only state round-trip (ui/xg_state.h).
//
//   The clock is the audio callback: rig.samples/RATE (no wall clock).
//   MIDI OUT is the only reply path (midi_out_take ring LIVE since W-TX;
//   the USB divert is LIVE since M7 `USB host (M37640)` — ledger rows).
//
// Deviations (disclosed):
// - stdout/stderr CRLF on Windows (MSVCRT text-mode CRT, boot.rs:17-19).
// - --usb: the divert + pump are LIVE (usb.rs, M7). With HOST SELECT USB
//   the firmware answers on the usb_line (mu2000.h:234-236); set_usb_host
//   BEFORE reset (xgtest.cpp:103/:473). The harness gate still runs plain
//   args (this row's parity gate is render --usb; see ledger NEXT).
// - load_sintab return value ignored exactly like disk (:102); a missing
//   sin-table then runs on an empty stand-in on BOTH machines.
// - `static rig h/k` on disk (stack-bypass) = Box<Machine> rigs (Invariant 3
//   explicit ctor, statetest.rs:436-451 precedent).
// - section 6 boot() reloads into FRESH Machines (attach-at-construction,
//   mu2000.cpp:383-393/997) instead of load_program over an existing mu2000;
//   load-failure return values ignored exactly like disk (:470-472).

use smu_compat::roms;
use smu_machine::xg::{fx, model, ram, state, sysfx};
use smu_machine::Machine;

// origin: xgtest.cpp:30 constexpr u32 RATE = 44100
const RATE: u64 = 44100;

// stdout/stderr carry CRLF on Windows (text-mode CRT, boot.rs:17-19 pitfall)
const EOL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// origin: xgtest.cpp:32-69 `struct rig`
struct Rig {
    mu: Box<Machine>, // :33 mu2000 mu (Box == statetest make_unique)
    reader: model::Model, // :34 "reader — read-only from MIDI OUT"
    samples: u64, // :35 = 0
    sintab: Vec<u16>, // the run_sample_pair seam (render.rs:510)
}

/// origin: render.rs:276-286 / statetest.rs:108-114 run_sample
/// (mu2000.cpp:3186-3189 cycle-debt + :3388 run_cycles + :3414-3415
/// run_sample_pair; the throwaway `s32 l, r` sinks on disk)
fn run_sample(m: &mut Machine, sintab: &[u16]) -> (i32, i32) {
    m.cycle_debt = m.cycle_debt.wrapping_add(28_000_000); // mu2000.cpp:3187
    let cycles = m.cycle_debt / 44100; // :3188
    m.cycle_debt -= cycles.wrapping_mul(44100); // :3189
    m.run_cycles(cycles); // :3388
    m.run_sample_pair(sintab) // :3414-3415
}

impl Rig {
    /// origin: xgtest.cpp:37 now_ms
    fn now_ms(&self) -> u64 {
        self.samples * 1000 / RATE // :37
    }

    /// origin: xgtest.cpp:39-49 pump(ms) — run, draining MIDI OUT into reader
    fn pump(&mut self, ms: u32) {
        let until = self.samples + ms as u64 * RATE / 1000; // :42
        let mut l: i32;
        let mut r: i32;
        while self.samples < until {
            // :43 for (; samples < until; samples++)
            (l, r) = run_sample(&mut self.mu, &self.sintab); // :44
            let _ = (&l, &r); // disk's throwaway sinks
            while let Some(b) = self.mu.midi.midi_out_take() {
                // :45-47
                self.reader.feed(b); // :47
            }
            self.samples += 1; // :43 samples++
        }
    }

    /// origin: xgtest.cpp:51-55 send
    fn send(&mut self, m: &[u8]) {
        for b in m {
            self.mu.midi_in(*b, 0); // :53-54 mu.midi_in(b, 0)
        }
    }

    /// origin: xgtest.cpp:57-68 ask — ask, then run until the mirror answers
    /// (max 500ms)
    fn ask(&mut self, p: &model::Param, part: i32) -> Option<i32> {
        self.reader.forget(p, part); // :60
        let req = model::param_request(p, part); // :61
        self.send(&req); // :61
        for _ in 0..50 {
            // :62 t < 50 (10ms pumps => 500ms max)
            self.pump(10); // :63
            if let Some(v) = self.reader.get(p, part) {
                return Some(v); // :64-65
            }
        }
        None // :67
    }
}

/// origin: xgtest.cpp:71-77 label
fn label(p: &model::Param, part: i32) -> String {
    if p.where_ == model::Area::Part {
        // :74-75 key + "[N]" (1-based)
        format!("{}[{}]", p.key, part + 1)
    } else {
        p.key.to_string() // :73
    }
}

/// "無し"/number for the problem strings (disk std::to_string/無し pairs)
fn or_nashi(v: Option<i32>) -> String {
    match v {
        Some(v) => v.to_string(),
        None => "無し".to_string(),
    }
}

/// origin: xgtest.cpp:458-464 snapshot(mu, s) — mirror -> XgSnapshot
fn snapshot(mu: &Machine, s: &mut state::XgSnapshot) {
    let ram_v = &mu.soc.bus.ram; // :459 mu.nvram()
    s.system.copy_from_slice(&ram_v[ram::SYSTEM as usize..][..state::XG_SYSTEM_SIZE]); // :460
    s.effect
        .copy_from_slice(&ram_v[ram::EFFECT as usize..][..state::XG_EFFECT_SIZE]); // :461
    for p in 0..state::XG_PARTS {
        // :462-463
        let base = ram::part_base(p as i32) as usize;
        s.parts[p].copy_from_slice(&ram_v[base..base + state::XG_PART_COPY]);
    }
}

fn main() {
    // origin: xgtest.cpp:83-95 args
    let raw: Vec<String> = std::env::args().skip(1).collect(); // argv[1..]
    if raw.is_empty() {
        // :84 usage (exact string; stderr is CRLF too)
        eprint!("xgtest <rom ディレクトリ> [-v]{EOL}");
        std::process::exit(2); // :85
    }
    let dir = raw[0].clone(); // :87
    let mut verbose = false; // :88
    let mut usb_host = false; // :88
    let mut drumprobe = false; // :88
    for a in raw[1..].iter() {
        // :89-95 (each `if`, not else-if — disk shape)
        if a == "-v" {
            verbose = true; // :90
        }
        if a == "--usb" {
            usb_host = true; // :91
        }
        if a == "--drumprobe" {
            drumprobe = true; // :92-94
        }
    }

    // :97-101 rig g + loads (fatal: stderr mu.error(), exit 2). Short-circuit
    // || == the same single message on the first failure.
    let mut g = match roms::load_program(&format!("{dir}/mu2000_flash.bin")) {
        Ok(prog) => match roms::load_wave(&format!("{dir}/dump")) {
            Ok(wave) => {
                let mut mu = Box::new(Machine::new(prog)); // Machine attaches
                mu.wave = wave; // the wave bus seam (render.rs:494)
                Rig {
                    mu,
                    reader: model::Model::new(), // :34
                    samples: 0, // :35
                    sintab: Vec::new(),
                }
            }
            Err(e) => {
                eprint!("{e}{EOL}"); // :99 mu.error()
                std::process::exit(2); // :100
            }
        },
        Err(e) => {
            eprint!("{e}{EOL}"); // :99 mu.error()
            std::process::exit(2); // :100
        }
    };
    // :102 load_sintab — return value IGNORED on disk; empty stand-in on Err
    g.sintab = match roms::load_sintab(&format!("{dir}/standin/sin-table.bin")) {
        Ok(s) => s,
        Err(_) => Vec::new(),
    };
    g.mu.set_usb_host(usb_host); // :103 set_usb_host — BEFORE reset (M7 LIVE)
    g.mu.reset(); // :104 "no NVRAM: factory state every time"
    let mut l: i32;
    let mut r: i32;
    // :105-107 boot wait: 30 s max, until SCI RX is live
    while g.samples < 30 * RATE && !g.mu.midi_ready(0) {
        (l, r) = run_sample(&mut g.mu, &g.sintab); // :107
        let _ = (&l, &r);
        g.samples += 1; // :106 samples++
    }
    // :108-110 起動 %.2f 秒で SCI 受信 %s（USB モード %s）
    print!(
        "起動 {:.2} 秒で SCI 受信 {}（USB モード {}）{EOL}",
        g.samples as f64 / RATE as f64,
        if g.mu.midi_ready(0) { "有効" } else { "無効" },
        if usb_host { "入" } else { "切" }
    );
    if usb_host {
        // :111-113 USB mode does not use DIN, so do not wait (normally a
        // no-op: boot already overshot 10 s)
        while g.samples < 10 * RATE {
            (l, r) = run_sample(&mut g.mu, &g.sintab);
            let _ = (&l, &r);
            g.samples += 1; // :112
        }
    }
    g.pump(500); // :114

    // ---- --drumprobe (origin: xgtest.cpp:116-145) ----
    if drumprobe {
        let key: i32 = 36; // :117
        let row = |g: &Rig| -> [u8; ram::DRUM_SETUP_PARAM as usize] {
            // :118-123 the 23-byte row of group 0 / key `key`
            let mut v = [0u8; ram::DRUM_SETUP_PARAM as usize]; // :119
            for k in 0..ram::DRUM_SETUP_PARAM as usize {
                v[k] = g.mu.soc.bus.ram[ram::drum_setup(0, key, k as i32) as usize]; // :121
            }
            v
        };
        print!("組 0・鍵 {key} の既定: "); // :124
        for x in row(&g) {
            print!("{:02x} ", x); // :125-126
        }
        print!("{EOL}"); // :127
        for addr in 0..0x80i32 {
            // :128
            let before = row(&g); // :129
            for val in [0x01u8, 0x22, 0x33] {
                // :130-139
                g.send(&[0xf0, 0x43, 0x10, 0x4c, 0x30, key as u8, addr as u8, val, 0xf7]); // :131
                g.pump(40); // :132
                let after = row(&g); // :133
                for k in 0..after.len() {
                    // :134-136
                    if after[k] != before[k] {
                        print!(
                            "番地 {:02x} -> RAM の {:>2} 番目（{:02x} を書いて {:02x}）{EOL}",
                            addr, k, val, after[k]
                        );
                    }
                }
                if after != before {
                    break; // :137-138
                }
            }
            // :140-142 put it back (drum setup reset)
            g.send(&[0xf0, 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7d, 0x00, 0xf7]); // :141
            g.pump(60); // :142
        }
        std::process::exit(0); // :144 return 0
    }

    let mut checked: i32 = 0; // :147
    let mut bad: i32 = 0; // :147
    let mut problems: Vec<String> = Vec::new(); // :148

    // ---- 1. write and read back (origin: xgtest.cpp:150-207) ----
    const PARTS: [i32; 4] = [0, 16, 32, 63]; // :151 one part per port A-D
    for p in model::params() {
        // :152
        let nparts = if p.where_ == model::Area::Part { 4 } else { 1 }; // :153
        for k in 0..nparts {
            let part = if p.where_ == model::Area::Part {
                PARTS[k] // :155
            } else {
                0
            };
            let original = match g.ask(p, part) {
                // :156-161
                Some(v) => v,
                None => {
                    problems.push(format!("{}: 問い合わせに返事が無い", label(p, part))); // :158
                    bad += 1; // :159
                    continue; // :160
                }
            };
            // :163-173 the values to try. Types jump around, so just the
            // current value and NO EFFECT (0); element_reserve has a
            // all-parts budget so keep it small
            let mut values: Vec<i32>;
            if p.max == 0x3fff {
                values = vec![0, original]; // :167
            } else if p.key == "part.element_reserve" {
                values = vec![0, 4, original]; // :169
            } else {
                values = vec![p.min, p.max, (p.min + p.max + 1) / 2]; // :171
                if p.special >= 0 {
                    values.push(p.special); // :172
                }
            }
            let mut seen = String::new(); // :175
            let prog = model::find("part.program").unwrap(); // :176
            let mut program: i32 = 0; // :177
            if model::applies_on_program(p) {
                // :178-179 (ask return ignored on disk; program stays 0)
                if let Some(v) = g.ask(prog, part) {
                    program = v;
                }
            }
            for v in values {
                g.send(&model::param_change(p, part, v)); // :181
                // :182-184 the bank only applies once the program is written
                // (model::set does the same)
                if model::applies_on_program(p) {
                    g.send(&model::param_change(prog, part, program)); // :184
                }
                g.pump(30); // :185
                let got = g.ask(p, part); // :186-187 (got=-1 sentinel: None)
                checked += 1; // :188
                let okv = matches!(got, Some(x) if x == v); // :189
                if !okv {
                    // :190-195 "%s: %d を書いたら %s"
                    problems.push(format!(
                        "{}: {} を書いたら {}",
                        label(p, part),
                        v,
                        match got {
                            Some(x) => x.to_string(),
                            None => "返事が無い".to_string(),
                        }
                    ));
                    bad += 1; // :194
                }
                if verbose {
                    // :196-197
                    seen.push_str(&format!(
                        " {}→{}",
                        v,
                        match got {
                            Some(x) => x.to_string(),
                            None => "?".to_string(),
                        }
                    ));
                }
            }
            g.send(&model::param_change(p, part, original)); // :199
            if model::applies_on_program(p) {
                g.send(&model::param_change(prog, part, program)); // :200-201
            }
            g.pump(30); // :202
            if verbose {
                // :203-204  "  %-24s 元 %-6d %s\n"
                print!("  {:<24} 元 {:<6} {}{EOL}", label(p, part), original, seen);
            }
        }
    }
    // :207 書いて読み返す: %d 回、食い違い %d
    print!("書いて読み返す: {checked} 回、食い違い {bad}{EOL}");

    // ---- 2. bulk-dump slice == single ask (origin: xgtest.cpp:209-274) ----
    {
        let mut n: i32 = 0; // :211
        let mut diff: i32 = 0; // :211
        for part in PARTS {
            // :212
            let mut dumped = model::Model::new(); // :213
            let req = model::part_dump_request(part); // :214
            // :215-216 answers go into a separate mirror (and the reader)
            g.send(&req); // :216
            let until = g.samples + RATE / 2; // :217
            while g.samples < until {
                (l, r) = run_sample(&mut g.mu, &g.sintab); // :219
                let _ = (&l, &r);
                while let Some(b) = g.mu.midi.midi_out_take() {
                    // :221-224
                    dumped.feed(b); // :222
                    g.reader.feed(b); // :223
                }
                g.samples += 1; // :218 samples++
            }
            for p in model::params() {
                // :226
                // :227 the part dump is only 08 pp 00-28 (41 bytes); EQ
                // (72-77) and HPF (0A pp 20) are not in it
                if p.where_ != model::Area::Part
                    || p.hi != 0x08
                    || p.lo as u32 >= ram::PART_XG_SIZE
                {
                    continue; // :228-229
                }
                let in_dump = dumped.get(p, part); // :231
                let single = g.ask(p, part); // :232
                n += 1; // :233
                if in_dump.is_none() || single.is_none() || in_dump != single {
                    // :234-239
                    diff += 1; // :235
                    problems.push(format!(
                        "{}: ダンプ {} / 問い合わせ {}",
                        label(p, part),
                        or_nashi(in_dump),
                        or_nashi(single)
                    ));
                }
            }
        }
        // :242-244 system and effect blocks too (the effect page reads via this)
        let blocks: [u32; 6] = [
            model::pack(0x00, 0x00, 0x00),
            model::pack(0x02, 0x01, 0x00),
            model::pack(0x02, 0x01, 0x20),
            model::pack(0x02, 0x01, 0x40),
            model::pack(0x03, 0x00, 0x00),
            model::pack(0x03, 0x01, 0x00),
        ];
        for blk in blocks {
            // :245
            let mut dumped = model::Model::new(); // :246
            let req = model::dump_request(blk); // :247
            g.send(&req); // :247
            let until = g.samples + RATE / 2; // :248
            while g.samples < until {
                (l, r) = run_sample(&mut g.mu, &g.sintab); // :250
                let _ = (&l, &r);
                while let Some(b) = g.mu.midi.midi_out_take() {
                    dumped.feed(b); // :252-253
                }
                g.samples += 1; // :249
            }
            for p in model::params() {
                // :255
                // :256 02 01 comes in three 0x20 blocks (reverb/chorus/variation)
                let at = model::address(p, 0); // :257
                if p.where_ == model::Area::Part || at < blk || at - blk >= 0x20 {
                    continue; // :258-259
                }
                let in_dump = dumped.get(p, 0); // :261
                let single = g.ask(p, 0); // :262
                n += 1; // :263
                if in_dump.is_none() || single.is_none() || in_dump != single {
                    // :264-269
                    diff += 1; // :265
                    problems.push(format!(
                        "{}: ダンプ {} / 問い合わせ {}",
                        label(p, 0),
                        or_nashi(in_dump),
                        or_nashi(single)
                    ));
                }
            }
        }
        // :272 ダンプと問い合わせの一致: %d 個、食い違い %d
        print!("ダンプと問い合わせの一致: {n} 個、食い違い {diff}{EOL}");
        bad += diff; // :273
    }

    // ---- 3. values changed by non-XG paths are readable too
    //      (origin: xgtest.cpp:276-361) ----
    {
        let prog = model::find("part.program").unwrap(); // :278
        let vol = model::find("part.volume").unwrap(); // :279
        g.send(&[0xc0, 0x30]); // :280 part 1 -> program 48 (shows as 49)
        g.send(&[0xb0, 7, 77]); // :281 part 1 volume 77
        g.send(&[0xc0 | 0x0, 0x30]); // :282 (disk repeats it — kept)
        g.mu.midi_in(0xc2, 1); // :283 port B ch3 = part 19 -> ...
        g.mu.midi_in(0x04, 1); // :284 ... program 4 (shows as 5)
        g.pump(100); // :285
        let mut m = model::Model::new(); // :286
        m.want_part(0); // :287
        m.want_part(18); // :288
        while m.busy() {
            // :289-299
            let out = m.poll(g.now_ms()); // :290
            g.send(&out); // :291
            let until = g.samples + RATE / 100; // :292
            while g.samples < until {
                (l, r) = run_sample(&mut g.mu, &g.sintab); // :294
                let _ = (&l, &r);
                while let Some(b) = g.mu.midi.midi_out_take() {
                    m.feed(b); // :296-297
                }
                g.samples += 1; // :293
            }
        }
        // :300-301 p1/v1/p19 = -1; ok = get && get && get (short-circuit: the
        // later gets do not run once one fails — value stays -1)
        let mut p1: i32 = -1;
        let mut v1: i32 = -1;
        let mut p19: i32 = -1;
        let mut ok = false;
        if let Some(v) = m.get(prog, 0) {
            p1 = v;
            if let Some(v) = m.get(vol, 0) {
                v1 = v;
                if let Some(v) = m.get(prog, 18) {
                    p19 = v;
                    ok = true;
                }
            }
        }
        let good = ok && p1 == 0x30 && v1 == 77 && p19 == 4; // :302
        // :303-305
        print!(
            "プログラムチェンジと CC7 を読み返す（model の頼み方で）: {}（パート 1 = {} / 音量 {}、パート 19 = {}。チェックサム違い {}）{EOL}",
            if good { "合" } else { "違" },
            p1,
            v1,
            p19,
            m.rejected()
        );
        if !good {
            // :306-309
            bad += 1;
            problems.push("プログラムチェンジ・CC7 の読み返し".to_string());
        }

        // :311 — model::set writes the bank and chains the mirror's program
        // so it applies immediately
        let msb = model::find("part.bank_msb").unwrap(); // :312
        let out = m.set(msb, 0, 64); // :313
        g.send(&out); // :313
        m.forget(msb, 0); // :314
        m.want_part(0); // :315
        while m.busy() {
            // :316-325
            let out = m.poll(g.now_ms()); // :317
            g.send(&out); // :317
            let until = g.samples + RATE / 100; // :318
            while g.samples < until {
                (l, r) = run_sample(&mut g.mu, &g.sintab); // :320
                let _ = (&l, &r);
                while let Some(b) = g.mu.midi.midi_out_take() {
                    m.feed(b); // :322-323
                }
                g.samples += 1; // :319
            }
        }
        // :326-327: bank only written by get on success (else -1)
        let mut bank: i32 = -1;
        let mut bank_ok = false;
        if let Some(v) = m.get(msb, 0) {
            bank = v;
            bank_ok = v == 64;
        }
        // :328
        print!(
            "model::set でバンクを書いてすぐ効くか: {}（MSB = {}）{EOL}",
            if bank_ok { "合" } else { "違" },
            bank
        );
        if !bank_ok {
            // :329-332
            bad += 1;
            problems.push("model::set のバンク".to_string());
        }

        // :334-335 does a read asked BEFORE a write lose to it when it lands
        // later? (keeps the knob from visually snapping back mid-drag)
        let pan = model::find("part.pan").unwrap(); // :336
        let t0 = g.now_ms(); // :337
        m.want_part(0); // :338
        let out = m.poll(t0); // :339 ask (the box answers with the OLD value)
        g.send(&out); // :339
        let out = m.set(pan, 0, 20); // :340 write before the answer arrives
        g.send(&out); // :340
        for _ in 0..10 {
            // :341-350
            let until = g.samples + RATE / 100; // :342
            while g.samples < until {
                (l, r) = run_sample(&mut g.mu, &g.sintab); // :344
                let _ = (&l, &r);
                while let Some(b) = g.mu.midi.midi_out_take() {
                    m.feed(b); // :346-347
                }
                g.samples += 1; // :343
            }
            let out = m.poll(g.now_ms()); // :349
            g.send(&out); // :349
        }
        let mut seen: i32 = -1; // :351
        if let Some(v) = m.get(pan, 0) {
            seen = v; // :352 return ignored on disk
        }
        let mut actual: i32 = -1; // :351
        if let Some(v) = g.ask(pan, 0) {
            actual = v; // :353 return ignored on disk
        }
        let pin_ok = seen == 20 && actual == 20; // :354
        // :355-356
        print!(
            "読み返しと書き込みが行き違っても書いた値が残るか: {}（写し {} / 音源 {}）{EOL}",
            if pin_ok { "合" } else { "違" },
            seen,
            actual
        );
        if !pin_ok {
            // :357-360
            bad += 1;
            problems.push("読み返しと書き込みの行き違い".to_string());
        }
    }

    // ---- 5. work-RAM values == asks (origin: xgtest.cpp:363-421) ----
    //      the screen reads RAM without asking, so the table must be right
    {
        let mut n: i32 = 0; // :366
        let mut diff: i32 = 0; // :366
        // :367 defaults are mostly 0, so scatter first. In RAM 10 and 26 are
        // the port heads (the reordering, ram.h:98-100)
        const RAM_PARTS: [i32; 9] = [0, 9, 16, 25, 31, 32, 41, 48, 63];
        for part in RAM_PARTS {
            // :369-374
            g.send(&model::param_change(
                model::find("part.volume").unwrap(),
                part,
                37 + part,
            ));
            g.send(&model::param_change(
                model::find("part.pan").unwrap(),
                part,
                20 + part,
            ));
            g.send(&model::param_change(
                model::find("part.cutoff").unwrap(),
                part,
                90 - part,
            ));
            g.send(&model::param_change(
                model::find("part.detune").unwrap(),
                part,
                0x5a + part,
            ));
        }
        g.send(&model::param_change(
            model::find("reverb.return").unwrap(),
            0,
            0x33,
        )); // :375
        g.send(&model::param_change(
            model::find("chorus.pan").unwrap(),
            0,
            0x21,
        )); // :376
        g.send(&model::param_change(
            model::find("variation.part").unwrap(),
            0,
            3,
        )); // :377
        g.send(&model::param_change(
            model::find("insertion2.part").unwrap(),
            0,
            7,
        )); // :378
        g.pump(200); // :379
        for p in model::params() {
            // :381
            let nparts = if p.where_ == model::Area::Part { 5 } else { 1 }; // :382
            for k in 0..nparts {
                let part = if p.where_ == model::Area::Part {
                    RAM_PARTS[k] // :384
                } else {
                    0
                };
                let off = match ram::locate(model::address(p, part)) {
                    // :385-390
                    Some(off) => off,
                    None => {
                        problems.push(format!("{}: RAM の番地が表に無い", label(p, part))); // :387
                        diff += 1; // :388
                        continue; // :389
                    }
                };
                // :391-393 fold the RAM bytes (read BEFORE the ask, disk order)
                let mut v: i32 = 0;
                for i in 0..p.size as usize {
                    v = (v << if p.enc == model::Coding::Nibble { 4 } else { 7 })
                        | (g.mu.soc.bus.ram[(off + i as u32) as usize]
                            & if p.enc == model::Coding::Nibble {
                                0x0f
                            } else {
                                0x7f
                            }) as i32;
                }
                let asked = g.ask(p, part); // :394
                n += 1; // :395
                if asked != Some(v) {
                    // :396-401
                    diff += 1; // :397
                    problems.push(format!(
                        "{}: RAM {} / 問い合わせ {}",
                        label(p, part),
                        v,
                        or_nashi(asked)
                    ));
                }
            }
        }
        // :404 RAM と問い合わせの一致: %d 個、食い違い %d
        print!("RAM と問い合わせの一致: {n} 個、食い違い {diff}{EOL}");
        bad += diff; // :405

        // :407-408 insertion params 1-10 (2-byte, XG 30-43) live in RAM as
        // 16-bit numbers from block +0x18. Test above 128: insertion 3 to
        // DELAY LCR, Rch Delay 1234
        g.send(&[0xf0, 0x43, 0x10, 0x4c, 0x03, 0x02, 0x00, 0x05, 0x00, 0xf7]); // :409
        g.pump(200); // :410
        g.send(&[
            0xf0,
            0x43,
            0x10,
            0x4c,
            0x03,
            0x02,
            0x32,
            (1234 >> 7) as u8,
            (1234 & 0x7f) as u8,
            0xf7,
        ]); // :411
        g.pump(200); // :412
        let at = (ram::INS_BLOCK[2] + ram::INS_WIDE + 2) as usize; // :413-414
        let wide = (g.mu.soc.bus.ram[at] as i32) << 8 | g.mu.soc.bus.ram[at + 1] as i32; // :415
        // :416
        print!(
            "インサーションの 2 バイトのパラメータが RAM の 16bit の数に入るか: {}（{}）{EOL}",
            if wide == 1234 { "合" } else { "違" },
            wide
        );
        if wide != 1234 {
            // :417-420
            bad += 1;
            problems.push("インサーションの 2 バイトのパラメータの RAM の位置".to_string());
        }
    }

    // ---- 6. round-trip the XG-only value copy (ui/xg_state.h setup_messages)
    //      into a booted machine and back (origin: xgtest.cpp:423-580) ----
    {
        // :426-432 set() — a bare parameter change + 60ms
        let mut set = |g: &mut Rig, m: &[i32]| {
            let mut v: Vec<u8> = vec![0xf0, 0x43, 0x10, 0x4c]; // :427
            for b in m {
                v.push(*b as u8); // :428
            }
            v.push(0xf7); // :429
            g.send(&v); // :430
            g.pump(60); // :431
        };
        set(&mut g, &[0x02, 0x01, 0x00, 0x01, 0x01]); // :433 reverb HALL 2
        set(&mut g, &[0x02, 0x01, 0x02, 0x2a]); // :434
        set(&mut g, &[0x02, 0x01, 0x10, 0x05]); // :435 reverb params 11/15
        set(&mut g, &[0x02, 0x01, 0x14, 0x33]); // :436
        set(&mut g, &[0x02, 0x01, 0x20, 0x43, 0x00]); // :437 chorus FLANGER 1
        set(&mut g, &[0x02, 0x01, 0x24, 0x55]); // :438
        set(&mut g, &[0x02, 0x01, 0x40, 0x05, 0x00]); // :439 variation DELAY LCR
        set(&mut g, &[0x02, 0x01, 0x44, 2000 >> 7, 2000 & 0x7f]); // :440 >128 value
        set(&mut g, &[0x02, 0x01, 0x56, 0x51]); // :441
        set(&mut g, &[0x02, 0x01, 0x72, 0x0a]); // :442 param 13
        for part in [0i32, 9, 17, 40] {
            // :443
            set(&mut g, &[0x08, part, 0x67, 0x01]); // :444 porta/pitch EG/HPF/EQ
            set(&mut g, &[0x08, part, 0x68, 0x30 + part]); // :445
            set(&mut g, &[0x08, part, 0x69, 0x50]); // :446
            set(&mut g, &[0x08, part, 0x6c, 0x22]); // :447
            set(&mut g, &[0x0a, part, 0x20, 0x55 - part]); // :448
            set(&mut g, &[0x08, part, 0x72, 0x46]); // :449
            set(&mut g, &[0x08, part, 0x43, 0x50]); // :450 scale tune/AC1/vel range
            set(&mut g, &[0x08, part, 0x5b, 0x30 + part]); // :451
            set(&mut g, &[0x08, part, 0x6d, 0x20]); // :452
            set(&mut g, &[0x08, part, 0x76, 0x10]); // :453
        }
        set(&mut g, &[0x08, 0x09, 0x07, 0x04]); // :455 part 10 -> DRUMS3
        g.pump(300); // :456

        let mut from = state::XgSnapshot::new(); // :465 static ui::xg_snapshot from, to
        snapshot(&g.mu, &mut from); // :466
        let _msgs0 = state::setup_messages(&from); // :467 (recomputed per block below)

        // :469-481 boot(rig) — Rust: load into FRESH machines (see header
        // deviation); load return values ignored exactly like disk (:470-472)
        let boot = |dir: &str, usb_host: bool| -> Rig {
            let prog = roms::load_program(&format!("{dir}/mu2000_flash.bin")).unwrap_or_default(); // :470
            let wave = roms::load_wave(&format!("{dir}/dump")).unwrap_or_default(); // :471
            let sintab = roms::load_sintab(&format!("{dir}/standin/sin-table.bin")).unwrap_or_default(); // :472
            let mut mu = Box::new(Machine::new(prog));
            mu.wave = wave;
            mu.set_usb_host(usb_host); // :473 BEFORE reset (M7 LIVE)
            mu.reset(); // :474
            let mut samples: u64 = 0;
            while samples < 30 * RATE && !mu.midi_ready(0) {
                // :475-476
                run_sample(&mut mu, &sintab); // :476
                samples += 1;
            }
            if usb_host {
                // :477-479
                while samples < 10 * RATE {
                    run_sample(&mut mu, &sintab); // :479
                    samples += 1;
                }
            }
            let mut x = Rig {
                mu,
                reader: model::Model::new(),
                samples,
                sintab,
            };
            x.pump(500); // :480
            x
        };

        // :484-556 compare — every address in the table (system, effect, 64 parts)
        let compare = |to: &state::XgSnapshot,
                       from: &state::XgSnapshot,
                       what: &str,
                       bytes: usize,
                       bad: &mut i32,
                       problems: &mut Vec<String>| {
            let mut n: i32 = 0; // :485
            let mut diff: i32 = 0; // :485
            let mut where_: Vec<String> = Vec::new(); // :486 std::vector<std::string> where
            let mut cmp = |a: &[u8], b: &[u8], size: usize, name: &str, part: i32, lo: usize| {
                // :487-498
                for i in 0..size {
                    n += 1; // :489
                    if a[i] == b[i] {
                        continue; // :490-491
                    }
                    diff += 1; // :492
                    where_.push(format!( // :493-496 "%s%s%d %02X: %02x / %02x"
                        "{}{}{} {:02X}: {:02x} / {:02x}",
                        name,
                        if part >= 0 { " パート " } else { "" }, // :494
                        if part >= 0 { part + 1 } else { 0 }, // :495
                        lo + i, // :495
                        a[i], // :495
                        b[i] // :495
                    ));
                }
            };
            cmp(
                &from.system,
                &to.system,
                state::XG_SYSTEM_SIZE,
                "システム",
                -1,
                0,
            ); // :499
            // :500-502 effect: only the table addresses and the params the
            // CURRENT types use (bytes the current type does not use are
            // firmware-free — history may differ)
            let mut used = vec![false; state::XG_EFFECT_SIZE]; // :502
            let mut mark = |used: &mut Vec<bool>, addr: u32, size: i32| {
                // :503-508
                if let Some(off) = ram::locate(addr) {
                    // :505
                    for i in 0..size {
                        used[(off - ram::EFFECT + i as u32) as usize] = true; // :506-507
                    }
                }
            };
            for p in model::params() {
                // :509-511
                if p.where_ == model::Area::Effect {
                    mark(&mut used, model::address(p, 0), p.size as i32);
                }
            }
            let vw = (ram::VAR_BLOCK - ram::EFFECT + ram::VAR_WIDE) as usize; // :512
            for which in [sysfx::Sysfx::Reverb, sysfx::Sysfx::Chorus, sysfx::Sysfx::Variation] {
                // :513
                let off = match ram::locate(model::pack(
                    0x02,
                    0x01,
                    sysfx::sysfx_type_lo(which),
                )) {
                    // :514-515
                    Some(off) => off,
                    None => 0, // disk ignores the return; locate(02 01 type) is always in the table
                };
                let t = &from.effect[(off - ram::EFFECT) as usize..]; // :516
                let kind = (t[0] as i32) << 7 | t[1] as i32; // :517
                let def = fx::fx_find(kind); // :517
                if let Some(def) = def {
                    // :518 def && i < count
                    for prm in def.params {
                        let mut size: i32 = 0; // :519
                        let lo = sysfx::sysfx_addr(which, *prm, &mut size); // :520
                        if lo < 0 {
                            continue; // :521-522
                        }
                        if size == 2 {
                            // :523-524
                            used[vw + (lo - 0x42) as usize] = true;
                            used[vw + (lo - 0x42) as usize + 1] = true;
                        } else {
                            mark(&mut used, model::pack(0x02, 0x01, lo as u8), 1); // :526
                        }
                    }
                }
            }
            for k in 0..4usize {
                // :529
                let blk = (ram::INS_BLOCK[k] - ram::EFFECT) as usize; // :530
                let kind = (from.effect[blk] as i32) << 7 | from.effect[blk + 1] as i32; // :531
                let def = fx::fx_find(kind); // :531
                if let Some(def) = def {
                    // :532 def && i < count
                    for prm in def.params {
                        // :533 const xg::fx_param &fp = def->params[i]
                        if prm.1 == 2 {
                            // :534 fp.size == 2
                            let at = blk + ram::INS_WIDE as usize + (prm.0 as usize - 0x30); // :535
                            used[at] = true;
                            used[at + 1] = true;
                        } else {
                            // :537 mark(xg::pack(0x03, k, fp.addr), 1)
                            mark(&mut used, model::pack(0x03, k as u8, prm.0), 1);
                        }
                    }
                }
            }
            for i in 0..state::XG_EFFECT_SIZE {
                // :540-542
                if used[i] {
                    cmp(
                        &from.effect[i..],
                        &to.effect[i..],
                        1,
                        "エフェクトの RAM +",
                        -1,
                        i,
                    );
                }
            }
            for p in 0..state::XG_PARTS {
                // :543-551
                cmp(
                    &from.parts[p],
                    &to.parts[p],
                    ram::PART_XG_SIZE as usize,
                    "08",
                    p as i32,
                    0,
                );
                cmp(
                    &from.parts[p][ram::PART_EXT_RAM as usize..],
                    &to.parts[p][ram::PART_EXT_RAM as usize..],
                    ram::PART_EXT_SIZE as usize,
                    "08",
                    p as i32,
                    ram::PART_EXT_XG as usize,
                );
                cmp(
                    &from.parts[p][ram::PART_HPF_RAM as usize..],
                    &to.parts[p][ram::PART_HPF_RAM as usize..],
                    1,
                    "0A",
                    p as i32,
                    ram::PART_HPF_XG as usize,
                );
                for k in [0usize, 1, 4, 5] {
                    // :548-550
                    cmp(
                        &from.parts[p][ram::PART_EQ_RAM as usize + k..],
                        &to.parts[p][ram::PART_EQ_RAM as usize + k..],
                        1,
                        "08",
                        p as i32,
                        (ram::PART_EQ_XG as usize) + k,
                    );
                }
            }
            // :552 "%s（%zu バイト）を流して戻るか: %d バイト、食い違い %d"
            print!("{}（{} バイト）を流して戻るか: {} バイト、食い違い {}{EOL}", what, bytes, n, diff);
            for s in where_.iter().take(20) {
                // :553-554
                problems.push(format!("{what}から戻らない: {s}"));
            }
            *bad += diff; // :555
        };

        // :558-567 everything (plugin "XG values only" state, .syx export)
        {
            let mut h = boot(&dir, usb_host); // :560-561 static rig h; boot(h)
            let msgs = state::setup_messages(&from); // :562
            h.send(&msgs); // :563
            h.pump(6000); // :564 ~10KB over DIN is >3s
            let mut to = state::XgSnapshot::new(); // :565 (C++ `static to`)
            snapshot(&h.mu, &mut to); // :565
            compare(&to, &from, "XG の値の控え", msgs.len(), &mut bad, &mut problems); // :566
        }
        // :568-579 only the differences from a booted machine (.syx "not default")
        {
            let mut k = boot(&dir, usb_host); // :570-572 static rig k; static base; boot(k)
            let mut base = state::XgSnapshot::new(); // :571 static ui::xg_snapshot base
            snapshot(&k.mu, &mut base); // :573
            let msgs = state::setup_diff_messages(&from, &base); // :574
            k.send(&msgs); // :575
            k.pump(3000); // :576
            let mut to = state::XgSnapshot::new(); // :577
            snapshot(&k.mu, &mut to); // :577
            compare(
                &to,
                &from,
                "既定との違いだけの控え",
                msgs.len(),
                &mut bad,
                &mut problems,
            ); // :578
        }
    }

    // :582-585 report
    for s in problems.iter() {
        print!("  NG {s}{EOL}"); // :583 "  NG %s\n"
    }
    print!("{}", if bad != 0 { "食い違いあり" } else { "全部合った" }); // :584
    print!("{EOL}");
    if bad != 0 {
        std::process::exit(1); // :585 return bad ? 1 : 0
    }
    let _ = &verbose; // -v only shapes the (verbose) lines above
}
