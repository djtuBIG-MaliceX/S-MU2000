# ROM の吸い出し

S-MU2000 を動かすには、**自分の MU2000 から吸い出した ROM** が要る。ここに
その手順と道具が置いてある。ROM そのものは配らない。

> **ここで手元にできるものは、どれも公開・共有しないこと。**
>
> * `roms/` に置く吸い出した ROM イメージ（波形 ROM、プログラム ROM、LCD の字）
> * `build/` にできるダンパのファームウェアイメージ（`firmware_*.bin`）と `.ydl`
> * 展開したヤマハの更新プログラム
>
> GitHub への再配布も、Issue・Pull Request・Discussion・Release への添付もしない。
> 道具（ソースコード）を公開することと、それで作ったもの・取り出したものを共有することは
> 別の話。くわしくは [README の注意書き](../../README.ja.md#実機由来のデータは配らない載せない)。

要るのは 2 つ。

| | 大きさ | どうやって手に入れるか |
|---|---|---|
| プログラム ROM | 4MB | **吸い出さなくていい。** ヤマハが公開している更新プログラムから復元する |
| 波形 ROM | 32MB | USB ケーブル 1 本で約 36 分。分解も MIDI インターフェースも要らない |

## 1. プログラム ROM（吸い出し不要）

ヤマハの更新プログラム [`mu2r1_uw.zip`](https://jp.yamaha.com/support/updates/mu2r1_uw.html)
に入っている `.ydl` は、先頭 4 バイトを差し替えただけの Standard MIDI File で、
中身は Flash に書き込む SysEx そのもの。そこから元のイメージを組み直せる。

```bash
python tools/dump/ydl_extract.py part1/images/v200U12k.ydl part2/images/v200u22k.ydl \
       -o roms/mu2000_flash.bin
```

出来たものは MAME に登録されている SHA1 と一致する。
くわしくは [rom-dump.md](rom-dump.md)。

## 2. 波形 ROM（USB 経由・約 36 分）

**自作のファームウェアを実機に書き込む。** 手順は [usb.md](usb.md)。

```bash
# ダンパを組み立てる。自分の roms/mu2000_flash.bin を土台にする
python tools/dump/build_firmware.py --module usbdump --words 64 \
       --base roms/mu2000_flash.bin -o build/firmware_usbdump64.bin
python tools/dump/make_ydl.py --image build/firmware_usbdump64.bin \
       --ref roms/updater/x/mu2r1_uw/part2/images/v200u22k.ydl \
       -o build/usbdump64.ydl

# 書き込んで吸う
python tools/dump/stage_ydl.py build/usbdump64.ydl
build/upgrade_dumper/Upgrade.exe                        # 約 12 分
python tools/dump/recv_dump.py --port "Yamaha MU2000-1" --prime-port "Yamaha MU2000-1" \
       --words-per-block 64 --out roms/dump             # 約 36 分
python tools/dump/verify_roms.py roms/dump
```

**ダンパの .ydl は配っていない。** 中身は純正ファームウェア + 自作の
ダンパなので、ヤマハのファームを再配布することになってしまう。上のように
自分の手元で組み立てる。組み上がったものが正しいことは、この repo の
スクリプトだけで作った `.ydl` が実績のあるものと 1 バイトも違わないことで
確かめてある。

### 先に読んでほしいこと

書き換えるのは **本体ファームウェアの領域（0x040000-0x3DFFFF）だけ**で、
ダウンローダ（0x000000-0x00C001）には触れない。だから
[Drum]+[PLAY]+[VALUE+] 起動のダウンロードモードは常に生きていて、純正の
`mu2r1_uw.zip` でいつでも元に戻せる。実機で確認してある。

とはいえ**ファームウェアの書き換えであることに変わりはない。自己責任で**。
戻せなくなったときに困らないよう、始める前に純正アップデータを手元に置き、
ダウンロードモードに入れることを先に確かめておくこと。

うまくいかないときの切り分けは [usb.md](usb.md) の探査ファーム
（`make_usbprobe.py` / `make_usbinit.py` / `make_usbhook.py`）を使う。

## 3. 予備の経路（MIDI 経由・約 3.8 時間）

USB がうまくいかないときは、DIN の MIDI から同じことができる。
[softdump.md](softdump.md) と [procedure.md](procedure.md)。

USB 版と MIDI 版で吸ったものが **1 バイト残らず一致**することを確かめてある。
独立した 2 経路での相互検証になっている。

## 4. LCD のフォントと sin 表

LCD の字は HD44780 の内蔵フォント ROM から。持っていない場合の代替は
`make_standins.py` が作る（見た目は近いが実機とは別物）。

MEG が使う sin 表も同じスクリプトが近似で作る。**実チップの表とは一致しない**
ので、LFO の波形がわずかに違う。

```bash
python tools/dump/make_standins.py
```

## 置き場所

吸い出したものは、S-MU2000 が読む形に並べる。

| ファイル | 中身 |
|---|---|
| `roms/mu2000_flash.bin` | プログラム ROM 4MB |
| `roms/dump/xv364a0.ic49` ほか 3 つ | 波形 ROM 8MB × 4 |
| `roms/standin/sin-table.bin` | MEG が使う sin 表 64KB |
| `roms/hd44780u_b04.bin` | LCD の字（無ければ standin） |

`roms/` は `.gitignore` に入っている。**吸い出したものを公開しないこと**（冒頭の注意書き）。

## 資料

| | |
|---|---|
| [hardware.md](hardware.md) | 実機の中身。基板、チップ、アドレスの割り振り |
| [updater-protocol.md](updater-protocol.md) | `.ydl` の形式とダウンロードモードの手順 |
| [usb.md](usb.md) | USB マイコン（M37640）の調査と USB 版ダンパ |
| [softdump.md](softdump.md) | MIDI 版ダンパ |
| [procedure.md](procedure.md) | MIDI 版の実機での作業手順 |
| [rom-dump.md](rom-dump.md) | チップを外して読む場合（参考） |
| [hello.md](hello.md) | 最初の一歩。LED を光らせるところから |
