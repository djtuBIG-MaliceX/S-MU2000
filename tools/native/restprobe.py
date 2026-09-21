#!/usr/bin/env python3
# license:BSD-3-Clause
"""**実機の「残り」の 2 つ（ボイスの塊 +119・+120）を強さ違いで読む**。

実機（`0x12A542`-`0x12A550`）は

    減衰 = 表[0x1E6818 + 索引] + 塊[+0x78] + 塊[+0x77]

と、残りを**2 つに分けて**持っている。片方が強さの曲線、もう片方が
波形の段のはず。どちらがどう違うのかを直に読む。

  python restprobe.py [--msb N] [--lsb N] [--prog N] [--note N]
"""
import argparse
import os
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "build"
EXE = ".exe" if os.name == "nt" else ""
NEEDED = ("mu2000.zip", "swp30.zip")
WORK = BUILD / "tests" / "restprobe"
VB, VSTRIDE = 0x424364, 0x94
PARTS, PSTRIDE = 0x28d64, 0x134
PART_VOICE = 0xf8
VELS = [1, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 96, 104, 112, 120, 127]


def find_roms(given):
    cands = [Path(given)] if given else []
    if os.environ.get("SMU2000_ROMS"):
        cands.append(Path(os.environ["SMU2000_ROMS"]))
    cands += [ROOT / "roms", ROOT.parent / "MU2000" / "roms"]
    for c in cands:
        if all((c / n).exists() for n in NEEDED):
            return c
    return None


def vlq(n):
    out = [n & 0x7f]
    n >>= 7
    while n:
        out.append((n & 0x7f) | 0x80)
        n >>= 7
    return bytes(reversed(out))


def part_base(p):
    port, k = p // 16, p % 16
    return PARTS + (port * 16 + (0 if k == 9 else (k + 1 if k < 9 else k))) * PSTRIDE


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--roms")
    ap.add_argument("--msb", type=int, default=0)
    ap.add_argument("--lsb", type=int, default=0)
    ap.add_argument("--prog", type=int, default=48)
    ap.add_argument("--note", type=int, default=60)
    a = ap.parse_args()

    roms = find_roms(a.roms)
    if not roms:
        print("ROM が見つからない（--roms か SMU2000_ROMS で指す）")
        return 1
    WORK.mkdir(parents=True, exist_ok=True)
    for old in WORK.glob("ram*.bin"):
        old.unlink()

    tick, step = 480, 240
    ev = [(0, bytes([0xff, 0x51, 0x03]) + (500000).to_bytes(3, 'big'))]
    t = tick
    for v in VELS:
        ev.append((t, bytes([0xb0, 0x78, 0x00])))
        ev.append((t + 4, bytes([0xb0, 0x00, a.msb])))
        ev.append((t + 6, bytes([0xb0, 0x20, a.lsb])))
        ev.append((t + 8, bytes([0xc0, a.prog])))
        ev.append((t + step // 4, bytes([0x90, a.note, v])))
        ev.append((t + step - step // 5, bytes([0x80, a.note, 0])))
        t += step
    body = bytearray()
    prev = 0
    for tt, b in sorted(ev, key=lambda e: e[0]):
        body += vlq(tt - prev) + b
        prev = tt
    body += vlq(0) + bytes([0xff, 0x2f, 0x00])
    mid = WORK / "rp.mid"
    mid.write_bytes(b'MThd' + struct.pack('>IHHH', 6, 0, 1, 480) +
                    b'MTrk' + struct.pack('>I', len(body)) + bytes(body))

    sec = lambda ticks: ticks / 480.0 * 0.5
    env = dict(os.environ)
    env["SMU2000_NO_VOICECACHE"] = "1"
    env["SMU2000_RAMSNAP"] = str(WORK)
    env["SMU2000_RAMSNAP_T0"] = "%.4f" % sec(tick + step // 2)
    env["SMU2000_RAMSNAP_DT"] = "%.4f" % sec(step)
    env["SMU2000_RAMSNAP_N"] = str(len(VELS))
    r = subprocess.run([str(BUILD / ("render" + EXE)), str(roms), str(mid),
                        str(WORK / "rp.wav"), "%.3f" % sec(t + 480),
                        "--boot", "8.0"], env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if r.returncode != 0:
        print("鳴らせなかった")
        return 1

    rom = (roms / "mu2000_flash.bin").read_bytes()
    print("msb=%d lsb=%d prog=%d 鍵=%d" % (a.msb, a.lsb, a.prog, a.note))
    print("%4s  %s" % ("強さ", "要素ごとの（目盛り +118 / +119 / +120）"))
    prev_lv = None
    for i, v in enumerate(VELS):
        f = WORK / ("ram%03d.bin" % i)
        if not f.exists():
            continue
        ram = f.read_bytes()
        cells = []
        for s in range(64):
            o = VB + s * VSTRIDE - 0x400000
            cells.append((ram[o + 118], ram[o + 119], ram[o + 120]))
        used = [s for s in range(64)
                if cells[s][0] and (prev_lv is None or cells[s] != prev_lv[s])]
        prev_lv = cells
        b = part_base(0)
        rec = int.from_bytes(ram[b + PART_VOICE: b + PART_VOICE + 4], "big")
        nel = sum(1 for k in range(4) if rom[rec] & (1 << k)) if rec else 0
        txt = "  ".join("%3d/%3d/%3d" % cells[s] for s in used[:nel])
        print("%4d  %s" % (v, txt))
    return 0


if __name__ == "__main__":
    sys.exit(main())
