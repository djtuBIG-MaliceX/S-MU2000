// license:BSD-3-Clause
//
// Primitives that genuinely differ per platform.
//
// Keep this layer tiny and behavior-preserving: this is a synthesizer and its
// output is compared bit for bit between builds, so anything added here must
// not change what the emulator computes.

#ifndef S_MU2000_COMPAT_PLATFORM_H
#define S_MU2000_COMPAT_PLATFORM_H

#pragma once

#include <chrono>
#include <cstdint>
#include <thread>

// windows.h is needed for the performance counter below, and the sources that
// use it all pulled it in anyway before the port, so nothing new is leaking in.
#if defined(_WIN32)
#  ifndef WIN32_LEAN_AND_MEAN
#    define WIN32_LEAN_AND_MEAN
#  endif
#  include <windows.h>
#endif

// The x86 pause intrinsic. Only pull the header on x86 so the ARM build does
// not trip over <immintrin.h> (which errors out on non-x86 targets).
#if defined(_MSC_VER)
#  include <intrin.h>
#elif defined(__i386__) || defined(__x86_64__)
#  include <immintrin.h>
#endif

namespace smu2000 {

// Spin-wait hint for a tight loop that is waiting on another thread (the two
// SWP30s hand a sample off every 22.7us). It must not sleep or yield the core.
//
//   x86   … PAUSE, exactly what _mm_pause() emitted before
//   arm64 … YIELD, the Apple silicon equivalent
//   other … give up the timeslice; a fallback that neither target hits
inline void cpu_pause() noexcept
{
#if defined(_MSC_VER) && (defined(_M_IX86) || defined(_M_X64))
	_mm_pause();
#elif defined(__i386__) || defined(__x86_64__)
	_mm_pause();
#elif defined(__aarch64__) || defined(__arm__)
	__asm__ __volatile__("yield" ::: "memory");
#else
	std::this_thread::yield();
#endif
}

// A monotonic counter and its frequency, so that elapsed ticks can be turned
// into seconds. This is the same idea as QueryPerformanceCounter() /
// QueryPerformanceFrequency(), and on Windows it *is* those two calls, so a
// measurement taken here means the same thing it always did on that side.
// Only differences are meaningful; the absolute value is not a time.
inline std::uint64_t perf_ticks() noexcept
{
#if defined(_WIN32)
	LARGE_INTEGER t;
	QueryPerformanceCounter(&t);
	return std::uint64_t(t.QuadPart);
#else
	// CLOCK_MONOTONIC via steady_clock, which is what a stopwatch wants: it
	// keeps counting across a sleep and is not moved by the clock being set
	return std::uint64_t(std::chrono::steady_clock::now().time_since_epoch().count());
#endif
}

inline std::uint64_t perf_freq() noexcept
{
#if defined(_WIN32)
	LARGE_INTEGER f;
	QueryPerformanceFrequency(&f);
	return std::uint64_t(f.QuadPart);
#else
	// steady_clock counts its own periods, so the frequency is the reciprocal
	// of one period in seconds. libc++ makes it nanoseconds (1e9)
	using period = std::chrono::steady_clock::period;
	return std::uint64_t(period::den) / std::uint64_t(period::num);
#endif
}

// Sleep for a number of milliseconds.
//
// std::this_thread::sleep_for rather than Sleep() on purpose: on Windows it
// waits on the same timer, so it rounds up to the same granularity when
// timeBeginPeriod() has not been asked for.
inline void sleep_ms(int ms) noexcept
{
	std::this_thread::sleep_for(std::chrono::milliseconds(ms));
}

} // namespace smu2000

#endif // S_MU2000_COMPAT_PLATFORM_H
