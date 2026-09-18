// license:BSD-3-Clause
//
// CoreMIDI output for macOS. Same interface as midi_out.cpp (WinMM), and the
// same split: the audio thread only drops bytes into a ring, and a second
// thread does the actual sending.
//
// That split matters more here than on Windows. The audio thread has to hand a
// byte over without ever blocking, and CoreMIDI's send path can take a lock, so
// calling MIDISend from the render callback would put an unpredictable wait
// inside the audio callback.

#include "midi_out.h"

#include <CoreFoundation/CoreFoundation.h>
#include <CoreMIDI/CoreMIDI.h>

#include <cstring>
#include <string>
#include <vector>

namespace ui {

struct midi_out::ctx
{
	MIDIClientRef   client = 0;
	MIDIPortRef     port   = 0;
	MIDIEndpointRef dest   = 0;
};

namespace {

std::string cf_to_utf8(CFStringRef s)
{
	if (!s)
		return {};
	const CFIndex len = CFStringGetLength(s);
	const CFIndex max = CFStringGetMaximumSizeForEncoding(len, kCFStringEncodingUTF8) + 1;
	std::string out(size_t(max), '\0');
	if (!CFStringGetCString(s, out.data(), max, kCFStringEncodingUTF8))
		return {};
	out.resize(std::strlen(out.c_str()));   // drop the NUL we reserved
	return out;
}

std::string endpoint_name(MIDIEndpointRef ep)
{
	CFStringRef name = nullptr;
	if (MIDIObjectGetStringProperty(ep, kMIDIPropertyDisplayName, &name) != noErr)
		MIDIObjectGetStringProperty(ep, kMIDIPropertyName, &name);
	std::string out = cf_to_utf8(name);
	if (name)
		CFRelease(name);
	return out.empty() ? std::string("?") : out;
}

// How many data bytes follow a given status byte
int message_length(u8 status)
{
	if (status >= 0xf8) return 1;                    // real-time
	switch (status) {
	case 0xf1: case 0xf3: case 0xf5: return 2;   // F5 nn: cable message
	case 0xf2:            return 3;
	case 0xf4: case 0xf6: case 0xf7: return 1;
	default: break;
	}
	const u8 kind = status & 0xf0;
	return (kind == 0xc0 || kind == 0xd0) ? 2 : 3;
}

} // namespace


std::vector<std::string> midi_out::list()
{
	std::vector<std::string> out;
	const ItemCount n = MIDIGetNumberOfDestinations();
	for (ItemCount i = 0; i < n; i++)
		out.push_back(endpoint_name(MIDIGetDestination(i)));
	return out;
}

bool midi_out::open(int device, std::string &err)
{
	close();
	if (device < 0)
		return true;
	if (ItemCount(device) >= MIDIGetNumberOfDestinations()) {
		err = "その番号の MIDI 出力は無い";
		return false;
	}

	auto *c = new ctx();
	c->dest = MIDIGetDestination(ItemCount(device));

	if (MIDIClientCreate(CFSTR("S-MU2000"), nullptr, nullptr, &c->client) != noErr) {
		delete c;
		err = "MIDI クライアントを作れない";
		return false;
	}
	if (MIDIOutputPortCreate(c->client, CFSTR("out"), &c->port) != noErr) {
		MIDIClientDispose(c->client);
		delete c;
		err = "MIDI 出力を開けない";
		return false;
	}

	m_name = endpoint_name(c->dest);

	m_read.store(0);
	m_write.store(0);
	m_have = m_want = 0;
	m_status = 0;
	m_in_sysex = false;
	m_sysex.clear();

	m_quit.store(false);
	m_ctx = c;
	m_thread = std::thread([this] { run(); });
	m_open.store(true, std::memory_order_release);
	return true;
}

midi_out::~midi_out()
{
	close();
}

void midi_out::close()
{
	if (!m_ctx)
		return;
	m_open.store(false, std::memory_order_release);
	m_quit.store(true);
	m_wake.notify_all();
	if (m_thread.joinable())
		m_thread.join();

	if (m_ctx->port)
		MIDIPortDispose(m_ctx->port);
	if (m_ctx->client)
		MIDIClientDispose(m_ctx->client);
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
		return;                       // overflow; the only option is to drop it
	m_buf[w] = v;
	m_write.store(next, std::memory_order_release);
	m_wake.notify_one();
}

void midi_out::emit(const u8 *p, size_t n)
{
	if (!m_ctx || !m_ctx->port || n == 0)
		return;

	// CoreMIDI wants a packet list in a buffer the caller owns. A SysEx dump
	// can be tens of kilobytes, so size it for the worst case and let
	// MIDIPacketListAdd lay out the packet; sizeof(MIDIPacket) covers the
	// list's own header plus the padding the call accounts for
	std::vector<u8> buf(sizeof(MIDIPacketList) + n + sizeof(MIDIPacket) + 16);
	auto *list = reinterpret_cast<MIDIPacketList *>(buf.data());
	MIDIPacket *pkt = MIDIPacketListInit(list);
	pkt = MIDIPacketListAdd(list, ByteCount(buf.size()), pkt, 0, ByteCount(n), p);
	if (!pkt)
		return;
	MIDISend(m_ctx->port, m_ctx->dest, list);
}

void midi_out::run()
{
	for (;;) {
		{
			std::unique_lock<std::mutex> lock(m_wake_mutex);
			m_wake.wait_for(lock, std::chrono::milliseconds(50));
		}

		for (;;) {
			const size_t r = m_read.load(std::memory_order_relaxed);
			if (r == m_write.load(std::memory_order_acquire))
				break;
			const u8 b = m_buf[r];
			m_read.store((r + 1) & MASK, std::memory_order_release);

			// Real-time bytes pass through wherever they land in the stream
			if (b >= 0xf8) {
				emit(&b, 1);
				continue;
			}

			if (m_in_sysex) {
				if (b == 0xf7) {
					m_sysex.push_back(b);
					emit(m_sysex.data(), m_sysex.size());
					m_sysex.clear();
					m_in_sysex = false;
					continue;
				}
				if (!(b & 0x80)) {
					if (m_sysex.size() < 65536)
						m_sysex.push_back(b);
					continue;
				}
				m_sysex.clear();              // cut off mid-message; drop it and fall through
				m_in_sysex = false;
			}

			if (b == 0xf0) {
				m_in_sysex = true;
				m_sysex.clear();
				m_sysex.push_back(b);
				m_status = 0;
				continue;
			}

			if (b & 0x80) {
				m_status = (b < 0xf0) ? b : 0;   // system messages clear running status
				m_msg[0] = b;
				m_have = 1;
				m_want = message_length(b);
				if (m_have == m_want) {
					emit(m_msg, size_t(m_want));
					m_have = 0;
				}
				continue;
			}

			// Data byte. When the status was left out (running status), reuse the last one
			if (m_have == 0) {
				if (!m_status)
					continue;
				m_msg[0] = m_status;
				m_have = 1;
				m_want = message_length(m_status);
			}
			m_msg[m_have++] = b;
			if (m_have == m_want) {
				emit(m_msg, size_t(m_want));
				m_have = m_status ? 1 : 0;       // running status carries on
			}
		}

		if (m_quit.load())
			break;
	}
}

} // namespace ui
