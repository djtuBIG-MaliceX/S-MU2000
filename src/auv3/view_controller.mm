// license:BSD-3-Clause
//
// AUv3 の画面。AUv2・VST3 と同じパネルを、AUv3 の作法で出す。
//
// 絵の二重持ちはしない。AUv2 の editor_mac.mm が作るのと同じ
// smu2000::vst3::plug_view（src/vst3/view.cpp が compat/gdi_mac.cpp 越しに描く）を
// NSView に貼って、NSViewController ごとホストへ渡す。AUv2 と違うのは口の形だけ:
// AUv2 は AUCocoaUIBase の工場経由で、AUv3 は requestViewController への答えとして。
// AUv2 側は engine を間接参照（kEngineProperty）で取るが、こちらは
// AUAudioUnit と一緒に生まれるので指し示す先を直接渡せる。

#import <Cocoa/Cocoa.h>

#import <AudioToolbox/AudioToolbox.h>
#import <CoreAudioKit/CoreAudioKit.h>

#import "view_controller.h"

#include "au/editor.h"
#include "vst3/engine.h"
#include "vst3/plug_window.h"
#include "vst3/view.h"

#include <cstdio>

using smu2000::vst3::plug_view;

// AUv2 の editor_mac.mm と同じ数え。窓の大きさも、潰れたときの敷居も揃える
constexpr int kPanelWidth  = 1400;
constexpr int kPanelHeight = 360;
constexpr int kMinWidth    = 640;
constexpr int kMinHeight   = 180;


// 中身は plug_view が持つ。枠でしかない点は AUv2 の SMUAUEditorView と同じ
@interface SMU2000PanelViewV3 : NSView
{
@public
	plug_view *_plug;
}
// The machine the panel draws, held by the panel itself.
//
// The engine belongs to the AU (SMU2000AudioUnitV3's _engine), and the host is
// free to let go of the AU before the view it was handed -- auval does. The
// panel is then left pointing at a freed engine, and its own teardown ends up
// there: plug_view's destructor writes the card back through the engine, so it
// locks a mutex on freed memory
//
//   mutex lock failed: Invalid argument -> std::terminate
//
// and the whole extension goes with it. What died is the view service, so from
// the host it reads as "asked for a view, got nothing" -- the second reason the
// AUv3 showed no interface.
//
// Holding it here puts ARC's release of the AU after this view's dealloc has
// finished, so plug_view is always torn down while the engine is still there
@property (nonatomic, strong) AUAudioUnit *unit;
@end

@implementation SMU2000PanelViewV3

- (BOOL)isFlipped { return YES; }

// For the reason the AUv2 and VST3 views give in view_mac.mm: a host that puts
// the editor in a window which never takes key focus spends the click activating
// the window, and the panel never hears about it
- (BOOL)acceptsFirstMouse:(NSEvent *)event
{
	(void)event;
	return YES;
}

- (void)dealloc
{
	// plug_view first: the engine (unit, above) is still alive here
	if (_plug)
		_plug->release();
}

// ホストが摘んで大きさを変えたら、パネルと中の子窓を追いかける
- (void)setFrameSize:(NSSize)size
{
	[super setFrameSize:size];
	if (!_plug)
		return;
	Steinberg::ViewRect r(0, 0, (Steinberg::int32)size.width, (Steinberg::int32)size.height);
	_plug->onSize(&r);
}

@end


// 開いている台の engine から 1 枚作る。AUv2 の make_editor と同じ組み立て
static NSView *make_panel(smu2000::vst3::engine *eng, AUAudioUnit *unit)
{
	if (!eng)
		return nil;
	eng->log_line("画面を作る");
	plug_view *plug = new plug_view(*eng);
	Steinberg::ViewRect r(0, 0, kPanelWidth, kPanelHeight);
	plug->onSize(&r);
	SMU2000PanelViewV3 *view =
	    [[SMU2000PanelViewV3 alloc] initWithFrame:NSMakeRect(0, 0, plug->width(), plug->height())];
	const Steinberg::tresult ar =
	    plug->attached((__bridge void *)view, smu2000::vst3::plug_window_type());
	if (ar != Steinberg::kResultOk) {
		char b[64];
		std::snprintf(b, sizeof(b), "画面を貼れない: %d", int(ar));
		eng->log_line(b);
		plug->release();
		return nil;
	}
	view->_plug = plug;
	// So the panel outlives the engine it draws (note on the @property above)
	view.unit = unit;
	eng->log_line("画面ができた");
	return view;
}


@implementation SMU2000ViewControllerV3 {
	smu2000::vst3::engine *_eng;
	// The machine itself; the panel (SMU2000PanelViewV3) holds the same one.
	// AUAudioUnit does not refer back to this controller, so it is not a cycle
	AUAudioUnit *_au;
}

- (instancetype)initWithEngine:(smu2000::vst3::engine *)eng audioUnit:(AUAudioUnit *)au
{
	self = [super initWithNibName:nil bundle:nil];
	if (self) {
		_eng = eng;
		_au = au;
	}
	return self;
}

- (void)loadView
{
	NSView *panel = make_panel(_eng, _au);
	self.view = panel ? panel : [[NSView alloc] initWithFrame:NSMakeRect(0, 0, kMinWidth, kMinHeight)];
	self.preferredContentSize = self.view.frame.size;
}

@end
