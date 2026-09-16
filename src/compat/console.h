// license:BSD-3-Clause
//
// Console setup for the command-line tools.
//
// Every string here is UTF-8: the sources are, and so is anything we read back
// from a device name. The Windows console defaults to CP932, so Japanese output
// turns into mojibake until the code page is switched. macOS terminals speak
// UTF-8 already, so there this call does nothing.

#ifndef S_MU2000_COMPAT_CONSOLE_H
#define S_MU2000_COMPAT_CONSOLE_H

#pragma once

#if defined(_WIN32)
#  ifndef WIN32_LEAN_AND_MEAN
#    define WIN32_LEAN_AND_MEAN
#  endif
#  ifndef NOMINMAX
#    define NOMINMAX
#  endif
#  include <windows.h>
#endif

namespace smu2000 {

inline void init_console_utf8() noexcept
{
#if defined(_WIN32)
	SetConsoleOutputCP(CP_UTF8);
#endif
}

} // namespace smu2000

#endif // S_MU2000_COMPAT_CONSOLE_H
