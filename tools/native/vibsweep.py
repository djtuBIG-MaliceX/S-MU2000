#!/usr/bin/env python3
# license:BSD-3-Clause
"""**ビブラートのせり上がり**を、たくさんの音色でまとめて測る。

`0x0a` の下位の動きから、実機の内部カウンタを表で逆引きして
（表C[表B[カウンタ]] なので一意）、遅れ・1 歩・止まる所を出す。
1 回の演奏で何音色も鳴らすので速い。
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
W = BUILD / 'tests' / 'vibsweep'
W.mkdir(parents=True, exist_ok=True)
BOOT = 8.0
LINE = re.compile(r'^(N |W |R )?(00800000) ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
KEYON = 0x20e
TICK = 0.020

sys.path.insert(0, str(ROOT / 'tools'))
import make_test_midi as M

rom = (Path(ROMS) / 'mu2000_flash.bin').read_bytes()
VS, VT, GX, GL = 0x200ee0, 0x267f50, 0x283950, 0x2839d0
TAB_B = [rom[0x1E63F0 + i] for i in range(128)]
TAB_C = [rom[0x1E6596 + i] for i in range(128)]
# 表C[表B[c]] -> c の逆引き（重なったら小さい方）
BACK = {}
for c in range(63, -1, -1):
    BACK[TAB_C[TAB_B[c]]] = c


def elem(prog, note=60, vel=100):
    kind = rom[GX]
    group = rom[GL] if kind == 0 else kind
    slot = VT + group * 512 + prog * 4
    off = (rom[slot] << 24) | (rom[slot + 1] << 16) | (rom[slot + 2] << 8) | rom[slot + 3]
    rec = VS + off * 2
    n = bin(rom[rec] & 15).count('1')
    out = [rom[rec + 12 + i * 84: rec + 12 + (i + 1) * 84] for i in range(n)]
    return [e for e in out if e[4] <= note <= e[5] and e[6] <= vel <= e[7]]


PROGS = [int(x) for x in sys.argv[1].split(',')] if len(sys.argv) > 1 else \
    [p for p in range(128)]
# 揺れを持つものだけ（byte12 か byte14 が 0 でない）
PICK = []
for p in PROGS:
    el = elem(p)
    if el and (el[0][12] or el[0][14]):
        PICK.append(p)

STEP = 3.0          # 1 音色あたりの間
ev = M.head()
at = 1.0
spots = []
for p in PICK:
    ev.append((at, bytes([0xb0, 0x78, 0x00])))
    ev.append((at + 0.05, bytes([0xc0, p])))
    ev += M.note(0, 60, 100, at + 0.2, 2.5)
    spots.append((p, at + 0.2))
    at += STEP
mid = W / 'sweep.mid'
M.write(mid, [M.track(M.seq(ev))])
secs = at + 1.0

trc = W / 'sweep.txt'
env = dict(os.environ)
env['SMU2000_NO_VOICECACHE'] = '1'
env.pop('SMU2000_NOCAL', None)
subprocess.run([str(BUILD / ('render' + EXE)), ROMS, str(mid),
                str(W / 'sweep.wav'), '%.3f' % secs, '--boot', '%.3f' % BOOT,
                '--trace-swp', str(trc)],
               env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

base = int(round(BOOT * 44100))
ons, writes = [], []
with open(trc, errors='replace') as f:
    for line in f:
        m = LINE.match(line)
        if not m or m.group(1) == 'R ':
            continue
        reg, val, sp = int(m.group(3), 16), int(m.group(4), 16), int(m.group(5))
        t = (sp - base) / 44100.0
        if reg == KEYON:
            ons.append(t)
        elif reg < 0x1000 and reg % 64 == 0x0a:
            writes.append((t, reg // 64, val & 0xff))

print('%-4s %-16s %-26s %s'
      % ('音色', '記録', '実機（遅れ・歩・止）', '式の当て'))
for p, at in spots:
    e = elem(p)[0]
    on = next((t for t in ons if t >= at - 0.05), None)
    if on is None:
        continue
    seq, last, slot = [], None, None
    for t, sl, v in writes:
        if not (on <= t <= on + 2.4):
            continue
        if slot is None:
            slot = sl
        if sl != slot:
            continue
        if v != last:
            seq.append((t - on, BACK.get(v, -1)))
            last = v
    if len(seq) < 2:
        print('%-4d b12=%-3d b13=%-3d b14=%-3d  揺れなし' % (p, e[12], e[13], e[14]))
        continue
    t0 = seq[1][0]
    cs = [c for _, c in seq[1:]]
    step = cs[0] - seq[0][1] if len(cs) else 0
    tgt = cs[-1]
    # 遅れ（目盛り）= t0 を 20ms で割った数から、押鍵〜最初の目までを引く
    dly = round((t0 - 0.095) / TICK)
    exp = (3 * e[12] // 4 + 3) if e[12] else 0
    print('%-4d b12=%-3d b13=%-3d b14=%-3d  遅れ%-3d 歩%-2d 止%-3d  | 式の遅れ %-3d%s'
          % (p, e[12], e[13], e[14], dly, step, tgt, exp,
             '' if dly == exp else ' *'))
