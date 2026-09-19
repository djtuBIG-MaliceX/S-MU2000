#!/usr/bin/env python3
# license:BSD-3-Clause
"""**式だけで組んだスロットのレジスタ**を、実機（firmware）が書く値と比べる。

native の口（doc/native-engine.md の段 3）がどこまで来たかを測る道具。
`nativeplay --nocal` は写し取りを一切混ぜずにレジスタを組むので、その値を
`render --trace-swp` で録った firmware の書き込みと 1 本ずつ突き合わせる。

  regsweep.py <rom ディレクトリ> [音色] [鍵] [強さ]

    音色   "0,0,0;0,0,4" のように msb,lsb,prog を `;` で並べる。
           省略すると GM の 128 音色（バンク 0）
    鍵     "36,60,84" のように `,` で並べる（既定 60）
    強さ   同上（既定 100）

例:
  # GM 128 音色を鍵 60・強さ 100 で
  python tools/native/regsweep.py ../MU2000/roms
  # 14 音色を鍵 3 通り × 強さ 3 通りで
  python tools/native/regsweep.py ../MU2000/roms 0,0,0;0,0,40 36,60,84 30,100,127

`SMU2000_CUT_EXACT=1` を立てると、切る高さ（0x00）も式で出した値になる
（doc の 6.71）。立てないと 0x00 は写し取り前提の値のままなので食い違う。
"""
import collections
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "build"
WORK = BUILD / "regsweep"

LINE = re.compile(r'^(N |W )?00800000 ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
REG = re.compile(r'([0-9a-f]{2})=([0-9a-f]{4})')
# 毎サンプル書き替わるので比べない（MEG の戻りのミキサ）
SKIP = set([0x0e, 0x0f] + list(range(0x38, 0x40)))


def vlq(n):
    out = [n & 0x7f]
    n >>= 7
    while n:
        out.append((n & 0x7f) | 0x80)
        n >>= 7
    return bytes(reversed(out))


def make_mid(path, msb, lsb, prog, note, vel):
    ev = [(0, bytes([0xff, 0x51, 0x03]) + (500000).to_bytes(3, 'big')),
          (240, bytes([0xb0, 0x00, msb])), (240, bytes([0xb0, 0x20, lsb])),
          (240, bytes([0xc0, prog])),
          (960, bytes([0x90, note, vel])), (1920, bytes([0x80, note, 0]))]
    body = bytearray()
    prev = 0
    for t, b in ev:
        body += vlq(t - prev) + b
        prev = t
    body += vlq(0) + bytes([0xff, 0x2f, 0x00])
    path.write_bytes(b'MThd' + struct.pack('>IHHH', 6, 0, 1, 480) +
                     b'MTrk' + struct.pack('>I', len(body)) + bytes(body))


def fw_regs(roms, msb, lsb, prog, note, vel):
    """firmware が鍵を押した瞬間にスロットへ書いた値"""
    mid = WORK / "rs.mid"
    trc = WORK / "rs.txt"
    make_mid(mid, msb, lsb, prog, note, vel)
    subprocess.run([str(BUILD / "render"), roms, str(mid), str(WORK / "rs.wav"),
                    "4", "--bootcache", "--trace-swp", str(trc)],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    cur = collections.defaultdict(dict)
    mask = 0
    with open(trc, errors="replace") as f:
        for line in f:
            m = LINE.match(line)
            if not m:
                continue
            reg, val = int(m.group(2), 16), int(m.group(3), 16)
            if reg == 0x18e:   mask = (mask & ~(0xffff << 48)) | (val << 48)
            elif reg == 0x18f: mask = (mask & ~(0xffff << 32)) | (val << 32)
            elif reg == 0x1ce: mask = (mask & ~(0xffff << 16)) | (val << 16)
            elif reg == 0x1cf: mask = (mask & ~0xffff) | val
            elif reg == 0x20e:
                got = [dict(cur[i]) for i in range(64) if (mask >> i) & 1 and cur.get(i)]
                if got:
                    return got
            elif reg < 0x1000 and reg % 64 not in SKIP:
                cur[reg // 64][reg % 64] = val
    return []


def nv_regs(roms, msb, lsb, prog, note, vel):
    """式だけで組んだ値（nativeplay --nocal）"""
    out = subprocess.run([str(BUILD / "nativeplay"), roms, str(WORK / "rs2.wav"),
                          "-b", "%d,%d,%d" % (msb, lsb, prog),
                          "-n", str(note), "-v", str(vel), "--nocal"],
                         capture_output=True, text=True,
                         encoding="utf-8", errors="replace").stdout
    d = {}
    started = False
    for line in out.splitlines():
        if "スロット" in line and "レジスタ" in line:
            started = True
            continue
        if started:
            if not line.startswith("  "):
                break
            for a, b in REG.findall(line):
                d[int(a, 16)] = int(b, 16)
    return d


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    roms = sys.argv[1]
    WORK.mkdir(parents=True, exist_ok=True)
    if len(sys.argv) > 2 and sys.argv[2] not in ("", "-"):
        cases = [tuple(int(x) for x in a.split(",")) for a in sys.argv[2].split(";")]
    else:
        cases = [(0, 0, p) for p in range(128)]
    notes = [int(x) for x in (sys.argv[3].split(",") if len(sys.argv) > 3 else ["60"])]
    vels = [int(x) for x in (sys.argv[4].split(",") if len(sys.argv) > 4 else ["100"])]

    bad = collections.Counter()
    ncase = 0
    for msb, lsb, prog in cases:
        for note in notes:
            for vel in vels:
                slots = fw_regs(roms, msb, lsb, prog, note, vel)
                n = nv_regs(roms, msb, lsb, prog, note, vel)
                if not slots or not n:
                    print("%d,%d,%-3d 鍵%-3d 強さ%-4d 測れず" % (msb, lsb, prog, note, vel))
                    continue
                ncase += 1
                # **多要素の音色**は、実機が鳴らしたスロットのどれかと合えばよい
                # （こちらが組むのは 1 要素ぶんで、実機がどの順で並べるかは別）
                best, bestd = None, None
                for f in slots:
                    d = [r for r in sorted(f) if r in n and f[r] != n[r]]
                    if bestd is None or len(d) < len(bestd):
                        best, bestd = f, d
                f, diff = best, bestd
                miss = [r for r in sorted(f) if r not in n]
                for r in diff:
                    bad[r] += 1
                if not diff and not miss:
                    continue
                print("%d,%d,%-3d 鍵%-3d 強さ%-4d 違う %2d 本: %s%s" % (
                    msb, lsb, prog, note, vel, len(diff),
                    " ".join("0x%02x(%04x/%04x)" % (r, f[r], n[r]) for r in diff[:8]),
                    "  無い:" + " ".join("0x%02x" % r for r in miss) if miss else ""))
    print()
    print("音色をまたいで違ったレジスタ（多い順）: %s" %
          " ".join("0x%02x×%d" % (r, c) for r, c in bad.most_common()))
    print("食い違い %d 本 / 当たった %d 通り" % (sum(bad.values()), ncase))
    return 0


if __name__ == "__main__":
    sys.exit(main())
