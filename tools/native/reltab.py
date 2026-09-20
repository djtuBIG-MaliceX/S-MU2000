#!/usr/bin/env python3
# license:BSD-3-Clause
"""**離しのつまみ（CC72 / 08 pp 1C）が離しの速さをどう動かすか、128 段測る**。

離しの速さはレジスタ `0x09` の上位（ビット 15 が「離せ」の印、残りが速さ）で、
**鍵を離したときに書かれる**。押鍵時のレジスタしか見ない `egtab.py` では
拾えないので、こちらで測る。

  python tools/native/reltab.py [--voice msb,lsb,prog] [--note N] [--vel N]
                               [--addr 1c] [--chunk N]

1 回の演奏でつまみを変えては 1 音鳴らして離す。離しの書き込みを順に拾う。
"""
import argparse
import collections
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "build"
WORK = BUILD / "tests" / "reltab"
EXE = ".exe" if os.name == "nt" else ""
NEEDED = ("mu2000.zip", "swp30.zip")
BOOT = 8.0
LINE = re.compile(r'^(N |W |R )?(00800000) ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
DECAY_TAB = 0x1F4E38


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


def xg_sysex(body):
    data = bytes([0x43, 0x10, 0x4c]) + bytes(body)
    return bytes([0xf0]) + vlq(len(data) + 1) + data + bytes([0xf7])


def make_mid(path, vals, msb, lsb, prog, note, vel, addr):
    tick = 480
    ev = [(0, bytes([0xff, 0x51, 0x03]) + (500000).to_bytes(3, 'big')),
          (240, bytes([0xb0, 0x00, msb])), (240, bytes([0xb0, 0x20, lsb])),
          (240, bytes([0xc0, prog]))]
    t = tick * 2
    for v in vals:
        ev.append((t, bytes([0xb0, 0x78, 0x00])))          # 声を空ける
        ev.append((t + tick // 16, xg_sysex([0x08, 0x00, addr, v & 0x7f])))
        ev.append((t + tick // 8, bytes([0x90, note, vel])))
        ev.append((t + tick // 4, bytes([0x80, note, 0])))  # ここで離しが書かれる
        t += tick // 2
    body = bytearray()
    prev = 0
    for tt, b in sorted(ev, key=lambda e: e[0]):
        body += vlq(tt - prev) + b
        prev = tt
    body += vlq(0) + bytes([0xff, 0x2f, 0x00])
    path.write_bytes(b'MThd' + struct.pack('>IHHH', 6, 0, 1, 480) +
                     b'MTrk' + struct.pack('>I', len(body)) + bytes(body))
    return (t + tick) / float(tick) * 0.5


def releases(trc):
    """離しの書き込みを (サンプル, 速さ) で返す。消し込み（`f0ff`）は外す"""
    out = []
    with open(trc, errors="replace") as f:
        for line in f:
            m = LINE.match(line)
            if not m or m.group(1) == "R ":
                continue
            reg, val, sp = int(m.group(3), 16), int(m.group(4), 16), int(m.group(5))
            if (reg < 0x1000 and reg % 64 == 9 and (val & 0x8000)
                    and (val & 0xff) != 0xff):
                out.append((sp, (val >> 8) & 0x7f))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--roms")
    ap.add_argument("--voice", default="0,0,48")
    ap.add_argument("--note", type=int, default=60)
    ap.add_argument("--vel", type=int, default=100)
    ap.add_argument("--addr", default="1c")
    ap.add_argument("--chunk", type=int, default=32)
    a = ap.parse_args()

    roms = find_roms(a.roms)
    if not roms:
        print("ROM が見つからない（--roms か SMU2000_ROMS で指す）")
        return 1
    WORK.mkdir(parents=True, exist_ok=True)
    msb, lsb, prog = (int(x) for x in a.voice.split(","))
    addr = int(a.addr, 16)
    rom = (roms / "mu2000_flash.bin").read_bytes()
    back = {}
    for i in range(127, -1, -1):
        back[rom[DECAY_TAB + i]] = i

    print("音色 %d,%d,%d  鍵 %d  強さ %d  08 pp %02x  離しの速さ"
          % (msb, lsb, prog, a.note, a.vel, addr))
    vals = list(range(128))
    got = {}
    for c0 in range(0, len(vals), a.chunk):
        chunk = [64] + vals[c0:c0 + a.chunk]
        mid = WORK / "rel.mid"
        secs = make_mid(mid, chunk, msb, lsb, prog, a.note, a.vel, addr)
        trc = WORK / "rel.txt"
        env = dict(os.environ)
        env["SMU2000_NO_VOICECACHE"] = "1"
        r = subprocess.run([str(BUILD / ("render" + EXE)), str(roms), str(mid),
                            str(WORK / "rel.wav"), "%.3f" % secs, "--bootcache",
                            "--trace-swp", str(trc)],
                           env=env, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
        if r.returncode != 0:
            print("鳴らせなかった")
            return 1
        rel = releases(trc)
        # **鍵を離した時刻のすぐあと**の書き込みを拾う（オールサウンドオフや
        # 消し込みが混ざるので、順番だけでは合わない）
        boot = 0
        for sp, _ in rel:
            boot = sp
            break
        for i, v in enumerate(chunk):
            # 段 i の note off は 2*480 + i*240 + 120 tick ＝ その秒
            off = (2 * 480 + i * 240 + 120) / 480.0 * 0.5
            want = int(off * 44100)
            hit = None
            for sp, rate in rel:
                # `--bootcache` なので起動ぶんは 0。前後 3000 サンプルで拾う
                if abs((sp % (1 << 31)) - want) < 3000:
                    hit = rate
                    break
            if i == 0 or hit is None:
                continue
            got[v] = back.get(hit)
    base = got.get(64)
    if base is None:
        # 頭の基準は chunk の 1 つ目にある
        for c0 in range(0, len(vals), a.chunk):
            pass
    print("%-5s %-6s %s" % ("つまみ", "目盛り", "ずれ"))
    b0 = got.get(64)
    for v in vals:
        g = got.get(v)
        if g is None:
            continue
        print("%-5d %-6d %s" % (v, g, ("%+d" % (g - b0)) if b0 is not None else ""))
    if b0 is not None:
        print()
        print("ずれだけ並べたもの（128 段）:")
        row = [got[v] - b0 if got.get(v) is not None else 0 for v in vals]
        for r0 in range(0, 128, 16):
            print("  %3d: %s" % (r0, " ".join("%4d" % x for x in row[r0:r0 + 16])))
    return 0


if __name__ == "__main__":
    sys.exit(main())
