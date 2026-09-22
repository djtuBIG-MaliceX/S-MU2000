// license:BSD-3-Clause
//
// ALSA sequencer output for Linux. Same interface as midi_out.cpp (WinMM) and
// midi_out_mac.cpp (CoreMIDI): the synth's THRU path.
//
// The audio thread must never wait (doc/design.md), so it only drops bytes
// into a lock-free ring; a sender thread assembles messages and hands them to
// the sequencer. The wake-up is a condition variable, like the CoreMIDI side.

#include "midi_out.h"

#include <alsa/asoundlib.h>

#include <algorithm>
#include <condition_variable>
#include <mutex>

namespace ui {

struct midi_out::ctx {
	snd_seq_t *seq = nullptr;
	int        port = -1;
	int        dst_client = -1, dst_port = -1;
};

namespace {

struct seq_port {
	int         client = -1, port = -1;
	std::string name;
};

bool is_writable(snd_seq_port_info_t *pinfo)
{
	const unsigned caps = snd_seq_port_info_get_capability(pinfo);
	return (caps & (SND_SEQ_PORT_CAP_WRITE | SND_SEQ_PORT_CAP_SUBS_WRITE)) != 0;
}

std::vector<seq_port> writable_ports(snd_seq_t *seq)
{
	std::vector<seq_port> out;
	snd_seq_client_info_t *cinfo = nullptr;
	snd_seq_client_info_alloca(&cinfo);
	snd_seq_port_info_t *pinfo = nullptr;
	snd_seq_port_info_alloca(&pinfo);
	snd_seq_client_info_set_client(cinfo, -1);
	while (snd_seq_query_next_client(seq, cinfo) == 0) {
		const int client = snd_seq_client_info_get_client(cinfo);
		snd_seq_port_info_set_client(pinfo, client);
		snd_seq_port_info_set_port(pinfo, -1);
		while (snd_seq_query_next_port(seq, pinfo) == 0) {
			if (!is_writable(pinfo))
				continue;
			seq_port p;
			p.client = client;
			p.port   = snd_seq_port_info_get_port(pinfo);
			p.name   = snd_seq_port_info_get_name(pinfo);
			out.push_back(p);
		}
	}
	return out;
}

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

midi_out::~midi_out()
{
	close();
}

std::vector<std::string> midi_out::list()
{
	std::vector<std::string> out;
	snd_seq_t *seq = nullptr;
	if (snd_seq_open(&seq, "default", SND_SEQ_OPEN_DUPLEX, SND_SEQ_NONBLOCK) < 0)
		return out;
	snd_seq_set_client_name(seq, "S-MU2000-list");
	for (const seq_port &p : writable_ports(seq)) {
		char head[32];
		std::snprintf(head, sizeof(head), "%d:%d ", p.client, p.port);
		out.push_back(head + p.name);
	}
	snd_seq_close(seq);
	return out;
}

bool midi_out::open(int device, std::string &err)
{
	close();
	if (device < 0)
		return true;

	snd_seq_t *seq = nullptr;
	if (snd_seq_open(&seq, "default", SND_SEQ_OPEN_DUPLEX, 0) < 0) {
		err = "MIDI シーケンサを開けない";
		return false;
	}
	snd_seq_set_client_name(seq, "S-MU2000");

	const std::vector<seq_port> ports = writable_ports(seq);
	if (device >= int(ports.size())) {
		snd_seq_close(seq);
		err = "その番号の MIDI 出力は無い";
		return false;
	}

	auto *c = new ctx();
	c->seq        = seq;
	c->dst_client = ports[size_t(device)].client;
	c->dst_port   = ports[size_t(device)].port;
	c->port = snd_seq_create_simple_port(
		seq, "out", SND_SEQ_PORT_CAP_READ | SND_SEQ_PORT_CAP_SUBS_READ,
		SND_SEQ_PORT_TYPE_MIDI_GENERIC | SND_SEQ_PORT_TYPE_APPLICATION);
	if (c->port < 0) {
		snd_seq_close(seq);
		delete c;
		err = "MIDI 出力を開けない";
		return false;
	}

	// Subscribe our readable port to the destination's writable one.
	snd_seq_addr_t sender{}, dest{};
	sender.client = snd_seq_client_id(seq);
	sender.port   = c->port;
	dest.client   = c->dst_client;
	dest.port     = c->dst_port;
	snd_seq_port_subscribe_t *subs = nullptr;
	snd_seq_port_subscribe_alloca(&subs);
	snd_seq_port_subscribe_set_sender(subs, &sender);
	snd_seq_port_subscribe_set_dest(subs, &dest);
	if (snd_seq_subscribe_port(seq, subs) < 0) {
		snd_seq_delete_simple_port(seq, c->port);
		snd_seq_close(seq);
		delete c;
		err = "MIDI 出力に繋げない";
		return false;
	}

	char head[32];
	std::snprintf(head, sizeof(head), "%d:%d ", c->dst_client, c->dst_port);
	m_name = head + ports[size_t(device)].name;
	m_ctx  = c;
	m_open.store(true, std::memory_order_release);
	m_quit.store(false);
	m_thread = std::thread([this] { run(); });
	return true;
}

void midi_out::close()
{
	if (!m_ctx)
		return;
	m_quit.store(true);
	m_wake.notify_all();
	if (m_thread.joinable())
		m_thread.join();
	m_open.store(false, std::memory_order_release);
	if (m_ctx->seq) {
		if (m_ctx->port >= 0)
			snd_seq_delete_simple_port(m_ctx->seq, m_ctx->port);
		snd_seq_close(m_ctx->seq);
	}
	delete m_ctx;
	m_ctx = nullptr;
	m_name.clear();
}

void midi_out::send(u8 v)
{
	if (!m_open.load(std::memory_order_acquire))
		return;
	const size_t w = m_write.load(std::memory_order_relaxed);
	const size_t next = (w + 1) & MASK;
	if (next == m_read.load(std::memory_order_acquire))
		return;   // full: drop, like the machine's TX buffer
	m_buf[w] = v;
	m_write.store(next, std::memory_order_release);
	m_wake.notify_one();
}

void midi_out::emit(const u8 *p, size_t n)
{
	if (!m_ctx || !m_ctx->seq || !p || !n)
		return;
	snd_seq_event_t ev{};
	snd_seq_ev_clear(&ev);
	snd_seq_ev_set_source(&ev, m_ctx->port);
	snd_seq_ev_set_dest(&ev, m_ctx->dst_client, m_ctx->dst_port);
	const u8 st = p[0];
	const auto byte = [&](size_t i) { return i < n ? p[i] : u8(0); };
	switch (st) {
	case 0xf0:   // SysEx, F0..F7 in one message
		snd_seq_ev_set_sysex(&ev, n, const_cast<u8 *>(p));
		break;
	case 0xf1:
		snd_seq_ev_set_fixed(&ev);
		ev.type = SND_SEQ_EVENT_QFRAME;
		ev.data.control.value = byte(1);
		break;
	case 0xf2: {
		const int v = (byte(1) & 0x7f) | ((byte(2) & 0x7f) << 7);
		snd_seq_ev_set_fixed(&ev);
		ev.type = SND_SEQ_EVENT_SONGPOS;
		ev.data.control.value = v;
		break;
	}
	case 0xf3:
		snd_seq_ev_set_fixed(&ev);
		ev.type = SND_SEQ_EVENT_SONGSEL;
		ev.data.control.value = byte(1);
		break;
	case 0xf6:
		ev.type = SND_SEQ_EVENT_TUNE_REQUEST;
		break;
	case 0xf8: ev.type = SND_SEQ_EVENT_CLOCK; break;
	case 0xfa: ev.type = SND_SEQ_EVENT_START; break;
	case 0xfb: ev.type = SND_SEQ_EVENT_CONTINUE; break;
	case 0xfc: ev.type = SND_SEQ_EVENT_STOP; break;
	case 0xfe: ev.type = SND_SEQ_EVENT_SENSING; break;
	case 0xff: ev.type = SND_SEQ_EVENT_RESET; break;
	default: {
		const u8 kind = st & 0xf0, ch = st & 0x0f;
		switch (kind) {
		case 0x80: snd_seq_ev_set_noteoff(&ev, ch, byte(1), byte(2)); break;
		case 0x90: snd_seq_ev_set_noteon(&ev, ch, byte(1), byte(2)); break;
		case 0xa0: snd_seq_ev_set_keypress(&ev, ch, byte(1), byte(2)); break;
		case 0xb0: snd_seq_ev_set_controller(&ev, ch, byte(1), byte(2)); break;
		case 0xc0: snd_seq_ev_set_pgmchange(&ev, ch, byte(1)); break;
		case 0xd0: snd_seq_ev_set_chanpress(&ev, ch, byte(1)); break;
		case 0xe0: {
			const int v = ((byte(1) & 0x7f) | ((byte(2) & 0x7f) << 7)) - 8192;
			snd_seq_ev_set_pitchbend(&ev, ch, v);
			break;
		}
		default: return;   // F4/F5/F7-alone and friends: nowhere to send
		}
		break;
	}
	}
	snd_seq_event_output(m_ctx->seq, &ev);
	snd_seq_drain_output(m_ctx->seq);
}

void midi_out::run()
{
	// Assembles one message from the byte stream. Only this thread touches
	// m_msg / m_have / m_want / m_status / m_in_sysex / m_sysex.
	while (!m_quit.load()) {
		// Wait for bytes.
		{
			std::unique_lock<std::mutex> lk(m_wake_mutex);
			m_wake.wait(lk, [this] {
				return m_quit.load() ||
				       m_read.load(std::memory_order_acquire) !=
				           m_write.load(std::memory_order_acquire);
			});
			if (m_quit.load())
				break;
		}
		size_t r = m_read.load(std::memory_order_relaxed);
		while (r != m_write.load(std::memory_order_acquire)) {
			const u8 v = m_buf[r];
			r = (r + 1) & MASK;
			m_read.store(r, std::memory_order_release);

			if (v >= 0xf8) {
				// Real-time: single byte, even inside SysEx.
				u8 one[1] = { v };
				emit(one, 1);
				continue;
			}
			if (v == 0xf0) {
				m_in_sysex = true;
				m_sysex.clear();
				m_sysex.push_back(v);
				continue;
			}
			if (m_in_sysex) {
				if (m_sysex.size() < 65536)
					m_sysex.push_back(v);
				if (v == 0xf7) {
					m_in_sysex = false;
					emit(m_sysex.data(), m_sysex.size());
				}
				continue;
			}
			if (v & 0x80) {
				m_status = v;
				m_have   = 0;
				m_want   = msg_len(v) - 1;
				m_msg[0] = v;
				if (!m_want)
					emit(m_msg, 1);
				continue;
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
	}
}

} // namespace ui
