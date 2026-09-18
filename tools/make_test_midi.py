#!/usr/bin/env python3
# license:BSD-3-Clause
"""回帰試験に使う MIDI を組む。

**MIDI はリポジトリに置かない。** ここから毎回作る。中身が一目で分かり、
直したいときに直せるほうが、バイナリを置くより見通しがよい。

  python tools/make_test_midi.py [出力ディレクトリ]      既定 build/tests

各ファイルの狙いは CASES の表に書いてある。tools/run_tests.py が使う。
"""
import struct
import sys
from pathlib import Path

PPQN = 480
BPM120 = 500000               # マイクロ秒 / 四分音符。480 tick = 0.5 秒


def vlq(n):
    out = bytearray([n & 0x7f])
    n >>= 7
    while n:
        out.insert(0, (n & 0x7f) | 0x80)
        n >>= 7
    return bytes(out)


def sysex(data):
    """F0 ... F7 を SMF の形（長さ付き）で"""
    return b'\xf0' + vlq(len(data)) + bytes(data)


def xg(addr_and_data):
    """XG パラメータチェンジ。F0 43 10 4C <番地 3> <データ> F7"""
    return sysex([0x43, 0x10, 0x4c] + addr_and_data + [0xf7])


XG_RESET = xg([0x00, 0x00, 0x7e, 0x00])


def track(events, port=None):
    """events は (デルタ tick, バイト列) の並び"""
    body = b''
    if port is not None:
        body += vlq(0) + b'\xff\x21\x01' + bytes([port])
    for delta, b in events:
        body += vlq(delta) + b
    body += vlq(0) + b'\xff\x2f\x00'
    return b'MTrk' + struct.pack('>I', len(body)) + body


def write(path, tracks):
    fmt = 0 if len(tracks) == 1 else 1
    head = b'MThd' + struct.pack('>IHHH', 6, fmt, len(tracks), PPQN)
    Path(path).write_bytes(head + b''.join(tracks))


def sec(s):
    """秒を tick に"""
    return int(round(s * PPQN * 1e6 / BPM120))


def seq(items):
    """(時刻秒, バイト列) の並びを (デルタ, バイト列) に直す。時刻順に並べ替える"""
    items = sorted(items, key=lambda x: x[0])
    out, last = [], 0
    for t, b in items:
        tk = sec(t)
        out.append((tk - last, b))
        last = tk
    return out


def note(ch, key, vel, at, dur):
    return [(at, bytes([0x90 | ch, key, vel])),
            (at + dur, bytes([0x80 | ch, key, 0x40]))]


def head(extra=()):
    """テンポと XG リセット。リセットは効くまで間を置く"""
    ev = [(0.0, b'\xff\x51\x03' + struct.pack('>I', BPM120)[1:]),
          (0.0, XG_RESET)]
    ev += list(extra)
    return ev


# ---- 試験の本体
#
# 狙いが重ならないようにしてある。どれかが落ちたとき、何が壊れたかが分かる。

def case_piano():
    """一番素朴な一音。keyon のサンプル番地・形式、エンベロープ、減衰"""
    ev = head()
    ev += [(1.0, b'\xc0\x00')]                       # GrandPno
    ev += note(0, 60, 100, 1.25, 2.0)
    return [track(seq(ev))], 5.0


def case_chord():
    """和音。声の取り合いと、伸ばした音の重なり"""
    ev = head()
    ev += [(1.0, b'\xc0\x30')]                       # Strings
    for i, k in enumerate((55, 59, 62, 67, 71, 74)):
        ev += note(0, k, 90, 1.25 + i * 0.05, 2.2)
    return [track(seq(ev))], 5.0


def case_drums():
    """ドラム。8bit 圧縮サンプルを通る経路（MAME の展開器が不正確な場所）"""
    ev = head()
    for i, k in enumerate((36, 38, 42, 46, 49, 36, 38, 42)):
        ev += note(9, k, 110, 1.0 + i * 0.25, 0.1)
    return [track(seq(ev))], 4.5


def case_effects():
    """MEG を通す。リバーブ Hall1 + コーラス + バリエーション"""
    ev = head()
    ev += [(1.0, b'\xc0\x30')]
    ev += [(1.0, xg([0x02, 0x01, 0x00, 0x01, 0x00])),   # リバーブ Hall1
           (1.0, xg([0x02, 0x01, 0x20, 0x41, 0x00])),   # コーラス Chorus1
           (1.0, xg([0x02, 0x01, 0x40, 0x05, 0x00])),   # バリエーション Delay
           (1.05, b'\xb0\x5b\x7f'),                     # CC91 リバーブ送り
           (1.05, b'\xb0\x5d\x50'),                     # CC93 コーラス送り
           (1.05, b'\xb0\x5e\x40')]                     # CC94 バリエーション送り
    ev += note(0, 64, 100, 1.25, 1.5)
    return [track(seq(ev))], 6.0


def case_dense():
    """16 パートを同時に鳴らして声をスレーブ側の SWP30 まで溢れさせる。
    チップ間の MELO/MELI の受け渡しが壊れるとここで出る"""
    ev = head()
    progs = (0, 11, 19, 24, 30, 33, 40, 48, 56, 0, 65, 71, 73, 81, 89, 98)
    for ch in range(16):
        if ch == 9:
            continue                                  # 9 はドラム。音色は替えない
        ev.append((1.0, bytes([0xc0 | ch, progs[ch]])))
    for ch in range(16):
        base = 48 + ch
        for j, k in enumerate((base, base + 4, base + 7)):
            ev += note(ch, k, 88, 1.3 + ch * 0.02 + j * 0.01, 1.6)
    return [track(seq(ev))], 5.0


def case_port_b():
    """MIDI IN B（パート 17-32）。口ごとの振り分けが壊れると無音になる"""
    a = head()
    a += [(1.0, b'\xc0\x00')]
    a += note(0, 60, 100, 1.25, 1.5)
    b = [(1.0, b'\xc0\x30')]
    b += note(0, 72, 100, 1.4, 1.5)
    return [track(seq(a), port=0), track(seq(b), port=1)], 4.5


def case_bend():
    """ピッチベンドとモジュレーション。LFO とピッチの道"""
    ev = head()
    ev += [(1.0, b'\xc0\x50')]                        # Square Lead
    ev += [(1.1, b'\xb0\x01\x60')]                    # CC1 モジュレーション
    ev += note(0, 64, 100, 1.25, 2.5)
    for i in range(12):
        v = 8192 + int(4000 * (i + 1) / 12)
        ev += [(1.5 + i * 0.1, bytes([0xe0, v & 0x7f, v >> 7]))]
    ev += [(2.9, b'\xe0\x00\x40')]                    # 戻す
    return [track(seq(ev))], 5.5


def case_lofi():
    """分岐のある MEG プログラム（インサーションの LO-FI、種類 5E-00）。
    飛ばされた命令の後始末が壊れると、ここで JIT と解釈実行の音が変わる"""
    ev = head()
    ev += [(1.0, b'\xc0\x00'),                       # GrandPno
           (1.05, b'\xb0\x5e\x7f'),                   # CC94 バリエーション送り
           (1.05, xg([0x02, 0x01, 0x40, 0x5e, 0x00])),  # 種類: LO-FI
           (1.05, xg([0x02, 0x01, 0x5a, 0x00])),      # 接続: インサーション
           (1.05, xg([0x02, 0x01, 0x5b, 0x00]))]      # パート 1
    ev += note(0, 48, 110, 1.9, 2.0)
    ev += note(0, 55, 110, 4.3, 2.0)
    return [track(seq(ev))], 7.0


def case_egcc():
    """EG のつまみ（CC73 立ち上がり・CC75 減衰・CC72 離し）。
    この 3 つは式が起こせていない（CC73 は 0x06 だけでなく 0x00・0x07・0x0b も
    動かす）。native の口は**つまみが動いたら写し取り直す**ようにしてある。
    そこが壊れると、つまみを動かしたあとの音が既定のままになる。
    最後に既定へ戻すので、いちばん最初の写しが使い回せているかも見える"""
    ev = head()
    ev += [(1.0, b'\xc0\x30')]                       # Strings（立ち上がりが遅い）
    ev += note(0, 60, 100, 1.2, 0.8)                 # 既定。ここで写し取る
    ev += [(2.1, b'\xb0\x49\x14')]                   # CC73=20 立ち上がりを遅く
    ev += note(0, 62, 100, 2.2, 0.8)
    ev += [(3.1, b'\xb0\x49\x64')]                   # CC73=100 立ち上がりを速く
    ev += note(0, 64, 100, 3.2, 0.8)
    ev += [(4.1, b'\xb0\x4b\x14')]                   # CC75=20 減衰を遅く
    ev += note(0, 65, 100, 4.2, 0.8)
    ev += [(5.1, b'\xb0\x48\x64')]                   # CC72=100 離しを速く
    ev += note(0, 67, 100, 5.2, 0.8)
    ev += [(6.1, b'\xb0\x49\x40'), (6.1, b'\xb0\x4b\x40'),
           (6.1, b'\xb0\x48\x40')]                   # 既定に戻す
    ev += note(0, 60, 100, 6.2, 0.8)                 # 最初の写しが効くはず
    return [track(seq(ev))], 7.5


def case_porta():
    """ポルタメント（CC65 入切・CC5 速さ）。音程を鳴らしている間ずっと動かすので
    写し取り 1 回では足りない。native の口は ROM の表 0x1E6698 から式で滑らせる
    （doc/native-engine.md の 6.41）。ここが壊れると、滑らずに飛ぶ音になる"""
    ev = head()
    ev += [(1.0, b'\xc0\x50')]                       # Square Lead
    ev += note(0, 48, 100, 1.2, 0.6)                 # 1 音目。ここで写し取る
    ev += [(1.9, b'\xb0\x41\x7f'), (1.9, b'\xb0\x05\x40')]   # 入・CC5=64
    ev += note(0, 60, 100, 2.0, 1.0)                 # 48 から滑る
    ev += note(0, 55, 100, 3.1, 1.0)                 # 60 から滑る
    ev += [(4.2, b'\xb0\x05\x20')]                   # もっと速く
    ev += note(0, 67, 100, 4.3, 1.0)
    ev += [(5.4, b'\xb0\x41\x00')]                   # 切る
    ev += note(0, 60, 100, 5.5, 0.8)                 # ここは滑らない
    return [track(seq(ev))], 7.0


def case_at():
    """アフタータッチ。**割り当て（CAT）が既定のあいだは音に何も起きない**ので
    native のまま鳴らす（doc/native-engine.md の 6.43）。08 pp 4E（CAT フィルタ）を
    既定から外したら、そのパートは firmware に任せる。
    ここが壊れると、アフタータッチが効くはずの所で効かない音になる"""
    ev = head()
    ev += [(1.0, b'\xc0\x00')]                       # GrandPno
    ev += note(0, 60, 100, 1.2, 0.8)                 # 1 音目。ここで写し取る
    ev += note(0, 62, 100, 2.1, 0.8)                 # native
    ev += [(2.9, b'\xd0\x7f')]                       # 触れた強さ（既定なので効かない）
    ev += note(0, 64, 100, 3.0, 0.8)                 # native のまま
    ev += [(3.9, xg([0x08, 0x00, 0x4e, 0x00]))]      # CAT フィルタを既定から外す
    ev += [(4.0, b'\xd0\x7f')]
    ev += note(0, 65, 100, 4.1, 1.0)                 # ここからは firmware に任せる
    ev += [(5.2, b'\xd0\x00')]
    ev += note(0, 67, 100, 5.3, 0.8)
    return [track(seq(ev))], 6.5


CASES = {
    "piano":   case_piano,
    "chord":   case_chord,
    "drums":   case_drums,
    "effects": case_effects,
    "dense":   case_dense,
    "port_b":  case_port_b,
    "bend":    case_bend,
    "lofi":    case_lofi,
    "egcc":    case_egcc,
    "porta":   case_porta,
    "at":      case_at,
}


def build(out_dir):
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    made = {}
    for name, fn in CASES.items():
        tracks, seconds = fn()
        path = out / ("%s.mid" % name)
        write(path, tracks)
        made[name] = (path, seconds)
    return made


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "build/tests"
    made = build(out)
    for name, (path, seconds) in made.items():
        print("%-8s %s  %.1f 秒" % (name, path, seconds))
    return 0


if __name__ == "__main__":
    sys.exit(main())
