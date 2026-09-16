// license:BSD-3-Clause
//
// A small host that loads and runs S-MU2000's Audio Unit v2 with no DAW around
// it.
//
//   aubprobe <bundle> [<MIDI> [<out.wav>]] [--rate 48000] [--block 512]
//                                          [--tail 3] [--torture] [--list]
//   aubprobe - [<MIDI> ...]        use whatever AU is installed
//
// Same job as vst3probe (src/vst3/probe.cpp), reached differently: that one
// looks for GetPluginFactory through CFBundle, while this one puts the factory
// into AudioComponentRegister and opens the unit with AudioComponentInstanceNew
// -- **the road a host actually drives down**, so a spelling mistake in
// Info.plist's factoryFunction shows up right here.
//
// Sound is made the same way as vst3probe: an event's time is turned into an
// offset within the block and handed to MusicDeviceMIDIEvent. By AU's rules that
// offset points into the block about to be rendered, so the plug-in injects it
// at that moment inside Render (src/au/plugin.cpp) -- which is why comparing the
// two renders shows up timing differences.

#include "compat/console.h"
#include "smf.h"

#include <AudioToolbox/AudioToolbox.h>
#include <CoreFoundation/CoreFoundation.h>

// The editor check below is written against the Objective-C runtime rather than
// AppKit: it has to make a view, and aubprobe is plain C++
#include <objc/message.h>
#include <objc/runtime.h>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

// For the out-of-process check below: AudioComponentInstantiate calls back on a
// queue, and the probe waits for it
#include <dispatch/dispatch.h>
#include <limits.h>
#include <thread>
#include <vector>

namespace {

constexpr OSType kType         = 'aumu';
constexpr OSType kSubtype      = 'SMU2';
constexpr OSType kManufacturer = 'Trbh';

// These two are in libobjc but not declared in the SDK's headers. The view a
// host is handed is autoreleased, so without a pool every editor it makes
// complains on the way out
extern "C" void *objc_autoreleasePoolPush(void);
extern "C" void  objc_autoreleasePoolPop(void *pool);

long long now_ms()
{
	return std::chrono::duration_cast<std::chrono::milliseconds>(
		std::chrono::steady_clock::now().time_since_epoch()).count();
}

void write_wav(const std::string &path, const std::vector<int16_t> &pcm, uint32_t rate)
{
	std::FILE *f = std::fopen(path.c_str(), "wb");
	if (!f) {
		std::fprintf(stderr, "書けない: %s\n", path.c_str());
		return;
	}
	const uint32_t data = uint32_t(pcm.size() * 2);
	const uint32_t riff = 36 + data;
	const uint16_t ch = 2, bits = 16;
	const uint32_t byte_rate = rate * ch * bits / 8;
	const uint16_t align = uint16_t(ch * bits / 8);
	auto u32 = [&](uint32_t v) { std::fwrite(&v, 4, 1, f); };
	auto u16 = [&](uint16_t v) { std::fwrite(&v, 2, 1, f); };
	std::fwrite("RIFF", 1, 4, f); u32(riff);
	std::fwrite("WAVEfmt ", 1, 8, f); u32(16); u16(1); u16(ch);
	u32(rate); u32(byte_rate); u16(align); u16(bits);
	std::fwrite("data", 1, 4, f); u32(data);
	if (!pcm.empty())
		std::fwrite(pcm.data(), 2, pcm.size(), f);
	std::fclose(f);
}

AudioStreamBasicDescription float_format(double rate, uint32_t channels)
{
	AudioStreamBasicDescription f{};
	f.mSampleRate       = rate;
	f.mFormatID         = kAudioFormatLinearPCM;
	// The two flags come from different anonymous enums, so the or needs a cast
	f.mFormatFlags      = AudioFormatFlags(kAudioFormatFlagsNativeFloatPacked) |
	                      AudioFormatFlags(kAudioFormatFlagIsNonInterleaved);
	f.mChannelsPerFrame = channels;
	f.mBitsPerChannel   = 32;
	f.mFramesPerPacket  = 1;
	f.mBytesPerFrame    = 4;
	f.mBytesPerPacket   = 4;
	return f;
}

// Read the bundle and assemble the component the way a host does. An empty
// bundle means "find whichever AU is already installed", which is the road
// auval takes
AudioComponent find_component(const std::string &bundle, std::string &err)
{
	AudioComponentDescription desc{};
	desc.componentType = kType;
	desc.componentSubType = kSubtype;
	desc.componentManufacturer = kManufacturer;
	desc.componentFlags = 0;
	desc.componentFlagsMask = 0;

	if (bundle.empty() || bundle == "-")
		return AudioComponentFindNext(nullptr, &desc);

	// Open it with CFBundle. CFBundleCreate succeeds because Info.plist is there
	CFURLRef url = CFURLCreateFromFileSystemRepresentation(
		nullptr, reinterpret_cast<const UInt8 *>(bundle.c_str()), bundle.size(), true);
	CFBundleRef bun = url ? CFBundleCreate(nullptr, url) : nullptr;
	if (url)
		CFRelease(url);
	if (!bun) {
		err = "バンドルを開けない: " + bundle;
		return nullptr;
	}
	if (!CFBundleLoadExecutable(bun)) {
		err = "バンドルを読めない: " + bundle;
		return nullptr;
	}
	auto entry = reinterpret_cast<bool (*)(CFBundleRef)>(
		CFBundleGetFunctionPointerForName(bun, CFSTR("bundleEntry")));
	if (entry)
		entry(bun);

	auto factory = reinterpret_cast<AudioComponentFactoryFunction>(
		CFBundleGetFunctionPointerForName(bun, CFSTR("SMU2000AUFactory")));
	if (!factory) {
		err = "SMU2000AUFactory が無い（Info.plist の factoryFunction と綴りを合わせる）";
		return nullptr;
	}
	// CFSTR needs a literal, so the name is spelled out rather than shared
	return AudioComponentRegister(&desc, CFSTR("S-MU2000"), 0, factory);
}

// Hand a run of MIDI bytes to the MusicDevice entry point one message at a
// time. An SMF event can carry several messages back to back, so running status
// has to be followed to split them up
void send_midi(AudioUnit unit, const std::vector<u8> &bytes, UInt32 offset)
{
	size_t i = 0;
	UInt8 status = 0;
	while (i < bytes.size()) {
		const UInt8 b = bytes[i];
		if (b >= 0xf8) {                    // real-time: complete in one byte
			MusicDeviceMIDIEvent(unit, b, 0, 0, offset);
			i++;
			continue;
		}
		if (b == 0xf0) {
			// By AU's rules SysEx is handed over whole, F0 through F7
			size_t end = i + 1;
			while (end < bytes.size() && bytes[end] != 0xf7)
				end++;
			if (end < bytes.size())
				end++;
			MusicDeviceSysEx(unit, bytes.data() + i, UInt32(end - i));
			i = end;
			status = 0;
			continue;
		}
		if (b >= 0x80) {
			status = b;
			i++;
		} else if (status == 0) {
			i++;                            // stray data with no status: drop it
			continue;
		}
		if (status >= 0xf0) {
			i++;
			continue;
		}
		const UInt8 cmd = status & 0xf0;
		const int n = (cmd == 0xc0 || cmd == 0xd0) ? 1 : 2;
		const UInt32 d1 = (i < bytes.size()) ? bytes[i] : 0;
		const UInt32 d2 = (i + 1 < bytes.size()) ? bytes[i + 1] : 0;
		MusicDeviceMIDIEvent(unit, status, d1, d2, offset);
		i += size_t(n);
	}
}

// AudioBufferList declares one buffer; a second has to be carried alongside it
// and the two have to stay adjacent, which is what this is for
struct audio_buffers_2 {
	AudioBufferList list;
	AudioBuffer extra;
};

} // namespace


// ---------------------------------------------------------------------------

namespace {

int show_info(AudioUnit unit)
{
	std::printf("パラメータ:\n");
	UInt32 size = 0;
	if (AudioUnitGetPropertyInfo(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0,
	                             &size, nullptr) == noErr) {
		std::vector<AudioUnitParameterID> ids(size / sizeof(AudioUnitParameterID));
		if (AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0,
		                         ids.data(), &size) == noErr) {
			for (AudioUnitParameterID id : ids) {
				AudioUnitParameterInfo info{};
				UInt32 is = sizeof(info);
				if (AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo,
				                         kAudioUnitScope_Global, id, &info, &is) == noErr) {
					char name[64] = {};
					if ((info.flags & kAudioUnitParameterFlag_HasCFNameString) && info.cfNameString)
						CFStringGetCString(info.cfNameString, name, sizeof(name), kCFStringEncodingUTF8);
					AudioUnitParameterValue v = 0;
					AudioUnitGetParameter(unit, id, kAudioUnitScope_Global, 0, &v);
					std::printf("  [%u] %-20s %.3f - %.3f（今 %.3f）\n", unsigned(id), name,
					            double(info.minValue), double(info.maxValue), double(v));
				}
			}
		}
	}

	Float64 latency = 0.0;
	size = sizeof(latency);
	if (AudioUnitGetProperty(unit, kAudioUnitProperty_Latency, kAudioUnitScope_Global, 0,
	                         &latency, &size) == noErr)
		std::printf("遅れ %.6f 秒\n", latency);

	CFStringRef iname = nullptr;
	size = sizeof(iname);
	if (AudioUnitGetProperty(unit, kMusicDeviceProperty_InstrumentName, kAudioUnitScope_Global, 0,
	                         &iname, &size) == noErr && iname) {
		char buf[128] = {};
		CFStringGetCString(iname, buf, sizeof(buf), kCFStringEncodingUTF8);
		std::printf("楽器名 %s\n", buf);
		CFRelease(iname);
	}
	return 0;
}

int run_render(AudioUnit unit, double rate, int block, double extra,
               const std::string &mid, const std::string &wav)
{
	std::vector<smf::event> events;
	std::string err;
	if (!smf::load(mid, events, err)) {
		std::fprintf(stderr, "MIDI を読めない: %s\n", err.c_str());
		return 1;
	}
	double last = 0.0;
	for (const smf::event &e : events)
		last = std::max(last, e.time);
	std::printf("MIDI: %zu イベント、%.2f 秒\n", events.size(), last);

	const AudioStreamBasicDescription fmt = float_format(rate, 2);
	if (AudioUnitSetProperty(unit, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output,
	                         0, &fmt, sizeof(fmt)) != noErr) {
		std::fprintf(stderr, "標本化周波数を入れられない\n");
		return 1;
	}
	const UInt32 maxf = UInt32(block);
	AudioUnitSetProperty(unit, kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global,
	                     0, &maxf, sizeof(maxf));

	if (AudioUnitInitialize(unit) != noErr) {
		std::fprintf(stderr, "初期化できない\n");
		return 1;
	}

	std::printf("起動待ち...");
	std::fflush(stdout);
	const long long t_wait = now_ms();
	for (;;) {
		AudioUnitParameterValue v = 0;
		AudioUnitGetParameter(unit, 1 /* status */, kAudioUnitScope_Global, 0, &v);
		if (v >= 1.0f)
			break;
		if (now_ms() - t_wait > 60000) {
			std::printf(" 60 秒待っても始まらない\n");
			break;
		}
		std::this_thread::sleep_for(std::chrono::milliseconds(50));
	}
	std::printf(" %ld ms\n", long(now_ms() - t_wait));

	const int64_t total = int64_t((last + extra) * rate);
	std::vector<float> l;
	std::vector<float> r;
	l.resize(size_t(block));
	r.resize(size_t(block));
	std::vector<int16_t> pcm;
	pcm.reserve(size_t(total) * 2);

	size_t next = 0;
	int64_t pos = 0;
	const long long t0 = now_ms();
	while (pos < total) {
		const UInt32 n = UInt32(std::min<int64_t>(block, total - pos));

		// Hand over the MIDI that falls in this block, turned into an offset
		// within it
		while (next < events.size() && events[next].time * rate < double(pos + n)) {
			const double at = events[next].time * rate;
			const UInt32 off = UInt32(std::clamp<double>(at - double(pos), 0.0, double(n > 0 ? n - 1 : 0)));
			send_midi(unit, events[next].bytes, off);
			next++;
		}

		audio_buffers_2 bufs{};
		bufs.list.mNumberBuffers = 2;
		bufs.list.mBuffers[0].mNumberChannels = 1;
		bufs.list.mBuffers[0].mDataByteSize = n * 4;
		bufs.list.mBuffers[0].mData = l.data();
		bufs.list.mBuffers[1].mNumberChannels = 1;
		bufs.list.mBuffers[1].mDataByteSize = n * 4;
		bufs.list.mBuffers[1].mData = r.data();

		AudioTimeStamp ts{};
		ts.mSampleTime = double(pos);
		ts.mFlags = kAudioTimeStampSampleTimeValid;

		AudioUnitRenderActionFlags flags = 0;
		const OSStatus st = AudioUnitRender(unit, &flags, &ts, 0, n, &bufs.list);
		if (st != noErr) {
			std::fprintf(stderr, "Render が失敗: %d（%lld サンプル目）\n", int(st), (long long)pos);
			break;
		}

		for (UInt32 i = 0; i < n; i++) {
			const float lv = std::clamp(l[i], -1.0f, 1.0f);
			const float rv = std::clamp(r[i], -1.0f, 1.0f);
			pcm.push_back(int16_t(std::lround(lv * 32767.0f)));
			pcm.push_back(int16_t(std::lround(rv * 32767.0f)));
		}
		pos += n;
	}
	const long long t1 = now_ms();

	AudioUnitUninitialize(unit);

	write_wav(wav, pcm, uint32_t(rate));
	double peak = 0.0, sum = 0.0;
	for (int16_t v : pcm) { peak = std::max(peak, std::fabs(double(v))); sum += std::fabs(double(v)); }
	std::printf("書き出した: %s（%.1f 秒 / %.0f Hz、実時間 %.1f 秒）\n",
	            wav.c_str(), double(total) / rate, rate, (t1 - t0) / 1000.0);
	std::printf("最大 %.0f  平均 %.1f\n", peak, sum / std::max<size_t>(pcm.size(), 1));
	return 0;
}

} // namespace


// ---------------------------------------------------------------------------

namespace {

// The editor, walked the way a host walks it: read kAudioUnitProperty_CocoaUI,
// resolve the class it names, and ask that class for a view. The parts a DAW
// would notice first are exactly these -- the property naming a class that
// exists, the name matching, and the class building a panel.
//
// There is no window in here, so what ends up on screen is still a DAW's word;
// that the panel is painted correctly is checked in doc/porting-macos.md
int check_editor(AudioUnit u, const std::string &bundle_path)
{
	UInt32 size = 0;
	Boolean writable = true;
	if (AudioUnitGetPropertyInfo(u, kAudioUnitProperty_CocoaUI, kAudioUnitScope_Global, 0,
	                             &size, &writable) != noErr) {
		std::printf("NG: CocoaUI に答えない\n");
		return 1;
	}
	int bad = 0;
	if (size != sizeof(AudioUnitCocoaViewInfo) || writable) {
		std::printf("NG: CocoaUI の大きさが %u（%zu のはず）、読み取り専用は %d\n",
		            unsigned(size), sizeof(AudioUnitCocoaViewInfo), int(writable));
		bad++;
	}

	AudioUnitCocoaViewInfo info{};
	UInt32 got = sizeof(info);
	if (AudioUnitGetProperty(u, kAudioUnitProperty_CocoaUI, kAudioUnitScope_Global, 0,
	                         &info, &got) != noErr) {
		std::printf("NG: CocoaUI を読めない\n");
		return bad + 1;
	}

	char url_path[4096] = {};
	char class_name[256] = {};
	const bool have_url = info.mCocoaAUViewBundleLocation &&
	    CFURLGetFileSystemRepresentation(info.mCocoaAUViewBundleLocation, true,
	                                     reinterpret_cast<UInt8 *>(url_path), sizeof(url_path));
	if (info.mCocoaAUViewClass[0])
		CFStringGetCString(info.mCocoaAUViewClass[0], class_name, sizeof(class_name),
		                   kCFStringEncodingUTF8);

	if (!have_url || !class_name[0]) {
		std::printf("NG: CocoaUI がバンドルとクラス名を返さない\n");
		return bad + 1;
	}
	// The AU publishes an absolute URL, which is what a host needs; the bundle
	// this probe was handed may be a relative path
	char resolved[4096] = {};
	const char *want_path = realpath(bundle_path.c_str(), resolved) ? resolved
	                                                                : bundle_path.c_str();
	if (std::string(url_path) != want_path) {
		std::printf("NG: CocoaUI のバンドルが違う: %s（%s のはず）\n", url_path, want_path);
		bad++;
	}

	Class cls = objc_getClass(class_name);
	if (!cls) {
		std::printf("NG: クラス %s が無い（CocoaUI の綴りと @interface を合わせる）\n", class_name);
		return bad + 1;
	}

	void *pool = objc_autoreleasePoolPush();

	id factory = ((id (*)(id, SEL))objc_msgSend)((id)cls, sel_registerName("alloc"));
	factory = ((id (*)(id, SEL))objc_msgSend)(factory, sel_registerName("init"));
	const unsigned version = ((unsigned (*)(id, SEL))objc_msgSend)(
	    factory, sel_registerName("interfaceVersion"));
	if (version != 0) {
		std::printf("NG: interfaceVersion が %u（0 のはず）\n", version);
		bad++;
	}

	// NSSize is two doubles
	struct ns_size { double width, height; };
	const ns_size want{ 1400.0, 360.0 };
	id view = ((id (*)(id, SEL, AudioUnit, ns_size))objc_msgSend)(
	    factory, sel_registerName("uiViewForAudioUnit:withSize:"), u, want);
	if (!view) {
		std::printf("NG: エディタが画面を作れない\n");
		objc_autoreleasePoolPop(pool);
		return bad + 1;
	}

	id subs = ((id (*)(id, SEL))objc_msgSend)(view, sel_registerName("subviews"));
	const unsigned long panels = subs ? ((unsigned long (*)(id, SEL))objc_msgSend)(
	    subs, sel_registerName("count")) : 0;
	if (panels < 1) {
		std::printf("NG: エディタの中にパネルが無い\n");
		bad++;
	}
	std::printf("OK: エディタ %s が画面を作った（下位ビュー %lu 枚）\n",
	            object_getClassName(view), panels);

	objc_autoreleasePoolPop(pool);
	return bad;
}


int run_torture(AudioComponent comp, const std::string &bundle_path)
{
	std::printf("\n---- 乱暴に扱ってみる ----\n");
	int bad = 0;

	// 1. create and dispose without ever initialising
	for (int i = 0; i < 8; i++) {
		AudioUnit u = nullptr;
		if (AudioComponentInstanceNew(comp, &u) != noErr || !u) {
			std::printf("NG: 作れない\n");
			return 1;
		}
		AudioComponentInstanceDispose(u);
	}
	std::printf("OK: 初期化せずに 8 個作って捨てた\n");

	// 2. initialize / uninitialize round trips
	{
		AudioUnit u = nullptr;
		AudioComponentInstanceNew(comp, &u);
		for (int i = 0; i < 3; i++) {
			AudioUnitInitialize(u);
			AudioUnitUninitialize(u);
		}
		AudioComponentInstanceDispose(u);
		std::printf("OK: initialize/uninitialize を 3 往復\n");
	}

	// 3. every sample rate and block length, rendering without listening
	{
		const double rates[] = { 22050, 32000, 44100, 48000, 96000, 192000 };
		std::vector<float> l(1024), r(1024);
		for (double rate : rates) {
			AudioUnit u = nullptr;
			AudioComponentInstanceNew(comp, &u);
			const AudioStreamBasicDescription fmt = float_format(rate, 2);
			const OSStatus sf = AudioUnitSetProperty(u, kAudioUnitProperty_StreamFormat,
			                                         kAudioUnitScope_Output, 0, &fmt, sizeof(fmt));
			if (sf != noErr) {
				std::printf("NG: %.0f Hz を受けない\n", rate);
				bad++;
			}
			const UInt32 maxf = 1024;
			AudioUnitSetProperty(u, kAudioUnitProperty_MaximumFramesPerSlice,
			                     kAudioUnitScope_Global, 0, &maxf, sizeof(maxf));
			AudioUnitInitialize(u);

			audio_buffers_2 bufs{};
			bufs.list.mNumberBuffers = 2;
			for (int i = 0; i < 2; i++) {
				bufs.list.mBuffers[i].mNumberChannels = 1;
				bufs.list.mBuffers[i].mDataByteSize = 1024 * 4;
				bufs.list.mBuffers[i].mData = i == 0 ? (void *)l.data() : (void *)r.data();
			}
			AudioTimeStamp ts{};
			ts.mFlags = kAudioTimeStampSampleTimeValid;

			// a zero-length render, then ordinary ones
			AudioUnitRenderActionFlags flags = 0;
			AudioUnitRender(u, &flags, &ts, 0, 0, &bufs.list);
			for (int blk = 0; blk < 8; blk++) {
				flags = 0;
				const OSStatus st = AudioUnitRender(u, &flags, &ts, 0, 256, &bufs.list);
				if (st != noErr) {
					std::printf("NG: %.0f Hz の Render が %d\n", rate, int(st));
					bad++;
					break;
				}
				ts.mSampleTime += 256;
			}
			// The A/D INPUT bus: a host feeds it with a render callback, and two
			// things about that are easy to get wrong in a way hosts notice.
			//
			// 1. the timestamp the render was given has to travel to the callback.
			//    A host lines the input up with the block it is rendering by it,
			//    and auval reports a unit that hands over 0 instead ("AU is not
			//    passing time stamp correctly")
			// 2. a callback that fails has to be reported out of AURender. A unit
			//    that answers noErr hides the broken graph (auval: "Render Input
			//    returned an error, but was not returned to the caller of AURender")
			struct seen_t { AudioTimeStamp ts; UInt32 calls; } seen{};
			AURenderCallbackStruct in_cb{};
			in_cb.inputProcRefCon = &seen;
			in_cb.inputProc = [](void *ref, AudioUnitRenderActionFlags *, const AudioTimeStamp *t,
			                     UInt32, UInt32, AudioBufferList *) -> OSStatus {
				auto *s = static_cast<seen_t *>(ref);
				s->ts = *t;
				s->calls++;
				return noErr;
			};
			if (AudioUnitSetProperty(u, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input,
			                         0, &in_cb, sizeof(in_cb)) != noErr) {
				std::printf("NG: 入力のコールバックを受けない\n");
				bad++;
			} else {
				ts.mSampleTime = 4096;
				flags = 0;
				AudioUnitRender(u, &flags, &ts, 0, 256, &bufs.list);
				if (seen.calls != 1 || seen.ts.mSampleTime != 4096.0) {
					std::printf("NG: 入力に渡った時刻が %g（%u 回呼ばれた）\n",
					            double(seen.ts.mSampleTime), unsigned(seen.calls));
					bad++;
				}
			}

			// ...and the same callback, failing, must come back out of Render
			in_cb.inputProc = [](void *, AudioUnitRenderActionFlags *, const AudioTimeStamp *,
			                     UInt32, UInt32, AudioBufferList *) -> OSStatus {
				return kAudioUnitErr_InvalidParameter;
			};
			AudioUnitSetProperty(u, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input,
			                     0, &in_cb, sizeof(in_cb));
			flags = 0;
			const OSStatus fail = AudioUnitRender(u, &flags, &ts, 0, 256, &bufs.list);
			if (fail != kAudioUnitErr_InvalidParameter) {
				std::printf("NG: 入力を渡せないとき Render が %d を返した\n", int(fail));
				bad++;
			}

			AudioUnitUninitialize(u);
			AudioComponentInstanceDispose(u);
		}
		std::printf("OK: %.0f から 192000 まで setup と Render\n", rates[0]);
	}

	// 4. ask which properties answer, and how
	{
		AudioUnit u = nullptr;
		AudioComponentInstanceNew(comp, &u);
		// Asked with the scope included: the same number in another scope is a
		// different question
		struct prop_test { AudioUnitPropertyID id; AudioUnitScope scope; };
		const prop_test props[] = {
			{ kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global },
			{ kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global },
			{ kAudioUnitProperty_Latency, kAudioUnitScope_Global },
			// How long the machine keeps sounding after the last note (the reverb)
			{ kAudioUnitProperty_TailTime, kAudioUnitScope_Global },
			{ kAudioUnitProperty_ParameterList, kAudioUnitScope_Global },
			{ kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global },
			// Global, not Output. Apple's header says Global for this one, and
			// DLSMusicDevice -- Apple's own aumu -- refuses it for both Input and
			// Output. Asking for it in Output made this test fail a unit that
			// answers exactly the way the reference does
			{ kAudioUnitProperty_SupportedNumChannels, kAudioUnitScope_Global },
			{ kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output },
			{ kAudioUnitProperty_ElementCount, kAudioUnitScope_Input },
			// The input bus is the A/D INPUT, and this unit has one (the VST3 build
			// has the same bus), so its format is answered rather than refused
			{ kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input },
		};
		for (const prop_test &p : props) {
			UInt32 size = 0;
			Boolean writable = false;
			const OSStatus st = AudioUnitGetPropertyInfo(u, p.id, p.scope, 0, &size, &writable);
			if (st != noErr) {
				std::printf("NG: プロパティ %u（スコープ %u）に答えない\n",
				            unsigned(p.id), unsigned(p.scope));
				bad++;
			}
		}

		// The tail is asked for by value as well: a host stops rendering when it
		// runs out, so a tail that is too short cuts the reverb off. The VST3 side
		// answers four seconds (getTailSamples); this has to agree with it
		{
			Float64 tail = 0.0;
			UInt32 n = sizeof(tail);
			if (AudioUnitGetProperty(u, kAudioUnitProperty_TailTime, kAudioUnitScope_Global, 0,
			                         &tail, &n) != noErr) {
				std::printf("NG: 残響の長さを読めない\n");
				bad++;
			} else if (tail < 3.0) {
				std::printf("NG: 残響の長さが %.2f 秒しかない\n", double(tail));
				bad++;
			}
		}

		// The channel matrix a host (and auval) walks: it reads SupportedNumChannels
		// and then configures the unit for the combination it was told about.
		//
		// Answering "that property is here" without also saying it is *writable*,
		// and how many bytes it is, reads as "this unit cannot be configured".
		// auval says `Cannot Set Input Num Channels:2 when unit says it can`
		// (kAudioUnitErr_PropertyNotWritable, -10865), fails the format tests and
		// never reaches the render tests. From a host that looked like a dead
		// plug-in: no audio and no editor
		{
			AUChannelInfo chans[4] = {};
			UInt32 csize = sizeof(chans);
			if (AudioUnitGetProperty(u, kAudioUnitProperty_SupportedNumChannels,
			                         kAudioUnitScope_Global, 0, chans, &csize) != noErr ||
			    csize < sizeof(AUChannelInfo)) {
				std::printf("NG: 入出力の本数を読めない\n");
				bad++;
			} else {
				// The input bus is the A/D INPUT, so the host asks how big the
				// property is and whether it may write it before ever setting it
				UInt32 size = 0;
				Boolean writable = false;
				const OSStatus info = AudioUnitGetPropertyInfo(u, kAudioUnitProperty_StreamFormat,
				                                               kAudioUnitScope_Input, 0,
				                                               &size, &writable);
				if (info != noErr || !writable || size < sizeof(AudioStreamBasicDescription)) {
					std::printf("NG: 入力の形式を書けない（status %d、%u バイト、writable %d）\n",
					            int(info), unsigned(size), int(writable));
					bad++;
				}
				// ...and it has to answer with a format a host can take and
				// change only the channel count of, which is what auval does
				AudioStreamBasicDescription cur{};
				size = sizeof(cur);
				if (AudioUnitGetProperty(u, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input,
				                         0, &cur, &size) != noErr) {
					std::printf("NG: 入力の今の形式を読めない\n");
					bad++;
				} else {
					AudioStreamBasicDescription want = cur;
					want.mChannelsPerFrame = 2;       // what the advert promises
					if (AudioUnitSetProperty(u, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input,
					                         0, &want, sizeof(want)) != noErr) {
						std::printf("NG: 入力を 2 本にできない（%d 本と名乗っている）\n",
						            int(chans[0].inChannels));
						bad++;
					}
					want.mChannelsPerFrame = 1;       // and what it does not
					if (AudioUnitSetProperty(u, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input,
					                         0, &want, sizeof(want)) == noErr) {
						std::printf("NG: 名乗っていない 1 本の入力を通してしまう\n");
						bad++;
					}
				}

				// The two properties a host uses to wire the input bus up. Both carry
				// an AURenderCallbackStruct, and both have to be answerable *and*
				// writable: a host asks GetPropertyInfo first and takes a unit that
				// cannot answer as one it may not connect to at all
				const AudioUnitPropertyID into[] = { kAudioUnitProperty_SetRenderCallback,
				                                     kAudioUnitProperty_MakeConnection };
				for (AudioUnitPropertyID id : into) {
					UInt32 isize = 0;
					Boolean iw = false;
					if (AudioUnitGetPropertyInfo(u, id, kAudioUnitScope_Input, 0, &isize, &iw) != noErr ||
					    !iw || isize < sizeof(AURenderCallbackStruct)) {
						std::printf("NG: 入力の %u を書けない（%u バイト、writable %d）\n",
						            unsigned(id), unsigned(isize), int(iw));
						bad++;
						continue;
					}
					// and setting it has to stick, so a host can read back what it
					// connected
					AURenderCallbackStruct cb{};
					cb.inputProcRefCon = &cb;
					cb.inputProc = [](void *, AudioUnitRenderActionFlags *, const AudioTimeStamp *,
					                  UInt32, UInt32, AudioBufferList *) -> OSStatus { return noErr; };
					if (AudioUnitSetProperty(u, id, kAudioUnitScope_Input, 0, &cb, sizeof(cb)) != noErr) {
						std::printf("NG: 入力の %u を設定できない\n", unsigned(id));
						bad++;
					}
				}
			}
		}

		// The other half of the same question. A property that answers in every
		// scope gets read as if the value belonged to that scope, so refusing the
		// wrong ones matters as much as answering the right ones. Each pair below
		// is one DLSMusicDevice also refuses -- checked against it, not assumed
		const prop_test wrong_scope[] = {
			{ kAudioUnitProperty_SupportedNumChannels,  kAudioUnitScope_Output },
			{ kAudioUnitProperty_Latency,               kAudioUnitScope_Output },
			{ kAudioUnitProperty_TailTime,              kAudioUnitScope_Output },
			{ kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Output },
			{ kAudioUnitProperty_SetRenderCallback,     kAudioUnitScope_Output },
			{ kAudioUnitProperty_ClassInfo,             kAudioUnitScope_Output },
		};
		for (const prop_test &p : wrong_scope) {
			UInt32 size = 0;
			if (AudioUnitGetPropertyInfo(u, p.id, p.scope, 0, &size, nullptr) == noErr) {
				std::printf("NG: プロパティ %u にスコープ %u で答えてしまう\n",
				            unsigned(p.id), unsigned(p.scope));
				bad++;
			}
		}

		// An unknown number must be answered with "don't know"
		UInt32 size = 0;
		if (AudioUnitGetPropertyInfo(u, 0x7fff, kAudioUnitScope_Global, 0, &size, nullptr) == noErr) {
			std::printf("NG: 知らないプロパティに noErr を返した\n");
			bad++;
		}
		std::printf("OK: プロパティの受け答え\n");
		AudioComponentInstanceDispose(u);
	}

	// 5. saving the state and reading it back
	{
		AudioUnit u = nullptr;
		AudioComponentInstanceNew(comp, &u);
		AudioUnitInitialize(u);

		// There is nothing inside until boot finishes. As vst3probe does, wait
		// until the blob has grown
		CFPropertyListRef plist = nullptr;
		CFIndex blob_size = 0;
		for (int t = 0; t < 300; t++) {
			if (plist)
				CFRelease(plist);
			plist = nullptr;
			UInt32 size = sizeof(plist);
			if (AudioUnitGetProperty(u, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0,
			                         &plist, &size) != noErr || !plist)
				break;
			CFDataRef d = static_cast<CFDataRef>(const_cast<void *>(
				CFDictionaryGetValue(static_cast<CFDictionaryRef>(plist), CFSTR("S-MU2000"))));
			blob_size = d ? CFDataGetLength(d) : 0;
			if (blob_size >= 1000)
				break;
			std::this_thread::sleep_for(std::chrono::milliseconds(50));
		}

		if (!plist) {
			std::printf("NG: ClassInfo を読めない\n");
			bad++;
		} else if (blob_size < 1000) {
			std::printf("NG: ClassInfo が %ld バイトしかない（機械の中身が入っていない）\n",
			            long(blob_size));
			bad++;
		} else {
			UInt32 size = sizeof(plist);
			if (AudioUnitSetProperty(u, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0,
			                         &plist, size) != noErr) {
				std::printf("NG: ClassInfo を書き戻せない\n");
				bad++;
			} else {
				std::printf("OK: 状態を %ld バイトで保存して読み戻した\n", long(blob_size));
			}
		}
		if (plist)
			CFRelease(plist);
		AudioUnitUninitialize(u);
		AudioComponentInstanceDispose(u);
	}

	// 6. four at once
	{
		const int N = 4;
		AudioUnit us[N] = {};
		for (int i = 0; i < N; i++)
			AudioComponentInstanceNew(comp, &us[i]);

		std::vector<float> l(512), r(512);
		for (int i = 0; i < N; i++) {
			const AudioStreamBasicDescription fmt = float_format(48000.0, 2);
			AudioUnitSetProperty(us[i], kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output,
			                     0, &fmt, sizeof(fmt));
			AudioUnitInitialize(us[i]);
		}
		std::printf("4 枚ぶん起動を待つ...\n");
		std::this_thread::sleep_for(std::chrono::milliseconds(9000));

		audio_buffers_2 bufs{};
		bufs.list.mNumberBuffers = 2;
		for (int i = 0; i < 2; i++) {
			bufs.list.mBuffers[i].mNumberChannels = 1;
			bufs.list.mBuffers[i].mDataByteSize = 512 * 4;
			bufs.list.mBuffers[i].mData = i == 0 ? (void *)l.data() : (void *)r.data();
		}
		AudioTimeStamp ts{};
		ts.mFlags = kAudioTimeStampSampleTimeValid;

		const long long t0 = now_ms();
		double peak = 0.0;
		for (int blk = 0; blk < 200; blk++)
			for (int i = 0; i < N; i++) {
				AudioUnitRenderActionFlags flags = 0;
				AudioUnitRender(us[i], &flags, &ts, 0, 512, &bufs.list);
				for (float v : l) peak = std::max(peak, std::fabs(double(v)));
			}
		const long long t1 = now_ms();
		const double audio = 200.0 * 512.0 / 48000.0;
		std::printf(" 4 枚同時に %.2f 秒ぶん作って実時間 %.2f 秒（1 枚あたり CPU %.0f%%）\n",
		            audio, (t1 - t0) / 1000.0, 100.0 * (t1 - t0) / 1000.0 / audio / 4.0);

		for (int i = 0; i < N; i++) {
			AudioUnitUninitialize(us[i]);
			AudioComponentInstanceDispose(us[i]);
		}
		std::printf("OK: 4 枚同時に作って捨てた\n");
	}

	// 7. the editor a host would put in its window
	{
		AudioUnit u = nullptr;
		AudioComponentInstanceNew(comp, &u);
		bad += check_editor(u, bundle_path);
		AudioComponentInstanceDispose(u);
	}

	// 8. the way a sandboxed host runs it: out of process, in AUHostingService
	//
	// GarageBand (and any other sandboxed host) cannot load the bundle into its
	// own process, so AudioToolbox hosts it for them. That layer builds its graph
	// from the unit's bus counts, and a unit that *counts* an input bus whose
	// input nobody connected then fails every render with
	// kAudioUnitErr_NoConnection -- no audio, and the machine never advances, so
	// the panel sits on its power-on screen. Declaration, not code: the same unit
	// renders perfectly in process, and the same unit renders out of process once
	// something is connected to that bus.
	//
	// Only a component the system has on disk can be hosted that way, so this
	// runs when the bundle we were handed is the installed one; otherwise it says
	// so and moves on (make install-au first)
	{
		// A bundle can only be hosted that way once the system knows about it,
		// which is what the standard directories are -- an AU found in the build
		// tree is not one the AudioComponentRegistrar can hand out. So the test
		// runs when the bundle we were handed is one of those, and says so when it
		// is not, rather than testing whatever happens to be installed instead
		char here_s[PATH_MAX] = {};
		const char *at = realpath(bundle_path.c_str(), here_s) ? here_s : bundle_path.c_str();
		const std::string where(at);
		const char *homes[] = { "/Library/Audio/Plug-Ins/Components/",
		                        "/System/Library/Components/" };
		std::string user_dir;
		if (const char *h = std::getenv("HOME"))
			user_dir = std::string(h) + "/Library/Audio/Plug-Ins/Components/";
		bool installed = !user_dir.empty() && where.rfind(user_dir, 0) == 0;
		for (const char *d : homes)
			installed = installed || where.rfind(d, 0) == 0;

		AudioComponentDescription want{};
		want.componentType = kType;
		want.componentSubType = kSubtype;
		want.componentManufacturer = kManufacturer;
		AudioComponent system_comp = installed ? AudioComponentFindNext(nullptr, &want) : nullptr;
		if (!system_comp) {
			std::printf("OK: 別プロセスの口は飛ばした（make install-au して、入れた束を渡すと試す）\n");
		} else {
			// The service inherits this environment but not this working
			// directory, so a relative ROM path has to be made absolute first --
			// otherwise the service cannot find the ROMs and this measures that
			// rather than the unit
			if (const char *r = std::getenv("S_MU2000_ROMS")) {
				char abs[PATH_MAX] = {};
				if (realpath(r, abs))
					setenv("S_MU2000_ROMS", abs, 1);
			}
			__block AudioUnit u = nullptr;
			__block OSStatus oerr = noErr;
			dispatch_semaphore_t sem = dispatch_semaphore_create(0);
			AudioComponentInstantiate(system_comp, kAudioComponentInstantiation_LoadOutOfProcess,
			                          ^(AudioComponentInstance inst, OSStatus e) {
				u = inst;
				oerr = e;
				dispatch_semaphore_signal(sem);
			});
			dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 10ll * NSEC_PER_SEC));
			if (!u) {
				std::printf("NG: 別プロセスで開けない（%d）\n", int(oerr));
				bad++;
			} else {
				const AudioStreamBasicDescription ofmt = float_format(44100.0, 2);
				AudioUnitSetProperty(u, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 0,
				                     &ofmt, sizeof(ofmt));
				AudioUnitInitialize(u);

				const UInt32 frames = 512;
				std::vector<float> l(frames), r(frames);
				audio_buffers_2 bufs{};
				bufs.list.mNumberBuffers = 2;
				bufs.list.mBuffers[0].mNumberChannels = 1;
				bufs.list.mBuffers[0].mDataByteSize = frames * 4;
				bufs.list.mBuffers[0].mData = l.data();
				bufs.list.mBuffers[1].mNumberChannels = 1;
				bufs.list.mBuffers[1].mDataByteSize = frames * 4;
				bufs.list.mBuffers[1].mData = r.data();
				AudioTimeStamp ts{};
				ts.mFlags = kAudioTimeStampSampleTimeValid;

				// The machine boots on its own thread inside the service. Rendering
				// faster than it boots would measure the boot, not the unit, so wait
				// for it the way the render above does, over the same round trip
				for (int t = 0; t < 600; t++) {
					AudioUnitParameterValue v = 0;
					AudioUnitGetParameter(u, 1 /* status */, kAudioUnitScope_Global, 0, &v);
					if (v >= 1.0f)
						break;
					std::this_thread::sleep_for(std::chrono::milliseconds(50));
				}

				const unsigned char on[] = { 0x90, 60, 100 };
				send_midi(u, std::vector<u8>(on, on + 3), 0);

				OSStatus first_err = noErr;
				double peak = 0.0;
				for (int blk = 0; blk < 44100 / int(frames); blk++) {
					AudioUnitRenderActionFlags flags = 0;
					const OSStatus st = AudioUnitRender(u, &flags, &ts, 0, frames, &bufs.list);
					if (st != noErr && first_err == noErr)
						first_err = st;
					for (float v : l)
						peak = std::max(peak, std::fabs(double(v)));
					ts.mSampleTime += frames;
				}
				if (first_err != noErr) {
					std::printf("NG: 別プロセスの Render が %d で落ちる\n", int(first_err));
					bad++;
				} else if (peak <= 0.0) {
					std::printf("NG: 別プロセスでは音が出ない\n");
					bad++;
				}
				AudioUnitUninitialize(u);
				AudioComponentInstanceDispose(u);
				std::printf("OK: 別プロセス（AUHostingService）でも Render できた\n");
			}
		}
	}

	std::printf("---- 悪いところ %d 件 ----\n", bad);
	return bad ? 1 : 0;
}

} // namespace


int main(int argc, char **argv)
{
	smu2000::init_console_utf8();

	if (argc < 2) {
		std::fprintf(stderr,
			"使い方: aubprobe <バンドル|-> [<MIDI> [<出力 wav>]] [--rate 48000] [--block 512]\n"
			"                                 [--tail 3] [--torture] [--list]\n");
		return 1;
	}

	std::string bundle = argv[1], mid, wav;
	double rate = 48000.0;
	int block = 512;
	double extra = 3.0;
	bool torture = false, list = false;
	for (int i = 2; i < argc; i++) {
		if (!std::strcmp(argv[i], "--rate") && i + 1 < argc) rate = std::atof(argv[++i]);
		else if (!std::strcmp(argv[i], "--block") && i + 1 < argc) block = std::atoi(argv[++i]);
		else if (!std::strcmp(argv[i], "--tail") && i + 1 < argc) extra = std::atof(argv[++i]);
		else if (!std::strcmp(argv[i], "--torture")) torture = true;
		else if (!std::strcmp(argv[i], "--list")) list = true;
		else if (mid.empty()) mid = argv[i];
		else if (wav.empty()) wav = argv[i];
	}

	std::string err;
	AudioComponent comp = find_component(bundle, err);
	if (!comp) {
		std::fprintf(stderr, "%s\n", err.empty() ? "AU が見つからない" : err.c_str());
		return 1;
	}

	AudioComponentDescription got{};
	AudioComponentGetDescription(comp, &got);
	CFStringRef cname = nullptr;
	AudioComponentCopyName(comp, &cname);
	char name[256] = {};
	if (cname) {
		CFStringGetCString(cname, name, sizeof(name), kCFStringEncodingUTF8);
		CFRelease(cname);
	}
	std::printf("見つけた: %s\n", name);
	std::printf("  種別 %c%c%c%c / 番号 %c%c%c%c / 作り手 %c%c%c%c\n",
	            char(got.componentType >> 24), char(got.componentType >> 16),
	            char(got.componentType >> 8), char(got.componentType),
	            char(got.componentSubType >> 24), char(got.componentSubType >> 16),
	            char(got.componentSubType >> 8), char(got.componentSubType),
	            char(got.componentManufacturer >> 24), char(got.componentManufacturer >> 16),
	            char(got.componentManufacturer >> 8), char(got.componentManufacturer));

	{
		AudioUnit u = nullptr;
		if (AudioComponentInstanceNew(comp, &u) != noErr || !u) {
			std::fprintf(stderr, "開けない\n");
			return 1;
		}
		std::printf("開けた\n");
		if (list)
			show_info(u);
		AudioComponentInstanceDispose(u);
	}

	int rc = 0;
	if (!mid.empty() && !wav.empty()) {
		AudioUnit u = nullptr;
		if (AudioComponentInstanceNew(comp, &u) != noErr) {
			std::fprintf(stderr, "開けない\n");
			return 1;
		}
		rc = run_render(u, rate, block, extra, mid, wav);
		AudioComponentInstanceDispose(u);
	} else if (!torture && !list) {
		std::printf("音は出していない（MIDI と出力先を渡すと鳴らす）\n");
	}

	if (torture && run_torture(comp, bundle))
		rc = 1;
	return rc;
}
