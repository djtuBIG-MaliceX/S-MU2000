// license:BSD-3-Clause
//
// AUv3 の画面。AUv2・VST3 と同じパネルを、AUv3 の作法で出す。
//
// 絵の二重持ちはしない。パネルを貼った NSView は src/vst3/panel_nsview.mm が
// 1 枚だけ作り、AUv2（editor_mac.mm）も VST3 も同じものを出す。この file の
// 仕事は AUv3 の口の形だけ: それを NSViewController に載せてホストへ渡す。
// AUv2 は AUCocoaUIBase の工場経由で、AUv3 は requestViewController への答えと
// して。AUv2 側は engine を間接参照（kEngineProperty）で取るが、こちらは
// AUAudioUnit と一緒に生まれるので指し示す先を直接渡せる。
//
// AUAudioUnit を panel_nsview に渡すのは、ホストが渡した画面より先に台を
// 手放すことがあるから（auval がそうする）。中の plug_view は engine 越しに
// 書き戻して終わるので、画面が台より長生きする形にしておく -- 詳しくは
// panel_nsview.h の make_panel_view の注

#import <Cocoa/Cocoa.h>

#import <AudioToolbox/AudioToolbox.h>
#import <CoreAudioKit/CoreAudioKit.h>

#import "view_controller.h"

#include "vst3/engine.h"
#include "vst3/panel_nsview.h"

@implementation SMU2000ViewControllerV3 {
	smu2000::vst3::engine *_eng;
	// The machine itself; the panel view holds the same one. AUAudioUnit does
	// not refer back to this controller, so it is not a cycle
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
	// NSZeroSize: 大きさはパネルに任せる（AUv2 はホストの言い値を渡す）
	NSView *panel = _eng ? smu2000::vst3::make_panel_view(*_eng, NSZeroSize, _au) : nil;
	self.view = panel ? panel
	                  : [[NSView alloc] initWithFrame:NSMakeRect(0, 0, smu2000::vst3::kPanelMinW,
	                                                             smu2000::vst3::kPanelMinH)];
	self.preferredContentSize = self.view.frame.size;
}

@end
