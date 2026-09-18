#!/usr/bin/env python3
# license:BSD-3-Clause
"""パネルの SVG を、人が絵の道具（Inkscape など）で直しやすいように部品に分ける。

MAME から取り出したパネルの絵（art/mame/mu2000-mame.svg）は、線と塗りがそれぞれ 1 本の大きな
パスにまとまっている（部分パスが 200 ほど）。これでは「ダイヤルの線だけ直す」ができない。

分け方:
  * 1 本のパスを部分パス（m ... z）に切る。相対座標の m は、前の部分パスの終わりに対する位置なので、
    切るときに絶対座標（M）に直す
  * 同じパスの部分パスのうち、形が重なる・触れ合うものは 1 本にまとめたままにする。塗りは
    部分パスを交互に塗る決まり（穴の開いた形）なので、離すと穴が埋まる
  * 塊ごとに、パネルのどの部品か（液晶の枠・ダイヤル・ジャック…）を位置と形から決め、
    名前の付いたグループ（Inkscape のレイヤー）に入れる
  * パスの変換（<g> の変換を含む）は各パスの transform に焼き込む。gui.exe の読み手
    （src/ui/svg.cpp）は <g> の変換を 1 段しか読まないので、グループには変換を付けない

描いた結果は元の絵と同じになる（gui.exe --shot で書いた絵を比べて確かめる）。

  python tools/svgsplit.py art/mame/mu2000-mame.svg art/mame/parts
    → parts/mu2000-parts.svg   部品ごとのレイヤーに分けた 1 枚（元の絵と置き換えられる）
      parts/<部品>.svg         部品 1 つずつ（どれも元と同じ大きさ。重ねると元の絵になる）
      parts/README.md          部品の一覧
"""

import math
import os
import re
import sys
import xml.etree.ElementTree as ET

SVG_NS = 'http://www.w3.org/2000/svg'

# 部品の名前と、その部品が収まる範囲（パネルの論理座標 1000 × 385。art/mame/panel.txt と同じ）。
# 塊の真ん中が入る最初のものの名前にする。形の条件（fn）があれば、それも満たすときだけ。
# ボタンと表示灯は gui.exe が描くので、この絵には入っていない
def _thin_vertical(w, h, filled):
    return w <= 8 and 5 <= h <= 18        # ダイヤルに隠れるところは短い


def _lamp(w, h, filled):
    return filled and w > 15


def _tick(w, h, filled):
    return filled and w < 9 and h < 11


PARTS = [
    # key                title                                         x0   y0    x1   y1   fn
    ('body',            '本体の外形',                                    0,   0, 1000, 385, lambda w, h, f: w > 600 or h > 300),
    ('plg-lamps',       'MU / PLG の表示灯の台',                        470, 325,  660, 350, _lamp),
    ('grille',          '下の縁のすき間の線',                            40, 325,  960, 350, _thin_vertical),
    ('card',            'SmartMedia の差し込み口',                       90, 305,  280, 350, None),
    ('lcd-marks',       '液晶の下の目印（PART〜KEY）と右の目印（XG / GS / PERFORM）', 330, 165, 660, 185, _tick),
    ('lcd-marks',       None,                                          636, 130,  650, 165, _tick),
    ('lcd',             '液晶の窓の枠',                                  260,  45,  660, 190, None),
    ('ad-level',        'A/D INPUT の音量つまみ',                        160,  55,  230, 120, None),
    ('volume',          'VOLUME のつまみ',                               160, 120,  230, 200, None),
    ('ad-input',        'A/D INPUT の 2 つのジャックと括り線',             75,  55,  170, 200, None),
    ('power',           'STANDBY / ON のスイッチ',                        50, 215,  130, 310, None),
    ('midi-in',         'MIDI IN A の端子',                              135, 205,  220, 295, None),
    ('phones',          'PHONES のジャック',                             225, 225,  280, 295, None),
    ('select-guide',    'SELECT の横の三角と、下の折れ線',               620, 200,  700, 345, None),
    ('dial',            'ジョグダイヤル',                                780, 180,  950, 335, None),
    ('nav-marks',       'ALL の括り線と、PART / SELECT / VALUE の − ＋ の印', 850, 40, 930, 170, None),
]

TOKEN = re.compile(r'[MmLlHhVvCcSsQqTtAaZz]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?')
ARGS = {'m': 2, 'l': 2, 'h': 1, 'v': 1, 'c': 6, 's': 4, 'q': 4, 't': 2, 'a': 7, 'z': 0}


def tag(e):
    return e.tag.split('}')[-1]


# ---- 変換（a b c d e f の 6 つ）

def mat_mul(m, n):
    """m を先、n をあとに掛けた変換"""
    a, b, c, d, e, f = m
    A, B, C, D, E, F = n
    return (A * a + C * b, B * a + D * b, A * c + C * d, B * c + D * d, A * e + C * f + E, B * e + D * f + F)


def parse_transform(s):
    m = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)
    if not s:
        return m
    for name, args in re.findall(r'(\w+)\s*\(([^)]*)\)', s):
        v = [float(x) for x in re.findall(r'[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?', args)]
        if name == 'matrix' and len(v) == 6:
            t = tuple(v)
        elif name == 'translate':
            t = (1, 0, 0, 1, v[0], v[1] if len(v) > 1 else 0)
        elif name == 'scale':
            t = (v[0], 0, 0, v[1] if len(v) > 1 else v[0], 0, 0)
        elif name == 'rotate':
            r = math.radians(v[0])
            t = (math.cos(r), math.sin(r), -math.sin(r), math.cos(r), 0, 0)
        else:
            continue
        m = mat_mul(t, m)
    return m


def apply(m, x, y):
    a, b, c, d, e, f = m
    return a * x + c * y + e, b * x + d * y + f


def fmt(v):
    s = '%.6f' % v
    s = s.rstrip('0').rstrip('.')
    return s if s not in ('-0', '') else '0'


# ---- パスを部分パスに切る

def split_path(d):
    """[(部分パスの d（先頭は絶対の M）, 点の並び（パスの座標）)] を返す"""
    toks = TOKEN.findall(d)
    subs = []
    cur_tokens = None
    cur_pts = None
    x = y = 0.0          # 今の点
    sx = sy = 0.0        # 部分パスの始まり
    cmd = None
    i = 0
    first_pair = False   # moveto の最初の組を読んだら、残りは lineto

    def num():
        nonlocal i
        v = float(toks[i])
        i += 1
        return v

    while i < len(toks):
        t = toks[i]
        if t.isalpha():
            cmd = t
            i += 1
            if cmd in 'Zz':
                if cur_tokens is not None:
                    cur_tokens.append('z')
                x, y = sx, sy
                continue
            if cmd in 'Mm':
                first_pair = True
            elif cur_tokens is not None:
                cur_tokens.append(cmd)
            continue
        if cmd is None:
            i += 1
            continue
        low = cmd.lower()
        rel = cmd.islower()
        n = ARGS[low]
        if i + n > len(toks):
            break
        if low == 'm':
            ax, ay = num(), num()
            if first_pair:
                if rel:
                    ax, ay = x + ax, y + ay
                x, y = ax, ay
                sx, sy = x, y
                cur_tokens = ['M', fmt(x), fmt(y)]
                cur_pts = [(x, y)]
                subs.append((cur_tokens, cur_pts))
                first_pair = False
            else:
                # moveto の後ろに続く組は lineto
                cur_tokens += ['l' if rel else 'L', fmt(ax), fmt(ay)]
                if rel:
                    ax, ay = x + ax, y + ay
                x, y = ax, ay
                cur_pts.append((x, y))
            continue
        vals = [num() for _ in range(n)]
        if cur_tokens is None:
            cur_tokens = ['M', fmt(x), fmt(y)]
            cur_pts = [(x, y)]
            subs.append((cur_tokens, cur_pts))
        # 命令の字は上で積んである。同じ命令が続くときは数だけ積む（省略のまま）
        cur_tokens += [fmt(v) for v in vals]
        if low == 'h':
            x = x + vals[0] if rel else vals[0]
            cur_pts.append((x, y))
        elif low == 'v':
            y = y + vals[0] if rel else vals[0]
            cur_pts.append((x, y))
        elif low == 'a':
            ex, ey = vals[5], vals[6]
            if rel:
                ex, ey = x + ex, y + ey
            # 弧は端の点と、半径ぶんの広がりで大まかに囲む
            r = max(abs(vals[0]), abs(vals[1]))
            cur_pts += [(x - r, y - r), (x + r, y + r), (ex - r, ey - r), (ex + r, ey + r)]
            x, y = ex, ey
        else:
            pts = [(vals[k], vals[k + 1]) for k in range(0, n, 2)]
            if rel:
                pts = [(x + px, y + py) for px, py in pts]
            cur_pts += pts
            x, y = pts[-1]
    out = []
    for toks_, pts in subs:
        out.append((' '.join(toks_), pts))
    return out


def bbox(points):
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    return min(xs), min(ys), max(xs), max(ys)


def touch(a, b, margin):
    return not (a[2] + margin < b[0] or b[2] + margin < a[0] or a[3] + margin < b[1] or b[3] + margin < a[1])


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 1
    src, out_dir = sys.argv[1], sys.argv[2]
    ET.register_namespace('', SVG_NS)
    tree = ET.parse(src)
    root = tree.getroot()
    vb = [float(v) for v in root.attrib.get('viewBox', '0 0 %s %s' % (root.attrib['width'], root.attrib['height'])).split()]

    # パネルの論理座標へ。gui.exe は art 0 0 1000 385 に縦横比を保って真ん中に置く
    k = min(1000.0 / vb[2], 385.0 / vb[3])
    ox = (1000.0 - vb[2] * k) / 2.0 - vb[0] * k
    oy = (385.0 - vb[3] * k) / 2.0 - vb[1] * k

    # パスを集める（<g> の変換を重ねる）
    items = []           # (元のパスの番号, style, 変換, 部分パスの d, 論理座標の囲み)

    def visit(e, m):
        m = mat_mul(parse_transform(e.attrib.get('transform', '')), m)
        if tag(e) == 'path':
            style = e.attrib.get('style', '')
            extra = {k2: v for k2, v in e.attrib.items() if k2 in ('fill', 'stroke', 'stroke-width')}
            index = len(paths)
            paths.append((style, extra, m))
            for d, pts in split_path(e.attrib.get('d', '')):
                vp = [apply(m, px, py) for px, py in pts]
                lp = [(ox + px * k, oy + py * k) for px, py in vp]
                items.append((index, d, bbox(lp)))
            return
        for c in e:
            visit(c, m)

    paths = []
    for c in root:
        visit(c, (1.0, 0.0, 0.0, 1.0, 0.0, 0.0))

    # 同じパスの中で、触れ合う部分パスを塊にする（穴の開いた形を離さない）
    parent = list(range(len(items)))

    def find(a):
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a

    def filled(index):
        style, extra, _ = paths[index]
        m = re.search(r'fill\s*:\s*([^;]+)', style)
        v = m.group(1).strip() if m else extra.get('fill', 'black')
        return v != 'none'

    # 線だけのパスは穴の心配が無いので、部分パスを 1 本ずつ部品に振り分ける
    for a in range(len(items)):
        if not filled(items[a][0]):
            continue
        for b in range(a + 1, len(items)):
            if items[a][0] == items[b][0] and touch(items[a][2], items[b][2], 0.5):
                parent[find(a)] = find(b)
    clusters = {}
    for n in range(len(items)):
        clusters.setdefault(find(n), []).append(n)

    # 塊ごとに部品の名前を決める
    groups = {}          # 名前 → [(パスの番号, [d...], 囲み)]
    for members in clusters.values():
        box = bbox([(it[2][0], it[2][1]) for it in (items[n] for n in members)] +
                   [(it[2][2], it[2][3]) for it in (items[n] for n in members)])
        w, h = box[2] - box[0], box[3] - box[1]
        cx, cy = (box[0] + box[2]) / 2, (box[1] + box[3]) / 2
        fill = any(filled(items[n][0]) for n in members)
        name = 'other'
        for key, _, x0, y0, x1, y1, fn in PARTS:
            if x0 <= cx <= x1 and y0 <= cy <= y1 and (fn is None or fn(w, h, fill)):
                name = key
                break
        index = items[members[0]][0]
        groups.setdefault(name, []).append((index, [items[n][1] for n in sorted(members)], box))

    titles = {key: title for key, title, *_ in PARTS if title}
    titles['other'] = 'そのほか（どの部品にも入らない線）'

    os.makedirs(out_dir, exist_ok=True)
    order = []
    for key, *_ in PARTS:
        if key not in order:
            order.append(key)
    order.append('other')

    def path_elements(name, indent):
        out = []
        for n, (index, ds, box) in enumerate(sorted(groups[name], key=lambda g: (g[0], g[2][1], g[2][0]))):
            style, extra, m = paths[index]
            attrs = ' '.join('%s="%s"' % (k2, v) for k2, v in extra.items())
            out.append('%s<path id="%s-%d" style="%s"%s transform="matrix(%s)"\n%s      d="%s" />' % (
                indent, name, n + 1, style, (' ' + attrs) if attrs else '',
                ','.join(fmt(v) for v in m), indent, ' '.join(ds)))
        return out

    head = ('<svg xmlns="http://www.w3.org/2000/svg" '
            'xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" '
            'width="%s" height="%s" viewBox="%s">\n' % (root.attrib.get('width', ''), root.attrib.get('height', ''),
                                                        ' '.join(fmt(v) for v in vb)))
    lines = [head.rstrip('\n'),
             '  <!-- tools/svgsplit.py が %s から作った。部品ごとのレイヤー。変換は各パスに焼き込んである -->' % os.path.basename(src)]
    for name in order:
        if name not in groups:
            continue
        lines.append('  <g id="%s" inkscape:groupmode="layer" inkscape:label="%s">' % (name, titles[name]))
        lines += path_elements(name, '    ')
        lines.append('  </g>')
    lines.append('</svg>')
    with open(os.path.join(out_dir, 'mu2000-parts.svg'), 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(lines) + '\n')

    readme = ['# パネルの絵の部品', '',
              '`tools/svgsplit.py` が `%s` から作った（元の絵は CC0。art/mame/README.md）。' % os.path.basename(src), '',
              '* `mu2000-parts.svg` … 部品ごとのレイヤーに分けた 1 枚。元の絵と同じ見た目で、`panel.txt` の',
              '  `art 0 0 1000 385 "mu2000-mame.svg"` をこれに替えても同じに描ける',
              '* `<部品>.svg` … 部品 1 つずつ。どれも元の絵と同じ大きさ（viewBox）なので、`panel.txt` に',
              '  `art 0 0 1000 385 "parts/<部品>.svg"` を部品の数だけ並べると元の絵になる。',
              '  要らない部品を外したり、1 つだけ描き直したりできる', '',
              '| ファイル | 部品 | パス | 部分パス | だいたいの位置（論理座標 x, y, 幅, 高さ） |', '|---|---|---|---|---|']
    for name in order:
        if name not in groups:
            continue
        g = groups[name]
        box = bbox([(b[2][0], b[2][1]) for b in g] + [(b[2][2], b[2][3]) for b in g])
        subs = sum(len(b[1]) for b in g)
        readme.append('| `%s.svg` | %s | %d | %d | %.0f, %.0f, %.0f, %.0f |' % (
            name, titles[name], len(g), subs, box[0], box[1], box[2] - box[0], box[3] - box[1]))
        one = [head.rstrip('\n'), '  <g id="%s" inkscape:groupmode="layer" inkscape:label="%s">' % (name, titles[name])]
        one += path_elements(name, '    ')
        one += ['  </g>', '</svg>']
        with open(os.path.join(out_dir, name + '.svg'), 'w', encoding='utf-8', newline='\n') as f:
            f.write('\n'.join(one) + '\n')
    readme += ['', '作り直すには', '', '```bash', 'python tools/svgsplit.py %s %s' % (src.replace('\\', '/'), out_dir.replace('\\', '/')), '```', '']
    with open(os.path.join(out_dir, 'README.md'), 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(readme))
    for name in order:
        if name in groups:
            print('%-18s paths %3d  subpaths %4d' % (name, len(groups[name]), sum(len(b[1]) for b in groups[name])))
    return 0


if __name__ == '__main__':
    sys.exit(main())
