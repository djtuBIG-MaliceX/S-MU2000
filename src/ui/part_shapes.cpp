// license:BSD-3-Clause

#include "part_shapes.h"

#include "eq_curve.h"
#include "overview.h"

#include "imgui.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>

namespace ui {

using namespace xgui;

namespace {

constexpr int PARTS = 32;

// 値の棒 1 本。表示は層の書式（xg::format）で、ダブルクリックか Ctrl+クリックで数を打てる
void param_slider(const char *key, int part, xg::model &m, bridge &br)
{
	const xg::param &p = P(key);
	int v = 0;
	if (!m.get(p, part, v)) {
		ImGui::BeginDisabled();
		int dummy = p.min;
		ImGui::SliderInt(p.label, &dummy, p.min, p.max, "--");
		ImGui::EndDisabled();
		return;
	}
	// 書式の % は SliderInt の書式として読まれないよう重ねる。EQ の周波数は表の番号でなく Hz で出す
	const bool hz = std::strstr(key, "eq_") && std::strstr(key, "_freq");
	std::string text;
	for (char c : hz ? eq::hz_text(v) + " Hz" : xg::format(p, v)) {
		if (c == '%')
			text += '%';
		text += c;
	}
	int nv = v;
	if (ImGui::SliderInt(p.label, &nv, p.min, p.max, text.c_str()) && nv != v)
		br.send(m.set(p, part, nv));
	help_tip(key);
}

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
	if (ImGui::BeginCombo("##part", part_name(part).c_str())) {
		for (int p = 0; p < PARTS; p++)
			if (ImGui::Selectable(part_name(p).c_str(), p == part))
				set_shape_window_part(part = p);
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

	// ---- 左に 4 つの区画（2 × 2）、右に音色を選ぶ面
	const ImGuiStyle &st = ImGui::GetStyle();
	const ImVec2 avail = ImGui::GetContentRegionAvail();
	const float pane_w = std::min(fs * 15.0f, avail.x * 0.4f);
	const float shapes_w = avail.x - pane_w - st.ItemSpacing.x;
	const float w = (shapes_w - st.ItemSpacing.x) * 0.5f;
	const float h = (avail.y - st.ItemSpacing.y) * 0.5f;

	ImGui::BeginGroup();
	panel("vib", "ビブラート（VIB）", w, h, part, m, br, { "part.vib_rate", "part.vib_depth", "part.vib_delay" },
	      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::vib_cell(p, mm, b, pw, ph, false); });
	ImGui::SameLine();
	panel("filter", "フィルタ（FILTER）", w, h, part, m, br, { "part.cutoff", "part.resonance" },
	      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::filter_cell(p, mm, b, pw, ph, false); });
	panel("eg", "音量の形（EG）", w, h, part, m, br, { "part.attack", "part.decay", "part.release" },
	      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::eg_cell(p, mm, b, pw, ph, false); });
	ImGui::SameLine();
	panel("eq", "パートの EQ", w, h, part, m, br,
	      { "part.eq_bass_gain", "part.eq_bass_freq", "part.eq_treble_gain", "part.eq_treble_freq" },
	      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::eq_cell(p, mm, b, pw, ph, false); });
	ImGui::EndGroup();

	ImGui::SameLine();
	if (ImGui::BeginChild("voicepane", ImVec2(pane_w, 0)))
		program_pane(part, m, &ram, br);
	ImGui::EndChild();

	ImGui::PopFont();
	ImGui::End();
}

} // namespace ui
