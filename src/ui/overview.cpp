// license:BSD-3-Clause

#include "overview.h"

#include "eq_curve.h"

#include "imgui.h"
#include "imgui_internal.h"
#include "xg/fx_types.h"
#include "xg/ram.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

namespace ui {

using namespace xgui;

namespace {

constexpr int PARTS = XG_PARTS;   // 口 A-D の 64 パート

enum class src { param, exp, mod, bend, hold, vib, filter, eq, eg, ins };

// 絵で触る列（1 マスが広い）
bool wide(src s) { return s == src::vib || s == src::filter || s == src::eq || s == src::eg; }

ImU32 col(ImGuiCol c, float a = 1.0f) { return ImGui::GetColorU32(c, a); }

// 押さえている鍵の色。VEL メーターと同じ
const ImU32 NOTE_ON = IM_COL32(236, 116, 70, 255);

// マスターの鍵盤でのパートの色。パートの数だけ色相で振る（隣のパートが似ないよう 7 つ飛ばし）
ImU32 part_color(int part)
{
	float r, g, b;
	ImGui::ColorConvertHSVtoRGB(float((part * 7) % PARTS) / float(PARTS), 0.75f, 1.0f, r, g, b);
	return IM_COL32(int(r * 255), int(g * 255), int(b * 255), 255);
}

// 鍵盤の上の点が、どの鍵か。黒鍵を先に見る。外なら -1。vel に強さ（下ほど強い）
int key_at(ImVec2 pos, float w, float h, ImVec2 at, int &vel)
{
	const float fs = ImGui::GetFontSize();
	const float pad = fs * 0.2f;
	const float top = pos.y + pad, bottom = pos.y + h - pad;
	static const bool BLACK[12] = { 0, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1, 0 };
	static const float WHITE_POS[12] = { 0, 0.6f, 1, 1.6f, 2, 3, 3.6f, 4, 4.6f, 5, 5.6f, 6 };
	const float kw = (w - pad * 2) / 75;
	const float left = pos.x + pad;
	if (at.y < top || at.y > bottom || at.x < left || at.x > left + kw * 75)
		return -1;
	const float frac = (at.y - top) / std::max(1.0f, bottom - top);
	vel = std::clamp(int(30 + 97 * frac), 1, 127);
	if (at.y < top + (bottom - top) * 0.6f) {
		for (int note = 0; note < 128; note++) {
			if (!BLACK[note % 12]) continue;
			const float x = left + (note / 12 * 7 + WHITE_POS[note % 12]) * kw;
			if (at.x >= x && at.x < x + kw * 0.8f)
				return note;
		}
	}
	for (int note = 0; note < 128; note++) {
		if (BLACK[note % 12]) continue;
		const float x = left + (note / 12 * 7 + WHITE_POS[note % 12]) * kw;
		if (at.x >= x && at.x < x + kw)
			return note;
	}
	return -1;
}

// 128 鍵の鍵盤。color は鍵ごとの色（0 なら押さえていない）
template <typename F>
void draw_keys(ImDrawList *dl, ImVec2 pos, float w, float h, F color)
{
	const float fs = ImGui::GetFontSize();
	const float pad = fs * 0.2f;
	const float top = pos.y + pad, bottom = pos.y + h - pad;
	static const bool BLACK[12] = { 0, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1, 0 };
	static const float WHITE_POS[12] = { 0, 0.6f, 1, 1.6f, 2, 3, 3.6f, 4, 4.6f, 5, 5.6f, 6 };
	constexpr int WHITES = 75;                    // 0-127 の白鍵
	const float kw = (w - pad * 2) / WHITES;
	const float left = pos.x + pad;
	for (int note = 0; note < 128; note++) {
		if (BLACK[note % 12]) continue;
		const float x = left + (note / 12 * 7 + WHITE_POS[note % 12]) * kw;
		const ImU32 c = color(note);
		dl->AddRectFilled(ImVec2(x, top), ImVec2(x + kw - 1, bottom), c ? c : IM_COL32(220, 220, 215, 255));
	}
	for (int note = 0; note < 128; note++) {
		if (!BLACK[note % 12]) continue;
		const float x = left + (note / 12 * 7 + WHITE_POS[note % 12]) * kw;
		const ImU32 c = color(note);
		dl->AddRectFilled(ImVec2(x, top), ImVec2(x + kw * 0.8f, top + (bottom - top) * 0.6f), c ? c : IM_COL32(30, 30, 32, 255));
	}
}

} // namespace

// 小さなマスの説明に足す一言
constexpr const char *BIG_HINT = "\nダブルクリックで大きな窓に出す";

struct overview::column {
	const char *title;
	src from;
	const char *key;      // from が param のとき
};

// 列の並び。前半は Domino の並び（VOL EXP PAN P.BEND MOD HOLD）。後半は音の流れの順に、
// 音色を作るもの（揺れ → フィルタ → 音量の形 → パートの EQ）、インサーション、
// 送り（バリエーション → コーラス → リバーブ。前のものは後ろへも送れる）
static const overview::column COLUMNS[] = {
	{ "VOL",    src::param, "part.volume" },
	{ "EXP",    src::exp,   nullptr },
	{ "PAN",    src::param, "part.pan" },
	{ "P.BEND", src::bend,  nullptr },
	{ "MOD",    src::mod,   nullptr },
	{ "HOLD",   src::hold,  nullptr },
	{ "VIB",    src::vib,   nullptr },          // ビブラートの速さ・深さ・掛かり始めを 1 マスで
	{ "FILTER", src::filter, nullptr },         // カットオフとレゾナンスを 1 マスで（Domino の CUT RESO）
	{ "EG",     src::eg,    nullptr },          // アタック・ディケイ・リリースを 1 マスで
	{ "EQ",     src::eq,    nullptr },          // パートの EQ（低音・高音の周波数とゲイン）を 1 マスで
	{ "INS",    src::ins,   nullptr },          // 掛かっているインサーション
	{ "VAR",    src::param, "part.variation_send" },
	{ "CHO",    src::param, "part.chorus_send" },
	{ "REV",    src::param, "part.reverb_send" },
};
static constexpr int NCOLS = int(sizeof(COLUMNS) / sizeof(COLUMNS[0]));

// マスターの行で、その列に出すもの。無ければ空欄
static const char *master_key(const char *title)
{
	if (!std::strcmp(title, "VOL")) return "system.master_volume";
	if (!std::strcmp(title, "REV")) return "reverb.return";
	if (!std::strcmp(title, "CHO")) return "chorus.return";
	if (!std::strcmp(title, "VAR")) return "variation.return";
	return nullptr;
}


void overview::cell(const column &c, int part, xg::model &m, const xg_snapshot &ram, bridge &br,
                    float w, float h)
{
	ImGuiIO &io = ImGui::GetIO();
	const float fs = ImGui::GetFontSize();

	if (wide(c.from) || c.from == src::ins) {
		if (part < 0)
			ImGui::Dummy(ImVec2(w, h));
		else if (c.from == src::ins)
			ins_cell(part, m, br, h);
		else {
			if (c.from == src::eg)
				eg_cell(part, m, br, w, h, true);
			else if (c.from == src::filter)
				filter_cell(part, m, br, w, h, true);
			else if (c.from == src::eq)
				eq_cell(part, m, br, w, h, true);
			else
				vib_cell(part, m, br, w, h, true);
			// 小さなマスでは点をつまみにくいので、ダブルクリックでパートの音色の窓に大きく出す
			if (ImGui::IsItemHovered() && ImGui::IsMouseDoubleClicked(ImGuiMouseButton_Left)) {
				m_part = part;
				request_part(part);
			}
		}
		return;
	}

	// part が -1 ならマスターの行。列ごとに、システムやエフェクトの戻りの値を出す
	const bool master = part < 0;
	src from = c.from;
	const char *key = c.key;
	if (master) {
		key = master_key(c.title);
		from = src::param;
		if (!key) {
			ImGui::Dummy(ImVec2(w, h));
			return;
		}
	}
	const int at = master ? 0 : part;
	const u8 *blk = master ? nullptr : ram.parts[part];

	// バリエーションの接続が INSERTION のとき、送り（パートの VAR）も戻り（マスターの VAR）も
	// 使われない。触れはするが薄く出す
	int conn = 1;
	const bool dim = !std::strcmp(c.title, "VAR") && m.get(P("variation.connect"), 0, conn) && conn == 0;

	// 値と、見せ方
	int v = 0, lo = 0, hi = 127;
	bool known = true, bipolar = false, editable = false;
	std::string text;
	const xg::param *p = from == src::param ? &P(key) : nullptr;
	switch (from) {
	case src::param:
		known = m.get(*p, at, v);
		lo = p->min; hi = p->max;
		bipolar = p->how == xg::view::center || p->how == xg::view::pan;
		editable = known;
		text = known ? xg::format(*p, v) : "--";
		break;
	case src::exp:  v = blk[xg::ram::PART_EXP] & 0x7f; text = std::to_string(v); break;
	case src::mod:  v = blk[xg::ram::PART_MOD] & 0x7f; text = std::to_string(v); break;
	case src::bend: {
		// RAM には MSB の半分と、下のバイトの最下位ビットに MSB の残り
		const int msb = (blk[xg::ram::PART_BEND] & 0x3f) * 2 + (blk[xg::ram::PART_BEND + 1] & 1);
		v = msb; bipolar = true;
		char buf[8];
		std::snprintf(buf, sizeof(buf), "%+d", msb - 64);
		text = msb == 64 ? "0" : buf;
		break;
	}
	case src::hold: v = blk[xg::ram::PART_HOLD] ? 127 : 0; text = v ? "ON" : "OFF"; break;
	default: break;
	}

	ImGui::PushID(c.title);
	const ImVec2 pos = ImGui::GetCursorScreenPos();
	ImGui::InvisibleButton("##cell", ImVec2(w, h), ImGuiButtonFlags_MouseButtonLeft);
	const ImGuiID id = ImGui::GetItemID();
	const bool hovered = ImGui::IsItemHovered();
	const bool active = ImGui::IsItemActive();

	int nv = v;
	if (editable) {
		if (active && ImGui::IsMouseDragging(ImGuiMouseButton_Left, 0.0f)) {
			// 横にも縦にも効く。全域を 200px ほどで（Shift で細かく）
			// a Get*Ref reference goes stale when an insert grows the storage,
			// so take a value and write it back
			ImGuiStorage *st = ImGui::GetStateStorage();
			float acc = st->GetFloat(id, 0.0f);
			acc += (io.MouseDelta.x - io.MouseDelta.y) * float(hi - lo) / (io.KeyShift ? 800.0f : 200.0f);
			const int step = int(acc);
			if (step) { nv = std::clamp(nv + step, lo, hi); acc -= float(step); }
			st->SetFloat(id, acc);
		}
		if (ImGui::IsItemDeactivated())
			ImGui::GetStateStorage()->SetFloat(id, 0.0f);
		if (hovered && ImGui::GetTime() - m_scrolled_at > 0.5) {
			ImGui::SetItemKeyOwner(ImGuiKey_MouseWheelY);
			if (io.MouseWheel != 0.0f) {
				nv = std::clamp(nv + (io.MouseWheel > 0 ? 1 : -1) * (io.KeyCtrl ? 10 : 1), lo, hi);
				m_wheel_taken = true;
			}
		}
		if (hovered && ImGui::IsMouseDoubleClicked(ImGuiMouseButton_Left))
			ImGui::OpenPopup("##type");
		if (ImGui::BeginPopup("##type")) {
			ImGui::TextDisabled("%s %s（%d-%d）", master ? "MASTER" : part_name(part).c_str(), p->label, lo, hi);
			const ImGuiID typed_id = ImGui::GetID("typed");
			ImGuiStorage *st = ImGui::GetStateStorage();
			int typed = st->GetInt(typed_id, v);
			if (ImGui::IsWindowAppearing()) { typed = v; ImGui::SetKeyboardFocusHere(); }
			ImGui::SetNextItemWidth(fs * 6);
			if (ImGui::InputInt("##n", &typed, 1, 10, ImGuiInputTextFlags_EnterReturnsTrue)) {
				nv = std::clamp(typed, lo, hi);
				ImGui::CloseCurrentPopup();
			}
			st->SetInt(typed_id, typed);
			ImGui::EndPopup();
		}
		if (nv != v) {
			br.send(m.set(*p, at, nv));
			text = xg::format(*p, nv);
		}
	}

	// 描く。上に棒、下に数
	ImDrawList *dl = ImGui::GetWindowDrawList();
	const float pad = fs * 0.2f;
	const float bar_h = std::max(3.0f, fs * 0.45f);
	const ImVec2 b0(pos.x + pad, pos.y + pad);
	const ImVec2 b1(pos.x + w - pad, b0.y + bar_h);
	dl->AddRectFilled(b0, b1, col(ImGuiCol_FrameBg));
	if (known && hi > lo) {
		const float frac = std::clamp(float(nv - lo) / float(hi - lo), 0.0f, 1.0f);
		ImU32 fill = editable ? col(active || hovered ? ImGuiCol_SliderGrabActive : ImGuiCol_SliderGrab)
		                      : IM_COL32(200, 70, 60, 255);
		if (dim)
			fill = col(ImGuiCol_TextDisabled, 0.5f);
		const float x = b0.x + (b1.x - b0.x) * frac;
		if (bipolar) {
			const float mid = (b0.x + b1.x) * 0.5f;
			dl->AddRectFilled(ImVec2(std::min(mid, x) - 1, b0.y), ImVec2(std::max(mid, x) + 1, b1.y), fill);
		} else {
			dl->AddRectFilled(b0, ImVec2(x, b1.y), fill);
		}
	}
	if (hovered)
		dl->AddRect(ImVec2(pos.x + 1, pos.y + 1), ImVec2(pos.x + w - 1, pos.y + h - 1), col(ImGuiCol_Border));
	const ImVec2 ts = ImGui::CalcTextSize(text.c_str());
	dl->AddText(ImVec2(pos.x + w - pad - ts.x, b1.y + (pos.y + h - b1.y - ts.y) * 0.5f),
	            known && !dim ? col(ImGuiCol_Text) : col(ImGuiCol_TextDisabled), text.c_str());

	if (hovered && !active) {
		const char *what = master ? p->label : c.title;
		if (dim)
			ImGui::SetItemTooltip("%s  %s\nバリエーションの接続が INSERTION なので、この値は使われない", what, text.c_str());
		else
			ImGui::SetItemTooltip(editable ? "%s  %s\n左右か上下にドラッグ・ホイール・ダブルクリックで打つ"
			                               : "%s  %s\n演奏の値（表示だけ）", what, text.c_str());
	}
	ImGui::PopID();
}



namespace {

// INS 列で扱うエフェクト。1-4 がインサーション、5 がバリエーション（接続が INSERTION のとき）
struct fx_slot { int id; const char *mark; ImU32 color; const char *part_key; const char *type_key; const char *title; };

const fx_slot FX_SLOTS[] = {
	{ 1, "1", IM_COL32(214, 160, 48, 255),  "insertion1.part", "insertion1.type", "インサーション 1" },
	{ 2, "2", IM_COL32(214, 160, 48, 255),  "insertion2.part", "insertion2.type", "インサーション 2" },
	{ 3, "3", IM_COL32(214, 160, 48, 255),  "insertion3.part", "insertion3.type", "インサーション 3" },
	{ 4, "4", IM_COL32(214, 160, 48, 255),  "insertion4.part", "insertion4.type", "インサーション 4" },
	{ 5, "V", IM_COL32(150, 110, 220, 255), "variation.part",  "variation.type",  "バリエーション" },
};

constexpr const char *DRAG_FX = "S_MU2000_FX";

// そのエフェクトが今どのパートに掛かっているか。掛かっていなければ -1
int fx_target(const fx_slot &f, xg::model &m)
{
	int who = 127, conn = 1;
	if (!m.get(P(f.part_key), 0, who) || who >= PARTS + 2)
		return -1;
	if (f.id == 5 && (!m.get(P("variation.connect"), 0, conn) || conn != 0))
		return -1;                               // SYSTEM のバリエーションはパートに掛からない
	return who;
}

// エフェクトを別のパートへ（バリエーションは INSERTION にもする）
void fx_move(const fx_slot &f, int part, xg::model &m, bridge &br)
{
	if (f.id == 5)
		br.send(m.set(P("variation.connect"), 0, 0));
	br.send(m.set(P(f.part_key), 0, part));
}

void fx_menu(int part, xg::model &m, bridge &br)
{
	ImGui::TextDisabled("パート %s に掛けるエフェクト", part_name(part).c_str());
	ImGui::Separator();
	for (const fx_slot &f : FX_SLOTS) {
		int type = 0;
		const bool has_type = m.get(P(f.type_key), 0, type);
		const int where = fx_target(f, m);
		char label[128];
		if (f.id == 5 && where < 0)
			std::snprintf(label, sizeof(label), "%s（いま SYSTEM・%s）", f.title, has_type ? xg::fx_name(type).c_str() : "--");
		else
			std::snprintf(label, sizeof(label), "%s（いま %s・%s）", f.title,
			              where >= 0 ? part_name(where).c_str() : "OFF", has_type ? xg::fx_name(type).c_str() : "--");
		if (!ImGui::BeginMenu(label))
			continue;
		if (where == part) {
			if (ImGui::MenuItem(f.id == 5 ? "このパートから外して SYSTEM に戻す" : "このパートから外す")) {
				if (f.id == 5) br.send(m.set(P("variation.connect"), 0, 1));
				else           br.send(m.set(P(f.part_key), 0, 127));
			}
		} else if (ImGui::MenuItem(f.id == 5 ? "INSERTION にしてこのパートに掛ける" : "このパートに掛ける")) {
			fx_move(f, part, m, br);
		}
		ImGui::Separator();
		ImGui::TextDisabled("種類");
		int chosen = 0;
		if (fx_type_menu(xg::ins_types(), has_type ? type : -1, chosen))
			br.send(m.set(P(f.type_key), 0, chosen));
		ImGui::EndMenu();
	}
	ImGui::Separator();
	ImGui::TextDisabled("印をドラッグして、別のパートの INS 欄に落とすと移る。\n種類が NO EFFECT のまま掛けると、そのパートの音が消える");
}

} // namespace


void overview::ins_cell(int part, xg::model &m, bridge &br, float h)
{
	const float fs = ImGui::GetFontSize();
	ImDrawList *dl = ImGui::GetWindowDrawList();

	struct on_part { const fx_slot *slot; std::string name; };
	std::vector<on_part> on;
	for (const fx_slot &f : FX_SLOTS) {
		int type = 0;
		if (fx_target(f, m) == part && m.get(P(f.type_key), 0, type))
			on.push_back({ &f, xg::fx_name(type) });
	}

	const ImVec2 pos = ImGui::GetCursorScreenPos();
	const float w = ImGui::GetContentRegionAvail().x;
	ImGui::SetNextItemAllowOverlap();
	ImGui::InvisibleButton("##ins", ImVec2(w, h), ImGuiButtonFlags_MouseButtonLeft | ImGuiButtonFlags_MouseButtonRight);
	const bool cell_hovered = ImGui::IsItemHovered();
	// 落とし先。別のパートから印を持ってきたら、そのエフェクトをこのパートへ
	if (ImGui::BeginDragDropTarget()) {
		if (const ImGuiPayload *pl = ImGui::AcceptDragDropPayload(DRAG_FX)) {
			const int id = *static_cast<const int *>(pl->Data);
			for (const fx_slot &f : FX_SLOTS)
				if (f.id == id)
					fx_move(f, part, m, br);
		}
		ImGui::EndDragDropTarget();
	}
	if (ImGui::BeginPopupContextItem("fxmenu")) {
		fx_menu(part, m, br);
		ImGui::EndPopup();
	}
	if (cell_hovered && on.empty() && !ImGui::IsDragDropActive())
		ImGui::SetItemTooltip("右クリックでエフェクトを掛ける");

	dl->PushClipRect(pos, ImVec2(pos.x + w, pos.y + h), true);
	const float line = fs * 1.05f;
	for (size_t i = 0; i < on.size() && i < 2; i++) {
		const float y = pos.y + fs * 0.1f + line * float(i);
		const float bw = fs * 1.0f;
		const fx_slot &f = *on[i].slot;
		std::string name = on[i].name;
		if (i == 1 && on.size() > 2)
			name += " ほか";

		// 印の行はつかめる（ドラッグで移す）
		ImGui::SetCursorScreenPos(ImVec2(pos.x, y));
		ImGui::PushID(f.id);
		ImGui::InvisibleButton("##fx", ImVec2(w, line), ImGuiButtonFlags_MouseButtonLeft | ImGuiButtonFlags_MouseButtonRight);
		const bool hot = ImGui::IsItemHovered() || ImGui::IsItemActive();
		if (f.id <= 4 && ImGui::IsItemHovered() && ImGui::IsMouseDoubleClicked(ImGuiMouseButton_Left))
			request_fx(f.id);                    // 設定の窓を出す
		if (ImGui::BeginDragDropSource()) {
			ImGui::SetDragDropPayload(DRAG_FX, &f.id, sizeof(f.id));
			ImGui::Text("%s（%s）を移す", f.title, name.c_str());
			ImGui::EndDragDropSource();
		}
		ImGui::OpenPopupOnItemClick("fxmenu_badge", ImGuiPopupFlags_MouseButtonRight);
		if (ImGui::BeginPopup("fxmenu_badge")) {
			fx_menu(part, m, br);
			ImGui::EndPopup();
		}
		if (ImGui::IsItemHovered() && !ImGui::IsDragDropActive())
			ImGui::SetItemTooltip(f.id <= 4 ? "%s: %s\nダブルクリックで設定の窓・ドラッグで別のパートへ・右クリックで種類や外す"
			                                : "%s: %s\nドラッグで別のパートへ・右クリックで種類や外す", f.title, on[i].name.c_str());
		ImGui::PopID();

		dl->AddRectFilled(ImVec2(pos.x + fs * 0.2f, y + 1), ImVec2(pos.x + fs * 0.2f + bw, y + fs), f.color, 3.0f);
		const ImVec2 ms = ImGui::CalcTextSize(f.mark);
		dl->AddText(ImVec2(pos.x + fs * 0.2f + (bw - ms.x) * 0.5f, y), IM_COL32(20, 20, 20, 255), f.mark);
		dl->AddText(ImVec2(pos.x + fs * 1.5f, y), hot ? col(ImGuiCol_SliderGrabActive) : col(ImGuiCol_Text), name.c_str());
	}
	// 落とせる欄を光らせる
	if (cell_hovered && ImGui::GetDragDropPayload() && ImGui::GetDragDropPayload()->IsDataType(DRAG_FX))
		dl->AddRect(ImVec2(pos.x + 1, pos.y + 1), ImVec2(pos.x + w - 1, pos.y + h - 1), col(ImGuiCol_DragDropTarget), 3.0f, 0, 2.0f);
	dl->PopClipRect();
	ImGui::SetCursorScreenPos(ImVec2(pos.x, pos.y + h));
	ImGui::Dummy(ImVec2(0, 0));
}


// EG の 1 マス。音量の形（立ち上がり → 落ち着き → 伸ばし → 離して消える）を折れ線で描き、
// 3 つの点をつまんで横に動かすと、アタック・ディケイ・リリースが変わる。
// XG の値は音色の元の値に対する増減（64 が音色のまま）。形の長さは 2 の (値 - 64) / 24 乗で伸び縮みさせ、
// 真ん中の値で各区間が同じくらいの長さになるようにした（見た目だけ。実際の秒数ではない）
void overview::eg_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact)
{
	ImGuiIO &io = ImGui::GetIO();
	const float fs = ImGui::GetFontSize();
	ImDrawList *dl = ImGui::GetWindowDrawList();
	const xg::param &pa = P("part.attack"), &pd = P("part.decay"), &pr = P("part.release");
	int va = 64, vd = 64, vr = 64;
	const bool known = m.get(pa, part, va) && m.get(pd, part, vd) && m.get(pr, part, vr);

	ImGui::PushID("eg");
	const ImVec2 pos = ImGui::GetCursorScreenPos();
	ImGui::InvisibleButton("##eg", ImVec2(w, h), ImGuiButtonFlags_MouseButtonLeft);
	const ImGuiID id = ImGui::GetItemID();
	const bool hovered = ImGui::IsItemHovered();
	const bool active = ImGui::IsItemActive();

	const float pad = fs * 0.25f;
	const float x0 = pos.x + pad, x1 = pos.x + w - pad;
	const float top = pos.y + pad, bottom = pos.y + h - pad;
	const float sustain_y = top + (bottom - top) * 0.45f;
	const float unit = (x1 - x0) / 4.0f;                    // 真ん中の値のときの 1 区間
	auto len = [&](int v) { return unit * 0.5f * std::pow(2.0f, float(v - 64) / 24.0f); };
	const float hold = unit * 0.5f;                          // 伸ばしている間（固定）

	// 3 つの点の位置。はみ出すときは全体を縮める
	float la = len(va), ld = len(vd), lr = len(vr);
	const float total = la + ld + hold + lr;
	const float squeeze = total > (x1 - x0) ? (x1 - x0) / total : 1.0f;
	const float xa = x0 + la * squeeze;
	const float xd = xa + ld * squeeze;
	const float xs = xd + hold * squeeze;
	const float xr = xs + lr * squeeze;

	// つかむ点。押した瞬間に一番近い点を選び、離すまで同じ点を動かす
	ImGuiStorage *st = ImGui::GetStateStorage();
	int grab = st->GetInt(id, -1);
	if (ImGui::IsItemActivated() && known) {
		const float mx = io.MousePos.x;
		const float dists[3] = { std::fabs(mx - xa), std::fabs(mx - xd), std::fabs(mx - xr) };
		grab = int(std::min_element(dists, dists + 3) - dists);
	}
	if (!active)
		grab = -1;
	st->SetInt(id, grab);
	if (active && known && grab >= 0 && io.MouseDelta.x != 0.0f) {
		// 動かした幅を値に直す。2 倍の長さが 24 目盛り
		auto apply = [&](const xg::param &p, int v, float from, float to_len) {
			const float cur = std::max(1.0f, from);
			const float want = std::max(1.0f, to_len);
			int nv = std::clamp(int(std::lround(64 + 24 * std::log2(want / (unit * 0.5f)))), p.min, p.max);
			(void)cur;
			if (nv != v)
				br.send(m.set(p, part, nv));
		};
		const float mx = io.MousePos.x;
		if (grab == 0) apply(pa, va, la, (mx - x0) / squeeze);
		if (grab == 1) apply(pd, vd, ld, (mx - xa) / squeeze);
		if (grab == 2) apply(pr, vr, lr, (mx - xs) / squeeze);
	}

	// 描く
	dl->AddRectFilled(pos, ImVec2(pos.x + w, pos.y + h), col(hovered || active ? ImGuiCol_FrameBgHovered : ImGuiCol_FrameBg), 3.0f);
	if (known) {
		const ImU32 line = col(ImGuiCol_SliderGrabActive);
		const ImVec2 pts[] = { { x0, bottom }, { xa, top }, { xd, sustain_y }, { xs, sustain_y }, { xr, bottom } };
		// 面を薄く塗ってから線
		dl->PathClear();
		for (const ImVec2 &p : pts) dl->PathLineTo(p);
		dl->PathFillConcave(col(ImGuiCol_SliderGrab, 0.25f));
		dl->AddPolyline(pts, 5, line, 0, std::max(1.5f, fs * 0.1f));
		const float r = std::max(2.5f, fs * 0.22f);
		const ImVec2 handles[] = { pts[1], pts[2], pts[4] };
		for (int i = 0; i < 3; i++)
			dl->AddCircleFilled(handles[i], i == grab ? r * 1.4f : r, i == grab ? col(ImGuiCol_Text) : line);
	} else {
		const ImVec2 ts = ImGui::CalcTextSize("--");
		dl->AddText(ImVec2(pos.x + (w - ts.x) * 0.5f, pos.y + (h - ts.y) * 0.5f), col(ImGuiCol_TextDisabled), "--");
	}

	if ((hovered || active) && known)
		ImGui::SetItemTooltip("Attack %s   Decay %s   Release %s\n点を横につまんで動かす（右へ長く、左へ短く）%s",
		                      xg::format(pa, va).c_str(), xg::format(pd, vd).c_str(), xg::format(pr, vr).c_str(),
		                      compact ? BIG_HINT : "");
	ImGui::PopID();
}

// フィルタの 1 マス。低い音から高い音への通り方（2 次のローパス）を描き、カットオフの位置の点を
// つまむ。横に動かすとカットオフ、縦に動かすとレゾナンス（山の高さ）が変わる。
// 横軸は値に比例（64 が真ん中 = 音色のまま）で、1 マスの幅が 8 オクターブ。
// 点の高さは、カットオフでの持ち上がり 20log10(Q) dB。Q = 2 の (値 - 64) / 16 乗なので、
// 高さも値に比例する。どちらも見た目だけで、実際の周波数や Q ではない
void overview::filter_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact)
{
	ImGuiIO &io = ImGui::GetIO();
	const float fs = ImGui::GetFontSize();
	ImDrawList *dl = ImGui::GetWindowDrawList();
	const xg::param &pc = P("part.cutoff"), &pq = P("part.resonance");
	int vc = 64, vq = 64;
	const bool known = m.get(pc, part, vc) && m.get(pq, part, vq);

	ImGui::PushID("filter");
	const ImVec2 pos = ImGui::GetCursorScreenPos();
	ImGui::InvisibleButton("##filter", ImVec2(w, h), ImGuiButtonFlags_MouseButtonLeft);
	const ImGuiID id = ImGui::GetItemID();
	const bool hovered = ImGui::IsItemHovered();
	const bool active = ImGui::IsItemActive();

	const float pad = fs * 0.25f;
	const float x0 = pos.x + pad, x1 = pos.x + w - pad;
	const float top = pos.y + pad, bottom = pos.y + h - pad;
	const float DB_TOP = 26.0f, DB_BOTTOM = -30.0f;
	auto y_of = [&](float db) { return top + (bottom - top) * (DB_TOP - std::clamp(db, DB_BOTTOM, DB_TOP)) / (DB_TOP - DB_BOTTOM); };
	auto db_of_value = [](int v) { return 6.0206f * float(v - 64) / 16.0f; };
	const float y0db = y_of(0.0f);
	const float xc = x0 + (x1 - x0) * float(vc) / 127.0f;
	const float yq = y_of(db_of_value(vq));

	// つかんだときの、点とマウスのずれを覚えておき、点が指に飛ばないようにする
	// a Get*Ref reference goes stale when an insert grows the storage, so take
	// a value and write it back
	ImGuiStorage *st = ImGui::GetStateStorage();
	float gx = st->GetFloat(id, 0.0f), gy = st->GetFloat(id + 1, 0.0f);
	if (ImGui::IsItemActivated() && known) {
		gx = xc - io.MousePos.x;
		gy = yq - io.MousePos.y;
		st->SetFloat(id, gx);
		st->SetFloat(id + 1, gy);
	}
	if (active && known && (io.MouseDelta.x != 0.0f || io.MouseDelta.y != 0.0f)) {
		const float fx = io.MousePos.x + gx, fy = io.MousePos.y + gy;
		const int nc = std::clamp(int(std::lround((fx - x0) / (x1 - x0) * 127.0f)), pc.min, pc.max);
		const float db = DB_TOP - (fy - top) / (bottom - top) * (DB_TOP - DB_BOTTOM);
		const int nq = std::clamp(int(std::lround(64 + db * 16.0f / 6.0206f)), pq.min, pq.max);
		if (nc != vc)
			br.send(m.set(pc, part, nc));
		if (nq != vq)
			br.send(m.set(pq, part, nq));
	}

	// 描く
	dl->AddRectFilled(pos, ImVec2(pos.x + w, pos.y + h), col(hovered || active ? ImGuiCol_FrameBgHovered : ImGuiCol_FrameBg), 3.0f);
	if (known) {
		// 目安の線。0 dB と、音色のままのカットオフ
		const ImU32 guide = col(ImGuiCol_TextDisabled, 0.35f);
		dl->AddLine(ImVec2(x0, y0db), ImVec2(x1, y0db), guide);
		const float xmid = x0 + (x1 - x0) * 64.0f / 127.0f;
		dl->AddLine(ImVec2(xmid, top), ImVec2(xmid, bottom), guide);

		const float q = std::pow(2.0f, float(vq - 64) / 16.0f);
		const int n = std::max(8, int(x1 - x0) / 2);
		std::vector<ImVec2> pts;
		pts.reserve(n + 1);
		for (int i = 0; i <= n; i++) {
			const float x = x0 + (x1 - x0) * float(i) / float(n);
			const float r = std::pow(2.0f, (x - xc) / (x1 - x0) * 8.0f);          // 周波数 / カットオフ
			const float r2 = r * r;
			const float mag = 1.0f / std::sqrt((1 - r2) * (1 - r2) + r2 / (q * q));
			pts.push_back(ImVec2(x, y_of(20.0f * std::log10(std::max(mag, 1e-4f)))));
		}
		const ImU32 line = col(ImGuiCol_SliderGrabActive);
		dl->PathClear();
		dl->PathLineTo(ImVec2(x0, bottom));
		for (const ImVec2 &p : pts) dl->PathLineTo(p);
		dl->PathLineTo(ImVec2(x1, bottom));
		dl->PathFillConcave(col(ImGuiCol_SliderGrab, 0.25f));
		dl->PushClipRect(pos, ImVec2(pos.x + w, pos.y + h), true);
		dl->AddPolyline(pts.data(), int(pts.size()), line, 0, std::max(1.5f, fs * 0.1f));
		dl->PopClipRect();
		const float r = std::max(2.5f, fs * 0.22f);
		dl->AddCircleFilled(ImVec2(xc, yq), active ? r * 1.4f : r, active ? col(ImGuiCol_Text) : line);
	} else {
		const ImVec2 ts = ImGui::CalcTextSize("--");
		dl->AddText(ImVec2(pos.x + (w - ts.x) * 0.5f, pos.y + (h - ts.y) * 0.5f), col(ImGuiCol_TextDisabled), "--");
	}

	if ((hovered || active) && known)
		ImGui::SetItemTooltip("Cutoff %s   Resonance %s\n点をつまんで、横でカットオフ（右へ明るく）、縦でレゾナンス（上へ強く）%s",
		                      xg::format(pc, vc).c_str(), xg::format(pq, vq).c_str(), compact ? BIG_HINT : "");
	ImGui::PopID();
}

namespace {

using eq::HZ;
using eq::hz_text;
using eq::t_of_hz;
using eq::hz_of_t;
using eq::index_near;
using eq::band_db;
using band_shape = eq::shape;

struct eq_band {
	band_shape shape;
	const xg::param *gain, *freq, *q;     // q は無ければ nullptr
	int part;
	int vg, vf, vq;
	bool known;
};

// EQ の絵の 1 マス。帯ごとの点をつまんで、横で周波数、縦でゲイン。ホイールで Q（あれば）。
// 戻り値はつかんでいる帯（無ければ -1）
int eq_plot(const char *id, eq_band *bands, int n, xg::model &m, bridge &br, float w, float h, const char *tip)
{
	ImGuiIO &io = ImGui::GetIO();
	const float fs = ImGui::GetFontSize();
	ImDrawList *dl = ImGui::GetWindowDrawList();
	bool known = true;
	for (int i = 0; i < n; i++) {
		eq_band &b = bands[i];
		b.known = m.get(*b.gain, b.part, b.vg) && m.get(*b.freq, b.part, b.vf) && (!b.q || m.get(*b.q, b.part, b.vq));
		known &= b.known;
	}

	ImGui::PushID(id);
	const ImVec2 pos = ImGui::GetCursorScreenPos();
	ImGui::InvisibleButton("##eq", ImVec2(w, h), ImGuiButtonFlags_MouseButtonLeft);
	const ImGuiID iid = ImGui::GetItemID();
	const bool hovered = ImGui::IsItemHovered();
	const bool active = ImGui::IsItemActive();

	const float pad = fs * 0.25f;
	const float x0 = pos.x + pad, x1 = pos.x + w - pad;
	const float top = pos.y + pad, bottom = pos.y + h - pad;
	const float DB = 15.0f;
	auto x_of = [&](float hz) { return x0 + (x1 - x0) * t_of_hz(hz); };
	auto y_of = [&](float db) { return (top + bottom) * 0.5f - (bottom - top) * 0.5f * std::clamp(db, -DB, DB) / DB; };
	auto handle = [&](const eq_band &b) { return ImVec2(x_of(float(HZ[b.vf])), y_of(float(b.vg - 64))); };

	// 押した瞬間に一番近い点を選ぶ。ずれを覚えて、点が指に飛ばないようにする
	ImGuiStorage *st = ImGui::GetStateStorage();
	int grab = st->GetInt(iid, -1);
	float gx = st->GetFloat(iid + 1, 0.0f), gy = st->GetFloat(iid + 2, 0.0f);
	auto nearest = [&]() {
		int best = -1; float bd = 1e9f;
		for (int i = 0; i < n; i++) {
			if (!bands[i].known) continue;
			const ImVec2 hp = handle(bands[i]);
			const float d = (hp.x - io.MousePos.x) * (hp.x - io.MousePos.x) + (hp.y - io.MousePos.y) * (hp.y - io.MousePos.y);
			if (d < bd) { bd = d; best = i; }
		}
		return best;
	};
	if (ImGui::IsItemActivated() && known) {
		grab = nearest();
		if (grab >= 0) {
			const ImVec2 hp = handle(bands[grab]);
			gx = hp.x - io.MousePos.x;
			gy = hp.y - io.MousePos.y;
		}
	}
	if (!active)
		grab = -1;
	st->SetInt(iid, grab);
	st->SetFloat(iid + 1, gx);
	st->SetFloat(iid + 2, gy);
	if (active && grab >= 0 && (io.MouseDelta.x != 0.0f || io.MouseDelta.y != 0.0f)) {
		const eq_band &b = bands[grab];
		const float t = (io.MousePos.x + gx - x0) / (x1 - x0);
		const int nf = index_near(t, b.freq->min, b.freq->max);
		const float db = -((io.MousePos.y + gy) - (top + bottom) * 0.5f) / ((bottom - top) * 0.5f) * DB;
		const int ng = std::clamp(int(std::lround(64 + db)), b.gain->min, b.gain->max);
		if (nf != b.vf) br.send(m.set(*b.freq, b.part, nf));
		if (ng != b.vg) br.send(m.set(*b.gain, b.part, ng));
	}
	// ホイールで Q。カーソルに一番近い帯
	const int hot = hovered && !active ? nearest() : grab;
	if (hovered && known && hot >= 0 && bands[hot].q) {
		ImGui::SetItemKeyOwner(ImGuiKey_MouseWheelY);
		if (io.MouseWheel != 0.0f) {
			const eq_band &b = bands[hot];
			const int nq = std::clamp(b.vq + (io.MouseWheel > 0 ? 1 : -1) * (io.KeyCtrl ? 10 : 2), b.q->min, b.q->max);
			if (nq != b.vq) br.send(m.set(*b.q, b.part, nq));
		}
	}

	dl->AddRectFilled(pos, ImVec2(pos.x + w, pos.y + h), col(hovered || active ? ImGuiCol_FrameBgHovered : ImGuiCol_FrameBg), 3.0f);
	if (known) {
		const ImU32 guide = col(ImGuiCol_TextDisabled, 0.35f);
		dl->AddLine(ImVec2(x0, y_of(0)), ImVec2(x1, y_of(0)), guide);
		for (float hz : { 100.0f, 1000.0f, 10000.0f })
			dl->AddLine(ImVec2(x_of(hz), top), ImVec2(x_of(hz), bottom), guide);
		const int np = std::max(8, int(x1 - x0) / 2);
		std::vector<ImVec2> pts;
		pts.reserve(np + 1);
		for (int i = 0; i <= np; i++) {
			const float t = float(i) / float(np);
			const float f = hz_of_t(t);
			float db = 0;
			for (int k = 0; k < n; k++)
				db += band_db(bands[k].shape, float(bands[k].vg - 64), float(HZ[bands[k].vf]),
				              bands[k].q ? bands[k].vq / 10.0f : 0.7f, f);
			pts.push_back(ImVec2(x0 + (x1 - x0) * t, y_of(db)));
		}
		const ImU32 line = col(ImGuiCol_SliderGrabActive);
		dl->PathClear();
		dl->PathLineTo(ImVec2(x0, y_of(0)));
		for (const ImVec2 &p : pts) dl->PathLineTo(p);
		dl->PathLineTo(ImVec2(x1, y_of(0)));
		dl->PathFillConcave(col(ImGuiCol_SliderGrab, 0.25f));
		dl->AddPolyline(pts.data(), int(pts.size()), line, 0, std::max(1.5f, fs * 0.1f));
		const float r = std::max(2.5f, fs * 0.2f);
		for (int k = 0; k < n; k++) {
			const ImVec2 hp = handle(bands[k]);
			const bool on = k == grab || (k == hot && hovered);
			dl->AddCircleFilled(hp, on ? r * 1.4f : r, on ? col(ImGuiCol_Text) : line);
		}
	} else {
		const ImVec2 ts = ImGui::CalcTextSize("--");
		dl->AddText(ImVec2(pos.x + (w - ts.x) * 0.5f, pos.y + (h - ts.y) * 0.5f), col(ImGuiCol_TextDisabled), "--");
	}

	if ((hovered || active) && known) {
		std::string text;
		for (int k = 0; k < n; k++) {
			char buf[96];
			const eq_band &b = bands[k];
			std::snprintf(buf, sizeof(buf), "%s%d: %sHz %+ddB", k ? "\n" : "", k + 1, hz_text(b.vf).c_str(), b.vg - 64);
			text += buf;
			if (b.q) {
				std::snprintf(buf, sizeof(buf), "  Q %.1f", b.vq / 10.0);
				text += buf;
			}
		}
		ImGui::SetItemTooltip("%s\n%s", text.c_str(), tip);
	}
	ImGui::PopID();
	return grab;
}

} // namespace


// パートの EQ の 1 マス。低音（シェルフ）と高音（シェルフ）の 2 つの点
void overview::eq_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact)
{
	eq_band bands[] = {
		{ band_shape::low_shelf,  &P("part.eq_bass_gain"),   &P("part.eq_bass_freq"),   nullptr, part, 64, 12, 0, false },
		{ band_shape::high_shelf, &P("part.eq_treble_gain"), &P("part.eq_treble_freq"), nullptr, part, 64, 54, 0, false },
	};
	eq_plot("eq", bands, 2, m, br, w, h, compact ? "点をつまんで、横で周波数、縦でゲイン（1 が低音、2 が高音）\nダブルクリックで大きな窓に出す"
	                                            : "点をつまんで、横で周波数、縦でゲイン（1 が低音、2 が高音）");
}


// マスター EQ の 1 マス。5 つの帯。1 と 5 は形（シェルフ／ピーク）を右クリックで選ぶ
void overview::master_eq_cell(xg::model &m, bridge &br, float h)
{
	const float w = ImGui::GetContentRegionAvail().x;
	int s1 = 0, s5 = 0;
	m.get(P("master_eq.shape1"), 0, s1);
	m.get(P("master_eq.shape5"), 0, s5);
	eq_band bands[] = {
		{ s1 ? band_shape::peak : band_shape::low_shelf,  &P("master_eq.gain1"), &P("master_eq.freq1"), &P("master_eq.q1"), 0, 64, 12, 7, false },
		{ band_shape::peak,                               &P("master_eq.gain2"), &P("master_eq.freq2"), &P("master_eq.q2"), 0, 64, 28, 7, false },
		{ band_shape::peak,                               &P("master_eq.gain3"), &P("master_eq.freq3"), &P("master_eq.q3"), 0, 64, 34, 7, false },
		{ band_shape::peak,                               &P("master_eq.gain4"), &P("master_eq.freq4"), &P("master_eq.q4"), 0, 64, 46, 7, false },
		{ s5 ? band_shape::peak : band_shape::high_shelf, &P("master_eq.gain5"), &P("master_eq.freq5"), &P("master_eq.q5"), 0, 64, 52, 7, false },
	};
	eq_plot("meq", bands, 5, m, br, w, h,
	        "点をつまんで、横で周波数、縦でゲイン。ホイールで幅（Q）。右クリックで種類と、両端の帯の形");
	if (ImGui::BeginPopupContextItem("meqmenu", ImGuiPopupFlags_MouseButtonRight)) {
		ImGui::TextDisabled("マスター EQ");
		ImGui::Separator();
		int type = 0;
		m.get(P("master_eq.type"), 0, type);
		const xg::param &pt = P("master_eq.type");
		for (int t = pt.min; t <= pt.max; t++)
			if (ImGui::MenuItem(pt.choices[t], nullptr, t == type))
				br.send(m.set(pt, 0, t));
		ImGui::Separator();
		if (ImGui::MenuItem("帯 1 をピークにする", nullptr, s1 == 1))
			br.send(m.set(P("master_eq.shape1"), 0, s1 ? 0 : 1));
		if (ImGui::MenuItem("帯 5 をピークにする", nullptr, s5 == 1))
			br.send(m.set(P("master_eq.shape5"), 0, s5 ? 0 : 1));
		ImGui::Separator();
		ImGui::TextDisabled("種類を選ぶと、firmware が 5 つの帯を\nその種類の値に書き換える");
		ImGui::EndPopup();
	}
}


// ビブラートの 1 マス。弾いてからの揺れの形。平らな所が掛かり始めるまで（Delay）、
// そのあとの波の山の点をつまんで、横で速さ（山が近いほど速い）、縦で深さ。
// 平らな所の終わりの点を横に動かすと Delay。どれも音色の元の値に対する増減（64 が音色のまま）で、
// 形は 2 の (値 - 64) / 24 乗で伸び縮みさせた見た目だけのもの
void overview::vib_cell(int part, xg::model &m, bridge &br, float w, float h, bool compact)
{
	ImGuiIO &io = ImGui::GetIO();
	const float fs = ImGui::GetFontSize();
	ImDrawList *dl = ImGui::GetWindowDrawList();
	const xg::param &pr = P("part.vib_rate"), &pd = P("part.vib_depth"), &pl = P("part.vib_delay");
	int vr = 64, vd = 64, vl = 64;
	const bool known = m.get(pr, part, vr) && m.get(pd, part, vd) && m.get(pl, part, vl);

	ImGui::PushID("vib");
	const ImVec2 pos = ImGui::GetCursorScreenPos();
	ImGui::InvisibleButton("##vib", ImVec2(w, h), ImGuiButtonFlags_MouseButtonLeft);
	const ImGuiID id = ImGui::GetItemID();
	const bool hovered = ImGui::IsItemHovered();
	const bool active = ImGui::IsItemActive();

	const float pad = fs * 0.25f;
	const float x0 = pos.x + pad, x1 = pos.x + w - pad;
	const float top = pos.y + pad, bottom = pos.y + h - pad;
	const float mid = (top + bottom) * 0.5f, half = (bottom - top) * 0.5f;
	const float unit = (x1 - x0) / 5.0f;
	auto scale = [](int v) { return std::pow(2.0f, float(v - 64) / 24.0f); };
	const float delay = std::min(unit * scale(vl), (x1 - x0) * 0.8f);
	const float period = std::clamp(unit * 0.8f / scale(vr), 3.0f, (x1 - x0));
	const float amp = std::min(half * 0.45f * scale(vd), half);
	const float xd = x0 + delay;
	const ImVec2 crest(xd + period * 0.25f, mid - amp);

	ImGuiStorage *st = ImGui::GetStateStorage();
	int grab = st->GetInt(id, -1);
	float gx = st->GetFloat(id + 1, 0.0f), gy = st->GetFloat(id + 2, 0.0f);
	if (ImGui::IsItemActivated() && known) {
		const float dd = std::fabs(io.MousePos.x - xd) + std::fabs(io.MousePos.y - mid);
		const float dc = std::fabs(io.MousePos.x - crest.x) + std::fabs(io.MousePos.y - crest.y);
		grab = dd < dc ? 0 : 1;
		gx = (grab == 0 ? xd : crest.x) - io.MousePos.x;
		gy = (grab == 0 ? mid : crest.y) - io.MousePos.y;
	}
	if (!active)
		grab = -1;
	st->SetInt(id, grab);
	st->SetFloat(id + 1, gx);
	st->SetFloat(id + 2, gy);
	if (active && known && grab >= 0 && (io.MouseDelta.x != 0.0f || io.MouseDelta.y != 0.0f)) {
		auto value = [](float ratio, const xg::param &p) {
			return std::clamp(int(std::lround(64 + 24 * std::log2(std::max(ratio, 1e-3f)))), p.min, p.max);
		};
		const float fx = io.MousePos.x + gx, fy = io.MousePos.y + gy;
		if (grab == 0) {
			const int nl = value((fx - x0) / unit, pl);
			if (nl != vl) br.send(m.set(pl, part, nl));
		} else {
			const int nr = value(unit * 0.8f / std::max(1.0f, (fx - xd) * 4.0f), pr);
			const int nd = value((mid - fy) / (half * 0.45f), pd);
			if (nr != vr) br.send(m.set(pr, part, nr));
			if (nd != vd) br.send(m.set(pd, part, nd));
		}
	}

	dl->AddRectFilled(pos, ImVec2(pos.x + w, pos.y + h), col(hovered || active ? ImGuiCol_FrameBgHovered : ImGuiCol_FrameBg), 3.0f);
	if (known) {
		const ImU32 line = col(ImGuiCol_SliderGrabActive);
		dl->AddLine(ImVec2(x0, mid), ImVec2(x1, mid), col(ImGuiCol_TextDisabled, 0.35f));
		std::vector<ImVec2> pts;
		pts.push_back(ImVec2(x0, mid));
		pts.push_back(ImVec2(xd, mid));
		for (float x = xd + 1.0f; x <= x1; x += 1.0f)
			pts.push_back(ImVec2(x, mid - amp * std::sin((x - xd) / period * 2.0f * IM_PI)));
		dl->PushClipRect(pos, ImVec2(pos.x + w, pos.y + h), true);
		dl->AddPolyline(pts.data(), int(pts.size()), line, 0, std::max(1.5f, fs * 0.1f));
		const float r = std::max(2.5f, fs * 0.2f);
		dl->AddCircleFilled(ImVec2(xd, mid), grab == 0 ? r * 1.4f : r, grab == 0 ? col(ImGuiCol_Text) : line);
		dl->AddCircleFilled(crest, grab == 1 ? r * 1.4f : r, grab == 1 ? col(ImGuiCol_Text) : line);
		dl->PopClipRect();
	} else {
		const ImVec2 ts = ImGui::CalcTextSize("--");
		dl->AddText(ImVec2(pos.x + (w - ts.x) * 0.5f, pos.y + (h - ts.y) * 0.5f), col(ImGuiCol_TextDisabled), "--");
	}
	if ((hovered || active) && known)
		ImGui::SetItemTooltip("Rate %s   Depth %s   Delay %s\n波の山の点: 横で速さ、縦で深さ\n平らな所の終わりの点: 横で掛かり始めるまでの時間%s",
		                      xg::format(pr, vr).c_str(), xg::format(pd, vd).c_str(), xg::format(pl, vl).c_str(),
		                      compact ? BIG_HINT : "");
	ImGui::PopID();
}


void overview::row(int part, xg::model &m, const xg_snapshot &ram, bridge &br, float h)
{
	const float fs = ImGui::GetFontSize();
	ImDrawList *dl = ImGui::GetWindowDrawList();
	ImGui::PushID(part);

	// ---- パートと音色
	ImGui::TableNextColumn();
	{
		const ImVec2 pos = ImGui::GetCursorScreenPos();
		const float w = ImGui::GetContentRegionAvail().x;
		ImGui::SetNextItemAllowOverlap();
		if (ImGui::InvisibleButton("##name", ImVec2(w, h), ImGuiButtonFlags_MouseButtonLeft | ImGuiButtonFlags_MouseButtonRight))
			m_part = part;
		if (ImGui::IsItemClicked(ImGuiMouseButton_Right))
			m_part = part;
		if (ImGui::BeginPopupContextItem("program")) {
			program_menu(part, m, &ram, br);
			ImGui::EndPopup();
		}
		if (m_part == part)
			dl->AddRectFilled(pos, ImVec2(pos.x + w, pos.y + h), col(ImGuiCol_Header));
		else if (ImGui::IsItemHovered())
			dl->AddRectFilled(pos, ImVec2(pos.x + w, pos.y + h), col(ImGuiCol_HeaderHovered, 0.4f));

		int msb = 0, lsb = 0, prog = 0, rcv = 0;
		const bool voice = m.get(P("part.bank_msb"), part, msb) && m.get(P("part.bank_lsb"), part, lsb) &&
		                   m.get(P("part.program"), part, prog);
		bool has_rcv = m.get(P("part.rcv_channel"), part, rcv);
		const bool silenced = m_saved_rcv[part] >= 0;
		if (silenced) {
			rcv = m_saved_rcv[part];                 // 表示は元のチャンネル
			has_rcv = true;
		}
		const std::string name = part_name(part);
		dl->AddText(ImVec2(pos.x + fs * 0.3f, pos.y + fs * 0.1f), silenced ? col(ImGuiCol_TextDisabled) : col(ImGuiCol_Text), name.c_str());
		// 音色の名前と楽器の絵。利用者の ROM から読めれば MU2000 の本当の名前、
		// 読めなければ GM の名前（xg/voices.h）
		std::string vt = voice ? voice_text(msb, lsb, prog) : "--";
		const xg::voice_rom *vr = voices();
		const u8 *blk = ram.parts[part];
		if (voice && vr) {
			const std::string real = vr->name(blk, msb, prog);
			if (!real.empty()) {
				char buf[40];
				std::snprintf(buf, sizeof(buf), "%3d  %s", prog + 1, real.c_str());
				vt = buf;
			}
		}
		dl->PushClipRect(pos, ImVec2(pos.x + w, pos.y + h), true);
		const float icon_x = pos.x + fs * 2.2f;
		// 実機の LCD に寄せて、横に 2 倍（1 ドットが横 2 : 縦 1）
		const float dot = std::max(1.0f, std::floor((h - fs * 0.3f) / 16.0f));
		const float dot_w = dot * 2;
		u16 rows[16];
		if (vr) {
			// パネルの LCD と同じ色（draw.h の LCD_BACK / LCD_GHOST / LCD_DOT）。
			// 絵が引けないもの（ROM の版が違うなど）も、LCD の枠だけ出して並びを揃える
			static const ImU32 LCD_BACK  = IM_COL32(150, 205, 45, 255);
			static const ImU32 LCD_GHOST = IM_COL32(140, 194, 44, 255);
			static const ImU32 LCD_DOT   = IM_COL32(18, 22, 14, 255);
			const bool has = voice && vr->icon(blk, msb, prog, rows);
			const float top = pos.y + (h - dot * 16) * 0.5f;
			const float frame = std::max(1.0f, dot);
			dl->AddRectFilled(ImVec2(icon_x - frame, top - frame),
			                  ImVec2(icon_x + dot_w * 16 + frame, top + dot * 16 + frame), LCD_BACK, 2.0f);
			for (int y = 0; y < 16; y++)
				for (int x = 0; x < 16; x++) {
					const bool on = has && ((rows[y] >> (15 - x)) & 1);
					dl->AddRectFilled(ImVec2(icon_x + x * dot_w, top + y * dot),
					                  ImVec2(icon_x + (x + 1) * dot_w, top + (y + 1) * dot),
					                  on ? LCD_DOT : LCD_GHOST);
				}
		}
		const float text_x = icon_x + dot_w * 16 + fs * 0.4f;
		dl->AddText(ImVec2(text_x, pos.y + fs * 0.1f), col(ImGuiCol_Text), vt.c_str());
		char sub[64];
		if (silenced)
			std::snprintf(sub, sizeof(sub), "受信 %s（%s）", channel_name(rcv).c_str(), m_mute[part] ? "ミュート" : "ソロの外");
		else
			std::snprintf(sub, sizeof(sub), "受信 %s   M %d  L %d", has_rcv ? channel_name(rcv).c_str() : "--", msb, lsb);
		dl->AddText(ImVec2(text_x, pos.y + fs * 1.15f), col(ImGuiCol_TextDisabled), sub);
		dl->PopClipRect();
		mute_buttons(part, pos.x, pos.y, w, h);
	}

	// 受信チャンネルから、見張りの口×チャンネル（ミュート中は元のチャンネル）
	int rcv = 127;
	m.get(P("part.rcv_channel"), part, rcv);
	if (m_saved_rcv[part] >= 0)
		rcv = m_saved_rcv[part];
	const int slot = rcv >= 0 && rcv < PARTS ? rcv : -1;

	// ---- VEL メーター
	ImGui::TableNextColumn();
	{
		if (slot >= 0 && ram.note_ons[slot] != m_seen_ons[part]) {
			m_seen_ons[part] = ram.note_ons[slot];
			m_level[part] = std::max(m_level[part], ram.velocity[slot] / 127.0f);
		}
		m_level[part] = std::max(0.0f, m_level[part] - ImGui::GetIO().DeltaTime * 1.6f);
		const ImVec2 pos = ImGui::GetCursorScreenPos();
		const float w = ImGui::GetContentRegionAvail().x;
		ImGui::Dummy(ImVec2(w, h));
		const float pad = fs * 0.2f;
		const ImVec2 a(pos.x + pad, pos.y + pad), b(pos.x + w - pad, pos.y + h - pad);
		dl->AddRectFilled(a, b, col(ImGuiCol_FrameBg));
		const float top = b.y - (b.y - a.y) * m_level[part];
		dl->AddRectFilled(ImVec2(a.x, top), b, NOTE_ON);
	}

	// ---- 値の棒
	for (const column &c : COLUMNS) {
		ImGui::TableNextColumn();
		cell(c, part, m, ram, br, ImGui::GetContentRegionAvail().x, h);
	}

	// ---- 鍵盤。128 鍵を全部並べる
	ImGui::TableNextColumn();
	{
		const ImVec2 pos = ImGui::GetCursorScreenPos();
		const float w = ImGui::GetContentRegionAvail().x;
		// 押すと鳴らす（左でも右でも）。押したまま横に動かすと鍵が替わる。離すとノートオフ。
		// 送り先はこのパートの受信チャンネル（口 B なら口 B へ）
		ImGui::InvisibleButton("##keys", ImVec2(w, h), ImGuiButtonFlags_MouseButtonLeft | ImGuiButtonFlags_MouseButtonRight);
		const bool down = ImGui::IsItemActive() && slot >= 0 &&
		                  (ImGui::IsMouseDown(ImGuiMouseButton_Left) || ImGui::IsMouseDown(ImGuiMouseButton_Right));
		int vel = 100;
		const int want = down ? key_at(pos, w, h, ImGui::GetIO().MousePos, vel) : -1;
		if (want != m_playing[part]) {
			auto send = [&](const u8 msg[3]) {
				br.send_port(m_playing_slot[part] / 16, msg, 3);
			};
			if (m_playing[part] >= 0) {
				const u8 off[3] = { u8(0x80 | (m_playing_slot[part] & 15)), u8(m_playing[part]), 64 };
				send(off);
			}
			m_playing[part] = want;
			if (want >= 0) {
				m_playing_slot[part] = slot;
				const u8 on[3] = { u8(0x90 | (slot & 15)), u8(want), u8(vel) };
				send(on);
			}
		}
		if (ImGui::IsItemHovered() && !down && slot >= 0)
			ImGui::SetItemTooltip("押すと鳴らす（左右どちらのボタンでも）。下ほど強く");
		draw_keys(dl, pos, w, h, [&](int note) -> ImU32 {
			return slot >= 0 && ((ram.notes[slot][note >> 6] >> (note & 63)) & 1) ? NOTE_ON : 0;
		});
	}

	ImGui::PopID();
}


namespace {

const overview::column &column_of(const char *title)
{
	for (const overview::column &c : COLUMNS)
		if (!std::strcmp(c.title, title))
			return c;
	return COLUMNS[0];
}

// 種類の品書き（分類 → 系統 → LSB 違い）。今の種類に印
void type_menu(const std::vector<xg::fx_type> &types, const char *key, xg::model &m, bridge &br)
{
	int cur = 0, chosen = 0;
	const bool has = m.get(P(key), 0, cur);
	if (fx_type_menu(types, has ? cur : -1, chosen))
		br.send(m.set(P(key), 0, chosen));
}

// 掛け先のパートの品書き（A1-B16 と OFF）
void part_menu(const char *key, xg::model &m, bridge &br, bool with_off)
{
	int cur = 127;
	m.get(P(key), 0, cur);
	static const char *PORT_MENU[4] = { "A1-A16", "B1-B16", "C1-C16", "D1-D16" };
	for (int port = 0; port < PARTS / 16; port++) {
		if (!ImGui::BeginMenu(PORT_MENU[port]))
			continue;
		for (int i = port * 16; i < port * 16 + 16; i++)
			if (ImGui::MenuItem(part_name(i).c_str(), nullptr, cur == i))
				br.send(m.set(P(key), 0, i));
		ImGui::EndMenu();
	}
	// 64 パートの後ろに A/D INPUT が 2 つ並ぶ（実機で確かめた）
	for (int i = PARTS; i < PARTS + 2; i++)
		if (ImGui::MenuItem(part_name(i).c_str(), nullptr, cur == i))
			br.send(m.set(P(key), 0, i));
	if (with_off && ImGui::MenuItem("OFF（どのパートにも掛けない）", nullptr, cur >= PARTS + 2))
		br.send(m.set(P(key), 0, 127));
}

} // namespace


// システムのエフェクト（リバーブ・コーラス・バリエーション）の 1 マス。
// 上の行が種類（右クリックで選ぶ）、下が戻り量の棒
void overview::system_fx_cell(const char *title, const std::vector<xg::fx_type> &types, const char *type_key,
                              const char *return_col, bool variation, xg::model &m, const xg_snapshot &ram,
                              bridge &br, float h)
{
	const float fs = ImGui::GetFontSize();
	ImDrawList *dl = ImGui::GetWindowDrawList();
	const ImVec2 pos = ImGui::GetCursorScreenPos();
	const float w = ImGui::GetContentRegionAvail().x;
	const float line = fs * 1.1f;

	ImGui::PushID(title);
	ImGui::InvisibleButton("##type", ImVec2(w, line), ImGuiButtonFlags_MouseButtonLeft | ImGuiButtonFlags_MouseButtonRight);
	const bool hot = ImGui::IsItemHovered();
	if (ImGui::BeginPopupContextItem("typemenu", ImGuiPopupFlags_MouseButtonRight)) {
		ImGui::TextDisabled("%s の種類", title);
		ImGui::Separator();
		type_menu(types, type_key, m, br);
		if (variation) {
			int conn = 1;
			m.get(P("variation.connect"), 0, conn);
			ImGui::Separator();
			ImGui::TextDisabled("接続");
			if (ImGui::MenuItem("SYSTEM（全パートから送る）", nullptr, conn == 1))
				br.send(m.set(P("variation.connect"), 0, 1));
			if (ImGui::BeginMenu("INSERTION（1 つのパートに掛ける）")) {
				part_menu("variation.part", m, br, false);
				ImGui::EndMenu();
			}
			if (conn == 0 && ImGui::IsItemHovered())
				ImGui::SetTooltip("選んだパートに掛かる");
		}
		ImGui::EndPopup();
	}
	if (ImGui::IsItemHovered() && !ImGui::IsPopupOpen("typemenu"))
		ImGui::SetItemTooltip("右クリックで種類を選ぶ");

	int type = 0, conn = 1, vpart = 127;
	std::string name = m.get(P(type_key), 0, type) ? xg::fx_name(type) : "--";
	bool dim = false;
	if (variation && m.get(P("variation.connect"), 0, conn) && conn == 0) {
		m.get(P("variation.part"), 0, vpart);
		name += vpart < PARTS + 2 ? " → " + part_name(vpart) : " → OFF";
		dim = false;
	}
	dl->PushClipRect(pos, ImVec2(pos.x + w, pos.y + line), true);
	if (hot)
		dl->AddRectFilled(pos, ImVec2(pos.x + w, pos.y + line), col(ImGuiCol_HeaderHovered, 0.35f));
	dl->AddText(ImVec2(pos.x + fs * 0.3f, pos.y + (line - fs) * 0.5f), dim ? col(ImGuiCol_TextDisabled) : col(ImGuiCol_Text), name.c_str());
	dl->PopClipRect();
	ImGui::PopID();

	// 戻り量
	ImGui::SetCursorScreenPos(ImVec2(pos.x, pos.y + line));
	cell(column_of(return_col), -1, m, ram, br, w, h - line);
}


// インサーション（とバリエーション）の 1 マス。上の行が印と種類、下が掛け先。
// 右クリックで種類と掛け先、印をつかんでパートの INS 欄に落とすと掛け先が変わる
void overview::insertion_cell(int slot_index, xg::model &m, bridge &br, float h)
{
	const float fs = ImGui::GetFontSize();
	ImDrawList *dl = ImGui::GetWindowDrawList();
	const fx_slot &f = FX_SLOTS[slot_index];
	const ImVec2 pos = ImGui::GetCursorScreenPos();
	const float w = ImGui::GetContentRegionAvail().x;

	ImGui::PushID(f.id);
	ImGui::InvisibleButton("##slot", ImVec2(w, h), ImGuiButtonFlags_MouseButtonLeft | ImGuiButtonFlags_MouseButtonRight);
	const bool hot = ImGui::IsItemHovered() || ImGui::IsItemActive();
	int type = 0;
	const bool has_type = m.get(P(f.type_key), 0, type);
	const std::string name = has_type ? xg::fx_name(type) : "--";
	const int where = fx_target(f, m);
	if (f.id <= 4 && ImGui::IsItemHovered() && ImGui::IsMouseDoubleClicked(ImGuiMouseButton_Left))
		request_fx(f.id);                        // 設定の窓を出す
	if (ImGui::BeginDragDropSource()) {
		ImGui::SetDragDropPayload(DRAG_FX, &f.id, sizeof(f.id));
		ImGui::Text("%s（%s）を掛けるパートの INS 欄へ", f.title, name.c_str());
		ImGui::EndDragDropSource();
	}
	if (ImGui::BeginPopupContextItem("slotmenu", ImGuiPopupFlags_MouseButtonRight)) {
		ImGui::TextDisabled("%s", f.title);
		ImGui::Separator();
		if (ImGui::BeginMenu("種類")) {
			type_menu(xg::ins_types(), f.type_key, m, br);
			ImGui::EndMenu();
		}
		if (ImGui::BeginMenu("掛けるパート")) {
			part_menu(f.part_key, m, br, true);
			ImGui::EndMenu();
		}
		ImGui::Separator();
		ImGui::TextDisabled("つかんでパートの INS 欄に落としても掛けられる。\n種類が NO EFFECT のまま掛けると、そのパートの音が消える");
		ImGui::EndPopup();
	}
	if (ImGui::IsItemHovered() && !ImGui::IsDragDropActive())
		ImGui::SetItemTooltip("%s: %s → %s\nダブルクリックで設定の窓・右クリックで種類と掛けるパート・つかんでパートの INS 欄へ",
		                      f.title, name.c_str(), where >= 0 ? part_name(where).c_str() : "OFF");
	ImGui::PopID();

	dl->PushClipRect(pos, ImVec2(pos.x + w, pos.y + h), true);
	if (hot)
		dl->AddRectFilled(pos, ImVec2(pos.x + w, pos.y + h), col(ImGuiCol_HeaderHovered, 0.35f));
	const float bw = fs * 1.0f;
	const float y = pos.y + fs * 0.1f;
	const ImU32 badge = where >= 0 ? f.color : col(ImGuiCol_TextDisabled, 0.5f);
	dl->AddRectFilled(ImVec2(pos.x + fs * 0.2f, y + 1), ImVec2(pos.x + fs * 0.2f + bw, y + fs), badge, 3.0f);
	const ImVec2 ms = ImGui::CalcTextSize(f.mark);
	dl->AddText(ImVec2(pos.x + fs * 0.2f + (bw - ms.x) * 0.5f, y), IM_COL32(20, 20, 20, 255), f.mark);
	dl->AddText(ImVec2(pos.x + fs * 1.5f, y), where >= 0 ? col(ImGuiCol_Text) : col(ImGuiCol_TextDisabled), name.c_str());
	const std::string to = where >= 0 ? "→ " + part_name(where) : "OFF";
	dl->AddText(ImVec2(pos.x + fs * 1.5f, y + fs * 1.05f), col(ImGuiCol_TextDisabled), to.c_str());
	dl->PopClipRect();
}


// マスターの表。パートの表とは見出しを分ける
void overview::master_pane(xg::model &m, const xg_snapshot &ram, bridge &br)
{
	const float fs = ImGui::GetFontSize();
	const float h = fs * 2.3f;
	ImDrawList *dl = ImGui::GetWindowDrawList();

	const ImGuiTableFlags flags = ImGuiTableFlags_RowBg | ImGuiTableFlags_BordersInnerV | ImGuiTableFlags_BordersOuterH |
	                              ImGuiTableFlags_SizingFixedFit | ImGuiTableFlags_PadOuterX;
	constexpr int NCOL = 11;
	if (!ImGui::BeginTable("master", NCOL, flags))
		return;
	ImGui::TableSetupColumn("マスター", ImGuiTableColumnFlags_WidthFixed, fs * 18.5f);
	ImGui::TableSetupColumn("M.VOL", ImGuiTableColumnFlags_WidthFixed, fs * 3.4f);
	// 音の流れの順（インサーション → バリエーション → コーラス → リバーブ → マスター EQ）
	ImGui::TableSetupColumn("INS 1", ImGuiTableColumnFlags_WidthFixed, fs * 7);
	ImGui::TableSetupColumn("INS 2", ImGuiTableColumnFlags_WidthFixed, fs * 7);
	ImGui::TableSetupColumn("INS 3", ImGuiTableColumnFlags_WidthFixed, fs * 7);
	ImGui::TableSetupColumn("INS 4", ImGuiTableColumnFlags_WidthFixed, fs * 7);
	ImGui::TableSetupColumn("VARIATION", ImGuiTableColumnFlags_WidthFixed, fs * 9.5f);
	ImGui::TableSetupColumn("CHORUS", ImGuiTableColumnFlags_WidthFixed, fs * 7.5f);
	ImGui::TableSetupColumn("REVERB", ImGuiTableColumnFlags_WidthFixed, fs * 7.5f);
	ImGui::TableSetupColumn("MASTER EQ", ImGuiTableColumnFlags_WidthFixed, fs * 11);
	ImGui::TableSetupColumn("##mkeys", ImGuiTableColumnFlags_WidthStretch);
	headers_with_help(NCOL);
	ImGui::TableNextRow(0, h);
	ImGui::PushID("master");

	// ---- 名前。移調とマスターチューンも
	ImGui::TableNextColumn();
	{
		const ImVec2 pos = ImGui::GetCursorScreenPos();
		const float w = ImGui::GetContentRegionAvail().x;
		ImGui::Dummy(ImVec2(w, h));
		dl->AddText(ImVec2(pos.x + fs * 0.3f, pos.y + fs * 0.1f), col(ImGuiCol_Text), "MASTER");
		int tr = 0x40, tune = 0x400;
		char sub[64];
		if (m.get(P("system.transpose"), 0, tr) && m.get(P("system.master_tune"), 0, tune))
			std::snprintf(sub, sizeof(sub), "Transpose %s   Tune %s",
			              xg::format(P("system.transpose"), tr).c_str(), xg::format(P("system.master_tune"), tune).c_str());
		else
			std::snprintf(sub, sizeof(sub), "--");
		dl->AddText(ImVec2(pos.x + fs * 0.3f, pos.y + fs * 1.15f), col(ImGuiCol_TextDisabled), sub);
	}

	ImGui::TableNextColumn();
	cell(column_of("VOL"), -1, m, ram, br, ImGui::GetContentRegionAvail().x, h);
	for (int i = 0; i < 4; i++) {
		ImGui::TableNextColumn();
		insertion_cell(i, m, br, h);
	}
	ImGui::TableNextColumn();
	system_fx_cell("バリエーション", xg::ins_types(), "variation.type", "VAR", true, m, ram, br, h);
	ImGui::TableNextColumn();
	system_fx_cell("コーラス", xg::cho_types(), "chorus.type", "CHO", false, m, ram, br, h);
	ImGui::TableNextColumn();
	system_fx_cell("リバーブ", xg::rev_types(), "reverb.type", "REV", false, m, ram, br, h);
	ImGui::TableNextColumn();
	master_eq_cell(m, br, h);

	// ---- 鍵盤。全パートで鳴っている鍵を重ねる。色はパートごと、重なったら混ぜる
	ImGui::TableNextColumn();
	{
		const ImVec2 pos = ImGui::GetCursorScreenPos();
		const float w = ImGui::GetContentRegionAvail().x;
		ImGui::Dummy(ImVec2(w, h));
		int slots[PARTS];
		for (int p = 0; p < PARTS; p++) {
			int rcv = 127;
			slots[p] = m.get(P("part.rcv_channel"), p, rcv) && rcv < PARTS ? rcv : -1;
		}
		draw_keys(dl, pos, w, h, [&](int note) -> ImU32 {
			int r = 0, g = 0, b = 0, n = 0;
			for (int p = 0; p < PARTS; p++) {
				const int sl = slots[p];
				if (sl < 0 || !((ram.notes[sl][note >> 6] >> (note & 63)) & 1))
					continue;
				const ImU32 c = part_color(p);
				r += (c >> IM_COL32_R_SHIFT) & 0xff;
				g += (c >> IM_COL32_G_SHIFT) & 0xff;
				b += (c >> IM_COL32_B_SHIFT) & 0xff;
				n++;
			}
			return n ? IM_COL32(r / n, g / n, b / n, 255) : 0;
		});
	}
	ImGui::PopID();
	ImGui::EndTable();
}


void overview::release_keys(bridge &br)
{
	for (int part = 0; part < PARTS; part++) {
		if (m_playing[part] < 0)
			continue;
		const u8 off[3] = { u8(0x80 | (m_playing_slot[part] & 15)), u8(m_playing[part]), 64 };
		br.send_port(m_playing_slot[part] / 16, off, 3);
		m_playing[part] = -1;
	}
}


void overview::mute_buttons(int part, float px, float py, float w, float h)
{
	const float fs = ImGui::GetFontSize();
	ImDrawList *dl = ImGui::GetWindowDrawList();
	const ImVec2 pos(px, py);
	const float bw = fs * 1.25f, bh = (h - fs * 0.3f) * 0.5f;
	const float x = pos.x + w - bw - fs * 0.15f;
	struct { const char *id, *mark; bool *on; ImU32 lit; float y; const char *tip; } b[] = {
		{ "##mute", "M", &m_mute[part], IM_COL32(230, 80, 60, 255),  pos.y + fs * 0.1f,
		  "ミュート（このパートを鳴らさない）" },
		{ "##solo", "S", &m_solo[part], IM_COL32(240, 200, 60, 255), pos.y + fs * 0.2f + bh,
		  "ソロ（S を入れたパートだけを鳴らす）" },
	};
	for (auto &e : b) {
		ImGui::SetCursorScreenPos(ImVec2(x, e.y));
		if (ImGui::InvisibleButton(e.id, ImVec2(bw, bh)))
			*e.on = !*e.on;
		const bool hot = ImGui::IsItemHovered();
		if (hot)
			ImGui::SetItemTooltip("%s", e.tip);
		dl->AddRectFilled(ImVec2(x, e.y), ImVec2(x + bw, e.y + bh),
		                  *e.on ? e.lit : col(hot ? ImGuiCol_ButtonHovered : ImGuiCol_Button), 3.0f);
		const ImVec2 ts = ImGui::CalcTextSize(e.mark);
		dl->AddText(ImVec2(x + (bw - ts.x) * 0.5f, e.y + (bh - ts.y) * 0.5f),
		            *e.on ? IM_COL32(20, 20, 20, 255) : col(ImGuiCol_Text), e.mark);
	}
	ImGui::SetCursorScreenPos(ImVec2(pos.x, pos.y + h));
	ImGui::Dummy(ImVec2(0, 0));
}


void overview::apply_mutes(xg::model &m, bridge &br)
{
	bool any_solo = false;
	for (int p = 0; p < PARTS; p++)
		any_solo |= m_solo[p];
	const xg::param &prcv = P("part.rcv_channel");
	for (int p = 0; p < PARTS; p++) {
		const bool want = m_mute[p] || (any_solo && !m_solo[p]);
		int rcv = 127;
		const bool known = m.get(prcv, p, rcv);
		if (m_saved_rcv[p] >= 0 && known && rcv != 127)
			m_saved_rcv[p] = -1;                    // 曲などが受信チャンネルを書き換えた
		if (want && m_saved_rcv[p] < 0 && known && rcv < PARTS) {
			// 鳴っている音を先に止める（受信を切るとノートオフも届かなくなるため）
			const u8 off[3] = { u8(0xb0 | (rcv & 15)), 120, 0 };
			br.send_port(rcv / 16, off, 3);
			br.send(m.set(prcv, p, 127));
			m_saved_rcv[p] = rcv;
		} else if (!want && m_saved_rcv[p] >= 0) {
			br.send(m.set(prcv, p, m_saved_rcv[p]));
			m_saved_rcv[p] = -1;
		}
	}
}


void overview::hidden(bridge &br)
{
	release_keys(br);
	// ミュートとソロは、この窓で聞き比べるためのもの。閉じたら外す（受信チャンネルを戻す）
	for (int p = 0; p < PARTS; p++) {
		m_mute[p] = m_solo[p] = false;
		if (m_saved_rcv[p] >= 0 && m_model)
			br.send(m_model->set(P("part.rcv_channel"), p, m_saved_rcv[p]));
		m_saved_rcv[p] = -1;
	}
}


void overview::draw(xg::model &m, const xg_snapshot &ram, bridge &br)
{
	m_wheel_taken = false;
	m_model = &m;
	apply_mutes(m, br);
	const ImGuiViewport *vp = ImGui::GetMainViewport();
	ImGui::SetNextWindowPos(vp->WorkPos);
	ImGui::SetNextWindowSize(vp->WorkSize);
	const ImGuiWindowFlags wf = ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoMove |
	                            ImGuiWindowFlags_NoSavedSettings | ImGuiWindowFlags_NoBringToFrontOnFocus;
	ImGui::PushStyleVar(ImGuiStyleVar_WindowRounding, 0);
	ImGui::Begin("overview", nullptr, wf);
	ImGui::PopStyleVar();

	// 表示の大きさ。32 パートを見渡すための窓なので、既定は小さめ（文字 10px）。
	// 棒や絵も文字の大きさから決まるので、全部が一緒に縮む
	float &zoom = overview_zoom();
	help_checkbox();
	ImGui::SameLine();
	ImGui::TextDisabled("|");
	ImGui::SameLine();
	if (ImGui::SmallButton("-"))
		set_overview_zoom(zoom - 0.125f);
	ImGui::SameLine();
	ImGui::Text("%d%%", int(std::lround(zoom * 100)));
	ImGui::SameLine();
	if (ImGui::SmallButton("+"))
		set_overview_zoom(zoom + 0.125f);
	ImGui::SameLine();
	ImGui::TextDisabled("表示の大きさ（小さな絵はダブルクリックで大きな窓に出る）");

	ImGui::PushFont(nullptr, ImGui::GetStyle().FontSizeBase * zoom);
	const float fs = ImGui::GetFontSize();
	const float h = fs * 2.3f;

	// マスターの表（見出しは別）。インサーションとバリエーションの設定もここ
	master_pane(m, ram, br);
	ImGui::Spacing();

	const ImGuiTableFlags flags = ImGuiTableFlags_RowBg | ImGuiTableFlags_ScrollY | ImGuiTableFlags_BordersInnerV |
	                              ImGuiTableFlags_SizingFixedFit | ImGuiTableFlags_PadOuterX;
	ImGui::PushStyleVar(ImGuiStyleVar_CellPadding, ImVec2(1, 1));
	if (ImGui::BeginTable("rows", NCOLS + 3, flags)) {
		ImGui::TableSetupScrollFreeze(1, 1);            // 見出しは流さない
		ImGui::TableSetupColumn("パート（右クリックで音色）", ImGuiTableColumnFlags_WidthFixed, fs * 18.5f);
		ImGui::TableSetupColumn("VEL", ImGuiTableColumnFlags_WidthFixed, fs * 2.2f);
		for (const column &c : COLUMNS)
			ImGui::TableSetupColumn(c.title, ImGuiTableColumnFlags_WidthFixed,
			                        wide(c.from) ? fs * 7.5f : c.from == src::ins ? fs * 8.5f : fs * 3.4f);
		ImGui::TableSetupColumn("##keys", ImGuiTableColumnFlags_WidthStretch);   // 見出しは要らない
		headers_with_help(NCOLS + 3);

		for (int part = 0; part < PARTS; part++) {
			ImGui::TableNextRow(0, h);
			row(part, m, ram, br, h);
		}
		ImGui::EndTable();
	}
	ImGui::PopStyleVar();
	ImGui::PopFont();
	ImGui::End();

	if (ImGui::GetIO().MouseWheel != 0.0f && !m_wheel_taken)
		m_scrolled_at = ImGui::GetTime();
}

} // namespace ui
