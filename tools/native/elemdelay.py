#!/usr/bin/env python3
# license:BSD-3-Clause
"""**要素を遅らせて鳴らす段**（byte72）の実際のサンプル数を測る。

byte72 が 0 でない音色を鳴らして、1 つ目の押鍵から 2 つ目の押鍵までの
サンプル数を、実機と native で並べる。
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
W = BUILD / 'tests' / 'elemdelay'
W.mkdir(parents=True, exist_ok=True)
BOOT = 8.0
LINE = re.compile(r'^(N |W |R )?(00800000) ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
KEYON = 0x20e

sys.path.insert(0, str(ROOT / 'tools'))
import make_test_midi as M

rom = (Path(ROMS) / 'mu2000_flash.bin').read_bytes()
VS, VT, GX, GL = 0x200ee0, 0x267f50, 0x283950, 0x2839d0


def els(prog):
    kind = rom[GX]
    group = rom[GL] if kind == 0 else kind
    slot = VT + group * 512 + prog * 4
    off = (rom[slot] << 24) | (rom[slot + 1] << 16) | (rom[slot + 2] << 8) | rom[slot + 3]
    rec = VS + off * 2
    n = bin(rom[rec] & 15).count('1')
    return [rom[rec + 12 + i * 84: rec + 12 + (i + 1) * 84] for i in range(n)]


PROGS = [int(x) for x in (sys.argv[1] if len(sys.argv) > 1 else '10,50,51,60').split(',')]
NOTES = [int(x) for x in (sys.argv[2] if len(sys.argv) > 2 else '60').split(',')]


def run(mid, native, secs):
    trc = W / ('%s.txt' % ('nv' if native else 'fw'))
    env = dict(os.environ)
    env['SMU2000_NO_VOICECACHE'] = '1'
    cmd = [str(BUILD / ('render' + EXE)), ROMS, str(mid),
           str(W / 'x.wav'), '%.3f' % secs, '--boot', '%.3f' % BOOT,
           '--trace-swp', str(trc)]
    if native:
        cmd.append('--native-engine')
        env['SMU2000_NOCAL'] = '1'
    else:
        env.pop('SMU2000_NOCAL', None)
    subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL)
    base = int(round(BOOT * 44100))
    out = []
    with open(trc, errors='replace') as f:
        for line in f:
            m = LINE.match(line)
            if not m or m.group(1) == 'R ':
                continue
            if int(m.group(3), 16) == KEYON:
                out.append(int(m.group(5)) - base)
    return out


print('%-5s %-5s %-7s %10s %10s %8s'
      % ('音色', '鍵', 'byte72', '実機', 'native', '式'))
for prog in PROGS:
    e = els(prog)
    for note in NOTES:
        ev = M.head()
        ev += [(1.0, bytes([0xc0, prog]))]
        ev += M.note(0, note, 100, 1.3, 1.0)
        mid = W / ('p%d_%d.mid' % (prog, note))
        M.write(mid, [M.track(M.seq(ev))])
        a, b = run(mid, False, 3.0), run(mid, True, 3.0)
        n72 = max((x[72] & 0x7f) for x in e)
        exp = 441 * (1 << (n72 - 1 if n72 < 8 else 7)) - 82 if n72 else 0
        da = (a[1] - a[0]) if len(a) > 1 else -1
        db = (b[1] - b[0]) if len(b) > 1 else -1
        print('%-5d %-5d %-7d %10d %10d %8d'
              % (prog, note, n72, da, db, exp))
