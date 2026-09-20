#!/usr/bin/env python3
# license:BSD-3-Clause
"""**液晶の字を手描きで差し替える道具**（`art/lcdfont.txt`）。

手元にある字形 ROM（MAME の `mulcd.zip` の `hd44780u_b04.bin`）は MU2000 自身の
ものではないらしく、記号のところが実機と違う。実機の画面を見ながら描き起こした
ものを `art/lcdfont.txt` に書けば、エミュレータがそちらを上から被せる。

  python tools/lcdfont.py show [コード…]     手描きの表を絵にして見る
  python tools/lcdfont.py show --rom 41 42   いまの字形 ROM と並べて見る
  python tools/lcdfont.py diff               ROM と違う字だけ並べる
  python tools/lcdfont.py sheet 41-5a        空の枠を出す（文字も添える）
  python tools/lcdfont.py dump 10-1f         いまの字形 ROM を下書きに出す
  python tools/lcdfont.py where 10 11        その字が画面のどこに出ているか探す

**文字（`20`-`7f`）も同じように直せる。** 実機の写真を見ながら
`sheet 41-5a` で枠を出して埋めていくのがいちばん早い。

**`dump` の出力を `art/lcdfont.txt` に貼らないこと。** あそこに入れてよいのは
実機を見て人が描いたものだけ（ROM から起こした字は配れない）。下書きとして
`.local/` に置いて、形を直しながら手で写す。
"""
import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ART = ROOT / "art" / "lcdfont.txt"
BUILD = ROOT / "build"
EXE = ".exe" if os.name == "nt" else ""
NEEDED = ("mu2000.zip", "swp30.zip")


def find_roms():
    if os.environ.get("SMU2000_ROMS"):
        c = Path(os.environ["SMU2000_ROMS"])
        if (c / NEEDED[0]).exists():
            return c
    for c in (ROOT / "roms", ROOT.parent / "MU2000" / "roms"):
        if all((c / n).exists() for n in NEEDED):
            return c
    return None


def font_bytes():
    """字形 ROM（4096 バイト）を探して返す"""
    roms = find_roms()
    for c in ([roms / "hd44780u_b04.bin", roms / "standin" / "hd44780u_b04.bin"]
              if roms else []):
        if c.exists():
            return c.read_bytes()
    return None


def parse_art(path=ART):
    """art/lcdfont.txt を {コード: [8 行]} にする"""
    out, code, rows = {}, None, []
    if not Path(path).exists():
        return out
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        t = line.strip()
        if not t:
            if code is not None:
                out[code] = (rows + [0] * 8)[:8]
                code, rows = None, []
            continue
        # **点の行かどうかを、覚え書きより先に見る**（src/lcdfont.h と同じ）。
        # いちばん左が点いている行は `#` で始まるので、覚え書きと間違えていた
        if len(t) <= 5 and set(t) <= set(".#"):
            if code is not None and len(rows) < 8:
                rows.append(sum(1 << (4 - x) for x, c in enumerate(t[:5]) if c == "#"))
                continue
        if t.startswith("#"):
            continue
        m = re.match(r"([0-9a-fA-F]{1,2})\b", t)
        if m:
            if code is not None:
                out[code] = (rows + [0] * 8)[:8]
            code, rows = int(m.group(1), 16), []
    if code is not None:
        out[code] = (rows + [0] * 8)[:8]
    return out


def draw(rows):
    return ["".join("#" if (v >> (4 - x)) & 1 else "." for x in range(5)) for v in rows]


def label(c):
    """コードに文字を添える（20-7e は見えるので）"""
    return "%02x %s" % (c, chr(c) if 0x21 <= c <= 0x7e else " ")


def side_by_side(items, per_line=6):
    """[(見出し, 8 行)] を横に並べて出す"""
    for i in range(0, len(items), per_line):
        chunk = items[i:i + per_line]
        print("   ".join("%-5s" % t for t, _ in chunk))
        for y in range(8):
            print("   ".join("%-5s" % draw(r)[y] for _, r in chunk))
        print()


def codes_of(args, default):
    if not args:
        return default
    out = []
    for a in args:
        if "-" in a:
            lo, hi = a.split("-")
            out += list(range(int(lo, 16), int(hi, 16) + 1))
        else:
            out.append(int(a, 16))
    return out


def rom_rows(f, c):
    return list(f[c * 16: c * 16 + 8])


def cmd_show(a):
    art = parse_art()
    cs = codes_of(a.codes, sorted(art))
    if a.rom:
        # **字形 ROM と手描きを並べる**。ROM の側は見るだけ（貼らないこと）
        f = font_bytes()
        if not f:
            print("字形 ROM が見つからない（SMU2000_ROMS で指す）")
            return 1
        items = []
        for c in cs:
            items.append(("ROM " + label(c), rom_rows(f, c)))
            items.append(("手 " + label(c), art.get(c, [0] * 8)))
        side_by_side(items, per_line=6)
        return 0
    miss = [c for c in cs if c not in art]
    side_by_side([(label(c), art[c]) for c in cs if c in art])
    if miss:
        print("手描きがまだ無いコード: %s" % " ".join("%02x" % c for c in miss))


def cmd_diff(a):
    """手描きと字形 ROM が違う字だけ並べる（直した所の一覧）"""
    art = parse_art()
    f = font_bytes()
    if not f:
        print("字形 ROM が見つからない（SMU2000_ROMS で指す）")
        return 1
    cs = codes_of(a.codes, sorted(art))
    items = []
    for c in cs:
        if art.get(c) != rom_rows(f, c):
            items.append(("ROM " + label(c), rom_rows(f, c)))
            items.append(("手 " + label(c), art.get(c, [0] * 8)))
    if not items:
        print("字形 ROM と違う字は無い")
        return 0
    print("字形 ROM と違う字: %d 個" % (len(items) // 2))
    side_by_side(items, per_line=6)
    return 0


def cmd_dump(a):
    f = font_bytes()
    if not f:
        print("字形 ROM が見つからない（SMU2000_ROMS で指す）")
        return 1
    cs = codes_of(a.codes, list(range(0x10, 0x20)))
    print("# いまの字形 ROM の中身（**下書き**。art/lcdfont.txt に貼らないこと）")
    for c in cs:
        rows = list(f[c * 16: c * 16 + 8])
        print()
        print("%02x" % c)
        for line in draw(rows):
            print(line)
    return 0


def cmd_sheet(a):
    art = parse_art()
    cs = codes_of(a.codes, list(range(0x10, 0x20)))
    print("# 空の枠。実機の画面を見ながら `.` を `#` に置き換える")
    print("# （そのまま art/lcdfont.txt に足してよい）")
    for c in cs:
        if c in art and not a.force:
            continue
        print()
        print("%-3s %s" % ("%02x" % c,
                           ("文字 '%s'" % chr(c)) if 0x21 <= c <= 0x7e else ""))
        for _ in range(8):
            print(".....")
    return 0


def cmd_where(a):
    """その字が画面のどこに出ているか、パネルを鳴らして探す"""
    roms = find_roms()
    if not roms:
        print("ROM が見つからない")
        return 1
    cs = codes_of(a.codes, [])
    if not cs:
        print("探したいコードを渡す（例: where 10 11）")
        return 1
    keys = a.keys or ""
    cmd = [str(BUILD / ("panel" + EXE)), str(roms), "--lcd-hex"]
    if keys:
        cmd += ["--keys", keys]
    out = subprocess.run(cmd, capture_output=True, text=True,
                         encoding="utf-8", errors="replace").stdout
    hexes = re.findall(r"^\s*\|(.*)\|\s*$", out, re.M)
    # --lcd-hex は表示できない字を ? にするので、DDRAM を直に読む道が無い。
    # ここでは panel の行をそのまま出して、目で見つけてもらう
    for line in out.splitlines():
        if line.strip().startswith("|"):
            print(line)
    print()
    print("（コード %s がどのマスかは `SMU2000_RAMSNAP` の lcd000.bin で見る）"
          % " ".join("%02x" % c for c in cs))
    return 0


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd")
    p = sub.add_parser("show"); p.add_argument("codes", nargs="*")
    p.add_argument("--rom", action="store_true", help="字形 ROM と並べて見る")
    p.set_defaults(fn=cmd_show)
    p = sub.add_parser("diff"); p.add_argument("codes", nargs="*")
    p.set_defaults(fn=cmd_diff)
    p = sub.add_parser("dump"); p.add_argument("codes", nargs="*"); p.set_defaults(fn=cmd_dump)
    p = sub.add_parser("sheet"); p.add_argument("codes", nargs="*")
    p.add_argument("--force", action="store_true"); p.set_defaults(fn=cmd_sheet)
    p = sub.add_parser("where"); p.add_argument("codes", nargs="*")
    p.add_argument("--keys"); p.set_defaults(fn=cmd_where)
    a = ap.parse_args()
    if not a.cmd:
        print(__doc__)
        return 2
    return a.fn(a) or 0


if __name__ == "__main__":
    sys.exit(main())
