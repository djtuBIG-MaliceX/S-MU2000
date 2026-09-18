// license:BSD-3-Clause
//
// AUv3 の入口。.appex の NSExtensionPrincipalClass がこれ。
//
// macOS では、AUv3 の画面が出るかどうかが拡張の種類で決まる。Info.plist の
// NSExtensionPointIdentifier が com.apple.AudioUnit-UI の拡張は「画面を持てる」
// 拡張で、システムは principal class を NSViewController として読み、AU の工場も
// そこに頼む。com.apple.AudioUnit（画面を持たない方）だと、音はきちんと出るのに
// ホストは画面を頼みに来ない -- 記録に「画面を頼まれた」が 1 行も残っていなかった
// のがこれで、AU 側の requestViewController が呼ばれることも無かった。
//
// だから 1 つのクラスが両方を兼ねる:
//   createAudioUnitWithComponentDescription:error:  AU を作る（工場）
//   loadView                                        パネルを貼る（画面）
// Apple の雛形（Audio Unit Extension.xctemplate の
// _UISpecific/Common/UI/AudioUnitViewController.swift）が AUViewController と
// AUAudioUnitFactory を 1 つのクラスにしているのと同じ形。
//
// パネルそのものは view_controller.mm の 1 枚で、AUv2・VST3 と同じ plug_view を貼る

#import "audio_unit.h"
#import "view_controller.h"

#import <AudioToolbox/AudioToolbox.h>
#import <Cocoa/Cocoa.h>
#import <CoreAudioKit/CoreAudioKit.h>

// パネルが用意できるまでの枠。ホストは AU より先に画面を頼むことがあるので
// （Apple の雛形も viewDidLoad で AU を作らせる）、どちらが先でも貼れる形にする
static const CGFloat kEmptyWidth  = 640;
static const CGFloat kEmptyHeight = 180;

@interface SMU2000FactoryV3 : AUViewController <AUAudioUnitFactory>
@end

// 台は XPC の糸が作り、画面は主の糸が貼る。どちらから読んでも壊れないよう
// atomic な property で持ち回す（既定で atomic）
@interface SMU2000FactoryV3 ()
@property (atomic, strong) SMU2000AudioUnitV3 *unit;
@end

@implementation SMU2000FactoryV3 {
	SMU2000ViewControllerV3 *_panel;
	NSView *_frame;
}

// 拡張が AU を 1 台作るように頼まれる。ここで作った台の engine がパネルの描く先
- (AUAudioUnit *)createAudioUnitWithComponentDescription:(AudioComponentDescription)desc
                                                   error:(NSError **)error
{
	self.unit = [[SMU2000AudioUnitV3 alloc] initWithComponentDescription:desc error:error];
	[self installPanel];   // この呼びは XPC の糸に来るので、中で主の糸へ渡す
	return self.unit;
}

- (void)loadView
{
	_frame = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, kEmptyWidth, kEmptyHeight)];
	self.view = _frame;
	[self installPanel];
}

// 台と枠のどちらも揃ったときに 1 度だけ貼る。**view を触る仕事は主の糸で**
//
// AU を作れと言われるのは拡張の XPC の糸（AURemoteExtensionContext open:）で、
// そこから NSViewController に触ると AppKit が例外を投げる -- パネルの loadView は
// preferredContentSize を立てるので、まさにその据えが非主糸では通らない。
// 投げた例外は ExtensionFoundation の中まで抜けて、拡張ごと落ちる
// （症状は「AUv3 が開けない」-- auval も OpenAComponent が 4099 で失敗していた）。
// Apple の雛形も同じ理屈で、AU を DispatchQueue.main.sync の中で作り、画面の
// 拵えは DispatchQueue.main.async へ回している
- (void)installPanel
{
	if (![NSThread isMainThread]) {
		dispatch_async(dispatch_get_main_queue(), ^{ [self installPanel]; });
		return;
	}
	SMU2000AudioUnitV3 *unit = self.unit;
	if (!_frame || !unit || _panel)
		return;
	_panel = [[SMU2000ViewControllerV3 alloc] initWithEngine:[unit panelEngine]
	                                              audioUnit:unit];
	if (!_panel.view)
		return;
	_panel.view.frame = _frame.bounds;
	_panel.view.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
	[_frame addSubview:_panel.view];
	self.preferredContentSize = _panel.preferredContentSize;
}

@end
