// license:BSD-3-Clause

#include "automation.h"

#include "ui/eq_curve.h"
#include "ui/xg_state.h"
#include "xg/fx_params.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <unordered_map>

namespace smu2000 {
namespace automation {

namespace {

// パートの値。**並びは番号なので動かさない**（足すなら後ろへ。32 まで）
struct key_cc { const char *key; const char *name; int cc; };
constexpr key_cc PART_KEYS[] = {
	{ "part.volume",         "Volume",       7 },
	{ "part.pan",            "Pan",         10 },
	{ "part.reverb_send",    "Reverb Send", 91 },
	{ "part.chorus_send",    "Chorus Send", 93 },
	{ "part.variation_send", "Var Send",    94 },
	{ "part.cutoff",         "Cutoff",      74 },
	{ "part.resonance",      "Resonance",   71 },
	{ "part.attack",         "Attack",      73 },
	{ "part.decay",          "Decay",       75 },
	{ "part.release",        "Release",     72 },
	{ "part.vib_rate",       "Vib Rate",    76 },
	{ "part.vib_depth",      "Vib Depth",   77 },
	{ "part.vib_delay",      "Vib Delay",   78 },
	{ "part.eq_bass_gain",   "EQ Bass Gain",   -1 },
	{ "part.eq_bass_freq",   "EQ Bass Freq",   -1 },
	{ "part.eq_treble_gain", "EQ Treble Gain", -1 },
	{ "part.eq_treble_freq", "EQ Treble Freq", -1 },
	{ "part.dry_level",      "Dry Level",   -1 },
	{ "part.note_shift",     "Note Shift",  -1 },
};

// マスターの値。**並びは番号なので動かさない**
constexpr key_cc MASTER_KEYS[] = {
	{ "system.master_volume", "Master Volume",   -1 },
	{ "system.master_tune",   "Master Tune",     -1 },
	{ "system.transpose",     "Transpose",       -1 },
	{ "reverb.return",        "Reverb Return",   -1 },
	{ "reverb.pan",           "Reverb Pan",      -1 },
	{ "chorus.return",        "Chorus Return",   -1 },
	{ "chorus.pan",           "Chorus Pan",      -1 },
	{ "chorus.to_reverb",     "Chorus to Reverb", -1 },
	{ "variation.return",     "Var Return",      -1 },
	{ "variation.pan",        "Var Pan",         -1 },
	{ "variation.to_reverb",  "Var to Reverb",   -1 },
	{ "variation.to_chorus",  "Var to Chorus",   -1 },
	{ "master_eq.gain1",      "Master EQ Gain 1", -1 },
	{ "master_eq.gain2",      "Master EQ Gain 2", -1 },
	{ "master_eq.gain3",      "Master EQ Gain 3", -1 },
	{ "master_eq.gain4",      "Master EQ Gain 4", -1 },
	{ "master_eq.gain5",      "Master EQ Gain 5", -1 },
	{ "master_eq.freq1",      "Master EQ Freq 1", -1 },
	{ "master_eq.freq2",      "Master EQ Freq 2", -1 },
	{ "master_eq.freq3",      "Master EQ Freq 3", -1 },
	{ "master_eq.freq4",      "Master EQ Freq 4", -1 },
	{ "master_eq.freq5",      "Master EQ Freq 5", -1 },
	{ "master_eq.q1",         "Master EQ Q 1",   -1 },
	{ "master_eq.q2",         "Master EQ Q 2",   -1 },
	{ "master_eq.q3",         "Master EQ Q 3",   -1 },
	{ "master_eq.q4",         "Master EQ Q 4",   -1 },
	{ "master_eq.q5",         "Master EQ Q 5",   -1 },
};

std::string part_label(int part)
{
	char buf[8];
	std::snprintf(buf, sizeof(buf), "%c%d", char('A' + part / 16), part % 16 + 1);
	return buf;
}

struct table {
	std::vector<entry> list;
	std::unordered_map<uint32_t, int> by_id;
	std::unordered_map<uint64_t, int> by_param;    // 定義表の位置 << 8 | パート

	table()
	{
		const std::vector<xg::param> &defs = xg::params();
		auto add = [&](uint32_t id, const key_cc &k, int part, bool is_part) {
			const xg::param *p = xg::find(k.key);
			if (!p)
				return;               // 定義表から消えた（起きないはず）。番号は空けたまま
			entry e;
			e.id = id;
			e.p = p;
			e.part = part;
			e.is_part = is_part;
			e.cc = k.cc;
			e.group = is_part ? "Part " + part_label(part) : std::string("Master");
			e.name = is_part ? part_label(part) + " " + k.name : std::string(k.name);
			const int index = int(list.size());
			by_id[id] = index;
			by_param[uint64_t(p - defs.data()) << 8 | uint64_t(part)] = index;
			list.push_back(std::move(e));
		};
		for (int part = 0; part < 64; part++)
			for (size_t k = 0; k < sizeof(PART_KEYS) / sizeof(PART_KEYS[0]); k++)
				add(PART_BASE + uint32_t(part) * PART_STRIDE + uint32_t(k), PART_KEYS[k], part, true);
		for (size_t k = 0; k < sizeof(MASTER_KEYS) / sizeof(MASTER_KEYS[0]); k++)
			add(MASTER_BASE + uint32_t(k), MASTER_KEYS[k], 0, false);
		// インサーション 1-4 のパラメータ 1-16
		for (int b = 0; b < 4; b++)
			for (int n = 1; n <= 16; n++) {
				entry e;
				e.id = INS_BASE + uint32_t(b) * 16 + uint32_t(n - 1);
				e.p = nullptr;
				e.part = 0;
				e.is_part = false;
				e.cc = -1;
				e.k = kind::insertion;
				e.block = b;
				e.number = n;
				char buf[32];
				std::snprintf(buf, sizeof(buf), "INS%d Param %d", b + 1, n);
				e.name = buf;
				std::snprintf(buf, sizeof(buf), "Insertion %d", b + 1);
				e.group = buf;
				by_id[e.id] = int(list.size());
				list.push_back(std::move(e));
			}
	}
};

const table &the_table()
{
	static const table t;
	return t;
}

bool starts(const char *key, const char *prefix)
{
	return !std::strncmp(key, prefix, std::strlen(prefix));
}

// ---- インサーションのパラメータ
//
// 番地（03 0n の後ろ）とパラメータの番号。1-10 は 1 バイトの種類なら 02-0B、2 バイトの種類なら 30-43（2 つおき）、
// 11-16 は 20-25（xg/fx_params.h）
int number_of(int addr)
{
	if (addr >= 0x02 && addr <= 0x0b) return addr - 0x02 + 1;
	if (addr >= 0x30 && addr <= 0x43 && ((addr - 0x30) & 1) == 0) return (addr - 0x30) / 2 + 1;
	if (addr >= 0x20 && addr <= 0x25) return addr - 0x20 + 11;
	return 0;
}

// そのときの種類での、このパラメータの定義。種類が持たなければ nullptr
const xg::fx_param *ins_param(const entry &e, const ui::xg_snapshot &ram)
{
	const u8 *blk = ram.effect + (xg::ram::INS_BLOCK[e.block] - xg::ram::EFFECT);
	const int type = (blk[0] & 0x7f) << 7 | (blk[1] & 0x7f);
	const xg::fx_def *d = xg::fx_find(type);
	if (!d)
		return nullptr;
	for (int i = 0; i < d->count; i++)
		if (number_of(d->params[i].addr) == e.number)
			return &d->params[i];
	return nullptr;
}

// 写しの中の firmware の値
bool ins_raw(const entry &e, const xg::fx_param &fp, const ui::xg_snapshot &ram, int &raw)
{
	const u8 *blk = ram.effect + (xg::ram::INS_BLOCK[e.block] - xg::ram::EFFECT);
	if (fp.addr >= 0x30) {
		const u8 *w = blk + xg::ram::INS_WIDE + (fp.addr - 0x30);
		raw = w[0] << 8 | w[1];
		return true;
	}
	const u8 *b = ui::locate_byte(ram, 0x03, u8(e.block), fp.addr);
	if (!b)
		return false;
	raw = *b & 0x7f;
	return true;
}

int to_raw(const xg::fx_param &fp, int value)
{
	return std::clamp(int(fp.lo) + int(std::lround(double(value) * double(fp.hi - fp.lo) / FX_SCALE)), int(fp.lo), int(fp.hi));
}

int from_raw_value(const xg::fx_param &fp, int raw)
{
	if (fp.hi <= fp.lo)
		return 0;
	return std::clamp(int(std::lround(double(raw - fp.lo) * FX_SCALE / double(fp.hi - fp.lo))), 0, FX_SCALE);
}

// インサーションの設定の窓と同じ書式（src/ui/fx_editor.cpp の value_text）
std::string fx_value_text(const xg::fx_param &p, int v)
{
	char buf[24];
	switch (p.fmt) {
	case xg::fx_fmt::table:
		if (p.texts && v >= p.lo && v <= p.hi)
			return p.texts[v - p.lo];
		break;
	case xg::fx_fmt::tenths:
		std::snprintf(buf, sizeof(buf), "%.1f", v / 10.0);
		return buf;
	default:
		break;
	}
	std::snprintf(buf, sizeof(buf), "%d", v);
	return buf;
}

} // namespace


const std::vector<entry> &entries() { return the_table().list; }

int index_of(uint32_t id)
{
	const table &t = the_table();
	const auto it = t.by_id.find(id);
	return it == t.by_id.end() ? -1 : it->second;
}

int index_of(const xg::param &p, int part)
{
	const table &t = the_table();
	const std::vector<xg::param> &defs = xg::params();
	if (&p < defs.data() || &p >= defs.data() + defs.size())
		return -1;
	const auto it = t.by_param.find(uint64_t(&p - defs.data()) << 8 | uint64_t(p.where == xg::area::part ? part : 0));
	return it == t.by_param.end() ? -1 : it->second;
}

int index_of_raw(uint32_t addr)
{
	const int hi_ = int(addr >> 14), mid = int((addr >> 7) & 0x7f), lo_ = int(addr & 0x7f);
	if (hi_ != 0x03 || mid > 3)
		return -1;
	const int n = number_of(lo_);
	return n ? index_of(INS_BASE + uint32_t(mid) * 16 + uint32_t(n - 1)) : -1;
}

bool from_raw(const entry &e, const ui::xg_snapshot &ram, int raw, int &value)
{
	if (e.k != kind::insertion)
		return false;
	const xg::fx_param *fp = ins_param(e, ram);
	if (!fp)
		return false;
	value = from_raw_value(*fp, raw);
	return true;
}

int to_value(const entry &e, double normalized)
{
	return clamp_value(e, lo(e) + std::clamp(normalized, 0.0, 1.0) * double(steps(e)));
}

int clamp_value(const entry &e, double plain)
{
	return std::clamp(int(std::lround(plain)), lo(e), hi(e));
}

std::string text(const entry &e, int value, const ui::xg_snapshot *ram)
{
	char buf[32];
	if (e.k == kind::insertion) {
		const xg::fx_param *fp = ram ? ins_param(e, *ram) : nullptr;
		if (!fp) {
			std::snprintf(buf, sizeof(buf), "%.1f%%", value / 10.0);
			return buf;
		}
		return std::string(fp->label) + " " + fx_value_text(*fp, to_raw(*fp, value));
	}
	const xg::param &p = *e.p;
	if (std::strstr(p.key, "eq") && std::strstr(p.key, "freq"))
		return ui::eq::hz_text(value) + " Hz";
	if (starts(p.key, "master_eq.q")) {
		std::snprintf(buf, sizeof(buf), "%.1f", value / 10.0);
		return buf;
	}
	if (std::strstr(p.key, "eq") && std::strstr(p.key, "gain")) {
		std::snprintf(buf, sizeof(buf), "%+d dB", value - p.center);
		return buf;
	}
	if (!std::strcmp(p.key, "system.master_tune")) {
		std::snprintf(buf, sizeof(buf), "%+.1f cent", (value - p.center) / 10.0);
		return buf;
	}
	if (!std::strcmp(p.key, "system.transpose") || !std::strcmp(p.key, "part.note_shift")) {
		std::snprintf(buf, sizeof(buf), "%+d semi", value - p.center);
		return buf;
	}
	return xg::format(p, value);
}

bool parse(const entry &e, const char *s, int &value, const ui::xg_snapshot *ram)
{
	if (!s)
		return false;
	while (*s == ' ')
		s++;
	if (e.k == kind::insertion) {
		const xg::fx_param *fp = ram ? ins_param(e, *ram) : nullptr;
		if (!fp) {
			char *end = nullptr;
			const double v = std::strtod(s, &end);
			if (end == s)
				return false;
			value = clamp_value(e, v * 10.0);        // 割合（%）
			return true;
		}
		// 名前が付いていれば飛ばす（"Drive 40" の形で戻ってくる）
		const size_t label = std::strlen(fp->label);
		if (!std::strncmp(s, fp->label, label))
			s += label;
		while (*s == ' ')
			s++;
		if (fp->fmt == xg::fx_fmt::table && fp->texts) {
			for (int v = fp->lo; v <= fp->hi; v++)
				if (!std::strcmp(s, fp->texts[v - fp->lo])) {
					value = from_raw_value(*fp, v);
					return true;
				}
		}
		char *end = nullptr;
		const double v = std::strtod(s, &end);
		if (end == s)
			return false;
		const int raw = std::clamp(int(std::lround(fp->fmt == xg::fx_fmt::tenths ? v * 10.0 : v)), int(fp->lo), int(fp->hi));
		value = from_raw_value(*fp, raw);
		return true;
	}
	const xg::param &p = *e.p;
	// パンは L/R/C/Rnd も読む
	if (p.how == xg::view::pan) {
		if (*s == 'C' || *s == 'c') { value = 64; return true; }
		if (!std::strncmp(s, "Rnd", 3) || !std::strncmp(s, "rnd", 3)) { value = 0; return true; }
		if (*s == 'L' || *s == 'l') { value = clamp_value(e, 64 - std::atof(s + 1)); return true; }
		if (*s == 'R' || *s == 'r') { value = clamp_value(e, 64 + std::atof(s + 1)); return true; }
	}
	char *end = nullptr;
	const double v = std::strtod(s, &end);
	if (end == s)
		return false;
	if (std::strstr(p.key, "eq") && std::strstr(p.key, "freq")) {
		// Hz（k が付けば 1000 倍）で打たれたら、一番近い表の番号
		double hz = v;
		if (end && (*end == 'k' || *end == 'K'))
			hz *= 1000.0;
		int best = p.min;
		for (int i = p.min; i <= p.max; i++)
			if (std::fabs(double(ui::eq::HZ[i]) - hz) < std::fabs(double(ui::eq::HZ[best]) - hz))
				best = i;
		value = best;
		return true;
	}
	if (starts(p.key, "master_eq.q")) { value = clamp_value(e, v * 10.0); return true; }
	if (!std::strcmp(p.key, "system.master_tune")) { value = clamp_value(e, p.center + v * 10.0); return true; }
	if (p.how == xg::view::center) { value = clamp_value(e, p.center + v); return true; }
	if (p.how == xg::view::plus1) { value = clamp_value(e, v - 1.0); return true; }
	value = clamp_value(e, v);
	return true;
}

bool current(const entry &e, const ui::xg_snapshot &ram, int &value)
{
	if (e.k == kind::insertion) {
		const xg::fx_param *fp = ins_param(e, ram);
		int raw = 0;
		if (!fp || !ins_raw(e, *fp, ram, raw))
			return false;
		value = from_raw_value(*fp, raw);
		return true;
	}
	return ui::read_value(*e.p, e.part, ram, value);
}

int midi(const entry &e, int value, const ui::xg_snapshot *ram, uint8_t *out, int &port)
{
	port = 0;
	if (e.k == kind::insertion) {
		// そのときの種類の範囲に当てはめて、パラメータチェンジ 03 0n xx
		const xg::fx_param *fp = ram ? ins_param(e, *ram) : nullptr;
		if (!fp)
			return 0;
		const int raw = to_raw(*fp, std::clamp(value, 0, FX_SCALE));
		int n = 0;
		out[n++] = 0xf0;
		out[n++] = 0x43;
		out[n++] = 0x10;
		out[n++] = 0x4c;
		out[n++] = 0x03;
		out[n++] = uint8_t(e.block);
		out[n++] = fp->addr;
		for (int i = 0; i < fp->size && i < 4; i++)
			out[n++] = uint8_t((raw >> ((fp->size - 1 - i) * 7)) & 0x7f);
		out[n++] = 0xf7;
		return n;
	}
	const xg::param &p = *e.p;
	value = std::clamp(value, p.min, p.max);

	// CC で入れられるか。受信チャンネルがこのパートだけのもので、値が CC で作れる範囲のとき
	if (ram && e.is_part && e.cc >= 0) {
		const int rcv = ram->parts[e.part][0x04];
		bool own = rcv <= 63;
		for (int i = 0; own && i < ui::XG_PARTS; i++)
			if (i != e.part && ram->parts[i][0x04] == rcv)
				own = false;
		if (e.cc == 10 && value == 0)
			own = false;                          // パンのランダムは CC では作れない
		if (e.cc == 94) {
			// バリエーションの送りは、接続が SYSTEM（1）のときだけ CC94 が効く
			int connect = 0;
			const xg::param *pc = xg::find("variation.connect");
			if (!pc || !ui::read_value(*pc, 0, *ram, connect) || connect != 1)
				own = false;
		}
		if (own) {
			port = rcv / 16;
			out[0] = uint8_t(0xb0 | (rcv & 15));
			out[1] = uint8_t(e.cc);
			out[2] = uint8_t(value & 0x7f);
			return 3;
		}
	}

	// パラメータチェンジ。F0 43 10 4C 番地 中身 F7
	int n = 0;
	out[n++] = 0xf0;
	out[n++] = 0x43;
	out[n++] = 0x10;
	out[n++] = 0x4c;
	out[n++] = p.hi;
	out[n++] = p.where == xg::area::part ? uint8_t(e.part) : p.mid;
	out[n++] = p.lo;
	for (int i = 0; i < p.size && i < 4; i++) {
		const int shift = (p.size - 1 - i) * (p.enc == xg::coding::nibble ? 4 : 7);
		out[n++] = uint8_t((value >> shift) & (p.enc == xg::coding::nibble ? 0x0f : 0x7f));
	}
	out[n++] = 0xf7;
	return n;
}

} // namespace automation
} // namespace smu2000
