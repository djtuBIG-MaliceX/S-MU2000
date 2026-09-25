// license:BSD-3-Clause
//
// WebMIDI input for the browser build (issue: WASM). The page owns the
// dropdowns and the WebMIDI subscriptions; when a device sends a message
// JS calls smu_midi_push (js_wasm.cpp) and this file drops the bytes into
// the same lock-free ring engine::fill() drains on every other platform.
//
// There is no receive thread: WebMIDI callbacks already run on the main
// thread, and "the audio thread" here is the frame/rpc callback itself,
// so push (JS event) and pop (fill) never fight over the ring -- but the
// atomics stay, so the code is the Linux one byte for byte.

#include "midi_in.h"
#include "wasm_glue.h"

#include <mutex>

namespace ui {

namespace {

// What the page last put in the dropdowns. list() answers from this so a
// menu pick and a dropdown selection name the same cable.
std::mutex g_names_lock;
std::vector<std::string> g_in_ports;

} // namespace

void wasm_set_midi_in_ports(std::vector<std::string> names)
{
	std::lock_guard<std::mutex> hold(g_names_lock);
	g_in_ports = std::move(names);
}

std::vector<std::string> wasm_midi_in_ports()
{
	std::lock_guard<std::mutex> hold(g_names_lock);
	return g_in_ports;
}

// The WebMIDI device itself lives in JS; keep the handle small.
struct midi_in::ctx
{
	int index = -1;
};

std::vector<std::string> midi_in::list()
{
	return wasm_midi_in_ports();
}

bool midi_in::open(int device, std::string &err)
{
	close();
	if (device < 0)
		return true;

	const std::vector<std::string> ports = wasm_midi_in_ports();
	if (size_t(device) >= ports.size()) {
		err = "その番号の MIDI 入力は無い";
		return false;
	}

	m_ctx  = new ctx{ device };
	m_name = ports[size_t(device)];
	m_closing.store(false, std::memory_order_release);
	return true;
}

void midi_in::close()
{
	if (!m_ctx)
		return;
	m_closing.store(true, std::memory_order_release);
	delete m_ctx;
	m_ctx = nullptr;
	m_name.clear();
}

void midi_in::push(u8 v)
{
	const size_t w = m_write.load(std::memory_order_relaxed);
	const size_t next = (w + 1) & MASK;
	if (next == m_read.load(std::memory_order_acquire))
		return;                       // 満杯:実機の受信リングと同じく捨てる
	m_buf[w] = v;
	m_write.store(next, std::memory_order_release);
	m_bytes.fetch_add(1, std::memory_order_relaxed);
}

bool midi_in::pop(u8 &v)
{
	const size_t r = m_read.load(std::memory_order_relaxed);
	if (r == m_write.load(std::memory_order_acquire))
		return false;
	v = m_buf[r];
	m_read.store((r + 1) & MASK, std::memory_order_release);
	return true;
}

} // namespace ui
