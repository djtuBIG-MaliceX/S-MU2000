# CLAP の口の定義

CLAP（CLever Audio Plug-in）のヘッダ。**MIT ライセンス**なので、この
リポジトリ（BSD-3-Clause）にそのまま取り込める。ライセンス条文は
LICENSE.txt にある。

出どころ: https://github.com/free-audio/clap の 1.2.10（コミット 195b42a0）

`include/clap` をそのまま `third_party/clap/clap` に置いた。手は加えていない。
ヘッダが C の構造体と関数の表だけなので、差し込む中身は `src/clap/plugin.cpp` に
自前で書いてある。音源と画面は VST3 版（`src/vst3/engine.*`, `view.*`）を共有する。

`-I third_party/clap` で通し、`#include "clap/clap.h"` と書く。
