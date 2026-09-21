// license:BSD-3-Clause

#include "pc_window.h"

#include "imgui.h"
#include "backends/imgui_impl_dx11.h"
#include "backends/imgui_impl_win32.h"

#include <commdlg.h>
#include <cstdio>
#include <d3d11.h>
#include <iterator>
#include <shellapi.h>
#include <vector>

extern IMGUI_IMPL_API LRESULT ImGui_ImplWin32_WndProcHandler(HWND hWnd, UINT msg, WPARAM wParam, LPARAM lParam);

namespace ui {

namespace {

const wchar_t CLASS_NAME[] = L"SMU2000PcEditor";

// 日本語の出る字。Windows に入っているものを順に探す（配らない）
const char *const FONTS[] = {
	"C:\\Windows\\Fonts\\YuGothM.ttc",
	"C:\\Windows\\Fonts\\meiryo.ttc",
	"C:\\Windows\\Fonts\\msgothic.ttc",
};

bool file_exists(const char *path)
{
	return GetFileAttributesA(path) != INVALID_FILE_ATTRIBUTES;
}

// .syx の書き出し・読み込みの窓（xgui::ask_save_file・ask_open_file の頼み）。
// 描き終えたあとに開く。窓が回っている間にタイマーが別のコマを描いても、前のコマは終わっている
void file_dialog(HWND owner, xgui::file_ask ask, const std::vector<u8> &bytes)
{
	wchar_t path[MAX_PATH * 4] = {};
	if (ask == xgui::file_ask::save)
		wcscpy_s(path, L"S-MU2000.syx");
	OPENFILENAMEW o{};
	o.lStructSize = sizeof(o);
	o.hwndOwner   = owner;
	o.lpstrFilter = L"SysEx (*.syx)\0*.syx\0すべてのファイル\0*.*\0";
	o.lpstrFile   = path;
	o.nMaxFile    = DWORD(std::size(path));
	o.lpstrDefExt = L"syx";
	if (ask == xgui::file_ask::save) {
		o.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST;
		if (!GetSaveFileNameW(&o))
			return;
		std::FILE *f = _wfopen(path, L"wb");
		const bool ok = f && std::fwrite(bytes.data(), 1, bytes.size(), f) == bytes.size();
		if (f)
			std::fclose(f);
		char note[64];
		std::snprintf(note, sizeof(note), ok ? "書き出した（%zu バイト）" : "書き出せなかった", bytes.size());
		xgui::set_file_note(note);
		return;
	}
	o.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
	if (!GetOpenFileNameW(&o))
		return;
	std::vector<u8> in;
	if (std::FILE *f = _wfopen(path, L"rb")) {
		u8 buf[65536];
		size_t n;
		while ((n = std::fread(buf, 1, sizeof(buf), f)) > 0 && in.size() < (16u << 20))
			in.insert(in.end(), buf, buf + n);
		std::fclose(f);
		xgui::give_opened_file(std::move(in));
	} else {
		xgui::set_file_note("読めなかった");
	}
}

} // namespace


pc_window::~pc_window()
{
	destroy();
}

bool pc_window::visible() const
{
	return m_hwnd && IsWindowVisible(m_hwnd);
}

void pc_window::shutdown(bridge &br)
{
	if (!m_imgui)
		return;
	ImGui::SetCurrentContext(m_imgui);
	m_view->hidden(br);
}

bool pc_window::show(HINSTANCE inst, std::string &err)
{
	if (!m_hwnd && !create(inst, err))
		return false;
	ShowWindow(m_hwnd, SW_SHOWNORMAL);
	SetForegroundWindow(m_hwnd);
	return true;
}

bool pc_window::create(HINSTANCE inst, std::string &err)
{
	xgui::set_file_dialogs(true);
	WNDCLASSEXW wc{};
	wc.cbSize        = sizeof(wc);
	wc.style         = CS_CLASSDC;
	wc.lpfnWndProc   = proc;
	wc.hInstance     = inst;
	wc.hCursor       = LoadCursor(nullptr, IDC_ARROW);
	wc.lpszClassName = CLASS_NAME;
	RegisterClassExW(&wc);          // 2 回目は失敗するが、登録済みなので構わない

	// DPI 対応は宣言しない（プロセス全体に効いて、パネルの窓の見え方まで変わるため）。
	// 高 DPI の画面では Windows が窓ごと拡大する
	const float scale = 1.0f;

	m_hwnd = CreateWindowExW(0, CLASS_NAME, m_view->title(), WS_OVERLAPPEDWINDOW,
	                         CW_USEDEFAULT, CW_USEDEFAULT, int(m_view->default_width() * scale), int(m_view->default_height() * scale),
	                         nullptr, nullptr, inst, this);
	if (!m_hwnd) {
		err = "エディタの窓を出せない";
		return false;
	}
	SetWindowLongPtrW(m_hwnd, GWLP_USERDATA, LONG_PTR(this));
	if (s_drop)
		DragAcceptFiles(m_hwnd, TRUE);

	if (!create_device(err)) {
		destroy();
		return false;
	}

	IMGUI_CHECKVERSION();
	// 窓ごとに文脈を持つ（ImGui の口は今の文脈に付く）
	m_imgui = ImGui::CreateContext();
	ImGui::SetCurrentContext(m_imgui);
	ImGuiIO &io = ImGui::GetIO();
	io.ConfigFlags |= ImGuiConfigFlags_NavEnableKeyboard;
	io.IniFilename = nullptr;       // 作業フォルダに imgui.ini を撒かない

	ImGui::StyleColorsDark();
	ImGuiStyle &style = ImGui::GetStyle();
	style.ScaleAllSizes(scale);
	style.FontScaleDpi = scale;
	style.FrameRounding = 3;

	for (const char *path : FONTS) {
		if (file_exists(path) && io.Fonts->AddFontFromFileTTF(path, 16.0f))
			break;
	}

	ImGui_ImplWin32_Init(m_hwnd);
	ImGui_ImplDX11_Init(m_dev, m_ctx);
	return true;
}

bool pc_window::create_device(std::string &err)
{
	DXGI_SWAP_CHAIN_DESC sd{};
	sd.BufferCount       = 2;
	sd.BufferDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
	sd.BufferUsage       = DXGI_USAGE_RENDER_TARGET_OUTPUT;
	sd.OutputWindow      = m_hwnd;
	sd.SampleDesc.Count  = 1;
	sd.Windowed          = TRUE;
	sd.SwapEffect        = DXGI_SWAP_EFFECT_DISCARD;

	const D3D_FEATURE_LEVEL levels[] = { D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_0 };
	D3D_FEATURE_LEVEL got;
	HRESULT hr = D3D11CreateDeviceAndSwapChain(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0, levels, 2,
	                                           D3D11_SDK_VERSION, &sd, &m_swap, &m_dev, &got, &m_ctx);
	if (hr == DXGI_ERROR_UNSUPPORTED)        // GPU が無い機械。ソフトウェアで描く
		hr = D3D11CreateDeviceAndSwapChain(nullptr, D3D_DRIVER_TYPE_WARP, nullptr, 0, levels, 2,
		                                   D3D11_SDK_VERSION, &sd, &m_swap, &m_dev, &got, &m_ctx);
	if (FAILED(hr)) {
		char buf[96];
		std::snprintf(buf, sizeof(buf), "Direct3D 11 を使えない（0x%08lx）", (unsigned long)hr);
		err = buf;
		return false;
	}
	make_target();
	return true;
}

void pc_window::make_target()
{
	ID3D11Texture2D *back = nullptr;
	if (SUCCEEDED(m_swap->GetBuffer(0, __uuidof(ID3D11Texture2D), (void **)&back))) {
		m_dev->CreateRenderTargetView(back, nullptr, &m_rtv);
		back->Release();
	}
}

void pc_window::drop_target()
{
	if (m_rtv) { m_rtv->Release(); m_rtv = nullptr; }
}

void pc_window::destroy()
{
	if (m_imgui) {
		ImGui::SetCurrentContext(m_imgui);
		ImGui_ImplDX11_Shutdown();
		ImGui_ImplWin32_Shutdown();
		ImGui::DestroyContext(m_imgui);
		m_imgui = nullptr;
	}
	drop_target();
	if (m_swap) { m_swap->Release(); m_swap = nullptr; }
	if (m_ctx)  { m_ctx->Release();  m_ctx = nullptr; }
	if (m_dev)  { m_dev->Release();  m_dev = nullptr; }
	if (m_hwnd) {
		SetWindowLongPtrW(m_hwnd, GWLP_USERDATA, 0);
		DestroyWindow(m_hwnd);
		m_hwnd = nullptr;
	}
}

void pc_window::frame(xg::model &m, const xg_snapshot &ram, bridge &br)
{
	const bool shown = m_imgui && visible() && !IsIconic(m_hwnd);
	if (m_was_visible && !shown && m_imgui) {
		ImGui::SetCurrentContext(m_imgui);
		m_view->hidden(br);                  // 閉じた・しまった。押しっぱなしを離す
	}
	m_was_visible = shown;
	if (!shown)
		return;
	ImGui::SetCurrentContext(m_imgui);
	if (m_resize_w) {
		drop_target();
		m_swap->ResizeBuffers(0, m_resize_w, m_resize_h, DXGI_FORMAT_UNKNOWN, 0);
		m_resize_w = m_resize_h = 0;
		make_target();
	}

	ImGui_ImplDX11_NewFrame();
	ImGui_ImplWin32_NewFrame();
	ImGui::NewFrame();
	m_view->draw(m, ram, br);
	ImGui::Render();

	const float clear[4] = { 0.10f, 0.10f, 0.11f, 1.0f };
	m_ctx->OMSetRenderTargets(1, &m_rtv, nullptr);
	m_ctx->ClearRenderTargetView(m_rtv, clear);
	ImGui_ImplDX11_RenderDrawData(ImGui::GetDrawData());
	// 待たない。gui のタイマー（30 コマ／秒）が間隔を決める
	m_swap->Present(0, 0);

	std::vector<u8> bytes;
	const xgui::file_ask ask = xgui::take_file_ask(bytes);
	if (ask != xgui::file_ask::none)
		file_dialog(m_hwnd, ask, bytes);
}

LRESULT CALLBACK pc_window::proc(HWND h, UINT msg, WPARAM wp, LPARAM lp)
{
	auto *self = reinterpret_cast<pc_window *>(GetWindowLongPtrW(h, GWLP_USERDATA));
	if (self && self->m_imgui) {
		ImGui::SetCurrentContext(self->m_imgui);
		if (ImGui_ImplWin32_WndProcHandler(h, msg, wp, lp))
			return 1;
	}

	switch (msg) {
	case WM_SIZE:
		if (self && wp != SIZE_MINIMIZED) {
			self->m_resize_w = LOWORD(lp);
			self->m_resize_h = HIWORD(lp);
		}
		return 0;
	case WM_SYSCOMMAND:
		if ((wp & 0xfff0) == SC_KEYMENU)     // Alt で窓の品書きに入らない
			return 0;
		break;
	case WM_CLOSE:
		ShowWindow(h, SW_HIDE);              // 消さずに隠す
		return 0;
	case WM_DROPFILES: {
		// 落とされたファイルの 1 つ目だけを渡す
		const HDROP drop = HDROP(wp);
		wchar_t path[MAX_PATH * 4] = {};
		const bool got = DragQueryFileW(drop, 0, path, UINT(std::size(path))) > 0;
		DragFinish(drop);
		if (got && s_drop)
			s_drop(path);
		return 0;
	}
	}
	return DefWindowProcW(h, msg, wp, lp);
}

} // namespace ui
