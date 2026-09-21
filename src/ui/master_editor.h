// license:BSD-3-Clause
//
// マスターの窓（doc/pc-editor.md）。一覧のマスターの行（MASTER の名前か MASTER EQ の絵）を
// ダブルクリックすると開く。一覧の小さなマスター EQ は見るだけなので、触るのはこの窓で。
//
//   上の左   システム: マスターボリューム、マスターチューン、移調
//   上の右   システムエフェクト: リバーブ・コーラス・バリエーションの種類、戻り量、パン、ほかへの送り、
//            バリエーションの接続と掛けるパート
//   下       マスター EQ: 種類、帯 1・5 の形、5 つの帯の特性（点をつまむ）と、帯ごとのゲイン・周波数・Q の棒
//
// 表示の大きさは xgui::master_zoom（既定 0.8）。窓の大きさもその分だけ小さくしてある

#ifndef S_MU2000_UI_MASTER_EDITOR_H
#define S_MU2000_UI_MASTER_EDITOR_H

#pragma once

#include "xg_ui.h"

namespace ui {

class master_editor : public imgui_view
{
public:
	const wchar_t *title() const override { return L"S-MU2000 マスター"; }
	int default_width() const override  { return 900; }
	int default_height() const override { return 560; }
	void draw(xg::model &m, const xg_snapshot &ram, bridge &br) override;

private:
	// .syx の書き出し・読み込み（issue #35）
	void sysex_pane(const xg_snapshot &ram, bridge &br);
	bool m_diff_only = true;              // 既定と違うものだけ書き出す
	bool m_export_waiting = false;        // 既定値ができるのを待っている（bridge の request_defaults）
	std::vector<u8> m_import;             // 読み込んだ中身。1 通ずつ音源へ流す
	size_t m_import_at = 0;
	u64 m_import_hold_until = 0;          // XG System On などの後は少し待つ（音源の時計、ミリ秒）
};

} // namespace ui

#endif // S_MU2000_UI_MASTER_EDITOR_H
