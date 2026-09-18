// license:BSD-3-Clause

#include "midi_out.h"
#include "mm_open.h"
#include "text.h"

#include <cstring>

#include <windows.h>
#include <mmsystem.h>

namespace ui {

namespace {

// 状態バイトから、そのあとに続くデータの数
int message_length(u8 status)
{
	if (status >= 0xf8) return 1;                    // リアルタイム
	switch (status) {
	case 0xf1: case 0xf3: case 0xf5: return 2;   // F5 nn はケーブルメッセージ（口の切り替え）
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
	const UINT n = midiOutGetNumDevs();
	for (UINT i = 0; i < n; i++) {
		MIDIOUTCAPSW caps{};
		if (midiOutGetDevCapsW(i, &caps, sizeof(caps)) == MMSYSERR_NOERROR)
			out.push_back(to_utf8(caps.szPname));
		else
			out.push_back("?");
	}
	return out;
}

bool midi_out::open(int device, std::string &err)
{
	close();
	if (device < 0)
		return true;
	if (UINT(device) >= midiOutGetNumDevs()) {
		err = "その番号の MIDI 出力は無い";
		return false;
	}

	// 口の持ち主が固まっていると返ってこないので、時間を区切る（mm_open.h）
	HMIDIOUT h = nullptr;
	const int r = open_with_timeout<HMIDIOUT>(
		[device](HMIDIOUT &out) {
			return unsigned(midiOutOpen(&out, UINT(device), 0, 0, CALLBACK_NULL));
		},
		[](HMIDIOUT late) { midiOutClose(late); }, h);
	if (r == 2) {
		err = "MIDI 出力が応答しない（loopMIDI やドライバが固まっているかもしれない。"
		      "loopMIDI を起動し直すか、機器を挿し直す）";
		return false;
	}
	if (r != 0) {
		err = "MIDI 出力を開けない";
		return false;
	}
	MIDIOUTCAPSW caps{};
	midiOutGetDevCapsW(UINT(device), &caps, sizeof(caps));
	m_name = to_utf8(caps.szPname);

	m_read.store(0);
	m_write.store(0);
	m_have = m_want = 0;
	m_status = 0;
	m_in_sysex = false;
	m_sysex.clear();

	m_quit.store(false);
	m_stuck.store(false);
	if (!m_wake)
		m_wake = CreateEventA(nullptr, FALSE, FALSE, nullptr);
	m_handle = h;
	const unsigned gen = m_gen.fetch_add(1) + 1;
	m_thread_done.store(false);
	m_thread = std::thread([this, gen, h] { run(gen, h); });
	m_open.store(true, std::memory_order_release);
	return true;
}

midi_out::~midi_out()
{
	close();
	if (m_wake) {
		CloseHandle(HANDLE(m_wake));
		m_wake = nullptr;
	}
}

void midi_out::close()
{
	if (!m_handle)
		return;
	m_open.store(false, std::memory_order_release);
	m_quit.store(true);
	SetEvent(HANDLE(m_wake));
	// 送りスレッドが抜けるのを 2 秒まで待つ。抜けないのは相手が固まっているとき。
	// そのときは置いていき、口も閉じない（閉じる呼び出しも戻ってこないため）
	for (int i = 0; i < 200 && !m_thread_done.load(); i++)
		Sleep(10);
	HMIDIOUT h = reinterpret_cast<HMIDIOUT>(m_handle);
	if (m_thread_done.load()) {
		if (m_thread.joinable())
			m_thread.join();
		midiOutReset(h);
		midiOutClose(h);
	} else if (m_thread.joinable()) {
		m_thread.detach();
	}
	m_handle = nullptr;
	m_name.clear();
}

void midi_out::send(u8 v)
{
	if (!m_open.load(std::memory_order_acquire))
		return;
	const size_t w = m_write.load(std::memory_order_relaxed);
	const size_t next = (w + 1) & MASK;
	if (next == m_read.load(std::memory_order_acquire))
		return;                       // 溢れ。落とすしかない
	m_buf[w] = v;
	m_write.store(next, std::memory_order_release);
	SetEvent(HANDLE(m_wake));
}

void midi_out::emit(void *handle, const u8 *p, size_t n)
{
	HMIDIOUT h = reinterpret_cast<HMIDIOUT>(handle);
	if (n == 0)
		return;

	if (p[0] == 0xf0) {
		// 入れ物は自前で持つ。送り終わらないまま諦めたときに、ドライバが後から
		// 触っても壊れないよう、そのときは捨てずに置いておく
		MIDIHDR *hdr = new MIDIHDR{};
		u8 *copy = new u8[n];
		std::memcpy(copy, p, n);
		hdr->lpData = reinterpret_cast<LPSTR>(copy);
		hdr->dwBufferLength = DWORD(n);
		if (midiOutPrepareHeader(h, hdr, sizeof(MIDIHDR)) != MMSYSERR_NOERROR) {
			delete hdr;
			delete[] copy;
			return;
		}
		midiOutLongMsg(h, hdr, sizeof(MIDIHDR));
		// 送り終わるまで待つ。ここは送りスレッドなので待ってよいが、
		// **相手が固まっていると終わらない**。31250bps でも 1 秒で 3000 バイト
		// 流れるので、長さに見合った時間だけ待って諦める
		const DWORD limit = 1000 + DWORD(n / 3);
		const DWORD t0 = GetTickCount();
		while (!(hdr->dwFlags & MHDR_DONE)) {
			if (GetTickCount() - t0 > limit) {
				m_stuck.store(true, std::memory_order_release);
				return;               // hdr と copy は置いておく（ドライバがまだ持っている）
			}
			Sleep(1);
		}
		midiOutUnprepareHeader(h, hdr, sizeof(MIDIHDR));
		delete hdr;
		delete[] copy;
		return;
	}

	DWORD m = p[0];
	if (n > 1) m |= DWORD(p[1]) << 8;
	if (n > 2) m |= DWORD(p[2]) << 16;
	midiOutShortMsg(h, m);
}

void midi_out::run(unsigned gen, void *handle)
{
	for (;;) {
		WaitForSingleObject(HANDLE(m_wake), 50);

		for (;;) {
			// 置いていかれたあとで戻ってきた。もう新しい口のもの
			if (m_gen.load() != gen)
				return;
			const size_t r = m_read.load(std::memory_order_relaxed);
			if (r == m_write.load(std::memory_order_acquire))
				break;
			const u8 b = m_buf[r];
			m_read.store((r + 1) & MASK, std::memory_order_release);

			// リアルタイムはどこに挟まっていてもそのまま通す
			if (b >= 0xf8) {
				emit(handle, &b, 1);
				continue;
			}

			if (m_in_sysex) {
				if (b == 0xf7) {
					m_sysex.push_back(b);
					emit(handle, m_sysex.data(), m_sysex.size());
					m_sysex.clear();
					m_in_sysex = false;
					continue;
				}
				if (!(b & 0x80)) {
					if (m_sysex.size() < 65536)
						m_sysex.push_back(b);
					continue;
				}
				m_sysex.clear();              // 途中で切れた。捨てて下へ落とす
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
				m_status = (b < 0xf0) ? b : 0;   // システムは走り状態を消す
				m_msg[0] = b;
				m_have = 1;
				m_want = message_length(b);
				if (m_have == m_want) {
					emit(handle, m_msg, size_t(m_want));
					m_have = 0;
				}
				continue;
			}

			// データ。状態が省かれていたら（走り状態）前のものを使う
			if (m_have == 0) {
				if (!m_status)
					continue;
				m_msg[0] = m_status;
				m_have = 1;
				m_want = message_length(m_status);
			}
			m_msg[m_have++] = b;
			if (m_have == m_want) {
				emit(handle, m_msg, size_t(m_want));
				m_have = m_status ? 1 : 0;       // 走り状態は続く
			}
		}

		if (m_quit.load() || m_gen.load() != gen)
			break;
	}
	if (m_gen.load() == gen)
		m_thread_done.store(true);
}

} // namespace ui
