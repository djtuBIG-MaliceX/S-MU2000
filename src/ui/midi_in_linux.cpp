// license:BSD-3-Clause
//
// ALSA シーケンサの MIDI 入力（Linux。issue #25）。口の形は midi_in.cpp（WinMM）・
// midi_in_mac.cpp（CoreMIDI）と同じ。
//
// ALSA は CoreMIDI と同じで、SysEx も含めて 1 つの出来事として渡してくる。
// WinMM のように**入れ物をあらかじめ渡しておく**必要は無い。
// 出来事はバイト列に戻してから輪っかへ積む（音源の SCI が自分で区切りを読むので、
// こちらは実機の MIDI 線と同じ「ただのバイトの並び」にしておく）。
//
// 受ける口はこちらが 1 つ作る（`S-MU2000` として見える）。ほかのアプリ（qsynth の
// ような音源を繋ぐのと同じ要領で、aconnect や DAW から）ここへ繋げば鳴る。
// list() は「こちらへ流せる口」＝ほかのアプリの読み出し口を並べ、
// open() はその口からこちらへ繋ぐ。

#include "midi_in.h"

#include <alsa/asoundlib.h>

#include <atomic>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

namespace ui {

namespace {

// ALSA は口が無いだけで標準エラーへ書く（WSL のように /dev/snd が無い機械では
// 「open /dev/snd/seq failed」が出る）。困りごとはこちらが文字列で返すので黙らせる
void alsa_quiet(const char *, int, const char *, int, const char *, ...) {}

void hush_alsa()
{
	static bool done = false;
	if (!done) {
		snd_lib_error_set_handler(alsa_quiet);
		done = true;
	}
}

// 「client:port」と、人の読む名前
struct seq_port {
	int         client = 0;
	int         port   = 0;
	std::string name;
};

// ほかのアプリ・機械の「読み出せる」口（＝こちらが受け取れる相手）
std::vector<seq_port> readable_ports(snd_seq_t *seq)
{
	std::vector<seq_port> out;
	if (!seq)
		return out;
	snd_seq_client_info_t *ci = nullptr;
	snd_seq_port_info_t   *pi = nullptr;
	snd_seq_client_info_alloca(&ci);
	snd_seq_port_info_alloca(&pi);
	const int self = snd_seq_client_id(seq);
	snd_seq_client_info_set_client(ci, -1);
	while (snd_seq_query_next_client(seq, ci) >= 0) {
		const int client = snd_seq_client_info_get_client(ci);
		if (client == self || client == SND_SEQ_CLIENT_SYSTEM)
			continue;
		snd_seq_port_info_set_client(pi, client);
		snd_seq_port_info_set_port(pi, -1);
		while (snd_seq_query_next_port(seq, pi) >= 0) {
			const unsigned caps = snd_seq_port_info_get_capability(pi);
			if ((caps & SND_SEQ_PORT_CAP_READ) == 0 ||
			    (caps & SND_SEQ_PORT_CAP_SUBS_READ) == 0)
				continue;
			seq_port p;
			p.client = client;
			p.port   = snd_seq_port_info_get_port(pi);
			char buf[256];
			std::snprintf(buf, sizeof(buf), "%s: %s (%d:%d)",
			              snd_seq_client_info_get_name(ci),
			              snd_seq_port_info_get_name(pi), p.client, p.port);
			p.name = buf;
			out.push_back(p);
		}
	}
	return out;
}

} // namespace

// 受ける側の一式。ALSA のシーケンサの控えと、出来事を待つスレッド
struct midi_in::ctx
{
	snd_seq_t           *seq  = nullptr;
	snd_midi_event_t    *dec  = nullptr;   // 出来事 → バイト列
	int                  port = -1;        // こちらが作った受け口
	int                  src_client = -1, src_port = -1;
	std::thread          thread;
	std::atomic<bool>    quit{false};
};

std::vector<std::string> midi_in::list()
{
	hush_alsa();
	snd_seq_t *seq = nullptr;
	if (snd_seq_open(&seq, "default", SND_SEQ_OPEN_INPUT, 0) < 0)
		return {};
	std::vector<std::string> out;
	for (const seq_port &p : readable_ports(seq))
		out.push_back(p.name);
	snd_seq_close(seq);
	return out;
}

bool midi_in::open(int device, std::string &err)
{
	close();
	if (device < 0)
		return true;
	hush_alsa();

	auto *c = new ctx();
	if (snd_seq_open(&c->seq, "default", SND_SEQ_OPEN_INPUT, 0) < 0) {
		delete c;
		err = "ALSA のシーケンサを開けない";
		return false;
	}
	snd_seq_set_client_name(c->seq, "S-MU2000");
	c->port = snd_seq_create_simple_port(c->seq, "MIDI IN",
	                                     SND_SEQ_PORT_CAP_WRITE | SND_SEQ_PORT_CAP_SUBS_WRITE,
	                                     SND_SEQ_PORT_TYPE_MIDI_GENERIC |
	                                     SND_SEQ_PORT_TYPE_SYNTHESIZER);
	if (c->port < 0) {
		snd_seq_close(c->seq);
		delete c;
		err = "MIDI の受け口を作れない";
		return false;
	}

	const std::vector<seq_port> ports = readable_ports(c->seq);
	if (size_t(device) >= ports.size()) {
		snd_seq_close(c->seq);
		delete c;
		err = "その番号の MIDI 入力は無い";
		return false;
	}
	const seq_port &want = ports[size_t(device)];
	if (snd_seq_connect_from(c->seq, c->port, want.client, want.port) < 0) {
		snd_seq_close(c->seq);
		delete c;
		err = "MIDI 入力に繋げない: " + want.name;
		return false;
	}
	c->src_client = want.client;
	c->src_port   = want.port;

	// 出来事をバイト列に戻す側。**動作状態（ランニングステータス）は使わない**。
	// 実機の SCI には、送り手が省いたぶんも含めて毎回そろった形で渡す
	if (snd_midi_event_new(SYSEX_SIZE, &c->dec) < 0) {
		snd_seq_close(c->seq);
		delete c;
		err = "MIDI の組み直しを用意できない";
		return false;
	}
	snd_midi_event_no_status(c->dec, 1);

	m_name = want.name;
	m_ctx = c;
	m_closing.store(false, std::memory_order_release);

	midi_in *self = this;
	c->thread = std::thread([self, c] {
		std::vector<u8> buf(SYSEX_SIZE);
		while (!c->quit.load(std::memory_order_acquire)) {
			// 出来事が来るまで待つ（100ms ごとに止める合図を見に戻る）
			int npfd = snd_seq_poll_descriptors_count(c->seq, POLLIN);
			std::vector<pollfd> pfd(size_t(npfd > 0 ? npfd : 1));
			snd_seq_poll_descriptors(c->seq, pfd.data(), unsigned(pfd.size()), POLLIN);
			if (poll(pfd.data(), pfd.size(), 100) <= 0)
				continue;
			snd_seq_event_t *ev = nullptr;
			while (snd_seq_event_input(c->seq, &ev) >= 0 && ev) {
				const long n = snd_midi_event_decode(c->dec, buf.data(), long(buf.size()), ev);
				for (long i = 0; i < n; i++)
					self->push(buf[size_t(i)]);
				if (snd_seq_event_input_pending(c->seq, 0) <= 0)
					break;
			}
		}
	});
	return true;
}

void midi_in::close()
{
	if (!m_ctx)
		return;
	m_closing.store(true, std::memory_order_release);
	m_ctx->quit.store(true, std::memory_order_release);
	if (m_ctx->thread.joinable())
		m_ctx->thread.join();
	if (m_ctx->dec)
		snd_midi_event_free(m_ctx->dec);
	if (m_ctx->seq) {
		if (m_ctx->port >= 0 && m_ctx->src_client >= 0)
			snd_seq_disconnect_from(m_ctx->seq, m_ctx->port, m_ctx->src_client, m_ctx->src_port);
		snd_seq_close(m_ctx->seq);
	}
	delete m_ctx;
	m_ctx = nullptr;
	m_name.clear();
}

void midi_in::push(u8 v)
{
	const size_t w = m_write.load(std::memory_order_relaxed);
	const size_t next = (w + 1) & MASK;
	if (next == m_read.load(std::memory_order_acquire))
		return;                       // 溢れた。実機の受信の溜めと同じ扱い
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
