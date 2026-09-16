// license:BSD-3-Clause
//
// S-MU2000 の VST3 プラグイン。
//
// Steinberg の SDK のうち **インターフェース定義（pluginterfaces, MIT）だけ**
// を使い、土台（public.sdk, GPLv3）は使っていない。だから配線は全部ここにある。
// third_party/vst3/README.md に経緯がある。
//
// 作りは単一コンポーネント（single component effect）。IComponent と
// IEditController を 1 つのクラスが両方受け持つ。画面は持たない。
//
// MIDI の入り方は 2 通りある。VST3 はここが独特で、
//   ・ノートオン／オフ、ポリプレッシャ、システムエクスクルーシブ … イベント
//   ・コントロールチェンジ、ピッチベンド、プログラムチェンジ  … パラメータ
// になる。後者のために IMidiMapping で「チャンネル×番号 → パラメータ番号」を
// 教えてやる必要がある。受け取った側でまた MIDI のバイト列に組み直して音源へ渡す。
//
// MIDI の入力バスは **2 本**。実機の MIDI IN A（パート 1-16）と B（パート 17-32）
// に当たる。パラメータも口ごとに 16ch × 131 本ずつ持つ。

#include "engine.h"
#include "state.h"
#include "view.h"

#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstmidicontrollers.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <thread>
#include <vector>

#if defined(_WIN32)
#include "compat/platform.h"      // windows.h (lean) for QueryPerformanceCounter
#endif

#if defined(__APPLE__)
#include <CoreFoundation/CoreFoundation.h>      // CFBundleRef, the host's handle
#include <mach/mach_time.h>                     // mach_absolute_time
#endif

using namespace Steinberg;
using namespace Steinberg::Vst;

namespace {

// ---- このプラグインを表す番号。一度決めたら変えられない
//      （変えるとホストが別物とみなし、保存した曲から見つからなくなる）
static const FUID kProcessorUID(0x5D2E4B70, 0xA1C34F92, 0x8B0E7A61, 0x4D553000);

constexpr const char *kPlugName   = "S-MU2000";
constexpr const char *kVendor     = "tarboh";
constexpr const char *kVersion    = "0.1.0.0";

// ---- A clock for measuring the load
//
// In the shape QueryPerformanceCounter has: a constant frequency and a tick
// count, so "busy seconds" and "did this block miss its deadline" are the same
// arithmetic on both platforms. Only ever used to report load -- nothing here
// reaches the audio path, so the exact rate does not matter.
//
//   Windows … QueryPerformanceCounter
//   macOS   … mach_absolute_time, scaled to nanoseconds by mach_timebase_info
int64 perf_frequency()
{
#if defined(_WIN32)
	LARGE_INTEGER f;
	QueryPerformanceFrequency(&f);
	return f.QuadPart;
#else
	return 1000000000;      // perf_ticks() below hands back nanoseconds
#endif
}

uint64 perf_ticks()
{
#if defined(_WIN32)
	LARGE_INTEGER t;
	QueryPerformanceCounter(&t);
	return uint64(t.QuadPart);
#else
	// The timebase is constant on a given machine, so resolve it once
	static const double scale = [] {
		mach_timebase_info_data_t tb{};
		mach_timebase_info(&tb);
		return double(tb.numer) / double(tb.denom);
	}();
	return uint64(double(mach_absolute_time()) * scale);
#endif
}

// MIDI のコントロール番号は 0-127 のほか、
//   128 チャンネルプレッシャ / 129 ピッチベンド / 130 プログラムチェンジ
// まである。チャンネルごとにこれだけのパラメータを並べる
constexpr int32 kCtrlCount = 131;
constexpr int32 kChannels  = 16;
constexpr int32 kMidiParams = kChannels * kCtrlCount;

// 画面を持たないので、ホストの汎用パネルに出す物がこれだけ要る。
//   出力レベル … 音源の外で掛ける素の掛け算
//   状態      … 起動中か、鳴る用意ができたか、ROM が無いか。読むだけ
constexpr ParamID kGainId   = 4096;
constexpr ParamID kStatusId = 4097;

// MIDI IN B（パート 17-32）のぶん。A の 0-2095 と Output / Status の番号は
// 保存した曲が覚えているので動かさず、B は離れた 8192 番から並べる
constexpr int32   kPorts      = 2;
constexpr ParamID kPortBBase  = 8192;
constexpr int32   kParamCount = kPorts * kMidiParams + 2;

ParamID param_of(int32 port, int32 ch, int32 ctrl)
{
	return ParamID((port ? kPortBBase : 0) + ch * kCtrlCount + ctrl);
}

// パラメータ番号を、口・チャンネル・番号と m_value の位置に戻す。
// MIDI のパラメータでなければ false
bool midi_param(ParamID id, int32 &port, int32 &ch, int32 &ctrl, int32 &slot)
{
	int32 x = 0;
	if (id < ParamID(kMidiParams)) {
		port = 0;
		x = int32(id);
	} else if (id >= kPortBBase && id < kPortBBase + ParamID(kMidiParams)) {
		port = 1;
		x = int32(id - kPortBBase);
	} else {
		return false;
	}
	ch   = x / kCtrlCount;
	ctrl = x % kCtrlCount;
	slot = port * kMidiParams + x;
	return true;
}

// ピッチベンドのパラメータか（これだけ 14bit で、目盛りの付け方が違う）
bool is_bend(ParamID id)
{
	int32 port, ch, ctrl, slot;
	return midi_param(id, port, ch, ctrl, slot) && ctrl == 129;
}

// パラメータの初期値。ホストが起動時にこれを送ってくることがあるので、
// 「初期値と同じ値が来たら何も送らない」ようにするための表でもある
double default_of(int32 ctrl)
{
	switch (ctrl) {
	case 7:   return 100.0 / 127.0;   // ボリューム
	case 10:  return  64.0 / 127.0;   // パン（中央）
	case 11:  return 1.0;             // エクスプレッション
	case 129: return 0.5;             // ピッチベンド（中央）
	default:  return 0.0;
	}
}

void set_str(String128 dst, const char *ascii)
{
	int i = 0;
	for (; ascii[i] && i < 127; i++)
		dst[i] = TChar(uint8(ascii[i]));
	dst[i] = 0;
}


// ---- 本体

class mu_plugin : public IComponent, public IAudioProcessor,
                  public IEditController, public IMidiMapping
{
public:
	mu_plugin()
	{
		for (int32 i = 0; i < kPorts * kMidiParams; i++)
			m_value[i] = default_of(i % kCtrlCount);
		m_engine.panel().set_gain(1.0f);
		m_gain_now = 1.0f;
		m_msgs.reserve(8192);
		m_engine.set_output_rate(smu2000::vst3::NATIVE_RATE);
		m_qpc_freq = perf_frequency();
	}

	virtual ~mu_plugin() = default;

	// ---- FUnknown

	tresult PLUGIN_API queryInterface(const TUID _iid, void **obj) override
	{
		if (FUnknownPrivate::iidEqual(_iid, FUnknown::iid) ||
		    FUnknownPrivate::iidEqual(_iid, IComponent::iid)) {
			addRef(); *obj = static_cast<IComponent *>(this); return kResultOk;
		}
		if (FUnknownPrivate::iidEqual(_iid, IPluginBase::iid)) {
			// IComponent と IEditController の両方が IPluginBase を持つので、
			// どちらの側か決めてやらないと曖昧になる
			addRef(); *obj = static_cast<IPluginBase *>(static_cast<IComponent *>(this));
			return kResultOk;
		}
		if (FUnknownPrivate::iidEqual(_iid, IAudioProcessor::iid)) {
			addRef(); *obj = static_cast<IAudioProcessor *>(this); return kResultOk;
		}
		if (FUnknownPrivate::iidEqual(_iid, IEditController::iid)) {
			addRef(); *obj = static_cast<IEditController *>(this); return kResultOk;
		}
		if (FUnknownPrivate::iidEqual(_iid, IMidiMapping::iid)) {
			addRef(); *obj = static_cast<IMidiMapping *>(this); return kResultOk;
		}
		*obj = nullptr;
		return kNoInterface;
	}

	uint32 PLUGIN_API addRef() override
	{ return uint32(FUnknownPrivate::atomicAdd(m_refs, 1)); }

	uint32 PLUGIN_API release() override
	{
		if (FUnknownPrivate::atomicAdd(m_refs, -1) == 0) { delete this; return 0; }
		return uint32(m_refs);
	}

	// ---- IPluginBase

	tresult PLUGIN_API initialize(FUnknown *) override
	{
		// ROM 読みと起動（音にして 4 秒ぶんの空回し）は時間がかかるので、
		// ここでは走らせるだけ。終わるまでは無音を返す
		m_engine.start();
		return kResultOk;
	}

	tresult PLUGIN_API terminate() override { return kResultOk; }

	// ---- IComponent

	tresult PLUGIN_API getControllerClassId(TUID) override
	{
		// 単一コンポーネント。ホストはこの同じ物から IEditController を取る
		return kNotImplemented;
	}

	tresult PLUGIN_API setIoMode(IoMode) override { return kNotImplemented; }

	int32 PLUGIN_API getBusCount(MediaType type, BusDirection dir) override
	{
		if (type == kAudio) return 1;          // 出力 1 つと、A/D INPUT の入力 1 つ
		if (type == kEvent) return dir == kInput  ? kPorts : 0;
		return 0;
	}

	tresult PLUGIN_API getBusInfo(MediaType type, BusDirection dir, int32 index,
	                              BusInfo &bus) override
	{
		if (type == kAudio && dir == kOutput && index == 0) {
			bus.mediaType    = kAudio;
			bus.direction    = kOutput;
			bus.channelCount = 2;
			set_str(bus.name, "Stereo Out");
			bus.busType = kMain;
			bus.flags   = BusInfo::kDefaultActive;
			return kResultOk;
		}
		// A/D INPUT。サンプリングで録る音。補助の入力（サイドチェーン）として出す
		if (type == kAudio && dir == kInput && index == 0) {
			bus.mediaType    = kAudio;
			bus.direction    = kInput;
			bus.channelCount = 2;
			set_str(bus.name, "A/D Input");
			bus.busType = kAux;
			bus.flags   = 0;
			return kResultOk;
		}
		if (type == kEvent && dir == kInput && index >= 0 && index < kPorts) {
			bus.mediaType    = kEvent;
			bus.direction    = kInput;
			bus.channelCount = 16;
			// 実機の MIDI IN A / B。B はパート 17-32 に届く
			set_str(bus.name, index == 0 ? "MIDI In A (Part 1-16)" : "MIDI In B (Part 17-32)");
			bus.busType = index == 0 ? kMain : kAux;
			bus.flags   = BusInfo::kDefaultActive;
			return kResultOk;
		}
		return kInvalidArgument;
	}

	tresult PLUGIN_API getRoutingInfo(RoutingInfo &, RoutingInfo &) override
	{ return kNotImplemented; }

	tresult PLUGIN_API activateBus(MediaType, BusDirection, int32, TBool) override
	{ return kResultOk; }

	tresult PLUGIN_API setActive(TBool state) override
	{
		if (state) {
			m_engine.start();
		} else {
			m_hush.store(true);
			m_engine.set_processing(false);
			report();
		}
		return kResultOk;
	}

	// 画面が無いので、間に合っていたかどうかは止めるときに記録へ書く
	void report()
	{
		if (!m_produced)
			return;
		const double audio = double(m_produced) / m_rate;
		const double busy  = double(m_busy_ticks) / double(m_qpc_freq);
		char line[256];
		std::snprintf(line, sizeof(line),
		              "%.0f Hz で %.0f 秒ぶん: CPU %.1f%%、1 ブロックの最悪 %.2f ms、"
		              "間に合わなかった回数 %llu",
		              m_rate, audio, 100.0 * busy / audio,
		              1000.0 * double(m_worst_ticks) / double(m_qpc_freq),
		              (unsigned long long)m_late);
		m_engine.log_line(line);
		m_busy_ticks = m_produced = m_worst_ticks = m_late = 0;
	}

	tresult PLUGIN_API setState(IBStream *stream) override
	{
		if (!stream)
			return kResultFalse;
		int32 version = 0, got = 0;
		float gain = 1.0f;
		if (stream->read(&version, sizeof(version), &got) != kResultOk || got != sizeof(version))
			return kResultOk;   // 空でも困らない
		if (stream->read(&gain, sizeof(gain), &got) == kResultOk && got == sizeof(gain) &&
		    gain >= 0.0f && gain <= 1.0f)
			m_engine.panel().set_gain(gain);
		if (version < 2)
			return kResultOk;   // 古い形。出力レベルだけ

		// 機械まるごとの状態。詰めた形で入っている
		int32 packed_size = 0;
		if (stream->read(&packed_size, sizeof(packed_size), &got) != kResultOk ||
		    got != sizeof(packed_size) || packed_size <= 0 || packed_size > (64 << 20))
			return kResultOk;
		std::vector<uint8_t> packed;
		packed.resize(size_t(packed_size));
		if (stream->read(packed.data(), packed_size, &got) != kResultOk ||
		    got != packed_size)
			return kResultOk;
		std::vector<u8> blob;
		if (!state_unpack(packed.data(), packed.size(), blob))
			return kResultOk;

		// 起動が終わっていないと戻せない。終わるまで待つ
		for (int i = 0; i < 300 && m_engine.state() == smu2000::vst3::status::loading; i++)
			std::this_thread::sleep_for(std::chrono::milliseconds(10));
		m_engine.load_state(blob.data(), blob.size());

		// 版 3 から: 差していた SmartMedia のファイル（UTF-8）。無くなっていたら差さない
		if (version >= 3) {
			int32 len = 0;
			if (stream->read(&len, sizeof(len), &got) == kResultOk && got == sizeof(len) && len > 0 && len < 4096) {
				std::string path(size_t(len), '\0');
				if (stream->read(path.data(), len, &got) == kResultOk && got == len) {
					std::string err;
					if (!m_engine.card_insert(path, err))
						m_engine.log_line(("SmartMedia を差せない: " + err).c_str());
				}
			}
		} else if (!m_engine.card_path().empty()) {
			m_engine.card_eject();
		}
		return kResultOk;
	}

	tresult PLUGIN_API getState(IBStream *stream) override
	{
		// **機械まるごと**（CPU・RAM・SWP30・LCD）を入れる。だから曲を
		// 開き直しても、音色もエフェクトもそのまま戻る
		if (!stream)
			return kResultFalse;
		int32 version = 3;
		float gain = m_engine.panel().gain();
		m_engine.card_flush();   // プロジェクトを保存するときに、カードのファイルも揃える
		int32 written = 0;
		stream->write(&version, sizeof(version), &written);
		stream->write(&gain, sizeof(gain), &written);

		const std::vector<uint8_t> blob = m_engine.save_state();
		if (blob.empty())
			return kResultOk;                 // まだ起動中など
		const std::vector<u8> packed = state_pack(blob);
		int32 n = int32(packed.size());
		stream->write(&n, sizeof(n), &written);
		stream->write(const_cast<uint8_t *>(packed.data()), n, &written);
		// 版 3: 差している SmartMedia のファイル。中身はプロジェクトに入れない（16MB から 128MB あるので）
		const std::string card = m_engine.card_path();
		int32 len = int32(card.size());
		stream->write(&len, sizeof(len), &written);
		if (len)
			stream->write(const_cast<char *>(card.data()), len, &written);
		return kResultOk;
	}

	// ---- IAudioProcessor

	tresult PLUGIN_API setBusArrangements(SpeakerArrangement *inputs, int32 numIns,
	                                      SpeakerArrangement *outputs, int32 numOuts) override
	{
		if (numOuts == 1 && outputs[0] == SpeakerArr::kStereo &&
		    (numIns == 0 || (numIns == 1 && inputs[0] == SpeakerArr::kStereo)))
			return kResultTrue;
		return kResultFalse;
	}

	tresult PLUGIN_API getBusArrangement(BusDirection dir, int32 index,
	                                     SpeakerArrangement &arr) override
	{
		if (index == 0) { arr = SpeakerArr::kStereo; return kResultOk; }
		return kInvalidArgument;
	}

	tresult PLUGIN_API canProcessSampleSize(int32 size) override
	{ return size == kSample32 ? kResultTrue : kResultFalse; }

	uint32 PLUGIN_API getLatencySamples() override { return m_engine.latency_samples(); }

	tresult PLUGIN_API setupProcessing(ProcessSetup &setup) override
	{
		m_rate = setup.sampleRate;
		m_engine.set_output_rate(m_rate);
		return kResultOk;
	}

	tresult PLUGIN_API setProcessing(TBool state) override
	{
		if (!state)
			m_hush.store(true);
		// 動いているあいだ、機械に触れてよいのは音声スレッドだけ
		m_engine.set_processing(state != 0);
		return kResultOk;
	}

	uint32 PLUGIN_API getTailSamples() override
	{
		// 残響がある。4 秒みておく
		return uint32(m_rate * 4.0);
	}

	tresult PLUGIN_API process(ProcessData &data) override;

	// ---- IEditController

	tresult PLUGIN_API setComponentState(IBStream *) override { return kResultOk; }

	int32 PLUGIN_API getParameterCount() override { return kParamCount; }

	tresult PLUGIN_API getParameterInfo(int32 index, ParameterInfo &info) override
	{
		if (index < 0 || index >= kParamCount)
			return kInvalidArgument;
		// 並びは A の 2096 本、Output、Status、B の 2096 本。
		// 前からあるものの位置を変えないよう、B は後ろに足した
		if (index == kMidiParams || index == kMidiParams + 1) {
			std::memset(&info, 0, sizeof(info));
			if (index == kMidiParams) {
				info.id = kGainId;
				set_str(info.title, "Output");
				set_str(info.shortTitle, "Out");
				set_str(info.units, "%");
				info.defaultNormalizedValue = 1.0;
				info.flags = ParameterInfo::kCanAutomate;
			} else {
				info.id = kStatusId;
				set_str(info.title, "Status");
				set_str(info.shortTitle, "Stat");
				info.stepCount = 2;
				info.flags = ParameterInfo::kIsReadOnly;
			}
			return kResultOk;
		}
		const int32 port = index < kMidiParams ? 0 : 1;
		const int32 x = port ? index - kMidiParams - 2 : index;
		const int32 ch = x / kCtrlCount, ctrl = x % kCtrlCount;

		// B の口は頭に "B " を付ける（A は前からの名前のまま）
		const char *pre = port ? "B " : "";
		char name[64];
		if (ctrl < 128)      std::snprintf(name, sizeof(name), "%sCh%d CC%d", pre, ch + 1, ctrl);
		else if (ctrl == 128) std::snprintf(name, sizeof(name), "%sCh%d Aftertouch", pre, ch + 1);
		else if (ctrl == 129) std::snprintf(name, sizeof(name), "%sCh%d Pitch Bend", pre, ch + 1);
		else                  std::snprintf(name, sizeof(name), "%sCh%d Program", pre, ch + 1);

		std::memset(&info, 0, sizeof(info));
		info.id = param_of(port, ch, ctrl);
		set_str(info.title, name);
		set_str(info.shortTitle, name);
		info.stepCount = (ctrl == 129) ? 0 : 127;   // ピッチベンドだけ連続
		info.defaultNormalizedValue = default_of(ctrl);
		info.unitId = 0;   // kRootUnitId
		// 4192 本もあるので、一覧に並べさせない
		info.flags = ParameterInfo::kCanAutomate | ParameterInfo::kIsHidden;
		return kResultOk;
	}

	tresult PLUGIN_API getParamStringByValue(ParamID id, ParamValue v, String128 str) override
	{
		if (id == kStatusId) {
			// ここが唯一の「表に見える」窓口。無音の理由がこれで分かる
			const int k = int(std::lround(v * 2.0));
			set_str(str, k == 0 ? "Booting" : k == 1 ? "Ready" : "No ROM");
			return kResultOk;
		}
		if (id == kGainId) {
			char g[32];
			std::snprintf(g, sizeof(g), "%.0f", v * 100.0);
			set_str(str, g);
			return kResultOk;
		}
		int32 port, ch, ctrl, slot;
		if (!midi_param(id, port, ch, ctrl, slot))
			return kInvalidArgument;
		char buf[32];
		if (ctrl == 129)
			std::snprintf(buf, sizeof(buf), "%+d", int(std::lround(v * 16383.0)) - 8192);
		else
			std::snprintf(buf, sizeof(buf), "%d", int(std::lround(v * 127.0)));
		set_str(str, buf);
		return kResultOk;
	}

	tresult PLUGIN_API getParamValueByString(ParamID id, TChar *str, ParamValue &v) override
	{
		if (!str)
			return kInvalidArgument;
		if (id == kGainId || id == kStatusId)
			return kResultFalse;
		int32 port, ch, ctrl, slot;
		if (!midi_param(id, port, ch, ctrl, slot))
			return kInvalidArgument;
		char buf[32];
		int i = 0;
		for (; i < 31 && str[i]; i++)
			buf[i] = char(str[i]);
		buf[i] = 0;
		const double plain = std::atof(buf);
		v = ctrl == 129 ? std::clamp((plain + 8192.0) / 16383.0, 0.0, 1.0)
		                : std::clamp(plain / 127.0, 0.0, 1.0);
		return kResultOk;
	}

	ParamValue PLUGIN_API normalizedParamToPlain(ParamID id, ParamValue v) override
	{
		if (id == kGainId)   return v * 100.0;
		if (id == kStatusId) return std::lround(v * 2.0);
		return is_bend(id) ? std::lround(v * 16383.0) - 8192.0
		                   : std::lround(v * 127.0);
	}

	ParamValue PLUGIN_API plainParamToNormalized(ParamID id, ParamValue plain) override
	{
		if (id == kGainId)   return std::clamp(plain / 100.0, 0.0, 1.0);
		if (id == kStatusId) return std::clamp(plain / 2.0, 0.0, 1.0);
		return is_bend(id) ? std::clamp((plain + 8192.0) / 16383.0, 0.0, 1.0)
		                   : std::clamp(plain / 127.0, 0.0, 1.0);
	}

	ParamValue PLUGIN_API getParamNormalized(ParamID id) override
	{
		if (id == kGainId)
			return m_engine.panel().gain();
		if (id == kStatusId) {
			switch (m_engine.state()) {
			case smu2000::vst3::status::loading: return 0.0;
			case smu2000::vst3::status::ready:   return 0.5;
			default:                             return 1.0;
			}
		}
		int32 port, ch, ctrl, slot;
		return midi_param(id, port, ch, ctrl, slot) ? m_value[slot] : 0.0;
	}

	tresult PLUGIN_API setParamNormalized(ParamID id, ParamValue v) override
	{
		if (id == kGainId) { m_engine.panel().set_gain(float(std::clamp(v, 0.0, 1.0))); return kResultOk; }
		if (id == kStatusId)
			return kResultFalse;   // 読むだけ
		int32 port, ch, ctrl, slot;
		if (!midi_param(id, port, ch, ctrl, slot))
			return kInvalidArgument;
		m_value[slot] = v;
		return kResultOk;
	}

	tresult PLUGIN_API setComponentHandler(IComponentHandler *) override { return kResultOk; }

	// 画面。実機のフロントパネル風。中身は gui.exe と同じ ui::panel
	IPlugView *PLUGIN_API createView(FIDString name) override
	{
		if (name && std::strcmp(name, ViewType::kEditor) != 0)
			return nullptr;
		return new smu2000::vst3::plug_view(m_engine);
	}

	// 音量は bridge が 1 つだけ持つ。Output パラメータも画面のつまみも同じ値

	// ---- IMidiMapping

	tresult PLUGIN_API getMidiControllerAssignment(int32 busIndex, int16 channel,
	                                               CtrlNumber ctrl, ParamID &id) override
	{
		if (busIndex < 0 || busIndex >= kPorts || channel < 0 || channel >= kChannels)
			return kResultFalse;
		if (ctrl < 0 || ctrl >= kCtrlCount)
			return kResultFalse;
		id = param_of(busIndex, channel, ctrl);
		return kResultTrue;
	}

private:
	// process の中で時刻順に並べ直すための入れ物。
	// 短いものは中に持ち、システムエクスクルーシブはホストの領域を指す
	struct msg {
		int32          off;
		int32          seq;
		uint8          port;          // 0 が MIDI IN A、1 が B
		uint8          n;
		uint8          b[3];
		const uint8   *sysex;
		uint32         sysex_len;
	};

	void queue(int32 port, int32 off, uint8 a, uint8 b = 0, uint8 c = 0, int n = 3)
	{
		if (m_msgs.size() >= m_msgs.capacity())
			return;
		m_msgs.push_back({ off, int32(m_msgs.size()), uint8(port), uint8(n), { a, b, c }, nullptr, 0 });
	}

	smu2000::vst3::engine m_engine;
	std::vector<msg>      m_msgs;
	double                m_rate = smu2000::vst3::NATIVE_RATE;
	double                m_value[kPorts * kMidiParams] = {};
	// 出力レベルは bridge が持つ。ここは 1 サンプルずつ寄せる途中の値
	float                 m_gain_now = 1.0f;
	std::atomic<bool>     m_hush{false};
	// 間に合っているかの記録。音声スレッドだけが触る
	uint64                m_busy_ticks = 0, m_produced = 0, m_worst_ticks = 0, m_late = 0;
	int64                 m_qpc_freq = 1;
	int32                 m_refs = 1;
};


tresult PLUGIN_API mu_plugin::process(ProcessData &data)
{
	// 64bit 浮動小数は受けないと答えてある。それでも来たら音を出さない
	// （倍精度の配列を単精度として書けば壊れる）
	AudioBusBuffers *out = (data.numOutputs > 0 &&
	                        data.symbolicSampleSize == kSample32) ? &data.outputs[0] : nullptr;
	float *left  = (out && out->numChannels > 0) ? out->channelBuffers32[0] : nullptr;
	float *right = (out && out->numChannels > 1) ? out->channelBuffers32[1] : left;
	// A/D INPUT（補助の入力）。繋がっていなければ無し
	AudioBusBuffers *in = (data.numInputs > 0 && data.inputs &&
	                       data.symbolicSampleSize == kSample32) ? &data.inputs[0] : nullptr;
	const float *in_l = (in && in->numChannels > 0 && in->channelBuffers32) ? in->channelBuffers32[0] : nullptr;
	const float *in_r = (in && in->numChannels > 1 && in->channelBuffers32) ? in->channelBuffers32[1] : in_l;

	const uint64 t0 = perf_ticks();

	if (m_hush.exchange(false))
		m_engine.all_notes_off();

	// ---- まず、この区間に来た MIDI を全部集める

	m_msgs.clear();

	if (IParameterChanges *changes = data.inputParameterChanges) {
		const int32 nq = changes->getParameterCount();
		for (int32 q = 0; q < nq; q++) {
			IParamValueQueue *pq = changes->getParameterData(q);
			if (!pq)
				continue;
			const ParamID id = pq->getParameterId();
			if (id == kGainId) {
				// 出力レベルは音源に流さない。外で掛ける。最後の値だけ見る
				int32 off = 0;
				ParamValue v = 0.0;
				if (pq->getPointCount() > 0 &&
				    pq->getPoint(pq->getPointCount() - 1, off, v) == kResultOk)
					m_engine.panel().set_gain(float(std::clamp(v, 0.0, 1.0)));
				continue;
			}
			int32 port, ch, ctrl, slot;
			if (!midi_param(id, port, ch, ctrl, slot))
				continue;
			const int32 np = pq->getPointCount();
			for (int32 p = 0; p < np; p++) {
				int32 off = 0;
				ParamValue v = 0.0;
				if (pq->getPoint(p, off, v) != kResultOk)
					continue;
				m_value[slot] = v;
				// ここで「前と同じ値だから」と捨ててはいけない。RPN/NRPN は
				// CC101=0, CC100=0, CC6=n のように同じ値を続けて送ることに
				// 意味があり、捨てるとピッチベンド幅などが化ける

				if (ctrl < 128) {
					queue(port, off, uint8(0xb0 | ch), uint8(ctrl),
					      uint8(std::clamp(int(std::lround(v * 127.0)), 0, 127)));
				} else if (ctrl == 128) {
					queue(port, off, uint8(0xd0 | ch),
					      uint8(std::clamp(int(std::lround(v * 127.0)), 0, 127)), 0, 2);
				} else if (ctrl == 129) {
					const int bend = std::clamp(int(std::lround(v * 16383.0)), 0, 16383);
					queue(port, off, uint8(0xe0 | ch), uint8(bend & 127), uint8(bend >> 7));
				} else {
					queue(port, off, uint8(0xc0 | ch),
					      uint8(std::clamp(int(std::lround(v * 127.0)), 0, 127)), 0, 2);
				}
			}
		}
	}

	if (IEventList *events = data.inputEvents) {
		const int32 n = events->getEventCount();
		for (int32 i = 0; i < n; i++) {
			Event e;
			if (events->getEvent(i, e) != kResultOk)
				continue;
			const int32 off = e.sampleOffset;
			const int32 port = e.busIndex == 1 ? 1 : 0;
			switch (e.type) {
			case Event::kNoteOnEvent: {
				const int v = std::clamp(int(std::lround(e.noteOn.velocity * 127.0)), 1, 127);
				queue(port, off, uint8(0x90 | (e.noteOn.channel & 15)),
				      uint8(e.noteOn.pitch & 127), uint8(v));
				break;
			}
			case Event::kNoteOffEvent: {
				const int v = std::clamp(int(std::lround(e.noteOff.velocity * 127.0)), 0, 127);
				queue(port, off, uint8(0x80 | (e.noteOff.channel & 15)),
				      uint8(e.noteOff.pitch & 127), uint8(v));
				break;
			}
			case Event::kPolyPressureEvent: {
				const int v = std::clamp(int(std::lround(e.polyPressure.pressure * 127.0)), 0, 127);
				queue(port, off, uint8(0xa0 | (e.polyPressure.channel & 15)),
				      uint8(e.polyPressure.pitch & 127), uint8(v));
				break;
			}
			case Event::kDataEvent: {
				if (e.data.type != DataEvent::kMidiSysEx || !e.data.bytes || !e.data.size)
					break;
				// ホストによっては、チャンネルメッセージまでここに入れてくる
				// （VSTHost 1.58 はプログラムチェンジを C0 xx 00 と 3 byte にして渡す）。
				// そのまま音源へ流すと、余分な 00 が走行状態のデータバイトになり、
				// 続けてプログラム 0 が入って音色が戻ってしまう。頭がチャンネルの
				// ステータスならシステムエクスクルーシブではないので、
				// 決まった長さだけ取り出して流し、後ろの詰め物は捨てる
				const uint8 first = e.data.bytes[0];
				if (first >= 0x80 && first < 0xf0) {
					for (uint32 at = 0; at < e.data.size;) {
						const uint8 st = e.data.bytes[at];
						if (st < 0x80 || st >= 0xf0)
							break;      // 詰め物。ここから先は読まない
						const uint32 len = uint32(smu2000::vst3::midi_length(st));
						if (at + len > e.data.size)
							break;
						queue(port, off, st,
						      len > 1 ? e.data.bytes[at + 1] : uint8(0),
						      len > 2 ? e.data.bytes[at + 2] : uint8(0), int(len));
						at += len;
					}
					break;
				}
				// 本物のシステムエクスクルーシブ（と、その途中の切れ端）はバイト列のまま
				if (m_msgs.size() < m_msgs.capacity())
					m_msgs.push_back({ off, int32(m_msgs.size()), uint8(port), 0, { 0, 0, 0 },
					                   e.data.bytes, e.data.size });
				break;
			}
			default:
				break;
			}
		}
	}

	std::sort(m_msgs.begin(), m_msgs.end(), [](const msg &a, const msg &b) {
		return a.off != b.off ? a.off < b.off : a.seq < b.seq;
	});

	// ---- 時刻順に、音を作りながら流し込む

	const int32 n = data.numSamples;
	int32 done = 0;
	for (const msg &m : m_msgs) {
		const int32 at = std::clamp(m.off, done, n);
		if (at > done && left) {
			m_engine.fill(left + done, right + done, at - done,
			              in_l ? in_l + done : nullptr, in_r ? in_r + done : nullptr);
			done = at;
		} else if (at > done) {
			done = at;
		}
		if (m.sysex)
			m_engine.midi(m.sysex, m.sysex_len, m.port);
		else
			m_engine.midi(m.b, m.n, m.port);
	}
	if (left && done < n)
		m_engine.fill(left + done, right + done, n - done,
		              in_l ? in_l + done : nullptr, in_r ? in_r + done : nullptr);

	// 出力レベル。一気に変えると音が跳ねるので 1 サンプルずつ寄せる
	if (left) {
		const float target = m_engine.panel().gain();
		if (target != m_gain_now || target != 1.0f) {
			const float step = 1.0f / 512.0f;
			for (int32 i = 0; i < n; i++) {
				if (m_gain_now < target) m_gain_now = std::min(target, m_gain_now + step);
				else if (m_gain_now > target) m_gain_now = std::max(target, m_gain_now - step);
				left[i]  *= m_gain_now;
				right[i] *= m_gain_now;
			}
		}
	}

	// 間に合っているかの目安。setActive(false) のときに記録へ書く
	const uint64 took = perf_ticks() - t0;
	m_busy_ticks += took;
	m_produced   += uint64(n);
	if (took > m_worst_ticks) m_worst_ticks = took;
	if (double(took) / double(m_qpc_freq) > double(n) / m_rate) m_late++;

	if (out) {
		out->silenceFlags = 0;
		// 片側しか無いホストのために、右が左と同じ配列でも困らないようにしてある
		if (out->numChannels > 2)
			for (int32 c = 2; c < out->numChannels; c++)
				std::memset(out->channelBuffers32[c], 0, size_t(n) * sizeof(float));
	}
	return kResultOk;
}


// ---- 工場。ホストはまずこれを取りに来る

class factory : public IPluginFactory3
{
public:
	tresult PLUGIN_API queryInterface(const TUID _iid, void **obj) override
	{
		if (FUnknownPrivate::iidEqual(_iid, FUnknown::iid) ||
		    FUnknownPrivate::iidEqual(_iid, IPluginFactory::iid) ||
		    FUnknownPrivate::iidEqual(_iid, IPluginFactory2::iid) ||
		    FUnknownPrivate::iidEqual(_iid, IPluginFactory3::iid)) {
			addRef(); *obj = static_cast<IPluginFactory3 *>(this); return kResultOk;
		}
		*obj = nullptr;
		return kNoInterface;
	}

	uint32 PLUGIN_API addRef() override  { return uint32(FUnknownPrivate::atomicAdd(m_refs, 1)); }
	uint32 PLUGIN_API release() override { return uint32(FUnknownPrivate::atomicAdd(m_refs, -1)); }

	tresult PLUGIN_API getFactoryInfo(PFactoryInfo *info) override
	{
		if (!info)
			return kInvalidArgument;
		std::memset(static_cast<void *>(info), 0, sizeof(*info));
		std::strncpy(info->vendor, kVendor, PFactoryInfo::kNameSize - 1);
		std::strncpy(info->url, "https://github.com/tarboh/S-MU2000", PFactoryInfo::kURLSize - 1);
		info->flags = PFactoryInfo::kUnicode;
		return kResultOk;
	}

	int32 PLUGIN_API countClasses() override { return 1; }

	tresult PLUGIN_API getClassInfo(int32 index, PClassInfo *info) override
	{
		if (index != 0 || !info)
			return kInvalidArgument;
		std::memset(static_cast<void *>(info), 0, sizeof(*info));
		std::memcpy(info->cid, kProcessorUID.toTUID(), sizeof(TUID));
		info->cardinality = PClassInfo::kManyInstances;
		std::strncpy(info->category, kVstAudioEffectClass, PClassInfo::kCategorySize - 1);
		std::strncpy(info->name, kPlugName, PClassInfo::kNameSize - 1);
		return kResultOk;
	}

	tresult PLUGIN_API getClassInfo2(int32 index, PClassInfo2 *info) override
	{
		if (index != 0 || !info)
			return kInvalidArgument;
		std::memset(static_cast<void *>(info), 0, sizeof(*info));
		std::memcpy(info->cid, kProcessorUID.toTUID(), sizeof(TUID));
		info->cardinality = PClassInfo::kManyInstances;
		std::strncpy(info->category, kVstAudioEffectClass, PClassInfo::kCategorySize - 1);
		std::strncpy(info->name, kPlugName, PClassInfo::kNameSize - 1);
		info->classFlags = 0;
		std::strncpy(info->subCategories, PlugType::kInstrumentSynth,
		             PClassInfo2::kSubCategoriesSize - 1);
		std::strncpy(info->vendor, kVendor, PClassInfo2::kVendorSize - 1);
		std::strncpy(info->version, kVersion, PClassInfo2::kVersionSize - 1);
		std::strncpy(info->sdkVersion, kVstVersionString, PClassInfo2::kVersionSize - 1);
		return kResultOk;
	}

	tresult PLUGIN_API getClassInfoUnicode(int32 index, PClassInfoW *info) override
	{
		if (index != 0 || !info)
			return kInvalidArgument;
		std::memset(static_cast<void *>(info), 0, sizeof(*info));
		std::memcpy(info->cid, kProcessorUID.toTUID(), sizeof(TUID));
		info->cardinality = PClassInfo::kManyInstances;
		std::strncpy(info->category, kVstAudioEffectClass, PClassInfo::kCategorySize - 1);
		set_str16(info->name, kPlugName, PClassInfo::kNameSize);
		info->classFlags = 0;
		std::strncpy(info->subCategories, PlugType::kInstrumentSynth,
		             PClassInfo2::kSubCategoriesSize - 1);
		set_str16(info->vendor, kVendor, PClassInfo2::kVendorSize);
		set_str16(info->version, kVersion, PClassInfo2::kVersionSize);
		set_str16(info->sdkVersion, kVstVersionString, PClassInfo2::kVersionSize);
		return kResultOk;
	}

	tresult PLUGIN_API setHostContext(FUnknown *) override { return kResultOk; }

	tresult PLUGIN_API createInstance(FIDString cid, FIDString _iid, void **obj) override
	{
		if (!cid || !_iid || !obj)
			return kInvalidArgument;
		if (!FUnknownPrivate::iidEqual(cid, kProcessorUID.toTUID()))
			return kNoInterface;
		mu_plugin *p = new mu_plugin;
		const tresult r = p->queryInterface(_iid, obj);
		p->release();
		return r;
	}

private:
	static void set_str16(char16 *dst, const char *ascii, int max)
	{
		int i = 0;
		for (; ascii[i] && i < max - 1; i++)
			dst[i] = char16(uint8(ascii[i]));
		dst[i] = 0;
	}

	int32 m_refs = 1;
};

factory g_factory;

} // namespace


// ---- DLL の出口
//
// The entry points differ by platform: a Windows DLL is opened with
// LoadLibrary and announces InitDll/ExitDll, while macOS loads the bundle with
// CFBundle and looks for bundleEntry/bundleExit. Both still have to hand back
// the same factory.

extern "C" {

SMTG_EXPORT_SYMBOL IPluginFactory *PLUGIN_API GetPluginFactory()
{
	g_factory.addRef();
	return &g_factory;
}

#if defined(_WIN32)

__declspec(dllexport) bool InitDll() { return true; }
__declspec(dllexport) bool ExitDll() { return true; }

#elif defined(__APPLE__)

SMTG_EXPORT_SYMBOL bool bundleEntry(CFBundleRef bundle);
SMTG_EXPORT_SYMBOL bool bundleExit(void);

SMTG_EXPORT_SYMBOL bool bundleEntry(CFBundleRef bundle)
{
	(void)bundle;
	return true;
}

SMTG_EXPORT_SYMBOL bool bundleExit(void) { return true; }

#endif

}
