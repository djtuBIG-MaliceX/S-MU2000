#!/usr/bin/env python3
# license:BSD-3-Clause
"""**曲まるごとで、押した瞬間のレジスタを実機と突き合わせる**（段 4）。

`regsweep.py` は 1 音ずつ `nativeplay` で組んで比べるので、パートのつまみや
ドラムの設定が絡む所を見られない。こちらは**同じ MIDI を 2 回鳴らして**
（実機の道と `SMU2000_NOCAL=1` の native の口）、`--trace-swp` に残った
書き込みを押鍵ごとに突き合わせる。

  python tools/native/regdiff.py <試験の名前> [--roms DIR] [--pairs]

`SMU2000_NOCAL=1` は写し取りを一切しないので、**食い違ったレジスタが
そのまま「まだ式が分かっていない所」**になる。ドラムでもパートのつまみを
振った曲でも同じように見られる。

  --pairs   1 打ずつ並べる（どの音が違うか見るとき）
"""
import argparse
import collections
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "build"
WORK = BUILD / "tests" / "regdiff"
EXE = ".exe" if os.name == "nt" else ""
BOOT = 8.0
NEEDED = ("mu2000.zip", "swp30.zip")

LINE = re.compile(r'^(N |W |R )?(00800000) ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
MASK = {0x1cf: 0, 0x1ce: 16, 0x18f: 32, 0x18e: 48}
KEYON = 0x20e
# 毎サンプル書き替わる（MEG の戻りのミキサ）ので比べない
# 毎サンプル書き替わる（MEG の戻りのミキサ）ので比べない。
# **0x21-0x2b の奇数番と 0x30・0x31 も外す**。実機の firmware は
# 1 音ごとには書かないので、起動のときの残りが最初の押鍵に混ざる
SKIP = set([0x0e, 0x0f, 0x30, 0x31] + list(range(0x38, 0x40))
           + [r for r in range(0x20, 0x2c) if r & 1])


def find_roms(given):
    cands = [Path(given)] if given else []
    if os.environ.get("SMU2000_ROMS"):
        cands.append(Path(os.environ["SMU2000_ROMS"]))
    cands += [ROOT / "roms", ROOT.parent / "MU2000" / "roms"]
    for c in cands:
        if all((c / n).exists() for n in NEEDED):
            return c
    return None


def run(roms, midi, seconds, tag, native):
    trc = WORK / ("%s.txt" % tag)
    cmd = [str(BUILD / ("render" + EXE)), str(roms), str(midi),
           str(WORK / ("%s.wav" % tag)), "%.3f" % seconds,
           "--boot", "%.3f" % BOOT, "--trace-swp", str(trc)]
    env = dict(os.environ)
    env["SMU2000_NO_VOICECACHE"] = "1"
    if native:
        cmd.append("--native-engine")
        env["SMU2000_NOCAL"] = "1"
    else:
        env.pop("SMU2000_NOCAL", None)
    r = subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
    if r.returncode != 0:
        return None
    return trc


def keyons(trc):
    """[(サンプル, {スロット: {レジスタ: 値}})] を押鍵の順に返す"""
    cur = collections.defaultdict(dict)
    mask = 0
    out = []
    with open(trc, errors="replace") as f:
        for line in f:
            m = LINE.match(line)
            if not m or m.group(1) == "R ":
                continue
            reg, val, s = int(m.group(3), 16), int(m.group(4), 16), int(m.group(5))
            if reg in MASK:
                sh = MASK[reg]
                mask = (mask & ~(0xffff << sh)) | (val << sh)
            elif reg == KEYON:
                got = {i: dict(cur[i]) for i in range(64)
                       if (mask >> i) & 1 and cur.get(i)}
                if got:
                    out.append((s, got))
                cur = collections.defaultdict(dict)
                mask = 0
            elif reg < 0x1000 and reg % 64 not in SKIP:
                cur[reg // 64][reg % 64] = val
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("name")
    ap.add_argument("--roms")
    ap.add_argument("--pairs", action="store_true")
    a = ap.parse_args()

    roms = find_roms(a.roms)
    if not roms:
        print("ROM が見つからない（--roms か SMU2000_ROMS で指す）")
        return 1
    sys.path.insert(0, str(ROOT / "tools"))
    import make_test_midi
    cases = make_test_midi.build(BUILD / "tests")
    if a.name not in cases:
        print("その名前の試験は無い: %s（%s）" % (a.name, " ".join(sorted(cases))))
        return 1
    midi, seconds = cases[a.name]
    WORK.mkdir(parents=True, exist_ok=True)

    tf = run(roms, midi, seconds, "fw", False)
    tn = run(roms, midi, seconds, "nv", True)
    if not tf or not tn:
        print("鳴らせなかった")
        return 1
    fw, nv = keyons(tf), keyons(tn)
    # **時刻で結び付ける**。番号順だと、片方だけ
    # レジスタを 1 本も書かない押鍵があったときに
    # そこから先が全部ずれる（6.206）
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
    print("%s  実機 %d 回 / native %d 回" % (a.name, len(fw), len(nv)))

    bad = collections.Counter()
    miss = collections.Counter()
    ncmp = 0
    for i, (fi, ni) in enumerate(pairs):
        at_f, af = fw[fi]
        at_n, an = nv[ni]
        # **スロットは波形の番地で結び付ける**。実機は 0 から、native は 63 から
        # 取るので、番号の順に並べると要素が逆になる（多要素の音色で全部
        # 食い違って見えていた）。番地が引けないものは残りを順に当てる
        fs, ns = sorted(af), sorted(an)
        def wave(d):
            return (d.get(0x16, -1) << 16) | d.get(0x17, -1)
        left = list(ns)
        pair = []
        for slot_f in fs:
            if not left:
                break
            d = af[slot_f]

            def score(sn):
                e = an[sn]
                # 波形の番地が合うものを最優先。そのうえで食い違いの少ない相手
                same_wave = 0 if (wave(d) >= 0 and wave(d) == wave(e)) else 1
                nd = sum(1 for r in d if r in e and d[r] != e[r])
                return (same_wave, nd)

            hit = min(left, key=score)
            left.remove(hit)
            pair.append((slot_f, hit))
        rows = []
        for sf, sn in pair:
            f, n = af[sf], an[sn]
            diff = [r for r in sorted(f) if r in n and f[r] != n[r]]
            for r in diff:
                bad[r] += 1
            for r in sorted(f):
                if r not in n:
                    miss[r] += 1
            ncmp += 1
            rows.append((sf, sn, f, n, diff))
        if a.pairs:
            print("  %4d 打目  実機 s=%-8d native s=%-8d (%+d)%s"
                  % (i + 1, at_f, at_n, at_n - at_f,
                     "  スロット数が違う" if len(fs) != len(ns) else ""))
            for slot_f, slot_n, f, n, diff in rows:
                if diff:
                    print("    slot %2d/%2d: %s" % (
                        slot_f, slot_n,
                        " ".join("0x%02x(%04x/%04x)" % (r, f[r], n[r])
                                 for r in diff[:10])))
    print()
    print("違ったレジスタ（多い順）: %s" %
          " ".join("0x%02x×%d" % (r, c) for r, c in bad.most_common()))
    if miss:
        print("native が書かなかったレジスタ: %s" %
              " ".join("0x%02x×%d" % (r, c) for r, c in miss.most_common()))
    print("食い違い %d 本 / 突き合わせたスロット %d" % (sum(bad.values()), ncmp))
    # 押鍵の時刻のずれも出す（レジスタが合っていても鳴り出しがずれれば波形は違う）
    if pairs:
        d = sorted(nv[ni][0] - fw[fi][0] for fi, ni in pairs)
        hist = collections.Counter(d)
        print("押鍵のずれ: %s" %
              " ".join("%+d×%d" % (k, v) for k, v in sorted(hist.items())))
    return 0


if __name__ == "__main__":
    sys.exit(main())
