#!/usr/bin/env python3
# license:BSD-3-Clause
"""**つまみを 1 つ振って、実機が押鍵時に書くレジスタを並べる**。

写し取りを捨てる（段 4）ために、まだ式が分かっていないのは
「XG のパートのつまみ → スロットのレジスタ」の変換だけになった。
その式を当てるには、つまみを端から端まで振って値を並べるのがいちばん早い。

  python tools/native/ccprobe.py <CC 番号> [--roms DIR] [--voice msb,lsb,prog]
                                [--note N] [--vel N] [--step N] [--regs 06,07]

例:
  python tools/native/ccprobe.py 73                 立ち上がり（0x06 など）
  python tools/native/ccprobe.py 75 --regs 08       減衰 2
  python tools/native/ccprobe.py 71 --regs 04       共振
"""
import argparse
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import regsweep


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cc", type=int)
    ap.add_argument("--roms")
    ap.add_argument("--voice", default="0,0,48")
    ap.add_argument("--note", type=int, default=60)
    ap.add_argument("--vel", type=int, default=100)
    ap.add_argument("--step", type=int, default=8)
    ap.add_argument("--lo", type=int, default=0)
    ap.add_argument("--hi", type=int, default=127)
    ap.add_argument("--regs", default="00,04,06,07,08,09,0a,0b")
    a = ap.parse_args()

    roms = a.roms or os.environ.get("SMU2000_ROMS")
    if not roms:
        for c in (regsweep.ROOT / "roms", regsweep.ROOT.parent / "MU2000" / "roms"):
            if (c / "mu2000.zip").exists():
                roms = str(c)
                break
    if not roms:
        print("ROM が見つからない")
        return 1
    regsweep.WORK.mkdir(parents=True, exist_ok=True)
    msb, lsb, prog = (int(x) for x in a.voice.split(","))
    regs = [int(x, 16) for x in a.regs.split(",")]

    vals = list(range(a.lo, a.hi + 1, a.step))
    if a.hi not in vals:
        vals.append(a.hi)
    print("音色 %d,%d,%d  鍵 %d  強さ %d  CC%d を振る"
          % (msb, lsb, prog, a.note, a.vel, a.cc))
    print("%-5s %s" % ("値", " ".join("0x%02x " % r for r in regs)))
    for v in vals:
        slots = regsweep.fw_regs(roms, msb, lsb, prog, a.note, a.vel,
                                 [(a.cc, v)])
        if not slots:
            print("%-5d 測れず" % v)
            continue
        f = slots[0]
        print("%-5d %s" % (v, " ".join("%04x  " % f.get(r, 0xffff) for r in regs)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
