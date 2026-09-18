// license:BSD-3-Clause
//
// DAW のオートメーションに見せる XG の値（doc/automation.md）。VST3 と CLAP が同じ表を使う。
//
// 64 パートそれぞれの音量・パン・送り・フィルタ・EG・ビブラート・EQ などと、マスター
// （マスターボリューム・チューン・移調、リバーブ・コーラス・バリエーションの戻り、マスター EQ）、
// インサーション 1-4 のパラメータ 1-16。
//
// **番号は保存した曲が覚えているので、一度決めたら動かさない。** 足すときは空いている所に。
//   パート pp（0-63）の k 番目   65536 + pp × 32 + k    （k は 0-31。PART_KEYS の並び）
//   マスターの k 番目            67584 + k              （MASTER_KEYS の並び）
//   インサーション b（0-3）の n   67648 + b × 16 + n - 1  （n は 1-16）
// VST3 の今ある番号（MIDI の CC 0-32767 ほか、Output 4096、Status 4097）とは重ならない。
//
// 値は XG の値そのもの（定義表の min〜max の整数）。VST3 はそれを 0-1 に畳んで渡す。
// インサーションのパラメータだけは、**種類によって意味も範囲も変わる**ので、そのときの種類の範囲に
// 対する割合を 0-1000 で持つ（0 が下の端、1000 が上の端）。表示はそのときの種類の名前と書式
// （"Drive 40" など）。種類がそのパラメータを持たなければ、音源へは何も送らない。
//
// 音源へは、なるべくコントロールチェンジで入れる（SysEx だと LCD に Ex の印が出るので）。
// firmware で確かめた、XG の値と 1 対 1 で同じになる CC:
//   音量 7、パン 10（1-127。0 のランダムは CC では作れない）、リバーブ 91、コーラス 93、
//   バリエーション 94（接続が SYSTEM のときだけ効く）、カットオフ 74、レゾナンス 71、
//   アタック 73、ディケイ 75、リリース 72、ビブラートの速さ 76・深さ 77・遅れ 78
// CC で入れるのは、パートの受信チャンネルがそのパートだけのものなとき（CC はチャンネルの全パートに効く）。
// それ以外（EQ、ドライ、ノートシフト、マスター、チャンネルが共有・OFF）はパラメータチェンジ。

#ifndef S_MU2000_VST3_AUTOMATION_H
#define S_MU2000_VST3_AUTOMATION_H

#pragma once

#include "ui/snapshot.h"
#include "xg/model.h"

#include <algorithm>
#include <cstdint>
#include <string>
#include <vector>

namespace smu2000 {
namespace automation {

constexpr uint32_t PART_BASE   = 65536;
constexpr uint32_t PART_STRIDE = 32;
constexpr uint32_t MASTER_BASE = PART_BASE + 64 * PART_STRIDE;   // 67584
constexpr uint32_t INS_BASE    = MASTER_BASE + 64;               // 67648
constexpr int      FX_SCALE    = 1000;       // インサーションのパラメータの割合の目盛り

enum class kind : uint8_t {
	xg,             // 定義表の値（p）
	insertion,      // インサーション block のパラメータ number（1-16）
};

struct entry {
	uint32_t id;
	const xg::param *p;     // kind::xg のとき
	int part;               // マスターは 0（定義表の番地のパート番号として渡す）
	bool is_part;
	int cc;                 // 同じ値になる CC（無ければ -1）
	std::string name;       // "A1 Volume" / "Reverb Return" / "INS1 Param 3"
	std::string group;      // "Part A1" / "Master" / "Insertion 1"
	kind k = kind::xg;
	int block = 0;          // kind::insertion のとき 0-3
	int number = 0;         // kind::insertion のとき 1-16
};

// 表。最初に呼んだときに作る（本の糸で先に 1 回呼んでおくこと。音声の糸で初めて作らせない）
const std::vector<entry> &entries();
// 番号から。無ければ -1
int index_of(uint32_t id);
// 定義表の値とパートから（画面で触った値を番号に直す）。無ければ -1
int index_of(const xg::param &p, int part);
// インサーションのパラメータの番地（03 0n xx）から。無ければ -1
int index_of_raw(uint32_t addr);

// 値の範囲
inline int lo(const entry &e) { return e.k == kind::xg ? e.p->min : 0; }
inline int hi(const entry &e) { return e.k == kind::xg ? e.p->max : FX_SCALE; }
inline int def(const entry &e) { return e.k == kind::xg ? std::clamp(e.p->def, e.p->min, e.p->max) : 0; }

// 値の変換
inline int  steps(const entry &e) { return hi(e) - lo(e); }
inline double to_normalized(const entry &e, int value)
{
	const int n = steps(e);
	return n > 0 ? double(value - lo(e)) / double(n) : 0.0;
}
int to_value(const entry &e, double normalized);
int clamp_value(const entry &e, double plain);
// 画面に出す文字（"80 Hz"、"+3 dB"、"L12"、"Drive 40" など）。
// インサーションのパラメータは種類で変わるので、写し（ram）が要る（無ければ割合を出す）
std::string text(const entry &e, int value, const ui::xg_snapshot *ram = nullptr);
// 打った文字から値へ。読めなければ false
bool parse(const entry &e, const char *text, int &value, const ui::xg_snapshot *ram = nullptr);
// インサーションのパラメータの、firmware の値（種類の範囲の中の数）から割合へ。種類が持たなければ false
bool from_raw(const entry &e, const ui::xg_snapshot &ram, int raw, int &value);

// 写しから今の値を読む。読めなければ false
bool current(const entry &e, const ui::xg_snapshot &ram, int &value);

// 音源へ入れる MIDI を作る。out は 16 バイト以上。長さを返し、port に流す口を入れる（0-3）。
// ram が無ければ、いつもパラメータチェンジにする
int midi(const entry &e, int value, const ui::xg_snapshot *ram, uint8_t *out, int &port);

} // namespace automation
} // namespace smu2000

#endif // S_MU2000_VST3_AUTOMATION_H
