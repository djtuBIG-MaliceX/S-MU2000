# パネルの絵の部品

`tools/svgsplit.py` が `mu2000-mame.svg` から作った（元の絵は CC0。art/mame/README.md）。

* `mu2000-parts.svg` … 部品ごとのレイヤーに分けた 1 枚。元の絵と同じ見た目で、`panel.txt` の
  `art 0 0 1000 385 "mu2000-mame.svg"` をこれに替えても同じに描ける
* `<部品>.svg` … 部品 1 つずつ。どれも元の絵と同じ大きさ（viewBox）なので、`panel.txt` に
  `art 0 0 1000 385 "parts/<部品>.svg"` を部品の数だけ並べると元の絵になる。
  要らない部品を外したり、1 つだけ描き直したりできる

| ファイル | 部品 | パス | 部分パス | だいたいの位置（論理座標 x, y, 幅, 高さ） |
|---|---|---|---|---|
| `body.svg` | 本体の外形 | 2 | 2 | 51, 15, 898, 357 |
| `plg-lamps.svg` | MU / PLG の表示灯の台 | 4 | 4 | 487, 329, 130, 17 |
| `grille.svg` | 下の縁のすき間の線 | 102 | 102 | 60, 329, 879, 17 |
| `card.svg` | SmartMedia の差し込み口 | 4 | 5 | 96, 310, 172, 35 |
| `lcd-marks.svg` | 液晶の下の目印（PART〜KEY）と右の目印（XG / GS / PERFORM） | 12 | 12 | 345, 137, 302, 41 |
| `lcd.svg` | 液晶の窓の枠 | 5 | 5 | 267, 49, 383, 133 |
| `ad-level.svg` | A/D INPUT の音量つまみ | 15 | 16 | 167, 62, 54, 54 |
| `volume.svg` | VOLUME のつまみ | 15 | 16 | 167, 133, 54, 54 |
| `ad-input.svg` | A/D INPUT の 2 つのジャックと括り線 | 6 | 8 | 84, 61, 75, 136 |
| `power.svg` | STANDBY / ON のスイッチ | 3 | 3 | 62, 226, 62, 74 |
| `midi-in.svg` | MIDI IN A の端子 | 1 | 8 | 140, 212, 74, 74 |
| `phones.svg` | PHONES のジャック | 2 | 3 | 233, 241, 32, 32 |
| `select-guide.svg` | SELECT の横の三角と、下の折れ線 | 2 | 2 | 626, 217, 48, 104 |
| `dial.svg` | ジョグダイヤル | 2 | 2 | 798, 195, 131, 131 |
| `nav-marks.svg` | ALL の括り線と、PART / SELECT / VALUE の − ＋ の印 | 8 | 14 | 863, 48, 53, 114 |

作り直すには

```bash
python tools/svgsplit.py art/mame/mu2000-mame.svg art/mame/parts
```
