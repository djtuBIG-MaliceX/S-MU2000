#!/usr/bin/env python3
# license:BSD-3-Clause
"""**要素の 84 バイトが、実機のボイスの塊のどこへ入るかを総当たりで当てる**。

音色を順に切り替えながらワーク RAM を写して、鳴ったスロットの
**ボイスの塊（`0x424364 + 0x94 × スロット`。0x94 バイト）**を集める。
そのあと

  * 塊の各番地 o について、`塊[o] == 要素[i]` が全部の音色で成り立つ i
  * 成り立たなくても、`要素[i]` と 1 対 1 に対応している（値の組が
    ぶれない）番地

を出す。6.204 の byte36 のように「読んでいない要素のバイト」の
行き先を探すときの道具（doc/native-engine.md の 6.204）。

  python tools/native/blockmap.py [--roms DIR] [--lo N] [--hi N]
                                  [--note N] [--vel N] [--lsb N]
"""
import argparse
import os
import struct
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "build"
WORK = BUILD / "tests" / "blockmap"
EXE = ".exe" if os.name == "nt" else ""
NEEDED = ("mu2000.zip", "swp30.zip")
VB, VSTRIDE, VLEVEL = 0x424364, 0x94, 118
PARTS, PSTRIDE = 0x28d64, 0x134
PART_VOICE = 0xf8


def find_roms(given):
    cands = [Path(given)] if given else []
    if os.environ.get("SMU2000_ROMS"):
        cands.append(Path(os.environ["SMU2000_ROMS"]))
    cands += [ROOT / "roms", ROOT.parent / "MU2000" / "roms"]
    for c in cands:
        if all((c / n).exists() for n in NEEDED):
            return c
    return None


def part_base(p):
    port, k = p // 16, p % 16
    return PARTS + (port * 16 + (0 if k == 9 else (k + 1 if k < 9 else k))) * PSTRIDE


def vlq(n):
    out = [n & 0x7f]
    n >>= 7
    while n:
        out.append((n & 0x7f) | 0x80)
        n >>= 7
    return bytes(reversed(out))


def make_mid(path, progs, note, vel, step, lsb):
    tick = 480
    ev = [(0, bytes([0xff, 0x51, 0x03]) + (500000).to_bytes(3, 'big'))]
    t = tick
    for p in progs:
        ev.append((t, bytes([0xb0, 0x78, 0x00])))
        ev.append((t + 4, bytes([0xb0, 0x00, 0x00])))
        ev.append((t + 6, bytes([0xb0, 0x20, lsb & 0x7f])))
        ev.append((t + 8, bytes([0xc0, p & 0x7f])))
        ev.append((t + step // 4, bytes([0x90, note, vel])))
        ev.append((t + step - step // 5, bytes([0x80, note, 0])))
        t += step
    body = bytearray()
    prev = 0
    for tt, b in sorted(ev, key=lambda e: e[0]):
        body += vlq(tt - prev) + b
        prev = tt
    body += vlq(0) + bytes([0xff, 0x2f, 0x00])
    path.write_bytes(b'MThd' + struct.pack('>IHHH', 6, 0, 1, 480) +
                     b'MTrk' + struct.pack('>I', len(body)) + bytes(body))
    return tick, t


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--roms")
    ap.add_argument("--lo", type=int, default=0)
    ap.add_argument("--hi", type=int, default=127)
    ap.add_argument("--note", type=int, default=60)
    ap.add_argument("--vel", type=int, default=100)
    ap.add_argument("--lsb", type=int, default=0)
    a = ap.parse_args()

    roms = find_roms(a.roms)
    if not roms:
        print("ROM が見つからない（--roms か SMU2000_ROMS で指す）")
        return 1
    WORK.mkdir(parents=True, exist_ok=True)
    for old in WORK.glob("ram*.bin"):
        old.unlink()
    progs = list(range(a.lo, a.hi + 1))
    step = 240
    mid = WORK / "bm.mid"
    tick0, tend = make_mid(mid, progs, a.note, a.vel, step, a.lsb)
    sec = lambda ticks: ticks / 480.0 * 0.5
    env = dict(os.environ)
    env["SMU2000_NO_VOICECACHE"] = "1"
    env["SMU2000_RAMSNAP"] = str(WORK)
    env["SMU2000_RAMSNAP_T0"] = "%.4f" % sec(tick0 + step // 2)
    env["SMU2000_RAMSNAP_DT"] = "%.4f" % sec(step)
    env["SMU2000_RAMSNAP_N"] = str(len(progs))
    r = subprocess.run([str(BUILD / ("render" + EXE)), str(roms), str(mid),
                        str(WORK / "bm.wav"), "%.3f" % sec(tend + 480),
                        "--boot", "8.0"], env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if r.returncode != 0:
        print("鳴らせなかった")
        return 1

    rom = (roms / "mu2000_flash.bin").read_bytes()
    # 1 要素の音色だけ集める（どのスロットがどの要素か迷わないように）
    samples = []                      # (要素 84 バイト, 塊 0x94 バイト)
    prev = None
    for i, p in enumerate(progs):
        f = WORK / ("ram%03d.bin" % i)
        if not f.exists():
            continue
        ram = f.read_bytes()
        lv = [ram[VB + s * VSTRIDE + VLEVEL - 0x400000] for s in range(64)]
        used = [s for s in range(64)
                if lv[s] and (prev is None or lv[s] != prev[s])]
        prev = lv
        b = part_base(0)
        rec = int.from_bytes(ram[b + PART_VOICE: b + PART_VOICE + 4], "big")
        if not rec or rom[rec] != 1 or len(used) != 1:
            continue                  # 1 要素で、スロットが 1 本だけのものに絞る
        o = VB + used[0] * VSTRIDE - 0x400000
        samples.append((rom[rec + 12: rec + 12 + 84], ram[o: o + VSTRIDE]))
    print("使えた音色: %d 件（1 要素・スロット 1 本のものだけ）" % len(samples))
    if len(samples) < 8:
        print("少なすぎる。--lo/--hi や --lsb を変えてみる")
        return 1

    # 1. そのまま一致する番地
    same = defaultdict(list)
    for o in range(VSTRIDE):
        for i in range(84):
            if all(blk[o] == el[i] for el, blk in samples):
                same[i].append(o)
    print()
    print("■ 塊にそのまま入っている要素のバイト")
    for i in sorted(same):
        print("   要素[%2d] → 塊 +%s" % (i, "・+".join("0x%02x" % o for o in same[i])))

    # 2. 1 対 1 に対応している番地（値は違っても組がぶれない）
    print()
    print("■ 値は違うが 1 対 1 に対応している所（表を 1 枚はさんでいる見込み）")
    for i in range(84):
        if i in same:
            continue
        vals = set(el[i] for el, _ in samples)
        if len(vals) < 3:
            continue
        for o in range(VSTRIDE):
            m = {}
            ok = True
            for el, blk in samples:
                if m.setdefault(el[i], blk[o]) != blk[o]:
                    ok = False
                    break
            if ok and len(set(m.values())) == len(m):
                print("   要素[%2d] ←→ 塊 +0x%02x  （%d 通り）" % (i, o, len(m)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
