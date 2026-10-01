//! Hand-derived unit tests for `meg::meg_step` / `meg_pack24` / `meg_cond`
//! (MEG phase B). Expectations are computed by hand from `swp30.cpp`
//! :3576-3894 semantics — the compiled-C++ harness pass comes next (ledger
//! NEXT §2). Bit arithmetic here was derived against the disk windows
//! re-read 2026-10-01 (two independent methods each).

use crate::meg::{self, MegState};
use crate::regs::Swp30;
use crate::voice::swp_rand;

fn program(s: &mut Swp30, pc: usize, word: u64) {
    s.meg.program[pc] = word;
    s.meg.decode_program();
}

fn sintab_zeros() -> Vec<u16> {
    vec![0; 0x8000]
}

#[test]
fn pack24_all_legs() {
    // :3580-3583 — truncation toward zero, corner snaps, sext24 fold
    assert_eq!(meg::meg_pack24(0), 0);
    assert_eq!(meg::meg_pack24(32767), 0); // trunc toward 0
    assert_eq!(meg::meg_pack24(32768), 1);
    assert_eq!(meg::meg_pack24(-32768), -1i32 as u32); // 0xffff_ffff
    assert_eq!(meg::meg_pack24(-32767), 0); // trunc toward 0 (upstream 18)
    assert_eq!(meg::meg_pack24(0x80_0000 << 15), 0x7f_ffff); // :3581
    assert_eq!(meg::meg_pack24((-0x80_0001i64) << 15), 0xff80_0000); // :3582
    // s32(q) wrap leg: q=0x80000000 -> as i32 = i32::MIN -> sext24 -> 0
    assert_eq!(meg::meg_pack24(0x8000_0000i64 << 15), 0);
}

#[test]
fn cond_truth_table() {
    // :3599-3607
    for z in [false, true] {
        for n in [false, true] {
            assert!(meg::meg_cond(0x00, n, z)); // bit3=0 -> always (3601)
            assert_eq!(meg::meg_cond(0x08, n, z), !n); // bit3, !n (3603)
            assert_eq!(meg::meg_cond(0x0C, n, z), n);
            assert_eq!(meg::meg_cond(0x0A, n, z), !n || z); // ||z (3604)
            assert_eq!(meg::meg_cond(0x0E, n, z), n || z);
        }
    }
}

#[test]
fn step_nop_advances_pipeline() {
    let mut s = Swp30::new();
    s.step(&sintab_zeros());
    assert_eq!(s.meg.pc, 1); // :3689
    assert_eq!(s.meg.icount, -1); // :3690
    assert_eq!(s.meg.delay_3, 1); // :3881-3883
    assert_eq!(s.meg.delay_2, 1); // :3885-3887
    assert_eq!(s.meg.mw_reg[0], 0); // dm=0 latch
    assert_eq!(s.meg.t_value[0], 0); // :3833 clamp(0>>23)
    assert_eq!(s.meg_skip_to, 0);
}

#[test]
fn step_mmode3_m2_shift15() {
    let mut s = Swp30::new();
    // mmode=3 (bits22-23), m2_from_m (bit18), sm=1 (bits4-9), asel=3 (bits24-25)
    program(&mut s, 0, (3 << 22) | (1 << 18) | (1 << 4) | (3 << 24));
    s.meg.m[1] = 100;
    s.step(&sintab_zeros());
    assert_eq!(s.meg.p, 100 << 15); // :3713 + :3728 (a=0)
}

#[test]
fn step_mmode1_flags_and_m1t2() {
    let mut s = Swp30::new();
    // mmode=1, m1t=0 (const), asel=3, rop=0, flag latch bit0x20
    program(&mut s, 0, (1 << 22) | (3 << 24) | (1u64 << 32));
    s.meg.konst[0] = 3;
    s.step(&sintab_zeros());
    assert_eq!(s.meg.p, 3 << 23); // :3707
    assert!(!s.meg_flag_n && !s.meg_flag_z); // :3766-3767

    // m1t=2 (bits20-21): flag_n false -> const again
    let mut s2 = Swp30::new();
    program(&mut s2, 0, (1 << 22) | (2 << 20) | (3 << 24) | (1u64 << 32));
    s2.meg.konst[0] = 5;
    s2.meg.t[0] = 9;
    s2.step(&sintab_zeros());
    assert_eq!(s2.meg.p, 5 << 23); // const leg (flag_n was false)
    assert!(s2.meg.p > 0 && !s2.meg_flag_n);
}

#[test]
fn step_rop2_abs_via_gate_mmode0() {
    let mut s = Swp30::new();
    // gate opens on rop!=0 with mmode==0: m=0, a=p, r=0+|p| (:3692,:3734)
    program(&mut s, 0, 2 << 26); // asel=0, rop=2
    s.meg.p = -1000;
    s.step(&sintab_zeros());
    assert_eq!(s.meg.p, 1000);
}

#[test]
fn step_clamp_saturates_and_skips_sext42() {
    let mut s = Swp30::new();
    // clamp=1 (bits30-31), asel=0, rop=2 (r = 0 + |p|), flag latch bit32
    program(&mut s, 0, (1 << 30) | (2 << 26) | (1u64 << 32));
    s.meg.p = 1i64 << 44; // above BOTH the 2^42 wrap and the 2^38 clamp
    s.step(&sintab_zeros());
    // saturates at 2^38-1 (not sext42-wrapped, not 2^42): :3747 skips sext
    assert_eq!(s.meg.p, 0x3f_ffff_ffff);
    assert!(!s.meg_flag_n);
}

#[test]
fn step_shift_x16() {
    let mut s = Swp30::new();
    program(&mut s, 0, 3 << 28); // shift=3 -> <<4 (:3743), asel=0, rop=0
    s.meg.p = 2;
    s.step(&sintab_zeros());
    assert_eq!(s.meg.p, 32);
}

#[test]
fn step_branch_skip_and_t_write() {
    let mut s = Swp30::new();
    // jump (bit63), cond=0 (always), target=5 (bits16-23), t=2, t_write (bit59),
    // t_from_p=0 -> const (:3664-3665)
    program(&mut s, 0, (1u64 << 63) | (5 << 16) | (2 << 56) | (1u64 << 59));
    s.meg.konst[0] = 7;
    s.step(&sintab_zeros());
    assert_eq!(s.meg_skip_to, 5); // :3660-3661 (target > pc)
    assert_eq!(s.meg.t[2], 7);
    assert_eq!(s.meg.pc, 1);
    // skipped instruction: early-return leg (:3639-3652)
    s.step(&sintab_zeros());
    assert_eq!(s.meg_skip_to, 5);
    assert_eq!(s.meg.pc, 2);
    // walk to the target
    s.step(&sintab_zeros());
    s.step(&sintab_zeros());
    s.step(&sintab_zeros()); // pc was 4 -> 5 here? counts: 1,2,3,4 steps 2..4
    assert_eq!(s.meg.pc, 5);
    // step ON the target: skip_to clears, normal path runs (:3654)
    s.step(&sintab_zeros());
    assert_eq!(s.meg_skip_to, 0);
    assert_eq!(s.meg.pc, 6);
}

#[test]
fn step_dm_src5_rand_sign_extend() {
    let mut s = Swp30::new();
    // dm=1 (bits39-44), dm_src=5 (bits45-47)
    program(&mut s, 0, (1u64 << 39) | (5u64 << 45));
    let seed0 = s.rand_seed;
    s.step(&sintab_zeros());
    let mut seed = seed0;
    let rv = swp_rand(&mut seed);
    let mut v = rv & 0xff_ffff;
    if v & 0x0080_0000 != 0 {
        v |= 0xff00_0000;
    }
    assert_eq!(s.rand_seed, seed); // LCG seam advanced once (:3779)
    assert_eq!(s.meg.mw_reg[0], 1); // :3771
    assert_eq!(s.meg.mw_value[0], v as i32); // :3789
    // reg write lands 3 steps later (:3615-3616)
    s.step(&sintab_zeros());
    s.step(&sintab_zeros());
    let want = s.meg.mw_value[0];
    s.step(&sintab_zeros()); // top-of-step delay_3 back to 0
    assert_eq!(s.meg.m[1], want);
}

#[test]
fn step_dm_src0_lfo_leg() {
    let mut s = Swp30::new();
    program(&mut s, 0, 1u64 << 39); // dm=1, dm_src=0 -> get_lfo(pc>>4)=lfo0
    let mut st = vec![0u16; 0x8000];
    st[1] = 0x555;
    s.meg.lfo_counter[0] = 0x20; // base = 0x20>>5 = 1 (:3466)
    s.step(&st);
    assert_eq!(s.meg.mw_value[0], (0x555 << 7) as i32); // :3497 <<7
}

#[test]
fn step_mem_table_read_pipeline() {
    let mut s = Swp30::new();
    // memop=2 (bits36-37), mem_table (bit35)
    program(&mut s, 0, (2u64 << 36) | (1u64 << 35));
    s.meg.offset[0] = 4; // pc/3 == 0
    s.reverb_ram[4] = 0x1234;
    s.step(&sintab_zeros());
    assert!(s.meg.memr_active[0]); // :3854
    let want = MegState::revram_decode(0x1234) as i32; // :3853
    assert_eq!(s.meg.memr_value[0], want);
    s.step(&sintab_zeros()); // delay_2 1: nothing fires
    s.step(&sintab_zeros()); // delay_2 0: ram_read loads (:3632-3634)
    assert_eq!(s.meg.ram_read, want as u32);
    assert!(!s.meg.memr_active[0]);
}

#[test]
fn step_revram_enable_blocks_write_and_zeroes_read() {
    let mut s = Swp30::new();
    s.revram_enable = 1; // region 0 disabled (map all-zero -> region_of==0)
    // memop=1 (bit36) + memw (0x3d bit61 without 0x3e)
    program(&mut s, 0, (1u64 << 36) | (1u64 << 61));
    s.meg.p = 1234 << 15;
    s.step(&sintab_zeros());
    assert!(s.meg.memw_active[0]); // latch still set (:3807)
    assert_eq!(s.reverb_ram[0], 0); // write dropped (:3840-3841)
    // read leg: disabled region -> 0 + active (:3858-3861)
    let mut s2 = Swp30::new();
    s2.revram_enable = 1;
    program(&mut s2, 0, 2u64 << 36);
    s2.step(&sintab_zeros());
    assert!(s2.meg.memr_active[0]);
    assert_eq!(s2.meg.memr_value[0], 0);
}

#[test]
fn step_index2_three_cycle_delay() {
    let mut s = Swp30::new();
    // index2 = 0x3e && 0x3d (bits62+61) (:3560)
    program(&mut s, 0, (1u64 << 62) | (1u64 << 61));
    s.meg.p = 64 << 23; // p>>23 = 64
    s.step(&sintab_zeros());
    assert_eq!(s.meg_ix2_act[0], 1); // :3817
    assert_eq!(s.meg_ix2_value[0], 64); // :3819
    assert_eq!(s.meg.t_value[0], ((64u64 << 15) & 0x7fff) as i16); // :3832 = 0
    s.step(&sintab_zeros());
    s.step(&sintab_zeros());
    assert_eq!(s.meg_ram_index2, 0); // not yet (3-cycle delay)
    s.step(&sintab_zeros()); // top of 4th: d3 back to 0 (:3624-3625)
    assert_eq!(s.meg_ram_index2, 64);
    // act[0] overwritten to 0 by THIS step's :3817 (program[3] has no index2)
    assert_eq!(s.meg_ix2_act[0], 0);
}

#[test]
fn flush_writes_drains_all_three_lines() {
    let mut s = Swp30::new();
    // leave a pending dm (reg1) + dr (reg2) + index + ix2 from pc=0 word,
    // then flush without stepping through the pipeline.
    // dm=1 src7 (m[sm]) sm=3; dr=1 from_r sr=5; index (bit62 only)
    program(
        &mut s,
        0,
        (1u64 << 39) | (7u64 << 45) | (3 << 4) | (1u64 << 48) | (1u64 << 55) | (5 << 11)
            | (1u64 << 62),
    );
    s.meg.m[3] = 77;
    s.meg.r[5] = 88;
    s.meg.p = 64 << 23; // index value = 64
    s.step(&sintab_zeros());
    assert_eq!(s.meg.mw_reg[0], 1);
    assert_eq!(s.meg.m[1], 0); // still pending
    s.meg_flush_writes(); // k starts at delay_3 == 1 -> drains slots 1,2,0
    assert_eq!(s.meg.m[1], 77); // dm reg number=1, src7 passthrough (:3787, :3911)
    assert_eq!(s.meg.r[1], 88); // dr reg number=1, from_r sr=5 (:3796, :3913)
    assert_eq!(s.meg.ram_index, 64); // index drain (:3914-3915)
    assert_eq!(s.meg.mw_reg[0], 0); // :3918-3921
    assert_eq!(s.meg_ix2_act[0], 0);
}
