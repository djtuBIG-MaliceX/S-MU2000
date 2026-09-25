// license:BSD-3-Clause
//
// Browser audio output (Emscripten). There is no device to open: the page
// runs a ScriptProcessor (js side) that asks the wasm for blocks while it
// needs them, via smu_audio_render (js_wasm.cpp -> wasm_audio_pull here).
//
// Same contract as the CoreAudio / ALSA back-ends, same shape (a pimpl):
// start() keeps the fill function and hands it over; every frame of 44100
// rate the machine advances exactly by what was asked (doc/design.md --
// no clock of our own).

#include "audio_out.h"
#include "wasm_glue.h"

#include <cstring>
#include <mutex>

namespace ui {

namespace {

std::mutex g_active_lock;
audio_out *g_active = nullptr;         // the one running instance

// Registered by audio_out::start(), cleared by stop(). Kept here rather
// than on the instance so the export can find it while main() runs the
// frame loop with a pointer that might move.
std::function<void(s16 *, u32)> g_fill;
std::string g_fill_name;

// Capture lives at file scope too: wasm_audio_pull is a plain function
// and the class keeps its capture members for the desktop back-ends.
// One instance runs in the browser, so one capture buffer is enough.
std::vector<s16> g_cap;
bool g_cap_on = false;
u64 g_produced = 0;                    // frames handed to the page

} // namespace

struct audio_out::impl
{
	u64 produced = 0;
	u64 starved  = 0;      // pulled while nothing was registered
};

audio_out::audio_out() : m_impl(new impl()) {}
audio_out::~audio_out() { stop(); }

std::vector<std::string> audio_out::list()
{
	// The browser always has exactly this "device"; the page decides
	// where it goes (destination / AudioContext), the menu only confirms
	// there is something to open.
	return { "Browser audio output" };
}

bool audio_out::start(int latency_ms, fill_fn fill, std::string &err,
                      bool exclusive, const std::string &device, bool raw)
{
	(void)latency_ms; (void)exclusive; (void)device; (void)raw;
	stop();
	if (!fill) {
		err = "fill が無い";
		return false;
	}
	{
		std::lock_guard<std::mutex> hold(g_active_lock);
		g_fill = std::move(fill);
		g_fill_name = "Browser audio output";
		g_active = this;
	}
	return true;
}

std::string audio_out::device_name() const { return "Browser audio output"; }

bool audio_out::exclusive() const { return false; }

void audio_out::set_capture(const std::string &path)
{
	std::lock_guard<std::mutex> hold(g_active_lock);
	m_cap_path = path;
	g_cap.clear();
	g_cap_on = !path.empty();
}

u64 audio_out::capture_frames() const
{
	std::lock_guard<std::mutex> hold(g_active_lock);
	return g_cap.size() / 2;
}

bool audio_out::write_capture(std::string &err)
{
	std::lock_guard<std::mutex> hold(g_active_lock);
	if (!g_cap_on || m_cap_path.empty()) {
		err = "記録していない";
		return false;
	}
	FILE *f = std::fopen(m_cap_path.c_str(), "wb");
	if (!f) {
		err = m_cap_path;
		return false;
	}
	const u32 frames = u32(g_cap.size() / 2);
	const u32 bytes  = frames * 4;
	const u32 rate   = AUDIO_RATE;
	const auto put32 = [&](u32 v) { std::fwrite(&v, 1, 4, f); };
	const auto put16 = [&](u16 v) { std::fwrite(&v, 1, 2, f); };
	std::fwrite("RIFF", 1, 4, f);
	put32(36 + bytes);
	std::fwrite("WAVEfmt ", 1, 4, f);
	put32(16);
	put16(1);                       // PCM
	put16(2);                       // stereo
	put32(rate);
	put32(rate * 4);
	put16(4);
	put16(16);
	std::fwrite("data", 1, 4, f);
	put32(bytes);
	std::fwrite(g_cap.data(), 2, g_cap.size(), f);
	std::fclose(f);
	g_cap.clear();
	g_cap_on = false;
	(void)err;
	return true;
}

void audio_out::stop()
{
	std::lock_guard<std::mutex> hold(g_active_lock);
	if (g_active == this) {
		g_active = nullptr;
		g_fill = nullptr;
		g_fill_name.clear();
	}
}

// ---- called from js_wasm.cpp's smu_audio_render -------------------------

bool wasm_audio_pull(s16 *out, u32 frames)
{
	std::function<void(s16 *, u32)> fill;
	audio_out *ao = nullptr;
	{
		std::lock_guard<std::mutex> hold(g_active_lock);
		fill = g_fill;
		ao = g_active;
	}
	if (!fill)
		return false;
	// fill runs unlocked: it is the engine, and it may reach start()/stop()
	fill(out, frames);
	if (ao) {
		std::lock_guard<std::mutex> hold(g_active_lock);
		if (g_cap_on && g_active == ao)
			g_cap.insert(g_cap.end(), out, out + size_t(frames) * 2);
		g_produced += frames;
	}
	return true;
}

void wasm_audio_unregister()
{
	std::lock_guard<std::mutex> hold(g_active_lock);
	g_active = nullptr;
	g_fill = nullptr;
}

// ---- progress numbers ----------------------------------------------------

u32 audio_out::buffer_frames() const { return 0; }   // no queue: pull-through
u64 audio_out::produced() const { return g_produced; }
u64 audio_out::starved() const { return 0; }
bool audio_out::mmcss() const { return false; }      // the OS thread class
double audio_out::cpu_percent() const { return 0.0; } // the page can measure
double audio_out::worst_ms() const { return 0.0; }

} // namespace ui
