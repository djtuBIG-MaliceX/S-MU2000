#!/usr/bin/env python3
"""**ドラムの音量（NRPN 16）がレジスタをどう動かすか**を 128 段測る。

1 回の演奏で音量を変えては 1 打鳴らす。押鍵のときの 0x06・0x09 を拾う。
"""
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(r'C:\Users\gugug\GitHub\S-MU2000')
W = Path(__file__).resolve().parent / 'dl'
W.mkdir(parents=True, exist_ok=True)
ROMS = r'C:\Users\gugug\GitHub\MU2000\roms'
BOOT = 8.0
LINE = re.compile(r'^(N |W |R )?(00800000) ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
KEYON = 0x20e

sys.path.insert(0, str(ROOT / 'tools'))
import make_test_midi as M

NOTE = int(sys.argv[1]) if len(sys.argv) > 1 else 36
VALS = [int(x) for x in sys.argv[2].split(',')] if len(sys.argv) > 2 else \
    list(range(0, 128, 4)) + [127]
MSB = int(sys.argv[3], 16) if len(sys.argv) > 3 else 0x16


def nr(msb, note, val):
    return [b'\xb9\x63' + bytes([msb]), b'\xb9\x62' + bytes([note]),
            b'\xb9\x06' + bytes([val])]


ev = M.head()
t = 1.0
for v in VALS:
    ev += M.spread(t, nr(MSB, NOTE, v))
    ev += M.note(9, NOTE, 110, t + 0.2, 0.08)
    t += 0.5
mid = W / 'd.mid'
M.write(mid, [M.track(M.seq(ev))])
def render(tag, native):
    trc = W / ('%s.txt' % tag)
    env = dict(os.environ)
    env['SMU2000_NO_VOICECACHE'] = '1'
    cmd = [str(ROOT / 'build' / 'render.exe'), ROMS, str(mid),
           str(W / 'd.wav'), '%.3f' % (t + 0.5), '--boot', '%.3f' % BOOT,
           '--trace-swp', str(trc)]
    if native:
        cmd.append('--native-engine')
        env['SMU2000_NOCAL'] = '1'
    else:
        env.pop('SMU2000_NOCAL', None)
    subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL)
    return trc


base = int(round(BOOT * 44100))
def scan(trc):
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
            g = [dict(cur[i]) for i in range(64) if (mask >> i) & 1 and cur.get(i)]
            if g:
                got.append(g[0])
            cur, mask = {}, 0
        elif reg in (0x1cf, 0x1ce, 0x18f, 0x18e):
            sh = {0x1cf: 0, 0x1ce: 16, 0x18f: 32, 0x18e: 48}[reg]
            mask = (mask & ~(0xffff << sh)) | (val << sh)
        elif reg < 0x1000:
            cur.setdefault(reg // 64, {})[reg % 64] = val
  return got


a = scan(render('fw', False))
b = scan(render('nv', True))
print('鍵 %d  NRPN %02x  （実機 %d / native %d / 期待 %d）'
      % (NOTE, MSB, len(a), len(b), len(VALS)))
REGS = [int(x, 16) for x in (sys.argv[4] if len(sys.argv) > 4 else '06,09,00,11').split(',')]
print('%-5s %s' % ('値', ' '.join('%-13s' % ('0x%02x' % r) for r in REGS)))
for i, v in enumerate(VALS):
    if i >= len(a) or i >= len(b):
        break
    c = []
    for r in REGS:
        x, y = a[i].get(r, 0), b[i].get(r, 0)
        c.append('%04x/%04x%s' % (x, y, ' ' if x == y else '*'))
    print('%-5d %s' % (v, ' '.join('%-13s' % z for z in c)))
