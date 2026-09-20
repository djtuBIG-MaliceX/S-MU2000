#!/usr/bin/env python3
# license:BSD-3-Clause
"""ズラしかた 3 通りを実機で比べる。

  python tools/native/shift4.py [音色] [ずらす半音] [鍵]

  a 粗調       RPN 2
  b ずらした先の鍵をそのまま押したもの
  c 素の鍵
  d ノートシフト  08 pp 08
  e 移調         00 00 06

違ったレジスタだけ並べる。a・d・e が c と同じなら「鍵の曲線は
押した鍵で引いている」、b と同じなら「ずらした先で引いている」（6.172）
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

W = BUILD / 'tests' / 'shift4'
W.mkdir(parents=True, exist_ok=True)
LINE = re.compile(r'^(N |W |R )?(00800000) ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
MASK = {0x1cf: 0, 0x1ce: 16, 0x18f: 32, 0x18e: 48}
KEYON = 0x20e
SKIP = set([0x0e, 0x0f, 0x30, 0x31] + list(range(0x38, 0x40))
           + [r for r in range(0x20, 0x2c) if r & 1])

prog = int(sys.argv[1]) if len(sys.argv) > 1 else 0x50
SH = int(sys.argv[2]) if len(sys.argv) > 2 else -24   # ずらす半音
KEY = int(sys.argv[3]) if len(sys.argv) > 3 else 60

sys.path.insert(0, str(ROOT / 'tools'))
import make_test_midi as M


def rpn2(v):
    return [bytes([0xb0, 0x65, 0]), bytes([0xb0, 0x64, 2]), bytes([0xb0, 0x06, v])]


ev = M.head()
ev += [(1.0, bytes([0xc0, prog]))]
plan = [('a 粗調', KEY, rpn2(64 + SH)),
        ('b 鍵%d' % (KEY + SH), KEY + SH, rpn2(64)),
        ('c 鍵%d' % KEY, KEY, rpn2(64)),
        ('d ノートシフト', KEY, [M.xg([0x08, 0x00, 0x08, 64 + SH])]),
        ('e 移調', KEY, [M.xg([0x00, 0x00, 0x06, 64 + SH])])]
undo = {3: [M.xg([0x08, 0x00, 0x08, 64])], 4: [M.xg([0x00, 0x00, 0x06, 64])]}
t = 1.5
for i, (_, note, pre) in enumerate(plan):
    for j, m in enumerate(pre):
        ev += [(t - 0.2 + j * 0.02, m)]
    ev += M.note(0, note, 100, t, 0.2)
    for j, m in enumerate(undo.get(i, [])):
        ev += [(t + 0.3 + j * 0.02, m)]
    t += 1.0
mid = W / 's.mid'
M.write(mid, [M.track(M.seq(ev))])
secs = t + 1.0

got = []
trc = W / 'f.txt'
env = dict(os.environ)
env['SMU2000_NO_VOICECACHE'] = '1'
env.pop('SMU2000_NOCAL', None)
subprocess.run([str(BUILD / ('render' + EXE)), ROMS, str(mid),
                str(W / 's.wav'), '%.3f' % secs, '--boot', '8.0',
                '--trace-swp', str(trc)],
               env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
cur, mask = {}, 0
with open(trc, errors='replace') as f:
    for line in f:
        m = LINE.match(line)
        if not m or m.group(1) == 'R ':
            continue
        r, val = int(m.group(3), 16), int(m.group(4), 16)
        if r in MASK:
            sh = MASK[r]
            mask = (mask & ~(0xffff << sh)) | (val << sh)
        elif r == KEYON:
            g = [dict(cur[i]) for i in range(64)
                 if (mask >> i) & 1 and cur.get(i)]
            if g:
                got.append(sorted(g, key=lambda d: (d.get(0x16, 0), d.get(0x17, 0))))
            cur, mask = {}, 0
        elif r < 0x1000 and r % 64 not in SKIP:
            cur.setdefault(r // 64, {})[r % 64] = val

print('押鍵 %d 回（期待 %d）' % (len(got), len(plan)))
if len(got) < len(plan):
    sys.exit(0)
ne = min(len(x) for x in got[:len(plan)])
for j in range(ne):
    print('-- 要素 %d' % j)
    keys = sorted(set().union(*[set(g[j]) for g in got[:len(plan)]]))
    print('%-5s %s' % ('reg', ' '.join('%-9s' % p[0][:8] for p in plan)))
    for k in keys:
        v = [g[j].get(k) for g in got[:len(plan)]]
        if len(set(v)) > 1:
            print('%-5s %s' % ('%02x' % k,
                               ' '.join('%-9s' % ('%04x' % x if x is not None
                                                  else '----') for x in v)))
