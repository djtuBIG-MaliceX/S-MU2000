#!/usr/bin/env python3
"""分かった規則（doc/native-engine.md の 6.2-6.6）が、掃引の結果で本当に成り立つか検算する。

使い方:
    python tools/voicesweep/check.py voicesweep.txt <rom ディレクトリ>

段 1 を進めるたびにこれを回せば、前に分かったことを壊していないか分かる。
"""
import struct
import sys

# ROM の中の番地（MU2000 EX firmware v2.01）
SET_TAB = 0x200AF0      # 波形の組 → 波形の並びの中の位置（16bit を 503 個）
WAVE = 0x1F55A0         # 波形の記録（16 バイトずつ）
ATTACK_TAB = 0x1F4DB8   # アタックの速さの表（128 バイト）
DECAY_TAB = 0x1F4E38    # 減衰の速さの表（128 バイト）
LEVEL_TAB = 0x1E6818    # 音量 0-127 → 減衰（128 バイト）


def load_rom(path):
    with open(path, 'rb') as f:
        rom = f.read()
    if len(rom) != 0x400000:
        sys.exit('プログラム ROM は 4MB のはず: %s' % path)
    return rom


def u16(rom, a):
    return (rom[a] << 8) | rom[a + 1]


def u32(rom, a):
    return struct.unpack('>I', rom[a:a + 4])[0]


def wave_entry(rom, setno, note):
    """組の番号と鍵から、使う波形の記録（16 バイト）の番地"""
    if setno * 2 + 1 >= 0x200EE0 - SET_TAB:
        return None
    s = WAVE + u16(rom, SET_TAB + setno * 2)
    for _ in range(80):
        if rom[s + 3] >= note or rom[s + 3] == 0x7F:
            return s
        s += 16
    return None


def parse(path):
    voices = []
    cur = None
    note = None
    for line in open(path, encoding='utf-8', errors='replace'):
        w = line.split()
        if not w:
            continue
        if w[0] == 'VOICE':
            cur = dict(prog=int(w[3]), elems=int(w[5][6:]), elem=[], notes=[])
            voices.append(cur)
        elif w[0] == 'ELEM':
            cur['elem'].append([int(x, 16) for x in w[2:]])
        elif w[0] == 'NOTE':
            note = (int(w[1]), int(w[2]))
        elif w[0] == 'CH':
            regs = {}
            for f in w[3:]:
                a, b = f.split('=')
                regs[int(a, 16)] = int(b, 16)
            cur['notes'].append((note, int(w[1]), regs))
    return voices


def main():
    sweep = sys.argv[1]
    romdir = sys.argv[2] if len(sys.argv) > 2 else '../MU2000/roms'
    rom = load_rom(romdir.rstrip('/') + '/mu2000_flash.bin')

    checks = {
        '波形 0x12/0x13（ループ前の数）': [0, 0],
        '波形 0x14/0x15（ループの長さ）': [0, 0],
        '波形 0x16/0x17（形式と番地）': [0, 0],
        '減衰 1 の目標 0x07 下位': [0, 0],
        '減衰 2 の目標 0x08 下位': [0, 0],
        'アタックの速さ 0x06 上位': [0, 0],
        '鍵を押した時の 0x09 上位は 0': [0, 0],
        'フィルタ後の音量 0x03 = 0x5010': [0, 0],
        '音量 0x09 下位は偶数': [0, 0],
    }

    def tally(name, ok):
        checks[name][0 if ok else 1] += 1

    for v in voices_iter(parse(sweep)):
        e, note, regs = v
        setno = (e[2] << 7) | (e[3] & 0x7F)
        ent = wave_entry(rom, setno, note)
        if ent is not None and 0x12 in regs:
            tally('波形 0x12/0x13（ループ前の数）',
                  (regs[0x12] << 16 | regs[0x13]) == u32(rom, ent + 4))
            tally('波形 0x14/0x15（ループの長さ）',
                  (regs[0x14] << 16 | regs[0x15]) == u32(rom, ent + 8))
            tally('波形 0x16/0x17（形式と番地）',
                  (regs[0x16] << 16 | regs[0x17]) == u32(rom, ent + 12))
        if 0x07 in regs:
            tally('減衰 1 の目標 0x07 下位', (regs[0x07] & 0xFF) == ((0x7F - e[77]) * 2) & 0xFF)
        if 0x08 in regs:
            tally('減衰 2 の目標 0x08 下位', (regs[0x08] & 0xFF) == ((0x7F - e[78]) * 2) & 0xFF)
        if 0x06 in regs:
            # ワーク RAM の値は「byte73 の 2 倍」に鍵などの補正が入る。補正無しのときは合う
            tally('アタックの速さ 0x06 上位', (regs[0x06] >> 8) == rom[ATTACK_TAB + min(0x7F, e[73] * 2)])
        if 0x09 in regs:
            # 鍵を押したときの 0x09 は「音量の足し算」だけを 16bit で書くので、上位は 0。
            # 離しの速さはあとから別の所（0x12E34E ほか）が書く
            tally('鍵を押した時の 0x09 上位は 0', (regs[0x09] >> 8) == 0)
            tally('音量 0x09 下位は偶数', (regs[0x09] & 1) == 0)
        if 0x03 in regs:
            tally('フィルタ後の音量 0x03 = 0x5010', regs[0x03] == 0x5010)

    # 「ぴったり合うはず」の規則。ここが崩れたら、どこかを壊している
    EXACT = ('減衰 1 の目標 0x07 下位', '減衰 2 の目標 0x08 下位', 'アタックの速さ 0x06 上位',
             '鍵を押した時の 0x09 上位は 0', 'フィルタ後の音量 0x03 = 0x5010', '音量 0x09 下位は偶数')
    print('%-34s %8s %8s' % ('規則', '合う', '合わない'))
    bad = 0
    for k, (ok, ng) in checks.items():
        mark = '' if k in EXACT else '  （まだ近いだけ）'
        print('%-34s %8d %8d%s' % (k, ok, ng, mark))
        if k in EXACT:
            bad += ng
    print()
    print('ぴったり合うはずの規則で合わないもの: %d 件' % bad)
    return 1 if bad else 0


def voices_iter(voices):
    for v in voices:
        if v['elems'] != 1 or not v['elem']:
            continue
        e = v['elem'][0]
        if len(e) < 84:
            continue
        # 1 つの鍵で 1 スロットだけ鳴ったものを使う
        by_note = {}
        for note, ch, regs in v['notes']:
            by_note.setdefault(note, []).append(regs)
        for note, lst in by_note.items():
            if len(lst) == 1:
                yield e, note[0], lst[0]


if __name__ == '__main__':
    sys.exit(main())
