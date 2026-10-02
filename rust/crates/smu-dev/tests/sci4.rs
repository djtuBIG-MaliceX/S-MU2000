//! origin: src/mame/machine/sci4.cpp + sci4.h (row `sci4`, smu-dev).
//! Register map (sci4.cpp:24-52, bus 0xf00000-0x3f per mu2000.cpp:965-972):
//! chan c base = c*8; +0 data, +1 enable, +2 status (READ only — writes fall
//! to default_w :51), +3 datamode, +5 reset (READ-triggered :32); 0x20 target.
//! Timers go through the SHARED compat queue (sci4.cpp:76-79,730-737) at the
//! device clock 8 MHz (sci4.h:21) on the 28 MHz machine (mu2000.cpp:81):
//! full bit = div*16/8e6*28e6 = div*56 machine cycles, half = div*28.

use smu_compat::timers::RunningMachine;
use smu_dev::sci4::Sci4;

fn mach() -> RunningMachine {
    let mut m = RunningMachine::new();
    m.set_clock_hz(28_000_000); // mu2000.cpp:81 (7 MHz ×4)
    m
}

/// start + reset, per mu2000.cpp:1043+ start_devices / :1151-1152 resets.
/// Device clock 8 MHz = sci4.h:21 default (mu2000.cpp:78 passes none).
fn dev(m: &mut RunningMachine) -> Sci4 {
    let mut d = Sci4::new(8_000_000);
    d.device_start(m);
    d.device_reset();
    d
}

/// expected full-bit delay at div: from_ticks(div*16, 8e6) then the
/// `u64(t * 28e6)` truncation of mamecompat.h:736 (timers.rs:233).
fn full(div: u32) -> u64 {
    let d = if div != 0 { div } else { 0x100 }; // sci4.cpp:315
    ((d * 16) as f64 / 8_000_000.0 * 28_000_000.0) as u64
}

/// expected half-bit delay (rx resync / start-edge arm), full=0 branch.
fn half(div: u32) -> u64 {
    let d = if div != 0 { div } else { 0x100 };
    ((d * 8) as f64 / 8_000_000.0 * 28_000_000.0) as u64
}

fn advance(m: &mut RunningMachine, t: u64) {
    m.set_cycles(t);
    m.run_timers(t);
}

#[test]
fn t01_new_matches_device_reset_values() {
    // origin: sci4.cpp:82-102 + invariant 3 (explicit init at construction).
    let mut m = mach();
    let d = dev(&mut m);
    let c = d.core().borrow();
    assert_eq!(c.rx, [1u8; 7]); // :84
    assert_eq!(c.cur_rx, [1u8; 4]); // :89
    assert_eq!((c.enable, c.status, c.datamode, c.div), ([0; 4], [0; 4], [0; 4], [0; 4])); // :85-88
    assert_eq!((c.tdr, c.tsr, c.tdr_full, c.tx_step, c.tx_active), ([0; 4], [0; 4], [0; 4], [0; 4], [0; 4])); // :90-94
    assert_eq!((c.rdr, c.rsr, c.rdr_full, c.rx_step, c.rx_active), ([0; 4], [0; 4], [0; 4], [0; 4], [0; 4])); // :95-99
    assert_eq!(c.targets, 0); // :101
    assert_eq!(c.irq_line, [0i8; 4]); // mu2000.h:898 {0,0}
    assert_eq!(c.tx_line, [1i8; 7]); // idle-high seam init
}

#[test]
fn t02_device_reset_clears_and_timer_survives_quirk() {
    // origin: sci4.cpp:82-102 — device_reset does NOT cancel timers (disk
    // quirk; mu2000.cpp:1151-1152 only resets at boot, idle).
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 3, 0x80); // datamode 0 = baud mode
    d.write8(&mut m, 0, 1); // div[0] = 1
    d.write8(&mut m, 3, 2);
    d.write8(&mut m, 1, 2); // enable tx -> status2/irq1 (:277-285)
    m.set_cycles(1000);
    d.write8(&mut m, 0, 0x41); // tx_start armed at 1000+56
    assert_eq!(m.next_timer_cycles(), 1000 + full(1));
    d.device_reset();
    let c = d.core().borrow();
    assert_eq!((c.status, c.tx_active, c.tdr_full, c.enable, c.div), ([0; 4], [0; 4], [0; 4], [0; 4], [0; 4]));
    assert_eq!(c.irq_line, [0i8; 4]);
    drop(c);
    assert_ne!(m.next_timer_cycles(), u64::MAX); // stale tx timer survives reset
}

#[test]
fn t03_read_holes_and_default_r() {
    // origin: sci4.cpp:24-36 — &7 ∈ {4,6,7} and >=0x20 fall to default_r→0
    // (:128-132). 0x20 is WRITE-ONLY (target_w never decoded on read).
    let mut m = mach();
    let mut d = dev(&mut m);
    for off in [4u32, 6, 7, 0x14, 0x1d, 0x20, 0x21, 0x3f] {
        assert_eq!(d.read8(&mut m, off), 0, "hole read 0x{off:02x}");
    }
}

#[test]
fn t04_reset_r_read_triggers_full_clear() {
    // origin: sci4.cpp:194-204 — READ of offset&7==5 clears status/IRQ,
    // adjusts the tx timer to never (:198, disk default param 0), idles tx.
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 3, 0x80);
    d.write8(&mut m, 0, 1);
    d.write8(&mut m, 3, 2);
    d.write8(&mut m, 1, 2);
    m.set_cycles(1000);
    d.write8(&mut m, 0, 0x55); // in flight
    assert_eq!(d.read8(&mut m, 5), 0); // :203 returns 0
    let c = d.core().borrow();
    assert_eq!(c.status[0], 0); // :197
    assert_eq!(c.tdr_full[0], 0); // :201
    assert_eq!(c.tx_active[0], 0); // :199
    assert_eq!(c.tx_line[0], 1); // :200 tx_set(chan,1)
    assert_eq!(c.irq_line[0], 0); // :202
    drop(c);
    assert_eq!(m.next_timer_cycles(), u64::MAX); // :198 cancel
}

#[test]
fn t05_write_holes_do_not_reset_or_touch_status() {
    // origin: sci4.cpp:38-52 — writes to &7 ∈ {2,5} and >=0x21 (except the
    // bus edge; map ends at 0x3f mu2000.cpp:968) are default_w (:123-126).
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 3, 2);
    d.write8(&mut m, 1, 2); // status2 + irq1 (:282-283)
    d.write8(&mut m, 2, 0xff); // status "register" write -> hole
    d.write8(&mut m, 5, 0xff); // reset "register" write -> hole (NOT reset!)
    d.write8(&mut m, 6, 0xff);
    d.write8(&mut m, 0x21, 0xff);
    d.write8(&mut m, 0x3f, 0xff);
    let c = d.core().borrow();
    assert_eq!(c.status[0], 2);
    assert_eq!(c.irq_line[0], 1);
    assert_eq!(c.enable[0], 2);
    assert_eq!(c.targets, 0);
}

#[test]
fn t06_datamode_div_roundtrip_all_slots() {
    // origin: sci4.cpp:144-166 — datamode 0x80 turns the data register into
    // the baud divisor; slot select is offset>>3 (.select(0x18) comment :20).
    let mut m = mach();
    let mut d = dev(&mut m);
    for ch in 0..4u32 {
        let b = ch * 8;
        d.write8(&mut m, b + 3, 0x80);
        d.write8(&mut m, b, (0x11 * (ch + 1) as u8) | 1);
    }
    for ch in 0..4u32 {
        let b = ch * 8;
        assert_eq!(d.read8(&mut m, b), (0x11 * (ch + 1) as u8) | 1);
    }
}

#[test]
fn t07_baud_divider_vectors_8mhz_to_28mhz() {
    // origin: sci4.cpp:313-318 — div*16 ticks at DEVICE clock 8 MHz
    // (sci4.h:21) truncated into 28 MHz cycles at mamecompat.h:736.
    // Proves the ratio is 3.5×, not 1:1 (a machine-clock bug would give
    // div*16 cycles).
    for (div, want) in [(1u32, 56u64), (4, 224), (16, 896), (255, 14280), (0, 14336)] {
        // fresh machine each vector: a previous tx timer would still be
        // scheduled at its old expire and pollute next_timer_cycles
        let mut m = mach();
        let mut d = dev(&mut m);
        d.write8(&mut m, 3, 0x80);
        d.write8(&mut m, 0, div as u8);
        d.write8(&mut m, 3, 2);
        d.write8(&mut m, 1, 2);
        m.set_cycles(0);
        d.write8(&mut m, 0, 0x41); // tx_start -> wait(0,1)
        assert_eq!(m.next_timer_cycles(), want, "div {div}");
        assert_eq!(want, full(div));
        if div != 0 {
            assert_ne!(m.next_timer_cycles(), u64::from(div * 16)); // not raw ticks
        }
    }
}

#[test]
fn t08_boot_irq_assert_on_tx_enable_no_peer() {
    // THE boot path (row note: firmware waits on this IRQ or the main loop
    // never starts): enable bit2 rising with an EMPTY data register asserts
    // status=2 + the IRQ line at once — sci4.cpp:168-174 -> :277-285
    // (:282-283). No PLG peer byte exists on this machine (mu2000.cpp:77).
    // Wiring: line0/1 -> CPU IRQ0 pin -> INTC vector 64 (mu2000.cpp:1130-1131,
    // sh7042.cpp:141-143, intc.rs:206-232); line3 -> IRQ1 -> vector 66.
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 1, 2); // enable ch0: tx
    let c = d.core().borrow();
    assert_eq!(c.status[0], 2); // :282
    assert_eq!(c.irq_line[0], 1); // :283 ASSERT (level)
    assert_eq!(c.tx_line, [1i8; 7]); // nothing driven
    drop(c);
    assert_eq!(m.next_timer_cycles(), u64::MAX); // no timer
}

#[test]
fn t09_disable_falling_edge_clears_level_irq() {
    // origin: sci4.cpp:176-181 — enable bit2 falling with status==2 is the
    // IRQ deassert path (level: the line tracks status).
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 1, 2);
    assert_eq!(d.core().borrow().irq_line[0], 1);
    d.write8(&mut m, 1, 0);
    let c = d.core().borrow();
    assert_eq!(c.status[0], 0); // :178
    assert_eq!(c.irq_line[0], 0); // :179 CLEAR
}

#[test]
fn t10_tx_frame_bits_lsb_first_level_irq() {
    // origin: :240-245 accept clears the status-2 IRQ then :301-302
    // re-asserts; :296-311 start bit low; :320-333 ticks 0-7 = data bits
    // LSB-first, tick 8 = stop high, tick 9 = done (:328 tdr_full=0 ->
    // :331 tx_active=0). IRQ is HELD (status stays 2) all frame.
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 3, 0x80);
    d.write8(&mut m, 0, 1); // div=1 -> 56 cycles/bit @28MHz
    d.write8(&mut m, 3, 2);
    d.write8(&mut m, 1, 2); // status2 + irq (:282-283)
    m.set_cycles(1000);
    d.write8(&mut m, 0, 0x55); // 0b01010101
    let bits = [0i8, 1, 0, 1, 0, 1, 0, 1, 0, 1, 1]; // start, b0..b7, stop, idle
    let mut t = 1000u64;
    assert_eq!(d.core().borrow().tx_line[0], bits[0]); // :308 start low
    assert_eq!(d.core().borrow().irq_line[0], 1); // :302 re-assert
    for k in 1..=9u64 {
        // ticks 1..9 = steps 0..8 (data LSB-first, tick9 step8 = stop)
        t += full(1);
        advance(&mut m, t);
        assert_eq!(d.core().borrow().tx_line[0], bits[k as usize], "tick {k}");
    }
    t += full(1);
    advance(&mut m, t); // tick 10 = step9 -> done
    let c = d.core().borrow();
    assert_eq!(c.tx_active[0], 0); // :331
    assert_eq!(c.irq_line[0], 1); // LEVEL: still asserted (status==2)
    drop(c);
    assert_eq!(m.next_timer_cycles(), u64::MAX);
    assert_eq!(d.read8(&mut m, 5), 0); // register reset ends the conversation
    assert_eq!(d.core().borrow().irq_line[0], 0);
}

#[test]
fn t11_tx_overrun_status6_irq_enable_mask() {
    // origin: sci4.cpp:231-235 — second byte while tdr_full AND (enable & 4)
    // -> status=6 + IRQ assert. Enable 4 = IRQ-enable mask (:233, :352).
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 0x0b, 2); // ch1 datamode (0x0b>>3==1)
    d.write8(&mut m, 9, 0x04); // enable: irq-en only (no tx bit2)
    d.write8(&mut m, 8, 0x11); // accepted (:238-239), no start (no bit2)
    {
        let c = d.core().borrow();
        assert_eq!((c.tdr[1], c.tdr_full[1], c.status[1]), (0x11, 1, 0));
    }
    d.write8(&mut m, 8, 0x22); // overrun
    let c = d.core().borrow();
    assert_eq!(c.status[1], 6); // :234
    assert_eq!(c.irq_line[1], 1); // :235
    assert_eq!(c.tdr[1], 0x11); // new byte dropped
}

#[test]
fn t12_rx_byte_ff_flag_clear_by_data_read() {
    // origin: rx start edge arms HALF period (:261-265 wait(1,0)); :335-358
    // ticks: 0=resync, 1-8 sample cur_rx LSB-first, 9=stop -> rdr, status=4
    // (:355), IRQ ASSERT (:356, unconditional). The clear is the fifo_r on a
    // data read with datamode bit1 (:162-163, :249-256) -> level IRQ drops.
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 0x0b, 0x80); // ch1 datamode 0x80 (0x0b>>3==1)
    d.write8(&mut m, 8, 1); // div=1
    d.write8(&mut m, 0x0b, 1); // datamode bit1 -> data_r reads the rx fifo
    d.write8(&mut m, 9, 1); // enable ch1 rx
    m.set_cycles(500);
    d.rx_w(&mut m, 1, 0); // start bit at t=500 (window 500..556, center 528)
    assert_eq!(m.next_timer_cycles(), 500 + half(1)); // :316 div*8 branch
    advance(&mut m, 500 + half(1)); // tick0 :338-339 resync full -> 584
    m.set_cycles(500 + 2 * half(1)); // start-bit boundary 556 (no tick due)
    d.rx_w(&mut m, 1, 1); // line high (data=0xFF); :272-273 resync half -> 584
    let mut t = 500 + 3 * half(1); // 584 = center of bit0
    for _ in 0..8 {
        advance(&mut m, t); // ticks 1..8 sample cur_rx=1 (:341-342)
        t += full(1);
    }
    advance(&mut m, t); // tick 9 = stop (line already high)
    let c = d.core().borrow();
    assert_eq!(c.rdr[1], 0xff); // :351
    assert_eq!(c.status[1], 4); // :355 (status 6 is disk-dead: rdr_full never 1)
    assert_eq!(c.irq_line[1], 1); // :356 ASSERT on the disk vector path
    assert_eq!(c.rx_active[1], 0); // :350
    drop(c);
    assert_eq!(m.next_timer_cycles(), u64::MAX);
    assert_eq!(d.read8(&mut m, 8), 0xff); // fifo_r :249-256
    let c = d.core().borrow();
    assert_eq!(c.status[1], 0); // :253
    assert_eq!(c.irq_line[1], 0); // :254 CLEAR -> wiring drops the CPU line
}

#[test]
fn t13_rx_bit_sampled_lsb_first_with_resyncs() {
    // origin: every rx line change re-edges do_rx_w (:110-114) and a change
    // mid-frame forces a HALF-period resync (:272-273). 0x01: bit0=1, bit7..1
    // =0; sampling lands bit-center on each bit window.
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 0x13, 0x80); // ch2
    d.write8(&mut m, 16, 1);
    d.write8(&mut m, 0x13, 0);
    d.write8(&mut m, 17, 1); // enable rx
    m.set_cycles(0);
    d.rx_w(&mut m, 2, 0); // start (window 0..56)
    advance(&mut m, half(1)); // tick0 (start center 28) -> resync full -> 84
    m.set_cycles(2 * half(1)); // boundary 56, bit0 window starts
    d.rx_w(&mut m, 2, 1); // bit0=1 -> resync half -> 84 (bit0 center)
    advance(&mut m, 3 * half(1)); // tick1 samples 1
    m.set_cycles(4 * half(1)); // boundary 112, zeros begin
    d.rx_w(&mut m, 2, 0); // bits 1..7 = 0 -> resync half -> 140 (bit1 center)
    for k in 7..=17u64 {
        advance(&mut m, k * half(1)); // ticks 2..8 at bit centers 196..476
    }
    m.set_cycles(18 * half(1)); // 504 = stop window start
    d.rx_w(&mut m, 2, 1); // stop high -> resync half -> 532 (stop center)
    advance(&mut m, 19 * half(1)); // tick9 = stop
    let c = d.core().borrow();
    assert_eq!(c.rsr[1 + 0], 0); // ch1 untouched (:342 only samples asserted)
    assert_eq!(c.rdr[2], 0x01); // only bit0 set
    assert_eq!(c.status[2], 4);
    assert_eq!(c.irq_line[2], 1); // note: line2 is UNBOUND on this machine
    // (mu2000.cpp wires irq 0,1,3 only) — the seam records it faithfully.
}

#[test]
fn t14_rx_framing_error_no_irq_stays_active() {
    // origin: sci4.cpp:345-347 — stop bit low: log only; rx_active STAYS 1,
    // no timer armed. The next line change then takes the :272-273 resync
    // branch (not a fresh arm). No IRQ ever asserts.
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 0x0b, 0x80);
    d.write8(&mut m, 8, 1);
    d.write8(&mut m, 0x0b, 0);
    d.write8(&mut m, 9, 1);
    m.set_cycles(0);
    d.rx_w(&mut m, 1, 0); // start, never released properly:
    let mut t = half(1);
    advance(&mut m, t); // tick0
    for _ in 1..=9 {
        // ticks 1..9: all bits low, tick9 = failed stop check
        t += full(1);
        advance(&mut m, t);
    }
    let c = d.core().borrow();
    assert_eq!(c.irq_line[1], 0); // no IRQ
    assert_eq!(c.rx_active[1], 1); // :346 path leaves it active
    assert_eq!(c.rdr[1], 0); // nothing latched
    drop(c);
    assert_eq!(m.next_timer_cycles(), u64::MAX); // :347 no re-arm
    d.rx_w(&mut m, 1, 1); // recovery edge -> resync
    assert_eq!(m.next_timer_cycles(), m.cycles() + half(1)); // :273
}

#[test]
fn t15_mux_chan3_rx_from_peer_ports_30() {
    // origin: :104-107 port 30..33 -> chan 3..6; :116 the chan3 aggregate is
    // the AND of selected lines, selected by the HIGH nibble (targets>>4,
    // :116 — the LOW nibble selects tx fan-out, :216/:226; disk asymmetry,
    // kept verbatim). Peer seam = rx_w(30..32) — disk wiring is
    // ymmu2000.cpp:407-425 PLG connector midi_tx; mu2000 feeds NOTHING.
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 0x1b, 0x80); // ch3 datamode 0x80 (0x1b>>3==3)
    d.write8(&mut m, 24, 1); // div[3]=1
    d.write8(&mut m, 0x1b, 0);
    d.write8(&mut m, 25, 1); // enable ch3 rx
    d.write8(&mut m, 0x20, 0x10); // targets: rx-select line3, unselected tx->1
    {
        let c = d.core().borrow();
        assert_eq!((c.tx_line[4], c.tx_line[5], c.tx_line[6]), (1, 1, 1)); // :215-217
    }
    m.set_cycles(0);
    d.rx_w(&mut m, 30, 0); // chan3 start
    assert_eq!(m.next_timer_cycles(), half(1)); // aggregate edge -> :261-265
    advance(&mut m, half(1)); // tick0 resync
    advance(&mut m, 2 * half(1));
    d.rx_w(&mut m, 30, 1); // aggregate high again
    let mut t = m.cycles() + half(1);
    for _ in 0..8 {
        advance(&mut m, t);
        t += full(1);
    }
    advance(&mut m, t); // stop tick
    let c = d.core().borrow();
    assert_eq!(c.rdr[3], 0xff);
    assert_eq!(c.status[3], 4);
    assert_eq!(c.irq_line[3], 1); // -> IRQ1 pin, INTC vector 66 (:1132)
}

#[test]
fn t16_mux_chan3_tx_drives_selected_target_lines() {
    // origin: :220-229 — chan3 tx drives EVERY line whose LOW-nibble target
    // bit is set (:226); the IRQ of chan3 also uses the vector-66 path.
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 0x1b, 0x80);
    d.write8(&mut m, 24, 1);
    d.write8(&mut m, 0x1b, 2);
    d.write8(&mut m, 25, 2); // tx enable -> boot-style IRQ (:282-283)
    d.write8(&mut m, 0x20, 0x03); // tx-select lines 3,4; forces 5,6 high
    m.set_cycles(0);
    d.write8(&mut m, 24, 0xab); // start bit
    let c = d.core().borrow();
    assert_eq!((c.tx_line[3], c.tx_line[4]), (0, 0)); // :226-227 start low
    assert_eq!((c.tx_line[5], c.tx_line[6]), (1, 1)); // :215-217 + never touched
    assert_eq!(c.irq_line[3], 1); // still asserted (status==2, :301-302)
}

#[test]
fn t17_status_read_does_not_clear_irq() {
    // origin: sci4.cpp:189-192 status_r is a pure read — LEVEL semantics:
    // the firmware must read the DATA register (fifo_r :252-255), disable
    // (:176-181) or read reset (:194-204) to drop the line.
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 1, 2);
    for _ in 0..3 {
        assert_eq!(d.read8(&mut m, 2), 2);
        assert_eq!(d.core().borrow().irq_line[0], 1);
    }
}

#[test]
fn t18_start_bit_abort_before_sample() {
    // origin: sci4.cpp:266-271 — line returns high while rx_active and
    // rx_step==0 (before the sample tick): receiver aborts and the rx timer
    // is adjusted to never (disk default param 0, mirrors :270).
    let mut m = mach();
    let mut d = dev(&mut m);
    d.write8(&mut m, 1, 1); // enable ch0 rx; div 0 -> default 0x100
    m.set_cycles(0);
    d.rx_w(&mut m, 0, 0); // start
    assert_eq!(m.next_timer_cycles(), half(0)); // default div 0x100
    d.rx_w(&mut m, 0, 1); // glitch: back high before tick0
    let c = d.core().borrow();
    assert_eq!(c.rx_active[0], 0); // :269
    drop(c);
    assert_eq!(m.next_timer_cycles(), u64::MAX); // :270 cancel
}

#[test]
fn t19_target_read_is_hole_and_target_w_decodes_only_at_20() {
    // origin: read8 has no case for >=0x20 (:26-33) -> 0; write8 decodes
    // target only at exactly 0x20 (:40-43).
    let mut m = mach();
    let mut d = dev(&mut m);
    assert_eq!(d.read8(&mut m, 0x20), 0);
    d.write8(&mut m, 0x18, 0x0f); // 0x18: &7==0 -> data_w (datamode 0 -> no-op)
    assert_eq!(d.core().borrow().targets, 0);
    d.write8(&mut m, 0x20, 0x5a);
    assert_eq!(d.core().borrow().targets, 0x5a);
}

#[test]
fn t20_slot_decode_18_stride() {
    // origin: .select(0x18) comment :20 — handlers receive the raw offset and
    // shift >>3 themselves (e.g. :170,:186). Same 4 registers per channel at
    // offsets c*8+{0,1,2,3,5}.
    let mut m = mach();
    let mut d = dev(&mut m);
    // (ch<<2) keeps the tx-enable bit (value 2) clear in every slot — a set
    // bit would fire the :173-174 edge (that is t08's job).
    for ch in 0..4u32 {
        d.write8(&mut m, ch * 8 + 3, 0xa0 | ch as u8);
        d.write8(&mut m, ch * 8 + 1, (0x20 | (ch << 2)) as u8);
    }
    for ch in 0..4u32 {
        assert_eq!(d.read8(&mut m, ch * 8 + 3), 0xa0 | ch as u8);
        assert_eq!(d.read8(&mut m, ch * 8 + 1), (0x20 | (ch << 2)) as u8);
        assert_eq!(d.read8(&mut m, ch * 8 + 2), 0); // status untouched
    }
}

// ---- M5-W3a: state() (sci4.cpp:363-374) + timers via machine state_sync ----

use smu_compat::timers::TimerId;
use smu_compat::StateIo;

fn dsave(d: &mut Sci4) -> Vec<u8> {
    let mut o = Vec::new();
    {
        let mut s = StateIo::writer(&mut o);
        d.state(&mut s);
        assert!(s.ok(), "sci4 save: {}", s.error());
    }
    o
}

fn msync(m: &mut RunningMachine) -> Vec<u8> {
    let mut o = Vec::new();
    {
        let mut s = StateIo::writer(&mut o);
        m.state_sync(&mut s);
        assert!(s.ok(), "mach sync: {}", s.error());
    }
    o
}

#[test]
fn sci4_state_roundtrip_quirky_and_timers_ride_machine_sync() {
    let mut m = mach();
    let mut d = Sci4::new(8_000_000);
    d.device_start(&mut m); // 8 timers, birth order tx0,rx0,tx1,rx1,... (:76-79)
    let (tx, rx) = d.timer_ids();
    // shared queue slots: the SCI4 owns indices 0..8 in interleaved order
    assert_eq!(tx[1], TimerId(2));
    assert_eq!(rx[3], TimerId(7));
    {
        let c = d.core();
        let mut c = c.borrow_mut();
        c.rx = [9, 8, 7, 6, 5, 4, 3]; // :366
        c.enable = [0x81, 0x02, 0x04, 0x08]; // :367
        c.status = [1, 2, 4, 6];
        c.datamode = [0x80, 3, 5, 7];
        c.div = [0x10, 0x81, 0x30, 0xff]; // :368
        c.cur_rx = [0, 1, 0, 1];
        c.tdr = [0xa5, 0x5c, 0, 0]; // :369
        c.tsr = [0, 0, 0, 0x7f];
        c.tdr_full = [1, 0, 1, 0];
        c.tx_step = [9, 254, 0, 0]; // :370 254 rides the u8 wrap width
        c.tx_active = [0, 1, 0, 0];
        c.rdr = [0x33, 0, 0, 0]; // :371
        c.rsr = [0, 0xcc, 0, 0];
        c.rdr_full[2] = 1; // DEAD-QUIRK byte — load may inject it (header doc)
        c.rx_step = [1, 0, 0, 10]; // :372
        c.rx_active = [0, 0, 1, 0];
        c.targets = 0x2a; // :373
        c.wait(&mut m, 0, 1, 1); // arm tx1 for div*16 ticks (sci4.cpp:313-318)
    }
    let s1 = dsave(&mut d);
    assert_eq!(s1.len(), 8 + 7 + 15 * 4 + 1); // tag + rx + 15 stdarr + targets
    assert_eq!(&s1[0..8], b"sci4\0\0\0\0");
    // hand-verified offsets: rx@8, then 15 x [u8;4] legs, targets last
    assert_eq!(s1[8 + 3], 6); // rx[3]
    assert_eq!(s1[8 + 7 + 12 * 4 + 2], 1); // rdr_full[2] == byte 65
    assert_eq!(s1[75], 0x2a); // targets

    // machine leg (mamecompat.h:699-712): tag + seed + cycles + n + 8 timers
    let m1 = msync(&mut m);
    assert_eq!(&m1[0..8], b"mach\0\0\0\0");
    assert_eq!(&m1[20..24], &8u32.to_le_bytes()); // timer count
    assert_eq!(&m1[24..32], &[0xff; 8]); // tx0 expire == never
    // tx1 = queue slot 2: expire @ 24+2*12 = 48, param i32 @ 56
    assert_eq!(m1[56], 1); // param = chan 1 (sci4.cpp:317 adjust param)
    let armed = u64::from_le_bytes(m1[48..56].try_into().unwrap());
    // disk math (mamecompat.h:736): trunc((div*16/8e6)*28e6), div=0x81
    let want = ((0x81u64 * 16) as f64 / 8_000_000.0 * 28_000_000.0) as u64;
    assert_eq!(armed, want);
    assert_eq!(m.timer_expire(tx[1]), want);
    assert!(!m.timer_scheduled(tx[0]));

    // load into a FRESH machine + device (same birth order ⇒ slot alignment)
    let mut m2 = mach();
    let mut d2 = Sci4::new(8_000_000);
    d2.device_start(&mut m2);
    {
        let mut s = StateIo::reader(&s1);
        d2.state(&mut s);
        assert!(s.ok(), "sci4 load: {}", s.error());
        let mut s = StateIo::reader(&m1);
        m2.state_sync(&mut s);
        assert!(s.ok(), "mach load: {}", s.error());
    }
    assert_eq!(d2.core().borrow().rdr_full[2], 1); // dead-quirk injected
    assert_eq!(d2.core().borrow().targets, 0x2a);
    assert_eq!(m2.timer_expire(TimerId(2)), want); // the schedule itself rode
    assert_eq!(dsave(&mut d2), s1);
    assert_eq!(msync(&mut m2), m1);
}
