// license:BSD-3-Clause
//
// The host window a probe stands in for.
//
// vst3probe's --view mode puts the plugin's editor on screen, which means it
// has to do what a DAW does: make a parent window, hand it to
// IPlugView::attached(), and pump events. Everything else about the probe is
// shared, so only that part is per platform -- the same split as the view
// itself (plug_window.h).
//
// probe_host_mac.mm includes this, so nothing here may mention a Windows type
// or pull in compat/gdi.h.

#ifndef S_MU2000_VST3_PROBE_HOST_H
#define S_MU2000_VST3_PROBE_HOST_H

#pragma once

namespace Steinberg { class FUnknown; class IPlugView; }

namespace smu2000 {
namespace vst3 {

class probe_host
{
public:
	virtual ~probe_host() = default;

	// The string to pass to attached() and to ask isPlatformTypeSupported()
	// about: kPlatformTypeHWND on Windows, kPlatformTypeNSView on macOS
	virtual const char *platform_type() const = 0;

	virtual bool create(int w, int h) = 0;

	// Put the plugin's view inside this window, the way a host does
	virtual bool attach(Steinberg::IPlugView *view) = 0;

	virtual void show() = 0;

	// Serve events for this many seconds, then return. Blocks
	virtual void pump(double seconds) = 0;

	virtual void destroy() = 0;
};

// Makes this platform's host window. The caller owns it
probe_host *probe_host_create();

} // namespace vst3
} // namespace smu2000

#endif // S_MU2000_VST3_PROBE_HOST_H
