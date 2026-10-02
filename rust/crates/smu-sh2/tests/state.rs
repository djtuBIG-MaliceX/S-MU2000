//! M5-W2 device-tree `state(state_io&)` — layout golden + per-device
//! save→load→save byte equality.
//!
//! The shcore offsets below are NOT hand-computed: they are the verbatim
//! `g++ -std=c++20 -O3` (MSYS2 mingw64) `offsetof`/`sizeof` output for a
//! byte-copy of `src/mame/cpu/sh.h:106-159 struct internal_sh2_state`
//! (ground-truth harness, 2026-10-02, %TEMP%\shlayout\l.cpp):
//!   sizeof=424 align=8, sleep_mode@120(1B) + 3B tail pad, arg0@124.
//! Stream length for `Sh2Core::state` = 8 tag + 424 POD + 4 pcfsel +
//! 8 total_cycles + 4 cycles_this_run = 448.

use smu_compat::StateIo;
use smu_sh2::core::Sh2Core;
use smu_sh2::device::Sh2Device;
use smu_sh2::periph::cmt::ShCmt;
use smu_sh2::periph::intc::Sh2Intc;
use smu_sh2::periph::mtu::Sh2Mtu;
use smu_sh2::periph::port::{ShPort16, ShPort32};
use smu_sh2::periph::sci::Sh2Sci;
use smu_sh2::periph::stubs::{ShBsc, ShDmac, ShDmacChannel};
use smu_sh2::sh7042::{Sh7042, Sh7042Peripherals};
use std::cell::RefCell;
use std::rc::Rc;

fn save<T>(d: &mut T, mut f: impl FnMut(&mut T, &mut StateIo)) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut s = StateIo::writer(&mut out);
        f(d, &mut s);
        assert!(s.ok(), "save failed: {}", s.error());
    }
    out
}

fn roundtrip<T>(mut a: T, mut fresh: T, mut f: impl FnMut(&mut T, &mut StateIo)) -> Vec<u8> {
    let s1 = save(&mut a, |d, s| f(d, s));
    {
        let mut s = StateIo::reader(&s1);
        f(&mut fresh, &mut s);
        assert!(s.ok(), "load failed: {}", s.error());
    }
    let s2 = save(&mut fresh, |d, s| f(d, s));
    assert_eq!(s1, s2, "save->load->save bytes diverge");
    s1
}

// ---------------------------------------------------------------------------
// shcore layout golden (g++ harness values — see module doc)
// ---------------------------------------------------------------------------
#[test]
fn shcore_stream_is_448_and_fresh_is_zero_apart_from_tags() {
    let mut core = Sh2Core::new(0xffff_ffff);
    let s = save(&mut core, |d, s| d.state(s));
    assert_eq!(s.len(), 8 + 424 + 4 + 8 + 4); // tag + POD + pcfsel + u64 + int
    assert_eq!(&s[0..8], b"shcore\0\0");
    assert!(s[8..432].iter().all(|&b| b == 0), "fresh POD must be zero");
}

#[test]
fn shcore_field_offsets_match_gpp_ground_truth() {
    let mut c = Sh2Core::new(0xffff_ffff);
    // one distinctive value per field; the byte must land at 8 + offsetof
    c.pc = 0x0102_0304; // off 0
    c.pr = 0x0506_0708; // 4
    c.sr = 0x090a_0b0c; // 8
    c.mach = 0x0d0e_0f10; // 12
    c.macl = 0x1112_1314; // 16
    c.r[15] = 0x1516_1718; // 20+15*4 = 80
    c.ea = 0x191a_1b1c; // 84
    c.pending_irq = 0x1d1e_1f20; // 88
    c.pending_nmi = 0x2122_2324; // 92
    c.irqline = -0x1122_3344; // 96 (int32)
    c.evec = 0x2526_2728; // 100
    c.irqsr = 0x292a_2b2c; // 104
    c.target = 0x2d2e_2f30; // 108
    c.internal_irq_level = -1; // 112 (int)
    c.icount = -123456; // 116 (int)
    c.sleep_mode = 0x5a; // 120
    c.pad_sleep = [0xa1, 0xa2, 0xa3]; // 121..124 (ABI tail pad)
    c.arg0 = 0x3132_3334; // 124
    c.arg1 = 0x3536_3738; // 128
    c.gbr = 0x393a_3b3c; // 132
    c.vbr = 0x3d3e_3f40; // 136
    c.m_delay = 0x4142_4344; // 140
    c.m_ppc = 0x4546_4748; // 144
    c.m_spc = 0x494a_4b4c; // 148
    c.m_ssr = 0x4d4e_4f50; // 152
    c.m_rbnk[15] = 0x5152_5354; // 156+60 = 216 ([2][8] flat, last)
    c.m_sgr = 0x5556_5758; // 220
    c.m_fr[15] = 0x595a_5b5c; // 224+60 = 284
    c.m_xf[15] = 0x5d5e_5f60; // 288+60 = 348
    c.m_cpu_off_drc = 0x6162_6364; // 352
    c.m_pending_irq = 0x6566_6768; // 356
    c.m_test_irq_drc = 0x696a_6b6c; // 360
    c.m_fpscr = 0x6d6e_6f70; // 364
    c.m_fpul = 0x7172_7374; // 368
    c.m_dbr = 0x7576_7778; // 372
    c.m_ftrc_dmin = f64::from_bits(0x4008_d2f1_a9fc_e66a); // 376 (1.1d)
    c.m_ftrc_dmax = f64::from_bits(0xc08f_ffffffff_ffffu64 & 0xffff_ffff_ffff_ffff); // 384 (-1000d-ish)
    c.m_ftrc_dmax = f64::from_bits(0xc08f_f333_3333_3333); // 384 (-1000.0)
    c.m_ftrc_smin = f32::from_bits(0xcf_adbe_ef); // 392
    c.m_ftrc_smax = f32::from_bits(0x447c_0000); // 396 (1008.0f)
    c.m_fzero = f32::from_bits(0x8000_0000); // 400 (-0.0f — byte-checked, not ==)
    c.m_fone = f32::from_bits(0x3f80_0000); // 404 (1.0f)
    c.m_fpmode = [1, 2, 3, 4]; // 408
    c.m_frt_input = -7; // 412
    c.m_fpu_sz = 0x7fff_0001u32 as i32; // 416
    c.m_fpu_pr = i32::MIN; // 420
    let s = save(&mut c, |d, s| d.state(s));
    let o = |off: usize, bytes: &[u8]| {
        assert_eq!(&s[8 + off..8 + off + bytes.len()], bytes, "offset {off}");
    };
    o(0, &0x0102_0304u32.to_le_bytes());
    o(4, &0x0506_0708u32.to_le_bytes());
    o(8, &0x090a_0b0cu32.to_le_bytes());
    o(12, &0x0d0e_0f10u32.to_le_bytes());
    o(16, &0x1112_1314u32.to_le_bytes());
    o(80, &0x1516_1718u32.to_le_bytes()); // r[15]
    o(84, &0x191a_1b1cu32.to_le_bytes());
    o(88, &0x1d1e_1f20u32.to_le_bytes());
    o(92, &0x2122_2324u32.to_le_bytes());
    o(96, &(-0x1122_3344i32).to_le_bytes());
    o(100, &0x2526_2728u32.to_le_bytes());
    o(104, &0x292a_2b2cu32.to_le_bytes());
    o(108, &0x2d2e_2f30u32.to_le_bytes());
    o(112, &(-1i32).to_le_bytes());
    o(116, &(-123456i32).to_le_bytes());
    o(120, &[0x5a]);
    o(121, &[0xa1, 0xa2, 0xa3]); // THE 3-BYTE PAD rides verbatim
    o(124, &0x3132_3334u32.to_le_bytes());
    o(128, &0x3536_3738u32.to_le_bytes());
    o(132, &0x393a_3b3cu32.to_le_bytes());
    o(136, &0x3d3e_3f40u32.to_le_bytes());
    o(140, &0x4142_4344u32.to_le_bytes());
    o(144, &0x4546_4748u32.to_le_bytes());
    o(148, &0x494a_4b4cu32.to_le_bytes());
    o(152, &0x4d4e_4f50u32.to_le_bytes());
    o(216, &0x5152_5354u32.to_le_bytes()); // m_rbnk[1][7] == flat [15]
    o(220, &0x5556_5758u32.to_le_bytes());
    o(284, &0x595a_5b5cu32.to_le_bytes()); // m_fr[15]
    o(348, &0x5d5e_5f60u32.to_le_bytes()); // m_xf[15]
    o(352, &0x6162_6364u32.to_le_bytes());
    o(356, &0x6566_6768u32.to_le_bytes());
    o(360, &0x696a_6b6cu32.to_le_bytes());
    o(364, &0x6d6e_6f70u32.to_le_bytes());
    o(368, &0x7172_7374u32.to_le_bytes());
    o(372, &0x7576_7778u32.to_le_bytes());
    o(376, &0x4008_d2f1_a9fc_e66au64.to_le_bytes());
    o(384, &0xc08f_f333_3333_3333u64.to_le_bytes());
    o(392, &0xcf_adbe_efu32.to_le_bytes());
    o(396, &0x447c_0000u32.to_le_bytes());
    o(400, &0x8000_0000u32.to_le_bytes());
    o(404, &0x3f80_0000u32.to_le_bytes());
    o(408, &[1, 2, 3, 4]);
    o(412, &(-7i32).to_le_bytes());
    o(416, &0x7fff_0001u32.to_le_bytes());
    o(420, &i32::MIN.to_le_bytes());
    // tail scalars (sh.cpp:1885/1888/1889)
    o(424, &0i32.to_le_bytes()); // pcfsel (untouched)
    o(428, &0u64.to_le_bytes()); // total_cycles (untouched)
    o(436, &0i32.to_le_bytes()); // cycles_this_run (untouched)
}

#[test]
fn shcore_roundtrip_quirky() {
    let mut a = Sh2Core::new(0xffff_ffff);
    a.pc = 0x8bad_f00d;
    a.sr = 0xf0f0_0f0f;
    a.r = std::array::from_fn(|i| 0x1000 * i as u32 ^ 0xdeadbeef);
    a.irqline = i32::MIN;
    a.icount = -1;
    a.sleep_mode = 2;
    a.pad_sleep = [0xde, 0xad, 0xbe];
    a.m_ftrc_dmin = f64::from_bits(0x7ff0_0000_0000_0001); // signalling-NaN bits
    a.m_fone = f32::from_bits(0x7f80_0000); // +inf
    a.m_fpmode = [0, 3, 0xff, 1];
    a.m_fpu_pr = -1;
    a.m_pending_irq = u32::MAX;
    a.m_test_irq_drc = 0xa5a5_a5a5;
    let s1 = roundtrip(a, Sh2Core::new(0xffff_ffff), |d, s| d.state(s));
    // the never-assigned DRC block rode a quirky value THROUGH the round trip
    assert_eq!(&s1[8 + 121..8 + 124], &[0xde, 0xad, 0xbe]);
}

#[test]
fn sh2_device_roundtrip_quirky() {
    let mut a = Sh2Device::new(28_000_000, 1, 0xffff_ffff);
    a.core.pc = 0x1234_5678;
    a.core.m_test_irq = 1;
    a.core.m_internal_irq_vector = -2;
    a.core.m_nmi_line_state = i8::MIN;
    a.core.m_cpu_off = 0x7fff_ffff;
    a.core.m_irq_line_state = std::array::from_fn(|i| (i as i8) - 8);
    a.core.m_total_cycles = u64::MAX - 5;
    a.core.m_cycles_this_run = i32::MIN;
    a.core.m_pcfsel = -1;
    let s1 = roundtrip(a, Sh2Device::new(28_000_000, 1, 0xffff_ffff), |d, s| d.state(s));
    // nesting per sh2.cpp:407-413: sh7042-less order = "sh2" then core dump
    assert_eq!(&s1[0..8], b"sh2\0\0\0\0\0");
    assert_eq!(&s1[8..16], b"shcore\0\0");
    assert_eq!(s1.len(), 8 + 448 + 4 + 4 + 1 + 4 + 17); // + test/vec/nmi/cpuoff/lines
}

// ---------------------------------------------------------------------------
// peripherals
// ---------------------------------------------------------------------------
#[test]
fn intc_roundtrip_quirky() {
    let mut a = Sh2Intc::new();
    a.m_pending = std::array::from_fn(|i| (0x8000_0000u32 >> i as u32) | i as u32);
    a.m_ipr = [0x8000, 0x40, 0, 0xf, 0xffff, 1, 0x7fff, 0];
    a.m_isr = 0xabcd;
    a.m_icr = 0x8080;
    a.m_lines = 0x55;
    let s = roundtrip(a, Sh2Intc::new(), |d, s| d.state(s));
    assert_eq!(&s[0..8], b"intc\0\0\0\0");
    assert_eq!(s.len(), 8 + 32 + 16 + 2 + 2 + 1);
}

#[test]
fn mtu_shared_and_channel_roundtrip_quirky() {
    let mut a = Sh2Mtu::new();
    a.m_tstr = 0x1f;
    a.m_tsyr = 0x81;
    a.m_toer = 0xfe;
    a.m_tocr = 0x02;
    a.m_tgcr = 0x8f;
    a.m_tcdr = 0x1234;
    a.m_tddr = 0xabcd;
    a.m_tcnts = 0xffff;
    a.m_tcbr = 1;
    for (i, c) in a.ch.iter_mut().enumerate() {
        c.m_tgr_clearing = i as i32 - 2;
        c.m_tcr = 0x3a;
        c.m_tmdr = 0x01;
        c.m_tior = 0xf0;
        c.m_tier = 0x61;
        c.m_tsr = 0xa2;
        c.m_clock_type = 7 - i as i32;
        c.m_clock_divider = -1000 - i as i32;
        c.m_tcnt = 0xffff - i as u16;
        c.m_tgr = [0x1111, 0x2222, 0x3333, 0x4444];
        c.m_last_clock_update = u64::MAX / (i as u64 + 3);
        c.m_event_time = 1u64 << (i as u64 + 40);
        c.m_phase = 0xffff_fff0 + i as u32;
        c.m_counter_cycle = 0x8000_0001;
        c.m_counter_incrementing = i % 2 == 0;
        c.m_channel_active = i != 3;
    }
    let mut fresh = Sh2Mtu::new();
    let f = |d: &mut Sh2Mtu, s: &mut StateIo| {
        d.state(s);
        for c in d.ch.iter_mut() {
            c.state(s);
        }
    };
    let s1 = roundtrip(a, fresh, f);
    assert_eq!(&s1[0..8], b"mtu\0\0\0\0\0");
    let shared = 8 + 5 + 8; // 5 u8 + 4 u16
    assert_eq!(&s1[shared..shared + 8], b"mtuch\0\0\0");
    let chlen = 8 + 4 + 5 + 4 + 4 + 2 + 8 + 8 + 8 + 4 + 4 + 1 + 1; // tag + fields
    assert_eq!(s1.len(), shared + 5 * chlen);
}

#[test]
fn port16_and_port32_roundtrip_quirky() {
    let mut a = ShPort16::portb();
    a.m_dr = 0xff00;
    a.m_io = 0x0402;
    let s = roundtrip(a, ShPort16::portb(), |d, s| d.state(s));
    assert_eq!(&s[0..8], b"port16\0\0");
    assert_eq!(s.len(), 8 + 2 + 2);

    let mut b = ShPort32::porta();
    b.m_dr = 0x0003_ffff;
    b.m_io = 0xffff_0000;
    let s = roundtrip(b, ShPort32::porta(), |d, s| d.state(s));
    assert_eq!(&s[0..8], b"port32\0\0");
    assert_eq!(s.len(), 8 + 4 + 4);
}

#[test]
fn sci_roundtrip_quirky() {
    let mut a = Sh2Sci::new(0, 128, 129, 130, 131);
    a.m_tx_state = 5;
    a.m_rx_state = -1;
    a.m_tx_bit = 9;
    a.m_rx_bit = -3;
    a.m_clock_state = 2;
    a.m_tx_parity = 1;
    a.m_rx_parity = 0;
    a.m_tx_clock_counter = i32::MIN;
    a.m_rx_clock_counter = i32::MAX;
    a.m_clock_mode = 0x8000_0000;
    a.m_ext_clock_value = true;
    a.m_rx_value = false;
    a.m_rdr = 0xa5;
    a.m_tdr = 0x5c;
    a.m_smr = 0x37;
    a.m_scr = 0x91;
    a.m_ssr = 0x84;
    a.m_brr = 0x0f;
    a.m_rsr = 0xf0;
    a.m_tsr = 0x01;
    a.m_clock_event = u64::MAX;
    a.m_clock_step = 0x00ff_00ff_00ff_00ff;
    a.m_divider = 1;
    let s = roundtrip(a, Sh2Sci::new(0, 128, 129, 130, 131), |d, s| d.state(s));
    assert_eq!(&s[0..8], b"sci\0\0\0\0\0");
    // 22 ints/u32s+bools+bytes: 9*4 + 4 + 2*1 + 8*1 + 3*8 = 83
    assert_eq!(s.len(), 8 + 9 * 4 + 4 + 2 + 8 + 3 * 8);
}

#[test]
fn cmt_roundtrip_quirky() {
    let mut a = ShCmt::new(144, 148);
    a.m_next_event = [u64::MAX - 1, 0x00ff_0000_0000_0001];
    a.m_str = 0x8002;
    a.m_csr = [0x0300, 0xc001];
    a.m_cnt = [0xffff, 0];
    a.m_cor = [0x8000, 0x1234];
    let s = roundtrip(a, ShCmt::new(144, 148), |d, s| d.state(s));
    assert_eq!(&s[0..8], b"cmt\0\0\0\0\0");
    assert_eq!(s.len(), 8 + 16 + 2 + 4 + 4 + 4);
}

#[test]
fn bsc_dmac_roundtrip_quirky() {
    let mut a = ShBsc::new();
    a.m_bcr1 = 0x1234;
    a.m_bcr2 = 0xffff;
    a.m_wcr1 = 0x0001;
    a.m_wcr2 = 0x800f;
    a.m_dcr = 0xabcd;
    a.m_rtcsr = 0xef00;
    a.m_rtcnt = 0x0f0f;
    a.m_rtcor = 0xf0f0;
    let s = roundtrip(a, ShBsc::new(), |d, s| d.state(s));
    assert_eq!(&s[0..8], b"bsc\0\0\0\0\0");
    assert_eq!(s.len(), 8 + 16);

    let mut d = ShDmac::new();
    d.m_dmaor = 0x8102;
    d.channels[2].m_sar = 0x0c00_0000;
    d.channels[2].m_dar = 0xffff_fff0;
    d.channels[2].m_dmatcr = 0x8000_0001; // u32 on disk (sh_dmac.h:80)
    d.channels[2].m_chcr = 0x1f;
    let mut fresh = ShDmac::new();
    let f = |x: &mut ShDmac, s: &mut StateIo| {
        x.state(s);
        for c in x.channels.iter_mut() {
            c.state(s);
        }
    };
    let s = roundtrip(d, fresh, f);
    assert_eq!(&s[0..8], b"dmac\0\0\0\0");
    assert_eq!(&s[10..18], b"dmach\0\0\0");
    assert_eq!(s.len(), 8 + 2 + 4 * (8 + 16));

    let mut ch = ShDmacChannel::new();
    ch.m_sar = i32::MIN as u32;
    let s = roundtrip(ch, ShDmacChannel::new(), |d, s| d.state(s));
    assert_eq!(&s[0..8], b"dmach\0\0\0");
    assert_eq!(s.len(), 8 + 16);
}

// ---------------------------------------------------------------------------
// sh7042 tree: exact birth order + the version>=8 adc1 leg
// ---------------------------------------------------------------------------
struct Spy {
    log: Rc<RefCell<Vec<String>>>,
}

// per-device spies: record the call, then emit the device's real tag plus a
// one-byte marker so stream length is deterministic.
macro_rules! spy_seam {
    ($method:ident, $tag:expr, $marker:expr) => {
        fn $method(&mut self, s: &mut StateIo) {
            self.log.borrow_mut().push($tag.to_string());
            s.tag($tag);
            let mut m = $marker;
            s.v(&mut m);
        }
    };
}

impl Sh7042Peripherals for Spy {
    spy_seam!(intc_state, "intc", 1u8);
    spy_seam!(adc0_state, "adc", 2u8);
    spy_seam!(adc1_state, "adc", 3u8);
    spy_seam!(bsc_state, "bsc", 4u8);
    spy_seam!(cmt_state, "cmt", 5u8);
    spy_seam!(dmac_state, "dmac", 6u8);
    fn dmac_ch_state(&mut self, ch: usize, s: &mut StateIo) {
        self.log.borrow_mut().push(format!("dmach{ch}"));
        s.tag("dmach");
        let mut m = [0u32; 4];
        m[0] = ch as u32;
        s.arr(&mut m);
    }
    spy_seam!(mtu_state, "mtu", 7u8);
    fn mtu_ch_state(&mut self, ch: usize, s: &mut StateIo) {
        self.log.borrow_mut().push(format!("mtuch{ch}"));
        s.tag("mtuch");
        let mut m = ch as u32;
        s.v(&mut m);
    }
    fn port_state(&mut self, port: usize, s: &mut StateIo) {
        const NAMES: [&str; 6] = ["porta", "portb", "portc", "portd", "porte", "portf"];
        self.log.borrow_mut().push(NAMES[port].to_string());
        let wide = port == 0 || port == 3; // a/d are the 32-bit devices
        s.tag(if wide { "port32" } else { "port16" });
        if wide {
            let mut v = port as u32;
            s.v(&mut v);
        } else {
            let mut v = port as u16;
            s.v(&mut v);
        }
    }
    fn sci_state(&mut self, sci: usize, s: &mut StateIo) {
        self.log.borrow_mut().push(format!("sci{sci}"));
        s.tag("sci");
        let mut m = sci as u8;
        s.v(&mut m);
    }
}

fn spied_cpu(die_a: bool) -> (Sh7042, Rc<RefCell<Vec<String>>>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut cpu = if die_a {
        Sh7042::new_a(28_000_000, vec![0u8; 0x400])
    } else {
        Sh7042::new(28_000_000, vec![0u8; 0x400])
    };
    cpu.bus.periph = Some(Box::new(Spy { log: Rc::clone(&log) }));
    cpu.m_event_cycles = 0x0102_0304_0506_0708;
    cpu.m_in_event = true;
    cpu.bus.m_pcf_ah = 0x1122;
    cpu.bus.m_pcf_al = 0x3344_5566;
    cpu.bus.m_pcf_b = 0x7788_99aa;
    cpu.bus.m_pcf_c = 0xbbcc;
    cpu.bus.m_pcf_dh = 0xddee_ff00;
    cpu.bus.m_pcf_dl = 0x1011;
    cpu.bus.m_pcf_e = 0x1213_1415;
    cpu.bus.m_pcf_if = 0x1617;
    (cpu, log)
}

fn save_v(cpu: &mut Sh7042, version: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut s = StateIo::writer(&mut out);
        s.set_version(version);
        cpu.state(&mut s);
        assert!(s.ok(), "save v{version}: {}", s.error());
    }
    out
}

#[test]
fn sh7042_tree_order_is_disk_birth_order_with_adc1_at_v8() {
    // die-A, version 15 (the machine default): adc1 leg PRESENT
    let (mut cpu, log) = spied_cpu(true);
    let s1 = save_v(&mut cpu, 15);
    let want = [
        "intc", "adc", "adc", "bsc", "cmt", "dmac", "dmach0", "dmach1", "dmach2", "dmach3",
        "mtu", "mtuch0", "mtuch1", "mtuch2", "mtuch3", "mtuch4", "porta", "portb", "portc",
        "portd", "porte", "portf", "sci0", "sci1",
    ];
    assert_eq!(log.borrow().as_slice(), want);
    // CPU-part nesting: sh7042 -> sh2 -> shcore, tags at the right offsets
    assert_eq!(&s1[0..8], b"sh7042\0\0");
    assert_eq!(&s1[8..16], b"sh2\0\0\0\0\0");
    assert_eq!(&s1[16..24], b"shcore\0\0");
    // length: tag + sh2(8+448+4+4+1+4+17) + u64 + bool + pcf(2+4+4+2+4+2+4+2)
    //       + spy payload (intc 9 + adc 9 + adc 9 + bsc 9 + cmt 9 + dmac 9
    //       + 4*dmach 24 + mtu 9 + 5*mtuch 12 + port widths + 2*sci 9)
    let periph: usize = 9 * 6 + 4 * 24 + 9 + 5 * 12 + (12 + 10 + 10 + 12 + 10 + 10) + 2 * 9;
    assert_eq!(s1.len(), 8 + (8 + 448 + 4 + 4 + 1 + 4 + 17) + 8 + 1 + 24 + periph);
    // pcf fields sit exactly after event_cycles+in_event
    let p = 8 + 486 + 8 + 1;
    assert_eq!(&s1[p..p + 2], &0x1122u16.to_le_bytes());
    assert_eq!(&s1[p + 2..p + 6], &0x3344_5566u32.to_le_bytes());
    assert_eq!(&s1[p + 18..p + 22], &0x1213_1415u32.to_le_bytes()); // pcf_e
    assert_eq!(&s1[p + 22..p + 24], &0x1617u16.to_le_bytes()); // pcf_if last
}

#[test]
fn sh7042_adc1_leg_absent_below_v8_and_on_non_a_die() {
    let (mut cpu, log) = spied_cpu(true);
    save_v(&mut cpu, 7);
    assert_eq!(log.borrow().iter().filter(|t| **t == "adc").count(), 1);
    let (mut cpu, log) = spied_cpu(false); // m_die_a false: no m_adc1 device
    save_v(&mut cpu, 15);
    assert_eq!(log.borrow().iter().filter(|t| **t == "adc").count(), 1);
    assert_eq!(log.borrow()[2], "bsc"); // intc, adc0, bsc — adc1 slot skipped
}

#[test]
fn sh7042_roundtrip_quirky_v15() {
    // save v15, reload into a FRESH tree: every device field + spy marker must
    // land so the re-save is byte-identical.
    let (mut cpu, _log) = spied_cpu(true);
    cpu.dev.core.pc = 0x0f0f_0f0f;
    cpu.dev.core.sleep_mode = 1;
    cpu.dev.core.pad_sleep = [1, 2, 3];
    cpu.dev.core.m_test_irq = 1;
    cpu.dev.core.m_irq_line_state[16] = -1;
    let s1 = save_v(&mut cpu, 15);
    let (mut fresh, _log2) = spied_cpu(true);
    {
        let mut s = StateIo::reader(&s1);
        s.set_version(15);
        fresh.state(&mut s);
        assert!(s.ok(), "load: {}", s.error());
    }
    assert_eq!(fresh.dev.core.pc, 0x0f0f_0f0f);
    assert_eq!(fresh.dev.core.pad_sleep, [1, 2, 3]);
    assert_eq!(fresh.dev.core.m_irq_line_state[16], -1);
    assert_eq!(fresh.m_event_cycles, cpu.m_event_cycles);
    assert_eq!(fresh.bus.m_pcf_e, cpu.bus.m_pcf_e);
    assert_eq!(save_v(&mut fresh, 15), s1);
}

#[test]
fn version_gate_is_read_from_the_stream_reader_too() {
    // a v7 READER must skip the adc1 bytes a v15 WRITER produced — the stream
    // then desyncs loudly (sticky ok()==false), matching C++ tag mismatch.
    let (mut cpu, _) = spied_cpu(true);
    let s15 = save_v(&mut cpu, 15);
    let (mut fresh, log) = spied_cpu(true);
    {
        let mut s = StateIo::reader(&s15);
        s.set_version(7);
        fresh.state(&mut s);
        assert!(!s.ok(), "v7 reader must trip over the v15 adc1 block");
    }
    assert_eq!(log.borrow().iter().filter(|t| **t == "adc").count(), 1);
}
