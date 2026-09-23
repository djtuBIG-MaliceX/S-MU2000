# issue #15 の「曲頭の遅れ」を、チャンネル数を変えて測る。
#
# DAW（FL Studio）は曲の頭で、最初の音と同じ tick に音量・パン・ベンドなどの
# 初期 CC をまとめて送る。その塊が MIDI の口を通り終わるまで、音は鳴らない。
# 何チャンネルぶん送ると何ミリ秒遅れるかを、実際に鳴らして測る。
#
# 使い方:
#   SMU2000_ROMS=../MU2000/roms SMU2000_WORK=build/tests python tools/latency/ccburst.py
#   …… --din を付けると DIN の口（31250bps）で測る
#
# 結果は doc/vst3.md の「残りの遅れは、MIDI の口の速さそのもの」。
import os
import struct
import subprocess
import sys
import wave

# 作った MIDI と WAV の置き場（既定は今いる所）
SP = os.environ.get('SMU2000_WORK', os.path.dirname(os.path.abspath(__file__)))
ROOT = os.environ.get('SMU2000_ROOT', os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
ROMS = os.environ.get('SMU2000_ROMS', '../MU2000/roms')
BOOT = 8.0


def vlq(n):
    out = [n & 0x7f]
    n >>= 7
    while n:
        out.insert(0, 0x80 | (n & 0x7f))
        n >>= 7
    return bytes(out)


def build(path, channels):
    """最初の tick に channels ぶんの初期 CC、同じ tick に ch1 の音"""
    ev = []
    sx = bytes([0x43, 0x10, 0x4c, 0x00, 0x00, 0x7e, 0x00, 0xf7])
    ev.append((0, b'\xf0' + vlq(len(sx)) + sx))                   # XG システムオン
    nbytes = 10
    for c in range(channels):
        ch = c & 0x0f
        port = c // 16
        if port and c % 16 == 0:
            ev.append((480, bytes([0xf5, 1 + port])))             # 口を切り替える
            nbytes += 2
        for cc, val in ((0x00, 0), (0x20, 0), (0x07, 100), (0x0a, 64),
                        (0x0b, 127), (0x5b, 40), (0x5d, 0), (0x41, 0)):
            ev.append((480, bytes([0xb0 | ch, cc, val])))
            nbytes += 3
        ev.append((480, bytes([0xc0 | ch, 0])))                   # プログラム
        ev.append((480, bytes([0xe0 | ch, 0x00, 0x40])))          # ベンド中央
        nbytes += 5
    if channels > 16:
        ev.append((480, bytes([0xf5, 1])))                        # 口 A に戻す
        nbytes += 2
    ev.append((480, bytes([0x90, 0x3c, 0x64])))                   # ここを測る
    nbytes += 3
    ev.append((960, bytes([0x80, 0x3c, 0x40])))
    ev.sort(key=lambda e: e[0])
    trk = b''
    last = 0
    for t, b in ev:
        trk += vlq(t - last) + b
        last = t
    trk += vlq(0) + bytes([0xff, 0x2f, 0])
    data = b'MThd' + struct.pack('>IHHH', 6, 0, 1, 480) + b'MTrk' + struct.pack('>I', len(trk)) + trk
    open(path, 'wb').write(data)
    return nbytes


def onset(path, thresh=200):
    w = wave.open(path)
    n, rate = w.getnframes(), w.getframerate()
    d = w.readframes(n)
    s = struct.unpack('<%dh' % (len(d) // 2), d)
    start = int(BOOT * rate)
    for i in range(start, n):
        if abs(s[i * 2]) > thresh or abs(s[i * 2 + 1]) > thresh:
            return (i - start) / rate
    return None


def main():
    usb = '--din' not in sys.argv
    rate = 10000.0 if usb else 3125.0        # USB は実測 10,000 byte/s（6.218）、DIN は 31250 baud
    print('口: %s' % ('USB' if usb else 'DIN'))
    print('%6s %7s %10s %10s %10s' % ('ch', 'バイト', '遅れ(ms)', '理論(ms)', '差'))
    for ch in (1, 3, 8, 16):
        mid = os.path.join(SP, 'cc%d.mid' % ch)
        nb = build(mid, ch)
        wav = os.path.join(SP, 'cc%d.wav' % ch)
        cmd = [os.path.join(ROOT, 'build', 'render' + ('.exe' if os.name == 'nt' else '')), ROMS, mid, wav, '2',
               '--boot', str(BOOT)] + (['--usb'] if usb else [])
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
        t = onset(wav)
        # 曲では音は 0.5 秒の所（480 tick、120bpm、division 480）
        delay = (t - 0.5) * 1000.0 if t is not None else float('nan')
        theory = nb / rate * 1000.0
        print('%6d %7d %10.2f %10.2f %10.2f' % (ch, nb, delay, theory, delay - theory))


main()
