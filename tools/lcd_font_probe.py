#!/usr/bin/env python3
# license:BSD-3-Clause
"""**液晶のメーターの棒を、高さ 1-8 ぜんぶ 1 画面に並べる曲**を作る。

  python tools/lcd_font_probe.py [出力ディレクトリ]      既定 build/tests

液晶の字の絵（CGROM）は手に入っていない。メーターの棒（コード 0x7F-0xCF の
81 個）は `mu2000::fill_missing_glyphs` が規則から起こした作り物
（doc/native-engine.md の 6.148）。

**字形は決着済み**（2026-09-20）。この曲を実機で鳴らして画面を見てもらい、
棒の形が作り物と同じ（1 マスに 2 本、幅 2 ドット、左は 0-1 列・右は 3-4 列、
下から積む）と確かめた。違っていたのは `mulcd.zip` の ROM に入っている
`0x89` のほうで、実機は `##.##`、ROM は `#.#.#` だった。いまは
`0x80-0xCF` を ROM より優先して作り直している。

いまは、字形を確かめ直したいときや、メーターの見た目を実機と見比べたい
ときに使う。

**実機に流すので、頭は All Sound Off / All Notes Off だけ**にしてある
（XG System On や GS リセットは入れない。入れると実機を Performance モードに
戻せなくなる）。終わりにパートの音量を XG の既定（100）へ戻す。

見方:

  * 演奏画面（PLAY）にしておく
  * 音が出ている 10 秒の間に、液晶の左 9 マスを見る
  * 左端が A1/A2、その右がパート 1-16（1 マスに 2 パート）
"""
import struct
import sys
from pathlib import Path

PPQN = 480
BPM120 = 500000


def vlq(n):
    out = bytearray([n & 0x7f])
    n >>= 7
    while n:
        out.insert(0, (n & 0x7f) | 0x80)
        n >>= 7
    return bytes(out)


def track(events):
    body = b''
    for delta, b in events:
        body += vlq(delta) + b
    body += vlq(0) + b'\xff\x2f\x00'
    return b'MTrk' + struct.pack('>I', len(body)) + body


def seq(items):
    items = sorted(items, key=lambda x: x[0])
    out, last = [], 0
    for t, b in items:
        tk = int(round(t * PPQN * 1e6 / BPM120))
        out.append((tk - last, b))
        last = tk
    return out


# **1 マスに 2 本**（左＝偶数パート、右＝奇数パート）。8 マスで 16 パート。
# 高さ 1-8 が左右とも 1 度ずつ出るように組む
PAIRS = [(1, 2), (3, 4), (5, 6), (7, 8), (2, 1), (4, 3), (6, 5), (8, 7)]


# **その点の数になる強さ**。式から出すより測ったほうが確か（音量 127 で
# 実機の道を鳴らして、液晶に出た棒の字から引いた）
VEL_FOR_DOTS = { 1: 2, 2: 10, 3: 18, 4: 34, 5: 42, 6: 50, 7: 58, 8: 66 }


def main():
    out = Path(sys.argv[1] if len(sys.argv) > 1 else 'build/tests')
    out.mkdir(parents=True, exist_ok=True)
    ev = [(0.0, b'\xff\x51\x03' + struct.pack('>I', BPM120)[1:])]
    # 頭は音を止めるだけ（実機に流すため）
    for ch in range(16):
        ev += [(0.0, bytes([0xb0 | ch, 120, 0])),      # All Sound Off
               (0.0, bytes([0xb0 | ch, 123, 0]))]      # All Notes Off
    want = {}
    for cell, (a, b) in enumerate(PAIRS):
        for side, dots in ((0, a), (1, b)):
            ch = cell * 2 + side
            want[ch] = dots
            # 目盛り = 強さ x (パートの目盛り - 1) / 128。音量を 127 にすれば
            # パートの目盛りは 128 なので、目盛り ≒ 強さ
            ev += [(0.5, bytes([0xb0 | ch, 0x07, 127]))]
            vel = VEL_FOR_DOTS[dots]
            ev += [(1.0, bytes([0x90 | ch, 60, vel])),
                   (11.0, bytes([0x80 | ch, 60, 0x40]))]
    # 後始末。音量を XG の既定へ戻す
    for ch in range(16):
        ev += [(11.5, bytes([0xb0 | ch, 0x07, 100])),
               (11.6, bytes([0xb0 | ch, 120, 0])),
               (11.6, bytes([0xb0 | ch, 123, 0]))]
    path = out / 'lcdfont.mid'
    head = b'MThd' + struct.pack('>IHHH', 6, 0, 1, PPQN)
    path.write_bytes(head + track(seq(ev)))
    print('%s  12 秒（1.0 秒から 10 秒鳴る）' % path)
    print()
    print('出るはずの棒（左の点 / 右の点）:')
    for cell, (a, b) in enumerate(PAIRS):
        print('  下の行 %d 桁目  左 %d 点 / 右 %d 点   （今の作り物だと 0x%02X）'
              % (cell + 1, a, b, 0x7f + a * 9 + b))
    print()
    print('※ ドラムの ch10 も鳴らす。音が気になるなら音量を絞ってよい')
    print('※ パートの音量を 127 にして、終わりに 100 へ戻す')


if __name__ == '__main__':
    main()
