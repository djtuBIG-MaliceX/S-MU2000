// license:BSD-3-Clause
//
// The probe's host window on macOS: an NSWindow the plugin's editor is attached
// to, and the run loop that serves it.
//
// Objective-C++ only because it holds Cocoa classes. This file deliberately
// sees neither the panel nor gdi.h; it only knows a window and a view to put
// in it.
//
// The run loop is the real one ([NSApp run]) rather than a hand-rolled event
// pump. That matters here: the plugin's view installs a repaint timer on the
// current run loop, and its editor only animates if that loop is turning.

#include "probe_host.h"

#include "pluginterfaces/gui/iplugview.h"

#import <Cocoa/Cocoa.h>

namespace smu2000 {
namespace vst3 {

namespace {

// Closes a window that has been asked to go away, and stops the run loop with
// it. [NSApp stop:] only takes effect once one more event has been handled, so
// the event is posted right behind it -- the usual way to end a run loop early
void stop_app()
{
	[NSApp stop:nil];
	NSEvent *wake = [NSEvent otherEventWithType:NSEventTypeApplicationDefined
	                                   location:NSZeroPoint
	                              modifierFlags:0
	                                  timestamp:0
	                               windowNumber:0
	                                    context:nil
	                                    subtype:0
	                                      data1:0
	                                      data2:0];
	[NSApp postEvent:wake atStart:YES];
}

} // namespace

} // namespace vst3
} // namespace smu2000


// A window that ends the run loop when the user closes it, so the probe quits
// instead of pumping events at a window that is gone
@interface SMUProbeWindowDelegate : NSObject <NSWindowDelegate>
@end

@implementation SMUProbeWindowDelegate

- (void)windowWillClose:(NSNotification *)note
{
	(void)note;
	smu2000::vst3::stop_app();
}

@end


namespace smu2000 {
namespace vst3 {

namespace {

class mac_host : public probe_host
{
public:
	~mac_host() override { destroy(); }

	const char *platform_type() const override { return Steinberg::kPlatformTypeNSView; }

	bool create(int w, int h) override;
	bool attach(Steinberg::IPlugView *view) override;
	void show() override;
	void pump(double seconds) override;
	void destroy() override;

private:
	NSWindow *m_window = nil;
	NSView   *m_content = nil;
	SMUProbeWindowDelegate *m_delegate = nil;
};

bool mac_host::create(int w, int h)
{
	// A plain executable still has to become an application before it can own
	// a window; unlike the GUI front end this one stays out of the Dock
	NSApplication *app = [NSApplication sharedApplication];
	[app setActivationPolicy:NSApplicationActivationPolicyAccessory];

	NSWindowStyleMask style = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
	                          NSWindowStyleMaskMiniaturizable;
	m_window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, w, h)
	                                       styleMask:style
	                                         backing:NSBackingStoreBuffered
	                                           defer:NO];
	if (!m_window)
		return false;

	[m_window setTitle:@"S-MU2000 probe host"];
	m_delegate = [[SMUProbeWindowDelegate alloc] init];
	[m_window setDelegate:m_delegate];

	// The plugin attaches to this, exactly as it would to a DAW's view
	m_content = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, w, h)];
	[m_window setContentView:m_content];
	return true;
}

bool mac_host::attach(Steinberg::IPlugView *view)
{
	if (!m_content)
		return false;
	return view->attached((__bridge void *)m_content, Steinberg::kPlatformTypeNSView) ==
	       Steinberg::kResultOk;
}

void mac_host::show()
{
	[m_window center];
	[m_window makeKeyAndOrderFront:nil];
	[NSApp activateIgnoringOtherApps:YES];
}

void mac_host::pump(double seconds)
{
	// Stop the loop after the requested time, and wake it so the stop is seen
	[NSTimer scheduledTimerWithTimeInterval:seconds
	                                repeats:NO
	                                  block:^(NSTimer *timer) {
		(void)timer;
		stop_app();
	}];
	[NSApp run];
}

void mac_host::destroy()
{
	if (m_window) {
		[m_window setDelegate:nil];
		[m_window close];
		m_window = nil;
		m_content = nil;
		m_delegate = nil;
	}
}

} // namespace


probe_host *probe_host_create()
{
	return new mac_host;
}

} // namespace vst3
} // namespace smu2000
