# MAME の絵から起こしたパネル

`mu2000-mame.svg` は MAME の `src/mame/layout/mu2000.lay` に埋め込まれて
いた絵をそのまま取り出したもの。

* **license:CC0-1.0**（パブリックドメイン）
* 作った人: hap、Felipe Sanches

CC0 なので条件なしで使ってよい。感謝して使わせてもらっている。

`panel.txt` は `tools/lay2panel.py` が同じ `.lay` から起こしたもので、
MAME の座標（1640 × 680）をこちらの論理座標（1000 × 400）へ移してある。
だから**絵とボタンの位置がぴたりと合う**。

```bash
build/gui.exe <rom ディレクトリ> --layout art/mame/panel.txt
```

作り直すには

```bash
python tools/lay2panel.py <mame>/src/mame/layout/mu2000.lay art/mame
```

ボタン・LED・つまみの絵は `../parts/` を指している。あちらはこちらの
書き起こし（BSD-3-Clause）。

## 部品に分けた絵（`parts/`）

`mu2000-mame.svg` は線と塗りがそれぞれ 1 本の大きなパスにまとまっていて、
「ダイヤルの線だけ直す」がやりにくい。`parts/` は `tools/svgsplit.py` で
それを部品（液晶の枠・ダイヤル・ジャック・下の縁の線…）ごとに分けたもの。

* `parts/mu2000-parts.svg` … 部品ごとの **Inkscape のレイヤー**に分けた 1 枚。
  `panel.txt` の `"mu2000-mame.svg"` をこれに替えても同じ見た目になる
* `parts/<部品>.svg` … 部品 1 つずつ。`art 0 0 1000 385 "parts/dial.svg"` のように
  部品の数だけ並べると元の絵になる。要らない部品を外す・1 つだけ描き直す、ができる

分けても描いた結果は元と 1 画素も変わらない（`gui.exe --shot` で確かめた）。
一覧は `parts/README.md`。
