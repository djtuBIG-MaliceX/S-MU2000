# clickload.mid / clickload2.mid を鳴らした録音を読んで、区画ごとの遅れを出す。
#
# 基準音（ch1・真ん中・小さい）とクリック（ch16・右いっぱい・大きい）を左右で分けて拾い、
# その差を出す。差 = その区画の荷物ぶんの遅れ（MIDI 上の 2 tick は引いてある）。
# 最後にバイト数と本数それぞれに直線を当てて、口の速さを出す。
#
# 実機の録音でもエミュ（build/render.exe）の書き出しでも同じように読める。
#
# 使い方:
#   python tools/latency/clickread.py 録音.wav [--second] [区画1のクリックの秒数]
#     --second … clickload2.mid のほう
import array
import struct
import sys

SEC = 60.0 / 110 * 4                 # 1 区画 ＝ 110bpm の 4 拍
TICK = 60.0 / 110 / 384

# (名前, 荷物のバイト数, 本数)
LOAD1 = [('なし', 0, 0), ('1ch', 74, 25), ('2ch', 148, 50), ('4ch', 296, 100),
         ('7ch', 518, 175), ('14ch', 1036, 350), ('14ch x2', 2072, 700),
         ('progだけ', 28, 14), ('CCだけ', 1008, 336), ('なし(確認)', 0, 0)]
LOAD2 = [('なし', 0, 0), ('CC336（14ch）', 1008, 336), ('CC336（ch2）', 1008, 336),
         ('SysEx1008×1', 1008, 1), ('SysEx84×12', 1008, 12),
         ('CC112（14ch）', 336, 112), ('SysEx336×1', 336, 1), ('なし(確認)', 0, 0)]


def load(path):
    d = open(path, 'rb').read()
    i, fmt, data = 12, None, None
    while i + 8 <= len(d):
        cid = d[i:i + 4]
        sz = struct.unpack('<I', d[i + 4:i + 8])[0]
        body = d[i + 8:i + 8 + sz]
        if cid == b'fmt ':
            fmt = struct.unpack('<HHIIHH', body[:16])
        elif cid == b'data':
            data = body
        i += 8 + sz + (sz & 1)
    af, ch, rate, br, ba, bits = fmt
    if bits != 16:
        raise SystemExit('16 ビットの WAV にしてください（いまは %d ビット）' % bits)
    n = len(data) // ba
    a = array.array('h')
    a.frombytes(data[:n * ba])
    left = [a[k * ch] / 32768.0 for k in range(n)]
    right = [a[k * ch + (1 if ch > 1 else 0)] / 32768.0 for k in range(n)]
    return left, right, rate


def first_over(xs, a, b, th):
    for k in range(max(a, 0), min(b, len(xs))):
        if abs(xs[k]) > th:
            return k
    return None


def fit(xs, ys):
    n = len(xs)
    mx, my = sum(xs) / n, sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs)
    if sxx == 0:
        return None
    m = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sxx
    b = my - m * mx
    return m, b, max(abs(y - (m * x + b)) for x, y in zip(xs, ys))


def main(argv):
    second = '--second' in argv
    rest = [a for a in argv if a != '--second']
    path = rest[0]
    table = LOAD2 if second else LOAD1
    left, right, rate = load(path)
    peak = max(abs(v) for v in right)
    if len(rest) > 1:
        t0 = float(rest[1])
    else:
        t0 = first_over(right, 0, len(right), peak * 0.25) / rate
    # 基準音の大きさは、いちばん離れる区画（1 のほうは 14ch x2、2 のほうは CC336）で測る
    far = t0 + SEC * (1 if second else 6)
    k = first_over(left, int((far - 0.25) * rate), int((far + 0.15) * rate), peak * 0.004)
    refpk = max(abs(x) for x in left[k:k + int(0.02 * rate)]) if k else peak * 0.012

    print('%s   クリック %.4f（右）／基準音 %.4f（左）、区画 1 のクリック %.4f 秒'
          % (path, peak, refpk, t0))
    print('区画            バイト   本数     基準音        クリック        差')
    pts = []
    for i, (name, nb, nm) in enumerate(table):
        c = t0 + SEC * i
        a, b = int((c - 0.25) * rate), int((c + 1.20) * rate)
        ref = first_over(left, a, b, refpk * 0.35)
        clk = first_over(right, a, b, peak * 0.25)
        if ref is None or clk is None:
            print('%-14s %6d %6d   --- 見つからない ---' % (name, nb, nm))
            continue
        gap = ((clk - ref) / rate - 2 * TICK) * 1000.0
        pts.append((nb, nm, gap, name))
        print('%-14s %6d %6d   %9.4f s  %9.4f s  %7.1f ms'
              % (name, nb, nm, ref / rate, clk / rate, gap))

    use = [p for p in pts if p[3] != 'progだけ']
    if len(use) >= 3:
        a = fit([p[0] for p in use], [p[2] for p in use])
        b = fit([p[1] for p in use], [p[2] for p in use])
        print('\nバイト数に直線: %.4f ms/バイト + %.1f ms → **%.0f バイト/秒**（外れ最大 %.1f ms）'
              % (a[0], a[1], 1000.0 / a[0], a[2]))
        if b and b[0] > 0:
            print('本数に直線    : %.4f ms/本 + %.1f ms → **%.0f 本/秒**（外れ最大 %.1f ms）'
                  % (b[0], b[1], 1000.0 / b[0], b[2]))
        for p in pts:
            if p[3] == 'progだけ' and b:
                want = b[0] * p[1] + b[1]
                print('progだけ %d 件: 実測 %.1f ms、CC と同じ重さなら %.1f ms → 1 件 +%.2f ms'
                      % (p[1], p[2], want, (p[2] - want) / p[1]))


main(sys.argv[1:])
