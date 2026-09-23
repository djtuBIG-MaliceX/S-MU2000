# 遅れは「バイト数」で決まるのか「メッセージの本数」で決まるのかを分ける MIDI を作る。
#
# clickload.py と同じ作り（基準音 → 荷物 → クリック）。こちらは**荷物のバイト数を揃えたまま
# 本数だけ**変える。区画 4・5・7 が短ければ本数で決まる（本体の 1 本ごとの手間が重い）、
# 区画 2 と同じくらい長ければバイト数で決まる（道が細い）。
#
# 実機で測った答えは**バイト数**だった（doc/native-engine.md の 6.218）。
# USB-MIDI の 4 バイト小包で見ると、CC 1 件も SysEx 3 バイトも小包 1 つで同じ重さになる。
#
# SysEx は製造元 ID 7D（非商用）なので MU2000 は中身を読まずに捨てる。設定は何も変わらない。
#
# 使い方:
#   python tools/latency/clickload2.py clickload2.mid
import struct
import sys

PPQ = 384
BPM = 110
BAR = PPQ * 4

# 1 チャンネルぶんの CC（24 種・72 バイト）。プログラムチェンジは入れない
CCS = [(10, 127), (7, 127), (101, 0), (100, 0), (6, 12), (10, 127), (7, 127), (91, 0),
       (93, 0), (74, 127), (71, 127), (72, 0), (75, 0), (73, 0), (1, 0), (76, 127),
       (65, 63), (5, 1), (77, 64), (76, 127), (78, 127), (11, 127), (74, 100), (71, 100)]


def vlq(n):
    out = bytearray([n & 0x7f])
    n >>= 7
    while n:
        out.insert(0, 0x80 | (n & 0x7f))
        n >>= 7
    return bytes(out)


def cc(ch, a, b):
    return bytes([0xb0 | ch, a, b])


def prog(ch, v):
    return bytes([0xc0 | ch, v])


def sysex(total):
    """F0 7D … F7 でちょうど total バイト"""
    return b'\xf0\x7d' + bytes((i % 0x78) for i in range(total - 3)) + b'\xf7'


# (名前, {チャンネル: CC の数}, SysEx の一覧)
SECTIONS = [
    ('なし',                {}, []),
    ('CC 336（14ch）',      {c: 24 for c in range(1, 15)}, []),
    ('CC 336（ch2 だけ）',  {1: 336}, []),
    ('SysEx 1008×1本',     {}, [sysex(1008)]),
    ('SysEx 84×12本',      {}, [sysex(84) for _ in range(12)]),
    ('CC 112（14ch）',      {c: 8 for c in range(1, 15)}, []),
    ('SysEx 336×1本',      {}, [sysex(336)]),
    ('なし(確認)',           {}, []),
]


def track(evs, name):
    out = bytearray()
    out += b'\x00\xff\x03' + vlq(len(name)) + name
    last = 0
    for t, b in sorted(evs, key=lambda x: x[0]):
        out += vlq(t - last)
        out += (b'\xf0' + vlq(len(b) - 1) + b[1:]) if b[0] == 0xf0 else b
        last = t
    out += b'\x00\xff\x2f\x00'
    return bytes(out)


def main(path):
    ref, clk, sysx = [], [], []
    flood = {c: [] for c in range(1, 15)}           # ch2..ch15
    for a, b in CCS:                                # ch16 を先に作っておく
        clk.append((0, cc(15, a, b)))
    clk.append((0, prog(15, 87)))

    pre = BAR
    report = []
    for i, (name, ccs, sxs) in enumerate(SECTIONS):
        t = pre + BAR * i
        ref.append((t - 1, bytes([0x90, 127, 127])))
        ref.append((t + 1, bytes([0x80, 127, 64])))
        nb = nm = 0
        for c, n in sorted(ccs.items()):
            for k in range(n):
                a, b = CCS[k % len(CCS)]
                flood[c].append((t, cc(c, a, b)))
                nb += 3
                nm += 1
        for s in sxs:
            sysx.append((t, s))
            nb += len(s)
            nm += 1
        clk.append((t + 1, bytes([0x9f, 59, 127])))
        clk.append((t + 3, bytes([0x8f, 59, 64])))
        report.append((name, nb, nm))

    trk = [b'\x00\xff\x51\x03' + struct.pack('>I', int(60e6 / BPM))[1:] + b'\x00\xff\x2f\x00',
           track(ref, b'ref ch1')]
    for c in range(1, 15):
        trk.append(track(flood[c], ('load ch%d' % (c + 1)).encode()))
    trk.append(track(sysx, b'sysex'))
    trk.append(track(clk, b'click ch16'))

    d = bytearray(b'MThd' + struct.pack('>IHHH', 6, 1, len(trk), PPQ))
    for t in trk:
        d += b'MTrk' + struct.pack('>I', len(t)) + t
    open(path, 'wb').write(bytes(d))

    print('書いた: %s（%d トラック、%d バイト）' % (path, len(trk), len(d)))
    print('区画                 バイト   本数   バイトで決まるなら   本数で決まるなら')
    for i, (name, nb, nm) in enumerate(report):
        print('  %d %-18s %5d %6d   %8.1f ms         %8.1f ms'
              % (i + 1, name, nb, nm, nb / 10.0, nm * 0.29))


main(sys.argv[1] if len(sys.argv) > 1 else 'clickload2.mid')
