// license:BSD-3-Clause
//
// AUv3 の本体。中身は src/vst3/engine.h の engine（VST3 と同じもの）。

#ifndef S_MU2000_AUV3_AUDIO_UNIT_H
#define S_MU2000_AUV3_AUDIO_UNIT_H

#pragma once

#import <AudioToolbox/AudioToolbox.h>

NS_ASSUME_NONNULL_BEGIN

// パネルが描く engine。VST3 の型は出てこないので、前方宣言だけで足りる
namespace smu2000 {
namespace vst3 {
class engine;
} // namespace vst3
} // namespace smu2000

/// MU2000 一台。実機の端子をそのまま口にしてある。
///
///   出力 0   MAIN OUT L/R（PHONES と DIGITAL OUT も同じ信号）
///   入力 0   A/D INPUT（AD1 が左、AD2 が右）
///   MIDI 入  ケーブル 0 = MIDI IN A（パート 1-16）、1 = MIDI IN B（パート 17-32）、
///             2 = MIDI IN C（パート 33-48）、3 = MIDI IN D（パート 49-64）。
///             C・D は実機では USB だけの口
///   MIDI 出  MIDI OUT（firmware の返事。XG の問い合わせやダンプ要求への応答）
///
/// AUv2（SMU2/Trbh）とは種別を変えてある（SMU3）ので、両方入れても取り違えない。
@interface SMU2000AudioUnitV3 : AUAudioUnit

/// この台が回している engine。画面（view_controller.mm のパネル）へ渡すためだけの
/// 口。AUv2 は AU のハンドルから辿れないので kEngineProperty を使うが、AUv3 は
/// 拡張の中で AU のオブジェクトを直に持っているので、そのまま渡せる
- (smu2000::vst3::engine *)panelEngine;

@end

NS_ASSUME_NONNULL_END

#endif // S_MU2000_AUV3_AUDIO_UNIT_H
