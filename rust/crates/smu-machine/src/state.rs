//! Machine state plumbing. Ledger row `state serializer` keeps the byte layout
//! identical to C++ `mu2000::save_state()`: "S2MU" magic + state_version + ordered
//! per-device field dumps. Boot-cache envelope ("S2BC" v1) lives in `bootcache`
//! (added with the M5 row).
//!
//! M5-W1: the layout engine itself lives in `smu_compat::state_io`
//! (`src/state.h:27-171`), because `smu_compat`'s `timers::state_sync` needs
//! `StateIo` and the crate dep points this way (`smu-machine` → `smu-compat`,
//! never the reverse). This module re-exports for machine-side call sites.
//!
//! M5-W4: the machine glue `Machine::state` + `save_state`/`load_state`,
//! transliterated from `src/mu2000.cpp:3507-3664` in EXACT field order.

#[allow(unused_imports)] // re-export for call sites (statetest.rs uses state_pack/unpack)
pub use smu_compat::state_io::{state_pack, state_unpack, StateIo};

use crate::Machine;
use crate::midi::MIDI_DIN_PORTS;

// ---- 状態の保存と復元
//
// ROM（プログラム・波形・sin 表・字の絵）は入れない。戻すときは同じものを
// 積んでおくこと。調べもの用の数え上げも入れない。
//
// origin: mu2000.cpp:3507-3511 (anonymous-namespace banner, byte-copied)

// 保存の形。中身の並びを変えたら上げる
// origin: mu2000.cpp:3515
const STATE_MAGIC: u32 = 0x554d3253; // "S2MU"
// origin: mu2000.cpp:3516 — version-history comment byte-copied
const STATE_VERSION: u32 = 15; // 15: MEG の書き換わった命令（6.238） / 14: MEG の静まった区画（6.237） / 13: d80000（LCD のコントラスト） / 2: MIDI の入口が A/B の 2 口になった / 3: SWP30 のピッチ EG / 4: サンプリングの録音の位置 / 5: SmartMedia の命令の途中 / 6: MEG の印と 2 つ目の idx / 7: USB の口（C・D）の受け取り途中 / 8: 2 つ目の A/D 変換器（AN4 = HOST SELECT） / 9: SWP30 の書き込みの待ち / 10: USB のコマンド（M37640 からの知らせ） / 11: 液晶の「native の持ち物」（6.188） / 12: 外字の「native の持ち物」（6.190）
// origin: mu2000.cpp:3517
const STATE_VERSION_OLDEST: u32 = 2;

/// origin: mu2000::state_version mu2000.cpp:3614-3617.
pub fn state_version() -> u32 {
    STATE_VERSION // :3616
}

/// origin: mu2000::state mu2000.cpp:3521-3612. EXACT order; widths per
/// mu2000.h as re-transliterated by the W1-W3 device rows.
// DEVIATION (ledger-disclosed, M5-W4): disk's optional-pointer guards
// `if (m_cpu)` (:3535) / `if (m_sci4)` (:3539) are unconditional here —
// the Rust Machine owns the CPU tree and sci4 by value (always present),
// so the stream is byte-identical on every real machine.
impl Machine {
    pub fn state(&mut self, s: &mut StateIo) {
        s.tag("mu2000"); // :3523
        self.rm.state_sync(s); // :3524 (m_machine.state_sync)

        // 主記憶。番地の割り振りは build_bus() と同じ (:3526)
        s.mem(&mut self.soc.bus.ram[..]); // :3527 m_ram
        s.mem(&mut self.soc.bus.dram[..]); // :3528 m_dram
        s.mem(&mut self.soc.bus.iram[..]); // :3529 m_iram
        s.mem(&mut self.sampram); // :3530 m_sampram (Machine-owned; overlay
        // wiring is the sampling-RAM row — fetch.rs:15-16, see field doc)
        // 版 5 から: SmartMedia の命令の途中の状態（カードの中身は入れない）
        // (:3531-3533)
        if s.version() >= 5 {
            // shared Rc — bus arms hold no borrow across a save/load
            self.card.borrow_mut().state(s); // :3533 (smartmedia.cpp:362-375, no tag)
        }

        self.soc.state(s); // :3535 (sh7042_device::state, sh7042.cpp:403-431
        // + Hub trait seam: intc/adc0/adc1(v8,die-A)/bsc/cmt/dmac+ch/mtu+ch/
        // porta..f/sci0/sci1 in birth order)
        self.swpm.borrow_mut().state(s); // :3536 (swp30.cpp:4706-4800)
        self.swps.borrow_mut().state(s); // :3537
        self.lcd.borrow_mut().state(s); // :3538 (hd44780.cpp:273-)
        self.sci4.state(s); // :3539 (sci4.cpp:363-374)

        s.tag("panel"); // :3541
        s.v(&mut self.ledsw1); // :3542 u8
        s.v(&mut self.ledsw2); // :3542 u8
        {
            // :3542 arr m_sws — lives in the shared Rc<RefCell<[u8;6]>>
            // (Hub reads the same rows); stream through the borrow.
            let mut rows = self.sws.borrow_mut();
            s.arr(&mut rows[..]);
        }
        s.v(&mut self.enc_pending); // :3543 int (W5: PORTA runtime seam is
        // EncState — re-tap so the machine latch tracks it; both machines
        // compare equal while the latch stays 0, as on disk's boot path)
        {
            // :3543 m_enc_high — lives in EncState (same RefCell, disjoint field)
            let mut e = self.enc.borrow_mut();
            s.v(&mut e.high);
        }
        s.v(&mut self.pe); // :3543 u16 (W5 re-tap note: PORTA seam writes EncState)
        s.arr(&mut self.m_sci_irq); // :3544 arr m_sci_irq (h:905 int[2] —
        // widened to [i32;2] this row so the `int` widths match disk)
        s.v(&mut self.cycle_debt); // :3545 u64
        // **前のサンプルからのはみ出し**。これが無いと、戻した直後の 1 サンプルで
        // CPU の回す量が数サイクルずれる (:3546-3547)
        s.v(&mut self.overrun); // :3548
        // 版 9 から: SWP30 へ書いた待ちの残り（サンプルを跨ぐことがある）(:3549)
        if s.version() >= 9 {
            // :3551 m_swp_wait — shared Rc with the Hub (swp_hold arms it);
            // round-trip through the borrow
            let mut w = *self.swp_wait.borrow();
            s.v(&mut w);
            *self.swp_wait.borrow_mut() = w;
        }

        // 受け取り途中の MIDI。A と B の 2 口ぶん (:3553)
        s.tag("midi"); // :3554
        for line in self.midi.lines[..MIDI_DIN_PORTS].iter_mut() {
            // :3555 for (midi_line &m : m_midi)
            let mut n = line.queue.len() as u32; // :3556
            s.v(&mut n); // :3557
            if s.writing() {
                // :3558-3560
                for b in line.queue.iter() {
                    let mut b = *b;
                    s.v(&mut b); // :3560 s.v(b) u8
                }
            } else {
                // :3561-3567 clear, then pull min(n, stream-ok) bytes
                line.queue.clear();
                for _ in 0..n {
                    if !s.ok() {
                        break; // :3563 i < n && s.ok()
                    }
                    let mut b: u8 = 0; // :3564
                    s.v(&mut b); // :3565
                    line.queue.push_back(b); // :3566
                }
            }
            s.v(&mut line.bit); // :3569 int
            s.v(&mut line.cur); // :3569 u8
            s.v(&mut line.next); // :3569 u64
        }

        // 版 7 から: USB の口（C・D）の受け取り途中。firmware へ渡す前のバイト列
        // (:3572)
        if s.version() >= 7 {
            s.tag("usb"); // :3574
            // M7 row: `midi.usb` is the shared Rc<RefCell<UsbLine>> — one
            // borrow_mut for the whole leg (the stream touches nothing else
            // re-entrant here; field order/types EXACTLY as disk :3575-3604)
            let mut usb = self.midi.usb.borrow_mut();
            let mut n = usb.rx.len() as u32; // :3575
            s.v(&mut n); // :3576
            if s.writing() {
                // :3577-3580
                for b in usb.rx.iter() {
                    let mut b = *b;
                    s.v(&mut b);
                }
            } else {
                // :3581-3586
                usb.rx.clear();
                for _ in 0..n {
                    if !s.ok() {
                        break;
                    }
                    let mut b: u8 = 0;
                    s.v(&mut b);
                    usb.rx.push_back(b);
                }
            }
            s.v(&mut usb.in_port); // :3588 int
            s.v(&mut usb.next); // :3588 u64
            s.v(&mut usb.have); // :3588 bool
            s.v(&mut usb.cur); // :3588 u8
            s.v(&mut usb.tx_next); // :3588 u64 (tx/out_port do NOT ride — disk)
            if s.version() >= 10 {
                // :3589-3604: command queue + cur_cmd
                let mut c = usb.cmd.len() as u32; // :3590
                s.v(&mut c); // :3591
                if s.writing() {
                    for b in usb.cmd.iter() {
                        let mut b = *b;
                        s.v(&mut b);
                    }
                } else {
                    usb.cmd.clear();
                    for _ in 0..c {
                        if !s.ok() {
                            break;
                        }
                        let mut b: u8 = 0;
                        s.v(&mut b);
                        usb.cmd.push_back(b);
                    }
                }
                s.v(&mut usb.cur_cmd); // :3603 bool
            }
        }

        // 版 13 から: d80000 の値（LCD のコントラスト）(:3607)
        if s.version() >= 13 {
            s.tag("d80"); // :3609
            s.v(&mut self.d80); // :3610 u8
        }
    }
}

/// origin: mu2000::save_state mu2000.cpp:3619-3629.
/// DEVIATION: `&mut self` where disk is `const` + `const_cast` (:3627) —
/// Rust models the interior-mutability of the state stream directly.
pub fn save_state(m: &mut Machine) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new(); // :3621
    {
        let mut s = StateIo::writer(&mut out); // :3622
        let mut magic = STATE_MAGIC; // :3623
        let mut ver = STATE_VERSION; // :3623
        s.v(&mut magic); // :3624
        s.v(&mut ver); // :3625
        s.set_version(ver); // :3626
        m.state(&mut s); // :3627
    }
    out // :3628
}

/// origin: mu2000::load_state mu2000.cpp:3631-3664. Error strings verbatim
/// UTF-8 (disk :3640/:3644).
pub fn load_state(m: &mut Machine, p: &[u8], err: &mut String) -> bool {
    // 読み戻したら CC の控えは忘れる（線で見た値と、機械の中身が合わなくなるので）
    // (:3633-3634) — DEVIATION (cited): the Rust port has no `m_cc_last`
    // (mu2000.h:449 CC cache is fast-midi/native-side, M7 row); no-op here.
    let mut s = StateIo::reader(p); // :3635
    let mut magic: u32 = 0; // :3636
    let mut ver: u32 = 0; // :3636
    s.v(&mut magic); // :3637
    s.v(&mut ver); // :3638
    if !s.ok() || magic != STATE_MAGIC {
        // :3639-3641
        *err = "これは S-MU2000 の状態ではない".to_string();
        return false;
    }
    if ver < STATE_VERSION_OLDEST || ver > STATE_VERSION {
        // :3643-3645
        *err = "状態の形が違う（この版では読めない）".to_string();
        return false;
    }
    s.set_version(ver); // :3647
    m.state(&mut s); // :3648
    if !s.ok() {
        // :3649-3652
        *err = s.error().to_string();
        return false;
    }
    // MIDI OUT の途中の枠と溜めは保存していない。空から始める (:3653)
    // DEVIATION (cited): the Rust port has no `m_tx_r/m_tx_w/m_tx_bit`
    // ring (mu2000.h:1028-1030) — MIDI OUT crosses the SCI wire, the
    // byte-level ring is M7; zeroing is a no-op.
    // 軽量モード（C++ のエフェクト）の中身は状態に**入れない**… (:3657-3660)
    // DEVIATION (cited): native FX is not built (AGENTS: native engine
    // off) — disk's `if (m_nfx_on) m_nfx.reset();` (:3661-3662) is a no-op.
    true // :3663
}
