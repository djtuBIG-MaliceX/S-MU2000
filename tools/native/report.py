#!/usr/bin/env python3
# license:BSD-3-Clause
"""**全部の試験で「どこがどれだけ違うか」を 1 枚にまとめる**（段 4）。

`resid.py --nocal`（波形の引き算）と `regdiff.py`（押鍵時のレジスタ）を
**1 回の演奏で両方**出す。残っている差を

  * **直せる差** … 押した瞬間のレジスタが違う（式がまだ）
  * **firmware の都合** … レジスタは同じで、打鍵の時刻だけずれている

に分けて見るための道具。実機の打鍵は主ループの位置で 1-2 サンプル揺れる
ので（6.159）、後者は真似られない。

  python tools/native/report.py [名前 ...] [--roms DIR]
"""
import argparse
import collections
import math
import os
import subprocess
import sys
import wave
from array import array
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import regdiff

ROOT = regdiff.ROOT
BUILD = regdiff.BUILD
WORK = BUILD / "tests" / "report"
EXE = regdiff.EXE
BOOT = regdiff.BOOT


def rd(path):
    with wave.open(str(path)) as w:
        a = array('h')
        a.frombytes(w.readframes(w.getnframes()))
        return a, w.getframerate(), w.getnchannels()


def rms(v):
    return math.sqrt(sum(float(x) * x for x in v) / max(1, len(v)))


CAL = False      # True なら写し取りをする既定の道で測る（--cal）


def render(roms, midi, seconds, tag, native):
    wav = WORK / ("%s.wav" % tag)
    trc = WORK / ("%s.txt" % tag)
    cmd = [str(BUILD / ("render" + EXE)), str(roms), str(midi), str(wav),
           "%.3f" % seconds, "--boot", "%.3f" % BOOT, "--trace-swp", str(trc)]
    env = dict(os.environ)
    env["SMU2000_NO_VOICECACHE"] = "1"
    if native:
        cmd.append("--native-engine")
        # **写し取りをする道でも測れるようにする**（6.209）。既定の
        # `SMU2000_NOCAL=1` は式だけでレジスタを組むので、写し取りの
        # 最中にしか出ない差（10ms 格子のずれなど）が見えない
        if CAL:
            env.pop("SMU2000_NOCAL", None)
        else:
            env["SMU2000_NOCAL"] = "1"
    else:
        env.pop("SMU2000_NOCAL", None)
    r = subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
    return (wav, trc) if r.returncode == 0 and wav.exists() else (None, None)


LAG = 3          # 窓ごとに合わせる幅（サンプル）
WHERE = 0        # 0 でなければ、悪い窓をこの数だけ出す


def resid(wa, wb):
    a, sr, ch = rd(wa)
    b, _, _ = rd(wb)
    n = min(len(a), len(b))
    skip = int(round(BOOT * sr)) * ch
    x, y = a[skip:n], b[skip:n]
    r0 = rms(x)
    if r0 < 1.0:
        return None, None
    d = rms([p - q for p, q in zip(x, y)])
    # **窓ごとに ±3 サンプルまで合わせる**。1 サンプルずれただけでも
    # 打楽器のような雑音は残差が 130% になるので、それを除いた「音そのもの
    # の違い」を見る。実機の打鍵は主ループの位置で揺れる（6.159）ので、
    # そこまで真似るのは筋が悪い
    step = int(0.2 * sr) * ch
    num = den = 0.0
    worst = []
    for s0 in range(LAG * ch, len(x) - step - LAG * ch, step):
        xa = x[s0:s0 + step]
        r1 = rms(xa)
        if r1 < 5.0:
            continue
        bv = None
        for lag in range(-LAG, LAG + 1):
            ya = y[s0 + lag * ch: s0 + lag * ch + step]
            if len(ya) != len(xa):
                continue
            v = rms([p - q for p, q in zip(xa, ya)])
            if bv is None or v < bv:
                bv = v
        if bv is not None:
            num += bv * bv * len(xa)
            den += r1 * r1 * len(xa)
            worst.append((bv * bv * len(xa), s0 / float(ch) / sr + BOOT,
                          100.0 * bv / r1))
    aligned = math.sqrt(num / den) if den > 0 else 0.0
    if WHERE:
        worst.sort(reverse=True)
        print("   悪い窓: " + "  ".join(
            "%.1f秒 %.0f%%" % (t, pc) for _, t, pc in worst[:WHERE]))
    return 100.0 * d / r0, 100.0 * aligned


def regs(tf, tn):
    fw, nv = regdiff.keyons(tf), regdiff.keyons(tn)
    bad = collections.Counter()
    ncmp = 0
    lags = collections.Counter()
    # **時刻で結び付ける**（6.206）。番号順だと、片方だけ
    # レジスタを 1 本も書かない押鍵があるとそこから先が
    # 全部ずれる（regdiff.py と同じ）
    TOL = 300                      # これ以上離れたものは別の打と見る
    pairs = []
    fi = ni = 0
    while fi < len(fw) and ni < len(nv):
        d = nv[ni][0] - fw[fi][0]
        if abs(d) <= TOL:
            pairs.append((fi, ni))
            fi += 1
            ni += 1
        elif d < 0:
            ni += 1                # native の方が早い
        else:
            fi += 1                # 実機の方が早い
    for fi, ni in pairs:
        at_f, af = fw[fi]
        at_n, an = nv[ni]
        lags[at_n - at_f] += 1
        fs, ns = sorted(af), sorted(an)

        def wave_(d):
            return (d.get(0x16, -1) << 16) | d.get(0x17, -1)

        left = list(ns)
        for slot_f in fs:
            if not left:
                break
            d = af[slot_f]

            def score(sn):
                e = an[sn]
                same = 0 if (wave_(d) >= 0 and wave_(d) == wave_(e)) else 1
                return (same, sum(1 for r in d if r in e and d[r] != e[r]))

            hit = min(left, key=score)
            left.remove(hit)
            e = an[hit]
            for r in sorted(d):
                if r in e and d[r] != e[r]:
                    bad[r] += 1
            ncmp += 1
    return bad, ncmp, lags, len(fw), len(pairs)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*")
    ap.add_argument("--roms")
    ap.add_argument("--lag", type=int, default=3,
                    help="窓ごとに合わせる幅（サンプル）")
    ap.add_argument("--where", type=int, default=0,
                    help="悪い窓を N つ出す")
    ap.add_argument("--cal", action="store_true",
                    help="写し取りをする既定の道で測る（6.209）")
    a = ap.parse_args()
    global LAG, WHERE, CAL
    LAG, WHERE, CAL = a.lag, a.where, a.cal

    roms = regdiff.find_roms(a.roms)
    if not roms:
        print("ROM が見つからない（--roms か SMU2000_ROMS で指す）")
        return 1
    sys.path.insert(0, str(ROOT / "tools"))
    import make_test_midi
    cases = make_test_midi.build(BUILD / "tests")
    names = a.names or sorted(cases)
    WORK.mkdir(parents=True, exist_ok=True)

    print("%-10s %8s %8s %6s %-22s %s"
          % ("試験", "残差", "合わせると", "打鍵", "違ったレジスタ", "打鍵のずれ"))
    rows = []
    for name in names:
        if name not in cases:
            continue
        midi, seconds = cases[name]
        wf, tf = render(roms, midi, seconds, "fw", False)
        wn, tn = render(roms, midi, seconds, "nv", True)
        if not wf or not wn:
            print("%-10s 鳴らせなかった" % name)
            continue
        whole, shifted = resid(wf, wn)
        bad, ncmp, lags, nf, nn = regs(tf, tn)
        reg = " ".join("%02x×%d" % (r, c) for r, c in bad.most_common(4)) or "なし"
        lag = " ".join("%+d×%d" % (k, v) for k, v in sorted(lags.items())[:4])
        print("%-10s %7.2f%% %7.2f%% %6s %-22s %s"
              % (name, whole or 0, shifted or 0,
                 "%d/%d" % (nf, nn), reg, lag))
        rows.append((name, whole, shifted, sum(bad.values())))
    print()
    clean = [r for r in rows if r[3] == 0]
    print("押鍵のレジスタが 1 本も違わない試験: %d / %d" % (len(clean), len(rows)))
    if rows:
        print("残差の平均 %.2f%%（窓ごとに合わせると %.2f%%）"
              % (sum(r[1] or 0 for r in rows) / len(rows),
                 sum(r[2] or 0 for r in rows) / len(rows)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
