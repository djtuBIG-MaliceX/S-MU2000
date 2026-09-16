// license:BSD-3-Clause
//
// CoreAudio input for macOS: the device the machine samples as its A/D INPUT.
// Same interface as audio_in.cpp (WASAPI) -- list(), start(), pop() -- so
// engine::fill() and the front ends do not know which platform they are on.
//
// A HAL input AudioUnit calls us on its own real-time thread, so unlike the
// Windows side there is no worker thread here (the output side is the same).
// What the device says (48000Hz float, say) is converted to 44100Hz 16bit 2ch
// with ui::resampler and pushed into the ring that pop() reads, which is the
// arrangement audio_in.cpp describes: the two clocks drift, so the ring is the
// elastic part between them.

#include "audio_in.h"
#include "audio_out.h"          // AUDIO_RATE, shared with the output side
#include "resampler.h"

#include <AudioToolbox/AudioToolbox.h>
#include <CoreAudio/CoreAudio.h>

#include <algorithm>
#include <cctype>
#include <cmath>
#include <cstdio>
#include <cstring>

namespace ui {

namespace {

AudioDeviceID default_input_device()
{
	// AudioObjectPropertyAddress is { selector, scope, element } in that order
	AudioObjectPropertyAddress addr = {
		kAudioHardwarePropertyDefaultInputDevice,
		kAudioObjectPropertyScopeGlobal,
		kAudioObjectPropertyElementMain
	};
	AudioDeviceID dev = kAudioObjectUnknown;
	UInt32 size = sizeof(dev);
	if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &addr, 0, nullptr,
	                               &size, &dev) != noErr)
		return kAudioObjectUnknown;
	return dev;
}

// Does this device have anything to record from? The device list also holds
// output-only devices, and offering those as an input would be a lie
bool has_input(AudioDeviceID dev)
{
	AudioObjectPropertyAddress addr = {
		kAudioDevicePropertyStreams,
		kAudioObjectPropertyScopeInput,
		kAudioObjectPropertyElementMain
	};
	UInt32 size = 0;
	if (AudioObjectGetPropertyDataSize(dev, &addr, 0, nullptr, &size) != noErr)
		return false;
	return size >= sizeof(AudioStreamID);
}

// How many channels this device records. Read from the stream configuration,
// which is a list of buffers, one per stream
u32 input_channels(AudioDeviceID dev)
{
	AudioObjectPropertyAddress addr = {
		kAudioDevicePropertyStreamConfiguration,
		kAudioObjectPropertyScopeInput,
		kAudioObjectPropertyElementMain
	};
	UInt32 size = 0;
	if (AudioObjectGetPropertyDataSize(dev, &addr, 0, nullptr, &size) != noErr || !size)
		return 0;
	std::vector<u8> room(size);
	auto *list = reinterpret_cast<AudioBufferList *>(room.data());
	if (AudioObjectGetPropertyData(dev, &addr, 0, nullptr, &size, list) != noErr)
		return 0;
	u32 ch = 0;
	for (UInt32 i = 0; i < list->mNumberBuffers; i++)
		ch += list->mBuffers[i].mNumberChannels;
	return ch;
}

double nominal_rate(AudioDeviceID dev)
{
	AudioObjectPropertyAddress addr = {
		kAudioDevicePropertyNominalSampleRate,
		kAudioObjectPropertyScopeGlobal,
		kAudioObjectPropertyElementMain
	};
	Float64 rate = 0.0;
	UInt32 size = sizeof(rate);
	if (AudioObjectGetPropertyData(dev, &addr, 0, nullptr, &size, &rate) != noErr)
		return 0.0;
	return double(rate);
}

u32 buffer_frames(AudioDeviceID dev)
{
	AudioObjectPropertyAddress addr = {
		kAudioDevicePropertyBufferFrameSize,
		kAudioObjectPropertyScopeGlobal,
		kAudioObjectPropertyElementMain
	};
	UInt32 frames = 0;
	UInt32 size = sizeof(frames);
	if (AudioObjectGetPropertyData(dev, &addr, 0, nullptr, &size, &frames) != noErr)
		return 0;
	return frames;
}

// Named rather than called device_name(), which would collide with the member
// function of the same name wherever one is in scope
std::string name_of(AudioDeviceID dev)
{
	AudioObjectPropertyAddress addr = {
		kAudioObjectPropertyName,
		kAudioObjectPropertyScopeGlobal,
		kAudioObjectPropertyElementMain
	};
	CFStringRef name = nullptr;
	UInt32 size = sizeof(name);
	if (AudioObjectGetPropertyData(dev, &addr, 0, nullptr, &size, &name) != noErr || !name)
		return {};
	char buf[256] = {};
	const bool ok = CFStringGetCString(name, buf, sizeof(buf), kCFStringEncodingUTF8);
	CFRelease(name);
	return ok ? std::string(buf) : std::string();
}

std::vector<AudioDeviceID> input_devices()
{
	AudioObjectPropertyAddress addr = {
		kAudioHardwarePropertyDevices,
		kAudioObjectPropertyScopeGlobal,
		kAudioObjectPropertyElementMain
	};
	UInt32 size = 0;
	if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &addr, 0, nullptr, &size) != noErr)
		return {};
	std::vector<AudioDeviceID> devs(size / sizeof(AudioDeviceID));
	if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &addr, 0, nullptr, &size,
	                               devs.data()) != noErr)
		return {};
	devs.erase(std::remove_if(devs.begin(), devs.end(),
	                          [](AudioDeviceID d) { return !has_input(d); }),
	           devs.end());
	return devs;
}

std::string lowered(const std::string &s)
{
	std::string out;
	out.reserve(s.size());
	for (char c : s)
		out.push_back(char(std::tolower((unsigned char)c)));
	return out;
}

} // namespace


struct audio_in::impl
{
	static constexpr u32 RING = 1 << 16, MASK = RING - 1;      // 約 1.5 秒
	static constexpr u32 TARGET_FRAMES = 2205;                  // 50ms
	static constexpr u32 DROP_FRAMES = 8820;                    // 200ms を超えたら捨てる

	AudioDeviceID dev = kAudioObjectUnknown;
	AudioUnit     unit = nullptr;

	std::string want, dev_name, err;
	u32  dev_rate = 0, dev_channels = 0;

	resampler rs;
	// The buffers the input callback renders into, one per device channel since
	// the client format asked for non-interleaved float. Allocated once, sized
	// to the maximum the unit says it will ask for
	std::vector<u8>   room;        // storage for the AudioBufferList and its buffers
	AudioBufferList  *list = nullptr;
	u32               cap_frames = 0;

	std::vector<s16>  staging;     // device frames, already 2ch 16bit
	std::vector<float> conv;
	std::vector<s16>  out16;

	std::vector<s16> m_ring = std::vector<s16>(size_t(RING) * 2);
	std::atomic<u32> m_w{0}, m_r{0};
	std::atomic<u64> m_empty{0}, m_dropped{0};

	// 44100Hz 16bit 2ch を輪に積む。溢れる分は捨てる（読み手が止まっている）
	void push(const s16 *frames, u32 n)
	{
		u32 wr = m_w.load(std::memory_order_relaxed);
		for (u32 i = 0; i < n; i++) {
			const u32 rd = m_r.load(std::memory_order_acquire);
			if (((wr + 1) & MASK) == rd)
				break;
			m_ring[wr * 2] = frames[i * 2];
			m_ring[wr * 2 + 1] = frames[i * 2 + 1];
			wr = (wr + 1) & MASK;
			m_w.store(wr, std::memory_order_release);
		}
	}

	static OSStatus input_proc(void *ref, AudioUnitRenderActionFlags *flags,
	                           const AudioTimeStamp *ts, UInt32 bus, UInt32 frames,
	                           AudioBufferList * /*unused*/)
	{
		static_cast<impl *>(ref)->handle(flags, ts, bus, frames);
		return noErr;
	}

	// Called on the AudioUnit's real-time thread. What the device has recorded
	// since last time is asked for with AudioUnitRender, then converted
	void handle(AudioUnitRenderActionFlags *flags, const AudioTimeStamp *ts,
	            UInt32 bus, UInt32 frames)
	{
		if (frames > cap_frames)
			return;                        // should not happen; MaximumFramesPerSlice covers it
		for (u32 c = 0; c < dev_channels; c++) {
			list->mBuffers[c].mDataByteSize = frames * sizeof(float);
			list->mBuffers[c].mData = buffers[c];
		}
		if (AudioUnitRender(unit, flags, ts, bus, frames, list) != noErr)
			return;

		// 2ch の 16bit に直す。1ch なら両方に、3ch 以上は頭の 2 つ
		staging.resize(size_t(frames) * 2);
		const u32 ch = dev_channels;
		for (UInt32 i = 0; i < frames; i++) {
			for (u32 c = 0; c < 2; c++) {
				const u32 src = std::min(c, ch - 1);
				const auto *src_buf = static_cast<const float *>(list->mBuffers[src].mData);
				const float f = src_buf ? src_buf[i] : 0.0f;
				staging[size_t(i) * 2 + c] = s16(std::lround(
				    std::clamp(f, -1.0f, 1.0f) * 32767.0f));
			}
		}

		if (rs.direct()) {
			push(staging.data(), frames);
			return;
		}
		// 変換器の輪は 4096 フレーム。1 回に入れる量をそれより小さく刻む
		for (UInt32 at = 0; at < frames;) {
			const UInt32 k = std::min<UInt32>(1024, frames - at);
			rs.push(staging.data() + size_t(at) * 2, int(k));
			at += k;
			const int n = rs.output_available();
			if (n <= 0)
				continue;
			conv.resize(size_t(n) * 2);
			out16.resize(size_t(n) * 2);
			rs.pull(conv.data(), n);
			for (size_t j = 0; j < out16.size(); j++)
				out16[j] = s16(std::lround(std::clamp(conv[j], -1.0f, 1.0f) * 32767.0f));
			push(out16.data(), u32(n));
		}
	}

	// The per-channel pointers the callback renders into. Kept beside `room`
	// rather than inside the AudioBufferList, which is flat storage
	std::vector<void *> buffers;

	void release()
	{
		if (unit) {
			AudioOutputUnitStop(unit);
			AudioUnitUninitialize(unit);
			AudioComponentInstanceDispose(unit);
			unit = nullptr;
		}
		list = nullptr;
		buffers.clear();
		room.clear();
	}
};


audio_in::audio_in() : m_impl(new impl) {}
audio_in::~audio_in() { stop(); }


std::vector<std::string> audio_in::list()
{
	std::vector<std::string> out;
	for (AudioDeviceID d : input_devices()) {
		const std::string n = name_of(d);
		if (!n.empty())
			out.push_back(n);
	}
	return out;
}

bool audio_in::start(const std::string &device, std::string &err)
{
	stop();
	impl &im = *m_impl;
	im.want = device;

	auto fail = [&](const char *what) {
		im.err = what;
		err = what;
		im.release();
		return false;
	};

	// Which device. An empty name is the system default. A name is matched the
	// way the Windows side matches it (exact), but a prefix is accepted too so
	// that a shortened name still finds the device
	if (im.want.empty()) {
		im.dev = default_input_device();
	} else {
		for (AudioDeviceID d : input_devices())
			if (name_of(d) == im.want) { im.dev = d; break; }
		if (im.dev == kAudioObjectUnknown) {
			const std::string needle = lowered(im.want);
			for (AudioDeviceID d : input_devices())
				if (lowered(name_of(d)).find(needle) != std::string::npos) { im.dev = d; break; }
		}
	}
	if (im.dev == kAudioObjectUnknown)
		return fail("録音デバイスが見つからない");
	im.dev_name = name_of(im.dev);

	const u32 ch = input_channels(im.dev);
	const double rate = nominal_rate(im.dev);
	if (!ch || rate <= 0.0)
		return fail("録音デバイスの形式が読めない");
	im.dev_channels = ch;
	im.dev_rate = u32(rate + 0.5);

	// The input side of a HAL unit. Output is turned off so the unit only
	// records; the device is pinned with CurrentDevice rather than left at the
	// system default, or choosing a device in the menu would do nothing
	AudioComponentDescription desc = {};
	desc.componentType = kAudioUnitType_Output;
	desc.componentSubType = kAudioUnitSubType_HALOutput;
	desc.componentManufacturer = kAudioUnitManufacturer_Apple;
	AudioComponent comp = AudioComponentFindNext(nullptr, &desc);
	if (!comp || AudioComponentInstanceNew(comp, &im.unit) != noErr)
		return fail("録音の口を開けない");

	UInt32 on = 1, off = 0;
	if (AudioUnitSetProperty(im.unit, kAudioOutputUnitProperty_EnableIO,
	                         kAudioUnitScope_Input, 1, &on, sizeof(on)) != noErr ||
	    AudioUnitSetProperty(im.unit, kAudioOutputUnitProperty_EnableIO,
	                         kAudioUnitScope_Output, 0, &off, sizeof(off)) != noErr ||
	    AudioUnitSetProperty(im.unit, kAudioOutputUnitProperty_CurrentDevice,
	                         kAudioUnitScope_Global, 0, &im.dev, sizeof(im.dev)) != noErr)
		return fail("録音デバイスを選べない");

	// What we want handed to the callback: float, non-interleaved (one buffer
	// per channel), at the device's own rate. The conversion to 44100Hz is ours.
	//
	// **The scope is Output, element 1.** On a HAL unit the input bus is element
	// 1: the *device* side of it is the unit's Input scope (that is what EnableIO
	// above turns on) and the side we read from is its Output scope. Setting the
	// format on Input/1 asks the device for a format, which it refuses
	AudioStreamBasicDescription fmt = {};
	fmt.mSampleRate = rate;
	fmt.mFormatID = kAudioFormatLinearPCM;
	fmt.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked |
	                   kAudioFormatFlagIsNonInterleaved;
	fmt.mFramesPerPacket = 1;
	fmt.mChannelsPerFrame = ch;
	fmt.mBitsPerChannel = 32;
	fmt.mBytesPerFrame = 4;
	fmt.mBytesPerPacket = 4;
	if (AudioUnitSetProperty(im.unit, kAudioUnitProperty_StreamFormat,
	                         kAudioUnitScope_Output, 1, &fmt, sizeof(fmt)) != noErr)
		return fail("録音の形式を決められない");

	// How much the unit may ask for in one callback. Reading the device's own
	// buffer size and setting the slice to it keeps the buffers below big
	// enough for anything that arrives
	u32 cap = buffer_frames(im.dev);
	if (!cap)
		cap = 4096;
	if (AudioUnitSetProperty(im.unit, kAudioUnitProperty_MaximumFramesPerSlice,
	                         kAudioUnitScope_Global, 0, &cap, sizeof(cap)) != noErr)
		return fail("録音の刻みを決められない");
	im.cap_frames = cap;

	// One buffer per channel, with the room for the list and the audio itself
	// in one allocation
	const size_t list_bytes = sizeof(AudioBufferList) + sizeof(AudioBuffer) * (ch - 1);
	im.room.resize(list_bytes + size_t(ch) * cap * sizeof(float));
	im.list = reinterpret_cast<AudioBufferList *>(im.room.data());
	im.list->mNumberBuffers = ch;
	im.buffers.resize(ch);
	u8 *audio = im.room.data() + list_bytes;
	for (u32 c = 0; c < ch; c++) {
		im.buffers[c] = audio + size_t(c) * cap * sizeof(float);
		im.list->mBuffers[c].mNumberChannels = 1;
		im.list->mBuffers[c].mDataByteSize = cap * sizeof(float);
		im.list->mBuffers[c].mData = im.buffers[c];
	}

	AURenderCallbackStruct cb = {};
	cb.inputProc = impl::input_proc;
	cb.inputProcRefCon = &im;
	if (AudioUnitSetProperty(im.unit, kAudioOutputUnitProperty_SetInputCallback,
	                         kAudioUnitScope_Global, 0, &cb, sizeof(cb)) != noErr)
		return fail("録音の受け口を付けられない");

	im.staging.reserve(size_t(cap) * 2);
	im.rs.configure(rate, double(AUDIO_RATE));

	if (AudioUnitInitialize(im.unit) != noErr)
		return fail("録音の準備");
	if (AudioOutputUnitStart(im.unit) != noErr) {
		AudioUnitUninitialize(im.unit);
		return fail("録音の開始");
	}
	return true;
}

void audio_in::stop()
{
	if (!m_impl)
		return;
	m_impl->release();
}

bool audio_in::running() const
{
	return m_impl && m_impl->unit != nullptr;
}

// One frame (44100Hz, 16bit scale). 0 while the ring is empty
void audio_in::pop(s32 &l, s32 &r)
{
	impl &im = *m_impl;
	u32 rd = im.m_r.load(std::memory_order_relaxed);
	const u32 wr = im.m_w.load(std::memory_order_acquire);
	u32 level = (wr - rd) & impl::MASK;
	if (level > impl::DROP_FRAMES) {
		// 溜まり過ぎ。目標まで古い分を捨てる
		rd = (wr - impl::TARGET_FRAMES) & impl::MASK;
		level = impl::TARGET_FRAMES;
		im.m_dropped.fetch_add(1, std::memory_order_relaxed);
	}
	if (!level) {
		l = r = 0;
		if (running())
			im.m_empty.fetch_add(1, std::memory_order_relaxed);
		return;
	}
	l = im.m_ring[rd * 2];
	r = im.m_ring[rd * 2 + 1];
	im.m_r.store((rd + 1) & impl::MASK, std::memory_order_relaxed);
}

std::string audio_in::device_name() const { return m_impl->dev_name; }

std::string audio_in::format_line() const
{
	char buf[160];
	std::snprintf(buf, sizeof buf, "CoreAudio / %u Hz %u ch float32 → 44100 Hz",
	              m_impl->dev_rate, m_impl->dev_channels);
	return buf;
}

u64 audio_in::empty_count() const   { return m_impl->m_empty.load(); }
u64 audio_in::dropped_count() const { return m_impl->m_dropped.load(); }

} // namespace ui
