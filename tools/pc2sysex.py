#!/usr/bin/env python3
# license:BSD-3-Clause
"""口 B 以降のプログラムチェンジを XG のパラメータチェンジ（SysEx）に置き換える。

SONAR の VST3 は、口 B-D のトラックから送ったプログラムチェンジを口 A のものとして
渡してくる（doc/vst3.md の「SONAR では口 B-D のプログラムチェンジが口 A に届く」）。
SysEx は口ごとに正しく届くので、音色の指定を SysEx にしておけば取り違えない。

  python tools/pc2sysex.py 曲.mid [出力.mid] [--all]

  出力を省くと「曲_pc2sx.mid」に書く。
  --all  口 A のプログラムチェンジも置き換える（ふつうは要らない）

置き換え方:
* 口はトラックの「口の指定」（メタイベント FF 21。Domino などが書く）で見分ける。
  指定の無いトラックは口 A
* プログラムチェンジ 1 つを、そのチャンネルを受けているパートごとに
  `F0 43 10 4C 08 pp 01 msb F7`・`… 02 lsb F7`・`… 03 prog F7` の 3 つにする
  （pp はパート番号 0x00-0x3F）。バンクは、それまでにそのチャンネルに来ていた
  CC0・CC32 の値。まだ来ていなければバンクは書かない（パートの今のバンクのまま）
* 置き換えた SysEx はプログラムチェンジと同じ時刻・同じトラックに置く。CC0・CC32 は残す
* **受けているパート**は既定では「口 × 16 + チャンネル」。曲が XG の受信チャンネル
  （08 pp 04）を付け替えていれば、それを時刻順に追う。XG System On・GM On・
  GS リセットで既定に戻す
* ほかのイベントは 1 バイトも変えない
"""
import struct
import sys


def read_vlq(d, p):
    v = 0
    while True:
        c = d[p]
        p += 1
        v = (v << 7) | (c & 0x7f)
        if not c & 0x80:
            return v, p


def vlq(n):
    out = [n & 0x7f]
    n >>= 7
    while n:
        out.append(0x80 | (n & 0x7f))
        n >>= 7
    return bytes(reversed(out))


def parse_track(data):
    """トラックを (絶対 tick, 生バイト列, 種類, 中身) の並びにする。生バイト列は
    ランニングステータスを使わない形に直したもの（書き戻しでそのまま使える）"""
    evs = []
    p, tick, run = 0, 0, 0
    while p < len(data):
        dt, p = read_vlq(data, p)
        tick += dt
        st = data[p]
        if st & 0x80:
            p += 1
        else:
            st = run
        if st == 0xff:
            ty = data[p]
            n, q = read_vlq(data, p + 1)
            body = data[q:q + n]
            evs.append((tick, bytes([0xff, ty]) + vlq(n) + body, 'meta', (ty, body)))
            p = q + n
            if ty == 0x2f:
                break
        elif st in (0xf0, 0xf7):
            n, q = read_vlq(data, p)
            body = data[q:q + n]
            evs.append((tick, bytes([st]) + vlq(n) + body, 'sysex', (st, body)))
            p = q + n
        else:
            run = st
            n = 1 if (st & 0xf0) in (0xc0, 0xd0) else 2
            body = data[p:p + n]
            p += n
            evs.append((tick, bytes([st]) + body, 'chan', (st, body)))
    return evs


def write_track(evs):
    out, last = b'', 0
    for tick, raw, _, _ in evs:
        out += vlq(tick - last) + raw
        last = tick
    return b'MTrk' + struct.pack('>I', len(out)) + out


def xg(pp, addr, value):
    body = bytes([0x43, 0x10, 0x4c, 0x08, pp, addr, value & 0x7f, 0xf7])
    return bytes([0xf0]) + vlq(len(body)) + body


def is_reset(st, body):
    b = bytes(body)
    return st == 0xf0 and (b.startswith(bytes([0x7e, 0x7f, 0x09, 0x01])) or
                           b.startswith(bytes([0x7e, 0x7f, 0x09, 0x03])) or
                           (len(b) >= 7 and b[0] == 0x43 and (b[1] & 0xf0) == 0x10 and
                            b[2:7] in (bytes([0x4c, 0, 0, 0x7e, 0]), bytes([0x4c, 0, 0, 0x7f, 0]))) or
                           b.startswith(bytes([0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7f])))


def convert(src, dst, all_ports=False):
    d = open(src, 'rb').read()
    if d[:4] != b'MThd':
        raise SystemExit('MIDI ファイルではない: %s' % src)
    hlen = struct.unpack('>I', d[4:8])[0]
    fmt, ntrk, div = struct.unpack('>HHH', d[8:14])
    pos = 8 + hlen
    tracks = []
    for _ in range(ntrk):
        if d[pos:pos + 4] != b'MTrk':
            raise SystemExit('トラックが壊れている')
        ln = struct.unpack('>I', d[pos + 4:pos + 8])[0]
        tracks.append(parse_track(d[pos + 8:pos + 8 + ln]))
        pos += 8 + ln

    # 全トラックを時刻順に 1 本に並べて、受信チャンネルとバンクを追う
    # （同じ時刻ならトラックの順、トラックの中は元の順）
    flat = []
    for ti, evs in enumerate(tracks):
        port = 0
        for ei, (tick, raw, kind, val) in enumerate(evs):
            if kind == 'meta' and val[0] == 0x21 and len(val[1]) >= 1:
                port = val[1][0] & 3
            flat.append((tick, ti, ei, port))
    flat.sort()

    rcv = {pp: pp for pp in range(64)}        # パート → 受けている 口×16+ch（0x7f は受けない）
    bank = {}                                  # 口×16+ch → [msb, lsb]（来ていなければ None）
    add = {}                                   # (トラック, イベント番号) → 置き換える生バイト列の並び
    count = 0
    for tick, ti, ei, port in flat:
        _, raw, kind, val = tracks[ti][ei]
        if kind == 'sysex':
            st, body = val
            if is_reset(st, body):
                rcv = {pp: pp for pp in range(64)}
                bank = {}
            b = bytes(body)
            # XG の受信チャンネル 08 pp 04
            if len(b) >= 7 and b[0] == 0x43 and (b[1] & 0xf0) == 0x10 and b[2] == 0x4c and b[3] == 0x08:
                pp, addr = b[4], b[5]
                vals = b[6:-1] if b.endswith(b'\xf7') else b[6:]
                for k, v in enumerate(vals):
                    if addr + k == 0x04 and pp < 64:
                        rcv[pp] = v
            continue
        if kind != 'chan':
            continue
        st, body = val
        slot = port * 16 + (st & 0x0f)
        if (st & 0xf0) == 0xb0 and body[0] in (0, 32):
            bank.setdefault(slot, [None, None])[0 if body[0] == 0 else 1] = body[1]
        elif (st & 0xf0) == 0xc0 and (all_ports or port > 0):
            parts = [pp for pp in range(64) if rcv[pp] == slot]
            out = []
            msb, lsb = bank.get(slot, [None, None])
            for pp in parts:
                if msb is not None:
                    out.append(xg(pp, 0x01, msb))
                if lsb is not None:
                    out.append(xg(pp, 0x02, lsb))
                out.append(xg(pp, 0x03, body[0]))
            add[(ti, ei)] = out
            count += 1

    out = d[:8 + hlen]
    for ti, evs in enumerate(tracks):
        new = []
        for ei, e in enumerate(evs):
            if (ti, ei) in add:
                for raw in add[(ti, ei)]:
                    new.append((e[0], raw, 'sysex', None))
            else:
                new.append(e)
        out += write_track(new)
    open(dst, 'wb').write(out)
    return count


def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    if not args:
        print(__doc__)
        return 1
    src = args[0]
    dst = args[1] if len(args) > 1 else (src[:-4] if src.lower().endswith('.mid') else src) + '_pc2sx.mid'
    n = convert(src, dst, '--all' in sys.argv)
    print('%s: プログラムチェンジ %d 個を XG の SysEx に置き換えた' % (dst, n))
    return 0


if __name__ == '__main__':
    sys.exit(main())
