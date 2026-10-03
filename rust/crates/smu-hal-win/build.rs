// license:BSD-3-Clause
//
// The `audio out` row's math parity is NOT reproducible from any DLL: the
// ground-truth mingw-w64 16.1 binaries get double sin/cos from STATIC x87
// long-double objects (proof chain, 2026-10-03 session S4: gt.exe has no
// sin/cos import; -Wl,-Map -> libmsvcrt.a(...sin.o)+libmingwex.a
// (...sinl_internal.o); sin.o's disassembly = fld/fstpt -> call
// __sinl_internal; sin(PI) through msvcrt = 0x3ca1a62633145c07 vs the
// static x87 0x3ca1a60000000000 — only the latter reproduces the golden
// resampler table entry tab[256]=0x24330e6e). So instead of re-deriving
// x87 sinl, we LINK THE EXACT OBJECTS: extract them from the same
// archives the C++ build links, rename sin/cos so the MSVC linker cannot
// resolve them to the UCRT (bare `sin` imports api-ms-win-crt-math!), and
// hand them to link.exe as a static lib. `table_golden` then fc-matches
// the proven table_cpp.bin (SHA1 645AADA4834B09F276F16202CCBB27CC2057AACD).
//
// Deviations (disclosed):
// - Build machine requirement: MSYS2 mingw-w64 (AGENTS.md already requires
//   it for the C++ side). Override the root with SMU_MINGW_ROOT.
// - Non-Windows hosts: nothing is emitted; the crate must not be LINKED
//   there today anyway (kernel32/winmm FFI across the crate).

use std::path::{Path, PathBuf};
use std::process::Command;

fn run(ar: &Path, args: &[&str]) {
    let out = Command::new(ar).args(args).output().expect("spawn ar/objcopy");
    if !out.status.success() {
        panic!(
            "tool {:?} {:?} failed: {}",
            ar,
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=SMU_MINGW_ROOT");
    println!("cargo:rerun-if-env-changed=SMU_GTmath_SKIP");
    if std::env::var_os("SMU_GTmath_SKIP").is_some() {
        return;
    }
    let root = std::env::var("SMU_MINGW_ROOT").unwrap_or_else(|_| "C:\\msys64\\mingw64".to_string());
    let root = PathBuf::from(&root);
    let ar = root.join("bin").join("ar.exe");
    let objcopy = root.join("bin").join("objcopy.exe");
    let msvcrt = root.join("lib").join("libmsvcrt.a");
    let mingwex = root.join("lib").join("libmingwex.a");
    for p in [&ar, &objcopy, &msvcrt, &mingwex] {
        if !p.exists() {
            // On a machine that cannot host the GT objects at all, stay
            // silent ONLY when the whole build is non-Windows (cargo check
            // / cross builds must not choke on a dev-only requirement).
            if std::env::var("CARGO_CFG_WINDOWS").as_deref() == Ok("true") {
                panic!(
                    "smu-hal-win build.rs: mingw ground-truth objects not found at {root:?} \
                     (set SMU_MINGW_ROOT or SMU_GTmath_SKIP=1 to opt out — opting out makes \
                     wasapi's table_golden red, it IS the gate)"
                );
            }
            return;
        }
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    println!("cargo:rerun-if-changed={}", msvcrt.display());
    println!("cargo:rerun-if-changed={}", mingwex.display());

    // the exact members lib.exe/ld resolve for `sin`/`cos` in the mingw build
    for (lib, members) in [
        (
            &msvcrt,
            [
                "lib64_libmsvcrt_common_a-sin.o",
                "lib64_libmsvcrt_common_a-cos.o",
            ],
        ),
        (
            &mingwex,
            [
                "lib64_libmingwex_a-sinl_internal.o",
                "lib64_libmingwex_a-cosl_internal.o",
            ],
        ),
    ] {
        for m in members {
            run(&ar, &["x", &lib.to_string_lossy(), m]);
            // ar x drops the member in CWD — cargo does NOT set it to OUT_DIR
            let src = PathBuf::from(m);
            if src.exists() {
                std::fs::copy(&src, out.join(m)).expect("move extracted member");
                std::fs::remove_file(&src).ok();
            }
        }
    }

    // rename the exports away from the CRT's `sin`/`cos` (the MSVC linker
    // prefers the import lib for bare names — LNK-verified 2026-10-03)
    let rsin = out.join("gt_sin.o");
    let rcos = out.join("gt_cos.o");
    run(
        &objcopy,
        &[
            "--redefine-sym",
            "sin=smu_gt_sin",
            &out.join("lib64_libmsvcrt_common_a-sin.o").to_string_lossy(),
            &rsin.to_string_lossy(),
        ],
    );
    run(
        &objcopy,
        &[
            "--redefine-sym",
            "cos=smu_gt_cos",
            &out.join("lib64_libmsvcrt_common_a-cos.o").to_string_lossy(),
            &rcos.to_string_lossy(),
        ],
    );

    let lib = out.join("gtmath.lib");
    std::fs::remove_file(&lib).ok();
    run(
        &ar,
        &[
            "rcs",
            &lib.to_string_lossy(),
            &rsin.to_string_lossy(),
            &rcos.to_string_lossy(),
            &out.join("lib64_libmingwex_a-sinl_internal.o").to_string_lossy(),
            &out.join("lib64_libmingwex_a-cosl_internal.o").to_string_lossy(),
        ],
    );

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=gtmath");
}
