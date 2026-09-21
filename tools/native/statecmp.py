#!/usr/bin/env python3
# license:BSD-3-Clause
"""**鳴っている最中のスロットの中身**を、実機と native で丸ごと比べる（段 4）。

`regdiff.py` は「前の引き金からこの引き金まで」に書いたぶんしか見ない。
だから

  * **遅れて鳴る 2 つ目の要素**（firmware は先にまとめて書いてから、あとで引く）
  * **鳴っている途中で動く値**（包絡線・LFO・つまみの追従）

を取りこぼす。こちらは走らせながらレジスタの控えを持って、引き金のところで
**その時点の 64 本ぜんぶ**を写して突き合わせる。6.189 のフィルタ側 LFO は
これで見つけた。

  python tools/native/statecmp.py <試験の名前> [--roms DIR] [--at <サンプル>]

`--at` を付けると、引き金ではなく**その時刻**の中身を出す（鳴っている
最中に動く値を見るとき）。スロットの組は「実機は 0 から、native は 63 から」
の順に当てる。
"""
import argparse
import collections
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import regdiff

LINE, MASK, KEYON, SKIP = regdiff.LINE, regdiff.MASK, regdiff.KEYON, regdiff.SKIP


def snaps(trc, at=None):
    """[(サンプル, {スロット: {レジスタ: 値}})]。中身は**その時点の全部**"""
    file = collections.defaultdict(dict)
    mask = 0
    out = []
    with open(trc, errors="replace") as f:
        for line in f:
            m = LINE.match(line)
            if not m or m.group(1) == "R ":
                continue
            reg = int(m.group(3), 16)
            val = int(m.group(4), 16)
            s = int(m.group(5))
            if at is not None and s > at:
                break
            if reg in MASK:
                sh = MASK[reg]
                mask = (mask & ~(0xffff << sh)) | (val << sh)
            elif reg == KEYON:
                if at is None:
                    got = {i: dict(file[i]) for i in range(64) if (mask >> i) & 1}
                    if got:
                        out.append((s, got))
                mask = 0
            elif reg < 0x1000 and reg % 64 not in SKIP:
                file[reg // 64][reg % 64] = val
    if at is not None:
        out.append((at, {i: dict(file[i]) for i in range(64) if file[i]}))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("name")
    ap.add_argument("--roms")
    ap.add_argument("--at", type=int, default=None,
                    help="このサンプルの時点の中身を見る（既定は引き金ごと）")
    a = ap.parse_args()

    roms = regdiff.find_roms(a.roms)
    if not roms:
        print("ROM が見つからない（--roms か SMU2000_ROMS で指す）")
        return 1
    sys.path.insert(0, str(regdiff.ROOT / "tools"))
    import make_test_midi
    cases = make_test_midi.build(regdiff.BUILD / "tests")
    if a.name not in cases:
        print("その名前の試験は無い: %s（%s）" % (a.name, " ".join(sorted(cases))))
        return 1
    midi, seconds = cases[a.name]
    regdiff.WORK.mkdir(parents=True, exist_ok=True)

    tf = regdiff.run(roms, midi, seconds, "fw", False)
    tn = regdiff.run(roms, midi, seconds, "nv", True)
    if not tf or not tn:
        print("鳴らせなかった")
        return 1
    fw, nv = snaps(tf, a.at), snaps(tn, a.at)
    print("%s  実機 %d 回 / native %d 回" % (a.name, len(fw), len(nv)))

    bad = collections.Counter()
    for i in range(min(len(fw), len(nv))):
        at_f, af = fw[i]
        _, an = nv[i]
        # 実機は 0 から、native は 63 から取るので、並べる向きを合わせる
        fs, ns = sorted(af), sorted(an, reverse=True)
        line = []
        for sf, sn in zip(fs, ns):
            f, n = af[sf], an[sn]
            d = [r for r in sorted(set(f) | set(n))
                 if f.get(r, -1) != n.get(r, -1)]
            for r in d:
                bad[r] += 1
            if d:
                line.append("slot %2d/%2d: %s" % (
                    sf, sn, " ".join(
                        "0x%02x(%s/%s)" % (
                            r,
                            "----" if r not in f else "%04x" % f[r],
                            "----" if r not in n else "%04x" % n[r])
                        for r in d[:10])))
        if line:
            print("  %d 打目 s=%d" % (i + 1, at_f))
            for l in line:
                print("    " + l)
    print()
    print("違ったレジスタ: %s" %
          (" ".join("0x%02x×%d" % (r, c) for r, c in bad.most_common())
           or "なし"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
