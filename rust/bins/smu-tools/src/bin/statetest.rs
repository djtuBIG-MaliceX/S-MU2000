// license:BSD-3-Clause
//
// 状態の保存と復元が正しいかを確かめる。
//
//   statetest <rom ディレクトリ> [<MIDI ファイル>] [--warm 秒] [--steps 数]
//
// やること
//
//   1. 起動して MIDI を流し、warm 秒ぶん進める
//   2. そこで状態を保存する
//   3. まっさらな機械を作り、その状態を読み戻す
//   4. **両方を 1 サンプルずつ同じだけ進めて、状態を突き合わせる**
//
// 写し忘れた状態が 1 つでもあれば、そこから先がずれる。ずれた場所の
// 直前の目印を出すので、どの区画が足りないかがすぐ分かる。
//
// 音で比べるより先に状態で比べるのが要点。音は最後の出口なので、
// ずれても原因の場所が分からない。
//
// MIDI を流したあとで止めるのも要点。無音のまま比べても、エンベロープも
// フィルタも動いていないので何も見つからない。
//
// origin: src/statetest.cpp (237 L) — ledger row M5 `state serializer` W4.
// stdout/stderr carry CRLF on Windows (text-mode CRT, boot.rs:17-19 pitfall);
// every printf string byte-verbatim, CJK included.
//
// Deviations (disclosed; the two-machine STATE comparison itself is exact):
// - set_threaded(false) (:57) — cited no-op: the Rust machine is
//   single-threaded (run_sample_pair == the :3413-3416 else-arm).
// - set_native_fx (:60-61) — cited no-op: the native FX engine is not
//   built (AGENTS). The env var IS still READ (:227) exactly like disk:
//   it only gates the mismatch-escape printf + `return 0` at :227-229;
//   with the engine absent no native DSP exists, so a machine never
//   diverges *because* of it and the escape stays unreached.
// - load_sintab/load_lcd_font path names mirror boot (:54/:55-56); the
//   LCD font load is omitted exactly like the render.rs port (:292-300) —
//   the CGROM is never written, identically on BOTH machines, and the
//   firmware render path does not read glyph bytes (63/63 render parity).
// - midi_in(b) uses disk's default port 0 (mu2000.h:140).

use smu_compat::{paths, roms, state_pack, state_unpack};
use smu_machine::{state, Machine};
use smu_smf::{self, Event};

// origin: statetest.cpp:37 constexpr u32 RATE = 44100
const RATE: usize = 44100;

// stdout/stderr are CRLF-translated by the C++ CRT (see header pitfall)
const EOL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

// USB の口で起こすか（--usb）。HOST SELECT が USB のときしか動かない所を
// 突き合わせるため。これを入れるまで、2 つ目の A/D 変換器（AN4 = HOST SELECT）の
// 写し忘れに気付けなかった（issue #18）
// origin: statetest.cpp:39-42 — g_usb_host (main-local here, threaded through boot)
struct Globals {
    usb_host: bool,
}

// origin: statetest.cpp:44-70 boot(mu2000 &mu, const std::string &dir)
// DEVIATION (see header): no set_threaded/set_native_fx side effects; the
// ROM loads mirror render.rs:479-503 (load_program/load_wave fatal,
// load_sintab warning-only like render.cpp:299-300 — sintab is loaded by
// the caller once and threaded through, like render.rs:497 feeds run_sample).
fn boot(mu: &mut Machine, dir: &str, g: &Globals, sintab: &[u16]) -> bool {
    // :46-49 load_program — done by new_machine BEFORE Machine::new (the
    // Rust machine attaches the program bus at construction,
    // mu2000.cpp:383-393/997 == attach-before-reset); same loader, same
    // stderr line, once.
    // :50-53 load_wave(dir + "/dump")
    let wave = match roms::load_wave(&format!("{dir}/dump")) {
        Ok(w) => w,
        Err(e) => {
            eprint!("{e}{EOL}"); // :51
            return false;
        }
    };
    mu.wave = wave; // the Rust wave bus seam (render.rs:494)
    // :54 load_sintab(dir + "/standin/sin-table.bin") — loaded by the
    // caller (sintab arg): warning-only semantics == render.rs:497-503,
    // and identical standin on BOTH machines either way.
    // :55-56 load_lcd_font(hd44780u_b04.bin) / standin fallback — OMITTED
    // exactly like render.rs (both machines never get a CGROM; equal).
    // :57 set_threaded(false) — cited no-op (single-threaded build)
    // :58-61 SMU2000_NATIVE_FX getenv + set_native_fx(atoi(e)) — the env
    // READ happens in main (:227 path is the only OBSERVABLE use);
    // set_native_fx itself is a cited no-op (engine not built).
    // :62 set_usb_host(g_usb_host) — **reset() の前に**. Rust seam:
    // Machine::midi.usb_host (M7 flag row pins false; --usb sets it).
    mu.midi.usb_host = g.usb_host; // :62 BEFORE reset()
    mu.reset(); // :63

    // :65-69 const size_t limit = 30.0 * RATE; run_sample until midi_ready
    let limit = (30.0 * RATE as f64) as usize; // :65
    let mut i = 0usize;
    while i < limit && !mu.midi_ready(0) {
        // :67 mu.run_sample(l, r) — midi_ready() default port 0 (mu2000.h:119)
        run_sample(mu, sintab);
        i += 1;
    }
    true
}

// origin: render.rs:276-286 run_sample (mu2000.cpp:3186-3189 cycle-debt +
// run_cycles + run_sample_pair). Debt rides the MACHINE field now
// (m.cycle_debt, mu2000.h:925) so it is the same object disk's run_sample
// updates (and future state legs can stream); DAC outputs are dropped
// exactly like disk's throwaway `s32 l, r` sinks (statetest.cpp:66/82).
fn run_sample(m: &mut Machine, sintab: &[u16]) {
    m.cycle_debt = m.cycle_debt.wrapping_add(28_000_000); // mu2000.cpp:3187
    let cycles = m.cycle_debt / 44100; // :3188
    m.cycle_debt -= cycles.wrapping_mul(44100); // :3189
    m.run_cycles(cycles); // mu2000.cpp:3388 (run_cpu -> run_cycles)
    m.run_sample_pair(sintab); // :3414-3415
}

// origin: statetest.cpp:72-86 advance(mu2000 &mu, size_t n, events, at, clock)
// n サンプルぶん進める。events はそのあいだに流す MIDI
fn advance(
    mu: &mut Machine,
    n: usize,
    events: &[Event],
    at: &mut usize,
    clock: &mut f64,
    sintab: &[u16],
) {
    for _ in 0..n {
        // :77-81 while (at < events.size() && events[at].time <= clock)
        while *at < events.len() && events[*at].time <= *clock {
            for b in events[*at].bytes.iter() {
                mu.midi_in(*b, 0); // :79 mu.midi_in(b) — default port 0
            }
            *at += 1;
        }
        run_sample(mu, sintab); // :83 mu.run_sample(l, r)
        *clock += 1.0 / RATE as f64; // :84
    }
}

// origin: statetest.cpp:88-109 report_where — ずれた場所の直前にある目印を探す
fn report_where(blob: &[u8], at: usize) {
    // :91-95 tags[] — verbatim
    const TAGS: [&str; 22] = [
        "mu2000", "mach", "shcore", "sh2", "sh7042", "intc", "adc", "bsc", "cmt", "dmac", "dmach",
        "mtu", "mtuch", "port16", "port32", "sci", "swp30", "meg", "lcd", "sci4", "panel", "midi",
    ];
    let mut last = "（無し）".to_string(); // :96
    let mut last_at: usize = 0; // :97
    // :98 for (i = 0; i + 8 <= at && i + 8 <= blob.size(); i++)
    let mut i = 0usize;
    while i + 8 <= at && i + 8 <= blob.len() {
        for t in TAGS.iter() {
            // :100-101 char buf[8] = {}; strncpy(buf, t, 7)
            let mut buf = [0u8; 8];
            let nb = t.as_bytes();
            let n = nb.len().min(7);
            buf[..n].copy_from_slice(&nb[..n]);
            // :102 memcmp(&blob[i], buf, 8)
            if blob[i..i + 8] == buf {
                last_at = i; // :103-104
                last = t.to_string();
            }
        }
        i += 1;
    }
    // :107-108 printf("  直前の目印: %s（%zu 番目、そこから %zu バイト先）\n")
    print!("  直前の目印: {last}（{last_at} 番目、そこから {} バイト先）{EOL}", at - last_at);
}

// origin: statetest.cpp:124/132 std::atof — same port as render.rs:233-274
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

// origin: std::atoi (strtol clamped to int): ws/sign/digits, saturate like
// C on overflow (UB-free substitute; --steps values are small)
fn atoi(s: &str) -> i32 {
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let neg = if i < b.len() && b[i] == b'-' {
        i += 1;
        true
    } else {
        if i < b.len() && b[i] == b'+' {
            i += 1;
        }
        false
    };
    let mut v: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        let d = (b[i] - b'0') as i64;
        if v > (i64::MAX - d) / 10 {
            v = i64::MAX; // saturate (strtol LONG_MAX; int clamp below)
        } else {
            v = v * 10 + d;
        }
        i += 1;
    }
    if neg {
        (if v > 2147483648 { 2147483648 } else { v }).clamp(-2147483648, 2147483647) as i32
    } else {
        v.min(2147483647).clamp(-2147483648, 2147483647) as i32
    }
}

// origin: statetest.cpp:213 printf("  %zu-%zu（%zu バイト）\n", from, i - 1, i - from)
fn range_bytes(from: usize, to: usize) -> String {
    // `i - 1` is never evaluated with i == 0 on disk (a mismatch always
    // sets from = i BEFORE i advances past it); saturating_sub is the
    // panic-free mirror of that unreachable edge.
    format!("  {from}-{}（{} バイト）", to - 1, to - from)
}

fn main() {
    paths::init_console_utf8(); // :116
    // :117 setvbuf(stdout, nullptr, _IONBF, 0) — byte-identical output;
    // Rust stdout buffering cannot reorder a single-stream sequence.

    // :120-129 argv parse
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = String::new(); // :120
    let mut mid = String::new(); // :120
    let mut warm = 3.0f64; // :121
    let mut steps: i32 = 400; // :122
    let mut g = Globals { usb_host: false }; // :42 g_usb_host
    let mut i = 0usize;
    while i < raw.len() {
        let a = raw[i].as_str();
        if a == "--warm" && i + 1 < raw.len() {
            warm = atof(&raw[i + 1]); // :124 std::atof
            i += 2;
        } else if a == "--steps" && i + 1 < raw.len() {
            steps = atoi(&raw[i + 1]); // :125 std::atoi
            i += 2;
        } else if a == "--usb" {
            g.usb_host = true; // :126
            i += 1;
        } else if dir.is_empty() {
            dir = raw[i].clone(); // :127
            i += 1;
        } else if mid.is_empty() {
            mid = raw[i].clone(); // :128
            i += 1;
        } else {
            i += 1; // disk: no branch matches -> skip
        }
    }
    if dir.is_empty() {
        // :130-134 usage (stderr)
        eprint!(
            "使い方: statetest <rom ディレクトリ> [<MIDI ファイル>]"
        );
        eprint!(" [--warm 秒] [--steps 数] [--usb]{EOL}");
        std::process::exit(1); // :134
    }

    // :137-145 smf::load
    let mut events: Vec<Event> = Vec::new();
    if !mid.is_empty() {
        let mut err = String::new();
        if !smu_smf::load(&mid, &mut events, &mut err) {
            eprint!("{err}{EOL}"); // :141
            std::process::exit(1); // :142
        }
        print!("MIDI {} 件{EOL}", events.len()); // :144 "MIDI %zu 件\n"
    }

    // sintab for both machines (statetest.cpp:54; loader + warning-only
    // fallback == render.rs:497-503)
    let sintab: Vec<u16> = match roms::load_sintab(&format!("{dir}/standin/sin-table.bin")) {
        Ok(t) => t,
        Err(_) => Vec::new(),
    };

    // ---- 1. 起動して warm 秒 (:147)
    print!("起動中...{EOL}"); // :148 "起動中...\n"
    // :149-151 **積み場には置かない**。mu2000 は 2 台ぶんが積み場に収まらず、
    // 何も出さずに落ちる -> make_unique = Box here (the Rust Machine is
    // heap-allocated like disk's unique_ptr; Invariant-3 ctor).
    let mut one_p = match new_machine(&dir, &g, &sintab) {
        Some(m) => m,
        None => std::process::exit(1), // :153-154 boot failure
    };
    let one = &mut *one_p;
    let mut at: usize = 0; // :155
    let mut clock: f64 = 0.0; // :156
    // :157 advance(one, size_t(warm * RATE), events, at, clock)
    advance(one, (warm * RATE as f64) as usize, &events, &mut at, &mut clock, &sintab);

    // ---- 2. 保存 (:159)
    let saved = state::save_state(one); // :160 one.save_state()
    // :161 "保存した: %zu バイト（%.2f 秒のところ）\n"
    print!("保存した: {} バイト（{warm:.2} 秒のところ）{EOL}", saved.len());
    {
        // :162-171 詰めた形も往復できることを見ておく。DAW に入れるのはこちら
        let packed = state_pack(&saved); // :164
        let ok = matches!(state_unpack(&packed), Ok(ref back) if *back == saved); // :166-167
        // :168-170 "詰めると %zu バイト（%.1f%%）。戻し: %s\n"
        print!(
            "詰めると {} バイト（{:.1}%）。戻し: {}{EOL}",
            packed.len(),
            100.0 * packed.len() as f64 / saved.len() as f64,
            if ok { "一致" } else { "だめ" } // :170
        );
    }

    // ---- 3. まっさらな機械へ読み戻す (:173)
    let mut two_p = match new_machine(&dir, &g, &sintab) {
        Some(m) => m,
        None => std::process::exit(1), // :176-177 boot failure
    };
    let two = &mut *two_p;
    let mut err = String::new(); // :178
    if !state::load_state(two, &saved, &mut err) {
        // :180 "読み戻せない: %s\n"
        eprint!("読み戻せない: {err}{EOL}");
        std::process::exit(1); // :181
    }
    {
        // :183-195
        let again = state::save_state(two); // :184
        if again == saved {
            print!("戻した直後の状態は一致{EOL}"); // :186
        } else {
            let mut d = 0usize; // :188
            while d < again.len() && d < saved.len() && again[d] == saved[d] {
                d += 1;
            }
            print!("戻した直後から食い違う。{d} 番目{EOL}"); // :191
            report_where(&saved, d); // :192
            std::process::exit(1); // :193
        }
    }

    // ---- 4. 両方を同じだけ進めて、1 サンプルごとに突き合わせる (:197)
    let mut at1 = at; // :198
    let mut at2 = at;
    let mut c1 = clock; // :199
    let mut c2 = clock;
    let mut k: i32 = 1;
    while k <= steps {
        advance(one, 1, &events, &mut at1, &mut c1, &sintab); // :201
        advance(two, 1, &events, &mut at2, &mut c2, &sintab); // :202
        let x = state::save_state(one); // :203
        let y = state::save_state(two);
        if x == y {
            k += 1;
            continue; // :204-205
        }
        let mut d = 0usize; // :206-208
        while d < x.len() && d < y.len() && x[d] == y[d] {
            d += 1;
        }
        // :209 "---- %d サンプル目でずれた。%zu 番目から ----\n"
        print!("---- {k} サンプル目でずれた。{d} 番目から ----{EOL}");
        {
            // :210-223 違っているところを固まりごとに並べる。どこが本命かを見る
            let mut shown = 0i32; // :212
            let mut j = 0usize;
            while j < x.len() && j < y.len() && shown < 8 {
                if x[j] == y[j] {
                    j += 1; // :215
                    continue;
                }
                let from = j; // :216
                while j < x.len() && j < y.len() && x[j] != y[j] {
                    j += 1; // :217-218
                }
                print!("{}{EOL}", range_bytes(from, j)); // :219
                report_where(&x, from); // :220
                shown += 1; // :221
            }
        }
        // 軽量モード（C++ のエフェクト）では、DSP の中身を状態に入れていないので
        // **ずれて当たり前**（doc/native-dsp.md「機械まるごとの状態には入らない」）。
        // ここでは「2 台が同時に軽量モードで動いても落ちない」ことだけを見る
        // :224-230 — env READ is FAITHFUL (see header deviation note)
        if std::env::var("SMU2000_NATIVE_FX").is_ok() {
            // :228
            print!("軽量モードなので、ここのずれは想定どおり（DSP の中身は状態に入れていない）{EOL}");
            std::process::exit(0); // :229
        }
        print!("写し忘れている状態がある{EOL}"); // :231
        std::process::exit(1); // :232
    }

    // :235 "---- %d サンプル進めても状態は完全に一致 ----\n"
    print!("---- {steps} サンプル進めても状態は完全に一致 ----{EOL}");
    // :236 return 0
}

// origin: statetest.cpp:149-153 / :174-177 — make_unique<mu2000> + boot().
// Box (積み場 note :149-150) around the explicit-invariant Machine; boot()
// prints its stderr line exactly like the C++ and yields exit(1) upstream.
fn new_machine(dir: &str, g: &Globals, sintab: &[u16]) -> Option<Box<Machine>> {
    // boot() loads the program itself (disk :46 is INSIDE boot); Machine::new
    // needs it at construction (attach-before-reset), so load first, then
    // boot — the same load order, the same single stderr line on failure.
    let prog = match roms::load_program(&format!("{dir}/mu2000_flash.bin")) {
        Ok(p) => p,
        Err(e) => {
            eprint!("{e}{EOL}"); // :47 (boot()'s mu.error())
            return None;
        }
    };
    let mut m = Box::new(Machine::new(prog)); // :151 make_unique
    if !boot(&mut m, dir, g, sintab) {
        return None; // boot() already emitted the stderr line
    }
    Some(m)
}
