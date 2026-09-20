#!/usr/bin/env python3
# license:BSD-3-Clause
"""**引き算で見る**（doc/native-engine.md の 6.152）。

  python tools/native/resid.py [名前 ...]          make test の残した wav を測る
  python tools/native/resid.py --warm [名前 ...]   写し取り済みで鳴らし直して測る

`make test` が残した wav（`build/tests/<名前>.wav` と `<名前>_ne.wav`）を
サンプルごとに引き算して、残差の大きさを出す。

波形の相関は 1 秒ごとの**中央値**で見ているので、当たりの少ない場所の
食い違いが埋もれる。`drumrcv` は「試験は通っているのに 6.2 秒で実機 177 に
対し native 382」だった。引き算ならそういう所が一目で出る。

  残差 = rms(native - 実機) / rms(実機)

0% なら 1 ビットも違わない。1 サンプルずれているだけでも大きく出るので、
**小さいことより「前より増えていないこと」**を見るのに使う。

## `--warm` — こちらが本当の物差し

`make test` の native は**写し取りを持ち越さない**ので、どの音色も
**1 音目は firmware が鳴らしている**。つまり測っているものの中に
「実機が鳴らした音」が混ざっていて、native の出来は見えない。しかも
CPU を止めている間は firmware の打鍵が 1 サンプル遅れる（6.153）ので、
**こちらが何もしていない音でも残差が出る**。

`--warm` は写し取りを 1 度貯めてから鳴らし直す。**1 音目から native が
鳴らす**ので、出てくる残差は丸ごと native の責任になる。
「全機能で 1 ビットも差が出ない」を目指すなら見るのはこちら。
"""
import argparse
import math
import os
import shutil
import subprocess
import sys
import wave
from array import array
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
BUILD = Path(os.environ.get("SMU_BUILD") or (ROOT / "build"))
if not BUILD.is_absolute():
    BUILD = ROOT / BUILD
WORK = BUILD / "tests"
BOOT = 8.0
EXE = ".exe" if os.name == "nt" else ""
# ROM に要るもの（run_tests.py と同じ）
NEEDED = ("mu2000.zip", "swp30.zip")


def rd(path):
    with wave.open(str(path)) as w:
        a = array('h')
        a.frombytes(w.readframes(w.getnframes()))
        return a, w.getframerate(), w.getnchannels()


def rms(v):
    return math.sqrt(sum(float(x) * x for x in v) / max(1, len(v)))


def compare(wa, wb):
    """(同一か, 残差%, 最悪の窓%, その時刻) を返す"""
    if not wa.exists() or not wb.exists():
        return None
    a, ra, ca = rd(wa)
    b, _, cb = rd(wb)
    n = min(len(a), len(b))
    skip = int(round(BOOT * ra)) * ca
    x, y = a[skip:n], b[skip:n]
    if not x:
        return None
    same = x == y
    ra_ = rms(x)
    if ra_ < 1.0:
        return None
    d = rms([p - q for p, q in zip(x, y)])
    # いちばん悪い 0.2 秒の窓も出す（どこで離れたかの手がかり）
    w = int(0.2 * ra) * ca
    worst, at = 0.0, 0.0
    for s in range(0, len(x) - w, w):
        xa, ya = x[s:s + w], y[s:s + w]
        r0 = rms(xa)
        if r0 < 20.0:
            continue
        r1 = rms([p - q for p, q in zip(xa, ya)]) / r0
        if r1 > worst:
            worst, at = r1, s / float(ra * ca)
    return same, 100.0 * d / ra_, 100.0 * worst, at


def find_roms(given):
    cands = []
    if given:
        cands.append(Path(given))
    if os.environ.get("SMU2000_ROMS"):
        cands.append(Path(os.environ["SMU2000_ROMS"]))
    cands += [ROOT / "roms", ROOT.parent / "MU2000" / "roms"]
    for c in cands:
        if all((c / n).exists() for n in NEEDED):
            return c
    return None


def render(roms, midi, wav, seconds, extra, env):
    cmd = [str(BUILD / ("render" + EXE)), str(roms), str(midi), str(wav),
           "%.3f" % seconds, "--boot", "%.3f" % BOOT] + list(extra)
    e = dict(os.environ)
    e.update(env or {})
    r = subprocess.run(cmd, env=e, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
    return r.returncode == 0 and Path(wav).exists()


def warm_pair(roms, name, midi, seconds, out, home, nocal=False):
    """実機の道と、**写し取り済みの** native の口で 1 本ずつ鳴らす"""
    env = {"LOCALAPPDATA": str(home), "XDG_DATA_HOME": str(home),
           "HOME": str(home)}
    env.pop("SMU2000_NO_VOICECACHE", None)
    fw = out / ("%s.wav" % name)
    ne = out / ("%s_ne.wav" % name)
    if not render(roms, midi, fw, seconds, [], {"SMU2000_NO_VOICECACHE": "1"}):
        return "実機の道が鳴らせなかった"
    if nocal:
        # **写し取りを一切しない**（段 4）。1 回目も 2 回目も無いので 1 本だけ
        if not render(roms, midi, ne, seconds, ["--native-engine"],
                      {"SMU2000_NOCAL": "1", "SMU2000_NO_VOICECACHE": "1"}):
            return "写し取り無しで鳴らせなかった"
        return None
    # 1 回目で写しを貯める（捨てる）
    warm = ["--native-engine", "--voicecache"]
    if not render(roms, midi, out / ("%s_w1.wav" % name), seconds, warm, env):
        return "1 回目が鳴らせなかった"
    if not render(roms, midi, ne, seconds, warm, env):
        return "2 回目が鳴らせなかった"
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*")
    ap.add_argument("--warm", action="store_true",
                    help="写し取り済みで鳴らし直して測る（1 音目から native）")
    ap.add_argument("--nocal", action="store_true",
                    help="写し取りを一切せず、式だけで鳴らして測る（段 4）")
    ap.add_argument("--roms")
    a = ap.parse_args()

    if a.nocal:
        a.warm = True
    if not a.warm:
        names = a.names
        if not names:
            names = sorted(p.stem for p in WORK.glob("*.wav")
                           if not p.stem.endswith("_ne")
                           and (WORK / (p.stem + "_ne.wav")).exists())
        head("make test の wav・1 音目は実機")
        for name in names:
            row(name, compare(WORK / ("%s.wav" % name),
                              WORK / ("%s_ne.wav" % name)))
        return 0

    roms = find_roms(a.roms)
    if not roms:
        print("ROM が見つからない（--roms か SMU2000_ROMS で指す）")
        return 1
    sys.path.insert(0, str(ROOT / "tools"))
    import make_test_midi
    cases = make_test_midi.build(WORK)
    names = a.names or sorted(cases)
    out = WORK / "warmresid"
    home = out / "home"
    shutil.rmtree(home / "S-MU2000" / "voicecal", ignore_errors=True)
    out.mkdir(parents=True, exist_ok=True)
    home.mkdir(parents=True, exist_ok=True)
    head("写し取り無し・式だけ" if a.nocal else "写し取り済み・1 音目から native")
    for name in names:
        if name not in cases:
            print("%-10s その名前の試験は無い" % name)
            continue
        midi, seconds = cases[name]
        bad = warm_pair(roms, name, midi, seconds, out, home, a.nocal)
        if bad:
            print("%-10s %s" % (name, bad))
            continue
        row(name, compare(out / ("%s.wav" % name), out / ("%s_ne.wav" % name)))
    return 0


def head(warm):
    print("%s（%s）" % ("引き算で見る", warm))
    print("%-10s %-6s %8s %8s %8s" % ("試験", "同一", "残差", "最悪の窓", "その時刻"))


def row(name, r):
    if r is None:
        return
    same, whole, worst, at = r
    print("%-10s %-6s %7.2f%% %7.1f%%  %6.1f 秒"
          % (name, "はい" if same else "いいえ", whole, worst, at))


if __name__ == "__main__":
    sys.exit(main())
