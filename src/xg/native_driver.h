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
		u16 drum_rel = 0;
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
		m_clock = 0;
		m_traj = false;
		m_pend.clear();
		m_age = 0;
	}

	// その音色の写し取りがもう有るか
	bool calibrated(u32 rec) const { return m_cal.find(rec) != m_cal.end(); }

	// firmware に鳴らさせた 1 音から写し取る
	void learn(u32 rec, std::vector<nv::voice_cal> cals)
	{
		if (!cals.empty() && m_cal.find(rec) == m_cal.end())
			m_cal[rec] = std::move(cals);
	}

	// ドラムは音ごとに中身が違うので、**鍵ごと**に覚える。
	// 同じ音を何度も叩くので、これだけで打楽器のほとんどが native になる
	void learn_drum(u64 key, std::vector<nv::voice_cal> cals)
	{
		if (!cals.empty() && m_drum.find(key) == m_drum.end())
			m_drum[key] = std::move(cals);
	}

	// ドラムのパートか。XG の「パートモード」（08 pp 07。0 が普通、1 以上がドラム）を見る。
	// 「記録が引けない＝ドラム」では、音色を選び終える前の旋律パートまで拾ってしまう
	bool is_drum(int part) const
	{
		if (!m_ram || part < 0 || part >= PARTS)
			return false;
		return m_ram[ram::part_base(part) + 0x07] != 0;
	}

	// 写し取ったものを、あとから直せるように渡す（フィルタの包絡線の追記用）
	std::vector<nv::voice_cal> *cals_of(u32 rec)
	{
		const auto it = m_cal.find(rec);
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
		int live = 0;
		for (int i = 0; i < SLOTS; i++) {
			slot_use &s = m_slot[i];
			// 写し取りの途中で段が増えることがあるので、まだ段が無くても数える
			if (!s.on || !s.cal)
				continue;
			live++;
			if (s.tpos >= s.cal->filter_env.size())
				continue;
			const std::vector<nv::fstep> &fe = s.cal->filter_env;
			while (s.tpos < fe.size() && s.tstart + fe[s.tpos].at <= clock) {
				m_poke(u32(i) * 64 + fe[s.tpos].reg, fe[s.tpos].v);
				s.tpos++;
			}
		}
		m_traj = live > 0;
	}

	// ドラムの覚え先の鍵（バンクとプログラムと音の高さ）
	u64 drum_key(int part, int note) const
	{
		if (!m_ram)
			return 0;
		const u8 *p = m_ram + ram::part_base(part);
		return u64(p[1]) << 24 | u64(p[2]) << 16 | u64(p[3]) << 8 | u64(note & 0x7f);
	}
	bool drum_known(int part, int note) const
	{
		return m_drum.find(drum_key(part, note)) != m_drum.end();
	}

	// パートの音色の記録を、ワーク RAM から読む（firmware が入れた値）
	u32 record_of(int part) const
	{
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
		int bend = 8192, range = 2;            // ピッチベンドと、その幅（半音）
		bool damper = false;                   // CC64
	};

	// firmware を回したあとに、パートの音量・表現・パンをワーク RAM から取り直す。
	// SysEx やパネルで変えられた場合も、これで追い付く
	void sync_cc()
	{
		if (!m_ram)
			return;
		for (int p = 0; p < PARTS; p++) {
			const u8 *b = m_ram + ram::part_base(p);
			m_cc[p].vol  = b[0x0b];
			m_cc[p].expr = b[ram::PART_EXP];
			m_cc[p].pan  = b[0x0e];
		}
	}

	// 写し取ったときのつまみの位置（ワーク RAM から）
	int part_vol(int part) const  { return m_ram ? int(m_ram[ram::part_base(part) + 0x0b]) : 100; }
	int part_expr(int part) const { return m_ram ? int(m_ram[ram::part_base(part) + ram::PART_EXP]) : 127; }
	int part_pan(int part) const  { return m_ram ? int(m_ram[ram::part_base(part) + 0x0e]) : 64; }

	// その CC を native でさばけるか（実際にさばく前に決める）
	static bool handles_cc(int cc)
	{ return cc == 0x07 || cc == 0x0b || cc == 0x0a || cc == 0x40; }

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
		case 0x40:                             // ダンパー
			p.damper = value >= 64;
			if (!p.damper)
				release_held(part);
			return true;
		case 0x78: case 0x7b:                  // 音を全部切る
			all_off(part);
			return false;
		default:
			return false;                      // 知らない CC は firmware に任せる
		}
		apply_cc(part);
		return true;
	}

	void bend(int part, int value14)
	{
		if (part < 0 || part >= PARTS)
			return;
		m_cc[part].bend = value14;
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
		}
	}

	void apply_bend(int part)
	{
		for (int i = 0; i < SLOTS; i++) {
			slot_use &s = m_slot[i];
			if (!s.on || s.part != part || !s.elem || !s.wave)
				continue;
			m_poke(u32(i) * 64 + 0x11,
			       nv::pitch_reg(nv::read_wave(s.wave), s.note, nv::key_follow(s.elem),
			                     nv::bend_cents(m_cc[part].bend, m_cc[part].range)
			                     + nv::elem_tune(s.elem)));
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
		if (c) {
			if (p.vol >= 0)
				a += nv::cc_vol_att(m_rom, p.vol) - nv::cc_vol_att(m_rom, c->cal_vol);
			if (p.expr >= 0)
				a += nv::cc_vol_att(m_rom, p.expr) - nv::cc_vol_att(m_rom, c->cal_expr);
		}
		return nv::clamp_att(a);
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
	// その音を native で鳴らせるか（実際に鳴らす前に決める必要がある。
	// 鳴らせないなら firmware に回すので、遅らせてはいけない）
	bool can_play(int part, int note) const
	{
		if (!m_rom || part < 0 || part >= PARTS)
			return false;
		if (is_drum(part))
			return m_drum.find(drum_key(part, note)) != m_drum.end();
		const u32 rec = record_of(part);
		return rec && m_cal.find(rec) != m_cal.end();
	}

	// 鍵を押す。写し取りが無ければ false（呼んだ側が firmware に回す）
	bool note_on(int part, int note, int vel)
	{
		if (is_drum(part))
			return drum_on(part, note, vel);
		const u32 rec = record_of(part);
		if (!rec || !m_rom)
			return false;
		const auto it = m_cal.find(rec);
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
			const u8 *we = nv::wave_entry(m_rom, nv::wave_set(el), note);
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
			slot_use &su = m_slot[slot];
			su.elem = el;
			su.wave = we;
			su.cal = c;
			su.tpos = 0;
			su.tstart = m_clock;
			if (c)
				m_traj = true;
			su.att = nv::volume_att(m_rom, el, c ? c->base_level : 64, note, vel);
			const part_cc &pc = m_cc[part];
			nv::slot_regs sr = nv::build_note(m_rom, el, note, note_att(su, part), c,
			                                  nv::defaults(), nv::bend_cents(pc.bend, pc.range));
			if (c && c->has(0x32))
				sr.set(0x32, pan_reg(*c, part));
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
		if (!keymask)
			return any;                  // 遅らせた要素だけの音もある
		key_on(keymask);
		return true;
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
			su.att = att0 + 2 * (nv::velocity_att(m_rom, vel) - nv::velocity_att(m_rom, c.cal_vel));
			const int att = note_att(su, part);
			for (int i = 0; i < 0x40; i++)
				if (c.has(i))
					m_poke(u32(slot) * 64 + u32(i),
					       i == 9 ? u16(att) : (i == 0x32 ? pan_reg(c, part) : c.reg[i]));

			su.drum_rel = c.has(9) ? u16(c.reg[9]) : 0;
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

	// 空きスロットを取る。無ければ一番古い声を止めて使う。
	// **上から**取る。firmware は下から使うので、写し取りのために firmware が
	// 鳴らしている音とぶつかりにくい
	int take_slot(int part, int note)
	{
		int oldest = -1;
		u64 oldest_age = ~u64(0);
		for (int n2 = 0; n2 < SLOTS; n2++) {
			const int i = SLOTS - 1 - n2;
			if (!m_slot[i].on) {
				m_slot[i] = fresh(part, note);
				return i;
			}
			if (m_slot[i].age < oldest_age) {
				oldest_age = m_slot[i].age;
				oldest = i;
			}
		}
		if (oldest < 0)
			return -1;
		m_slot[oldest] = fresh(part, note);
		return oldest;
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
	std::unordered_map<u32, std::vector<nv::voice_cal>> m_cal;
	std::unordered_map<u64, std::vector<nv::voice_cal>> m_drum;
	std::array<slot_use, SLOTS> m_slot;
	std::array<part_cc, PARTS> m_cc;
	u64 m_clock = 0;
	bool m_traj = false;
	// 遅らせて鳴らす要素（byte72）。時が来たら key_on する
	struct pending_key { u64 mask; u64 at; };
	std::vector<pending_key> m_pend;
	u64 m_age = 0;
};

} // namespace xg

#endif // S_MU2000_XG_NATIVE_DRIVER_H
