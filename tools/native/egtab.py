#!/usr/bin/env python3
# license:BSD-3-Clause
"""**EG のつまみ（CC72/73/75）が速さの目盛りをどう動かすか、128 段ぜんぶ測る**。

写し取りを捨てる（段 4）ために残っている宿題のひとつ。CC73（立ち上がり）は
`0x06` の上位、CC75（減衰）は `0x08`、CC72（離し）は離しの速さを動かす。
つまみと目盛りの関係は線形でも対数でもなく、実機を測るのがいちばん早い。

**1 回の演奏で 128 段ぜんぶ取る**。つまみを変えては 1 音鳴らす MIDI を作り、
`--trace-svp` に残った押鍵時のレジスタを順に拾う（1 段ずつ鳴らすと
128 回起動することになって話にならない）。

  python tools/native/egtab.py 73 [--voice msb,lsb,prog] [--note N] [--vel N]
                                  [--reg 06] [--tab attack|decay]

出すのは「CC=64 のときの目盛りからのずれ」。音色によらなければ、
そのまま表として native の口に持てる。
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
WORK = BUILD / "tests" / "egtab"
EXE = ".exe" if os.name == "nt" else ""
NEEDED = ("mu2000.zip", "swp30.zip")

LINE = re.compile(r'^(N |W |R )?(00800000) ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
MASK = {0x1cf: 0, 0x1ce: 16, 0x18f: 32, 0x18e: 48}
KEYON = 0x20e
# 毎サンプル書き替わる（MEG の戻りのミキサ）ので比べない。
# **0x21-0x2b の奇数番と 0x30・0x31 も外す**。実機の firmware は
# 1 音ごとには書かないので、起動のときの残りが最初の押鍵に混ざる
SKIP = set([0x0e, 0x0f, 0x30, 0x31] + list(range(0x38, 0x40))
           + [r for r in range(0x20, 0x2c) if r & 1])
# src/xg/native_voice.h と同じ
ATTACK_TAB, DECAY_TAB = 0x1F4DB8, 0x1F4E38


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
    """XG のパラメータチェンジ（43 10 4C hh mm ll dd）"""
    data = bytes([0x43, 0x10, 0x4c]) + bytes(body)
    return bytes([0xf0]) + vlq(len(data) + 1) + data + bytes([0xf7])


def make_mid(path, cc, vals, msb, lsb, prog, note, vel, addr=None):
    tick = 480                       # 4 分音符 = 0.5 秒
    ev = [(0, bytes([0xff, 0x51, 0x03]) + (500000).to_bytes(3, 'big')),
          (240, bytes([0xb0, 0x00, msb])), (240, bytes([0xb0, 0x20, lsb])),
          (240, bytes([0xc0, prog]))]
    t = tick * 2
    for v in vals:
        # **1 段ごとに声を空ける**（オールサウンドオフ）。余韻の長い音色だと
        # 64 声が埋まって、65 段目から実機が鳴らしてくれない
        ev.append((t, bytes([0xb0, 0x78, 0x00])))
        if addr is None:
            ev.append((t + tick // 16, bytes([0xb0, cc & 0x7f, v & 0x7f])))
        else:
            # **パートの設定を SysEx で振る**（08 pp <addr> vv）。
            # CC の無いもの（ビブラートの速さ・深さなど）はこちら
            ev.append((t + tick // 16, xg_sysex([0x08, 0x00, addr, v & 0x7f])))
        ev.append((t + tick // 8, bytes([0x90, note, vel])))
        ev.append((t + tick // 4, bytes([0x80, note, 0])))
        t += tick // 2               # 0.25 秒ごと
    body = bytearray()
    prev = 0
    for tt, b in sorted(ev, key=lambda e: e[0]):
        body += vlq(tt - prev) + b
        prev = tt
    body += vlq(0) + bytes([0xff, 0x2f, 0x00])
    path.write_bytes(b'MThd' + struct.pack('>IHHH', 6, 0, 1, 480) +
                     b'MTrk' + struct.pack('>I', len(body)) + bytes(body))
    return (t + tick) / float(tick) * 0.5


def keyons(trc):
    cur = collections.defaultdict(dict)
    mask = 0
    out = []
    with open(trc, errors="replace") as f:
        for line in f:
            m = LINE.match(line)
            if not m or m.group(1) == "R ":
                continue
            reg, val = int(m.group(3), 16), int(m.group(4), 16)
            if reg in MASK:
                sh = MASK[reg]
                mask = (mask & ~(0xffff << sh)) | (val << sh)
            elif reg == KEYON:
                got = [dict(cur[i]) for i in range(64)
                       if (mask >> i) & 1 and cur.get(i)]
                if got:
                    out.append(got)
                cur = collections.defaultdict(dict)
                mask = 0
            elif reg < 0x1000 and reg % 64 not in SKIP:
                cur[reg // 64][reg % 64] = val
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cc", type=int, nargs="?", default=0)
    ap.add_argument("--roms")
    ap.add_argument("--voice", default="0,0,48")
    ap.add_argument("--note", type=int, default=60)
    ap.add_argument("--vel", type=int, default=100)
    ap.add_argument("--reg", default="06")
    ap.add_argument("--tab", default="attack", choices=("attack", "decay", "raw"))
    ap.add_argument("--chunk", type=int, default=48)
    ap.add_argument("--addr", help="CC ではなく 08 pp <addr> を振る（16 進）")
    ap.add_argument("--byte", default="hi", choices=("hi", "lo", "all"),
                    help="レジスタの上位・下位どちらを見るか")
    a = ap.parse_args()

    roms = find_roms(a.roms)
    if not roms:
        print("ROM が見つからない（--roms か SMU2000_ROMS で指す）")
        return 1
    WORK.mkdir(parents=True, exist_ok=True)
    msb, lsb, prog = (int(x) for x in a.voice.split(","))
    reg = int(a.reg, 16)
    rom = (roms / "mu2000_flash.bin").read_bytes()
    # `raw` は表を通さず、上位バイトをそのまま目盛りとして見る
    raw = a.tab == "raw"
    tab = ATTACK_TAB if a.tab == "attack" else DECAY_TAB
    back = {}
    if raw:
        back = {i: i for i in range(65536)}
    else:
        # 値 → 目盛り（同じ値が 2 つ並ぶので、いちばん小さい位置）
        for i in range(127, -1, -1):
            back[rom[tab + i]] = i

    print("音色 %d,%d,%d  鍵 %d  強さ %d  %s  レジスタ 0x%02x の%s（%s の表）"
          % (msb, lsb, prog, a.note, a.vel,
             ("CC%d" % a.cc) if a.addr is None else ("08 pp %s" % a.addr),
             reg, "上位" if a.byte == "hi" else "下位", a.tab))
    # **1 回の演奏で取れるのは 56 段まで**。実機は 64 声を使い切ると、
    # 離したあともしばらく声を返さないので、65 段目から鳴らしてくれない。
    # 少しずつに割って、どの回も頭に CC=64 を置いて基準を揃える
    vals = list(range(128))
    got = {}
    CH = a.chunk
    for c0 in range(0, len(vals), CH):
        chunk = [64] + vals[c0:c0 + CH]
        mid = WORK / "eg.mid"
        secs = make_mid(mid, a.cc, chunk, msb, lsb, prog, a.note, a.vel,
                        None if a.addr is None else int(a.addr, 16))
        trc = WORK / "eg.txt"
        env = dict(os.environ)
        env["SMU2000_NO_VOICECACHE"] = "1"
        r = subprocess.run([str(BUILD / ("render" + EXE)), str(roms), str(mid),
                            str(WORK / "eg.wav"), "%.3f" % secs, "--bootcache",
                            "--trace-swp", str(trc)],
                           env=env, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
        if r.returncode != 0:
            print("鳴らせなかった")
            return 1
        ko = keyons(trc)
        if len(ko) < len(chunk):
            print("%d 段目から: 押鍵 %d 回しか取れなかった（%d 段ぶん要る）"
                  % (c0, len(ko), len(chunk)))
        for i, v in enumerate(chunk):
            if i >= len(ko):
                break
            raw16 = ko[i][0].get(reg, 0)
            hi = ((raw16 >> 8) & 0xff if a.byte == "hi"
                  else (raw16 & 0xff if a.byte == "lo" else raw16))
            if i == 0:
                continue             # 頭の基準（CC=64）は読み飛ばす
            got[v] = back.get(hi)
        if 64 not in got and len(ko):
            r0 = ko[0][0].get(reg, 0)
            got[64] = back.get(((r0 >> 8) & 0xff if a.byte == "hi"
                                else (r0 & 0xff if a.byte == "lo" else r0)))
    base = got.get(64)
    if base is None:
        print("CC=64 のときの目盛りが引けなかった")
        return 1
    print("CC=64 の目盛り %d（レジスタ %02x）。以下はそこからのずれ"
          % (base, base if raw else rom[tab + base]))
    print("%-5s %-5s %-7s %s" % ("CC", "上位", "目盛り", "ずれ"))
    for v in vals:
        g = got.get(v)
        if g is None:
            continue
        print("%-5d %02x    %-7d %+d" % (v, g if raw else rom[tab + g], g, g - base))
    print()
    print("ずれだけ並べたもの（128 段）:")
    row = [got[v] - base if got.get(v) is not None else 0 for v in vals]
    for r0 in range(0, 128, 16):
        print("  %3d: %s" % (r0, " ".join("%4d" % x for x in row[r0:r0 + 16])))
    return 0


if __name__ == "__main__":
    sys.exit(main())
