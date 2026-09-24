// license:BSD-3-Clause
//
// The panel's bottom line, identical on both graphical front ends.
//
// gui.exe and the Mac GUI formatted it by hand and drifted. Only the audio
// middle fragment is per backend, formatted from what each one actually
// measures (WASAPI: wait + drops, CoreAudio: drops).

#ifndef S_MU2000_UI_STATUS_H
#define S_MU2000_UI_STATUS_H

#pragma once

#include <cstddef>
#include <cstdio>

#include "ui/texts.h"

namespace ui {

inline void format_status_line(char *dst, std::size_t n, int voices,
                               double cpu_pct, double worst_ms, const char *middle,
                               const char *in_name, const char *out_name)
{
	std::snprintf(dst, n, UI_TEXT(status_format, "Voices %d/128  CPU %.0f%%  worst %.1f ms  %s   IN: %s   OUT: %s"
                               "   (pick ports from the MIDI IN A jack or by right-click)"),
	              voices, cpu_pct, worst_ms, middle, in_name, out_name);
}

} // namespace ui

#endif // S_MU2000_UI_STATUS_H
