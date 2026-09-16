// license:BSD-3-Clause
//
// S-MU2000 の CLAP プラグイン。
//
// CLAP の口の定義（third_party/clap, MIT）だけを使い、中身は VST3 版と同じ
// smu2000::vst3::engine（ROM 探し・起動・周波数の変換）と plug_view（パネルの画面）を
// そのまま借りている。VST3 と違うのは口の形だけ:
//
//   ・MIDI はバイト列のまま届く（CLAP_EVENT_MIDI / MIDI_SYSEX）。VST3 のように
//     コントロールチェンジをパラメータに化けさせる必要が無い
//   ・ノートは CLAP 流（CLAP_EVENT_NOTE_ON 等）で来ることもあるので、MIDI に直す
//   ・パラメータは出力レベル 1 本だけ
//
// ノートの入力は 2 本。実機の MIDI IN A（パート 1-16）と B（パート 17-32）。
// 状態の保存の形は VST3 版の getState と同じにしてある。

#include "vst3/engine.h"
#include "vst3/plug_window.h"
#include "vst3/view.h"
#include "state.h"
#include "ui/snapshot.h"

#include "clap/clap.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

namespace {

using smu2000::vst3::engine;
using smu2000::vst3::plug_view;

// ---- このプラグインを表す名前。一度決めたら変えられない
//      （変えるとホストが別物とみなし、保存した曲から見つからなくなる）
constexpr const char *kPlugId = "io.github.tarboh.s-mu2000";

const char *const kFeatures[] = {
	CLAP_PLUGIN_FEATURE_INSTRUMENT,
	CLAP_PLUGIN_FEATURE_SYNTHESIZER,
	CLAP_PLUGIN_FEATURE_STEREO,
	nullptr,
};

const clap_plugin_descriptor_t kDescriptor = {
	CLAP_VERSION_INIT,
	kPlugId,
	"S-MU2000",
	"tarboh",
	"https://github.com/tarboh/S-MU2000",
	"https://github.com/tarboh/S-MU2000",
	"https://github.com/tarboh/S-MU2000/issues",
	"0.1.0",
	"Yamaha MU2000 emulation",
	kFeatures,
};

constexpr int kPorts = 2;
constexpr clap_id kGainId = 0;

#if defined(_WIN32)
constexpr const char *kWindowApi = CLAP_WINDOW_API_WIN32;
#else
constexpr const char *kWindowApi = CLAP_WINDOW_API_COCOA;
#endif

// 状態の保存は途中までしか書けない・読めないことがある。全部済むまで回す
bool write_all(const clap_ostream_t *s, const void *p, size_t n)
{
	const auto *b = static_cast<const uint8_t *>(p);
	while (n) {
		const int64_t w = s->write(s, b, n);
		if (w <= 0)
			return false;
		b += w;
		n -= size_t(w);
	}
	return true;
}

bool read_all(const clap_istream_t *s, void *p, size_t n)
{
	auto *b = static_cast<uint8_t *>(p);
	while (n) {
		const int64_t r = s->read(s, b, n);
		if (r <= 0)
			return false;
		b += r;
		n -= size_t(r);
	}
	return true;
}

// MIDI 1 メッセージの長さ（smu2000::vst3::midi_length）は VST3 版と同じものを使う
using smu2000::vst3::midi_length;


// ---- 本体

class mu_plugin
{
public:
	explicit mu_plugin(const clap_host_t *host) : m_host(host)
	{
		m_plugin.desc = &kDescriptor;
		m_plugin.plugin_data = this;
		m_plugin.init             = [](const clap_plugin *p) { return self(p)->init(); };
		m_plugin.destroy          = [](const clap_plugin *p) { delete self(p); };
		m_plugin.activate         = [](const clap_plugin *p, double rate, uint32_t, uint32_t) { return self(p)->activate(rate); };
		m_plugin.deactivate       = [](const clap_plugin *) {};
		m_plugin.start_processing = [](const clap_plugin *p) { return self(p)->start_processing(); };
		m_plugin.stop_processing  = [](const clap_plugin *p) { self(p)->stop_processing(); };
		m_plugin.reset            = [](const clap_plugin *p) { self(p)->m_hush.store(true); };
		m_plugin.process          = [](const clap_plugin *p, const clap_process_t *pr) { return self(p)->process(pr); };
		m_plugin.get_extension    = [](const clap_plugin *p, const char *id) { return self(p)->extension(id); };
		m_plugin.on_main_thread   = [](const clap_plugin *) {};
		m_engine.panel().set_gain(1.0f);
		m_engine.set_output_rate(smu2000::vst3::NATIVE_RATE);
	}

	~mu_plugin()
	{
		gui_destroy();
	}

	const clap_plugin_t *plugin() const { return &m_plugin; }

private:
	static mu_plugin *self(const clap_plugin *p) { return static_cast<mu_plugin *>(p->plugin_data); }

	// ---- clap_plugin

	bool init()
	{
		m_host_params = static_cast<const clap_host_params_t *>(
			m_host->get_extension(m_host, CLAP_EXT_PARAMS));
		// ROM 読みと起動（音にして 4 秒ぶんの空回し）は時間がかかるので、
		// ここでは走らせるだけ。終わるまでは無音を返す
		m_engine.start();
		return true;
	}

	bool activate(double rate)
	{
		m_rate = rate;
		m_engine.set_output_rate(rate);
		m_engine.start();
		return true;
	}

	bool start_processing()
	{
		// 動いているあいだ、機械に触れてよいのは音声スレッドだけ
		m_engine.set_processing(true);
		return true;
	}

	void stop_processing()
	{
		m_hush.store(true);
		m_engine.set_processing(false);
	}

	clap_process_status process(const clap_process_t *pr);

	// 音を作りながら 1 イベントを音源へ流す（出力レベルは流さない）
	void event(const clap_event_header_t *h);

	const void *extension(const char *id)
	{
		if (!std::strcmp(id, CLAP_EXT_AUDIO_PORTS)) return &s_audio_ports;
		if (!std::strcmp(id, CLAP_EXT_NOTE_PORTS))  return &s_note_ports;
		if (!std::strcmp(id, CLAP_EXT_PARAMS))      return &s_params;
		if (!std::strcmp(id, CLAP_EXT_STATE))       return &s_state;
		if (!std::strcmp(id, CLAP_EXT_LATENCY))     return &s_latency;
		if (!std::strcmp(id, CLAP_EXT_TAIL))        return &s_tail;
		if (!std::strcmp(id, CLAP_EXT_GUI))         return &s_gui;
		return nullptr;
	}

	// ---- 音の口。出力 1 つと、A/D INPUT（サンプリングで録る音）の入力 1 つ

	static const clap_plugin_audio_ports_t s_audio_ports;

	static bool audio_port(uint32_t index, bool is_input, clap_audio_port_info_t *info)
	{
		if (index != 0 || !info)
			return false;
		std::memset(info, 0, sizeof(*info));
		info->id            = is_input ? 1 : 0;
		info->flags         = is_input ? 0 : CLAP_AUDIO_PORT_IS_MAIN;
		info->channel_count = 2;
		info->port_type     = CLAP_PORT_STEREO;
		info->in_place_pair = CLAP_INVALID_ID;
		std::snprintf(info->name, sizeof(info->name), "%s", is_input ? "A/D Input" : "Stereo Out");
		return true;
	}

	// ---- ノートの口。MIDI IN A / B

	static const clap_plugin_note_ports_t s_note_ports;

	static bool note_port(uint32_t index, bool is_input, clap_note_port_info_t *info)
	{
		if (!is_input || index >= kPorts || !info)
			return false;
		std::memset(info, 0, sizeof(*info));
		info->id = index;
		info->supported_dialects = CLAP_NOTE_DIALECT_MIDI | CLAP_NOTE_DIALECT_CLAP;
		info->preferred_dialect  = CLAP_NOTE_DIALECT_MIDI;
		std::snprintf(info->name, sizeof(info->name), "%s",
		              index == 0 ? "MIDI In A (Part 1-16)" : "MIDI In B (Part 17-32)");
		return true;
	}

	// ---- パラメータ。出力レベルだけ（音源の外で掛ける素の掛け算）

	static const clap_plugin_params_t s_params;

	void params_flush(const clap_input_events_t *in)
	{
		if (!in)
			return;
		const uint32_t n = in->size(in);
		for (uint32_t i = 0; i < n; i++)
			gain_event(in->get(in, i));
	}

	// 出力レベルのイベントなら受け取って true
	bool gain_event(const clap_event_header_t *h)
	{
		if (!h || h->space_id != CLAP_CORE_EVENT_SPACE_ID || h->type != CLAP_EVENT_PARAM_VALUE)
			return false;
		const auto *e = reinterpret_cast<const clap_event_param_value_t *>(h);
		if (e->param_id == kGainId)
			m_engine.panel().set_gain(float(std::clamp(e->value, 0.0, 1.0)));
		return true;
	}

	// ---- 状態。VST3 版の getState / setState と同じ並び
	//
	//   版（3）、出力レベル、詰めた機械まるごとの長さと中身、SmartMedia のファイル名の長さと中身

	static const clap_plugin_state_t s_state;

	bool save(const clap_ostream_t *s)
	{
		const int32_t version = 3;
		const float gain = m_engine.panel().gain();
		m_engine.card_flush();   // プロジェクトを保存するときに、カードのファイルも揃える
		if (!write_all(s, &version, sizeof(version)) || !write_all(s, &gain, sizeof(gain)))
			return false;
		const std::vector<uint8_t> blob = m_engine.save_state();
		if (blob.empty())
			return true;                  // まだ起動中など
		const std::vector<u8> packed = state_pack(blob);
		const int32_t n = int32_t(packed.size());
		if (!write_all(s, &n, sizeof(n)) || !write_all(s, packed.data(), packed.size()))
			return false;
		// 中身はプロジェクトに入れない（16MB から 128MB あるので）
		const std::string card = m_engine.card_path();
		const int32_t len = int32_t(card.size());
		return write_all(s, &len, sizeof(len)) && (!len || write_all(s, card.data(), card.size()));
	}

	bool load(const clap_istream_t *s)
	{
		int32_t version = 0;
		float gain = 1.0f;
		if (!read_all(s, &version, sizeof(version)))
			return true;   // 空でも困らない
		if (read_all(s, &gain, sizeof(gain)) && gain >= 0.0f && gain <= 1.0f)
			m_engine.panel().set_gain(gain);
		if (m_host_params)
			m_host_params->rescan(m_host, CLAP_PARAM_RESCAN_VALUES);
		if (version < 2)
			return true;

		int32_t packed_size = 0;
		if (!read_all(s, &packed_size, sizeof(packed_size)) || packed_size <= 0 || packed_size > (64 << 20))
			return true;
		std::vector<uint8_t> packed(static_cast<size_t>(packed_size));
		if (!read_all(s, packed.data(), packed.size()))
			return true;
		std::vector<u8> blob;
		if (!state_unpack(packed.data(), packed.size(), blob))
			return true;

		// 起動が終わっていないと戻せない。終わるまで待つ
		for (int i = 0; i < 300 && m_engine.state() == smu2000::vst3::status::loading; i++)
			std::this_thread::sleep_for(std::chrono::milliseconds(10));
		m_engine.load_state(blob.data(), blob.size());

		// 版 3 から: 差していた SmartMedia のファイル（UTF-8）。無くなっていたら差さない
		if (version >= 3) {
			int32_t len = 0;
			if (read_all(s, &len, sizeof(len)) && len > 0 && len < 4096) {
				std::string path(size_t(len), '\0');
				if (read_all(s, path.data(), path.size())) {
					std::string err;
					if (!m_engine.card_insert(path, err))
						m_engine.log_line(("SmartMedia を差せない: " + err).c_str());
				}
			}
		} else if (!m_engine.card_path().empty()) {
			m_engine.card_eject();
		}
		return true;
	}

	// ---- 遅れと残響

	static const clap_plugin_latency_t s_latency;
	static const clap_plugin_tail_t s_tail;

	// ---- 画面。VST3 版の plug_view をそのまま、ホストの窓の中に入れる

	static const clap_plugin_gui_t s_gui;

	bool gui_create(const char *api, bool floating)
	{
		if (!api || std::strcmp(api, kWindowApi) || floating)
			return false;
		if (!m_view)
			m_view = new plug_view(m_engine);
		return true;
	}

	void gui_destroy()
	{
		if (!m_view)
			return;
		m_view->removed();
		m_view->release();
		m_view = nullptr;
	}

	bool gui_adjust(uint32_t *w, uint32_t *h)
	{
		if (!m_view || !w || !h)
			return false;
		Steinberg::ViewRect r(0, 0, Steinberg::int32(*w), Steinberg::int32(*h));
		m_view->checkSizeConstraint(&r);
		*w = uint32_t(r.getWidth());
		*h = uint32_t(r.getHeight());
		return true;
	}

	clap_plugin_t          m_plugin{};
	const clap_host_t     *m_host = nullptr;
	const clap_host_params_t *m_host_params = nullptr;
	engine                 m_engine;
	plug_view             *m_view = nullptr;
	double                 m_rate = smu2000::vst3::NATIVE_RATE;
	// 出力レベルは bridge が持つ。ここは 1 サンプルずつ寄せる途中の値
	float                  m_gain_now = 1.0f;
	std::atomic<bool>      m_hush{false};

	// 音を作る途中の入れ物。process の間だけ有効
	float       *m_left = nullptr, *m_right = nullptr;
	const float *m_in_l = nullptr, *m_in_r = nullptr;
	uint32_t     m_done = 0;

	void fill_to(uint32_t at)
	{
		if (at <= m_done)
			return;
		m_engine.fill(m_left + m_done, m_right + m_done, int(at - m_done),
		              m_in_l ? m_in_l + m_done : nullptr, m_in_r ? m_in_r + m_done : nullptr);
		m_done = at;
	}
};


const clap_plugin_audio_ports_t mu_plugin::s_audio_ports = {
	[](const clap_plugin_t *, bool) -> uint32_t { return 1; },
	[](const clap_plugin_t *, uint32_t index, bool is_input, clap_audio_port_info_t *info) {
		return audio_port(index, is_input, info);
	},
};

const clap_plugin_note_ports_t mu_plugin::s_note_ports = {
	[](const clap_plugin_t *, bool is_input) -> uint32_t { return is_input ? kPorts : 0; },
	[](const clap_plugin_t *, uint32_t index, bool is_input, clap_note_port_info_t *info) {
		return note_port(index, is_input, info);
	},
};

const clap_plugin_params_t mu_plugin::s_params = {
	[](const clap_plugin_t *) -> uint32_t { return 1; },
	[](const clap_plugin_t *, uint32_t index, clap_param_info_t *info) {
		if (index != 0 || !info)
			return false;
		std::memset(info, 0, sizeof(*info));
		info->id            = kGainId;
		info->flags         = CLAP_PARAM_IS_AUTOMATABLE;
		info->min_value     = 0.0;
		info->max_value     = 1.0;
		info->default_value = 1.0;
		std::snprintf(info->name, sizeof(info->name), "Output");
		return true;
	},
	[](const clap_plugin_t *p, clap_id id, double *out) {
		if (id != kGainId || !out)
			return false;
		*out = self(p)->m_engine.panel().gain();
		return true;
	},
	[](const clap_plugin_t *, clap_id id, double v, char *buf, uint32_t cap) {
		if (id != kGainId || !buf || !cap)
			return false;
		std::snprintf(buf, cap, "%.0f %%", v * 100.0);
		return true;
	},
	[](const clap_plugin_t *, clap_id id, const char *text, double *out) {
		if (id != kGainId || !text || !out)
			return false;
		*out = std::clamp(std::atof(text) / 100.0, 0.0, 1.0);
		return true;
	},
	[](const clap_plugin_t *p, const clap_input_events_t *in, const clap_output_events_t *) {
		self(p)->params_flush(in);
	},
};

const clap_plugin_state_t mu_plugin::s_state = {
	[](const clap_plugin_t *p, const clap_ostream_t *s) { return s && self(p)->save(s); },
	[](const clap_plugin_t *p, const clap_istream_t *s) { return s && self(p)->load(s); },
};

const clap_plugin_latency_t mu_plugin::s_latency = {
	[](const clap_plugin_t *p) -> uint32_t { return self(p)->m_engine.latency_samples(); },
};

// 残響がある。4 秒みておく
const clap_plugin_tail_t mu_plugin::s_tail = {
	[](const clap_plugin_t *p) -> uint32_t { return uint32_t(self(p)->m_rate * 4.0); },
};

const clap_plugin_gui_t mu_plugin::s_gui = {
	// is_api_supported
	[](const clap_plugin_t *, const char *api, bool floating) {
		return api && !std::strcmp(api, kWindowApi) && !floating;
	},
	// get_preferred_api
	[](const clap_plugin_t *, const char **api, bool *floating) {
		if (api) *api = kWindowApi;
		if (floating) *floating = false;
		return true;
	},
	// create / destroy
	[](const clap_plugin_t *p, const char *api, bool floating) { return self(p)->gui_create(api, floating); },
	[](const clap_plugin_t *p) { self(p)->gui_destroy(); },
	// set_scale: 大きさはホストの窓の大きさで決まる（パネルは描くときに合わせる）
	[](const clap_plugin_t *, double) { return false; },
	// get_size
	[](const clap_plugin_t *p, uint32_t *w, uint32_t *h) {
		plug_view *v = self(p)->m_view;
		if (!v || !w || !h)
			return false;
		*w = uint32_t(v->width());
		*h = uint32_t(v->height());
		return true;
	},
	// can_resize
	[](const clap_plugin_t *) { return true; },
	// get_resize_hints。横に長い機械なので、縦横比はこちらで決める
	[](const clap_plugin_t *, clap_gui_resize_hints_t *hints) {
		if (!hints)
			return false;
		hints->can_resize_horizontally = true;
		hints->can_resize_vertically   = true;
		hints->preserve_aspect_ratio   = true;
		hints->aspect_ratio_width      = uint32_t(ui::LOGICAL_W);
		hints->aspect_ratio_height     = uint32_t(ui::LOGICAL_H);
		return true;
	},
	// adjust_size
	[](const clap_plugin_t *p, uint32_t *w, uint32_t *h) { return self(p)->gui_adjust(w, h); },
	// set_size
	[](const clap_plugin_t *p, uint32_t w, uint32_t h) {
		plug_view *v = self(p)->m_view;
		if (!v)
			return false;
		Steinberg::ViewRect r(0, 0, Steinberg::int32(w), Steinberg::int32(h));
		return v->onSize(&r) == Steinberg::kResultOk;
	},
	// set_parent
	[](const clap_plugin_t *p, const clap_window_t *win) {
		plug_view *v = self(p)->m_view;
		if (!v || !win || !win->api || std::strcmp(win->api, kWindowApi))
			return false;
		return v->attached(win->ptr, smu2000::vst3::plug_window_type()) == Steinberg::kResultOk;
	},
	// set_transient: 浮いた窓は出さない
	[](const clap_plugin_t *, const clap_window_t *) { return false; },
	// suggest_title
	[](const clap_plugin_t *, const char *) {},
	// show / hide: 子窓はホストの窓と一緒に見え隠れする
	[](const clap_plugin_t *) { return true; },
	[](const clap_plugin_t *) { return true; },
};


clap_process_status mu_plugin::process(const clap_process_t *pr)
{
	const uint32_t n = pr->frames_count;
	clap_audio_buffer_t *out = pr->audio_outputs_count > 0 ? &pr->audio_outputs[0] : nullptr;
	// 64bit 浮動小数は受けないと答えてある。それでも来たら音を出さない
	if (!out || !out->data32 || out->channel_count < 1) {
		params_flush(pr->in_events);
		return CLAP_PROCESS_CONTINUE;
	}
	m_left  = out->data32[0];
	m_right = out->channel_count > 1 ? out->data32[1] : m_left;
	// A/D INPUT。繋がっていなければ無し
	const clap_audio_buffer_t *in = pr->audio_inputs_count > 0 ? &pr->audio_inputs[0] : nullptr;
	m_in_l = (in && in->data32 && in->channel_count > 0) ? in->data32[0] : nullptr;
	m_in_r = (in && in->data32 && in->channel_count > 1) ? in->data32[1] : m_in_l;
	m_done = 0;

	if (m_hush.exchange(false))
		m_engine.all_notes_off();

	// イベントは時刻順に来る。その時刻まで音を作ってから流す
	if (const clap_input_events_t *ev = pr->in_events) {
		const uint32_t count = ev->size(ev);
		for (uint32_t i = 0; i < count; i++) {
			const clap_event_header_t *h = ev->get(ev, i);
			if (!h || h->space_id != CLAP_CORE_EVENT_SPACE_ID)
				continue;
			fill_to(std::min(h->time, n));
			event(h);
		}
	}
	fill_to(n);

	// 出力レベル。一気に変えると音が跳ねるので 1 サンプルずつ寄せる
	const float target = m_engine.panel().gain();
	if (target != m_gain_now || target != 1.0f) {
		const float step = 1.0f / 512.0f;
		for (uint32_t i = 0; i < n; i++) {
			if (m_gain_now < target) m_gain_now = std::min(target, m_gain_now + step);
			else if (m_gain_now > target) m_gain_now = std::max(target, m_gain_now - step);
			m_left[i] *= m_gain_now;
			if (m_right != m_left)
				m_right[i] *= m_gain_now;
		}
	}
	for (uint32_t c = 2; c < out->channel_count; c++)
		std::memset(out->data32[c], 0, n * sizeof(float));
	out->constant_mask = 0;

	m_left = m_right = nullptr;
	m_in_l = m_in_r = nullptr;
	return CLAP_PROCESS_CONTINUE;
}

void mu_plugin::event(const clap_event_header_t *h)
{
	if (gain_event(h))
		return;
	switch (h->type) {
	case CLAP_EVENT_MIDI: {
		const auto *e = reinterpret_cast<const clap_event_midi_t *>(h);
		m_engine.midi(e->data, size_t(midi_length(e->data[0])), e->port_index == 1 ? 1 : 0);
		break;
	}
	case CLAP_EVENT_MIDI_SYSEX: {
		const auto *e = reinterpret_cast<const clap_event_midi_sysex_t *>(h);
		if (e->buffer && e->size)
			m_engine.midi(e->buffer, e->size, e->port_index == 1 ? 1 : 0);
		break;
	}
	// CLAP 流のノート。チャンネルやキーが「どれでも」（-1）なら MIDI にできないので捨てる
	case CLAP_EVENT_NOTE_ON:
	case CLAP_EVENT_NOTE_OFF:
	case CLAP_EVENT_NOTE_CHOKE: {
		const auto *e = reinterpret_cast<const clap_event_note_t *>(h);
		if (e->channel < 0 || e->channel > 15 || e->key < 0 || e->key > 127)
			break;
		const bool on = h->type == CLAP_EVENT_NOTE_ON;
		int v = int(std::lround(e->velocity * 127.0));
		v = on ? std::clamp(v, 1, 127) : std::clamp(v, 0, 127);
		const uint8_t msg[3] = { uint8_t((on ? 0x90 : 0x80) | e->channel), uint8_t(e->key), uint8_t(v) };
		m_engine.midi(msg, 3, e->port_index == 1 ? 1 : 0);
		break;
	}
	default:
		break;
	}
}


// ---- 工場

const clap_plugin_factory_t g_factory = {
	[](const clap_plugin_factory *) -> uint32_t { return 1; },
	[](const clap_plugin_factory *, uint32_t index) -> const clap_plugin_descriptor_t * {
		return index == 0 ? &kDescriptor : nullptr;
	},
	[](const clap_plugin_factory *, const clap_host_t *host, const char *id) -> const clap_plugin_t * {
		if (!host || !id || std::strcmp(id, kPlugId) || !clap_version_is_compatible(host->clap_version))
			return nullptr;
		return (new mu_plugin(host))->plugin();
	},
};

} // namespace


// ---- DLL の出口。ホストはまずこれを取りに来る

namespace {

bool entry_init(const char *) { return true; }
void entry_deinit() {}
const void *entry_factory(const char *id)
{
	return (id && !std::strcmp(id, CLAP_PLUGIN_FACTORY_ID)) ? &g_factory : nullptr;
}

} // namespace

extern "C" {

CLAP_EXPORT extern const clap_plugin_entry_t clap_entry = {
	CLAP_VERSION_INIT,
	entry_init,
	entry_deinit,
	entry_factory,
};

}
