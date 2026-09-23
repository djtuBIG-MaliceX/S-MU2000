# 「曲頭の遅れ」の中身を、口の速さと本体の手間に分けて測るための MIDI を作る。
#
# 実機で鳴らして録音し、clickread.py で読む（doc/native-engine.md の 6.218）。
# 各区画は
#   ch1 の基準音（1 tick 前・真ん中・小さい）→ 設定の荷物 → ch16 のクリック（1 tick 後・右いっぱい）
# の順に並ぶ。基準音は荷物より前に出るので、**基準音とクリックの差 = 荷物ぶんの遅れ**。
# 左右に振ってあるので、二つが重なっていても分けて読める。
#
# 荷物は 0・74・148・296・518・1,036・2,072 バイトと、プログラムチェンジだけ・CC だけ。
# 直線を当てれば口の速さが、prog だけの区画との差でプログラムチェンジの重さが出る。
#
# **1 トラックには 1 チャンネルだけ**にしてある（Domino は混ざったトラックを読まない）。
#
# 使い方:
#   python tools/latency/clickload.py clickload.mid
import struct
import sys

PPQ = 384
BPM = 110
BAR = PPQ * 4                      # 4 拍 ＝ 2.18 秒。DIN でも 2,072 バイトが収まる

# clicktest.mid（issue #15 の bryc 作）と同じ並び。1 チャンネルぶん 25 件・74 バイト
BLOCK = [
    ('cc', 10, 127), ('cc', 7, 127), ('bend', 0, 64), ('cc', 101, 0), ('cc', 100, 0),
    ('cc', 6, 12), ('cc', 10, 127), ('cc', 7, 127), ('bend', 0, 64), ('prog', 87, None),
    ('cc', 91, 0), ('cc', 93, 0), ('cc', 74, 127), ('cc', 71, 127), ('cc', 72, 0),
    ('cc', 75, 0), ('cc', 73, 0), ('cc', 1, 0), ('cc', 76, 127), ('cc', 65, 63),
    ('cc', 5, 1), ('cc', 77, 64), ('cc', 76, 127), ('cc', 78, 127), ('cc', 11, 127),
]

# (名前, チャンネル数, 繰り返し, 中身)。中身 all=全部 / prog=プログラムチェンジだけ / cc=CC だけ
SECTIONS = [
    ('なし',           0,  0, 'all'),
    ('1ch',            1,  1, 'all'),
    ('2ch',            2,  1, 'all'),
    ('4ch',            4,  1, 'all'),
    ('7ch',            7,  1, 'all'),
    ('14ch',          14,  1, 'all'),
    ('14ch x2',       14,  2, 'all'),
    ('progだけ',      14,  1, 'prog'),
    ('CCだけ',        14,  1, 'cc'),
    ('なし(確認)',     0,  0, 'all'),
]


def vlq(n):
    out = bytearray([n & 0x7f])
    n >>= 7
    while n:
        out.insert(0, 0x80 | (n & 0x7f))
        n >>= 7
    return bytes(out)


def enc(ch, ev):
    kind, a, b = ev
    if kind == 'cc':
        return bytes([0xb0 | ch, a, b])
    if kind == 'bend':
        return bytes([0xe0 | ch, a, b])
    if kind == 'prog':
        return bytes([0xc0 | ch, a])
    raise ValueError(kind)


def track(evs, name):
    out = bytearray()
    out += b'\x00\xff\x03' + vlq(len(name)) + name
    last = 0
    for t, b in sorted(evs, key=lambda x: x[0]):
        out += vlq(t - last) + b
        last = t
    out += b'\x00\xff\x2f\x00'
    return bytes(out)


def main(path):
    ref, clk = [], []
    flood = [[] for _ in range(14)]                 # ch2..ch15
    for ev in BLOCK:                                # ch16 を先に作っておく
        clk.append((0, enc(15, ev)))

    pre = BAR                                       # 1 小節目は ch16 の支度に使う
    report = []
    for i, (name, nch, rep, what) in enumerate(SECTIONS):
        t = pre + BAR * i
        ref.append((t - 1, bytes([0x90, 127, 127])))
        ref.append((t + 1, bytes([0x80, 127, 64])))
        nb = nm = 0
        for c in range(nch):
            for _ in range(rep):
                for ev in BLOCK:
                    if what == 'prog' and ev[0] != 'prog':
                        continue
                    if what == 'cc' and ev[0] == 'prog':
                        continue
                    b = enc(c + 1, ev)
                    flood[c].append((t, b))
                    nb += len(b)
                    nm += 1
        clk.append((t + 1, bytes([0x9f, 59, 127])))
        clk.append((t + 3, bytes([0x8f, 59, 64])))
        report.append((name, nb, nm))

    trk = [b'\x00\xff\x51\x03' + struct.pack('>I', int(60e6 / BPM))[1:] + b'\x00\xff\x2f\x00',
           track(ref, b'ref ch1')]
    for c in range(14):
        trk.append(track(flood[c], ('load ch%d' % (c + 2)).encode()))
    trk.append(track(clk, b'click ch16'))

    d = bytearray(b'MThd' + struct.pack('>IHHH', 6, 1, len(trk), PPQ))
    for t in trk:
        d += b'MTrk' + struct.pack('>I', len(t)) + t
    open(path, 'wb').write(bytes(d))

    print('書いた: %s（%d トラック、%d バイト）' % (path, len(trk), len(d)))
    print('区画            荷物バイト   本数   口だけなら DIN /   USB(10,000)')
    for i, (name, nb, nm) in enumerate(report):
        print('  %2d %-12s %6d %6d   %8.1f ms / %8.1f ms'
              % (i + 1, name, nb, nm, nb / 3.125, nb / 10.0))


main(sys.argv[1] if len(sys.argv) > 1 else 'clickload.mid')
