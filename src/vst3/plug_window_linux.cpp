// license:BSD-3-Clause
//
// The Linux half of the VST3 view's window seam (plug_window.h).
//
// Headless for now: attached() says no, so hosts fall back to their generic
// parameter UI while the DSP side sings. The type string is still X11, which
// is what a future X11 window will answer to. See doc/porting-linux-gui.md.

#include "vst3/plug_window.h"

#include "pluginterfaces/gui/iplugview.h"

namespace smu2000 {
namespace vst3 {

const char *plug_window_type()
{
	return Steinberg::kPlatformTypeX11EmbedWindowID;
}

namespace {

class linux_window : public plug_window
{
public:
	explicit linux_window(plug_view &) {}

	bool attach(void *, int, int) override { return false; }
	void detach() override {}
	void set_size(int, int) override {}
	void card_menu(int, int) override {}
	void alert(const std::string &) override {}
};

} // namespace

plug_window *plug_window_create(plug_view &owner)
{
	return new linux_window(owner);
}

} // namespace vst3
} // namespace smu2000
