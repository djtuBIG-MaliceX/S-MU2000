//! M5-W3b: `swp30_device::state` (swp30.cpp:4706-4800) wire-format tests.
//! Offsets/sizes are the g++ -std=c++20 -O3 x86-64 ground truth from
//! `%TEMP%\opencode\stategt\gt.cpp` (harness bytes copied from swp30.h):
//! sizeof streaming_block=52 filter_block=88 iir1_block=28
//! envelope_block=16 lfo_block=16 mixer_slot=12 decoded=25 meg_state=14872
//! (m_swp@9600).

use smu_compat::StateIo;
use smu_swp30::fetch::StreamingBlock;
use smu_swp30::meg::MegState;
use smu_swp30::mix::MixerSlot;
use smu_swp30::voice::{EnvelopeBlock, FilterBlock, Iir1Block, LfoBlock};
use smu_swp30::Swp30;

const VER: u32 = 15; // C++ STATE_VERSION (W1)

fn save(dev: &mut Swp30, ver: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut s = StateIo::writer(&mut out);
    s.set_version(ver);
    dev.state(&mut s);
    assert!(s.ok(), "save failed: {}", s.error());
    out
}

fn load(bytes: &[u8], ver: u32, dev: &mut Swp30) {
    let mut s = StateIo::reader(bytes);
    s.set_version(ver);
    dev.state(&mut s);
    assert!(s.ok(), "load failed: {}", s.error());
}

// ---- harness sizeof goldens (row gate: element sizes == harness) ----

#[test]
fn element_sizes_match_harness() {
    assert_eq!(StreamingBlock::STATE_SIZE, 52);
    assert_eq!(FilterBlock::STATE_SIZE, 88);
    assert_eq!(Iir1Block::STATE_SIZE, 28);
    assert_eq!(EnvelopeBlock::STATE_SIZE, 16);
    assert_eq!(LfoBlock::STATE_SIZE, 16);
    assert_eq!(MixerSlot::STATE_SIZE, 12);
    assert_eq!(smu_swp30::meg::Decoded::STATE_SIZE, 25);
    assert_eq!(MegState::STATE_POD_SIZE, 14872);
}

#[test]
fn filter_field_offsets_match_harness() {
    // harness: five u16 @0..10, pad 10-11, 19 s32 consecutive from 12
    let mut f = FilterBlock::new();
    f.m_filter_1_a = 0x0102;
    f.m_level_1 = 0x0304;
    f.m_filter_2_a = 0x0506;
    f.m_level_2 = 0x0708;
    f.m_filter_b = 0x090a;
    f.m_filter_1_p1 = -1;
    f.m_filter_2_n = 0x7f7f7f7f;
    let mut b = Vec::new();
    f.state_bytes(&mut b);
    assert_eq!(b.len(), 88);
    assert_eq!(&b[0..10], &[0x02, 0x01, 0x04, 0x03, 0x06, 0x05, 0x08, 0x07, 0x0a, 0x09]);
    assert_eq!(&b[10..12], &[0, 0], "pad after m_filter_b");
    assert_eq!(&b[12..16], &(-1i32).to_le_bytes(), "m_filter_1_p1@12");
    assert_eq!(&b[84..88], &0x7f7f_7f7fu32.to_le_bytes(), "m_filter_2_n@84");
    let mut g = FilterBlock::new();
    g.state_load(&b);
    assert_eq!(g.m_filter_1_x1, 0);
    let mut b2 = Vec::new();
    g.state_bytes(&mut b2);
    assert_eq!(b, b2);
}

#[test]
fn iir1_env_lfo_offsets_match_harness() {
    // iir1: m_a@0(8B) m_b@8 m_hx@12 m_hy@20, size 28
    let mut i = Iir1Block::new();
    i.m_a[0][1] = -2;
    i.m_a[1][0] = 300;
    i.m_b[1] = -4;
    i.m_hx[0] = 0x1122_3344;
    i.m_hy[1] = -0x5566_7788;
    let mut b = Vec::new();
    i.state_bytes(&mut b);
    assert_eq!(b.len(), 28);
    assert_eq!(&b[2..4], &(-2i16).to_le_bytes());
    assert_eq!(&b[4..6], &300i16.to_le_bytes());
    assert_eq!(&b[10..12], &(-4i16).to_le_bytes());
    assert_eq!(&b[12..16], &0x1122_3344u32.to_le_bytes());
    assert_eq!(&b[24..28], &(-0x5566_7788i32).to_le_bytes());
    let mut i2 = Iir1Block::new();
    i2.state_load(&b);
    assert_eq!(i2.m_a[1][0], 300);
    assert_eq!(i2.m_hy[1], -0x5566_7788);

    // env: u16*4, s32@8, u8@12, tail pad 13-15
    let mut e = EnvelopeBlock::new();
    e.m_attack = 1;
    e.m_decay1 = 2;
    e.m_decay2 = 3;
    e.m_release_glo = 4;
    e.m_envelope_level = -7;
    e.m_envelope_mode = 3;
    let mut b = Vec::new();
    e.state_bytes(&mut b);
    assert_eq!(b.len(), 16);
    assert_eq!(&b[8..12], &(-7i32).to_le_bytes());
    assert_eq!(b[12], 3);
    assert_eq!(&b[13..16], &[0, 0, 0]);
    let mut e2 = EnvelopeBlock::new();
    e2.state_load(&b);
    assert_eq!(e2.m_envelope_mode, 3);

    // lfo: counter@0 state@4 r_ts@6 r_amp@8 type@10 step@11 amp@12
    // pitch_mode@13 depth@14 tail pad 15
    let mut l = LfoBlock::new();
    l.m_counter = 0xdead_beef;
    l.m_state = 0x1234;
    l.m_type = 3;
    l.m_pitch_mode = true;
    l.m_pitch_depth = -5;
    let mut b = Vec::new();
    l.state_bytes(&mut b);
    assert_eq!(b.len(), 16);
    assert_eq!(&b[0..4], &0xdead_beefu32.to_le_bytes());
    assert_eq!(&b[4..6], &0x1234u16.to_le_bytes(), "m_state@4");
    assert_eq!(&b[10..15], &[3, 0, 0, 1, 0xfb]);
    assert_eq!(b[15], 0);
    let mut l2 = LfoBlock::new();
    l2.state_load(&b);
    assert!(l2.m_pitch_mode);
    assert_eq!(l2.m_pitch_depth, -5);
    assert_eq!(l2.m_state, 0x1234);
}

#[test]
fn mixer_slot_offsets_match_harness() {
    let mut m = MixerSlot::ZERO;
    m.vol = [1, 2, 3];
    m.route = [0x20, 0x21, 0x22];
    let mut b = Vec::new();
    m.state_bytes(&mut b);
    assert_eq!(b.len(), 12);
    assert_eq!(&b[0..6], &[1, 0, 2, 0, 3, 0]);
    assert_eq!(&b[6..12], &[0x20, 0, 0x21, 0, 0x22, 0]);
    let mut m2 = MixerSlot::ZERO;
    m2.state_load(&b);
    assert_eq!(m2.route, [0x20, 0x21, 0x22]);
}

// ---- device stream structure ----

/// v15 stream layout offsets from the harness sizes (row-gate table):
/// header 36 + reverb(4+0x80000) + stdarrs + "meg" tag + POD 14872 + legs
fn v15_tail_offsets() -> (usize, usize) {
    let mut o = 8 + 8 + 4 + 8 + 4; // swp30 tag + mach tag/seed/cycles/count
    o += 4; // rand_seed
    o += 4 + 0x40000 * 2; // reverb n + bytes
    o += 0x40 * StreamingBlock::STATE_SIZE;
    o += 0x40 * FilterBlock::STATE_SIZE;
    o += 0x40 * Iir1Block::STATE_SIZE;
    o += 0x40 * EnvelopeBlock::STATE_SIZE;
    o += 0x40 * LfoBlock::STATE_SIZE;
    o += 0x80 * MixerSlot::STATE_SIZE;
    o += 0x10 * 4 + 0x10 * 4 + 4 * 4; // melo meli adc
    let meg_tag = o;
    o += 8;
    (meg_tag, o)
}

#[test]
fn machine_and_header_block_golden() {
    let mut dev = Swp30::new();
    let b = save(&mut dev, VER);
    assert_eq!(&b[0..8], b"swp30\0\0\0"); // :4708 tag (:4708 s.tag("swp30"))
    assert_eq!(&b[8..16], b"mach\0\0\0\0"); // :4709 chip-local machine
    assert_eq!(&b[16..20], &0x9d14_abd7u32.to_le_bytes()); // never-rand'd seed
    assert_eq!(&b[20..28], &0u64.to_le_bytes()); // cycles never set
    assert_eq!(&b[28..32], &0u32.to_le_bytes()); // zero timers (no alloc here)
    assert_eq!(&b[32..36], &0x9d14_abd7u32.to_le_bytes()); // :4710 chip seed
    assert_eq!(&b[36..40], &0x40000u32.to_le_bytes()); // :4714-4715 n
    let (_meg_tag, pod) = v15_tail_offsets();
    assert_eq!(&b[_meg_tag.._meg_tag + 8], b"meg\0\0\0\0\0"); // :4732
    // :4737-4741 — the m_swp pointer slot is ZEROS on save (harness @9600)
    assert_eq!(&b[pod + 9600..pod + 9608], &[0u8; 8]);
    // full v15 length: pod + POD + program_changed + SCALARS :4747-4751
    // (4+4+4+4+4+4+2+2+8+2 = 38) + v6(21) + v4(6) + v3(576) + v14(36)
    // + v15(49)
    let want = pod + MegState::STATE_POD_SIZE + 1 + 38 + 21 + 6 + 576 + 36 + 49;
    assert_eq!(b.len(), want, "total v15 stream length");
}

#[test]
fn meg_pod_field_offsets_match_harness() {
    let mut dev = Swp30::new();
    dev.meg.decoded[3].rop = 0xab;
    dev.meg.decoded[3].mem_table = true;
    dev.meg.program[1] = 0x0102_0304_0506_0708;
    dev.meg.konst[2] = -3;
    dev.meg.offset[3] = 0x1234;
    dev.meg.lfo[4] = 5;
    dev.meg.lfo_increment[5] = 6;
    dev.meg.lfo_counter[6] = 7;
    dev.meg.map[7] = 8;
    dev.meg.m[8] = -9;
    dev.meg.r[9] = 10;
    dev.meg.t[4] = -11;
    dev.meg.p = -12;
    dev.meg.mw_value[0] = 13;
    dev.meg.mw_reg[1] = 14;
    dev.meg.rw_value[2] = 15;
    dev.meg.rw_reg[0] = 16;
    dev.meg.index_value[1] = 17;
    dev.meg.index_active[2] = true;
    dev.meg.memw_value[2] = 18;
    dev.meg.memr_value[0] = 19;
    dev.meg.t_value[1] = -20;
    dev.meg.memw_active[0] = true;
    dev.meg.memr_active[2] = true;
    dev.meg.delay_3 = 21;
    dev.meg.delay_2 = 22;
    dev.meg.ram_read = 23;
    dev.meg.ram_write = 24;
    dev.meg.ram_index = -25;
    dev.meg.sample_counter = 26;
    dev.meg.program_address = 27;
    dev.meg.pc = 28;
    dev.meg.icount = -29;
    dev.meg.retval = 30;
    let b = save(&mut dev, VER);
    let (_, pod) = v15_tail_offsets();
    let at = |o: usize, n: usize| b[pod + o..pod + o + n].to_vec();
    assert_eq!(at(3 * 25 + 8, 1), [0xab], "decoded[3].rop");
    assert_eq!(at(3 * 25 + 24, 1), [1], "decoded[3].mem_table bool");
    assert_eq!(at(9608 + 8, 8), 0x0102_0304_0506_0708u64.to_le_bytes());
    assert_eq!(at(12680 + 4, 2), (-3i16).to_le_bytes());
    assert_eq!(at(13448 + 6, 2), 0x1234u16.to_le_bytes());
    assert_eq!(at(13704 + 8, 2), [5, 0]);
    assert_eq!(at(13752 + 20, 4), 6u32.to_le_bytes());
    assert_eq!(at(13848 + 24, 4), 7u32.to_le_bytes());
    assert_eq!(at(13944 + 14, 2), 8u16.to_le_bytes());
    assert_eq!(at(13960 + 32, 4), (-9i32).to_le_bytes());
    assert_eq!(at(14216 + 36, 4), 10i32.to_le_bytes());
    assert_eq!(at(14728 + 8, 2), (-11i16).to_le_bytes());
    assert_eq!(at(14744, 8), (-12i64).to_le_bytes());
    assert_eq!(at(14752, 4), 13i32.to_le_bytes());
    assert_eq!(at(14764 + 1, 1), [14]);
    assert_eq!(at(14768 + 8, 4), 15i32.to_le_bytes(), "pad@14767 before");
    assert_eq!(at(14780, 1), [16]);
    assert_eq!(at(14784 + 4, 4), 17i32.to_le_bytes(), "pad@14783 before");
    assert_eq!(at(14796 + 2, 1), [1], "index_active bool@14796");
    assert_eq!(at(14800 + 8, 4), 18i32.to_le_bytes(), "pad@14799 before");
    assert_eq!(at(14812, 4), 19i32.to_le_bytes());
    assert_eq!(at(14824 + 2, 2), (-20i16).to_le_bytes());
    assert_eq!(at(14828, 1), [1]);
    assert_eq!(at(14831 + 2, 1), [1]);
    assert_eq!(at(14834, 2), [0, 0], "pad@14834-35");
    assert_eq!(at(14836, 4), 21u32.to_le_bytes());
    assert_eq!(at(14840, 4), 22u32.to_le_bytes());
    assert_eq!(at(14844, 4), 23u32.to_le_bytes());
    assert_eq!(at(14848, 4), 24u32.to_le_bytes());
    assert_eq!(at(14852, 4), (-25i32).to_le_bytes());
    assert_eq!(at(14856, 4), 26u32.to_le_bytes());
    assert_eq!(at(14860, 2), 27u16.to_le_bytes());
    assert_eq!(at(14862, 2), 28u16.to_le_bytes());
    assert_eq!(at(14864, 4), (-29i32).to_le_bytes());
    assert_eq!(at(14868, 4), 30u32.to_le_bytes());
    // pads stay zero across a load (discarded) + resave
    let mut dev2 = Swp30::new();
    load(&b, VER, &mut dev2);
    assert_eq!(dev2.meg.retval, 30);
    assert_eq!(dev2.meg.index_active[2], true);
    let b2 = save(&mut dev2, VER);
    assert_eq!(b, b2, "quirky meg save->load->save byte equality");
}

// ---- multi-voice quirky cross-instance round-trip (master/slave) ----

#[test]
fn quirky_multivoice_roundtrip_master_slave() {
    // master (default seed) with quirks everywhere
    let mut m = Swp30::new();
    m.rand_seed = 0x1234_5678; // seeds differ between the instances
    for (ci, v) in m.voices.voices.iter_mut().enumerate() {
        if ci % 3 == 0 {
            v.streaming.keyon();
            v.streaming.pitch_w(0x2000 + ci as u16);
            v.streaming.start_h_w(0x1234);
            v.filter.m_filter_1_x1 = -(ci as i32) - 1;
            v.filter.m_filter_b = ci as u16;
            v.iir1.m_a[1][1] = ci as i16;
            v.envelope.m_envelope_level = 0x2000 + ci as i32;
            v.envelope.m_envelope_mode = (ci % 4) as u8;
            v.lfo.m_type = (ci % 5) as u8;
            v.lfo.m_pitch_mode = ci % 2 == 0;
            v.lfo.m_pitch_depth = -(ci as i8);
            v.pitch_offset = 0x3fff - ci as u16;
            v.peg_rate = ci as u16 * 256;
            v.peg_cur = -(ci as i32 * 64);
            v.peg_reached = (ci % 2) as u8;
        }
    }
    m.mixer.mixer[7].vol = [1, 9, 0xffff];
    m.mixer.mixer[0x7f].route = [0x20, 3, 0];
    m.mixer.melo[3] = -0x1234_5678;
    m.mixer.meli[0xf] = 0x7fff_ffff;
    m.mixer.adc[2] = -0x10000;
    m.meg.p = -1;
    m.meg.r[0x7f] = 0x3fff_ffff;
    m.meg.program[0x17f] = u64::MAX;
    m.meg.decoded[0x17f].mem_use_index2 = true;
    m.meg.pc = 0x123;
    m.meg_skip_mask = 0x5a;
    for (i, g) in m.meg_regions.iter_mut().enumerate() {
        g.quiet = i as u32 * 7;
    }
    m.meg_prg_dirty[2] = 0xdead_beef_cafe;
    m.meg_map_dirty = false;
    m.meg_flag_n = true;
    m.meg_ix2_value = [-1, 2, -3];
    m.meg_ix2_act = [0, 1, 0];
    m.meg_ram_index2 = -7;
    m.rec_pos = 0x30f;
    m.rec_ctrl = 0x1f;
    m.sample_counter = 12345;
    m.wave_adr = 0x400;
    m.wave_size = 9;
    m.wave_val = 0xa5a5;
    m.wave_access = 0x7000;
    m.keyon_mask = u64::MAX >> 8;
    m.internal_adr = 0x0405;
    m.revram_adr = 3;
    m.revram_data = 0x1_2345;
    m.reverb_ram[7] = 0xdead;
    m.reverb_ram[0x3_ffff] = 0xbeef;

    let bm = save(&mut m, VER);
    // slave: fresh device, different (default) seed
    let mut sl = Swp30::new();
    let bs_fresh = save(&mut sl, VER);
    assert_ne!(&bm[32..36], &bs_fresh[32..36], "chip seeds must differ pre-load");
    load(&bm, VER, &mut sl);
    let bs = save(&mut sl, VER);
    assert_eq!(bm, bs, "master->slave cross-load: full streams byte-identical");
    // back into master itself: still identical (load->save stability)
    load(&bs, VER, &mut m);
    let bm2 = save(&mut m, VER);
    assert_eq!(bm, bm2);
    // spot values really moved
    assert_eq!(sl.voices.voices[3].peg_cur, -3 * 64);
    assert_eq!(sl.reverb_ram[0x3_ffff], 0xbeef);
    assert_eq!(sl.meg_prg_dirty[2], 0xdead_beef_cafe);
    assert_eq!(sl.rand_seed, 0x1234_5678);
}

// ---- side effects and post-effects ----

#[test]
fn mix_dirty_stays_dirty_after_save_and_load() {
    let mut dev = Swp30::new();
    dev.mixer.mix_dirty = [0, 0]; // pretend everything is rebuilt
    let _b = save(&mut dev, VER);
    // :4727 fires UNCONDITIONALLY after stdarr(m_mixer): the save itself
    // dirties the device again (taps are derived state)
    assert_eq!(dev.mixer.mix_dirty, [!0u64, !0u64], "dirty STAYS dirty after save");
    let mut dev2 = Swp30::new();
    load(&_b, VER, &mut dev2);
    assert_eq!(dev2.mixer.mix_dirty, [!0u64, !0u64], "load also forces rebuild");
    assert_eq!(dev2.mixer.mixer[0].vol, [0; 3]); // slots rode the stream
}

#[test]
fn read_posteffects_ops_stale_and_awm_idle() {
    let mut dev = Swp30::new();
    dev.meg_ops_stale = false;
    dev.awm_idle = u64::MAX;
    dev.meg_program_changed = false;
    let b = save(&mut dev, VER);
    let mut dev2 = Swp30::new();
    dev2.meg_ops_stale = false;
    dev2.awm_idle = 0x5555;
    load(&b, VER, &mut dev2);
    assert!(dev2.meg_ops_stale, ":4744-4745 always on load");
    assert_eq!(dev2.awm_idle, 0, ":4779-4780 load clears idle");
    // write side must NOT touch them
    assert!(!dev.meg_ops_stale);
    assert_eq!(dev.awm_idle, u64::MAX);
}

// ---- version legs ----

#[test]
fn v2_stream_is_short_and_zero_inits_legs() {
    let mut dev = Swp30::new();
    dev.voices.voices[1].pitch_offset = 0x2000;
    dev.voices.voices[1].peg_cur = -8;
    dev.meg_flag_z = true;
    dev.rec_ctrl = 0x1f;
    dev.meg_skip_mask = 7;
    dev.meg_regions[0].quiet = 9;
    dev.meg_prg_dirty[0] = 3;
    let b2 = save(&mut dev, 2);
    let (_, pod) = v15_tail_offsets();
    // v2 = scalars block (:4747-4751, 38 B) then stop — no version legs
    assert_eq!(
        b2.len(),
        pod + MegState::STATE_POD_SIZE + 1 + 38,
        "v2 stops after the scalar block"
    );
    let mut dev2 = Swp30::new();
    dev2.voices.voices[1].pitch_offset = 0x1111;
    dev2.meg_flag_z = true;
    dev2.rec_pos = 5;
    dev2.meg_skip_mask = 3;
    dev2.meg_regions[0].quiet = 2;
    dev2.meg_prg_dirty[1] = 9;
    dev2.meg_map_dirty = false;
    load(&b2, 2, &mut dev2);
    // v3 else-zero: pitch/peg wiped (:4772-4777)
    assert_eq!(dev2.voices.voices[1].pitch_offset, 0);
    assert_eq!(dev2.voices.voices[1].peg_cur, 0);
    assert_eq!(dev2.voices.voices[0x3f].peg_reached, 0);
    // v4/v6/v14/v15 else-legs
    assert_eq!(dev2.rec_pos, 0);
    assert_eq!(dev2.rec_ctrl, 0);
    assert!(!dev2.meg_flag_z);
    assert_eq!(dev2.meg_ix2_value, [0; 3]);
    assert_eq!(dev2.meg_skip_mask, 0);
    assert_eq!(dev2.meg_regions[0].quiet, 0);
    assert_eq!(dev2.meg_prg_dirty, [0; 6]);
    assert!(dev2.meg_map_dirty, ":4798 zero-init sets map DIRTY=true");
    // re-save v2: byte-identical
    assert_eq!(save(&mut dev2, 2), b2);
}

#[test]
fn version_boundaries_5_and_13_and_14() {
    // streams are written AT the reader's version (a v15 image read by an
    // older loader is stream-garbage after its own layout on disk too —
    // only the matching-version legs are exercised here)
    let mut dev = Swp30::new();
    dev.meg_flag_n = true;
    dev.meg_ix2_value = [1, 2, 3];
    dev.rec_pos = 0x30f;
    dev.voices.voices[2].peg_rate = 0x0100;
    dev.meg_skip_mask = 0x81;
    dev.meg_regions[3].quiet = 11;
    dev.meg_prg_dirty[4] = 0xff;
    dev.meg_map_dirty = false;

    // version 5: no v6 bytes on either side -> flag/ix2 zero-init,
    // v4/v3 legs READ (:4761)
    let b5 = save(&mut dev, 5);
    let mut a = Swp30::new();
    a.meg_flag_n = true;
    a.meg_ix2_value = [9, 9, 9];
    load(&b5, 5, &mut a);
    assert!(!a.meg_flag_n, "v<6 zero-init leg");
    assert_eq!(a.meg_ix2_value, [0; 3]);
    assert_eq!(a.rec_pos, 0x30f, "v>=4 leg still read");
    assert_eq!(a.voices.voices[2].peg_rate, 0x0100);

    // version 13: v14+v15 zero-init
    let b13 = save(&mut dev, 13);
    let mut c = Swp30::new();
    c.meg_skip_mask = 2;
    c.meg_prg_dirty[0] = 1;
    load(&b13, 13, &mut c);
    assert_eq!(c.meg_skip_mask, 0);
    assert_eq!(c.meg_regions[3].quiet, 0);
    assert_eq!(c.meg_prg_dirty, [0; 6]);
    assert!(c.meg_map_dirty);
    assert_eq!(c.rec_pos, 0x30f);

    // version 14: skip_mask+quiet READ, v15 still zero-init
    let b14 = save(&mut dev, 14);
    let mut d = Swp30::new();
    load(&b14, 14, &mut d);
    assert_eq!(d.meg_skip_mask, 0x81);
    assert_eq!(d.meg_regions[3].quiet, 11);
    assert_eq!(d.meg_prg_dirty, [0; 6]);
    assert!(d.meg_map_dirty);

    // version 15: everything rides
    let b = save(&mut dev, VER);
    let mut e = Swp30::new();
    load(&b, 15, &mut e);
    assert_eq!(e.meg_prg_dirty[4], 0xff);
    assert_eq!(e.meg_regions[3].quiet, 11);
    assert!(!e.meg_map_dirty, "stream value (false) restored at v15");
}

// ---- reverb RAM: count-wins resize on read (:4714-4718) ----

#[test]
fn reverb_resize_on_load_count_wins() {
    let mut dev = Swp30::new();
    dev.reverb_ram[1] = 0xabcd;
    let b = save(&mut dev, VER);
    let mut dev2 = Swp30::new();
    load(&b, VER, &mut dev2);
    assert_eq!(dev2.reverb_ram.len(), 0x40000);
    assert_eq!(dev2.reverb_ram[1], 0xabcd);

    // crafted stream: n=2 -> resize(2), then exactly 4 bytes ride; the
    // stream then desyncs (tag mismatch) EXACTLY like C++ :4717-4718
    let mut raw = Vec::new();
    let mut s = StateIo::writer(&mut raw);
    s.set_version(VER);
    s.tag("swp30");
    s.tag("mach");
    s.v(&mut 0x9d14_abd7u32);
    s.v(&mut 0u64);
    s.v(&mut 0u32);
    s.v(&mut 0x9d14_abd7u32);
    s.v(&mut 2u32); // n
    raw.extend_from_slice(&0x1111u16.to_le_bytes());
    raw.extend_from_slice(&0x2222u16.to_le_bytes());
    let mut dev3 = Swp30::new();
    let mut s = StateIo::reader(&raw);
    s.set_version(VER);
    dev3.state(&mut s);
    assert_eq!(dev3.reverb_ram.len(), 2, "resize(n) wins on load (:4717)");
    assert_eq!(dev3.reverb_ram[0], 0x1111);
    assert_eq!(dev3.reverb_ram[1], 0x2222);
    assert!(!s.ok(), "truncated stream must fail (meg tag) like C++");
    // everything after the reverb block was left untouched by the failure
    assert_eq!(dev3.meg.retval, 0);
}

