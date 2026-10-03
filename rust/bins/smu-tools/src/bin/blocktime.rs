// license:BSD-3-Clause
//
// origin: src/blocktime.cpp (258 L) — 1 ブロックを作るのに何 ms かかるかを測る.
// Ledger row `verify/statetest/boot/blocktime` (M8 / M3–M4 bins). Audio device は
// 使わない。argv + 起動(warm) + per-block ms(平均/最悪/中央/95/99) + 超過(N) +
// 台数(copies) の出力形を blocktime.cpp から line-for-line で transliterate する。
//
// Ground truth 呼び出し（Perf ledger baseline 2026-09-30 = interpreter）:
//   blocktime <rom> <midi> <frames> [秒] [回数] [台数]
//   dense, 512-blk, 3 回 + 慣らし. 比較の公平性のため C++ 参照は
//   SMU2000_SH2_JIT=0 SMU2000_MEG_JIT=0（全解釈）で走らせる — Rust 側は
//   元々 JIT を持たない（M9 JIT は未実装）。set_profile(true) のとき clock を
//   1 サンプル 3 回読む（mu2000.cpp:3428/3442/3464）ので、Rust も同じ条件で
//   測る（QPC オーヘッドを足さないとその分だけ見かけ上速くなる＝ optical な
//   操作になる。M8 の honest measurement）。
//
// Deviations（ledger-disclosed — 黙って "fix" しない）:
// - 逐次計装 (profiling) は MACHINE 側ではなく BIN 側で行う。Rust の Machine には
//   `m_profile` / `m_t_cpu` / `m_t_swpm` / `m_t_sh2` / `m_n_sh2` が未移植（native
//   engine 非ビルドなのでそれらを積む `mu2000::run_sample` の native 節は dead）。
//   firmware パス（native off・non-threaded・run_cpu 常に true）で意味を持つ
//   CPU / SH-2 / SWP30 master の計測点は render.rs の run_sample helper と
//   同じく `run_cycles` / `run_sample_pair` を跨いで測る（mu2000.cpp:3414-3466 の
//   firmware-only 版）。よって:
//     * m_t_ndrv / m_t_nemisc = 0（disk は m_native_engine の下でしか積まない —
//       native 非ビルドなので 0 は faithfull に正しい値）。
//     * m_t_meg (swpm()/swps() の MEG 単独時間) は swp30.cpp:4438-4443 の
//       m_profile 節で steady_clock を読む内部計装。Rust の Swp30 は未計装 →
//       0 を印字（MEG の実行自体 meg.rs で走っているが時間は測っていない）。
//     * `set_profile` / `clear_profile` (mu2000.h:971-976) は Machine に無いので
//       BIN 側の Prof を毎 rep 作り直す事で等価化（boot-wait は未計装＝disk が
//       set_profile 前に回す run_sample に対応）。
// - copies>1 の ROM 共有: C++ は set_program_rom/wave_rom/sintab_rom でポインタを
//   分け合う（blocktime.cpp:93-95）。Rust の Machine は prog/wave を所有するので
//   2 台目以降は prog/wave を clone する（Sintab は &[u16] slice を分け合う）。
//   挙動（同じ MIDI を流して合算）は同じ、メモリ使用量のみ C++ と違う。既定の
//   copies=1 では clone は一切起きない。
// - SMU2000_SINGLE / SMU2000_NATIVE_FX / SMU2000_NATIVE_ENGINE は読む（getenv を
//   反映）。SMU2000_SINGLE は LIVE（M8 `threaded slave` 行）: 既定はスレーブ
//   別糸（disk と同じ !getenv 既定）、SMU2000_SINGLE があれば（値を問わない）
//   単糸。native engine は非ビルドなので NATIVE_* は引き続き無効。
// - CRLF: Windows text-mode では CRT が stdout/stderr とも \n を \r\n に変換する
//   （ledger CRLF pitfall）。Rust stdio は byte-verbatim なので各行で \r\n を
//   明示する（この機械で C++ blocktime.exe が出る実際の出力形に一致させる）。
//   render.rs precedent: 生の dump 行で \r\n を手で足すのと同じ做法。

use smu_compat::{paths, roms};
use smu_machine::state;
use smu_machine::Machine;
use smu_smf::{self, Event};

// origin: blocktime.cpp:60 (const u32 RATE = 44100)
const RATE: u64 = 44100;

// ---- platform.h 移植（perf clock / MXCSR / QoS）---------------------------

// origin: src/compat/platform.h:103-114 — QueryPerformanceCounter (Windows)。
// x86_64-pc-windows-msvc: kernel32 は既定リンク。LARGE_INTEGER は QuadPart が
// 最初の 8 バイトなので *mut i64 で渡す（C の LONGLONG* と同値）。
#[cfg(windows)]
mod qpc {
    #[link(name = "kernel32")]
    extern "system" {
        fn QueryPerformanceCounter(lp: *mut i64) -> i32;
        fn QueryPerformanceFrequency(lp: *mut i64) -> i32;
    }
    pub fn ticks() -> u64 {
        let mut t: i64 = 0;
        unsafe { QueryPerformanceCounter(&mut t) };
        t as u64
    }
    pub fn freq() -> u64 {
        let mut f: i64 = 0;
        unsafe { QueryPerformanceFrequency(&mut f) };
        f as u64
    }
}

// origin: blocktime.cpp:114/127/151/171 smu2000::perf_ticks()
#[cfg(windows)]
#[inline]
fn perf_ticks() -> u64 {
    qpc::ticks()
}
// origin: blocktime.cpp:114 smu2000::perf_freq()
#[cfg(windows)]
fn perf_freq() -> u64 {
    qpc::freq()
}

// origin: src/compat/realtime.h:42-47 realtime_raise_self() — macOS QoS のみ、
// Windows では空。slave_loop も同じ呼び方（blocktime の comment 由来）。
#[inline]
fn realtime_raise_self() {}

// origin: src/compat/platform.h:71-96 denormals_off — FTZ(bit15)|DAZ(bit6)=0x8040
// を MXCSR に立て、span 後に戻す。エミュレータ自身の算術は整数なので
// firmware パスの計算結果は変えない（native の float 道だけに関係、非ビルド）。
// 平台.h と同じく保存して戻す（inline asm: stmxcsr/ldmxcsr、SSE2 既定）。
#[cfg(target_arch = "x86_64")]
struct DenormalsOff {
    saved: u32,
}
#[cfg(target_arch = "x86_64")]
impl DenormalsOff {
    #[inline]
    fn getcsr() -> u32 {
        let mut csr: u32 = 0;
        unsafe { core::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack, preserves_flags)) };
        csr
    }
    #[inline]
    fn setcsr(csr: u32) {
        unsafe { core::arch::asm!("ldmxcsr [{}]", in(reg) &csr, options(nostack)) };
    }
    #[allow(clippy::new_without_default)]
    fn new() -> Self {
        // platform.h:76-79 — m_saved = _mm_getcsr(); set(m_saved | 0x8040)
        let saved = Self::getcsr();
        Self::setcsr(saved | 0x8040);
        DenormalsOff { saved }
    }
}
#[cfg(target_arch = "x86_64")]
impl Drop for DenormalsOff {
    fn drop(&mut self) {
        // platform.h:82-87 — ~denormals_off: _mm_setcsr(m_saved)
        Self::setcsr(self.saved);
    }
}

// ---- C 数値変換（render.rs precedent の atof に atoi を足す）-------------

// origin: blocktime.cpp:56/58 std::atoi — 先頭空白・符号・数字のみ、非数字で停止。
// 溢出は UB（入力は frames/回数/台数の小さい正の数）。i64 に寄せてから i32 に
// truncate（wrapping 相当）。
fn c_atoi(s: &str) -> i32 {
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let mut v: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        v = v.wrapping_mul(10).wrapping_add((b[i] - b'0') as i64);
        i += 1;
    }
    if neg {
        (-(v as i64)) as i32
    } else {
        v as i32
    }
}

// origin: render.cpp:271/210 std::atof（render.rs:239-280 と同一実装）—
// 先頭空白・符号・仮数・指数。末尾ゴミは無視、空/不正は 0.0。
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
            i = save; // 指数でない: strtod は 'e' の前で止まる
        }
    }
    s[start..i].parse::<f64>().unwrap_or(0.0)
}

// origin: blocktime.cpp:30-35 median — 引数コピーして sort（入力は変えない）、
// 偶数個は中央 2 個の平均 (0.5*(v[n/2-1]+v[n/2]))。
fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else if n == 0 {
        0.0
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

// origin: blocktime.cpp:37-45 run_result — 1 回の測定の集計（ms / 1 サンプル ns）。
// r{} ゼロ初期化に対応し全フィールド明示（Invariant 3）。
#[derive(Clone, Copy)]
struct RunResult {
    mean: f64,
    mid: f64,
    p95: f64,
    p99: f64,
    worst: f64, // ms
    over: i32,
    blocks: usize,
    cpu_ns: f64,
    swpm_ns: f64,
    megm_ns: f64,
    megs_ns: f64, // 1 サンプルあたり
    sh2_ns: f64,
    ndrv_ns: f64,
    nemisc_ns: f64,
    sh2_share: f64,
    loops: f64,
}

// bin 側のプロファイル（mu2000 の m_t_* に対応。native off なので ndrv/nemisc は 0）。
// mu2000.cpp:3414-3466 の firmware-path 計測点を run_cycles / run_sample_pair 跨ぎで
// 積む。clear_profile() 相当は毎 rep 新規生成（blocktime.cpp:141）。
struct Prof {
    cpu: u64,
    swpm: u64,
    sh2: u64,
    n_sh2: u64,
    n: u64,
}
impl Prof {
    fn new() -> Self {
        Prof { cpu: 0, swpm: 0, sh2: 0, n_sh2: 0, n: 0 }
    }
}

// origin: mu2000::run_sample mu2000.cpp:3229-3466 の firmware-path 版（render.rs:286-292
// の helper と同じ cycle-debt / run_cycles / run_sample_pair の組み立て）に、
// set_profile(true) のときの計装を加えたもの。native engine OFF →
// native 節 (mu2000.cpp:3250-3424) は inert。slave-thread 節 (:3449-3457) は
// M8 から LIVE（SMU2000_SINGLE 時のみ :3458-3461 の else 節）— 計装は
// run_sample_pair を跨ぐので disk の pt1..pt2（master 実行＋待ち込み）と
// 同じ区間を測る（mu2000.cpp:3441-3466）。
// debt は machine 所有（m_cycle_debt — mu2000.h:918/:3232-3234、state leg :3545）なので
// `&mut m.cycle_debt` をそのまま使う：load_state が戻した debt を自動的に拾う。
// prof が Some のときだけ clock を読む（= disk の `if (m_profile)` 節に対応）。
#[inline]
fn run_sample(m: &mut Machine, sintab: &[u16], prof: Option<&mut Prof>) -> (i32, i32) {
    // mu2000.cpp:3232-3234 cycle-debt
    m.cycle_debt = m.cycle_debt.wrapping_add(28_000_000);
    let cycles = m.cycle_debt / RATE;
    m.cycle_debt -= cycles.wrapping_mul(RATE);

    match prof {
        Some(p) => {
            // mu2000.cpp:3441-3444 pt0 .. run_cpu 節 .. pt1 を CPU として積む。
            // native off ⇒ pt0 と :3428 の pc0 はほぼ連続（CPU 節 == SH-2 節）。
            let pt0 = perf_ticks(); // :3241
            let pc0 = perf_ticks(); // :3428
            m.run_cycles(cycles); // :3429 run_cpu 常に true
            p.sh2 += perf_ticks() - pc0; // :3430 m_t_sh2
            p.n_sh2 += 1; // :3431 m_n_sh2
            let pt1 = perf_ticks(); // :3442
            p.cpu += pt1 - pt0; // :3443 m_t_cpu
            let (l, r) = m.run_sample_pair(sintab); // :3459-3460 (master + slave)
            let pt2 = perf_ticks(); // :3464
            p.swpm += pt2 - pt1; // :3465 m_t_swpm
            p.n += 1; // :3466 m_t_n
            (l, r)
        }
        None => {
            m.run_cycles(cycles); // :3433 (非計装)
            m.run_sample_pair(sintab)
        }
    }
}

fn main() {
    paths::init_console_utf8();

    // origin: blocktime.cpp:51-60 argv 解析。argc<4 -> usage exit 1。
    let tok: Vec<String> = std::env::args().collect(); // tok[0]=program==argv[0]
    if tok.len() < 4 {
        // :52 fprintf(stderr, "blocktime <rom> <midi> <frames> [秒] [回数] [台数]\n")
        // Windows text-mode stderr -> CRLF（上の Deviations 参照）
        eprint!("blocktime <rom> <midi> <frames> [秒] [回数] [台数]\r\n");
        std::process::exit(1);
    }
    let dir = tok[1].clone(); // :55
    let mid = tok[2].clone(); // :71 smf::load(argv[2])
    let block = c_atoi(&tok[3]); // :56 atoi(argv[3])
    let seconds = if tok.len() > 4 { atof(&tok[4]) } else { 20.0 }; // :57
    let repeats = if tok.len() > 5 { std::cmp::max(1, c_atoi(&tok[5])) } else { 5 }; // :58
    let copies = if tok.len() > 6 { std::cmp::max(1, c_atoi(&tok[6])) } else { 1 }; // :59

    // :67 realtime_raise_self() — macOS QoS のみ（Windows は空）
    realtime_raise_self();

    // :69-71 smf::load
    let mut events: Vec<Event> = Vec::new();
    let mut err = String::new();
    if !smu_smf::load(&mid, &mut events, &mut err) {
        eprint!("{err}\r\n"); // :71
        std::process::exit(1);
    }

    // :73-82 マシンを作る（render.rs precedent: ROM は fatal、sintab は黙って
    // 空で続ける — blocktime.cpp は sintab の戻り値を捨てている :76）。
    let prog = match roms::load_program(&format!("{dir}/mu2000_flash.bin")) {
        Ok(p) => p,
        Err(e) => {
            eprint!("{e}\r\n"); // :74 mu.error()
            std::process::exit(1);
        }
    };
    let wave = match roms::load_wave(&format!("{dir}/dump")) {
        Ok(w) => w,
        Err(e) => {
            eprint!("{e}\r\n"); // :75
            std::process::exit(1);
        }
    };
    let mut mu = Machine::new(prog); // :73 mu2000 mu（attach-before-reset）
    mu.set_wave_rom(wave); // load_wave の bus glue（render.rs:514 と同じ；W-SAMP1 で device pin も）
    // :76 load_sintab — 戻り値を捨てる（C++ も無視。失敗時は空表で進む）
    let sintab: Vec<u16> = roms::load_sintab(&format!("{dir}/standin/sin-table.bin")).unwrap_or_default();
    // :77 set_threaded(!SMU2000_SINGLE) — LIVE (M8 `threaded slave` row).
    // getenv semantics = PRESENT (any value) means single; default threaded.
    let single = std::env::var("SMU2000_SINGLE").is_ok(); // :77 :96
    mu.set_sintab_pin(&sintab); // :76 の set_sintab_rom ピン（延期接着）
    mu.set_threaded(!single); // :77
    // :79-80 set_native_fx（SMU2000_NATIVE_FX）— native engine 非ビルドなので無効
    let _nfx = std::env::var("SMU2000_NATIVE_FX").ok();
    // :81 pc_prof_start（paths.rs:878 — compat.cpp の PC プロファイル開始）
    paths::pc_prof_start();
    mu.reset(); // :82

    // :84-85 起動を待つ（ここは測らない＝prof=None）
    let limit = 30u64 * RATE;
    let mut i = 0u64;
    while i < limit && !mu.midi_ready(0) {
        run_sample(&mut mu, &sintab, None);
        i += 1;
    }

    // :86 起動直後の状態を保存（毎回ここへ戻す）
    let booted: Vec<u8> = state::save_state(&mut mu);
    // :87 set_profile(true) — Machine 側に計装は無いので BIN 側 Prof で代用
    //     （boot-wait まで終わった今から測る）。

    // :89-107 2 台目から。ROM を分け合う…つもりが、Rust の Machine は ROM を所有
    //     するので prog/wave を clone（Sintab は slice を共有）。copies=1 では回らない。
    let mut more: Vec<Machine> = Vec::new();
    for _ in 1..copies {
        let mut m = Machine::new(prog0_clone(&dir)); // Deviation: ROM を再ロード/clone
        m.set_wave_rom(mu.wave.clone());
        // :95 set_sintab_rom（C++ は同じポインタを分け合う — Rust も同じ
        // バッファをピン）と :96 set_threaded を reset の前に
        m.set_sintab_pin(&sintab); // :95
        m.set_threaded(!single); // :96
        m.reset(); // :99
        if !state::load_state(&mut m, &booted, &mut err) {
            // :100-102
            eprint!("{err}\r\n");
            std::process::exit(1);
        }
        // :104-105 set_native_engine（SMU2000_NATIVE_ENGINE）— 非ビルドなので無効
        let _ = std::env::var("SMU2000_NATIVE_ENGINE").ok();
        more.push(m);
    }
    if copies > 1 {
        // :108-109 printf "MU2000 を %d 台、同じ MIDI で同時に回す\n"
        print!("MU2000 を {copies} 台、同じ MIDI で同時に回す\r\n");
    }

    // :111-119 定数（tick = ms/tick、span = ブロックの長さ ms、total = 総サンプル）
    let freq = perf_freq(); // :114
    let tick = 1000.0 / freq as f64; // :115
    let span = 1000.0 * block as f64 / RATE as f64; // :116
    let total = ((seconds * RATE as f64) as u64).max(1); // :117 u64(seconds*RATE)
    // :119 loop_at = 曲を繰り返す地点（最後のイベント + 0.5 秒）
    let loop_at = if events.is_empty() { 0.0 } else { events.last().unwrap().time + 0.5 };

    // :121 printf "ブロック %d フレーム（%.2f ms ぶん）× %.0f 秒 を %d 回\n"
    print!(
        "ブロック {block} フレーム（{:.2} ms ぶん）× {:.0} 秒 を {repeats} 回\r\n",
        span, seconds
    );

    // :123-126 慣らし（WARM_SECONDS=20。測らずに回してから測る）
    const WARM_SECONDS: f64 = 20.0; // :126
    let w0 = perf_ticks(); // :127
    let mut runs: Vec<RunResult> = Vec::new(); // :128
    let mut warm = 0i32; // :129

    let mut rep = 0i32; // for (rep=0; rep<repeats; rep++) の rep
    while rep < repeats {
        // :131 warming = (ticks-w0)*tick < WARM_SECONDS*1000
        let warming = ((perf_ticks() - w0) as f64) * tick < WARM_SECONDS * 1000.0;

        // :132-134 状態を毎回戻す
        if !state::load_state(&mut mu, &booted, &mut err) {
            eprint!("{err}\r\n");
            std::process::exit(1);
        }
        for m in more.iter_mut() {
            if !state::load_state(m, &booted, &mut err) {
                eprint!("{err}\r\n");
                std::process::exit(1);
            }
        }
        // :135-140 native の口を入れ直す（非ビルドなので無効）— env は読む
        let _ = std::env::var("SMU2000_NATIVE_ENGINE").ok();
        // :141 clear_profile — Prof を毎回新規（loops は machine -global なので窓の
        //     先頭を snapshot して差分で 1 サンプルあたりを出す）
        let mut prof = Prof::new();
        let loops_base = mu.loops; // m_loops (:1169) — run_cycles 内で ++

        // :143-144 denormals_off（per-block/thread の MXCSR FTZ/DAZ — 整数道は無関係）
        let _no_denormals = DenormalsOff::new();

        let mut ms: Vec<f64> = Vec::new(); // :145
        let mut next = 0usize; // :146
        let mut done = 0u64; // :147
        let mut base = 0.0f64; // :148
        while done < total {
            let n = std::cmp::min(block as u64, total - done) as usize; // :150
            let t0 = perf_ticks(); // :151
            for s in 0..n {
                // :153 t = (done+i)/RATE - base
                let t = (done + s as u64) as f64 / RATE as f64 - base;
                // :154-161 時間になったイベントを流す（port!=0 -> 1, else 0）
                while next < events.len() && events[next].time <= t {
                    let port = if events[next].port != 0 { 1 } else { 0 }; // :156
                    // events は mutable borrow しないので iter を直接回せる（C++ の
                    // `for (u8 b : events[next].bytes)` と同じ。hot loop で alloc なし）
                    for &b in events[next].bytes.iter() {
                        mu.midi_in(b, port); // :156
                        for m in more.iter_mut() {
                            m.midi_in(b, port); // :157-158
                        }
                    }
                    next += 1; // :160
                }
                // :162 ループ地点で巻き戻る
                if loop_at > 0.0 && t >= loop_at {
                    next = 0;
                    base = (done + s as u64) as f64 / RATE as f64;
                }
                // :163-169 マスタ + 各台を 1 サンプル（マスタだけ計装）
                let (mut l, mut r) = run_sample(&mut mu, &sintab, Some(&mut prof));
                for m in more.iter_mut() {
                    let (l2, r2) = run_sample(m, &sintab, None);
                    l = l.wrapping_add(l2); // :168 l += l2（s32 wrap）
                    r = r.wrapping_add(r2); // :168
                }
            }
            let t1 = perf_ticks(); // :171
            ms.push((t1 - t0) as f64 * tick); // :172
            done += n as u64; // :173
        }

        // :176-186 集計
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pct = |p: f64| -> f64 { ms[(p * (ms.len() - 1) as f64) as usize] }; // :177
        let mut sum = 0.0f64;
        let mut over = 0i32;
        for &v in ms.iter() {
            sum += v;
            if v > span {
                over += 1; // :180 v>span 超過
            }
        }

        let mut r = RunResult {
            mean: sum / ms.len() as f64, // :183
            mid: pct(0.5),              // :184
            p95: pct(0.95),
            p99: pct(0.99),
            worst: *ms.last().unwrap(), // :184 ms.back()
            over,                       // :185
            blocks: ms.len(),           // :186
            cpu_ns: 0.0,
            swpm_ns: 0.0,
            megm_ns: 0.0,
            megs_ns: 0.0,
            sh2_ns: 0.0,
            ndrv_ns: 0.0,
            nemisc_ns: 0.0,
            sh2_share: 0.0,
            loops: 0.0,
        };
        // :187-198 if (mu.m_t_n) — Rust では prof.n > 0 が同じ条件
        if prof.n > 0 {
            let n = prof.n as f64;
            let fd = freq as f64;
            r.cpu_ns = 1e9 * prof.cpu as f64 / fd / n; // :189
            r.swpm_ns = 1e9 * prof.swpm as f64 / fd / n; // :190
            // :191-192 swpm().m_t_meg / swps().m_t_meg — Rust の Swp30 は未計装 -> 0
            r.megm_ns = 0.0;
            r.megs_ns = 0.0;
            r.sh2_ns = 1e9 * prof.sh2 as f64 / fd / n; // :193
            r.ndrv_ns = 0.0; // :194 native 非ビルド -> 0（disk も native 下でしか積まない）
            r.nemisc_ns = 0.0; // :195 同上
            r.sh2_share = prof.n_sh2 as f64 / n; // :196（native off ⇒ 常に 1.0）
            r.loops = (mu.loops - loops_base) as f64 / n; // :197 m_loops/n
        }
        // :199-201 1 行目 — ラベルは慣らし / "%d 回目"
        let label = if warming {
            "慣らし ".to_string() // :200 "慣らし "（末尾の空白込み）
        } else {
            format!("{} 回目", rep + 1) // :200 to_string(rep+1)+" 回目"
        };
        print!(
            "  {label}  平均 {:.3} ms  最悪 {:.2}  超過 {over}  | CPU {:.0} ns  SWP30 {:.0} ns（うち MEG {:.0}）  スレーブの MEG {:.0} ns\r\n",
            r.mean, r.worst, r.cpu_ns, r.swpm_ns, r.megm_ns, r.megs_ns
        );
        // :202 fflush(stdout) — Rust の stdout は行単位で flush されるが明示
        let _ = std::io::Write::flush(&mut std::io::stdout());

        if warming {
            warm += 1; // :204
            // :205 rep--; の代わり：rep を進めない（loop の ++ を相殺）
            continue;
        }
        runs.push(r); // :208
        rep += 1; // for ループの rep++
    }

    // :211-220 col / spread（run_result のメンバ列を Vec<f64> に / 幅%）
    let col = |get: fn(&RunResult) -> f64| -> Vec<f64> {
        runs.iter().map(|r| get(r)).collect()
    };
    let spread = |v: &Vec<f64>| -> f64 {
        // :216-220 minmax + median、m>0 なら 100*(hi-lo)/m else 0
        if v.is_empty() {
            return 0.0;
        }
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &x in v.iter() {
            if x < lo {
                lo = x;
            }
            if x > hi {
                hi = x;
            }
        }
        let m = median(v.clone());
        if m > 0.0 {
            100.0 * (hi - lo) / m
        } else {
            0.0
        }
    };

    // :222-225 中央値集計 + 超過の最大
    let means = col(|r| r.mean);
    let mean = median(means.clone());
    let mut over_max = 0i32;
    for r in runs.iter() {
        over_max = std::cmp::max(over_max, r.over); // :225
    }

    // :227 printf "中央値（%d 回。先に慣らしを %d 回捨てた）\n"
    print!("中央値（{repeats} 回。先に慣らしを {warm} 回捨てた）\r\n");
    // :228-230 printf "  平均 %.3f ms（回ごとの幅 %.1f%%）  中央 %.2f  95%% %.2f  99%% %.2f  最悪 %.2f ms\n"
    print!(
        "  平均 {:.3} ms（回ごとの幅 {:.1}%）  中央 {:.2}  95% {:.2}  99% {:.2}  最悪 {:.2} ms\r\n",
        mean,
        spread(&means),
        median(col(|r| r.mid)),
        median(col(|r| r.p95)),
        median(col(|r| r.p99)),
        median(col(|r| r.worst)),
    );
    // :231-233 printf "  実時間に対する割合: 平均 %.1f%%  最悪 %.0f%%%s\n"
    let tail = if copies > 1 { "（全部の台を合わせて）" } else { "" };
    print!(
        "  実時間に対する割合: 平均 {:.1}%  最悪 {:.0}%{tail}\r\n",
        100.0 * mean / span,
        100.0 * median(col(|r| r.worst)) / span,
    );
    // :234-236 printf "  1 台あたり: 平均 %.1f%%  → この機械で実時間に入るのは %d 台まで\n"
    if copies > 1 {
        print!(
            "  1 台あたり: 平均 {:.1}%  → この機械で実時間に入るのは {} 台まで\r\n",
            100.0 * mean / span / copies as f64,
            (span * copies as f64 / mean) as i32,
        );
    }
    // :237 printf "  ブロックの長さを超えた回数: 多い回で %d / %zu\n"
    print!("  ブロックの長さを超えた回数: 多い回で {over_max} / {}\r\n", runs[0].blocks);
    // :238-255 if (mu.m_t_n) — prof を測れた（repeats>=1 で毎回測る）とき
    if prof_any(&runs) {
        // :239-243 printf "  1 サンプルあたり: CPU %.0f ns / SWP30 マスタ %.0f ns（うち MEG
        //                 %.0f ns、幅 %.1f%%） / スレーブの MEG %.0f ns（別糸）\n"
        let megm = col(|r| r.megm_ns);
        print!(
            "  1 サンプルあたり: CPU {:.0} ns / SWP30 マスタ {:.0} ns（うち MEG {:.0} ns、幅 {:.1}%） / スレーブの MEG {:.0} ns（別糸）\r\n",
            median(col(|r| r.cpu_ns)),
            median(col(|r| r.swpm_ns)),
            median(megm.clone()),
            spread(&megm),
            median(col(|r| r.megs_ns)),
        );
        // :244 printf "  実行ループ %.1f 周 / サンプル\n"
        print!("  実行ループ {:.1} 周 / サンプル\r\n", median(col(|r| r.loops)));
        // :245-254 printf "    CPU の内訳: SH-2 %.0f ns（回したのは %.1f%% のサンプル） /
        //                 native の tick %.0f ns / そのほかの面倒 %.0f ns / 時計と残り %.0f ns\n"
        let sh2 = median(col(|r| r.sh2_ns));
        let ndrv = median(col(|r| r.ndrv_ns));
        let nemisc = median(col(|r| r.nemisc_ns));
        let share = median(col(|r| r.sh2_share));
        let cpu_med = median(col(|r| r.cpu_ns));
        print!(
            "    CPU の内訳: SH-2 {:.0} ns（回したのは {:.1}% のサンプル） / native の tick {:.0} ns / そのほかの面倒 {:.0} ns / 時計と残り {:.0} ns\r\n",
            sh2,
            100.0 * share,
            ndrv,
            nemisc,
            cpu_med - sh2 - ndrv - nemisc,
        );
    }
    // :256 pc_prof_report
    paths::pc_prof_report();
    // :257 return 0
}

// :89-95 の ROM 共有の代わり（Machine が prog を所有するので 2 台目は prog を
// 再ロード）。copies=1 では呼ばれない。dir は load_program の path。
fn prog0_clone(dir: &str) -> Vec<u8> {
    roms::load_program(&format!("{dir}/mu2000_flash.bin")).unwrap_or_default()
}

// mu.m_t_n 相当：測れた rep が 1 つでもあるか（repeats>=1 では常に真）。
fn prof_any(runs: &[RunResult]) -> bool {
    runs.iter().any(|r| r.cpu_ns > 0.0 || r.swpm_ns > 0.0 || r.blocks > 0)
}
