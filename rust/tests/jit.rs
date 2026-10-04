//! M9 JIT layout pins (JIT_S9_HANDOFF §"Layout tests to add"): every struct
//! offset that jit-emitted code bakes via offset_of! (jit.rs `offs()`),
//! pinned NUMERICALLY here so a silent field reorder in smu-sh2/core.rs,
//! sh7042.rs or smu-machine lib.rs fails the workspace gate instead of
//! corrupting compiled blocks. x86-64/Windows only (the JIT is cfg'd out
//! elsewhere).

#![cfg(all(target_arch = "x86_64", target_os = "windows"))]

use std::mem::{offset_of, size_of};

use smu_machine::jit::JitCtx;
use smu_machine::{Ctx, HubNow};
use smu_sh2::core::Sh2Core;
use smu_sh2::sh7042::Sh7042Bus;

#[test]
fn test_jit_layout() {
    // Computed 2026-10-04 (rustc 1.98.1, `cargo test --release -p smu-machine
    // --test jit -- --nocapture` on this box) — LOCKED below. Table:
    //   JitCtx:     bus=0 core=8 hn=16 base=24          (repr(C), size 32)
    //   HubNow:     now=0 in_event=8 pc=12 pre_seam=16  (repr(C), size 24)
    //   Sh2Core:    pc=0 pr=4 sr=8 mach=12 macl=16 r=20 ea=84 icount=116
    //               gbr=132 vbr=136 m_delay=140 m_am=424 m_test_irq=428
    //               m_total_cycles=464 m_cycles_this_run=472
    //   Ctx:        bus=0
    //   Sh7042Bus:  dev_dirty=88
    assert_eq!(offset_of!(JitCtx, bus), 0);
    assert_eq!(offset_of!(JitCtx, core), 8);
    assert_eq!(offset_of!(JitCtx, hn), 16);
    assert_eq!(offset_of!(JitCtx, base), 24);
    assert_eq!(size_of::<JitCtx>(), 32);

    assert_eq!(offset_of!(HubNow, now), 0);
    assert_eq!(offset_of!(HubNow, in_event), 8);
    assert_eq!(offset_of!(HubNow, pc), 12);
    assert_eq!(offset_of!(HubNow, pre_seam), 16);
    assert_eq!(size_of::<HubNow>(), 24);

    assert_eq!(offset_of!(Sh2Core, pc), 0);
    assert_eq!(offset_of!(Sh2Core, pr), 4);
    assert_eq!(offset_of!(Sh2Core, sr), 8);
    assert_eq!(offset_of!(Sh2Core, mach), 12);
    assert_eq!(offset_of!(Sh2Core, macl), 16);
    assert_eq!(offset_of!(Sh2Core, r), 20);
    assert_eq!(offset_of!(Sh2Core, ea), 84);
    assert_eq!(offset_of!(Sh2Core, icount), 116);
    assert_eq!(offset_of!(Sh2Core, gbr), 132);
    assert_eq!(offset_of!(Sh2Core, vbr), 136);
    assert_eq!(offset_of!(Sh2Core, m_delay), 140);
    assert_eq!(offset_of!(Sh2Core, m_am), 424);
    assert_eq!(offset_of!(Sh2Core, m_test_irq), 428);
    assert_eq!(offset_of!(Sh2Core, m_total_cycles), 464);
    assert_eq!(offset_of!(Sh2Core, m_cycles_this_run), 472);

    assert_eq!(offset_of!(Ctx<'static>, bus), 0);
    assert_eq!(offset_of!(Sh7042Bus, dev_dirty), 88);

    println!(
        "jit_layout: JitCtx 0/8/16/24 | HubNow 0/8/12/16 | Sh2Core pc0 r20 ea84 ic116 gbr132 vbr136 dl140 am424 ti428 tot464 ctr472 | Ctx.bus 0 | dev_dirty 88"
    );
}

/// The default-off gate would also be "green" if the JIT silently fell back
/// to the interpreter — so prove it actually runs: boot the real machine
/// and require emitted blocks + enter() calls. ROM-less boxes skip
/// gracefully (boot_golden convention).
#[test]
fn jit_actually_compiles() {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    let mut d: Option<&Path> = Some(Path::new(env!("CARGO_MANIFEST_DIR")));
    let root: PathBuf = loop {
        match d {
            Some(dir) if dir.join("roms").join("mu2000_flash.bin").exists() => break dir.to_path_buf(),
            Some(dir) => d = dir.parent(),
            None => {
                eprintln!("SKIP jit_actually_compiles: roms/mu2000_flash.bin not found");
                return;
            }
        }
    };
    let mut m = match smu_machine::Machine::boot(&root.join("roms").display().to_string()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("SKIP jit_actually_compiles: Machine::boot failed: {e}");
            return;
        }
    };
    let b0 = smu_machine::jit::JIT_BLOCKS_COMPILED.load(Ordering::Relaxed);
    let e0 = smu_machine::jit::JIT_ENTERS.load(Ordering::Relaxed);
    m.run_cycles(5_000_000); // plenty of ROM code: boot polling loop + SWP handtalk
    let db = smu_machine::jit::JIT_BLOCKS_COMPILED.load(Ordering::Relaxed) - b0;
    let de = smu_machine::jit::JIT_ENTERS.load(Ordering::Relaxed) - e0;
    println!(
        "jit_actually_compiles: {db} blocks compiled, {de} enters (5M cycles); total={} pc={:08x}",
        m.total_cycles(),
        m.pc()
    );
    assert!(db > 100, "JIT compiled only {db} blocks in 5M cycles — not running?");
    assert!(de > 1000, "JIT entered only {de} batches — not running?");
}
