#!/usr/bin/env python3
# license:BSD-3-Clause
"""**試験がまだ触れていない軸**を洗い出す（doc/native-engine.md の 6.149）。

  python tools/native/axes.py


  * 試験 MIDI が送っている CC 番号 / XG のパート番地
  * native の口が自分でさばく CC / 見ているパート番地
を突き合わせる。
"""
import io
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
T = io.open(ROOT / 'tools' / 'make_test_midi.py', encoding='utf-8').read()
D = io.open(ROOT / 'src' / 'xg' / 'native_driver.h', encoding='utf-8').read()

cc = set()
# bytes([0xb0 | ch, NN, vv]) の形
for m in re.finditer(r'0xb0\s*\|\s*\w+\s*,\s*(0x[0-9a-fA-F]+|\d+)', T):
    cc.add(int(m.group(1), 0))
for m in re.finditer(r'0xb0\s*,\s*(0x[0-9a-fA-F]+|\d+)', T):
    cc.add(int(m.group(1), 0))
# b'\xb0\xNN' の形
for m in re.finditer(r"b'" + re.escape(chr(92)) + r"xb0" + re.escape(chr(92)) + r'x([0-9a-fA-F]{2})', T):
    cc.add(int(m.group(1), 16))
print('試験が送っている CC: ' + ' '.join('%02X' % c for c in sorted(cc)))

# driver が case で拾っている CC
hand = set()
i = D.find('bool handles_cc')
if i < 0:
    i = D.find('handles_cc')
seg = D[i:i + 900]
for m in re.finditer(r'0x([0-9a-fA-F]{2})', seg):
    hand.add(int(m.group(1), 16))
print()
print('handles_cc のあたりに出てくる番号: ' + ' '.join('%02X' % c for c in sorted(hand)))
print()
print('driver がさばくのに試験が送っていない: '
      + ' '.join('%02X' % c for c in sorted(hand - cc)))

# XG のパート番地（xg([0x08, 0, LO, ...])）
lo = set()
for m in re.finditer(r'xg\(\[0x08,\s*[^,]+,\s*(0x[0-9a-fA-F]+|\d+)', T):
    lo.add(int(m.group(1), 0))
print()
print('試験が触るパート番地 08 pp: ' + ' '.join('%02X' % c for c in sorted(lo)))
