// license:BSD-3-Clause
//
// origin: src/ui/midi_in.cpp (251 L) + midi_in.h Windows class (77-129) —
// winmm MIDI-in: lock-free SPSC ring SIZE=65536 (midi_in.h:112), 4×8192 SysEx
// buffers pre-posted to Windows (midi_in.cpp:88-103 — "これをやらないと SysEx は
// 来ない"), callback pushes whole messages (commit only at message end,
// midi_in.h:96-99), audio thread pops one byte at a time (:241-249).
// Ledger row `midi in` (M6). Deviations, disclosed:
// - open()/close() call winmm DIRECTLY; C++ wraps them in mm_open.h's
//   open_with_timeout / run_with_timeout threads (midi_in.cpp:65-75, 139-153).
//   A wedged driver can therefore block here — the timeout threads are M6b.
//   The timeout error string "MIDI 入力が応答しない…" (:72-73) is unreachable.
// - Device-name fetch uses midiInGetDevCapsW + UTF-16 (midi_in.cpp:80-82/41-46);
//   live.cpp's --list win32 arm (:83-85) used the A/W variants with CP_ACP —
//   W is the same source midi_in::list() uses, and utf8-safe.
// - Windows serializes all callbacks for one HMIDIIN on a single thread, so
//   pending/overflow/in_sysex need no atomics (the same assumption disk makes,
//   midi_in.h:115-117 plain `size_t/bool`).
// Struct layouts pinned against the SAME header the ground-truth C++ build
// compiles: mingw-w64 mmeapi.h:492-502 (MIDIHDR) inside the mmsystem.h
// pshpack1 wrapper -> repr(C, packed).

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

// midi_in.h:104-105 SYSEX_BUFFERS / SYSEX_SIZE
pub const SYSEX_BUFFERS: usize = 4;
pub const SYSEX_SIZE: usize = 8192;
// midi_in.h:112 SIZE = 65536, MASK = SIZE - 1
const SIZE: usize = 65536;
const MASK: usize = SIZE - 1;

// mmsyscom.h constants (verified against the mingw headers on this box)
const MMSYSERR_NOERROR: u32 = 0;      // mmsyscom.h:154
const CALLBACK_FUNCTION: u32 = 0x00030000; // mmsyscom.h:184
const MIM_DATA: u32 = 0x3C3;          // MM_MIM_DATA, mmsyscom.h:108
const MIM_LONGDATA: u32 = 0x3C4;      // MM_MIM_LONGDATA, :109
const MAXPNAMELEN: usize = 32;        // mmsyscom.h:28

#[repr(C, packed)]
struct MidiInCapsW {
    w_mid: u16,
    w_pid: u16,
    v_driver_version: u32,
    sz_pname: [u16; MAXPNAMELEN],
    dw_support: u32, // mmeapi.h:457 — the OS INVALPARAMs (r=0xb) if you pass
    // a smaller cbMidiInCaps (72 was rejected; the true size is 76)
}

// origin: mmeapi.h:492-502 midihdr_tag (pack(1) via mmsystem.h pshpack1)
#[repr(C, packed)]
pub struct MidiHdr {
    lp_data: *mut u8,            // :493
    dw_buffer_length: u32,       // :494
    dw_bytes_recorded: u32,      // :495
    dw_user: usize,              // :496 DWORD_PTR
    dw_flags: u32,               // :497
    lp_next: *mut MidiHdr,       // :498
    reserved: usize,             // :499 DWORD_PTR
    dw_offset: u32,              // :500
    dw_reserved: [usize; 8],     // :501 DWORD_PTR dwReserved[8]
}

#[link(name = "winmm")]
extern "system" {
    fn midiInGetNumDevs() -> u32;
    fn midiInGetDevCapsW(u_device_id: u32, pmic: *mut MidiInCapsW, cb_mic: u32) -> u32;
    fn midiInOpen(
        phmi: *mut *mut c_void,
        u_device_id: u32,
        dw_callback: usize,
        dw_instance: usize,
        fdw_open: u32,
    ) -> u32;
    fn midiInClose(hmi: *mut c_void) -> u32;
    fn midiInStart(hmi: *mut c_void) -> u32;
    fn midiInStop(hmi: *mut c_void) -> u32;
    fn midiInReset(hmi: *mut c_void) -> u32;
    fn midiInPrepareHeader(hmi: *mut c_void, pmh: *mut MidiHdr, cb_mh: u32) -> u32;
    fn midiInUnprepareHeader(hmi: *mut c_void, pmh: *mut MidiHdr, cb_mh: u32) -> u32;
    fn midiInAddBuffer(hmi: *mut c_void, pmh: *mut MidiHdr, cb_mh: u32) -> u32;
}

/// callback-only state (midi_in.h:115-117,123-128 non-atomic members)
struct State {
    handle: *mut c_void,                    // :123 m_handle
    pending: usize,                         // :115 m_pending
    overflow: bool,                         // :116 m_overflow
    in_sysex: bool,                         // :117 m_in_sysex
    hdrs: [*mut MidiHdr; SYSEX_BUFFERS],    // :126 m_hdr
    sysex: [*mut u8; SYSEX_BUFFERS],        // :127 m_sysex
}

/// origin: ui::midi_in (midi_in.h:77-129). One instance: the g_midi global
/// (live.cpp:57). Callback writes the ring, audio thread drains it.
pub struct MidiIn {
    buf: UnsafeCell<[u8; SIZE]>, // :112 m_buf[SIZE] = {}
    read: AtomicUsize,           // :113 m_read
    write: AtomicUsize,          // :113 m_write
    bytes: AtomicU64,            // :121 m_bytes
    closing: AtomicBool,         // :128 m_closing
    st: UnsafeCell<State>,
    name: UnsafeCell<String>,    // :124 m_name
}
unsafe impl Sync for MidiIn {}

// The live binary keeps exactly one, like disk's `ui::midi_in g_midi` (:57)
static G_MIDI: MidiIn = MidiIn {
    buf: UnsafeCell::new([0; SIZE]),
    read: AtomicUsize::new(0),
    write: AtomicUsize::new(0),
    bytes: AtomicU64::new(0),
    closing: AtomicBool::new(false),
    st: UnsafeCell::new(State {
        handle: std::ptr::null_mut(),
        pending: 0,
        overflow: false,
        in_sysex: false,
        hdrs: [std::ptr::null_mut(); SYSEX_BUFFERS],
        sysex: [std::ptr::null_mut(); SYSEX_BUFFERS],
    }),
    name: UnsafeCell::new(String::new()),
};

// ---- free functions (live.cpp seams) ----

/// live.cpp:103-106 midi_input_count() win32 arm = midiInGetNumDevs()
pub fn count() -> i32 {
    unsafe { midiInGetNumDevs() as i32 }
}

/// origin: midi_in.cpp:37-49 list() — caps via W + utf8 (see DEVIATIONS)
pub fn list() -> Vec<String> {
    let mut out = Vec::new();
    let n = unsafe { midiInGetNumDevs() }; // :40
    for i in 0..n {
        let mut caps: MidiInCapsW = unsafe { std::mem::zeroed() }; // :42
        let r = unsafe {
            midiInGetDevCapsW(i, &mut caps, std::mem::size_of::<MidiInCapsW>() as u32) // :43
        };
        if r == MMSYSERR_NOERROR {
            out.push(utf16(&pname(&caps))); // :44 to_utf8(caps.szPname)
        } else {
            out.push("?".to_string()); // :46
        }
    }
    out
}

fn utf16(w: &[u16; MAXPNAMELEN]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// unaligned read of the packed szPname (E0793-safe)
fn pname(caps: &MidiInCapsW) -> [u16; MAXPNAMELEN] {
    unsafe { std::ptr::addr_of!(caps.sz_pname).read_unaligned() }
}

/// live.cpp:539 g_midi.open(midi_dev, merr)
pub fn open(device: i32, err: &mut String) -> bool {
    G_MIDI.open(device, err)
}
/// live.cpp:575 g_midi.close()
pub fn close() {
    G_MIDI.close()
}
/// live.cpp:154 g_midi.pop(b) — audio thread, one byte at a time
pub fn pop(v: &mut u8) -> bool {
    G_MIDI.pop(v)
}
/// live.cpp:185/587 g_midi.bytes()
pub fn bytes() -> u64 {
    G_MIDI.bytes.load(Ordering::Relaxed) // midi_in.h:90 m_bytes.load()
}
/// live.cpp:543 g_midi.device_name()
pub fn device_name() -> String {
    unsafe { (*G_MIDI.name.get()).clone() }
}

// ---- the class ----

/// origin: midi_in.cpp:16-32 cb — the winmm callback trampoline.
/// dwInstance is &G_MIDI (midiInOpen passes it, :67).
unsafe extern "system" fn cb(
    _hmi: *mut c_void,
    msg: u32,
    dw_inst: usize,
    p1: usize,
    _p2: usize,
) {
    if dw_inst == 0 {
        return; // :19-20 if (!self) return
    }
    let me = &*(dw_inst as *const MidiIn);
    if msg == MIM_DATA {
        me.on_short(p1 as u32); // :22-23
    } else if msg == MIM_LONGDATA {
        let h = p1 as *mut MidiHdr; // :25
        // packed reads: the OS wrote these through the same pshpack1 layout
        let lp = std::ptr::addr_of!((*h).lp_data).read_unaligned();
        let n = std::ptr::addr_of!((*h).dw_bytes_recorded).read_unaligned() as usize;
        me.on_long(std::slice::from_raw_parts(lp, n), n); // :26
        // :27-30 使い終わった入れ物をすぐ返す — requeue immediately
        me.requeue(h); // :30
    }
}

impl MidiIn {
    /// origin: midi_in.cpp:51-107 open (see DEVIATIONS for mm_open.h deferral)
    fn open(&self, device: i32, err: &mut String) -> bool {
        self.close(); // :53
        if device < 0 {
            return true; // :54-55 負なら開かない
        }
        if (device as u32) >= unsafe { midiInGetNumDevs() } {
            *err = "その番号の MIDI 入力は無い".to_string(); // :56-59
            return false;
        }
        // :64-70 midiInOpen(CALLBACK_FUNCTION, this) — direct (no timeout thread)
        let mut h: *mut c_void = std::ptr::null_mut();
        let r = unsafe {
            midiInOpen(
                &mut h,
                device as u32,
                cb as *const () as usize, // midiInOpen dwCallback
                self as *const MidiIn as usize,
                CALLBACK_FUNCTION,
            )
        };
        if r != MMSYSERR_NOERROR {
            *err = "MIDI 入力を開けない".to_string(); // :76-79
            return false;
        }
        // :80-82 caps -> m_name
        let mut caps: MidiInCapsW = unsafe { std::mem::zeroed() };
        unsafe {
            midiInGetDevCapsW(device as u32, &mut caps, std::mem::size_of::<MidiInCapsW>() as u32)
        };
        unsafe { *self.name.get() = utf16(&pname(&caps)) }; // :82
        let st = unsafe { &mut *self.st.get() };
        st.handle = h; // :83 m_handle = h
        self.closing.store(false, Ordering::Release); // :84
        st.in_sysex = false; // :85
        unsafe { self.rollback() }; // :86
        // :88-103 SysEx の入れ物を Windows へ渡す — pre-post 4×8192
        for i in 0..SYSEX_BUFFERS {
            let buf: *mut u8 =
                Box::into_raw(Box::new([0u8; SYSEX_SIZE])) as *mut u8; // :91 new u8[SYSEX_SIZE]
            let hdr: *mut MidiHdr = Box::into_raw(Box::new(MidiHdr {
                lp_data: buf, // :93
                dw_buffer_length: SYSEX_SIZE as u32, // :94
                dw_bytes_recorded: 0,
                dw_user: 0,
                dw_flags: 0,
                lp_next: std::ptr::null_mut(),
                reserved: 0,
                dw_offset: 0,
                dw_reserved: [0; 8],
            })); // :92 new MIDIHDR{}
            let (p, a) = unsafe {
                (
                    midiInPrepareHeader(h, hdr, std::mem::size_of::<MidiHdr>() as u32), // :95
                    midiInAddBuffer(h, hdr, std::mem::size_of::<MidiHdr>() as u32),     // :96
                )
            };
            if p != MMSYSERR_NOERROR || a != MMSYSERR_NOERROR {
                // :97-100 delete hdr; delete[] buf; continue — 音符は受けられる
                unsafe {
                    drop(Box::from_raw(hdr));
                    drop(Box::from_raw(buf as *mut [u8; SYSEX_SIZE]));
                }
                continue;
            }
            st.hdrs[i] = hdr; // :102 m_hdr[i] = hdr
            st.sysex[i] = buf;
        }
        unsafe { midiInStart(h) }; // :105
        true
    }

    /// origin: midi_in.cpp:110-116 requeue — callback returns an empty bin
    unsafe fn requeue(&self, hdr: *mut MidiHdr) {
        let st = &*self.st.get();
        if st.handle.is_null() || self.closing.load(Ordering::Acquire) {
            return; // :112 閉じている最中は返さない
        }
        midiInAddBuffer(st.handle, hdr, std::mem::size_of::<MidiHdr>() as u32); // :114-115
    }

    /// origin: midi_in.cpp:118-157 close (run inline; see DEVIATIONS — disk
    /// runs the stop/reset/close chain on a timed thread, mm_open.h)
    fn close(&self) {
        let st = unsafe { &mut *self.st.get() };
        if st.handle.is_null() {
            return; // :120-121
        }
        let h = st.handle;
        // :123-125 先に印を立てる (m_closing) — reset のコールバックで返さない
        self.closing.store(true, Ordering::Release);
        unsafe {
            midiInStop(h); // :140
            midiInReset(h); // :141
            for i in 0..SYSEX_BUFFERS {
                let hdr = st.hdrs[i]; // :131-136 steal slots
                let buf = st.sysex[i];
                st.hdrs[i] = std::ptr::null_mut();
                st.sysex[i] = std::ptr::null_mut();
                if !hdr.is_null() {
                    midiInUnprepareHeader(h, hdr, std::mem::size_of::<MidiHdr>() as u32); // :145
                    drop(Box::from_raw(hdr)); // :146
                }
                if !buf.is_null() {
                    drop(Box::from_raw(buf as *mut [u8; SYSEX_SIZE])); // :148 delete[] bufs
                }
            }
            midiInClose(h); // :150
        }
        st.handle = std::ptr::null_mut(); // :155
        unsafe { (*self.name.get()).clear() }; // :156
    }

    /// origin: midi_in.cpp:160-190 on_short — Windows keeps running statuses
    unsafe fn on_short(&self, p1: u32) {
        let status = p1 as u8; // :162
        if status < 0x80 {
            return; // :163-164
        }
        let st = &mut *self.st.get();
        // :166-172 realtime may interleave a SysEx; commit only outside one
        if status >= 0xf8 {
            self.push(status); // :168
            if !st.in_sysex {
                self.commit(); // :169-170
            }
            return;
        }
        if st.in_sysex {
            // :174-177 anything else ends the SysEx on the wire
            st.in_sysex = false;
            self.commit();
        }
        // :178-184 length by kind
        let mut n = 3usize;
        let kind = status & 0xf0;
        if kind == 0xc0 || kind == 0xd0 {
            n = 2; // :181
        }
        if status == 0xf1 || status == 0xf3 || status == 0xf5 {
            n = 2; // :182 F5 nn ケーブルメッセージ
        } else if status == 0xf2 {
            n = 3; // :183
        } else if status >= 0xf4 && status < 0xf8 {
            n = 1; // :184
        }
        self.push(status); // :186
        if n > 1 {
            self.push((p1 >> 8) as u8); // :187
        }
        if n > 2 {
            self.push((p1 >> 16) as u8); // :188
        }
        self.commit(); // :189
    }

    /// origin: midi_in.cpp:193-210 on_long — SysEx fragments until F7
    unsafe fn on_long(&self, p: &[u8], n: usize) {
        let st = &mut *self.st.get();
        for i in 0..n {
            let v = p[i]; // :195
            if v == 0xf0 {
                if st.in_sysex {
                    self.commit(); // :198-199 F7 の無いまま次が始まった
                }
                st.in_sysex = true; // :200
            }
            if !st.in_sysex {
                continue; // :202-203 F0 より前の半端なバイト
            }
            self.push(v); // :204
            if v == 0xf7 {
                st.in_sysex = false; // :205-208
                self.commit();
            }
        }
    }

    /// origin: midi_in.cpp:212-224 push — overflow drops the whole message
    unsafe fn push(&self, v: u8) {
        let st = &mut *self.st.get();
        if st.overflow {
            return; // :214-215
        }
        let next = (st.pending + 1) & MASK; // :216
        if next == self.read.load(Ordering::Acquire) {
            st.overflow = true; // :217-220 溢れ — このメッセージは捨てる
            return;
        }
        (*self.buf.get())[st.pending] = v; // :221
        st.pending = next; // :222
        self.bytes.fetch_add(1, Ordering::Relaxed); // :223
    }

    /// origin: midi_in.cpp:226-233 commit — advance the readable edge only
    /// at message end (midi_in.h:96-99)
    unsafe fn commit(&self) {
        let st = &*self.st.get();
        if st.overflow {
            self.rollback(); // :227-229
            return;
        }
        self.write.store(st.pending, Ordering::Release); // :232
    }

    /// origin: midi_in.cpp:235-239 rollback
    unsafe fn rollback(&self) {
        let st = &mut *self.st.get();
        st.pending = self.write.load(Ordering::Relaxed); // :237
        st.overflow = false; // :238
    }

    /// origin: midi_in.cpp:241-249 pop — audio thread only
    fn pop(&self, v: &mut u8) -> bool {
        let r = self.read.load(Ordering::Relaxed); // :243
        if r == self.write.load(Ordering::Acquire) {
            return false; // :244
        }
        *v = unsafe { (*self.buf.get())[r] }; // :246
        self.read.store((r + 1) & MASK, Ordering::Release); // :247
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_is_64k_mask_and_empty_at_start() {
        // midi_in.h:112 shape + live drain seam on a never-opened device
        assert_eq!(SIZE, 65536);
        assert_eq!(MASK, 65535);
        assert!(!pop(&mut 0u8)); // empty ring pops nothing (no device opened)
    }

    #[test]
    fn ffi_struct_sizes_match_packed_headers() {
        // mmeapi.h:492-502 under pshpack1, x64: lpData 0, dwBufferLength 8,
        // dwBytesRecorded 12, dwUser 16, dwFlags 24, lpNext 28, reserved 36,
        // dwOffset 44, dwReserved[8] 48 -> 112
        assert_eq!(std::mem::size_of::<MidiHdr>(), 112);
        assert_eq!(std::mem::offset_of!(MidiHdr, dw_bytes_recorded), 12);
        assert_eq!(std::mem::offset_of!(MidiHdr, dw_flags), 24);
        assert_eq!(std::mem::offset_of!(MidiHdr, lp_next), 28);
        // midi_in caps (mmeapi.h:452-458): 2+2+4+32*2+4 = 76
        assert_eq!(std::mem::size_of::<MidiInCapsW>(), 76);
    }
}
