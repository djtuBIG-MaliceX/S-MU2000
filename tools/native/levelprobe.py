#!/usr/bin/env python3
# license:BSD-3-Clause
"""**実機が「掛ける前の音量の目盛り」に何を入れるか、音色ごとに読む**。

目盛りは実機のボイスの塊（`0x424364 + 0x94 × スロット + 118`）にそのまま
入っている（doc/native-engine.md の 6.101・6.113）。レジスタ `0x09` から
逆に引くと表の平らなところで幅が出るが、ここを読めば**そのものの値**が取れる。

  python tools/native/levelprobe.py [--roms DIR] [--note N] [--vel N]
                                    [--lo N] [--hi N]

1 回の演奏で音色を順に切り替えて、そのたびにワーク RAM を写す。
写しの差分で「その音色が使ったスロット」が分かるので、要素ごとの目盛りが
そのまま並ぶ。式（`voice_raw_level`）と突き合わせて出す。
"""
import argparse
import os
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "build"
WORK = BUILD / "tests" / "levelprobe"
EXE = ".exe" if os.name == "nt" else ""
NEEDED = ("mu2000.zip", "swp30.zip")
VB, VSTRIDE, VLEVEL = 0x424364, 0x94, 118
PARTS, PSTRIDE = 0x28d64, 0x134
PART_VOICE = 0xf8                      # src/xg/ram.h の PART_VOICE
LEVEL_CURVE = None                      # 下で src から読む


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


def make_mid(path, progs, note, vel, step, msb=0, lsb=0):
    tick = 480
    ev = [(0, bytes([0xff, 0x51, 0x03]) + (500000).to_bytes(3, 'big'))]
    t = tick
    for p in progs:
        ev.append((t, bytes([0xb0, 0x78, 0x00])))          # 声を空ける
        ev.append((t + 4, bytes([0xb0, 0x00, msb & 0x7f])))
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
    ap.add_argument("--note", type=int, default=60)
    ap.add_argument("--vel", type=int, default=100)
    ap.add_argument("--lo", type=int, default=0)
    ap.add_argument("--hi", type=int, default=127)
    ap.add_argument("--msb", type=int, default=0)
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
    step = 240                            # 0.25 秒
    mid = WORK / "lv.mid"
    tick0, tend = make_mid(mid, progs, a.note, a.vel, step, a.msb, a.lsb)
    sec = lambda ticks: ticks / float(480) * 0.5
    env = dict(os.environ)
    env["SMU2000_NO_VOICECACHE"] = "1"
    env["SMU2000_RAMSNAP"] = str(WORK)
    env["SMU2000_RAMSNAP_T0"] = "%.4f" % sec(tick0 + step // 2)
    env["SMU2000_RAMSNAP_DT"] = "%.4f" % sec(step)
    env["SMU2000_RAMSNAP_N"] = str(len(progs))
    r = subprocess.run([str(BUILD / ("render" + EXE)), str(roms), str(mid),
                        str(WORK / "lv.wav"), "%.3f" % sec(tend + 480),
                        "--boot", "8.0"],
                       env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if r.returncode != 0:
        print("鳴らせなかった")
        return 1

    rom = (roms / "mu2000_flash.bin").read_bytes()
    import re
    h = (ROOT / "src" / "xg" / "native_voice.h").read_text(encoding="utf-8")
    lc = int(re.search(r'LEVEL_CURVE\s*=\s*(0x[0-9a-fA-F]+)', h).group(1), 16)

    def curve(e):
        """音量の鍵による増減（level_key_curve × 2 を 1 バイトに詰めたもの）"""
        if rom[e + 60] != 0xff:
            return 0
        idx = (rom[e + 66] << 8) | rom[e + 67]
        ad = lc + idx * 128 + (a.note & 0x7f)
        if ad >= 0x400000:
            return 0
        v = rom[ad]
        v = v - 256 if v > 127 else v
        w = (v * 2) & 0xff
        return w - 256 if w > 127 else w

    print("鍵 %d  強さ %d。実機のボイスの塊 +118 と式を突き合わせる" % (a.note, a.vel))
    print("%-5s %-9s %-4s %-5s %-6s %-6s %-6s %s"
          % ("音色", "記録", "要素", "b59", "実機", "切捨", "四捨五入", "判定"))
    bad_tr = bad_rn = tot = 0
    prev = None
    for i, p in enumerate(progs):
        f = WORK / ("ram%03d.bin" % i)
        if not f.exists():
            continue
        ram = f.read_bytes()
        lv = [ram[VB + s * VSTRIDE + VLEVEL - 0x400000] for s in range(64)]
        used = [s for s in range(64) if lv[s] and (prev is None or lv[s] != prev[s])]
        prev = lv
        b = part_base(0)
        rec = int.from_bytes(ram[b + PART_VOICE: b + PART_VOICE + 4], "big")
        if not rec:
            continue
        nel = sum(1 for k in range(4) if rom[rec] & (1 << k))
        for k in range(min(nel, len(used))):
            e = rec + 12 + k * 84
            b59, r1 = rom[e + 59], rom[rec + 1]
            cv = curve(e)
            tr = max(0, min(128, (r1 * b59) // 99 + cv))
            rn = max(0, min(128, (r1 * b59 + 49) // 99 + cv))
            fw = lv[used[k]]
            tot += 1
            if fw != tr:
                bad_tr += 1
            if fw != rn:
                bad_rn += 1
            if tr != rn or fw not in (tr, rn):
                print("%-5d %06x    %-4d %-5d %-6d %-6d %-6d %s"
                      % (p, rec, k, b59, fw, tr, rn,
                         "切捨" if fw == tr else ("四捨五入" if fw == rn else "どちらも違う")))
    print()
    print("合わなかった数: 切り捨て %d / 四捨五入 %d（%d 件中）"
          % (bad_tr, bad_rn, tot))
    return 0


if __name__ == "__main__":
    sys.exit(main())
