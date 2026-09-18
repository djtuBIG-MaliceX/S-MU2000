// license:BSD-3-Clause
//
// XG の値のパラメータ（automation.h）を、プラグインの口で扱うための入れ物。VST3 と CLAP が同じものを使う。
//
// 1 つの値につき:
//   sent_value / sent_ms     最後に音源へ送った値と時刻。ホストはオートメーションの値を区間ごとに
//                            送り続けることがあり、全部を SysEx にすると 31250bps の直列が溢れるので、
//                            同じ値は送らない。画面で触ったときもここに書く（ホストから戻ってくる
//                            同じ値を送り直さない）
//   recent_value / recent_ms ホストに見せる値の控え。触ってからしばらくは RAM の写しより新しい
//   editing / edit_ms        画面で触っている最中か（ホストへの操作の始まりと終わり）。画面の糸だけ
//
// 糸の決まり:
//   host_value   音声の糸（VST3 の process、CLAP の process / flush）
//   shown_value  ホストが値を聞いてくる糸（どこでもよい。写しの読みは錠で守る）
//   gui_edit / gui_idle  画面の糸

#ifndef S_MU2000_VST3_AUTOMATION_HOST_H
#define S_MU2000_VST3_AUTOMATION_HOST_H

#pragma once

#include "automation.h"
#include "engine.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <memory>
#include <mutex>
#include <vector>

namespace smu2000 {
namespace automation {

class host
{
public:
	explicit host(vst3::engine &eng)
		: m_engine(eng),
		  m_slots(new slot[entries().size()]),
		  m_audio_ram(new ui::xg_snapshot),
		  m_view_ram(new ui::xg_snapshot)
	{
	}

	static int64_t now_ms()
	{
		return int64_t(std::chrono::duration_cast<std::chrono::milliseconds>(
			std::chrono::steady_clock::now().time_since_epoch()).count());
	}

	// ---- 音声の糸

	// 区間の頭で呼ぶ（写しはその区間で要るときに 1 回だけ読む）
	void begin_block() { m_audio_ram_read = false; }

	// ホストから値が来た。値が変わったところだけ、emit(port, bytes, n) で CC かパラメータチェンジを出す
	template <typename Emit>
	void host_value(int i, int value, Emit &&emit)
	{
		const entry &e = entries()[size_t(i)];
		slot &s = m_slots[i];
		const int64_t now = now_ms();
		s.recent_value.store(value);
		s.recent_norm.store(-1.0);
		s.recent_ms.store(now);
		const bool fresh = now - s.sent_ms.load() < 500;
		if (s.sent_value.load() == value && fresh)
			return;                          // 送ったばかり（画面で触って戻ってきた値も）
		if (!m_audio_ram_read) {
			m_engine.panel().read_xg(*m_audio_ram);
			m_audio_ram_read = true;
		}
		int cur = 0;
		const bool known = m_audio_ram->serial && current(e, *m_audio_ram, cur);
		if (!fresh && known && cur == value)
			return;                          // 音源はもうその値（曲や画面が先に入れた）
		uint8_t bytes[16];
		int port = 0;
		const int n = midi(e, value, m_audio_ram->serial ? m_audio_ram.get() : nullptr, bytes, port);
		if (n > 0)
			emit(port, bytes, n);
		s.sent_value.store(value);
		s.sent_ms.store(now);
	}

	// ---- ホストに見せる値

	int shown_value(int i)
	{
		const entry &e = entries()[size_t(i)];
		const slot &s = m_slots[i];
		const int64_t now = now_ms();
		if (now - s.recent_ms.load() < 1000 && s.recent_value.load() >= 0)
			return s.recent_value.load();
		std::lock_guard<std::mutex> lock(m_view_mutex);
		// 写しを読むのは 30ms に 1 回まで（ホストは 1,000 本をまとめて聞いてくることがある）
		if (now - m_view_ram_ms > 30) {
			m_engine.panel().read_xg(*m_view_ram);
			m_view_ram_ms = now;
		}
		int v = 0;
		if (m_view_ram->serial && current(e, *m_view_ram, v))
			return v;
		return def(e);
	}

	// 表示のための写し（インサーションのパラメータは種類で書式が変わる）。ホストが値を聞いてくる糸から。
	// 返した写しは次の呼び出しまで使える（錠の外で読むが、書くのも同じ糸だけ）
	const ui::xg_snapshot *view_ram()
	{
		std::lock_guard<std::mutex> lock(m_view_mutex);
		const int64_t now = now_ms();
		if (now - m_view_ram_ms > 30) {
			m_engine.panel().read_xg(*m_view_ram);
			m_view_ram_ms = now;
		}
		return m_view_ram->serial ? m_view_ram.get() : nullptr;
	}

	// ホストが値を置いた（音源へは host_value で入れる。ここは見せる値の控えだけ）。
	// normalized は VST3 の 0-1 の値。置いた値をそのまま読み返せるよう控える（無ければ負）
	void remember(int i, int value, double normalized = -1.0)
	{
		m_slots[i].recent_value.store(value);
		m_slots[i].recent_norm.store(normalized);
		m_slots[i].recent_ms.store(now_ms());
	}

	// VST3 に見せる 0-1 の値。ホストが置いたばかりで整数が同じなら、置かれた値そのもの
	double shown_normalized(int i)
	{
		const int v = shown_value(i);
		const slot &s = m_slots[i];
		const double n = s.recent_norm.load();
		const entry &e = entries()[size_t(i)];
		if (n >= 0.0 && now_ms() - s.recent_ms.load() < 1000 && to_value(e, n) == v)
			return n;
		return to_normalized(e, v);
	}

	// 状態を戻したあとなど。控えを捨てて写しから読み直させる
	void forget_recent()
	{
		for (size_t i = 0; i < entries().size(); i++)
			m_slots[i].recent_ms.store(0);
		std::lock_guard<std::mutex> lock(m_view_mutex);
		m_view_ram_ms = 0;
	}

	// ---- 画面の糸

	// 画面で値を触った。定義表の値がパラメータに無ければ -1。
	// 操作の始まりなら began を立てる（ホストへ「触り始めた」を伝える）
	int gui_edit(const xg::param &p, int part, int value, bool &began)
	{
		began = false;
		const int i = index_of(p, part);
		if (i < 0)
			return -1;
		const int64_t now = now_ms();
		slot &s = m_slots[i];
		s.sent_value.store(value);
		s.sent_ms.store(now);
		s.recent_value.store(value);
		s.recent_norm.store(-1.0);
		s.recent_ms.store(now);
		if (!s.editing) {
			s.editing = true;
			began = true;
			m_editing.push_back(i);
		}
		s.edit_ms = now;
		return i;
	}

	// 画面でインサーションのパラメータを触った（xg::model の set_raw）。番地がパラメータに無ければ -1
	int gui_edit_raw(uint32_t addr, int raw, int &value, bool &began)
	{
		began = false;
		const int i = index_of_raw(addr);
		if (i < 0)
			return -1;
		const ui::xg_snapshot *ram = view_ram();
		if (!ram || !from_raw(entries()[size_t(i)], *ram, raw, value))
			return -1;
		const int64_t now = now_ms();
		slot &s = m_slots[i];
		s.sent_value.store(value);
		s.sent_ms.store(now);
		s.recent_value.store(value);
		s.recent_norm.store(-1.0);
		s.recent_ms.store(now);
		if (!s.editing) {
			s.editing = true;
			began = true;
			m_editing.push_back(i);
		}
		s.edit_ms = now;
		return i;
	}

	// 画面の 1 コマ。しばらく触られていない値の操作を終え、end(i) を呼ぶ
	// （つまみを離したのは分からないので、時間で見る）。closing なら全部終える
	template <typename End>
	void gui_idle(bool closing, End &&end)
	{
		if (m_editing.empty())
			return;
		const int64_t now = now_ms();
		for (size_t k = 0; k < m_editing.size();) {
			slot &s = m_slots[m_editing[k]];
			if (closing || now - s.edit_ms > 400) {
				end(m_editing[k]);
				s.editing = false;
				m_editing[k] = m_editing.back();
				m_editing.pop_back();
			} else {
				k++;
			}
		}
	}

private:
	struct slot {
		std::atomic<int>     sent_value{-1};
		std::atomic<int64_t> sent_ms{0};
		std::atomic<int>     recent_value{-1};
		std::atomic<double>  recent_norm{-1.0};
		std::atomic<int64_t> recent_ms{0};
		bool                 editing = false;
		int64_t              edit_ms = 0;
	};

	vst3::engine &m_engine;
	std::unique_ptr<slot[]> m_slots;
	std::vector<int> m_editing;

	std::unique_ptr<ui::xg_snapshot> m_audio_ram;
	bool m_audio_ram_read = false;

	std::unique_ptr<ui::xg_snapshot> m_view_ram;
	std::mutex m_view_mutex;
	int64_t m_view_ram_ms = 0;
};

} // namespace automation
} // namespace smu2000

#endif // S_MU2000_VST3_AUTOMATION_HOST_H
