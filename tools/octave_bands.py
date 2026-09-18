import math
import sys
import wave

import numpy as np


def load(p):
    w = wave.open(p)
    n, ch = w.getnframes(), w.getnchannels()
    a = np.frombuffer(w.readframes(n), dtype='<i2').astype(float)
    w.close()
    return a[0::ch] if ch > 1 else a


fw = load(sys.argv[1])
ne = load(sys.argv[2])
t0, t1 = float(sys.argv[3]), float(sys.argv[4])
R = 44100
n = min(len(fw), len(ne))
i0, i1 = int(t0 * R), min(n, int(t1 * R))
a, b = fw[i0:i1], ne[i0:i1]
m = len(a)
win = np.hanning(m)
A = np.abs(np.fft.rfft(a * win))
B = np.abs(np.fft.rfft(b * win))
f = np.fft.rfftfreq(m, 1 / R)
print('%.0f-%.0f 秒のオクターブ帯（native − firmware）' % (t0, t1))
lo = 31.25
while lo < 16000:
    sel = (f >= lo) & (f < lo * 2)
    ea = np.sqrt((A[sel] ** 2).sum())
    eb = np.sqrt((B[sel] ** 2).sum())
    print('  %6.0f-%6.0f Hz  %+6.2f dB' % (lo, lo * 2,
                                           20 * math.log10(max(eb, 1e-9) / max(ea, 1e-9))))
    lo *= 2
