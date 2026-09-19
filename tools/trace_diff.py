"""firmware と native の SWP30 書き込みを、音ごとに突き合わせる。

使い方: trdiff.py <firmware の記録> <native の記録> [出す音の数]

どちらの記録も render --trace-swp で取る（native 側は "N " 付きの行が
native の書き込み）。鍵を押した瞬間（レジスタ 0x20e）ごとに、そのとき
鳴らすスロットのレジスタ一式を集めて、同じ順番の音どうしで比べる。
"""
import collections
import itertools
import re
import sys

LINE = re.compile(r'^(N |W )?00(80|80)0000 ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')
MASTER = re.compile(r'^(N |W )?00800000 ([0-9a-f]{4}) ([0-9a-f]{4}).*s=(\d+)')

# 毎サンプル書き替わるので比べない（MEG の戻りのミキサ）
SKIP = set([0x0e, 0x0f] + list(range(0x38, 0x40)))


def notes_of(path):
    """鍵を押すたびに (サンプル, {スロット: {レジスタ: 値}}) を返す"""
    cur = collections.defaultdict(dict)   # slot -> reg -> value
    mask = 0
    out = []
    with open(path, 'r', errors='replace') as f:
        for line in f:
            m = MASTER.match(line)
            if not m:
                continue
            reg = int(m.group(2), 16)
            val = int(m.group(3), 16)
            smp = int(m.group(4))
            if reg == 0x18e:
                mask = (mask & ~(0xffff << 48)) | (val << 48)
            elif reg == 0x18f:
                mask = (mask & ~(0xffff << 32)) | (val << 32)
            elif reg == 0x1ce:
                mask = (mask & ~(0xffff << 16)) | (val << 16)
            elif reg == 0x1cf:
                mask = (mask & ~0xffff) | val
            elif reg == 0x20e:
                WIN = 441 * 2      # 鍵の 20ms 前まで
                slots = {}
                for i in range(64):
                    if (mask >> i) & 1:
                        slots[i] = {r: v for r, (v, ts) in cur.get(i, {}).items()
                                    if smp - ts <= WIN}
                if slots:
                    out.append((smp, slots))
            elif reg < 0x1000:
                rr = reg % 64
                if rr not in SKIP:
                    # **いつ書かれたか**も持つ。鍵の直前に書かれたものだけを
                    # その音のものとみなす（持ち越すと前の音の値と比べてしまう）
                    cur[reg // 64][rr] = (val, smp)
    return out


def main():
    fw = notes_of(sys.argv[1])
    ne = notes_of(sys.argv[2])
    limit = int(sys.argv[3]) if len(sys.argv) > 3 else 40
    print('firmware の音 %d 個 / native の音 %d 個' % (len(fw), len(ne)))
    # **時刻で対応づける**（順番だと片方に余分な音があるとずれる）
    pairs = []
    j = 0
    for fs, fslots in fw:
        while j < len(ne) and ne[j][0] < fs - 200:
            j += 1
        if j < len(ne) and abs(ne[j][0] - fs) <= 200:
            pairs.append((fs, fslots, ne[j][0], ne[j][1]))
            j += 1
    print('時刻で対応づいた音 %d 個' % len(pairs))
    tally = collections.Counter()
    size = collections.Counter()
    shown = 0
    for k, (fs, fslots, ns, nslots) in enumerate(pairs):
        # スロット番号は違って当たり前。**波形の番地（0x16/0x17）で要素を
        # 対応づける**。こちらはスロットを上から、firmware は下から取るので、
        # 番号順に並べると要素の順が逆になってしまう
        def key(d):
            return (d.get(0x16), d.get(0x17))
        fl = [fslots[i] for i in sorted(fslots)]
        nl = [nslots[i] for i in sorted(nslots)]
        if len(fl) != len(nl):
            tally['要素の数が違う'] += 1
            continue
        # **食い違いがいちばん少なくなる組み合わせ**を選ぶ。要素の数は
        # せいぜい数個なので総当たりでよい。こうしないと、こちらが
        # スロットを上から取ることによる並びの差を「違い」と数えてしまう
        def cost(a, b):
            return sum(1 for rr in set(a) | set(b) if a.get(rr) != b.get(rr))
        best_perm, best_cost = None, None
        for perm in itertools.permutations(range(len(nl))):
            c = sum(cost(fl[i], nl[perm[i]]) for i in range(len(fl)))
            if best_cost is None or c < best_cost:
                best_cost, best_perm = c, perm
        for i in range(len(fl)):
            a, b = fl[i], nl[best_perm[i]]
            for rr in sorted(set(a) | set(b)):
                av, bv = a.get(rr), b.get(rr)
                if av != bv:
                    tally['0x%02x' % rr] += 1
                    if av is not None and bv is not None:
                        # **ずれの大きさ**も足す。上位・下位で意味が違うので
                        # 上位バイトと下位バイトに分けて見る
                        size['0x%02x' % rr] += abs((av >> 8) - (bv >> 8)) * 256                             + abs((av & 0xff) - (bv & 0xff))
                    if shown < limit:
                        print('  音%3d (s=%d/%d) レジスタ 0x%02x  firmware=%s native=%s'
                              % (k, fs, ns, rr,
                                 '%04x' % av if av is not None else '----',
                                 '%04x' % bv if bv is not None else '----'))
                        shown += 1
    print()
    print('=== 違ったレジスタの多い順 ===')
    for rr, c in tally.most_common(30):
        print('  %-8s %3d 回  ずれの合計 %6d（1 回あたり %.1f）'
              % (rr, c, size[rr], size[rr] / c if c else 0))
    print('  ずれの合計（全部） %d' % sum(size.values()))


main()
