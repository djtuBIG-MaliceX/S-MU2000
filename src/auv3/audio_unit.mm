// license:BSD-3-Clause
//
// AUv3 の本体。音源そのものは src/vst3/engine.h の engine（VST3 と同じもの）で、
// ここがやるのは AUv3 の作法に合わせることだけ。
//
// 実機の端子をそのまま口にする:
//
//   出力 0    MAIN OUT L/R。PHONES と DIGITAL OUT にも同じ信号が出ている
//   入力 0    A/D INPUT。AD1 が左、AD2 が右（サンプリングと A/D の系統に入る）
//   MIDI 入   ケーブル 0 = MIDI IN A（パート 1-16）
//             ケーブル 1 = MIDI IN B（パート 17-32）
//             ケーブル 2 = MIDI IN C（パート 33-48）
//             ケーブル 3 = MIDI IN D（パート 49-64）
//             C・D は実機では USB だけの口（engine は既定で USB 側で起動する）
//   MIDI 出   MIDI OUT（SH7043 の SCI ch0）。firmware が送り出したもの
//
// VST3 は MIDI 入力 2 本と A/D INPUT までで、MIDI OUT を出していない。
// AUv3 には MIDI 出力の口があるので、そこは実機に合わせて足した。
//
// MU2000 の中身は 44100Hz でしか動かないので、ホストの標本化周波数への変換は
// engine が自前の sinc でやる。その遅れは latency で申告する。
//
// 描き出しの中では確保も錠もしない。器は allocateRenderResources で取る。

#import "audio_unit.h"
#import "view_controller.h"

#import <AVFoundation/AVFoundation.h>
#import <Cocoa/Cocoa.h>
#import <CoreAudioKit/CoreAudioKit.h>
#import <CoreMIDI/CoreMIDI.h>

#include "ui/midi_split.h"
#include "vst3/engine.h"

#include <algorithm>
#include <atomic>
#include <cstdio>
#include <memory>
#include <vector>

namespace {

constexpr AUAudioFrameCount MAX_FRAMES = 4096;
// MIDI IN A-D。機械の口の数から取るので、増えたら付いてくる
constexpr int PORTS = mu2000::MIDI_PORTS;

// 描き出しの中で使う器。確保はここではしない（allocateRenderResources で済ませる）
struct scratch {
	std::vector<float> out_l, out_r;
	std::vector<float> in_l, in_r;
	uint8_t            in_abl_mem[sizeof(AudioBufferList) + sizeof(AudioBuffer)] = {};
	std::vector<uint8_t> tx;
	ui::midi_split split;
	// Sounded channels, 16 bits per port. Stop hushing goes only there:
	// every channel would cost 61ms of 31250bps serial per port (issue #15)
	std::atomic<UInt16> sounded[PORTS] = {};
	// UMP の SysEx7 を組み立てる途中。1 メッセージぶん溜まったら engine へ
	std::vector<uint8_t> ump_sysex;
	// 出力レベル。目標は engine の panel が持ち、ここは 1 サンプルずつ寄せる途中の値
	// （VST3・CLAP と同じ掛け方。一気に変えると音が跳ねる）
	float gain_now = 1.0f;

	AudioBufferList *in_abl() { return reinterpret_cast<AudioBufferList *>(in_abl_mem); }

	void allocate(AUAudioFrameCount n)
	{
		out_l.assign(n, 0.0f);  out_r.assign(n, 0.0f);
		in_l.assign(n, 0.0f);   in_r.assign(n, 0.0f);
		tx.assign(4096, 0);
		split.reset();
		ump_sysex.clear();
	}
};

// MIDIOutputEventBlock へ 1 メッセージ渡すときの持ち物
struct emit_ctx {
	AUMIDIOutputEventBlock __unsafe_unretained block;
	AUEventSampleTime when;
};

void emit_one(void *ctx, const uint8_t *bytes, size_t n)
{
	emit_ctx *e = static_cast<emit_ctx *>(ctx);
	if (e->block)
		e->block(e->when, 0, NSInteger(n), bytes);
}

// Silence only the channels that sounded (see scratch::sounded): hushing
// every channel would cost 61ms of 31250bps serial per port (issue #15)
void hush_engine(smu2000::plug::engine *eng, scratch *sc)
{
	if (!eng || !sc)
		return;
	uint16_t mask[mu2000::MIDI_PORTS] = {};
	bool any = false;
	for (int p = 0; p < PORTS; p++) {
		mask[p] = sc->sounded[p].exchange(0);
		any = any || mask[p];
	}
	if (any)
		eng->all_notes_off(mask, mu2000::MIDI_PORTS);
}

// UMP（MIDI 1.0）の event-list を昔のバイト列に戻して engine へ。
// 声とシステムはそのまま渡し、SysEx7 は組み立ててから渡す。
// cable が口の番号。UMP の group も同じ意味なので、cable が変なときだけ見る
void feed_ump(smu2000::plug::engine *eng, scratch *sc, const AUMIDIEventList &ev)
{
	const MIDIEventList &list = ev.eventList;
	const MIDIEventPacket *pk = list.packet;
	for (UInt32 p = 0; p < list.numPackets; p++) {
		UInt32 at = 0;
		while (at < pk->wordCount) {
			const UInt32 w0 = pk->words[at];
			const uint32_t type = w0 >> 28;
			const int group = int((w0 >> 24) & 0xf);
			const int port = (ev.cable < PORTS) ? int(ev.cable)
			                 : (group < PORTS ? group : 0);
			if (type == 0x2 && at + 1 <= pk->wordCount) {
				const uint8_t st = uint8_t((w0 >> 16) & 0xff);
				const uint8_t d1 = uint8_t((w0 >> 8) & 0xff);
				const uint8_t d2 = uint8_t(w0 & 0xff);
				const uint8_t msg[3] = { st, d1, d2 };
				const int cmd = st & 0xf0;
				// Remember the sounded channel. Stop hushing goes only there
				if (cmd == 0x90 && d2)
					sc->sounded[port].fetch_or(UInt16(1u << (st & 0xf)), std::memory_order_relaxed);
				eng->midi(msg, size_t(cmd == 0xc0 || cmd == 0xd0 ? 2 : 3), port);
				at += 1;
			} else if (type == 0x1 && at + 1 <= pk->wordCount) {
				const uint8_t st = uint8_t((w0 >> 16) & 0xff);
				const uint8_t d1 = uint8_t((w0 >> 8) & 0xff);
				const uint8_t d2 = uint8_t(w0 & 0xff);
				const uint8_t msg[3] = { st, d1, d2 };
				size_t n = 1;
				if (st == 0xf1 || st == 0xf3) n = 2;
				else if (st == 0xf2) n = 3;
				eng->midi(msg, n, port);
				at += 1;
			} else if (type == 0x3 && at + 2 <= pk->wordCount) {
				const uint32_t w1 = pk->words[at + 1];
				const int endpoint = int((w0 >> 20) & 0xf);
				const size_t count = size_t((w0 >> 16) & 0xf);
				const uint8_t payload[6] = {
					uint8_t((w0 >> 8) & 0xff), uint8_t(w0 & 0xff),
					uint8_t((w1 >> 24) & 0xff), uint8_t((w1 >> 16) & 0xff),
					uint8_t((w1 >> 8) & 0xff), uint8_t(w1 & 0xff),
				};
				if (endpoint == 0 || endpoint == 1) {
					sc->ump_sysex.clear();
					sc->ump_sysex.push_back(0xf0);
				}
				for (size_t k = 0; k < count && k < 6; k++) {
					if (sc->ump_sysex.size() < 65536)
						sc->ump_sysex.push_back(payload[k]);
				}
				if (endpoint == 0 || endpoint == 3) {
					sc->ump_sysex.push_back(0xf7);
					eng->midi(sc->ump_sysex.data(), sc->ump_sysex.size(), port);
					sc->ump_sysex.clear();
				}
				at += 2;
			} else {
				break;
			}
		}
		pk = MIDIEventPacketNext(pk);
	}
}

} // namespace

// Fixed stereo in and out. A native v3 bus.format has no validation hook
// like the v2's set-time refusal, so the subclass holds the line instead:
// anything but stereo is silently kept at stereo (the v2 answers an error
// there; here there is no error channel, and the read-back shows stereo)
// Fixed stereo in and out. The v2 unit refuses odd formats when they are
// set; the native v3 equivalent is this setter (with an error channel) plus
// the unit's channelCapabilities below. Anything but 32-bit float stereo
// is refused, exactly like the v2's StreamFormat check
@interface SMUStereoBus : AUAudioUnitBus
@end

@implementation SMUStereoBus

- (BOOL)setFormat:(AVAudioFormat *)format error:(NSError **)outError
{
	// 32-bit float stereo, interleaved or not -- the v2's StreamFormat
	// check, translated. Anything else is refused like there
	if (!format || format.commonFormat != AVAudioPCMFormatFloat32 ||
	    format.channelCount != 2) {
		if (outError)
			*outError = [NSError errorWithDomain:NSOSStatusErrorDomain
			                                code:kAudioUnitErr_FormatNotSupported
			                            userInfo:nil];
		return NO;
	}
	return [super setFormat:format error:outError];
}

@end

@implementation SMU2000AudioUnitV3 {
	std::unique_ptr<smu2000::plug::engine> _engine;
	std::unique_ptr<scratch>               _scratch;
	AUAudioUnitBusArray                   *_inputBusArray;
	AUAudioUnitBusArray                   *_outputBusArray;
	AUAudioUnitBus                        *_inputBus;
	AUAudioUnitBus                        *_outputBus;
}

- (instancetype)initWithComponentDescription:(AudioComponentDescription)desc
                                     options:(AudioComponentInstantiationOptions)options
                                       error:(NSError **)outError
{
	self = [super initWithComponentDescription:desc options:options error:outError];
	if (!self)
		return nil;

	AVAudioFormat *fmt = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:44100.0
	                                                                   channels:2];

	_outputBus = [[SMUStereoBus alloc] initWithFormat:fmt error:nil];
	_outputBus.maximumChannelCount = 2;
	_outputBus.name = @"Main Out";

	// A/D INPUT。実機の背面の入力。サンプリングと A/D の系統に入る。
	// 繋がなくても鳴るので、ホストが何も寄越さなければ無音として扱う
	_inputBus = [[SMUStereoBus alloc] initWithFormat:fmt error:nil];
	_inputBus.maximumChannelCount = 2;
	_inputBus.name = @"A/D Input";

	_outputBusArray = [[AUAudioUnitBusArray alloc] initWithAudioUnit:self
	                                                         busType:AUAudioUnitBusTypeOutput
	                                                          busses:@[_outputBus]];
	_inputBusArray  = [[AUAudioUnitBusArray alloc] initWithAudioUnit:self
	                                                         busType:AUAudioUnitBusTypeInput
	                                                          busses:@[_inputBus]];

	_engine = std::make_unique<smu2000::plug::engine>();
	_scratch = std::make_unique<scratch>();

	// ROM を読んで起動するのは別スレッド。ここではすぐ返る
	_engine->start();

	self.maximumFramesToRender = MAX_FRAMES;
	return self;
}

// The host's audio workgroup for our parallel slave thread (Apple's Audio
// Unit auxiliary-thread pattern). The system calls the block before every
// render, so a call that lands before boot is picked up by the next one,
// and a null context leaves the workgroup. Overriding the readonly property
// works on OS versions predating it: it is simply never called there.
- (AURenderContextObserver)renderContextObserver
{
	__weak SMU2000AudioUnitV3 *weakSelf = self;
	return ^(const AudioUnitRenderContext *context) {
		SMU2000AudioUnitV3 *strong = weakSelf;
		if (!strong)
			return;
		strong->_engine->set_realtime_workgroup(
			context ? (__bridge void *)context->workgroup : nullptr);
	};
}

- (void)dealloc
{
	// Free the buffers here. super's dealloc (inserted by ARC) calls
	// deallocateRenderResources on the way down, which must not touch _engine
	_scratch.reset();
	_engine.reset();
}

// 画面が描く engine。拡張の中で生まれるので、そのまま渡せる
- (smu2000::vst3::engine *)panelEngine { return _engine.get(); }

- (AUAudioUnitBusArray *)inputBusses  { return _inputBusArray; }
- (AUAudioUnitBusArray *)outputBusses { return _outputBusArray; }

// Fixed 2-in/2-out, which auval learns here (bridged to the v2
// SupportedNumChannels the AUv2 answers). Anything else never gets set,
// so allocate only ever sees pairs this unit renders
- (NSArray<NSNumber *> *)channelCapabilities
{
	return @[@2, @2];
}

// 実機の MIDI OUT。1 本
- (NSArray<NSString *> *)MIDIOutputNames { return @[@"MIDI Out"]; }

// MPE は実機に無い
- (BOOL)supportsMPE { return NO; }

// 実機の MIDI IN は 4 口ある。ここで 4 と言わないとホストは数を信じて、
// 使わないケーブルは届かなくなる。ファイルを鳴らす種類のホストは
// 代わりに口ごとに音源をもう 1 台開くことがある
- (NSInteger)virtualMIDICableCount { return PORTS; }

// 標本化周波数の変換のぶんだけ音が遅れる
- (NSTimeInterval)latency
{
	const double rate = _outputBus.format.sampleRate;
	if (!_engine || rate <= 0.0)
		return 0.0;
	return double(_engine->latency_samples()) / rate;
}

- (BOOL)allocateRenderResourcesAndReturnError:(NSError **)outError
{
	if (![super allocateRenderResourcesAndReturnError:outError])
		return NO;

	// 入力と出力で周波数が食い違っていると、引いた音の長さが合わなくなる
	if (_inputBus.format.sampleRate != _outputBus.format.sampleRate) {
		if (outError)
			*outError = [NSError errorWithDomain:NSOSStatusErrorDomain
			                                code:kAudioUnitErr_FormatNotSupported
			                            userInfo:nil];
		return NO;
	}

	// Channel counts need no check here: anything but stereo never gets
	// set (SMUStereoBus refuses it), so allocate only ever sees pairs
	// this unit renders

	_engine->set_output_rate(_outputBus.format.sampleRate);
	_scratch->allocate(self.maximumFramesToRender);

	// 鳴らし始める前に、起動が終わるのをここで待つ。ここは実時間の糸ではないので待ってよい。
	// 待たずに始めると、起動が終わるまで無音を返し続け、その間の MIDI は溜まるだけになる。
	// 実時間より速く回すホストでは、その無音が曲の頭の十数秒ぶんに化けて、そこにあった音符も失われる。
	// 写しがあれば数ミリ秒で戻ってくる。起動しなかった（ROM が無いなど）ときも器は作る
	(void)_engine->wait_ready(120 * 1000);

	_engine->set_processing(true);
	return YES;
}

- (void)deallocateRenderResources
{
	// super also calls this on the way down from dealloc, after _engine is
	// gone (see dealloc). Step over it in that case
	if (_engine)
		_engine->set_processing(false);
	[super deallocateRenderResources];
}

- (AUInternalRenderBlock)internalRenderBlock
{
	// 描き出しの中で Objective-C を触らなくて済むよう、素のポインタで捕まえる
	smu2000::plug::engine *eng = _engine.get();
	scratch               *sc  = _scratch.get();
	__unsafe_unretained SMU2000AudioUnitV3 *unowned_self = self;

	return ^AUAudioUnitStatus(AudioUnitRenderActionFlags *actionFlags,
	                          const AudioTimeStamp       *timestamp,
	                          AUAudioFrameCount           frameCount,
	                          NSInteger                   outputBusNumber,
	                          AudioBufferList            *outputData,
	                          const AURenderEvent        *realtimeEventListHead,
	                          AURenderPullInputBlock      pullInputBlock)
	{
		(void)actionFlags; (void)outputBusNumber;
		if (frameCount > MAX_FRAMES)
			return kAudioUnitErr_TooManyFramesToProcess;

		// 出力の器。ホストが mData を寄越さないことがある（その場合はこちらが出す）
		float *out_l = nullptr, *out_r = nullptr;
		if (outputData->mNumberBuffers >= 1) {
			if (!outputData->mBuffers[0].mData) {
				outputData->mBuffers[0].mData = sc->out_l.data();
				outputData->mBuffers[0].mDataByteSize = frameCount * sizeof(float);
			}
			out_l = static_cast<float *>(outputData->mBuffers[0].mData);
		}
		if (outputData->mNumberBuffers >= 2) {
			if (!outputData->mBuffers[1].mData) {
				outputData->mBuffers[1].mData = sc->out_r.data();
				outputData->mBuffers[1].mDataByteSize = frameCount * sizeof(float);
			}
			out_r = static_cast<float *>(outputData->mBuffers[1].mData);
		} else {
			out_r = sc->out_r.data();
		}
		if (!out_l)
			return kAudioUnitErr_InvalidParameter;

		// A/D INPUT。繋がっていなければ無音のまま
		const float *in_l = nullptr, *in_r = nullptr;
		if (pullInputBlock) {
			AudioBufferList *abl = sc->in_abl();
			abl->mNumberBuffers = 2;
			abl->mBuffers[0].mNumberChannels = 1;
			abl->mBuffers[0].mDataByteSize   = frameCount * sizeof(float);
			abl->mBuffers[0].mData           = sc->in_l.data();
			abl->mBuffers[1].mNumberChannels = 1;
			abl->mBuffers[1].mDataByteSize   = frameCount * sizeof(float);
			abl->mBuffers[1].mData           = sc->in_r.data();

			AudioUnitRenderActionFlags f = 0;
			if (pullInputBlock(&f, timestamp, frameCount, 0, abl) == noErr) {
				in_l = static_cast<const float *>(abl->mBuffers[0].mData);
				in_r = abl->mNumberBuffers >= 2
				           ? static_cast<const float *>(abl->mBuffers[1].mData)
				           : in_l;
			}
		}

		// MIDI を挟みながら区間ごとに作る。事象の位置は標本単位で正しく効く
		const AURenderEvent *e = realtimeEventListHead;
		AUAudioFrameCount done = 0;
		while (done < frameCount) {
			while (e) {
				AUEventSampleTime off = e->head.eventSampleTime - timestamp->mSampleTime;
				if (off < 0)
					off = 0;
				if (AUAudioFrameCount(off) > done)
					break;
				if (e->head.eventType == AURenderEventMIDI ||
				    e->head.eventType == AURenderEventMIDISysEx) {
					const AUMIDIEvent *m = &e->MIDI;
					const int port = (m->cable < PORTS) ? int(m->cable) : 0;
					if (m->length) {
						if (m->length >= 3 && (m->data[0] & 0xf0) == 0x90 && m->data[2])
							sc->sounded[port].fetch_or(UInt16(1u << (m->data[0] & 0xf)),
							                           std::memory_order_relaxed);
						eng->midi(m->data, m->length, port);
					}
				} else if (e->head.eventType == AURenderEventMIDIEventList) {
					feed_ump(eng, sc, e->MIDIEventsList);
				}
				e = e->head.next;
			}

			AUAudioFrameCount upto = frameCount;
			if (e) {
				AUEventSampleTime off = e->head.eventSampleTime - timestamp->mSampleTime;
				if (off < 0)
					off = 0;
				if (AUAudioFrameCount(off) < upto)
					upto = AUAudioFrameCount(off);
			}
			if (upto <= done)
				upto = done + 1;
			if (upto > frameCount)
				upto = frameCount;

			const int n = int(upto - done);
			eng->fill(out_l + done, out_r + done, n,
			          in_l ? in_l + done : nullptr,
			          in_r ? in_r + done : nullptr);
			done = upto;
		}

		// 出力レベル。一気に変えると音が跳ねるので 1 サンプルずつ寄せる
		{
			const float target = eng->panel().gain();
			float &now = sc->gain_now;
			if (now != target || target != 1.0f) {
				const float step = 1.0f / 512.0f;
				for (AUAudioFrameCount i = 0; i < frameCount; i++) {
					if (now < target) now = std::min(target, now + step);
					else if (now > target) now = std::max(target, now - step);
					out_l[i] *= now;
					out_r[i] *= now;
				}
			}
		}

		// MIDI OUT。firmware が送り出したものをメッセージに切って渡す
		AUMIDIOutputEventBlock outBlock = unowned_self.MIDIOutputEventBlock;
		if (outBlock) {
			const size_t got = eng->midi_out(sc->tx.data(), sc->tx.size());
			if (got) {
				emit_ctx ctx{ outBlock, AUEventSampleTime(timestamp->mSampleTime) };
				sc->split.feed(sc->tx.data(), got, emit_one, &ctx);
			}
		}
		return noErr;
	};
}

// ---- 音色などの持ち帰り。DAW のプロジェクトに覚えさせる

static NSString *const kStateKey = @"S-MU2000.nvram";

- (NSDictionary<NSString *, id> *)fullState
{
	NSMutableDictionary *d = [[super fullState] mutableCopy] ?: [NSMutableDictionary dictionary];
	if (_engine) {
		std::vector<uint8_t> blob = _engine->save_state();
		if (!blob.empty())
			d[kStateKey] = [NSData dataWithBytes:blob.data() length:blob.size()];
	}
	return d;
}

- (void)setFullState:(NSDictionary<NSString *, id> *)state
{
	[super setFullState:state];
	NSData *d = state[kStateKey];
	if (_engine && [d isKindOfClass:[NSData class]] && d.length)
		_engine->load_state(static_cast<const uint8_t *>(d.bytes), d.length);
}

// 画面。AUv2・VST3 と同じパネル（view_controller.mm）。拡張の中で動くので
// engine は直接渡せる（AUv2 の kEngineProperty 回りは要らない）
- (void)requestViewControllerWithCompletionHandler:(void (^)(NSViewController * __nullable))completionHandler
{
	// **view を触るのは主の糸で。** ホストは普通は主の糸からこれを呼ぶが、
	// view_controller.mm の loadView は preferredContentSize を立てるので、
	// 別の糸から来たときにここで直に触ると AppKit が例外を投げる
	smu2000::vst3::engine *eng = _engine.get();
	AUAudioUnit *au = self;
	dispatch_async(dispatch_get_main_queue(), ^{
		// What was handed back goes to the log: "the host asked for a view and
		// nothing appeared" is told apart by this line, with the "画面を作る"
		// (building the panel) line before it
		eng->log_line("画面を頼まれた");
		SMU2000ViewControllerV3 *vc =
		    [[SMU2000ViewControllerV3 alloc] initWithEngine:eng audioUnit:au];
		NSViewController *answer = (vc && vc.view) ? vc : nil;
		char b[96];
		if (answer)
			std::snprintf(b, sizeof(b), "画面を渡した %g x %g",
			              answer.view.frame.size.width, answer.view.frame.size.height);
		else
			std::snprintf(b, sizeof(b), "画面を渡せない");
		eng->log_line(b);
		if (completionHandler)
			completionHandler(answer);
	});
}

// 画面の置き方。パネルは決まった大きさ（1400x360）1 枚だけなので、
// どれを渡されても全部使えると答える
- (NSIndexSet *)supportedViewConfigurations:(NSArray<AUAudioUnitViewConfiguration *> *)availableViewConfigurations
{
	_engine->log_line("置き方を聞かれた");
	NSMutableIndexSet *s = [NSMutableIndexSet indexSet];
	for (NSUInteger i = 0; i < availableViewConfigurations.count; i++)
		[s addIndex:i];
	return s;
}

// The one factory preset, like the AUv2's. Choosing it puts the defaults
// back; anything else is ignored
- (NSArray<AUAudioUnitPreset *> *)factoryPresets
{
	AUAudioUnitPreset *p = [[AUAudioUnitPreset alloc] init];
	p.number = 0;
	p.name = @"S-MU2000";
	return @[p];
}

- (AUAudioUnitPreset *)currentPreset
{
	return self.factoryPresets.firstObject;
}

- (void)setCurrentPreset:(AUAudioUnitPreset *)currentPreset
{
	if (!currentPreset || currentPreset.number != 0)
		return;
	if (_engine) {
		_engine->panel().set_gain(1.0f);
		hush_engine(_engine.get(), _scratch.get());
	}
}

// Silence whatever is still ringing (host stopped us). Only the channels
// that sounded get all-sound-off + all-notes-off; see the note on scratch::sounded
- (void)reset
{
	if (_engine) {
		hush_engine(_engine.get(), _scratch.get());
		_engine->flush_resampler();
	}
	if (_scratch) {
		_scratch->split.reset();
		_scratch->ump_sysex.clear();
	}
}

@end
