// license:BSD-3-Clause
//
// The synth side of a front end: load the ROMs, boot, and render blocks on
// demand from the audio device.
//
// This used to live inside gui.cpp. It moved here when the macOS front end
// arrived, because the part worth sharing is not the boilerplate but the
// routing in fill(): which MIDI port feeds which part, and what gets echoed
// back out. Two copies of that is two things that can drift, and a drift there
// would show up as the two platforms sounding different.
//
// Nothing in here touches a window, so it is the same on both platforms. What
// it does reach for is the OS clock and the settings file, both through
// compat/ (see doc/porting-macos.md).

#ifndef S_MU2000_UI_ENGINE_H
#define S_MU2000_UI_ENGINE_H

#pragma once

#include "audio_in.h"
#include "audio_out.h"
#include "bridge.h"
#include "driver.h"
#include "midi_guard.h"
#include "midi_in.h"
#include "midi_out.h"
#include "mu2000.h"
#include "nvram.h"

#include "compat/platform.h"

#include <atomic>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <string>
#include <vector>

namespace ui {

struct engine {
	mu2000 mu;
	bridge   &br;
	midi_in  &midi;        // MIDI IN A（パート 1-16）
	midi_in  *midi_b = nullptr;   // MIDI IN B（パート 17-32）
	midi_out *mout = nullptr;     // MIDI THRU A（A で受けたものを外へ）
	midi_out *mout_b = nullptr;   // MIDI THRU B（B で受けたものを外へ）
	// MIDI OUT。MU2000 が自分で送り出すもの（XG のダンプ要求への返事など）。
	// これを loopMIDI 越しに外のエディタへ返すと、外から読み書きできる
	midi_out *mout_mu = nullptr;
	audio_in *ain = nullptr;          // A/D INPUT に入れる音（無ければ無音）

	std::atomic<int> state{0};        // 0 起動中 / 1 準備完了 / 2 だめ
	// THRU A / B の流量の上限。MIDI の輪で溢れたものを実機へ流さない（midi_guard.h）
	thru_guard guard_a, guard_b;
	// fill() が機械に触っている最中か。起動し直すときはこれが落ちるのを待つ
	std::atomic<bool> in_fill{false};
	// SmartMedia を差す・抜く・書き戻す間は、音声の糸が機械を回さないようにする
	std::mutex card_lock;
	bool use_nvram = false;           // 覚えている設定で起動するか（窓を出すときだけ）
	std::string      message = "起動中...";

	driver drv;

	engine(bridge &b, midi_in &m) : br(b), midi(m) {}

	bool load(const std::string &dir)
	{
		if (!mu.load_program(dir + "/mu2000_flash.bin")) { message = mu.error(); return false; }
		if (!mu.load_wave(dir + "/dump"))                { message = mu.error(); return false; }
		if (!mu.load_sintab(dir + "/standin/sin-table.bin"))
			std::fprintf(stderr, "警告: %s\n", mu.error().c_str());
		if (!mu.load_lcd_font(dir + "/hd44780u_b04.bin") &&
		    !mu.load_lcd_font(dir + "/standin/hd44780u_b04.bin"))
			std::fprintf(stderr, "警告: %s\n", mu.error().c_str());
		return true;
	}

	// 起動（実機と同じ空回し）。窓を出したあと別スレッドで進める
	bool boot()
	{
		mu.set_threaded(true);
		if (use_nvram && smu2000::nvram::load(mu))
			std::printf("設定: %s\n", smu2000::nvram::path(mu).c_str());
		mu.reset();
		const size_t limit = size_t(30.0 * AUDIO_RATE);
		size_t i = 0;
		s32 l, r;
		for (; i < limit && !mu.midi_ready(); i++)
			mu.run_sample(l, r);
		if (i >= limit) {
			message = "起動しなかった";
			return false;
		}
		publish();
		return true;
	}

	// 工場出荷状態に戻す。覚えている設定を捨てて電源を入れ直す。
	// 音声の糸が機械から手を離すのを待ってから触る
	void factory_reset()
	{
		state.store(0);
		message = "工場出荷状態に戻している...";
		publish();
		while (in_fill.load())
			smu2000::sleep_ms(1);

		const std::vector<u8> zero(mu.nvram().size(), 0);
		mu.set_nvram(zero.data(), zero.size());
		const bool keep = use_nvram;
		use_nvram = false;
		const bool ok = boot();
		use_nvram = keep;
		if (!ok) {
			state.store(2);
			publish();
			return;
		}
		// すぐ残す。ここで落ちても前の設定に戻らないように
		smu2000::nvram::save(mu);
		std::printf("工場出荷状態に戻した\n");
		std::fflush(stdout);
		state.store(1);
		publish();
	}

	void publish()
	{
		if (state.load() == 1)
			driver::publish_now(mu, br, true, nullptr);
		else
			driver::publish_message(br, message.c_str());
	}

	// 音声デバイスに頼まれた分だけ進める
	void fill(s16 *out, u32 n)
	{
		const std::lock_guard<std::mutex> hold(card_lock);
		// 先に「触っている」を立ててから state を見る。逆にすると、見た直後に
		// 起動し直しが始まって、両方が機械に触ってしまう
		in_fill.store(true);
		if (state.load() != 1) {
			in_fill.store(false);
			std::memset(out, 0, size_t(n) * 4);
			return;
		}

		guard_a.refill(n, AUDIO_RATE);
		guard_b.refill(n, AUDIO_RATE);

		drv.apply_buttons(mu, br);
		// 画面から出したものも、外の MIDI 出力へ流す（実機の THRU）
		drv.pump_midi(mu, br, [this](u8 v) { if (mout && guard_a.pass(v)) mout->send(v); });
		drv.pump_wheel(mu, br);

		u8 b;
		while (midi.pop(b)) {
			mu.midi_in(b, 0);
			drv.watch(b, 0);
			if (mout && guard_a.pass(b)) mout->send(b);
		}
		// B は実機の 2 つめの DIN（内蔵 SCI ch1）。パート 17-32 に届く。
		// THRU も口ごとに分ける。A で受けたものは MIDI OUT A、
		// B で受けたものは MIDI OUT B へ。混ぜると、外に繋いだ音源で
		// パートの割り振りが崩れる
		if (midi_b)
			while (midi_b->pop(b)) {
				mu.midi_in(b, 1);
				drv.watch(b, 1);
				if (mout_b && guard_b.pass(b)) mout_b->send(b);
			}

		const float g = br.gain();

		for (u32 i = 0; i < n; i++) {
			s32 l = 0, r = 0;
			if (ain) {
				s32 a1, a2;
				ain->pop(a1, a2);
				mu.set_audio_input(a1, a2);
			}
			mu.run_sample(l, r);
			l = s32(l * g) * 32768 / mu2000::DAC_FULL_SCALE;
			r = s32(r * g) * 32768 / mu2000::DAC_FULL_SCALE;
			out[i * 2 + 0] = s16(l < -32768 ? -32768 : l > 32767 ? 32767 : l);
			out[i * 2 + 1] = s16(r < -32768 ? -32768 : r > 32767 ? 32767 : r);
		}

		// firmware が送り出したもの。画面（パラメータの層）と MIDI OUT の口へ。
		// 出口が無くても取り出しておく（溜めを空ける）
		drv.pump_out(mu, br, [this](u8 v) { if (mout_mu) mout_mu->send(v); });

		drv.publish(mu, br, n, AUDIO_RATE, true, nullptr);
		in_fill.store(false);
	}
};

} // namespace ui

#endif // S_MU2000_UI_ENGINE_H
