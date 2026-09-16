// license:BSD-3-Clause
//
// CoreMIDI input for macOS. Same interface as midi_in.cpp (WinMM).
//
// CoreMIDI is friendlier than WinMM here: it hands over a whole packet list,
// SysEx included, on its own thread. There are no receive buffers to pre-post,
// so the "SysEx silently disappears unless you hand Windows a buffer" trap from
// the Windows side simply does not exist. What stays identical is the lock-free
// ring, because the audio thread still drains it one byte at a time.

#include "midi_in.h"

#include <CoreFoundation/CoreFoundation.h>
#include <CoreMIDI/CoreMIDI.h>

#include <cstring>
#include <string>

namespace ui {

struct midi_in::ctx
{
	MIDIClientRef   client = 0;
	MIDIPortRef     port   = 0;
	MIDIEndpointRef source = 0;
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
	// DisplayName is what the user sees in other apps; fall back to the raw name
	if (MIDIObjectGetStringProperty(ep, kMIDIPropertyDisplayName, &name) != noErr)
		MIDIObjectGetStringProperty(ep, kMIDIPropertyName, &name);
	std::string out = cf_to_utf8(name);
	if (name)
		CFRelease(name);
	return out.empty() ? std::string("?") : out;
}

// Runs on a CoreMIDI thread, hence the lock-free push. Each packet is a run of
// bytes with no channel framing, so they go in as they are; the synth's SCI
// parses status bytes itself
void read_proc(const MIDIPacketList *list, void *refCon, void *)
{
	auto *self = static_cast<midi_in *>(refCon);
	if (!self)
		return;
	const MIDIPacket *pkt = &list->packet[0];
	for (UInt32 i = 0; i < list->numPackets; i++) {
		for (UInt16 j = 0; j < pkt->length; j++)
			self->push(pkt->data[j]);
		pkt = MIDIPacketNext(pkt);
	}
}

} // namespace


std::vector<std::string> midi_in::list()
{
	std::vector<std::string> out;
	const ItemCount n = MIDIGetNumberOfSources();
	for (ItemCount i = 0; i < n; i++)
		out.push_back(endpoint_name(MIDIGetSource(i)));
	return out;
}

bool midi_in::open(int device, std::string &err)
{
	close();
	if (device < 0)
		return true;
	if (ItemCount(device) >= MIDIGetNumberOfSources()) {
		err = "その番号の MIDI 入力は無い";
		return false;
	}

	auto *c = new ctx();
	c->source = MIDIGetSource(ItemCount(device));

	if (MIDIClientCreate(CFSTR("S-MU2000"), nullptr, nullptr, &c->client) != noErr) {
		delete c;
		err = "MIDI クライアントを作れない";
		return false;
	}
	if (MIDIInputPortCreate(c->client, CFSTR("in"), read_proc, this, &c->port) != noErr) {
		MIDIClientDispose(c->client);
		delete c;
		err = "MIDI 入力を開けない";
		return false;
	}
	if (MIDIPortConnectSource(c->port, c->source, nullptr) != noErr) {
		MIDIPortDispose(c->port);
		MIDIClientDispose(c->client);
		delete c;
		err = "MIDI 入力に繋げない";
		return false;
	}

	m_name = endpoint_name(c->source);
	m_ctx = c;
	m_closing.store(false, std::memory_order_release);
	return true;
}

void midi_in::close()
{
	if (!m_ctx)
		return;
	// Disconnect before disposing, so the read_proc cannot be called with a
	// half-destroyed port
	m_closing.store(true, std::memory_order_release);
	if (m_ctx->port) {
		MIDIPortDisconnectSource(m_ctx->port, m_ctx->source);
		MIDIPortDispose(m_ctx->port);
	}
	if (m_ctx->client)
		MIDIClientDispose(m_ctx->client);
	delete m_ctx;
	m_ctx = nullptr;
	m_name.clear();
}

void midi_in::push(u8 v)
{
	const size_t w = m_write.load(std::memory_order_relaxed);
	const size_t next = (w + 1) & MASK;
	if (next == m_read.load(std::memory_order_acquire))
		return;                       // overflow, same as the machine's RX buffer
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
