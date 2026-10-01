// license:BSD-3-Clause
//
// origin: src/verify.cpp (whole file, 43 L) — 移植が成立しているかの最小確認.
//
// Deviations (ledger row `verify/statetest/boot/blocktime`; do not "fix" silently):
// - :14-15 `set_wave_rom`/`set_sintab` (pointer stash) are the paired smu-swp30
//   seam: `Wave`/`sintab` are passed at call time to `run_sample` (:26).
//   The 64 MiB zero wave (:10) is still allocated verbatim (pow2 mask matters
//   to `Wave::new`, mamecompat.h:141); fresh voices never read non-zero bytes.
// - :37 `meg_jit_selftest`: the x86-64 C++ ground-truth build DOES run the
//   sweep (SMU2000_MEG_JIT=1 via swp30_jit.cpp:30-31 — emitted machine code
//   vs `meg_state::revram_encode/revram_decode/m1_expand`). The JIT is not
//   ported (M9), so the "machine-code side" here is an INDEPENDENT
//   re-transliteration of the three C++ functions (swp30.cpp:2428-2441,
//   :2443-2455, :3502-3509, re-read from disk) compared against the paired
//   `MegState` implementations over the identical input ranges (:555-557
//   0..0x8000_0000 + :558-560 specials, :562-564 0..0x1_0000, :574-576
//   -0x8000..0x8000 s64) with the identical `& 0xffff` masks (:556/:559)
//   and mismatch counter. Interpreter-vs-interpreter; both sides are
//   bit-exact C++-harness-paired, so the honest expected print is still 0.
// - :41 `a64::selftest()` returns 0 on x86-64 builds by definition
//   (a64asm.cpp:946-950 `#else ... return 0`); the emitter + ARM JIT are
//   dead on x86 (AGENTS-ignored). Literal 0, sweep not ported.
// - stdout written with CRLF on Windows (text-mode fprintf, ledger CRLF
//   pitfall; boot.rs:24 precedent).

use smu_compat::timers::RunningMachine;
use smu_swp30::fetch::Wave;
use smu_swp30::meg::MegState;
use smu_swp30::Swp30;

const EOL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// Re-transliteration of `meg_state::revram_encode` (swp30.cpp:2428-2441,
/// disk re-read) — selftest "JIT side", deliberately NOT a meg.rs alias.
fn ref_revram_encode(mut v: u32) -> u16 {
    v &= 0x7ff_ffff; // :2430
    let mut s = 0u32; // :2431
    if v & 0x400_0000 != 0 {
        // :2432-2435 sign bit = bit 26: complement the 27-bit value
        v ^= 0x7ff_ffff; // :2433
        s = 1; // :2434
    }
    let mut e = 15u32; // :2436
    while e != 0 && (v & (0x400 << e)) == 0 {
        e -= 1; // :2437-2438
    }
    let m = if e != 0 { (v >> (e - 1)) & 0x7ff } else { v }; // :2439
    ((e << 12) | (s << 11) | m) as u16 // :2440
}

/// Re-transliteration of `meg_state::revram_decode` (swp30.cpp:2443-2455),
/// S-MU2000 e==0 full-width fix (:2449-2453) kept verbatim.
fn ref_revram_decode(v: u16) -> u32 {
    let e = ((v >> 12) & 15) as u32; // :2445
    let s = ((v >> 11) & 1) as u32; // :2446
    let m = (v & 0x7ff) as u32; // :2447
    let mut vb = if e != 0 { (m | 0x800) << (e - 1) } else { m }; // :2448
    if s != 0 {
        // :2452-2453 (e-1 <= 14, both sides in range; outer mask is the C++ no-op)
        vb ^= if e != 0 { (0xffff_ffffu32 << (e - 1)) & 0xffff_ffff } else { 0xffff_ffff };
    }
    vb
}

/// Re-transliteration of `meg_state::m1_expand` (swp30.cpp:3502-3509).
/// v >= 0 here so `>>` is 0..7 both languages; max 0x1fff << 2 = 0x7ffc fits s16.
fn ref_m1_expand(v: i16) -> i16 {
    if v < 0 {
        return 0; // :3504-3505
    }
    let s = (v >> 12) as u32; // :3506
    let v = 0x1000 | (v & 0xfff); // :3507
    if s == 5 {
        v as i16 // :3508
    } else if s < 5 {
        (v >> (5 - s)) as i16 // :3508
    } else {
        (v << (s - 5)) as i16 // :3508
    }
}

/// origin: swp30_jit.cpp:520-585 `meg_jit_selftest` — see header deviation.
fn meg_jit_selftest() -> u64 {
    let mut bad: u64 = 0;
    // which==0 (swp30_jit.cpp:554-560): encode は下の 27bit しか見ない (:555)
    for v in 0..0x800_0000u32 {
        if (u32::from(ref_revram_encode(v)) & 0xffff) != u32::from(MegState::revram_encode(v)) {
            bad += 1; // :557
        }
    }
    for v in [0xffff_ffffu32, 0x8000_0000, 0xf800_0001] {
        // :558-560
        if (u32::from(ref_revram_encode(v)) & 0xffff) != u32::from(MegState::revram_encode(v)) {
            bad += 1; // :560
        }
    }
    // which==1 (:561-564)
    for v in 0..0x1_0000u32 {
        if ref_revram_decode(v as u16) != MegState::revram_decode(v as u16) {
            bad += 1; // :564
        }
    }
    // which==2, x64 leg (:571-576): 呼ぶ側は loads16 で 64bit に符号拡張 —
    // 入出力とも s64 のまま突き合わせる (:572)
    for v in -0x8000i32..0x8000 {
        if i64::from(ref_m1_expand(v as i16)) != i64::from(MegState::m1_expand(v as i16)) {
            bad += 1; // :576
        }
    }
    bad // :581
}

fn main() {
    // :10 波形 ROM 相当のダミー
    let wave = vec![0u8; 64 * 1024 * 1024];
    // :11
    let sintab = vec![0u16; 0x8000];

    // :13-16 (set_wave_rom/set_sintab -> call-time seam, see header)
    let mut swp = Swp30::new();
    let wave = Wave::new(&wave);
    swp.reset();

    // :18-22 レジスタの読み書きが素通しできるか（ピッチ = slot 0x11）
    swp.write16(0 * 0x40 + 0x11, 0x1234); // :19
    let back = swp.read16(0 * 0x40 + 0x11); // :20
    print!(
        "ピッチレジスタ 書き 0x1234 -> 読み 0x{back:04x}  {}{EOL}", // :21-22
        if back == 0x1234 { "一致" } else { "不一致" } // :22
    );

    // :24-27 1 サンプル回してみる（無音のはず）
    let (mut l, mut r): (i32, i32) = (0, 0); // :25
    for _ in 0..100 {
        let (sl, sr) = swp.run_sample(&sintab, &wave); // :26 (out-params -> tuple)
        l = sl;
        r = sr;
    }
    print!("100 サンプル実行  最後の出力 L={l} R={r}{EOL}"); // :27

    // :29-33 乱数が MAME と同じ数列か — printf の引数は評価順が未規定。
    // 1 個ずつ取り出さないと順序が入れ替わる (:30; Rust already evaluates in order)
    // swp.machine() (:32) = fresh standalone machine, seed 0x9d14abd7
    // (mamecompat.h:715) — nothing before this consumes a draw.
    let mut machine = RunningMachine::new();
    print!("乱数 1〜3 個目:"); // :31
    for _ in 0..3 {
        print!(" {:08x}", machine.rand()); // :32
    }
    print!("{EOL}"); // :33

    // :35-37 — deviation: interpreter-vs-retransliteration sweep (header)
    print!(
        "MEG の JIT のリバーブ RAM の詰め方と戻し方・係数の広げ方: 食い違い {}{EOL}", // :36
        meg_jit_selftest() // :37
    );

    // :39-41 aarch64 emitter selftest — x86 build prints 0 (a64asm.cpp:950)
    print!("a64 emitter selftest: {} mismatch(es){EOL}", 0u64); // :41
    // :42 return 0 — Rust main falls off with exit code 0
}
