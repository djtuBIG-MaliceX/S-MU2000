//! x86-64 byte emitter — transliteration of `src/compat/x64asm.h` mode-64
//! class (`SMU_X64ASM_MODE == 64`, the only live half on this box; the
//! x86-32 half at x64asm.h:215+ is DEAD there and NOT ported).
//! Ground truth: x64asm.h:37-213. Every method names its origin line.
//!
//! Hand-verified encoding vectors live in `tests` at the bottom.

#![allow(dead_code)] // full class fidelity — the JIT driver uses a subset

// origin: x64asm.h:37 enum : u8 { RAX=0, RCX, ... NOREG=0xff }
pub const RAX: u8 = 0;
pub const RCX: u8 = 1;
pub const RDX: u8 = 2;
pub const RBX: u8 = 3;
pub const RSP: u8 = 4;
pub const RBP: u8 = 5;
pub const RSI: u8 = 6;
pub const RDI: u8 = 7;
pub const R8: u8 = 8;
pub const R9: u8 = 9;
pub const R10: u8 = 10;
pub const R11: u8 = 11;
pub const R12: u8 = 12;
pub const R13: u8 = 13;
pub const R14: u8 = 14;
pub const R15: u8 = 15;
pub const NOREG: u8 = 0xff;

// origin: x64asm.h:40-42 (_WIN32 branch: ARG0..3 = rcx/rdx/r8/r9, sysv_abi=false)
pub const ARG0: u8 = RCX;
pub const ARG1: u8 = RDX;
pub const ARG2: u8 = R8;
pub const ARG3: u8 = R9;
pub const SYSV_ABI: bool = false;

// origin: x64asm.h:56-61 struct mem { u8 base; u8 index = NOREG; u8 scale = 1; s32 disp = 0; }
#[derive(Clone, Copy)]
pub struct Mem {
    pub base: u8,
    pub index: u8,
    pub scale: u8,
    pub disp: i32,
}

impl Mem {
    // origin: x64asm.h:350 mem{ RBP, NOREG, 1, off }
    #[inline(always)]
    pub fn b(base: u8, disp: i32) -> Mem {
        Mem { base, index: NOREG, scale: 1, disp }
    }
    // origin: x64asm.h:500 mem{ JIT_ROM, RDX, 1, 0 }
    #[inline(always)]
    pub fn bi(base: u8, index: u8, disp: i32) -> Mem {
        Mem { base, index, scale: 1, disp }
    }
}

// origin: x64asm.h:63 class assembler
pub struct Assembler {
    // origin: x64asm.h:66 std::vector<u8> code
    pub code: Vec<u8>,
}

impl Assembler {
    pub fn new() -> Assembler {
        Assembler { code: Vec::new() }
    }

    // origin: x64asm.h:68 void byte(u8 b)
    #[inline(always)]
    pub fn byte(&mut self, b: u8) {
        self.code.push(b);
    }
    // origin: x64asm.h:69 void d32(u32 v) — little-endian 4 bytes
    #[inline]
    pub fn d32(&mut self, v: u32) {
        for i in 0..4 {
            self.byte((v >> (8 * i)) as u8);
        }
    }
    // origin: x64asm.h:70 void d64(u64 v) — little-endian 8 bytes
    #[inline]
    pub fn d64(&mut self, v: u64) {
        for i in 0..8 {
            self.byte((v >> (8 * i)) as u8);
        }
    }

    // origin: x64asm.h:73-80 rr — reg/reg (or reg/rm-with-modrm-0xC0) form.
    // REX: 0x40 | W<<3 | R<<2 | B, emitted only if != 0x40.
    #[inline]
    pub fn rr(&mut self, prefix: u8, w: bool, opc: &[u8], reg: u8, rm: u8) {
        if prefix != 0 {
            self.byte(prefix);
        }
        let rex = 0x40 | (if w { 8 } else { 0 }) | (((reg >> 3) & 1) << 2) | ((rm >> 3) & 1);
        if rex != 0x40 {
            self.byte(rex);
        }
        for o in opc {
            self.byte(*o);
        }
        self.byte(0xc0 | ((reg & 7) << 3) | (rm & 7));
    }
    // origin: x64asm.h:81-98 rm — reg/memory form. ALWAYS disp32 (fixed length).
    // SIB whenever an index exists or base&7 == RSP; index field 4 when NOREG.
    #[inline]
    pub fn rm(&mut self, prefix: u8, w: bool, opc: &[u8], reg: u8, m: Mem) {
        if prefix != 0 {
            self.byte(prefix);
        }
        let x = if m.index == NOREG { 0 } else { (m.index >> 3) & 1 };
        let rex = 0x40 | (if w { 8 } else { 0 }) | (((reg >> 3) & 1) << 2) | (x << 1) | ((m.base >> 3) & 1);
        if rex != 0x40 {
            self.byte(rex);
        }
        for o in opc {
            self.byte(*o);
        }
        // いつも disp32 の形にする（長さが決まっていて楽）
        if m.index == NOREG && (m.base & 7) != RSP {
            self.byte(0x80 | ((reg & 7) << 3) | (m.base & 7));
        } else {
            self.byte(0x80 | ((reg & 7) << 3) | 4);
            let ss: u8 = if m.scale == 1 {
                0
            } else if m.scale == 2 {
                1
            } else if m.scale == 4 {
                2
            } else {
                3
            };
            let idx = if m.index == NOREG { 4 } else { m.index & 7 };
            self.byte((ss << 6) | (idx << 3) | (m.base & 7));
        }
        self.d32(m.disp as u32);
    }

    // ---- x64asm.h:100-160 core opcodes (1:1) ----
    // origin: x64asm.h:100 mov64: mov r64,r64 (0x8b /r W)
    #[inline]
    pub fn mov64(&mut self, d: u8, s: u8) { self.rr(0, true, &[0x8b], d, s); }
    // origin: x64asm.h:101 load64: mov r64, m64 (0x8b /r W)
    #[inline]
    pub fn load64(&mut self, d: u8, m: Mem) { self.rm(0, true, &[0x8b], d, m); }
    // origin: x64asm.h:102 store64: mov m64, r64 (0x89 /r W)
    #[inline]
    pub fn store64(&mut self, m: Mem, s: u8) { self.rm(0, true, &[0x89], s, m); }
    // origin: x64asm.h:103 load32: mov r32, m32 (0x8b /r)
    #[inline]
    pub fn load32(&mut self, d: u8, m: Mem) { self.rm(0, false, &[0x8b], d, m); }
    // origin: x64asm.h:104 store32: mov m32, r32 (0x89 /r)
    #[inline]
    pub fn store32(&mut self, m: Mem, s: u8) { self.rm(0, false, &[0x89], s, m); }
    // origin: x64asm.h:105 loads32: movsxd r64, m32 (0x63 /r W)
    #[inline]
    pub fn loads32(&mut self, d: u8, m: Mem) { self.rm(0, true, &[0x63], d, m); }
    // origin: x64asm.h:106 loads16: movsx r64, m16 (0f bf /r W)
    #[inline]
    pub fn loads16(&mut self, d: u8, m: Mem) { self.rm(0, true, &[0x0f, 0xbf], d, m); }
    // origin: x64asm.h:107 loadu16: movzx r32, m16 (0f b7 /r)
    #[inline]
    pub fn loadu16(&mut self, d: u8, m: Mem) { self.rm(0, false, &[0x0f, 0xb7], d, m); }
    // origin: x64asm.h:108 loadu8: movzx r32, m8 (0f b6 /r)
    #[inline]
    pub fn loadu8(&mut self, d: u8, m: Mem) { self.rm(0, false, &[0x0f, 0xb6], d, m); }
    // origin: x64asm.h:109 store16: 66 89 /r (mov m16, r16)
    #[inline]
    pub fn store16(&mut self, m: Mem, s: u8) { self.rm(0x66, false, &[0x89], s, m); }
    // origin: x64asm.h:110 store8i: mov byte m8, imm8 (c6 /0)
    #[inline]
    pub fn store8i(&mut self, m: Mem, v: u8) { self.rm(0, false, &[0xc6], 0, m); self.byte(v); }
    // origin: x64asm.h:111-116 imm64: mov r64, imm64 (REX.W B8+r)
    #[inline]
    pub fn imm64(&mut self, d: u8, v: u64) {
        self.byte(0x48 | ((d >> 3) & 1));
        self.byte(0xb8 | (d & 7));
        self.d64(v);
    }
    // origin: x64asm.h:117-122 imm32: mov r32, imm32 (B8+r; REX 0x41 when r>=8)
    #[inline]
    pub fn imm32(&mut self, d: u8, v: u32) {
        if d >= 8 {
            self.byte(0x41);
        }
        self.byte(0xb8 | (d & 7));
        self.d32(v);
    }
    // origin: x64asm.h:123 add64: add r64,r64 (0x01 /r W)
    #[inline]
    pub fn add64(&mut self, d: u8, s: u8) { self.rr(0, true, &[0x01], s, d); }
    // origin: x64asm.h:124 sub64: sub r64,r64 (0x29 /r W)
    #[inline]
    pub fn sub64(&mut self, d: u8, s: u8) { self.rr(0, true, &[0x29], s, d); }
    // origin: x64asm.h:125 and64: and r64,r64 (0x21 /r W)
    #[inline]
    pub fn and64(&mut self, d: u8, s: u8) { self.rr(0, true, &[0x21], s, d); }
    // origin: x64asm.h:126 cmp64: cmp a,b (0x39 /r W)
    #[inline]
    pub fn cmp64(&mut self, a: u8, b: u8) { self.rr(0, true, &[0x39], b, a); }
    // origin: x64asm.h:127 test64: test a,b (0x85 /r W)
    #[inline]
    pub fn test64(&mut self, a: u8, b: u8) { self.rr(0, true, &[0x85], b, a); }
    // origin: x64asm.h:128 test32: test a,b (0x85 /r)
    #[inline]
    pub fn test32(&mut self, a: u8, b: u8) { self.rr(0, false, &[0x85], b, a); }
    // origin: x64asm.h:129 xor32: xor r32,r32 (0x31 /r) — zero-extends r64
    #[inline]
    pub fn xor32(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x31], s, d); }
    // origin: x64asm.h:130 add32
    #[inline]
    pub fn add32(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x01], s, d); }
    // origin: x64asm.h:131 sub32
    #[inline]
    pub fn sub32(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x29], s, d); }
    // origin: x64asm.h:132 and32
    #[inline]
    pub fn and32(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x21], s, d); }
    // origin: x64asm.h:133 imul64 (0f af /r W)
    #[inline]
    pub fn imul64(&mut self, d: u8, s: u8) { self.rr(0, true, &[0x0f, 0xaf], d, s); }
    // origin: x64asm.h:134 imul32i: imul r32, r/m32, imm32 (69 /r)
    #[inline]
    pub fn imul32i(&mut self, d: u8, s: u8, v: u32) { self.rr(0, false, &[0x69], d, s); self.d32(v); }
    // origin: x64asm.h:135 imul64i
    #[inline]
    pub fn imul64i(&mut self, d: u8, s: u8, v: u32) { self.rr(0, true, &[0x69], d, s); self.d32(v); }
    // origin: x64asm.h:136 add32i (81 /0 imm32)
    #[inline]
    pub fn add32i(&mut self, d: u8, v: u32) { self.rr(0, false, &[0x81], 0, d); self.d32(v); }
    // origin: x64asm.h:137 and32i (81 /4 imm32)
    #[inline]
    pub fn and32i(&mut self, d: u8, v: u32) { self.rr(0, false, &[0x81], 4, d); self.d32(v); }
    // origin: x64asm.h:138 shl64 (c1 /4 ib W)
    #[inline]
    pub fn shl64(&mut self, d: u8, n: u8) { self.rr(0, true, &[0xc1], 4, d); self.byte(n); }
    // origin: x64asm.h:139 sar64 (c1 /7 ib W)
    #[inline]
    pub fn sar64(&mut self, d: u8, n: u8) { self.rr(0, true, &[0xc1], 7, d); self.byte(n); }
    // origin: x64asm.h:140 shl32 (c1 /4 ib)
    #[inline]
    pub fn shl32(&mut self, d: u8, n: u8) { self.rr(0, false, &[0xc1], 4, d); self.byte(n); }
    // origin: x64asm.h:141 sar32 (c1 /7 ib)
    #[inline]
    pub fn sar32(&mut self, d: u8, n: u8) { self.rr(0, false, &[0xc1], 7, d); self.byte(n); }
    // origin: x64asm.h:142 rol32 (c1 /0 ib)
    #[inline]
    pub fn rol32(&mut self, d: u8, n: u8) { self.rr(0, false, &[0xc1], 0, d); self.byte(n); }
    // origin: x64asm.h:143 neg64 (f7 /3 W)
    #[inline]
    pub fn neg64(&mut self, d: u8) { self.rr(0, true, &[0xf7], 3, d); }
    // origin: x64asm.h:144 cmovl64 (0f 4c /r W)
    #[inline]
    pub fn cmovl64(&mut self, d: u8, s: u8) { self.rr(0, true, &[0x0f, 0x4c], d, s); }
    // origin: x64asm.h:145 cmovg64 (0f 4f /r W)
    #[inline]
    pub fn cmovg64(&mut self, d: u8, s: u8) { self.rr(0, true, &[0x0f, 0x4f], d, s); }
    // origin: x64asm.h:146 cmovs64 (0f 48 /r W)
    #[inline]
    pub fn cmovs64(&mut self, d: u8, s: u8) { self.rr(0, true, &[0x0f, 0x48], d, s); }
    // origin: x64asm.h:147 cmove64 (0f 44 /r W)
    #[inline]
    pub fn cmove64(&mut self, d: u8, s: u8) { self.rr(0, true, &[0x0f, 0x44], d, s); }
    // origin: x64asm.h:148 cmp64ri: cmp r64, imm32 sign-extended (81 /7 W)
    #[inline]
    pub fn cmp64ri(&mut self, d: u8, v: u32) { self.rr(0, true, &[0x81], 7, d); self.d32(v); }
    // origin: x64asm.h:149 setl_mem (0f 9c /0 m8)
    #[inline]
    pub fn setl_mem(&mut self, m: Mem) { self.rm(0, false, &[0x0f, 0x9c], 0, m); }
    // origin: x64asm.h:150 sete_mem (0f 94 /0 m8)
    #[inline]
    pub fn sete_mem(&mut self, m: Mem) { self.rm(0, false, &[0x0f, 0x94], 0, m); }
    // origin: x64asm.h:151 call_reg (ff /2)
    #[inline]
    pub fn call_reg(&mut self, r: u8) { self.rr(0, false, &[0xff], 2, r); }
    // origin: x64asm.h:152 push (50+r; REX 0x41 when r>=8)
    #[inline]
    pub fn push(&mut self, r: u8) {
        if r >= 8 {
            self.byte(0x41);
        }
        self.byte(0x50 | (r & 7));
    }
    // origin: x64asm.h:153 pop (58+r; REX 0x41 when r>=8)
    #[inline]
    pub fn pop(&mut self, r: u8) {
        if r >= 8 {
            self.byte(0x41);
        }
        self.byte(0x58 | (r & 7));
    }
    // origin: x64asm.h:154 subrsp (81 /5 RSP imm32)
    #[inline]
    pub fn subrsp(&mut self, v: u32) { self.rr(0, true, &[0x81], 5, RSP); self.d32(v); }
    // origin: x64asm.h:155 addrsp (81 /0 RSP imm32)
    #[inline]
    pub fn addrsp(&mut self, v: u32) { self.rr(0, true, &[0x81], 0, RSP); self.d32(v); }
    // origin: x64asm.h:156 ret (c3)
    #[inline]
    pub fn ret(&mut self) { self.byte(0xc3); }
    // origin: x64asm.h:158 jz_fwd: jz rel32 (0f 84), returns patch position
    #[inline]
    pub fn jz_fwd(&mut self) -> usize {
        self.byte(0x0f);
        self.byte(0x84);
        self.d32(0);
        self.code.len()
    }
    // origin: x64asm.h:159 patch: fill the rel32 of the forward jump whose
    // position `at` returned (target == current end)
    #[inline]
    pub fn patch(&mut self, at: usize) {
        let rel = (self.code.len() - at) as u32;
        let b = rel.to_le_bytes();
        self.code[at - 4..at].copy_from_slice(&b);
    }
    // origin: x64asm.h:160 call_abs: movabs RAX, fn; call RAX
    #[inline]
    pub fn call_abs(&mut self, fn_: u64) {
        self.imm64(RAX, fn_);
        self.call_reg(RAX);
    }

    // ---- origin: x64asm.h:162-213 「SH2 の JIT で足したもの」 ----
    // origin: x64asm.h:163 store32i: mov dword [m], imm32 (c7 /0)
    #[inline]
    pub fn store32i(&mut self, m: Mem, v: u32) { self.rm(0, false, &[0xc7], 0, m); self.d32(v); }
    // origin: x64asm.h:164 cmp32i_mem: cmp dword [m], imm32 (81 /7)
    #[inline]
    pub fn cmp32i_mem(&mut self, m: Mem, v: u32) { self.rm(0, false, &[0x81], 7, m); self.d32(v); }
    // origin: x64asm.h:165 sub32i_mem: sub dword [m], imm32 (81 /5)
    #[inline]
    pub fn sub32i_mem(&mut self, m: Mem, v: u32) { self.rm(0, false, &[0x81], 5, m); self.d32(v); }
    // MEG JIT addition (B2b-2a; NO x64asm.h analogue — store16 there is the
    // reg form x64asm.h:109 only). Width adaptation for the Rust u16
    // meg_skip_to (C++ m_meg_jit_skip is u32, swp30.h:614, so swp30_jit.cpp
    // :840 uses store32i): mov word [m], imm16 (66 C7 /0). The 0x66 prefix
    // flips the imm to 16 bits too — rm() stays disp32 (fixed length).
    // Consumer: meg_jit::emit_skip_reset (tests/meg_jit.rs byte-pinned).
    #[inline]
    pub fn store16i(&mut self, m: Mem, v: u16) {
        self.rm(0x66, false, &[0xc7], 0, m);
        self.byte(v as u8);
        self.byte((v >> 8) as u8);
    }
    // MEG JIT addition (B2b-2a): the width-adapted twin of cmp32i_mem
    // (x64asm.h:164) for the u16 skip leg — cmp word [m], imm16 (66 81 /7).
    // Arrives live with the B2b-3 branch gate (swp30_jit.cpp:1082 equivalent);
    // emitted+pinned now.
    #[inline]
    pub fn cmp16i_mem(&mut self, m: Mem, v: u16) {
        self.rm(0x66, false, &[0x81], 7, m);
        self.byte(v as u8);
        self.byte((v >> 8) as u8);
    }
    // origin: x64asm.h:167 jcc_fwd: jcc rel32 (cc = 0x84 je / 0x85 jne / 0x8e jle …)
    #[inline]
    pub fn jcc_fwd(&mut self, cc: u8) -> usize {
        self.byte(0x0f);
        self.byte(cc);
        self.d32(0);
        self.code.len()
    }
    // origin: x64asm.h:168 jmp_fwd: jmp rel32 (e9)
    #[inline]
    pub fn jmp_fwd(&mut self) -> usize {
        self.byte(0xe9);
        self.d32(0);
        self.code.len()
    }
    // TEMP M9 debug helper (not in x64asm.h): int3
    #[inline]
    pub fn int3(&mut self) {
        self.byte(0xcc);
    }
    // TEMP M9 debug helper (not in x64asm.h): pushf
    #[inline]
    pub fn pushf(&mut self) {
        self.byte(0x9c);
    }
    // origin: x64asm.h:170 patch_to: point a forward jump at an absolute offset
    #[inline]
    pub fn patch_to(&mut self, at: usize, target: usize) {
        let rel = (target - at) as u32;
        let b = rel.to_le_bytes();
        self.code[at - 4..at].copy_from_slice(&b);
    }
    // origin: x64asm.h:171 mov32 (8b /r)
    #[inline]
    pub fn mov32(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x8b], d, s); }
    // origin: x64asm.h:173 add32rm: add r32, [m] (03 /r)
    #[inline]
    pub fn add32rm(&mut self, d: u8, m: Mem) { self.rm(0, false, &[0x03], d, m); }
    // origin: x64asm.h:174 cmp32rm: cmp r32, [m] (3b /r)
    #[inline]
    pub fn cmp32rm(&mut self, d: u8, m: Mem) { self.rm(0, false, &[0x3b], d, m); }
    // origin: x64asm.h:175 test32rm: test r32, [m] (85 /r)
    #[inline]
    pub fn test32rm(&mut self, d: u8, m: Mem) { self.rm(0, false, &[0x85], d, m); }
    // origin: x64asm.h:176 add32mr: add [m], r (01 /r)
    #[inline]
    pub fn add32mr(&mut self, m: Mem, s: u8) { self.rm(0, false, &[0x01], s, m); }
    // origin: x64asm.h:177 sub32mr: sub [m], r (29 /r)
    #[inline]
    pub fn sub32mr(&mut self, m: Mem, s: u8) { self.rm(0, false, &[0x29], s, m); }
    // origin: x64asm.h:178 and32mr (21 /r)
    #[inline]
    pub fn and32mr(&mut self, m: Mem, s: u8) { self.rm(0, false, &[0x21], s, m); }
    // origin: x64asm.h:179 or32mr (09 /r)
    #[inline]
    pub fn or32mr(&mut self, m: Mem, s: u8) { self.rm(0, false, &[0x09], s, m); }
    // origin: x64asm.h:180 xor32mr (31 /r)
    #[inline]
    pub fn xor32mr(&mut self, m: Mem, s: u8) { self.rm(0, false, &[0x31], s, m); }
    // origin: x64asm.h:181 add32i_mem (81 /0 imm32)
    #[inline]
    pub fn add32i_mem(&mut self, m: Mem, v: u32) { self.rm(0, false, &[0x81], 0, m); self.d32(v); }
    // origin: x64asm.h:182 or32i_mem (81 /1 imm32)
    #[inline]
    pub fn or32i_mem(&mut self, m: Mem, v: u32) { self.rm(0, false, &[0x81], 1, m); self.d32(v); }
    // origin: x64asm.h:183 and32i_mem (81 /4 imm32)
    #[inline]
    pub fn and32i_mem(&mut self, m: Mem, v: u32) { self.rm(0, false, &[0x81], 4, m); self.d32(v); }
    // origin: x64asm.h:184 xor32i_mem (81 /6 imm32)
    #[inline]
    pub fn xor32i_mem(&mut self, m: Mem, v: u32) { self.rm(0, false, &[0x81], 6, m); self.d32(v); }
    // origin: x64asm.h:185 test32i_mem: test dword [m], imm32 (f7 /0)
    #[inline]
    pub fn test32i_mem(&mut self, m: Mem, v: u32) { self.rm(0, false, &[0xf7], 0, m); self.d32(v); }
    // origin: x64asm.h:186 shl32i_mem (c1 /4 ib)
    #[inline]
    pub fn shl32i_mem(&mut self, m: Mem, n: u8) { self.rm(0, false, &[0xc1], 4, m); self.byte(n); }
    // origin: x64asm.h:187 shr32i_mem (c1 /5 ib)
    #[inline]
    pub fn shr32i_mem(&mut self, m: Mem, n: u8) { self.rm(0, false, &[0xc1], 5, m); self.byte(n); }
    // origin: x64asm.h:188 loads8_32: movsx r32, byte [m] (0f be /r)
    #[inline]
    pub fn loads8_32(&mut self, d: u8, m: Mem) { self.rm(0, false, &[0x0f, 0xbe], d, m); }
    // origin: x64asm.h:189 loads16_32: movsx r32, word [m] (0f bf /r)
    #[inline]
    pub fn loads16_32(&mut self, d: u8, m: Mem) { self.rm(0, false, &[0x0f, 0xbf], d, m); }
    // origin: x64asm.h:190 store8: mov [m], r8 (88 /r) — reg must be AL/CL/DL/BL
    // or R8B+ (REX from rm() whenever reg>=8 keeps the low-byte aliasing right)
    #[inline]
    pub fn store8(&mut self, m: Mem, s: u8) { self.rm(0, false, &[0x88], s, m); }
    // origin: x64asm.h:191 movzx8 (0f b6 /r)
    #[inline]
    pub fn movzx8(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x0f, 0xb6], d, s); }
    // origin: x64asm.h:192 movzx16 (0f b7 /r)
    #[inline]
    pub fn movzx16(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x0f, 0xb7], d, s); }
    // origin: x64asm.h:193 movsx8 (0f be /r)
    #[inline]
    pub fn movsx8(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x0f, 0xbe], d, s); }
    // origin: x64asm.h:194 movsx16 (0f bf /r)
    #[inline]
    pub fn movsx16(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x0f, 0xbf], d, s); }
    // origin: x64asm.h:195 imul32 (0f af /r)
    #[inline]
    pub fn imul32(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x0f, 0xaf], d, s); }
    // origin: x64asm.h:196 add32ri (81 /0 imm32)
    #[inline]
    pub fn add32ri(&mut self, d: u8, v: u32) { self.rr(0, false, &[0x81], 0, d); self.d32(v); }
    // origin: x64asm.h:197 sub32ri (81 /5 imm32)
    #[inline]
    pub fn sub32ri(&mut self, d: u8, v: u32) { self.rr(0, false, &[0x81], 5, d); self.d32(v); }
    // origin: x64asm.h:198 cmp32ri (81 /7 imm32)
    #[inline]
    pub fn cmp32ri(&mut self, d: u8, v: u32) { self.rr(0, false, &[0x81], 7, d); self.d32(v); }
    // origin: x64asm.h:199 test32ri (f7 /0 imm32)
    #[inline]
    pub fn test32ri(&mut self, d: u8, v: u32) { self.rr(0, false, &[0xf7], 0, d); self.d32(v); }
    // origin: x64asm.h:200 shr32 (c1 /5 ib)
    #[inline]
    pub fn shr32(&mut self, d: u8, n: u8) { self.rr(0, false, &[0xc1], 5, d); self.byte(n); }
    // origin: x64asm.h:201 neg32 (f7 /3)
    #[inline]
    pub fn neg32(&mut self, d: u8) { self.rr(0, false, &[0xf7], 3, d); }
    // origin: x64asm.h:202 not32 (f7 /2)
    #[inline]
    pub fn not32(&mut self, d: u8) { self.rr(0, false, &[0xf7], 2, d); }
    // origin: x64asm.h:203 bswap32 (0f c8+r; REX 0x41 when r>=8)
    #[inline]
    pub fn bswap32(&mut self, r: u8) {
        if r >= 8 {
            self.byte(0x41);
        }
        self.byte(0x0f);
        self.byte(0xc8 | (r & 7));
    }
    // origin: x64asm.h:205 setcc (0f cc /0 — sets AL)
    #[inline]
    pub fn setcc(&mut self, cc: u8, r: u8) { self.rr(0, false, &[0x0f, cc], 0, r); }
    // origin: x64asm.h:206 lea32 (8d /r)
    #[inline]
    pub fn lea32(&mut self, d: u8, m: Mem) { self.rm(0, false, &[0x8d], d, m); }
    // origin: x64asm.h:207 or32 (09 /r)
    #[inline]
    pub fn or32(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x09], s, d); }
    // origin: x64asm.h:208 or32ri (81 /1 imm32)
    #[inline]
    pub fn or32ri(&mut self, d: u8, v: u32) { self.rr(0, false, &[0x81], 1, d); self.d32(v); }
    // origin: x64asm.h:209 xor32ri (81 /6 imm32)
    #[inline]
    pub fn xor32ri(&mut self, d: u8, v: u32) { self.rr(0, false, &[0x81], 6, d); self.d32(v); }
    // origin: x64asm.h:210 shl32cl (d3 /4)
    #[inline]
    pub fn shl32cl(&mut self, d: u8) { self.rr(0, false, &[0xd3], 4, d); }
    // origin: x64asm.h:211 shr32cl (d3 /5)
    #[inline]
    pub fn shr32cl(&mut self, d: u8) { self.rr(0, false, &[0xd3], 5, d); }
    // origin: x64asm.h:212 bsr32 (0f bd /r — s must be nonzero)
    #[inline]
    pub fn bsr32(&mut self, d: u8, s: u8) { self.rr(0, false, &[0x0f, 0xbd], d, s); }
}

// Hand-verified byte vectors (Intel SDM encodings derived in the comments).
#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(a: &Assembler) -> Vec<u8> {
        a.code.clone()
    }

    #[test]
    fn push_rbx_no_rex() {
        // push rbx: RBX=3 < 8 ⇒ no REX. 50|3 = 0x53. (x64asm.h:152)
        let mut a = Assembler::new();
        a.push(RBX);
        assert_eq!(bytes(&a), [0x53]);
    }

    #[test]
    fn push_r12_rex41() {
        // push r12: R12=12 ≥ 8 ⇒ REX 0x41 prefix, 50|(12&7)=0x54. (x64asm.h:152)
        let mut a = Assembler::new();
        a.push(R12);
        a.pop(R12); // 58|(12&7)=0x5C, same REX rule. (x64asm.h:153)
        assert_eq!(bytes(&a), [0x41, 0x54, 0x41, 0x5C]);
    }

    #[test]
    fn sub_rsp_40() {
        // sub rsp,40: rr(0,W,{81},5,RSP): REX=0x40|8=0x48; op 81;
        // ModRM=C0|(5<<3)|4 = C0|(5<<3)=28|4 = EC; imm32 40 LE. (x64asm.h:154/73)
        let mut a = Assembler::new();
        a.subrsp(40);
        assert_eq!(bytes(&a), [0x48, 0x81, 0xec, 0x28, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn store32_rbp_disp32() {
        // mov [rbp+0x4c], eax: rm(0,0,{89},RAX,RBP): REX=0x40 suppressed
        // (reg 0, base 5). op 89. base&7=5 ≠ RSP ⇒ [80|(0<<3)|5]=0x85;
        // disp32 4c 00 00 00. (x64asm.h:104/81)
        let mut a = Assembler::new();
        a.store32(Mem::b(RBP, 0x4c), RAX);
        assert_eq!(bytes(&a), [0x89, 0x85, 0x4c, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn movsxd_icount() {
        // movsxd rcx,[rbp+0x74]: rm(0,W,{63},RCX,RBP): REX=0x48; op 63;
        // ModRM=80|(1<<3)|5=0x8D; disp32. (x64asm.h:105/81)
        let mut a = Assembler::new();
        a.loads32(RCX, Mem::b(RBP, 0x74));
        assert_eq!(bytes(&a), [0x48, 0x63, 0x8d, 0x74, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn cmp32i_mem_r12_sib() {
        // cmp dword [r12+0], imm32 0x3fff0000: rm(0,0,{81},reg=7,R12):
        // REX=0x40|B(12>>3)=0x41; op 81; base&7=4==RSP ⇒ SIB form:
        // ModRM=80|(7<<3)|4=0xBC; SIB=(0<<6)|(4<<3)|4=0x24; disp32 0;
        // imm32 LE = 00 00 ff 3f. (x64asm.h:164/81)
        let mut a = Assembler::new();
        a.cmp32i_mem(Mem::b(R12, 0), 0x3fff_0000);
        assert_eq!(
            bytes(&a),
            [0x41, 0x81, 0xbc, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x3f]
        );
    }

    #[test]
    fn store8_r8_force_rex() {
        // mov [r13+rax], r8b (mwrite sz=1 with JIT_VAL=R8):
        // rm(0,0,{88},R8,{base R13,index RAX,scale 1}): X=0, R=(8>>3)=1,
        // B=1 ⇒ REX=0x45 (R8B needs REX so it does NOT alias SPL);
        // op 88; index present ⇒ ModRM=80|((8&7)<<3)|4 = 0x84;
        // SIB=(ss 0 scale1)|(idx rax&7=0)<<3|(base 5)=0x05; disp32 0.
        // (x64asm.h:190/81)
        let mut a = Assembler::new();
        a.store8(Mem::bi(R13, RAX, 0), R8);
        assert_eq!(bytes(&a), [0x45, 0x88, 0x84, 0x05, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn jcc_patch_roundtrip() {
        // jne rel32 then patch(): 0F 85 <rel32>; two filler 0x90s then
        // patch(at): rel = len-at = 8-6 = 2 ⇒ 02 00 00 00. (x64asm.h:167/159)
        let mut a = Assembler::new();
        let at = a.jcc_fwd(0x85);
        assert_eq!(at, 6);
        a.byte(0x90);
        a.byte(0x90);
        a.patch(at);
        assert_eq!(bytes(&a), [0x0f, 0x85, 0x02, 0x00, 0x00, 0x00, 0x90, 0x90]);
    }

    #[test]
    fn jmp_patch_to() {
        // jmp rel32 then patch_to(at, target=7): E9 rel32; target-at=7-5=2.
        // (x64asm.h:168/170)
        let mut a = Assembler::new();
        let at = a.jmp_fwd();
        assert_eq!(at, 5);
        a.byte(0x90);
        a.byte(0x90);
        a.patch_to(at, 7);
        assert_eq!(bytes(&a), [0xe9, 0x02, 0x00, 0x00, 0x00, 0x90, 0x90]);
    }

    #[test]
    fn call_abs_form() {
        // call_abs(N): movabs rax, N = 48 B8 <8B LE>; call rax = FF D0.
        // (x64asm.h:160/111/151)
        let mut a = Assembler::new();
        a.call_abs(0x1122_3344_5566_7788);
        assert_eq!(
            bytes(&a),
            [0x48, 0xb8, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0xff, 0xd0]
        );
    }

    #[test]
    fn imm64_r15() {
        // movabs r15, v: REX=0x48|B=0x49; B8|(15&7)=0xBF. (x64asm.h:111)
        let mut a = Assembler::new();
        a.imm64(R15, 1);
        assert_eq!(
            bytes(&a),
            [0x49, 0xbf, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn jmp_indirect_rax() {
        // the enter tail jmp rax: rr(0,0,{ff},4,RAX): REX suppressed
        // (reg 4, rm 0); FF /4 = FF E0. (x64asm.h:73, sh2_jit.cpp:376)
        let mut a = Assembler::new();
        a.rr(0, false, &[0xff], 4, RAX);
        assert_eq!(bytes(&a), [0xff, 0xe0]);
    }
}
