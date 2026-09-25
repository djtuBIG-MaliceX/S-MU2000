// license:BSD-3-Clause
//
// WebMIDI output for the browser build. Same contract as the WinMM /
// CoreMIDI / ALSA back-ends: the engine hands over THRU and MIDI OUT
// bytes one at a time, messages get assembled, whole messages go out.
//
// On the desktop platforms an extra sender thread exists because opening
// or sending on the audio thread can block. In the browser there is no
// second thread and MIDIManager post can't block us (it queues in JS),
// so send() assembles inline and hands finished messages to js_wasm.cpp,
// which routes them to whatever the page's output dropdown picked.

#include "midi_out.h"
#include "wasm_glue.h"

#include <mutex>

namespace ui {

namespace {

std::mutex g_names_lock;
std::vector<std::string> g_out_ports;

int msg_len(u8 status)
{
	const u8 kind = status & 0xf0;
	if (kind == 0xc0 || kind == 0xd0)
		return 2;
	if (status == 0xf1 || status == 0xf3)
		return 2;
	if (status == 0xf2)
		return 3;
	if (status >= 0xf4 && status < 0xf8)
		return 1;
	return 3;
}

} // namespace

void wasm_set_midi_out_ports(std::vector<std::string> names)
{
	std::lock_guard<std::mutex> hold(g_names_lock);
	g_out_ports = std::move(names);
}

std::vector<std::string> wasm_midi_out_ports()
{
	std::lock_guard<std::mutex> hold(g_names_lock);
	return g_out_ports;
}

struct midi_out::ctx
{
	int index = -1;
};

midi_out::~midi_out()
{
	close();
}

std::vector<std::string> midi_out::list()
{
	return wasm_midi_out_ports();
}

bool midi_out::open(int device, std::string &err)
{
	close();
	if (device < 0)
		return true;

	const std::vector<std::string> ports = wasm_midi_out_ports();
	if (size_t(device) >= ports.size()) {
		err = "その番号の MIDI 出力は無い";
		return false;
	}

	m_ctx  = new ctx{ device };
	m_name = ports[size_t(device)];
	m_open.store(true, std::memory_order_release);
	m_quit.store(false);
	return true;
}

void midi_out::close()
{
	if (!m_ctx)
		return;
	m_open.store(false, std::memory_order_release);
	delete m_ctx;
	m_ctx = nullptr;
	m_name.clear();
	// Drop a half-assembled message so a reopen starts clean
	m_have = m_want = 0;
	m_status = 0;
	m_in_sysex = false;
	m_sysex.clear();
}

void midi_out::send(u8 v)
{
	if (!m_open.load(std::memory_order_acquire))
		return;

	// Message assembly, verbatim from the sender thread on the other
	// platforms. There is only this thread here, so the scratch state
	// needs nothing beyond what the header already has.
	if (v >= 0xf8) {                    // real-time: one byte, even mid-SysEx
		const u8 one[1] = { v };
		emit(one, 1);
		return;
	}
	if (v == 0xf0) {
		m_in_sysex = true;
		m_sysex.clear();
		m_sysex.push_back(v);
		return;
	}
	if (m_in_sysex) {
		if (m_sysex.size() < 65536)
			m_sysex.push_back(v);
		if (v == 0xf7) {
			m_in_sysex = false;
			emit(m_sysex.data(), m_sysex.size());
		}
		return;
	}
	if (v & 0x80) {
		m_status = v;
		m_have   = 0;
		m_want   = msg_len(v) - 1;
		m_msg[0] = v;
		if (!m_want)
			emit(m_msg, 1);
		return;
	}
	// Data byte: running status keeps the message going.
	if (!m_want && m_status && (m_status < 0xf0)) {
		m_msg[0] = m_status;
		m_have   = 0;
		m_want   = msg_len(m_status) - 1;
	}
	if (m_want > 0 && m_have < 2) {
		m_msg[++m_have] = v;
		if (m_have >= m_want) {
			emit(m_msg, size_t(m_have) + 1);
			m_want = (m_status >= 0x80 && m_status < 0xf0)
			             ? msg_len(m_status) - 1 : 0;
			m_have = 0;
		}
	}
}

void midi_out::emit(const u8 *p, size_t n)
{
	if (!p || !n)
		return;
	wasm_midi_out_bytes(p, n);          // js_wasm.cpp -> Module.onMidiOutBytes
}

void midi_out::run()
{
	// No sender thread in the browser; declared only because the header
	// (shared with the desktop back-ends) names it.
}

} // namespace ui
