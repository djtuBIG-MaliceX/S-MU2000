// license:BSD-3-Clause
//
// The Linux half of vst3probe's host window stand-in (probe_host.h).
//
// Headless: there is no X11 window yet (doc/porting-linux-gui.md), so create()
// says no and the probe's --view mode stops with "親の窓を作れない". Every
// headless mode -- describe, render, torture, automation -- is unaffected.

#include "vst3/probe_host.h"

#include "pluginterfaces/gui/iplugview.h"

namespace smu2000 {
namespace vst3 {

namespace {

class linux_host : public probe_host
{
public:
	const char *platform_type() const override
	{
		return Steinberg::kPlatformTypeX11EmbedWindowID;
	}

	bool create(int, int) override { return false; }
	bool attach(Steinberg::IPlugView *) override { return false; }
	void show() override {}
	void pump(double) override {}
	void destroy() override {}
};

} // namespace

probe_host *probe_host_create()
{
	return new linux_host();
}

} // namespace vst3
} // namespace smu2000
