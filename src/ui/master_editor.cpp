// license:BSD-3-Clause

#include "master_editor.h"

#include "driver.h"
#include "fx_icons.h"
#include "overview.h"
#include "ui/texts.h"
#include "xg_state.h"

#include "imgui.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <string>

namespace ui {

using namespace xgui;

namespace {

// 見出し 1 行。区画の頭に
void heading(const char *text)
{
	ImGui::TextUnformatted(text);
	ImGui::Separator();
}

// 選ぶ値（EQ の種類、バリエーションの接続など）の箱
void choice_combo(const char *id, const char *key, xg::model &m, bridge &br)
{
	const xg::param &p = P(key);
	int v = p.min;
	const bool known = m.get(p, 0, v);
	if (ImGui::BeginCombo(id, known ? xg::format(p, v).c_str() : "--")) {
		for (int i = p.min; i <= p.max; i++)
			if (ImGui::Selectable(p.choices[i - p.min], known && i == v))
				br.send(m.set(p, 0, i));
		ImGui::EndCombo();
	}
	help_tip(key);
}

// エフェクトの種類の箱。品書きの形（分類 → 系統 → LSB 違い）で選ぶ
void type_combo(const char *id, const std::vector<xg::fx_type> &types, const char *key, xg::model &m, bridge &br)
{
	const xg::param &p = P(key);
	int type = 0;
	const bool known = m.get(p, 0, type);
	if (begin_fx_combo(id, known ? type : -1, ImGuiComboFlags_HeightLarge)) {
		int chosen = 0;
		if (fx_type_menu(types, known ? type : -1, chosen)) {
			br.send(m.set(p, 0, chosen));
			ImGui::CloseCurrentPopup();
		}
		ImGui::EndCombo();
	}
}

// バリエーションを掛けるパートの箱（接続が INSERTION のとき）。64 パートの後ろに AD1・AD2、127 が OFF
void part_combo(const char *id, const char *key, xg::model &m, bridge &br)
{
	const xg::param &p = P(key);
	int part = 127;
	const bool known = m.get(p, 0, part);
	const std::string now = !known ? "--" : part < XG_PARTS + 2 ? part_name(part) : "OFF";
	if (ImGui::BeginCombo(id, now.c_str(), ImGuiComboFlags_HeightLarge)) {
		if (ImGui::Selectable("OFF", known && part >= XG_PARTS + 2))
			br.send(m.set(p, 0, 127));
		for (int i = 0; i < XG_PARTS + 2; i++) {
			if (i % 16 == 0)
				ImGui::Separator();
			if (ImGui::Selectable(part_name(i).c_str(), known && i == part))
				br.send(m.set(p, 0, i));
		}
		ImGui::EndCombo();
	}
}

} // namespace


void master_editor::draw(xg::model &m, const xg_snapshot &ram, bridge &br)
{
	(void)ram;
	const ImGuiViewport *vp = ImGui::GetMainViewport();
	ImGui::SetNextWindowPos(vp->WorkPos);
	ImGui::SetNextWindowSize(vp->WorkSize);
	const ImGuiWindowFlags wf = ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoMove |
	                            ImGuiWindowFlags_NoSavedSettings | ImGuiWindowFlags_NoBringToFrontOnFocus;
	ImGui::PushStyleVar(ImGuiStyleVar_WindowRounding, 0);
	ImGui::Begin("master_editor", nullptr, wf);
	ImGui::PopStyleVar();

	float &zoom = master_zoom();
	ImGui::PushFont(nullptr, ImGui::GetStyle().FontSizeBase * zoom);
	const float fs = ImGui::GetFontSize();
	const ImGuiStyle &st = ImGui::GetStyle();

	// ---- 上の帯。表示の大きさと説明は右端へ
	ImGui::AlignTextToFramePadding();
	ImGui::TextUnformatted("MASTER");
	ImGui::SameLine(ImGui::GetContentRegionAvail().x + ImGui::GetCursorPosX() - fs * 18);
	if (ImGui::SmallButton("-"))
		set_master_zoom(zoom - 0.1f);
	ImGui::SameLine();
	ImGui::Text("%d%%", int(std::lround(zoom * 100)));
	ImGui::SameLine();
	if (ImGui::SmallButton("+"))
		set_master_zoom(zoom + 0.1f);
	ImGui::SameLine();
	help_checkbox();

	const ImVec2 avail = ImGui::GetContentRegionAvail();
	const float row_h = ImGui::GetFrameHeightWithSpacing();
	// 上の段はシステムエフェクトの表の行数（区画の見出し + 表の見出し + 7 行）で高さを決める
	const float top_h = std::min(avail.y * 0.55f, row_h * 9.0f + fs * 1.5f);

	// ---- 上の左: システム
	const float sys_w = std::min(fs * 22.0f, avail.x * 0.35f);
	if (ImGui::BeginChild("system", ImVec2(sys_w, top_h), ImGuiChildFlags_Borders)) {
		heading(UI_TEXT(me_sys, "System"));
		ImGui::PushItemWidth(-fs * 6.5f);
		param_slider("system.master_volume", 0, m, br);
		param_slider("system.master_tune", 0, m, br);
		param_slider("system.transpose", 0, m, br);
		ImGui::PopItemWidth();
		ImGui::Spacing();
		ImGui::TextDisabled("%s", UI_TEXT(me_tune_note, "Tune moves in 0.1 cent steps,\ntranspose in semitones"));
		ImGui::Spacing();
		sysex_pane(ram, br);
	}
	ImGui::EndChild();
	ImGui::SameLine();

	// ---- 上の右: システムエフェクト（リバーブ・コーラス・バリエーション）
	if (ImGui::BeginChild("effects", ImVec2(0, top_h), ImGuiChildFlags_Borders)) {
		heading(UI_TEXT(me_fx, "System effects"));
		const ImGuiTableFlags tf = ImGuiTableFlags_SizingStretchSame | ImGuiTableFlags_BordersInnerV;
		if (ImGui::BeginTable("fx", 4, tf)) {
			ImGui::TableSetupColumn("##what", ImGuiTableColumnFlags_WidthFixed, fs * 6.5f);
			ImGui::TableSetupColumn(UI_TEXT(sys_reverb, "Reverb"));
			ImGui::TableSetupColumn(UI_TEXT(sys_chorus, "Chorus"));
			ImGui::TableSetupColumn(UI_TEXT(sys_variation, "Variation"));
			ImGui::TableHeadersRow();

			// 1 行ずつ。無い所は空けておく
			auto row = [&](const char *what, const char *rev, const char *cho, const char *var) {
				ImGui::TableNextRow();
				ImGui::TableNextColumn();
				ImGui::AlignTextToFramePadding();
				ImGui::TextUnformatted(what);
				for (const char *key : { rev, cho, var }) {
					ImGui::TableNextColumn();
					if (!key)
						continue;
					ImGui::SetNextItemWidth(-FLT_MIN);
					param_slider(key, 0, m, br, "##v");
				}
			};

			ImGui::TableNextRow();
			ImGui::TableNextColumn();
			ImGui::AlignTextToFramePadding();
			ImGui::TextUnformatted(UI_TEXT(fx_kind, "Type"));
			ImGui::TableNextColumn();
			ImGui::SetNextItemWidth(-FLT_MIN);
			type_combo("##revtype", xg::rev_types(), "reverb.type", m, br);
			ImGui::TableNextColumn();
			ImGui::SetNextItemWidth(-FLT_MIN);
			type_combo("##chotype", xg::cho_types(), "chorus.type", m, br);
			ImGui::TableNextColumn();
			ImGui::SetNextItemWidth(-FLT_MIN);
			type_combo("##vartype", xg::ins_types(), "variation.type", m, br);

			// 種類ごとのパラメータは、エフェクトの窓（インサーションと同じ窓の REV・CHO・VAR）で
			ImGui::TableNextRow();
			ImGui::TableNextColumn();
			ImGui::AlignTextToFramePadding();
			ImGui::TextUnformatted(UI_TEXT(me_params, "Parameters"));
			for (int slot : { 5, 6, 7 }) {
				ImGui::TableNextColumn();
				ImGui::PushID(slot);
				if (ImGui::Button(UI_TEXT(me_knobs_open, "Open knobs...")))
					request_fx(slot);
				ImGui::PopID();
			}

			row(UI_TEXT(me_back, "Return"), "reverb.return", "chorus.return", "variation.return");
			row(UI_TEXT(me_pan, "Pan"), "reverb.pan", "chorus.pan", "variation.pan");
			row(UI_TEXT(me_to_rev, "To reverb"), nullptr, "chorus.to_reverb", "variation.to_reverb");
			row(UI_TEXT(me_to_cho, "To chorus"), nullptr, nullptr, "variation.to_chorus");

			// バリエーションの接続。INSERTION のときは戻り量と送りは使われず、掛けるパートに直に入る
			int conn = 1;
			const bool known_conn = m.get(P("variation.connect"), 0, conn);
			ImGui::TableNextRow();
			ImGui::TableNextColumn();
			ImGui::AlignTextToFramePadding();
			ImGui::TextUnformatted(UI_TEXT(fx_connect, "Connection"));
			ImGui::TableNextColumn();
			ImGui::TableNextColumn();
			ImGui::TableNextColumn();
			ImGui::SetNextItemWidth(-FLT_MIN);
			choice_combo("##varconn", "variation.connect", m, br);

			ImGui::TableNextRow();
			ImGui::TableNextColumn();
			ImGui::AlignTextToFramePadding();
			ImGui::TextUnformatted(UI_TEXT(fx_part, "Part"));
			ImGui::TableNextColumn();
			ImGui::TableNextColumn();
			ImGui::TableNextColumn();
			ImGui::BeginDisabled(known_conn && conn != 0);
			ImGui::SetNextItemWidth(-FLT_MIN);
			part_combo("##varpart", "variation.part", m, br);
			ImGui::EndDisabled();
			if (known_conn && conn != 0 && ImGui::IsItemHovered(ImGuiHoveredFlags_AllowWhenDisabled))
				ImGui::SetTooltip("%s", UI_TEXT(me_insert_note, "Only used when connected as INSERTION"));
			ImGui::EndTable();
		}
	}
	ImGui::EndChild();

	// ---- 下: マスター EQ
	if (ImGui::BeginChild("eq", ImVec2(0, 0), ImGuiChildFlags_Borders)) {
		ImGui::AlignTextToFramePadding();
		ImGui::TextUnformatted(UI_TEXT(me_master_eq, "Master EQ"));
		ImGui::SameLine(0, fs * 1.5f);
		ImGui::TextUnformatted(UI_TEXT(fx_kind, "Type"));
		ImGui::SameLine();
		ImGui::SetNextItemWidth(fs * 8);
		choice_combo("##eqtype", "master_eq.type", m, br);
		if (ImGui::IsItemHovered())
			ImGui::SetTooltip("%s", UI_TEXT(me_eq_type_note, "Picking a type rewrites the 5 bands to that type's values"));
		for (int band : { 1, 5 }) {
			char key[24], label[32];
			std::snprintf(key, sizeof(key), "master_eq.shape%d", band);
			std::snprintf(label, sizeof(label), UI_TEXT(me_band_peak_fmt, "Make band %d peak"), band);
			int shape = 0;
			const bool known = m.get(P(key), 0, shape);
			bool peak = shape == 1;
			ImGui::SameLine(0, fs * 1.5f);
			ImGui::BeginDisabled(!known);
			if (ImGui::Checkbox(label, &peak))
				br.send(m.set(P(key), 0, peak ? 1 : 0));
			ImGui::EndDisabled();
			if (ImGui::IsItemHovered(ImGuiHoveredFlags_AllowWhenDisabled))
				ImGui::SetTooltip("%s", band == 1 ? UI_TEXT(me_band_shape1, "Band 1 shape. Off is low-shelf, on is peak")
				                                  : UI_TEXT(me_band_shape5, "Band 5 shape. Off is high-shelf, on is peak"));
		}
		ImGui::Separator();

		// 棒の表（見出し + ゲイン・周波数・Q）の高さを残して、残りを特性の絵に
		const float table_h = row_h * 4.0f + st.CellPadding.y * 8.0f;
		const float plot_w = ImGui::GetContentRegionAvail().x;
		const float plot_h = std::max(fs * 5.0f, ImGui::GetContentRegionAvail().y - table_h - st.ItemSpacing.y);
		overview::master_eq_plot(m, br, plot_w, plot_h, true);

		if (ImGui::BeginTable("bands", 6, ImGuiTableFlags_SizingStretchSame | ImGuiTableFlags_BordersInnerV)) {
			ImGui::TableSetupColumn("##what", ImGuiTableColumnFlags_WidthFixed, fs * 4.5f);
			for (int b = 1; b <= 5; b++) {
				char name[16];
				std::snprintf(name, sizeof(name), UI_TEXT(me_band_fmt, "Band %d"), b);
				ImGui::TableSetupColumn(name);
			}
			ImGui::TableHeadersRow();
			const char *const WHAT[3] = { UI_TEXT(me_w_gain, "Gain"), UI_TEXT(me_w_freq, "Freq"), UI_TEXT(me_w_q, "Q (width)") };
			static const char *const KEY[3] = { "master_eq.gain%d", "master_eq.freq%d", "master_eq.q%d" };
			for (int r = 0; r < 3; r++) {
				ImGui::TableNextRow();
				ImGui::TableNextColumn();
				ImGui::AlignTextToFramePadding();
				ImGui::TextUnformatted(WHAT[r]);
				for (int b = 1; b <= 5; b++) {
					ImGui::TableNextColumn();
					char key[24];
					std::snprintf(key, sizeof(key), KEY[r], b);
					ImGui::SetNextItemWidth(-FLT_MIN);
					param_slider(key, 0, m, br, "##v");
				}
			}
			ImGui::EndTable();
		}
	}
	ImGui::EndChild();

	ImGui::PopFont();
	ImGui::End();
}

// ---- .syx の書き出し・読み込み（issue #35）
//
// 書き出し  いまの XG の値を SysEx にしてファイルへ。「既定と違うものだけ」なら、頭に XG System On を
//           置き、そのあと既定値（XG System On の直後の値）と違うところだけを並べる。曲の頭に
//           貼るのに向く。外すと全部（プラグインの「XG の値だけ」の状態と同じもの）
// 読み込み  ファイルの SysEx を 1 通ずつ音源へ流す。リセットの後は 200ms 待つ（実機も受け付けない間がある）
void master_editor::sysex_pane(const xg_snapshot &ram, bridge &br)
{
	const bool ready = ram.serial != 0;

	// 既定値ができたら書き出す
	if (m_export_waiting && br.have_defaults()) {
		m_export_waiting = false;
		const std::unique_ptr<xg_snapshot> base = std::make_unique<xg_snapshot>();
		br.read_defaults(*base);
		std::vector<u8> out = { 0xf0, 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7e, 0x00, 0xf7 };
		const std::vector<u8> diff = setup_diff_messages(ram, *base);
		out.insert(out.end(), diff.begin(), diff.end());
		xgui::ask_save_file(std::move(out));
	}

	// 読み込んだものを流す。輪に入りきらなければ次のコマで続き
	std::vector<u8> opened;
	if (xgui::take_opened_file(opened)) {
		m_import = std::move(opened);
		m_import_at = 0;
		m_import_hold_until = 0;
		size_t n = 0;
		for (u8 b : m_import)
			n += b == 0xf0;
		char note[64];
		std::snprintf(note, sizeof(note), n ? UI_TEXT(note_sysex_busy_fmt, "Loading (%zu SysEx messages)") : UI_TEXT(note_sysex_idle, "No SysEx found"), n);
		xgui::set_file_note(note);
	}
	while (m_import_at < m_import.size() && br.audio_ms() >= m_import_hold_until) {
		if (m_import[m_import_at] != 0xf0) {        // SysEx の外は飛ばす
			m_import_at++;
			continue;
		}
		size_t end = m_import_at + 1;
		while (end < m_import.size() && m_import[end] != 0xf7 && !(m_import[end] & 0x80))
			end++;
		if (end >= m_import.size() || m_import[end] != 0xf7) {   // 閉じていない。そこから先を探し直す
			m_import_at = end;
			continue;
		}
		const size_t len = end + 1 - m_import_at;
		if (len >= 4096) {                         // 輪より大きいものは送れない
			m_import_at = end + 1;
			continue;
		}
		if (!br.send(m_import.data() + m_import_at, len))
			break;
		if (driver::is_reset(m_import.data() + m_import_at + 1, len - 2))
			m_import_hold_until = br.audio_ms() + 200;
		m_import_at = end + 1;
		if (m_import_at >= m_import.size())
			xgui::set_file_note(UI_TEXT(note_imported, "Imported"));
	}
	if (m_import_at >= m_import.size() && !m_import.empty()) {
		m_import.clear();
		m_import_at = 0;
	}

	ImGui::SeparatorText(UI_TEXT(me_sysex_title, "SysEx (.syx)"));
	const bool can = xgui::file_dialogs() && ready && !m_export_waiting && m_import.empty();
	ImGui::BeginDisabled(!can);
	if (ImGui::Button(UI_TEXT(me_export, "Export..."))) {
		if (!m_diff_only) {
			xgui::ask_save_file(setup_messages(ram));
		} else {
			m_export_waiting = true;
			br.request_defaults();              // できたら上で書き出す
		}
	}
	ImGui::SameLine();
	if (ImGui::Button(UI_TEXT(me_import, "Import...")))
		xgui::ask_open_file();
	ImGui::EndDisabled();
	if (!xgui::file_dialogs() && ImGui::IsItemHovered(ImGuiHoveredFlags_AllowWhenDisabled))
		ImGui::SetTooltip("%s", UI_TEXT(me_no_dialog, "No file dialog on this platform yet"));
	ImGui::Checkbox(UI_TEXT(me_diff_only, "Only non-defaults"), &m_diff_only);
	if (ImGui::IsItemHovered())
		ImGui::SetTooltip("%s", UI_TEXT(me_diff_only_tip, "On: start with XG System On, then only what differs from defaults (for pasting at a song start).\n"
		                                            "Off: write all XG values.\n"
		                                            "Defaults are read by saving the machine, playing XG System On and restoring.\n"
		                                            "Sound may glitch for a moment the first time"));
	if (m_export_waiting)
		ImGui::TextDisabled("%s", UI_TEXT(me_reading_defaults, "Reading defaults..."));
	else if (!xgui::file_note().empty())
		ImGui::TextDisabled("%s", xgui::file_note().c_str());
}

} // namespace ui
