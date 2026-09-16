#!/usr/bin/env python3
"""Decode the block dump printed by SMU2000_JIT_DUMP=<hex pc>.

Reads "blk <pc> <len>" lines followed by hex words on stdin and disassembles
each block with capstone, so a JIT block can be read next to the SH-2 code it
came from. Temporary aid for the arm64 port.
"""
import sys

import capstone


def main():
    md = capstone.Cs(capstone.CS_ARCH_ARM64, capstone.CS_MODE_LITTLE_ENDIAN)
    pc = None
    for line in sys.stdin:
        parts = line.split()
        if parts and parts[0] == "blk":
            pc = int(parts[1], 16)
            print("block %06X (%s words)" % (pc, parts[2] if len(parts) > 2 else "?"))
            continue
        for i, w in enumerate(parts):
            code = bytes.fromhex(w[6:8] + w[4:6] + w[2:4] + w[0:2])
            for ins in md.disasm(code, 0):
                print("  %3d  %s  %s %s" % (i, w, ins.mnemonic, ins.op_str))


if __name__ == "__main__":
    main()
