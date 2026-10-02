// license:BSD-3-Clause
//
// kernel32 / avrt FFI shared by the live row: QPC timing (live.cpp:143,150,166),
// Ctrl+C handler (live.cpp:65-73), thread priority + MMCSS "Pro Audio"
// (live.cpp:115-124, 290-291), event waits for waveOut (live.cpp:262,307).
// Hand-rolled extern blocks in the paths.rs style (paths.rs:96-121) — no crates
// (Cargo.toml note), MSVC host links kernel32/avrt from the SDK import libs.

use std::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn QueryPerformanceCounter(lp_performance_count: *mut i64) -> i32; // paths.rs shape
    fn QueryPerformanceFrequency(lp_frequency: *mut i64) -> i32;
    fn Sleep(dw_milliseconds: u32);
    pub fn GetCurrentThread() -> *mut c_void;
    pub fn SetThreadPriority(h_thread: *mut c_void, n_priority: i32) -> i32;
    pub fn CreateEventA(
        lp_event_attributes: *const c_void,
        b_manual_reset: i32,
        b_initial_state: i32,
        lp_name: *const u8,
    ) -> *mut c_void;
    pub fn WaitForSingleObject(h_handle: *mut c_void, dw_ms: u32) -> u32;
    pub fn CloseHandle(h_object: *mut c_void) -> i32;
    pub fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32;
}

// avrt.dll — MMCSS (live.cpp:115-124 mmcss_guard)
#[link(name = "avrt")]
extern "system" {
    fn AvSetMmThreadCharacteristicsW(task: *const u16, index: *mut u32) -> *mut c_void;
    fn AvSetMmThreadPriority(handle: *mut c_void, priority: i32) -> i32;
    fn AvRevertMmThreadCharacteristics(handle: *mut c_void) -> i32;
}

// live.cpp:121 AVRT_PRIORITY_CRITICAL
pub const AVRT_PRIORITY_CRITICAL: i32 = 3;
// winbase.h:455 THREAD_PRIORITY_TIME_CRITICAL == THREAD_BASE_PRIORITY_LOWRT == 15
pub const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;

/// QueryPerformanceCounter (live.cpp:143 QueryPerformanceFrequency)
pub fn qpf() -> i64 {
    let mut f = 0i64;
    unsafe { QueryPerformanceFrequency(&mut f) };
    f
}

/// QueryPerformanceCounter (live.cpp:150/166)
pub fn qpc() -> i64 {
    let mut c = 0i64;
    unsafe { QueryPerformanceCounter(&mut c) };
    c
}

/// live.cpp:71 Sleep(10)
pub fn sleep_ms(ms: u32) {
    unsafe { Sleep(ms) }
}

/// Wide literal for AvSetMmThreadCharacteristicsW(L"Pro Audio") (live.cpp:120)
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// live.cpp:120 AvSetMmThreadCharacteristicsW
///
/// # Safety
/// Windows API wrapper.
pub unsafe fn av_set_mm_thread_characteristics(task: *const u16) -> *mut c_void {
    let mut idx = 0u32;
    let h = AvSetMmThreadCharacteristicsW(task, &mut idx);
    // live.cpp:121 if (h) AvSetMmThreadPriority(h, AVRT_PRIORITY_CRITICAL)
    if !h.is_null() {
        AvSetMmThreadPriority(h, AVRT_PRIORITY_CRITICAL);
    }
    h
}

/// live.cpp:123 AvRevertMmThreadCharacteristics
///
/// # Safety
/// Windows API wrapper.
pub unsafe fn av_revert_mm_thread(handle: *mut c_void) {
    if !handle.is_null() {
        AvRevertMmThreadCharacteristics(handle);
    }
}
