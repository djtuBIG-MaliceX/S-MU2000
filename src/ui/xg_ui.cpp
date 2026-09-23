// license:BSD-3-Clause

#include "xg_ui.h"

#include "eq_curve.h"
#include "fx_help.h"
#include "fx_icons.h"

#include "imgui.h"
#include "imgui_internal.h"   // SetKeyOwner（棒が矢印キーをもらう）

#include <algorithm>
#include <cstdarg>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

#include "compat/paths.h"

namespace ui {
namespace xgui {

namespace {

// General MIDI の楽器名（規格の名前）。XG のバンク 0 はこの並び
const char *const GM_NAMES[128] = {
	"Acoustic Grand Piano", "Bright Acoustic Piano", "Electric Grand Piano", "Honky-tonk Piano",
	"Electric Piano 1", "Electric Piano 2", "Harpsichord", "Clavi",
	"Celesta", "Glockenspiel", "Music Box", "Vibraphone", "Marimba", "Xylophone", "Tubular Bells", "Dulcimer",
	"Drawbar Organ", "Percussive Organ", "Rock Organ", "Church Organ", "Reed Organ", "Accordion", "Harmonica", "Tango Accordion",
	"Acoustic Guitar (nylon)", "Acoustic Guitar (steel)", "Electric Guitar (jazz)", "Electric Guitar (clean)",
	"Electric Guitar (muted)", "Overdriven Guitar", "Distortion Guitar", "Guitar Harmonics",
	"Acoustic Bass", "Electric Bass (finger)", "Electric Bass (pick)", "Fretless Bass",
	"Slap Bass 1", "Slap Bass 2", "Synth Bass 1", "Synth Bass 2",
	"Violin", "Viola", "Cello", "Contrabass", "Tremolo Strings", "Pizzicato Strings", "Orchestral Harp", "Timpani",
	"String Ensemble 1", "String Ensemble 2", "Synth Strings 1", "Synth Strings 2",
	"Choir Aahs", "Voice Oohs", "Synth Voice", "Orchestra Hit",
	"Trumpet", "Trombone", "Tuba", "Muted Trumpet", "French Horn", "Brass Section", "Synth Brass 1", "Synth Brass 2",
	"Soprano Sax", "Alto Sax", "Tenor Sax", "Baritone Sax", "Oboe", "English Horn", "Bassoon", "Clarinet",
	"Piccolo", "Flute", "Recorder", "Pan Flute", "Blown Bottle", "Shakuhachi", "Whistle", "Ocarina",
	"Lead 1 (square)", "Lead 2 (sawtooth)", "Lead 3 (calliope)", "Lead 4 (chiff)",
	"Lead 5 (charang)", "Lead 6 (voice)", "Lead 7 (fifths)", "Lead 8 (bass + lead)",
	"Pad 1 (new age)", "Pad 2 (warm)", "Pad 3 (polysynth)", "Pad 4 (choir)",
	"Pad 5 (bowed)", "Pad 6 (metallic)", "Pad 7 (halo)", "Pad 8 (sweep)",
	"FX 1 (rain)", "FX 2 (soundtrack)", "FX 3 (crystal)", "FX 4 (atmosphere)",
	"FX 5 (brightness)", "FX 6 (goblins)", "FX 7 (echoes)", "FX 8 (sci-fi)",
	"Sitar", "Banjo", "Shamisen", "Koto", "Kalimba", "Bag pipe", "Fiddle", "Shanai",
	"Tinkle Bell", "Agogo", "Steel Drums", "Woodblock", "Taiko Drum", "Melodic Tom", "Synth Drum", "Reverse Cymbal",
	"Guitar Fret Noise", "Breath Noise", "Seashore", "Bird Tweet", "Telephone Ring", "Helicopter", "Applause", "Gunshot",
};

const char *const GM_GROUPS[16] = {
	"Piano", "Chromatic Percussion", "Organ", "Guitar", "Bass", "Strings", "Ensemble", "Brass",
	"Reed", "Pipe", "Synth Lead", "Synth Pad", "Synth Effects", "Ethnic", "Percussive", "Sound Effects",
};

} // namespace


namespace {
std::unique_ptr<xg::voice_rom> g_voices;
}

void set_voice_rom(std::shared_ptr<const std::vector<u8>> rom)
{
	if (rom)
		xg::set_fx_type_rom(*rom);                // エフェクトの種類の表も同じ ROM から
	g_voices = std::make_unique<xg::voice_rom>(std::move(rom));
	if (!g_voices->ok())
		g_voices.reset();                  // 版が違う。GM の名前で出す
}

const xg::voice_rom *voices() { return g_voices.get(); }

namespace {
const xg_snapshot *g_current_ram = nullptr;
}

namespace {
bool g_hint_bar = false;
std::string g_hint;
std::vector<std::string> g_values;               // begin_values から集めている点の字
bool g_collect = false;
}

void begin_hint_bar() { g_hint_bar = true; g_hint.clear(); }
void end_hint_bar() { g_hint_bar = false; }
bool hint_bar() { return g_hint_bar; }
const std::string &hint_text() { return g_hint; }

void begin_values()
{
	g_values.clear();
	g_collect = true;
}

std::vector<std::string> end_values()
{
	g_collect = false;
	return std::move(g_values);
}

void shape_value(const char *text)
{
	if (!g_collect)
		return;
	std::string line;
	for (const char *c = text;; c++) {          // 2 行の字は 2 行に分ける
		if (*c == '\n' || !*c) {
			if (!line.empty())
				g_values.push_back(line);
			line.clear();
			if (!*c)
				break;
		} else {
			line += *c;
		}
	}
}

void hint(const char *fmt, ...)
{
	char buf[1024];
	va_list ap;
	va_start(ap, fmt);
	std::vsnprintf(buf, sizeof(buf), fmt, ap);
	va_end(ap);
	if (g_hint_bar)
		g_hint = buf;
	else
		ImGui::SetItemTooltip("%s", buf);
}

// ---- マウスで動かしている間の送信の間引き
namespace {
struct drag_msg { std::string key; std::vector<u8> bytes; };
std::vector<drag_msg> g_drag;                       // 送っていない分（番地ごとに最新だけ）
std::chrono::steady_clock::time_point g_drag_sent;
constexpr auto DRAG_EVERY = std::chrono::milliseconds(60);   // 画面は 30 コマ／秒。2 コマに 1 回くらい

// 同じ行き先かを見分ける印。パラメータチェンジは番地まで、CC は番号まで
std::string drag_key(const std::vector<u8> &b)
{
	size_t n = b.size();
	if (!b.empty() && b[0] == 0xf0)
		n = std::min<size_t>(n, 7);             // F0 43 1n 4C 上 中 下
	else if (!b.empty() && (b[0] & 0xf0) == 0xb0)
		n = std::min<size_t>(n, 2);             // Bn 番号
	return std::string(b.begin(), b.begin() + std::ptrdiff_t(n));
}

void drag_send_now(bridge &br)
{
	for (drag_msg &d : g_drag)
		br.send(std::move(d.bytes));
	g_drag.clear();
	g_drag_sent = std::chrono::steady_clock::now();
}
} // namespace

void drag_send(bridge &br, std::vector<u8> bytes)
{
	if (bytes.empty())
		return;
	const std::string key = drag_key(bytes);
	auto it = std::find_if(g_drag.begin(), g_drag.end(), [&](const drag_msg &d) { return d.key == key; });
	if (it != g_drag.end())
		it->bytes = std::move(bytes);
	else
		g_drag.push_back({ key, std::move(bytes) });
	drag_flush(br);
}

void drag_flush(bridge &br)
{
	if (g_drag.empty())
		return;
	// ボタンを離したらすぐ。押している間は間を空けて
	if (!ImGui::IsMouseDown(ImGuiMouseButton_Left) || std::chrono::steady_clock::now() - g_drag_sent >= DRAG_EVERY)
		drag_send_now(br);
}

void set_current_ram(const xg_snapshot *ram) { g_current_ram = ram; }
const xg_snapshot *current_ram() { return g_current_ram; }

bool fx_type_menu(const std::vector<xg::fx_type> &types, int current, int &chosen)
{
	bool picked = false;
	// 印を置くぶん、名前の前を空白で空ける
	const float fs = ImGui::GetFontSize();
	const int pad = int(std::ceil(fs * 1.35f / std::max(1.0f, ImGui::CalcTextSize(" ").x)));
	auto with_icon = [&](int msb, const char *label, auto &&submit) {
		const ImVec2 pos = ImGui::GetCursorScreenPos();
		ImDrawList *dl = ImGui::GetWindowDrawList();
		const bool r = submit((std::string(size_t(pad), ' ') + label).c_str());
		fx_icon(dl, pos, ImGui::GetTextLineHeight(), msb, ImGui::GetColorU32(ImGuiCol_Text));
		return r;
	};
	auto item = [&](const xg::fx_type &t, bool with_code) {
		const int value = t.msb << 7 | t.lsb;
		char label[64];
		if (with_code) std::snprintf(label, sizeof(label), "%-10s  MSB %d / LSB %d", t.name, t.msb, t.lsb);
		else           std::snprintf(label, sizeof(label), "%s", t.name);
		const bool clicked = (t.msb == 0 || t.msb == 0x40)
		                   ? with_icon(t.msb, label, [&](const char *l) { return ImGui::MenuItem(l, nullptr, value == current); })
		                   : ImGui::MenuItem(label, nullptr, value == current);
		if (clicked) {
			chosen = value;
			picked = true;
		}
		if (help_on() && ImGui::IsItemHovered())
			if (const char *h = fx_type_help(t.msb, t.lsb))
				ImGui::SetTooltip("%s\n%s", t.name, h);
	};
	auto category_of = [](u8 msb) -> int { return fx_category_of(msb); };
	// 系統（MSB）ごとに、表の順で
	auto family = [&](u8 msb) {
		std::vector<const xg::fx_type *> list;
		for (const xg::fx_type &t : types)
			if (t.msb == msb)
				list.push_back(&t);
		if (list.size() == 1) {
			item(*list[0], false);
			return;
		}
		const bool here = current >= 0 && (current >> 7) == msb;
		char label[64];
		std::snprintf(label, sizeof(label), "%s（%d）", list[0]->name, int(list.size()));
		const bool open = ImGui::BeginMenu(label);
		if (help_on() && ImGui::IsItemHovered() && !open)
			if (const char *h = fx_type_help(msb, list[0]->lsb))
				ImGui::SetTooltip("%s", h);
		if (open) {
			for (const xg::fx_type *t : list)
				item(*t, true);
			ImGui::EndMenu();
		}
		if (here) {
			ImGui::SameLine();
			ImGui::TextDisabled("●");
		}
	};
	auto families_in = [&](int cat) {
		std::vector<u8> seen;
		for (const xg::fx_type &t : types) {
			if (t.msb == 0 || t.msb == 0x40 || category_of(t.msb) != cat)
				continue;
			if (std::find(seen.begin(), seen.end(), t.msb) != seen.end())
				continue;
			seen.push_back(t.msb);
			family(t.msb);
		}
		return !seen.empty();
	};

	// NO EFFECT と THRU は分類の外
	for (const xg::fx_type &t : types)
		if (t.msb == 0 || t.msb == 0x40)
			item(t, false);
	ImGui::Separator();

	std::vector<int> used;
	for (const xg::fx_type &t : types) {
		if (t.msb == 0 || t.msb == 0x40)
			continue;
		const int c = category_of(t.msb);
		if (std::find(used.begin(), used.end(), c) == used.end())
			used.push_back(c);
	}
	if (used.size() <= 1) {
		for (int c : used)
			families_in(c);
		return picked;
	}
	const auto &cats = xg::fx_categories();
	for (int c : used) {
		const bool here = current > 0 && (current >> 7) != 0x40 && category_of(u8(current >> 7)) == c;
		const int msb = c >= 0 ? cats[c].msbs[0] : -1;
		if (with_icon(msb, c >= 0 ? cats[c].name : "その他", [](const char *l) { return ImGui::BeginMenu(l); })) {
			families_in(c);
			ImGui::EndMenu();
		}
		if (here) {
			ImGui::SameLine();
			ImGui::TextDisabled("●");
		}
	}
	return picked;
}

namespace {
int  g_fx_slot = 1;
bool g_fx_request = false;
}

void request_fx(int slot) { g_fx_slot = std::clamp(slot, 1, 7); g_fx_request = true; }
bool take_fx_request() { const bool r = g_fx_request; g_fx_request = false; return r; }
int  fx_window_slot() { return g_fx_slot; }
void set_fx_window_slot(int slot) { g_fx_slot = std::clamp(slot, 1, 7); }

namespace {
int  g_shape_part = 0;
bool g_part_request = false;
}

void request_part(int part) { g_shape_part = std::clamp(part, 0, XG_PARTS - 1); g_part_request = true; }
bool take_part_request() { const bool r = g_part_request; g_part_request = false; return r; }
int  shape_window_part() { return g_shape_part; }
void set_shape_window_part(int part) { g_shape_part = std::clamp(part, 0, XG_PARTS - 1); }

namespace {
bool g_master_request = false;
}

void request_master() { g_master_request = true; }
bool take_master_request() { const bool r = g_master_request; g_master_request = false; return r; }

namespace {
bool g_file_dialogs = false;
file_ask g_file_ask = file_ask::none;
std::vector<u8> g_file_out, g_file_in;
bool g_file_in_ready = false;
std::string g_file_note;
}

void set_file_dialogs(bool on) { g_file_dialogs = on; }
bool file_dialogs() { return g_file_dialogs; }
void ask_save_file(std::vector<u8> bytes) { g_file_out = std::move(bytes); g_file_ask = file_ask::save; }
void ask_open_file() { g_file_ask = file_ask::open; }
file_ask take_file_ask(std::vector<u8> &bytes)
{
	const file_ask a = g_file_ask;
	g_file_ask = file_ask::none;
	if (a == file_ask::save)
		bytes = std::move(g_file_out);
	g_file_out.clear();
	return a;
}
void give_opened_file(std::vector<u8> bytes) { g_file_in = std::move(bytes); g_file_in_ready = true; }
bool take_opened_file(std::vector<u8> &bytes)
{
	if (!g_file_in_ready)
		return false;
	bytes = std::move(g_file_in);
	g_file_in.clear();
	g_file_in_ready = false;
	return true;
}
void set_file_note(std::string text) { g_file_note = std::move(text); }
const std::string &file_note() { return g_file_note; }

const xg::param &P(const char *key)
{
	const xg::param *p = xg::find(key);
	IM_ASSERT(p);
	return *p;
}

std::string value_text(const char *key, int v)
{
	const xg::param &p = P(key);
	if (std::strstr(key, "eq") && std::strstr(key, "freq"))
		return eq::hz_text(v) + " Hz";
	if (!std::strncmp(key, "master_eq.q", 11)) {
		char buf[16];
		std::snprintf(buf, sizeof(buf), "%.1f", v / 10.0);
		return buf;
	}
	return xg::format(p, v);
}

std::string param_line(const char *key, int part, xg::model &m)
{
	const xg::param &p = P(key);
	int v = 0;
	return std::string(p.label) + " : " + (m.get(p, part, v) ? value_text(key, v) : std::string("--"));
}

bool param_slider(const char *key, int part, xg::model &m, bridge &br, const char *label)
{
	ImGui::PushID(key);
	const xg::param &p = P(key);
	int v = 0;
	if (!m.get(p, part, v)) {
		ImGui::BeginDisabled();
		int dummy = p.min;
		ImGui::SliderInt(label ? label : p.label, &dummy, p.min, p.max, "--");
		ImGui::EndDisabled();
		ImGui::PopID();
		return false;
	}
	// 書式の % は SliderInt の書式として読まれないよう重ねる
	const std::string shown = value_text(key, v);
	std::string text;
	for (char c : shown) {
		if (c == '%')
			text += '%';
		text += c;
	}
	int nv = v;
	ImGui::SliderInt(label ? label : p.label, &nv, p.min, p.max, text.c_str());
	// ← → で 1 つずつ（Shift で 10）。カーソルが載っている棒か、ほかに載っていなければ最後に触った棒
	const bool typing = ImGui::GetIO().WantTextInput;
	if (!typing && (ImGui::IsItemHovered() || (ImGui::IsItemFocused() && !ImGui::IsAnyItemHovered()))) {
		// キーはこの棒がもらう（ImGui のキーボード移動で、隣の部品へ移らないように）
		const ImGuiID id = ImGui::GetItemID();
		ImGui::SetKeyOwner(ImGuiKey_LeftArrow, id);
		ImGui::SetKeyOwner(ImGuiKey_RightArrow, id);
		const int step = ImGui::GetIO().KeyShift ? 10 : 1;
		if (ImGui::IsKeyPressed(ImGuiKey_LeftArrow, ImGuiInputFlags_Repeat, id))
			nv = std::max(p.min, nv - step);
		if (ImGui::IsKeyPressed(ImGuiKey_RightArrow, ImGuiInputFlags_Repeat, id))
			nv = std::min(p.max, nv + step);
	}
	const bool changed = nv != v;
	if (changed)
		drag_send(br, m.set(p, part, nv));      // ドラッグ中は間引く。キーや数の打ち込みはすぐ送られる
	help_tip(key);
	ImGui::PopID();
	return changed;
}

std::string part_name(int part)
{
	// エフェクトの掛け先は 64 パートの後ろに A/D INPUT が 2 つ並ぶ（実機で確かめた）
	if (part == 64) return "AD1";
	if (part == 65) return "AD2";
	char buf[8];
	const int port = (part < 0 ? 0 : part) / 16;
	std::snprintf(buf, sizeof(buf), "%c%d", char('A' + (port < 4 ? port : 3)), part % 16 + 1);
	return buf;
}

// 受信チャンネルは 0-63 が A1-A16 ... D1-D16、127 が OFF。
// C・D は実機では USB だけの口
std::string channel_name(int v)
{
	if (v == 127)
		return "OFF";
	if (v < 0 || v > 63)
		return std::to_string(v);
	return part_name(v);
}

const char *gm_name(int program)
{
	return GM_NAMES[program & 0x7f];
}

std::string voice_text(int msb, int lsb, int prog)
{
	char buf[80];
	if (msb == 127)
		std::snprintf(buf, sizeof(buf), "%3d  Drum Kit", prog + 1);
	else if (msb == 0 && lsb == 0)
		std::snprintf(buf, sizeof(buf), "%3d  %s", prog + 1, gm_name(prog));
	else
		std::snprintf(buf, sizeof(buf), "%3d  %s（%d/%d）", prog + 1, gm_name(prog), msb, lsb);
	return buf;
}


namespace {

// パートの受信チャンネル（口 × 16 + ch）。ほかのパートと同じチャンネルなら（または
// 分からなければ）-1。チャンネルのメッセージはそのチャンネルのパート全部に効くので
int own_channel(int part, xg::model &m)
{
	const xg::param &rp = P("part.rcv_channel");
	int rcv = 127;
	if (!m.get(rp, part, rcv) || rcv < 0 || rcv > 63)
		return -1;
	for (int i = 0; i < XG_PARTS; i++) {
		int other = 127;
		if (i != part && (!m.get(rp, i, other) || other == rcv))
			return -1;
	}
	return rcv;
}

// バンクとプログラムを選ぶ。
// 受信チャンネルがそのパートだけのものなら、普通のバンクセレクト（CC0・CC32）とプログラムチェンジで
// 送る（SysEx だと LCD に Ex の印が出るので）。写しは set で書き換えるが、返ってくる SysEx は送らない。
// チャンネルが OFF・ほかのパートと共有・分からないときは、パラメータチェンジで送る
void select_voice(int part, int msb, int lsb, int prog, xg::model &m, bridge &br)
{
	const int ch = own_channel(part, m);
	if (ch < 0) {
		br.send(m.set(P("part.bank_msb"), part, msb));
		br.send(m.set(P("part.bank_lsb"), part, lsb));
		br.send(m.set(P("part.program"), part, prog));
		return;
	}
	m.set(P("part.bank_msb"), part, msb);
	m.set(P("part.bank_lsb"), part, lsb);
	m.set(P("part.program"), part, prog);
	const u8 c = u8(ch & 15);
	const u8 msg[8] = { u8(0xb0 | c), 0x00, u8(msb), u8(0xb0 | c), 0x20, u8(lsb), u8(0xc0 | c), u8(prog) };
	br.send_port(ch / 16, msg, sizeof(msg));
}

// ---- 試聴。音色を替えたら、そのパートの受信チャンネルで 1 秒だけ鳴らす
//
// 音色の切り替え（パラメータチェンジ）は口 A の送り口、ノートは受信チャンネルの口の送り口に乗るので、
// 同じコマに送ると読み込む前の音色で鳴ることがある。ノートオンは少し遅らせる。
// 送るのは画面のコマ（program_pane が描かれるたび）なので、窓を閉じたら audition_stop で止める
struct audition {
	int slot = -1, note = -1;          // 鳴らす先（口 × 16 + チャンネル）と鍵
	double on_at = -1.0, off_at = -1.0;
	bool sounding = false;
};
audition g_audition;

double now_seconds()
{
	return std::chrono::duration<double>(std::chrono::steady_clock::now().time_since_epoch()).count();
}

void audition_off(bridge &br)
{
	audition &a = g_audition;
	if (a.sounding) {
		const u8 off[3] = { u8(0x80 | (a.slot & 15)), u8(a.note), 64 };
		br.send_port(a.slot / 16, off, 3);
	}
	a = audition{};
}

// 試聴を始める。受信が OFF（ミュート中など）なら鳴らさない
void audition_start(int part, int msb, xg::model &m, bridge &br)
{
	audition_off(br);
	int rcv = 127;
	if (!m.get(P("part.rcv_channel"), part, rcv) || rcv < 0 || rcv > 63)
		return;
	audition &a = g_audition;
	a.slot = rcv;
	// 鍵盤の右クリックで決めた鍵。決まっていなければドラムキットはスネア、ほかは C3（60）
	a.note = audition_note() >= 0 ? audition_note() : msb == 127 ? 38 : 60;
	const double t = now_seconds();
	a.on_at = t + 0.06;
	a.off_at = a.on_at + 1.0;
}

void audition_tick(bridge &br)
{
	audition &a = g_audition;
	if (a.slot < 0)
		return;
	const double t = now_seconds();
	if (!a.sounding && t >= a.on_at) {
		const u8 on[3] = { u8(0x90 | (a.slot & 15)), u8(a.note), 100 };
		br.send_port(a.slot / 16, on, 3);
		a.sounding = true;
	}
	if (a.sounding && t >= a.off_at)
		audition_off(br);
}

// 音色を替えて試聴する（音色を選ぶ面から）
void select_and_audition(int part, int msb, int lsb, int prog, xg::model &m, bridge &br)
{
	select_voice(part, msb, lsb, prog, m, br);
	audition_start(part, msb, m, br);
}

// あるプログラム番号で選べる音色（MSB/LSB の組）。同じ記録に落ちるものは最初の 1 つだけで、
// 先頭が MSB 0 / LSB 0（その番号の基本の音色）。1 つの番号で 1 万回ほど引くので、
// 引き方が同じ間は番号ごとに覚えておく
struct bank_choice { int msb, lsb; std::string name; };

const std::vector<bank_choice> &bank_choices(const xg::voice_rom &vr, int mode, int set, int prog)
{
	static int key = -1;
	static std::vector<bank_choice> cache[128];
	static bool done[128] = {};
	const int k = (mode << 8) | set;
	if (k != key) {
		key = k;
		for (int i = 0; i < 128; i++) {
			cache[i].clear();
			done[i] = false;
		}
	}
	prog &= 0x7f;
	if (done[prog])
		return cache[prog];
	done[prog] = true;
	std::vector<u32> seen;
	for (int msb = 0; msb < 126; msb++) {
		for (int lsb = 0; lsb < 128; lsb++) {
			const u32 rec = vr.lookup(mode, set, msb, lsb, prog);
			if (!rec)
				continue;
			bool dup = false;
			for (u32 r : seen)
				dup |= r == rec;
			if (dup)
				continue;
			const std::string name = vr.record_name(rec);
			if (name.empty() || name == "Silence")
				continue;
			seen.push_back(rec);
			cache[prog].push_back({ msb, lsb, name });
		}
	}
	return cache[prog];
}

} // namespace

void audition_stop(bridge &br)
{
	audition_off(br);
}

void program_menu(int part, xg::model &m, const xg_snapshot *ram, bridge &br)
{
	int msb = 0, lsb = 0, prog = 0;
	const bool known = m.get(P("part.bank_msb"), part, msb) && m.get(P("part.bank_lsb"), part, lsb) &&
	                   m.get(P("part.program"), part, prog);
	const int mode = ram ? ram->voice_mode : 1;
	const int set  = ram ? ram->voice_set : 1;
	const xg::voice_rom *vr = voices();
	const bool drum = msb == 126 || msb == 127;

	ImGui::TextDisabled("パート %s", part_name(part).c_str());
	ImGui::Separator();

	// ---- 分類 → 基本の音色（プログラム番号）→ その音色のバンク違い
	for (int g = 0; g < 16; g++) {
		const bool here = known && !drum && prog / 8 == g;
		if (ImGui::BeginMenu(GM_GROUPS[g])) {
			for (int i = g * 8; i < g * 8 + 8; i++) {
				const bool current = known && !drum && i == prog;
				std::string base = GM_NAMES[i];
				if (vr) {
					const std::string real = vr->record_name(vr->lookup(mode, set, 0, 0, i));
					if (!real.empty())
						base = real;
				}
				char label[64];
				std::snprintf(label, sizeof(label), "%3d  %s", i + 1, base.c_str());
				const std::vector<bank_choice> *list = vr ? &bank_choices(*vr, mode, set, i) : nullptr;
				if (!list || list->size() <= 1) {
					// バンク違いが無い。そのまま選ぶ
					if (ImGui::MenuItem(label, nullptr, current))
						select_voice(part, 0, 0, i, m, br);
					continue;
				}
				char with_count[80];
				std::snprintf(with_count, sizeof(with_count), "%s（%d）", label, int(list->size()));
				if (ImGui::BeginMenu(with_count)) {
					for (const bank_choice &c : *list) {
						char item[64];
						std::snprintf(item, sizeof(item), "%-10s  MSB %d / LSB %d", c.name.c_str(), c.msb, c.lsb);
						if (ImGui::MenuItem(item, nullptr, current && c.msb == msb && c.lsb == lsb))
							select_voice(part, c.msb, c.lsb, i, m, br);
					}
					ImGui::EndMenu();
				}
				if (current) {
					ImGui::SameLine();
					ImGui::TextDisabled("●");
				}
			}
			ImGui::EndMenu();
		}
		if (here) {                                   // いまの分類に印
			ImGui::SameLine();
			ImGui::TextDisabled("●");
		}
	}

	// ---- ドラムキットと効果音キット
	ImGui::Separator();
	for (int kit_msb : { 127, 126 }) {
		const char *title = kit_msb == 127 ? "ドラムキット（MSB 127）" : "効果音キット（MSB 126）";
		if (!ImGui::BeginMenu(title))
			continue;
		for (int i = 0; i < 128; i++) {
			std::string kit = vr ? vr->kit_name(kit_msb, i) : std::string();
			if (vr && kit.empty())
				continue;
			if (!vr && i)
				break;                                // 名前が読めないときは 1 番だけ
			char label[48];
			std::snprintf(label, sizeof(label), "%3d  %s", i + 1, vr ? kit.c_str() : "Kit");
			if (ImGui::MenuItem(label, nullptr, known && msb == kit_msb && i == prog))
				select_voice(part, kit_msb, 0, i, m, br);
		}
		ImGui::EndMenu();
	}
	if (!vr) {
		ImGui::Separator();
		ImGui::TextDisabled("ROM から音色の名前を読めないので、GM の名前で出している");
	}
}


// 出しっぱなしの音色選び。品書きと違って、押しても閉じないので続けて選べる。
//   左: 分類（16 の組 + ドラム + 効果音）
//   右上: その分類の基本の音色（キットならキットの並び）
//   右下: いまの音色のバンク違い
// 分類は自分で選べるが、外から音色が変わったときは今の音色の分類へ移す
void program_pane(int part, xg::model &m, const xg_snapshot *ram, bridge &br)
{
	constexpr int GROUP_DRUM = 16, GROUP_SFX = 17;

	int msb = 0, lsb = 0, prog = 0;
	const bool known = m.get(P("part.bank_msb"), part, msb) && m.get(P("part.bank_lsb"), part, lsb) &&
	                   m.get(P("part.program"), part, prog);
	const int mode = ram ? ram->voice_mode : 1;
	const int set  = ram ? ram->voice_set : 1;
	const xg::voice_rom *vr = voices();
	const int now_group = !known ? 0 : msb == 127 ? GROUP_DRUM : msb == 126 ? GROUP_SFX : prog / 8;

	audition_tick(br);

	// 今見ている分類。音色が外から変わったら追いかける
	static int group = -1;
	static int last_part = -1, last_seen = -1;
	const int seen = known ? (msb << 8) | prog : -1;
	if (group < 0 || part != last_part || seen != last_seen)
		group = now_group;
	last_part = part;
	last_seen = seen;

	const float fs = ImGui::GetFontSize();
	const ImGuiStyle &st = ImGui::GetStyle();
	const ImVec2 avail = ImGui::GetContentRegionAvail();

	// ---- 左: 分類。いまの音色がある分類には印
	const float group_w = std::min(fs * 10.5f, avail.x * 0.45f);
	if (ImGui::BeginChild("groups", ImVec2(group_w, 0), ImGuiChildFlags_Borders)) {
		for (int g = 0; g < 18; g++) {
			const char *name = g == GROUP_DRUM ? "ドラムキット" : g == GROUP_SFX ? "効果音キット" : GM_GROUPS[g];
			char label[64];
			std::snprintf(label, sizeof(label), "%s%s##g%d", name, known && g == now_group ? " ●" : "", g);
			if (g == GROUP_DRUM)
				ImGui::Separator();
			// 今見ているのと違う分類を押したら、その分類の先頭の音色（キットなら先頭のキット）に替える
			if (ImGui::Selectable(label, g == group) && g != group) {
				group = g;
				if (g < GROUP_DRUM) {
					select_and_audition(part, 0, 0, g * 8, m, br);
				} else {
					const int kmsb = g == GROUP_DRUM ? 127 : 126;
					int first = 0;
					while (vr && first < 127 && vr->kit_name(kmsb, first).empty())
						first++;
					select_and_audition(part, kmsb, 0, first, m, br);
				}
			}
		}
	}
	ImGui::EndChild();
	ImGui::SameLine();

	// ---- 右: 上に音色、下にバンク違い。中身が無くても枠は残す（並びが跳ねないように）。
	// 音色は 8 つなら 8 行ぶんの高さ、キットのように多いときは半分まで。残りはバンク違いに回す
	const bool kits = group >= GROUP_DRUM;
	const int kit_msb = group == GROUP_DRUM ? 127 : 126;
	const std::vector<bank_choice> *banks = (known && msb < 126 && vr) ? &bank_choices(*vr, mode, set, prog) : nullptr;
	const float right_h = ImGui::GetContentRegionAvail().y;
	const float rows_h = ImGui::GetTextLineHeightWithSpacing() * 8 + st.WindowPadding.y * 2;
	const float voices_h = kits ? (right_h - st.ItemSpacing.y) * 0.5f : std::min(rows_h, (right_h - st.ItemSpacing.y) * 0.5f);

	ImGui::BeginGroup();
	if (ImGui::BeginChild("voices", ImVec2(0, voices_h), ImGuiChildFlags_Borders)) {
		if (kits) {
			for (int i = 0; i < 128; i++) {
				std::string kit = vr ? vr->kit_name(kit_msb, i) : std::string();
				if (vr && kit.empty())
					continue;
				if (!vr && i)
					break;
				char label[48];
				std::snprintf(label, sizeof(label), "%3d %s", i + 1, vr ? kit.c_str() : "Kit");
				if (ImGui::Selectable(label, known && msb == kit_msb && i == prog))
					select_and_audition(part, kit_msb, 0, i, m, br);
			}
		} else {
			for (int i = group * 8; i < group * 8 + 8; i++) {
				std::string base = GM_NAMES[i];
				if (vr) {
					const std::string real = vr->record_name(vr->lookup(mode, set, 0, 0, i));
					if (!real.empty())
						base = real;
				}
				char label[64];
				std::snprintf(label, sizeof(label), "%3d %s", i + 1, base.c_str());
				const bool current = known && msb < 126 && i == prog;
				if (ImGui::Selectable(label, current))
					select_and_audition(part, 0, 0, i, m, br);
			}
		}
	}
	ImGui::EndChild();

	// ---- 同じ番号のバンク違い（いまの音色の）
	if (ImGui::BeginChild("banks", ImVec2(0, 0), ImGuiChildFlags_Borders)) {
		if (banks && banks->size() > 1) {
			ImGui::TextDisabled("%3d のバンク違い", prog + 1);
			for (const bank_choice &c : *banks) {
				char item[72];
				std::snprintf(item, sizeof(item), "%s  %d/%d", c.name.c_str(), c.msb, c.lsb);
				if (ImGui::Selectable(item, known && c.msb == msb && c.lsb == lsb))
					select_and_audition(part, c.msb, c.lsb, prog, m, br);
			}
		} else if (known && msb >= 126) {
			ImGui::TextDisabled("キットにはバンク違いが無い");
		} else if (!vr) {
			ImGui::TextDisabled("ROM から音色を読めないので、バンク違いを出せない");
		} else {
			ImGui::TextDisabled("この音色にはバンク違いが無い");
		}
	}
	ImGui::EndChild();
	ImGui::EndGroup();
}



// ---- 説明（ヘルプ）と言語
//
// 文は「キー → 言語ごとの文」の表で持つ。言語を足すときは lang に 1 つ足し、
// HELP の各行に文を 1 つ足す（足りない言語は日本語で出る）。

namespace {

// 言語の並び。editor.ini には code で残す
struct language { const char *code; const char *name; };
const language LANGS[] = {
	{ "ja", "日本語" },
	{ "en", "English" },
};
constexpr int NLANG = int(sizeof(LANGS) / sizeof(LANGS[0]));

struct help_text { const char *name; const char *text[NLANG]; };

// 見出し（一覧の列）とパラメータのキー。初めて触る人に向けて、何が変わるかを書く
const help_text HELP[] = {
	{ "パート（右クリックで音色）", {
		"MU2000 は 32 のパートを同時に鳴らせる。A1-A16 は MIDI IN A の 1-16ch、\n"
		"B1-B16 は MIDI IN B の 1-16ch で受ける（受信チャンネルは変えられる）。\n"
		"右クリックで音色（プログラムとバンク）を選ぶ。\n"
		"右端の M でミュート、S でソロ（S を入れたパートだけが鳴る。いくつでも入れられる）。\n"
		"ミュートはパートの受信チャンネルを OFF にして行う（外すと元のチャンネルに戻す）",
		"The MU2000 plays 32 parts at once. A1-A16 receive MIDI IN A channels 1-16,\n"
		"B1-B16 receive MIDI IN B channels 1-16 (the receive channel can be changed).\n"
		"Right-click to choose the voice (program and bank).\n"
		"M on the right mutes the part, S solos it (only soloed parts play; any number can be soloed).\n"
		"Muting sets the part's receive channel to OFF and restores it afterwards." } },
	{ "マスター", {
		"全体に効く値。移調（Transpose）とマスターチューンもここに出る",
		"Settings for the whole mix, including transpose and master tune." } },
	{ "M.VOL", {
		"マスターボリューム。全体の音量",
		"Master volume. The overall output level." } },
	{ "REVERB", {
		"システムのリバーブ。全パートで 1 台を共有する残響のエフェクト。\n"
		"上の行が種類（右クリックで HALL 1 などを選ぶ）、下が戻り量（エフェクトの音をどれだけ全体に戻すか）。\n"
		"各パートがどれだけ送るかは、パートの表の REV",
		"The system reverb, one unit shared by all parts.\n"
		"Top line: type (right-click to choose, e.g. HALL 1). Bottom: return level.\n"
		"How much each part sends is the REV column in the part table." } },
	{ "CHORUS", {
		"システムのコーラス。全パートで 1 台を共有する揺れと広がりのエフェクト。\n"
		"上の行が種類（右クリックで選ぶ）、下が戻り量。各パートの送り量はパートの表の CHO",
		"The system chorus, one unit shared by all parts.\n"
		"Top line: type (right-click to choose). Bottom: return level. Per-part sends are the CHO column." } },
	{ "VARIATION", {
		"バリエーションエフェクト。種類はディレイやアンプシミュレータなど、インサーションと同じ 27 種類。\n"
		"右クリックで種類と接続を選ぶ。SYSTEM ならリバーブと同じく全パートから送り（パートの VAR）、\n"
		"INSERTION なら 1 つのパートの通り道に直に入る（→ の先のパート）",
		"The variation effect, with the same 27 types as the insertion effects.\n"
		"Right-click to choose the type and connection. SYSTEM: parts send to it like reverb (the VAR column).\n"
		"INSERTION: it is placed directly in one part's signal path (the part after the arrow)." } },
	{ "INS 1", {
		"インサーションエフェクト 1。1 つのパートにだけ掛かる。\n"
		"右クリックで種類と掛けるパート。つかんでパートの表の INS 欄に落としても掛けられる",
		"Insertion effect 1, applied to a single part.\n"
		"Right-click to choose the type and the part, or drag it onto a part's INS cell." } },
	{ "INS 2", { "インサーションエフェクト 2。使い方は INS 1 と同じ", "Insertion effect 2. Works like INS 1." } },
	{ "INS 3", { "インサーションエフェクト 3。使い方は INS 1 と同じ", "Insertion effect 3. Works like INS 1." } },
	{ "INS 4", { "インサーションエフェクト 4。使い方は INS 1 と同じ", "Insertion effect 4. Works like INS 1." } },
	{ "MASTER EQ", {
		"マスター EQ。全部の音の最後に掛かる 5 つの帯のイコライザ。左が低い音、右が高い音。\n"
		"点をつまんで、横で周波数、縦でゲイン（±12dB）。点の近くでホイールを回すと幅（Q）。\n"
		"右クリックで種類（FLAT / JAZZ / POPS / ROCK / CONCERT）と、両端の帯をシェルフにするかピークにするか",
		"Master EQ: a 5-band equaliser applied last, to everything. Low frequencies on the left, high on the right.\n"
		"Drag a point sideways for frequency and up/down for gain (+/-12 dB). Use the wheel near a point for its width (Q).\n"
		"Right-click for the preset type (FLAT / JAZZ / POPS / ROCK / CONCERT) and the shape of the outer bands." } },
	{ "MASTER", {
		"全体に効く値。VOL はマスターボリューム、REV・CHO・VAR はそれぞれのエフェクトの\n"
		"戻り量（エフェクトを通った音を、どれだけ全体に戻すか）",
		"Values for the whole mix. VOL is the master volume; REV, CHO and VAR are the\n"
		"effect return levels (how much of each effect's output is mixed back in)." } },
	{ "MASTER.INS", {
		"システムのエフェクトの種別。R がリバーブ、C がコーラス、V がバリエーション。\n"
		"バリエーションの接続が INSERTION のときは、1 つのパートにだけ掛かるので薄く出す",
		"System effect types: R reverb, C chorus, V variation.\n"
		"The variation is dimmed when it is connected as INSERTION (it then applies to one part only)." } },
	{ "INS", {
		"このパートだけに掛かっているエフェクト。\n"
		"1-4 はインサーションエフェクト、V は接続が INSERTION のバリエーション。\n"
		"歪みやワウ、アンプシミュレータなど、1 つの楽器にだけ掛けたいものに使う",
		"Effects applied to this part only.\n"
		"1-4 are insertion effects; V is the variation effect when connected as INSERTION.\n"
		"Used for things you want on a single instrument, such as distortion, wah or amp simulation." } },
	{ "VEL", {
		"鍵盤を弾いた強さ（ベロシティ）。音が鳴るたびに跳ねて、落ちていく",
		"How hard the key was played (velocity). Jumps on each note and falls back." } },
	{ "VOL", {
		"パートの音量（CC7 / Volume）。曲の中のパートどうしの大きさの釣り合いを取る",
		"Part volume (CC7). Balances the loudness of the parts against each other." } },
	{ "EXP", {
		"エクスプレッション（CC11）。音量をさらに絞る。VOL と掛け算で効き、\n"
		"曲の中で抑揚（だんだん大きく・小さく）をつけるのに使われる。\n"
		"触ると、そのパートの受信チャンネルへ CC11 を送る",
		"Expression (CC11). Scales the volume further, multiplied with VOL.\n"
		"Songs use it for swells and fades. Editing sends CC11 on the part's receive channel." } },
	{ "PAN", {
		"左右の位置（CC10 / Pan）。C が真ん中、L は左、R は右。Rnd は弾くたびにばらばら",
		"Stereo position (CC10). C is centre, L left, R right. Rnd moves on every note." } },
	{ "P.BEND", {
		"ピッチベンド。音程を滑らかに上げ下げする。0 が元の音程。表示だけ",
		"Pitch bend. Slides the pitch up or down; 0 is the original pitch. Display only." } },
	{ "MOD", {
		"モジュレーション（CC1）。ビブラートなど、音の揺れの深さ。\n"
		"触ると、そのパートの受信チャンネルへ CC1 を送る",
		"Modulation (CC1). Depth of vibrato and similar wobble.\n"
		"Editing sends CC1 on the part's receive channel." } },
	{ "HOLD", {
		"ダンパーペダル（CC64）。ON の間は、鍵盤を離しても音が伸びる。表示だけ",
		"Damper pedal (CC64). While ON, notes keep sounding after the keys are released. Display only." } },
	{ "FILTER", {
		"フィルタ。左が低い音、右が高い音で、どこまで通すかの形。\n"
		"点を横に動かすとカットオフ（CC74 / Brightness）。右へ明るく、左へこもった音になる。\n"
		"縦に動かすとレゾナンス（CC71 / Harmonic Content）。上へ、カットオフのあたりが強調されてクセのある音になる。\n"
		"どちらも音色の元の値に対する増減。形は目安で、実際の周波数ではない",
		"Filter. Shows how much of the sound passes, from low (left) to high (right).\n"
		"Drag the point sideways for cutoff (CC74, brightness): right is brighter, left is duller.\n"
		"Drag it up and down for resonance (CC71, harmonic content): up emphasises the area around the cutoff.\n"
		"Both are relative to the voice's own settings. The curve is a guide, not the real frequency response." } },
	{ "VIB", {
		"ビブラート。弾いてから音程がどう揺れるかの形。\n"
		"波の山の点を横に動かすと速さ（Rate、左へ速く）、縦に動かすと深さ（Depth）。\n"
		"平らな所の終わりの点を横に動かすと、揺れ始めるまでの時間（Delay）。\n"
		"どれも音色の元の値に対する増減。形は目安",
		"Vibrato: how the pitch wobbles after a note starts.\n"
		"Drag the crest of the wave sideways for speed (rate, left is faster) and up/down for depth.\n"
		"Drag the end of the flat part sideways for the delay before the vibrato starts.\n"
		"All relative to the voice's own settings. The drawing is a guide." } },
	{ "EQ", {
		"パートの EQ。左が低い音、右が高い音。点 1 が低音、点 2 が高音（どちらもシェルフ）。\n"
		"点をつまんで、横で周波数、縦でゲイン（±12dB）",
		"Part EQ. Low frequencies on the left, high on the right. Point 1 is bass, point 2 is treble (both shelving).\n"
		"Drag a point sideways for frequency and up/down for gain (+/-12 dB)." } },
	{ "EG", {
		"音量の変わり方（エンベロープ）。左から、鍵盤を押して立ち上がる（アタック）、\n"
		"伸ばしている音量へ落ち着く（ディケイ）、伸ばしている間、離して消える（リリース）。\n"
		"3 つの点を横につまんで動かすと、それぞれの長さが変わる（音色の元の長さに対する増減）",
		"How the volume changes over a note (the envelope). From the left: rise after the key is pressed (attack),\n"
		"settle to the held level (decay), the held part, and fade after release (release).\n"
		"Drag the three points sideways to change each time, relative to the voice's own settings." } },
	{ "REV", {
		"リバーブへの送り量（CC91）。部屋やホールの響き（残響）をどれだけ足すか",
		"Reverb send (CC91). How much room or hall ambience is added." } },
	{ "CHO", {
		"コーラスへの送り量（CC93）。音をわずかに揺らして、厚みや広がりを足す",
		"Chorus send (CC93). Adds thickness and width by gently detuning the sound." } },
	{ "VAR", {
		"バリエーションエフェクトへの送り量（CC94）。\n"
		"バリエーションの接続が SYSTEM のときだけ効く（リバーブやコーラスと同じく、\n"
		"全パートで 1 台を共有し、各パートが送る量を決める）。\n"
		"接続が INSERTION のときは 1 つのパートにだけ掛かり、この値は使われない",
		"Variation effect send (CC94).\n"
		"Only used when the variation is connected as SYSTEM (like reverb and chorus,\n"
		"one shared effect that every part sends to).\n"
		"When connected as INSERTION it applies to a single part and this value is ignored." } },

	{ "part.volume", { "パートの音量（CC7）", "Part volume (CC7)." } },
	{ "part.pan", { "左右の位置（CC10）。C が真ん中", "Stereo position (CC10). C is centre." } },
	{ "part.dry_level", {
		"エフェクトを通さない元の音の量。下げると、エフェクトの音だけが残る",
		"Level of the unprocessed sound. Lower it to hear only the effects." } },
	{ "part.reverb_send", { "リバーブへの送り量（CC91）。響きの量", "Reverb send (CC91)." } },
	{ "part.chorus_send", { "コーラスへの送り量（CC93）。広がりと揺れ", "Chorus send (CC93)." } },
	{ "part.variation_send", {
		"バリエーションへの送り量（CC94）。接続が SYSTEM のときだけ効く",
		"Variation send (CC94). Only used when the variation is connected as SYSTEM." } },
	{ "part.cutoff", { "フィルタのカットオフ（CC74）。音の明るさ", "Filter cutoff (CC74). Brightness." } },
	{ "part.resonance", {
		"フィルタのレゾナンス（CC71）。カットオフのあたりを強調する",
		"Filter resonance (CC71). Emphasises the area around the cutoff." } },
	{ "part.hpf_cutoff", {
		"ハイパスフィルタのカットオフ。この高さより低い音を削る。＋で低音が減って軽く薄い音に。\n"
		"音色の元の設定からのずらし量（+0 がそのまま）。レゾナンスは効かない",
		"High-pass filter cutoff. Removes the sound below it; + thins out the low end.\n"
		"An offset from the voice's own setting (+0 leaves it). Resonance does not apply to it." } },
	{ "part.peg_init_level", {
		"ピッチ EG の出だしの音程。鍵盤を押した瞬間、本来の音程からどれだけずれた所から始まるか。\n"
		"音色の元の設定からのずらし量",
		"Pitch EG start level: how far from the true pitch a note starts when the key is pressed.\n"
		"An offset from the voice's own setting." } },
	{ "part.peg_attack_time", {
		"ピッチ EG のアタック。出だしの音程から本来の音程へたどり着くまでの時間",
		"Pitch EG attack: how long the pitch takes to move from the start level to the true pitch." } },
	{ "part.peg_rel_level", {
		"ピッチ EG のリリースレベル。鍵盤を離したあと、音程が最後に向かう先",
		"Pitch EG release level: where the pitch heads after the key is released." } },
	{ "part.peg_rel_time", {
		"ピッチ EG のリリース。鍵盤を離してから、リリースレベルの音程へ移るまでの時間",
		"Pitch EG release time: how long the pitch takes to reach the release level after key-off." } },
	{ "part.vel_limit_low", {
		"このパートが鳴る強さ（ベロシティ）の下限。これより弱く弾いた音は鳴らない",
		"Lowest velocity this part plays. Softer notes are not played." } },
	{ "part.vel_limit_high", {
		"このパートが鳴る強さ（ベロシティ）の上限。これより強く弾いた音は鳴らない。\n"
		"同じチャンネルの 2 つのパートで範囲を分けると、強さで音色を切り替えられる",
		"Highest velocity this part plays. Harder notes are not played.\n"
		"Splitting the range between two parts on the same channel switches voices by velocity." } },
	{ "part.ac1_cc", {
		"AC1 に使うコントロールチェンジの番号（0-95）。下の AC1 の効き先がこの CC で動く",
		"Control change number used as AC1 (0-95). The AC1 settings below respond to it." } },
	{ "part.ac2_cc", {
		"AC2 に使うコントロールチェンジの番号（0-95）。下の AC2 の効き先がこの CC で動く",
		"Control change number used as AC2 (0-95). The AC2 settings below respond to it." } },
	{ "part.porta_switch", {
		"ポルタメント（CC65）。ON で、次の音へ音程が滑らかに移る。ドラムのパートでは使えない",
		"Portamento (CC65). When ON, the pitch glides into the next note. Not available on drum parts." } },
	{ "part.porta_time", {
		"ポルタメントの時間（CC5）。大きいほどゆっくり滑る",
		"Portamento time (CC5). Higher values glide more slowly." } },
	{ "part.attack", {
		"アタック（CC73）。鍵盤を押してから音が立ち上がるまでの速さ。−で速く、＋でゆっくり",
		"Attack (CC73). How fast the sound rises after a key is pressed. - is faster, + is slower." } },
	{ "part.decay", {
		"ディケイ（CC75）。立ち上がったあと、伸ばしている音の大きさへ落ち着くまでの速さ",
		"Decay (CC75). How fast the sound settles after the attack." } },
	{ "part.release", {
		"リリース（CC72）。鍵盤を離してから音が消えるまでの長さ",
		"Release (CC72). How long the sound takes to fade after the key is released." } },
	{ "part.vib_rate", { "ビブラートの速さ", "Vibrato speed." } },
	{ "part.eq_bass_gain", { "パートの EQ の低音のゲイン（±12 の目盛り）。フィルタのすぐ後ろ、声ごとに掛かる（インサーションより前）。"
	                         "実機でも効き方は ±2.4 dB ほどと小さい（2026-09-17 に実機と比べて確かめた）",
	                         "Part EQ bass gain (±12 steps). Applied per voice right after the filter (before insertion effects). "
	                         "The real unit only moves about ±2.4 dB (checked against hardware)." } },
	{ "part.eq_treble_gain", { "パートの EQ の高音のゲイン（±12 の目盛り）。フィルタのすぐ後ろ、声ごとに掛かる（インサーションより前）。"
	                           "実機でも効き方は ±2.4 dB ほどと小さい",
	                           "Part EQ treble gain (±12 steps). Applied per voice right after the filter. The real unit only moves about ±2.4 dB." } },
	{ "part.eq_bass_freq", { "パートの EQ の低音の周波数（32 Hz-2 kHz）。これより下を上げ下げする", "Part EQ bass shelf frequency (32 Hz-2 kHz)." } },
	{ "part.eq_treble_freq", { "パートの EQ の高音の周波数（500 Hz-16 kHz）。これより上を上げ下げする", "Part EQ treble shelf frequency (500 Hz-16 kHz)." } },
	{ "part.vib_depth", { "ビブラートの深さ（音色自身の揺れを深くする）。モジュレーションホイールなどの揺れとは足し合わさず、深いほうが効く",
	                      "Vibrato depth (the voice's own vibrato). It does not add to the vibrato from the mod wheel etc.; the deeper one wins." } },
	{ "part.vib_delay", { "弾いてからビブラートが掛かり始めるまでの時間", "Time before the vibrato starts." } },
	{ "part.note_shift", { "音程を半音単位でずらす（移調）", "Transposes the part in semitones." } },
	{ "part.detune", { "音程をわずかにずらす（音の厚みを出すときなど）", "Fine pitch offset, e.g. to thicken the sound." } },
	{ "part.rcv_channel", {
		"このパートが受ける MIDI チャンネル。A1-A16 は IN A、B1-B16 は IN B",
		"MIDI channel this part receives. A1-A16 are IN A, B1-B16 are IN B." } },
	{ "part.mono_poly", {
		"POLY は和音が鳴る。MONO は 1 音ずつ（前の音を切って次の音）",
		"POLY plays chords. MONO plays one note at a time." } },
	{ "part.mode", {
		"NORMAL は普通の楽器。DRUM 系はドラムセットとして鳴らす",
		"NORMAL is a regular instrument. The DRUM modes play a drum kit." } },
	{ "part.element_reserve", {
		"このパートのために取っておく同時発音数。音が途切れるパートで増やす",
		"Voices reserved for this part. Raise it if notes on this part get cut off." } },
	{ "part.program", { "音色の番号（プログラムチェンジ）", "Voice number (program change)." } },
	{ "part.bank_msb", {
		"音色の組（バンク）の上の桁。0 が普通、64 が効果音、127 がドラム",
		"Bank select MSB. 0 is normal, 64 sound effects, 127 drum kits." } },
	{ "part.bank_lsb", {
		"音色の組（バンク）の下の桁。同じ番号の音色の別版を選ぶ",
		"Bank select LSB. Picks variations of the same voice number." } },
};

bool  g_help = true;
int   g_lang = 0;
float g_zoom = 0.625f;                 // 一覧の表示の大きさ
float g_shapes_zoom = 0.6f;            // パートの音色の窓の表示の大きさ
float g_master_zoom = 0.8f;            // マスターの窓の表示の大きさ
unsigned g_shapes_knobs = 0;           // 音色の窓の区画ごとに「つまみで触る」か（ビットごと）
int   g_audition_note = -1;            // 試聴で鳴らす鍵（-1 は決まっていない）
bool  g_loaded = false;

// Windows: %LOCALAPPDATA%\S-MU2000\editor.ini -- the same place gui.ini lives
// (compat/paths.h), which is what the macOS side used to lack a path for
std::string settings_file()
{
	const std::string dir = smu2000::config_dir();
	return dir.empty() ? std::string() : smu2000::join(dir, "editor.ini");
}

void load_settings()
{
	g_loaded = true;
	const std::string path = settings_file();
	FILE *f = path.empty() ? nullptr : std::fopen(path.c_str(), "rb");
	if (!f)
		return;
	char line[256];
	while (std::fgets(line, sizeof(line), f)) {
		line[std::strcspn(line, "\r\n")] = 0;
		if (!std::strncmp(line, "help=", 5))
			g_help = line[5] != '0';
		else if (!std::strncmp(line, "overview_zoom=", 14))
			g_zoom = std::clamp(float(std::atof(line + 14)), 0.5f, 1.5f);
		else if (!std::strncmp(line, "shapes_zoom=", 12))
			g_shapes_zoom = std::clamp(float(std::atof(line + 12)), 0.4f, 1.5f);
		else if (!std::strncmp(line, "shapes_knobs=", 13))
			g_shapes_knobs = unsigned(std::strtoul(line + 13, nullptr, 10));
		else if (!std::strncmp(line, "audition_note=", 14))
			g_audition_note = std::clamp(std::atoi(line + 14), -1, 127);
		else if (!std::strncmp(line, "master_zoom=", 12))
			g_master_zoom = std::clamp(float(std::atof(line + 12)), 0.4f, 1.5f);
		else if (!std::strncmp(line, "lang=", 5))
			for (int i = 0; i < NLANG; i++)
				if (!std::strcmp(line + 5, LANGS[i].code))
					g_lang = i;
	}
	std::fclose(f);
}

void save_settings()
{
	const std::string path = settings_file();
	if (path.empty())
		return;
	smu2000::ensure_dir(path.substr(0, path.find_last_of("\\/")));
	if (FILE *f = std::fopen(path.c_str(), "wb")) {
		std::fprintf(f, "help=%d\nlang=%s\noverview_zoom=%.3f\nshapes_zoom=%.3f\nmaster_zoom=%.3f\naudition_note=%d\nshapes_knobs=%u\n",
		             g_help ? 1 : 0, LANGS[g_lang].code, g_zoom, g_shapes_zoom, g_master_zoom, g_audition_note,
		             g_shapes_knobs);
		std::fclose(f);
	}
}

void ensure_loaded()
{
	if (!g_loaded)
		load_settings();
}

// 操作の源（モジュレーションホイール・ピッチベンド・アフタータッチ・AC1・AC2）ごとに 6 つずつ並ぶ
// 「効き先」の説明は、源と効き先の 2 つの表から組み立てる（part.ac1_filter なら AC1 × フィルタ）
const char *source_help(const char *name)
{
	struct part_of { const char *key; const char *ja; const char *en; };
	static const part_of SOURCES[] = {
		{ "part.mw_",   "モジュレーションホイール（CC1）", "The modulation wheel (CC1)" },
		{ "part.bend_", "ピッチベンド",                     "Pitch bend" },
		{ "part.cat_",  "チャンネルアフタータッチ（鍵盤を押し込む強さ。チャンネルに 1 つ）",
		                "Channel aftertouch (pressure on the keys, one value per channel)" },
		{ "part.pat_",  "ポリアフタータッチ（鍵ごとの押し込む強さ）", "Polyphonic aftertouch (pressure per key)" },
		{ "part.ac1_",  "AC1（AC1 CC No で決めたコントロールチェンジ）", "AC1 (the control change chosen by AC1 CC No)" },
		{ "part.ac2_",  "AC2（AC2 CC No で決めたコントロールチェンジ）", "AC2 (the control change chosen by AC2 CC No)" },
	};
	static const part_of TARGETS[] = {
		{ "pitch",    "で音程を動かす幅。±24 半音", " moves the pitch by this many semitones (±24)." },
		{ "filter",   "でフィルタのカットオフを動かす量。＋なら上げるほど開き、−なら上げるほど閉じる",
		              " moves the filter cutoff by this much. With + raising it opens the filter, with - it closes it." },
		{ "amp",      "で音量を動かす量。＋なら上げるほど大きく、−なら上げるほど小さく", " changes the volume by this much. With + raising it gets louder, with - quieter." },
		{ "lfo_pmod", "でビブラート（音程の揺れ）を深くする量。音色のビブラート（Vib Depth を含む）とは足し合わさず、深いほうが効く",
		              " adds this much vibrato (pitch wobble). It does not add to the voice's own vibrato (including Vib Depth); the deeper one wins." },
		{ "lfo_fmod", "でフィルタの揺れ（ワウ）を深くする量", " adds this much filter wobble." },
		{ "lfo_amod", "でトレモロ（音量の揺れ）を深くする量", " adds this much tremolo (volume wobble)." },
	};
	static std::string text;
	for (const part_of &s : SOURCES) {
		const size_t n = std::strlen(s.key);
		if (std::strncmp(name, s.key, n))
			continue;
		for (const part_of &t : TARGETS)
			if (!std::strcmp(name + n, t.key)) {
				text = g_lang == 1 ? std::string(s.en) + t.en : std::string(s.ja) + t.ja;
				return text.c_str();
			}
	}
	return nullptr;
}

const char *find_help(const char *name)
{
	for (const help_text &h : HELP)
		if (!std::strcmp(h.name, name))
			return h.text[g_lang] ? h.text[g_lang] : h.text[0];
	return source_help(name);
}

} // namespace

int help_lang()
{
	ensure_loaded();
	return g_lang;
}

void set_help_lang(int lang)
{
	if (lang >= 0 && lang < NLANG)
		g_lang = lang;
}

float &overview_zoom()
{
	ensure_loaded();
	return g_zoom;
}

void set_overview_zoom(float zoom)
{
	ensure_loaded();
	const float z = std::clamp(zoom, 0.5f, 1.5f);
	if (z != g_zoom) {
		g_zoom = z;
		save_settings();
	}
}

float &shapes_zoom()
{
	ensure_loaded();
	return g_shapes_zoom;
}

bool shapes_knobs(int panel)
{
	ensure_loaded();
	return (g_shapes_knobs >> panel) & 1;
}

void set_shapes_knobs(int panel, bool knobs)
{
	ensure_loaded();
	const unsigned v = knobs ? g_shapes_knobs | (1u << panel) : g_shapes_knobs & ~(1u << panel);
	if (v != g_shapes_knobs) {
		g_shapes_knobs = v;
		save_settings();
	}
}

void set_shapes_zoom(float zoom)
{
	ensure_loaded();
	const float z = std::clamp(zoom, 0.4f, 1.5f);
	if (z != g_shapes_zoom) {
		g_shapes_zoom = z;
		save_settings();
	}
}

int audition_note()
{
	ensure_loaded();
	return g_audition_note;
}

void set_audition_note(int note)
{
	ensure_loaded();
	note = std::clamp(note, -1, 127);
	if (note != g_audition_note) {
		g_audition_note = note;
		save_settings();
	}
}

float &master_zoom()
{
	ensure_loaded();
	return g_master_zoom;
}

void set_master_zoom(float zoom)
{
	ensure_loaded();
	const float z = std::clamp(zoom, 0.4f, 1.5f);
	if (z != g_master_zoom) {
		g_master_zoom = z;
		save_settings();
	}
}

bool &help_on()
{
	ensure_loaded();
	return g_help;
}

void help_tip(const char *name)
{
	if (!help_on() || !ImGui::IsItemHovered(ImGuiHoveredFlags_DelayShort))
		return;
	if (const char *t = find_help(name)) {
		if (g_hint_bar) {
			// 説明の帯のある窓では帯へ。1 行目に XG の正式名（あれば）
			const std::string on = official_name(name);
			g_hint = on.empty() ? std::string(t) : on + "\n" + t;
		}
		else
			ImGui::SetTooltip("%s", t);
	}
}

std::string official_name(const char *key)
{
	// XG の仕様書のパラメータ名（パートの番地 08 pp xx の表の名前）
	static const std::pair<const char *, const char *> NAMES[] = {
		{ "part.element_reserve", "ELEMENT RESERVE" }, { "part.bank_msb", "BANK SELECT MSB" },
		{ "part.bank_lsb", "BANK SELECT LSB" }, { "part.program", "PROGRAM NUMBER" },
		{ "part.rcv_channel", "Rcv CHANNEL" }, { "part.mono_poly", "MONO/POLY MODE" },
		{ "part.key_assign", "SAME NOTE NUMBER KEY ON ASSIGN" }, { "part.mode", "PART MODE" },
		{ "part.note_shift", "NOTE SHIFT" }, { "part.detune", "DETUNE" }, { "part.volume", "VOLUME" },
		{ "part.vel_depth", "VELOCITY SENSE DEPTH" }, { "part.vel_offset", "VELOCITY SENSE OFFSET" },
		{ "part.pan", "PAN" }, { "part.note_low", "NOTE LIMIT LOW" }, { "part.note_high", "NOTE LIMIT HIGH" },
		{ "part.dry_level", "DRY LEVEL" }, { "part.chorus_send", "CHORUS SEND" },
		{ "part.reverb_send", "REVERB SEND" }, { "part.variation_send", "VARIATION SEND" },
		{ "part.vib_rate", "VIBRATO RATE" }, { "part.vib_depth", "VIBRATO DEPTH" },
		{ "part.vib_delay", "VIBRATO DELAY" }, { "part.cutoff", "FILTER CUTOFF FREQUENCY" },
		{ "part.resonance", "FILTER RESONANCE" }, { "part.attack", "EG ATTACK TIME" },
		{ "part.decay", "EG DECAY TIME" }, { "part.release", "EG RELEASE TIME" },
		{ "part.ac1_cc", "AC1 CONTROLLER NUMBER" }, { "part.ac2_cc", "AC2 CONTROLLER NUMBER" },
		{ "part.porta_switch", "PORTAMENTO SWITCH" }, { "part.porta_time", "PORTAMENTO TIME" },
		{ "part.peg_init_level", "PITCH EG INITIAL LEVEL" }, { "part.peg_attack_time", "PITCH EG ATTACK TIME" },
		{ "part.peg_rel_level", "PITCH EG RELEASE LEVEL" }, { "part.peg_rel_time", "PITCH EG RELEASE TIME" },
		{ "part.vel_limit_low", "VELOCITY LIMIT LOW" }, { "part.vel_limit_high", "VELOCITY LIMIT HIGH" },
		{ "part.hpf_cutoff", "HPF CUTOFF FREQUENCY" }, { "part.eq_bass_gain", "EQ BASS GAIN" },
		{ "part.eq_treble_gain", "EQ TREBLE GAIN" }, { "part.eq_bass_freq", "EQ BASS FREQUENCY" },
		{ "part.eq_treble_freq", "EQ TREBLE FREQUENCY" },
		// エフェクト（02 01 xx・03 0n xx の表の名前）
		{ "reverb.type", "REVERB TYPE" }, { "reverb.return", "REVERB RETURN" }, { "reverb.pan", "REVERB PAN" },
		{ "chorus.type", "CHORUS TYPE" }, { "chorus.return", "CHORUS RETURN" }, { "chorus.pan", "CHORUS PAN" },
		{ "chorus.to_reverb", "SEND CHORUS TO REVERB" },
		{ "variation.type", "VARIATION TYPE" }, { "variation.return", "VARIATION RETURN" },
		{ "variation.pan", "VARIATION PAN" }, { "variation.to_reverb", "SEND VARIATION TO REVERB" },
		{ "variation.to_chorus", "SEND VARIATION TO CHORUS" }, { "variation.connect", "VARIATION CONNECTION" },
		{ "variation.part", "VARIATION PART" },
		{ "insertion1.type", "INSERTION1 TYPE" }, { "insertion2.type", "INSERTION2 TYPE" },
		{ "insertion3.type", "INSERTION3 TYPE" }, { "insertion4.type", "INSERTION4 TYPE" },
		{ "insertion1.part", "INSERTION1 PART" }, { "insertion2.part", "INSERTION2 PART" },
		{ "insertion3.part", "INSERTION3 PART" }, { "insertion4.part", "INSERTION4 PART" },
	};
	// 操作子 × 行き先の 36 個は形がそろっている（MW LFO PMOD DEPTH など）
	static const std::pair<const char *, const char *> SRC[] = {
		{ "part.mw_", "MW" }, { "part.bend_", "BEND" }, { "part.cat_", "CAT" },
		{ "part.pat_", "PAT" }, { "part.ac1_", "AC1" }, { "part.ac2_", "AC2" },
	};
	static const std::pair<const char *, const char *> DST[] = {
		{ "pitch", "PITCH CONTROL" }, { "filter", "FILTER CONTROL" }, { "amp", "AMPLITUDE CONTROL" },
		{ "lfo_pmod", "LFO PMOD DEPTH" }, { "lfo_fmod", "LFO FMOD DEPTH" }, { "lfo_amod", "LFO AMOD DEPTH" },
	};
	std::string name;
	for (const auto &n : NAMES)
		if (!std::strcmp(n.first, key))
			name = n.second;
	if (name.empty())
		for (const auto &s : SRC) {
			const size_t len = std::strlen(s.first);
			if (std::strncmp(key, s.first, len))
				continue;
			for (const auto &d : DST)
				if (!std::strcmp(key + len, d.first))
					name = std::string(s.second) + " " + d.second;
		}
	if (name.empty())
		return std::string();
	// 番地も添える（08 pp 20 のように。pp はパート）
	if (const xg::param *p = xg::find(key)) {
		char b[24];
		if (p->where == xg::area::part)
			std::snprintf(b, sizeof(b), "（%02X pp %02X）", p->hi, p->lo);
		else
			std::snprintf(b, sizeof(b), "（%02X %02X %02X）", p->hi, p->mid, p->lo);
		name += b;
	}
	return name;
}

const char *help_for(const char *name)
{
	return help_on() ? find_help(name) : nullptr;
}

void help_checkbox()
{
	ensure_loaded();
	if (ImGui::Checkbox(g_lang == 0 ? "説明を出す" : "Show help", &g_help))
		save_settings();
	if (ImGui::IsItemHovered(ImGuiHoveredFlags_DelayShort))
		ImGui::SetTooltip(g_lang == 0 ? "見出しや名前にカーソルを当てたとき、何に効くのかを出す"
		                              : "Explain what each heading or name does when you hover over it");
	ImGui::SameLine();
	ImGui::SetNextItemWidth(ImGui::GetFontSize() * 6);
	if (ImGui::BeginCombo("##lang", LANGS[g_lang].name)) {
		for (int i = 0; i < NLANG; i++)
			if (ImGui::Selectable(LANGS[i].name, i == g_lang)) {
				g_lang = i;
				save_settings();
			}
		ImGui::EndCombo();
	}
}

void headers_with_help(int columns)
{
	ImGui::TableNextRow(ImGuiTableRowFlags_Headers);
	for (int c = 0; c < columns; c++) {
		if (!ImGui::TableSetColumnIndex(c))
			continue;
		const char *name = ImGui::TableGetColumnName(c);
		ImGui::TableHeader(name);
		help_tip(name);
	}
}

} // namespace xgui
} // namespace ui
