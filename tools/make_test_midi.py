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
    """ドラム。8bit 圧縮サンプルを通る経路（MAME の展開器が不正確な場所）と、
    **ドラムセットアップ**（XG の 3n rr nn）。

    セットアップは式が起こせていないので、値が変わったら写し取り直す
    （doc/native-engine.md の 6.107）。印に混ぜ忘れると、1 打ごとの高さ・
    音量・パン・リバーブ送りがまるごと効かない"""
    ev = head()
    for i, k in enumerate((36, 38, 42, 46, 49, 36, 38, 42)):
        ev += note(9, k, 110, 1.0 + i * 0.25, 0.1)
    # 鍵 36 の高さ・音量・パン・リバーブ送りを動かして、もう一度打つ
    t = 3.2
    for lo, val in ((0x00, 64 + 12), (0x02, 90), (0x04, 20), (0x05, 127)):
        ev += [(t, xg([0x30, 36, lo, val]))]
        t += 0.05
    for i, k in enumerate((36, 38, 36)):
        ev += note(9, k, 110, t + i * 0.3, 0.1)
    t += 1.1
    # 戻す（既定は高さ 64・音量 127・パン 64・送り 127 ではないので、
    # 「戻す」のではなく別の値にして、取り直しがもう一度走ることを見る）
    ev += [(t, xg([0x30, 36, 0x00, 64 - 12]))]
    ev += note(9, 36, 110, t + 0.15, 0.1)
    return [track(seq(ev))], t + 1.5


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


def case_sxparam():
    """エフェクトの**パラメータ**を鳴らしながら流す曲。種類は頭で 1 回決めるだけ。
    実機は種類を変えるとき MEG のプログラムを書き直して 176-212ms 掛かるが、
    値を変えるだけなら 0-4ms で終わる（nativeplay --sxsettle）。native の口は
    そこを見分けて SH-2 を長く回さない（doc/native-engine.md の 6.44）。
    音を短く並べてあるのは、長い音を 1 つ伸ばすと「firmware の音」で
    回しっぱなしになって SysEx のぶんが埋もれてしまうため"""
    ev = head()
    ev += [(1.0, b'\xc0\x30')]                        # Strings
    ev += [(1.0, xg([0x02, 0x01, 0x00, 0x01, 0x00])), # リバーブ Hall1（種類＝重い）
           (1.05, b'\xb0\x5b\x7f')]                   # CC91 リバーブ送り
    # 1 音目だけ写し取り、あとは native で鳴る
    for i in range(12):
        ev += note(0, 60 + i, 100, 1.4 + i * 0.35, 0.3)
    # その間、リバーブのパラメータだけを振る（種類は変えない）
    for i in range(12):
        ev += [(1.6 + i * 0.35, xg([0x02, 0x01, 0x02, 0x08 + i]))]
    return [track(seq(ev))], 6.5


# pedals の回で使う生のイベント
PROG48     = b'\xc0\x30'
DAMPER_ON  = b'\xb0\x40\x7f'
DAMPER_OFF = b'\xb0\x40\x00'
SOST_ON    = b'\xb0\x42\x7f'
SOST_OFF   = b'\xb0\x42\x00'
SOFT_ON    = b'\xb0\x43\x7f'
SOFT_OFF   = b'\xb0\x43\x00'
KEY64_ON   = b'\x90\x40\x64'
KEY64_OFF  = b'\x80\x40\x00'
VIB_RATE_A = b'\xb0\x4c\x20'
VIB_DEPTH_A= b'\xb0\x4d\x60'
VIB_DELAY_A= b'\xb0\x4e\x10'
VIB_RATE_B = b'\xb0\x4c\x40'
VIB_DEPTH_B= b'\xb0\x4d\x40'
VIB_DELAY_B= b'\xb0\x4e\x40'


def case_pedals():
    """ペダルとビブラート。

    CC64（ダンパー）・CC66（ソステヌート）・CC67（ソフト）と、
    CC76-78（ビブラートの速さ・深さ・遅れ）。

    ソステヌートは**踏んだ時点で鳴っている音だけ**を待たせるので、踏んだあとに
    押した鍵は普通に離れる。native の口がそこを取り違えると、あとの音が
    鳴りっぱなしになるか、待たせるはずの音が切れる。
    ビブラートの 3 つは式が起こせていないので**写し取り直し**で合わせている
    （doc/native-engine.md の 6.103）。印に混ぜ忘れると、つまみが効かない"""
    ev = head()
    ev += [(1.0, PROG48)]                            # Strings（伸びる音）
    ev += note(0, 60, 100, 1.2, 0.5)                 # 1 音目。ここで写し取る
    # ダンパー: 踏んでいる間は離しても鳴りつづける
    ev += [(1.9, DAMPER_ON)]
    ev += note(0, 62, 100, 2.0, 0.3)
    ev += [(2.6, DAMPER_OFF)]                        # 離す → ここで切れる
    # ソステヌート: 踏んだ時点の音だけ待たせる
    ev += [(3.0, KEY64_ON), (3.2, SOST_ON), (3.4, KEY64_OFF)]
    ev += note(0, 67, 100, 3.5, 0.3)                 # これは普通に離れる
    ev += [(4.2, SOST_OFF)]                          # 離す → 鍵 64 が切れる
    # ソフトペダル
    ev += [(4.5, SOFT_ON)]
    ev += note(0, 60, 100, 4.6, 0.4)
    ev += [(5.1, SOFT_OFF)]
    # ビブラート（次の音から効く）
    ev += [(5.3, VIB_RATE_A), (5.3, VIB_DEPTH_A), (5.3, VIB_DELAY_A)]
    ev += note(0, 64, 100, 5.5, 0.8)
    ev += [(6.5, VIB_RATE_B), (6.5, VIB_DEPTH_B), (6.5, VIB_DELAY_B)]
    ev += note(0, 64, 100, 6.7, 0.8)
    return [track(seq(ev))], 8.0


def case_partsx():
    """パートの設定（XG の 08 pp nn）。

    ノートシフト（08）・デチューン（09/0A）・ベロシティ感度の深さ（0C）と
    ずらし（0D）・素通しの量（11）。実機は**鍵を移し、強さを掛けてから**
    音色を選ぶので、native の口が同じ順でやらないと別の波形を鳴らす。
    doc/native-engine.md の 6.104"""
    ev = head()
    ev += [(1.0, bytes([0xc0, 48]))]                 # Strings
    ev += note(0, 60, 100, 1.2, 0.5)                 # 1 音目。ここで写し取る
    # ノートシフト（+7 半音 → -5 半音 → 戻す）
    ev += [(1.9, xg([0x08, 0x00, 0x08, 64 + 7]))]
    ev += note(0, 60, 100, 2.0, 0.5)
    ev += [(2.7, xg([0x08, 0x00, 0x08, 64 - 5]))]
    ev += note(0, 60, 100, 2.8, 0.5)
    ev += [(3.5, xg([0x08, 0x00, 0x08, 64]))]
    # デチューン（08 pp 09/0A。1/10 セント単位の 14bit）
    ev += [(3.6, xg([0x08, 0x00, 0x09, 0x08])), (3.6, xg([0x08, 0x00, 0x0a, 0x00]))]
    ev += note(0, 62, 100, 3.7, 0.5)
    ev += [(4.4, xg([0x08, 0x00, 0x09, 0x08])), (4.4, xg([0x08, 0x00, 0x0a, 0x00]))]
    # ベロシティ感度の深さ
    ev += [(4.5, xg([0x08, 0x00, 0x0c, 32]))]
    ev += note(0, 64, 100, 4.6, 0.5)
    ev += [(5.3, xg([0x08, 0x00, 0x0c, 96]))]
    ev += note(0, 64, 100, 5.4, 0.5)
    ev += [(6.1, xg([0x08, 0x00, 0x0c, 64]))]
    # ベロシティ感度のずらし
    ev += [(6.2, xg([0x08, 0x00, 0x0d, 32]))]
    ev += note(0, 65, 100, 6.3, 0.5)
    ev += [(7.0, xg([0x08, 0x00, 0x0d, 96]))]
    ev += note(0, 65, 100, 7.1, 0.5)
    ev += [(7.8, xg([0x08, 0x00, 0x0d, 64]))]
    # 素通しの量
    ev += [(7.9, xg([0x08, 0x00, 0x11, 64]))]
    ev += note(0, 67, 100, 8.0, 0.5)
    # **マスター移調**（00 00 06）。鍵を移してから音色を選ぶので、波形も動く
    ev += [(8.6, xg([0x00, 0x00, 0x06, 64 + 5]))]
    ev += note(0, 60, 100, 8.7, 0.5)
    ev += [(9.3, xg([0x00, 0x00, 0x06, 64 - 9]))]
    ev += note(0, 60, 100, 9.4, 0.5)
    ev += [(10.0, xg([0x00, 0x00, 0x06, 64]))]
    # **マスター音量**（00 00 04）。パートの音量と同じ形で目盛りに掛かる
    ev += [(10.1, xg([0x00, 0x00, 0x04, 88]))]
    ev += note(0, 62, 100, 10.2, 0.5)
    ev += [(10.8, xg([0x00, 0x00, 0x04, 127]))]
    ev += note(0, 62, 100, 10.9, 0.5)
    return [track(seq(ev))], 12.0


def case_keylevel():
    """**鍵と強さで音量・波形が動く音色**。今日いちばん大きかった直しの番人。

    * Bottle(76) … 鍵の音量曲線がいちばん急（鍵 36 と 84 で 45 段動く）。
      1.5 倍のままだと鍵 36 が 35dB 明るい（doc/native-engine.md の 6.108）
    * Bottle・SoundTrk(97) … 強さの曲線が行 1（要素の byte68）。行 0 の
      ままだと強さぶんの減衰が 1 ずれる（6.109）
    * Rain(96) … 鍵 84 で波形の記録が変わる。追従と支点を入れないと
      1 つ先の記録を鳴らす（6.110）
    * PickBass(34) … 鍵の曲線が Bottle の逆向き

    鍵は 36/60/84、強さは 30/127。**音を重ねない**（0.75 秒おきに 0.35 秒）。
    重ねると離しの尾どうしが混ざって、音量そのものが見えなくなる"""
    ev = head()
    t = 1.0
    for prog in (76, 97, 96, 34):
        ev += [(t, bytes([0xc0, prog]))]
        t += 0.2
        for key in (36, 60, 84):
            for vel in (30, 127):
                ev += note(0, key, vel, t, 0.35)
                t += 0.75
        t += 0.3
    return [track(seq(ev))], t + 1.5


def rpn(num_msb, num_lsb, data_msb, data_lsb=None):
    """RPN を 1 つ。番号を選んで値を入れ、最後に 0x7F 0x7F で閉じる"""
    ev = [b'\xb0\x65' + bytes([num_msb]), b'\xb0\x64' + bytes([num_lsb]),
          b'\xb0\x06' + bytes([data_msb])]
    if data_lsb is not None:
        ev.append(b'\xb0\x26' + bytes([data_lsb]))
    ev += [b'\xb0\x65\x7f', b'\xb0\x64\x7f']
    return ev


def nrpn(num_msb, num_lsb, data_msb):
    """NRPN を 1 つ。XG は閉じない（実機の曲もそうしている）"""
    return [b'\xb0\x63' + bytes([num_msb]), b'\xb0\x62' + bytes([num_lsb]),
            b'\xb0\x06' + bytes([data_msb])]


def spread(t, msgs, gap=0.01):
    """並びに時刻を振る（MIDI の線を詰まらせないよう少し空ける）"""
    return [(t + i * gap, m) for i, m in enumerate(msgs)]


def case_rpn():
    """**RPN と NRPN**。native の口はこの CC をさばかず firmware に回すので、
    firmware の RAM から拾い直せているかを見る（xg/native_driver.h の sync_cc）。

    * RPN 0 … ベンド幅。2 半音の既定から 12 半音に広げてベンドする
    * RPN 1 … 微調（ピッチベンドセンシティビティではなく音程そのもの）
    * RPN 2 … 粗調（半音単位）
    * NRPN 01 08/09/0A … ビブラートの速さ・深さ・遅れ
    * NRPN 01 20/21 … フィルタの切る高さ・共振
    * NRPN 01 63/64/66 … EG の立ち上がり・減衰・離し

    ここが壊れると、ベンド幅を広げた曲の音程や、NRPN で音を作る曲が狂う"""
    ev = head()
    ev += [(1.0, b'\xc0\x50')]                       # Square Lead
    ev += note(0, 60, 100, 1.2, 0.8)                  # 1 音目。ここで写し取る
    # RPN 0: ベンド幅を 12 半音に
    ev += spread(2.0, rpn(0, 0, 12))
    ev += note(0, 60, 100, 2.2, 1.0)
    ev += [(2.5, b'\xe0\x00\x60'), (2.8, b'\xe0\x00\x20'),
           (3.1, b'\xe0\x00\x40')]                  # 上げて下げて戻す
    # RPN 1: 微調（+50 セントくらい）
    ev += spread(3.4, rpn(0, 1, 0x48))
    ev += note(0, 60, 100, 3.6, 0.8)
    ev += spread(4.5, rpn(0, 1, 0x40))                # 戻す
    # RPN 2: 粗調（-3 半音）
    ev += spread(4.7, rpn(0, 2, 61))
    ev += note(0, 60, 100, 4.9, 0.8)
    ev += spread(5.8, rpn(0, 2, 64))                  # 戻す
    # NRPN: ビブラート
    ev += spread(6.0, nrpn(1, 0x08, 0x50) + nrpn(1, 0x09, 0x60) + nrpn(1, 0x0a, 0x10))
    ev += note(0, 60, 100, 6.4, 1.2)
    # NRPN: フィルタ
    ev += spread(7.8, nrpn(1, 0x20, 0x20) + nrpn(1, 0x21, 0x50))
    ev += note(0, 60, 100, 8.2, 0.8)
    # NRPN: EG
    ev += spread(9.1, nrpn(1, 0x63, 0x20) + nrpn(1, 0x64, 0x60) + nrpn(1, 0x66, 0x60))
    ev += note(0, 60, 100, 9.5, 1.0)
    return [track(seq(ev))], 11.0


def case_mono():
    """**モノ / ポリ**（CC126・CC127）とキーアサイン。
    モノのパートは後から押した鍵が前の音を止める。native が止め忘れると
    音が重なって 3dB 大きくなる"""
    ev = head()
    ev += [(1.0, b'\xc0\x50')]
    ev += note(0, 60, 100, 1.2, 0.8)                  # 1 音目。ここで写し取る
    ev += [(2.1, b'\xb0\x7e\x01')]                  # CC126 モノ
    # 重ねて押す（モノなら後の音だけ残る）
    ev += note(0, 60, 100, 2.3, 1.2)
    ev += note(0, 64, 100, 2.7, 0.8)
    ev += note(0, 67, 100, 3.1, 0.8)
    ev += [(4.2, b'\xb0\x7f\x00')]                  # CC127 ポリに戻す
    ev += note(0, 60, 100, 4.4, 1.2)
    ev += note(0, 64, 100, 4.8, 0.8)
    ev += note(0, 67, 100, 5.2, 0.8)
    return [track(seq(ev))], 7.0


def case_ports():
    """**MIDI IN A-D の 4 口**（パート 1-64）。C と D は USB でしか来ないので、
    `run_tests.py` は `--usb` で鳴らす（`step_usb`）。

    口ごとの振り分け・パートの番号の付け方・64 パートぶんのスロットの
    取り合いを一度に見る。ここが壊れると、3 口目から先が無音になるか、
    別のパートの音で鳴る"""
    tracks = []
    progs = (0x00, 0x30, 0x50, 0x0b)
    keys = (48, 55, 62, 69)
    for port in range(4):
        ev = head() if port == 0 else []
        ev += [(1.0, bytes([0xc0, progs[port]]))]
        ev += note(0, keys[port], 100, 1.3 + port * 0.12, 1.4)
        # 2 音目は native が鳴らす
        ev += note(0, keys[port] + 3, 100, 3.0 + port * 0.12, 1.2)
        tracks.append(track(seq(ev), port=port))
    return tracks, 5.5


def case_ctlreset():
    """**CC120 オールサウンドオフ・CC121 コントローラリセット・CC123 オールノートオフ**。

    * CC123 は押している鍵を**離す**（離しの尾は残る）
    * CC120 は**その場で切る**（尾も残らない）
    * CC121 はつまみを既定に戻す（音量・パン・ベンドなど）

    ここが壊れると、曲の切り替わりで音が残るか、逆に切れすぎる"""
    ev = head()
    ev += [(1.0, b'\xc0\x30')]                      # Strings（尾が長い）
    ev += note(0, 60, 100, 1.2, 0.8)                 # 1 音目。ここで写し取る
    # CC123 オールノートオフ（離す）
    ev += [(2.1, bytes([0x90, 64, 100])), (2.1, bytes([0x90, 67, 100]))]
    ev += [(2.6, b'\xb0\x7b\x00')]
    # CC120 オールサウンドオフ（その場で切る）
    ev += [(3.4, bytes([0x90, 60, 100])), (3.4, bytes([0x90, 64, 100]))]
    ev += [(3.9, b'\xb0\x78\x00')]
    # つまみを動かしてから CC121 で戻す
    ev += [(4.5, b'\xb0\x07\x40'), (4.55, b'\xb0\x0a\x20'),
           (4.6, b'\xe0\x00\x60')]
    ev += note(0, 62, 100, 4.8, 0.8)
    ev += [(5.8, b'\xb0\x79\x00')]                  # CC121 リセット
    ev += note(0, 62, 100, 6.0, 0.8)                 # 既定に戻っているはず
    return [track(seq(ev))], 7.5


def case_scale():
    """**スケールチューニング**（XG の 08 pp 41-4C。C から B まで 12 個、
    64 が 0 セント）と**パートの EQ**（08 pp 6A-6F）。

    スケールチューニングは音名ごとに音程をずらす（純正律などを作るためのもの）。
    パートの EQ は写し取りの「経路の印」に入っているので、動かしたら
    取り直しが走るはず。ここが壊れると、和音の中の特定の音だけ音程が違う"""
    ev = head()
    ev += [(1.0, b'\xc0\x50')]                       # Square Lead
    ev += note(0, 60, 100, 1.2, 0.8)                  # 1 音目。ここで写し取る
    # C を +30 セント、E を -20 セント、G を +50 セント
    t = 2.1
    for lo, val in ((0x41, 64 + 30), (0x45, 64 - 20), (0x48, 64 + 50)):
        ev += [(t, xg([0x08, 0x00, lo, val]))]
        t += 0.05
    for i, k in enumerate((60, 64, 67)):
        ev += note(0, k, 100, t + i * 0.7, 0.6)
    t += 2.2
    # 戻す
    for lo in (0x41, 0x45, 0x48):
        ev += [(t, xg([0x08, 0x00, lo, 64]))]
        t += 0.05
    ev += note(0, 60, 100, t + 0.2, 0.6)              # 最初の写しが効くはず
    t += 1.0
    # パートの EQ（低域を上げる・高域を下げる）
    ev += [(t, xg([0x08, 0x00, 0x72, 64 + 12]))]      # EQ 低域のゲイン
    ev += [(t + 0.05, xg([0x08, 0x00, 0x73, 64 - 12]))]  # EQ 高域のゲイン
    ev += note(0, 60, 100, t + 0.3, 0.8)
    return [track(seq(ev))], t + 2.0


def case_kits():
    """**ドラムキットの切り替え**（バンク MSB 127）と**SFX バンク**（MSB 64）。

    キットごとに 1 打の記録が別の場所にあるので、引き方が壊れると
    別の音が鳴るか無音になる。SFX バンクは旋律の音色と同じ引き方だが
    組が違う（doc/native-engine.md の 6.111）"""
    ev = head()
    # ドラム（チャンネル 10）でキットを替えながら打つ
    t = 1.0
    for kit in (0, 8, 16, 32, 40):
        ev += [(t, b'\xb9\x00\x7f'), (t + 0.02, b'\xb9\x20\x00'),
               (t + 0.04, bytes([0xc9, kit]))]
        for i, k in enumerate((36, 38, 42)):
            ev += note(9, k, 110, t + 0.2 + i * 0.2, 0.1)
        t += 1.0
    # SFX バンク（MSB 64）をチャンネル 1 で
    ev += [(t, b'\xb0\x00\x40'), (t + 0.02, b'\xb0\x20\x00'),
           (t + 0.04, b'\xc0\x7c')]                  # Telephone 系
    ev += note(0, 60, 100, t + 0.3, 0.8)
    ev += note(0, 67, 100, t + 1.3, 0.8)
    return [track(seq(ev))], t + 3.0


def case_ins2():
    """**インサーションを 2 系統**、別々のパートに掛ける。
    `lofi` は 1 系統だけ。2 つ目（02 01 60-）の番地と、パートごとの
    「経路の印」（写し取りの使い回し）が別々に効いているかを見る。
    ここが壊れると、片方のパートに両方の効果が乗るか、掛からない"""
    ev = head()
    ev += [(1.0, b'\xc0\x00'), (1.0, b'\xc1\x30')]   # ch1 GrandPno / ch2 Strings
    # インサーション 1 を ch1 に（DISTORTION）
    ev += [(1.05, xg([0x02, 0x01, 0x40, 0x4c, 0x00])),
           (1.05, xg([0x02, 0x01, 0x5a, 0x00])),        # 接続: インサーション
           (1.05, xg([0x02, 0x01, 0x5b, 0x00]))]        # パート 1
    # インサーション 2 を ch2 に（PHASER）
    ev += [(1.10, xg([0x03, 0x00, 0x00, 0x48, 0x00])),
           (1.10, xg([0x03, 0x00, 0x0a, 0x01]))]        # パート 2
    ev += [(1.15, b'\xb0\x5e\x7f')]                   # ch1 バリエーション送り
    ev += note(0, 48, 110, 1.6, 1.4)
    ev += note(1, 60, 110, 1.8, 1.4)
    ev += note(0, 55, 110, 3.4, 1.2)
    ev += note(1, 67, 110, 3.6, 1.2)
    return [track(seq(ev))], 6.5


def case_progchg():
    """**鳴らしている最中に音色とバンクを替える**。実機は鳴っている音は
    そのままで、つぎの鍵から新しい音色になる。native は音色を自分で引く
    （firmware の RAM を待たない）ので、切り替えの時刻がずれると
    1 音だけ古い音色で鳴る。実際の曲がよくやる形"""
    ev = head()
    ev += [(1.0, b'\xc0\x00')]                        # GrandPno
    ev += note(0, 60, 100, 1.2, 1.6)                   # 伸ばしている間に替える
    ev += [(1.9, b'\xc0\x30')]                        # Strings へ
    ev += note(0, 64, 100, 2.2, 1.0)
    ev += [(3.3, b'\xb0\x00\x08'), (3.32, b'\xb0\x20\x00'),
           (3.34, b'\xc0\x30')]                       # MSB 8 の変化音色
    ev += note(0, 67, 100, 3.6, 1.0)
    ev += [(4.7, b'\xb0\x00\x00'), (4.72, b'\xc0\x50')]   # 元のバンクで Square Lead
    ev += note(0, 60, 100, 5.0, 1.0)
    return [track(seq(ev))], 7.0


def case_running():
    """**ランニングステータス**（状態バイトを省く書き方）と、**鍵 0 での
    ノートオフ**（9n kk 00）。実際の MIDI ファイルはどちらも当たり前に使う
    のに、ここまでの試験は 1 度も使っていなかった。

    native の口は MIDI を 1 バイトずつ自分で読むので（`native_midi`）、
    状態バイトの持ち越しを取りこぼすと、音が鳴らないか別の鍵で鳴る。
    ノートオフに掛かる時間も変わる（2 バイトぶん短い）ので、
    鳴らし始める時刻にも効く"""
    ev = head()
    ev += [(1.0, b'\xc0\x30')]                       # Strings
    # 1 音目は普通に（ここで写し取る）
    ev += note(0, 60, 100, 1.2, 0.6)
    # ここから状態バイトを 1 度だけ送って、あとはデータだけ
    ev += [(2.0, b'\x90\x40\x64')]                   # 9n 付き
    ev += [(2.3, b'\x43\x64')]                       # ランニング（鍵 67 を押す）
    ev += [(2.6, b'\x40\x00')]                       # ランニング（鍵 64 を離す ＝ 強さ 0）
    ev += [(2.9, b'\x47\x64')]                       # ランニング（鍵 71 を押す）
    ev += [(3.2, b'\x43\x00')]                       # 鍵 67 を離す
    ev += [(3.5, b'\x47\x00')]                       # 鍵 71 を離す
    # 和音をランニングで一気に
    ev += [(4.0, b'\x90\x30\x64'), (4.0, b'\x34\x64'), (4.0, b'\x37\x64')]
    ev += [(5.0, b'\x30\x00'), (5.0, b'\x34\x00'), (5.0, b'\x37\x00')]
    return [track(seq(ev))], 7.0


def case_pat():
    """**ポリフォニックアフタータッチ**（An kk vv）。割り当てが既定の
    あいだは音に何も起きない（`at` の CAT と同じ）。割り当てを外したら
    firmware に任せる。ここが壊れると、PAT を送る曲で音が変わる"""
    ev = head()
    ev += [(1.0, b'\xc0\x30')]
    ev += note(0, 60, 100, 1.2, 0.8)                  # 1 音目。ここで写し取る
    ev += note(0, 62, 100, 2.1, 1.2)
    ev += [(2.5, b'\xa0\x3e\x7f')]                  # PAT（既定なので効かない）
    ev += note(0, 64, 100, 3.5, 1.2)
    ev += [(3.9, xg([0x08, 0x00, 0x57, 0x40]))]       # PAT をフィルタに割り当て
    ev += [(4.2, b'\xa0\x40\x7f')]                  # ここからは firmware へ
    ev += note(0, 65, 100, 5.0, 1.0)
    ev += [(6.1, xg([0x08, 0x00, 0x57, 0x40]))]
    return [track(seq(ev))], 7.5


def nrpn_note(kind, note, value):
    """ドラムの NRPN（MSB が種類、LSB が鍵の番号）"""
    return [b'\xb9\x63' + bytes([kind]), b'\xb9\x62' + bytes([note]),
            b'\xb9\x06' + bytes([value])]


def case_ccramp():
    """**伸ばしている音につまみを刻む**（CC7 音量・CC10 パン・CC91 リバーブ送り・
    CC11 エクスプレッション）。実際の曲がいちばんよくやる形（音量の山・
    パンの振り・送りの出し入れ）なのに、ここまでの試験は「鍵を押す前に
    決めて、鳴っている間は動かさない」ものばかりだった。

    native は鳴っているスロットを 1 つずつ書き直す（`apply_cc`）。
    離しの最中の音も追う必要がある（6.47）ので、そこも一緒に見る"""
    ev = head()
    ev += [(1.0, bytes([0xc0, 0x30]))]                # Strings（伸びる）
    ev += note(0, 60, 100, 1.2, 0.8)                  # 1 音目。ここで写し取る
    ev += note(0, 62, 100, 2.2, 2.6)                  # 伸ばす
    for i in range(8):                                # CC7 を落としていく
        ev += [(2.5 + i * 0.3, bytes([0xb0, 0x07, 100 - i * 10]))]
    for i in range(6):                                # CC10 を振る
        ev += [(2.6 + i * 0.4, bytes([0xb0, 0x0a, 20 + i * 18]))]
    ev += [(4.9, bytes([0xb0, 0x07, 100]))]           # 戻す
    # 離したあとも動かす（離しの尾を追えているか）
    ev += note(0, 65, 100, 5.2, 0.5)
    for i in range(5):
        ev += [(5.8 + i * 0.2, bytes([0xb0, 0x07, 100 - i * 20]))]
    ev += [(6.9, bytes([0xb0, 0x07, 100])), (6.95, bytes([0xb0, 0x0a, 64]))]
    # エクスプレッションと送り
    ev += note(0, 67, 100, 7.2, 1.6)
    for i in range(6):
        ev += [(7.4 + i * 0.25, bytes([0xb0, 0x0b, 127 - i * 18]))]
    for i in range(4):
        ev += [(7.5 + i * 0.35, bytes([0xb0, 0x5b, 20 + i * 30]))]
    return [track(seq(ev))], 10.0


def case_midreset():
    """**曲の途中でのリセット**（XG System On・GM System On）と
    **マスターチューン**（00 00 00-03）。実際の曲は頭以外でもリセットを
    入れることがあるし、マスターチューンで全体の音程をずらす曲もある。

    リセットはパートの設定も音色も全部戻すので、native の写し取りの
    「経路の印」が付いていかないと、古いつまみのまま鳴る。

    **実機にこの MIDI を流してはいけない**（モードが変わって戻せない）。
    エミュレータの中だけで使う試験"""
    ev = head()
    ev += [(1.0, bytes([0xc0, 0x50]))]                # Square Lead
    ev += [(1.05, bytes([0xb0, 0x07, 60])),           # 音量を絞る
           (1.06, bytes([0xb0, 0x0a, 20]))]           # 左へ振る
    ev += note(0, 60, 100, 1.3, 0.8)                  # 1 音目。ここで写し取る
    ev += note(0, 64, 100, 2.2, 0.8)                  # native
    # マスターチューン（00 00 00-03。4 バイトの下 4bit で 12bit の値。
    # 0x400 が中央、1 きざみ 0.1 セント）
    ev += [(3.1, xg([0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00]))]
    ev += note(0, 60, 100, 3.4, 0.8)                  # 音程が上がるはず
    ev += [(4.3, xg([0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00]))]   # 戻す
    # 曲の途中で XG System On（音量もパンも音色も既定に戻る）
    ev += [(4.6, XG_RESET)]
    ev += note(0, 60, 100, 5.3, 0.8)                  # GrandPno・音量 100 のはず
    # GM System On
    ev += [(6.2, sysex([0x7e, 0x7f, 0x09, 0x01, 0xf7]))]
    ev += note(0, 64, 100, 6.9, 0.8)
    return [track(seq(ev))], 8.5


def case_partmode():
    """**パートの種類**（08 pp 07。0 が旋律・2-5 がドラム 1-4）。
    旋律のパートをドラムにしたり、10 番をドラムから旋律に戻したりする。
    native は種類で写し取りの引き方を変える（`is_drum`）ので、
    切り替えに付いていけないと、鍵の番号の音色が鳴るか無音になる"""
    ev = head()
    ev += [(1.0, bytes([0xc0, 0x00]))]                # ch1 GrandPno
    ev += note(0, 60, 100, 1.2, 0.6)                  # 旋律として 1 音目
    # ch1 をドラム（ドラム 1）にする
    ev += [(2.0, xg([0x08, 0x00, 0x07, 0x02]))]
    for i, k in enumerate((36, 38, 42)):
        ev += note(0, k, 110, 2.3 + i * 0.3, 0.1)
    # 旋律に戻す
    ev += [(3.5, xg([0x08, 0x00, 0x07, 0x00]))]
    ev += [(3.6, bytes([0xc0, 0x30]))]                # Strings
    ev += note(0, 62, 100, 3.9, 0.8)
    # ch10 をドラムから旋律にする
    ev += [(4.9, xg([0x08, 0x09, 0x07, 0x00]))]
    ev += [(5.0, bytes([0xc9, 0x00]))]                # GrandPno
    ev += note(9, 60, 100, 5.3, 0.8)
    ev += [(6.3, xg([0x08, 0x09, 0x07, 0x02]))]       # ドラムに戻す
    ev += note(9, 38, 110, 6.6, 0.1)
    return [track(seq(ev))], 8.0


def case_drumnrpn():
    """**ドラムの NRPN**（14-1D。1 打ごとの高さ・音量・パン・送り）。
    `drums` は SysEx の形（3n rr nn）で同じ所を触るが、**NRPN の形**は
    ここまで 1 度も送っていなかった。実際の XG の打ち込みはこちらをよく使う。

    NRPN MSB が種類、LSB が鍵の番号、CC6 が値:
      14 高さ（粗）・15 高さ（細）・16 音量・18 パン・19 リバーブ送り・1A コーラス送り"""
    ev = head()
    t = 1.0
    for i, k in enumerate((36, 38, 42)):              # 素の音（ここで写し取る）
        ev += note(9, k, 110, t + i * 0.3, 0.1)
    t = 2.1
    # 鍵 36 の高さを上げ、音量を下げ、左へ振り、リバーブを増やす
    for msb, val in ((0x14, 64 + 10), (0x16, 80), (0x18, 20), (0x19, 120)):
        ev += spread(t, nrpn_note(msb, 36, val))
        t += 0.06
    # 鍵 38 は高さだけ下げる
    ev += spread(t, nrpn_note(0x14, 38, 64 - 8))
    t += 0.3
    for i, k in enumerate((36, 38, 42, 36)):
        ev += note(9, k, 110, t + i * 0.35, 0.1)
    t += 1.6
    # 戻す
    for msb, val in ((0x14, 64), (0x16, 127), (0x18, 64), (0x19, 40)):
        ev += spread(t, nrpn_note(msb, 36, val))
        t += 0.06
    ev += note(9, 36, 110, t + 0.3, 0.1)
    return [track(seq(ev))], t + 1.8


def case_retrig():
    """**同じ鍵の連打**と**極端に短い音**。実際の曲（刻みのベース・ドラムの
    ロール・トレモロ）が当たり前にやるのに、ここまでの試験は 1 度も
    やっていなかった。

    * 同じ鍵をすぐ押し直すと、実機は**前の音を残したまま**新しいスロットで
      鳴らす（離しの尾が重なる）。native がスロットを取り合うときに
      前の音を切ってしまうと、刻みが痩せる
    * 10ms しか押さない音は、離しの段に入るのが早い
    * 離す前に押し直す（重ねる）形も入れてある"""
    ev = head()
    ev += [(1.0, bytes([0xc0, 0x21]))]                # Finger Bass（減衰が速い）
    ev += note(0, 40, 110, 1.2, 0.4)                  # 1 音目。ここで写し取る
    # 同じ鍵を 16 分で刻む（離してすぐ押す）
    t = 2.0
    for i in range(12):
        ev += note(0, 40, 110, t + i * 0.125, 0.10)
    t = 3.6
    # 離す前に押し直す（重なる）
    for i in range(6):
        ev += [(t + i * 0.2, bytes([0x90, 40, 110]))]
    ev += [(t + 1.4, bytes([0x80, 40, 0x40]))]
    # 極端に短い音（5ms）
    t = 5.4
    for i in range(8):
        ev += [(t + i * 0.15, bytes([0x90, 47, 110])),
               (t + i * 0.15 + 0.005, bytes([0x80, 47, 0x40]))]
    # ドラムのロール
    ev += [(6.8, bytes([0xc9, 0x00]))]
    for i in range(16):
        ev += note(9, 38, 100 + (i % 3) * 9, 7.0 + i * 0.06, 0.02)
    return [track(seq(ev))], 9.0


def case_pedretrig():
    """**ダンパーを踏んだまま同じ鍵を繰り返す**。6.138 で「離しは 1 回ぶん
    だけ」にしたので、ダンパー（押さえたまま待たせる）とソステヌートとの
    組み合わせがいちばん危ない所になった。

    * 踏んだまま同じ鍵を 6 回 → 離しても待たされる → 離すと全部鳴り終わる
    * ソステヌートで一部だけ待たせて、残りは普通に離す
    * 踏んだまま和音を重ねて、途中で離す"""
    ev = head()
    ev += [(1.0, bytes([0xc0, 0x00]))]                # GrandPno
    ev += note(0, 60, 100, 1.2, 0.6)                  # 1 音目。ここで写し取る
    # ダンパーを踏んで、同じ鍵を 6 回
    ev += [(2.0, DAMPER_ON)]
    for i in range(6):
        ev += note(0, 60, 100, 2.2 + i * 0.22, 0.12)
    ev += [(3.8, DAMPER_OFF)]                         # ここで 6 つとも鳴り終わる
    # ソステヌート: 先に押した音だけ待たせ、あとの音は普通に離す
    ev += [(4.4, bytes([0x90, 52, 100]))]
    ev += [(4.7, SOST_ON)]
    ev += [(4.8, bytes([0x80, 52, 0x40]))]            # 待たされる
    ev += note(0, 59, 100, 5.0, 0.4)                  # 普通に離れる
    ev += note(0, 64, 100, 5.5, 0.4)
    ev += [(6.1, SOST_OFF)]                           # 52 がここで離れる
    # ダンパーを踏んで和音を重ね、途中で離す
    ev += [(6.6, DAMPER_ON)]
    for i, k in enumerate((48, 52, 55, 60)):
        ev += note(0, k, 100, 6.8 + i * 0.15, 0.1)
    ev += [(7.6, DAMPER_OFF)]
    return [track(seq(ev))], 9.0


def case_edges():
    """**鍵と強さの端**（鍵 0・12・120・127、強さ 1・127）。
    `keylevel` は鍵 36/60/84 × 強さ 30/127 の真ん中あたりだけを見ている。
    端は曲線の表の頭と尻（音量・切る高さ・強さ・鍵の追従）を当たるので、
    はみ出しや頭打ちの扱いが違うとここで出る。

    音域の外で鳴らない音色もある（鳴らないことも実機と合っていてほしい）"""
    ev = head()
    t = 1.0
    for prog in (0x00, 0x30, 0x4d):                   # GrandPno・Strings・FX
        ev += [(t, bytes([0xc0, prog]))]
        t += 0.2
        for key in (0, 12, 120, 127):
            for vel in (1, 127):
                ev += note(0, key, vel, t, 0.35)
                t += 0.55
        t += 0.3
    return [track(seq(ev))], t + 1.5


def case_fxchange():
    """**曲の途中でエフェクトの種類を替える**（リバーブ・コーラス・
    バリエーション）と、**ドラムのパートにインサーションを掛ける**。

    種類を替えると MEG のプログラムを 1 万件以上書き直すので、firmware を
    その間じゅう回す必要がある（`m_fw_hold` の 300ms）。native の口で
    そこを削ると、替えたあとの音が前の種類のまま鳴る。
    ドラムにインサーションを掛けるのは、旋律のパートと道が違う所"""
    ev = head()
    ev += [(1.0, bytes([0xc0, 0x30]))]                # ch1 Strings
    ev += [(1.05, bytes([0xb0, 0x5b, 100]))]          # CC91 リバーブ送り
    ev += [(1.06, bytes([0xb0, 0x5d, 80]))]           # CC93 コーラス送り
    ev += note(0, 60, 100, 1.4, 1.2)                  # 1 音目。ここで写し取る
    # リバーブを Hall1 から Room1 へ
    ev += [(2.9, xg([0x02, 0x01, 0x00, 0x02, 0x00]))]
    ev += note(0, 64, 100, 3.4, 1.2)
    # コーラスを Celeste へ
    ev += [(4.9, xg([0x02, 0x01, 0x20, 0x42, 0x08]))]
    ev += note(0, 67, 100, 5.4, 1.2)
    # ドラムにインサーション（バリエーションをインサーションにしてパート 10 へ）
    ev += [(6.9, xg([0x02, 0x01, 0x40, 0x4c, 0x00])),   # DISTORTION
           (6.95, xg([0x02, 0x01, 0x5a, 0x00])),        # 接続: インサーション
           (7.0, xg([0x02, 0x01, 0x5b, 0x09]))]         # パート 10（ドラム）
    ev += [(7.1, bytes([0xb9, 0x5e, 127]))]             # ch10 バリエーション送り
    for i, k in enumerate((36, 38, 42, 36)):
        ev += note(9, k, 110, 7.4 + i * 0.3, 0.1)
    return [track(seq(ev))], 10.0


def case_dialloop():
    """**ノートオン・ノートオフだけを繰り返す曲**（Domino のループ再生と同じ形）。
    音色の指定を 1 度も送らないので、**パネルのダイヤルで音色を替える道**を
    そのまま見られる（`run_tests.py` の「ダイヤル」がこれを使う）。

    native の口は音色を自分で引くので、パネルで替えられたことに気づけないと
    「画面は変わるのに音が変わらない」になる（doc/native-engine.md の 6.146）"""
    ev = head()
    t = 1.2
    for i in range(16):
        ev += note(0, 60 + (i % 3) * 4, 100, t, 0.4)
        t += 0.5
    return [track(seq(ev))], t + 1.0


def case_panrnd():
    """**パンの Rnd**（パートのパン = 0）と、その手前の値。

    実機はパンが 0 のとき、**要素を 1 つ鳴らすたびに 8bit の乱数を進めて**
    その上位 7bit をパンの位置にする（種はワーク RAM の 0x43E94C、
    `x = (0xB3 * x + 0x11) & 0xFF`）。音色が持つパンの寄りは無視され、
    送り（リバーブ・コーラス）は位置 0 で引かれるぶんだけ目減りする。
    doc/native-engine.md の 6.147。

    要素が 2 つある音色（Warm Pad）で、1 音に 2 回進むことも見る"""
    ev = head()
    ev += [(0.9, bytes([0xc0, 89]))]                  # Warm Pad（要素 2 つ）
    ev += [(0.95, bytes([0xb0, 0x5b, 64])), (0.96, bytes([0xb0, 0x5d, 64]))]
    ev += note(0, 60, 100, 1.2, 0.6)                  # 1 音目。ここで写し取る
    t = 2.2
    # 左端 → 中央 → 右端 → Rnd
    for val in (1, 64, 127, 0):
        ev += [(t, xg([0x08, 0x00, 0x0e, val]))]
        for i in range(4):
            ev += note(0, 60 + i * 3, 100, t + 0.25 + i * 0.45, 0.35)
        t += 2.2
    return [track(seq(ev))], t + 1.0


def case_meter():
    """**液晶のメーター**（doc/native-engine.md の 6.148）。

    実機は「演奏画面を描く係」でメーターを描くが、これは firmware が自分で
    音を持っている間しか呼ばれない。native の口は note on を firmware に
    渡さないので、そのままだと棒が 1 本も動かない（利用者からの報告）。
    native が棒の字を液晶へ直に置くようにしてある。

    15 パートに別々の強さと音量で 1 音ずつ。1 音目で写し取り、2 音目を
    native が鳴らす。`run_tests.py` の「メーター」が液晶を突き合わせる"""
    ev = head()
    chs = [c for c in range(16) if c != 9]
    combos = [(32, 32), (64, 64), (96, 100), (110, 127), (127, 100),
              (48, 80), (80, 48), (100, 110), (120, 64), (24, 127),
              (64, 32), (96, 127), (40, 100), (72, 96), (112, 112)]
    for i, ch in enumerate(chs):
        vel, vol = combos[i]
        ev += [(1.0, bytes([0xc0 | ch, 0x00])), (1.1, bytes([0xb0 | ch, 0x07, vol]))]
        ev += note(ch, 60, vel, 1.3, 0.5)      # 1 音目（写し取り）
        ev += note(ch, 62, vel, 2.6, 1.6)      # 2 音目（native が鳴らす）
    return [track(seq(ev))], 4.8


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
    "sxparam": case_sxparam,
    "pedals":  case_pedals,
    "partsx":  case_partsx,
    "keylevel": case_keylevel,
    "rpn":     case_rpn,
    "mono":    case_mono,
    "ctlreset": case_ctlreset,
    "ports":   case_ports,
    "scale":   case_scale,
    "kits":    case_kits,
    "ins2":    case_ins2,
    "progchg": case_progchg,
    "running": case_running,
    "pat":     case_pat,
    "ccramp":  case_ccramp,
    "midreset": case_midreset,
    "partmode": case_partmode,
    "drumnrpn": case_drumnrpn,
    "retrig":  case_retrig,
    "pedretrig": case_pedretrig,
    "edges":   case_edges,
    "fxchange": case_fxchange,
    "dialloop": case_dialloop,
    "panrnd": case_panrnd,
    "meter": case_meter,
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
