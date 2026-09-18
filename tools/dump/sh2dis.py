#!/usr/bin/env python3
# license:BSD-3-Clause
"""MU2000 の firmware を読むための SH-2 逆アセンブラ。

`tools/dump/sh2asm.py`（アセンブラ）の対。firmware が音色の記録から SWP30 の
レジスタをどう組み立てているかを追うために書いた（doc/native-engine.md の段 1）。

使い方:
    python tools/dump/sh2dis.py <rom> <開始番地> [<長さ>] [--base 0]

* 番地は 16 進（0x 付きでも可）。ROM は先頭が番地 0 のべた並び（MU2000 の
  プログラム ROM は 0x000000 から入っているので --base は既定の 0 でよい）
* `mov.l @(disp,PC)` は読み先の値も出す（リテラルプール）
* 分岐先は `-> 番地` で出す
"""
import sys

R = ['R%d' % i for i in range(16)]


def s8(v):
    return v - 256 if v & 0x80 else v


def s12(v):
    return v - 4096 if v & 0x800 else v


def dis(op, pc, read32=None, read16=None):
    """1 命令を文字列にする。pc はその命令の番地"""
    n = (op >> 8) & 15
    m = (op >> 4) & 15
    d4 = op & 15
    d8 = op & 0xff
    i8 = op & 0xff

    def pcrel32(d):
        a = ((pc + 4) & ~3) + d * 4
        v = read32(a) if read32 else None
        return a, v

    t = op >> 12
    if op == 0x0009:
        return 'NOP'
    if op == 0x000b:
        return 'RTS'
    if op == 0x0008:
        return 'CLRT'
    if op == 0x0018:
        return 'SETT'
    if op == 0x0019:
        return 'DIV0U'
    if op == 0x001b:
        return 'SLEEP'
    if op == 0x0028:
        return 'CLRMAC'
    if op == 0x002b:
        return 'RTE'

    if t == 0:
        if (op & 0xf) == 0x4:
            return 'MOV.B   %s,@(R0,%s)' % (R[m], R[n])
        if (op & 0xf) == 0x5:
            return 'MOV.W   %s,@(R0,%s)' % (R[m], R[n])
        if (op & 0xf) == 0x6:
            return 'MOV.L   %s,@(R0,%s)' % (R[m], R[n])
        if (op & 0xf) == 0x7:
            return 'MUL.L   %s,%s' % (R[m], R[n])
        if (op & 0xf) == 0xc:
            return 'MOV.B   @(R0,%s),%s' % (R[m], R[n])
        if (op & 0xf) == 0xd:
            return 'MOV.W   @(R0,%s),%s' % (R[m], R[n])
        if (op & 0xf) == 0xe:
            return 'MOV.L   @(R0,%s),%s' % (R[m], R[n])
        if (op & 0xf) == 0xf:
            return 'MAC.L   @%s+,@%s+' % (R[m], R[n])
        if (op & 0xff) == 0x02:
            return 'STC     SR,%s' % R[n]
        if (op & 0xff) == 0x12:
            return 'STC     GBR,%s' % R[n]
        if (op & 0xff) == 0x22:
            return 'STC     VBR,%s' % R[n]
        if (op & 0xff) == 0x03:
            return 'BSRF    %s' % R[n]
        if (op & 0xff) == 0x23:
            return 'BRAF    %s' % R[n]
        if (op & 0xff) == 0x0a:
            return 'STS     MACH,%s' % R[n]
        if (op & 0xff) == 0x1a:
            return 'STS     MACL,%s' % R[n]
        if (op & 0xff) == 0x2a:
            return 'STS     PR,%s' % R[n]
        return '.word   %04X' % op

    if t == 1:
        return 'MOV.L   %s,@(%d,%s)' % (R[m], d4 * 4, R[n])
    if t == 2:
        kind = ['MOV.B   %s,@%s', 'MOV.W   %s,@%s', 'MOV.L   %s,@%s',
                None, 'MOV.B   %s,@-%s', 'MOV.W   %s,@-%s', 'MOV.L   %s,@-%s',
                'DIV0S   %s,%s', 'TST     %s,%s', 'AND     %s,%s', 'XOR     %s,%s',
                'OR      %s,%s', 'CMP/STR %s,%s', 'XTRCT   %s,%s', 'MULU.W  %s,%s',
                'MULS.W  %s,%s'][d4]
        return kind % (R[m], R[n]) if kind else '.word   %04X' % op
    if t == 3:
        kind = ['CMP/EQ  %s,%s', None, 'CMP/HS  %s,%s', 'CMP/GE  %s,%s',
                'DIV1    %s,%s', 'DMULU.L %s,%s', 'CMP/HI  %s,%s', 'CMP/GT  %s,%s',
                'SUB     %s,%s', None, 'SUBC    %s,%s', 'SUBV    %s,%s',
                'ADD     %s,%s', 'DMULS.L %s,%s', 'ADDC    %s,%s', 'ADDV    %s,%s'][d4]
        return kind % (R[m], R[n]) if kind else '.word   %04X' % op
    if t == 4:
        lo = op & 0xff
        one = {0x00: 'SHLL', 0x01: 'SHLR', 0x04: 'ROTL', 0x05: 'ROTR',
               0x08: 'SHLL2', 0x09: 'SHLR2', 0x10: 'DT', 0x11: 'CMP/PZ',
               0x15: 'CMP/PL', 0x18: 'SHLL8', 0x19: 'SHLR8', 0x20: 'SHAL',
               0x21: 'SHAR', 0x24: 'ROTCL', 0x25: 'ROTCR', 0x28: 'SHLL16',
               0x29: 'SHLR16'}.get(lo)
        if one:
            return '%-7s %s' % (one, R[n])
        if lo == 0x0b:
            return 'JSR     @%s' % R[n]
        if lo == 0x2b:
            return 'JMP     @%s' % R[n]
        if lo == 0x0e:
            return 'LDC     %s,SR' % R[n]
        if lo == 0x1e:
            return 'LDC     %s,GBR' % R[n]
        if lo == 0x2e:
            return 'LDC     %s,VBR' % R[n]
        if lo == 0x0a:
            return 'LDS     %s,MACH' % R[n]
        if lo == 0x1a:
            return 'LDS     %s,MACL' % R[n]
        if lo == 0x2a:
            return 'LDS     %s,PR' % R[n]
        if lo == 0x06:
            return 'LDS.L   @%s+,MACH' % R[n]
        if lo == 0x16:
            return 'LDS.L   @%s+,MACL' % R[n]
        if lo == 0x26:
            return 'LDS.L   @%s+,PR' % R[n]
        if lo == 0x02:
            return 'STS.L   MACH,@-%s' % R[n]
        if lo == 0x12:
            return 'STS.L   MACL,@-%s' % R[n]
        if lo == 0x22:
            return 'STS.L   PR,@-%s' % R[n]
        if lo == 0x03:
            return 'STC.L   SR,@-%s' % R[n]
        if lo == 0x07:
            return 'LDC.L   @%s+,SR' % R[n]
        if (op & 0xf) == 0xf:
            return 'MAC.W   @%s+,@%s+' % (R[m], R[n])
        if (op & 0xf) == 0xc:
            return 'SHAD    %s,%s' % (R[m], R[n])
        if (op & 0xf) == 0xd:
            return 'SHLD    %s,%s' % (R[m], R[n])
        return '.word   %04X' % op
    if t == 5:
        return 'MOV.L   @(%d,%s),%s' % (d4 * 4, R[m], R[n])
    if t == 6:
        kind = ['MOV.B   @%s,%s', 'MOV.W   @%s,%s', 'MOV.L   @%s,%s', 'MOV     %s,%s',
                'MOV.B   @%s+,%s', 'MOV.W   @%s+,%s', 'MOV.L   @%s+,%s', 'NOT     %s,%s',
                'SWAP.B  %s,%s', 'SWAP.W  %s,%s', 'NEGC    %s,%s', 'NEG     %s,%s',
                'EXTU.B  %s,%s', 'EXTU.W  %s,%s', 'EXTS.B  %s,%s', 'EXTS.W  %s,%s'][d4]
        return kind % (R[m], R[n])
    if t == 7:
        return 'ADD     #%d,%s' % (s8(i8), R[n])
    if t == 8:
        sub = (op >> 8) & 15
        if sub == 0:
            return 'MOV.B   R0,@(%d,%s)' % (d4, R[m])
        if sub == 1:
            return 'MOV.W   R0,@(%d,%s)' % (d4 * 2, R[m])
        if sub == 4:
            return 'MOV.B   @(%d,%s),R0' % (d4, R[m])
        if sub == 5:
            return 'MOV.W   @(%d,%s),R0' % (d4 * 2, R[m])
        if sub == 8:
            return 'CMP/EQ  #%d,R0' % s8(i8)
        if sub == 9:
            return 'BT      %06X' % (pc + 4 + s8(d8) * 2)
        if sub == 11:
            return 'BF      %06X' % (pc + 4 + s8(d8) * 2)
        if sub == 13:
            return 'BT/S    %06X' % (pc + 4 + s8(d8) * 2)
        if sub == 15:
            return 'BF/S    %06X' % (pc + 4 + s8(d8) * 2)
        return '.word   %04X' % op
    if t == 9:
        a = pc + 4 + d8 * 2
        v = read16(a) if read16 else None
        return 'MOV.W   @(%d,PC),%s   ; [%06X] = %s' % (
            d8 * 2, R[n], a, '%04X' % v if v is not None else '?')
    if t == 10:
        return 'BRA     %06X' % (pc + 4 + s12(op & 0xfff) * 2)
    if t == 11:
        return 'BSR     %06X' % (pc + 4 + s12(op & 0xfff) * 2)
    if t == 12:
        sub = (op >> 8) & 15
        if sub == 0:
            return 'MOV.B   R0,@(%d,GBR)' % d8
        if sub == 1:
            return 'MOV.W   R0,@(%d,GBR)' % (d8 * 2)
        if sub == 2:
            return 'MOV.L   R0,@(%d,GBR)' % (d8 * 4)
        if sub == 3:
            return 'TRAPA   #%d' % d8
        if sub == 4:
            return 'MOV.B   @(%d,GBR),R0' % d8
        if sub == 5:
            return 'MOV.W   @(%d,GBR),R0' % (d8 * 2)
        if sub == 6:
            return 'MOV.L   @(%d,GBR),R0' % (d8 * 4)
        if sub == 7:
            a = ((pc + 4) & ~3) + d8 * 4
            return 'MOVA    @(%d,PC),R0   ; %06X' % (d8 * 4, a)
        if sub == 8:
            return 'TST     #%d,R0' % d8
        if sub == 9:
            return 'AND     #%d,R0' % d8
        if sub == 10:
            return 'XOR     #%d,R0' % d8
        if sub == 11:
            return 'OR      #%d,R0' % d8
        if sub == 12:
            return 'TST.B   #%d,@(R0,GBR)' % d8
        if sub == 13:
            return 'AND.B   #%d,@(R0,GBR)' % d8
        if sub == 14:
            return 'XOR.B   #%d,@(R0,GBR)' % d8
        if sub == 15:
            return 'OR.B    #%d,@(R0,GBR)' % d8
    if t == 13:
        a, v = pcrel32(d8)
        return 'MOV.L   @(%d,PC),%s   ; [%06X] = %s' % (
            d8 * 4, R[n], a, '%08X' % v if v is not None else '?')
    if t == 14:
        return 'MOV     #%d,%s' % (s8(i8), R[n])
    if t == 15:
        return '.word   %04X   ; FPU?' % op
    return '.word   %04X' % op


def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    base = 0
    for a in sys.argv[1:]:
        if a.startswith('--base'):
            base = int(a.split('=', 1)[1], 0)
    rom = open(args[0], 'rb').read()
    start = int(args[1], 16)
    length = int(args[2], 0) if len(args) > 2 else 0x80

    def rd16(a):
        a -= base
        return int.from_bytes(rom[a:a + 2], 'big') if 0 <= a and a + 2 <= len(rom) else None

    def rd32(a):
        a -= base
        return int.from_bytes(rom[a:a + 4], 'big') if 0 <= a and a + 4 <= len(rom) else None

    pc = start
    while pc < start + length:
        op = rd16(pc)
        if op is None:
            break
        print('%06X  %04X  %s' % (pc, op, dis(op, pc, rd32, rd16)))
        pc += 2


if __name__ == '__main__':
    main()
