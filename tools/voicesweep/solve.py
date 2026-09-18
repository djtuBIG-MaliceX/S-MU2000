#!/usr/bin/env python3
"""voicesweep.txt を読んで、音色の記録の 84 バイトと SWP30 のレジスタの対応を探す。

doc/native-engine.md の段 1。「そのまま入る」「上位/下位バイトに入る」「定数」の 3 つを、
全部の音色で成り立つかどうかで判定する。

使い方: python tools/voicesweep/solve.py voicesweep.txt [-n 鍵] [-v 強さ]
"""
import sys
import collections

# レジスタの意味（MAME 由来の並び。src/mame/sound/swp30.cpp の先頭）
REG_NAME = {
    0x00: 'filter1 mode+param', 0x01: 'bypass/dry level', 0x02: 'filter2 mode+param',
    0x03: 'post-filter level', 0x04: 'filters 2nd param', 0x05: 'LFO amp depth',
    0x06: 'attack speed+start vol', 0x07: 'decay1 speed+target', 0x08: 'decay2 speed+target',
    0x09: 'release speed+global vol', 0x0a: 'LFO type/step/pitch', 0x0b: '?0b',
    0x10: '?10', 0x11: 'pitch', 0x12: 'pre-loop hi', 0x13: 'pre-loop lo',
    0x14: 'loop size hi', 0x15: 'loop size lo', 0x16: 'format+addr hi', 0x17: 'addr lo',
}


def parse(path):
    """[(msb, lsb, prog, name, [elem bytes...], {(note, vel): [(chip, ch, {reg: val})]})]"""
    out = []
    cur = None
    note = None
    for line in open(path, encoding='utf-8', errors='replace'):
        w = line.split()
        if not w:
            continue
        if w[0] == 'VOICE':
            name = line.split('name=', 1)[1].strip() if 'name=' in line else ''
            cur = dict(msb=int(w[1]), lsb=int(w[2]), prog=int(w[3]),
                       rec=int(w[4][4:], 16), elems=int(w[5][6:]), name=name,
                       elem=[], notes=collections.OrderedDict())
            out.append(cur)
        elif w[0] == 'ELEM' and cur is not None:
            cur['elem'].append([int(x, 16) for x in w[2:]])
        elif w[0] == 'NOTE' and cur is not None:
            note = (int(w[1]), int(w[2]))
            cur['notes'][note] = []
        elif w[0] == 'CH' and cur is not None and note is not None:
            regs = {}
            for f in w[3:]:
                a, b = f.split('=')
                regs[int(a, 16)] = int(b, 16)
            cur['notes'][note].append((int(w[1]), int(w[2], 16), regs))
    return out


def as_function(rows, field, nbytes):
    """レジスタの値が「84 バイトのどれか 1 つだけで決まる」かどうかを見る。

    そのまま入っていなくても、表を引いていれば「同じバイトの値には必ず同じ結果」に
    なる。これで減衰の目標（= (0x7f - byte) * 2）やアタックの速さが見つかった。
    """
    vals = [field(rg) for _, rg in rows]
    if len(set(vals)) == 1:
        return ('定数 %d' % vals[0], [])
    hits = []
    for i in range(nbytes):
        col = [e[i] for e, _ in rows]
        if len(set(col)) == 1:
            continue
        m = {}
        ok = True
        for c, v in zip(col, vals):
            if m.setdefault(c, v) != v:
                ok = False
                break
        if ok:
            hits.append((i, sorted(m.items())))
    return (None, hits)


def main():
    args = sys.argv[1:]
    path = args[0]
    note_pick = None
    if '-n' in args:
        note_pick = int(args[args.index('-n') + 1])
    voices = parse(path)
    print('音色 %d 個' % len(voices))

    # 1 要素・1 スロットのものだけを使う（対応付けが一意になる）
    rows = []
    for v in voices:
        if v['elems'] != 1 or len(v['elem']) != 1:
            continue
        for (note, vel), chans in v['notes'].items():
            if note_pick is not None and note != note_pick:
                continue
            if len(chans) != 1:
                continue
            rows.append((v, note, vel, chans[0][2]))
    print('使える組 %d 個（1 要素・1 スロット）' % len(rows))
    if not rows:
        return

    regs = sorted(set(r for _, _, _, rg in rows for r in rg))
    nbytes = min(len(v['elem'][0]) for v, _, _, _ in rows)

    for reg in regs:
        vals = [rg.get(reg) for _, _, _, rg in rows]
        if any(v is None for v in vals):
            continue
        name = REG_NAME.get(reg, '')
        if len(set(vals)) == 1:
            print('  %02x %-26s 定数 %04x' % (reg, name, vals[0]))
            continue
        hi = [v >> 8 for v in vals]
        lo = [v & 0xff for v in vals]
        found = []
        for i in range(nbytes):
            col = [r[0]['elem'][0][i] for r in rows]
            if len(set(col)) == 1:
                continue        # 全部の音色で同じバイトは、何にでも当たるので見ない
            if col == hi:
                found.append('上位 = byte %d' % i)
            if col == lo:
                found.append('下位 = byte %d' % i)
            # 上位 7bit・反転・1 引きなど、よくある形も見る
            if [c ^ 0xff for c in col] == hi:
                found.append('上位 = ~byte %d' % i)
            if [c ^ 0xff for c in col] == lo:
                found.append('下位 = ~byte %d' % i)
            if [c * 2 & 0xff for c in col] == hi:
                found.append('上位 = byte %d * 2' % i)
        # 鍵や強さで変わるか
        by_voice = collections.defaultdict(set)
        for (v, note, vel, rg) in rows:
            by_voice[(v['msb'], v['lsb'], v['prog'])].add(rg.get(reg))
        varies = sum(1 for s in by_voice.values() if len(s) > 1)
        tag = '（鍵/強さで変わる音色 %d/%d）' % (varies, len(by_voice)) if varies else ''
        print('  %02x %-26s %s %s' % (reg, name, ' / '.join(found) if found else '-', tag))

    # 「表を引いているだけ」のものを探す（鍵と強さを 1 つに絞って見る）
    one = [(r[0]['elem'][0], r[3]) for r in rows
           if (note_pick is None or r[1] == note_pick)]
    if not one:
        return
    print()
    print('1 つのバイトだけで決まるもの:')
    FIELDS = [
        ('06 上位（アタックの速さ）', lambda rg: rg[0x06] >> 8),
        ('07 上位（減衰 1 の速さ）', lambda rg: rg[0x07] >> 8),
        ('07 下位（減衰 1 の目標）', lambda rg: rg[0x07] & 0xff),
        ('08 上位（減衰 2 の速さ）', lambda rg: rg[0x08] >> 8),
        ('08 下位（減衰 2 の目標）', lambda rg: rg[0x08] & 0xff),
        ('09 下位（全体の音量）', lambda rg: rg[0x09] & 0xff),
        ('00 下位 11bit（フィルタ）', lambda rg: rg[0x00] & 0x7ff),
        ('0a（LFO）', lambda rg: rg[0x0a]),
    ]
    for label, f in FIELDS:
        rs = [(e, rg) for e, rg in one if all(k in rg for k in (0x00, 0x06, 0x07, 0x08, 0x09, 0x0a))]
        if not rs:
            continue
        const, hits = as_function(rs, f, nbytes)
        if const:
            print('  %-24s %s' % (label, const))
        elif hits:
            for i, m in hits[:2]:
                # (0x7f - byte) * 2 のような簡単な形かどうかも見る
                rule = ''
                if all(v == (0x7f - k) * 2 for k, v in m):
                    rule = '  ＝ (0x7f - byte) * 2'
                print('  %-24s byte %d%s' % (label, i, rule))
                print('      ' + ' '.join('%02x->%02x' % kv for kv in m[:16]))
        else:
            print('  %-24s どの 1 バイトでも決まらない' % label)


main()
