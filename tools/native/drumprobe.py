#!/usr/bin/env python3
# license:BSD-3-Clause
"""**ドラムの 1 打ごとに、実機がスロットへ書いた値を並べる**。

`0x09`（音量）・`0x32`（パン）・`0x33`（リバーブ送り）は、写し取りを
捨てる（段 4）ための最後の宿題だった。式を当てるには「打ごとの値」と
「その打のドラムセットアップ・ROM の記録」を並べて見るのがいちばん早い。

  python tools/native/drumprobe.py [--roms DIR] [--kit N] [--vel N]
                                   [--lo N] [--hi N] [--csv ファイル]

キットは `--kit` で選ぶ（省略すると曲の既定のまま）。1 打ずつ 0.25 秒
おきに叩いて、`--trace-swp` に残った押鍵時のレジスタを拾う。あわせて
ワーク RAM のドラムセットアップ（`3n rr pp`）と ROM の記録（42 バイト）も
読んで、同じ行に並べる。
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
WORK = BUILD / "tests" / "drumprobe"
EXE = ".exe" if os.name == "nt" else ""
BOOT = 8.0
NEEDED = ("mu2000.zip", "swp30.zip")

LINE = re.compile(r'^(N |W |R )?(00800000) ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
MASK = {0x1cf: 0, 0x1ce: 16, 0x18f: 32, 0x18e: 48}
KEYON = 0x20e
# 毎サンプル書き替わる（MEG の戻りのミキサ）ので比べない。
# **0x21-0x2b の奇数番と 0x30・0x31 も外す**。実機の firmware は
# 1 音ごとには書かないので、起動のときの残りが最初の押鍵に混ざる
SKIP = set([0x0e, 0x0f, 0x30, 0x31] + list(range(0x38, 0x40))
           + [r for r in range(0x20, 0x2c) if r & 1])

# ワーク RAM の並び（src/xg/ram.h と同じ）
PARTS, STRIDE = 0x28d64, 0x134
DS_BASE, DS_PARAM, DS_NOTES, DS_NOTE0 = 0x226e1, 23, 79, 13
PART_KIT, PART_MODE = 0x110, 0x07
# ROM の並び（src/xg/native_voice.h と同じ）
DRUM_KIT_TABLE, DRUM_RECORDS = 0x292250, 0x283dd0


def part_base(p):
    port, k = p // 16, p % 16
    slot = 0 if k == 9 else (k + 1 if k < 9 else k)
    return PARTS + (port * 16 + slot) * STRIDE


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


def xg(body):
    data = bytes([0x43, 0x10, 0x4c]) + bytes(body)
    return b'\xf0' + vlq(len(data) + 1) + data + b'\xf7'


def make_mid(path, notes, vel, kit):
    """0.25 秒おきに 1 打ずつ。XG On のあと、要ればキットを選ぶ"""
    ev = [(0, bytes([0xff, 0x51, 0x03]) + (500000).to_bytes(3, 'big')),
          (0, xg([0x00, 0x00, 0x7e, 0x00]))]
    tick = 480                      # 4 分音符 = 0.5 秒
    t = tick * 2                    # XG On のあと 1 秒あける
    if kit is not None:
        ev += [(t, bytes([0xb9, 0x00, 127])), (t, bytes([0xb9, 0x20, 0])),
               (t, bytes([0xc9, kit & 0x7f]))]
        t += tick
    for n in notes:
        ev += [(t, bytes([0x99, n, vel])), (t + tick // 4, bytes([0x89, n, 0]))]
        t += tick // 2              # 0.25 秒
    body = bytearray()
    prev = 0
    for tt, b in sorted(ev, key=lambda e: e[0]):
        body += vlq(tt - prev) + b
        prev = tt
    body += vlq(0) + bytes([0xff, 0x2f, 0x00])
    path.write_bytes(b'MThd' + struct.pack('>IHHH', 6, 0, 1, 480) +
                     b'MTrk' + struct.pack('>I', len(body)) + bytes(body))
    return (t + tick * 2) / float(tick) * 0.5


def keyons(trc):
    cur = collections.defaultdict(dict)
    mask = 0
    out = []
    with open(trc, errors="replace") as f:
        for line in f:
            m = LINE.match(line)
            if not m or m.group(1) == "R ":
                continue
            reg, val, s = int(m.group(3), 16), int(m.group(4), 16), int(m.group(5))
            if reg in MASK:
                sh = MASK[reg]
                mask = (mask & ~(0xffff << sh)) | (val << sh)
            elif reg == KEYON:
                got = [dict(cur[i]) for i in range(64)
                       if (mask >> i) & 1 and cur.get(i)]
                if got:
                    out.append((s, got))
                cur = collections.defaultdict(dict)
                mask = 0
            elif reg < 0x1000 and reg % 64 not in SKIP:
                cur[reg // 64][reg % 64] = val
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--roms")
    ap.add_argument("--kit", type=int, default=None)
    ap.add_argument("--vel", type=int, default=100)
    ap.add_argument("--lo", type=int, default=25)
    ap.add_argument("--hi", type=int, default=81)
    ap.add_argument("--csv")
    a = ap.parse_args()

    roms = find_roms(a.roms)
    if not roms:
        print("ROM が見つからない（--roms か SMU2000_ROMS で指す）")
        return 1
    WORK.mkdir(parents=True, exist_ok=True)
    notes = list(range(a.lo, a.hi + 1))
    mid = WORK / "drumprobe.mid"
    secs = make_mid(mid, notes, a.vel, a.kit)

    trc = WORK / "fw.txt"
    for old in WORK.glob("ram*.bin"):
        old.unlink()
    env = dict(os.environ)
    env["SMU2000_NO_VOICECACHE"] = "1"
    env["SMU2000_RAMSNAP"] = str(WORK)
    # **1 打ごとに写す**。実機のボイスの塊 +118（掛ける前の音量の目盛り）を
    # 読めば、減衰の表の平らなところに埋もれずに値そのものが見える
    env["SMU2000_RAMSNAP_T0"] = "%.4f" % (1.0 + 0.125 + 0.06)
    env["SMU2000_RAMSNAP_DT"] = "0.25"
    env["SMU2000_RAMSNAP_N"] = str(len(notes) + 1)
    r = subprocess.run([str(BUILD / ("render" + EXE)), str(roms), str(mid),
                        str(WORK / "fw.wav"), "%.3f" % secs,
                        "--boot", "%.3f" % BOOT, "--trace-swp", str(trc)],
                       env=env, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
    if r.returncode != 0:
        print("鳴らせなかった")
        return 1

    ram = (WORK / ("ram%03d.bin" % len(notes))).read_bytes()
    if not (WORK / ("ram%03d.bin" % len(notes))).exists():
        ram = (WORK / "ram000.bin").read_bytes()
    rom = (roms / "mu2000_flash.bin").read_bytes()
    kit = ram[part_base(9) + PART_KIT]
    mode = ram[part_base(9) + PART_MODE]
    dset = mode - 2 if mode >= 2 else 0
    print("キット %d（パートモード %d ＝ 組 %d）、強さ %d" % (kit, mode, dset, a.vel))

    def setup(n, p):
        if n < DS_NOTE0 or n >= DS_NOTE0 + DS_NOTES:
            return -1
        return ram[DS_BASE + dset * DS_PARAM * DS_NOTES
                   + (n - DS_NOTE0) * DS_PARAM + p]

    def record(n):
        base = int.from_bytes(rom[DRUM_KIT_TABLE + (kit & 0x7f) * 4:][:4], "big")
        if base < 0x200000 or base > 0x2ffff0:
            return None
        off = int.from_bytes(rom[base + n * 2:][:2], "big")
        if off == 0xffff:
            return None
        return rom[DRUM_RECORDS + off: DRUM_RECORDS + off + 42]

    # 1 打ごとの写しから「実機が使った目盛り」を拾う
    VB, VSTRIDE, VLEVEL = 0x424364, 0x94, 118
    fwlvl = {}
    prevlv = None
    for i, n in enumerate(notes):
        f = WORK / ("ram%03d.bin" % i)
        if not f.exists():
            continue
        r2 = f.read_bytes()
        lv = [r2[VB + sl * VSTRIDE + VLEVEL - 0x400000] for sl in range(64)]
        used = [sl for sl in range(64) if lv[sl] and (prevlv is None or lv[sl] != prevlv[sl])]
        prevlv = lv
        if used:
            fwlvl[n] = lv[used[0]]

    ko = keyons(trc)
    rows = []
    # 打った順に並ぶ（1 打 1 スロット）
    for i, n in enumerate(notes):
        if i >= len(ko):
            break
        regs = ko[i][1][0]
        rec = record(n)
        rows.append({
            "note": n,
            "lvl": setup(n, 0x02), "pan": setup(n, 0x04),
            "rev": setup(n, 0x05), "cho": setup(n, 0x06),
            "r09": regs.get(0x09, -1), "r32": regs.get(0x32, -1),
            "r10": regs.get(0x10, -1), "r0b": regs.get(0x0b, -1),
            "r05": regs.get(0x05, -1), "r03": regs.get(0x03, -1),
            "r33": regs.get(0x33, -1), "r34": regs.get(0x34, -1),
            "wlvl": rec[26] if rec else -1,
            "fwlvl": fwlvl.get(n, -1),
            "rec": rec.hex() if rec else "",
        })

    print("%-4s %4s %4s %4s | %6s %6s %6s | %5s %6s %6s" %
          ("鍵", "音量", "パン", "送り", "0x09", "0x32", "0x10", "0x0b", "0x05", "0x03"))
    for r in rows:
        print("%-4d %4d %4d %4d |   %04x   %04x |  %04x   %04x   %04x   %04x"
              % (r["note"], r["lvl"], r["pan"], r["rev"],
                 r["r09"], r["r32"], r["r10"], r["r0b"], r["r05"], r["r03"]))
    if a.csv:
        import csv
        with open(a.csv, "w", newline="") as f:
            w = csv.DictWriter(f, fieldnames=list(rows[0]))
            w.writeheader()
            w.writerows(rows)
        print("書いた: %s" % a.csv)
    return 0


if __name__ == "__main__":
    sys.exit(main())
