//! MU2000 machine glue - mirror of `src/mu2000.cpp/h` (ledger rows in `smu-machine`).
//!
//! Row `wiring/bus map` (session F11): instantiates and wires ALL paired
//! devices and reproduces the C++ boot event order, replayed against the
//! golden `rust/tests/golden/trace_upd_boot.txt` (tests/boot_golden.rs).
//!
//! # Ownership model (chosen: flat `Machine` + boxed `Hub`, one bus)
//! - `Machine` owns the `Sh7042` (Sh2Device + `Sh7042Bus`), the compat
//!   `RunningMachine`, the `Sci4`, the panel/LCD state and the machine-side
//!   latches (`ledsw1/2`, `d80`) — everything by value (mu2000 is one object,
//!   mu2000.h).
//! - The SH-2-internal peripherals (sci pair, mtu, intc, ports A..F, cmt,
//!   bsc, dmac, lcd) live in a `Hub` boxed behind `Sh7042Bus::periph`
//!   (`Box<dyn Sh7042Peripherals>`), which the already-transliterated
//!   `Sh7042Bus::internal_r8/r16/r32/w8/w16/w32` dispatch (the
//!   `sh7042_map.hxx` port, mu2000.cpp:984-995) routes to. The `Hub` holds
//!   `Rc<RefCell<...>>` handles; the `Machine` keeps clones for the reset
//!   fan-out, IRQ drains and `do_rx_w`. This is the minimal split that keeps
//!   `&mut core` and `&mut bus` disjoint at the `core.run_cycles(&mut ctx,..)`
//!   call (`Sh7042 { dev, bus }` destructure), mirroring how the C++ device
//!   graph reaches devices through the bus while the caller holds the CPU.
//! - The bus seen by the core is a per-instruction `Ctx<'_>` borrowing the
//!   disjoint fields (`bus`, `rm`, sci4 core, latches). `Ctx` intercepts the
//!   machine-owned windows (SCI4 — its handlers need `&mut RunningMachine`,
//!   mu2000.cpp:965-972; led/d80 stubs; card/usb via the bus Dev arms) and
//!   delegates regions plus
//!   the internal register space to `Sh7042Bus` (its `fast_read`/`find_*`
//!   port the membus.h hot path and the `mu2000::build_bus` decode
//!   mu2000.cpp:712-997).
//!
//! # IRQ plumbing (disk chain, synchronous per instruction)
//! Disk: device `irq_req` -> `sh_intc_device::internal_interrupt` ->
//! `update_irq` -> `m_cpu->set_internal_interrupt` (sh_intc.cpp:95-99/:92).
//! External lines: `sci4::write_irq<0/1>` ORed -> `execute_set_input(0)` and
//! `<3>` -> `execute_set_input(1)` (mu2000.cpp:1128-1132), and sh7042.cpp:141
//! -143 sends those to `m_intc->set_input` — the CPU's raw
//! `Sh2Device::execute_set_input` (pending_irq bits) is NEVER used on this
//! board: interrupts reach the core only as internal `(level, vector)` via
//! `set_internal_interrupt` (vec 64 / 66 for IRQ0/IRQ1). Rust seam: bus-side
//! actions push `Evt` onto a shared queue; `Machine::pump()` applies them in
//! order, and the run loop advances the CPU ONE instruction per step
//! (`run_cycles(1)`) so every latch lands before the next instruction's
//! `m_test_irq` test (core.rs:2252-2257 — disk granularity is the instruction
//! too). Exception acks (`sh7042.cpp:397-401` override tail
//! `m_intc->interrupt_taken`) are detected inside `Ctx::read_long` from the
//! exception bus signature: this instruction wrote SR at `r15-4` and PC at
//! `r15-8` (core.rs:1911-1914) and now reads the vector at `vbr+vector*4`
//! (core.rs:1925) — the only path that long-reads the VBR table right after
//! an SP pair (TRAPA included: the disk override acks EVERY exception,
//! sh7042.cpp:397-401).
//!
//! # Deviations (see session report; none fire on the boot golden path)
//! - SWP30 (M3), MIDI bit-machine/fast_midi (M4) — the USB host (M7) is
//!   LIVE since the `USB host (M37640)` row (usb.rs + Hub::usb bus arms +
//!   run_cycles :1227 pump; guard :1323 keeps host-OFF runs byte-identical);
//!   its windows return the disk "no handler answered" 0. SmartMedia is
//!   LIVE since the W-CARD row (card.rs full port + Hub::card* bus arms +
//!   PA19/PA20 off the real card; CONTROL keeps its literal 0xff inside
//!   `Sh7042Bus`, mu2000.cpp:992).
//! - ADC0/ADC1 (cancelled row): instantiated nowhere; `adc0/adc1_update`
//!   trait defaults return 0 == disk `add_event` skip (sh7042.cpp:256-257).
//! - `--hash-pc`/`--trace-pc` fold through the `smu_compat::paths` global
//!   sinks (fold/format ported in row compat/paths); the `--trace-upd`
//!   writer lives here and emits CRLF on Windows via `cfg!(windows)`
//!   (ledger CRLF pitfall).
//! - One instruction per `core.run_cycles` call instead of the disk
//!   scheduler chunks; `done == 0` (mid-instruction `abort_timeslice`,
//!   sh.h:219-223) consumes 0 budget cycles on disk too when the chunk is 1.
//!   A disk `execute_set_input` landing mid-instruction would see the
//!   exception taken one retire earlier than here; level-hold lines (boot)
//!   are net-identical.
//! - ROM bytes arrive as constructor data (attach-before-reset,
//!   mu2000.cpp:997); ROM dir is an explicit argument like boot.cpp's
//!   `argv[1]` — NO `SMU2000_ROMS` env fallback (session-C finding).

pub mod bootcache;
pub mod card;
pub mod midi;
pub mod nvram;
pub mod state;
pub mod usb;
pub mod xg;

use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::VecDeque;
use std::io::Write;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use smu_compat::paths;
use smu_compat::roms;
use smu_compat::StateIo;
use smu_compat::timers::RunningMachine;
use smu_dev::hd44780::Hd44780;
use smu_dev::sci4::Sci4;
use smu_sh2::core::{InstructionHook, Sh2Bus, Sh2Core};
use smu_sh2::periph::stubs::{ShBsc, ShDmac};
use smu_sh2::periph::{Sh2Intc, Sh2Mtu, Sh2SciPair, ShCmt, ShPort16, ShPort32};
use smu_sh2::sh7042::{Sh7042, Sh7042Peripherals};
use smu_swp30::fetch::Wave;
use smu_swp30::{Swp30, MASTER_BASE, SLAVE_BASE};

/// origin: mu2000.cpp:72/81 — 7 MHz crystal x4 PLL = 28 MHz.
pub const CPU_HZ: u32 = 7_000_000 * 4;
/// origin: hd44780.h:33-34 ctor defaults (mu2000 member).
pub const LCD_HZ: u32 = 270_000;
/// origin: sci4.h:21 default clock (mu2000.cpp:78 uses the default).
pub const SCI4_HZ: u32 = 8_000_000;
/// origin: mu2000.h:924 `SWP_WRITE_CYCLES = 440` — BSC WAIT per master SWP
/// write, measured on hardware (mu2000.cpp:833-846)
pub const SWP_WRITE_CYCLES: u64 = 440;

/// CRLF-owning EOL for trace writers (ledger pitfall: C++ text-mode fprintf).
const EOL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// origin: mu2000.cpp:740/823/826/828 region + device window bases.
const ROM_END: u32 = 0x003f_ffff;
const RAM_END: u32 = 0x0043_ffff;
const DRAM_BASE: u32 = 0x0100_0000;
const DRAM_END: u32 = 0x0107_ffff;
const IRAM_BASE: u32 = 0xffff_f000;
const SCI4_BASE: u32 = 0x00f0_0000;
const SCI4_END: u32 = 0x00f0_003f;
const LED_ADDR: u32 = 0x00c8_0000; // mu2000.cpp:928 (start==end)
const PANEL_ADDR: u32 = 0x00e0_0000; // mu2000.cpp:935
const D80_ADDR: u32 = 0x00d8_0000; // mu2000.cpp:941
// The SmartMedia windows (c00000-c7ffff data, d00000-d7ffff control,
// mu2000.cpp:981-995) have NO Ctx intercept since this row: sh7042.rs
// Dev::CardData/CardCtrl (find_dev) decode all widths with the membus.h
// r8-chain demotion and the handlers live in Hub::card_data_r8/
// card_data_w8/card_ctrl_w8 -> smu_machine::card (disk lambdas :985-986/:993;
// CONTROL read keeps its literal 0xff, :992).
// SWP30 window bases live in smu_swp30::{MASTER_BASE,SLAVE_BASE} (mu2000.cpp:921-922).
// The USB window (mu2000.cpp:1009-1010, disk re-verified 2026-10-03) has NO
// Ctx intercept since the M7 row: sh7042.rs Dev::Usb (find_dev :479-480)
// decodes all widths with the membus.h r8-chain demotion and the handlers
// live in Hub::usb_r8/usb_w8 -> smu_machine::usb (exactly the disk shape
// mu2000.cpp:1011-1012 d.r8/d.w8 -> usb_r/usb_w).
const INT_BASE: u32 = 0xffff_8000; // mu2000.cpp:987
const INT_END: u32 = 0xffff_9fff;

/// Bus-side -> machine-side deferred actions, applied in `Machine::pump()`.
pub enum Evt {
    /// drained sci/mtu/cmt `irq_req` vectors -> `Sh7042::route_irqs`
    /// (sh_intc.cpp:95-99 latch + :92 set_internal_interrupt per vector)
    Irq(Vec<i32>),
    /// `execute_set_input` from the mu2000.cpp:1128-1132 wiring (OR at
    /// :1040) -> sh7042.cpp:143 -> intc.rs:211 `set_input` -> sh_intc.cpp:92
    SetInput { line: i32, state: i8 },
    /// intc `update_irq` re-issued after an IPR arbitration (sh_intc.cpp:92,
    /// via the intc.rs w8/w16 `Option<(level, vector)>` seam)
    CpuIrq { level: i32, vector: u32 },
    /// exception ack (sh7042.cpp:400 `m_intc->interrupt_taken`; `irqline`
    /// unused on disk, intc.rs:188-197)
    Ack { vector: i32 },
}

/// Adapter: the `m_cpu->execute_set_input(line, state)` seam of
/// `usb_step`/`usb_r` (mu2000.cpp:1341/:1349/:1355/:1367) onto the Evt
/// FIFO. Disk chain: sh7042.cpp:141-143 -> `m_intc->set_input` (:223
/// same-value early-out; intc.rs:228). Applied by `Machine::pump()` — the
/// pump right after the pump call / the per-instruction pump inside the
/// inner loop, both the same post-execute `m_test_irq` boundary as C++
/// (module-doc deviation note).
pub struct QIrq<'a> {
    pub q: &'a RefCell<VecDeque<Evt>>,
}
impl usb::UsbIrq for QIrq<'_> {
    #[inline]
    fn set_input(&mut self, line: i32, state: i8) {
        self.q.borrow_mut().push_back(Evt::SetInput { line, state });
    }
}

/// Shared clock seam: instruction-start total cycles + event-window flag.
pub struct HubNow {
    pub now: u64,
    pub in_event: bool,
    /// PC of the instruction being executed (written by `RunHook::instruction`
    /// at the instruction head, the exact C++ debug-pc position — sh2.cpp:268).
    /// Read by the SWP `--trace-swp` sink as `m_cpu->pc()` (mu2000.cpp:864).
    pub pc: u32,
}

impl HubNow {
    /// origin: sh7042.h:92-98 `current_cycles` (the MAME -1 outside events).
    pub fn cpu_now(&self) -> u64 {
        if self.in_event {
            self.now
        } else if self.now != 0 {
            self.now - 1
        } else {
            0
        }
    }
}

/// origin: mu2000.cpp:1083-1105 Port-A dial closure state.
pub struct EncState {
    pub pending: i32, // mu2000.h:889 `int m_enc_pending = 0`
    pub high: bool,   // mu2000.h:890 `bool m_enc_high = true`
}

// ---------------------------------------------------------------------------
// ADC — sh_adc.cpp/.h transliteration. The ledger's `ADC cancelled` row claim
// ("0 firmware refs") was a PHANTOM: firmware polls AD csr 0xffff8411/ADDR
// from cycle ~130 (first instruction divergence pc=0x1190, r0: C++ 0x20 vs 0
// — an in-conversion ADCSR). Board is die-A so BOTH ADCs are SH_ADC_MS
// (sh7042.cpp:166-168): adc0 port_base 0 / vector 136, adc1 port_base 4 /
// vector 137; MS = is_hs false, port_mask 3, port_shift 6, analog_powered
// true (ctor :42). The HS branch of mode_update (:308-329) is dead here.
// ---------------------------------------------------------------------------

/// origin: sh_adc.h:55-59 (ADCSR flag bits)
const F_ADF: u8 = 0x80;
const F_ADIE: u8 = 0x40;
const F_ADST: u8 = 0x20;
/// origin: sh_adc.h:61-70 (mode flags)
const A_IDLE: i32 = 0;
const A_ACTIVE: i32 = 1;
const A_HALTED: i32 = 2;
const A_REPEAT: i32 = 4;
const A_ROTATE: i32 = 8;
const A_DUAL: i32 = 16;   // dead on MS (HS-only adcr bits)
const A_BUFFER: i32 = 32; // dead on MS
const A_COUNTED: i32 = 64; // dead on MS
/// origin: sh_adc.h:49 (MS mode_update pins T_SOFT, sh_adc.cpp:335)
const T_SOFT: i32 = 1;
/// origin: sh_adc.cpp:40 (MS ctor port_mask = 3)
const MS_PORT_MASK: u8 = 3;

/// origin: mu2000.h:870-874 ad_level_adc — peak 0 at boot (mu2000.h:869
/// `s32 m_ad_peak[2] = {}`, no audio has run). u32 math, u16 result.
fn ad_level_adc(i: usize, ad_peak: &[i32; 2]) -> u16 {
    let v = 0x18u32 + (ad_peak[i] as u32).wrapping_mul(0x85 - 0x18) / 32768; // :873
    ((0xffu32 - v) as u16) << 2 // :874
}

/// origin: mu2000.cpp:1113-1122/1149-1151 read_adc<0..7> bindings. AN4 =
/// ホストスイッチ `m_usb_host ? 0x330 : 0` (:1151 — the disk binding is a
/// lambda reading m_usb_host LIVE; the Hub mirror is the shared
/// `Hub::usb_host` Cell, same value set_usb_host just wrote). firmware
/// drops to 8 bits and splits at the border: <0x20 DIN, 0xBA-0xE0 USB (:1149-1150).
fn ad_pin(port: usize, ad_peak: &[i32; 2], usb_host: bool) -> u16 {
    match port {
        0 => ad_level_adc(0, ad_peak), // :1113
        1 => 0,                        // :1114 constant 0
        2 => ad_level_adc(1, ad_peak), // :1115
        3 => 0,                        // :1116 constant 0
        4 => if usb_host { 0x330 } else { 0 }, // :1151 (HOST SELECT pin)
        5 => 0,                        // :1120 constant 0
        6 => 0x3ff,                    // :1121 battery full
        _ => 0,                        // :1122 AN7 constant 0
    }
}

/// origin: sh_adc.h:77-84 state; inits = ctor sh_adc.cpp:17-31/:35-43 +
/// device_reset :149-164 (called separately, like disk).
pub struct Adc {
    pub addr: [u16; 8],           // h:77 m_addr (zero-init `{}`)
    pub buf: [u16; 2],            // h:77 m_buf
    pub adcsr: u8,                // h:78 = 0 (:21)
    pub adcr: u8,                 // h:78 = 0 (:21)
    pub port_base: usize,         // h:44; MS ctor args (sh7042.cpp:167-168)
    pub port_shift: u32,          // :41 = 6
    pub intc_vector: i32,         // h:45 = 136 / 137 (:167-168)
    pub trigger: i32,             // :21 = 0 until device_reset T_SOFT (:154)
    pub start_mode: i32,          // :21 = 0 (:155 IDLE until mode_update)
    pub start_channel: i32,       // :21 = 0
    pub end_channel: i32,         // :22 = 0
    pub start_count: i32,         // :22 = 0 (:157 = 1 at reset)
    pub suspend_on_interrupt: bool,     // :30 = false (heap-zeroed disk value)
    pub analog_power_control: bool,     // :30 = false
    pub mode: i32,                // h:82 = 0
    pub channel: i32,             // h:82 = 0
    pub count: i32,               // h:82 = 0
    pub analog_powered: bool,     // :31 false, MS ctor :42 true; device_reset
                                  // does NOT touch it (disk)
    pub adtrg: bool,              // :31 false; reset :163 true
    pub next_event: u64,          // h:84 = 0
    /// sticky COUNT of `m_cpu->internal_update()` sites (:189): disk recurses
    /// once per call, so the wiring pump re-fans-out + re-traces once each.
    pub resched: u32,
}

impl Adc {
    /// origin: sh_adc.cpp:35-43 SH_ADC_MS ctor pre-reset state.
    pub fn new_ms(port_base: usize, intc_vector: i32) -> Adc {
        Adc {
            addr: [0; 8],
            buf: [0; 2],
            adcsr: 0,  // :21 m_adcsr(0)
            adcr: 0,   // :21
            port_base, // :39
            port_shift: 6, // :41
            intc_vector,
            trigger: 0,             // :21 m_trigger(0)
            start_mode: 0,          // :21
            start_channel: 0,       // :21
            end_channel: 0,         // :22
            start_count: 0,         // :22
            suspend_on_interrupt: false,    // :30
            analog_power_control: false,    // :30
            mode: 0,                // :31
            channel: 0,             // :31
            count: 0,               // :31
            analog_powered: true,   // :42 (MS)
            adtrg: false,           // :31
            next_event: 0,          // :31
            resched: 0,
        }
    }

    /// origin: sh_adc.cpp:149-164 device_reset.
    pub fn device_reset(&mut self) {
        self.addr = [0; 8]; // :151
        self.buf = [0; 2]; // :152
        self.adcsr = 0; // :153
        self.adcr = 0; // :153
        self.trigger = T_SOFT; // :154
        self.start_mode = A_IDLE; // :155
        self.start_channel = 0; // :156
        self.end_channel = 0; // :156
        self.start_count = 1; // :157
        self.mode = A_IDLE; // :158
        self.channel = 0; // :159
        self.count = 0; // :160
        self.next_event = 0; // :161
        self.mode_update(); // :162
        self.adtrg = true; // :163
    }

    /// origin: sh_adc.cpp:380-391 (`sh_adc_device::state`). Widths from
    /// sh_adc.h:77-84: addr u16 x8 / buf u16 x2 (`arr`), adcsr/adcr u8, the
    /// five channel-config ints, two bools (1 byte), mode/channel/count ints,
    /// analog_powered/adtrg bools, next_event u64, disk order :383-390.
    /// m_register_mask (h:79) and the ctor config (port_base/shift,
    /// intc_vector) are NOT serialized on disk :380-391.
    pub fn state(&mut self, s: &mut StateIo) {
        s.tag("adc");                          // :382
        s.arr(&mut self.addr);                 // :383 arr m_addr (h:77 u16[8])
        s.arr(&mut self.buf);                  // :383 arr m_buf (h:77 u16[2])
        s.v(&mut self.adcsr);                  // :384 (h:78 u8)
        s.v(&mut self.adcr);                   // :384
        s.v(&mut self.trigger);                // :385 (h:80 int)
        s.v(&mut self.start_mode);             // :385
        s.v(&mut self.start_channel);          // :385
        s.v(&mut self.end_channel);            // :386
        s.v(&mut self.start_count);            // :386
        s.v(&mut self.suspend_on_interrupt);   // :387 (h:81 bool)
        s.v(&mut self.analog_power_control);   // :387
        s.v(&mut self.mode);                   // :388 (h:82 int)
        s.v(&mut self.channel);                // :388
        s.v(&mut self.count);                  // :388
        s.v(&mut self.analog_powered);         // :389 (h:83 bool)
        s.v(&mut self.adtrg);                  // :389
        s.v(&mut self.next_event);             // :390 (h:84 u64)
    }

    /// origin: sh_adc.h:75 free_running (is_hs=false folded; MS ctor :38).
    fn free_running(&self) -> bool {
        (self.mode & A_REPEAT) != 0
            && (self.adcsr & F_ADST) != 0
            && (self.adcsr & F_ADIE) == 0
            && self.next_event == 0
    }

    /// origin: sh_adc.cpp:56-61 addr_r (port_shift = 6, MS :41).
    fn addr_r(&mut self, offset: usize, pin: &dyn Fn(usize) -> u16) -> u16 {
        if self.free_running()
            && (offset as i32) >= self.start_channel
            && (offset as i32) <= self.end_channel
        {
            // :59 m_cpu->do_read_adc(offset + m_port_base)
            self.addr[offset] = pin(offset + self.port_base);
        }
        ((self.addr[offset] as u32) << self.port_shift) as u16 // :60
    }

    /// origin: sh_adc.cpp:63-70 adcsr_r (S-MU2000 free-run quirk: ADF shows
    /// while spinning, :66-68).
    fn adcsr_r(&self) -> u8 {
        if self.free_running() {
            return self.adcsr | F_ADF; // :68
        }
        self.adcsr // :69
    }

    /// origin: sh_adc.cpp:72-76 adcr_r.
    fn adcr_r(&self) -> u8 {
        self.adcr
    }

    /// origin: sh_adc.cpp:78-105 adcsr_w. `total` = m_cpu->total_cycles()
    /// (no irqs out — disk interrupts only fire from timeout, :274).
    fn adcsr_w(
        &mut self,
        data: u8,
        total: u64,
        pin: &dyn Fn(usize) -> u16,
    ) {
        let prev = self.adcsr; // :81
        // :82 (data & (0x70|port_mask)) | (adcsr & data & F_ADF), mask=3 (:40)
        self.adcsr = (data & (0x70 | MS_PORT_MASK)) | (self.adcsr & data & F_ADF);
        self.mode_update(); // :83
        if (prev & F_ADF) != 0 && (self.adcsr & F_ADF) == 0 {
            // :84-93
            if (self.mode & A_HALTED) != 0 {
                self.mode &= !A_HALTED; // :86
                if (self.adcsr & F_ADST) == 0 {
                    self.sampling(pin); // :88
                    self.conversion_wait(false, false, 0, total); // :89
                } else {
                    self.done(); // :91
                }
            }
        }
        // :95-101 S-MU2000 (!is_hs REPEAT idle) hook
        if (self.mode & A_REPEAT) != 0 && self.next_event == 0 {
            if (self.adcsr & F_ADST) == 0 {
                self.done(); // :98
            } else if (self.adcsr & F_ADIE) != 0 {
                self.conversion_wait(false, false, 0, total); // :100
            }
        }
        if (prev & F_ADST) == 0 && (self.adcsr & F_ADST) != 0 {
            self.start_conversion(total, pin); // :103-104
        }
    }

    /// origin: sh_adc.cpp:107-112 adcr_w.
    fn adcr_w(&mut self, data: u8) {
        self.adcr = data; // :110
        self.mode_update(); // :111
    }

    /// origin: sh_adc.cpp:306-346 mode_update — MS branch :330-345 only
    /// (both instances SH_ADC_MS; HS branch dead).
    fn mode_update(&mut self) {
        self.trigger = T_SOFT; // :335
        if (self.adcsr & 0x10) != 0 {
            // :337-340 SCAN: rotate AN0..CH while ADST
            self.start_mode = A_ACTIVE | A_REPEAT | A_ROTATE;
            self.start_channel = 0;
            self.end_channel = (self.adcsr & 3) as i32;
        } else {
            // :341-344
            self.start_mode = A_ACTIVE;
            self.start_channel = (self.adcsr & 3) as i32;
            self.end_channel = self.start_channel;
        }
    }

    /// origin: sh_adc.cpp:296-304 conversion_time.
    fn conversion_time(&self, first: bool, poweron: bool) -> u64 {
        let mut tm: u64 = if (self.adcsr & 0x10) != 0 { 44 } else { 24 }; // :298
        if first {
            tm += if (self.adcsr & 0x10) != 0 { 20 } else { 10 }; // :300
        }
        if poweron {
            tm += 200; // :302
        }
        tm
    }

    /// origin: sh_adc.cpp:183-191 conversion_wait. cur_time==0 (write-path
    /// default arg) -> total_cycles + the sticky internal_update recursion
    /// (:189) -> `resched += 1` for the wiring pump.
    fn conversion_wait(&mut self, first: bool, poweron: bool, current_time: u64, total: u64) {
        if current_time != 0 {
            // :186
            self.next_event = current_time + self.conversion_time(first, poweron);
        } else {
            // :188 m_cpu->total_cycles() + tm
            self.next_event = total + self.conversion_time(first, poweron);
            self.resched += 1; // :189
        }
    }

    /// origin: sh_adc.cpp:204-213 sampling. COUNTED (:206) and DUAL (:208)
    /// are unreachable on MS (mode_update never sets them; the disk
    /// get_channel_index is abort(), :375-378).
    fn sampling(&mut self, pin: &dyn Fn(usize) -> u16) {
        debug_assert!(self.mode & (A_COUNTED | A_DUAL) == 0, "MS sampling: dead flag");
        self.buffer_value(self.channel as usize, 0, pin); // :212
    }

    /// origin: sh_adc.cpp:193-196 buffer_value.
    fn buffer_value(&mut self, port: usize, buffer: usize, pin: &dyn Fn(usize) -> u16) {
        self.buf[buffer] = pin(port + self.port_base); // :195
    }

    /// origin: sh_adc.cpp:198-202 commit_value.
    fn commit_value(&mut self, reg: usize, buffer: usize) {
        self.addr[reg] = self.buf[buffer]; // :201
    }

    /// origin: sh_adc.cpp:215-228 start_conversion (no irqs out of this one —
    /// disk `timeout` is the only internal_interrupt site, :274).
    fn start_conversion(
        &mut self,
        total: u64,
        pin: &dyn Fn(usize) -> u16,
    ) {
        self.mode = self.start_mode; // :217
        self.channel = self.start_channel; // :218
        self.count = self.start_count; // :219
        if self.free_running() {
            // :220-224 S-MU2000: spinning without ADIE — no clock ticks
            self.adcsr |= F_ADF;
            self.analog_powered = true;
            return;
        }
        self.sampling(pin); // :225
        self.conversion_wait(true, !self.analog_powered, 0, total); // :226
        self.analog_powered = true; // :227
    }

    /// origin: sh_adc.cpp:230-294 timeout. BUFFER/do_buffering (:232-238)
    /// and DUAL/COUNTED are dead on MS (see mode_update).
    fn timeout(
        &mut self,
        current_time: u64,
        total: u64,
        pin: &dyn Fn(usize) -> u16,
        irqs: &mut Vec<i32>,
    ) {
        debug_assert!(self.mode & (A_BUFFER | A_DUAL | A_COUNTED) == 0, "MS timeout: dead flag");
        self.commit_value(self.channel as usize, 0); // :250
        if (self.mode & A_ROTATE) != 0 {
            if self.channel != self.end_channel {
                self.channel += 1; // :255
                self.sampling(pin); // :256
                self.conversion_wait(false, false, current_time, total); // :257
                return;
            }
            self.channel = self.start_channel; // :260
        }
        self.adcsr |= F_ADF; // :272
        if (self.adcsr & F_ADIE) != 0 {
            irqs.push(self.intc_vector); // :274 m_intc->internal_interrupt
        }
        if (self.mode & A_REPEAT) != 0 {
            if (self.adcsr & F_ADST) == 0 {
                self.done(); // :277-280
                return;
            }
            if self.suspend_on_interrupt && (self.adcsr & F_ADIE) != 0 {
                self.mode |= A_HALTED; // :282-284
                return;
            }
            self.channel = self.start_channel; // :286
            self.count = self.start_count; // :287
            self.sampling(pin); // :288
            self.conversion_wait(false, false, current_time, total); // :289
            return;
        }
        self.done(); // :293
    }

    /// origin: sh_adc.cpp:166-172 done.
    fn done(&mut self) {
        self.mode = A_IDLE; // :168
        self.adcsr &= !F_ADST; // :169
        if self.analog_power_control {
            self.analog_powered = false; // :170-171 (false on MS)
        }
    }

    /// origin: sh_adc.cpp:174-181 internal_update.
    fn internal_update(
        &mut self,
        current_time: u64,
        total: u64,
        pin: &dyn Fn(usize) -> u16,
        irqs: &mut Vec<i32>,
    ) -> u64 {
        if self.next_event != 0 && self.next_event <= current_time {
            self.next_event = 0; // :177
            self.timeout(current_time, total, pin, irqs); // :178
        }
        self.next_event // :180
    }

    /// sticky take — one unit per disk recursion level (see field doc).
    fn take_resched(&mut self) -> bool {
        if self.resched != 0 {
            self.resched -= 1;
            true
        } else {
            false
        }
    }
}

/// origin: mu2000.h:914-915 `m_swp_trace` + `m_swp_trace_reads` and the
/// `--trace-swp` writers (mu2000.cpp:863-864/884-888/908-910). Shared between
/// Machine (installs it in `set_swp_trace`, boot.cpp:78) and the Hub (writes it
/// from the SWP r16/w16/w32 arms, where `m_cpu->pc()`/`total_cycles()` live).
/// `s=` is `trace_sample()` = `m_sample_count ? m_sample_count-1 : 0`
/// (mu2000.h:906); boot.exe is cycles-only (never `run_sample`), so the sample
/// count stays 0 and every line reads `s=0` (confirmed against the golden).
pub struct SwpSink {
    pub f: Option<std::fs::File>,
    pub reads: bool,
}

/// Byte-exact mirror of the SWP `--trace-swp` fprintf (mu2000.cpp:864/886/888/910):
/// `<prefix>%08x %04x %04x  pc=%08x  t=%.6f s=%llu`. Two spaces flank `pc=`,
/// one precedes `s=`. `prefix` is `"R "` for a read, `"W "`/`""` for a write
/// (reads-on/reads-off). `t` = total_cycles/28e6 — Rust `{:.6}` of the same f64
/// is byte-identical to C `%.6f` on x86 SSE (smf-row provenance, Pitfalls).
fn swp_line(prefix: &str, base: u32, reg: u32, val: u16, pc: u32, now: u64) -> String {
    let t = (now as f64) / 28_000_000.0;
    format!("{prefix}{base:08x} {reg:04x} {val:04x}  pc={pc:08x}  t={t:.6} s=0{EOL}")
}

/// The SH-2-internal peripheral arena behind `Sh7042Bus::periph`.
pub struct Hub {
    pub hn: Rc<RefCell<HubNow>>,
    pub pair: Rc<RefCell<Sh2SciPair>>, // sh7042.cpp:237-238
    pub mtu: Rc<RefCell<Sh2Mtu>>, // sh7042.cpp:178-229
    pub intc: Rc<RefCell<Sh2Intc>>, // sh7042.cpp:165
    pub porta: Rc<RefCell<ShPort32>>, // :231 (0, 0, 0xff000000)
    pub portb: Rc<RefCell<ShPort16>>, // :232
    pub portc: Rc<RefCell<ShPort16>>, // :233
    pub portd: Rc<RefCell<ShPort32>>, // :234
    pub porte: Rc<RefCell<ShPort16>>, // :235 — LCD (mu2000.cpp:1123-1124)
    pub portf: Rc<RefCell<ShPort16>>, // :236
    pub cmt: Rc<RefCell<ShCmt>>, // :172 (144, 148)
    pub bsc: Rc<RefCell<ShBsc>>, // :171
    pub dmac: Rc<RefCell<ShDmac>>, // :173-177
    pub lcd: Rc<RefCell<Hd44780>>,
    pub sws: Rc<RefCell<[u8; 6]>>, // mu2000.h:885 {0xff x6}, active-low rows
    pub enc: Rc<RefCell<EncState>>,
    /// mu2000.h `smartmedia m_card` — the live card behind the bus arms
    /// (mu2000.cpp:985-986/993) and the PA19/PA20 pins (:1121-1125).
    /// Shared with `Machine::card` (this row; was the M4 `card_inserted`
    /// bool mirror).
    pub card: Rc<RefCell<card::Card>>,
    /// sh7042.cpp:167-168 (die-A: both SH_ADC_MS) — see the `Adc` port above
    pub adc0: Rc<RefCell<Adc>>, // port_base 0, vector 136
    pub adc1: Rc<RefCell<Adc>>, // port_base 4, vector 137
    /// mu2000.h:869 `s32 m_ad_peak[2] = {}` — audio peaks (AN0/AN2); 0 until
    /// audio runs (never on the boot path)
    pub ad_peak: Rc<RefCell<[i32; 2]>>,
    pub q: Rc<RefCell<VecDeque<Evt>>>,
    /// mu2000.cpp:921-922 the two SWP30 devices (master @0x800000, slave
    /// @0x802000); reg-dispatch row (smu-swp30::Swp30).
    pub swpm: Rc<RefCell<Swp30>>,
    pub swps: Rc<RefCell<Swp30>>,
    /// shared `--trace-swp` sink (mu2000.h:914-915)
    pub swp_sink: Rc<RefCell<SwpSink>>,
    /// mu2000.h:925 `m_swp_wait` — undigested SWP-master BSC-WAIT cycles,
    /// charged by the `hold()` bus site (mu2000.cpp:848-857), consumed by
    /// run_cycles :1211-1217. Shared with `Machine::swp_wait`.
    pub swp_wait: Rc<RefCell<u64>>,
    /// mu2000.h:1021 `usb_line m_usb` (M7 `USB host` row) — the FULL line
    /// behind one Rc: the bus arms below (sh7042.rs Dev::Usb == the disk
    /// lambdas mu2000.cpp:1011-1012) and `Machine::run_cycles`'s usb_step
    /// pump (:1227) drive the SAME object; `Machine::midi.usb` is the
    /// matching clone (receiver-half inject + state stream live there).
    pub usb: Rc<RefCell<usb::UsbLine>>,
    /// mu2000.h:1022 `m_usb_host` — the LIVE value behind the :1151 ADC4
    /// lambda. Shared with `Machine::midi.usb_host`; ONLY
    /// `Machine::set_usb_host` writes it (mu2000.h:221 — BEFORE reset).
    pub usb_host: Rc<Cell<bool>>,
    /// reusable FIFO drain buffer (steady-state no-allocation)
    buf: Vec<i32>,
}

impl Hub {
    /// Drain the `irq_req` FIFOs (disk: `irq_req` -> intc synchronously).
    /// Order = sh7042.cpp:285-292 update dispatch (cmt, mtu0..4, sci0, sci1).
    fn drain(&mut self) {
        let mut n = 0;
        {
            let mut d = self.cmt.borrow_mut();
            n += d.drain_irqs(&mut self.buf); // cmt.rs:148
        }
        {
            let mut d = self.mtu.borrow_mut();
            n += d.drain_irqs(&mut self.buf); // mtu.rs:736 (ch0..4 concat)
        }
        {
            let mut p = self.pair.borrow_mut();
            n += p.sci[0].drain_irqs(&mut self.buf); // sci.rs:980
            n += p.sci[1].drain_irqs(&mut self.buf);
        }
        if n > 0 {
            self.q.borrow_mut().push_back(Evt::Irq(std::mem::take(&mut self.buf)));
        } else {
            self.buf.clear();
        }
    }

    #[inline]
    fn cpu_now(&self) -> u64 {
        self.hn.borrow().cpu_now()
    }

    /// Handle for the SWP30 device behind the window (mu2000.cpp:921-922):
    /// master @0x800000, slave @0x802000. Returns an Rc clone so the caller can
    /// borrow the RefCell independently of `&mut self`.
    #[inline]
    fn swp_dev(&self, master: bool) -> Rc<RefCell<Swp30>> {
        if master {
            Rc::clone(&self.swpm)
        } else {
            Rc::clone(&self.swps)
        }
    }

    /// origin: mu2000.cpp:1004-1013 lcd_port_r — clock is TOTAL cycles
    /// (:1006 `m_cpu->total_cycles()`, NOT current_cycles; hd44780 row).
    fn porte_pins(&self) -> u16 {
        let n = self.hn.borrow().now;
        self.lcd.borrow_mut().lcd_port_r(n)
    }

    /// origin: mu2000.cpp:1015-1036 lcd_port_w(v), v = the dr_w/io_w `Some`
    /// data argument = `m_dr & m_io` (port row devcb seam).
    fn porte_write(&self, v: u16) {
        let n = self.hn.borrow().now;
        self.lcd.borrow_mut().lcd_port_w(n, v);
    }

    /// origin: mu2000.cpp:1115-1136 read_porta closure (card lines + dial).
    fn porta_pins(&self) -> u32 {
        let mut v: u32 = 0xffff; // :1116
        {
            // :1117-1125 SmartMedia の線: PA19 差し込み / PA20 書き込みを
            // 禁じていない（busy PA18 never asserted — reads finish inline）
            let c = self.card.borrow();
            if c.inserted() {
                v |= 1 << 19; // :1122
                if !c.write_protected {
                    v |= 1 << 20; // :1123-1124
                }
            }
        }
        {
            let mut e = self.enc.borrow_mut();
            if e.pending != 0 {
                if e.pending < 0 {
                    v |= 1 << 16; // :1095 B 相は向きのあいだ立てておく
                }
                if e.high {
                    v |= 1 << 17; // :1096-1098 A 相の立ち上がりで 1 目盛り
                    e.high = false;
                } else {
                    e.high = true;
                    e.pending += if e.pending > 0 { -1 } else { 1 }; // :1101
                }
            }
        }
        v
    }

    /// origin: mu2000.cpp:531-538 ledsw_r (selected button rows AND-fold).
    fn ledsw_r(&self, ledsw1: u8) -> u8 {
        let mut res: u8 = 0xff;
        for i in 0..6u32 {
            if (ledsw1 >> i) & 1 != 0 {
                res &= self.sws.borrow()[i as usize];
            }
        }
        res
    }
}

// ---------------------------------------------------------------------------
// ---- フロントパネル（mu2000.cpp:500-570）—— ボタン表と enum
// firmware は c80000 に「行」を書いてから同じ番地を読む。押されている桁が 0。
// ---------------------------------------------------------------------------

/// origin: mu2000.cpp:507 `struct button_slot { u8 row, bit; const char *name; }`
struct ButtonSlot {
    row: u8,
    bit: u8,
    name: &'static str,
}

/// origin: mu2000.cpp:510-524 `const button_slot BUTTONS[]` — 1-to-1 with
/// [`Button`] (the disk `static_assert` :526-527 becomes the COUNT check in
/// [`button_slot`]).
const BUTTONS: [ButtonSlot; 35] = [
    ButtonSlot { row: 0, bit: 2, name: "Strings" },
    ButtonSlot { row: 0, bit: 3, name: "Bass" },
    ButtonSlot { row: 0, bit: 4, name: "Guitar" },
    ButtonSlot { row: 0, bit: 5, name: "Organ" },
    ButtonSlot { row: 0, bit: 6, name: "Chrom. Perc." },
    ButtonSlot { row: 0, bit: 7, name: "Piano" },
    ButtonSlot { row: 1, bit: 2, name: "Synth pad" },
    ButtonSlot { row: 1, bit: 3, name: "Synth lead" },
    ButtonSlot { row: 1, bit: 4, name: "Pipe" },
    ButtonSlot { row: 1, bit: 5, name: "Reed" },
    ButtonSlot { row: 1, bit: 6, name: "Brass" },
    ButtonSlot { row: 1, bit: 7, name: "Ensemble" },
    ButtonSlot { row: 2, bit: 2, name: "Drum" },
    ButtonSlot { row: 2, bit: 3, name: "Model excl." },
    ButtonSlot { row: 2, bit: 4, name: "SFX" },
    ButtonSlot { row: 2, bit: 5, name: "Percussive" },
    ButtonSlot { row: 2, bit: 6, name: "Ethnic" },
    ButtonSlot { row: 2, bit: 7, name: "Synth effects" },
    ButtonSlot { row: 3, bit: 1, name: "Part +" },
    ButtonSlot { row: 3, bit: 2, name: "Part -" },
    ButtonSlot { row: 3, bit: 3, name: "Mute/Solo" },
    ButtonSlot { row: 3, bit: 4, name: "Effect" },
    ButtonSlot { row: 3, bit: 5, name: "Util" },
    ButtonSlot { row: 3, bit: 6, name: "Edit" },
    ButtonSlot { row: 3, bit: 7, name: "Play" },
    ButtonSlot { row: 4, bit: 1, name: "Value +" },
    ButtonSlot { row: 4, bit: 2, name: "Value -" },
    ButtonSlot { row: 4, bit: 3, name: "Exit" },
    ButtonSlot { row: 4, bit: 4, name: "Select >" },
    ButtonSlot { row: 4, bit: 5, name: "Select <" },
    ButtonSlot { row: 4, bit: 6, name: "Enter" },
    ButtonSlot { row: 4, bit: 7, name: "Seq" },
    ButtonSlot { row: 5, bit: 5, name: "Audition" },
    ButtonSlot { row: 5, bit: 6, name: "Select" },
    ButtonSlot { row: 5, bit: 7, name: "Sampling/Mode" },
];

/// origin: mu2000.h:321-329 `enum class button { ... count }`
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum Button {
    Strings = 0,
    Bass,
    Guitar,
    Organ,
    ChromPerc,
    Piano,
    SynthPad,
    SynthLead,
    Pipe,
    Reed,
    Brass,
    Ensemble,
    Drum,
    ModelExcl,
    Sfx,
    Percussive,
    Ethnic,
    SynthEffects,
    PartPlus,
    PartMinus,
    MuteSolo,
    Effect,
    Util,
    Edit,
    Play,
    ValuePlus,
    ValueMinus,
    Exit,
    SelectRight,
    SelectLeft,
    Enter,
    Seq,
    Audition,
    Select,
    SamplingMode,
    /// mu2000.h:328 `count` — 35
    Count = 35,
}

impl Button {
    /// Disk-side callers pass the enum; tools pass raw ints (C++ `int(b)`).
    /// Out of range -> None (disk clamps by returning "" / no-op).
    pub fn from_i32(i: i32) -> Option<Button> {
        if (0..(Button::Count as i32)).contains(&i) {
            // SAFETY: repr(i32) + full dense range 0..=Count checked above
            Some(unsafe { std::mem::transmute::<i32, Button>(i) })
        } else {
            None
        }
    }
}

/// origin: mu2000.cpp:531-535 `button_name` (out of range -> "")
pub fn button_name(b: Button) -> &'static str {
    let i = b as i32;
    if i >= 0 && i < Button::Count as i32 {
        BUTTONS[i as usize].name
    } else {
        ""
    }
}

// ---------------------------------------------------------------------------
// Hub: `Sh7042Peripherals` — forward every internal-register arm to the real
// device, then drain the FIFOs. The `sh7042.rs` dispatcher already reproduces
// the per-device width case sets of `sh7042_map.hxx`, so the arms below see
// only disk-decoded addresses.
// ---------------------------------------------------------------------------
/// origin: mu2000.cpp:848-857 `hold` — master SWP writes stall the CPU
/// SWP_WRITE_CYCLES (440, mu2000.h:924; measured against real hardware,
/// mu2000.cpp:833-846). Control slots 0x0e/0x0f wait only for the data
/// channels 0x11/0x12/0x26; the slave never waits (:842-843).
/// DEVIATION: the C++ `m_cpu->abort_timeslice()` (:855) is a no-op here —
/// the Rust run loop steps ONE instruction per pass (run_cycles doc, ledger
/// `midi lines` row), so the wait is consumed at the very next loop head
/// (:1211-1217 port there), which is exactly where abort leaves C++.
fn swp_hold(master: bool, reg: u32, wait: &RefCell<u64>) {
    // :847 waits = base == 0x800000
    if !master {
        return;
    }
    let slot = reg & 0x3f; // :849
    let chan = (reg >> 6) & 0x3f; // :850
    let control = slot == 0x0e || slot == 0x0f; // :851
    let data = chan == 0x11 || chan == 0x12 || chan == 0x26; // :852
    if !control || data {
        // :853-856
        *wait.borrow_mut() += SWP_WRITE_CYCLES; // :854
    }
}

impl Sh7042Peripherals for Hub {
    /// W-SAMP1: the machine-side card swap reaches the Hub's `card` clone
    /// through this (`Machine::share_card_from`).
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    // ---- SCI0/1 (sh7042.cpp:237-238; address decode inside Sh2SciPair) ----
    fn sci_r8(&mut self, sci: usize, a: u32) -> u8 {
        let r = self.pair.borrow_mut().sci_r8(sci, a);
        self.drain();
        r
    }
    fn sci_r16(&mut self, sci: usize, a: u32) -> u16 {
        let r = self.pair.borrow_mut().sci_r16(sci, a);
        self.drain();
        r
    }
    fn sci_w8(&mut self, sci: usize, a: u32, v: u8) {
        self.pair.borrow_mut().sci_w8(sci, a, v);
        self.drain();
    }
    fn sci_w16(&mut self, sci: usize, a: u32, v: u16) {
        self.pair.borrow_mut().sci_w16(sci, a, v);
        self.drain();
    }

    // ---- MTU (decode + channels inside Sh2Mtu; cpu_now = mtu.rs:726 seam) ----
    fn mtu_r8(&mut self, a: u32) -> u8 {
        let t = self.cpu_now();
        let r = {
            let mut d = self.mtu.borrow_mut();
            d.set_cpu_now(t);
            Sh7042Peripherals::mtu_r8(&mut *d, a)
        };
        self.drain();
        r
    }
    fn mtu_r16(&mut self, a: u32) -> u16 {
        let t = self.cpu_now();
        let r = {
            let mut d = self.mtu.borrow_mut();
            d.set_cpu_now(t);
            Sh7042Peripherals::mtu_r16(&mut *d, a)
        };
        self.drain();
        r
    }
    fn mtu_w8(&mut self, a: u32, v: u8) {
        let t = self.cpu_now();
        {
            let mut d = self.mtu.borrow_mut();
            d.set_cpu_now(t);
            Sh7042Peripherals::mtu_w8(&mut *d, a, v);
        }
        self.drain();
    }
    fn mtu_w16(&mut self, a: u32, v: u16) {
        let t = self.cpu_now();
        {
            let mut d = self.mtu.borrow_mut();
            d.set_cpu_now(t);
            Sh7042Peripherals::mtu_w16(&mut *d, a, v); // mtu.rs:1079
        }
        self.drain();
    }

    // ---- INTC (decode inside Sh2Intc r8/r16/w8/w16; intc.rs:308-418).
    // w8/w16 return the post-write arbitration result; on disk `update_irq`
    // re-issues m_cpu->set_internal_interrupt (:92) — queue it.
    fn intc_r8(&mut self, a: u32) -> u8 {
        self.intc.borrow().r8(a)
    }
    fn intc_r16(&mut self, a: u32) -> u16 {
        self.intc.borrow().r16(a)
    }
    fn intc_w8(&mut self, a: u32, v: u8) {
        if let Some((lvl, vec)) = self.intc.borrow_mut().w8(a, v) {
            self.q.borrow_mut().push_back(Evt::CpuIrq { level: lvl, vector: vec });
        }
    }
    fn intc_w16(&mut self, a: u32, v: u16) {
        if let Some((lvl, vec)) = self.intc.borrow_mut().w16(a, v) {
            self.q.borrow_mut().push_back(Evt::CpuIrq { level: lvl, vector: vec });
        }
    }
    // execute_set_input / interrupt_taken are driven from Machine::pump only
    // (mu2000.cpp:1040/1132 -> sh7042.cpp:141-143/:397-401; + M7 USB
    // :1341/:1349/:1355/:1367 through QIrq -> Evt::SetInput) — the trait
    // defaults stay dead.

    // ---- CMT (decode inside ShCmt; cpu_now = current_cycles seam, cmt.rs) ----
    fn cmt_r8(&mut self, a: u32) -> u8 {
        let t = self.cpu_now();
        let r = {
            let mut d = self.cmt.borrow_mut();
            d.set_cpu_now(t);
            Sh7042Peripherals::cmt_r8(&mut *d, a)
        };
        self.drain();
        r
    }
    fn cmt_r16(&mut self, a: u32) -> u16 {
        let t = self.cpu_now();
        let r = {
            let mut d = self.cmt.borrow_mut();
            d.set_cpu_now(t);
            Sh7042Peripherals::cmt_r16(&mut *d, a)
        };
        self.drain();
        r
    }
    fn cmt_w8(&mut self, a: u32, v: u8) {
        let t = self.cpu_now();
        {
            let mut d = self.cmt.borrow_mut();
            d.set_cpu_now(t);
            Sh7042Peripherals::cmt_w8(&mut *d, a, v);
        }
        self.drain();
    }
    fn cmt_w16(&mut self, a: u32, v: u16) {
        let t = self.cpu_now();
        {
            let mut d = self.cmt.borrow_mut();
            d.set_cpu_now(t);
            Sh7042Peripherals::cmt_w16(&mut *d, a, v);
        }
        self.drain();
    }

    // ---- BSC / DMAC: pure register files (stubs.rs trait impls) ----
    fn bsc_r8(&mut self, a: u32) -> u8 {
        self.bsc.borrow_mut().bsc_r8(a)
    }
    fn bsc_r16(&mut self, a: u32) -> u16 {
        self.bsc.borrow_mut().bsc_r16(a)
    }
    fn bsc_w8(&mut self, a: u32, v: u8) {
        self.bsc.borrow_mut().bsc_w8(a, v)
    }
    fn bsc_w16(&mut self, a: u32, v: u16) {
        self.bsc.borrow_mut().bsc_w16(a, v)
    }
    fn dmac_r8(&mut self, a: u32) -> u8 {
        self.dmac.borrow_mut().dmac_r8(a)
    }
    fn dmac_r16(&mut self, a: u32) -> u16 {
        self.dmac.borrow_mut().dmac_r16(a)
    }
    fn dmac_w8(&mut self, a: u32, v: u8) {
        self.dmac.borrow_mut().dmac_w8(a, v)
    }
    fn dmac_w16(&mut self, a: u32, v: u16) {
        self.dmac.borrow_mut().dmac_w16(a, v)
    }
    fn dmac_ch_r8(&mut self, ch: usize, a: u32) -> u8 {
        self.dmac.borrow_mut().dmac_ch_r8(ch, a)
    }
    fn dmac_ch_r16(&mut self, ch: usize, a: u32) -> u16 {
        self.dmac.borrow_mut().dmac_ch_r16(ch, a)
    }
    fn dmac_ch_r32(&mut self, ch: usize, a: u32) -> u32 {
        self.dmac.borrow_mut().dmac_ch_r32(ch, a)
    }
    fn dmac_ch_w8(&mut self, ch: usize, a: u32, v: u8) {
        self.dmac.borrow_mut().dmac_ch_w8(ch, a, v)
    }
    fn dmac_ch_w16(&mut self, ch: usize, a: u32, v: u16) {
        self.dmac.borrow_mut().dmac_ch_w16(ch, a, v)
    }
    fn dmac_ch_w32(&mut self, ch: usize, a: u32, v: u32) {
        self.dmac.borrow_mut().dmac_ch_w32(ch, a, v)
    }

    // ---- ADC0/ADC1 (die-A, both SH_ADC_MS — `Adc` port above). Map case
    // sets: r8 sh7042_map.hxx:200-237, r16 :420-438, w8 :701-706, w16
    // :887-889 (w16 at 8410/8412 CROSSES both ADCs). Byte width picks the
    // shifted addr half (>>8 / >>0) like the map. IRQ vectors (:274) go to
    // the Evt queue -> pump -> route_irqs (disk synchronous internal_interrupt).
    fn adc0_r8(&mut self, a: u32) -> u8 {
        let peak = self.ad_peak.borrow();
        let uh = self.usb_host.get(); // :1151 lambda reads m_usb_host live
        let mut d = self.adc0.borrow_mut();
        match a {
            0xffff_83e0 | 0xffff_8410 => d.adcsr_r(), // :200/:234
            0xffff_83e1 | 0xffff_8412 => d.adcr_r(),  // :201/:236
            0xffff_83f0..=0xffff_83ff => {
                // :202-217
                let v = d.addr_r(((a - 0xffff_83f0) >> 1) as usize, &|p| ad_pin(p, &peak, uh));
                if a & 1 == 0 { (v >> 8) as u8 } else { v as u8 }
            }
            0xffff_8400..=0xffff_8407 => {
                // :218-225 (window 2 of the same AN0-3)
                let v = d.addr_r(((a - 0xffff_8400) >> 1) as usize, &|p| ad_pin(p, &peak, uh));
                if a & 1 == 0 { (v >> 8) as u8 } else { v as u8 }
            }
            _ => 0,
        }
    }
    fn adc0_r16(&mut self, a: u32) -> u16 {
        let peak = self.ad_peak.borrow();
        let uh = self.usb_host.get(); // :1151 lambda reads m_usb_host live
        let pin = |p: usize| ad_pin(p, &peak, uh);
        let mut d = self.adc0.borrow_mut();
        match a {
            0xffff_83e0 => ((d.adcsr_r() as u16) << 8) | d.adcr_r() as u16, // :420
            0xffff_83f0..=0xffff_83fe => d.addr_r(((a - 0xffff_83f0) >> 1) as usize, &pin), // :421-428
            0xffff_8400..=0xffff_8406 => d.addr_r(((a - 0xffff_8400) >> 1) as usize, &pin), // :429-432
            0xffff_8410 => {
                // :437 combined across BOTH adc adcsrs (die-A)
                let hi = d.adcsr_r() as u16;
                let lo = self.adc1.borrow().adcsr_r() as u16;
                (hi << 8) | lo
            }
            0xffff_8412 => {
                let hi = d.adcr_r() as u16; // :438
                let lo = self.adc1.borrow().adcr_r() as u16;
                (hi << 8) | lo
            }
            _ => 0,
        }
    }
    fn adc0_w8(&mut self, a: u32, v: u8) {
        // origin: sh7042_map.hxx:701-703
        let total = self.hn.borrow().now; // total_cycles() mid-instr (:188)
        let peak = self.ad_peak.borrow();
        let uh = self.usb_host.get(); // :1151 lambda reads m_usb_host live
        let pin = |p: usize| ad_pin(p, &peak, uh);
        let mut aq = std::mem::take(&mut self.buf);
        match a {
            0xffff_83e0 | 0xffff_8410 => self.adc0.borrow_mut().adcsr_w(v, total, &pin),
            0xffff_83e1 | 0xffff_8412 => self.adc0.borrow_mut().adcr_w(v),
            _ => {}
        }
        if !aq.is_empty() {
            self.q.borrow_mut().push_back(Evt::Irq(std::mem::take(&mut aq)));
        }
        self.buf = aq;
    }
    fn adc0_w16(&mut self, a: u32, v: u16) {
        // origin: sh7042_map.hxx:887-889 — 8410/8412 write BOTH ADCs
        let (hi, lo) = ((v >> 8) as u8, v as u8);
        let total = self.hn.borrow().now;
        let peak = self.ad_peak.borrow();
        let uh = self.usb_host.get(); // :1151 lambda reads m_usb_host live
        let pin = |p: usize| ad_pin(p, &peak, uh);
        let mut aq = std::mem::take(&mut self.buf);
        match a {
            0xffff_83e0 => {
                let mut d = self.adc0.borrow_mut();
                d.adcsr_w(hi, total, &pin); // :887
                d.adcr_w(lo);
            }
            0xffff_8410 => {
                self.adc0.borrow_mut().adcsr_w(hi, total, &pin); // :888
                self.adc1.borrow_mut().adcsr_w(lo, total, &pin);
            }
            0xffff_8412 => {
                self.adc0.borrow_mut().adcr_w(hi); // :889
                self.adc1.borrow_mut().adcr_w(lo);
            }
            _ => {}
        }
        if !aq.is_empty() {
            self.q.borrow_mut().push_back(Evt::Irq(std::mem::take(&mut aq)));
        }
        self.buf = aq;
    }
    fn adc1_r8(&mut self, a: u32) -> u8 {
        // origin: sh7042_map.hxx:226-237
        let peak = self.ad_peak.borrow();
        let uh = self.usb_host.get(); // :1151 lambda reads m_usb_host live
        let mut d = self.adc1.borrow_mut();
        match a {
            0xffff_8411 => d.adcsr_r(), // :235
            0xffff_8413 => d.adcr_r(),  // :237
            0xffff_8408..=0xffff_840f => {
                let v = d.addr_r(((a - 0xffff_8408) >> 1) as usize, &|p| ad_pin(p, &peak, uh));
                if a & 1 == 0 { (v >> 8) as u8 } else { v as u8 }
            }
            _ => 0,
        }
    }
    fn adc1_r16(&mut self, a: u32) -> u16 {
        // origin: sh7042_map.hxx:433-436 (adc1 has no other r16)
        let peak = self.ad_peak.borrow();
        let uh = self.usb_host.get(); // :1151 lambda reads m_usb_host live
        let pin = |p: usize| ad_pin(p, &peak, uh);
        let mut d = self.adc1.borrow_mut();
        match a {
            0xffff_8408..=0xffff_840e => d.addr_r(((a - 0xffff_8408) >> 1) as usize, &pin),
            _ => 0,
        }
    }
    fn adc1_w8(&mut self, a: u32, v: u8) {
        // origin: sh7042_map.hxx:704/:706 (adc1 w8 ONLY 8411/8413)
        let total = self.hn.borrow().now;
        let peak = self.ad_peak.borrow();
        let uh = self.usb_host.get(); // :1151 lambda reads m_usb_host live
        let pin = |p: usize| ad_pin(p, &peak, uh);
        let mut aq = std::mem::take(&mut self.buf);
        match a {
            0xffff_8411 => self.adc1.borrow_mut().adcsr_w(v, total, &pin),
            0xffff_8413 => self.adc1.borrow_mut().adcr_w(v),
            _ => {}
        }
        if !aq.is_empty() {
            self.q.borrow_mut().push_back(Evt::Irq(std::mem::take(&mut aq)));
        }
        self.buf = aq;
    }
    fn adc0_update(&mut self, current_time: u64) -> u64 {
        // origin: sh7042.cpp:282 -> sh_adc.cpp:174-181
        let total = self.hn.borrow().now;
        let peak = self.ad_peak.borrow();
        let uh = self.usb_host.get(); // :1151 lambda reads m_usb_host live
        let pin = |p: usize| ad_pin(p, &peak, uh);
        let mut aq = std::mem::take(&mut self.buf);
        let e = self.adc0.borrow_mut().internal_update(current_time, total, &pin, &mut aq);
        if !aq.is_empty() {
            self.q.borrow_mut().push_back(Evt::Irq(std::mem::take(&mut aq)));
        }
        self.buf = aq;
        e
    }
    fn adc1_update(&mut self, current_time: u64) -> u64 {
        // origin: sh7042.cpp:283-284 (die-A — sh7042.rs gates on m_die_a)
        let total = self.hn.borrow().now;
        let peak = self.ad_peak.borrow();
        let uh = self.usb_host.get(); // :1151 lambda reads m_usb_host live
        let pin = |p: usize| ad_pin(p, &peak, uh);
        let mut aq = std::mem::take(&mut self.buf);
        let e = self.adc1.borrow_mut().internal_update(current_time, total, &pin, &mut aq);
        if !aq.is_empty() {
            self.q.borrow_mut().push_back(Evt::Irq(std::mem::take(&mut aq)));
        }
        self.buf = aq;
        e
    }

    // ---- PORT A (32-bit, base 0xffff8380: PDR=dr@+0, PDDR=io@+4 — port
    // row). Writes have NO disk delegate (mu2000 wires no write_porta): the
    // Some arms drop into port32_default_w logerror (sh7042.cpp:106-109,
    // VERBOSE off). Reads carry the dial/card pins closure (:1083-1105).
    fn porta_r8(&mut self, a: u32) -> u8 {
        let off = a - 0xffff_8380;
        let v = if off < 4 {
            self.porta.borrow_mut().dr_r(|| self.porta_pins())
        } else {
            self.porta.borrow().io_r()
        };
        (v >> (8 * (3 - (off & 3)))) as u8
    }
    fn porta_r16(&mut self, a: u32) -> u16 {
        let off = a - 0xffff_8380;
        let v = if off < 4 {
            self.porta.borrow_mut().dr_r(|| self.porta_pins())
        } else {
            self.porta.borrow().io_r()
        };
        if off & 2 == 0 {
            (v >> 16) as u16
        } else {
            v as u16
        }
    }
    fn porta_r32(&mut self, a: u32) -> u32 {
        if a == 0xffff_8380 {
            self.porta.borrow_mut().dr_r(|| self.porta_pins())
        } else {
            self.porta.borrow().io_r() // map:489 io_r
        }
    }
    fn porta_w8(&mut self, a: u32, v: u8) {
        let off = a - 0xffff_8380;
        let shift = 8 * (3 - (off & 3));
        let (data, mask) = ((v as u32) << shift, 0xffu32 << shift);
        let _ = if off < 4 {
            self.porta.borrow_mut().dr_w(data, mask)
        } else {
            self.porta.borrow_mut().io_w(data, mask)
        };
    }
    fn porta_w16(&mut self, a: u32, v: u16) {
        let off = a - 0xffff_8380;
        let (data, mask) = if off & 2 == 0 {
            ((v as u32) << 16, 0xffff_0000u32)
        } else {
            (v as u32, 0x0000_ffffu32)
        };
        let _ = if off < 4 {
            self.porta.borrow_mut().dr_w(data, mask)
        } else {
            self.porta.borrow_mut().io_w(data, mask)
        };
    }
    fn porta_w32(&mut self, a: u32, v: u32) {
        let _ = if a == 0xffff_8380 {
            self.porta.borrow_mut().dr_w(v, 0xffff_ffff)
        } else {
            self.porta.borrow_mut().io_w(v, 0xffff_ffff)
        };
    }

    // ---- PORT D (32-bit, base 0xffff83a0) — un-wired on disk: reads hit the
    // 0xffff default (sh7042.cpp:99-104), writes drop (io stays 0).
    fn portd_r8(&mut self, a: u32) -> u8 {
        let off = a - 0xffff_83a0;
        let v = if off < 4 {
            self.portd.borrow_mut().dr_r(|| 0xffff_ffff)
        } else {
            self.portd.borrow().io_r()
        };
        (v >> (8 * (3 - (off & 3)))) as u8
    }
    fn portd_r16(&mut self, a: u32) -> u16 {
        let off = a - 0xffff_83a0;
        let v = if off < 4 {
            self.portd.borrow_mut().dr_r(|| 0xffff_ffff)
        } else {
            self.portd.borrow().io_r()
        };
        if off & 2 == 0 {
            (v >> 16) as u16
        } else {
            v as u16
        }
    }
    fn portd_r32(&mut self, a: u32) -> u32 {
        if a == 0xffff_83a0 {
            self.portd.borrow_mut().dr_r(|| 0xffff_ffff)
        } else {
            self.portd.borrow().io_r()
        }
    }
    fn portd_w8(&mut self, a: u32, v: u8) {
        let off = a - 0xffff_83a0;
        let shift = 8 * (3 - (off & 3));
        let _ = if off < 4 {
            self.portd.borrow_mut().dr_w((v as u32) << shift, 0xffu32 << shift)
        } else {
            self.portd.borrow_mut().io_w((v as u32) << shift, 0xffu32 << shift)
        };
    }
    fn portd_w16(&mut self, a: u32, v: u16) {
        let off = a - 0xffff_83a0;
        let (data, mask) = if off & 2 == 0 {
            ((v as u32) << 16, 0xffff_0000u32)
        } else {
            (v as u32, 0x0000_ffffu32)
        };
        let _ = if off < 4 {
            self.portd.borrow_mut().dr_w(data, mask)
        } else {
            self.portd.borrow_mut().io_w(data, mask)
        };
    }
    fn portd_w32(&mut self, a: u32, v: u32) {
        let _ = if a == 0xffff_83a0 {
            self.portd.borrow_mut().dr_w(v, 0xffff_ffff)
        } else {
            self.portd.borrow_mut().io_w(v, 0xffff_ffff)
        };
    }

    // ---- PORT B / C (16-bit, bases 0xffff8390 / 0xffff8392) — un-wired:
    // reads 0xffff default (sh7042.cpp:87-92), writes drop or latch silently.
    fn portb_r8(&mut self, a: u32) -> u8 {
        let off = a - 0xffff_8390;
        let v = if off < 4 {
            self.portb.borrow_mut().dr_r(|| 0xffff)
        } else {
            self.portb.borrow().io_r()
        };
        (v >> (8 * (1 - (off & 1)))) as u8
    }
    fn portb_r16(&mut self, a: u32) -> u16 {
        if a == 0xffff_8390 {
            self.portb.borrow_mut().dr_r(|| 0xffff)
        } else {
            self.portb.borrow().io_r()
        }
    }
    fn portb_w8(&mut self, a: u32, v: u8) {
        let off = a - 0xffff_8390;
        let s = 8 * (1 - (off & 1));
        let _ = if off < 4 {
            self.portb.borrow_mut().dr_w((v as u16) << s, 0xffu16 << s)
        } else {
            self.portb.borrow_mut().io_w((v as u16) << s, 0xffu16 << s)
        };
    }
    fn portb_w16(&mut self, a: u32, v: u16) {
        let _ = if a == 0xffff_8390 {
            self.portb.borrow_mut().dr_w(v, 0xffff)
        } else {
            self.portb.borrow_mut().io_w(v, 0xffff)
        };
    }
    fn portc_r8(&mut self, a: u32) -> u8 {
        let off = a - 0xffff_8392;
        let v = if off < 4 {
            self.portc.borrow_mut().dr_r(|| 0xffff)
        } else {
            self.portc.borrow().io_r()
        };
        (v >> (8 * (1 - (off & 1)))) as u8
    }
    fn portc_r16(&mut self, a: u32) -> u16 {
        if a == 0xffff_8392 {
            self.portc.borrow_mut().dr_r(|| 0xffff)
        } else {
            self.portc.borrow().io_r()
        }
    }
    fn portc_w8(&mut self, a: u32, v: u8) {
        let off = a - 0xffff_8392;
        let s = 8 * (1 - (off & 1));
        let _ = if off < 4 {
            self.portc.borrow_mut().dr_w((v as u16) << s, 0xffu16 << s)
        } else {
            self.portc.borrow_mut().io_w((v as u16) << s, 0xffu16 << s)
        };
    }
    fn portc_w16(&mut self, a: u32, v: u16) {
        let _ = if a == 0xffff_8392 {
            self.portc.borrow_mut().dr_w(v, 0xffff)
        } else {
            self.portc.borrow_mut().io_w(v, 0xffff)
        };
    }

    // ---- PORT E (16-bit, base 0xffff83b0) — THE LCD. Write fires
    // lcd_port_w(dr&io) on EVERY write (port.rs fire-on-every-write quirk);
    // reads carry lcd_port_r pins (mu2000.cpp:1123-1124).
    fn porte_r8(&mut self, a: u32) -> u8 {
        let off = a - 0xffff_83b0;
        let v = if off < 4 {
            self.porte.borrow_mut().dr_r(|| self.porte_pins())
        } else {
            self.porte.borrow().io_r()
        };
        (v >> (8 * (1 - (off & 1)))) as u8
    }
    fn porte_r16(&mut self, a: u32) -> u16 {
        if a == 0xffff_83b0 {
            self.porte.borrow_mut().dr_r(|| self.porte_pins())
        } else {
            self.porte.borrow().io_r()
        }
    }
    fn porte_w8(&mut self, a: u32, v: u8) {
        let off = a - 0xffff_83b0;
        let s = 8 * (1 - (off & 1));
        let (data, mask) = ((v as u16) << s, 0xffu16 << s);
        if off < 4 {
            if let Some((d, _ddr)) = self.porte.borrow_mut().dr_w(data, mask) {
                self.porte_write(d);
            }
        } else if let Some((d, _ddr)) = self.porte.borrow_mut().io_w(data, mask) {
            self.porte_write(d);
        }
    }
    fn porte_w16(&mut self, a: u32, v: u16) {
        if a == 0xffff_83b0 {
            if let Some((d, _ddr)) = self.porte.borrow_mut().dr_w(v, 0xffff) {
                self.porte_write(d);
            }
        } else if let Some((d, _ddr)) = self.porte.borrow_mut().io_w(v, 0xffff) {
            self.porte_write(d);
        }
    }

    // ---- PORT F (read-only, base 0xffff83b2): mu2000 wires NO portf
    // delegate — disk reads return the 0xffff port16_default_r
    // (sh7042.cpp:87-92; buttons reach the panel through ledsw_r instead).
    fn portf_r8(&mut self, a: u32) -> u8 {
        let off = a - 0xffff_83b2;
        let v = if off < 4 {
            self.portf.borrow_mut().dr_r(|| 0xffff)
        } else {
            self.portf.borrow().io_r()
        };
        (v >> (8 * (1 - (off & 1)))) as u8
    }
    fn portf_r16(&mut self, a: u32) -> u16 {
        if a == 0xffff_83b2 {
            self.portf.borrow_mut().dr_r(|| 0xffff)
        } else {
            self.portf.borrow().io_r()
        }
    }

    // ---- scheduler seams — call order fixed by sh7042.cpp:282-292 ----
    // ADC0/ADC1 never wired (cancelled row): trait defaults return 0 ==
    // disk add_event skip (sh7042.cpp:256-257).
    fn cmt_update(&mut self, current_time: u64) -> u64 {
        let r = {
            let mut d = self.cmt.borrow_mut();
            d.set_cpu_now(current_time);
            Sh7042Peripherals::cmt_update(&mut *d, current_time)
        };
        self.drain();
        r
    }
    fn mtu_ch_update(&mut self, ch: usize, current_time: u64) -> u64 {
        let r = {
            let mut d = self.mtu.borrow_mut();
            d.set_cpu_now(current_time);
            Sh7042Peripherals::mtu_ch_update(&mut *d, ch, current_time)
        };
        self.drain();
        r
    }
    fn sci_update(&mut self, sci: usize, current_time: u64) -> u64 {
        let r = self.pair.borrow_mut().sci_update(sci, current_time);
        self.drain();
        r
    }

    // ---- M5-W2 state seams (sh7042.cpp:412-430) — route each call to the
    // real device; `Sh7042::state` already enforces the disk call order.
    // No drain needed: state() never moves IRQ state.
    fn intc_state(&mut self, s: &mut StateIo) {
        self.intc.borrow_mut().state(s); // sh7042.cpp:412
    }
    fn adc0_state(&mut self, s: &mut StateIo) {
        self.adc0.borrow_mut().state(s); // :413
    }
    fn adc1_state(&mut self, s: &mut StateIo) {
        self.adc1.borrow_mut().state(s); // :417-418
    }
    fn bsc_state(&mut self, s: &mut StateIo) {
        self.bsc.borrow_mut().state(s); // :419
    }
    fn cmt_state(&mut self, s: &mut StateIo) {
        self.cmt.borrow_mut().state(s); // :420
    }
    fn dmac_state(&mut self, s: &mut StateIo) {
        self.dmac.borrow_mut().state(s); // :421 (shared DMAOR)
    }
    fn dmac_ch_state(&mut self, ch: usize, s: &mut StateIo) {
        self.dmac.borrow_mut().channels[ch].state(s); // :422 (m_dmac0..3)
    }
    fn mtu_state(&mut self, s: &mut StateIo) {
        self.mtu.borrow_mut().state(s); // :423
    }
    fn mtu_ch_state(&mut self, ch: usize, s: &mut StateIo) {
        self.mtu.borrow_mut().ch[ch].state(s); // :424-425 (m_mtu0..4)
    }
    /// porta..portf (sh7042.cpp:426-427); a/d are the 32-bit devices, b/c/e/f
    /// 16-bit (creation :231-236) — each emits its own "port32"/"port16" tag.
    fn port_state(&mut self, port: usize, s: &mut StateIo) {
        match port {
            0 => self.porta.borrow_mut().state(s),
            1 => self.portb.borrow_mut().state(s),
            2 => self.portc.borrow_mut().state(s),
            3 => self.portd.borrow_mut().state(s),
            4 => self.porte.borrow_mut().state(s),
            _ => self.portf.borrow_mut().state(s),
        }
    }
    fn sci_state(&mut self, sci: usize, s: &mut StateIo) {
        self.pair.borrow_mut().sci[sci].state(s); // :428-430 (m_sci[i].lookup())
    }

    // ---- SWP30 window (mu2000.cpp:861-918 wrappers). reg = (a - base)>>1,
    // base = master 0x800000 / slave 0x802000 (mu2000.cpp:921-922). Only r16
    // reads and w16/w32 writes are traced (the disk r8/w8 lambdas have no
    // fprintf); read trace prints AFTER the device read (:862-864), write trace
    // fires BEFORE the device write (:906-916 / :881-903).
    fn swp_r8(&mut self, master: bool, a: u32) -> u8 {
        // mu2000.cpp:877-880 d.r8 — NO trace line; byte lane = (a&1)?0:8.
        let base = if master { MASTER_BASE } else { SLAVE_BASE };
        let reg = (a - base) >> 1;
        let v = self.swp_dev(master).borrow_mut().read16(reg);
        (v >> if a & 1 != 0 { 0 } else { 8 }) as u8
    }
    fn swp_r16(&mut self, master: bool, a: u32) -> u16 {
        // mu2000.cpp:861-866 d.r16 — trace only when reads-on.
        let base = if master { MASTER_BASE } else { SLAVE_BASE };
        let reg = (a - base) >> 1;
        let v = self.swp_dev(master).borrow_mut().read16(reg);
        {
            let mut sink = self.swp_sink.borrow_mut();
            if sink.f.is_some() && sink.reads {
                let hn = self.hn.borrow();
                let line = swp_line("R ", base, reg, v, hn.pc, hn.now);
                sink.f.as_mut().unwrap().write_all(line.as_bytes()).ok();
            }
        }
        v
    }
    fn swp_w8(&mut self, master: bool, a: u32, v: u8) {
        // mu2000.cpp:869-876 d.w8 — 16-bit RMW, byte lane=(a&1); NO trace line.
        let base = if master { MASTER_BASE } else { SLAVE_BASE };
        let reg = (a - base) >> 1;
        let dev = self.swp_dev(master);
        let old = dev.borrow_mut().read16(reg);
        let merged = if a & 1 != 0 {
            (old & 0xff00) | u16::from(v) // :873 low byte lane
        } else {
            (old & 0x00ff) | (u16::from(v) << 8) // :874 high byte lane
        };
        dev.borrow_mut().write16(reg, merged);
        swp_hold(master, reg, &self.swp_wait); // :875
    }
    fn swp_w16(&mut self, master: bool, a: u32, v: u16) {
        // mu2000.cpp:906-918 d.w16 — trace BEFORE the device write.
        let base = if master { MASTER_BASE } else { SLAVE_BASE };
        let reg = (a - base) >> 1;
        {
            let mut sink = self.swp_sink.borrow_mut();
            if sink.f.is_some() {
                let pfx = if sink.reads { "W " } else { "" };
                let hn = self.hn.borrow();
                let line = swp_line(pfx, base, reg, v, hn.pc, hn.now);
                sink.f.as_mut().unwrap().write_all(line.as_bytes()).ok();
            }
        }
        self.swp_dev(master).borrow_mut().write16(reg, v);
        swp_hold(master, reg, &self.swp_wait); // :917
    }
    fn swp_w32(&mut self, master: bool, a: u32, v: u32) {
        // mu2000.cpp:881-905 d.w32 — TWO trace lines (hi@reg, lo@reg+1) BEFORE
        // the two device writes (:902-903).
        let base = if master { MASTER_BASE } else { SLAVE_BASE };
        let reg = (a - base) >> 1;
        {
            let mut sink = self.swp_sink.borrow_mut();
            if sink.f.is_some() {
                let pfx = if sink.reads { "W " } else { "" };
                let hn = self.hn.borrow();
                let hi = swp_line(pfx, base, reg, (v >> 16) as u16, hn.pc, hn.now);
                let lo = swp_line(pfx, base, reg + 1, v as u16, hn.pc, hn.now);
                let f = sink.f.as_mut().unwrap();
                f.write_all(hi.as_bytes()).ok();
                f.write_all(lo.as_bytes()).ok();
            }
        }
        let dev = self.swp_dev(master);
        dev.borrow_mut().write16(reg, (v >> 16) as u16); // :902
        dev.borrow_mut().write16(reg + 1, v as u16); // :903
        swp_hold(master, reg, &self.swp_wait); // :904 — ONCE, not twice (disk truth)
    }
    fn sci4_r8(&mut self, _a: u32) -> u8 {
        0 // unreachable: Ctx intercepts 0xf00000..=0xf0003f
    }
    fn sci4_w8(&mut self, _a: u32, _v: u8) {}
    fn usb_r8(&mut self, sel: u32) -> u8 {
        // mu2000.cpp:1011 -> usb_r (mu2000.cpp:1359-1369) — M7 `USB host`
        // row. Width demotion (word = r8 pair on the same window,
        // membus.h:75/:81) is the bus arm (sh7042.rs Dev::Usb); `sel` may
        // exceed 1 exactly like disk (only `sel & 1` is tested there).
        // The disk's mid-instruction execute_set_input(3,0) (:1367) rides
        // the Evt FIFO; the per-instruction pump applies it at the same
        // post-execute m_test_irq boundary (module-doc note).
        let mut irq = QIrq { q: &self.q };
        self.usb.borrow_mut().r(sel, &mut irq)
    }
    fn usb_w8(&mut self, sel: u32, v: u8) {
        self.usb.borrow_mut().w(sel, v) // mu2000.cpp:1012 -> :1371-1378
    }
    fn ledsw_r8(&mut self) -> u8 {
        0 // unreachable: Ctx intercepts 0xc80000
    }
    fn ledsw1_w8(&mut self, _v: u8) {}
    fn ledsw2_w8(&mut self, _v: u8) {}
    fn d80_r8(&mut self) -> u8 {
        0
    }
    fn d80_w8(&mut self, _v: u8) {}
    fn card_data_r8(&mut self) -> u8 {
        // mu2000.cpp:985 m_card.data_r() — no-card/CE-low/idle read is 0xFF
        // (smartmedia.cpp:247-248), NOT the old M4 stub's 0
        self.card.borrow_mut().data_r()
    }
    fn card_data_w8(&mut self, v: u8) {
        self.card.borrow_mut().data_w(v) // mu2000.cpp:986
    }
    fn card_ctrl_w8(&mut self, v: u8) {
        self.card.borrow_mut().control_w(v) // mu2000.cpp:993
    }
}

// ---------------------------------------------------------------------------
// Per-instruction bus view (module doc "Ownership model").
// ---------------------------------------------------------------------------

/// Instruction-head snapshot written by `RunHook` — the exception signature
/// reference (sh2_exception_internal pushes SR at r15-4, then PC at r15-8:
/// core.rs:1911-1914, then reads vbr+vec*4: core.rs:1925).
#[derive(Default, Clone, Copy)]
pub struct Snap {
    pub sr: u32,
    pub r15: u32,
    pub vbr: u32,
}

pub struct Ctx<'a> {
    // origin: mu2000.cpp:740-828 (regions) + 984-995 (internal window) — the
    // Sh7042Bus port owns that decode; only the machine-side windows below
    // are intercepted here.
    pub bus: &'a mut smu_sh2::sh7042::Sh7042Bus,
    pub rm: &'a mut RunningMachine,
    pub sci4: &'a RefCell<smu_dev::sci4::Core>,
    pub hn: Rc<RefCell<HubNow>>,
    pub snap: Rc<RefCell<Snap>>,
    pub q: Rc<RefCell<VecDeque<Evt>>>,
    pub ledsw1: &'a mut u8, // mu2000.h:881
    pub ledsw2: &'a mut u8, // mu2000.h:881
    pub d80: &'a mut u8,    // mu2000.h:883
    pub sws: Rc<RefCell<[u8; 6]>>,
}

impl Ctx<'_> {
    /// origin: mu2000.cpp:965-972 (m_sci4->read8/write8 with `a - 0xf00000`).
    /// M9: sci4 reads/writes move `irq_line` (sync_sci4 leg) — batch-stopper.
    #[inline]
    fn sci4_r(&mut self, a: u32) -> u8 {
        self.bus.dev_dirty.set(true);
        let off = a - SCI4_BASE;
        let v = self.sci4.borrow_mut().read8(&mut *self.rm, off);
        v
    }
    #[inline]
    fn sci4_w(&mut self, a: u32, v: u8) {
        self.bus.dev_dirty.set(true);
        let off = a - SCI4_BASE;
        self.sci4.borrow_mut().write8(&mut *self.rm, off, v);
    }
    #[inline]
    fn ledsw_r(&self) -> u8 {
        // origin: mu2000.cpp:531-538 — fold the rows selected by m_ledsw1
        let sel = *self.ledsw1;
        let mut res: u8 = 0xff;
        let rows = self.sws.borrow();
        for i in 0..6u32 {
            if (sel >> i) & 1 != 0 {
                res &= rows[i as usize];
            }
        }
        res
    }
}

impl Sh2Bus for Ctx<'_> {
    fn read_byte(&mut self, a: u32) -> u8 {
        if (SCI4_BASE..=SCI4_END).contains(&a) {
            return self.sci4_r(a); // mu2000.cpp:969
        }
        match a {
            LED_ADDR => self.ledsw_r(), // mu2000.cpp:929 ledsw_r()
            D80_ADDR => *self.d80,       // mu2000.cpp:942 (m_d80)
            // USB window: NO intercept since the M7 row — falls to the bus
            // Dev::Usb arm -> Hub::usb_r8 (disk lambda mu2000.cpp:1011).
            _ => self.bus.read_byte(a),  // regions + internal (Hub wired)
        }
    }

    fn read_word(&mut self, a: u32) -> u16 {
        let a = a & !1u32; // membus.h:75 `a &= ~1u`
        if (SCI4_BASE..=SCI4_END - 1).contains(&a) {
            // r8 chain demotion (mu2000.cpp:969 registers r8 only;
            // membus.h:81 `(d->r8(a) << 8) | d->r8(a + 1)` — BOTH bytes call
            // the sci4 read8 handler, like the r32 chain below at :852-858)
            let hi = self.sci4_r(a) as u16;
            let lo = self.sci4_r(a + 1) as u16;
            return (hi << 8) | lo;
        }
        match a {
            // single-byte windows demoted via r8: the lambdas IGNORE their
            // address (mu2000.cpp:929 `d.r8 = [this](offs_t) { return
            // ledsw_r(); }`, :942 `return m_d80`), so membus.h:81 fires the
            // handler twice — lo byte is a SECOND real scan/read, not 0
            // (same reason read_long :863 says "four real scans").
            LED_ADDR => {
                let hi = self.ledsw_r() as u16;
                let lo = self.ledsw_r() as u16;
                (hi << 8) | lo
            }
            D80_ADDR => {
                let hi = *self.d80 as u16;
                let lo = *self.d80 as u16;
                (hi << 8) | lo
            }
            // USB word: falls to the bus — Dev::Usb r8-pair demotion
            // (membus.h:81, TWO real handler calls — hi=usb_r(0) CLEARS
            // have and drops IRQ3 before lo=usb_r(1) reads the status;
            // faithful to the disk lambda pair, sh7042.rs :1116-1124)
            // M7 usb.rs row.
            // ROM/RAM/DRAM/IRAM regions + internal register space + card
            // windows (CONTROL keeps its literal 0xff inside the bus,
            // mu2000.cpp:960) + SWP stubs — all through the membus.h port.
            _ => self.bus.read_word(a),
        }
    }

    fn read_long(&mut self, a: u32) -> u32 {
        let a = a & !3u32; // membus.h:88
        if (SCI4_BASE..=SCI4_END - 3).contains(&a) {
            // r8 chain demotion (mu2000.cpp:969 registers r8 only)
            let b0 = self.sci4_r(a) as u32;
            let b1 = self.sci4_r(a + 1) as u32;
            let b2 = self.sci4_r(a + 2) as u32;
            let b3 = self.sci4_r(a + 3) as u32;
            return (b0 << 24) | (b1 << 16) | (b2 << 8) | b3;
        }
        match a {
            LED_ADDR => {
                let r = self.ledsw_r() as u32;
                (r << 24) | (r << 16) | (r << 8) | r // :929 four real scans
            }
            D80_ADDR => {
                let r = *self.d80 as u32;
                (r << 24) | (r << 16) | (r << 8) | r
            }
            _ => self.bus.read_long(a),
        }
    }

    fn write_byte(&mut self, a: u32, v: u8) {
        if (SCI4_BASE..=SCI4_END).contains(&a) {
            self.sci4_w(a, v); // mu2000.cpp:970
            return;
        }
        match a {
            LED_ADDR => *self.ledsw1 = v, // mu2000.cpp:930 (m_ledsw1 = v)
            PANEL_ADDR => *self.ledsw2 = v, // mu2000.cpp:936 (m_ledsw2 = v)
            D80_ADDR => *self.d80 = v,    // mu2000.cpp:945 (m_d80)
            // CARD windows fall through to the bus since this row
            // (Dev::CardData/CardCtrl -> Hub::card_data_w8/card_ctrl_w8;
            // CONTROL read keeps its literal 0xff in the bus, :992)
            // SWP falls through to the bus (Dev::Swpm/Swps -> Hub::swp_w8) :921-922
            // USB window: NO intercept since the M7 row — bus Dev::Usb ->
            // Hub::usb_w8 (disk lambda mu2000.cpp:1012 usb_w(a-0xf80000))
            _ => self.bus.write_byte(a, v),
        }
    }

    fn write_word(&mut self, a: u32, v: u16) {
        let a = a & !1u32;
        if (SCI4_BASE..=SCI4_END - 1).contains(&a) {
            self.sci4_w(a, (v >> 8) as u8); // BE demotion: hi byte first
            self.sci4_w(a + 1, v as u8);
            return;
        }
        match a {
            // single-byte windows: demotion writes byte@a (hi) then @a+1
            // (unmapped, drops) — final latch = the hi byte of the access.
            LED_ADDR => *self.ledsw1 = (v >> 8) as u8,
            PANEL_ADDR => *self.ledsw2 = (v >> 8) as u8,
            D80_ADDR => *self.d80 = (v >> 8) as u8,
            // SWP falls through to the bus (Dev::Swpm/Swps -> Hub::swp_w16)
            _ => self.bus.write_word(a, v),
        }
    }

    fn write_long(&mut self, a: u32, v: u32) {
        let a = a & !3u32;
        if (SCI4_BASE..=SCI4_END - 3).contains(&a) {
            self.sci4_w(a, (v >> 24) as u8);
            self.sci4_w(a + 1, (v >> 16) as u8);
            self.sci4_w(a + 2, (v >> 8) as u8);
            self.sci4_w(a + 3, v as u8);
            return;
        }
        match a {
            LED_ADDR => *self.ledsw1 = (v >> 24) as u8,
            PANEL_ADDR => *self.ledsw2 = (v >> 24) as u8,
            D80_ADDR => *self.d80 = (v >> 24) as u8,
            // SWP falls through to the bus (Dev::Swpm/Swps -> Hub::swp_w32)
            _ => self.bus.write_long(a, v),
        }
    }

    /// M9 (2026-10-03): end the CPU burst at the first instruction that
    /// touched a device space with pump-visible side effects (dev_dirty set
    /// in the `Sh7042Bus` arms / Ctx sci4 intercept / `exception_taken`).
    /// The machine pump block then runs exactly where the per-instruction
    /// loop ran it; region-only stretches batch through, like the C++
    /// `m_cpu->run_cycles(chunk)` (mu2000.cpp:1219) does.
    fn batch_stop(&self) -> bool {
        self.bus.dev_dirty.take()
    }

    /// origin: mu2000 Ctx = sh7042_device in the disk stack — the
    /// sh2_exception_internal override tail (sh7042.cpp:400). The core calls
    /// this AFTER the base exception (pushes + vector fetch); queueing keeps
    /// the single-writer Evt discipline, Machine::pump applies it before the
    /// handler's first instruction (run_cycles(1) per-instruction pump).
    /// `irqline` is unused on disk (intc.rs interrupt_taken, sh_intc.cpp:66-73).
    fn exception_taken(&mut self, vector: u32) {
        self.bus.dev_dirty.set(true); // M9: ack must reach pump before handler insn
        self.q.borrow_mut().push_back(Evt::Ack { vector: vector as i32 });
    }
}

// ---------------------------------------------------------------------------
// Instruction hook — the mamecompat.h:75-79 PC_HASH/PC_TRACE pair fired at
// the debugger_instruction_hook site (see core.rs:69-79 doc), plus the ack
// snapshot. Fired BEFORE the opcode fetch, AFTER delay-slot application —
// exactly the C++ position (sh2.cpp:268).
// ---------------------------------------------------------------------------
pub struct RunHook<'a> {
    pub hash: bool,  // g_pc_hash != nullptr (boot.cpp:94-96)
    pub trace: bool, // g_pc_trace != nullptr (boot.cpp:84-89)
    pub snap: Rc<RefCell<Snap>>,
    pub hn: Rc<RefCell<HubNow>>,
    /// M9 (2026-10-03): per-instruction clock seam moved here from the
    /// machine inner-loop pre-block (identical order/values — see
    /// `instruction`): `s.cpu_now` = the OLD `hn.now` − 1 (== pre of the
    /// PREVIOUS instruction, exactly what the per-instruction loop fed the
    /// SCI RX grid), then `hn.now` = this instruction's pre.
    pub pair: &'a RefCell<Sh2SciPair>,
}

impl InstructionHook for RunHook<'_> {
    fn instruction(&mut self, core: &Sh2Core) {
        // M9 batch clock sync — mirrors the retired pre-block
        // (2026-10-03, lib.rs @ 552-gate): cc read BEFORE hn.now update.
        let pre = core.total_cycles();
        {
            let cc = self.hn.borrow().cpu_now();
            let mut p = self.pair.borrow_mut();
            for s in p.sci.iter_mut() {
                s.cpu_now = cc;
            }
        }
        self.hn.borrow_mut().now = pre;
        // origin: src/compat/mamecompat.h:75-79 — hash fold first, then
        // g_pc_cycles = total_cycles() + pc_trace(pc, regs_text()).
        if self.hash {
            paths::pc_hash(core.pc, core.regs_hash());
        }
        if self.trace {
            paths::set_pc_cycles(core.total_cycles());
            // Build the register-text argument ONLY when the sink will emit
            // this instruction (see paths::pc_trace_will_emit); the skip tail
            // must not pay the per-instruction `format!`. pc_trace still runs
            // every call so g_pc_skip/g_pc_trace_left advance exactly as C++.
            if paths::pc_trace_will_emit() {
                paths::pc_trace(core.pc, &core.regs_text());
            } else {
                paths::pc_trace(core.pc, "");
            }
        }
        // PC for the SWP `--trace-swp` sink = the value `m_cpu->pc()` holds
        // DURING this instruction's memory accesses (mu2000.cpp:864). MAME's
        // SH2 advances pc at the fetch, BEFORE execute (sh2.cpp:274-282 ==
        // core.rs step-3: `m_delay != 0 ? m_delay : pc+2`), so the pc a bus
        // handler sees is that POST-fetch value, not the head-hook value.
        // Replicating step-3 exactly: a delay-slot store reports the branch
        // target (`m_delay`); an ordinary 16-bit store reports `pc + 2` (the
        // 32-bit extension word fetch never moves pc, so `+2` holds there too).
        let pc_exec = if core.m_delay != 0 { core.m_delay } else { core.pc.wrapping_add(2) };
        self.hn.borrow_mut().pc = pc_exec;
        let mut s = self.snap.borrow_mut();
        s.sr = core.sr;
        s.r15 = core.r[15];
        s.vbr = core.vbr;
    }
}

// ---------------------------------------------------------------------------
// Machine
// ---------------------------------------------------------------------------

/// origin: mu2000.cpp:336-338 `#define SLAVE_SPINS 20000` — spin this many
/// empty polls before parking on `go.wait` (a block gap in a DAW would burn
/// a core otherwise; between samples of one block the wait is sub-µs and
/// stays spinning — 「まず回して待つ。眠っていては 44100 回/秒には間に
/// 合わない」mu2000.cpp:350-355).
const SLAVE_SPINS: u32 = 20_000;

/// origin: mu2000.cpp:66 `static std::atomic<int> g_live_instances{0}` —
/// ++ in the ctor (:70), -- in the dtor (:289); bounds the thread fan-out.
static LIVE_INSTANCES: AtomicI32 = AtomicI32::new(0);

/// origin: mu2000.cpp:299-307 `threaded_max()` — static-once; env
/// `SMU2000_THREADED_MAX` (atoi, clamped ≥0) else
/// `max(1, hardware_concurrency/4)`. DEVIATION: `available_parallelism()`
/// instead of `std::thread::hardware_concurrency()` (honors the process
/// affinity mask; same value when unrestricted — the disk rule is 「台数が
/// 論理コア数の 1/4 以下のときだけ別スレッド」).
fn threaded_max() -> i32 {
    static N: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        if let Ok(e) = std::env::var("SMU2000_THREADED_MAX") {
            c_atoi(&e).max(0) // :303 std::max(0, std::atoi(e))
        } else {
            let hc = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
            ((hc as i32) / 4).max(1) // :304
        }
    })
}

/// C `atoi` prefix parse (blocktime.rs:136 helper, local copy so the
/// machine crate stays dependency-free like mu2000.cpp using ::atoi).
fn c_atoi(s: &str) -> i32 {
    let t = s.trim_start();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let mut v: i64 = 0;
    for c in digits.bytes() {
        if !c.is_ascii_digit() {
            break;
        }
        v = v * 10 + (c - b'0') as i64;
        if v > i32::MAX as i64 {
            v = i32::MAX as i64;
        }
    }
    if neg {
        -(v as i32)
    } else {
        v as i32
    }
}

/// The slave thread's captured pointers — the disk lambda capture `[this]`
/// (mu2000.cpp:333) split into its members:
/// - `swp`: `m_swps` (`Rc<RefCell<Swp30>>` interior — the RcBox never
///   moves while a strong reference exists; mu2000.cpp:922/:370),
/// - `wave`: `m_swps.set_wave_rom(m_wave->data(), size())` (mu2000.cpp:410
///   — pinned at load; bins never reassign `Machine::wave` afterwards),
/// - `sintab`: `m_swps.set_sintab(...)` (mu2000.cpp:419, via
///   `Machine::set_sintab_pin`).
/// - `sampram`: `m_swps.set_sample_ram(m_sampram.data(), size())` →
///   `m_wave_cache.set_overlay` (mu2000.cpp:90 + mamecompat.h:170-175).
///   Disk carries the overlay INSIDE the device cache; the Rust `Wave` is
///   rebuilt per sample, so the W-SAMP2 overlay rides along here too
///   (else the threaded slave's fetch could never see sampling RAM).
/// SAFETY: the go/done release/acquire handshake (mu2000.cpp:3450-3455 /
/// :357-371) makes thread ownership EXCLUSIVE per sample — see the field
/// notes on `Machine::slave_*`. The RefCell borrow flag and all SWP state
/// transfer through the publish/wait pair, so nothing ever races; the disk
/// is the identical object graph with `m_swps` shared by both threads.
struct SlaveRaws {
    swp: *const RefCell<Swp30>,
    wave: *const u8,
    wave_len: usize,
    sintab: *const u16,
    sintab_len: usize,
    sampram: *const u8,
    sampram_words: u32,
}
unsafe impl Send for SlaveRaws {}

/// `s32 m_slave_l, m_slave_r` (mu2000.h:943) behind the handshake.
/// SAFETY: written by the slave immediately before the `done` release,
/// read by the master immediately after the `done` acquire
/// (mu2000.cpp:369-371 / :3454-3457) — the release/acquire pair orders
/// every access; nothing is ever concurrent.
pub struct SlaveOutCell(UnsafeCell<(i32, i32)>);
unsafe impl Send for SlaveOutCell {}
unsafe impl Sync for SlaveOutCell {}
impl SlaveOutCell {
    #[inline]
    fn set(&self, v: (i32, i32)) {
        unsafe { *self.0.get() = v }; // m_slave_l = ...; m_slave_r = ...
    }
    #[inline]
    fn get(&self) -> (i32, i32) {
        unsafe { *self.0.get() } // ls = m_slave_l; rs = m_slave_r
    }
}

/// The sleeper's wake target for the `atomic::wait/notify` stand-in
/// (disk mu2000.cpp:363 `m_slave_go.wait(seen)` / :325/:3452
/// `notify_one` — `atomic_wait` is not stable on this rustc, so the park
/// carries the sleeper and the master's `kick_slave` (unpark) carries the
/// wake-up; µs-class latency, same "眠っていたら起こす" behavior).
/// SAFETY: the Option is WRITTEN exactly once per thread — the slave's
/// very first act, before `parked` is ever set true — and READ by the
/// master only when `parked` is observed true (the slave's
/// `parked.store(Release)` orders the write; the master's `Acquire` load
/// gives the happens-before). Between threads the slot is reset by the
/// master alone, AFTER join / BEFORE spawn. Nothing is ever concurrent.
pub struct SlaveWakeCell(UnsafeCell<Option<std::thread::Thread>>);
unsafe impl Send for SlaveWakeCell {}
unsafe impl Sync for SlaveWakeCell {}

/// origin: mu2000.h:935-943 — the slave-thread handoff. 「合図は atomic の
/// 回し合いで、錠は使わない」: go/done rotate through atomics only, no
/// lock; 44100 handoffs/second so the slave spins before it parks. The
/// four counters are MACHINE members (mu2000.h:941-943) and survive
/// stop/start cycles here exactly as on disk — `apply_threading` re-arms
/// from the CURRENT go count (mu2000.cpp:330-332), never re-running a tag.
pub struct Machine {
    /// compat clock/timer queue (mu2000.h `running_machine m_machine`)
    pub rm: RunningMachine,
    /// CPU + program bus (mu2000.cpp:72 make<sh7043a_device> + :997)
    pub soc: Sh7042,
    /// PLG-board serial (mu2000.cpp:78 "board absent, firmware pokes regs")
    pub sci4: Sci4,
    // ---- reset/pump handles (clones of the Hub's) ----
    pub pair: Rc<RefCell<Sh2SciPair>>,
    pub mtu: Rc<RefCell<Sh2Mtu>>,
    pub intc: Rc<RefCell<Sh2Intc>>,
    pub cmt: Rc<RefCell<ShCmt>>,
    pub bsc: Rc<RefCell<ShBsc>>,
    pub dmac: Rc<RefCell<ShDmac>>,
    pub porta: Rc<RefCell<ShPort32>>,
    pub portb: Rc<RefCell<ShPort16>>,
    pub portc: Rc<RefCell<ShPort16>>,
    pub portd: Rc<RefCell<ShPort32>>,
    pub porte: Rc<RefCell<ShPort16>>,
    pub portf: Rc<RefCell<ShPort16>>,
    pub lcd: Rc<RefCell<Hd44780>>,
    pub sws: Rc<RefCell<[u8; 6]>>,
    pub enc: Rc<RefCell<EncState>>,
    /// origin: mu2000.h `smartmedia m_card` — state leg (mu2000.cpp:3531-3533
    /// v>=5, M5-W3a) AND the live bus object (mu2000.cpp:985-986/993 +
    /// PA19/PA20 :1121-1125) behind one Rc: the Hub arms and `Hub::porta_pins`
    /// drive the SAME card (this row).
    pub card: Rc<RefCell<card::Card>>,
    pub adc0: Rc<RefCell<Adc>>, // sh7042.cpp:167 (MS, base 0, vec 136)
    pub adc1: Rc<RefCell<Adc>>, // :168 (MS, base 4, vec 137)
    pub ad_peak: Rc<RefCell<[i32; 2]>>, // mu2000.h:869 {0,0}
    pub hn: Rc<RefCell<HubNow>>,
    pub snap: Rc<RefCell<Snap>>,
    pub q: Rc<RefCell<VecDeque<Evt>>>,
    /// mu2000.cpp:921-922 SWP30 master + slave (reg-dispatch row) and the
    /// shared `--trace-swp` sink (mu2000.h:914-915; set by `set_swp_trace`).
    pub swpm: Rc<RefCell<Swp30>>,
    pub swps: Rc<RefCell<Swp30>>,
    pub swp_sink: Rc<RefCell<SwpSink>>,
    /// mu2000.h:925 `m_swp_wait = 0` (SWP_WRITE_CYCLES = 440, mu2000.h:924)
    pub swp_wait: Rc<RefCell<u64>>,
    // ---- machine-side latches (mu2000.h explicit defaults — Invariant 3) ----
    pub ledsw1: u8, //  :888 = 0
    pub ledsw2: u8, //  :888 = 0
    pub d80: u8,    //  :890 = 0
    pub m_sci_irq: [i32; 2], // :905 {0,0} — sci4 irq0/1 shadow (mu2000.cpp:1130-1131).
    // i32 like disk (state leg :3544 streams `int` = 4 bytes; M5-W4 widened
    // from i8 — sync_sci4 casts the Core::irq_line i8 through `as i32`).
    pub m_sci_irq3: i8,     //  irq3 shadow (:1132; NOT on the state stream — :3544 arr is [2])
    pub pe: u16,            // :898 `u16 m_pe = 0` — encoder phase latch (panel state leg :3543;
                            // the runtime PORTA seam lives in EncState — W5 re-tap note)
    pub enc_pending: i32,   // :896 `int m_enc_pending = 0` — panel state leg :3543 (runtime
                            // PORTA seam lives in EncState — W5 re-tap note)
    /// SWP30 sampling RAM (mu2000.h:874 `m_sampram`; ctor mu2000.cpp:88
    /// assign(0x400000, 0)). Rides the state stream (mu2000.cpp:3530,
    /// IN-PLACE `StateIo::mem` — the buffer never reallocs, so the
    /// `set_sample_ram` device pins taken at ctor stay valid).
    /// W-SAMP1: the overlay wiring landed — both SWP30 instances carry a
    /// raw pin on THIS buffer (`Swp30::set_sample_ram` ← swp30.h:53),
    /// the fetch path overlays reads (fetch.rs `Wave`), and the rec/
    /// download windows (0x5000/0x7000/0x8000/0x9000) write through it.
    pub sampram: Vec<u8>,
    // ---- run-loop counters (mu2000.h members) ----
    pub overrun: u64, // m_overrun
    pub cycle_debt: u64, // :925 `m_cycle_debt = 0` — state leg :3545; the
                        // render.rs statetest run_sample helper threads it
                        // through `&mut m.cycle_debt` (mu2000.cpp:3232-3234)
    pub loops: u64,   // m_loops (:1169)
    pub timer_fires: u64, // m_timer_fires (:1176)
    pub event_fires: u64, // m_event_fires (:1185)
    /// --trace-upd sink (boot.cpp:90-91; writes CRLF on Windows)
    pub upd: Option<std::fs::File>,
    /// hook gates mirroring g_pc_hash / g_pc_trace nullness
    pub hash_on: bool,
    pub trace_on: bool,
    /// wave ROM bytes (mu2000.cpp:395-402 loaded, parked until the M3 row)
    pub wave: Vec<u8>,
    /// origin: mu2000.h:868 m_ad_in[2] = {} (A/D INPUT, set_audio_input :258;
    /// mixer row B carries the 0 stub — the A/D capture row feeds it)
    pub ad_in: [i32; 2],
    /// DIN MIDI lines + cable routing + USB receiver half (mu2000.h:981-1015;
    /// `midi lines` row — the render/port_b feed lands here)
    pub midi: midi::Midi,
    /// reusable PIN-ring drain buffer for `drain_sci_pins` (the disk
    /// `write_sci_tx<0>` devcb call, mu2000.cpp:1179). Preallocated to the
    /// sci.rs ring bound — no allocation in run_cycles (Invariant 6 shape)
    tx_pin_buf: Vec<(u8, bool)>, // sci.rs:1049-1055 poll_pin sink
    /// reusable SCI irq-FIFO drain buffer for the fast_midi head sync
    /// (run_cycles; same no-alloc shape as tx_pin_buf)
    sci_irq_buf: Vec<i32>,
    // ---- threaded slave (M8; mu2000.h:935-944) --------------------------
    /// `std::thread m_slave_thread` (mu2000.h:937) — `slave.is_some()`
    /// == `m_slave_thread.joinable()` (`threaded()`, mu2000.h:254)
    pub slave: Option<JoinHandle<()>>,
    /// `std::atomic<u64> m_slave_go{0}` (mu2000.h:941). Arc only so the
    /// spawned closure can own a handle to the SAME counter the machine
    /// publishes (the disk thread shares `this`; Rust closures need a
    /// 'static handle — the object stays a Machine member, mu2000-
    /// member-for-member otherwise).
    pub slave_go: Arc<AtomicU64>,
    /// `std::atomic<u64> m_slave_done{0}` (mu2000.h:941)
    pub slave_done: Arc<AtomicU64>,
    /// `std::atomic<bool> m_slave_quit{false}` (mu2000.h:942)
    pub slave_quit: Arc<AtomicBool>,
    /// `s32 m_slave_l, m_slave_r = 0` (mu2000.h:943) — slave writes, then
    /// releases `done` (mu2000.cpp:369-371); master acquire-spins then
    /// reads (:3454-3457). Never touched while the other side runs.
    /// (Wrapper cell because `Arc<UnsafeCell>` alone is not Send; the
    /// SAFETY argument is identical to `SlaveRaws`.)
    pub slave_out: Arc<SlaveOutCell>,
    /// DEVIATION support members for the park/unpark stand-in (see
    /// `SlaveWakeCell`): the futex word's "someone sleeps here" bit and
    /// the wake target. Not on disk (C++ futexes are invisible).
    pub slave_parked: Arc<AtomicBool>,
    pub slave_waker: Arc<SlaveWakeCell>,
    /// `bool m_want_threaded = false` (mu2000.h:938)
    want_threaded: bool,
    /// `u32 m_thread_check = 0` (mu2000.h:939) — instance-count re-check
    /// every 8192 samples (mu2000.cpp:3226-3227)
    thread_check: u32,
    /// sin table pinned for the SWP30 devices — the deferred
    /// `set_sintab_rom` glue (mu2000.cpp:413-419). The bins call
    /// `set_sintab_pin` right after `roms::load_sintab`, exactly where the
    /// disk pins `m_swpm/m_swps.set_sintab` (:418-419). Only the slave
    /// thread dereferences it (SlaveCtl::sintab), under the handshake.
    sintab_pin: *const u16,
    sintab_len: usize,
}

impl Machine {
    /// Construct the machine around an already-loaded program ROM
    /// (attach-before-reset == `set_program_bus` at build time,
    /// mu2000.cpp:383-393/997 — the reset vector fetch needs it live).
    /// Every field explicit (ledger Invariant 3 — no `Default::default()`).
    // origin: mu2000::mu2000 ctor :68-104 (cpu :72, device_add_mconfig :75 =
    // the Hub below, sci4 :78, set_clock_hz :81, RAM zero-fill inside
    // Sh7042Bus::new mu2000.cpp:85-87, build_bus :103/712-997)
    pub fn new(prog: Vec<u8>) -> Machine {
        // mu2000.cpp:70 `g_live_instances++` (ctor) — bounds the slave
        // thread fan-out via `threaded_max()` (mu2000.cpp:318).
        LIVE_INSTANCES.fetch_add(1, Ordering::SeqCst); // :++ is seq_cst
        let mut rm = RunningMachine::new();
        rm.set_clock_hz(CPU_HZ); // mu2000.cpp:81 — before any timer exists
        let mut soc = Sh7042::new_a(CPU_HZ, prog); // mu2000.cpp:72 (SH7043A, die_a)
        let sci4 = Sci4::new(SCI4_HZ); // mu2000.cpp:78 (default 8 MHz)
        let hn = Rc::new(RefCell::new(HubNow { now: 0, in_event: false, pc: 0 }));
        let q = Rc::new(RefCell::new(VecDeque::new()));
        let pair = Rc::new(RefCell::new(Sh2SciPair::new())); // :237-238
        let mtu = Rc::new(RefCell::new(Sh2Mtu::new())); // :178-229
        let intc = Rc::new(RefCell::new(Sh2Intc::new())); // :165
        let porta = Rc::new(RefCell::new(ShPort32::porta())); // :231
        let portb = Rc::new(RefCell::new(ShPort16::portb())); // :232
        let portc = Rc::new(RefCell::new(ShPort16::portc())); // :233
        let portd = Rc::new(RefCell::new(ShPort32::portd())); // :234
        let porte = Rc::new(RefCell::new(ShPort16::porte())); // :235
        let portf = Rc::new(RefCell::new(ShPort16::portf())); // :236
        let cmt = Rc::new(RefCell::new(ShCmt::new(144, 148))); // :172
        let bsc = Rc::new(RefCell::new(ShBsc::new())); // :171
        let dmac = Rc::new(RefCell::new(ShDmac::new())); // :173
        let lcd = Rc::new(RefCell::new(Hd44780::new(CPU_HZ, LCD_HZ))); // mu2000.h m_lcd
        let sws = Rc::new(RefCell::new([0xffu8; 6])); // mu2000.h:885
        let enc = Rc::new(RefCell::new(EncState { pending: 0, high: true })); // :889-890
        // mu2000.h `smartmedia m_card` — empty slot (smartmedia.h:75-90
        // member initializers == Card::new; this row shares it with the Hub)
        let card = Rc::new(RefCell::new(card::Card::new()));
        let adc0 = Rc::new(RefCell::new(Adc::new_ms(0, 136))); // sh7042.cpp:167
        let adc1 = Rc::new(RefCell::new(Adc::new_ms(4, 137))); // :168
        let ad_peak = Rc::new(RefCell::new([0i32; 2])); // mu2000.h:869
        // mu2000.cpp:921-922 the two SWP30 devices + the shared --trace-swp
        // sink (mu2000.h:914-915, installed later via set_swp_trace).
        let swpm = Rc::new(RefCell::new(Swp30::new())); // master @0x800000
        let swps = Rc::new(RefCell::new(Swp30::new())); // slave @0x802000
        let swp_sink = Rc::new(RefCell::new(SwpSink { f: None, reads: false }));
        let swp_wait = Rc::new(RefCell::new(0u64)); // mu2000.h:925 m_swp_wait = 0
        // mu2000.h:1021-1022 m_usb + m_usb_host (M7 row) — born inside Midi
        // (the paired receiver half + state stream live there), SHARED with
        // the Hub bus arms below (the disk lambdas mu2000.cpp:1011-1012).
        let midi = midi::Midi::new(); // mu2000.h:990-1015 (every field explicit)

        // build_bus internal wiring (mu2000.cpp:984-995): the internal
        // register window of the bus dispatches into the Hub.
        soc.bus.periph = Some(Box::new(Hub {
            hn: Rc::clone(&hn),
            pair: Rc::clone(&pair),
            mtu: Rc::clone(&mtu),
            intc: Rc::clone(&intc),
            porta: Rc::clone(&porta),
            portb: Rc::clone(&portb),
            portc: Rc::clone(&portc),
            portd: Rc::clone(&portd),
            porte: Rc::clone(&porte),
            portf: Rc::clone(&portf),
            cmt: Rc::clone(&cmt),
            bsc: Rc::clone(&bsc),
            dmac: Rc::clone(&dmac),
            lcd: Rc::clone(&lcd),
            sws: Rc::clone(&sws),
            enc: Rc::clone(&enc),
            card: Rc::clone(&card), // mu2000.cpp:985-986/993 + :1121-1125
            adc0: Rc::clone(&adc0),
            adc1: Rc::clone(&adc1),
            ad_peak: Rc::clone(&ad_peak),
            q: Rc::clone(&q),
            swpm: Rc::clone(&swpm),
            swps: Rc::clone(&swps),
            swp_sink: Rc::clone(&swp_sink),
            swp_wait: Rc::clone(&swp_wait),
            usb: Rc::clone(&midi.usb),         // mu2000.h:1021 (shared M7)
            usb_host: Rc::clone(&midi.usb_host), // :1022 (AN4 lambda :1151)
            buf: Vec::new(),
        }));

        // mu2000.cpp:88 — the sampling RAM (4 MB, zero-filled)
        let sampram: Vec<u8> = vec![0u8; 0x400000];
        // mu2000.cpp:89-90 — install the overlay on BOTH devices.
        // SAFETY: raw-pin discipline, same as `sintab_pin` — the buffer is
        // owned by this Machine (`sampram` field, moved below; moves keep
        // the heap address), never reallocs (state restore is in-place
        // `StateIo::mem`), and outlives every pin user (both Rc<RefCell<
        // Swp30>> + their clones inside this Machine). Both devices share
        // the ONE window exactly like the single C++ `m_sampram.data()`.
        {
            let (base, len) = (sampram.as_ptr() as *mut u8, sampram.len());
            swpm.borrow_mut().set_sample_ram(base, len); // swp30.h:53
            swps.borrow_mut().set_sample_ram(base, len);
        }

        Machine {
            rm,
            soc,
            sci4,
            pair,
            mtu,
            intc,
            cmt,
            bsc,
            dmac,
            porta,
            portb,
            portc,
            portd,
            porte,
            portf,
            lcd,
            sws,
            enc,
            card, // mu2000.h m_card (shared with the Hub — smartmedia.h:75-90 inits)
            adc0,
            adc1,
            ad_peak,
            hn,
            snap: Rc::new(RefCell::new(Snap::default())),
            q,
            swpm,
            swps,
            swp_sink,
            swp_wait,
            ledsw1: 0,
            ledsw2: 0,
            d80: 0,
            m_sci_irq: [0, 0],
            m_sci_irq3: 0,
            pe: 0,                          // mu2000.h:898 = 0 (M5-W4 state leg :3543)
            enc_pending: 0,                 // mu2000.h:896 = 0 (M5-W4 state leg :3543)
            sampram,                         // mu2000.cpp:88 (pinned above, :89-90)
            overrun: 0,
            cycle_debt: 0,                  // mu2000.h:925 = 0 (M5-W4 state leg :3545)
            loops: 0,
            timer_fires: 0,
            event_fires: 0,
            upd: None,
            hash_on: false,
            trace_on: false,
            wave: Vec::new(),
            ad_in: [0, 0], // mu2000.h:868 = {}
            midi, // mu2000.h:990-1015 (built above; the usb Rcs are shared)
            tx_pin_buf: Vec::with_capacity(smu_sh2::periph::sci::PIN_RING), // sci.rs:114
            sci_irq_buf: Vec::with_capacity(8), // ctor prealloc; reused drained
            // mu2000.h:937-943 threaded slave — every member explicit. The
            // thread is default-constructed (NOT joinable); m_want_threaded
            // = false; m_thread_check = 0; go/done = 0; quit = false;
            // m_slave_l/m_slave_r = 0 (:369 re-zeroes them per run too).
            slave: None,
            slave_go: Arc::new(AtomicU64::new(0)), // mu2000.h:941
            slave_done: Arc::new(AtomicU64::new(0)), // :941
            slave_quit: Arc::new(AtomicBool::new(false)), // :942
            slave_out: Arc::new(SlaveOutCell(UnsafeCell::new((0, 0)))), // :943
            slave_parked: Arc::new(AtomicBool::new(false)), // stand-in state
            slave_waker: Arc::new(SlaveWakeCell(UnsafeCell::new(None))), // stand-in state
            want_threaded: false, // mu2000.h:938
            thread_check: 0,      // mu2000.h:939
            sintab_pin: std::ptr::null(), // set_sintab_rom NOT called yet (:416-417)
            sintab_len: 0,
        }
    }

    /// origin: boot.cpp:60-81 — load program + wave from the ROM dir
    /// (argv[1]-style argument; NO env fallback, session-C finding), then
    /// reset. The sin-table (boot.cpp:68-69, warning-only) serves the SWP
    /// sintab stand-in — M3 — skipped here with a note.
    pub fn boot(dir: &str) -> Result<Machine, String> {
        let prog = roms::load_program(&format!("{dir}/mu2000_flash.bin"))?;
        let wave = roms::load_wave(&format!("{dir}/dump"))?;
        let mut m = Machine::new(prog);
        m.set_wave_rom(wave); // mu2000.cpp:395-402 + :404-411 (device pins)
        m.reset();
        Ok(m)
    }

    /// origin: mu2000.cpp:404-411 `set_wave_rom` — stash the ROM, then pin
    /// BOTH SWP devices (the register-path `wave_read_dword` base,
    /// swp30.cpp:1919-1922). The heap buffer never moves after this point:
    /// `run_sample_pair`'s take/restore round-trip keeps the same address.
    pub fn set_wave_rom(&mut self, wave: Vec<u8>) {
        self.wave = wave; // :406 (an empty ROM rides through, pinning skipped)
        if self.wave.is_empty() {
            return; // :407-408
        }
        let (base, len) = (self.wave.as_ptr(), self.wave.len());
        self.swpm.borrow_mut().set_wave_rom(base, len); // :409
        self.swps.borrow_mut().set_wave_rom(base, len); // :410
    }

    /// origin: mu2000.h:271 `sample_ram()` — the shared 4 MB sampling RAM
    /// (read view for samptest / the recorder; the state stream and the
    /// device overlay pins address the SAME buffer).
    pub fn sample_ram(&self) -> &Vec<u8> {
        &self.sampram
    }

    /// W-SAMP1 card-sharing seam — the samptest harness runs two machines
    /// behind ONE card (`h.mu.card() = g.mu.card()`). Point this machine's
    /// handle AND the Hub's (the bus arms mu2000.cpp:985-993 and the
    /// PA19/PA20 pins closure :1115-1136 read the Hub clone) at `other`'s
    /// card; afterwards a mutation through either machine is visible in
    /// both, and PA19/PA20 follow on both consoles.
    pub fn share_card_from(&mut self, other: &Machine) {
        let card = Rc::clone(&other.card);
        self.card = Rc::clone(&card);
        if let Some(hub) = self
            .soc
            .bus
            .periph
            .as_mut()
            .and_then(|p| p.as_any_mut())
            .and_then(|a| a.downcast_mut::<Hub>())
        {
            hub.card = card;
        }
    }

    /// origin: mu2000.cpp:1043-1048 `start_devices` — "MAME calls in
    /// creation order; we call in creation order". Every device's Rust
    /// `new()` already folded in its `device_start` (periph-row design),
    /// so only the CPU and the sci4 timer births do work here.
    fn start_devices(&mut self) {
        self.soc.device_start(); // sh7042.cpp:118-139 (creation #1: sh2 + pcf zeros)
        // :165-238 mconfig fan-out (intc..sci1) — new() covers (no-op here).
        self.sci4.device_start(&mut self.rm); // mu2000.cpp:78 — born LAST
        // (sci4.cpp:76-79 birth order tx0,rx0,tx1,rx1,...: compat tie-break)
    }

    /// origin: mu2000::reset :1051-1153 (the boot entry the traces start from).
    pub fn reset(&mut self) {
        // :1053 m_cc_last memset — MIDI CC cache (M4 row, no boot effect).
        // :1061-1064 cable reset (midi row; disk order: BEFORE lcd.reset
        // :1126). Queues deliberately NOT cleared on disk — bytes survive.
        // :1086-1091 (M37640 host-present comment) — cmd queue and cur_cmd
        // reset BEFORE the cable loop (disk order; queues survive otherwise).
        {
            let mut u = self.midi.usb.borrow_mut();
            u.cmd.clear(); // :1090 m_usb.cmd.clear()
            u.cur_cmd = false; // :1091
        }
        // :1092-1096 cable reset (midi row; disk order: BEFORE lcd.reset
        // :1158). Queues deliberately NOT cleared on disk — bytes survive.
        self.midi.reset_cables();
        // :1097-1099 — 実機の M37640 は、PC に繋がっていると「ホストが居る」を
        // 知らせてくる（状態の bit6 を立てて F4 03 01 01 01。0x43810 が受け、
        // 0x43DAD1 を 1 にする）。これが来ないと HOST SELECT USB の firmware は
        // 0x1167CE で「HOST Is Offline!」を出す (:1086-1089). set_usb_host(true)
        // must have fired BEFORE this reset (mu2000.h:218-220).
        if self.midi.usb_host.get() {
            let mut u = self.midi.usb.borrow_mut();
            for b in [0xf4u8, 0x03, 0x01, 0x01, 0x01] {
                u.cmd.push_back(b); // :1098
            }
        }
        // :1083-1122 read_porta/read_adc bindings — static Hub closures.
        self.lcd.borrow_mut().reset(); // :1126 m_lcd.reset()
        // :1128-1132 sci4 write_irq<0/1/3> bindings — realized by
        // Machine::sync_sci4 diffing Sci4::Core::irq_line after every step.
        // :1135-1136 m_swpm/m_swps.set_rand_seed (0x9d14abd7 / 0x6c1f35e9) —
        // the SWP LCG is a later (voice/MEG noise) row; no rand state here yet.
        self.swpm.borrow_mut().reset(); // :1137 m_swpm.reset()
        self.swps.borrow_mut().reset(); // :1138 m_swps.reset()
        self.pair.borrow_mut().do_rx_w(0, 1); // :1141 sci_rx_w<0>(1) line idle high
        self.pair.borrow_mut().do_rx_w(1, 1); // :1142
        // :1145-1147 m_tx ring + write_sci_tx<0> — MIDI OUT (M4). The
        // disk-faithful clear (mu2000.cpp:1177-1178): r/w wrap to 0, bit back
        // to -1; m_tx_cur/m_tx_buf deliberately NOT cleared (disk doesn't —
        // both are fully overwritten before any take observes them). The
        // :1179 devcb bind lives at the pump sites as `drain_sci_pins`
        // (ch0 PIN_TX only; PIN_CLK and ch1 are UNBOUND on disk — rg
        // mu2000.cpp write_sci_clk/write_sci_tx: only :1179 ch0 matches).
        self.midi.tx_r = 0; // mu2000.cpp:1177 (m_tx_r = m_tx_w = 0)
        self.midi.tx_w = 0; // :1177
        self.midi.tx_bit = -1; // :1178 (ctor agrees: mu2000.h:1030)
        self.start_devices(); // :1149
        self.reset_devices(); // :1151-1152
        // :1183-1184 device_reset() fires do_sci_tx(1) (sh_sci.cpp:367 ->
        // sci.rs:322) synchronously through the :1179 bind — flush the
        // born-high edges now (each is a tx_line no-op at bit=-1,
        // mu2000.cpp:1273-1278), so the ring is empty at loop entry
        self.drain_sci_pins();
    }

    /// origin: mu2000.cpp:1151-1152 `for d : m_devices d->device_reset()` —
    /// creation order: CPU first (its reset fetches the ROM vectors), then
    /// the mconfig peripherals (:165-238), then sci4.
    fn reset_devices(&mut self) {
        self.soc.device_reset(); // sh7042.cpp:146-149 -> sh2_device::device_reset
        self.intc.borrow_mut().device_reset(); // sh_intc.cpp device_reset (intc.rs:96)
        // adc0/adc1: creation order sh7042.cpp:167-168 (BEFORE bsc :171)
        self.adc0.borrow_mut().device_reset(); // sh_adc.cpp:149-164
        self.adc1.borrow_mut().device_reset();
        self.bsc.borrow_mut().device_reset(); // stubs.rs:85
        self.cmt.borrow_mut().device_reset(); // cmt.rs:121
        self.dmac.borrow_mut().device_reset(); // stubs.rs:369 (channels too)
        self.mtu.borrow_mut().device_reset(); // mtu.rs:707 (channels too)
        self.porta.borrow_mut().device_reset(); // port.rs:219 (NO-OP disk)
        self.portb.borrow_mut().device_reset();
        self.portc.borrow_mut().device_reset();
        self.portd.borrow_mut().device_reset();
        self.porte.borrow_mut().device_reset();
        self.portf.borrow_mut().device_reset();
        self.pair.borrow_mut().device_reset(); // sci.rs:1052
        self.sci4.device_reset(); // sci4.rs:565 (timers survive, disk-verified)
    }

    /// origin: boot.cpp:90-91 `g_upd_trace = fopen(updtrace, "w")`.
    pub fn set_upd_trace(&mut self, path: &str) -> bool {
        self.upd = std::fs::File::create(path).ok();
        self.upd.is_some()
    }
    /// origin: boot.cpp:94-97 `g_pc_hash = fopen(...)` — folds route through
    /// `paths::pc_hash` (compat row).
    pub fn set_hash_pc(&mut self, path: &str) -> bool {
        self.hash_on = paths::open_pc_hash(path);
        self.hash_on
    }
    /// origin: boot.cpp:84-89 `g_pc_trace`/skip/left.
    pub fn set_trace_pc(&mut self, path: &str, skip: u64, left: u64) -> bool {
        self.trace_on = paths::open_pc_trace(path, skip, left);
        self.trace_on
    }

    /// origin: mu2000.h:353-354 `set_swp_trace(f, with_reads)` (boot.cpp:78).
    /// The caller (boot.rs) did the `fopen` + `書けない:` check (boot.cpp:72-77)
    /// and hands over the already-open handle, exactly like the C++ `FILE *tf`.
    pub fn set_swp_trace(&mut self, f: std::fs::File, with_reads: bool) {
        let mut sink = self.swp_sink.borrow_mut();
        sink.f = Some(f);
        sink.reads = with_reads;
    }

    /// origin: mu2000.cpp:537-551 `set_button` — drives the shared `m_sws`
    /// rows (active-low), state leg mu2000.cpp:3542. DEVIATION
    /// (ledger-disclosed): disk's `panel_touched()` (:550) sets
    /// `m_panel_hold`, consumed ONLY by the native-engine run path
    /// (:1063, :3294-3297) — that path is not ported (AGENTS ignore list),
    /// so the hold has no Rust meaning and the firmware-path machine runs
    /// full speed anyway. The sws effect is identical.
    pub fn set_button(&mut self, b: Button, pressed: bool) {
        let i = b as i32;
        if !(0..(Button::Count as i32)).contains(&i) {
            return; // :540-541
        }
        let s = &BUTTONS[i as usize]; // :542
        let mut rows = self.sws.borrow_mut();
        let was = (rows[s.row as usize] >> s.bit) & 1 == 0; // :543 !BIT
        if pressed {
            rows[s.row as usize] &= !(1 << s.bit); // :544-545
        } else {
            rows[s.row as usize] |= 1 << s.bit; // :546-547
        }
        // :548-550 `if (was != pressed) panel_touched()` — no-op here (doc)
        let _ = was;
    }

    /// origin: mu2000.cpp:553-560 `button_pressed`
    pub fn button_pressed(&self, b: Button) -> bool {
        let i = b as i32;
        if !(0..(Button::Count as i32)).contains(&i) {
            return false; // :556-557
        }
        let s = &BUTTONS[i as usize]; // :558
        (self.sws.borrow()[s.row as usize] >> s.bit) & 1 == 0 // :559 !BIT
    }

    /// origin: mu2000.h `m_lcd` DD RAM view — the harness seam disk exposes
    /// as `mu.lcd().ddram()` (render.cpp:495/521 consumers).
    pub fn lcd_ddram(&self) -> [u8; 0x80] {
        self.lcd.borrow().m_ddram // hd44780.h:121 (hd44780.rs:74-75)
    }

    /// Total deferred-slot hits across both SWP30 devices since reset. The
    /// `--trace-swp` boot gate requires this to be 0 over 28M cycles (row rule:
    /// a "deferred" handler that is actually hit becomes this row's problem).
    pub fn swp_deferred_total(&self) -> u64 {
        self.swpm.borrow().deferred_hits + self.swps.borrow().deferred_hits
    }

    #[inline]
    pub fn pc(&self) -> u32 {
        self.soc.dev.core.pc
    }
    #[inline]
    pub fn total_cycles(&self) -> u64 {
        self.soc.dev.core.total_cycles()
    }

    /// origin: mu2000.cpp:1128-1132 devcb chain. Sci4's `m_irq` seam records
    /// the level on `Core::irq_line` (sci4.rs:519-525); this realizes the
    /// disk lambda fan-out: irq0/irq1 -> `m_sci_irq[]` + `update_sci_irq`
    /// (:1038-1041 OR -> execute_set_input(0)), irq3 -> execute_set_input(1).
    /// Same-value repeats are faithful (intc.rs:213-214 early-out).
    fn sync_sci4(&mut self) {
        let cur = { self.sci4.core().borrow().irq_line };
        for i in 0..2 {
            // i8->i32 widen (M5-W4: the shadow is `int` like disk :905;
            // values are -1/0/1 so the widening is value-exact)
            if cur[i] as i32 != self.m_sci_irq[i] {
                self.m_sci_irq[i] = cur[i] as i32;
                let or = (self.m_sci_irq[0] != 0 || self.m_sci_irq[1] != 0) as i8;
                self.q.borrow_mut().push_back(Evt::SetInput { line: 0, state: or });
            }
        }
        if cur[3] != self.m_sci_irq3 {
            self.m_sci_irq3 = cur[3];
            self.q.borrow_mut().push_back(Evt::SetInput { line: 1, state: cur[3] });
        }
    }

    /// Emit one `U cur -> ev pc` line (sh7042.cpp:295-297) through the
    /// `--trace-upd` sink. `cur` is the CALLER's disk clock: exact tick
    /// inside `m_in_event`, `current_cycles` (the −1) at write sites.
    fn emit_upd(&mut self, cur: u64) {
        if self.upd.is_some() {
            // event recomputed + pc read at the disk fprintf position
            let line = format!(
                "U {} -> {} pc={:08x}{}",
                cur,
                self.soc.event_cycles(),
                self.pc(),
                EOL
            );
            if let Some(f) = self.upd.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }

    /// Sticky `m_cpu->internal_update()` sites (sh_mtu.cpp:416-417/:460-461,
    /// sh_cmt.cpp:166/180/190, sh_sci.cpp:477/486/505 and the `internal_update`
    /// recursion :440-441) -> re-run the sh7042 fan-out NOW, disk-style, and
    /// emit its traced line. Loop until quiet: each fan-out level equals one
    /// disk recursion level (sh7042.cpp:295 fires per entry).
    /// `tick = Some(now)`: recursion ran INSIDE the `event_tick` `m_in_event`
    /// bracket (sh7042.cpp:249-251) so the clock is the exact tick
    /// (sh7042.h:95-96). `None`: write site, clock = `hn` seam
    /// (`total-1`, same value the device write handlers saw via
    /// `set_cpu_now`/`cpu_now()` mid-instruction — M1 harness
    /// "register writes show cur==total-1").
    /// TRUE when any internal_update changed `m_event_cycles` — the C++
    /// `recompute_timer`/`abort_timeslice` burst-cut signal (sh7042.cpp:267-269).
    fn pump_resched(&mut self, tick: Option<u64>) -> bool {
        let mut aborts = false;
        loop {
            let mut r = false;
            r |= self.mtu.borrow_mut().take_resched(); // mtu.rs:745
            r |= self.cmt.borrow_mut().take_resched(); // cmt.rs:156
            r |= self.adc0.borrow_mut().take_resched(); // sh_adc.cpp:189
            r |= self.adc1.borrow_mut().take_resched();
            {
                let mut p = self.pair.borrow_mut();
                r |= p.sci[0].take_resched(); // sci.rs:988
                r |= p.sci[1].take_resched();
            }
            if !r {
                break;
            }
            let cur = match tick {
                Some(now) => {
                    self.soc.m_in_event = true; // sh7042.cpp:249-251 bracket
                    aborts |= self.soc.internal_update_at(now);
                    self.soc.m_in_event = false;
                    now
                }
                None => {
                    let t = { self.hn.borrow().cpu_now() }; // hn.now == pre-instruction total
                    aborts |= self.soc.internal_update_at(t);
                    t
                }
            };
            self.emit_upd(cur);
        }
        // internal_update_at fan-out above ran the SCI TX state machines
        // (sh_sci.cpp internal_update -> tx_*_step -> do_sci_tx); disk fed
        // tx_line inside those synchronous calls (:1179). Ring FIFO keeps
        // ch0 edge order exactly.
        self.drain_sci_pins();
        aborts
    }

    /// MIDI OUT pin drain — the Rust shape of disk
    /// `m_cpu->write_sci_tx<0>().set(tx_line)` (mu2000.cpp:1179): every
    /// PIN_TX edge of SCI **ch0** goes to `midi.tx_line` in ring order
    /// (sci.rs:1028-1030 push -> :1050 poll). PIN_CLK entries are dropped:
    /// `write_sci_clk` has no `.set` in mu2000.cpp (rg: defined sh7042.h:39,
    /// fired sh_sci.cpp, unbound devcb = no-op on disk), and ch1 is dropped
    /// for the same reason (:1179 wires ch0 only). Called at every pump
    /// site that can move the SCI TX state machine — event_tick fan-out
    /// (sh7042.cpp:282-292), pump_resched (sticky internal_update sites
    /// sh_sci.cpp:477/486/505 reach it through `internal_update_at`) and
    /// reset_devices (sh_sci.cpp:367) — so the ring is empty at every loop
    /// boundary and `midi_out_take` (mu2000.h:232) sees bytes at the same
    /// host-visible point as disk. No allocation: `tx_pin_buf` is
    /// preallocated to PIN_RING (sci.rs:114), emptied by the drain itself.
    fn drain_sci_pins(&mut self) {
        let mut p = self.pair.borrow_mut();
        for (id, s) in p.sci.iter_mut().enumerate() {
            if s.pin_len == 0 {
                continue; // hot path: ring empty (the norm — poll per pump)
            }
            self.tx_pin_buf.clear();
            s.poll_pin(&mut self.tx_pin_buf); // sci.rs:1050-1055
            if id == 0 {
                // mu2000.cpp:1179 — only ch0's TX line is wired to MIDI OUT
                for &(code, level) in self.tx_pin_buf.iter() {
                    if code & smu_sh2::periph::sci::PIN_TX != 0 {
                        self.midi.tx_line(level); // -> mu2000.cpp:1271
                    } // PIN_CLK (code 0x00) dropped — unbound on disk
                }
            }
        }
    }

    /// Apply the queued bus-side actions IN ORDER. Disk anchors:
    /// Irq -> sh_intc.cpp:95-99 + :92 (route_irqs per vector);
    /// SetInput -> sh7042.cpp:143 set_input then :92;
    /// CpuIrq -> :92 (ipr_w re-arbitration);
    /// Ack -> sh7042.cpp:400 interrupt_taken then :92 re-issue.
    pub fn pump(&mut self) {
        loop {
            let ev = match self.q.borrow_mut().pop_front() {
                Some(e) => e,
                None => break,
            };
            match ev {
                Evt::Irq(vecs) => {
                    if !vecs.is_empty() {
                        let mut ic = self.intc.borrow_mut();
                        let _ = self.soc.route_irqs(&mut ic, &vecs); // sh7042.rs:1517
                    }
                }
                Evt::SetInput { line, state } => {
                    let (l, v) = self.intc.borrow_mut().set_input(line as u32, state as i32);
                    self.soc.set_internal_interrupt(l, v);
                }
                Evt::CpuIrq { level, vector } => {
                    self.soc.set_internal_interrupt(level, vector)
                }
                Evt::Ack { vector } => {
                    // sh7042.cpp:400 — irqline unused on disk (intc.rs:189)
                    let (l, v) = self.intc.borrow_mut().interrupt_taken(-1, vector);
                    self.soc.set_internal_interrupt(l, v); // update_irq tail :92
                }
            }
        }
    }

    /// origin: mu2000.cpp:413-419 `set_sintab_rom` (the deferred glue —
    /// disk pins `m_swpm/m_swps.set_sintab(m_sintab->data(), size())` at
    /// load; the master keeps getting the table through `run_sample_pair`'s
    /// argument as before). Bins call this right after `roms::load_sintab`
    /// (render.cpp:299-300 / live.cpp:496-497 seams). SAFETY: the caller
    /// keeps the buffer alive as long as it may run threaded — every bin
    /// holds it for the process lifetime (== disk's `m_sintab` member).
    pub fn set_sintab_pin(&mut self, t: &[u16]) {
        self.sintab_pin = t.as_ptr(); // empty pins a dangling-but-unused ptr;
        self.sintab_len = t.len(); //   the slave rebuilds `&[]` when len==0
    }

    /// origin: mu2000.h:253 / mu2000.cpp:309-313 `set_threaded`
    pub fn set_threaded(&mut self, on: bool) {
        self.want_threaded = on; // :311
        self.apply_threading(); // :312
    }

    /// origin: mu2000.h:254 `threaded()` — `m_slave_thread.joinable()`
    #[inline]
    pub fn threaded(&self) -> bool {
        self.slave.is_some()
    }

    /// :325/:3452 `m_slave_go.notify_one()` stand-in — 「眠っていたら起こす。
    /// 起きていれば素通り」: unpark iff parked (µs wake; while the slave
    /// is spinning the call is a relaxed-load no-op, same "素通り").
    fn kick_slave(&self) {
        if self.slave_parked.load(Ordering::Acquire) {
            // SAFETY: see SlaveWakeCell — a true `parked` is release-ordered
            // AFTER the slave wrote its handle, so this read has a
            // happens-before with the only write of the current thread.
            if let Some(t) = unsafe { &*self.slave_waker.0.get() }.as_ref() {
                t.unpark();
            }
        }
    }

    /// origin: mu2000.cpp:315-334 `apply_threading` — 「頼まれていて、台数が
    /// 境を超えていなければ別スレッドにする。そうでなければ 1 本に戻す」
    fn apply_threading(&mut self) {
        // :318 (relaxed load like disk; instances counted by new/Drop)
        let on = self.want_threaded && LIVE_INSTANCES.load(Ordering::Relaxed) <= threaded_max();
        if on == self.threaded() {
            return; // :319-320
        }
        if !on {
            // :322-328 — raise quit, kick the sleeper, join, clear quit.
            // operator= / operator++ on disk are seq_cst; mirrored.
            self.slave_quit.store(true, Ordering::SeqCst); // :323
            self.slave_go.fetch_add(1, Ordering::SeqCst); // :324
            self.kick_slave(); // :325 (park/unpark stand-in)
            if let Some(h) = self.slave.take() {
                let _ = h.join(); // :326 (m_slave_thread.join())
            }
            self.slave_quit.store(false, Ordering::SeqCst); // :327
            // stand-in housekeeping (master-only, thread joined):
            self.slave_parked.store(false, Ordering::SeqCst);
            unsafe { *self.slave_waker.0.get() = None };
            return;
        }
        // :330-333 合図の数は前に回した分だけ進んでいるので、今の数から待ち始める
        // (the counters SURVIVE stop/start — re-arm from the CURRENT go, so
        // an already-consumed tag is never re-run: 「0 からだと着いた途端に
        // 1 サンプル余計に回す」)
        let seen = self.slave_go.load(Ordering::Acquire); // :331
        self.slave_done.store(seen, Ordering::Release); // :332
        // stand-in housekeeping before spawn (master-only):
        self.slave_parked.store(false, Ordering::SeqCst);
        unsafe { *self.slave_waker.0.get() = None };
        // mu2000.cpp:333 `m_slave_thread = std::thread([this, seen]{ slave_loop(seen); })`
        // — here the captured `this` splits into the member atomics + the
        // stable Rc<RefCell<Swp30>> interior + the pinned ROM pointers.
        let go = Arc::clone(&self.slave_go); // mu2000.h:941 (shared handle)
        let done = Arc::clone(&self.slave_done); // :941
        let quit = Arc::clone(&self.slave_quit); // :942
        let out = Arc::clone(&self.slave_out); // :943 m_slave_l/m_slave_r
        let parked = Arc::clone(&self.slave_parked); // wait-stand-in bit
        let waker = Arc::clone(&self.slave_waker); // wait-stand-in target
        let raw = SlaveRaws {
            swp: Rc::as_ptr(&self.swps), // RcBox value — fixed heap address
            wave: self.wave.as_ptr(),
            wave_len: self.wave.len(),
            sintab: self.sintab_pin,
            sintab_len: self.sintab_len,
            sampram: self.sampram.as_ptr(), // mu2000.cpp:90 pin (ctor :2298)
            sampram_words: (self.sampram.len() >> 2) as u32, // mamecompat.h:174
        };
        self.slave = Some(std::thread::spawn(move || {
            Self::slave_loop(seen, go, done, quit, out, parked, waker, raw)
        }));
    }

    /// origin: mu2000.cpp:341-373 `slave_loop` — wait (spin, then park),
    /// run ONE slave sample per published tag, hand the DAC pair to the
    /// master through `out` before the `done` release. The macOS
    /// workgroup re-join (:346-349, realtime.h) has no Windows arm —
    /// `realtime_join` is a no-op there (src/compat/realtime.h).
    fn slave_loop(
        mut seen: u64,
        go: Arc<AtomicU64>,
        done: Arc<AtomicU64>,
        quit: Arc<AtomicBool>,
        out: Arc<SlaveOutCell>,
        parked: Arc<AtomicBool>,
        waker: Arc<SlaveWakeCell>,
        raw: SlaveRaws,
    ) {
        // park/unpark stand-in setup (see SlaveWakeCell): publish MY handle
        // once, before `parked` can ever read true.
        unsafe { *waker.0.get() = Some(std::thread::current()) };
        // SAFETY: see the Machine-level SAFETY note — the handshake below
        // is byte-for-byte the disk one (:357-371), so these pointers are
        // only ever touched while this thread owns the SWP.
        let swp: &RefCell<Swp30> = unsafe { &*raw.swp };
        let wave: &[u8] = unsafe {
            if raw.wave_len == 0 {
                &[] // Wave::new maps empty to its own zero stand-in (fetch.rs:35)
            } else {
                std::slice::from_raw_parts(raw.wave, raw.wave_len)
            }
        };
        let sintab: &[u16] = unsafe {
            if raw.sintab_len == 0 {
                &[] // disk :416-417 — null sintab never pinned, device reads 0
            } else {
                std::slice::from_raw_parts(raw.sintab, raw.sintab_len)
            }
        };
        loop {
            // :356-364 合図を待つ (まず回して待つ; SLAVE_SPINS 空振りで眠る)
            let mut spins = 0u32;
            while go.load(Ordering::Acquire) == seen {
                if quit.load(Ordering::Relaxed) {
                    return; // :358-359
                }
                spins += 1;
                if spins < SLAVE_SPINS {
                    std::hint::spin_loop(); // :361 smu2000::cpu_pause
                } else {
                    // :363 `m_slave_go.wait(seen, acquire)` — park/unpark
                    // stand-in (atomic_wait is not stable here; see
                    // SlaveWakeCell). Nothing feeds the emulation from the
                    // clock — the machine's ONLY clock stays the sample
                    // count; parking is scheduling, not state. Once asleep
                    // the master's `kick_slave` (publish side) wakes it in
                    // µs, so no sample ever stalls past a thread wake-up.
                    parked.store(true, Ordering::Release);
                    if go.load(Ordering::Acquire) == seen && !quit.load(Ordering::Relaxed) {
                        std::thread::park();
                    }
                    parked.store(false, Ordering::SeqCst);
                }
            }
            seen = go.load(Ordering::Acquire); // :365
            if quit.load(Ordering::Relaxed) {
                return; // :366-367
            }
            // :369-371 m_slave_l = m_slave_r = 0; run; publish done
            out.set((0, 0));
            // zero-alloc wrapper (fetch.rs:33) + W-SAMP2 sampling-RAM
            // overlay — disk-faithful: m_swps' m_wave_cache carries
            // set_overlay(m_sampram, 0x1000000, bytes>>2) from the ctor
            // (mu2000.cpp:90/mamecompat.h:170-175). SAFETY: `sampram`
            // never reallocs (fixed 4 MB, ctor-sized; :2132-2136 note).
            let w = unsafe { Wave::with_overlay(wave, raw.sampram, raw.sampram_words) };
            let (l, r) = swp.borrow_mut().run_sample(sintab, &w); // :370
            out.set((l, r));
            done.store(seen, Ordering::Release); // :371
        }
    }


    /// origin: mu2000.cpp:3401-3453 — the per-sample SWP pair advance.
    /// Threaded arm LIVE (M8): :3449-3457 publishes the tag, runs the
    /// master, spins on `done`; the else-arm (:3458-3461) stays sequential.
    /// CPU/cycle debt stays in
    /// the run-loop row (run.rs); this is the pure DSP half of
    /// `mu2000::run_sample`. Returns the MASTER DAC pair (:3495-3498 —
    /// the slave DAC is wired to nothing).
    pub fn run_sample_pair(&mut self, sintab: &[u16]) -> (i32, i32) {
        // :3225-3227 台数が変わっていたら見直す (8192 サンプルごと;
        // m_want_threaded short-circuits the counter like disk)
        if self.want_threaded {
            self.thread_check = self.thread_check.wrapping_add(1); // ++m_thread_check
            if self.thread_check & 0x1fff == 0 {
                self.apply_threading();
            }
        }
        // :3448 s32 lm = 0, rm = 0, ls = 0, rs = 0;
        let wave = std::mem::take(&mut self.wave); // borrow seam, no alloc
        // W-SAMP2: the per-sample `Wave` must carry the sampling-RAM
        // overlay — disk's m_swpm.m_wave_cache gets set_overlay from the
        // ctor (mu2000.cpp:89 + mamecompat.h:170-175) and every fetch
        // (AUDITION, sample playback) reads through it. Without this the
        // fetch wraps ROM (samptest audition read garbage; samptest.cpp:7
        // 「サンプリング RAM と波形アクセス 0x7000」). SAFETY: sampram
        // never reallocs (fixed 4 MB; :2132-2136 note).
        let w = unsafe {
            Wave::with_overlay(
                &wave,
                self.sampram.as_ptr(),
                (self.sampram.len() >> 2) as u32,
            )
        };
        let (lm, rm, _ls, _rs) = if let Some(_st) = self.slave.as_ref() {
            // :3449-3457 threaded arm — publish, run master, spin, collect
            let tag = self.slave_go.load(Ordering::Relaxed).wrapping_add(1); // :3450
            self.slave_go.store(tag, Ordering::Release); // :3451
            self.kick_slave(); // :3452 眠っていたら起こす。起きていれば素通り
            let (lm, rm) = self.swpm.borrow_mut().run_sample(sintab, &w); // :3453
            while self.slave_done.load(Ordering::Acquire) != tag {
                std::hint::spin_loop(); // :3454-3455 cpu_pause spin (錠は使わない)
            }
            let (ls, rs) = self.slave_out.get(); // :3456-3457
            (lm, rm, ls, rs)
        } else {
            // :3458-3461 else-arm — sequential, master-first (:3414-3415 note)
            let (lm, rm) = self.swpm.borrow_mut().run_sample(sintab, &w); // :3459
            let (ls, rs) = self.swps.borrow_mut().run_sample(sintab, &w); // :3460
            (lm, rm, ls, rs)
        };
        self.wave = wave;
        // :3433-3434 slave melo(i) -> master meli(i), i = 0..13 (same index;
        // the :3427 "outputs 4..17" comment is legacy MAME naming, NOT the
        // melo[] indexing — disk code is `m_swpm.set_meli(i, m_swps.melo(i))`)
        let slave: [i32; 14] = {
            let s = self.swps.borrow();
            let mut b = [0i32; 14];
            for (i, v) in b.iter_mut().enumerate() {
                *v = s.melo(i);
            }
            b
        };
        {
            let mut m = self.swpm.borrow_mut();
            for (i, v) in slave.iter().enumerate() {
                m.set_meli(i, *v);
            }
        }
        // :3435-3437 master melo -> slave meli; NO lines for master 6/7
        // (:3429-3431 — a master<->slave ring would close without this gap)
        const TO_SLAVE: [usize; 8] = [0, 1, 2, 3, 4, 5, 8, 9];
        {
            let melo: [i32; 8] = {
                let m = self.swpm.borrow();
                let mut b = [0i32; 8];
                for (j, i) in TO_SLAVE.iter().enumerate() {
                    b[j] = m.melo(*i);
                }
                b
            };
            let mut s = self.swps.borrow_mut();
            for (j, i) in TO_SLAVE.iter().enumerate() {
                s.set_meli(*i, melo[j]);
            }
        }
        // :3438-3443 A/D INPUT = slave meli 6/7 (the two lines master skips),
        // 16bit << 8 scale. ad_in stays 0 until the A/D capture row feeds it.
        let (ad0, ad1) = (self.ad_in[0], self.ad_in[1]);
        {
            let mut s = self.swps.borrow_mut();
            s.set_meli(6, ad0.wrapping_mul(256)); // :3442
            s.set_meli(7, ad1.wrapping_mul(256)); // :3443
        }
        // :3444-3448 AN0/AN2 meter detection (mu2000.cpp:3490-3493, W-SAMP1)
        // — peak latch / decay ladder into the SHARED `ad_peak` Rc that the
        // ADC AN0/AN2 seam (ad_pin / ad_level_adc) converts for the
        // firmware's REC TriggerLvl poll. `a = min(abs(in), 32767)`:
        // abs is safe here — ad_in is the 16-bit domain (mu2000.h:258).
        {
            let mut peak = self.ad_peak.borrow_mut();
            for (i, v) in [ad0, ad1].iter().enumerate() {
                let a = v.abs().min(32767); // :3491
                peak[i] = if a >= peak[i] {
                    a // :3492 latch
                } else {
                    peak[i] - ((peak[i] >> 12) + 1) // :3492 decay (peak >= 1
                    // whenever a < peak, so the subtraction stays >= 0)
                };
            }
        }
        // :3450-3453 speaker = master DAC only
        (lm, rm)
    }

    /// origin: mu2000.h:258 `set_audio_input` (A/D INPUT, 16-bit domain)
    pub fn set_audio_input(&mut self, ad1: i32, ad2: i32) {
        self.ad_in[0] = ad1;
        self.ad_in[1] = ad2;
    }

    /// origin: mu2000.h:133 `midi_in` — one received byte on `port`
    /// (0..3; out-of-range clamps to 0, mu2000.h:135-136). Returns the
    /// routed port or -1 for a consumed `F5 nn`.
    pub fn midi_in(&mut self, byte: u8, port: i32) -> i32 {
        self.midi.midi_in(byte, port) // mu2000.h:133-163 (midi.rs)
    }

    /// origin: mu2000.h:112 `midi_ready` (RE up = firmware RX live; hold
    /// MIDI until then — same seam live.rs uses via `pair.midi_ready`)
    pub fn midi_ready(&self, port: usize) -> bool {
        self.pair.borrow().midi_ready(port) // sh_sci.h:65 rx_enabled
    }

    /// origin: mu2000.h:165 `midi_dropped`
    pub fn midi_dropped(&self) -> u64 {
        self.midi.dropped
    }

    /// origin: mu2000.h:120 `set_fast_midi` — applied by every tool before
    /// reset via `ui::apply_engine_options` (options.h:52; render.cpp:377).
    /// ON: the pump injects queued bytes straight into the SCI RDR
    /// (mu2000.cpp:1408-1416) and the run_cycles MIDI chunk clamp (:1234)
    /// opens; pending/idle count the SCI byte-in-flight instead of the wire
    /// bit (mu2000.h:184-207).
    pub fn set_fast_midi(&mut self, fast: bool) {
        self.midi.set_fast_midi(fast) // mu2000.h:120 (midi.rs)
    }

    /// origin: mu2000.h:221 `set_usb_host` — **must be called BEFORE
    /// `reset()`** (mu2000.h:218-220: HOST SELECT is a boot-time decision;
    /// the F4 03 01 01 01 host-online push in reset (:1097-1099) and the
    /// ADC4 AN4 pin (mu2000.cpp:1151, Hub::usb_host) read this value).
    /// With it ON, ALL ports (even A/B) route through the usb_line and the
    /// firmware answers on USB too (mu2000.h:234-236 — DIN MIDI OUT goes
    /// silent). The same Cell backs the Hub's :1151 lambda — single truth.
    pub fn set_usb_host(&mut self, on: bool) {
        self.midi.usb_host.set(on) // mu2000.h:221 (midi.rs)
    }
    /// origin: mu2000.h:222 `usb_host()` (bootcache key + transfer,
    /// bootcache.h:110/:202)
    pub fn usb_host(&self) -> bool {
        self.midi.usb_host.get()
    }

    /// origin: mu2000.h:168-172 `midi_queued(port)`
    pub fn midi_queued(&self, port: i32) -> usize {
        self.midi.midi_queued(port)
    }

    /// origin: mu2000.h:173-182 `midi_pending`
    pub fn midi_pending(&self) -> usize {
        self.midi.midi_pending(&PairSci(&self.pair))
    }

    /// origin: mu2000.h:183-189 `midi_idle(port)`
    pub fn midi_idle(&self, port: i32) -> bool {
        self.midi.midi_idle(port, &PairSci(&self.pair))
    }

    /// origin: mu2000.h:190-202 `midi_idle()`
    pub fn midi_idle_all(&self) -> bool {
        self.midi.midi_idle_all(&PairSci(&self.pair))
    }

    /// origin: mu2000::run_cycles :1156-1233. The scheduler loop is
    /// transliterated line-for-line; the CPU advances ONE instruction per
    /// `core.run_cycles(1)` so the Evt pump (IRQ latches / acks) lands at
    /// the same instruction boundary the disk core's `m_test_irq` test uses
    /// (core.rs:2252-2257; module doc "IRQ plumbing").
    pub fn run_cycles(&mut self, n_in: u64) {
        // 前回はみ出した分を先に返す (:1158-1161)
        let mut n = n_in;
        if self.overrun >= n {
            self.overrun -= n;
            return;
        }
        n -= self.overrun;
        self.overrun = 0;

        let mut idle = 0i32; // :1167
        while n > 0 {
            self.loops += 1; // :1169
            let now = self.total_cycles(); // :1170
            self.rm.set_cycles(now); // :1171
            {
                let mut h = self.hn.borrow_mut();
                h.now = now;
                h.in_event = false; // sh7042.h:92-98 clock seam
            }

            // MAME のスケジューラが持っていたタイマ (:1173-1180)
            let tmr = self.rm.next_timer_cycles(); // :1174
            if tmr <= now {
                self.timer_fires += 1; // :1176
                self.rm.run_timers(now); // :1177 (sci4 tx/rx ticks live here)
                self.rm.set_cycles(now); // :1178
                self.sync_sci4(); // ticks may move sci4 irq_line (:1130-1132)
                self.pump();
                continue;
            }

            let ev = self.soc.event_cycles(); // :1182

            if ev != 0 && now >= ev {
                self.event_fires += 1; // :1185
                // event_tick: sh7042.cpp:247-252 — m_in_event brackets the
                // internal_update fan-out (adc/cmt/mtu0-4/sci0-1 :282-292)
                self.hn.borrow_mut().in_event = true; // sh7042.cpp:249
                self.soc.event_tick(); // :1186
                self.hn.borrow_mut().in_event = false; // sh7042.cpp:251
                // disk called tx_line synchronously INSIDE this fan-out
                // (write_sci_tx<0> devcb, mu2000.cpp:1179) — ch0 TX edges
                // leave the pin ring here, before the recursion below
                self.drain_sci_pins();
                // g_upd_trace (sh7042.cpp:295-297) — emitted after the tick;
                // current_time == now (in_event current_cycles == total),
                // event == recomputed, pc unchanged between them.
                self.emit_upd(now); // :295 — cur==ev exact tick
                self.pump_resched(Some(now)); // sh_sci.cpp:440-441 recursion
                self.pump(); // FIFOs filled by the per-device updates
                if self.soc.event_cycles() == ev {
                    idle += 1;
                    if idle > 2 {
                        break; // :1187-1188 idle guard
                    }
                }
                continue;
            }
            idle = 0; // :1191

            // :1226 midi_step(now) — the DIN bit pump (`midi lines` row;
            // disk re-cited 2026-10-03, +32 from the N2-era note below).
            // sh_sci.cpp:475/484 read m_cpu->current_cycles() inside the
            // RX path; no CPU ran since the loop top, but disk
            // current_cycles() OUTSIDE m_in_event is `total-1` (sh7042.h:92-98
            // — hn.cpu_now() is the exact mirror). Host-sync cpu_now to that
            // value (sci.rs:1080 host note). 2026-10-01 piano row: syncing to
            // `now` re-anchored the RX clock grid (sh_sci.cpp:475-476
            // (now/step+1)*step) one step early whenever a start-bit edge hit
            // now % step == 0, desyncing every later sample tick (:434 chain),
            // shifting RE IRQs and stretching the firmware poll loop.
            let cc = { self.hn.borrow().cpu_now() }; // == now-1 (in_event=false)
            {
                let mut p = self.pair.borrow_mut();
                for s in p.sci.iter_mut() {
                    s.cpu_now = cc;
                }
            }
            let mut sink = PairSci(&self.pair);
            self.midi.midi_step(now, &mut sink);
            // Disk: the fast-arm `sci->receive_byte()` internal_interrupt
            // (sh_sci.cpp:141-146) reaches the INTC SYNCHRONOUSLY inside
            // the mu2000.cpp:1414 call at this same loop head, so the CPU
            // sees m_test_irq at the first execute_one of the chunk below
            // (sh2.cpp:284 post-execute check). The Rust SCI latches its
            // vectors into an irq FIFO drained by bus-op anchors
            // (Hub::drain) — the direct inject never touches the bus, so
            // drain+route it here at the head, once per armed injection.
            // Gated fast-only: the OFF wire path keeps its paired M4-era
            // drain anchors untouched (do_rx_w vectors are reached by the
            // firmware's own SCI bus reads on that path).
            if self.midi.fast_midi {
                let mut n = 0usize;
                {
                    let mut p = self.pair.borrow_mut();
                    n += p.sci[0].drain_irqs(&mut self.sci_irq_buf); // sci.rs:1012
                    n += p.sci[1].drain_irqs(&mut self.sci_irq_buf);
                }
                if n > 0 {
                    self.q.borrow_mut().push_back(Evt::Irq(std::mem::take(&mut self.sci_irq_buf)));
                    self.pump(); // route_irqs + set_internal_interrupt
                }
            }
            // :1227 usb_step(now) — LIVE since the M7 `USB host` row
            // (smu-machine/src/usb.rs). Disk guard :1323 (`!m_usb_host &&
            // rx.empty() && !have` -> return) keeps OFF-mode runs bit-
            // identical: with no USB traffic NEITHER IRQ line is touched.
            // Disk has NO chunk clamp for usb (:1234-1240 is DIN-only), so
            // a byte crossing its 2800-cycle grid mid-chunk is handed over
            // at the NEXT loop head — the Rust inner loop returns to this
            // head on the same chunk boundaries (P3 cpu_now sync row), so
            // the hand-over instruction is the same one disk sees. The
            // Evt::SetInput pushes realize the disk's synchronous
            // execute_set_input(2/3,..) (:1341/:1349/:1355) at the same
            // pre-chunk boundary via the pump right below.
            {
                let host = self.midi.usb_host.get(); // mu2000.h:1022
                let fast = self.midi.fast_midi; // :1338 (grid collapses to 0)
                let mut irq = QIrq { q: &self.q };
                self.midi.usb.borrow_mut().step(now, host, fast, &mut irq);
            }
            self.pump();
            // :1211-1217 m_swp_wait skip — ported in place below (this loop
            // is run_cycles itself; the old "lives in run_sample_pair" note
            // was wrong and the skip was dead — piano-render root cause
            // 2026-10-01: master SWP writes must stall the CPU 440 cyc ea).

            let mut chunk = n; // :1197
            if ev != 0 && ev - now < chunk {
                chunk = ev - now; // :1198-1199
            }
            if tmr != u64::MAX && tmr - now < chunk {
                chunk = tmr - now; // :1200-1201
            }
            if !self.midi.fast_midi {
                // :1234-1240 — never chunk across the next MIDI bit edge;
                // fast ON opens this clamp exactly as disk (:1234 gate)
                for m in self.midi.lines.iter() {
                    if m.bit >= 0 || !m.queue.is_empty() {
                        let left = if m.next > now { m.next - now } else { 1 }; // :1205
                        if left < chunk {
                            chunk = left; // :1206-1207
                        }
                    }
                }
            }
            // :1209-1217 — SWP30 に書いた後は、その待ちぶんだけ命令を進めずに
            // 時間を送る. The chunk clamps above (:1197-1208) bound the skip
            // exactly as C++, and this head — like C++ — runs only at C++
            // loop-head cadence (chunk end / abort / event), not per
            // instruction: omitting the skip ran the whole machine ~0.7%
            // fast and shifted every async IRQ out of the firmware's
            // register-download loop.
            if *self.swp_wait.borrow() != 0 {
                let skip = (*self.swp_wait.borrow()).min(chunk); // :1212
                self.soc.dev.core.skip_cycles(skip); // :1213 (sh.h:226)
                *self.swp_wait.borrow_mut() -= skip; // :1214
                n = if skip >= n { 0 } else { n - skip }; // :1215
                continue; // :1216
            }

            // ---- INNER: M9 2026-10-03 — batched chunk (was: ONE instruction
            // per call + full pump block, ~59 ns of RefCell/pump bookkeeping
            // per instruction). The burst now runs inside execute_run
            // (sh2.cpp:290 do/while, the SAME shape as the C++
            // `m_cpu->run_cycles(chunk)` at mu2000.cpp:1219) and stops at the
            // first instruction whose bus access has pump-visible side effects
            // (Sh2Bus::batch_stop / dev_dirty) or at the frozen chunk budget —
            // then the unchanged pump block below runs, i.e. at EXACTLY the
            // instruction boundaries the per-instruction loop stopped at.
            // The per-instruction clock seam (hn.now=pre, s.cpu_now) rode
            // along into RunHook::instruction at the identical position.
            let mut ran = 0u64;
            let mut core_abort = false;
            loop {
                self.soc.bus.dev_dirty.set(false); // consumed by batch_stop
                let (done, held) = {
                    let mut ctx = Ctx {
                        bus: &mut self.soc.bus,
                        rm: &mut self.rm,
                        sci4: &*self.sci4.core(),
                        hn: Rc::clone(&self.hn),
                        snap: Rc::clone(&self.snap),
                        q: Rc::clone(&self.q),
                        ledsw1: &mut self.ledsw1,
                        ledsw2: &mut self.ledsw2,
                        d80: &mut self.d80,
                        sws: Rc::clone(&self.sws),
                    };
                    let mut hook = RunHook {
                        hash: self.hash_on,
                        trace: self.trace_on,
                        snap: Rc::clone(&self.snap),
                        hn: Rc::clone(&self.hn),
                        pair: &self.pair,
                    };
                    let w0 = *self.swp_wait.borrow();
                    let budget = (chunk - ran).min(i32::MAX as u64) as i32;
                    let d = self.soc.dev.core.run_cycles(&mut ctx, &mut hook, budget);
                    (d, *self.swp_wait.borrow() > w0) // held = SWP-write abort (:855)
                };
                // sticky write-site `m_cpu->internal_update()` sites — disk
                // ran them INSIDE the instruction at current_cycles() == hn
                let ev_changed = self.pump_resched(None);
                self.sync_sci4(); // :1128-1132 — a sci4 write may move irq_line
                self.pump(); // old per-instruction position EXACTLY (line/ack
                // drains, intc arbitration, tirq) — sh7042::execute_set_input
                // routes through INTC (:141-143), so every ack lands on the
                // same post-execute check boundary as C++.
                if done <= 0 {
                    // :1220-1224 — abort_timeslice mid-instruction (sh.h:219-223)
                    core_abort = true;
                    break;
                }
                ran += done as u64;
                if held || ev_changed || ran >= chunk {
                    break;
                }
            }
            if core_abort {
                if self.soc.event_cycles() == ev {
                    break;
                }
                continue;
            }
            // :1225-1231 — over/under budget rides into m_overrun
            if ran >= n {
                self.overrun += ran - n;
                n = 0;
            } else {
                n -= ran;
            }
        }
    }
}

/// The `m_cpu->sci(port)` seam for the MIDI bit pump (mu2000.cpp:1407).
/// RefCell-based so the immutable accounting forwarders (`midi_pending` /
/// `midi_idle`) and the edge-driving pump (which takes a FRESH
/// `borrow_mut` per edge — the pump holds no borrow across the call)
/// share one adapter. `cpu_now` is host-synced by run_cycles before the
/// pump (sci.rs:1080 note; sh_sci.cpp:475/484 == loop-top `now`).
struct PairSci<'a>(&'a RefCell<Sh2SciPair>);

impl midi::MidiSci for PairSci<'_> {
    fn rx_can_accept(&self, port: usize) -> bool {
        // mu2000.cpp:1409 (fast arm — LIVE since the M7 fast_midi row)
        self.0.borrow().sci.get(port).is_some_and(|s| s.rx_can_accept()) // sci.rs:329
    }
    fn rx_byte_pending(&self, port: usize) -> bool {
        self.0.borrow().sci.get(port).is_some_and(|s| s.rx_byte_pending()) // sh_sci.cpp:134
    }
    fn receive_byte(&mut self, port: usize, data: u8) {
        if let Some(s) = self.0.borrow_mut().sci.get_mut(port) {
            s.receive_byte(data); // mu2000.cpp:1414 (fast inject — LIVE M7)
        }
    }
    fn do_rx_w(&mut self, port: usize, state: i32) {
        if let Some(s) = self.0.borrow_mut().sci.get_mut(port) {
            s.do_rx_w(state); // mu2000.cpp:1429/1439/1441 (sh_sci.cpp:375)
        }
    }
}

/// origin: mu2000.cpp:286-290 `~mu2000` — 「set_threaded(false)」joins the
/// slave BEFORE anything else dies (the thread holds raw pointers into the
/// Rc'd device), then the instance count goes down (mu2000.cpp:289).
impl Drop for Machine {
    fn drop(&mut self) {
        self.set_threaded(false); // :288
        LIVE_INSTANCES.fetch_sub(1, Ordering::SeqCst); // :289 (g_live_instances--)
    }
}

#[cfg(test)]
mod glue_tests {
    //! mixer row B: mu2000.cpp:3401-3453 per-sample glue (no CPU/cycle debt).
    use super::*;

    fn sintab0() -> Vec<u16> {
        vec![0u16; 0x8000] // harness d.m_sintab zeros (mixB gt.cpp)
    }

    #[test]
    fn pair_fresh_master_dac_and_zero_wiring() {
        let mut m = Machine::new(Vec::new());
        let (l, r) = m.run_sample_pair(&sintab0());
        // fresh zero MEG / fresh mixer: DAC pair + all serial lines at 0
        assert_eq!((l, r), (0, 0), "master DAC pair :3452-3453");
        {
            let d = m.swpm.borrow();
            assert_eq!(d.mixer.meli[0..14], [0i32; 14], "meli 0..13 <- slave melo");
            assert_eq!(d.deferred_hits, 0);
        }
        {
            let d = m.swps.borrow();
            // TO_SLAVE lines + the AD stub all read master melo(==0)/ad_in(0)
            for i in [0usize, 1, 2, 3, 4, 5, 6, 7, 8, 9] {
                assert_eq!(d.mixer.meli[i], 0, "slave meli {i}");
            }
            assert_eq!(d.deferred_hits, 0);
        }
        // second sample: jit_wait advances on both (:4191, no re-programming)
        let _ = m.run_sample_pair(&sintab0());
        assert_eq!(m.swpm.borrow().meg_jit_wait, m.swps.borrow().meg_jit_wait);
    }

    #[test]
    fn pair_interconnect_directions_and_gap() {
        let mut m = Machine::new(Vec::new());
        // slave: route MELI input 0 (mix 0x50 = bank 0x40 | chan 0x10; the
        // bank-0x40 route slots are 0x3b..0x3d) raw to every output -> slave
        // melo[0..16] all carry the injected meli.
        {
            let mut d = m.swps.borrow_mut();
            d.write16((0x10 << 6) | 0x3b, 0xffff); // route_w<0x40|0> :2172
            d.write16((0x10 << 6) | 0x3c, 0); //      route_w<0x40|1> :2173
            d.write16((0x10 << 6) | 0x3d, 0); //      route_w<0x40|2> :2174
            d.set_meli(0, 0x1234_567);
        }
        let _ = m.run_sample_pair(&sintab0());
        {
            let md = m.swpm.borrow();
            assert_eq!(md.mixer.meli[0], 0x1234_567, "slave->master :3433-3434");
            assert_eq!(md.mixer.meli[13], 0x1234_567, "14-line spread");
            // the handoff reads POST-run melo (mixer_step rewrote it this
            // sample) — that IS the 1-sample-delay geometry (:3432)
            assert_eq!(md.mixer.meli[3], 0x1234_567);
        }
        {
            let sd = m.swps.borrow();
            // master->slave TO_SLAVE gap: master produced nothing, so these
            // are overwritten with 0 AFTER the slave ran (:3436-3437)
            for i in [0usize, 5, 8, 9] {
                assert_eq!(sd.mixer.meli[i], 0, "master->slave meli {i}");
            }
            // master 6/7 lines DO NOT EXIST (:3429-3431); slave meli 6/7 are
            // the ad_in stub (still 0 here)
            assert_eq!(sd.mixer.meli[6], 0);
            assert_eq!(sd.mixer.meli[7], 0);
        }
        // ad_in now non-zero -> next run stamps slave meli 6/7 = ad*256
        m.set_audio_input(0x100, -3);
        let _ = m.run_sample_pair(&sintab0());
        let sd = m.swps.borrow();
        assert_eq!(sd.mixer.meli[6], 0x100 * 256); // :3442
        assert_eq!(sd.mixer.meli[7], (-3i32).wrapping_mul(256)); // :3443
    }

    #[test]
    fn pair_melo_clamp_across_wires() {
        let mut m = Machine::new(Vec::new());
        // master: MELI input 1 (mix 0x51) raw to every output; meli carries
        // an over-scale word -> master melo stays raw, wire CLAMPS (:3437)
        {
            let mut d = m.swpm.borrow_mut();
            d.write16((0x11 << 6) | 0x3b, 0xffff);
            d.write16((0x11 << 6) | 0x3c, 0);
            d.write16((0x11 << 6) | 0x3d, 0);
            d.set_meli(1, 0x7fff_ffff);
        }
        let _ = m.run_sample_pair(&sintab0());
        let sd = m.swps.borrow();
        assert_eq!(sd.mixer.meli[0], smu_swp30::mix::SERIAL_FULL_SCALE);
        assert_eq!(sd.mixer.meli[9], smu_swp30::mix::SERIAL_FULL_SCALE);
    }

    /// M8 `threaded slave` row gate: the go/done handshake must be a PURE
    /// per-sample handoff — running the slave SWP30 on a second thread has
    /// to leave every serialized byte identical (disk claim
    /// 「1 本でも別スレッドでも出る音は同じ」, mu2000.cpp:297; harness
    /// 別糸 is the same gate against the C++ binary).
    #[test]
    fn threaded_pair_state_eq_single() {
        // threaded_max env clamps ≥0 (mu2000.cpp:302-303); only THIS test
        // calls apply_threading-on in this binary, and the env is set
        // before the OnceLock fills — so the arm must engage here
        // regardless of core count (LIVE_INSTANCES is pinned low below:
        // other parallel tests may hold machines).
        std::env::set_var("SMU2000_THREADED_MAX", "4096");
        let st: &'static mut [u16] = Box::leak(sintab0().into_boxed_slice());
        let st: &[u16] = st;
        let seed = |m: &mut Machine| {
            // slave MELI 0 raw to every output (pair_interconnect pattern),
            // master MELI 1 over-scale -> the wires carry live numbers both
            // ways every sample; AD inputs move the slave's 6/7 lines
            {
                let mut d = m.swps.borrow_mut();
                d.write16((0x10 << 6) | 0x3b, 0xffff);
                d.write16((0x10 << 6) | 0x3c, 0);
                d.write16((0x10 << 6) | 0x3d, 0);
                d.set_meli(0, 0x1234_567);
            }
            {
                let mut d = m.swpm.borrow_mut();
                d.write16((0x11 << 6) | 0x3b, 0xffff);
                d.write16((0x11 << 6) | 0x3c, 0);
                d.write16((0x11 << 6) | 0x3d, 0);
                d.set_meli(1, 0x7fff_ffff);
            }
            m.set_audio_input(0x100, -3);
            m.set_sintab_pin(st);
        };
        const N: usize = 3000;
        let mut a = Machine::new(Vec::new()); // single-threaded reference
        seed(&mut a);
        for _ in 0..N + 500 {
            let _ = a.run_sample_pair(st);
        }
        let sa = state::save_state(&mut a);
        let mut b = Machine::new(Vec::new());
        seed(&mut b);
        LIVE_INSTANCES.store(1, Ordering::Relaxed);
        b.set_threaded(true);
        assert!(b.threaded(), "SMU2000_THREADED_MAX must arm the slave thread");
        for _ in 0..N {
            let _ = b.run_sample_pair(st);
        }
        // tag arithmetic: exactly one published tag per sample, exactly one
        // done per tag (:3450-3455 publish/spin == :357-371 wait/store)
        assert_eq!(b.slave_go.load(Ordering::Relaxed), N as u64);
        assert_eq!(b.slave_done.load(Ordering::Acquire), N as u64);
        // stop/start leg (mu2000.cpp:319-333): stop JOINS and the counters
        // SURVIVE; restart re-arms from the CURRENT go — an already-consumed
        // tag is never re-run (「0 からだと着いた途端に 1 サンプル余計に」)
        b.set_threaded(false);
        assert!(!b.threaded());
        // the STOP kick bumps go once (mu2000.cpp:324) — and the woken
        // slave QUITS at :366-367 without running that tag: done stays at
        // N, the hard proof that stop never runs an extra sample
        assert_eq!(b.slave_go.load(Ordering::Relaxed), N as u64 + 1);
        assert_eq!(b.slave_done.load(Ordering::Acquire), N as u64);
        LIVE_INSTANCES.store(1, Ordering::Relaxed);
        b.set_threaded(true); // re-arms from seen = N+1 (:331)
        assert!(b.threaded());
        for _ in 0..500 {
            let _ = b.run_sample_pair(st);
        }
        assert_eq!(b.slave_go.load(Ordering::Relaxed), (N + 501) as u64);
        assert_eq!(b.slave_done.load(Ordering::Acquire), (N + 501) as u64);
        b.set_threaded(false); // Drop would do it too (mu2000.cpp:288)
        assert_eq!(b.slave_done.load(Ordering::Acquire), (N + 501) as u64); // no extra run again
        let sb = state::save_state(&mut b);
        // full serialized universe (swpm+swps+midi+timers+latches): the
        // CPU never ran (run_sample_pair only), so any byte diff here is
        // SWP state / interconnect / an off-by-one handshake.
        assert_eq!(sa.len(), sb.len());
        let diff = sa.iter().zip(sb.iter()).position(|(x, y)| x != y);
        assert_eq!(diff, None, "threaded state != single state after {}+500 samples", N);
    }

    /// field-level twin of the state-digest test (cheap to read when it
    /// fails): master DAC + wires must track sample-for-sample.
    #[test]
    fn threaded_pair_dac_eq_sample_by_sample() {
        std::env::set_var("SMU2000_THREADED_MAX", "4096");
        let st: &'static mut [u16] = Box::leak(sintab0().into_boxed_slice());
        let st: &[u16] = st;
        let mut a = Machine::new(Vec::new());
        let mut b = Machine::new(Vec::new());
        for m in [&mut a, &mut b].iter_mut() {
            m.swpm.borrow_mut().set_meli(1, 0x1234_567);
            m.swps.borrow_mut().set_meli(0, -0x7654_321);
            m.set_sintab_pin(st);
        }
        LIVE_INSTANCES.store(1, Ordering::Relaxed);
        b.set_threaded(true);
        assert!(b.threaded());
        for _ in 0..1500 {
            let (la, ra) = a.run_sample_pair(st);
            let (lb, rb) = b.run_sample_pair(st);
            assert_eq!((la, ra), (lb, rb));
        }
        b.set_threaded(false);
        for i in 0..14 {
            assert_eq!(
                a.swpm.borrow().mixer.meli[i],
                b.swpm.borrow().mixer.meli[i],
                "master meli {i}"
            );
        }
        assert_eq!(a.swps.borrow().meg.sample_counter, b.swps.borrow().meg.sample_counter);
    }

    /// M5-W2: `Adc::state` (sh_adc.cpp:380-391) — save→load→save byte
    /// equality on quirky values through the real hub-side device type.
    #[test]
    fn adc_state_roundtrip_quirky() {
        use smu_compat::StateIo;
        let mut a = Adc::new_ms(0, 136);
        a.device_reset();
        a.addr = [1, 2, 3, 0x8000, 5, 6, 0xffff, 8];
        a.buf = [0xabcd, 0xef01];
        a.adcsr = 0xe0; // ADST|ADIE|ADF
        a.adcr = 0x07;
        a.trigger = -1;
        a.start_mode = 0x44; // COUNTED|BUFFER (HS-only bits — quirky payload)
        a.start_channel = -2;
        a.end_channel = i32::MAX;
        a.start_count = 0x55aa_55aa_u32 as i32;
        a.suspend_on_interrupt = true;
        a.analog_power_control = true;
        a.mode = 0x6d;
        a.channel = 7;
        a.count = -1;
        a.analog_powered = false; // quirky flip (ctor says true)
        a.adtrg = false;
        a.next_event = u64::MAX - 7;

        let mut s1 = Vec::new();
        {
            let mut s = StateIo::writer(&mut s1);
            a.state(&mut s);
            assert!(s.ok(), "save: {}", s.error());
        }
        assert_eq!(&s1[0..8], b"adc\0\0\0\0\0");
        // 16 + 4 addr, 4 buf, 2 u8, 5 ints, 2 bools, 3 ints, 2 bools, u64
        assert_eq!(s1.len(), 8 + 16 + 4 + 2 + 20 + 2 + 12 + 2 + 8);

        let mut fresh = Adc::new_ms(0, 136);
        {
            let mut s = StateIo::reader(&s1);
            fresh.state(&mut s);
            assert!(s.ok(), "load: {}", s.error());
        }
        let mut s2 = Vec::new();
        {
            let mut s = StateIo::writer(&mut s2);
            fresh.state(&mut s);
            assert!(s.ok());
        }
        assert_eq!(s1, s2);
        assert_eq!(fresh.addr, a.addr);
        assert_eq!(fresh.next_event, a.next_event);
        assert_eq!(fresh.analog_powered, false);
        assert_eq!(fresh.adtrg, false);
    }
}
