// license:BSD-3-Clause
//
// origin: src/live.cpp run_waveout (:252-320) — the winmm waveOut FFI,
// hand-rolled paths.rs-style (no crates). Only the declarations + structs
// live here; the fire-all-then-DONE-wait-refill ORDER (:294-312) is
// transliterated in bins/live.rs so it cites live.cpp line-for-line.
// Struct layout pinned against the ground-truth build's own header: mingw-w64
// mmeapi.h:60-69 wavehdr_tag inside the mmsystem.h pshpack1 wrapper ->
// repr(C, packed). x64 offsets: lpData 0, dwBufferLength 8, dwBytesRecorded 12,
// dwUser 16, dwFlags 24, dwLoops 28, lpNext 32, reserved 40 -> size 48.

use std::ffi::c_void;

// mmsyscom.h:154 MMSYSERR_NOERROR
pub const MMSYSERR_NOERROR: u32 = 0;
// live.cpp:264 CALLBACK_EVENT (mmsyscom.h:186 — NOT 0x30000; that is
// CALLBACK_FUNCTION :184 and belongs to midi.rs)
pub const CALLBACK_EVENT: u32 = 0x00050000;
// mmeapi.h:51 WAVE_MAPPER ((UINT)-1)
pub const WAVE_MAPPER: u32 = u32::MAX;
// mmeapi.h:71 WHDR_DONE
pub const WHDR_DONE: u32 = 0x00000001;
// winbase.h WAIT_OBJECT_0 / INFINITE not needed — live.cpp:307 waits with 100 ms
pub const WAIT_TIMEOUT: u32 = 0x00000102;

/// origin: mmeapi.h:60-69 wavehdr_tag (pack(1))
#[repr(C, packed)]
pub struct WaveHdr {
    pub lp_data: *mut u8,        // :61
    pub dw_buffer_length: u32,   // :62
    pub dw_bytes_recorded: u32,  // :63
    pub dw_user: usize,          // :64 DWORD_PTR
    pub dw_flags: u32,           // :65
    pub dw_loops: u32,           // :66
    pub lp_next: *mut WaveHdr,   // :67
    pub reserved: usize,         // :68 DWORD_PTR
}

/// origin: mmeapi.h:248-256 tWAVEFORMATEX (no padding either way)
#[repr(C, packed)]
pub struct WaveFormatEx {
    pub w_format_tag: u16,       // :249 WAVE_FORMAT_PCM = 1
    pub n_channels: u16,         // :250
    pub n_samples_per_sec: u32,  // :251
    pub n_avg_bytes_per_sec: u32, // :252
    pub n_block_align: u16,      // :253
    pub w_bits_per_sample: u16,  // :254
    pub cb_size: u16,            // :255
}

#[link(name = "winmm")]
extern "system" {
    pub fn waveOutOpen(
        phwo: *mut *mut c_void,
        u_device_id: u32,
        pwfx: *const WaveFormatEx,
        dw_callback: usize,
        dw_instance: usize,
        fdw_open: u32,
    ) -> u32;
    pub fn waveOutPrepareHeader(hwo: *mut c_void, pwh: *mut WaveHdr, cb_wh: u32) -> u32;
    pub fn waveOutWrite(hwo: *mut c_void, pwh: *mut WaveHdr, cb_wh: u32) -> u32;
    pub fn waveOutReset(hwo: *mut c_void) -> u32;
    pub fn waveOutUnprepareHeader(hwo: *mut c_void, pwh: *mut WaveHdr, cb_wh: u32) -> u32;
    pub fn waveOutClose(hwo: *mut c_void) -> u32;
}

/// WHDR_DONE reader for the packed header (live.cpp:306 dwFlags test)
///
/// # Safety
/// `hdr` must point at a prepared WAVEHDR.
pub unsafe fn hdr_done(hdr: *const WaveHdr) -> bool {
    std::ptr::addr_of!((*hdr).dw_flags).read_unaligned() & WHDR_DONE != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wavehdr_matches_packed_header() {
        assert_eq!(std::mem::size_of::<WaveHdr>(), 48);
        assert_eq!(std::mem::offset_of!(WaveHdr, dw_bytes_recorded), 12);
        assert_eq!(std::mem::offset_of!(WaveHdr, dw_flags), 24);
        assert_eq!(std::mem::offset_of!(WaveHdr, lp_next), 32);
        assert_eq!(std::mem::size_of::<WaveFormatEx>(), 18);
    }
}
