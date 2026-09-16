// license:BSD-3-Clause
//
// The probe's host window on Windows: a top-level window the plugin's editor is
// attached to, and the message loop that serves it. Extracted from probe.cpp
// when the macOS port arrived, so that probe.cpp could stop including
// windows.h.
//
// The resize behaviour matters: a host is expected to resize the child view
// when the parent changes, and a probe that did not would hide bugs in the
// plugin's onSize().

#include "probe_host.h"

#include "pluginterfaces/gui/iplugview.h"

#include <windows.h>

#include <chrono>
#include <cstdio>
#include <thread>

namespace smu2000 {
namespace vst3 {

namespace {

LRESULT CALLBACK host_proc(HWND h, UINT msg, WPARAM wp, LPARAM lp)
{
	if (msg == WM_DESTROY) { PostQuitMessage(0); return 0; }
	if (msg == WM_SIZE) {
		// 親が変わったら中身も合わせる（DAW も同じことをする）
		HWND child = GetWindow(h, GW_CHILD);
		if (child)
			MoveWindow(child, 0, 0, LOWORD(lp), HIWORD(lp), TRUE);
		return 0;
	}
	return DefWindowProcA(h, msg, wp, lp);
}

class win_host : public probe_host
{
public:
	~win_host() override { destroy(); }

	const char *platform_type() const override { return Steinberg::kPlatformTypeHWND; }

	bool create(int w, int h) override;
	bool attach(Steinberg::IPlugView *view) override;
	void show() override;
	void pump(double seconds) override;
	void destroy() override;

private:
	HWND m_hwnd = nullptr;
};

bool win_host::create(int w, int h)
{
	WNDCLASSA wc{};
	wc.lpfnWndProc   = host_proc;
	wc.hInstance     = GetModuleHandleA(nullptr);
	wc.hCursor       = LoadCursor(nullptr, IDC_ARROW);
	wc.lpszClassName = "SMU2000ProbeHost";
	RegisterClassA(&wc);

	RECT want{ 0, 0, w, h };
	AdjustWindowRect(&want, WS_OVERLAPPEDWINDOW, FALSE);
	m_hwnd = CreateWindowA("SMU2000ProbeHost", "S-MU2000 probe host",
	                       WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT,
	                       want.right - want.left, want.bottom - want.top,
	                       nullptr, nullptr, wc.hInstance, nullptr);
	return m_hwnd != nullptr;
}

bool win_host::attach(Steinberg::IPlugView *view)
{
	return view->attached(m_hwnd, Steinberg::kPlatformTypeHWND) == Steinberg::kResultOk;
}

void win_host::show() { ShowWindow(m_hwnd, SW_SHOW); }

void win_host::pump(double seconds)
{
	const DWORD end = GetTickCount() + DWORD(seconds * 1000.0);
	MSG msg;
	while (GetTickCount() < end) {
		while (PeekMessageA(&msg, nullptr, 0, 0, PM_REMOVE)) {
			if (msg.message == WM_QUIT)
				return;
			TranslateMessage(&msg);
			DispatchMessageA(&msg);
		}
		std::this_thread::sleep_for(std::chrono::milliseconds(10));
	}
}

void win_host::destroy()
{
	if (m_hwnd) {
		DestroyWindow(m_hwnd);
		m_hwnd = nullptr;
	}
}

} // namespace


probe_host *probe_host_create()
{
	return new win_host;
}

} // namespace vst3
} // namespace smu2000
