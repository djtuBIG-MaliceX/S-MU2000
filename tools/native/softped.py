#!/usr/bin/env python3
# license:BSD-3-Clause
"""**ソフトペダル（CC67）**が押鍵のレジスタをどう動かすか測る。

1 回の演奏で CC67 を振っては 1 音鳴らす。実機と native を並べる。
"""
import os
import re
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
W = BUILD / 'tests' / 'softped'
W.mkdir(parents=True, exist_ok=True)
BOOT = 8.0
LINE = re.compile(r'^(N |W |R )?(00800000) ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
MASK = {0x1cf: 0, 0x1ce: 16, 0x18f: 32, 0x18e: 48}
KEYON = 0x20e

sys.path.insert(0, str(ROOT / 'tools'))
import make_test_midi as M

PROG = int(sys.argv[1]) if len(sys.argv) > 1 else 0x30
CC = int(sys.argv[2], 16) if len(sys.argv) > 2 else 0x43
VALS = [int(x) for x in sys.argv[3].split(',')] if len(sys.argv) > 3 else \
    [0, 16, 32, 48, 63, 64, 65, 80, 96, 112, 127]
REGS = [0x00, 0x04, 0x09, 0x06, 0x07, 0x08]

ev = M.head()
ev += [(1.0, bytes([0xc0, PROG]))]
t = 1.4
for v in VALS:
    ev += [(t - 0.1, bytes([0xb0, CC, v]))]
    ev += M.note(0, 60, 100, t, 0.2)
    t += 0.6
mid = W / 's.mid'
M.write(mid, [M.track(M.seq(ev))])


def run(tag, native):
    trc = W / ('%s.txt' % tag)
    env = dict(os.environ)
    env['SMU2000_NO_VOICECACHE'] = '1'
    cmd = [str(BUILD / ('render' + EXE)), ROMS, str(mid),
           str(W / 's.wav'), '%.3f' % (t + 0.5), '--boot', '%.3f' % BOOT,
           '--trace-swp', str(trc)]
    if native:
        cmd.append('--native-engine')
        env['SMU2000_NOCAL'] = '1'
    else:
        env.pop('SMU2000_NOCAL', None)
    subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL)
    base = int(round(BOOT * 44100))
    cur, mask, got = {}, 0, []
    with open(trc, errors='replace') as f:
        for line in f:
            m = LINE.match(line)
            if not m or m.group(1) == 'R ':
                continue
            reg, val, sp = int(m.group(3), 16), int(m.group(4), 16), int(m.group(5))
            if sp <= base:
                continue
            if reg == KEYON:
                g = [dict(cur[i]) for i in range(64)
                     if (mask >> i) & 1 and cur.get(i)]
                if g:
                    got.append(g[0])
                cur, mask = {}, 0
            elif reg in MASK:
                sh = MASK[reg]
                mask = (mask & ~(0xffff << sh)) | (val << sh)
            elif reg < 0x1000:
                cur.setdefault(reg // 64, {})[reg % 64] = val
    return got


a, b = run('fw', False), run('nv', True)
print('音色 %d  CC%d  （実機 %d / native %d / 期待 %d）'
      % (PROG, CC, len(a), len(b), len(VALS)))
print('%-5s %s' % ('値', ' '.join('%-13s' % ('%02x' % r) for r in REGS)))
for i, v in enumerate(VALS):
    if i >= len(a) or i >= len(b):
        break
    c = []
    for r in REGS:
        x, y = a[i].get(r, 0), b[i].get(r, 0)
        c.append('%04x/%04x%s' % (x, y, ' ' if x == y else '*'))
    print('%-5d %s' % (v, ' '.join('%-13s' % z for z in c)))
