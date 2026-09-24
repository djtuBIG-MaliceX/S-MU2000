// license:BSD-3-Clause
//
// エフェクトの種類とパラメータの説明（日本語・英語）。
//
// 文はこのリポジトリで書いたもの。何をする種類・パラメータなのかは、MU1000/MU2000 の
// オーナーズマニュアルと MU2000 Extended Edition 追加機能説明書のエフェクトタイプリストと
// パラメーター解説を読んで確かめた（文は写していない）。

#include "fx_help.h"

#include "ui/lang.h"
#include "xg_ui.h"

#include <cstring>

namespace ui {
namespace xgui {

namespace {

struct type_text { int msb, lsb; const char *ja, *en; };   // lsb が -1 なら同じ MSB 全部

const type_text TYPES[] = {
	{ 0x00, -1, "エフェクトを掛けない", "No effect." },
	{ 0x40, -1, "何もせずに素通しする（音はそのまま出る）", "Passes the sound through unchanged." },

	{ 0x01, -1, "ホールの響き。大きな空間の、長く豊かな残響", "Concert hall reverb: a large space with a long, rich tail." },
	{ 0x01, 0x06, "ホールの響き（中くらいのホール。GM Level 2 の音）", "Hall reverb, medium hall (GM Level 2)." },
	{ 0x01, 0x07, "ホールの響き（大きなホール。GM Level 2 の音）", "Hall reverb, large hall (GM Level 2)." },
	{ 0x02, -1, "部屋の響き。ホールより短く、近い壁からの反射", "Room reverb: shorter than a hall, with close reflections." },
	{ 0x02, 0x05, "小さな部屋の響き（GM Level 2 の音）", "Small room reverb (GM Level 2)." },
	{ 0x02, 0x06, "中くらいの部屋の響き（GM Level 2 の音）", "Medium room reverb (GM Level 2)." },
	{ 0x02, 0x07, "大きな部屋の響き（GM Level 2 の音）", "Large room reverb (GM Level 2)." },
	{ 0x03, -1, "ステージの響き。ソロの楽器を立たせる、明るめの残響", "Stage reverb: a bright reverb that suits solo instruments." },
	{ 0x04, -1, "プレートリバーブ（金属板を震わせて作る残響）の真似。密で明るい", "Plate reverb (a vibrating metal plate): dense and bright." },
	{ 0x04, 0x07, "プレートリバーブの真似（GM Level 2 の音）", "Plate reverb (GM Level 2)." },
	{ 0x10, -1, "少し遅れて始まる、短く癖のある響き", "A short, characterful reverb that starts after a small delay." },
	{ 0x11, -1, "左右に広がる筒の中のような響き", "Reverb of a tube-like space stretching left and right." },
	{ 0x12, -1, "果てしなく広がる、幻想的な響き", "An endlessly wide, dreamy space." },
	{ 0x13, -1, "少し遅れてから、独特の響きが来る地下室のような残響", "A basement-like reverb whose distinctive tail arrives after a short delay." },

	{ 0x05, -1, "左・右・真ん中の 3 本のディレイ（やまびこ）", "Three delays: left, right and centre." },
	{ 0x06, -1, "左右 2 本のディレイと、2 本の繰り返し（フィードバック）のディレイ", "Left and right delays with two separate feedback delays." },
	{ 0x07, -1, "左右 2 本のディレイで、左右それぞれが別々に繰り返すエコー", "Left and right delays, each repeating with its own feedback." },
	{ 0x08, -1, "2 本のディレイの繰り返しを左右で交差させる。音が左右を行き来する", "Two delays whose feedback crosses over, so repeats bounce between left and right." },
	{ 0x15, 0x00, "長さを音符で決める、テンポに合わせたディレイ（EX で追加）。シーケンサーから MIDI クロックを送ること。長すぎると自動で半分になる", "Tempo-synced delay, length set as a note value (added in the EX). Needs MIDI clock from the sequencer; halves itself if too long." },
	{ 0x15, 0x08, "長さを音符で決める、テンポに合わせたエコー（EX で追加）。MIDI クロックが要る", "Tempo-synced echo (added in the EX). Needs MIDI clock." },
	{ 0x16, -1, "長さを音符で決める、テンポに合わせたクロスディレイ（EX で追加）。MIDI クロックが要る", "Tempo-synced cross delay (added in the EX). Needs MIDI clock." },

	{ 0x09, -1, "残響のうち、最初に返ってくる反射音（初期反射）だけを取り出したもの", "Only the early reflections of a reverb." },
	{ 0x0a, -1, "途中でばっさり切れる残響（ゲートリバーブ）。80 年代のドラムの音", "Gated reverb: a reverb cut off abruptly, the classic 80s drum sound." },
	{ 0x0b, -1, "ゲートリバーブを逆向きに鳴らしたような、だんだん大きくなる響き", "Reverse gate: a gated reverb that swells up instead of decaying." },
	{ 0x14, -1, "カラオケ向けのエコー。声に繰り返しを付ける", "Karaoke-style echo for vocals." },

	{ 0x41, -1, "一般的なコーラス。少し揺らしたずれた音を重ねて、自然に広げる", "Standard chorus: adds slightly detuned, wavering copies to widen the sound." },
	{ 0x41, 0x03, "一般的なコーラス（GM Level 2 の音）", "Chorus (GM Level 2)." },
	{ 0x41, 0x04, "一般的なコーラス（GM Level 2 の音）", "Chorus (GM Level 2)." },
	{ 0x41, 0x05, "一般的なコーラス（GM Level 2 の音）", "Chorus (GM Level 2)." },
	{ 0x41, 0x06, "一般的なコーラス（GM Level 2 の音）", "Chorus (GM Level 2)." },
	{ 0x41, 0x07, "繰り返し（フィードバック）のあるコーラス。癖が強い（GM Level 2 の音）", "Chorus with feedback, more pronounced (GM Level 2)." },
	{ 0x42, -1, "3 つの揺れ（LFO）で、うねりと広がりを付ける", "Celeste: three LFOs give movement and width." },
	{ 0x43, -1, "ジェット機が通り過ぎるような、シュワーという音の動き（フランジャー）", "Flanger: a sweeping 'jet plane' sound." },
	{ 0x43, 0x07, "フランジャー（GM Level 2 の音）", "Flanger (GM Level 2)." },
	{ 0x44, -1, "セレステの揺れをもっと重ねた、厚い広がり", "Symphonic: a denser, multi-layered version of the celeste." },
	{ 0x48, -1, "位相を周期的に動かして、うねりを付ける（フェイザー）", "Phaser: sweeps the phase periodically for a swirling sound." },
	{ 0x57, -1, "わずかに音程をずらした音を重ねる。うねりの無いコーラス", "Ensemble detune: adds slightly detuned copies, a chorus without wobble." },
	{ 0x68, -1, "アナログ機器らしい癖を出したフランジャー（EX で追加）", "Flanger with an analogue character (added in the EX)." },
	{ 0x6b, -1, "揺れの速さを音符で決める、テンポに合わせたフランジャー（EX で追加）。MIDI クロックが要る", "Tempo-synced flanger, speed set as a note value (added in the EX). Needs MIDI clock." },
	{ 0x6c, -1, "揺れの速さを音符で決める、テンポに合わせたフェイザー（EX で追加）。MIDI クロックが要る", "Tempo-synced phaser (added in the EX). Needs MIDI clock." },
	{ 0x6e, -1, "入ってくる音の大きさで揺れの深さが変わるフランジャー（EX で追加）", "Dynamic flanger: depth follows the input level (added in the EX)." },
	{ 0x6f, -1, "入ってくる音の大きさで揺れの深さが変わるフェイザー（EX で追加）", "Dynamic phaser: depth follows the input level (added in the EX)." },

	{ 0x45, 0x00, "回転スピーカー（オルガンのレスリー）の真似。回る速さは AC1 などで切り替えられる", "Rotary speaker (Leslie). The speed can be switched with AC1 and similar controllers." },
	{ 0x45, 0x01, "ディストーションのあとに回転スピーカーをつないだもの", "Distortion followed by a rotary speaker." },
	{ 0x45, 0x02, "オーバードライブのあとに回転スピーカーをつないだもの", "Overdrive followed by a rotary speaker." },
	{ 0x45, 0x03, "アンプシミュレータのあとに回転スピーカーをつないだもの", "Amp simulator followed by a rotary speaker." },
	{ 0x46, -1, "音量を周期的に上げ下げする（トレモロ）", "Tremolo: moves the volume up and down periodically." },
	{ 0x47, -1, "音の位置を左右や前後に周期的に動かす（オートパン）", "Auto pan: moves the sound left/right or front/back periodically." },
	{ 0x47, 0x01, "動き方の曲線（波の形）を選べるオートパン（EX で追加）", "Auto pan with a selectable panning curve (added in the EX)." },
	{ 0x56, 0x00, "高音と低音のスピーカーが別々に回る回転スピーカー。AC1 などで速さを切り替え", "Two-rotor rotary speaker (separate horn and drum). AC1 and similar switch the speed." },
	{ 0x56, 0x01, "ディストーションのあとに 2 ウェイの回転スピーカーをつないだもの", "Distortion followed by a two-rotor rotary speaker." },
	{ 0x56, 0x02, "オーバードライブのあとに 2 ウェイの回転スピーカーをつないだもの", "Overdrive followed by a two-rotor rotary speaker." },
	{ 0x56, 0x03, "アンプシミュレータのあとに 2 ウェイの回転スピーカーをつないだもの", "Amp simulator followed by a two-rotor rotary speaker." },
	{ 0x63, -1, "回転スピーカーを 2 台並べてつないだもの", "Two rotary speakers in parallel." },

	{ 0x49, 0x00, "エッジの効いた強い歪み。ノイズゲート付きで、A/D 入力（外の音）にも向く", "Hard-edged distortion with a noise gate; also suits the A/D input." },
	{ 0x49, 0x01, "前にコンプレッサーがあるので、入ってくる音の大きさによらず揃って歪む", "Distortion with a compressor in front, so it distorts evenly whatever the input level." },
	{ 0x49, 0x08, "ディストーションの左右が別々のもの（ステレオ）", "Stereo distortion." },
	{ 0x4a, 0x00, "穏やかな歪み。ノイズゲート付きで、A/D 入力にも向く", "Mild overdrive with a noise gate; also suits the A/D input." },
	{ 0x4a, 0x08, "オーバードライブの左右が別々のもの（ステレオ）", "Stereo overdrive." },
	{ 0x4b, 0x00, "ギターアンプの真似。ノイズゲート付きで、A/D 入力にも向く", "Guitar amp simulator with a noise gate; also suits the A/D input." },
	{ 0x4b, 0x01, "歪み方の違う、新しい型のギターアンプの真似（EX で追加）", "A newer amp model with a different distortion character (added in the EX)." },
	{ 0x4b, 0x08, "アンプシミュレータの左右が別々のもの（ステレオ）", "Stereo amp simulator." },
	{ 0x62, 0x00, "真空管やファズの歪みの真似（強め）", "Vintage tube / fuzz distortion, hard type." },
	{ 0x62, 0x01, "真空管やファズの歪み（強め）のあとにディレイをつないだもの", "Vintage distortion (hard) followed by a delay." },
	{ 0x62, 0x02, "真空管やファズの歪みの真似（柔らかめ）", "Vintage tube / fuzz distortion, soft type." },
	{ 0x62, 0x03, "真空管やファズの歪み（柔らかめ）のあとにディレイをつないだもの", "Vintage distortion (soft) followed by a delay." },

	{ 0x4c, -1, "低・中・高の 3 つの帯を上げ下げできるイコライザ（モノ）", "3-band mono EQ: low, mid and high." },
	{ 0x4d, -1, "低・高の 2 つの帯を上げ下げできるイコライザ（ステレオ）。ドラムのパートに向く", "2-band stereo EQ: low and high. Suits drum parts." },
	{ 0x4e, 0x00, "ワウの中心の周波数を周期的に動かす。AC1 などでペダルワウにもなる", "Auto wah: sweeps the wah filter periodically. Can act as a pedal wah via AC1." },
	{ 0x4e, 0x01, "オートワウの出力をディストーションで歪ませたもの", "Auto wah into distortion." },
	{ 0x4e, 0x02, "オートワウの出力をオーバードライブで歪ませたもの", "Auto wah into overdrive." },
	{ 0x52, 0x00, "入ってくる音の大きさでワウの周波数が動く（タッチワウ）", "Touch wah: the wah frequency follows the input level." },
	{ 0x52, 0x08, "タッチワウ（もう 1 つの型。戻る速さも決められる）", "Touch wah, second type (with a release time)." },
	{ 0x52, 0x01, "タッチワウの出力をディストーションで歪ませたもの", "Touch wah into distortion." },
	{ 0x52, 0x02, "タッチワウの出力をオーバードライブで歪ませたもの", "Touch wah into overdrive." },
	{ 0x6d, -1, "入ってくる音の大きさでカットオフが動くフィルタ（EX で追加）", "Dynamic filter: the cutoff follows the input level (added in the EX)." },
	{ 0x74, -1, "分解能（ビットの細かさ）を落として、ローファイな感じを出す（EX で追加）", "Low resolution: reduces the resolution for a lo-fi feel (added in the EX)." },
	{ 0x73, -1, "低・中・高の帯ごとに音量を変えたり消したりする。DJ ミキサーのアイソレーター（EX で追加）", "Isolator: level and mute for the low, mid and high bands, like a DJ mixer (added in the EX)." },

	{ 0x53, -1, "決めた大きさを超えた音を抑える。アタック感を付けることもできる", "Compressor: holds down the level above a threshold; can also add punch." },
	{ 0x54, -1, "決めた大きさより小さい音を消す。A/D 入力の雑音を抑えるのに向く", "Noise gate: silences the signal below a threshold. Useful for the A/D input." },
	{ 0x69, -1, "帯ごとに別々に掛かるコンプレッサーの、型を選んで使う版（EX で追加）", "Multi-band compressor with preset types (added in the EX)." },

	{ 0x5f, 0x00, "ディストーションのあとにディレイをつないだもの", "Distortion followed by a delay." },
	{ 0x5f, 0x01, "オーバードライブのあとにディレイをつないだもの", "Overdrive followed by a delay." },
	{ 0x60, 0x00, "コンプレッサー → ディストーション → ディレイ", "Compressor, distortion and delay in series." },
	{ 0x60, 0x01, "コンプレッサー → オーバードライブ → ディレイ", "Compressor, overdrive and delay in series." },
	{ 0x61, 0x00, "タッチワウ → ディストーション → ディレイ", "Touch wah, distortion and delay in series." },
	{ 0x61, 0x01, "タッチワウ → オーバードライブ → ディレイ", "Touch wah, overdrive and delay in series." },

	{ 0x50, -1, "入ってくる音の音程を変えて重ねる（ピッチシフター）", "Pitch change: shifts the pitch of the input and mixes it in." },
	{ 0x51, -1, "新しい倍音を足して、音をきわ立たせる（エキサイター）", "Harmonic enhancer: adds new harmonics to make the sound stand out." },
	{ 0x55, -1, "CD などの真ん中にいるボーカルを小さくする", "Voice cancel: reduces the centred vocal of a CD or similar." },
	{ 0x58, -1, "音の位置をぼかして、空間の広がりを出す", "Ambience: blurs the stereo image for a sense of space." },
	{ 0x5d, -1, "入ってくる音に「ア・イ・ウ・エ・オ」のような母音の響きを付ける", "Talking modulator: gives the input a vowel-like character." },
	{ 0x5e, -1, "音質をわざと粗くする（ビット数とサンプリング周波数を落とす）", "Lo-fi: deliberately degrades the sound (lower bits and sample rate)." },
	{ 0x70, -1, "入ってくる音の大きさで深さが変わるリングモジュレーター（EX で追加）", "Dynamic ring modulator: depth follows the input level (added in the EX)." },
	{ 0x71, -1, "別の波を掛け合わせて、金属的な響きを足す（リングモジュレーター。EX で追加）", "Ring modulator: multiplies with a carrier wave for a metallic sound (added in the EX)." },
	{ 0x72, -1, "入ってくる音を、音符で決めた刻みでぶつ切りにする（EX で追加）。MIDI クロックが要る", "Slice: chops the input at a note-value rhythm (added in the EX). Needs MIDI clock." },
	{ 0x75, -1, "レコードを掛けたような雑音（針のプチプチなど）を足す（EX で追加）", "Digital turntable: adds record-like noise such as crackle (added in the EX)." },
	{ 0x76, -1, "レコードをこするスクラッチの効果を足す（EX で追加）", "Digital scratch: adds a record-scratch effect (added in the EX)." },
};

struct param_text { const char *label, *ja, *en; };

const param_text PARAMS[] = {
	// 残響・反射
	{ "ReverbTime", "残響の長さ", "Length of the reverb tail." },
	{ "Diffusion", "響きの広がり方。大きいほど滑らかに広がる", "How the sound spreads; higher is smoother and wider." },
	{ "InitDelay", "最初の反射音（またはディレイ）までの時間", "Time before the first reflection (or the delay)." },
	{ "InitDelayL", "左の最初の遅れ", "Initial delay of the left channel." },
	{ "InitDelayR", "右の最初の遅れ", "Initial delay of the right channel." },
	{ "Rev Delay", "初期反射から残響が始まるまでの時間", "Time from the early reflections to the start of the reverb." },
	{ "Density", "反射音の密度。大きいほどきめ細かい", "Density of the reflections; higher is finer." },
	{ "Er/Rev", "初期反射と残響の音量の釣り合い", "Balance between early reflections and reverb." },
	{ "High Damp", "高い音の減り方。小さいほど高い音が早く消える", "High-frequency damping; lower values make highs die away faster." },
	{ "Room Size", "部屋の大きさ。大きいほど反射が長く続く", "Room size; larger makes the reflections last longer." },
	{ "Liveness", "初期反射の減り方。小さいほど早く消える", "How the early reflections decay; lower dies away faster." },
	{ "Early Type", "初期反射の型", "Type of early reflections." },
	{ "GateType", "ゲートリバーブの型", "Type of gated reverb." },
	{ "Width", "真似する部屋の幅", "Width of the simulated room." },
	{ "Height", "真似する部屋の高さ", "Height of the simulated room." },
	{ "Depth", "真似する部屋の奥行き（揺れの種類では揺れの深さ）", "Depth of the simulated room (for modulation effects, the modulation depth)." },
	{ "Wall Vary", "壁の状態。大きいほど乱れて反射する", "Wall character; higher scatters the reflections more." },
	// ディレイ
	{ "LchDelay", "左のディレイの長さ", "Left channel delay time." },
	{ "Lch Delay", "左のディレイの長さ", "Left channel delay time." },
	{ "RchDelay", "右のディレイの長さ", "Right channel delay time." },
	{ "Rch Delay", "右のディレイの長さ", "Right channel delay time." },
	{ "CchDelay", "真ん中のディレイの長さ", "Centre channel delay time." },
	{ "Cch Level", "真ん中のディレイの音量", "Centre channel delay level." },
	{ "LchDelay2", "左の 2 本目のディレイの長さ", "Left channel second delay time." },
	{ "RchDelay2", "右の 2 本目のディレイの長さ", "Right channel second delay time." },
	{ "Delay2Lvl", "2 本目のディレイの音量", "Level of the second delay." },
	{ "Lch FBLevl", "左の繰り返しの量", "Left channel feedback amount." },
	{ "Rch FBLevl", "右の繰り返しの量", "Right channel feedback amount." },
	{ "FB Delay", "繰り返し（フィードバック）のディレイの長さ", "Feedback delay time." },
	{ "FBDelay1", "1 本目の繰り返しのディレイの長さ", "First feedback delay time." },
	{ "FBDelay2", "2 本目の繰り返しのディレイの長さ", "Second feedback delay time." },
	{ "FB Level", "繰り返しの量。マイナスは位相を反転して戻す（残響では初期反射の繰り返し）", "Feedback amount. Negative values feed back with inverted phase (in reverbs, the initial delay feedback)." },
	{ "L~R Delay", "左から入って右へ出るディレイの長さ", "Delay time from left input to right output." },
	{ "R~L Delay", "右から入って左へ出るディレイの長さ", "Delay time from right input to left output." },
	{ "Dly L>R", "左から入って右へ出るディレイの長さ（音符で決める）", "Left-to-right delay time, as a note value." },
	{ "Dly R>L", "右から入って左へ出るディレイの長さ（音符で決める）", "Right-to-left delay time, as a note value." },
	{ "InputSelect", "どちらのチャンネルの音を入れるか", "Which channel feeds the effect." },
	{ "DelayTime", "ディレイの長さ（テンポのディレイでは音符で決める。カラオケでは反射の間隔）", "Delay time (tempo delays: a note value; karaoke: the spacing of the echoes)." },
	{ "Delay", "ディレイの長さ", "Delay time." },
	{ "Delay Mix", "ディレイを混ぜる量", "How much delay is mixed in." },
	{ "DelayFBLvl", "ディレイの繰り返しの量", "Delay feedback amount." },
	{ "DlyTimeOfst", "ディレイの長さの細かいずらし", "Fine offset of the delay time." },
	// 揺れ
	{ "LFO Freq", "揺れの速さ", "Speed of the modulation." },
	{ "LFO Speed", "揺れの速さ", "Speed of the modulation." },
	{ "LFO Depth", "揺れの深さ", "Depth of the modulation." },
	{ "LFO Phase", "左右の揺れのずれ（64 でずれ無し）", "Phase difference between left and right modulation (64 = none)." },
	{ "LFO Wave", "揺れの波の形（AUTO PAN2 では位置の動き方の曲線）", "Modulation waveform (for AUTO PAN2, the panning curve)." },
	{ "DelayOfst", "揺らすディレイの基準の長さ。短いとフランジャー、長いとコーラスに近い", "Base delay that is modulated; short sounds like a flanger, longer like a chorus." },
	{ "ModDlyOfst", "揺らすディレイの基準の長さ", "Base delay that is modulated." },
	{ "Mod Depth", "揺れの深さ", "Modulation depth." },
	{ "Mod FB", "揺れの繰り返しの量", "Modulation feedback." },
	{ "Mod Mix", "揺らした（変調した）成分を混ぜる量", "Mix level of the modulated component." },
	{ "ModPhase", "揺れの波の位相", "Phase of the modulation waveform." },
	{ "PhaseShift", "位相を動かす基準の位置", "Base amount of phase shift." },
	{ "Stage", "フェイザーの段の数。多いほど癖が強い", "Number of phaser stages; more is stronger." },
	{ "AM Depth", "音量の揺れの深さ", "Depth of the volume modulation." },
	{ "PM Depth", "音程（ディレイ）の揺れの深さ", "Depth of the pitch (delay) modulation." },
	{ "L/R Depth", "左右に動く深さ", "Depth of the left/right movement." },
	{ "F/R Depth", "前後に動く深さ（PAN Dir が回る動きのとき）", "Depth of the front/back movement (when PAN Dir is a turning pattern)." },
	{ "PAN Dir", "音の動き方（左右の往復・回転など）", "Movement pattern (left-right, turning, and so on)." },
	{ "Pan Depth", "位置を動かす深さ", "Depth of the panning." },
	{ "Pan Type", "位置の動き方", "Panning pattern." },
	{ "AutoPanSpd", "オートパンの速さ", "Auto pan speed." },
	{ "AutoPanDpth", "オートパンの深さ", "Auto pan depth." },
	{ "Detune", "音程をずらす量", "Amount of detune." },
	// 回転スピーカー
	{ "RotorSpd", "スピーカーの回る速さ", "Rotor speed." },
	{ "RtrSpSlw", "低音のスピーカーの遅い回転の速さ", "Slow speed of the low rotor." },
	{ "RtrSpFst", "低音のスピーカーの速い回転の速さ", "Fast speed of the low rotor." },
	{ "HrnSpSlw", "高音のスピーカーの遅い回転の速さ", "Slow speed of the horn." },
	{ "HrnSpFst", "高音のスピーカーの速い回転の速さ", "Fast speed of the horn." },
	{ "Drive High", "高音のスピーカーの回転による揺れの深さ", "Modulation depth from the horn's rotation." },
	{ "Drive Low", "低音のスピーカーの回転による揺れの深さ", "Modulation depth from the low rotor's rotation." },
	{ "Low/High", "低音と高音のスピーカーの音量の釣り合い", "Balance between the low rotor and the horn." },
	{ "CrsoverFrq", "高音と低音のスピーカーに分ける周波数", "Crossover frequency between horn and low rotor." },
	{ "Mic Angle", "音を拾うマイクの左右の角度", "Angle between the left and right microphones." },
	{ "S/F Time H", "高音のスピーカーの速さを切り替えるのに掛かる時間", "Time the horn takes to change speed." },
	{ "S/F Time R", "低音のスピーカーの速さを切り替えるのに掛かる時間", "Time the low rotor takes to change speed." },
	// 歪み
	{ "Drive", "歪ませる度合い（エンハンサーなどでは効かせる度合い）", "Amount of distortion (for the enhancer and similar, amount of effect)." },
	{ "Dist Drive", "ディストーションの歪ませる度合い", "Distortion drive." },
	{ "DistOutLvl", "ディストーションの出力の音量", "Distortion output level." },
	{ "Overdrive", "歪ませる度合い", "Amount of overdrive." },
	{ "Edge", "歪み方のカーブ。大きいほど急に歪む、小さいほどじわじわ歪む", "Clip curve: higher distorts sharply, lower more gradually." },
	{ "AmpType", "真似するアンプの型", "Amp model to simulate." },
	{ "Device", "歪ませる機器の型", "Distorting device to simulate." },
	{ "Speaker", "真似するスピーカーの型", "Speaker cabinet to simulate." },
	{ "Presence", "高い音の出方を整える（ギターアンプのプレゼンス）", "Shapes the upper highs, like a guitar amp's presence control." },
	{ "DT LowGain", "歪みの低音の上げ下げ", "Low gain of the distortion." },
	{ "DT MidGain", "歪みの中音の上げ下げ", "Mid gain of the distortion." },
	{ "AnalogFeel", "アナログのフランジャーらしい音の癖を足す", "Adds the character of an analogue flanger." },
	// EQ・フィルタ
	{ "EQ LowFreq", "低い帯の周波数", "Frequency of the low EQ band." },
	{ "EQ LowGain", "低い帯の上げ下げ", "Gain of the low EQ band." },
	{ "EQ MidFreq", "真ん中の帯の周波数", "Frequency of the mid EQ band." },
	{ "EQ MidGain", "真ん中の帯の上げ下げ", "Gain of the mid EQ band." },
	{ "EQ MidWidt", "真ん中の帯の幅", "Width of the mid EQ band." },
	{ "EQHighFreq", "高い帯の周波数", "Frequency of the high EQ band." },
	{ "EQHighGain", "高い帯の上げ下げ", "Gain of the high EQ band." },
	{ "EQ Freq", "イコライザの周波数", "EQ frequency." },
	{ "EQ Gain", "イコライザの上げ下げ", "EQ gain." },
	{ "EQ Width", "イコライザの幅", "EQ width." },
	{ "Low Freq", "低い帯の周波数", "Low band frequency." },
	{ "Low Gain", "低い帯の上げ下げ", "Low band gain." },
	{ "Mid Freq", "真ん中の帯の周波数", "Mid band frequency." },
	{ "Mid Gain", "真ん中の帯の上げ下げ", "Mid band gain." },
	{ "Mid Width", "真ん中の帯の幅", "Mid band width." },
	{ "High Freq", "高い帯の周波数", "High band frequency." },
	{ "High Gain", "高い帯の上げ下げ", "High band gain." },
	{ "Low Level", "低い帯の音量", "Low band level." },
	{ "Mid Level", "真ん中の帯の音量", "Mid band level." },
	{ "HighLevel", "高い帯の音量", "High band level." },
	{ "Low Mute", "低い帯を消す", "Mutes the low band." },
	{ "Mid Mute", "真ん中の帯を消す", "Mutes the mid band." },
	{ "High Mute", "高い帯を消す", "Mutes the high band." },
	{ "LowGainOfst", "低い帯の上げ下げの基準", "Offset of the low band gain." },
	{ "MidGainOfst", "真ん中の帯の上げ下げの基準", "Offset of the mid band gain." },
	{ "HiGainOfst", "高い帯の上げ下げの基準", "Offset of the high band gain." },
	{ "HPF Cutoff", "これより低い音を削る（ハイパスフィルタ）", "High-pass filter: cuts below this frequency." },
	{ "LPF Cutoff", "これより高い音を削る（ローパスフィルタ）", "Low-pass filter: cuts above this frequency." },
	{ "LPF Reso", "ローパスフィルタの癖（カットオフのあたりの強調）", "Low-pass filter resonance." },
	{ "DryLPFFreq", "元の音に掛けるローパスフィルタの周波数", "Low-pass filter frequency applied to the dry sound." },
	{ "CutoffFreq", "ワウ（フィルタ）の中心の周波数の基準", "Base centre frequency of the wah (filter)." },
	{ "Resonance", "ワウ（フィルタ）の帯の鋭さ", "Sharpness of the wah (filter) band." },
	{ "Sensitivty", "入ってくる音の変化に、揺れやフィルタがどれだけ反応するか", "How strongly the modulation or filter reacts to changes in the input." },
	{ "FltrType", "フィルタの種類（LO-FI では音色の型）", "Filter type (for LO-FI, the tone type)." },
	{ "Freq Fine", "掛け合わせる波（キャリア）の周波数の細かい調整", "Fine carrier frequency." },
	{ "FreqCourse", "掛け合わせる波（キャリア）の周波数", "Coarse carrier frequency." },
	// ダイナミクス
	{ "Attack", "効き始めるまでの時間（ゲートでは開き始めるまで）", "Time until the effect kicks in (for a gate, until it opens)." },
	{ "AttackTime", "入ってくる音の大きさを追う速さ（立ち上がり）", "Attack time of the envelope follower." },
	{ "Release", "効き終わるまでの時間（ゲートでは閉じるまで、ワウでは周波数が戻るまで）", "Time until the effect lets go (gate: until it closes; wah: until the frequency returns)." },
	{ "RelesTime", "入ってくる音の大きさを追う速さ（戻り）", "Release time of the envelope follower." },
	{ "RelesCurve", "大きさを追うときの戻り方のカーブ", "Release curve of the envelope follower." },
	{ "Threshold", "効き始める入力の大きさ（ゲートでは開く大きさ）", "Input level at which it acts (for a gate, the opening level)." },
	{ "ThreshLevel", "大きさを追い始める入力のレベル", "Level at which the envelope follower starts to act." },
	{ "ThreshOfst", "コンプレッサーの型ごとの効き始める大きさを、ずらす量", "Offset added to the preset threshold of the compressor type." },
	{ "Ratio", "コンプレッサーの圧縮の比率", "Compression ratio." },
	{ "Gate Time", "ぶつ切りにした 1 つ 1 つの長さ", "Length of each sliced gate." },
	{ "LevelOfst", "大きさを追った結果に足す量", "Offset added to the envelope follower output." },
	{ "Lag", "音符で決めたディレイに、少しずれを付ける長さ", "Small offset added to the note-value delay." },
	// ピッチ・その他
	{ "Pitch", "半音で音程をずらす量", "Pitch shift in semitones." },
	{ "Fine 1", "1 本目の音程の細かいずらし", "Fine pitch of the first voice." },
	{ "Fine 2", "2 本目の音程の細かいずらし", "Fine pitch of the second voice." },
	{ "Pan 1", "1 本目の位置", "Pan of the first voice." },
	{ "Pan 2", "2 本目の位置", "Pan of the second voice." },
	{ "OutputLvl1", "1 本目の音量", "Level of the first voice." },
	{ "OutputLvl2", "2 本目の音量", "Level of the second voice." },
	{ "Mix Level", "元の音に混ぜるエフェクト音の量", "How much effect sound is mixed into the dry sound." },
	{ "HighAdjust", "小さくする帯の上側の周波数の調整", "Adjusts the upper edge of the band that is reduced." },
	{ "Low Adjust", "小さくする帯の下側の周波数の調整", "Adjusts the lower edge of the band that is reduced." },
	{ "OutPhase", "エフェクト音の位相を左右で入れ替える", "Swaps the phase of the effect sound between left and right." },
	{ "PhasInv", "右チャンネルの位相を反転する", "Inverts the phase of the right channel." },
	{ "Vowel", "付ける母音の種類", "Vowel to impose." },
	{ "Move Speed", "選んだ母音に移るまでの時間", "Time taken to move to the selected vowel." },
	{ "SmplFreq", "サンプリング周波数を落とす量", "Reduces the sampling frequency." },
	{ "WordLength", "音の粗さ（ビット数）", "Coarseness (bit depth)." },
	{ "Bit Assign", "音の粗さの掛かり方", "How the word length reduction is applied." },
	{ "Emphasis", "高い音の特性を変える", "Changes the high-frequency character." },
	{ "OutputGain", "出力の増減", "Output gain." },
	{ "Resoltn", "出力の波のビットの細かさ", "Bit resolution of the output." },
	{ "Direction", "大きさを追ったときに動かす向き", "Direction in which the envelope follower moves the effect." },
	{ "Reset", "揺れの始めの位相の揃え方", "How the LFO start phase is reset." },
	{ "On/Off", "アイソレーターの入り切り", "Isolator on/off." },
	{ "Aeg Phase", "刻みの音量の動きの位相", "Phase of the slice amplitude envelope." },
	{ "PanAegType", "刻みに合わせて位置を動かす型", "How the pan follows the slice envelope." },
	{ "PanAegLvl", "刻みに合わせて位置を動かすときの一番小さいレベル", "Minimum level when the pan follows the slice envelope." },
	{ "DivideType", "ぶつ切りにする刻み（音符で決める）", "Slicing rhythm, as a note value." },
	{ "DivideLvl", "ぶつ切りにしたときの一番小さい音量", "Minimum level between slices." },
	{ "ScratchSpd", "スクラッチの速さ", "Scratch speed." },
	{ "ScratchDpth", "スクラッチの深さ", "Scratch depth." },
	{ "DryToNoise", "元の音を雑音の側へ混ぜる量", "How much dry signal is sent into the noise." },
	{ "NoiseLevel", "雑音の音量", "Noise level." },
	{ "Noise Tone", "雑音の音色", "Noise tone." },
	{ "N.LPF Freq", "雑音に掛けるローパスフィルタの周波数", "Low-pass frequency applied to the noise." },
	{ "N.LPF Q", "雑音に掛けるローパスフィルタの癖", "Low-pass resonance applied to the noise." },
	{ "N.ModSpeed", "雑音の揺れの速さ", "Noise modulation speed." },
	{ "N.ModDepth", "雑音の揺れの深さ", "Noise modulation depth." },
	{ "ClickLevel", "針のプチプチ音の音量", "Level of the crackle clicks." },
	{ "ClikDensity", "針のプチプチ音の多さ", "Density of the crackle clicks." },
	{ "L/R Diff", "広がりを出すための、左右のディレイの差", "Difference between left and right delays, for width." },
	// 共通
	{ "Dry/Wet", "元の音（D）とエフェクト音（W）の釣り合い", "Balance between the dry (D) and effect (W) sound." },
	{ "Dry Level", "元の音の音量", "Level of the dry sound." },
	{ "OutputLvl", "出力の音量", "Output level." },
	{ "InputLevel", "入力の音量", "Input level." },
	{ "InputMode", "入力をモノにするかステレオにするか", "Mono or stereo input." },
	{ "Type", "型の選択", "Type selection." },
};

} // namespace

const char *fx_type_help(int msb, int lsb)
{
	const bool en = ui::show_english();
	for (const type_text &t : TYPES)
		if (t.msb == msb && t.lsb == lsb)
			return en ? t.en : t.ja;
	for (const type_text &t : TYPES)
		if (t.msb == msb && t.lsb < 0)
			return en ? t.en : t.ja;
	return nullptr;
}

const char *fx_param_help(const char *label)
{
	const bool en = ui::show_english();
	for (const param_text &p : PARAMS)
		if (!std::strcmp(p.label, label))
			return en ? p.en : p.ja;
	return nullptr;
}

} // namespace xgui
} // namespace ui
