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
constexpr float BAR_SCALE = 0.85f;   // 下の説明の帯の字の大きさ（本文に対して）

// 「グラフ ○ つまみ」の切り替え。見出しの行の右端に描き、押されたら true。
// 見出しと並べて入らなければ字を外して切り替えだけにし、それでも入らなければ見出しを切る
bool title_toggle(const char *title, const char *id, bool knobs, bool &toggle_hovered)
{
	const float fs = ImGui::GetFontSize();
	const char *l = "グラフ", *r = "つまみ";
	const float lw = ImGui::CalcTextSize(l).x, rw = ImGui::CalcTextSize(r).x;
	const float th = fs * 0.9f, tw = th * 1.8f, gap = fs * 0.3f;
	const float room = ImGui::GetContentRegionAvail().x;
	const float title_w = ImGui::CalcTextSize(title).x;
	const bool words = title_w + fs + lw + gap + tw + gap + rw <= room;
	const float total = words ? lw + gap + tw + gap + rw : tw;
	const ImVec2 start = ImGui::GetCursorScreenPos();
	const float line_h = ImGui::GetTextLineHeight();
	// 見出し（切り替えにかからないところまで）
	ImDrawList *dl = ImGui::GetWindowDrawList();
	const float title_room = std::max(0.0f, room - total - fs * 0.5f);
	dl->PushClipRect(start, ImVec2(start.x + title_room, start.y + line_h), true);
	dl->AddText(start, ImGui::GetColorU32(ImGuiCol_Text), title);
	dl->PopClipRect();
	ImGui::Dummy(ImVec2(title_room, line_h));
	if (ImGui::IsItemHovered() && title_w > title_room)
		hint("%s", title);
	ImGui::SameLine(0, 0);
	ImGui::SetCursorScreenPos(ImVec2(start.x + room - total, start.y));
	const ImVec2 p = ImGui::GetCursorScreenPos();
	const bool pressed = ImGui::InvisibleButton(id, ImVec2(total, line_h));
	toggle_hovered = ImGui::IsItemHovered();
	if (toggle_hovered)
		hint(knobs ? "いまは「つまみ」（値の棒で触る）。クリックで「グラフ」（絵で触る）に切り替える"
		           : "いまは「グラフ」（絵で触る）。クリックで「つまみ」（値の棒で触る）に切り替える");
	const ImU32 on = ImGui::GetColorU32(ImGuiCol_Text), off = ImGui::GetColorU32(ImGuiCol_TextDisabled);
	float sx = p.x;
	if (words) {
		dl->AddText(p, knobs ? off : on, l);
		dl->AddText(ImVec2(p.x + lw + gap * 2 + tw, p.y), knobs ? on : off, r);
		sx += lw + gap;
	}
	const float ty = p.y + (line_h - th) * 0.5f;
	const ImVec2 a(sx, ty), b(sx + tw, ty + th);
	dl->AddRectFilled(a, b, ImGui::GetColorU32(ImGui::IsItemHovered() ? ImGuiCol_FrameBgHovered : ImGuiCol_FrameBg), th * 0.5f);
	const float kx = knobs ? b.x - th * 0.5f : a.x + th * 0.5f;
	dl->AddCircleFilled(ImVec2(kx, ty + th * 0.5f), th * 0.38f, ImGui::GetColorU32(ImGuiCol_SliderGrabActive));
	return pressed;
}

// 1 つの区画。見出しと、大きな絵か値の棒（右上の切り替えで選ぶ。index が負なら絵は無く棒だけ）
template <typename Draw>
void panel(const char *id, const char *title, float w, float h, int part, xg::model &m, bridge &br,
           std::initializer_list<const char *> keys, int index, Draw draw, const char *about = nullptr)
{
	const float fs = ImGui::GetFontSize();
	// 見出しを枠の上端に寄せる（上下の余白を詰める）
	ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, ImVec2(ImGui::GetStyle().WindowPadding.x, fs * 0.1f));
	const bool open = ImGui::BeginChild(id, ImVec2(w, h), ImGuiChildFlags_Borders);
	ImGui::PopStyleVar();
	if (!open) {
		ImGui::EndChild();
		return;
	}
	const bool knobs = index < 0 || shapes_knobs(index);
	bool toggle_hovered = false;
	ImGui::PushFont(nullptr, fs * 0.8f);      // 見出しは小さめに
	if (index < 0)
		ImGui::TextUnformatted(title);
	else if (title_toggle(title, "##mode", knobs, toggle_hovered))
		set_shapes_knobs(index, !knobs);
	ImGui::PopFont();
	const std::string before = hint_text();
	if (!knobs) {
		// 絵だけ。区画の残りを全部使う
		const ImVec2 avail = ImGui::GetContentRegionAvail();
		draw(part, m, br, avail.x, std::max(fs * 4.0f, avail.y));
	} else {
		ImGui::PushItemWidth(-fs * 6.0f);
		for (const char *k : keys)
			param_slider(k, part, m, br);
		ImGui::PopItemWidth();
	}
	// カーソルの下の部品が説明を出さなかったら、区画そのものの説明を
	if (about && !toggle_hovered && hint_text() == before &&
	    ImGui::IsWindowHovered(ImGuiHoveredFlags_ChildWindows | ImGuiHoveredFlags_AllowWhenBlockedByActiveItem))
		hint("%s\n%s", title, about);
	ImGui::EndChild();
}

// ---- モジュレーションのマトリクス。操作子 6 つ（行）× 行き先 6 つ（列）の深さ
//
// 行き先の 3 つ（音程・切る高さ・音量）は「動かす」量で 64 が 0（±）、残りの 3 つは LFO の揺れの深さ（0-127）。
// マスの色は、± の量なら + が青・− が赤、深さなら橙で、濃さが量。既定から外れたマスは枠を明るく。
// マスを上下にドラッグ（2 ドットで 1）、マウスホイール（1 目で 1、Ctrl で 10）、ダブルクリックで既定に戻す
void mod_matrix(int part, xg::model &m, bridge &br, float w, float h)
{
	struct src { const char *key, *name, *about; };
	static const src SRCS[6] = {
		{ "mw",   "モジュレーション",   "モジュレーションホイール（CC1）" },
		{ "bend", "ピッチベンド",       "ピッチベンド（中央から離した量。向きは問わない）" },
		{ "cat",  "チャンネル AT",      "チャンネルアフタータッチ（鍵盤を押し込む強さ。チャンネルに 1 つ）" },
		{ "pat",  "ポリ AT",            "ポリアフタータッチ（鍵ごとの押し込む強さ）" },
		{ "ac1",  "AC1",                "AC1（AC1 CC No で選んだコントロールチェンジ）" },
		{ "ac2",  "AC2",                "AC2（AC2 CC No で選んだコントロールチェンジ）" },
	};
	struct dst { const char *key, *name, *sub; };
	static const dst DSTS[6] = {
		{ "pitch",    "音程",     "Pitch" },
		{ "filter",   "音色",     "Filter" },
		{ "amp",      "音量",     "Amp" },
		{ "lfo_pmod", "LFO 音程", "ビブラート" },
		{ "lfo_fmod", "LFO 音色", "ワウ" },
		{ "lfo_amod", "LFO 音量", "トレモロ" },
	};
	ImGuiIO &io = ImGui::GetIO();
	const float fs = ImGui::GetFontSize();
	ImDrawList *dl = ImGui::GetWindowDrawList();
	const ImVec2 org = ImGui::GetCursorScreenPos();
	const float head_w = std::min(fs * 8.0f, w * 0.26f);
	const float head_h = ImGui::GetTextLineHeight() * 2.2f;
	const float gap = std::max(2.0f, fs * 0.15f);
	const float cw = (w - head_w) / 6.0f, ch = std::max(fs * 1.6f, (h - head_h) / 6.0f);
	ImGuiStorage *st = ImGui::GetStateStorage();

	// 列の見出し（2 行: 行き先と、揺れなら何になるか）。動かすのと揺らすのの間に線
	for (int c = 0; c < 6; c++) {
		const float x = org.x + head_w + cw * float(c);
		const ImVec2 a = ImGui::CalcTextSize(DSTS[c].name), b = ImGui::CalcTextSize(DSTS[c].sub);
		dl->AddText(ImVec2(x + (cw - a.x) * 0.5f, org.y), ImGui::GetColorU32(ImGuiCol_Text), DSTS[c].name);
		dl->AddText(ImVec2(x + (cw - b.x) * 0.5f, org.y + ImGui::GetTextLineHeight()), ImGui::GetColorU32(ImGuiCol_TextDisabled), DSTS[c].sub);
	}
	{
		const float x = org.x + head_w + cw * 3.0f - gap * 0.5f;
		dl->AddLine(ImVec2(x, org.y), ImVec2(x, org.y + head_h + ch * 6.0f), ImGui::GetColorU32(ImGuiCol_Separator), 1.0f);
	}

	for (int r = 0; r < 6; r++) {
		const float y = org.y + head_h + ch * float(r);
		// 行の見出し。AC1・AC2 は CC の番号もここで（マウスホイールで変える）
		ImGui::SetCursorScreenPos(ImVec2(org.x, y));
		ImGui::PushID(r);
		ImGui::InvisibleButton("##row", ImVec2(head_w - gap, ch - gap));
		const bool row_hover = ImGui::IsItemHovered();
		dl->AddRectFilled(ImGui::GetItemRectMin(), ImGui::GetItemRectMax(), ImGui::GetColorU32(row_hover ? ImGuiCol_HeaderHovered : ImGuiCol_Header, 0.35f), 3.0f);
		std::string label = SRCS[r].name;
		if (r >= 4) {
			const std::string cck = std::string("part.") + SRCS[r].key + "_cc";
			const xg::param &cp = P(cck.c_str());
			int ccv = 0;
			if (m.get(cp, part, ccv)) {
				char b[32];
				std::snprintf(b, sizeof(b), "  CC%d", ccv);
				label += b;
				if (row_hover) {
					ImGui::SetItemKeyOwner(ImGuiKey_MouseWheelY);
					if (io.MouseWheel != 0.0f) {
						const int nv = std::clamp(ccv + (io.MouseWheel > 0 ? 1 : -1) * (io.KeyCtrl ? 10 : 1), cp.min, cp.max);
						if (nv != ccv)
							br.send(m.set(cp, part, nv));
					}
				}
			}
		}
		const ImVec2 ls = ImGui::CalcTextSize(label.c_str());
		dl->AddText(ImVec2(org.x + fs * 0.3f, y + (ch - gap - ls.y) * 0.5f), ImGui::GetColorU32(ImGuiCol_Text), label.c_str());
		if (row_hover) {
			if (r >= 4)
				hint("%s\n%s。この行の 6 つのマスが、この操作子で動かす量。見出しの上でマウスホイールを回すと CC の番号が変わる",
				     official_name((std::string("part.") + SRCS[r].key + "_cc").c_str()).c_str(), SRCS[r].about);
			else
				hint("%s\nこの行の 6 つのマスが、この操作子で動かす量", SRCS[r].about);
		}

		for (int c = 0; c < 6; c++) {
			const std::string key = std::string("part.") + SRCS[r].key + "_" + DSTS[c].key;
			const xg::param &p = P(key.c_str());
			int v = p.def;
			const bool known = m.get(p, part, v);
			const float x = org.x + head_w + cw * float(c);
			ImGui::SetCursorScreenPos(ImVec2(x, y));
			ImGui::PushID(c);
			ImGui::InvisibleButton("##cell", ImVec2(cw - gap, ch - gap));
			const ImGuiID id = ImGui::GetItemID();
			const bool hov = ImGui::IsItemHovered(), act = ImGui::IsItemActive();
			if (known) {
				int nv = v;
				if (ImGui::IsItemActivated()) {
					st->SetInt(id, v);
					st->SetFloat(id + 1, io.MousePos.y);
				}
				if (act && !ImGui::IsMouseDoubleClicked(ImGuiMouseButton_Left))
					nv = st->GetInt(id, v) + int((st->GetFloat(id + 1, io.MousePos.y) - io.MousePos.y) / 2.0f);
				if (hov) {
					ImGui::SetItemKeyOwner(ImGuiKey_MouseWheelY);
					if (io.MouseWheel != 0.0f)
						nv = v + (io.MouseWheel > 0 ? 1 : -1) * (io.KeyCtrl ? 10 : 1);
					if (ImGui::IsMouseDoubleClicked(ImGuiMouseButton_Left))
						nv = p.def;
				}
				nv = std::clamp(nv, p.min, p.max);
				if (nv != v) {
					drag_send(br, m.set(p, part, nv));
					v = nv;
				}
			}
			// 描く
			const ImVec2 a = ImGui::GetItemRectMin(), b = ImGui::GetItemRectMax();
			const bool bip = p.how == xg::view::center;
			float t = 0.0f;
			ImU32 fill;
			if (bip) {
				const float span = float(v >= p.center ? p.max - p.center : p.center - p.min);
				t = span > 0 ? float(v - p.center) / span : 0.0f;
				fill = t >= 0 ? IM_COL32(70, 140, 235, int(40 + 190 * t)) : IM_COL32(230, 80, 70, int(40 - 190 * t));
			} else {
				t = float(v - p.min) / float(std::max(1, p.max - p.min));
				fill = IM_COL32(235, 160, 50, int(30 + 200 * t));
			}
			dl->AddRectFilled(a, b, ImGui::GetColorU32(ImGuiCol_FrameBg), 3.0f);
			if ((bip && v != p.center) || (!bip && v != p.min))
				dl->AddRectFilled(a, b, fill, 3.0f);
			const bool changed = known && v != p.def;
			dl->AddRect(a, b, changed ? IM_COL32(250, 230, 150, 255)
			                          : ImGui::GetColorU32(hov || act ? ImGuiCol_Border : ImGuiCol_BorderShadow), 3.0f,
			            0, changed ? 1.5f : 1.0f);
			const std::string text = known ? xg::format(p, v) : std::string("--");
			const ImVec2 ts = ImGui::CalcTextSize(text.c_str());
			dl->AddText(ImVec2((a.x + b.x - ts.x) * 0.5f, (a.y + b.y - ts.y) * 0.5f),
			            ImGui::GetColorU32(known && ((bip && v != p.center) || (!bip && v != p.min)) ? ImGuiCol_Text : ImGuiCol_TextDisabled),
			            text.c_str());
			if (hov || act) {
				const char *help = help_for(key.c_str());
				const std::string to = c >= 3 ? std::string(DSTS[c].name) + "（" + DSTS[c].sub + "）" : std::string(DSTS[c].name);
				hint("%s  %s\n%s → %s。%s（上下にドラッグ・マウスホイール・ダブルクリックで既定の %s）",
				     official_name(key.c_str()).c_str(), text.c_str(), SRCS[r].name, to.c_str(), help ? help : "",
				     xg::format(p, p.def).c_str());
			}
			ImGui::PopID();
		}
		ImGui::PopID();
	}
	ImGui::SetCursorScreenPos(ImVec2(org.x, org.y + head_h + ch * 6.0f));
	ImGui::Dummy(ImVec2(w, 0));
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
	int scope = -1;                   // パートの音を拾うか（形のタブのフィルタが絵のときだけ）

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
		const float strip_h = overview::part_strip_height() + st.WindowPadding.y * 2.0f + fs * 0.2f;
		if (ImGui::BeginChild("strip", ImVec2(0, strip_h), ImGuiChildFlags_Borders, ImGuiWindowFlags_NoScrollbar))
			m_strip.part_strip(part, m, ram, br);
		ImGui::EndChild();
	}

	// ---- 左に音色を選ぶ面、右はタブ: 「形」は 4 つの区画（2 × 2）、「すべて」はパートのパラメータ全部
	const ImVec2 avail = ImGui::GetContentRegionAvail();
	// 音色を選ぶ面は、左に分類・右に音色とバンク違いの 2 列（xgui::program_pane）
	const float pane_w = std::min(fs * 15.6f, avail.x * 0.3f);     // 前の 6 割
	// 下の説明の帯（小さめの字で 3 行）。「説明を出す」を切っていれば帯ごと出さず、その高さを絵に回す
	const bool show_bar = help_on();
	ImGui::PushFont(nullptr, fs * BAR_SCALE);
	const float bar_h = show_bar ? ImGui::GetTextLineHeightWithSpacing() * 3.0f + st.WindowPadding.y * 2.0f + st.ItemSpacing.y : 0.0f;
	ImGui::PopFont();
	const float body_h = std::max(fs * 8.0f, avail.y - bar_h);

	if (ImGui::BeginChild("voicepane", ImVec2(pane_w, body_h)))
	{
		ImGui::PushFont(nullptr, fs * 0.85f);   // 分類・音色・バンク違いの 3 つは小さめの字で
		program_pane(part, m, &ram, br);
		ImGui::PopFont();
	}
	ImGui::EndChild();
	ImGui::SameLine();

	ImGui::BeginGroup();
	const float top_y = ImGui::GetCursorScreenPos().y;
	if (ImGui::BeginTabBar("right")) {
		if (ImGui::BeginTabItem("形")) {
			if (!shapes_knobs(1))
				scope = part;
			// 3 列。左に VIB（上）とモジュレーション（下）。中央と右は上下 2 段がつながった区画（メゾネット）で、
			// 中央がフィルタと EQ、右が EG とピッチ EG（どちらも上の段が絵、下の段がフェーダー）。
			// ポルタメントは「すべて」のタブにある
			const ImVec2 room = ImGui::GetContentRegionAvail();
			const float room_h = body_h - (ImGui::GetCursorScreenPos().y - top_y);
			const float w = (room.x - st.ItemSpacing.x * 2.0f) / 3.0f;
			const float h = (room_h - st.ItemSpacing.y) * 0.5f;
			ImGui::BeginGroup();
			panel("vib", "ビブラート（VIB）", w, h, part, m, br, { "part.vib_rate", "part.vib_depth", "part.vib_delay" }, 0,
			      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::vib_cell(p, mm, b, pw, ph, false); },
			      "音色の揺れ（ビブラート）。絵は実際の揺れで、右のフェーダーで速さ（Rate）・深さ（Depth）・"
			      "掛かり始めるまでの時間（Delay）を変える");
			panel("mod", "モジュレーション（MW）", w, h, part, m, br,
			      { "part.mw_lfo_pmod", "part.mw_pitch", "part.mw_filter", "part.mw_amp", "part.mw_lfo_fmod", "part.mw_lfo_amod" }, 5,
			      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::mod_cell(p, mm, b, pw, ph, false); },
			      "モジュレーションホイールを上げたときに足すビブラート。横がホイールの位置、縦が揺れの深さ。"
			      "左のホイールが CC1、右のホイールが MW LFO PM。音色自身の揺れ（背景の帯）とは足さず、深いほうが効く");
			ImGui::EndGroup();
			ImGui::SameLine();
			const float tall = h * 2.0f + st.ItemSpacing.y;
			panel("filter", "フィルタと EQ（FILTER・EQ）", w, tall, part, m, br,
			      { "part.cutoff", "part.resonance", "part.hpf_cutoff",
			        "part.eq_bass_gain", "part.eq_bass_freq", "part.eq_treble_gain", "part.eq_treble_freq" }, 1,
			      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::filter_cell(p, mm, b, pw, ph, false); },
			      "音の明るさ。横は実際の周波数で、緑がこのパートの今の音のスペクトラム。太線がフィルタとパートの EQ を"
			      "合わせた実際の特性（細線がフィルタだけ、点線が EQ だけ）。どちらも声ごとに掛かり、EQ はフィルタのすぐ後ろ"
			      "（インサーションより前）。下のフェーダーで Cutoff・Resonance・HPF と、EQ の低音・高音のゲインと周波数を変える");
			ImGui::SameLine();
			panel("env", "EG とピッチ EG（EG・PEG）", w, tall, part, m, br,
			      { "part.attack", "part.decay", "part.release",
			        "part.peg_init_level", "part.peg_attack_time", "part.peg_rel_level", "part.peg_rel_time" }, 2,
			      [](int p, xg::model &mm, bridge &b, float pw, float ph) { overview::env_cell(p, mm, b, pw, ph); },
			      "音量の形（青。立ち上がり → 落ち着き → 伸ばし → 離して消える）と音程の動き（橙）を、同じ実際の時間の目盛り・"
			      "同じ離す時刻で 1 枚に。縦は左が音量（dB）、右が音程（セント）。下のフェーダーで EG の Attack・Decay・Release と"
			      "ピッチ EG の Init・Attack・Rel Lv・Rel Tm を変える");
			ImGui::EndTabItem();
		}
		if (ImGui::BeginTabItem("マトリクス")) {
			// 操作子 6 つ × 行き先 6 つ（モジュレーションのマトリクス）
			const float room_h = body_h - (ImGui::GetCursorScreenPos().y - top_y);
			if (ImGui::BeginChild("matrix", ImVec2(0, room_h), ImGuiChildFlags_None, ImGuiWindowFlags_NoScrollbar)) {
				const ImVec2 r = ImGui::GetContentRegionAvail();
				mod_matrix(part, m, br, r.x, r.y);
			}
			ImGui::EndChild();
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

	// ---- 説明の帯。カーソルを載せた項目の説明（無ければ使い方のひとこと）。「説明を出す」を切れば出さない
	if (show_bar) {
	if (ImGui::BeginChild("hint", ImVec2(0, 0), ImGuiChildFlags_Borders)) {
		ImGui::PushFont(nullptr, fs * BAR_SCALE);
		const std::string &t = hint_text();
		if (t.empty()) {
			ImGui::TextDisabled("項目にカーソルを載せると、ここに説明が出る（「説明を出す」を切るとこの欄は消える）");
		} else {
			// 1 行目（区画や項目の名前）は色を変える
			const size_t nl = t.find('\n');
			ImGui::PushStyleColor(ImGuiCol_Text, IM_COL32(255, 214, 120, 255));
			ImGui::TextUnformatted(t.c_str(), t.c_str() + (nl == std::string::npos ? t.size() : nl));
			ImGui::PopStyleColor();
			if (nl != std::string::npos)
				ImGui::TextWrapped("%s", t.c_str() + nl + 1);
		}
		ImGui::PopFont();
	}
	ImGui::EndChild();
	}
	end_hint_bar();
	br.want_scope(scope);

	ImGui::PopFont();
	ImGui::End();
}

} // namespace ui
