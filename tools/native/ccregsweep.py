#!/usr/bin/env python3
# license:BSD-3-Clause
"""**CC を動かしたときのスロットのレジスタ**を、実機（firmware）と native で比べる。

`regsweep.py` の対。あちらは鍵を押した瞬間を比べるが、こちらは**鳴らしたまま
つまみを動かして**、その先で書き替わる値を突き合わせる。エフェクトの送り
（CC91・CC93）や音量・パンのように、押したあとに動くものを詰めるための道具。

  ccregsweep.py <rom ディレクトリ> <CC 番号> [値] [音色]

    CC 番号  91（リバーブ送り）・93（コーラス）・7（音量）など
    値       "0,40,80,127" のように `,` で並べる（既定 0,40,80,127）
    音色     "0,0,0;0,0,48" のように msb,lsb,prog を `;` で並べる
             （既定 GrandPno・Strings1・Flute）

1 音目は firmware に鳴らさせて写し取らせ、2 音目（native が鳴らす）を鳴らした
まま CC を渡す。0.5 秒回してから、そのスロットのレジスタを 1 本ずつ比べる。

`SMU2000_CUT_EXACT=1` を立てないと `0x00`（切る高さ）は写し取り前提の値の
ままなので食い違う（doc/native-engine.md の 6.71）。

例:
  # リバーブ送りを全部の値で
  python tools/native/ccregsweep.py ../MU2000/roms 91 0,1,20,40,60,80,100,127
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
WORK = BUILD / "ccregsweep"

LINE = re.compile(r'^(N |W )?00800000 ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
# 毎サンプル書き替わるので比べない（MEG の戻りのミキサ）
SKIP = set([0x0e, 0x0f] + list(range(0x38, 0x40)))
# 鍵を離すより前だけを見る（実機は鳴り終わると書かなくなる）
T_MAX = int(3.7 * 44100)


def vlq(n):
    out = [n & 0x7f]
    n >>= 7
    while n:
        out.append((n & 0x7f) | 0x80)
        n >>= 7
    return bytes(reversed(out))


def make_mid(path, msb, lsb, prog, cc, val, note=60, vel=100):
    ev = [(0, bytes([0xff, 0x51, 0x03]) + (500000).to_bytes(3, 'big')),
          (240, bytes([0xb0, 0x00, msb])), (240, bytes([0xb0, 0x20, lsb])),
          (240, bytes([0xc0, prog])),
          (480, bytes([0x90, note, vel])),     # 1 音目: 写し取り
          (1440, bytes([0x80, note, 0])),
          (1920, bytes([0x90, note, vel])),    # 2 音目: native が鳴らす
          (2400, bytes([0xb0, cc, val])),
          (3840, bytes([0x80, note, 0]))]
    body = bytearray()
    prev = 0
    for t, b in ev:
        body += vlq(t - prev) + b
        prev = t
    body += vlq(0) + bytes([0xff, 0x2f, 0x00])
    path.write_bytes(b'MThd' + struct.pack('>IHHH', 6, 0, 1, 480) +
                     b'MTrk' + struct.pack('>I', len(body)) + bytes(body))


def trace(roms, mid, tag, native):
    """そのスロットの、落ち着いたあとのレジスタ"""
    trc = WORK / ("t_%s.txt" % tag)
    cmd = [str(BUILD / "render"), roms, str(mid), str(WORK / ("t_%s.wav" % tag)),
           "5", "--bootcache", "--trace-swp", str(trc)]
    if native:
        cmd.append("--native-engine")
    env = dict(os.environ)
    env["SMU2000_NO_VOICECACHE"] = "1"
    subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    cur = collections.defaultdict(dict)
    mask = 0
    last = None
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
                got = [i for i in range(64) if (mask >> i) & 1]
                if got:
                    last = got[0]
            elif reg < 0x1000 and reg % 64 not in SKIP:
                # **鍵を離す前までしか見ない**。実機は声が鳴り終わると
                # 書くのをやめるので、最後まで取ると native と比べられない
                if int(m.group(4)) <= T_MAX:
                    cur[reg // 64][reg % 64] = val
    return last, dict(cur.get(last, {}))


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    roms = sys.argv[1]
    cc = int(sys.argv[2])
    vals = [int(x) for x in (sys.argv[3].split(",") if len(sys.argv) > 3
                             else ["0", "40", "80", "127"])]
    cases = ([tuple(int(x) for x in a.split(",")) for a in sys.argv[4].split(";")]
             if len(sys.argv) > 4 else [(0, 0, 0), (0, 0, 48), (0, 0, 73)])
    WORK.mkdir(parents=True, exist_ok=True)

    bad = collections.Counter()
    ncase = 0
    for msb, lsb, prog in cases:
        for v in vals:
            mid = WORK / "cc.mid"
            make_mid(mid, msb, lsb, prog, cc, v)
            sf, f = trace(roms, mid, "fw", False)
            sn, d = trace(roms, mid, "nv", True)
            if not f or not d:
                print("%d,%d,%-3d CC%-3d=%-3d 測れず（実機 slot%s / native slot%s）"
                      % (msb, lsb, prog, cc, v, sf, sn))
                continue
            ncase += 1
            diff = [r for r in sorted(f) if r in d and f[r] != d[r]]
            for r in diff:
                bad[r] += 1
            if not diff:
                continue
            print("%d,%d,%-3d CC%-3d=%-3d 違う %2d 本: %s" % (
                msb, lsb, prog, cc, v, len(diff),
                " ".join("0x%02x(%04x/%04x)" % (r, f[r], d[r]) for r in diff[:10])))
    print()
    print("違ったレジスタ（多い順）: %s" %
          " ".join("0x%02x×%d" % (r, c) for r, c in bad.most_common()))
    print("食い違い %d 本 / 当たった %d 通り" % (sum(bad.values()), ncase))
    return 0


if __name__ == "__main__":
    sys.exit(main())
