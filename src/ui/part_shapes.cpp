// license:BSD-3-Clause

#include "part_shapes.h"

#include "overview.h"

#include "imgui.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <string>

namespace ui {

using namespace xgui;

namespace {

// パートは口 A-D の 64（C・D は実機では USB だけの口）
constexpr int PARTS = XG_PARTS;

// 1 つの区画。見出し、大きな絵、値の棒
template <typename Draw>
void panel(const char *id, const char *title, float w, float h, int part, xg::model &m, bridge &br,
           std::initializer_list<const char *> keys, Draw draw)
{
	const float fs = ImGui::GetFontSize();
	const ImGuiStyle &st = ImGui::GetStyle();
	if (!ImGui::BeginChild(id, ImVec2(w, h), ImGuiChildFlags_Borders)) {
		ImGui::EndChild();
		return;
	}
	ImGui::TextUnformatted(title);
	const float sliders = float(keys.size()) * (ImGui::GetFrameHeight() + st.ItemSpacing.y);
	const float avail_w = ImGui::GetContentRegionAvail().x;
	const float plot_h = std::max(fs * 4.0f, ImGui::GetContentRegionAvail().y - sliders - st.ItemSpacing.y);
	draw(part, m, br, avail_w, plot_h);
	ImGui::PushItemWidth(-fs * 6.0f);
	for (const char *k : keys)
		param_slider(k, part, m, br);
	ImGui::PopItemWidth();
	ImGui::EndChild();
}

} // namespace


void part_shapes::draw(xg::model &m, const xg_snapshot &ram, bridge &br)
{
	set_current_ram(&ram);            // 絵が音色の中身を読むため（ピッチ EG など）
	begin_hint_bar();                 // 絵や名前の説明は、マウスのそばでなく下の帯に出す
	const ImGuiViewport *vp = ImGui::GetMainViewport();
	ImGui::SetNextWindowPos(vp->WorkPos);
	ImGui::SetNextWindowSize(vp->WorkSize);
	const ImGuiWindowFlags wf = ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoMove |
	                            ImGuiWindowFlags_NoSavedSettings | ImGuiWindowFlags_NoBringToFrontOnFocus;
	ImGui::PushStyleVar(ImGuiStyleVar_WindowRounding, 0);
	ImGui::Begin("part_shapes", nullptr, wf);
	ImGui::PopStyleVar();

	// 表示の大きさ。文字も絵も同じ倍率で縮む（editor.ini に覚える）
	float &zoom = shapes_zoom();
	ImGui::PushFont(nullptr, ImGui::GetStyle().FontSizeBase * zoom);

	const float fs = ImGui::GetFontSize();
	int part = shape_window_part();

	// ---- パートを選ぶ。音色の名前も出す
	ImGui::SetNextItemWidth(fs * 5);
	if (ImGui::BeginCombo("##part", part_name(part).c_str(), ImGuiComboFlags_HeightLarge)) {
		for (int p = 0; p < PARTS; p++) {
			if (p && p % 16 == 0)
				ImGui::Separator();          // 口の境目
			if (ImGui::Selectable(part_name(p).c_str(), p == part))
				set_shape_window_part(part = p);
			if (p == part && ImGui::IsWindowAppearing())
				ImGui::SetScrollHereY();
		}
		ImGui::EndCombo();
	}
	ImGui::SameLine();
	if (ImGui::ArrowButton("##prev", ImGuiDir_Left))
		set_shape_window_part(part = (part + PARTS - 1) % PARTS);
	ImGui::SameLine();
	if (ImGui::ArrowButton("##next", ImGuiDir_Right))
		set_shape_window_part(part = (part + 1) % PARTS);
	ImGui::SameLine();
	int msb = 0, lsb = 0, prog = 0;
	std::string voice = "--";
	if (m.get(P("part.bank_msb"), part, msb) && m.get(P("part.bank_lsb"), part, lsb) && m.get(P("part.program"), part, prog)) {
		voice = voice_text(msb, lsb, prog);
		if (const xg::voice_rom *vr = voices()) {
			const std::string real = vr->name(ram.parts[part], msb, prog);
			if (!real.empty()) {
				char buf[48];
				std::snprintf(buf, sizeof(buf), "%3d  %s", prog + 1, real.c_str());
				voice = buf;
			}
		}
	}
	ImGui::TextUnformatted(voice.c_str());

	// 表示の大きさと、説明のチェックボックスは右端へ
	ImGui::SameLine(ImGui::GetContentRegionAvail().x + ImGui::GetCursorPosX() - fs * 18);
	if (ImGui::SmallButton("-"))
		set_shapes_zoom(zoom - 0.1f);
	ImGui::SameLine();
	ImGui::Text("%d%%", int(std::lround(zoom * 100)));
	ImGui::SameLine();
	if (ImGui::SmallButton("+"))
		set_shapes_zoom(zoom + 0.1f);
	ImGui::SameLine();
	help_checkbox();

	// ---- 上のペイン: エフェクト、棒、鍵盤
	const ImGuiStyle &st = ImGui::GetStyle();
	{
		// インサーションの行、見出しと棒の行、鍵盤の行
		const float strip_h = ImGui::GetFrameHeight() + ImGui::GetTextLineHeight() + fs * 0.15f + fs * 2.3f * 2.0f +
		                      st.ItemSpacing.y * 2.0f + st.WindowPadding.y * 2.0f + fs * 0.2f;
		if (ImGui::BeginChild("strip", ImVec2(0, strip_h), ImGuiChildFlags_Borders, ImGuiWindowFlags_NoScrollbar))
			m_strip.part_strip(part, m, ram, br);
		ImGui::EndChild();
	}

	// ---- 左に音色を選ぶ面、右はタブ: 「形」は 4 つの区画（2 × 2）、「すべて」はパートのパラメータ全部
	const ImVec2 avail = ImGui::GetContentRegionAvail();
	// 音色を選ぶ面は、左に分類・右に音色とバンク違いの 2 列（xgui::program_pane）
	const float pane_w = std::min(fs * 26.0f, avail.x * 0.5f);
	// 下の説明の帯（3 行ぶん）を残す
	const float bar_h = ImGui::GetTextLineHeightWithSpacing() * 3.0f + st.WindowPadding.y * 2.0f;
	const float body_h = std::max(fs * 8.0f, avail.y - bar_h - st.ItemSpacing.y);

	if (ImGui::BeginChild("voicepane", ImVec2(pane_w, body_h)))
		program_pane(part, m, &ram, br);
	ImGui::EndChild();
	ImGui::SameLine();

	ImGui::BeginGroup();
	const float top_y = ImGui::GetCursorScreenPos().y;
	if (ImGui::BeginTabBar("right")) {
		if (ImGui::BeginTabItem("形")) {
			// 3 × 2。上に VIB・FILTER・EG、下にピッチ EG・EQ・ポルタメント（絵は無く棒だけ）
			const ImVec2 room = ImGui::GetContentRegionAvail();
			const float room_h = body_h - (ImGui::GetCursorScreenPos().y - top_y);
			const float w = (room.x - st.ItemSpacing.x * 2.0f) / 3.0f;
			const float h = (room_h - st.ItemSpacing.y) * 0.5f;
			panel("vib", "ビブラート（VIB）", w, h, part, m, br, { "part.vib_rate", "part.vib_depth", "part.vib_delay" },
			      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::vib_cell(p, mm, b, pw, ph, false); });
			ImGui::SameLine();
			panel("filter", "フィルタ（FILTER）", w, h, part, m, br, { "part.cutoff", "part.resonance", "part.hpf_cutoff" },
			      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::filter_cell(p, mm, b, pw, ph, false); });
			ImGui::SameLine();
			panel("eg", "音量の形（EG）", w, h, part, m, br, { "part.attack", "part.decay", "part.release" },
			      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::eg_cell(p, mm, b, pw, ph, false); });
			panel("peg", "音程の形（ピッチ EG）", w, h, part, m, br,
			      { "part.peg_init_level", "part.peg_attack_time", "part.peg_rel_level", "part.peg_rel_time" },
			      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::peg_cell(p, mm, b, pw, ph, false); });
			ImGui::SameLine();
			panel("eq", "パートの EQ", w, h, part, m, br,
			      { "part.eq_bass_gain", "part.eq_bass_freq", "part.eq_treble_gain", "part.eq_treble_freq" },
			      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::eq_cell(p, mm, b, pw, ph, false); });
			ImGui::SameLine();
			panel("porta", "ポルタメント", w, h, part, m, br, { "part.porta_switch", "part.porta_time" },
			      [](int, xg::model &, bridge &, float pw, float ph) { ImGui::Dummy(ImVec2(pw, ph)); });
			ImGui::EndTabItem();
		}
		if (ImGui::BeginTabItem("すべて")) {
			// エディタのパートの面と同じ組（xgui::PART_GROUPS）を、幅に合わせた列数で並べる
			if (ImGui::BeginChild("all", ImVec2(0, body_h - (ImGui::GetCursorScreenPos().y - top_y)))) {
				const int columns = std::clamp(int(ImGui::GetContentRegionAvail().x / (fs * 16.0f)), 1, 3);
				if (ImGui::BeginTable("groups", columns, ImGuiTableFlags_SizingStretchSame)) {
					int n = 0;
					for (const part_group &g : PART_GROUPS) {
						if (n++ % columns == 0)
							ImGui::TableNextRow();
						ImGui::TableNextColumn();
						ImGui::SeparatorText(g.title);
						ImGui::PushItemWidth(-fs * 6.0f);
						for (const char *key : g.keys) {
							if (!key)
								break;
							param_slider(key, part, m, br);
						}
						ImGui::PopItemWidth();
						ImGui::Spacing();
					}
					ImGui::EndTable();
				}
			}
			ImGui::EndChild();
			ImGui::EndTabItem();
		}
		ImGui::EndTabBar();
	}
	ImGui::EndGroup();

	// ---- 説明の帯。カーソルを載せた絵・値・名前の説明（無ければ使い方のひとこと）
	if (ImGui::BeginChild("hint", ImVec2(0, 0), ImGuiChildFlags_Borders)) {
		const std::string &t = hint_text();
		if (t.empty())
			ImGui::TextDisabled("絵の点や値、名前にカーソルを載せると、ここに説明が出る");
		else
			ImGui::TextWrapped("%s", t.c_str());
	}
	ImGui::EndChild();
	end_hint_bar();

	ImGui::PopFont();
	ImGui::End();
}

} // namespace ui
