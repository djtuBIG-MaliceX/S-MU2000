#!/usr/bin/env python3
# license:BSD-3-Clause
"""`keylevel` の音を **1 つずつ** 実機と native で比べる。

曲ぜんぶの相関では「どの音色のどの鍵が悪いのか」が見えない。
この試験は音を重ねずに 0.75 秒おきに鳴らすので、音ごとに切って
音量（rms）と波形の相関を出せる。

  python tools/native/notelevel.py

`build/tests/keylevel.wav`（firmware の道）と `keylevel_ne.wav`（native の口）を
読む。先に `python tools/run_tests.py` で作っておくこと。
"""
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools'))
import fingerprint as fpmod          # noqa: E402

W = ROOT / 'build' / 'tests'
BOOT = 8.0                           # run_tests の BOOT_AT と同じ
NAMES = {76: 'Bottle', 97: 'SoundTrk', 96: 'Rain', 34: 'PickBass'}
HOLD = 0.35                          # 1 音の長さ（make_test_midi の case_keylevel）


def plan():
    """case_keylevel と同じ並びを組み直す"""
    out = []
    t = 1.0
    for prog in (76, 97, 96, 34):
        t += 0.2
        for key in (36, 60, 84):
            for vel in (30, 127):
                out.append((NAMES[prog], key, vel, t))
                t += 0.75
        t += 0.3
    return out


def main():
    a, b = W / 'keylevel.wav', W / 'keylevel_ne.wav'
    if not a.exists() or not b.exists():
        sys.exit('%s が無い。先に python tools/run_tests.py' % a)
    fa, ra, ca, _ = fpmod.load_wav(str(a))
    fb, _, cb, _ = fpmod.load_wav(str(b))
    A, B = fa[::ca], fb[::cb]
    print('%-9s %4s %4s  %9s %9s %7s   %6s'
          % ('音色', '鍵', '強さ', '実機rms', 'nativerms', 'dB', '相関'))
    bad = 0
    for nm, key, vel, tt in plan():
        s0 = int((tt + BOOT) * ra)
        n = int(HOLD * ra)
        if s0 + n > min(len(A), len(B)):
            break
        sa, sb = A[s0:s0 + n], B[s0:s0 + n]
        na = sum(float(x) * x for x in sa)
        nb = sum(float(x) * x for x in sb)
        if na < 1e3 or nb < 1e3:
            continue
        num = sum(float(x) * float(y) for x, y in zip(sa, sb))
        d = 10 * math.log10(nb / na)
        c = num / math.sqrt(na * nb)
        mark = ''
        if abs(d) > 0.5 or c < 0.95:
            mark = '  <<<'
            bad += 1
        print('%-9s %4d %4d  %9.1f %9.1f %+7.2f   %5.1f%%%s'
              % (nm, key, vel, math.sqrt(na / n), math.sqrt(nb / n), d, 100 * c, mark))
    print('気になるもの %d 件（0.5dB より大きい／相関 95%% 未満）' % bad)


if __name__ == '__main__':
    main()
