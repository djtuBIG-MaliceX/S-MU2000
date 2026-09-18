// license:BSD-3-Clause
//
// S-MU2000 as an Audio Unit v2 (type aumu = MusicDevice).
//
// It runs the **same engine** as the VST3 plug-in (src/vst3/plugin.cpp): making
// the audio, finding and booting the ROMs, resampling to the host's rate and
// packing the machine's state all happen there. What is here is only the AU
// side of the host interface.
//
//   engine.h         boot / MIDI / fill / state (no VST3 types appear in it)
//   vst3/plugin.cpp  the VST3 side (IComponent, IAudioProcessor, IEditController)
//   au/plugin.cpp    this file: the AU side (AudioComponentPlugInInterface)
//
// For the same reason Steinberg's public.sdk (GPLv3) was left out and only
// pluginterfaces (MIT) was brought in, Apple's AudioUnitSDK is not vendored
// either. An AUv2's wiring is a fixed table, so the parts that are needed are
// written out here.
//
// An AUv2 is never dlopen'd. A host reads the bundle in the Components
// directory, finds the name given by factoryFunction in Info.plist's
// AudioComponents entry, and calls that symbol -- SMU2000AUFactory below. The
// AudioComponentPlugInInterface it hands back has Open / Close / Lookup, and
// Lookup is the "selector number -> function" table.
//
// Checking it:
//   make au && make au-probe
//   build/aubprobe build/S-MU2000.component song.mid out.wav   (own host)
//   auval -v aumu SMU2 Trbh                                    (Apple's validator)

#include "editor.h"
#include "render_watch.h"
#include "state.h"
#include "vst3/engine.h"

#include <AudioToolbox/AudioToolbox.h>
#include <CoreFoundation/CoreFoundation.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <vector>

namespace {

// ---- Identity. Once chosen these cannot change: a host would stop finding the
//      plug-in, and saved sessions would no longer match it
constexpr OSType kType         = 'aumu';
constexpr OSType kSubtype      = 'SMU2';
// The manufacturer code is mixed case on purpose: auval treats an all-lowercase
// manufacturer as an error ("should have at least one non-lower case
// character") and refuses to open the unit at all, however well it works
constexpr OSType kManufacturer = 'Trbh';
constexpr UInt32 kVersion      = 0x00010000;      // 0.1.0

// The one factory preset. The name PresentPreset returns and the name inside
// ClassInfo have to agree, because auval compares them
constexpr const char *kPresetName = "S-MU2000";

// aumu's element rules: the audio output is element 0 of the output scope, and
// the MIDI input is element 1 of the input scope. The latter has no stream
// format, so it never shows up as a property -- MIDI arrives through the
// MusicDevice entry points
constexpr UInt32 kOutputElement = 0;
constexpr UInt32 kGlobalElement = 0;

// ---- Parameters
//
// An AU has no MIDI-number-to-parameter convention like VST3's IMidiMapping,
// so there is nothing to map and no reason to build the VST3 side's 2096 of
// them. These are only what a host's generic panel can usefully show; MIDI goes
// in through the MusicDevice entry points instead
enum : AudioUnitParameterID {
	kParamGain   = 0,
	kParamStatus = 1,
	kParamCount  = 2,
};

constexpr UInt32 kMaxFramesDefault = 1156;
// How many MIDI messages may be waiting between two render blocks. Past this
// the oldest is dropped rather than growing the queue without limit
constexpr size_t kMidiReserveMsgs  = 512;

// ---- Where MIDI from the host waits
//
// MusicDeviceMIDIEvent is not necessarily called from the audio thread, while
// the engine's midi() is audio-thread-only (it touches the pre-boot queue). So
// events are parked here and drained inside Render, which takes the lock with
// try_lock: if it is busy they are simply picked up in the next block
//
// Short messages (everything but SysEx) are kept inline so the audio thread
// never touches the allocator while draining: queue() may allocate on the
// MIDI thread for SysEx only, Render only reads.
struct msg
{
	UInt32 offset = 0;
	UInt32 seq = 0;          // arrival order, for a stable sort by offset
	UInt8  n = 0;            // inline bytes used (0 when sysex holds the message)
	UInt8  b[3] = {};
	std::vector<UInt8> sysex;

	const UInt8 *data() const { return n ? b : (sysex.empty() ? nullptr : sysex.data()); }
	size_t size() const { return n ? n : sysex.size(); }
	bool empty() const { return size() == 0; }
};

AudioStreamBasicDescription default_format()
{
	AudioStreamBasicDescription f{};
	f.mSampleRate       = smu2000::vst3::NATIVE_RATE;
	f.mFormatID         = kAudioFormatLinearPCM;
	// The two flags come from different anonymous enums, so the or needs a cast
	f.mFormatFlags      = AudioFormatFlags(kAudioFormatFlagsNativeFloatPacked) |
	                      AudioFormatFlags(kAudioFormatFlagIsNonInterleaved);
	f.mChannelsPerFrame = 2;
	f.mBitsPerChannel   = 32;
	f.mFramesPerPacket  = 1;
	f.mBytesPerFrame    = 4;
	f.mBytesPerPacket   = 4;
	return f;
}

} // namespace


// ---------------------------------------------------------------------------
// The plugin object.
//
// `iface` must stay first: the host is handed &iface, and hands that same
// pointer back as `self` for every method below, so the cast only works if the
// two share an address.

struct au_instance
{
	AudioComponentPlugInInterface iface{};
	AudioComponentInstance instance = nullptr;

	smu2000::vst3::engine eng;

	// ---- Settings
	AudioStreamBasicDescription out_format{};
	UInt32 max_frames = kMaxFramesDefault;

	// What the host is doing: block size, format, MIDI offsets, late blocks.
	// Silent while everything is in order; the summary comes out on Close.
	// See au/render_watch.h for what is counted and why
	smu2000::au::render_watch host_watch;

	// The shape of a block, once, for the watch's first line
	void note_layout(const AudioBufferList *io)
	{
		if (!io || io->mNumberBuffers == 0)
			return;
		UInt32 chans[smu2000::au::render_watch::kMaxBuffers] = {};
		const UInt32 n =
		    std::min<UInt32>(io->mNumberBuffers, smu2000::au::render_watch::kMaxBuffers);
		for (UInt32 i = 0; i < n; i++)
			chans[i] = io->mBuffers[i].mNumberChannels;
		host_watch.note_layout(io->mNumberBuffers, chans, n,
		                       io->mBuffers[0].mDataByteSize, io->mBuffers[0].mData);
	}

	// The end of every block that ran, whatever it produced
	void watch_end(UInt32 frames, std::chrono::steady_clock::time_point t0)
	{
		host_watch.set_rate(out_format.mSampleRate);
		host_watch.set_last_block(frames, max_frames);
		host_watch.note_block(frames, uint64_t(std::chrono::duration_cast<std::chrono::microseconds>(
		                                          std::chrono::steady_clock::now() - t0).count()));
		char b[320];
		if (host_watch.layout_line(b, sizeof(b)))
			eng.log_line(b);
		if (host_watch.report_line(b, sizeof(b)))
			eng.log_line(b);
	}
	UInt32 render_quality = 0;
	bool   initialized = false;
	AudioUnitParameterValue gain = 1.0f;

	// ---- Render notifies the host asked to be called back through
	struct notify { AURenderCallback proc; void *ref; };
	std::vector<notify> render_notifies;

	// ---- Property listeners. An id of 0 means "every property"
	struct watch { AudioUnitPropertyID id; AudioUnitPropertyListenerProc proc; void *ref; };
	std::vector<watch> watchers;

	// ---- The MIDI hand-off: midi_in is the host's side, midi_work the audio's
	std::mutex midi_mutex;
	std::vector<msg> midi_in;
	std::vector<msg> midi_work;
	UInt32 midi_seq = 0;
	// Sounded channels (16 bits for MIDI IN A). On stop, all-sound-off +
	// all-notes-off goes only there. Blasting every channel costs 61ms of
	// 31250bps serial, delaying whatever comes next (issue #15)
	std::atomic<UInt16> sounded{0};
	// The preset name as last set. auval round-trips it through ClassInfo
	// and PresentPreset and warns when it does not stick, so it is kept
	std::string preset_name{kPresetName};

	// Hush only the channels that sounded. Nothing to send when none did
	void hush()
	{
		uint16_t mask[mu2000::MIDI_PORTS] = {};
		mask[0] = sounded.exchange(0);
		if (mask[0])
			eng.all_notes_off(mask, mu2000::MIDI_PORTS);
	}
	// Output-level ramp state. The target lives in gain (set from any thread);
	// the audio thread walks gain_now toward it, as the VST3/CLAP builds do,
	// so automation never steps mid-block.
	float gain_now = 1.0f;

	// ---- The A/D INPUT bus, offered only to a host that asks for it
	//
	// Counting an input bus is a promise that something will be connected to it,
	// and the layer that hosts a unit out of process (AUHostingService -- every
	// sandboxed host, GarageBand among them) builds its graph from those counts.
	// A counted bus whose input nobody connected then makes every render come
	// back kAudioUnitErr_NoConnection: no audio, and the machine never advances,
	// so the panel sits on its power-on screen. An instrument is never handed an
	// input connection there, so counting the bus by default can only break that
	// promise -- and an effect is never in this position, because a host connects
	// an effect's input, which is what an effect is.
	//
	// So the bus is the host's to ask for, the way a side-chain input is added:
	// ElementCount on the input scope starts at 0, and a host that writes 1 gets
	// the bus -- its format, its render callback, and the machine's real A/D
	// input through engine::fill. Only that host is handed the promise, and it is
	// the host that keeps it. Measured both ways, in process and out, in
	// doc/porting-macos.md
	bool input_bus = false;
	AURenderCallbackStruct in_cb{};
	bool in_cb_set = false;
	// A graph connection (kAudioUnitProperty_MakeConnection) lands here; kept
	// apart from the callback because the two properties take same-sized but
	// different-typed structs
	AudioUnitConnection conn{};
	// One block of A/D INPUT. Grown to the host's block size, which never exceeds
	// max_frames, so render does not touch the allocator
	std::vector<float> in_l, in_r;

	// The host's own input, pulled once per block. A host that fails to hand it
	// over expects to hear about it from AURender, so the error travels up rather
	// than being swallowed, and the block it did not fill is silenced so nothing
	// stale is played in its place
	OSStatus pull_input(UInt32 frames, const AudioTimeStamp *ts)
	{
		// Called every block: until the host asks for the bus (ElementCount 1)
		// there is nothing to pull and this is a no-op, whatever a host stored
		// in the configurable-but-uncounted input scope
		if (!input_bus || !in_cb_set || !in_cb.inputProc)
			return noErr;
		if (in_l.size() < frames) {
			in_l.resize(frames);
			in_r.resize(frames);
		}
		// AudioBufferList declares room for one buffer, so a second is added by
		// hand: the unit's input is non-interleaved 32-bit float, which is what
		// default_format() asks for and what a host rendering into it will have
		struct { AudioBufferList list; AudioBuffer second; } bl{};
		bl.list.mNumberBuffers = 2;
		bl.list.mBuffers[0].mNumberChannels = 1;
		bl.list.mBuffers[0].mDataByteSize = frames * sizeof(float);
		bl.list.mBuffers[0].mData = in_l.data();
		bl.second.mNumberChannels = 1;
		bl.second.mDataByteSize = frames * sizeof(float);
		bl.second.mData = in_r.data();
		AudioUnitRenderActionFlags f = 0;
		// The render's own timestamp travels with the pull: a host handed 0 here
		// sees its input arrive at the wrong time (auval: "AU is not passing time
		// stamp correctly")
		AudioTimeStamp at = ts ? *ts : AudioTimeStamp{};
		if (!(at.mFlags & kAudioTimeStampSampleTimeValid)) {
			at.mSampleTime = 0;
			at.mFlags |= kAudioTimeStampSampleTimeValid;
		}
		const OSStatus rc = in_cb.inputProc(in_cb.inputProcRefCon, &f, &at, 0, frames, &bl.list);
		if (rc != noErr) {
			std::fill(in_l.begin(), in_l.begin() + frames, 0.0f);
			std::fill(in_r.begin(), in_r.begin() + frames, 0.0f);
		}
		return rc;
	}

	void notify_all(AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement element)
	{
		for (const watch &w : watchers)
			if (w.id == id || w.id == 0)
				w.proc(w.ref, instance, id, scope, element);
	}

	// Make one block. n never exceeds frames, and `at` is where this piece sits in
	// the host's block, so the A/D INPUT slice that belongs with it can be found
	void produce(float *left, float *right, UInt32 n, UInt32 at)
	{
		if (n == 0)
			return;
		if (eng.state() != smu2000::vst3::status::ready) {
			std::memset(left, 0, size_t(n) * sizeof(float));
			std::memset(right, 0, size_t(n) * sizeof(float));
			return;
		}
		// Only a host that asked for the bus (input_bus) has input to hand over,
		// and pull_input leaves the scratch empty until it hands some over
		const float *il = nullptr;
		const float *ir = nullptr;
		if (input_bus && in_l.size() >= size_t(at) + n && in_r.size() >= size_t(at) + n) {
			il = in_l.data() + at;
			ir = in_r.data() + at;
		}
		eng.fill(left, right, int(n), il, ir);
	}

	// Take what has piled up. midi_work is cleared first every time, so a block
	// that could not get the lock never replays the previous block
	void take_midi()
	{
		midi_work.clear();
		std::unique_lock<std::mutex> lock(midi_mutex, std::try_to_lock);
		if (lock.owns_lock() && !midi_in.empty())
			midi_work.swap(midi_in);
		if (midi_work.size() > 1)
			std::sort(midi_work.begin(), midi_work.end(),
			          [](const msg &a, const msg &b) {
				          return a.offset != b.offset ? a.offset < b.offset : a.seq < b.seq;
			          });
	}

	void queue(UInt32 offset, const UInt8 *bytes, size_t n)
	{
		std::lock_guard<std::mutex> lock(midi_mutex);
		if (midi_in.size() >= kMidiReserveMsgs)
			midi_in.erase(midi_in.begin());
		msg m;
		m.offset = offset;
		m.seq = midi_seq++;
		if (n <= sizeof(m.b)) {
			m.n = UInt8(n);
			if (n)
				std::memcpy(m.b, bytes, n);
			// Remember the sounded channel. Stop hushing goes only there
			if (m.n >= 3 && (m.b[0] & 0xf0) == 0x90 && m.b[2])
				sounded.fetch_or(UInt16(1u << (m.b[0] & 15)), std::memory_order_relaxed);
		} else {
			m.n = 0;
			m.sysex.assign(bytes, bytes + n);
		}
		midi_in.push_back(std::move(m));
	}

	// ---- The A/D INPUT bus
	//
	// Not advertised: ElementCount for the input scope is 0 (what lets an
	// out-of-process host render the unit at all -- the full story is on the
	// ElementCount case in prop_get), and the input scope answers nothing, the
	// way Apple's own aumu instruments do not. The machine's A/D input lives on
	// in the VST3/CLAP builds and in the standalone GUI's capture path

	void apply_gain(float *left, float *right, UInt32 n)
	{
		const float target = gain;
		if (gain_now == target && target == 1.0f)
			return;
		const float step = 1.0f / 512.0f;
		for (UInt32 i = 0; i < n; i++) {
			if (gain_now < target) gain_now = std::min(target, gain_now + step);
			else if (gain_now > target) gain_now = std::max(target, gain_now - step);
			left[i] *= gain_now;
			if (right != left)
				right[i] *= gain_now;
		}
	}
};


// ---------------------------------------------------------------------------
// The three entry points

namespace {

OSStatus au_open(void *self, AudioComponentInstance instance)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;

	au->instance = instance;
	au->out_format = default_format();
	au->conn = AudioUnitConnection{};
	au->eng.set_output_rate(smu2000::vst3::NATIVE_RATE);
	{
		char b[256];
		std::snprintf(b, sizeof(b), "開いた: 既定 %g Hz %u ch %s、上限 %u フレーム",
		              au->out_format.mSampleRate, unsigned(au->out_format.mChannelsPerFrame),
		              (au->out_format.mFormatFlags & kAudioFormatFlagIsNonInterleaved) ? "非交錯"
		                                                                         : "交錯",
		              unsigned(au->max_frames));
		au->eng.log_line(b);
	}

	// Reserve up front so the queue never makes the audio thread allocate
	au->midi_in.reserve(kMidiReserveMsgs);
	au->midi_work.reserve(kMidiReserveMsgs);

	// Find and read the ROMs and start booting on another thread. Returns at once
	au->eng.start();
	return noErr;
}

OSStatus au_close(void *self)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	au->eng.set_processing(false);
	char b[320];
	au->host_watch.summary_line(b, sizeof(b));
	au->eng.log_line(b);
	delete au;
	return noErr;
}


// ---------------------------------------------------------------------------
// Making sound

OSStatus render_block(au_instance *au, AudioUnitRenderActionFlags *flags,
                      const AudioTimeStamp *ts, UInt32 frames, AudioBufferList *io);

OSStatus au_render(void *self, AudioUnitRenderActionFlags *flags, const AudioTimeStamp *ts,
                   UInt32 bus, UInt32 frames, AudioBufferList *io)
{
	(void)bus;
	auto *au = static_cast<au_instance *>(self);
	if (!au || !io)
		return kAudio_ParamError;
	if (io->mNumberBuffers == 0)
		return noErr;
	return render_block(au, flags, ts, frames, io);
}

// Let the host watch the render, before and after. Pre-render carries the action
// flags in; post-render is told what actually happened
void tell_notifies(au_instance *au, UInt32 phase, AudioUnitRenderActionFlags *flags,
                   const AudioTimeStamp *ts, UInt32 frames, AudioBufferList *io)
{
	for (const au_instance::notify &n : au->render_notifies) {
		AudioUnitRenderActionFlags f = phase;
		n.proc(n.ref, &f, ts, 0, frames, io);
		if (flags && phase == kAudioUnitRenderAction_PreRender)
			*flags |= f;
	}
}

OSStatus render_block(au_instance *au, AudioUnitRenderActionFlags *flags,
                      const AudioTimeStamp *ts, UInt32 frames, AudioBufferList *io)
{
	const auto t0 = std::chrono::steady_clock::now();
	if (!au->initialized) {
		if (flags)
			*flags |= kAudioUnitRenderAction_OutputIsSilence;
		return kAudioUnitErr_Uninitialized;
	}
	if (frames == 0)
		return noErr;
	// A host is required to respect MaximumFramesPerSlice. Rather than quietly
	// making less, refuse (auval fails a unit that answers noErr here)
	if (frames > au->max_frames) {
		// A host asking for more than it promised is worth writing down: the
		// block is refused and nothing is written, so the host plays whatever
		// the buffer held -- which sounds like garbage rather than an error
		au->note_layout(io);
		au->host_watch.set_last_block(frames, au->max_frames);
		au->host_watch.note_over_max(frames, au->max_frames);
		au->host_watch.note_block(frames, 0);
		char b[320];
		if (au->host_watch.layout_line(b, sizeof(b)))
			au->eng.log_line(b);
		if (au->host_watch.report_line(b, sizeof(b)))
			au->eng.log_line(b);
		return kAudioUnitErr_TooManyFramesToProcess;
	}
	au->note_layout(io);

	// Interleaved output (one buffer holding both channels) is made into two
	// scratch buffers and written back. They cannot hold the whole block, so it
	// goes 256 frames at a time
	const UInt32 ch0 = io->mBuffers[0].mNumberChannels;
	const bool interleaved = (io->mNumberBuffers == 1 && ch0 > 1);
	if (interleaved) {
		tell_notifies(au, kAudioUnitRenderAction_PreRender, flags, ts, frames, io);
		au->take_midi();
		const OSStatus in_rc = au->pull_input(frames, ts);
		if (in_rc != noErr) {
			// The host could not produce its input: hand its error back rather than
			// hiding it, and silence the block so nothing stale is played
			if (io->mBuffers[0].mData)
				std::memset(io->mBuffers[0].mData, 0, size_t(frames) * ch0 * sizeof(float));
			if (flags)
				*flags |= kAudioUnitRenderAction_OutputIsSilence;
			tell_notifies(au, kAudioUnitRenderAction_PostRender, nullptr, ts, frames, io);
			au->watch_end(frames, t0);
			return in_rc;
		}

		float l[256], r[256];
		float lg[256], rg[256];
		auto *dst = static_cast<float *>(io->mBuffers[0].mData);
		UInt32 done = 0;
		for (const msg &m : au->midi_work) {
			au->host_watch.note_midi(m.offset, frames);
			const UInt32 at = std::min<UInt32>(std::max<UInt32>(m.offset, done), frames);
			while (done < at) {
				const UInt32 n = std::min<UInt32>(256, at - done);
				au->produce(l, r, n, done);
				std::memcpy(lg, l, n * sizeof(float));
				std::memcpy(rg, r, n * sizeof(float));
				au->apply_gain(lg, rg, n);
				for (UInt32 i = 0; i < n; i++) {
					dst[(done + i) * ch0 + 0] = lg[i];
					dst[(done + i) * ch0 + 1] = rg[i];
				}
				done += n;
			}
			// MusicDeviceMIDIEvent carries no port number, so the AUv2's one
			// stream always goes to MIDI IN A (port 0). B-D are AUv3-only.
			if (!m.empty())
				au->eng.midi(m.data(), m.size());
		}
		while (done < frames) {
			const UInt32 n = std::min<UInt32>(256, frames - done);
			au->produce(l, r, n, done);
			std::memcpy(lg, l, n * sizeof(float));
			std::memcpy(rg, r, n * sizeof(float));
			au->apply_gain(lg, rg, n);
			for (UInt32 i = 0; i < n; i++) {
				dst[(done + i) * ch0 + 0] = lg[i];
				dst[(done + i) * ch0 + 1] = rg[i];
			}
			done += n;
		}
		if (flags)
			*flags &= ~kAudioUnitRenderAction_OutputIsSilence;
		tell_notifies(au, kAudioUnitRenderAction_PostRender, nullptr, ts, frames, io);
		au->watch_end(frames, t0);
		return noErr;
	}

	auto *left  = static_cast<float *>(io->mBuffers[0].mData);
	auto *right = io->mNumberBuffers >= 2 ? static_cast<float *>(io->mBuffers[1].mData) : left;
	if (!left)
		return noErr;

	tell_notifies(au, kAudioUnitRenderAction_PreRender, flags, ts, frames, io);

	// Take the MIDI the host sent. If the lock is busy it is picked up in the
	// next block instead
	au->take_midi();
	// The host's own input, once per block, before anything is made from it
	const OSStatus in_rc = au->pull_input(frames, ts);
	if (in_rc != noErr) {
		std::memset(left, 0, size_t(frames) * sizeof(float));
		if (right != left)
			std::memset(right, 0, size_t(frames) * sizeof(float));
		if (flags)
			*flags |= kAudioUnitRenderAction_OutputIsSilence;
		tell_notifies(au, kAudioUnitRenderAction_PostRender, nullptr, ts, frames, io);
		au->watch_end(frames, t0);
		return in_rc;
	}
	if (au->midi_work.empty() && au->eng.state() != smu2000::vst3::status::ready) {
		std::memset(left, 0, size_t(frames) * sizeof(float));
		if (right != left)
			std::memset(right, 0, size_t(frames) * sizeof(float));
		if (flags)
			*flags |= kAudioUnitRenderAction_OutputIsSilence;
		tell_notifies(au, kAudioUnitRenderAction_PostRender, nullptr, ts, frames, io);
		au->watch_end(frames, t0);
		return noErr;
	}

	// In time order: make up to each event, inject it, carry on
	UInt32 done = 0;
	for (const msg &m : au->midi_work) {
		au->host_watch.note_midi(m.offset, frames);
		const UInt32 at = std::min<UInt32>(std::max<UInt32>(m.offset, done), frames);		if (at > done) {
			au->produce(left + done, right + done, at - done, done);
			done = at;
		}
		if (!m.empty())
			au->eng.midi(m.data(), m.size());
	}
	if (done < frames)
		au->produce(left + done, right + done, frames - done, done);

	au->apply_gain(left, right, frames);

	if (flags)
		*flags &= ~kAudioUnitRenderAction_OutputIsSilence;

	tell_notifies(au, kAudioUnitRenderAction_PostRender, nullptr, ts, frames, io);
	au->watch_end(frames, t0);
	return noErr;
}


// ---------------------------------------------------------------------------
// Parameters

void param_name(AudioUnitParameterID id, CFStringRef *out)
{
	*out = CFStringCreateWithCString(kCFAllocatorDefault,
	                                 id == kParamGain ? "Output Level" : "Status",
	                                 kCFStringEncodingUTF8);
}

bool param_info(AudioUnitParameterID id, AudioUnitParameterInfo *out)
{
	if (id >= kParamCount)
		return false;
	std::memset(out, 0, sizeof(*out));
	out->flags = kAudioUnitParameterFlag_IsReadable | kAudioUnitParameterFlag_IsWritable |
	             kAudioUnitParameterFlag_HasCFNameString |
	             kAudioUnitParameterFlag_CFNameRelease;
	param_name(id, &out->cfNameString);
	out->unit = kAudioUnitParameterUnit_LinearGain;
	out->minValue = 0.0f;
	out->maxValue = 1.0f;
	out->defaultValue = 1.0f;
	if (id == kParamStatus) {
		out->flags &= ~kAudioUnitParameterFlag_IsWritable;
		out->unit = kAudioUnitParameterUnit_Indexed;
		out->minValue = 0.0f;
		out->maxValue = 2.0f;
		out->defaultValue = 0.0f;
	}
	return true;
}

AudioUnitParameterValue param_get(au_instance *au, AudioUnitParameterID id)
{
	switch (id) {
	case kParamGain:   return au->gain;
	case kParamStatus: return au->eng.state() == smu2000::vst3::status::ready ? 1.0f
	                        : au->eng.state() == smu2000::vst3::status::failed ? 2.0f : 0.0f;
	default: break;
	}
	return 0.0f;
}

void param_set(au_instance *au, AudioUnitParameterID id, AudioUnitParameterValue v)
{
	switch (id) {
	case kParamGain:
		au->gain = std::clamp(v, 0.0f, 1.0f);
		au->eng.panel().set_gain(au->gain);
		au->notify_all(kAudioUnitProperty_ParameterStringFromValue, kAudioUnitScope_Global, id);
		break;
	default:
		break;
	}
}


// ---------------------------------------------------------------------------
// Properties
//
// As much of what AUBase does as is needed. It is written as a table lookup
// because GetPropertyInfo, GetProperty and SetProperty have to reach exactly
// the same verdict about what exists

struct prop_answer
{
	UInt32 size = 0;
	Boolean writable = false;
};

// The gate for a property that answers only in the global scope.
//
// If such a property answered in every scope, a host would read it as if the
// value belonged to that scope -- and auval says so out loud: it checks that
// Latency is *invalid* for Output/Part/Note. Returning "no such property" for
// the wrong scope is not the same answer as returning "wrong scope", and auval
// distinguishes them
// The parameter properties ask with the **parameter number in the element
// field**, so only the scope is checked here. Checking the element as well would
// make every parameter above 0 answer "no such element"
bool want_global_scope(AudioUnitScope scope, OSStatus &err)
{
	if (scope != kAudioUnitScope_Global) {
		err = kAudioUnitErr_InvalidScope;
		return false;
	}
	return true;
}

bool want_global(AudioUnitScope scope, AudioUnitElement element, OSStatus &err)
{
	if (scope != kAudioUnitScope_Global) {
		err = kAudioUnitErr_InvalidScope;
		return false;
	}
	if (element != kGlobalElement) {
		err = kAudioUnitErr_InvalidElement;
		return false;
	}
	return true;
}

OSStatus prop_info(au_instance *au, AudioUnitPropertyID id, AudioUnitScope scope,
                   AudioUnitElement element, prop_answer &out)
{
	OSStatus err = noErr;
	(void)au;

	switch (id) {
	case kAudioUnitProperty_ClassInfo:
	case kAudioUnitProperty_ClassInfoFromDocument:
		if (!want_global(scope, element, err))
			return err;
		out.size = sizeof(CFPropertyListRef);
		out.writable = true;
		return noErr;

	case kAudioUnitProperty_MaximumFramesPerSlice:
	case kAudioUnitProperty_RenderQuality:
		if (!want_global(scope, element, err))
			return err;
		out.size = sizeof(UInt32);
		out.writable = true;
		return noErr;

	case kAudioUnitProperty_Latency:
	case kAudioUnitProperty_TailTime:
		if (!want_global(scope, element, err))
			return err;
		out.size = sizeof(Float64);
		out.writable = false;
		return noErr;

	case kAudioUnitProperty_PresentPreset:
		if (!want_global(scope, element, err))
			return err;
		out.size = sizeof(AUPreset);
		out.writable = true;
		return noErr;

	// The element count is asked of every scope. Global is 1 and the audio output
	// is 1, as MusicDeviceBase answers; the input scope answers 0 until a host
	// asks for the A/D INPUT bus, and that is the one number a host writes to
	// (see the note on input_bus for what a counted bus does to an out-of-process
	// host). Writable on the input scope only, because that is the only scope a
	// host may change the count of
	case kAudioUnitProperty_ElementCount:
		if (element != kGlobalElement)
			return kAudioUnitErr_InvalidElement;
		if (scope > kAudioUnitScope_LayerItem)
			return kAudioUnitErr_InvalidScope;
		out.size = sizeof(UInt32);
		out.writable = (scope == kAudioUnitScope_Input);
		return noErr;

	// The main input bus stays configurable while it is not counted -- the way
	// a JUCE plug-in's "disabled" bus still accepts a stereo layout, so a host
	// can wire or format it before or while asking for the count (auval reads
	// SupportedNumChannels {2, 2} and configures the input right away). Setting
	// a callback or a connection is a no-op until the host asks for the bus
	// (ElementCount 1), which is when the stored one starts being pulled
	case kAudioUnitProperty_SetRenderCallback:
	case kAudioUnitProperty_MakeConnection:
		if (scope != kAudioUnitScope_Input || element != 0)
			return kAudioUnitErr_InvalidProperty;
		out.size = (id == kAudioUnitProperty_MakeConnection)
		               ? sizeof(AudioUnitConnection) : sizeof(AURenderCallbackStruct);
		out.writable = true;
		return noErr;

	case kAudioUnitProperty_ParameterList:
		if (!want_global(scope, element, err))
			return err;
		out.size = kParamCount * sizeof(AudioUnitParameterID);
		out.writable = false;
		return noErr;

	case kAudioUnitProperty_ParameterInfo:
		if (!want_global_scope(scope, err))
			return err;
		if (element >= kParamCount)
			return kAudioUnitErr_InvalidParameter;
		out.size = sizeof(AudioUnitParameterInfo);
		out.writable = false;
		return noErr;

	case kAudioUnitProperty_ParameterStringFromValue:
		if (!want_global_scope(scope, err))
			return err;
		if (element >= kParamCount)
			return kAudioUnitErr_InvalidParameter;
		out.size = sizeof(CFStringRef);
		out.writable = false;
		return noErr;

	case kAudioUnitProperty_ParameterValueFromString:
		if (!want_global_scope(scope, err))
			return err;
		if (element >= kParamCount)
			return kAudioUnitErr_InvalidParameter;
		out.size = sizeof(AudioUnitParameterValue);
		out.writable = true;
		return noErr;

	// One audio output. The global scope answers the same thing, as
	// DLSMusicDevice does. **There is no stream format for the MIDI input.**
	// Answering with a "MIDI stream" here makes auval treat it as the input
	// format and fail the unit for being initialisable at 3 channels.
	//
	// The audio input scope answers always, element 0 only: the main input bus
	// stays configurable while it is not counted, the way a JUCE plug-in's
	// "disabled" bus still accepts a stereo layout. (The harness that could not
	// survive that pairing, auval -real-time-safety, no longer runs on modern
	// macOS; plain auval *requires* the scope to answer, because
	// SupportedNumChannels says {2, 2} and it configures the input right away.)
	case kAudioUnitProperty_StreamFormat:
		if (scope == kAudioUnitScope_Input) {
			if (element != 0)
				return kAudioUnitErr_InvalidElement;
			out.size = sizeof(AudioStreamBasicDescription);
			out.writable = true;
			return noErr;
		}
		if (scope != kAudioUnitScope_Output && scope != kAudioUnitScope_Global)
			return kAudioUnitErr_InvalidScope;
		if (element != kOutputElement)
			return kAudioUnitErr_InvalidElement;
		out.size = sizeof(AudioStreamBasicDescription);
		out.writable = true;
		return noErr;

	// Legacy sample-rate access. Unlike StreamFormat it carries no bus
	// configuration semantics, so answering on the input scope is safe
	// (the StreamFormat case must keep refusing there). Both scopes share
	// the one rate: the A/D INPUT is resampled at the output rate by
	// construction, so setting either one sets the unit
	case kAudioUnitProperty_SampleRate:
		if (scope != kAudioUnitScope_Output && scope != kAudioUnitScope_Global &&
		    scope != kAudioUnitScope_Input)
			return kAudioUnitErr_InvalidScope;
		if (element != kOutputElement)
			return kAudioUnitErr_InvalidElement;
		out.size = sizeof(Float64);
		out.writable = true;
		return noErr;

	// 0 in and 2 out, as Apple's own aumu instruments answer (AUMIDISynth:
	// [0, -16]).
	//
	// The first number is **required** channels, and it has to agree with what
	// the scopes answer: the input bus is not counted (ElementCount for the
	// input scope is 0 -- the full story is on the ElementCount case in
	// prop_get) and the input scope answers nothing, so "2 in" here would have
	// a host configure an input bus the unit says it does not have
	case kAudioUnitProperty_SupportedNumChannels:
		if (!want_global(scope, element, err))
			return err;
		out.size = sizeof(AUChannelInfo);
		out.writable = false;
		return noErr;

	case kMusicDeviceProperty_InstrumentName:
		if (!want_global(scope, element, err))
			return err;
		out.size = sizeof(CFStringRef);
		out.writable = false;
		return noErr;

	// What the host reads to find the editor: a bundle and a class name in it.
	// One view class, so the size is one AudioUnitCocoaViewInfo
	case kAudioUnitProperty_CocoaUI:
		if (!want_global(scope, element, err))
			return err;
		out.size = sizeof(AudioUnitCocoaViewInfo);
		out.writable = false;
		return noErr;

	// Private, and read-only: the editor's way of reaching the engine it has to
	// draw (editor.h). Not something a host has any use for
	case smu2000::au::kEngineProperty:
		if (!want_global(scope, element, err))
			return err;
		out.size = sizeof(void *);
		out.writable = false;
		return noErr;

	default:
		break;
	}
	return kAudioUnitErr_InvalidProperty;
}

OSStatus prop_get(au_instance *au, AudioUnitPropertyID id, AudioUnitScope scope,
                  AudioUnitElement element, void *data, UInt32 *size)
{
	if (!data || !size)
		return kAudio_ParamError;

	switch (id) {
	case kAudioUnitProperty_ClassInfo:
	case kAudioUnitProperty_ClassInfoFromDocument: {
		if (*size < sizeof(CFPropertyListRef))
			return kAudioUnitErr_InvalidPropertyValue;
		// The whole machine, packed. state_pack is the same one the VST3 side uses,
		// so it fits in the same 300KB range (raw would be 6MB)
		const std::vector<u8> packed = state_pack(au->eng.save_state());
		CFDataRef d = CFDataCreate(kCFAllocatorDefault, packed.data(), CFIndex(packed.size()));
		CFMutableDictionaryRef dict = CFDictionaryCreateMutable(
			kCFAllocatorDefault, 8, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);

		// A host (and auval) reads these four as the unit's identity. Without them
		// the answer is "Class Data does not have required field: <type> ==
		// componentType". The value type is CFNumber, matching Apple's own AUs
		auto put_num = [&](const char *key, SInt32 v) {
			CFNumberRef n = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &v);
			CFDictionarySetValue(dict, CFStringCreateWithCString(kCFAllocatorDefault, key,
			                                                    kCFStringEncodingUTF8), n);
			CFRelease(n);
		};
		put_num(kAUPresetTypeKey, SInt32(kType));
		put_num(kAUPresetSubtypeKey, SInt32(kSubtype));
		put_num(kAUPresetManufacturerKey, SInt32(kManufacturer));
		put_num(kAUPresetVersionKey, SInt32(kVersion));
		CFStringRef nm = CFStringCreateWithCString(kCFAllocatorDefault, au->preset_name.c_str(),
		                                           kCFStringEncodingUTF8);
		CFDictionarySetValue(dict, CFSTR(kAUPresetNameKey), nm);
		CFRelease(nm);

		// And this AU's own contents
		CFDictionarySetValue(dict, CFSTR("S-MU2000"), d);
		CFNumberRef g = CFNumberCreate(kCFAllocatorDefault, kCFNumberFloat32Type, &au->gain);
		CFDictionarySetValue(dict, CFSTR("S-MU2000-OutputLevel"), g);
		CFRelease(g);
		CFRelease(d);

		// The SmartMedia in the slot, by file name. The image itself is not put in
		// the preset (16 to 128 MB), the same as the VST3 side: what is saved is
		// which file was in the machine. Any blocks the machine wrote are flushed
		// to it first, so the project and the file agree
		au->eng.card_flush();
		const std::string card = au->eng.card_path();
		if (!card.empty()) {
			CFStringRef c = CFStringCreateWithCString(kCFAllocatorDefault, card.c_str(),
			                                          kCFStringEncodingUTF8);
			CFDictionarySetValue(dict, CFSTR("S-MU2000-SmartMedia"), c);
			CFRelease(c);
		}
		*static_cast<CFPropertyListRef *>(data) = dict;
		*size = sizeof(CFPropertyListRef);
		return noErr;
	}

	case kAudioUnitProperty_MaximumFramesPerSlice:
		if (*size < sizeof(UInt32))
			return kAudioUnitErr_InvalidPropertyValue;
		*static_cast<UInt32 *>(data) = au->max_frames;
		*size = sizeof(UInt32);
		return noErr;

	case kAudioUnitProperty_RenderQuality:
		if (*size < sizeof(UInt32))
			return kAudioUnitErr_InvalidPropertyValue;
		*static_cast<UInt32 *>(data) = au->render_quality;
		*size = sizeof(UInt32);
		return noErr;

	case kAudioUnitProperty_Latency: {
		if (*size < sizeof(Float64))
			return kAudioUnitErr_InvalidPropertyValue;
		const double rate = au->out_format.mSampleRate > 0.0 ? au->out_format.mSampleRate
		                                                    : smu2000::vst3::NATIVE_RATE;
		*static_cast<Float64 *>(data) = double(au->eng.latency_samples()) / rate;
		*size = sizeof(Float64);
		return noErr;
	}

	// How long the machine keeps sounding after the last note. There is a reverb
	// on it, so a host that stops rendering the moment the MIDI stops would cut
	// the tail off: the VST3 side answers the same four seconds
	// (getTailSamples). Seconds, not samples -- that is what this property is in
	case kAudioUnitProperty_TailTime:
		if (*size < sizeof(Float64))
			return kAudioUnitErr_InvalidPropertyValue;
		*static_cast<Float64 *>(data) = 4.0;
		*size = sizeof(Float64);
		return noErr;

	// Where the editor is. Both references are made fresh here and belong to the
	// host afterwards; the view itself is built by editor_mac.mm
	case kAudioUnitProperty_CocoaUI: {
		if (*size < sizeof(AudioUnitCocoaViewInfo))
			return kAudioUnitErr_InvalidPropertyValue;
		CFURLRef url = nullptr;
		CFStringRef name = nullptr;
		if (!smu2000::au::view_info(&url, &name))
			return kAudioUnitErr_InvalidPropertyValue;
		auto *info = static_cast<AudioUnitCocoaViewInfo *>(data);
		info->mCocoaAUViewBundleLocation = url;
		info->mCocoaAUViewClass[0] = name;
		*size = sizeof(AudioUnitCocoaViewInfo);
		return noErr;
	}

	// Private: the engine this instance is running, for the editor
	case smu2000::au::kEngineProperty:
		if (*size < sizeof(void *))
			return kAudioUnitErr_InvalidPropertyValue;
		*static_cast<void **>(data) = &au->eng;
		*size = sizeof(void *);
		return noErr;

	case kAudioUnitProperty_ElementCount:
		// aumu's rule: Global is 1 and there is one audio output bus, as
		// MusicDeviceBase answers. The input scope is 0 **until the host asks for
		// the A/D INPUT bus** by writing 1 here -- and that number is answered as
		// the field says, so a host that asked for the bus is the only one that
		// sees it. Counting it by default is what makes a sandboxed host (every
		// out-of-process one, GarageBand among them) render nothing but
		// kAudioUnitErr_NoConnection, because an instrument is never handed an
		// input connection: the promise of a bus is one GarageBand cannot keep.
		// The A/D INPUT is unchanged for VST3, CLAP and the standalone GUI, which
		// have no such layer between them and the host
		if (*size < sizeof(UInt32))
			return kAudioUnitErr_InvalidPropertyValue;
		*static_cast<UInt32 *>(data) =
		    (scope == kAudioUnitScope_Global || scope == kAudioUnitScope_Output)
		        ? 1u
		        : (scope == kAudioUnitScope_Input && au->input_bus) ? 1u : 0u;
		*size = sizeof(UInt32);
		return noErr;

	case kAudioUnitProperty_PresentPreset: {
		if (*size < sizeof(AUPreset))
			return kAudioUnitErr_InvalidPropertyValue;
		// Only the one factory preset exists. A negative number marks it as not
		// coming from a bank, which is what DLSMusicDevice reports too
		auto *p = static_cast<AUPreset *>(data);
		p->presetNumber = -1;
		p->presetName = CFStringCreateWithCString(kCFAllocatorDefault, au->preset_name.c_str(),
		                                          kCFStringEncodingUTF8);
		*size = sizeof(AUPreset);
		return noErr;
	}

	case kAudioUnitProperty_ParameterList: {
		if (*size < kParamCount * sizeof(AudioUnitParameterID))
			return kAudioUnitErr_InvalidPropertyValue;
		auto *out = static_cast<AudioUnitParameterID *>(data);
		for (AudioUnitParameterID i = 0; i < kParamCount; i++)
			out[i] = i;
		*size = kParamCount * sizeof(AudioUnitParameterID);
		return noErr;
	}

	case kAudioUnitProperty_ParameterInfo: {
		if (*size < sizeof(AudioUnitParameterInfo))
			return kAudioUnitErr_InvalidPropertyValue;
		if (!param_info(element, static_cast<AudioUnitParameterInfo *>(data)))
			return kAudioUnitErr_InvalidParameter;
		*size = sizeof(AudioUnitParameterInfo);
		return noErr;
	}

	case kAudioUnitProperty_ParameterStringFromValue: {
		if (*size < sizeof(CFStringRef))
			return kAudioUnitErr_InvalidPropertyValue;
		if (element >= kParamCount)
			return kAudioUnitErr_InvalidParameter;
		const AudioUnitParameterValue v = param_get(au, element);
		CFStringRef s = nullptr;
		if (element == kParamGain) {
			char buf[32];
			std::snprintf(buf, sizeof(buf), "%.3f", double(v));
			s = CFStringCreateWithCString(kCFAllocatorDefault, buf, kCFStringEncodingUTF8);
		} else {
			const char *t = v == 1.0f ? "ready" : v == 2.0f ? "failed" : "loading";
			s = CFStringCreateWithCString(kCFAllocatorDefault, t, kCFStringEncodingUTF8);
		}
		*static_cast<CFStringRef *>(data) = s;
		*size = sizeof(CFStringRef);
		return noErr;
	}

	case kAudioUnitProperty_StreamFormat: {
		if (*size < sizeof(AudioStreamBasicDescription))
			return kAudioUnitErr_InvalidPropertyValue;
		// The A/D INPUT bus answers the output's format: the engine resamples the
		// input at the rate the host renders at, so the two are the same format by
		// construction. The bus is configurable while it is not counted (see the
		// SetRenderCallback case in prop_info), so the scope answers always
		if (scope == kAudioUnitScope_Input) {
			*static_cast<AudioStreamBasicDescription *>(data) = au->out_format;
			*size = sizeof(AudioStreamBasicDescription);
			return noErr;
		}
		if (scope != kAudioUnitScope_Output && scope != kAudioUnitScope_Global)
			return kAudioUnitErr_InvalidScope;
		if (element != kOutputElement)
			return kAudioUnitErr_InvalidElement;
		*static_cast<AudioStreamBasicDescription *>(data) = au->out_format;
		*size = sizeof(AudioStreamBasicDescription);
		return noErr;
	}

	// What the host set, so it can read it back before pulling the input
	case kAudioUnitProperty_SetRenderCallback:
	case kAudioUnitProperty_MakeConnection:
		if (scope != kAudioUnitScope_Input || element != 0)
			return kAudioUnitErr_InvalidProperty;
		if (id == kAudioUnitProperty_MakeConnection) {
			if (*size < sizeof(AudioUnitConnection))
				return kAudioUnitErr_InvalidPropertyValue;
			*static_cast<AudioUnitConnection *>(data) = au->conn;
			*size = sizeof(AudioUnitConnection);
		} else {
			if (*size < sizeof(AURenderCallbackStruct))
				return kAudioUnitErr_InvalidPropertyValue;
			*static_cast<AURenderCallbackStruct *>(data) = au->in_cb;
			*size = sizeof(AURenderCallbackStruct);
		}
		return noErr;

	case kAudioUnitProperty_SampleRate: {
		if (*size < sizeof(Float64))
			return kAudioUnitErr_InvalidPropertyValue;
		if (scope != kAudioUnitScope_Output && scope != kAudioUnitScope_Global &&
		    scope != kAudioUnitScope_Input)
			return kAudioUnitErr_InvalidScope;
		if (element != kOutputElement)
			return kAudioUnitErr_InvalidElement;
		*static_cast<Float64 *>(data) = au->out_format.mSampleRate;
		*size = sizeof(Float64);
		return noErr;
	}

	case kAudioUnitProperty_SupportedNumChannels: {
		if (*size < sizeof(AUChannelInfo))
			return kAudioUnitErr_InvalidPropertyValue;
		// {2, 2} always: the machine really does take audio in (the MU2000's
		// A/D INPUT runs through its filters), and per AUComponent.h the entry
		// declares a *supported* configuration, not a connection. The main
		// input bus stays configurable while it is not counted -- format,
		// callback and connection all answer at element 0 -- so a host that
		// takes the matrix at its word can configure the input right away,
		// the way a JUCE plug-in's "disabled" bus still accepts a stereo
		// layout. What is NOT done by default is count the bus (ElementCount
		// stays 0 until the host asks), so no host ends up rendering through
		// an input element nobody connected
		auto *out = static_cast<AUChannelInfo *>(data);
		out[0] = AUChannelInfo{ 2, 2 };
		*size = sizeof(AUChannelInfo);
		return noErr;
	}

	case kMusicDeviceProperty_InstrumentName: {
		if (*size < sizeof(CFStringRef))
			return kAudioUnitErr_InvalidPropertyValue;
		*static_cast<CFStringRef *>(data) =
			CFStringCreateWithCString(kCFAllocatorDefault, "S-MU2000 (MU2000 emulator)",
			                          kCFStringEncodingUTF8);
		*size = sizeof(CFStringRef);
		return noErr;
	}

	default:
		break;
	}
	return kAudioUnitErr_InvalidProperty;
}

OSStatus prop_set(au_instance *au, AudioUnitPropertyID id, AudioUnitScope scope,
                  AudioUnitElement element, const void *data, UInt32 size)
{
	if (!data)
		return kAudio_ParamError;

	switch (id) {
	case kAudioUnitProperty_ClassInfo:
	case kAudioUnitProperty_ClassInfoFromDocument: {
		if (size < sizeof(CFPropertyListRef))
			return kAudioUnitErr_InvalidPropertyValue;
		CFPropertyListRef plist = *static_cast<CFPropertyListRef const *>(data);
		CFDataRef blob = nullptr;
		CFDictionaryRef dict = nullptr;
		if (plist && CFGetTypeID(plist) == CFDictionaryGetTypeID()) {
			dict = static_cast<CFDictionaryRef>(plist);
			blob = static_cast<CFDataRef>(const_cast<void *>(
			    CFDictionaryGetValue(dict, CFSTR("S-MU2000"))));
		} else if (plist && CFGetTypeID(plist) == CFDataGetTypeID()) {
			blob = static_cast<CFDataRef>(plist);
		}
		if (!blob)
			return kAudioUnitErr_InvalidPropertyValue;

		if (dict) {
			CFNumberRef g = static_cast<CFNumberRef>(const_cast<void *>(
			    CFDictionaryGetValue(dict, CFSTR("S-MU2000-OutputLevel"))));
			if (g && CFGetTypeID(g) == CFNumberGetTypeID()) {
				float v = 1.0f;
				CFNumberGetValue(g, kCFNumberFloat32Type, &v);
				param_set(au, kParamGain, v);
			}
			CFStringRef nm = static_cast<CFStringRef>(const_cast<void *>(
			    CFDictionaryGetValue(dict, CFSTR(kAUPresetNameKey))));
			if (nm && CFGetTypeID(nm) == CFStringGetTypeID()) {
				char b[256] = {};
				if (CFStringGetCString(nm, b, sizeof(b), kCFStringEncodingUTF8) && *b)
					au->preset_name = b;
			}
		}

		std::vector<u8> raw;
		if (!state_unpack(CFDataGetBytePtr(blob), size_t(CFDataGetLength(blob)), raw))
			return kAudioUnitErr_InvalidPropertyValue;

		// Nothing to wait for. A host sets this straight after
		// AudioComponentInstanceNew, while the ROMs are still coming up, and
		// engine::load_state() keeps a restore that arrives that early and lets the
		// machine apply it once it is up. Blocking here instead would hold the
		// host's thread for as long as a cold boot takes
		au->eng.load_state(raw.data(), raw.size());

		// Put the card back in the slot, if there was one and the file is still
		// where it was. A preset with no card ejects whatever was there
		if (dict) {
			CFStringRef c = static_cast<CFStringRef>(const_cast<void *>(
			    CFDictionaryGetValue(dict, CFSTR("S-MU2000-SmartMedia"))));
			if (c && CFGetTypeID(c) == CFStringGetTypeID()) {
				char path[4096] = {};
				if (CFStringGetCString(c, path, sizeof(path), kCFStringEncodingUTF8)) {
					std::string err;
					if (!au->eng.card_insert(path, err))
						au->eng.log_line(("SmartMedia を差せない: " + err).c_str());
				}
			} else if (!au->eng.card_path().empty()) {
				au->eng.card_eject();
			}
		}
		return noErr;
	}	case kAudioUnitProperty_MaximumFramesPerSlice:
		if (size < sizeof(UInt32))
			return kAudioUnitErr_InvalidPropertyValue;
		{
			const UInt32 was = au->max_frames;
	au->max_frames = *static_cast<const UInt32 *>(data);
			if (au->max_frames != was) {
				// The block size a host promises to stay under. It is the number
				// this AU refuses past, so a value smaller than the blocks the
				// host then asks for is exactly the kind of thing worth seeing
				char b[128];
				std::snprintf(b, sizeof(b), "1 回の上限を %u フレームにされた（前は %u）",
				              unsigned(au->max_frames), unsigned(was));
				au->eng.log_line(b);
			}
		}
	au->notify_all(kAudioUnitProperty_MaximumFramesPerSlice, scope, element);
		return noErr;

	case kAudioUnitProperty_RenderQuality:
		if (size < sizeof(UInt32))
			return kAudioUnitErr_InvalidPropertyValue;
		au->render_quality = *static_cast<const UInt32 *>(data);
		return noErr;

	case kAudioUnitProperty_PresentPreset:
		// There is only the one factory preset, so choosing one just puts the
		// defaults back. The name sticks: auval round-trips it and warns when
		// a set name does not come back
		if (size < sizeof(AUPreset))
			return kAudioUnitErr_InvalidPropertyValue;
		{
			const auto *p = static_cast<const AUPreset *>(data);
			if (p->presetName) {
				char b[256] = {};
				if (CFStringGetCString(p->presetName, b, sizeof(b), kCFStringEncodingUTF8) && *b)
					au->preset_name = b;
			}
		}
		param_set(au, kParamGain, 1.0f);
		au->hush();
		return noErr;

	case kAudioUnitProperty_ParameterValueFromString: {
		if (element >= kParamCount || size < sizeof(CFStringRef))
			return kAudioUnitErr_InvalidParameter;
		const CFStringRef s = *static_cast<const CFStringRef *>(data);
		const double v = s ? CFStringGetDoubleValue(s) : 0.0;
		param_set(au, element, AudioUnitParameterValue(v));
		return noErr;
	}

	// The host's input procedure, taken only while it has the A/D INPUT bus
	// switched on (see the same case in prop_info). Both routes -- the property
	// and the connection a graph arrives as -- carry the same struct, so either
	// one lands in the same place
	case kAudioUnitProperty_SetRenderCallback:
	case kAudioUnitProperty_MakeConnection:
		if (scope != kAudioUnitScope_Input || element != 0)
			return kAudioUnitErr_InvalidProperty;
		if (id == kAudioUnitProperty_MakeConnection) {
			// A graph connection, stored as such: its struct is not a callback
			// pair, and calling one as the other would jump into the source unit
			// pointer. There is no side-chain-from-another-AU path in the engine,
			// so a connection is recorded but not pulled
			if (size < sizeof(AudioUnitConnection))
				return kAudioUnitErr_InvalidPropertyValue;
			au->conn = *static_cast<const AudioUnitConnection *>(data);
			return noErr;
		}
		if (size < sizeof(AURenderCallbackStruct))
			return kAudioUnitErr_InvalidPropertyValue;
		au->in_cb = *static_cast<const AURenderCallbackStruct *>(data);
		au->in_cb_set = (au->in_cb.inputProc != nullptr);
		return noErr;

	// The host asking for the A/D INPUT bus: 1 adds it (a side-chain input, the
	// way Logic's side-chain menu does), 0 takes it away. Everything about the
	// bus -- the count, its format, its callback, the engine's input side --
	// follows this one number, and a host that never writes it is offered no
	// input at all (the note on input_bus says why that is the only safe
	// default for an instrument)
	case kAudioUnitProperty_ElementCount: {
		if (scope != kAudioUnitScope_Input)
			return kAudioUnitErr_InvalidScope;
		if (element != 0)
			return kAudioUnitErr_InvalidElement;
		if (size < sizeof(UInt32))
			return kAudioUnitErr_InvalidPropertyValue;
		const UInt32 want = *static_cast<const UInt32 *>(data);
		if (want > 1)
			return kAudioUnitErr_InvalidPropertyValue;
		if ((want != 0) != au->input_bus) {
			au->input_bus = (want != 0);
			if (!au->input_bus) {
				// Taken away: neither route into the scope is kept for a bus
				// that is not there
				au->in_cb = AURenderCallbackStruct{};
				au->in_cb_set = false;
				au->conn = AudioUnitConnection{};
			}
			au->notify_all(id, scope, element);
		}
		return noErr;
	}

	case kAudioUnitProperty_StreamFormat: {
		if (size < sizeof(AudioStreamBasicDescription))
			return kAudioUnitErr_InvalidPropertyValue;
		const auto *f = static_cast<const AudioStreamBasicDescription *>(data);

		// The A/D INPUT bus carries the same format as the output (the engine
		// resamples the input at the host's rate), so it is accepted and checked
		// the same way -- but the output is what the unit renders at, so only that
		// one sets the engine's rate. prop_info lets an input-scope set through
		// only while the host has the bus switched on
		const bool input_scope = (scope == kAudioUnitScope_Input);
		if (input_scope) {
			if (element != 0)
				return kAudioUnitErr_InvalidElement;
		} else {
			if (scope != kAudioUnitScope_Output && scope != kAudioUnitScope_Global)
				return kAudioUnitErr_InvalidScope;
			if (element != kOutputElement)
				return kAudioUnitErr_InvalidElement;
		}

		// Double precision is refused: the engine is single precision throughout, so
		// accepting it and pretending would corrupt the output. Exactly 2 channels
		// keeps this consistent with SupportedNumChannels -- letting 1 through makes
		// the unit initialisable at a format it never advertised, which auval warns
		// about
		if (f->mFormatID != kAudioFormatLinearPCM ||
		    (f->mFormatFlags & kAudioFormatFlagIsFloat) == 0 ||
		    f->mBitsPerChannel != 32 || f->mChannelsPerFrame != 2)
			return kAudioUnitErr_FormatNotSupported;

		if (input_scope) {
			// Any rate is accepted: the input is resampled into the machine by
			// engine::fill, and the engine's own rate comes from the output
			// alone. auval configures the promised stereo input at rates the
			// output is not at, so refusing a rate here fails the unit
			au->notify_all(kAudioUnitProperty_StreamFormat, scope, element);
			return noErr;
		}

	au->out_format = *f;
		au->eng.set_output_rate(f->mSampleRate);
		au->host_watch.set_rate(f->mSampleRate);
		{
			char b[256];
			std::snprintf(b, sizeof(b),
			              "描き出しの形式: %g Hz %u ch %u bit、flags 0x%x（%s）、1 パケット %u フレーム"
			              "（scope %u / element %u）",
			              f->mSampleRate, unsigned(f->mChannelsPerFrame), unsigned(f->mBitsPerChannel),
			              unsigned(f->mFormatFlags),
			              (f->mFormatFlags & kAudioFormatFlagIsNonInterleaved) ? "非交錯" : "交錯",
			              unsigned(f->mFramesPerPacket), unsigned(scope), unsigned(element));
			au->eng.log_line(b);
		}
		au->notify_all(kAudioUnitProperty_StreamFormat, scope, element);
		return noErr;
	}

	case kAudioUnitProperty_SampleRate: {
		if (size < sizeof(Float64))
			return kAudioUnitErr_InvalidPropertyValue;
		if (scope != kAudioUnitScope_Output && scope != kAudioUnitScope_Global &&
		    scope != kAudioUnitScope_Input)
			return kAudioUnitErr_InvalidScope;
		if (element != kOutputElement)
			return kAudioUnitErr_InvalidElement;
		const Float64 rate = *static_cast<const Float64 *>(data);
		if (!(rate > 0.0))
			return kAudioUnitErr_InvalidPropertyValue;
		au->out_format.mSampleRate = rate;
		au->eng.set_output_rate(rate);
		au->host_watch.set_rate(rate);
		{
			char b[128];
			std::snprintf(b, sizeof(b), "標本化周波数を %g Hz にされた（scope %u / element %u）",
			              rate, unsigned(scope), unsigned(element));
			au->eng.log_line(b);
		}
		au->notify_all(kAudioUnitProperty_SampleRate, scope, element);
		au->notify_all(kAudioUnitProperty_StreamFormat, scope, element);
		return noErr;
	}

	default:
		break;
	}
	return kAudioUnitErr_InvalidProperty;
}


// ---------------------------------------------------------------------------
// The functions Lookup hands out

OSStatus au_initialize(void *self)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	au->initialized = true;
	{
		char b[256];
		std::snprintf(b, sizeof(b),
		              "初期化: %g Hz %u ch %s、上限 %u フレーム、入力の口 0 / 出力の口 1",
		              au->out_format.mSampleRate, unsigned(au->out_format.mChannelsPerFrame),
		              (au->out_format.mFormatFlags & kAudioFormatFlagIsNonInterleaved) ? "非交錯"
		                                                                         : "交錯",
		              unsigned(au->max_frames));
		au->eng.log_line(b);
	}
	// Wait out the boot here. Initialize runs on the main thread where time
	// may be taken, and without waiting the host starts playing into a
	// booting unit: the MIDI piles up and comes out as one lump, collapsing
	// the head of the song (same fix as upstream issue #19 for VST3/CLAP)
	if (!au->eng.wait_ready(30000))
		au->eng.log_line("起動が終わらないまま演奏に入る");
	return noErr;
}

OSStatus au_uninitialize(void *self)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	au->initialized = false;
	au->eng.set_processing(false);
	return noErr;
}

OSStatus au_get_property_info(void *self, AudioUnitPropertyID id, AudioUnitScope scope,
                              AudioUnitElement element, UInt32 *size, Boolean *writable)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au || !size)
		return kAudio_ParamError;
	prop_answer a;
	const OSStatus st = prop_info(au, id, scope, element, a);
	if (st != noErr)
		return st;
	*size = a.size;
	if (writable)
		*writable = a.writable;
	return noErr;
}

OSStatus au_get_property(void *self, AudioUnitPropertyID id, AudioUnitScope scope,
                         AudioUnitElement element, void *data, UInt32 *size)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	prop_answer a;
	const OSStatus st = prop_info(au, id, scope, element, a);
	if (st != noErr)
		return st;
	return prop_get(au, id, scope, element, data, size);
}

OSStatus au_set_property(void *self, AudioUnitPropertyID id, AudioUnitScope scope,
                         AudioUnitElement element, const void *data, UInt32 size)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	prop_answer a;
	const OSStatus st = prop_info(au, id, scope, element, a);
	if (st != noErr)
		return st;
	if (!a.writable)
		return kAudioUnitErr_PropertyNotWritable;
	return prop_set(au, id, scope, element, data, size);
}

OSStatus au_add_property_listener(void *self, AudioUnitPropertyID id,
                                  AudioUnitPropertyListenerProc proc, void *ref)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au || !proc)
		return kAudio_ParamError;
	au->watchers.push_back({ id, proc, ref });
	return noErr;
}

// Two of these exist because the AU API grew a second one. This is the newer
// form, which can tell listeners of the same procedure apart; the older form
// removes every listener using that procedure
OSStatus au_remove_property_listener_ud(void *self, AudioUnitPropertyID id,
                                        AudioUnitPropertyListenerProc proc, void *ref)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au || !proc)
		return kAudio_ParamError;
	auto &w = au->watchers;
	w.erase(std::remove_if(w.begin(), w.end(), [&](const au_instance::watch &x) {
		return x.proc == proc && x.id == id && x.ref == ref;
	}), w.end());
	return noErr;
}

OSStatus au_remove_property_listener(void *self, AudioUnitPropertyID id,
                                     AudioUnitPropertyListenerProc proc)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au || !proc)
		return kAudio_ParamError;
	auto &w = au->watchers;
	w.erase(std::remove_if(w.begin(), w.end(), [&](const au_instance::watch &x) {
		return x.proc == proc && x.id == id;
	}), w.end());
	return noErr;
}

OSStatus au_add_render_notify(void *self, AURenderCallback proc, void *ref)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au || !proc)
		return kAudio_ParamError;
	au->render_notifies.push_back({ proc, ref });
	return noErr;
}

OSStatus au_remove_render_notify(void *self, AURenderCallback proc, void *ref)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	auto &v = au->render_notifies;
	v.erase(std::remove_if(v.begin(), v.end(),
	                       [&](const au_instance::notify &x) { return x.proc == proc && x.ref == ref; }),
	        v.end());
	return noErr;
}

// Parameters live in the global scope only. Both a host and auval set each
// parameter once per scope and read it back, and a unit that answers in every
// scope makes those copies look like they disagree with one another
OSStatus param_where(AudioUnitScope scope, AudioUnitElement element)
{
	if (scope != kAudioUnitScope_Global)
		return kAudioUnitErr_InvalidScope;
	if (element != kGlobalElement)
		return kAudioUnitErr_InvalidElement;
	return noErr;
}

OSStatus au_get_parameter(void *self, AudioUnitParameterID id, AudioUnitScope scope,
                          AudioUnitElement element, AudioUnitParameterValue *value)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au || !value)
		return kAudio_ParamError;
	const OSStatus st = param_where(scope, element);
	if (st != noErr)
		return st;
	if (id >= kParamCount)
		return kAudioUnitErr_InvalidParameter;
	*value = param_get(au, id);
	return noErr;
}

OSStatus au_set_parameter(void *self, AudioUnitParameterID id, AudioUnitScope scope,
                          AudioUnitElement element, AudioUnitParameterValue value, UInt32 offset)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	(void)offset;
	const OSStatus st = param_where(scope, element);
	if (st != noErr)
		return st;
	if (id >= kParamCount)
		return kAudioUnitErr_InvalidParameter;
	param_set(au, id, value);
	return noErr;
}

OSStatus au_schedule_parameters(void *self, const AudioUnitParameterEvent *events, UInt32 count)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	for (UInt32 i = 0; i < count; i++) {
		const AudioUnitParameterEvent &e = events[i];
		if (param_where(e.scope, e.element) != noErr || e.parameter >= kParamCount)
			continue;
		// A ramp is answered with its start value. This machine's output level is a
		// single multiply, so stepping it would not be audible anyway
		const AudioUnitParameterValue v = e.eventType == kParameterEvent_Ramped
		    ? e.eventValues.ramp.startValue : e.eventValues.immediate.value;
		param_set(au, e.parameter, v);
	}
	return noErr;
}

OSStatus au_reset(void *self, AudioUnitScope scope, AudioUnitElement element)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	(void)scope;
	(void)element;
	au->hush();
	return noErr;
}

OSStatus au_midi_event(void *self, UInt32 status, UInt32 d1, UInt32 d2, UInt32 offset)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	const UInt8 cmd = UInt8(status & 0xf0);
	UInt8 b[3] = { UInt8(status & 0xff), UInt8(d1 & 0x7f), UInt8(d2 & 0x7f) };
	size_t n = 3;
	if (cmd == 0xc0 || cmd == 0xd0)
		n = 2;
	au->queue(offset, b, n);
	return noErr;
}

OSStatus au_sysex(void *self, const UInt8 *data, UInt32 length)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au || !data || length == 0)
		return kAudio_ParamError;
	// By AU's rules the bytes arrive complete, F0 through F7, and the engine
	// takes them in that form (the same as vst3/plugin.cpp)
	au->queue(0, data, length);
	return noErr;
}

OSStatus au_start_note(void *self, MusicDeviceInstrumentID, MusicDeviceGroupID group,
                       NoteInstanceID *id, UInt32 offset, const MusicDeviceNoteParams *params)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	if (id)
		*id = 0;
	if (!params || params->argCount < 2)
		return kAudio_ParamError;
	const UInt8 note = UInt8(int(std::lround(params->mPitch)) & 0x7f);
	const UInt8 vel  = UInt8(int(std::lround(params->mVelocity * 127.0)) & 0x7f);
	const UInt8 b[3] = { UInt8(0x90 | (group & 0x0f)), note, vel };
	au->queue(offset, b, 3);
	return noErr;
}

OSStatus au_stop_note(void *self, MusicDeviceGroupID group, NoteInstanceID, UInt32 offset)
{
	auto *au = static_cast<au_instance *>(self);
	if (!au)
		return kAudio_ParamError;
	// Which note this refers to is not known (NoteInstanceID is not remembered),
	// so the whole channel is stopped -- all-notes-off on that channel, which is
	// what plain MIDI would do anyway
	const UInt8 b[3] = { UInt8(0xb0 | (group & 0x0f)), 123, 0 };
	au->queue(offset, b, 3);
	return noErr;
}

OSStatus au_prepare_instrument(void *self, MusicDeviceInstrumentID, MusicDeviceGroupID, UInt32)
{
	return noErr;
}

OSStatus au_release_instrument(void *self, MusicDeviceInstrumentID, MusicDeviceGroupID, UInt32)
{
	return noErr;
}

} // namespace


// ---------------------------------------------------------------------------
// The selector table. This is the whole of an AU's wiring

namespace {

AudioComponentMethod au_lookup(SInt16 selector)
{
	switch (selector) {
	case kAudioUnitInitializeSelect:            return reinterpret_cast<AudioComponentMethod>(au_initialize);
	case kAudioUnitUninitializeSelect:          return reinterpret_cast<AudioComponentMethod>(au_uninitialize);
	case kAudioUnitGetPropertyInfoSelect:       return reinterpret_cast<AudioComponentMethod>(au_get_property_info);
	case kAudioUnitGetPropertySelect:           return reinterpret_cast<AudioComponentMethod>(au_get_property);
	case kAudioUnitSetPropertySelect:           return reinterpret_cast<AudioComponentMethod>(au_set_property);
	case kAudioUnitAddPropertyListenerSelect:   return reinterpret_cast<AudioComponentMethod>(au_add_property_listener);
	case kAudioUnitRemovePropertyListenerSelect:return reinterpret_cast<AudioComponentMethod>(au_remove_property_listener);
	case kAudioUnitRemovePropertyListenerWithUserDataSelect:
		return reinterpret_cast<AudioComponentMethod>(au_remove_property_listener_ud);
	case kAudioUnitAddRenderNotifySelect:       return reinterpret_cast<AudioComponentMethod>(au_add_render_notify);
	case kAudioUnitRemoveRenderNotifySelect:    return reinterpret_cast<AudioComponentMethod>(au_remove_render_notify);
	case kAudioUnitGetParameterSelect:          return reinterpret_cast<AudioComponentMethod>(au_get_parameter);
	case kAudioUnitSetParameterSelect:          return reinterpret_cast<AudioComponentMethod>(au_set_parameter);
	case kAudioUnitScheduleParametersSelect:    return reinterpret_cast<AudioComponentMethod>(au_schedule_parameters);
	case kAudioUnitRenderSelect:                return reinterpret_cast<AudioComponentMethod>(au_render);
	case kAudioUnitResetSelect:                 return reinterpret_cast<AudioComponentMethod>(au_reset);
	case kMusicDeviceMIDIEventSelect:           return reinterpret_cast<AudioComponentMethod>(au_midi_event);
	case kMusicDeviceSysExSelect:               return reinterpret_cast<AudioComponentMethod>(au_sysex);
	case kMusicDeviceStartNoteSelect:           return reinterpret_cast<AudioComponentMethod>(au_start_note);
	case kMusicDeviceStopNoteSelect:            return reinterpret_cast<AudioComponentMethod>(au_stop_note);
	case kMusicDevicePrepareInstrumentSelect:   return reinterpret_cast<AudioComponentMethod>(au_prepare_instrument);
	case kMusicDeviceReleaseInstrumentSelect:   return reinterpret_cast<AudioComponentMethod>(au_release_instrument);
	default:
		break;
	}
	return nullptr;
}

} // namespace


// ---------------------------------------------------------------------------
// The factory. Info.plist's factoryFunction names this symbol

extern "C"
__attribute__((visibility("default")))
AudioComponentPlugInInterface *SMU2000AUFactory(const AudioComponentDescription *desc)
{
	if (desc && (desc->componentType != kType || desc->componentSubType != kSubtype))
		return nullptr;

	auto *au = new au_instance();
	au->iface.Open   = &au_open;
	au->iface.Close  = &au_close;
	au->iface.Lookup = &au_lookup;
	au->iface.reserved = nullptr;
	return &au->iface;
}

// The four-character identity, readable from outside (aubprobe uses it)
extern "C" __attribute__((visibility("default"))) OSType SMU2000AUType()      { return kType; }
extern "C" __attribute__((visibility("default"))) OSType SMU2000AUSubtype()   { return kSubtype; }
extern "C" __attribute__((visibility("default"))) OSType SMU2000AUManu()      { return kManufacturer; }
extern "C" __attribute__((visibility("default"))) UInt32 SMU2000AUVers()      { return kVersion; }
