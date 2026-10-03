// license:BSD-3-Clause
//
// origin: src/samptest.cpp (348 L) — サンプリングが一回りするかを確かめる。
//   samptest <rom ディレクトリ> [-v]
//
// W-SAMP2 port. Panel SAMPLING → REC, 440Hz sine into A/D INPUT, AUDITION +
// Goertzel, A/D part level, SmartMedia format → SAVE → LOAD (second machine
// behind ONE shared card), REC InputSrc AD2/AD1+2, TriggerLvl Waiting! →
// Recording!. Ground truth: `build/samptest.exe roms` (Makefile:323).
//
// Boot pattern matches render.rs (load_program/load_wave/load_sintab + reset
// + boot-wait on midi_ready) with NO bootcache — disk boots two rigs from
// scratch (samptest.cpp:101-108 and :242-250), so the config-dir cache/nvram
// stays untouched. Default want_threaded=false == disk default (mu2000.h:938;
// samptest never calls set_threaded).
//
// Deviations (reported, do NOT "fix" silently):
// - printf %-Ns width counts BYTES in C (C locale); Rust {:<N} counts CHARS,
//   so pad_bytes() reproduces the byte-width column fill exactly.
// - stdout/stderr LF (Rust byte-verbatim); disk printf text mode is CRLF.
//   The harness (tools/run_tests.py:566) splits lines, so LF is fine.
// - lround -> f64::round (both round half away from zero); libm sin/cos may
//   differ by ULPs — the gate is threshold-based by design.

use smu_compat::{paths, roms};
use smu_machine::{button_name, Button, Machine};

// origin: samptest.cpp:22-23
const RATE: u64 = 44100;
const PI: f64 = 3.14159265358979323846;
// origin: mu2000.h:259 `static constexpr s32 DAC_FULL_SCALE = 1 << 17`
const DAC_FULL_SCALE: f64 = (1i32 << 17) as f64;

// origin: samptest.cpp:115/262 `printf("%s %-28s [%s]\n", ...)` — C printf
// width is BYTE-count in the C locale; pad with spaces to `w` BYTES.
fn pad_bytes(s: &str, w: usize) -> String {
    let mut out = String::with_capacity(s.len() + w);
    out.push_str(s);
    for _ in s.len()..w {
        out.push(' ');
    }
    out
}

// origin: samptest.cpp:25-74 struct rig
struct Rig {
    mu: Machine, // :26 mu2000 mu
    verbose: bool, // :27
    sine_amp: f64, // :28 A/D INPUT に流す正弦の振幅（0 なら無音）
    sine2_amp: f64, // :29 AD2 だけ 660Hz にするときの振幅（負なら AD1 と同じもの）
    n: u64, // :30
    out: Vec<f64>, // :31 集めている間の出力（左右の平均）
    collect: bool, // :32
    debt: u64, // mu2000.h:918 m_cycle_debt — render.rs:620 pattern
}

// origin: samptest.cpp:34-48 pump — the audio callback's N frames are the clock
impl Rig {
    fn new(prog: Vec<u8>, wave: Vec<u8>, verbose: bool) -> Rig {
        // samptest.cpp has no Machine construction (mu2000 default ctor at
        // :26); render.rs:503-517 order — prog attach, then set_wave_rom
        // (mu2000.cpp:395-411 device pins). Threaded stays default OFF.
        let mut mu = Machine::new(prog);
        mu.set_wave_rom(wave);
        Rig {
            mu,
            verbose,
            sine_amp: 0.0, // :28
            sine2_amp: -1.0, // :29
            n: 0, // :30
            out: Vec::new(), // :31
            collect: false, // :32
            debt: 0, // mu2000.cpp:1051 reset zeroes m_cycle_debt (:925/:3545)
        }
    }

    fn pump(&mut self, ms: u32, sintab: &[u16]) {
        let until = self.n + (ms as u64) * RATE / 1000; // :36
        while self.n < until {
            let n = self.n as f64;
            // :38 s32(lround(sine_amp * sin(2*PI*440.0*n/RATE))) — 2*PI*440*n
            // and /RATE stay left-associative like C++
            let v = (self.sine_amp * (2.0 * PI * 440.0 * n / RATE as f64).sin()).round() as i32;
            // :39 sine2_amp < 0 ? v : s32(lround(sine2_amp * sin(2*PI*660.0*n/RATE)))
            let v2 = if self.sine2_amp < 0.0 {
                v
            } else {
                (self.sine2_amp * (2.0 * PI * 660.0 * n / RATE as f64).sin()).round() as i32
            };
            self.mu.set_audio_input(v, v2); // :40 mu2000.h:258
            // :42 mu.run_sample(l, r) — mu2000.cpp:3186-3189 debt + :3388
            // run_cycles + :3459-3460 sequential pair (threaded OFF, disk
            // default mu2000.h:938). Same glue as render.rs:289-295.
            self.debt = self.debt.wrapping_add(28_000_000); // :3187
            let cycles = self.debt / 44100; // :3188
            self.debt -= cycles.wrapping_mul(44100); // :3189
            self.mu.run_cycles(cycles); // :3388
            let (l, r) = self.mu.run_sample_pair(sintab); // :3459-3460
            // :43-44 while (mu.midi_out_take(b)) {} — mu2000.h:232-246
            while self.mu.midi.midi_out_take().is_some() {}
            if self.collect {
                // :46 (double(l)+double(r))*0.5/DAC_FULL_SCALE (exact ops)
                self.out
                    .push((l as f64 + r as f64) * 0.5 / DAC_FULL_SCALE);
            }
            self.n += 1;
        }
    }

    // origin: samptest.cpp:50-63 lcd — 2 rows × 24 cols, '|' between the rows
    fn lcd(&self) -> String {
        let dd = self.mu.lcd_ddram(); // :52 mu.lcd().ddram() (mu2000.h m_lcd)
        let mut s = String::new();
        for row in 0..2usize {
            for c in 0..24usize {
                let ch = dd[row * 0x40 + c]; // :56
                s.push(if ch >= 32 && ch < 127 { ch as char } else { ' ' }); // :57
            }
            if row == 0 {
                s.push('|'); // :59-60
            }
        }
        s
    }

    // origin: samptest.cpp:65-73 press(b, hold_ms = 80)
    fn press(&mut self, b: Button, hold_ms: u32, sintab: &[u16]) {
        self.mu.set_button(b, true); // :67
        self.pump(hold_ms, sintab); // :68
        self.mu.set_button(b, false); // :69
        self.pump(300, sintab); // :70
        if self.verbose {
            // :72 printf("  %-14s [%s]\n", button_name(b), lcd())
            println!("  {} [{}]", pad_bytes(button_name(b), 14), self.lcd());
        }
    }
}

// origin: samptest.cpp:76-87 tone — 周波数 f の成分の大きさ（Goertzel）.
// Exact double expression order: C++ `v + c*s1 - s2` is left-associative
// == Rust; `s1*s1 + s2*s2 - c*s1*s2` == `((s1*s1)+(s2*s2))-((c*s1)*s2)`.
fn tone(x: &[f64], f: f64) -> f64 {
    let w = 2.0 * PI * f / RATE as f64; // :79
    let c = 2.0 * w.cos(); // :79
    let mut s1 = 0.0f64; // :80
    let mut s2 = 0.0f64;
    for &v in x {
        let s0 = v + c * s1 - s2; // :82
        s2 = s1; // :83
        s1 = s0; // :84
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2).sqrt() / x.len() as f64 // :86
}

// origin: samptest.cpp:112-118 expect lambda
fn expect(g: &Rig, what: &str, text: &str, bad: &mut i32) {
    let s = g.lcd(); // :113
    let ok = s.contains(text); // :114 std::string::find != npos
    println!("{} {} [{}]", if ok { "合" } else { "NG" }, pad_bytes(what, 28), s); // :115
    if !ok {
        *bad += 1; // :116-117
    }
}

// origin: samptest.cpp:173-186 level_440 — returns (rms, tone440)
fn level_440(g: &mut Rig, sintab: &[u16]) -> (f64, f64) {
    g.out.clear(); // :174
    g.sine_amp = 8000.0; // :175
    g.pump(200, sintab); // :176
    g.collect = true; // :177
    g.pump(500, sintab); // :178
    g.collect = false; // :179
    g.sine_amp = 0.0; // :180
    let mut sum = 0.0f64; // :181
    for &v in g.out.iter() {
        sum += v * v; // :182-183
    }
    let rms = (sum / g.out.len().max(1) as f64).sqrt(); // :184 std::max<size_t>(1, size)
    (rms, tone(&g.out, 440.0)) // :185
}

// origin: samptest.cpp:281-313 record_src — returns (n_samples, f440, f660)
fn record_src(g: &mut Rig, sintab: &[u16], presses: i32) -> (usize, f64, f64) {
    g.press(Button::Exit, 80, sintab); // :282
    g.press(Button::SelectRight, 80, sintab); // :283 SAVE の隣が REC
    g.press(Button::Enter, 80, sintab); // :284
    for _ in 0..3 {
        g.press(Button::SelectRight, 80, sintab); // :285-286
    }
    for _ in 0..presses {
        g.press(Button::ValuePlus, 80, sintab); // :287-288
    }
    for _ in 0..3 {
        g.press(Button::SelectLeft, 80, sintab); // :289-290
    }
    let before: Vec<u8> = g.mu.sample_ram().clone(); // :291 vector copy
    g.sine_amp = 8000.0; // :292
    g.sine2_amp = 8000.0; // :293
    g.pump(200, sintab); // :294
    g.press(Button::Enter, 80, sintab); // :295
    g.pump(700, sintab); // :296
    g.press(Button::Enter, 80, sintab); // :297
    g.sine_amp = 0.0; // :298
    g.sine2_amp = -1.0; // :299
    g.pump(300, sintab); // :300
    let mut x: Vec<f64> = Vec::new(); // :301
    {
        let after = g.mu.sample_ram(); // :302
        let mut i = 0usize;
        while i + 1 < after.len() {
            // :303-305 i += 2 pairs; changed words only
            if after[i] != before[i] || after[i + 1] != before[i + 1] {
                let w = (after[i] as u16) | ((after[i + 1] as u16) << 8); // :305 u8|u8<<8
                x.push((w as i16) as f64 / 32768.0); // :305 s16 / 32768.0
            }
            i += 2;
        }
    }
    // :307 mid = x.size() > 8000 ? [begin+4000, begin+8000) : {}
    let mid: Vec<f64> = if x.len() > 8000 {
        x[4000..8000].to_vec()
    } else {
        Vec::new()
    };
    let f440 = if mid.is_empty() { 0.0 } else { tone(&mid, 440.0) }; // :308
    let f660 = if mid.is_empty() { 0.0 } else { tone(&mid, 660.0) }; // :309
    g.press(Button::Exit, 80, sintab); // :310 Keep Sample? から抜ける
    g.press(Button::Exit, 80, sintab); // :311
    (x.len(), f440, f660) // :312
}

// origin: samptest.cpp:91-348 main
fn main() {
    paths::init_console_utf8();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    // :93-96 argc < 2 -> 使い方 exit 1
    if raw.is_empty() {
        eprintln!("使い方: samptest <rom ディレクトリ> [-v]");
        std::process::exit(1);
    }
    let dir = raw[0].clone(); // :97
    // :99 verbose = argc > 2 && !strcmp(argv[2], "-v")
    let verbose = raw.len() > 1 && raw[1] == "-v";

    // origin: samptest.cpp:101-105 — load_program/load_wave fatal, error() ->
    // stderr; load_sintab return ignored (silent, unlike render.cpp:300 警告).
    let prog = match roms::load_program(&format!("{dir}/mu2000_flash.bin")) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}"); // :102
            std::process::exit(1); // :103
        }
    };
    let wave = match roms::load_wave(&format!("{dir}/dump")) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}"); // :102
            std::process::exit(1); // :103
        }
    };
    let mut g = Rig::new(prog, wave, verbose);
    let sintab: Vec<u16> = match roms::load_sintab(&format!("{dir}/standin/sin-table.bin")) {
        Ok(t) => t,
        Err(_) => Vec::new(), // :105 return value discarded
    };
    // mu2000.cpp:413-419 set_sintab_rom pin (render.rs:530 pattern)
    g.mu.set_sintab_pin(&sintab);
    g.mu.reset(); // :106
    // :107-108 boot-wait: for (i=0; i < 30*RATE && !midi_ready(); i += RATE/100) pump(10)
    let mut i = 0u64;
    while i < 30 * RATE && !g.mu.midi_ready(0) {
        g.pump(10, &sintab);
        i += RATE / 100;
    }
    g.pump(1500, &sintab); // :109

    let mut bad = 0i32; // :111

    // origin: samptest.cpp:120-127 SAMPLING の品書き → REC（Enter で入る）
    g.press(Button::SamplingMode, 80, &sintab); // :122
    expect(&g, "SAMPLING の品書き", "REC", &mut bad); // :123
    for _ in 0..3 {
        g.press(Button::SelectRight, 80, &sintab); // :124-125 REC は 4 つ目
    }
    g.press(Button::Enter, 80, &sintab); // :126
    expect(&g, "REC の画面", "Sp=001", &mut bad); // :127

    // origin: samptest.cpp:129-141 録音。始める前から正弦を流しておく
    g.sine_amp = 12000.0; // :130
    g.pump(200, &sintab); // :131
    g.press(Button::Enter, 80, &sintab); // :132
    expect(&g, "録音中", "Recording!", &mut bad); // :133
    g.pump(1000, &sintab); // :134
    g.press(Button::Enter, 80, &sintab); // :135 止める
    g.sine_amp = 0.0; // :136
    g.pump(300, &sintab); // :137
    g.press(Button::Exit, 80, &sintab); // :138
    expect(&g, "残すか聞かれる", "Keep Sample 001?", &mut bad); // :139
    g.press(Button::Enter, 80, &sintab); // :140
    g.press(Button::Exit, 80, &sintab); // :141

    // origin: samptest.cpp:143-153 EDIT → SAMPLE → SMPL001 で試聴
    for _ in 0..3 {
        g.press(Button::SelectLeft, 80, &sintab); // :144-145
    }
    g.press(Button::Enter, 80, &sintab); // :146
    g.press(Button::Enter, 80, &sintab); // :147
    expect(&g, "出来たサンプル", "SMPL001", &mut bad); // :148
    g.collect = true; // :149
    g.mu.set_button(Button::Audition, true); // :150
    g.pump(1000, &sintab); // :151
    g.mu.set_button(Button::Audition, false); // :152
    g.collect = false; // :153

    // origin: samptest.cpp:155-166 試聴の rms + 440Hz 判定
    let mut rms = 0.0f64; // :155
    for &v in g.out.iter() {
        rms += v * v; // :156-157
    }
    rms = (rms / g.out.len().max(1) as f64).sqrt(); // :158 std::max<size_t>(1, size)
    let t440 = tone(&g.out, 440.0); // :159
    let t330 = tone(&g.out, 330.0);
    let t587 = tone(&g.out, 587.0);
    let loud = rms > 0.01; // :160
    let pitch = t440 > 10.0 * t330.max(t587); // :161
    println!("{} 試聴の音の大きさ              rms {:.4}（全振幅 1）", if loud { "合" } else { "NG" }, rms); // :162
    println!(
        "{} 試聴の音が 440Hz              440Hz {:.5} / 330Hz {:.5} / 587Hz {:.5}", // :163-164
        if pitch { "合" } else { "NG" },
        t440,
        t330,
        t587
    );
    if !loud {
        bad += 1; // :165
    }
    if !pitch {
        bad += 1; // :166
    }

    // origin: samptest.cpp:168-202 A/D パート。既定の音量は 0 で、入力は聞こえない
    g.press(Button::Exit, 80, &sintab); // :170
    g.press(Button::Exit, 80, &sintab); // :171
    g.press(Button::Exit, 80, &sintab); // :172
    let (rms_off, ad_off) = level_440(&mut g, &sintab); // :188
    // :189-193 2× XG パート音量 sysex（part 0/1 を 100 へ）を port 0 へ
    for part in 0..2i32 {
        let msg: [u8; 9] = [0xf0, 0x43, 0x10, 0x4c, 0x10, part as u8, 0x0b, 100, 0xf7]; // :190
        for &b in msg.iter() {
            g.mu.midi_in(b, 0); // :191-192
        }
    }
    g.pump(300, &sintab); // :194
    let (rms_on, ad_on) = level_440(&mut g, &sintab); // :195
    let off_ok = rms_off < 0.001; // :196
    // :197 rms_on > 0.05 && ad_on > 10*max(tone(330), tone(587)) on the LIVE g.out
    let on_ok = rms_on > 0.05 && ad_on > 10.0 * tone(&g.out, 330.0).max(tone(&g.out, 587.0));
    println!(
        "{} A/D パートの音量 0 では無音      rms {:.5}", // :198
        if off_ok { "合" } else { "NG" },
        rms_off
    );
    println!(
        "{} A/D パートの音量 100 で入力が鳴る rms {:.4} / 440Hz {:.5}", // :199
        if on_ok { "合" } else { "NG" },
        rms_on,
        ad_on
    );
    let _ad_off = ad_off; // :200 (void)ad_off
    if !off_ok {
        bad += 1; // :201
    }
    if !on_ok {
        bad += 1; // :202
    }

    // origin: samptest.cpp:204-239 SmartMedia。空のカードで書式化 → SAVE ALL+SEQ
    let _ = g.mu.card.borrow_mut().create(32); // :207 card().create(32) (disk void)
    g.pump(500, &sintab); // :208
    g.press(Button::Util, 80, &sintab); // :209
    for _ in 0..4 {
        g.press(Button::SelectRight, 80, &sintab); // :210-211
    }
    g.press(Button::Enter, 80, &sintab); // :212
    for _ in 0..4 {
        g.press(Button::SelectRight, 80, &sintab); // :213-214
    }
    expect(&g, "UTIL → CARD → Format", "Format", &mut bad); // :215
    g.press(Button::Enter, 80, &sintab); // :216
    g.press(Button::Enter, 80, &sintab); // :217 書式化してよいか
    // :218-219 Executing が消えるまで ≤100×100 ms
    for _ in 0..100 {
        if !g.lcd().contains("Executing") {
            break;
        }
        g.pump(100, &sintab);
    }
    expect(&g, "書式化を終えた", "Format", &mut bad); // :220
    g.press(Button::Exit, 80, &sintab); // :221
    g.press(Button::Exit, 80, &sintab); // :222
    g.press(Button::Exit, 80, &sintab); // :223

    // :225-239 SAMPLING → SAVE → ALL+SEQ を書く
    g.press(Button::SamplingMode, 80, &sintab); // :225
    g.press(Button::SelectRight, 80, &sintab); // :226
    g.press(Button::SelectRight, 80, &sintab); // :227
    g.press(Button::Enter, 80, &sintab); // :228
    expect(&g, "SAVE の画面", "ALL+SEQ", &mut bad); // :229
    g.press(Button::Enter, 80, &sintab); // :230 保存先のディレクトリ
    g.pump(1000, &sintab); // :231
    g.press(Button::Enter, 80, &sintab); // :232 ファイルの名前
    g.pump(1000, &sintab); // :233
    expect(&g, "ファイルの名前", "ALL_SEQ", &mut bad); // :234
    g.press(Button::Enter, 80, &sintab); // :235
    expect(&g, "書き出し中", "SAVING", &mut bad); // :236
    // :237-238 SAVING が消えるまで ≤100×100 ms
    for _ in 0..100 {
        if !g.lcd().contains("SAVING") {
            break;
        }
        g.pump(100, &sintab);
    }
    expect(&g, "書き終えた", "<SAVE>", &mut bad); // :239

    // origin: samptest.cpp:241-252 別の機械（fresh boot）に同じカードを差し込む。
    // h.mu.card() = g.mu.card() の Rust 形 = share_card_from（W-SAMP1 seam）
    let hprog = match roms::load_program(&format!("{dir}/mu2000_flash.bin")) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}"); // :243
            std::process::exit(1); // :244
        }
    };
    let hwave = match roms::load_wave(&format!("{dir}/dump")) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}"); // :243
            std::process::exit(1); // :244
        }
    };
    let mut h = Rig::new(hprog, hwave, verbose); // :246 h.verbose = g.verbose
    let hsintab: Vec<u16> = match roms::load_sintab(&format!("{dir}/standin/sin-table.bin")) {
        Ok(t) => t,
        Err(_) => Vec::new(), // :247 return value discarded
    };
    h.mu.set_sintab_pin(&hsintab);
    h.mu.reset(); // :248
    let mut i = 0u64; // :249-250 boot-wait
    while i < 30 * RATE && !h.mu.midi_ready(0) {
        h.pump(10, &hsintab);
        i += RATE / 100;
    }
    h.mu.share_card_from(&g.mu); // :251
    h.pump(1500, &hsintab); // :252

    // origin: samptest.cpp:253-267 SAMPLING → LOAD で読み戻す
    h.press(Button::SamplingMode, 80, &hsintab); // :253
    h.press(Button::SelectRight, 80, &hsintab); // :254
    h.press(Button::Enter, 80, &hsintab); // :255
    h.pump(1000, &hsintab); // :256
    h.press(Button::Enter, 80, &hsintab); // :257 ディレクトリの中
    h.pump(1000, &hsintab); // :258
    expect(&h, "カードにファイルがある", "ALL_SEQ.M2A", &mut bad); // :259-264
    h.press(Button::Enter, 80, &hsintab); // :265
    // :266-267 LOADING が消えるまで ≤100×100 ms
    for _ in 0..100 {
        if !h.lcd().contains("LOADING") {
            break;
        }
        h.pump(100, &hsintab);
    }

    // origin: samptest.cpp:268-277 sample RAM 比較。最後の 1 語の後ろ半分は
    // 書き出されないので differ ≤ 2 が許される
    let a = g.mu.sample_ram(); // :269
    let b = h.mu.sample_ram();
    let mut differ = 0usize; // :270
    let mut used = 0usize;
    for i in 0..a.len() {
        differ += (a[i] != b[i]) as usize; // :272
        used += (a[i] != 0) as usize; // :273
    }
    let same = used > 50000 && differ <= 2; // :275
    println!(
        "{} 読み戻したサンプリング RAM     使っている {} バイト、違う {} バイト", // :276
        if same { "合" } else { "NG" },
        used,
        differ
    );
    if !same {
        bad += 1; // :277
    }

    // origin: samptest.cpp:279-324 REC の InputSrc。AD1 に 440Hz、AD2 に 660Hz
    let (na, a440, a660) = record_src(&mut g, &sintab, 1); // :316 AD1 → AD2
    let ad2 = a660 > 0.05 && a440 < a660 / 20.0; // :317
    println!(
        "{} InputSrc=AD2 で AD2 だけ録る    {} サンプル、440Hz {:.4} / 660Hz {:.4}", // :318
        if ad2 { "合" } else { "NG" },
        na,
        a440,
        a660
    );
    let (nb, b440, b660) = record_src(&mut g, &sintab, 1); // :319 AD2 → AD1+2
    let both = b440 > 0.05 && b660 > 0.05; // :320
    println!(
        "{} InputSrc=AD1+2 で両方を録る     {} サンプル、440Hz {:.4} / 660Hz {:.4}", // :321
        if both { "合" } else { "NG" },
        nb,
        b440,
        b660
    );
    if !ad2 {
        bad += 1; // :322
    }
    if !both {
        bad += 1; // :323
    }

    // origin: samptest.cpp:326-344 REC の TriggerLvl。待ってから入力で録る
    g.press(Button::Exit, 80, &sintab); // :328
    g.press(Button::SelectRight, 80, &sintab); // :329
    g.press(Button::Enter, 80, &sintab); // :330
    g.press(Button::SelectRight, 80, &sintab); // :331
    for _ in 0..6 {
        g.press(Button::ValuePlus, 80, &sintab); // :332-333
    }
    expect(&g, "TriggerLvl を上げた", "TriggerLvl=06", &mut bad); // :334
    g.press(Button::SelectLeft, 80, &sintab); // :335
    g.press(Button::Enter, 80, &sintab); // :336
    g.pump(500, &sintab); // :337
    expect(&g, "入力が無いと待つ", "Waiting!", &mut bad); // :338
    g.sine_amp = 12000.0; // :339
    g.pump(500, &sintab); // :340
    expect(&g, "入力が来ると録音する", "Recording!", &mut bad); // :341
    g.press(Button::Enter, 80, &sintab); // :342
    g.sine_amp = 0.0; // :343
    g.pump(300, &sintab); // :344

    // origin: samptest.cpp:346-347 final line + exit
    println!("サンプリング: 食い違い {bad}");
    std::process::exit(if bad != 0 { 1 } else { 0 });
}
