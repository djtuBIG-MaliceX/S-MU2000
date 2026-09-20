#!/usr/bin/env python3
# license:BSD-3-Clause
"""CC73 で**フィルタ包絡線の立ち上がりの速さ**がどう変わるか測る。
10ms ごとの `0x00` の伸びを見て、増分の表（0x1E5C58）から目盛りを引き戻す。"""
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "build"
EXE = ".exe" if os.name == "nt" else ""
NEEDED = ("mu2000.zip", "swp30.zip")


def find_roms(given=None):
    cands = [Path(given)] if given else []
    if os.environ.get("SMU2000_ROMS"):
        cands.append(Path(os.environ["SMU2000_ROMS"]))
    cands += [ROOT / "roms", ROOT.parent / "MU2000" / "roms"]
    for c in cands:
        if all((c / n).exists() for n in NEEDED):
            return c
    raise SystemExit("ROM が見つからない（SMU2000_ROMS で指す）")


ROMS = str(find_roms())

W = BUILD / 'tests' / 'fenvrate'
W.mkdir(parents=True, exist_ok=True)
LINE = re.compile(r'^(N |W |R )?(00800000) ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
INC = 0x1E5C58

rom = (Path(ROMS) / 'mu2000_flash.bin').read_bytes()
tab = [struct.unpack('>h', rom[INC + i * 2:INC + i * 2 + 2])[0] for i in range(64)]

prog = int(sys.argv[1]) if len(sys.argv) > 1 else 48
CCS = [int(x) for x in (sys.argv[2] if len(sys.argv) > 2 else
                        '65,66,68,70,72,76,80,88,96,104,112,120,127').split(',')]
NOTE = int(sys.argv[3]) if len(sys.argv) > 3 else 60


def vlq(n):
    out = [n & 0x7f]
    n >>= 7
    while n:
        out.append((n & 0x7f) | 0x80)
        n >>= 7
    return bytes(reversed(out))


tick = 480
ev = [(0, bytes([0xff, 0x51, 0x03]) + (500000).to_bytes(3, 'big'))]
t = tick * 2
spots = []
for c in CCS:
    ev.append((t, bytes([0xb0, 0x78, 0x00])))
    ev.append((t + 2, bytes([0xc0, prog])))
    ev.append((t + 6, bytes([0xb0, 0x49, c])))
    ev.append((t + tick // 8, bytes([0x90, NOTE, 100])))
    ev.append((t + tick - tick // 8, bytes([0x80, NOTE, 0])))
    spots.append((c, (t + tick // 8) / float(tick) * 0.5))
    t += tick
body = bytearray()
prev = 0
for tt, b in sorted(ev, key=lambda e: e[0]):
    body += vlq(tt - prev) + b
    prev = tt
body += vlq(0) + bytes([0xff, 0x2f, 0x00])
mid = W / 'f.mid'
mid.write_bytes(b'MThd' + struct.pack('>IHHH', 6, 0, 1, 480) +
                b'MTrk' + struct.pack('>I', len(body)) + bytes(body))
secs = (t + tick) / float(tick) * 0.5

trc = W / 'f.txt'
env = dict(os.environ)
env['SMU2000_NO_VOICECACHE'] = '1'
env.pop('SMU2000_NOCAL', None)
subprocess.run([str(BUILD / ('render' + EXE)), ROMS, str(mid),
                str(W / 'f.wav'), '%.3f' % secs, '--bootcache',
                '--trace-swp', str(trc)],
               env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

wr = []
with open(trc, errors='replace') as f:
    for line in f:
        m = LINE.match(line)
        if not m or m.group(1) == 'R ':
            continue
        r, val, s = int(m.group(3), 16), int(m.group(4), 16), int(m.group(5))
        if r < 0x1000 and r % 64 == 0:
            wr.append((s / 44100.0, r // 64, val & 0xfff))

print('%-5s %-8s %-8s %-6s %s' % ('つまみ', '初め', '伸び/10ms', '増分', '目盛り'))
for c, at in spots:
    seq = [(t2, sl, v) for t2, sl, v in wr if at - 0.002 <= t2 <= at + 0.09]
    if len(seq) < 3:
        print('%-5d 取れず' % c)
        continue
    sl = seq[0][1]
    seq = [x for x in seq if x[1] == sl]
    d = [seq[i + 1][2] - seq[i][2] for i in range(len(seq) - 1)]
    step = max(set(d), key=d.count) if d else 0
    inc = step * 4
    idx = [i for i, x in enumerate(tab) if x == inc]
    print('%-5d %-8s %-8d %-6d %s' % (c, '%03x' % seq[0][2], step, inc,
                                      idx[0] if idx else '?'))
