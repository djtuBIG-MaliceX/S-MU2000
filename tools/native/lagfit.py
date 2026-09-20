#!/usr/bin/env python3
# license:BSD-3-Clause
"""**「ずらせば合うのか、中身が違うのか」を仕分ける。**

波形の相関が低いとき、原因は 2 つに分かれる。

* **時刻の差** … 値は合っていて、鳴り出す時刻だけずれている。
  ずらしてやれば相関が戻る。直せることが多い（6.117・6.121 はこれだった）
* **中身の差** … ずらしても戻らない。式かレジスタが違う

1 秒ごとに ±200 サンプルの範囲でいちばん合うずらしを探し、
そのままの相関と並べて出す。

  python tools/native/lagfit.py [試験の名前 …]

`build/tests/<名前>.wav`（firmware の道）と `<名前>_ne.wav`（native の口）を
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
MAX = 200                            # 探すずらしの幅（サンプル）
DEFAULT = ['piano', 'chord', 'drums', 'effects', 'dense', 'port_b', 'bend',
           'lofi', 'egcc', 'porta', 'at', 'sxparam', 'pedals', 'partsx',
           'keylevel', 'rpn', 'mono', 'ctlreset', 'ports', 'scale', 'kits',
           'ins2', 'progchg', 'running', 'pat', 'ccramp', 'midreset',
           'partmode', 'drumnrpn', 'retrig']


def corr(sa, sb, na):
    nb = sum(float(x) * x for x in sb)
    if nb < 1e4:
        return None
    return sum(float(x) * float(y) for x, y in zip(sa, sb)) / math.sqrt(na * nb)


def one(name):
    a, b = W / ('%s.wav' % name), W / ('%s_ne.wav' % name)
    if not a.exists() or not b.exists():
        return None
    fa, ra, ca, _ = fpmod.load_wav(str(a))
    fb, _, cb, _ = fpmod.load_wav(str(b))
    A, B = fa[::ca], fb[::cb]
    n = min(len(A), len(B))
    raw, fit, lags = [], [], []
    for s0 in range(int(BOOT * ra), n - ra - MAX, ra):
        sa = A[s0:s0 + ra]
        na = sum(float(x) * x for x in sa)
        if na < 1e4:
            continue
        best, bl, r0 = -2.0, 0, None
        for lag in range(-MAX, MAX + 1):
            j = s0 + lag
            if j < 0 or j + ra > len(B):
                continue
            c = corr(sa, B[j:j + ra], na)
            if c is None:
                continue
            if lag == 0:
                r0 = c
            if c > best:
                best, bl = c, lag
        if r0 is None:
            continue
        raw.append(r0)
        fit.append(best)
        lags.append(bl)
    if not raw:
        return None
    raw.sort()
    fit.sort()
    lags.sort()
    return raw[len(raw) // 2], fit[len(fit) // 2], lags[len(lags) // 2]


def main():
    names = sys.argv[1:] or DEFAULT
    print('%-10s %8s %8s %8s   %s'
          % ('試験', 'そのまま', 'ずらして', 'ずらし', '見立て'))
    for name in names:
        got = one(name)
        if got is None:
            print('%-10s 波形が無い（先に run_tests.py）' % name)
            continue
        mr, mf, ml = got
        if mf < 0.97:
            v = '**中身が違う**'
        elif mr < 0.95:
            v = '時刻だけ'
        else:
            v = '合っている'
        print('%-10s %7.0f%% %7.0f%% %8d   %s' % (name, 100 * mr, 100 * mf, ml, v))


if __name__ == '__main__':
    main()
