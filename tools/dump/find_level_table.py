import sys

import numpy as np

ROM = np.frombuffer(open('C:/Users/gugug/GitHub/MU2000/roms/mu2000_flash.bin', 'rb').read(),
                    dtype=np.uint8).astype(np.int32)

meas = {}
for line in open(sys.argv[1]):
    f = line.split()
    if len(f) > 0x0a and f[0].isdigit():
        meas[int(f[0])] = int(f[0x0a], 16) & 0xff
ks = sorted(meas)
ref = max(ks)
target = np.array([meas[cc] - meas[ref] for cc in ks], dtype=np.int32)
idx = np.array(ks, dtype=np.int64)

N = len(ROM)
for mul in (1, 2, 4):
    for off in (-1, 0):
        # base + idx + off がすべて範囲に入る base の範囲
        lo = max(0, -(idx.min() + off))
        hi = N - (idx.max() + off) - 1
        bases = np.arange(lo, hi, dtype=np.int64)
        # 1 回に 200000 base ずつ
        step = 200000
        for s in range(0, len(bases), step):
            b = bases[s:s + step]
            cols = ROM[(b[:, None] + idx[None, :] + off)]
            r0 = ROM[b + ref + off][:, None]
            ok = np.all(mul * (cols - r0) == target[None, :], axis=1)
            for j in np.nonzero(ok)[0]:
                print('見つかった: 係数 %d 添字 cc%+d 番地 0x%06X' % (mul, off, b[j]))
print('探索おわり')
