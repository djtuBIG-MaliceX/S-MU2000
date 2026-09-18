// license:BSD-3-Clause
//
// **firmware を走らせずに音を鳴らす口**（doc/native-engine.md の段 2）。
//
// 考え方はこう。
//
//   * 起動と、音色を選ぶところ（プログラムチェンジ・SysEx）は firmware に任せる。
//     そこは曲の頭で数回しか起きないので、重さに効かない
//   * **その音色の 1 音目も firmware に鳴らさせて、スロットに書かれた値を写し取る**
//     （voice_cal）。式が分かっていない所（フィルタ・素通しの量など）はこれで埋まる
//   * 2 音目からは CPU を止めたまま、この口が式でレジスタを作って鳴らす
//
// 鍵と強さで動くもの（音程・波形・包絡線・音量）は式で出すので、写し取りは
// 音色あたり 1 回で足りる。覚えるのは利用者の ROM から起こした値で、配らない。

#ifndef S_MU2000_XG_NATIVE_DRIVER_H
#define S_MU2000_XG_NATIVE_DRIVER_H

#pragma once

#include "xg/native_voice.h"
#include "xg/ram.h"

#include <algorithm>
#include <array>
#include <cstdio>
#include <cstdlib>
#include <functional>
#include <unordered_map>
#include <vector>

namespace xg {

class native_driver
{
public:
	static constexpr int PARTS = 64;
	static constexpr int SLOTS = 64;

	// SMU2000_NATIVE_DEBUG が立っていれば、鳴らすたびに値を出す（調べもの用）
	static bool debug_on()
	{
		static const bool on = std::getenv("SMU2000_NATIVE_DEBUG") != nullptr;
		return on;
	}

	// スロット 1 つの使われ方
	struct slot_use {
		bool on = false;
		u32 tpos = 0;                   // フィルタの包絡線の、つぎに書く段
		u64 tstart = 0;                 // 鳴らし始めた時刻
		bool held = false;              // ダンパーで離しを待たせている
		int part = -1, note = -1, att = 0;
		const u8 *elem = nullptr;
		const u8 *wave = nullptr;       // ベンドで音程を作り直すのに要る
		const nv::voice_cal *cal = nullptr;
		u16 lfo = 0;                    // いま鳴らしている 0x0a（モジュレーションを足す前）
		u16 cut = 0;                    // いま鳴らしている 0x00（明るさを足す前）
		u16 drum_rel = 0;
		// **ポルタメント**。glide は「まだ残っている音程のずれ」（セント × 256。
		// 前の鍵の側が正にも負にもなる）。10ms ごとに step ずつ 0 へ寄せる
		s32 glide = 0, glide_step = 0;
		u64 glide_next = 0;
		u64 age = 0;
	};

	// SWP30 のマスタへ 1 レジスタ書く口
	using poke_fn = std::function<void(u32 reg, u16 value)>;

	void set_poke(poke_fn f) { m_poke = std::move(f); }
	void set_rom(const u8 *rom) { m_rom = rom; }
	// ワーク RAM（firmware が音色を選んだ結果を読む）
	void set_ram(const u8 *ram) { m_ram = ram; }

	void reset()
	{
		m_cal.clear();
		m_drum.clear();
		for (auto &s : m_slot)
			s = slot_use();
		for (auto &c : m_cc)
			c = part_cc();
		for (auto &s : m_seen)
			s = ram_seen();
		for (auto &r : m_recsel)
			r = 0;
		for (u64 &t : m_fw_touch)
			t = 0;
		for (auto &d : m_recsel_drum)
			d = -1;
		m_clock = 0;
		m_traj = false;
		m_rec = false;
		m_traj_next = 0;
		m_pend.clear();
		m_age = 0;
	}

	// 写し取りの覚え先の鍵。**音色の記録（下 32bit）＋パートの経路（上 32bit）**。
	// 経路が違えば別物として覚えるので、つまみを行き来しても取り直しは 1 度で済む
	u64 cal_key(u32 rec, int part) const { return u64(rec) | (u64(part_ctx(part)) << 32); }

	// 覚えておく写し取りの上限。ふだんは音色の数だけなので数十で足りるが、
	// DAW がつまみを掃くと経路の印がそのぶん増えるので、天井を付けておく。
	// 溢れたら覚えないだけ（その音は firmware が鳴らす）
	static constexpr size_t CAL_MAX = 512;

	// firmware に鳴らさせた 1 音から写し取る。鍵は写しに入っている経路から組む
	void learn(u32 rec, std::vector<nv::voice_cal> cals)
	{
		if (cals.empty() || m_cal.size() >= CAL_MAX)
			return;
		const u64 k = u64(rec) | (u64(cals[0].cal_ctx) << 32);
		if (m_cal.find(k) == m_cal.end())
			m_cal[k] = std::move(cals);
	}

	// ドラムは音ごとに中身が違うので、**鍵ごと**に覚える。
	// 同じ音を何度も叩くので、これだけで打楽器のほとんどが native になる
	void learn_drum(u64 key, std::vector<nv::voice_cal> cals)
	{
		if (cals.empty() || m_drum.size() >= CAL_MAX)
			return;
		if (m_drum.find(key) == m_drum.end())
			m_drum[key] = std::move(cals);
	}

	// ドラムのパートか。XG の「パートモード」（08 pp 07。0 が普通、1 以上がドラム）を見る。
	// 「記録が引けない＝ドラム」では、音色を選び終える前の旋律パートまで拾ってしまう
	bool is_drum(int part) const
	{
		if (part >= 0 && part < PARTS && m_recsel_drum[part] >= 0)
			return m_recsel_drum[part] != 0;
		if (!m_ram || part < 0 || part >= PARTS)
			return false;
		return m_ram[ram::part_base(part) + 0x07] != 0;
	}

	// 写し取ったものを取っておく・戻す（voicecache.h）
	const std::unordered_map<u64, std::vector<nv::voice_cal>> &cal_map() const { return m_cal; }
	const std::unordered_map<u64, std::vector<nv::voice_cal>> &drum_map() const { return m_drum; }
	size_t cal_count() const { return m_cal.size() + m_drum.size(); }
	int peak_slots() const { return m_peak; }

	// **firmware が最近触ったスロット**を覚える。firmware はこちらの使用中を
	// 知らないので、避けないと「firmware が自分の音の続きを書く」ときに
	// こちらの音が壊れる（doc/native-engine.md の 6.47）。
	// 呼ぶのは mu2000 のバス書き込みの所（firmware の書き込みだけが通る）
	void mark_fw_slots(u64 mask)
	{
		for (int i = 0; i < SLOTS; i++)
			if ((mask >> i) & 1)
				m_fw_touch[i] = m_clock + 1;   // 0 は「触っていない」
	}

	// そのスロットを firmware がまだ使っていそうか
	bool fw_recent(int slot) const
	{
		const u64 t = m_fw_touch[slot];
		return t && m_clock + 1 - t < FW_KEEP;
	}

	// いまこちらが鳴らしているスロットの印。firmware が写し取りのために
	// 鳴らすとき、ここと重なっていないかを見るのに使う
	u64 slot_mask() const
	{
		u64 m = 0;
		for (int i = 0; i < SLOTS; i++)
			if (m_slot[i].on)
				m |= u64(1) << i;
		return m;
	}

	// 写し取りの最中は、段が後から増えるので毎サンプル見る
	void set_recording(bool on) { m_rec = on; m_traj_next = 0; }

	// 写し取ったものを、あとから直せるように渡す（フィルタの包絡線の追記用）
	std::vector<nv::voice_cal> *cals_of(u32 rec, int part)
	{
		const auto it = m_cal.find(cal_key(rec, part));
		return it == m_cal.end() ? nullptr : &it->second;
	}
	std::vector<nv::voice_cal> *drum_cals_of(u64 key)
	{
		const auto it = m_drum.find(key);
		return it == m_drum.end() ? nullptr : &it->second;
	}

	// フィルタの包絡線を流し、遅らせた要素を鳴らす。1 サンプルに 1 回呼ぶ
	void tick(u64 clock)
	{
		m_clock = clock;
		if (!m_pend.empty()) {
			size_t w = 0;
			for (size_t i = 0; i < m_pend.size(); i++) {
				if (m_pend[i].at <= clock)
					key_on(m_pend[i].mask);
				else
					m_pend[w++] = m_pend[i];
			}
			m_pend.resize(w);
		}
		if (!m_traj)
			return;
		// **つぎの段の時刻まで何もしない**。ここを毎サンプル 64 スロット見ていると、
		// SH-2 を止めた意味が薄れるくらい重かった。
		// 写し取りの最中だけは、段が後から増えるので毎回見る
		if (!m_rec && clock < m_traj_next)
			return;
		u64 next = ~u64(0);
		int live = 0;
		for (int i = 0; i < SLOTS; i++) {
			slot_use &s = m_slot[i];
			// 写し取りの途中で段が増えることがあるので、まだ段が無くても数える
			if (!s.on || !s.cal)
				continue;
			live++;
			// ポルタメント: 10ms ごとに残りのずれを step だけ 0 へ寄せて、
			// 音程のレジスタを書き直す（6.41）
			if (s.glide && s.elem && s.wave) {
				while (s.glide && s.glide_next <= clock) {
					if (s.glide > 0)
						s.glide = s.glide > s.glide_step ? s.glide - s.glide_step : 0;
					else
						s.glide = -s.glide > s.glide_step ? s.glide + s.glide_step : 0;
					s.glide_next += nv::PORTA_TICK;
				}
				m_poke(u32(i) * 64 + 0x11, pitch_of(s));
			}
			if (s.glide && s.glide_next < next)
				next = s.glide_next;
			if (s.tpos >= s.cal->filter_env.size())
				continue;
			const std::vector<nv::fstep> &fe = s.cal->filter_env;
			while (s.tpos < fe.size() && s.tstart + fe[s.tpos].at <= clock) {
				u16 v = fe[s.tpos].v;
				if (fe[s.tpos].reg == 0x0a) {      // 深さにモジュレーションを足す
					s.lfo = v;
					v = lfo_reg(v, *s.cal, s.part);
				} else if (fe[s.tpos].reg == 0x00) {   // 切る高さに明るさを足す
					s.cut = v;
					v = cutoff_reg(v, *s.cal, s.part);
				} else if (fe[s.tpos].reg == 0x04) {
					v = reso_reg(v, *s.cal, s.part);
				}
				m_poke(u32(i) * 64 + fe[s.tpos].reg, v);
				s.tpos++;
			}
			if (s.tpos < fe.size() && s.tstart + fe[s.tpos].at < next)
				next = s.tstart + fe[s.tpos].at;
		}
		m_traj = live > 0;
		m_traj_next = next;
	}

	// ドラムの覚え先の鍵（バンクとプログラムと音の高さ）
	u64 drum_key(int part, int note) const
	{
		if (!m_ram)
			return 0;
		const u8 *p = m_ram + ram::part_base(part);
		return u64(p[1]) << 24 | u64(p[2]) << 16 | u64(p[3]) << 8 | u64(note & 0x7f) |
		       (u64(part_ctx(part)) << 32);
	}
	bool drum_known(int part, int note) const
	{
		return m_drum.find(drum_key(part, note)) != m_drum.end();
	}

	// **音色を自分で決める**（xg::voice_rom::lookup。旋律系のバンク 640 音色で
	// firmware と食い違い 0 だった）。0 を渡すと、またワーク RAM を見る
	void set_record(int part, u32 rec, int drum)
	{
		if (part < 0 || part >= PARTS)
			return;
		m_recsel[part] = rec;
		m_recsel_drum[part] = s8(drum);
		// **音色を替えると firmware がつまみを音色の既定値で上書きする**
		// （XG の決まり）。実測: 曲が CC91=40 を送っていても、そのあとの
		// プログラムチェンジでパートの塊 +0x13 が 33 や 31 になっていた。
		// こちらが CC の生値を握ったままだと、送りの差分が丸ごと狂う。
		// -1 に戻して、firmware が処理し終えたあと sync_cc() で読み直す
		forget_cc(part);
	}

	// そのパートの「こちらが覚えているつまみ」を捨てて、ワーク RAM から
	// 読み直させる（firmware が書き替えたかもしれないとき）
	void forget_cc(int part)
	{
		if (part < 0 || part >= PARTS)
			return;
		part_cc &p = m_cc[part];
		p.vol = p.expr = p.pan = p.mod = -1;
		p.rev = p.cho = p.bri = p.res = -1;
	}

	// パートの音色の記録。自分で引けていればそれを、そうでなければワーク RAM を読む
	u32 record_of(int part) const
	{
		if (part >= 0 && part < PARTS && m_recsel[part])
			return m_recsel[part];
		if (!m_ram || part < 0 || part >= PARTS)
			return 0;
		const u8 *p = m_ram + ram::part_base(part);
		const u32 r = u32(p[ram::PART_VOICE]) << 24 | u32(p[ram::PART_VOICE + 1]) << 16 |
		              u32(p[ram::PART_VOICE + 2]) << 8 | p[ram::PART_VOICE + 3];
		return (r >= 0x200ee0 && r + 16 <= 0x23cece) ? r : 0;
	}


	// ---- コントローラ（doc/native-engine.md の 6.14）
	//
	// これを native 側で持つと、DAW の自動演奏でつまみが動いても SH-2 が起きない。
	// 実機と同じレジスタを、実機と同じ式で書く

	// パートごとの、いまのつまみの位置
	struct part_cc {
		// -1 は「まだ動かされていない＝写し取ったときのまま」
		int vol = -1, expr = -1, pan = -1;     // CC7 / CC11 / CC10
		int mod = -1;                          // CC1（モジュレーション）
		int rev = -1, cho = -1;                // CC91 / CC93（送り）
		int bri = -1, res = -1;                // CC74 / CC71（明るさ・共振）
		int var = -1;                          // CC94（バリエーション送り）
		// ポルタメント（CC5 速さ・CC65 入切・CC84 で滑り出す鍵を指定）。
		// last は最後に押した鍵で、つぎの音はここから滑る
		int porta_time = 0, porta_src = -1, last = -1;
		bool porta_on = false;
		// **こちらでさばけない CC が既定から外れている**印（ビットごとに 1 つ）。
		// 立っている間、そのパートの音は firmware に鳴らしてもらう。
		// 黙って無視すると、ポルタメントや EG の設定が効かない音になる
		u32 unknown = 0;
		int bend = 8192, range = 2;            // ピッチベンドと、その幅（半音）
		bool damper = false;                   // CC64
	};

	// firmware を回したあとに、パートの音量・表現・パンをワーク RAM から取り直す。
	// SysEx やパネルで変えられた場合も、これで追い付く
	// ワーク RAM のその値を、こちらの控えに取り込むか決める。
	//
	// 前は「こちらが触っていない（-1）ものだけ拾う」だった。それだと
	// **firmware が裏で書き替えたとき**に気づけない。実際、音色を替えると
	// firmware はパートのつまみを音色の既定値で上書きする（XG の決まり）。
	// 曲が CC91=40 を送っていても、そのあとのプログラムチェンジで
	// パートの塊 +0x13 は 33 になっていた。こちらが 40 を握ったままだと
	// 送りの差分が丸ごと狂う（実測でリバーブ送りが 5 段ずれた）。
	//
	// そこで**前に見た RAM の値**を覚えておき、RAM が動いていたら
	// 「firmware が書き替えた」とみなして取り込む。動いていなければ
	// こちらの値（まだ firmware が処理していない新しい CC）を残す
	void take_ram(int &mine, u8 &seen, u8 now)
	{
		if (mine < 0 || now != seen)
			mine = now;
		seen = now;
	}

	void sync_cc()
	{
		if (!m_ram)
			return;
		for (int p = 0; p < PARTS; p++) {
			const u8 *b = m_ram + ram::part_base(p);
			ram_seen &s = m_seen[p];
			take_ram(m_cc[p].vol,  s.vol,  b[0x0b]);
			take_ram(m_cc[p].expr, s.expr, b[ram::PART_EXP]);
			take_ram(m_cc[p].pan,  s.pan,  b[0x0e]);
			take_ram(m_cc[p].mod,  s.mod,  b[ram::PART_MOD]);
			take_ram(m_cc[p].rev,  s.rev,  b[0x13]);
			take_ram(m_cc[p].cho,  s.cho,  b[0x12]);
			take_ram(m_cc[p].bri,  s.bri,  b[0x18]);
			take_ram(m_cc[p].res,  s.res,  b[0x19]);
			// ベンド幅（08 pp 23。64 が 0 半音）。RPN でも SysEx でもここに入る
			const int r2 = int(b[0x23]) - 64;
			m_cc[p].range = r2 < 0 ? 0 : (r2 > 24 ? 24 : r2);
		}
	}

	// **パートの「経路」の印**。素通しの量（08 pp 11）・バリエーション送り（14）・
	// パートの EQ（+0x6A-0x6F）・インサーション 4 つの掛かり先を混ぜる。
	// 写し取りはこの経路ごとの値なので、違う経路では使い回せない
	u32 part_ctx(int part) const
	{
		if (!m_ram || part < 0 || part >= PARTS)
			return 0;
		u32 h = 2166136261u;
		auto mix = [&h](u8 x) { h ^= x; h *= 16777619u; };
		const u8 *b = m_ram + ram::part_base(part);
		mix(b[0x11]);
		mix(b[0x14]);
		// EG のつまみ（CC73 アタック +0x1a・CC75 ディケイ +0x1b・CC72 リリース +0x1c）。
		// この 3 つは式が起こせていない（CC73 は 0x06 だけでなく 0x00・0x07・0x0b も
		// 動かす多目標のつまみだった）。**式の代わりに写し取り直す**：
		// ここに混ぜておくと、つまみが動いた時点で写し取りが別物になり、
		// 次の 1 音だけ firmware が鳴らして取り直す。以後はまた native
		mix(b[0x1a]);
		mix(b[0x1b]);
		mix(b[0x1c]);
		// バリエーション送り（CC94）は口の側で覚えたものを使う
		mix(u8(m_cc[part].var < 0 ? 0 : m_cc[part].var));
		for (int i = 0; i < 6; i++)
			mix(b[ram::PART_EQ_RAM + i]);
		for (int n = 0; n < 4; n++)
			mix(m_ram[ram::INS_BLOCK[n] + 0x0c]);
		return h ? h : 1;
	}

	// 写し取ったときのつまみの位置（ワーク RAM から）
	int part_vol(int part) const  { return m_ram ? int(m_ram[ram::part_base(part) + 0x0b]) : 100; }
	int part_expr(int part) const { return m_ram ? int(m_ram[ram::part_base(part) + ram::PART_EXP]) : 127; }
	int part_pan(int part) const  { return m_ram ? int(m_ram[ram::part_base(part) + 0x0e]) : 64; }
	int part_mod(int part) const  { return m_ram ? int(m_ram[ram::part_base(part) + ram::PART_MOD]) : 0; }
	int part_rev(int part) const  { return m_ram ? int(m_ram[ram::part_base(part) + 0x13]) : 40; }
	int part_cho(int part) const  { return m_ram ? int(m_ram[ram::part_base(part) + 0x12]) : 0; }
	int part_bri(int part) const  { return m_ram ? int(m_ram[ram::part_base(part) + 0x18]) : 64; }
	int part_res(int part) const  { return m_ram ? int(m_ram[ram::part_base(part) + 0x19]) : 64; }

	// ---- つまみの割り当て（doc/native-engine.md の 6.43）
	//
	// XG の「モジュレーション・ベンド・アフタータッチ・AC1・AC2 が音の何を
	// どれだけ動かすか」は、パートの塊に**6 つ組**（音程・フィルタ・音量・
	// LFO の PMOD/FMOD/AMOD）で並んでいる。位置は `nativeplay --xgmap` で
	// XG のアドレスを 1 つずつ書いて見つけた（08 pp 4D → +0x46 など）。
	//
	// **既定のままなら、そのつまみは SWP30 のレジスタを 1 つも動かさない**
	// （`nativeplay --at` で確かめた）。だから既定のあいだは firmware に
	// 任せる必要がない。既定から外れているときだけ任せる
	static constexpr u32 MW_BLOCK  = 0x1d;   // モジュレーション（CC1）
	static constexpr u32 PB_BLOCK  = 0x23;   // ベンド（+0x23 は幅なので別扱い）
	static constexpr u32 AT_BLOCK  = 0x46;   // アフタータッチ（08 pp 4D-52）
	static constexpr u32 PAT_BLOCK = 0x4c;   // 鍵ごとのアフタータッチ
	static constexpr u32 AC1_NUM   = 0x52;   // AC1 の CC 番号（既定 16）
	static constexpr u32 AC1_BLOCK = 0x53;
	static constexpr u32 AC2_NUM   = 0x59;   // AC2 の CC 番号（既定 17）
	static constexpr u32 AC2_BLOCK = 0x5a;

	// その 6 つ組が既定（＝音に何も起きない）か。既定は 64,64,64,0,0,0
	bool assign_idle(int part, u32 off) const
	{
		if (!m_ram || part < 0 || part >= PARTS)
			return false;                  // 分からないときは任せる側に倒す
		const u8 *b = m_ram + ram::part_base(part) + off;
		return b[0] == 64 && b[1] == 64 && b[2] == 64 && !b[3] && !b[4] && !b[5];
	}

	// モジュレーションの割り当ては既定が 64,64,64,**10**,0,0（LFO の音程が 10）。
	// ここが動いていると、こちらの CC1 の式（6.14 の 10 段の表）が合わない
	bool mod_idle(int part) const
	{
		if (!m_ram || part < 0 || part >= PARTS)
			return false;
		const u8 *b = m_ram + ram::part_base(part) + MW_BLOCK;
		return b[0] == 64 && b[1] == 64 && b[2] == 64 && b[3] == 10 && !b[4] && !b[5];
	}

	// ベンドは +0x23 が幅（RPN で普通に動く。こちらも読んでいる）なので、
	// 音程以外の 5 つだけを見る
	bool bend_idle(int part) const
	{
		if (!m_ram || part < 0 || part >= PARTS)
			return false;
		const u8 *b = m_ram + ram::part_base(part) + PB_BLOCK;
		return b[1] == 64 && b[2] == 64 && !b[3] && !b[4] && !b[5];
	}

	// アフタータッチ（触れた強さ）。**割り当てが既定なら音に何も起きない**
	void aftertouch(int part, bool poly)
	{
		if (part < 0 || part >= PARTS)
			return;
		const u32 bit = poly ? 30u : 31u;
		if (assign_idle(part, poly ? PAT_BLOCK : AT_BLOCK))
			m_cc[part].unknown &= ~(1u << bit);
		else
			m_cc[part].unknown |= 1u << bit;
	}

	// AC1・AC2（好きな CC を割り当てられるつまみ）。番号が合っていて割り当てが
	// 既定から外れていれば、native では何も起きないので firmware に任せる
	void assignable(int part, int cc, int value)
	{
		if (!m_ram || part < 0 || part >= PARTS)
			return;
		const u8 *pb = m_ram + ram::part_base(part);
		const u32 num[2] = { AC1_NUM, AC2_NUM };
		const u32 blk[2] = { AC1_BLOCK, AC2_BLOCK };
		for (int k = 0; k < 2; k++) {
			if (cc != int(pb[num[k]]))
				continue;
			const u32 bit = k ? 28u : 29u;
			if (value && !assign_idle(part, blk[k]))
				m_cc[part].unknown |= 1u << bit;
			else
				m_cc[part].unknown &= ~(1u << bit);
		}
	}


	// その CC を native でさばけるか（実際にさばく前に決める）
	static bool handles_cc(int cc)
	{
		return cc == 0x07 || cc == 0x0b || cc == 0x0a || cc == 0x40 || cc == 0x01 ||
		       cc == 0x5b || cc == 0x5d || cc == 0x4a || cc == 0x47 ||
		       cc == 0x05 || cc == 0x41 || cc == 0x54;
	}

	// CC を受ける。native でさばけたら true（firmware にも短く回す）
	bool control(int part, int cc, int value)
	{
		if (part < 0 || part >= PARTS)
			return false;
		part_cc &p = m_cc[part];
		switch (cc) {
		case 0x07: p.vol = value; break;
		case 0x0b: p.expr = value; break;
		case 0x0a: p.pan = value; break;
		case 0x01:
			p.mod = value;
			// モジュレーションの割り当てが動いていると、こちらの式が合わない
			if (value && !mod_idle(part))
				p.unknown |= 1u << 27;
			else
				p.unknown &= ~(1u << 27);
			break;
		case 0x5b: p.rev = value; break;
		case 0x5d: p.cho = value; break;
		case 0x4a: p.bri = value; break;
		case 0x47: p.res = value; break;
		case 0x05: p.porta_time = value; return true;     // ポルタメントの速さ
		case 0x41: p.porta_on = value >= 64; return true; // ポルタメント 入切
		case 0x54: p.porta_src = value & 0x7f; return true;   // 滑り出す鍵を指定
		case 0x40:                             // ダンパー
			p.damper = value >= 64;
			if (!p.damper)
				release_held(part);
			return true;
		case 0x78: case 0x7b:                  // 音を全部切る
			all_off(part);
			return false;
		default: {
			// バリエーション送り。ワーク RAM には出てこない（掛かり先が
			// パートに繋がっていないと firmware が何も書かない）ので、
			// **口の側で覚えて経路の印に混ぜる**。送りの値は写し取った
			// ミキサのレジスタに入っているので、値が変われば取り直せばよい
			if (cc == 0x5e) {
				p.var = value;
				return false;                  // firmware にも見せる（写し取りのため）
			}
			// 知らない CC は AC1・AC2 に割り当てられているかもしれない
			assignable(part, cc, value);
			return false;                      // 知らない CC は firmware に任せる
		}
		}
		apply_cc(part);
		return true;
	}

	void bend(int part, int value14)
	{
		if (part < 0 || part >= PARTS)
			return;
		m_cc[part].bend = value14;
		// ベンドの割り当て（音程以外）が動いていると、こちらの式が合わない
		if (value14 != 8192 && !bend_idle(part))
			m_cc[part].unknown |= 1u << 26;
		else
			m_cc[part].unknown &= ~(1u << 26);
		apply_bend(part);
	}

	void set_bend_range(int part, int semitones)
	{
		if (part >= 0 && part < PARTS)
			m_cc[part].range = semitones;
	}

	void reset_cc(int part)
	{
		if (part >= 0 && part < PARTS)
			m_cc[part] = part_cc();
	}

	const part_cc &cc_of(int part) const { return m_cc[part]; }

private:
	// いま鳴っているスロットに、つまみの動きを反映する
	void apply_cc(int part)
	{
		for (int i = 0; i < SLOTS; i++) {
			slot_use &s = m_slot[i];
			if (!s.on || s.part != part || !s.cal)
				continue;
			m_poke(u32(i) * 64 + 9, u16(note_att(s, part)));
			if (s.cal->has(0x32))
				m_poke(u32(i) * 64 + 0x32, pan_reg(*s.cal, part));
			if (s.lfo)
				m_poke(u32(i) * 64 + 0x0a, lfo_reg(s.lfo, *s.cal, part));
			if (s.cal->has(0x33))
				m_poke(u32(i) * 64 + 0x33,
				       send_reg(*s.cal, 0x33, false, m_cc[part].rev, s.cal->cal_rev));
			if (s.cal->has(0x34))
				m_poke(u32(i) * 64 + 0x34,
				       send_reg(*s.cal, 0x34, true, m_cc[part].cho, s.cal->cal_cho));
			if (s.cut)
				m_poke(u32(i) * 64 + 0x00, cutoff_reg(s.cut, *s.cal, part));
			if (s.cal->has(0x04))
				m_poke(u32(i) * 64 + 0x04, reso_reg(s.cal->reg[0x04], *s.cal, part));
		}
	}

	// そのスロットの、いまの音程レジスタ（ベンドと滑りの残りを入れて作る）
	u16 pitch_of(const slot_use &s) const
	{
		const part_cc &pc = m_cc[s.part];
		return nv::pitch_reg(nv::read_wave(s.wave), s.note, nv::key_follow(s.elem),
		                     nv::bend_cents(pc.bend, pc.range) + nv::elem_tune(s.elem)
		                     + s.glide / 256);
	}

	void apply_bend(int part)
	{
		for (int i = 0; i < SLOTS; i++) {
			slot_use &s = m_slot[i];
			if (!s.on || s.part != part || !s.elem || !s.wave)
				continue;
			m_poke(u32(i) * 64 + 0x11, pitch_of(s));
		}
	}

	// ダンパーを離したとき、待たせていた音を切る
	void release_held(int part)
	{
		for (int i = 0; i < SLOTS; i++) {
			slot_use &s = m_slot[i];
			if (s.on && s.held && s.part == part) {
				s.held = false;
				note_off(part, s.note);
			}
		}
	}

	// つまみのぶんを足した減衰
	int note_att(const slot_use &s, int part) const
	{
		const part_cc &p = m_cc[part];
		const nv::voice_cal *c = s.cal;
		int a = s.att;
		if (c && (p.vol >= 0 || p.expr >= 0)) {
			// **掛けてから一度だけ減衰に直す**（nv::vol_gain を見よ）。
			// 触られていない側は写し取ったときの値のまま
			const int now = nv::vol_gain(p.vol >= 0 ? p.vol : c->cal_vol,
			                             p.expr >= 0 ? p.expr : c->cal_expr);
			const int was = nv::vol_gain(c->cal_vol, c->cal_expr);
			a += nv::gain_att(m_rom, now) - nv::gain_att(m_rom, was);
		}
		return nv::clamp_att(a);
	}

	// フィルタのレジスタ。下 12bit が切る高さで、明るさ（CC74）のぶんをずらす
	u16 cutoff_reg(u16 base, const nv::voice_cal &c, int part) const
	{
		const int now = m_cc[part].bri;
		if (now < 0 || now == c.cal_bri)
			return base;
		int v = int(base & 0xfff) + nv::bright_shift(now) - nv::bright_shift(c.cal_bri);
		v = v < 0 ? 0 : (v > nv::CUTOFF_MAX ? nv::CUTOFF_MAX : v);
		return u16((base & 0xf000) | u16(v));
	}

	// 共振のレジスタ。上 5bit が共振で、CC71 のぶんをずらす
	u16 reso_reg(u16 base, const nv::voice_cal &c, int part) const
	{
		const int now = m_cc[part].res;
		if (now < 0 || now == c.cal_res)
			return base;
		int v = int(base >> 11) + nv::reso_shift(now) - nv::reso_shift(c.cal_res);
		v = v < 0 ? 0 : (v > 31 ? 31 : v);
		return u16((base & 0x07ff) | u16(v << 11));
	}

	// 送りのレジスタ。下位が減衰で、写し取ったときからの差ぶんだけ動かす。
	// 写し取ったときに切れていた（0xff）送りは差が取れないので、
	// **もう一方の送りから下駄を借りる**（どちらもパートの同じ下駄に乗っている）
	// 0x32-0x37 は 1 つで 2 本ぶんの送りを持つ。リバーブは 0x33 の**下位**、
	// コーラスは 0x34 の**上位**（nativeplay --ccwatch で確かめた）
	u16 send_reg(const nv::voice_cal &c, int which, bool hi, int now, int was) const
	{
		const u16 base = c.reg[which];
		if (now < 0 || now == was)
			return base;
		const int cur = hi ? (base >> 8) : (base & 0xff);
		// 写し取ったときに切れていた（0xff）送りは差が取れない。
		// 下駄は 16（CC91=127・CC93=127 のどちらも 16 になる）
		const int v = (cur >= 0xff && was <= 0)
		            ? 16 + nv::send_att(m_rom, now)
		            : cur + nv::send_att(m_rom, now) - nv::send_att(m_rom, was);
		const int w = nv::clamp_att(v);
		return u16(hi ? ((w << 8) | (base & 0xff)) : ((base & 0xff00) | w));
	}

	// LFO のレジスタ。下位が深さで、モジュレーション（CC1）のぶんを足す
	u16 lfo_reg(u16 base, const nv::voice_cal &c, int part) const
	{
		const int now = m_cc[part].mod;
		if (now < 0)
			return base;
		const int d = nv::mod_depth(now) - nv::mod_depth(c.cal_mod);
		return u16((base & 0xff00) | nv::clamp_att(int(base & 0xff) + d));
	}

	// パンのレジスタ（写し取った値からの差ぶんで動かす）
	u16 pan_reg(const nv::voice_cal &c, int part) const
	{
		const int now = m_cc[part].pan, was = c.cal_pan;
		if (now < 0 || now == was)
			return c.reg[0x32];
		const int l = nv::clamp_att((c.reg[0x32] >> 8) + nv::pan_att(now) - nv::pan_att(was));
		const int r = nv::clamp_att((c.reg[0x32] & 0xff) + nv::pan_att(128 - now) - nv::pan_att(128 - was));
		return u16(l << 8 | r);
	}

public:
	// そのパートの音色をもう写し取ってあるか（CC を firmware にどれだけ
	// 見せるかの目安。まだなら 1 音目は firmware が鳴らすので、CC も効かせてもらう）
	bool part_learned(int part) const
	{
		if (!m_rom || part < 0 || part >= PARTS)
			return false;
		if (is_drum(part))
			return !m_drum.empty();
		const u32 rec = record_of(part);
		return rec && m_cal.find(cal_key(rec, part)) != m_cal.end();
	}

	// そのパートは firmware に任せきりか（知らない CC が効いている）。
	// このパートでは写し取りをしても使い道が無いので、やらない
	bool delegated(int part) const
	{ return part >= 0 && part < PARTS && m_cc[part].unknown != 0; }

	// その音を native で鳴らせるか（実際に鳴らす前に決める必要がある。
	// 鳴らせないなら firmware に回すので、遅らせてはいけない）
	bool can_play(int part, int note) const
	{
		if (!m_rom || part < 0 || part >= PARTS)
			return false;
		if (m_cc[part].unknown)              // 知らない CC が効いている間は firmware へ
			return false;
		if (is_drum(part))
			return m_drum.find(drum_key(part, note)) != m_drum.end();
		const u32 rec = record_of(part);
		return rec && m_cal.find(cal_key(rec, part)) != m_cal.end();
	}

	// 鍵を押す。写し取りが無ければ false（呼んだ側が firmware に回す）
	bool note_on(int part, int note, int vel)
	{
		if (is_drum(part))
			return drum_on(part, note, vel);
		const u32 rec = record_of(part);
		if (!rec || !m_rom)
			return false;
		const auto it = m_cal.find(cal_key(rec, part));
		if (it == m_cal.end())
			return false;
		const std::vector<nv::voice_cal> &cals = it->second;

		const int nelem = nv::element_count(m_rom, rec);
		u64 keymask = 0;
		bool any = false;
		u32 taken = 0;                   // もう使った写し取りの印
		int used = 0;
		for (int k = 0; k < nelem; k++) {
			const u8 *el = nv::element(m_rom, rec, k);
			if (!nv::element_active(el, note, vel))
				continue;
			// 波形の番地で、写し取ったスロットと結び付ける
			const u8 *we = nv::wave_entry(m_rom, nv::wave_set(el), nv::wave_note(el, note));
			const nv::voice_cal *c =
			    we ? nv::match_cal(cals, nv::read_wave(we).format_addr, &taken) : nullptr;
			if (!c && size_t(used) < cals.size()) {
				c = &cals[used];
				taken |= u32(1) << used;
			}
			used++;
			const int slot = take_slot(part, note);
			if (slot < 0)
				break;
			if (busy() > m_peak)
				m_peak = busy();
			slot_use &su = m_slot[slot];
			su.elem = el;
			su.wave = we;
			su.cal = c;
			su.tpos = 0;
			su.tstart = m_clock;
			if (c) {
				m_traj = true;
				m_traj_next = 0;       // つぎの tick で見直す
			}
			su.att = nv::volume_att(m_rom, el, c ? c->base_level : 64, note, vel);
			const part_cc &pc = m_cc[part];
			// **ポルタメント**（6.41）。前の鍵（CC84 があればその鍵）の音程で
			// 鳴らし始めて、10ms ごとに寄せていく。残りのずれはセント × 256 で持つ。
			// 追従を掛けるのは、鍵 1 つぶんの音程がその要素の追従で決まるから
			su.glide = 0;
			su.glide_step = 0;
			const int src = pc.porta_src >= 0 ? pc.porta_src : pc.last;
			if (pc.porta_on && src >= 0 && src != note) {
				su.glide_step = nv::porta_step(m_rom, pc.porta_time);
				if (su.glide_step > 0) {
					su.glide = (src - note) * nv::key_follow(el) * 256;
					// firmware の 10ms タイマは世界共通なので、鍵を押した時刻からで
					// なく**格子**に乗せる（同時に鳴る音の滑りがそろう）
					su.glide_next = (m_clock / nv::PORTA_TICK + 1) * nv::PORTA_TICK;
				}
			}
			nv::slot_regs sr = nv::build_note(m_rom, el, note, note_att(su, part), c,
			                                  nv::defaults(),
			                                  nv::bend_cents(pc.bend, pc.range) + su.glide / 256);
			if (c && c->has(0x32))
				sr.set(0x32, pan_reg(*c, part));
			su.lfo = sr.v[0x0a];
			su.cut = sr.v[0x00];
			if (c) {
				sr.set(0x0a, lfo_reg(su.lfo, *c, part));
				sr.set(0x00, cutoff_reg(su.cut, *c, part));
				if (c->has(0x04))
					sr.set(0x04, reso_reg(c->reg[0x04], *c, part));
				if (c->has(0x33))
					sr.set(0x33, send_reg(*c, 0x33, false, pc.rev, c->cal_rev));
				if (c->has(0x34))
					sr.set(0x34, send_reg(*c, 0x34, true, pc.cho, c->cal_cho));
			}
			write_slot(slot, sr);
			if (debug_on())
				std::fprintf(stderr, "note part=%d note=%d vel=%d vol=%d/%d expr=%d/%d pan=%d/%d att=%d->%d\n",
				             part, note, vel, pc.vol, c ? c->cal_vol : -9, pc.expr, c ? c->cal_expr : -9,
				             pc.pan, c ? c->cal_pan : -9, su.att, note_att(su, part));
			// byte72 が 0 でなければ、その要素は遅れて鳴る
			const u32 dly = nv::elem_delay(el);
			if (dly)
				m_pend.push_back({ u64(1) << slot, m_clock + dly });
			else
				keymask |= u64(1) << slot;
			any = true;
		}
		if (any) {
			m_cc[part].last = note;      // つぎの音はここから滑る
			m_cc[part].porta_src = -1;   // CC84 の指定は 1 度で使い切る
		}
		if (!keymask)
			return any;                  // 遅らせた要素だけの音もある
		key_on(keymask);
		return true;
	}

	// **firmware が鳴らした音**も、最後に押した鍵として覚える。
	// これが無いと、写し取りの 1 音目のつぎの音が滑らない
	void note_fw(int part, int note)
	{
		if (part < 0 || part >= PARTS)
			return;
		m_cc[part].last = note;
		m_cc[part].porta_src = -1;
	}

	// 鍵を離す。鳴っていなければ false
	bool note_off(int part, int note)
	{
		bool any = false;
		for (int i = 0; i < SLOTS; i++) {
			slot_use &s = m_slot[i];
			if (!s.on || s.part != part || s.note != note)
				continue;
			if (m_cc[part].damper) {       // ダンパーを踏んでいる間は切らない
				s.held = true;
				any = true;
				continue;
			}
			if (s.elem)
				m_poke(u32(i) * 64 + 9, nv::release_reg(m_rom, s.elem, note, s.att));
			// ドラムは離しでも音を切らない（実機も打ったら鳴りきる）
			s.on = false;
			any = true;
		}
		return any;
	}

	// そのパートの音を全部止める
	// **こちらで鳴らしている音を全部離す**（native の口を切るときに呼ぶ）。
	// 切ったあとは firmware がこのスロットを知らないので、離しておかないと
	// 鳴りっぱなしになる。ぶつ切りではなく離しの速さで鳴り終わらせる
	void silence()
	{
		for (int i = 0; i < SLOTS; i++) {
			slot_use &s = m_slot[i];
			if (!s.on)
				continue;
			if (s.elem && m_rom && m_poke)
				m_poke(u32(i) * 64 + 9, nv::release_reg(m_rom, s.elem, s.note, s.att));
			s.on = false;
			s.held = false;
		}
		m_pend.clear();
		m_traj = false;
		m_traj_next = 0;
	}

	void all_off(int part)
	{
		for (int i = 0; i < SLOTS; i++)
			if (m_slot[i].on && m_slot[i].part == part)
				note_off(part, m_slot[i].note);
	}

	// ドラムの 1 打。写し取った値をそのまま使い、音量だけ強さで動かす
	bool drum_on(int part, int note, int vel)
	{
		const auto it = m_drum.find(drum_key(part, note));
		if (it == m_drum.end() || !m_rom)
			return false;
		u64 keymask = 0;
		for (const nv::voice_cal &c : it->second) {
			const int slot = take_slot(part, note);
			if (slot < 0)
				break;
			// 減衰は足し算なので、強さのぶんだけずらせばよい（6.5）
			const int att0 = c.has(9) ? (c.reg[9] & 0xff) : 0x40;
			slot_use &su = m_slot[slot];
			su.elem = nullptr;               // ドラムは離しの速さを写しの値で済ませる
			su.wave = nullptr;
			su.cal = &c;
			su.tpos = 0;
			su.tstart = m_clock;
			m_traj = true;
			m_traj_next = 0;
			su.att = att0 + 2 * (nv::velocity_att(m_rom, vel) - nv::velocity_att(m_rom, c.cal_vel));
			const int att = note_att(su, part);
			su.lfo = c.has(0x0a) ? c.reg[0x0a] : 0;
			for (int i = 0; i < 0x40; i++)
				if (c.has(i))
					m_poke(u32(slot) * 64 + u32(i),
					       i == 9 ? u16(att)
					              : (i == 0x32 ? pan_reg(c, part)
					              : (i == 0x0a ? lfo_reg(c.reg[0x0a], c, part)
					              : (i == 0x33 ? send_reg(c, 0x33, false, m_cc[part].rev, c.cal_rev)
					              : (i == 0x34 ? send_reg(c, 0x34, true, m_cc[part].cho, c.cal_cho)
					                           : c.reg[i])))));

			su.drum_rel = c.has(9) ? u16(c.reg[9]) : 0;
			if (busy() > m_peak)
				m_peak = busy();
			if (debug_on())
				std::fprintf(stderr, "drum part=%d note=%d vel=%d/%d att=%d->%d 段 %d 写し %016llx%s",
				             part, note, vel, c.cal_vel, att0, att,
				             int(c.filter_env.size()), (unsigned long long)c.mask, "\n");
			keymask |= u64(1) << slot;
		}
		if (!keymask)
			return false;
		key_on(keymask);
		return true;
	}

private:

	// いちばん多いときに、いくつのスロットを使ったか（取り合いを見るため）
	int busy() const
	{
		int n = 0;
		for (const slot_use &s : m_slot)
			if (s.on)
				n++;
		return n;
	}

	slot_use fresh(int part, int note)
	{
		slot_use s;
		s.on = true;
		s.part = part;
		s.note = note;
		s.tstart = m_clock;
		s.age = ++m_age;
		return s;
	}

	// **下の 8 スロットは firmware のために空けておく。**
	// firmware は下から使うので、写し取りの 1 音目とぶつからない。
	// dense（16 パート・60 音）でもこちらが使うのは 36 までなので足りる
	static constexpr int FW_SLOTS = 8;

	// 空きスロットを取る。無ければ一番古い声を止めて使う。
	// **上から**取る（firmware は下から使うため）
	int take_slot(int part, int note)
	{
		// **firmware が最近触ったスロットは避ける**。下 8 個を空けるだけでは
		// 足りなかった（声が増えると firmware は上の方も使う）。
		// 窓やプラグインのように MIDI がブロック単位で届くと、同時に鳴る音が
		// 増えて firmware が上まで伸びる。避けないと、firmware が自分の音の
		// 続きを書いたときにこちらの音の包絡線が書き替わって壊れる
		for (int pass = 0; pass < 2; pass++) {
			const bool avoid = pass == 0;
			int oldest = -1;
			u64 oldest_age = ~u64(0);
			for (int n2 = 0; n2 < SLOTS - FW_SLOTS; n2++) {
				const int i = SLOTS - 1 - n2;
				if (avoid && fw_recent(i))
					continue;
				if (!m_slot[i].on) {
					m_slot[i] = fresh(part, note);
					return i;
				}
				if (m_slot[i].age < oldest_age) {
					oldest_age = m_slot[i].age;
					oldest = i;
				}
			}
			// 避けた結果どこも空いていなければ、2 周目で避けずに探す
			// （音が出ないより、稀にぶつかる方がまし）
			if (oldest >= 0) {
				m_slot[oldest] = fresh(part, note);
				return oldest;
			}
		}
		return -1;
	}

	void write_slot(int slot, const nv::slot_regs &r)
	{
		for (int i = 0; i < 0x40; i++)
			if (r.write & (u64(1) << i))
				m_poke(u32(slot) * 64 + u32(i), r.v[i]);
	}

	void key_on(u64 mask)
	{
		static const u32 MASK_REG[4] = { 0x1cf, 0x1ce, 0x18f, 0x18e };
		for (int i = 0; i < 4; i++)
			m_poke(MASK_REG[i], u16((mask >> (i * 16)) & 0xffff));
		m_poke(0x20e, 1);
	}

	poke_fn m_poke;
	const u8 *m_rom = nullptr;
	const u8 *m_ram = nullptr;
	std::unordered_map<u64, std::vector<nv::voice_cal>> m_cal;
	std::unordered_map<u64, std::vector<nv::voice_cal>> m_drum;
	std::array<slot_use, SLOTS> m_slot;
	std::array<part_cc, PARTS> m_cc;
	// 前に sync_cc() で見たワーク RAM の値。ここから動いていれば
	// firmware が書き替えたということ
	struct ram_seen { u8 vol = 0, expr = 0, pan = 0, mod = 0, rev = 0, cho = 0, bri = 0, res = 0; };
	std::array<ram_seen, PARTS> m_seen{};
	// 自分で引いた音色（0 なら引けていない）と、ドラムかどうか（-1 なら分からない）
	// firmware がそのスロットに最後に書いた時刻（+1。0 は触っていない）
	u64 m_fw_touch[SLOTS] = {};
	static constexpr u64 FW_KEEP = 44100 * 2;   // 2 秒は firmware のものとみなす
	std::array<u32, PARTS> m_recsel{};
	std::array<s8, PARTS> m_recsel_drum{};
	u64 m_clock = 0;
	int m_peak = 0;
	bool m_traj = false;
	bool m_rec = false;            // 写し取りの最中（段が後から増える）
	u64 m_traj_next = 0;           // つぎに段を書く時刻
	// 遅らせて鳴らす要素（byte72）。時が来たら key_on する
	struct pending_key { u64 mask; u64 at; };
	std::vector<pending_key> m_pend;
	u64 m_age = 0;
};

} // namespace xg

#endif // S_MU2000_XG_NATIVE_DRIVER_H
