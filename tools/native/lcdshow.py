#!/usr/bin/env python3
# license:BSD-3-Clause
"""**`render --lcd-every` の吐き出しを、変わったところだけ画面の形で見せる**。

演奏画面の中身を測るための道具（doc/native-engine.md の 6.190）。

  build/render.exe <roms> <mid> out.wav <秒> --boot 8.0 --lcd-every 0.05 \\
      | findstr /b LCD > lcd.txt
  python tools/native/lcdshow.py lcd.txt [--cg]

`--cg` を付けると、同じ吐き出しに混ざっている外字（`CG` の行。1 文字
8 バイト × 8 文字）も 5 × 8 の点で出す。音色の絵がどう入るかを見るとき。
"""
import sys
from pathlib import Path


def ch(x):
    return chr(x) if 0x20 <= x < 0x7f else "."


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    show_cg = "--cg" in sys.argv
    if not args:
        print(__doc__)
        return 1
    path = Path(args[0])

    seen = set()
    cg_at = {}
    rows = []
    for line in path.read_text(errors="replace").splitlines():
        f = line.split()
        if not f:
            continue
        if f[0] == "CG" and len(f) >= 66:
            cg_at[f[1]] = [int(x, 16) for x in f[2:66]]
            continue
        if f[0] != "LCD" or len(f) < 50:
            continue
        b = [int(x, 16) for x in f[2:50]]
        key = tuple(b)
        if key in seen:
            continue
        seen.add(key)
        rows.append((f[1], b))

    for t, b in rows:
        r0, r1 = b[:24], b[24:]
        print("%8s |%s|  %s" % (t, "".join(ch(x) for x in r0),
                                " ".join("%02x" % x for x in r0)))
        print("         |%s|  %s" % ("".join(ch(x) for x in r1),
                                     " ".join("%02x" % x for x in r1)))
        cg = cg_at.get(t)
        if show_cg and cg:
            for r in range(8):
                print("          " + " ".join(
                    "".join("#" if (cg[c * 8 + r] >> (4 - k)) & 1 else "."
                            for k in range(5))
                    for c in range(8)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
