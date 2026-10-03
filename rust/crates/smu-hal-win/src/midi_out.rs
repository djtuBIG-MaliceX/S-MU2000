// license:BSD-3-Clause
//
// origin: src/midisend.cpp (117 L) — the winmm MIDI-OUT side. midisend.cpp
// includes <mmsystem.h> and calls midiOut* + timeBeginPeriod/timeGetTime
// directly; this module is the hand-rolled FFI mirror (waveout.rs style —
// no crates, declarations only; the ORDER/pacing of the calls lives in
// bins/smu-tools/src/bin/midisend.rs citing midisend.cpp line-for-line).
// Struct layouts pinned against the SAME mingw-w64 headers the ground-truth
// C++ build compiles (mmeapi.h inside the mmsystem.h pshpack1 wrapper).
// Deviations:
// - szPname read from the A (ANSI) caps struct and written RAW (like C++
//   printf("%s", caps.szPname)) — no CP932 round-trip. midi.rs used W for
//   MIDI-in (documented there); midisend.cpp:29-31/70-72 uses midiOutGetDevCapsA
//   so byte-exact stdout = raw A bytes here.

use std::ffi::c_void;

// mmsyscom.h:154 MMSYSERR_NOERROR
pub const MMSYSERR_NOERROR: u32 = 0;
// mmsyscom.h:181 CALLBACK_NULL (midisend.cpp:66 fdwOpen)
pub const CALLBACK_NULL: u32 = 0x00000000;
// mmeapi.h:517 MHDR_DONE (midisend.cpp:100 spin test)
pub const MHDR_DONE: u32 = 0x00000001;
// mmsyscom.h:28 MAXPNAMELEN
pub const MAXPNAMELEN: usize = 32;

/// origin: mmeapi.h:367-377 tagMIDIOUTCAPSA (pack(1); all members
/// naturally aligned so packed size == 52)
#[repr(C, packed)]
pub struct MidiOutCapsA {
    pub w_mid: u16,                    // :368
    pub w_pid: u16,                    // :369
    pub v_driver_version: u32,         // :370 MMVERSION
    pub sz_pname: [u8; MAXPNAMELEN],   // :371 CHAR[32] — raw ANSI bytes
    pub w_technology: u16,             // :372
    pub w_voices: u16,                 // :373
    pub w_notes: u16,                  // :374
    pub w_channel_mask: u16,           // :375
    pub dw_support: u32,               // :376
}

/// origin: mmeapi.h:492-502 midihdr_tag (pack(1)) — the SAME layout as
/// midi.rs MidiHdr (MIDIHDR is in/out shared); own copy with pub fields
/// because midisend owns the header itself (midisend.cpp:78 MIDIHDR hdr{}).
#[repr(C, packed)]
pub struct MidiOutHdr {
    pub lp_data: *mut u8,          // :493 lpData
    pub dw_buffer_length: u32,     // :494 dwBufferLength
    pub dw_bytes_recorded: u32,    // :495
    pub dw_user: usize,            // :496 DWORD_PTR
    pub dw_flags: u32,             // :497 dwFlags (MHDR_DONE polled)
    pub lp_next: *mut MidiOutHdr,  // :498
    pub reserved: usize,           // :499 DWORD_PTR
    pub dw_offset: u32,            // :500
    pub dw_reserved: [usize; 8],   // :501 DWORD_PTR dwReserved[8]
}

#[link(name = "winmm")]
extern "system" {
    pub fn midiOutGetNumDevs() -> u32; // mmeapi.h:555
    pub fn midiOutGetDevCapsA(u_device_id: u32, pmoc: *mut MidiOutCapsA, cb_moc: u32) -> u32; // :568
    pub fn midiOutOpen(
        phmo: *mut *mut c_void,
        u_device_id: u32,
        dw_callback: usize,
        dw_instance: usize,
        fdw_open: u32,
    ) -> u32; // :578
    pub fn midiOutClose(hmo: *mut c_void) -> u32;                    // :579
    pub fn midiOutPrepareHeader(hmo: *mut c_void, pmh: *mut MidiOutHdr, cb_mh: u32) -> u32; // :580
    pub fn midiOutUnprepareHeader(hmo: *mut c_void, pmh: *mut MidiOutHdr, cb_mh: u32) -> u32; // :581
    pub fn midiOutShortMsg(hmo: *mut c_void, dw_msg: u32) -> u32;   // :582
    pub fn midiOutLongMsg(hmo: *mut c_void, pmh: *mut MidiOutHdr, cb_mh: u32) -> u32; // :583
    pub fn midiOutReset(hmo: *mut c_void) -> u32;                   // :584
    // timeapi.h:38/45/46 (winmm) — midisend.cpp:74/75/83/112
    pub fn timeGetTime() -> u32;
    pub fn timeBeginPeriod(u_period: u32) -> u32;
    pub fn timeEndPeriod(u_period: u32) -> u32;
}

/// short-message packer, origin: midisend.cpp:104-107 —
/// msg = b0 | b1<<8 | b2<<16 (bytes beyond index 2 are dropped, and short
/// messages with <3 bytes keep the missing octets zero, exactly as disk).
pub fn short_msg(bytes: &[u8]) -> u32 {
    let mut msg = bytes[0] as u32; // :104
    if bytes.len() > 1 {
        msg |= (bytes[1] as u32) << 8; // :105
    }
    if bytes.len() > 2 {
        msg |= (bytes[2] as u32) << 16; // :106
    }
    msg
}

/// dwFlags reader for the packed header (midisend.cpp:100 test)
///
/// # Safety
/// `hdr` must point at a prepared MIDIHDR.
pub unsafe fn hdr_done(hdr: *const MidiOutHdr) -> bool {
    std::ptr::addr_of!((*hdr).dw_flags).read_unaligned() & MHDR_DONE != 0
}

/// szPname up to the first NUL, raw bytes (midisend.cpp:31/72 printf %s)
pub fn pname_bytes(caps: &MidiOutCapsA) -> Vec<u8> {
    let p = unsafe { std::ptr::addr_of!(caps.sz_pname).read_unaligned() }; // packed
    let end = p.iter().position(|&c| c == 0).unwrap_or(p.len());
    p[..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_struct_sizes_match_packed_headers() {
        // mmeapi.h:367-377 under pshpack1: 2+2+4+32+2+2+2+2+4 = 52
        assert_eq!(std::mem::size_of::<MidiOutCapsA>(), 52);
        assert_eq!(std::mem::offset_of!(MidiOutCapsA, sz_pname), 8);
        assert_eq!(std::mem::offset_of!(MidiOutCapsA, dw_support), 48);
        // mmeapi.h:492-502 x64 — same 112 B as the midi-in MidiHdr pin
        assert_eq!(std::mem::size_of::<MidiOutHdr>(), 112);
        assert_eq!(std::mem::offset_of!(MidiOutHdr, dw_bytes_recorded), 12);
        assert_eq!(std::mem::offset_of!(MidiOutHdr, dw_flags), 24);
        assert_eq!(std::mem::offset_of!(MidiOutHdr, lp_next), 28);
    }

    #[test]
    fn short_msg_packs_little_endian_three_bytes() {
        // midisend.cpp:104-107 — b0 low, b1<<8, b2<<16, rest dropped
        assert_eq!(short_msg(&[0x90, 0x3c, 0x7f]), 0x007f3c90);
        assert_eq!(short_msg(&[0xc3, 0x28]), 0x000028c3); // 2-byte C0/D0
        assert_eq!(short_msg(&[0xf8]), 0x000000f8); // realtime 1-byte
        assert_eq!(short_msg(&[0xf0, 0x01, 0x02, 0x03]), 0x000201f0); // >3 dropped
    }

    #[test]
    fn pname_stops_at_first_nul() {
        let mut caps: MidiOutCapsA = unsafe { std::mem::zeroed() };
        caps.sz_pname[0] = b'A';
        caps.sz_pname[1] = b'B';
        assert_eq!(pname_bytes(&caps), vec![b'A', b'B']);
        let z = unsafe { std::mem::zeroed::<MidiOutCapsA>() };
        assert!(pname_bytes(&z).is_empty());
    }
}
