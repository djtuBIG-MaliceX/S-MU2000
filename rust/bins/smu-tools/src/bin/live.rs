// license:BSD-3-Clause
//
// origin: src/live.cpp (592 L) — Windows の MIDI 入力を受けて、そのまま音を鳴らす.
// Ledger row `live main` (M6), replacing the session-H skeleton. Ground-truth
// seams: argument parse (:449-476), ROM load + sintab warn (:490-497), NVRAM
// before reset (:503-506) via smu-machine nvram.rs (S2 row), boot-wait
// per-sample on midi_ready (:509-523, skeleton's chunked+warn deviations CLOSED
// — RE rises in this build), the MIDI tree (:533-545) over the smu-hal-win
// winmm MIDI-in (`midi in` row, ui/midi_in.cpp), run_waveout (:252-320) over
// the smu-hal-win winmm FFI, the QPC busy accounting of `generator`
// (:132-190), Ctrl+C clean exit + NVRAM save (:65-73, :573-591).
//
// Deviations (disclosed):
// - WASAPI (run_wasapi :200-248, the default path) is the real `audio out`
//   row now (smu-hal-win wasapi.rs, ui/audio_out.cpp): event-driven
//   shared/exclusive, own sinc resampler, GetCurrentPadding stats.
//   --latency/--audio/--exclusive/--dump-dev/--raw are HONORED (:462-466 +
//   options.h:68-73). The fill bundle (Generator + Machine + sintab + debt)
//   MOVES into the audio thread (wasapi.rs header deviation) and is
//   reclaimed after stop() via take_fill; main reads produced/busy/worst/
//   cushion through the GenShared atomics at the SAME points disk races
//   gen's plain fields (:223 write, :226/:234 reads, :236-237 report).
//   Machine holds Rc<RefCell> (lib.rs:588-607) so it is not Send:
//   MachineBox carries one manual `unsafe impl Send` for the single-owner
//   move/join handoff — exactly one thread touches the machine at any
//   instant (audio thread between start and stop/join, main before/after);
//   MIDI still crosses only via the SPSC ring (midi.rs G_MIDI, global Sync).
// - --fast-midi/--native-* stay warn-ignored at parse (native engine not
//   built, AGENTS).
// - --single :472 / set_threaded :499 are LIVE (M8 `threaded slave` row):
//   default spins the slave SWP30 on its own thread (mu2000.cpp:3449-3457,
//   Machine::run_sample_pair threaded arm); the audio-out worker thread
//   (audio_out.h:210 m_thread) stays as the `audio out` row built it.
//   The slave thread is spawned BEFORE the machine moves into the audio
//   thread — its captured pointers live in the Rc'd device / ROM buffers,
//   not in the Machine struct, so the MachineBox move cannot disturb it.
// - --midi-file <mid> is a RUST-ONLY seam (ledger S3: gate live with file-fed
//   MIDI, no midisend/loopMIDI): smu_smf::load -> events fed at EXACT
//   event-sample time through the same F5/mu_port routing as render.rs:721-748
//   (render.cpp:539-556).
// - Hardware MIDI pacing: disk drains the whole SPSC ring per block
//   (:152-155, the firmware SCI queue absorbs); Rust caps the drain at the
//   31250 bps serial line — at most one byte per 44100/31250 = 14.112
//   samples, pumped in the per-sample loop like render.rs pumps events.
//   No audio-state difference; delivery is a few ms more granular.
// - NVRAM --factory prints :504 and skips load EXACTLY like disk.

use smu_compat::{paths, roms};
use smu_hal_win::midi;
use smu_hal_win::sys;
use smu_hal_win::wasapi::{AudioOut, Fill};
use smu_hal_win::waveout as wo;
use smu_machine::{nvram, Machine};
use smu_smf::{self, Event};

use std::ffi::c_void;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

// live.cpp:52 constexpr u32 RATE = 44100
const RATE: u32 = 44100;
const MIDI_PORTS: i32 = 4; // mu2000.h:108 (clamp upper bound, render.rs:49)

// ---- Ctrl+C (live.cpp:62-73) ----

// live.cpp:62-63 std::atomic<bool> g_quit / g_done
static QUIT: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

/// origin: live.cpp:65-73 on_console_ctrl
unsafe extern "system" fn on_console_ctrl(kind: u32) -> i32 {
    QUIT.store(true, Ordering::Relaxed); // :67
    // :68-71 窓を閉じられたときは…後始末を待つ (CTRL_CLOSE=2, LOGOFF=5, SHUTDOWN=6)
    if kind == 2 || kind == 5 || kind == 6 {
        for _ in 0..400 {
            if DONE.load(Ordering::Acquire) {
                break;
            }
            sys::sleep_ms(10); // :71 Sleep(10)
        }
    }
    1 // TRUE
}

/// origin: live.cpp:115-124 mmcss_guard — MMCSS "Pro Audio"
struct MmcssGuard {
    h: *mut c_void, // :116
}
impl MmcssGuard {
    fn new() -> MmcssGuard {
        let task = sys::wide("Pro Audio"); // :120 L"Pro Audio"
        let h = unsafe { sys::av_set_mm_thread_characteristics(task.as_ptr()) }; // :120-121
        MmcssGuard { h }
    }
}
impl Drop for MmcssGuard {
    fn drop(&mut self) {
        unsafe { sys::av_revert_mm_thread(self.h) }; // :123
    }
}

/// origin: live.cpp:408-424 write_wav — 出来た録音を 16bit stereo で書き出す
/// (same body as render.rs:52-90; RATE=44100 stereo s16 LE; :411 fopen fail
/// SILENT, :423 prints 録音を書き出した)
fn write_wav(path: &str, pcm: &[i16]) {
    let f = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(_) => return, // :410-412
    };
    let mut w = std::io::BufWriter::new(f);
    let bytes = (pcm.len() as u32).wrapping_mul(2); // :413 u32(pcm.size()*2)
    let u32w = |w: &mut std::io::BufWriter<std::fs::File>, v: u32| {
        let _ = w.write_all(&v.to_le_bytes()); // :414-415
    };
    let u16w = |w: &mut std::io::BufWriter<std::fs::File>, v: u16| {
        let _ = w.write_all(&v.to_le_bytes()); // :416
    };
    let _ = w.write_all(b"RIFF"); // :417
    u32w(&mut w, 36u32.wrapping_add(bytes));
    let _ = w.write_all(b"WAVE");
    let _ = w.write_all(b"fmt "); // :418-419
    u32w(&mut w, 16);
    u16w(&mut w, 1);
    u16w(&mut w, 2);
    u32w(&mut w, RATE);
    u32w(&mut w, RATE.wrapping_mul(4));
    u16w(&mut w, 4);
    u16w(&mut w, 16);
    let _ = w.write_all(b"data"); // :420
    u32w(&mut w, bytes);
    let mut buf: Vec<u8> = Vec::with_capacity(bytes as usize); // :421 fwrite(pcm...)
    for &v in pcm {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    let _ = w.write_all(&buf);
    // :422 fclose -> BufWriter flush on drop
    println!("録音を書き出した: {path}"); // :423
}

// ---- C-stdlib arg helpers (std::atoi live.cpp:459-461, std::atof :467;
// atof + base-0 parsers are render.rs:236-277, proven against strtod) ----

/// std::atoi: leading ws, [sign], digit run; trailing junk ignored.
/// C UB on overflow -> saturate (never reached by sane --frames).
fn atoi(s: &str) -> i32 {
    let t = s.trim_start();
    let (neg, d) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits: String = d
        .bytes()
        .take_while(|b| b.is_ascii_digit())
        .map(|b| b as char)
        .collect();
    if digits.is_empty() {
        return 0; // atoi("") == 0
    }
    let v = digits.parse::<i64>().unwrap_or(i64::MAX).min(i64::from(i32::MAX));
    if neg {
        (v as i32).wrapping_neg()
    } else {
        v as i32
    }
}

/// origin: render.rs:236-277 (strtod prefix parse; garbage -> 0.0)
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
            i = save;
        }
    }
    s[start..i].parse::<f64>().unwrap_or(0.0)
}

// ---- generator (live.cpp:132-190) ----

/// origin: live.cpp:132-190 struct generator — 音を作る側. The Machine +
/// sin-table + cycle-debt ride in through fill (Rust has no mu reference
/// member); everything else is disk field-for-field.
struct Generator {
    // :134 std::vector<s16> *rec — 確認用の録音。要らなければ nullptr
    rec: Option<Vec<i16>>,
    freq: i64, // :135 LARGE_INTEGER freq (QueryPerformanceFrequency, :143)
    busy_ticks: u64, // :136
    produced: u64,   // :136
    #[allow(dead_code)] // disk keeps it too (:136); only starved is reported
    late: u64,       // :136
    worst_ticks: u64, // :136
    cushion_frames: u32, // :138 取りこぼしの判定に使う「一杯ぶん」
    starved: u64,        // :139 (waveout never sets it — disk :305-312 too)
    pump: u64,           // DEVIATION: 31250-bps drain accumulator (units: bits)
    ev: Vec<Event>,      // --midi-file seam (RUST-ONLY)
    ev_next: usize,
    ev_port: i32, // render.cpp:539 port (-1 = follow SMF track)
}

impl Generator {
    /// origin: live.cpp:141-144 ctor (QueryPerformanceFrequency)
    fn new(rec: Option<Vec<i16>>, ev: Vec<Event>) -> Generator {
        Generator {
            rec,
            freq: sys::qpf(), // :143
            busy_ticks: 0,
            produced: 0,
            late: 0,
            worst_ticks: 0,
            cushion_frames: 0,
            starved: 0,
            pump: 0,
            ev,
            ev_next: 0,
            ev_port: -1,
        }
    }

    /// origin: live.cpp:147-178 fill — n サンプルぶん作って out に書く
    fn fill(&mut self, m: &mut Machine, sintab: &[u16], debt: &mut u64, out: &mut [i16]) {
        let t0 = sys::qpc(); // :149-150 QueryPerformanceCounter
        let n = out.len() / 2;
        for i in 0..n {
            // :152-155 溜まっている MIDI を音源へ (実機と同じく 31250bps).
            // DEVIATION: disk drains the ring whole per block here; we cap the
            // drain at the serial line — 31250 bits/s of byte-slots, at most
            // one byte per 44100/31250 samples, pumped per-sample like
            // render.rs:721-748 pumps SMF events.
            self.pump += 31250;
            while self.pump >= RATE as u64 {
                let mut b = 0u8;
                if midi::pop(&mut b) {
                    m.midi_in(b, 0); // :155 mu.midi_in(b) — default port 0
                    self.pump -= RATE as u64;
                } else {
                    self.pump = 0; // idle: keep no credit for a later burst
                    break;
                }
            }
            // --midi-file seam: exact event-sample time, same F5/mu_port
            // routing as render.rs:721-748 (render.cpp:541-542 F5 switch,
            // :544-545 route, :550-551 byte feed). t runs from generator start
            // = RE rise (disk takes MIDI after boot, live.cpp:524).
            let t = self.produced as f64 / RATE as f64;
            while self.ev_next < self.ev.len() && self.ev[self.ev_next].time <= t {
                let ev = &self.ev[self.ev_next];
                if ev.bytes.len() == 2 && ev.bytes[0] == 0xf5 {
                    self.ev_port = (ev.bytes[1] as i32 - 1).clamp(0, MIDI_PORTS - 1);
                } else {
                    let to = if self.ev_port >= 0 {
                        self.ev_port
                    } else {
                        smu_smf::mu_port(ev.port, true, false)
                    };
                    for &bb in ev.bytes.iter() {
                        m.midi_in(bb, to);
                    }
                }
                self.ev_next += 1;
            }
            let (l, r) = run_sample(m, sintab, debt); // :158-159 mu.run_sample
            // :160-163 DAC scale + clamp. GCC folds l*32768/(1<<17) to
            // trunc(l/4) (render.rs:291-304 ground truth); Rust / truncates
            // toward zero identically.
            out[i * 2] = to_s16(l); // :162
            out[i * 2 + 1] = to_s16(r); // :163
            self.produced += 1; // :177 produced += n (per-sample here; same sum)
        }
        let one = sys::qpc() - t0; // :166-167 QueryPerformanceCounter
        let one = one as u64;
        self.busy_ticks += one; // :168
        if one > self.worst_ticks {
            self.worst_ticks = one; // :169
        }
        // :170-173 取りこぼすのは、1 回の生成が「溜めてある量」を超えたとき
        if self.cushion_frames != 0
            && one as f64 / self.freq as f64 > self.cushion_frames as f64 / RATE as f64
        {
            self.late += 1;
        }
        if let Some(rec) = &mut self.rec {
            rec.extend_from_slice(out); // :175-176 rec->insert(...)
        }
    }

    /// origin: live.cpp:180-189 report (period_frames param is disk-dead code;
    /// the line uses cushion_frames :188 — same here with one fewer arg)
    fn report(&self) {
        report_line(
            self.freq,
            self.produced,
            self.busy_ticks,
            self.worst_ticks,
            self.cushion_frames,
            self.starved,
        );
    }
}

/// live.cpp:182-188 — the two report lines, split out so the WASAPI main
/// loop (live.cpp:236-237, where the generator lives on the audio thread)
/// prints byte-identically off the GenShared atomics
fn report_line(
    freq: i64,
    produced: u64,
    busy_ticks: u64,
    worst_ticks: u64,
    cushion_frames: u32,
    starved: u64,
) {
    let audio = produced as f64 / RATE as f64; // :182
    let busy = busy_ticks as f64 / freq as f64; // :183
    println!(
        "  {:.0} 秒経過  MIDI {} バイト  CPU 使用率 {:.1}%", // :184-185
        audio,
        midi::bytes(), // g_midi.bytes()
        100.0 * busy / audio
    );
    println!(
        "     間に合わなかった {} 回、生成の最悪 {:.1} ms（余裕は {:.1} ms）", // :186-188
        starved,
        1000.0 * worst_ticks as f64 / freq as f64,
        1000.0 * cushion_frames as f64 / RATE as f64
    );
}

/// origin: mu2000.cpp:3186-3189 + :3388 + :3414-3415 (render.rs:283-289 —
/// live.cpp's mu.run_sample (:159) is the same member seam)
fn run_sample(m: &mut Machine, sintab: &[u16], debt: &mut u64) -> (i32, i32) {
    *debt = debt.wrapping_add(28_000_000); // :3187
    let cycles = *debt / 44100; // :3188
    *debt -= cycles.wrapping_mul(44100); // :3189
    m.run_cycles(cycles); // :3388
    m.run_sample_pair(sintab) // :3414-3415 + interconnect + master DAC
}

/// live.cpp:160-163 scale+clamp, GCC-folded (render.rs:291-304)
#[inline]
fn to_s16(l: i32) -> i16 {
    (l / 4).clamp(-32768, 32767) as i16
}

// ---- run_waveout (live.cpp:252-320) ----

/// live.cpp:297-300 emit — fill THEN waveOutWrite. The refill order below is
/// the whole anti-overwrite proof (:294-296 comment): every buffer is filled
/// ONLY after WHDR_DONE rose on it (it is out of the device queue), and the
/// write goes out immediately after the fill, so a playing buffer is never
/// re-filled while queued.
fn emit(
    gen: &mut Generator,
    m: &mut Machine,
    sintab: &[u16],
    debt: &mut u64,
    hwo: *mut c_void,
    pcm: &mut [i16],
    hdr: *mut wo::WaveHdr,
) {
    gen.fill(m, sintab, debt, pcm); // :298 gen.fill(pcm[i].data(), frames)
    unsafe { wo::waveOutWrite(hwo, hdr, std::mem::size_of::<wo::WaveHdr>() as u32) }; // :299
}

/// origin: live.cpp:252-320 run_waveout — WinMM waveOut。素直だが待ち時間を詰められない
fn run_waveout(
    gen: &mut Generator,
    m: &mut Machine,
    sintab: &[u16],
    debt: &mut u64,
    seconds: f64,
    frames: i32,
    buffers: i32,
) -> i32 {
    let fr = frames as usize;
    let bufs = buffers as usize;
    // :254-260 WAVEFORMATEX PCM stereo 44100 16bit block 4
    let fmt = wo::WaveFormatEx {
        w_format_tag: 1,
        n_channels: 2,
        n_samples_per_sec: RATE,
        n_avg_bytes_per_sec: RATE.wrapping_mul(4),
        n_block_align: 4,
        w_bits_per_sample: 16,
        cb_size: 0,
    };
    // :262 auto-reset event, CALLBACK_EVENT sink
    let done = unsafe { sys::CreateEventA(std::ptr::null(), 0, 0, std::ptr::null()) };
    let mut hwo: *mut c_void = std::ptr::null_mut();
    // :264-265 waveOutOpen(WAVE_MAPPER, done, CALLBACK_EVENT)
    let r = unsafe {
        wo::waveOutOpen(
            &mut hwo,
            wo::WAVE_MAPPER,
            &fmt,
            done as usize,
            0,
            wo::CALLBACK_EVENT,
        )
    };
    if r != wo::MMSYSERR_NOERROR {
        eprintln!("音声デバイスを開けない"); // :266
        return 1; // :267
    }
    // :270 buffers x frames*2 s16
    let mut pcm: Vec<Vec<i16>> = (0..bufs).map(|_| vec![0i16; fr * 2]).collect();
    // :271-276 zeroed headers (hdr[i]={}), pinned Boxes, prepared
    let mut hdr: Vec<Box<wo::WaveHdr>> = (0..bufs)
        .map(|i| {
            Box::new(wo::WaveHdr {
                lp_data: pcm[i].as_mut_ptr() as *mut u8, // :274
                dw_buffer_length: (pcm[i].len() * 2) as u32, // :275 bytes
                dw_bytes_recorded: 0,
                dw_user: 0,
                dw_flags: 0,
                dw_loops: 0,
                lp_next: std::ptr::null_mut(),
                reserved: 0,
            })
        })
        .collect();
    for i in 0..bufs {
        unsafe { wo::waveOutPrepareHeader(hwo, &mut *hdr[i], std::mem::size_of::<wo::WaveHdr>() as u32) }; // :276
    }
    // :279-280 latency = formula from the queue depth (disk prints no measured
    // value here); :281-284 short-buffer warning; :285-288 stop hint
    println!(
        "waveOut  待ち時間 {:.1} ms（{frames} サンプル × {buffers} 枚）",
        1000.0 * fr as f64 * bufs as f64 / RATE as f64
    );
    if frames < 1024 {
        println!(
            "警告: 1 枚が {:.1} ms しかない。waveOut は 20ms 前後の間隔でしか\n\
             \x20     回収しないので、これより短いと供給が追いつかず細切れになる",
            1000.0 * fr as f64 / RATE as f64
        );
    }
    if seconds > 0.0 {
        println!("{seconds:.1} 秒で終了");
    } else {
        println!("Ctrl+C で終了");
    }
    // :290-291 TIME_CRITICAL + MMCSS on this (generator) thread
    unsafe { sys::SetThreadPriority(sys::GetCurrentThread(), sys::THREAD_PRIORITY_TIME_CRITICAL) };
    let _mmcss = MmcssGuard::new();
    gen.cushion_frames = (fr * bufs) as u32; // :292
    // :294-302 先に全枚を投入 — FIRE ALL FIRST, then wait completions in
    // issue order (:304-309). See emit()'s doc for the overwrite proof.
    for i in 0..bufs {
        emit(gen, m, sintab, debt, hwo, &mut pcm[i], &mut *hdr[i]);
    }
    let mut next = 0usize; // :304
    // :305 quit / seconds (checked BETWEEN blocks, like disk — one block may
    // overshoot the stop time)
    while !QUIT.load(Ordering::Acquire)
        && (seconds <= 0.0 || gen.produced < (seconds * RATE as f64) as u64)
    {
        // :306-307 wait for THIS buffer's DONE (never hunt for "a free one")
        while !unsafe { wo::hdr_done(&*hdr[next]) } {
            unsafe { sys::WaitForSingleObject(done, 100) };
        }
        emit(gen, m, sintab, debt, hwo, &mut pcm[next], &mut *hdr[next]); // :308
        next = (next + 1) % bufs; // :309
        if gen.produced % (RATE as u64 * 5) < fr as u64 {
            gen.report(); // :310-311
        }
    }
    // :314-318 teardown
    unsafe {
        wo::waveOutReset(hwo);
        for i in 0..bufs {
            wo::waveOutUnprepareHeader(hwo, &mut *hdr[i], std::mem::size_of::<wo::WaveHdr>() as u32);
        }
        wo::waveOutClose(hwo);
        sys::CloseHandle(done);
    }
    0 // :319
}

// ---- run_wasapi (live.cpp:200-248) over smu-hal-win wasapi.rs ----

/// live.cpp:223/:226/:234-237 — disk races the generator's PLAIN counters
/// between main and the audio thread; Rust mirrors the shape with atomics
/// (Relaxed everywhere == no ordering guarantee beyond eventual visibility,
/// exactly what the C++ plain-field races provide. every field explicit).
struct GenShared {
    freq: i64,               // :143 — immutable after ctor, read by both threads
    produced: AtomicU64,     // :136
    busy_ticks: AtomicU64,   // :136
    worst_ticks: AtomicU64,  // :136
    cushion_frames: AtomicU32, // :138 — main writes once (:223), fill reads
}

/// Machine holds Rc<RefCell> (smu-machine/src/lib.rs:588-607) so it is not
/// Send on its own. SAFETY (disclosed deviation): single-owner handoff —
/// the Machine MOVES into the audio thread at start() and comes back
/// through the join at stop()/take_fill; exactly one thread touches it at
/// any instant. While running, main only reads the GenShared atomics; MIDI
/// crosses only via the SPSC ring (midi.rs G_MIDI global, winmm callback ->
/// audio thread, the same direction as disk's g_midi).
struct MachineBox(Machine);
unsafe impl Send for MachineBox {}

/// the fill bundle that MOVES into the audio thread — disk's lambda
/// `[&gen](s16 *o, u32 n) { gen.fill(o, n); }` (live.cpp:207) stayed on
/// main's stack; Rust returns it back via take_fill after stop()
/// (wasapi.rs header deviation).
struct WasapiFill {
    gen: Generator,
    m: MachineBox,
    sintab: Vec<u16>,
    debt: u64,
    sh: Arc<GenShared>,
}

impl Fill for WasapiFill {
    /// origin: live.cpp:207 the fill lambda -> generator::fill (:147-178)
    fn fill(&mut self, out: &mut [i16]) {
        self.gen.cushion_frames = self.sh.cushion_frames.load(Ordering::Relaxed); // :223
        self.gen.fill(&mut self.m.0, &self.sintab, &mut self.debt, out); // :147
        self.sh.produced.store(self.gen.produced, Ordering::Relaxed); // :177
        self.sh.busy_ticks.store(self.gen.busy_ticks, Ordering::Relaxed); // :168
        self.sh.worst_ticks.store(self.gen.worst_ticks, Ordering::Relaxed); // :169
    }
}

/// dummy Fill, only to reach the static AudioOut::list() (audio_out.cpp:96-121)
struct NoFill;
impl Fill for NoFill {
    fn fill(&mut self, _out: &mut [i16]) {} // never called
}

/// what comes back out of the audio thread (main reads gen.produced/
/// busy_ticks/rec at live.cpp:558-559/:573 after run_wasapi returns).
/// sintab+debt are NOT returned: disk main never touches them again
/// (:573 rec, :578 nvram, :581 counters — disk's debt lives inside mu and
/// dies with main too); they drop with the bundle.
struct WasapiOut {
    rc: i32,
    gen: Generator,
    m: Machine,
}

/// origin: live.cpp:200-248 run_wasapi — WASAPI 共有モード。ui::audio_out に
/// 任せる (:194-198). Device pacing/cushion/latency all come from the HAL.
#[allow(clippy::too_many_arguments)]
fn run_wasapi(
    gen: Generator,
    m: Machine,
    sintab: Vec<u16>,
    debt: u64,
    seconds: f64,
    latency_ms: i32,
    exclusive: bool,
    dump_dev: Option<&str>,
    audio_dev: &str,
    raw: bool,
) -> WasapiOut {
    let sh = Arc::new(GenShared {
        freq: gen.freq, // the generator's own QPF (:143) shared for report_line
        produced: AtomicU64::new(0),
        busy_ticks: AtomicU64::new(0),
        worst_ticks: AtomicU64::new(0),
        cushion_frames: AtomicU32::new(0),
    });
    let mut out = AudioOut::new(); // :203 ui::audio_out out;
    if let Some(p) = dump_dev {
        out.set_capture(p); // :205-206 デバイスへ渡したものをそのまま書き出す
    }
    let mut err = String::new(); // :204
    let started = out.start(
        latency_ms,
        WasapiFill { gen, m: MachineBox(m), sintab, debt, sh: Arc::clone(&sh) },
        &mut err,
        exclusive,
        audio_dev, // :208 audio_dev ? audio_dev : ""
        raw,
    ); // :207-208
    let (rc, bundle);
    if !started {
        eprintln!("{err}"); // :209 fprintf(stderr, "%s\n", err.c_str())
        rc = 1; // :210
        bundle = out.take_fill().expect("start()==false must still return the bundle");
    } else {
        println!("音声の出口: {}", out.device_name()); // :213
        println!("{}", out.format_line()); // :214
        println!(
            "MMCSS: {}", // :215-216
            if out.mmcss() {
                "Pro Audio で登録した"
            } else {
                "登録できず（途切れやすい）"
            }
        );
        if seconds > 0.0 {
            println!("{seconds:.1} 秒で終了"); // :217-218
        } else {
            println!("Ctrl+C で終了"); // :219-220
        }
        // :222-223 取りこぼしの判定に使う「一杯ぶん」。デバイス側の長さを
        // 44100 側に直す
        sh.cushion_frames
            .store((out.target_ms() * RATE as f64 / 1000.0) as u32, Ordering::Relaxed);
        let mut shown: u64 = 0; // :225
        // :226 while (!g_quit && (seconds <= 0 || gen.produced < u64(seconds*RATE)))
        while !QUIT.load(Ordering::Acquire)
            && (seconds <= 0.0
                || sh.produced.load(Ordering::Relaxed) < (seconds * RATE as f64) as u64)
        {
            sys::sleep_ms(20); // :227
            if !out.running() {
                // :228-233
                let e = out.error(); // :229
                if !e.is_empty() {
                    eprintln!("{e}"); // :230-231
                }
                break; // :232
            }
            if sh.produced.load(Ordering::Relaxed) - shown >= (RATE as u64) * 5 {
                // :234-239 5 秒ごと: gen.starved = out.late(); gen.report(...)
                shown = sh.produced.load(Ordering::Relaxed); // :235
                report_line(
                    sh.freq,
                    sh.produced.load(Ordering::Relaxed),
                    sh.busy_ticks.load(Ordering::Relaxed),
                    sh.worst_ticks.load(Ordering::Relaxed),
                    sh.cushion_frames.load(Ordering::Relaxed),
                    out.late(), // :236
                );
                println!("     {}", out.latency_line()); // :238
            }
        }
        // :242 gen.starved = out.late() — disk keeps it only for report; the
        // two lines below read out.late()/slack_min_ms() directly
        println!("待ち時間: {}", out.latency_line()); // :243
        println!(
            "余裕の最小: {:.1} ms、間に合わなかった {} 回", // :244-245
            out.slack_min_ms(),
            out.late()
        );
        out.stop(); // :246
        rc = 0; // :247
        bundle = out.take_fill().expect("thread ended without handing the bundle back");
    }
    WasapiOut {
        rc,
        gen: bundle.gen,
        m: bundle.m.0,
    }
}

// ---- main (live.cpp:429-592) ----

fn main() {
    paths::init_console_utf8(); // live.cpp:431

    // :433-447 option state, disk defaults (frames 1024 :434, buffers 3 :435)
    let mut midi_dev: i32 = -1; // :433
    let mut frames: i32 = 1024; // :434
    let mut buffers: i32 = 3; // :435
    // :439 latency_ms 20 — 溜める目標 (disk note: Windows 最小周期 23.5ms だと
    // 音源の山で 25 秒に 1 回枯渇、30ms なら 0 回)
    let mut latency_ms: i32 = 20; // :439
    let mut exclusive = false; // :440 out_opts.exclusive (options.h:61/71)
    let mut audio_dev = String::new(); // :440 out_opts.audio_dev, "" = default
    let mut dump_dev: Option<String> = None; // :441
    let mut raw = false; // :442
    let mut seconds: f64 = 0.0; // :443 0 なら Ctrl+C まで
    let mut nomidi = false; // :444
    let mut use_waveout = false; // :444
    let mut single = false; // :444 (set_threaded(!single) LIVE — M8)
    let mut factory = false; // :445 out_opts.factory via consume_output_option
    let mut wav: Option<String> = None; // :446
    let mut dir = String::new(); // :447
    let mut midi_file: Option<String> = None; // RUST-ONLY seam (ledger S3)

    // origin: live.cpp:449-476 arg loop (argv mirror; the --native-*/fast-midi
    // engine flags stay ACCEPT-and-ignore, render.rs:14 precedent).
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0usize;
    while i < argv.len() {
        let a = argv[i].as_str();
        let has1 = i + 1 < argv.len();
        if a == "--list" {
            // :450-458 — MIDI ports via midiInGetNumDevs/CapsW (midi_in::list);
            // the audio list is ui::audio_out::list (:453-455) = wasapi.rs
            let names = midi::list();
            if names.is_empty() {
                println!("MIDI 入力が見つからない"); // :77-78
            } else {
                println!("MIDI 入力:"); // :81
                for (k, nm) in names.iter().enumerate() {
                    println!("  {k}: {nm}"); // :85
                }
            }
            println!(); // :452 "\n音声の出口:"
            println!("音声の出口:");
            let outs = AudioOut::<NoFill>::list(); // :453
            for (k, nm) in outs.iter().enumerate() {
                println!("  {k}: {nm}"); // :454-455
            }
            println!("  --audio に名前の一部を渡すと、そこへ出す"); // :456
            std::process::exit(0); // :457
        } else if a == "--midi" && has1 {
            midi_dev = atoi(&argv[i + 1]); // :459
            i += 2;
            continue;
        } else if a == "--frames" && has1 {
            frames = atoi(&argv[i + 1]); // :460
            i += 2;
            continue;
        } else if a == "--buffers" && has1 {
            buffers = atoi(&argv[i + 1]); // :461
            i += 2;
            continue;
        } else if a == "--latency" && has1 {
            latency_ms = atoi(&argv[i + 1]); // :462 std::atoi
            i += 2;
            continue;
        } else if a == "--audio" && has1 {
            // :463 consume_output_option -> options.h:73 (--audio 名前の一部)
            audio_dev = argv[i + 1].clone();
            i += 2;
            continue;
        } else if a == "--exclusive" {
            exclusive = true; // :463 options.h:71
            i += 1;
        } else if a == "--factory" {
            factory = true; // :463 HONORED (skip nvram load, print :504)
            i += 1;
        } else if a == "--nomidi" {
            nomidi = true; // :464 (disk dup at :470 — same effect)
            i += 1;
        } else if a == "--dump-dev" && has1 {
            dump_dev = Some(argv[i + 1].clone()); // :465 デバイスへ渡したものをそのまま書き出す
            i += 2;
            continue;
        } else if a == "--raw" {
            raw = true; // :466 エンジンの信号処理を飛ばす
            i += 1;
        } else if a == "--seconds" && has1 {
            seconds = atof(&argv[i + 1]); // :467
            i += 2;
            continue;
        } else if a == "--wav" && has1 {
            wav = Some(argv[i + 1].clone()); // :468
            i += 2;
            continue;
        } else if a == "--waveout" {
            use_waveout = true; // :469
            i += 1;
        } else if a == "--native-off" && has1 {
            eprintln!("(rust live: --native-off は未実装 — native エンジンは M7、無視)"); // :471
            i += 2;
            continue;
        } else if matches!(
            a,
            "--fast-midi"
                | "--native-engine"
                | "--native-fx"
                | "--native-fx-full"
                | "--cal"
                | "--nocal"
                | "--voicecache"
                | "--no-voicecache"
        ) {
            // :471 ui::consume_engine_option (no-value flags; render.rs:420-431)
            if matches!(
                a,
                "--fast-midi" | "--native-engine" | "--native-fx" | "--native-fx-full"
            ) {
                eprintln!("(rust live: {a} は未実装 — 無視)");
            }
            i += 1;
        } else if a == "--single" {
            single = true; // :472-473 (set_threaded note below, M8)
            i += 1;
        } else if a == "-v" {
            paths::set_verbose(true); // :474
            i += 1;
        } else if a == "--midi-file" && has1 {
            // RUST-ONLY seam (ledger S3 — file-fed MIDI replaces midisend/loopMIDI)
            midi_file = Some(argv[i + 1].clone());
            i += 2;
            continue;
        } else if dir.is_empty() {
            dir = argv[i].clone(); // :475 first positional = ROM dir (disk
            // takes ANY unmatched token, dash or not — mirrored exactly)
            i += 1;
        } else {
            i += 1; // disk: unmatched with dir set = silently skipped
        }
    }
    // :499 set_threaded(!single) happens at the disk seam — AFTER the ROM
    // loads (:490-497 mirror below), BEFORE nvram/reset/boot (mu2000.cpp:376
    // render / live.cpp:499 order). M8: the slave thread is real now.

    // :477-487 usage (disk strings byte-for-byte; the --waveout line is the
    // #if _WIN32 arm and we are Windows-only)
    if dir.is_empty() {
        eprint!(
            "使い方: live <rom ディレクトリ> [--midi 番号] [--latency ミリ秒] [--fast-midi]\n\
             \x20       [--exclusive]  デバイスを独り占めして待ち時間を詰める\n\
             \x20       [--factory]    覚えている設定を捨てて工場出荷状態で起動する\n\
             \x20       live <rom ディレクトリ> --waveout [--frames 数] [--buffers 数]\n\
             \x20       live --list        MIDI 入力の一覧\n"
        );
        std::process::exit(1); // :486
    }

    // ---- --midi-file (RUST-ONLY seam): load before the machine spins up so
    // a bad file fails fast like render.cpp:276-278
    let mut events: Vec<Event> = Vec::new();
    if let Some(mid) = &midi_file {
        let mut err = String::new();
        if !smu_smf::load(mid, &mut events, &mut err) {
            eprintln!("{err}"); // render.cpp:278
            std::process::exit(1);
        }
        let last = events.last().map(|e| e.time).unwrap_or(0.0); // :286-287
        println!(
            "MIDI ファイル: {} イベント、最後は {last:.2} 秒",
            events.len()
        );
    }

    // origin: live.cpp:490-497 — prog + wave fatal, sintab WARN-only. Done by
    // hand (not Machine::boot lib.rs:1964-1971, which skips the sintab and
    // resets before NVRAM could be loaded): render.rs:492-516 pattern.
    let prog = match roms::load_program(&format!("{dir}/mu2000_flash.bin")) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}"); // :491 mu.error()
            std::process::exit(1);
        }
    };
    let wave = match roms::load_wave(&format!("{dir}/dump")) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}"); // :494
            std::process::exit(1);
        }
    };
    let mut m = Machine::new(prog); // attach-before-reset (mu2000.cpp:383-393/997)
    m.wave = wave; // :493 wave bus glue (mu2000.cpp:395-402)
    let sintab: Vec<u16> = match roms::load_sintab(&format!("{dir}/standin/sin-table.bin")) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("警告: {e}"); // :496-497 警告のみ、終了コードは変えない
            Vec::new()
        }
    };
    // mu2000.cpp:413-419 set_sintab_rom pin (deferred glue) + :499
    // set_threaded(!single) — LIVE (M8 `threaded slave` row). Spawned while
    // the machine is still on this thread; the thread captures only the
    // heap-stable Rc<RefCell<Swp30>> interior + ROM pointers (see
    // SlaveRaws), so the MachineBox move into the audio thread is safe.
    m.set_sintab_pin(&sintab);
    m.set_threaded(!single); // :499

    // :500-502 SMU2000_VOICECACHE env + apply_engine_options: engine options
    // are warn-ignored above (native engine is M7, AGENTS ignores it) — no-op.
    // origin: live.cpp:503-506 — NVRAM BEFORE reset (nvram.rs:9-10 contract).
    if factory {
        println!("工場出荷状態で起動する（覚えていた設定は終わるときに上書きされる）"); // :504
    } else if nvram::load(&mut m) {
        // :505-506 設定: <path>
        println!("設定: {}", nvram::path(&m));
    }
    m.reset(); // :507

    // origin: live.cpp:509-523 — 起動を待つ. EXACT per-sample loop (skeleton's
    // chunked-check and warn-and-continue deviations CLOSED: RE rises in this
    // build, so the disk timeout is back to fatal).
    println!("起動中..."); // :510
    std::io::stdout().flush().unwrap(); // :511 fflush(stdout)
    let limit = (30.0 * RATE as f64) as usize; // :513 const size_t limit = 30*RATE
    let mut debt: u64 = m.cycle_debt; // mu2000.h:918 member debt (render.rs:586)
    let mut i = 0usize;
    while i < limit && !m.midi_ready(0) {
        // :516-517 midi_ready() == port 0 (mu2000.h:112 default)
        let (l, r) = run_sample(&mut m, &sintab, &mut debt); // :517 mu.run_sample
        let _ = (l, r); // audio discarded until the generator exists (disk too)
        i += 1;
    }
    if i >= limit {
        // :518-520 起動しなかった — fatal, like disk (skeleton deviation closed)
        eprintln!("\n起動しなかった");
        std::process::exit(1);
    }
    println!(" {:.2} 秒", i as f64 / RATE as f64); // :522

    // :525-531 native engine block: M7 (native off — nothing to do)

    // origin: live.cpp:533-545 — MIDI 入力 tree
    if nomidi {
        midi_dev = -1; // :534
    } else if midi_dev < 0 && midi::count() > 0 {
        midi_dev = 0; // :535-536 auto-open when devices exist (:103-106)
    }
    if midi_dev >= 0 {
        let mut merr = String::new();
        if !midi::open(midi_dev, &mut merr) {
            // :539-541 g_midi.open failed — exact disk line
            eprintln!("MIDI 入力 {midi_dev}: {merr}");
            std::process::exit(1);
        }
        println!("MIDI 入力: {midi_dev}: {}", midi::device_name()); // :543
    } else {
        println!("MIDI 入力なし（音は出るが何も鳴らない）"); // :545
    }

    // :547-549 rec / produced / busy_sec live in the generator
    let mut gen = Generator::new(wav.as_ref().map(|_| Vec::new()), events); // :552

    unsafe { sys::SetConsoleCtrlHandler(Some(on_console_ctrl), 1) }; // :553

    // :555-557 waveout vs WASAPI(default). The WASAPI arm MOVES the bundle
    // into the audio thread and hands it back after stop() (disk passes gen
    // by reference and races it; Rust moves and joins — same numbers at
    // :558-559 because take_fill happens after the join).
    let rc = if use_waveout {
        run_waveout(&mut gen, &mut m, &sintab, &mut debt, seconds, frames, buffers)
    } else {
        let w = run_wasapi(
            gen,
            m,
            sintab,
            debt,
            seconds,
            latency_ms,
            exclusive,
            dump_dev.as_deref(),
            &audio_dev,
            raw,
        );
        gen = w.gen;
        m = w.m;
        w.rc
    };
    let produced = gen.produced; // :558
    let busy_sec = gen.busy_ticks as f64 / gen.freq as f64; // :559

    // :573-574 録音を書き出す
    if let (Some(path), Some(rec)) = (&wav, &gen.rec) {
        if !rec.is_empty() {
            write_wav(path, rec);
        }
    }
    midi::close(); // :575 g_midi.close()

    // :577-579 音はもう止まっている。ここで機械に触ってよい
    if !nvram::save(&m) {
        eprintln!("設定を残せなかった: {}", nvram::path(&m)); // :579
    }

    // :581-587 the shared two counters
    let audio = produced as f64 / RATE as f64; // :583
    println!(
        "終了。{:.1} 秒ぶんを {:.2} 秒で生成（CPU 使用率 {:.1}%）  MIDI {} バイト", // :585-587
        audio,
        busy_sec,
        if audio > 0.0 {
            100.0 * busy_sec / audio
        } else {
            0.0
        },
        midi::bytes()
    );
    DONE.store(true, Ordering::Release); // :589 g_done — release the ctrl-c waiter
    std::process::exit(rc); // :591
}
