#!/usr/bin/env python3
# license:BSD-3-Clause
"""**同じキーオンで鳴り出したスロット同士**でレジスタを突き合わせる。

スロットの番号は firmware の道と native の口で違う（実機は下から、
native は上から使う）。波形の番地だけで対応付けると、使い回された
スロットの古い値を掴んで嘘の差が出る（実測で何度も引っかかった）。
ここでは**鍵を押した合図（`0x20e`）**を両方で拾い、そのときのマスクに
入っているスロットだけを相手にする。

  python tools/native/notereg.py <ROM のディレクトリ> <試験の名前> <秒数> <見る時刻>

`<見る時刻>` にいちばん近いキーオンを選び、その **0.05 秒後**の値を比べる。
`--at <秒>` で比べる時点をずらせる。`--usb` は USB の口で鳴らす。
"""
import argparse
import collections
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / 'build'
LINE = re.compile(r'^(N |W )?00800000 ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
MASK = {0x1cf: 0, 0x1ce: 1, 0x18f: 2, 0x18e: 3}
KEYON = 0x20e
# チップが自分で動かす値・音に出ない値は見ない
SKIP = set([0x0e, 0x0f, 0x21, 0x23, 0x25, 0x27, 0x29, 0x2b, 0x30, 0x31]
           + list(range(0x38, 0x40)))


def trace(roms, mid, out, tag, native, secs, usb, boot=None):
    trc = out / ('notereg_%s.txt' % tag)
    cmd = [str(BUILD / 'render'), str(roms), str(mid),
           str(out / ('notereg_%s.wav' % tag)), str(secs),
           '--trace-swp', str(trc)]
    # **起動のしかたを音の試験とそろえる**（doc/native-engine.md の 6.129）
    cmd += (['--boot', '%.3f' % boot] if boot else ['--bootcache'])
    if usb:
        cmd.append('--usb')
    if native:
        cmd.append('--native-engine')
    env = dict(os.environ)
    env['SMU2000_NO_VOICECACHE'] = '1'
    env['SMU2000_CUT_EXACT'] = '1'
    subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL, check=True)
    keyons, cur = [], [0, 0, 0, 0]
    rows = []
    with open(trc, errors='replace') as f:
        for line in f:
            m = LINE.match(line)
            if not m:
                continue
            r, v, s = int(m.group(2), 16), int(m.group(3), 16), int(m.group(4))
            if r in MASK:
                cur[MASK[r]] = v
            elif r == KEYON:
                mk = cur[0] | (cur[1] << 16) | (cur[2] << 32) | (cur[3] << 48)
                if mk:
                    keyons.append((s, [i for i in range(64) if (mk >> i) & 1]))
                cur = [0, 0, 0, 0]
            elif r < 0x1000:
                rows.append((s, r // 64, r % 64, v))
    return keyons, rows


def state(rows, slots, upto):
    """upto までに書かれた値（スロットごと）"""
    st = collections.defaultdict(dict)
    want = set(slots)
    for s, sl, rr, v in rows:
        if s > upto:
            break
        if sl in want and rr not in SKIP:
            st[sl][rr] = v
    return st


def pair_slots(sa, sb, ea, eb):
    """要素の並びはスロット番号の順とは限らない。波形の番地で組み直す"""
    left = list(eb)
    out = []
    for x in ea:
        va = sa.get(x, {})
        wa = (va.get(0x16), va.get(0x17))
        # 波形の番地が同じものから選び、**いちばん本数が合うもの**を取る
        # （重ねの音色は 2 つの要素が同じ波形を使うので、番地だけでは決まらない）
        cand = [y for y in left
                if (sb.get(y, {}).get(0x16), sb.get(y, {}).get(0x17)) == wa] or left
        best = None
        if cand:
            best = max(cand, key=lambda y: sum(1 for r in va
                                               if r in sb.get(y, {})
                                               and va[r] == sb[y][r]))
            left.remove(best)
        out.append((x, best))
    return out


def report_all(a, ka, ra, kb, rb):
    """キーオンを全部見て、レジスタが何本合っているかをまとめる"""
    n = min(len(ka), len(kb))
    tot = same = 0
    bad = collections.Counter()
    events = 0
    for i in range(n):
        ea, eb = ka[i], kb[i]
        if len(ea[1]) != len(eb[1]):
            continue
        events += 1
        sa = state(ra, ea[1], ea[0] + int(a.at * 44100))
        sb = state(rb, eb[1], eb[0] + int(a.at * 44100))
        for x, y in pair_slots(sa, sb, ea[1], eb[1]):
            if y is None:
                continue
            va, vb = sa.get(x, {}), sb.get(y, {})
            miss = []
            for r in va:
                if r not in vb:
                    continue
                tot += 1
                if va[r] == vb[r]:
                    same += 1
                else:
                    bad[r] += 1
                    miss.append('0x%02x(%04x/%04x)' % (r, va[r], vb[r]))
            if miss:
                print('  %8.4f 秒 slot%-3d ⇔ %-3d  %s'
                      % (ea[0] / 44100.0, x, y, ' '.join(miss[:6])))
    if not tot:
        print('比べられるキーオンが無い')
        return
    print('%s  キーオン %d 件（%d 件は声の数が違うので飛ばした）'
          % (a.name, events, min(len(ka), len(kb)) - events))
    print('  レジスタ %d 本中 %d 本が一致（%.1f%%）' % (tot, same, 100.0 * same / tot))
    for r, c in bad.most_common(8):
        print('    0x%02x が %d 本違う' % (r, c))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('roms')
    ap.add_argument('name')
    ap.add_argument('secs')
    ap.add_argument('when', type=float, nargs='?', default=None,
                    help='この時刻に近いキーオンを見る（秒）。--all なら要らない')
    ap.add_argument('--all', action='store_true',
                    help='キーオンを全部見て、合っている本数をまとめる')
    ap.add_argument('--at', type=float, default=0.05, help='キーオンから何秒後の値か')
    ap.add_argument('--usb', action='store_true')
    ap.add_argument('--boot', type=float, default=None,
                    help='ほんとうに起動させる（run_tests.py と同じ 8.0 秒）')
    a = ap.parse_args()

    mid = BUILD / 'tests' / (a.name + '.mid')
    if not mid.exists():
        sys.exit('%s が無い。先に python tools/make_test_midi.py' % mid)
    out = BUILD / 'tests'
    ka, ra = trace(a.roms, mid, out, 'fw', False, a.secs, a.usb, a.boot)
    kb, rb = trace(a.roms, mid, out, 'nv', True, a.secs, a.usb, a.boot)
    if not ka or not kb:
        sys.exit('キーオンが見つからない')

    if a.all:
        report_all(a, ka, ra, kb, rb)
        return
    if a.when is None:
        sys.exit('時刻を渡すか --all を付ける')
    target = a.when * 44100
    ea = min(ka, key=lambda k: abs(k[0] - target))
    eb = min(kb, key=lambda k: abs(k[0] - target))
    print('%s  実機 %.4f 秒 slot%s  /  native %.4f 秒 slot%s  （%+d サンプル）'
          % (a.name, ea[0] / 44100.0, ea[1], eb[0] / 44100.0, eb[1], eb[0] - ea[0]))
    if len(ea[1]) != len(eb[1]):
        print('  鳴り出したスロットの数が違う（%d / %d）' % (len(ea[1]), len(eb[1])))
    sa = state(ra, ea[1], ea[0] + int(a.at * 44100))
    sb = state(rb, eb[1], eb[0] + int(a.at * 44100))
    print('  キーオンから %.0f ms 後:' % (a.at * 1000))
    # **要素の並びはスロット番号の順とは限らない**（実機は下から、native は
    # 上から配るので逆になる）。波形の番地（0x16/0x17）で組み直す
    left = list(eb[1])
    pairs = []
    for x in ea[1]:
        wa = (sa.get(x, {}).get(0x16), sa.get(x, {}).get(0x17))
        best = None
        for y in left:
            if (sb.get(y, {}).get(0x16), sb.get(y, {}).get(0x17)) == wa:
                best = y
                break
        if best is None and left:
            best = left[0]
        if best is not None:
            left.remove(best)
        pairs.append((x, best))
    for n, (x, y) in enumerate(pairs):
        if y is None:
            print('    要素 %d  実機 slot%-3d  相手なし' % (n, x))
            continue
        va, vb = sa.get(x, {}), sb.get(y, {})
        diff = [r for r in sorted(va) if r in vb and va[r] != vb[r]]
        onlya = [r for r in sorted(va) if r not in vb]
        onlyb = [r for r in sorted(vb) if r not in va]
        tag = '一致' if not diff else ' '.join('0x%02x(%04x/%04x)' % (r, va[r], vb[r])
                                               for r in diff)
        if onlya:
            tag += '  実機だけ:' + ' '.join('0x%02x' % r for r in onlya)
        if onlyb:
            tag += '  nativeだけ:' + ' '.join('0x%02x' % r for r in onlyb)
        print('    要素 %d  実機 slot%-3d ⇔ native slot%-3d  %s' % (n, x, y, tag))


if __name__ == '__main__':
    main()
