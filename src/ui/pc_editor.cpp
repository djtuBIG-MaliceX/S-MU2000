// license:BSD-3-Clause

#include "pc_editor.h"
#include "ui/texts.h"
#include "xg_ui.h"

#include "imgui.h"
#include "imgui_internal.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <string>

namespace ui {

using namespace xgui;

namespace {

constexpr int PARTS = 32;


bool is_rcv(const xg::param &p) { return std::string(p.key) == "part.rcv_channel"; }

std::string shown(const xg::param &p, int v)
{
	return is_rcv(p) ? channel_name(v) : xg::format(p, v);
}

// パートの面に並べる組（音色の窓の「すべて」と同じ表。xg_ui.h）
using group = part_group;

ImU32 col(ImGuiCol c, float alpha = 1.0f) { return ImGui::GetColorU32(c, alpha); }

} // namespace


void pc_editor::write(const xg::param &p, int part, int v, xg::model &m, bridge &br)
{
	br.send(m.set(p, part, v));
}


// つまみ（と、閉じているときの数の箱）。戻り値は「値が変わったか」
//   上下ドラッグ     動かす（Shift で細かく）
//   ホイール         1 つずつ（Ctrl で 10 ずつ）
//   ダブルクリック   数を打つ
bool pc_editor::knob(const xg::param &p, int part, int &v, bool known, float width)
{
	ImGuiIO &io = ImGui::GetIO();
	const float fs = ImGui::GetFontSize();
	const bool big = m_knobs;
	const ImVec2 size = big ? ImVec2(std::max(width, fs * 3.2f), fs * 3.6f)
	                        : ImVec2(width, ImGui::GetFrameHeight());

	ImGui::PushID(p.key);
	ImGui::PushID(part);
	const ImVec2 pos = ImGui::GetCursorScreenPos();
	ImGui::InvisibleButton("##knob", size, ImGuiButtonFlags_MouseButtonLeft);
	const ImGuiID id = ImGui::GetItemID();
	const bool hovered = ImGui::IsItemHovered();
	const bool active  = ImGui::IsItemActive();

	int nv = v;
	if (known) {
		const int range = p.max - p.min;
		// ドラッグ。全域を 200px ほどで動かす（Shift で 4 倍細かく）
		if (active && ImGui::IsMouseDragging(ImGuiMouseButton_Left, 0.0f)) {
			// a Get*Ref reference goes stale when an insert grows the storage,
			// so take a value and write it back
			ImGuiStorage *st = ImGui::GetStateStorage();
			float acc = st->GetFloat(id, 0.0f);
			const float per_px = float(range) / (io.KeyShift ? 800.0f : 200.0f);
			acc -= io.MouseDelta.y * per_px;
			const int step = int(acc);
			if (step) {
				nv = std::clamp(nv + step, p.min, p.max);
				acc -= float(step);
			}
			st->SetFloat(id, acc);
		}
		if (ImGui::IsItemDeactivated())
			ImGui::GetStateStorage()->SetFloat(id, 0.0f);
		// ホイール。ただし表をスクロールしている最中（直前 0.5 秒に回っていた）なら表に回す。
		// そうしないと、スクロールで下から来たつまみの値を知らずに変えてしまう
		if (hovered && ImGui::GetTime() - m_scrolled_at > 0.5) {
			ImGui::SetItemKeyOwner(ImGuiKey_MouseWheelY);
			if (io.MouseWheel != 0.0f) {
				nv = std::clamp(nv + (io.MouseWheel > 0 ? 1 : -1) * (io.KeyCtrl ? 10 : 1), p.min, p.max);
				m_wheel_taken = true;
			}
		}
		if (hovered && ImGui::IsMouseDoubleClicked(ImGuiMouseButton_Left))
			ImGui::OpenPopup("##type");
	}

	// ---- 描く
	ImDrawList *dl = ImGui::GetWindowDrawList();
	const int shown_v = nv;
	const std::string text = known ? shown(p, shown_v) : "--";
	const float frac = (known && p.max > p.min) ? float(shown_v - p.min) / float(p.max - p.min) : 0.0f;
	const bool bipolar = p.how == xg::view::center || p.how == xg::view::pan;
	const ImU32 accent = col(ImGuiCol_SliderGrabActive);
	const ImU32 track  = col(ImGuiCol_FrameBg);
	const ImU32 txt    = known ? col(ImGuiCol_Text) : col(ImGuiCol_TextDisabled);

	if (big) {
		const float r = fs * 1.15f;
		const ImVec2 c(pos.x + size.x * 0.5f, pos.y + r + fs * 0.15f);
		const float a0 = IM_PI * 0.75f, a1 = IM_PI * 2.25f;
		const float av = a0 + (a1 - a0) * frac;
		const float thick = std::max(2.0f, fs * 0.18f);
		// 輪
		dl->PathArcTo(c, r, a0, a1, 40);
		dl->PathStroke(track, 0, thick);
		if (known) {
			const float from = bipolar ? (a0 + a1) * 0.5f : a0;
			dl->PathArcTo(c, r, std::min(from, av), std::max(from, av), 40);
			dl->PathStroke(accent, 0, thick);
		}
		// 本体と指し
		const float body = r - thick * 1.6f;
		dl->AddCircleFilled(c, body, col(hovered || active ? ImGuiCol_FrameBgHovered : ImGuiCol_Button), 32);
		if (known) {
			const ImVec2 dir(std::cos(av), std::sin(av));
			dl->AddLine(ImVec2(c.x + dir.x * body * 0.25f, c.y + dir.y * body * 0.25f),
			            ImVec2(c.x + dir.x * body * 0.9f,  c.y + dir.y * body * 0.9f), col(ImGuiCol_Text), thick * 0.8f);
		}
		const ImVec2 ts = ImGui::CalcTextSize(text.c_str());
		dl->AddText(ImVec2(c.x - ts.x * 0.5f, c.y + r + fs * 0.15f), txt, text.c_str());
	} else {
		const ImVec2 b(pos.x + size.x, pos.y + size.y);
		const float rr = ImGui::GetStyle().FrameRounding;
		dl->AddRectFilled(pos, b, col(active ? ImGuiCol_FrameBgActive : hovered ? ImGuiCol_FrameBgHovered : ImGuiCol_FrameBg), rr);
		const ImVec2 ts = ImGui::CalcTextSize(text.c_str());
		dl->AddText(ImVec2(pos.x + (size.x - ts.x) * 0.5f, pos.y + (size.y - ts.y) * 0.5f), txt, text.c_str());
	}

	// 数を打つ
	if (ImGui::BeginPopup("##type")) {
		ImGui::TextDisabled("%s（%d-%d）", p.label, p.min, p.max);
		const ImGuiID typed_id = ImGui::GetID("typed");
		ImGuiStorage *st = ImGui::GetStateStorage();
		int typed = st->GetInt(typed_id, v);
		if (ImGui::IsWindowAppearing()) {
			typed = v;
			ImGui::SetKeyboardFocusHere();
		}
		ImGui::SetNextItemWidth(fs * 6);
		if (ImGui::InputInt("##n", &typed, 1, 10, ImGuiInputTextFlags_EnterReturnsTrue)) {
			nv = std::clamp(typed, p.min, p.max);
			ImGui::CloseCurrentPopup();
		}
		st->SetInt(typed_id, typed);
		ImGui::EndPopup();
	}

	if (hovered && !active)
		ImGui::SetItemTooltip(UI_TEXT(ed_slider_tip_fmt, "%s  %s\nDrag, wheel (Ctrl for 10), or double-click to type a value"),
		                      p.label, text.c_str());

	ImGui::PopID();
	ImGui::PopID();
	const bool changed = known && nv != v;
	v = nv;
	return changed;
}


void pc_editor::value(const xg::param &p, int part, xg::model &m, bridge &br, float width)
{
	int v = 0;
	const bool known = m.get(p, part, v);

	if (p.how == xg::view::choice || is_rcv(p)) {
		// 選ぶ種類。品書きで
		ImGui::PushID(p.key);
		ImGui::PushID(part);
		ImGui::SetNextItemWidth(width);
		if (!known) {
			ImGui::BeginDisabled();
			if (ImGui::BeginCombo("##c", "--"))
				ImGui::EndCombo();
			ImGui::EndDisabled();
		} else if (ImGui::BeginCombo("##c", shown(p, v).c_str(), ImGuiComboFlags_HeightLarge)) {
			const bool rcv = is_rcv(p);
			const int hi = rcv ? 32 : p.max;
			for (int i = p.min; i <= hi; i++) {
				const int value = rcv && i == 32 ? 127 : i;
				if (ImGui::Selectable(shown(p, value).c_str(), value == v))
					write(p, part, value, m, br);
			}
			ImGui::EndCombo();
		}
		ImGui::PopID();
		ImGui::PopID();
		return;
	}
	if (knob(p, part, v, known, width))
		write(p, part, v, m, br);
}


void pc_editor::part_list(xg::model &m, bridge &br)
{
	const ImGuiTableFlags flags = ImGuiTableFlags_RowBg | ImGuiTableFlags_ScrollY |
	                              ImGuiTableFlags_BordersInnerV | ImGuiTableFlags_SizingFixedFit;
	if (!ImGui::BeginTable("parts", 4, flags))
		return;
	ImGui::TableSetupScrollFreeze(0, 1);
	ImGui::TableSetupColumn(UI_TEXT(ed_col_part, "Part"));
	ImGui::TableSetupColumn(UI_TEXT(ed_col_rcv, "Ch"));
	ImGui::TableSetupColumn(UI_TEXT(ed_col_voice, "Voice (right-click)"), ImGuiTableColumnFlags_WidthStretch);
	ImGui::TableSetupColumn(UI_TEXT(ed_col_vol, "Vol"));
	ImGui::TableHeadersRow();

	for (int i = 0; i < PARTS; i++) {
		ImGui::PushID(i);
		ImGui::TableNextRow();
		ImGui::TableNextColumn();
		const std::string name = part_name(i);
		if (ImGui::Selectable(name.c_str(), m_part == i, ImGuiSelectableFlags_SpanAllColumns)) {
			m_part = i;
		}
		if (ImGui::BeginPopupContextItem("program")) {
			xgui::program_menu(i, m, m_ram, br);
			ImGui::EndPopup();
		}
		int v = 0;
		ImGui::TableNextColumn();
		ImGui::TextUnformatted(m.get(P("part.rcv_channel"), i, v) ? channel_name(v).c_str() : "--");
		ImGui::TableNextColumn();
		int msb = 0, lsb = 0, prog = 0;
		if (m.get(P("part.bank_msb"), i, msb) && m.get(P("part.bank_lsb"), i, lsb) && m.get(P("part.program"), i, prog))
			ImGui::TextUnformatted(voice_text(shown_bank_msb(i, m, msb), lsb, prog).c_str());
		else
			ImGui::TextUnformatted("--");
		ImGui::TableNextColumn();
		if (m.get(P("part.volume"), i, v))
			ImGui::Text("%3d", v);
		else
			ImGui::TextUnformatted(" --");
		ImGui::PopID();
	}
	ImGui::EndTable();
}


void pc_editor::mixer(xg::model &m, bridge &br)
{
	static const char *const COLS[] = {
		"part.volume", "part.pan", "part.dry_level", "part.reverb_send", "part.chorus_send", "part.variation_send",
	};
	static const char *const TITLES[] = { "Volume", "Pan", "Dry", "Reverb", "Chorus", "Variation" };
	const int ncols = int(sizeof(COLS) / sizeof(COLS[0]));

	const ImGuiTableFlags flags = ImGuiTableFlags_RowBg | ImGuiTableFlags_ScrollY |
	                              ImGuiTableFlags_BordersInnerV | ImGuiTableFlags_SizingStretchSame;
	if (!ImGui::BeginTable("mixer", ncols + 1, flags))
		return;
	ImGui::TableSetupScrollFreeze(1, 1);
	ImGui::TableSetupColumn(UI_TEXT(ed_col_part, "Part"), ImGuiTableColumnFlags_WidthFixed);
	for (const char *t : TITLES)
		ImGui::TableSetupColumn(t, ImGuiTableColumnFlags_WidthStretch);
	ImGui::TableNextRow(ImGuiTableRowFlags_Headers);
	for (int c = 0; c <= ncols; c++) {
		ImGui::TableSetColumnIndex(c);
		ImGui::TableHeader(ImGui::TableGetColumnName(c));
		if (c > 0)
			help_tip(COLS[c - 1]);
	}

	for (int i = 0; i < PARTS; i++) {
		ImGui::PushID(i);
		ImGui::TableNextRow();
		ImGui::TableNextColumn();
		if (m_knobs)
			ImGui::Dummy(ImVec2(0, ImGui::GetFontSize() * 1.1f));  // 名前をつまみの真ん中あたりへ
		if (ImGui::Selectable(part_name(i).c_str(), m_part == i)) {
			m_part = i;
		}
		for (const char *key : COLS) {
			ImGui::TableNextColumn();
			const float w = ImGui::GetContentRegionAvail().x;
			value(P(key), i, m, br, w);
		}
		ImGui::PopID();
	}
	ImGui::EndTable();
}


void pc_editor::part_page(xg::model &m, bridge &br)
{
	const float fs = ImGui::GetFontSize();
	if (!ImGui::BeginChild("groups", ImVec2(0, 0)))
		{ ImGui::EndChild(); return; }

	if (!m_knobs) {
		// 数の形。2 列に、名前と値を並べる
		const float label_w = fs * 7;
		if (ImGui::BeginTable("groups", 2, ImGuiTableFlags_SizingStretchSame)) {
			int n = 0;
			for (const group &g : part_groups()) {
				if (n++ % 2 == 0)
					ImGui::TableNextRow();
				ImGui::TableNextColumn();
				ImGui::SeparatorText(g.title);
				for (const char *key : g.keys) {
					if (!key)
						break;
					const xg::param &p = P(key);
					ImGui::AlignTextToFramePadding();
					ImGui::TextUnformatted(p.label);
					help_tip(p.key);
					ImGui::SameLine(label_w);
					value(p, m_part, m, br, std::min(fs * 9, ImGui::GetContentRegionAvail().x));
				}
				if (!std::strcmp(g.title, texts().xgui_group_bend))
					xgui::bend_now_line(m_part, m, m_ram);
				ImGui::Spacing();
			}
			ImGui::EndTable();
		}
	} else {
		// つまみの形。組ごとに横へ流す
		const float cell = fs * 5.2f;
		for (const group &g : part_groups()) {
			ImGui::SeparatorText(g.title);
			const float right = ImGui::GetCursorScreenPos().x + ImGui::GetContentRegionAvail().x;
			bool first = true;
			for (const char *key : g.keys) {
				if (!key)
					break;
				const xg::param &p = P(key);
				if (!first) {
					ImGui::SameLine();
					if (ImGui::GetCursorScreenPos().x + cell > right)
						ImGui::NewLine();
				}
				first = false;
				ImGui::BeginGroup();
				const ImVec2 ts = ImGui::CalcTextSize(p.label);
				ImGui::SetCursorPosX(ImGui::GetCursorPosX() + std::max(0.0f, (cell - ts.x) * 0.5f));
				ImGui::TextUnformatted(p.label);
				help_tip(p.key);
				if (p.how == xg::view::choice || is_rcv(p)) {
					ImGui::Dummy(ImVec2(0, fs * 0.9f));
					value(p, m_part, m, br, cell);
					ImGui::Dummy(ImVec2(cell, fs * 1.2f));
				} else {
					value(p, m_part, m, br, cell);
				}
				ImGui::EndGroup();
			}
			if (!std::strcmp(g.title, texts().xgui_group_bend))
				xgui::bend_now_line(m_part, m, m_ram);
			ImGui::Spacing();
		}
	}
	ImGui::EndChild();
}


void pc_editor::draw(xg::model &m, const xg_snapshot &ram, bridge &br)
{
	m_ram = &ram;
	m_wheel_taken = false;

	const ImGuiViewport *vp = ImGui::GetMainViewport();
	ImGui::SetNextWindowPos(vp->WorkPos);
	ImGui::SetNextWindowSize(vp->WorkSize);
	const ImGuiWindowFlags wf = ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoMove |
	                            ImGuiWindowFlags_NoSavedSettings | ImGuiWindowFlags_NoBringToFrontOnFocus;
	ImGui::PushStyleVar(ImGuiStyleVar_WindowRounding, 0);
	ImGui::Begin("editor", nullptr, wf);
	ImGui::PopStyleVar();

	// 上の帯
	if (ImGui::Button(UI_TEXT(editor_xg_reset, "XG reset"))) {
		static const u8 XG_ON[] = { 0xf0, 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7e, 0x00, 0xf7 };
		br.send(XG_ON, sizeof(XG_ON));
	}
	ImGui::SameLine();
	if (ImGui::Button(UI_TEXT(editor_all_off, "All notes off"))) {
		for (int ch = 0; ch < 16; ch++) {
			const u8 msg[3] = { u8(0xb0 | ch), 123, 0 };
			br.send(msg, 3);
		}
	}
	ImGui::SameLine();
	if (ImGui::Button(m_knobs ? UI_TEXT(ed_knobs_on, "▲ Numbers only") : UI_TEXT(ed_knobs_off, "▼ Show knobs")))
		m_knobs = !m_knobs;
	ImGui::SameLine();
	help_checkbox();

	// 左にパートの一覧、右に面
	const float list_w = ImGui::GetFontSize() * 20;
	ImGui::BeginChild("list", ImVec2(list_w, 0), ImGuiChildFlags_Borders | ImGuiChildFlags_ResizeX);
	part_list(m, br);
	ImGui::EndChild();
	ImGui::SameLine();
	ImGui::BeginChild("page", ImVec2(0, 0));
	if (ImGui::BeginTabBar("tabs")) {
		if (ImGui::BeginTabItem(UI_TEXT(ed_tab_mixer, "Mixer"))) {
			mixer(m, br);
			ImGui::EndTabItem();
		}
		if (ImGui::BeginTabItem(UI_TEXT(ed_tab_part, "Part"))) {
			ImGui::Text(UI_TEXT(xgui_part_fmt, "Part %s"), part_name(m_part).c_str());
			part_page(m, br);
			ImGui::EndTabItem();
		}
		ImGui::EndTabBar();
	}
	ImGui::EndChild();

	ImGui::End();

	// つまみが取らなかったホイールはスクロール。しばらくつまみに取らせない
	if (ImGui::GetIO().MouseWheel != 0.0f && !m_wheel_taken)
		m_scrolled_at = ImGui::GetTime();
}

} // namespace ui
