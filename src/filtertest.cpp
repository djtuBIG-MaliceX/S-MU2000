// license:BSD-3-Clause
//
// 重複落し（ui/midi_filter.h）の試験。ROM 不要 — 表の上げ下げだけを見る。
// 「落としてはならないもの」を落とさないことが第一、落せる洪水が本当に
// 落ちることが第二。make test の先頭で走る

#include "ui/midi_filter.h"

#include <cstdio>

namespace {

int g_fail = 0;

void expect(bool cond, const char *what)
{
	if (!cond) {
		std::printf("  落ちた: %s\n", what);
		g_fail++;
	}
}

// メッセージを作って duplicate を見る。apply は別途呼ぶ
bool dup(ui::midi_filter &f, int port, int st, int d0 = 0, int d1 = 0, int n = 3)
{
	const uint8_t msg[3] = { uint8_t(st), uint8_t(d0), uint8_t(d1) };
	return f.duplicate(port, msg, n);
}

void feed(ui::midi_filter &f, int port, int st, int d0, int d1, int n = 3)
{
	// watch の真似: ステータス + 完成したデータで apply
	f.apply(port, uint8_t(st), uint8_t(d0), uint8_t(d1));
	(void)n;
}

} // namespace

int main()
{
	ui::midi_filter f;

	// ---- CC: 同じ値の再送は落として、違う値は通す
	expect(!dup(f, 0, 0xB0, 7, 100), "まだ何も届いていない CC7=100 は通す");
	feed(f, 0, 0xB0, 7, 100);
	expect(dup(f, 0, 0xB0, 7, 100), "CC7=100 の再送は落とす");
	expect(!dup(f, 0, 0xB0, 7, 99),  "CC7=99 は違う値なので通す");
	feed(f, 0, 0xB0, 7, 99);
	expect(!dup(f, 0, 0xB0, 7, 100), "90 にしてから 100 を送り直したら通す");

	// ---- 口とチャンネルは別々
	expect(!dup(f, 1, 0xB0, 7, 99),  "口 B ではまだ未知");
	expect(!dup(f, 0, 0xB1, 7, 99),  "ch2 ではまだ未知");

	// ---- 落としてはいけない CC（値が同じでも意味が変わる）
	feed(f, 0, 0xB0, 101, 0);
	feed(f, 0, 0xB0, 100, 0);           // RPN0 を選択
	feed(f, 0, 0xB0, 6, 12);            // ピッチベンド幅 12
	expect(dup(f, 0, 0xB0, 101, 0), "RPN セレクトの再選は落ちる（状態は同じ）");
	expect(!dup(f, 0, 0xB0, 6, 12),  "CC6 は同じ値でも絶対に落とさない");
	expect(!dup(f, 0, 0xB0, 38, 12), "CC38 も同じ");
	feed(f, 0, 0xB0, 100, 1);           // RPN1（モジュレーションレンジ）
	expect(!dup(f, 0, 0xB0, 6, 12),  "選んだパラメータが変われば CC6=12 は仕事");
	expect(!dup(f, 0, 0xB0, 96, 0),  "インクリメントは繰り返しが仕事");
	expect(!dup(f, 0, 0xB0, 97, 0),  "インクリメント（LSB）も");
	expect(!dup(f, 0, 0xB0, 120, 0), "オールノートオフは落ちない（間に鍵が鳴るかも）");
	expect(!dup(f, 0, 0xB0, 123, 0), "オールノートオフも");

	// ---- CC121 のあとは表が白紙
	feed(f, 0, 0xB1, 11, 64);
	expect(dup(f, 0, 0xB1, 11, 64), "CC11=64 の再送は落ちる");
	feed(f, 0, 0xB1, 121, 0);
	expect(!dup(f, 0, 0xB1, 11, 64), "全コントローラ初期化後は再送でも通す");

	// ---- ピッチベンド
	feed(f, 0, 0xE0, 0, 64);            // センター
	expect(dup(f, 0, 0xE0, 0, 64), "同じベンドの再送は落とす");
	expect(!dup(f, 0, 0xE0, 1, 64), "LSB が 1 違えば通す");

	// ---- チャンネルプレッシャー
	feed(f, 0, 0xD0, 50, 0, 2);
	expect(dup(f, 0, 0xD0, 50, 0, 2), "同じ AT は落とす");
	expect(!dup(f, 0, 0xD0, 51, 0, 2), "違う AT は通す");

	// ---- プログラムチェンジ: バンクレールが動いたら同じ番号でも仕事
	feed(f, 0, 0xC0, 42, 0, 2);
	expect(dup(f, 0, 0xC0, 42, 0, 2), "同じバンクの C0 再送は落とす");
	feed(f, 0, 0xB0, 0, 8);             // バンク MSB を動かす
	expect(!dup(f, 0, 0xC0, 42, 0, 2), "バンクが変われば C0 は通す");

	// ---- ノートと AFT とポリAT は毎回仕事
	feed(f, 0, 0x90, 60, 100);
	expect(!dup(f, 0, 0x90, 60, 100), "同じ鍵の再送でもノートは落とさない");
	expect(!dup(f, 0, 0x80, 60, 0),   "ノートオフも");
	expect(!dup(f, 0, 0xA0, 60, 64),  "ポリAT も");

	// ---- SysEx のあとは白紙（裏で値が変わっているかも）
	feed(f, 0, 0xB0, 10, 33);
	expect(dup(f, 0, 0xB0, 10, 33), "CC10=33 は落ちる");
	f.wipe(0);                          // watch が F7 で呼ぶのと同じ
	expect(!dup(f, 0, 0xB0, 10, 33), "SysEx のあとは同じ値でも通す");

	// ---- 完成していないもの・ステータスの無いものは絶対に落とさない
	const uint8_t raw[2] = { 0x33, 0x33 };
	expect(!f.duplicate(0, raw, 2), "ランニングステータスは読まない");
	expect(!dup(f, 0, 0xB0, 7, 0, 2), "データが足りない CC は通す");
	expect(!dup(f, -1, 0xB0, 7, 100), "口の範囲外は通す");
	const uint8_t f5[2] = { 0xF5, 0x02 };
	expect(!f.duplicate(0, f5, 2), "ケーブルメッセージは素通し");

	// ---- 洪水の実演: Automation が 200 回同じ値を送ってきたら
	int killed = 0;
	for (int i = 0; i < 200; i++) {
		if (dup(f, 1, 0xB0, 7, 100)) { killed++; continue; }
		feed(f, 1, 0xB0, 7, 100);
	}
	expect(killed == 199, "同じ値の洪水は 200 通のうち 199 落ちる");

	if (g_fail) {
		std::printf("midi_filter 試験: %d 件落ちた\n", g_fail);
		return 1;
	}
	std::printf("midi_filter 試験: 全部そろった\n");
	return 0;
}
