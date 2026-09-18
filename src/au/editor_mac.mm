// license:BSD-3-Clause
//
// The AU's editor: the machine's front panel, in Cocoa.
//
// There is no second copy of the panel in here. A host reads
// kAudioUnitProperty_CocoaUI, makes the class at the bottom of this file and
// calls uiViewForAudioUnit:withSize:; what it gets back is the *same* view the
// VST3 build shows -- smu2000::vst3::plug_view, which is src/vst3/view.cpp
// drawing through compat/gdi_mac.cpp, inside the NSView from
// src/vst3/view_mac.mm. Only the way a host asks for it differs, which is the
// whole reason the two formats can share one engine.
//
// Objective-C++ for the same reason view_mac.mm is: compat/gdi.h and Cocoa both
// define BOOL, and Quickdraw defines Polygon, so the drawing layer is reached
// only through the void* entry points on plug_view and never included here.

#import <Cocoa/Cocoa.h>

#include <AudioToolbox/AudioToolbox.h>
// AUCocoaUIBase itself lives here, and AudioToolbox.h does not pull it in
#import <AudioToolbox/AUCocoaUIView.h>

#include "editor.h"

#include "vst3/engine.h"
#include "vst3/plug_window.h"
#include "vst3/view.h"

using smu2000::vst3::plug_view;

namespace smu2000 {
namespace au {

// Must match the @interface below: this is the string a host hands to
// NSClassFromString
const char *const kViewClassName = "SMU2000AUViewFactory";

// The panel's own size, and the smallest a host can ask for before it is given
// the panel's size rather than a squeezed one. The same numbers and the same
// clamping the VST3 view applies in onSize() / checkSizeConstraint()
constexpr int kPanelWidth  = 1400;
constexpr int kPanelHeight = 360;
constexpr int kMinWidth    = 640;
constexpr int kMinHeight   = 180;

bool view_info(CFURLRef *out_bundle_url, CFStringRef *out_class_name)
{
	*out_bundle_url  = nullptr;
	*out_class_name  = nullptr;

	// The class below lives in this bundle, so asking for it by name answers
	// with wherever the host has put us -- build/ while developing, the
	// Components directory once installed. No path is written down
	NSString *name = [NSString stringWithUTF8String:kViewClassName];
	Class cls = name ? NSClassFromString(name) : Nil;
	NSBundle *bundle = cls ? [NSBundle bundleForClass:cls] : nil;
	NSURL *url = [bundle bundleURL];
	if (!url)
		return false;

	// One reference for the host on each. The property publishes them and hosts
	// do not all release what they read, so a fresh pair is made per call
	// rather than a shared pair being handed out over and over
	CFURLRef cf_url = (CFURLRef)CFRetain((__bridge CFTypeRef)url);
	CFStringRef cf_name = CFStringCreateWithCString(kCFAllocatorDefault, kViewClassName,
	                                                kCFStringEncodingUTF8);
	if (!cf_url || !cf_name) {
		if (cf_url)
			CFRelease(cf_url);
		if (cf_name)
			CFRelease(cf_name);
		return false;
	}

	*out_bundle_url = cf_url;
	*out_class_name = cf_name;
	return true;
}

} // namespace au
} // namespace smu2000


// The view a host is handed. It is only a frame: the panel, the child view that
// paints it and the input handling all belong to the plug_view inside, which
// this holds for as long as it lives
@interface SMUAUEditorView : NSView
{
@public
	plug_view *_plug;
}
@end

@implementation SMUAUEditorView

- (BOOL)isFlipped { return YES; }

// The panel handles the clicks, but a host that hands the click to this frame
// (a window that does not take key focus makes the first click a "first mouse"
// one) should get the same answer as the panel does
- (BOOL)acceptsFirstMouse:(NSEvent *)event
{
	(void)event;
	return YES;
}

- (void)dealloc
{
	// plug_view counts its own references (it implements FUnknown's addRef /
	// release); the factory below took one, and this gives it back
	if (_plug)
		_plug->release();
}

// A host that lets the editor window be resized moves this view's frame. The
// panel and the child view inside have to follow it, which is plug_view::onSize
- (void)setFrameSize:(NSSize)size
{
	[super setFrameSize:size];
	if (!_plug)
		return;
	Steinberg::ViewRect r(0, 0, (Steinberg::int32)size.width, (Steinberg::int32)size.height);
	_plug->onSize(&r);
}

@end


// Builds one editor for an open instance, or nil if it cannot be built. A
// factory function: every call makes a new view, as AUCocoaUIBase requires
static NSView *make_editor(AudioUnit unit, NSSize preferred)
{
	// The engine of the open instance. AudioUnitGetProperty runs the ordinary
	// property dispatch, so plugin.cpp's answer already knows which instance
	// this is -- see the note about the handle in editor.h
	void *eng = nullptr;
	UInt32 size = sizeof(eng);
	if (AudioUnitGetProperty(unit, smu2000::au::kEngineProperty, kAudioUnitScope_Global, 0,
	                         &eng, &size) != noErr || !eng)
		return nil;

	plug_view *plug = new plug_view(*static_cast<smu2000::vst3::engine *>(eng));

	int w = (int)preferred.width;
	int h = (int)preferred.height;
	if (w < smu2000::au::kMinWidth || h < smu2000::au::kMinHeight) {
		w = smu2000::au::kPanelWidth;
		h = smu2000::au::kPanelHeight;
	}
	// onSize clamps the way the VST3 host's size is clamped, and resizes the
	// panel to match, so the frame below is what the panel was laid out for
	Steinberg::ViewRect r(0, 0, w, h);
	plug->onSize(&r);
	w = plug->width();
	h = plug->height();

	SMUAUEditorView *view = [[SMUAUEditorView alloc] initWithFrame:NSMakeRect(0, 0, w, h)];
	// attached() answers a VST3 tresult, where kResultOk is 0 -- so this is a
	// comparison and not a truth test, or a view that attached perfectly would
	// be thrown away
	if (plug->attached((__bridge void *)view, smu2000::vst3::plug_window_type()) != Steinberg::kResultOk) {
		plug->release();
		return nil;
	}
	view->_plug = plug;
	return view;
}


// What kViewClassName names. A host allocs it, asks for the interface version
// (0 is the only one there has ever been), and then asks for the view
@interface SMU2000AUViewFactory : NSObject <AUCocoaUIBase>
@end

@implementation SMU2000AUViewFactory

- (unsigned)interfaceVersion { return 0; }

- (NSView *)uiViewForAudioUnit:(AudioUnit)inAudioUnit withSize:(NSSize)inPreferredSize
{
	return make_editor(inAudioUnit, inPreferredSize);
}

// Hosts show this in the menu that picks between a unit's views. The protocol
// asks for a copy rather than a static string
- (NSString *)description { return [@"S-MU2000 Panel" copy]; }

@end
