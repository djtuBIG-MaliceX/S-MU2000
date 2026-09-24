// license:BSD-3-Clause
//
// 画面の口。AUv2 の au/editor.h と同じ役目で、こちらは AUv3 の 2 つの出し方が
// 同じ 1 枚を使うための約束。
//
// macOS の AUv3 で画面が出るかどうかは拡張の種類で決まる。拡張の Info.plist が
// com.apple.AudioUnit-UI を名乗っていないと、ホストは AU を鳴らせても画面を
// 頼みに来ない（platform がそう決めている。AudioToolbox の AUAudioUnit.h の
// providesUserInterface の説明がそのまま書いている）。画面を持つ種類の拡張では
// **principal class 自身が NSViewController で、AU の工場も兼ねる** -- Apple の
// 雛形（Audio Unit Extension.xctemplate の
// _UISpecific/Common/UI/AudioUnitViewController.swift）が
// AUViewController と AUAudioUnitFactory を 1 つのクラスにしている。
//
// だから factory.mm と audio_unit.mm の両方がこの 1 枚を使う:
//   拡張の principal class  → パネルを自分の view に貼る（factory.mm）
//   AU がホストへ渡す画面    → requestViewController の答え（audio_unit.mm）

#ifndef S_MU2000_AUV3_VIEW_CONTROLLER_H
#define S_MU2000_AUV3_VIEW_CONTROLLER_H

#pragma once

#import <AudioToolbox/AudioToolbox.h>
#import <Cocoa/Cocoa.h>
#import <CoreAudioKit/CoreAudioKit.h>

// パネルが描く engine。VST3 の型は出てこないので、前方宣言だけで足りる
namespace smu2000 {
namespace vst3 {
class engine;
} // namespace vst3
} // namespace smu2000

// パネル 1 枚。中身は src/vst3/panel_nsview.mm が作る NSView そのもので、
// AUv2・VST3 が出すのと同じ 1 枚。絵の二重持ちはしない
@interface SMU2000ViewControllerV3 : AUViewController
- (instancetype)initWithEngine:(smu2000::vst3::engine *)eng audioUnit:(AUAudioUnit *)au;
@end

#endif // S_MU2000_AUV3_VIEW_CONTROLLER_H
