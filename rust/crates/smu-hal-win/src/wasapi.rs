// license:BSD-3-Clause
//
// origin: src/ui/audio_out.cpp (715 L) + src/ui/resampler.h (182 L) — the
// `audio out` row (M6). WASAPI on its own thread, event-driven: the device's
// 「N サンプルくれ」 is the only clock (audio_out.h:6-8).
//
// **標本化周波数の変換は自分でやる** (audio_out.h:10-13): the own sinc
// resampler (resampler.h) is transliterated below as `Resampler`. The OS
// converter (AUTOCONVERTPCM) is armed ONLY for the exotic not-float/
// not-pcm16 mix-format fallback (audio_out.cpp:432-458), exactly like disk.
//
// Hand-rolled COM, paths.rs/waveout.rs style, no crates (Cargo.toml note).
// Vtable slot order and GUIDs verified against the ground-truth build
// headers (mingw-w64 audioclient.h / mmdeviceapi.h / propsys.h / propidl.h /
// audiosessiontypes.h / ksmedia.h) and re-printed live by
// %TEMP%\opencode\audiobt\gt.exe (2026-10-02): CLSID_MMDeviceEnumerator is
// {bcde0395,e52f,467c,8e3d,c4579291692e} (mmdeviceapi.h:965 — NOT the
// commonly quoted d5d4-4e82 variant), IID_IMMDeviceCollection
// {0bd7a1be,...} (mmdeviceapi.h:415), IID_IAudioClient2 {726778cd,...}
// (audioclient.h:415), IID_IAudioClock {cd63314f,3fba,4a1b,812c,
// ef96358728e7} (audioclient.h:1082), IAudioClock slot 5 is
// GetCharacteristics (audioclient.h:1094, not GetBufferSize),
// AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED = 0x88890019 (audioclient.h:1867,
// FACILITY_AUDCLNT 2185 winerror.h:143).
//
// Deviations (disclosed, audited):
// - m_err (audio_out.h:213, plain std::string raced in C++) is a
//   Mutex<String>. Written only at setup/error paths, never in the audio
//   hot loop; read by main at the same points disk reads it (:151, :229).
//   Zero locks in the callback.
// - The fill callback (m_fill, audio_out.h:209) MOVES into the thread (Rust
//   ownership). C++ main kept the generator on its own stack and raced its
//   counters; live.rs bridges produced/busy/worst/cushion through atomics
//   updated at the SAME points as disk (audio_out.cpp:589/661,
//   live.cpp:168-169/223) and reclaims the whole bundle after stop()
//   (= join) so live.cpp:558-559/:573-578 read the exact same numbers.
// - m_qpc_freq (audio_out.h:236) rides in Shared as an atomic: written in
//   start() before the spawn (:131-133), read by the thread only at :675 —
//   write-before-spawn makes any ordering safe.
// - `goto done` (:296..:643) → labelled block `break 'body`; the cleanup at
//   :686-712 runs on every path in the same order. CoUninitialize runs even
//   when CoInitializeEx failed — disk's `goto done` from :259-262 falls
//   through :712 the same way (ported verbatim; the thread dies right
//   after, so the imbalance is inert).
// - endpoint_name's byte string → String::from_utf8_lossy (C++ kept raw
//   bytes in std::string; names on real endpoints are valid UTF-8 — only
//   the invalid-UTF-8 tail could differ).

use std::ffi::c_void;
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::sys;

// audio_out.h:50 constexpr u32 AUDIO_RATE = 44100
pub const AUDIO_RATE: u32 = 44100;

// ---------------------------------------------------------------------------
// resampler.h — 窓関数付き sinc の畳み込み。64 本で阻止域 -74dB くらい
// (resampler.h:10). **遅れは足さない** (resampler.h:17-18).
// ---------------------------------------------------------------------------

// The GT math: build_table must emit the very bits the mingw-built ground
// truth produces, and NO DLL does — gt.exe has no sin/cos import at all;
// the mingw 16.1 double sin/cos link STATICALLY from libmsvcrt.a(sin.o/
// cos.o), which computes through x87 long-double __sinl_internal/
// __cosl_internal (proof chain in build.rs + session log 2026-10-03:
// sin(PI) via msvcrt = 0x3ca1a62633145c07, via the static x87 object =
// 0x3ca1a60000000000 — only the latter reproduces golden tab[256]
// 0x24330e6e). build.rs extracts those EXACT archive members (renamed so
// the MSVC linker cannot resolve the bare `sin` to the UCRT) and links
// them as gtmath.lib; the pins below + the full table_cpp.bin compare in
// table_golden gate the bits. An earlier pass of this row believed
// "msvcrt.dll imports" — wrong (objdump of the import table + link map
// settled it); that version was red on exactly tab[256].

extern "system" {
    #[link_name = "smu_gt_sin"] // libmsvcrt.a sin.o -> __sinl_internal (x87)
    fn smu_gt_sin(x: f64) -> f64;
    #[link_name = "smu_gt_cos"] // libmsvcrt.a cos.o -> __cosl_internal (x87)
    fn smu_gt_cos(x: f64) -> f64;
}

// _errno / __mingw_raise_matherr: CRT hooks sin.o/cos.o reference ONLY on
// their TDOMAIN/TLEN argument-error arms — unreachable for the finite
// table-build inputs. Explicit zero-stubs (never a Default device state).
static mut GT_ERRNO: i32 = 0;

#[no_mangle]
unsafe extern "system" fn _errno() -> *mut i32 {
    std::ptr::addr_of_mut!(GT_ERRNO) // mingw errno macro = (*_errno())
}

/// mingw math.h `void __mingw_raise_matherr(int, char*, double, double, double)`
#[no_mangle]
unsafe extern "system" fn __mingw_raise_matherr(
    _kind: i32,
    _name: *mut c_void,
    _x1: f64,
    _x2: f64,
    _ret: f64,
) {
}

// resampler.h:113-118
const TAPS: i32 = 64;
const HALF: i32 = TAPS / 2; // 32
const STEPS: i32 = 256; // 1 サンプル間隔あたりの表の刻み
const RING: usize = 4096;
const RMASK: i64 = (RING as i64) - 1;

/// origin: resampler.h:34-178 class resampler
pub struct Resampler {
    m_tab: Vec<f32>, // :170 (empty == not yet built, checked at :47)
    m_ring_l: [f32; RING], // :171 float m_ring_l[RING] = {}
    m_ring_r: [f32; RING], // :172
    m_written: i64, // :173
    m_pos: f64, // :174
    m_step: f64, // :175
    m_cutoff: f64, // :176
    m_direct: bool, // :177
}

impl Resampler {
    /// ctor: fields == resampler.h:170-177 initializers, explicit
    pub fn new() -> Resampler {
        Resampler {
            m_tab: Vec::new(), // :170 empty vector
            m_ring_l: [0.0f32; RING], // :171 = {}
            m_ring_r: [0.0f32; RING], // :172 = {}
            m_written: 0, // :173
            m_pos: 0.0, // :174
            m_step: 1.0, // :175
            m_cutoff: 1.0, // :176
            m_direct: true, // :177
        }
    }

    /// origin: resampler.h:38-50 configure — 同じなら direct() が真になり、
    /// 畳み込みを丸ごと省く
    pub fn configure(&mut self, mut in_rate: f64, mut out_rate: f64) {
        if in_rate <= 0.0 {
            in_rate = 44100.0; // :40
        }
        if out_rate <= 0.0 {
            out_rate = in_rate; // :41
        }
        self.m_direct = (in_rate - out_rate).abs() < 1e-6; // :42 std::fabs
        self.m_step = in_rate / out_rate; // :43
        // :44-46 上へ変換するときは入力のナイキストまで通す。下へ変換すると
        // きは出力のナイキストで切らないと折り返す —— std::min(1.0, out/in)
        let lim = if out_rate / in_rate < 1.0 {
            out_rate / in_rate
        } else {
            1.0
        };
        self.m_cutoff = lim * 0.955; // :46
        if self.m_tab.is_empty() {
            self.build_table(); // :47-48
        }
        self.reset(); // :49
    }

    /// resampler.h:52 bool direct()
    pub fn direct(&self) -> bool {
        self.m_direct
    }

    /// origin: resampler.h:55-61 reset — 中身を捨てる
    pub fn reset(&mut self) {
        self.m_ring_l = [0.0f32; RING]; // :57 memset(m_ring_l, 0, ...)
        self.m_ring_r = [0.0f32; RING]; // :58
        self.m_written = 0; // :59
        self.m_pos = 0.0; // :60
    }

    /// origin: resampler.h:64-74 input_needed — out フレーム出すのに、
    /// あと何フレーム入力が要るか
    pub fn input_needed(&self, out_frames: i32) -> i32 {
        if self.m_direct {
            return out_frames; // :66-67
        }
        if out_frames <= 0 {
            return 0; // :68-69
        }
        // :70-71 最後の 1 フレームの畳み込みに要る、いちばん先のサンプル
        let last = self.m_pos + self.m_step * (out_frames - 1) as f64;
        let need = last.floor() as i64 + HALF as i64 + 1; // :72
        (need - self.m_written).max(0) as i32 // :73 std::max<s64>(0, ...)
    }

    /// origin: resampler.h:77-85 output_available (録音のように入力が先に来る
    /// 向きで使う — the live path has no caller; ported for contract parity)
    pub fn output_available(&self) -> i32 {
        if self.m_direct {
            return (self.m_written - self.m_pos as i64).max(0) as i32; // :80
        }
        let room = (self.m_written - HALF as i64 - 1) as f64 - self.m_pos; // :81
        if room < 0.0 {
            return 0; // :82-83
        }
        (room / self.m_step).floor() as i32 + 1 // :84
    }

    /// origin: resampler.h:88-96 push — 16bit 2ch インタリーブで入れる
    pub fn push(&mut self, input: &[i16], frames: i32) {
        let k = 1.0f32 / 32768.0f32; // :90
        for i in 0..frames as usize {
            // :92-93 float(in[i*2+0]) * k, ring index wraps
            self.m_ring_l[(self.m_written & RMASK) as usize] = input[i * 2] as f32 * k;
            self.m_ring_r[(self.m_written & RMASK) as usize] = input[i * 2 + 1] as f32 * k;
            self.m_written += 1; // :94
        }
    }

    /// origin: resampler.h:99-107 pull — float 2ch インタリーブで出す
    pub fn pull(&mut self, out: &mut [f32], frames: i32) {
        for i in 0..frames as usize {
            // :101-105
            let (l, r) = self.one();
            out[i * 2] = l;
            out[i * 2 + 1] = r;
        }
    }

    /// resampler.h:110 written — 入力に積んだ数（入れ過ぎの確認用）
    pub fn written(&self) -> i64 {
        self.m_written
    }

    /// origin: resampler.h:120-133 build_table — ブラックマン窓.
    /// sin/cos call the ground truth's OWN static objects (see the FFI
    /// block above + build.rs): rustc's Celestial `f64::sin` and even
    /// msvcrt.dll's sin differ at x=π where the table is razor-thin
    /// (golden tab[256] 0x24330e6e; celestial 0x24331292, msvcrt
    /// 0x24330ff2). `table_golden` pins the full table against the
    /// proven table_cpp.bin (SHA1 645AADA4834B09F276F16202CCBB27CC2057AACD).
    /// Deviation vs disk: HOW the math is reached — link-time extraction
    /// of the same libmsvcrt.a/libmingwex.a members, same machine code.
    fn build_table(&mut self) {
        let n = (HALF as usize) * (STEPS as usize) + 2; // :122
        self.m_tab.resize(n, 0.0);
        for k in 0..n {
            let d = k as f64 / STEPS as f64; // :124 中心からの距離
            let x: f64 = 3.14159265358979323846 * d; // :125
            let sinc: f64 = if k == 0 { 1.0 } else { unsafe { smu_gt_sin(x) / x } }; // :126
            let t = (d + HALF as f64) / TAPS as f64; // :128
            // :129-130 0.42 - 0.5cos(2πt) + 0.08cos(4πt)
            let w: f64 = 0.42
                - 0.5 * unsafe { smu_gt_cos(2.0 * 3.14159265358979323846 * t) }
                + 0.08 * unsafe { smu_gt_cos(4.0 * 3.14159265358979323846 * t) };
            self.m_tab[k] = (sinc * w) as f32; // :131
        }
    }

    /// origin: resampler.h:135-168 one
    fn one(&mut self) -> (f32, f32) {
        let centre = self.m_pos.floor() as i64; // :137 s64(std::floor(m_pos))
        if self.m_direct {
            let idx = centre; // :139
            let l = self.m_ring_l[(idx & RMASK) as usize]; // :140
            let r = self.m_ring_r[(idx & RMASK) as usize]; // :141
            self.m_pos += 1.0; // :142
            return (l, r);
        }
        let mut al = 0.0f64;
        let mut ar = 0.0f64;
        let mut sum = 0.0f64; // :145
        let mut k = -HALF + 1; // :146
        while k <= HALF {
            let idx = centre + k as i64; // :147
            if idx < 0 || idx >= self.m_written {
                k += 1; // :148-149 continue
                continue;
            }
            let d = ((self.m_pos - idx as f64) * self.m_cutoff).abs(); // :150
            let fx = d * STEPS as f64; // :151
            let j = fx as usize; // :152 size_t(fx) trunc
            if j + 1 >= self.m_tab.len() {
                k += 1; // :153-154 continue
                continue;
            }
            let t = fx - j as f64; // :155
            // :156 float tab[] promoted into the double expression
            let h = self.m_tab[j] as f64
                + (self.m_tab[j + 1] as f64 - self.m_tab[j] as f64) * t;
            al += h * self.m_ring_l[(idx & RMASK) as usize] as f64; // :157
            ar += h * self.m_ring_r[(idx & RMASK) as usize] as f64; // :158
            sum += h; // :159
            k += 1;
        }
        if sum > 1e-9 {
            // :161-164
            al /= sum;
            ar /= sum;
        }
        let l = al.clamp(-1.0, 1.0) as f32; // :165 std::clamp then float()
        let r = ar.clamp(-1.0, 1.0) as f32; // :166
        self.m_pos += self.m_step; // :167
        (l, r)
    }
}

// ---------------------------------------------------------------------------
// COM / WASAPI FFI — hand-rolled (Cargo.toml: no crates)
// ---------------------------------------------------------------------------

pub type HResult = i32;
type Obj = *mut c_void;

// wtypes.h GUID — gt: sizeof GUID 16
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

const fn g(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    Guid { data1: d1, data2: d2, data3: d3, data4: d4 }
}

// audio_out.cpp:69-72 PKEY_Device_FriendlyName — {a45c254e-df1c-4efd-8020-
// 67d146a850e0} の 14 番。INITGUID を持ち込むと他と衝突するので直に書く
const K_FRIENDLY_NAME: PropertyKey = PropertyKey {
    fmtid: g(0xa45c254e, 0xdf1c, 0x4efd, [0x80, 0x20, 0x67, 0xd1, 0x46, 0xa8, 0x50, 0xe0]),
    pid: 14,
};

// All nine GUIDs below: DEFINE_GUID line in the mingw header + re-printed by
// gt.exe (test `guid_pins`).
pub const CLSID_MM_DEVICE_ENUMERATOR: Guid =
    g(0xbcde0395, 0xe52f, 0x467c, [0x8e, 0x3d, 0xc4, 0x57, 0x92, 0x91, 0x69, 0x2e]); // mmdeviceapi.h:965
pub const IID_IMM_DEVICE_ENUMERATOR: Guid =
    g(0xa95664d2, 0x9614, 0x4f35, [0xa7, 0x46, 0xde, 0x8d, 0xb6, 0x36, 0x17, 0xe6]); // mmdeviceapi.h:582
pub const IID_IMM_DEVICE_COLLECTION: Guid =
    g(0x0bd7a1be, 0x7a1a, 0x44db, [0x83, 0x97, 0xcc, 0x53, 0x92, 0x38, 0x7b, 0x5e]); // mmdeviceapi.h:415
pub const IID_IMM_DEVICE: Guid =
    g(0xd666063f, 0x1587, 0x4e43, [0x81, 0xf1, 0xb9, 0x48, 0xe8, 0x07, 0x36, 0x3f]); // mmdeviceapi.h:297
pub const IID_I_PROPERTY_STORE: Guid =
    g(0x886d8eeb, 0x8cf2, 0x4446, [0x8d, 0x02, 0xcd, 0xba, 0x1d, 0xbd, 0xcf, 0x99]); // propsys.h:460
pub const IID_IAUDIO_CLIENT: Guid =
    g(0x1cb9ad4c, 0xdbfa, 0x4c32, [0xb1, 0x78, 0xc2, 0xf5, 0x68, 0xa7, 0x03, 0xb2]); // audioclient.h:201
pub const IID_IAUDIO_CLIENT2: Guid =
    g(0x726778cd, 0xf60a, 0x4eda, [0x82, 0xde, 0xe4, 0x76, 0x10, 0xcd, 0x78, 0xaa]); // audioclient.h:415
pub const IID_IAUDIO_RENDER_CLIENT: Guid =
    g(0xf294acfc, 0x3146, 0x4483, [0xa7, 0xbf, 0xad, 0xdc, 0xa7, 0xc2, 0x60, 0xe2]); // audioclient.h:879
pub const IID_IAUDIO_CLOCK: Guid =
    g(0xcd63314f, 0x3fba, 0x4a1b, [0x81, 0x2c, 0xef, 0x96, 0x35, 0x87, 0x28, 0xe7]); // audioclient.h:1082

// objbase.h COINIT_MULTITHREADED=0 · CLSCTX_ALL (combaseapi.h:135) =23 ·
// objbase.h STGM_READ=0 · winerror.h E_FAIL=0x80004005
const COINIT_MULTITHREADED: u32 = 0;
const CLSCTX_ALL: u32 = 23;
const STGM_READ: u32 = 0;
const E_FAIL: HResult = 0x8000_4005u32 as i32;
// audiosessiontypes.h:29-30 AUDCLNT_SHAREMODE_SHARED/EXCLUSIVE
pub const AUDCLNT_SHAREMODE_SHARED: u32 = 0;
pub const AUDCLNT_SHAREMODE_EXCLUSIVE: u32 = 1;
// audiosessiontypes.h:50/:53/:54
pub const AUDCLNT_STREAMFLAGS_EVENTCALLBACK: u32 = 0x0004_0000;
pub const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
pub const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
// audioclient.h:1867 AUDCLNT_ERR(0x19) with FACILITY_AUDCLNT=2185
// (winerror.h:143) → 0x88890019 (gt: aligned 0x88890019)
pub const AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED: HResult = 0x8889_0019u32 as i32;
// audiosessiontypes.h:45 AudioCategory_Media (gt: enum Media 11)
const AUDIO_CATEGORY_MEDIA: u32 = 11;
// audioclient.h:159 AUDCLNT_STREAMOPTIONS_RAW = 0x1
const AUDCLNT_STREAMOPTIONS_RAW: u32 = 0x1;
// mmreg.h tags; ksmedia.h:622/628 pin the SubFormat Data1 1/3 used at :61
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
// ksmedia.h:1103/1104 SPEAKER_FRONT_LEFT/RIGHT
const SPEAKER_FRONT_LEFT: u32 = 0x1;
const SPEAKER_FRONT_RIGHT: u32 = 0x2;
// mmdeviceapi.h:136 eRender, :142 eConsole, :111 DEVICE_STATE_ACTIVE
const E_RENDER: i32 = 0;
const E_CONSOLE: i32 = 0;
const DEVICE_STATE_ACTIVE: u32 = 0x1;
// winbase.h WAIT_OBJECT_0
const WAIT_OBJECT_0: u32 = 0;
// winbase.h CP_UTF8
const CP_UTF8: u32 = 65001;

/// origin: mmreg.h WAVEFORMATEX (pshpack1 — gt: size 18). Same packed shape
/// as waveout.rs:39-49 (same header family).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct WaveFormatEx {
    pub w_format_tag: u16,
    pub n_channels: u16,
    pub n_samples_per_sec: u32,
    pub n_avg_bytes_per_sec: u32,
    pub n_block_align: u16,
    pub w_bits_per_sample: u16,
    pub cb_size: u16,
}

/// origin: mmreg.h WAVEFORMATEXTENSIBLE (pshpack1 — gt: size 40,
/// Format@0 Samples@18 dwChannelMask@20 SubFormat@24)
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct WaveFormatExtensible {
    pub format: WaveFormatEx,
    pub samples: u16, // union wValidBitsPerSample / wSamplesPerBlock / wReserved
    pub channel_mask: u32,
    pub sub_format: Guid,
}

/// origin: propidl.h:224-309 tagPROPVARINAT (gt: size 24, vt@0, pwszVal@8)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PropVariant {
    pub vt: u16,
    pub r1: u16,
    pub r2: u16,
    pub r3: u16,
    pub u: [u64; 2], // union body; pwszVal == first 8 bytes
}

/// origin: audioclient.h:175-180 AudioClientProperties (gt: size 16 =
/// cb@0 off@4 cat@8 opt@12)
#[repr(C)]
struct AudioClientProperties {
    cb_size: u32,
    b_is_offload: i32,
    e_category: u32,
    options: u32,
}

/// origin: wtypes.h PROPERTYKEY (gt: size 20, fmtid@0, pid@16)
#[repr(C)]
#[derive(Clone, Copy)]
struct PropertyKey {
    fmtid: Guid,
    pid: u32,
}

// ---- vtables. Slot order verified against each interface's method listing
// in the headers (all four were Read line-by-line 2026-10-02); `_pad` slots
// are declared-but-never-called tail methods kept ONLY so the slot indices
// of everything above stay pinned to the ABI. ----

type FnQi = unsafe extern "system" fn(Obj, *const Guid, *mut *mut c_void) -> HResult;
type FnVoidU32 = unsafe extern "system" fn(Obj) -> u32;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: FnQi,
    add_ref: FnVoidU32,
    release: FnVoidU32,
}

/// origin: mmdeviceapi.h:584-606 IMMDeviceEnumerator — EnumAudioEndpoints@3,
/// GetDefaultAudioEndpoint@4; pad GetDevice@5, Register@6, Unregister@7
#[repr(C)]
struct ImmDeviceEnumeratorVtbl {
    unknown: IUnknownVtbl,
    enum_audio_endpoints: unsafe extern "system" fn(Obj, i32, u32, *mut Obj) -> HResult,
    get_default_audio_endpoint: unsafe extern "system" fn(Obj, i32, i32, *mut Obj) -> HResult,
    _pad: [usize; 3],
}

/// origin: mmdeviceapi.h:417-424 IMMDeviceCollection — GetCount@3, Item@4
#[repr(C)]
struct ImmDeviceCollectionVtbl {
    unknown: IUnknownVtbl,
    get_count: unsafe extern "system" fn(Obj, *mut u32) -> HResult,
    item: unsafe extern "system" fn(Obj, u32, *mut Obj) -> HResult,
}

/// origin: mmdeviceapi.h:299-315 IMMDevice — Activate@3, OpenPropertyStore@4;
/// pad GetId@5, GetState@6
#[repr(C)]
struct ImmDeviceVtbl {
    unknown: IUnknownVtbl,
    activate: unsafe extern "system" fn(Obj, *const Guid, u32, *mut c_void, *mut Obj) -> HResult,
    open_property_store: unsafe extern "system" fn(Obj, u32, *mut Obj) -> HResult,
    _pad: [usize; 2],
}

/// origin: propsys.h:462-483 IPropertyStore — GetCount@3, GetAt@4,
/// GetValue@5; pad SetValue@6, Commit@7
#[repr(C)]
struct IPropertyStoreVtbl {
    unknown: IUnknownVtbl,
    _get_count: usize,
    _get_at: usize,
    get_value: unsafe extern "system" fn(Obj, *const PropertyKey, *mut PropVariant) -> HResult,
    _pad: [usize; 2],
}

/// origin: audioclient.h:203-251 IAudioClient — 12 methods after IUnknown
/// in exactly this order (Initialize@3 … GetService@14); 15 slots total
#[repr(C)]
struct IAudioClientVtbl {
    unknown: IUnknownVtbl,
    initialize:
        unsafe extern "system" fn(Obj, u32, u32, i64, i64, *const WaveFormatEx, *const Guid) -> HResult, // :206-212
    get_buffer_size: unsafe extern "system" fn(Obj, *mut u32) -> HResult, // :214-215
    get_stream_latency: unsafe extern "system" fn(Obj, *mut i64) -> HResult, // :217-218
    get_current_padding: unsafe extern "system" fn(Obj, *mut u32) -> HResult, // :220-221
    is_format_supported:
        unsafe extern "system" fn(Obj, u32, *const WaveFormatEx, *mut *mut WaveFormatEx) -> HResult, // :223-226
    get_mix_format: unsafe extern "system" fn(Obj, *mut *mut WaveFormatEx) -> HResult, // :228-229
    get_device_period: unsafe extern "system" fn(Obj, *mut i64, *mut i64) -> HResult, // :231-233
    start: unsafe extern "system" fn(Obj) -> HResult, // :235-236 HRESULT, not ULONG
    stop: unsafe extern "system" fn(Obj) -> HResult,  // :238-239
    reset: unsafe extern "system" fn(Obj) -> HResult, // :241-242
    set_event_handle: unsafe extern "system" fn(Obj, *mut c_void) -> HResult, // :244-245
    get_service: unsafe extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult, // :247-249
}

/// origin: audioclient.h:417-433 IAudioClient2 : IAudioClient —
/// IsOffloadCapable@15 (pad), SetClientProperties@16, GetBufferSizeLimits@17
/// (pad)
#[repr(C)]
struct IAudioClient2Vtbl {
    client: IAudioClientVtbl,
    _is_offload_capable: usize,
    set_client_properties: unsafe extern "system" fn(Obj, *const AudioClientProperties) -> HResult,
    _pad: usize,
}

/// origin: audioclient.h:881-892 IAudioRenderClient — GetBuffer@3,
/// ReleaseBuffer@4
#[repr(C)]
struct IAudioRenderClientVtbl {
    unknown: IUnknownVtbl,
    get_buffer: unsafe extern "system" fn(Obj, u32, *mut *mut u8) -> HResult,
    release_buffer: unsafe extern "system" fn(Obj, u32, u32) -> HResult,
}

/// origin: audioclient.h:1084-1097 IAudioClock — GetFrequency@3,
/// GetPosition@4; pad GetCharacteristics@5 (NOT GetBufferSize)
#[repr(C)]
struct IAudioClockVtbl {
    unknown: IUnknownVtbl,
    get_frequency: unsafe extern "system" fn(Obj, *mut u64) -> HResult,
    get_position: unsafe extern "system" fn(Obj, *mut u64, *mut u64) -> HResult,
    _pad: usize,
}

// an object pointer points AT a pointer to the vtable
macro_rules! vtbl {
    ($obj:expr, $t:ty) => {
        &**($obj as *const *const $t)
    };
}

#[link(name = "ole32")]
extern "system" {
    // objbase.h — probe.exe (audiobt, 2026-10-02): imports through ole32.
    // link_name = the real export names (MSVC import libs, same rule as
    // crt_math above — the snake_case Rust names never reach the linker)
    #[link_name = "CoInitializeEx"] // CoInitializeEx
    fn co_initialize(p_pvreserved: *const c_void, dw_coinitflags: u32) -> HResult;
    #[link_name = "CoUninitialize"]
    fn co_uninitialize();
    #[link_name = "CoCreateInstance"]
    fn co_create_instance(
        rclsid: *const Guid,
        p_unkouter: *mut c_void,
        cls_context: u32,
        riid: *const Guid,
        ppv: *mut Obj,
    ) -> HResult;
    #[link_name = "CoTaskMemFree"]
    fn co_task_mem_free(p_pv: *mut c_void);
    #[link_name = "PropVariantClear"] // propidl, ole32 export
    fn prop_variant_clear(pvar: *mut PropVariant) -> HResult;
}

#[link(name = "kernel32")]
extern "system" {
    #[link_name = "WideCharToMultiByte"]
    fn wide_char_to_multi_byte(
        code_page: u32,
        dw_flags: u32,
        lp_wide_char_str: *const u16,
        c_wide_char: i32,
        lp_multi_byte_str: *mut u8,
        c_multi_byte: i32,
        lp_default_char: *const u8,
        lp_used_default_char: *mut i32,
    ) -> i32;
}

#[inline]
fn succeeded(hr: HResult) -> bool {
    hr >= 0 // winerror.h SUCCEEDED
}
#[inline]
fn failed(hr: HResult) -> bool {
    hr < 0 // winerror.h FAILED
}

/// origin: audio_out.cpp:20-29 is_float
fn is_float(f: *const WaveFormatEx) -> bool {
    if unsafe { (*f).w_format_tag } == WAVE_FORMAT_IEEE_FLOAT {
        return true; // :22-23
    }
    if unsafe { (*f).w_format_tag } == WAVE_FORMAT_EXTENSIBLE {
        // :25-26 reinterpret_cast<const WAVEFORMATEXTENSIBLE *>
        let e = unsafe { &*(f as *const WaveFormatExtensible) };
        return e.sub_format.data1 == 3; // :26 KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    }
    false // :28
}

/// origin: audio_out.cpp:31-42 is_pcm16
fn is_pcm16(f: *const WaveFormatEx) -> bool {
    if unsafe { (*f).w_bits_per_sample } != 16 {
        return false; // :33-34
    }
    if unsafe { (*f).w_format_tag } == WAVE_FORMAT_PCM {
        return true; // :35-36
    }
    if unsafe { (*f).w_format_tag } == WAVE_FORMAT_EXTENSIBLE {
        let e = unsafe { &*(f as *const WaveFormatExtensible) };
        return e.sub_format.data1 == 1; // :39 KSDATAFORMAT_SUBTYPE_PCM
    }
    false // :41
}

/// origin: audio_out.cpp:44-45 enum class devfmt { f32, i16, i24in32 } —
/// デバイスへ渡す形。24bit は 32bit の器に左詰めで入れる（RME の本来の形式）
#[derive(Clone, Copy, PartialEq, Eq)]
enum DevFmt {
    F32,
    I16,
    I24In32,
}

/// origin: audio_out.cpp:47-67 make_format — 独り占めモードで試す形式を組む
fn make_format(rate: u32, flt: bool, bits: i32, container: i32) -> WaveFormatExtensible {
    let mut e = WaveFormatExtensible {
        format: WaveFormatEx {
            w_format_tag: 0,
            n_channels: 0,
            n_samples_per_sec: 0,
            n_avg_bytes_per_sec: 0,
            n_block_align: 0,
            w_bits_per_sample: 0,
            cb_size: 0,
        },
        samples: 0,
        channel_mask: 0,
        sub_format: g(0, 0, 0, [0; 8]),
    }; // :50 WAVEFORMATEXTENSIBLE e{}
    e.format.w_format_tag = WAVE_FORMAT_EXTENSIBLE; // :51
    e.format.n_channels = 2; // :52
    e.format.n_samples_per_sec = rate; // :53
    e.format.w_bits_per_sample = container as u16; // :54 WORD(container)
    e.format.n_block_align = (2 * container / 8) as u16; // :55 WORD(2*container/8)
    e.format.n_avg_bytes_per_sec = rate.wrapping_mul(e.format.n_block_align as u32); // :56
    e.format.cb_size = (std::mem::size_of::<WaveFormatExtensible>()
        - std::mem::size_of::<WaveFormatEx>()) as u16; // :57
    e.samples = bits as u16; // :58 Samples.wValidBitsPerSample
    e.channel_mask = SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT; // :59
    // :60-65 KSDATAFORMAT_SUBTYPE_{PCM,IEEE_FLOAT}
    e.sub_format.data1 = if flt { 3 } else { 1 }; // :61
    e.sub_format.data2 = 0x0000; // :62
    e.sub_format.data3 = 0x0010; // :63
    e.sub_format.data4 = [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71]; // :64-65
    e
}

/// origin: audio_out.cpp:74-91 endpoint_name — 口の名前を取る
unsafe fn endpoint_name(d: Obj) -> String {
    let mut out = String::new(); // :77
    let mut ps: Obj = std::ptr::null_mut();
    if failed((vtbl!(d, ImmDeviceVtbl).open_property_store)(d, STGM_READ, &mut ps)) {
        return out; // :79-80
    }
    let mut v = PropVariant { vt: 0, r1: 0, r2: 0, r3: 0, u: [0; 2] }; // :81 PROPVARIANT v
    // (PropVariantInit(:82) == VT_EMPTY + union zero == all-zero)
    // :83 SUCCEEDED(ps->GetValue(kFriendlyName, &v)) && v.pwszVal
    if succeeded((vtbl!(ps, IPropertyStoreVtbl).get_value)(ps, &K_FRIENDLY_NAME, &mut v))
        && v.u[0] != 0
    {
        let mut buf = [0u8; 256]; // :84 char buf[256] = {}
        // :85 WideCharToMultiByte(CP_UTF8, 0, v.pwszVal, -1, buf, 255, 0, 0)
        wide_char_to_multi_byte(
            CP_UTF8,
            0,
            v.u[0] as *const u16,
            -1,
            buf.as_mut_ptr(),
            255,
            std::ptr::null(),
            std::ptr::null_mut(),
        );
        let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        out = String::from_utf8_lossy(&buf[..n]).into_owned(); // :86 (lossy deviation)
    }
    prop_variant_clear(&mut v); // :88 PropVariantClear
    (vtbl!(ps, IUnknownVtbl).release)(ps); // :89 ps->Release()
    out
}

// ---------------------------------------------------------------------------
// AudioOut — origin: audio_out.h:134-237 (Windows arm) + audio_out.cpp
// ---------------------------------------------------------------------------

/// origin: audio_out.h:137-138 using fill_fn = void(s16*, u32) —
/// 16bit 2ch インタリーブで frames サンプルぶん書く（44100Hz）. Moved into
/// the thread (deviation note at file top).
pub trait Fill: Send + 'static {
    fn fill(&mut self, out: &mut [i16]);
}

/// the m_* members of audio_out.h:211-236 that cross the thread boundary.
/// Every field explicitly initialized == the disk in-class initializer
/// (ledger hard rule).
struct Shared {
    quit: AtomicBool, // :211 m_quit{false}
    running: AtomicBool, // :212 m_running{false}
    err: Mutex<String>, // :213 m_err (mutex deviation, header note)
    buffer_frames: AtomicU32, // :215 m_buffer_frames{0}
    produced: AtomicU64, // :216 m_produced{0}
    late: AtomicU64, // :216 m_late{0}
    slack_min: AtomicU64, // :217 m_slack_min{~u64(0)}
    busy_ticks: AtomicU64, // :218 m_busy_ticks{0}
    worst_ticks: AtomicU64, // :218 m_worst_ticks{0}
    mmcss: AtomicBool, // :219 m_mmcss{false}
    start_state: AtomicI32, // :220-222 m_start_state{0} 0 待ち / 1 動いた / 2 だめ
    dev_rate: AtomicU32, // :223 m_dev_rate{AUDIO_RATE}
    dev_channels: AtomicU32, // :223 m_dev_channels{2}
    converting: AtomicBool, // :224 m_converting{false}
    exclusive_f: AtomicBool, // :225 m_exclusive{false}
    period_ms: AtomicU64, // :226 m_period_ms{0.0} — f64 bits
    stream_ms: AtomicU64, // :226 m_stream_ms{0.0} — f64 bits
    dev_bits: AtomicI32, // :227 m_dev_bits{16}
    dev_float: AtomicBool, // :228 m_dev_float{false}
    target_frames: AtomicU32, // :229 m_target_frames{0}
    queue_sum: AtomicU64, // :230 m_queue_sum{0}
    queue_n: AtomicU64, // :230 m_queue_n{0}
    queue_worst: AtomicU64, // :230 m_queue_worst{0}
    inflight_sum: AtomicU64, // :231 m_inflight_sum{0}
    inflight_n: AtomicU64, // :231 m_inflight_n{0}
    inflight_worst: AtomicU64, // :231 m_inflight_worst{0}
    dev_name: Mutex<String>, // :233 m_dev_name (thread writes at :322; deviation mutex)
    raw_f: AtomicBool, // :235 m_raw{false}
    qpc_freq: AtomicI64, // :236 m_qpc_freq{1} (atomic deviation, header note)
}

#[inline]
fn set_f64(a: &AtomicU64, v: f64) {
    a.store(v.to_bits(), Ordering::SeqCst);
}
#[inline]
fn get_f64(a: &AtomicU64) -> f64 {
    f64::from_bits(a.load(Ordering::SeqCst))
}

impl Shared {
    /// every default mirrors the audio_out.h:211-236 initializer
    fn new() -> Shared {
        Shared {
            quit: AtomicBool::new(false),
            running: AtomicBool::new(false),
            err: Mutex::new(String::new()),
            buffer_frames: AtomicU32::new(0),
            produced: AtomicU64::new(0),
            late: AtomicU64::new(0),
            slack_min: AtomicU64::new(u64::MAX),
            busy_ticks: AtomicU64::new(0),
            worst_ticks: AtomicU64::new(0),
            mmcss: AtomicBool::new(false),
            start_state: AtomicI32::new(0),
            dev_rate: AtomicU32::new(AUDIO_RATE),
            dev_channels: AtomicU32::new(2),
            converting: AtomicBool::new(false),
            exclusive_f: AtomicBool::new(false),
            period_ms: AtomicU64::new(0.0f64.to_bits()),
            stream_ms: AtomicU64::new(0.0f64.to_bits()),
            dev_bits: AtomicI32::new(16),
            dev_float: AtomicBool::new(false),
            target_frames: AtomicU32::new(0),
            queue_sum: AtomicU64::new(0),
            queue_n: AtomicU64::new(0),
            queue_worst: AtomicU64::new(0),
            inflight_sum: AtomicU64::new(0),
            inflight_n: AtomicU64::new(0),
            inflight_worst: AtomicU64::new(0),
            dev_name: Mutex::new(String::new()),
            raw_f: AtomicBool::new(false),
            qpc_freq: AtomicI64::new(1),
        }
    }
}

/// origin: audio_out.h:134 class audio_out (Windows arm). Generic over the
/// fill bundle (deviation: Rust moves it into the thread; take_fill() hands
/// it back after stop()).
pub struct AudioOut<G: Fill> {
    st: Arc<Shared>,
    thread: Option<JoinHandle<()>>, // :210 m_thread
    fill_rx: Option<std::sync::mpsc::Receiver<G>>, // deviation support
    cap_path: String, // :232 m_cap_path
    want_dev: String, // :233 m_want_dev
    want_raw: bool, // :234 m_want_raw = false
}

impl<G: Fill> AudioOut<G> {
    /// ctor: every field explicit
    pub fn new() -> AudioOut<G> {
        AudioOut {
            st: Arc::new(Shared::new()),
            thread: None,
            fill_rx: None,
            cap_path: String::new(),
            want_dev: String::new(),
            want_raw: false,
        }
    }

    /// origin: audio_out.h:198-200 set_capture — デバイスへ渡したバイト列を
    /// そのまま書き出す（切り分け用）。start() の前に呼ぶ
    pub fn set_capture(&mut self, path: &str) {
        self.cap_path = path.to_string();
    }

    /// origin: audio_out.cpp:124-155 start
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &mut self,
        latency_ms: i32,
        fill: G,
        err: &mut String,
        exclusive: bool,
        device: &str,
        raw: bool,
    ) -> bool {
        self.want_raw = raw; // :127 m_want_raw = raw
        if self.thread.is_some() {
            return true; // :128-129 if (m_thread.joinable()) return true
        }

        let f = sys::qpf(); // :131-133 LARGE_INTEGER f; QueryPerformanceFrequency
        self.st.qpc_freq.store(f, Ordering::SeqCst); // :133 m_qpc_freq = f.QuadPart

        // :135-140 m_fill = move; m_quit=false; m_err.clear(); m_want_dev;
        // m_start_state=0
        self.st.quit.store(false, Ordering::SeqCst); // :136
        *self.st.err.lock().unwrap() = String::new(); // :137
        self.want_dev = device.to_string(); // :139
        self.st.start_state.store(0, Ordering::SeqCst); // :140

        // :141-144 m_thread = std::thread([this, latency_ms, exclusive] {
        //     run(latency_ms, exclusive);
        //     m_start_state.store(m_running ? 1 : 2); });
        let st = Arc::clone(&self.st);
        let want_dev = self.want_dev.clone();
        let want_raw = self.want_raw;
        let cap_path = self.cap_path.clone();
        let (tx, rx) = std::sync::mpsc::channel::<G>();
        self.fill_rx = Some(rx);
        self.thread = Some(
            std::thread::spawn(move || {
                let mut fill = fill;
                unsafe {
                    run_inner(&st, latency_ms, exclusive, &want_dev, want_raw, &cap_path, &mut fill);
                }
                let _ = tx.send(fill); // deviation: hand the bundle back
                let running = st.running.load(Ordering::SeqCst); // :143
                st.start_state.store(if running { 1 } else { 2 }, Ordering::SeqCst);
            }),
        );

        // :146-148 開始に失敗したかどうかだけ待つ. だめなら理由を返す
        for _ in 0..400 {
            if self.st.start_state.load(Ordering::SeqCst) != 0
                || self.st.running.load(Ordering::SeqCst)
            {
                break;
            }
            sys::sleep_ms(5); // :148 Sleep(5)
        }
        if !self.st.running.load(Ordering::SeqCst)
            && self.st.start_state.load(Ordering::SeqCst) == 2
        {
            // :149-152
            if let Some(t) = self.thread.take() {
                let _ = t.join(); // :150
            }
            let e = self.st.err.lock().unwrap().clone();
            *err = if e.is_empty() { "音声デバイスを開けない".to_string() } else { e }; // :151
            return false;
        }
        true // :154
    }

    /// origin: audio_out.cpp:157-163 stop
    pub fn stop(&mut self) {
        self.st.quit.store(true, Ordering::SeqCst); // :159 m_quit.store(true)
        if let Some(t) = self.thread.take() {
            // :160-161 if (m_thread.joinable()) m_thread.join()
            let _ = t.join();
        }
        self.st.running.store(false, Ordering::SeqCst); // :162
    }

    /// deviation support: recover the fill bundle after stop() (disk's
    /// generator lived on main's stack; Rust returns it through the join).
    pub fn take_fill(&mut self) -> Option<G> {
        self.fill_rx.as_mut()?.recv().ok()
    }

    // ---- 進度 getters — すべて音声スレッドが書き、他所から読んでよい (audio_out.h:156) ----

    pub fn buffer_frames(&self) -> u32 {
        self.st.buffer_frames.load(Ordering::SeqCst) // audio_out.h:157
    }
    pub fn produced(&self) -> u64 {
        self.st.produced.load(Ordering::SeqCst) // :158
    }
    /// 間に合わなかった回数。**デバイスが待たされた回数** (audio_out.h:159-161)
    pub fn late(&self) -> u64 {
        self.st.late.load(Ordering::SeqCst)
    }
    /// 起きたときに残っていた最小の量（ミリ秒）。余裕の実測 (audio_out.h:162-163)
    pub fn slack_min_ms(&self) -> f64 {
        // origin: audio_out.cpp:207-212
        let v = self.st.slack_min.load(Ordering::SeqCst); // :209
        let r = self.st.dev_rate.load(Ordering::SeqCst); // :210
        if v == u64::MAX || r == 0 {
            0.0 // :211 (v == ~u64(0) || !r)
        } else {
            1000.0 * v as f64 / r as f64
        }
    }
    /// MMCSS（Pro Audio）に登録できたか (audio_out.h:164-165)
    pub fn mmcss(&self) -> bool {
        self.st.mmcss.load(Ordering::SeqCst)
    }
    pub fn running(&self) -> bool {
        self.st.running.load(Ordering::SeqCst) // :166
    }
    pub fn error(&self) -> String {
        self.st.err.lock().unwrap().clone() // :167 (mutex deviation)
    }

    /// origin: audio_out.cpp:165-173 cpu_percent
    pub fn cpu_percent(&self) -> f64 {
        let done = self.st.produced.load(Ordering::SeqCst); // :167
        if done == 0 {
            return 0.0; // :168-169
        }
        let audio = done as f64 / self.st.dev_rate.load(Ordering::SeqCst) as f64; // :170
        let busy = self.st.busy_ticks.load(Ordering::SeqCst) as f64
            / self.st.qpc_freq.load(Ordering::SeqCst) as f64; // :171
        100.0 * busy / audio // :172
    }

    /// origin: audio_out.cpp:175-178 worst_ms
    pub fn worst_ms(&self) -> f64 {
        1000.0 * self.st.worst_ticks.load(Ordering::SeqCst) as f64
            / self.st.qpc_freq.load(Ordering::SeqCst) as f64
    }

    /// origin: audio_out.cpp:180-184 buffer_ms
    pub fn buffer_ms(&self) -> f64 {
        let r = self.st.dev_rate.load(Ordering::SeqCst); // :182
        if r == 0 {
            0.0 // :183
        } else {
            1000.0 * self.st.buffer_frames.load(Ordering::SeqCst) as f64 / r as f64
        }
    }

    /// origin: audio_out.cpp:186-193 queue_ms — 起きたときに溜まっていた量
    pub fn queue_ms(&self) -> f64 {
        let n = self.st.queue_n.load(Ordering::SeqCst); // :188
        let r = self.st.dev_rate.load(Ordering::SeqCst); // :189
        if n == 0 || r == 0 {
            return 0.0; // :190-191
        }
        1000.0 * (self.st.queue_sum.load(Ordering::SeqCst) as f64 / n as f64) / r as f64 // :192
    }

    /// origin: audio_out.cpp:195-199 queue_worst_ms
    pub fn queue_worst_ms(&self) -> f64 {
        let r = self.st.dev_rate.load(Ordering::SeqCst);
        if r == 0 {
            0.0
        } else {
            1000.0 * self.st.queue_worst.load(Ordering::SeqCst) as f64 / r as f64
        }
    }

    /// origin: audio_out.cpp:201-205 target_ms
    pub fn target_ms(&self) -> f64 {
        let r = self.st.dev_rate.load(Ordering::SeqCst);
        if r == 0 {
            0.0
        } else {
            1000.0 * self.st.target_frames.load(Ordering::SeqCst) as f64 / r as f64
        }
    }

    /// origin: audio_out.cpp:214-221 inflight_ms — 書いたのに、まだ鳴っていない量
    pub fn inflight_ms(&self) -> f64 {
        let n = self.st.inflight_n.load(Ordering::SeqCst);
        let r = self.st.dev_rate.load(Ordering::SeqCst);
        if n == 0 || r == 0 {
            return 0.0;
        }
        1000.0 * (self.st.inflight_sum.load(Ordering::SeqCst) as f64 / n as f64) / r as f64
    }

    /// origin: audio_out.cpp:223-227 inflight_worst_ms
    pub fn inflight_worst_ms(&self) -> f64 {
        let r = self.st.dev_rate.load(Ordering::SeqCst);
        if r == 0 {
            0.0
        } else {
            1000.0 * self.st.inflight_worst.load(Ordering::SeqCst) as f64 / r as f64
        }
    }

    /// 実際に開いた口の名前 (audio_out.h:152-153 device_name)
    pub fn device_name(&self) -> String {
        self.st.dev_name.lock().unwrap().clone() // mutex deviation
    }
    pub fn device_rate(&self) -> u32 {
        self.st.dev_rate.load(Ordering::SeqCst) // :172
    }
    pub fn device_channels(&self) -> u32 {
        self.st.dev_channels.load(Ordering::SeqCst) // :173
    }
    pub fn converting(&self) -> bool {
        self.st.converting.load(Ordering::SeqCst) // :174
    }
    pub fn exclusive(&self) -> bool {
        self.st.exclusive_f.load(Ordering::SeqCst) // :175
    }
    /// エンジンの信号処理を飛ばせたか（共有モードのみ）(audio_out.h:176-177)
    pub fn raw(&self) -> bool {
        self.st.raw_f.load(Ordering::SeqCst)
    }
    pub fn period_ms(&self) -> f64 {
        get_f64(&self.st.period_ms) // :178
    }
    /// GetStreamLatency (audio_out.h:188 device_ms)
    pub fn device_ms(&self) -> f64 {
        get_f64(&self.st.stream_ms)
    }
    /// 書いた音が鳴るまでの見込み (audio_out.h:190 output_ms)
    pub fn output_ms(&self) -> f64 {
        self.queue_ms() + self.device_ms()
    }

    /// origin: audio_out.cpp:229-243 format_line — 人が読む行
    pub fn format_line(&self) -> String {
        // :233-239 snprintf("%s / %u Hz %u ch %s / 周期 %.1f ms / 変換 %s")
        let mode = if self.st.exclusive_f.load(Ordering::SeqCst) { "独り占め" } else { "共有" }; // :234
        let bits = self.st.dev_bits.load(Ordering::SeqCst);
        let fmt_s = if bits == 24 {
            "24bit(32)" // :236
        } else if self.st.dev_float.load(Ordering::SeqCst) {
            "float" // :237
        } else {
            "16bit"
        };
        let conv = if self.st.converting.load(Ordering::SeqCst) {
            "自前 sinc" // :239
        } else {
            "無し（44100 のまま）"
        };
        let mut s = format!(
            "{} / {} Hz {} ch {} / 周期 {:.1} ms / 変換 {}",
            mode,
            self.st.dev_rate.load(Ordering::SeqCst),
            self.st.dev_channels.load(Ordering::SeqCst),
            fmt_s,
            get_f64(&self.st.period_ms),
            conv
        );
        if self.st.raw_f.load(Ordering::SeqCst) {
            s.push_str(" / RAW"); // :240-241 strncat " / RAW"
        }
        s
    }

    /// origin: audio_out.cpp:245-255 latency_line — 待ち時間の内訳
    pub fn latency_line(&self) -> String {
        // :248-253
        format!(
            "溜め 目標 {:.1} / 実測 平均 {:.1} 最悪 {:.1} ms。**まだ鳴っていない量 平均 {:.1} 最悪 {:.1} ms**（GetStreamLatency {:.1} / 器 {:.1} ms）",
            self.target_ms(),
            self.queue_ms(),
            self.queue_worst_ms(),
            self.inflight_ms(),
            self.inflight_worst_ms(),
            self.device_ms(),
            self.buffer_ms()
        )
    }

    /// origin: audio_out.cpp:96-121 list — 使える再生デバイスの名前。
    /// 番号は挿し直すとずれるので、**名前で選ぶ**
    pub fn list() -> Vec<String> {
        let mut out: Vec<String> = Vec::new(); // :98
        // :99 const bool com = SUCCEEDED(CoInitializeEx(nullptr, COINIT_MULTITHREADED))
        let com = succeeded(unsafe { co_initialize(std::ptr::null(), COINIT_MULTITHREADED) });
        let mut en: Obj = std::ptr::null_mut();
        // :101-102 CoCreateInstance(__uuidof(MMDeviceEnumerator), ...)
        if unsafe {
            succeeded(co_create_instance(
                &CLSID_MM_DEVICE_ENUMERATOR,
                std::ptr::null_mut(),
                CLSCTX_ALL,
                &IID_IMM_DEVICE_ENUMERATOR,
                &mut en,
            ))
        } {
            let mut all: Obj = std::ptr::null_mut();
            // :104 EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE, &all)
            if unsafe {
                succeeded((vtbl!(en, ImmDeviceEnumeratorVtbl).enum_audio_endpoints)(
                    en,
                    E_RENDER,
                    DEVICE_STATE_ACTIVE,
                    &mut all,
                ))
            } {
                let mut n: u32 = 0; // :105 UINT n = 0
                unsafe { (vtbl!(all, ImmDeviceCollectionVtbl).get_count)(all, &mut n) }; // :106
                for i in 0..n {
                    // :107-113
                    let mut d: Obj = std::ptr::null_mut();
                    if unsafe { (vtbl!(all, ImmDeviceCollectionVtbl).item)(all, i, &mut d) } >= 0 {
                        out.push(unsafe { endpoint_name(d) }); // :110
                        unsafe { (vtbl!(d, IUnknownVtbl).release)(d) }; // :111
                    }
                }
                unsafe { (vtbl!(all, IUnknownVtbl).release)(all) }; // :114
            }
            unsafe { (vtbl!(en, IUnknownVtbl).release)(en) }; // :116
        }
        if com {
            unsafe { co_uninitialize() }; // :118-119
        }
        out // :120
    }
}

impl<G: Fill> Drop for AudioOut<G> {
    /// origin: audio_out.h:140 ~audio_out() { stop(); } — take_fill() must
    /// be called BEFORE drop (disk's m_fill dies with the object too).
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// origin: audio_out.cpp:257-713 run — handshake, event loop, teardown.
// `goto done` → labelled block 'body; cleanup :686-712 runs on every path.
// ---------------------------------------------------------------------------

/// origin: audio_out.cpp:355-374 — **周期は 2 の冪のフレーム数にする** +
/// the min_period growth loop. Pure; golden-tested offline.
/// Returns (frames, period_hns).
pub(crate) fn exclusive_period(latency_ms: i32, rate: u32, min_period: i64) -> (u32, i64) {
    let want_ms = if latency_ms > 0 { latency_ms as f64 } else { 10.0 }; // :362
    let mut frames = (want_ms * rate as f64 / 1000.0 + 0.5) as u32; // :363
    let mut pow2: u32 = 64; // :364
    while pow2 < frames && pow2 < 8192 {
        // :365-366
        pow2 <<= 1;
    }
    if pow2 > 64 && pow2.wrapping_sub(frames) > frames.wrapping_sub(pow2 >> 1) {
        pow2 >>= 1; // :367-368 下のほうが近ければそちら
    }
    frames = pow2; // :369
    let mut per = (10000000.0 * frames as f64 / rate as f64 + 0.5) as i64; // :370
    while per < min_period && frames < 8192 {
        // :371-374
        frames <<= 1;
        per = (10000000.0 * frames as f64 / rate as f64 + 0.5) as i64;
    }
    (frames, per)
}

/// origin: audio_out.cpp:466-474 — **目標は周期の倍数にする**. Pure;
/// golden-tested offline. Returns (target, period_frames).
pub(crate) fn shared_target(dev_rate: u32, want_ms: f64, def_period_hns: i64) -> (u32, u32) {
    let per_ms = def_period_hns as f64 / 10000.0; // :467
    let per_frames = (per_ms * dev_rate as f64 / 1000.0 + 0.5) as u32; // :468
    let mut n = (want_ms / per_ms + 0.5) as u32; // :469
    if n < 1 {
        n = 1; // :470-471
    }
    (per_frames.wrapping_mul(n), per_frames) // :472-473
}

/// origin: audio_out.cpp:257-713 run — the whole device handshake, event
/// loop and teardown in one scope, mirroring the C++ local-declaration
/// order (:259-286). `goto done` → `break 'body`; cleanup :686-712 runs on
/// every path in disk order.
unsafe fn run_inner<G: Fill>(
    st: &Shared,
    latency_ms: i32,
    want_exclusive: bool,
    want_dev: &str,
    want_raw: bool,
    cap_path: &str,
    fill: &mut G,
) {
    // :259-262 COM を初期化できない → goto done
    let com_failed = failed(co_initialize(std::ptr::null(), COINIT_MULTITHREADED));
    if com_failed {
        *st.err.lock().unwrap() = "COM を初期化できない".to_string(); // :260
    }

    // :264-277 every handle declared before the first `goto` — same here.
    let mut en: Obj = std::ptr::null_mut(); // :264
    let mut dev: Obj = std::ptr::null_mut(); // :265
    let mut client: Obj = std::ptr::null_mut(); // :266
    let mut render: Obj = std::ptr::null_mut(); // :267
    let ev: *mut c_void = if com_failed {
        std::ptr::null_mut() // goto done skipped :268
    } else {
        sys::CreateEventA(std::ptr::null(), 0, 0, std::ptr::null()) // :268 auto-reset
    };
    let mut mix: *mut WaveFormatEx = std::ptr::null_mut(); // :269
    let mut buf_frames: u32 = 0; // :270
    #[allow(unused_variables)]
    let mmcss_index: u32 = 0; // :271 out-param, value unused (:530)
    let mut mmcss: Obj = std::ptr::null_mut(); // :272
    let mut clock: Obj = std::ptr::null_mut(); // :274
    let mut clock_freq: u64 = 0; // :275
    let mut cap: Option<std::io::BufWriter<std::fs::File>> = None; // :276
    let mut cap_frames: u64 = 0; // :277

    let mut rs = Resampler::new(); // :279 resampler rs
    let mut stage: Vec<i16> = Vec::new(); // :280 std::vector<s16> stage
    let mut mixbuf: Vec<f32> = Vec::new(); // :281 std::vector<float> mixbuf
    let mut dev_float = false; // :282
    let mut exclusive = false; // :282
    let mut autoconv = false; // :282
    let mut fmt = DevFmt::F32; // :283 devfmt fmt = devfmt::f32
    let mut dev_ch: u32 = 2; // :284
    let mut dev_rate: u32 = AUDIO_RATE; // :284
    let mut target: u32 = 0; // :285
    let mut period_frames: u32 = 0; // :285
    const CHUNK: u32 = 480; // :286 const UINT32 CHUNK = 480

    // :288-292 fail lambda — snprintf "%s に失敗 (0x%08lx)"
    let fail = |what: &str, hr: HResult| {
        *st.err.lock().unwrap() = format!("{what} に失敗 (0x{:08x})", hr as u32);
    };

    if !com_failed {
        'body: {
            // :294-296 CoCreateInstance(MMDeviceEnumerator)
            let mut hr = co_create_instance(
                &CLSID_MM_DEVICE_ENUMERATOR,
                std::ptr::null_mut(),
                CLSCTX_ALL,
                &IID_IMM_DEVICE_ENUMERATOR,
                &mut en,
            );
            if failed(hr) {
                fail("デバイス一覧の取得", hr); // :296
                break 'body;
            }

            // :298-321 名前で指定されていればそれを探す。無ければ Windows の既定
            if !want_dev.is_empty() {
                let mut all: Obj = std::ptr::null_mut(); // :300
                if succeeded((vtbl!(en, ImmDeviceEnumeratorVtbl).enum_audio_endpoints)(
                    en,
                    E_RENDER,
                    DEVICE_STATE_ACTIVE,
                    &mut all,
                )) {
                    let mut n: u32 = 0; // :302 UINT n = 0
                    (vtbl!(all, ImmDeviceCollectionVtbl).get_count)(all, &mut n); // :303
                    let mut i: u32 = 0;
                    while i < n && dev.is_null() {
                        // :304 for (i=0; i<n && !dev; i++)
                        let mut d: Obj = std::ptr::null_mut();
                        if failed((vtbl!(all, ImmDeviceCollectionVtbl).item)(all, i, &mut d)) {
                            i += 1; // :306-307 continue
                            continue;
                        }
                        if endpoint_name(d).contains(want_dev) {
                            dev = d; // :308-309 掴んだまま使う
                        } else {
                            (vtbl!(d, IUnknownVtbl).release)(d); // :310-311
                        }
                        i += 1;
                    }
                    (vtbl!(all, IUnknownVtbl).release)(all); // :313
                }
                if dev.is_null() {
                    // :315-316 その名前の再生デバイスが無い: <name>
                    *st.err.lock().unwrap() = format!("その名前の再生デバイスが無い: {want_dev}");
                }
            }
            if dev.is_null() {
                // :318-321 GetDefaultAudioEndpoint(eRender, eConsole, &dev)
                hr = (vtbl!(en, ImmDeviceEnumeratorVtbl).get_default_audio_endpoint)(
                    en,
                    E_RENDER,
                    E_CONSOLE,
                    &mut dev,
                );
                if failed(hr) {
                    fail("既定の音声デバイスの取得", hr); // :320
                    break 'body;
                }
            }
            *st.dev_name.lock().unwrap() = endpoint_name(dev); // :322 m_dev_name

            // :324-325 dev->Activate(IAudioClient)
            hr = (vtbl!(dev, ImmDeviceVtbl).activate)(
                dev,
                &IID_IAUDIO_CLIENT,
                CLSCTX_ALL,
                std::ptr::null_mut(),
                &mut client,
            );
            if failed(hr) {
                fail("音声デバイスの起動", hr); // :325
                break 'body;
            }
            // :326 GetMixFormat || !mix → 形式の取得 に失敗 E_FAIL
            if failed((vtbl!(client, IAudioClientVtbl).get_mix_format)(client, &mut mix))
                || mix.is_null()
            {
                fail("形式の取得", E_FAIL); // :326
                break 'body;
            }

            {
                // :328-486 the format-decision block
                let mut def_period: i64 = 0; // :329 REFERENCE_TIME def_period = 0
                let mut min_period: i64 = 0; // :329
                (vtbl!(client, IAudioClientVtbl).get_device_period)(client, &mut def_period, &mut min_period); // :330

                // :332-413 独り占めモード。Windows の混ぜ合わせを通さないので
                // 一番短い。**デバイスが言っている周波数を先に試す** (:334-336)
                if want_exclusive {
                    // :338-341 形式は **24bit を 32bit の器に入れたものを先に**。
                    // 44100 が通れば変換も要らなくなるので、それも試す
                    // :342-349 the candidate table verbatim
                    let mix_rate = (*mix).n_samples_per_sec;
                    let cands: [(u32, bool, i32, i32); 6] = [
                        (mix_rate, false, 24, 32), // :343
                        (mix_rate, true, 32, 32),  // :344
                        (mix_rate, false, 16, 16), // :345
                        (AUDIO_RATE, false, 24, 32), // :346
                        (AUDIO_RATE, true, 32, 32), // :347
                        (AUDIO_RATE, false, 16, 16), // :348
                    ];
                    for c in cands.iter() {
                        // :350 for (const auto &c : cands)
                        let want = make_format(c.0, c.1, c.2, c.3); // :351
                        let want_fmt = std::ptr::addr_of!(want.format); // &want.Format
                        // :352-354 IsFormatSupported(EXCLUSIVE, ..., nullptr)
                        if failed((vtbl!(client, IAudioClientVtbl).is_format_supported)(
                            client,
                            AUDCLNT_SHAREMODE_EXCLUSIVE,
                            want_fmt,
                            std::ptr::null_mut(),
                        )) {
                            continue; // :354
                        }
                        // :355-374 「周期は 2 の冪のフレーム数にする」+
                        // min_period 水増し (extracted verbatim, golden test)
                        let (mut frames, mut per) = exclusive_period(latency_ms, c.0, min_period);
                        let _ = &mut frames;
                        // :375-377 Initialize(EXCLUSIVE, EVENTCALLBACK, per, per)
                        hr = (vtbl!(client, IAudioClientVtbl).initialize)(
                            client,
                            AUDCLNT_SHAREMODE_EXCLUSIVE,
                            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                            per,
                            per,
                            want_fmt,
                            std::ptr::null(),
                        );
                        if hr == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
                            // :378-380 揃っていない長さを断られた。教えられた
                            // 長さで作り直す。**client を作り直すのが決まり**
                            let mut aligned: u32 = 0; // :381
                            (vtbl!(client, IAudioClientVtbl).get_buffer_size)(client, &mut aligned); // :382
                            (vtbl!(client, IUnknownVtbl).release)(client); // :383
                            client = std::ptr::null_mut(); // :384
                            if failed((vtbl!(dev, ImmDeviceVtbl).activate)(
                                dev,
                                &IID_IAUDIO_CLIENT,
                                CLSCTX_ALL,
                                std::ptr::null_mut(),
                                &mut client,
                            )) // :385-386
                            {
                                break; // :387
                            }
                            per = (10000.0 * 1000.0 * aligned as f64 / c.0 as f64 + 0.5) as i64; // :388
                            hr = (vtbl!(client, IAudioClientVtbl).initialize)(
                                client,
                                AUDCLNT_SHAREMODE_EXCLUSIVE,
                                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                                per,
                                per,
                                want_fmt,
                                std::ptr::null(),
                            ); // :389-391
                        }
                        if succeeded(hr) {
                            // :393-401
                            exclusive = true; // :394
                            dev_float = c.1; // :395
                            fmt = if c.1 {
                                // :396-397 c.flt ? f32 : (container==32 ? i24in32 : i16)
                                DevFmt::F32
                            } else if c.3 == 32 {
                                DevFmt::I24In32
                            } else {
                                DevFmt::I16
                            };
                            dev_ch = 2; // :398
                            dev_rate = c.0; // :399
                            set_f64(&st.period_ms, per as f64 / 10000.0); // :400 m_period_ms
                            break; // :401
                        }
                        // :403-408 失敗したら client は Initialize 前の状態に戻す
                        (vtbl!(client, IUnknownVtbl).release)(client); // :404
                        client = std::ptr::null_mut(); // :405
                        if failed((vtbl!(dev, ImmDeviceVtbl).activate)(
                            dev,
                            &IID_IAUDIO_CLIENT,
                            CLSCTX_ALL,
                            std::ptr::null_mut(),
                            &mut client,
                        )) // :406-407
                        {
                            break; // :408
                        }
                    }
                    if !exclusive && !client.is_null() {
                        // :410-411 独り占めで開けなかった（共有に落とす）
                        *st.err.lock().unwrap() = "独り占めで開けなかった（共有に落とす）".to_string();
                    }
                    if client.is_null() {
                        fail("音声デバイスの起動", E_FAIL); // :412
                        break 'body;
                    }
                }

                // :415-475 共有モード
                if !exclusive {
                    // :417-418 RAW を頼む。エンジンの信号処理（APO）を飛ばす
                    // ことがある。断られても構わない
                    if want_raw {
                        let mut a2: Obj = std::ptr::null_mut(); // :420
                        // :421-422 QueryInterface(IAudioClient2) — slot 0
                        if succeeded((vtbl!(client, IUnknownVtbl).query_interface)(
                            client,
                            &IID_IAUDIO_CLIENT2,
                            &mut a2,
                        )) {
                            // :423-427 AudioClientProperties props{}
                            let props = AudioClientProperties {
                                cb_size: std::mem::size_of::<AudioClientProperties>() as u32,
                                b_is_offload: 0, // FALSE
                                e_category: AUDIO_CATEGORY_MEDIA, // AudioCategory_Media
                                options: AUDCLNT_STREAMOPTIONS_RAW,
                            };
                            // :428 m_raw.store(SUCCEEDED(a2->SetClientProperties))
                            let hr2 = (vtbl!(a2, IAudioClient2Vtbl).set_client_properties)(a2, &props);
                            st.raw_f.store(succeeded(hr2), Ordering::SeqCst);
                            (vtbl!(a2, IUnknownVtbl).release)(a2); // :429
                        }
                    }
                    // :432-441 16bit PCM の器を自分で渡す最後の道 (autoconv)
                    let mut fallback = WaveFormatEx {
                        w_format_tag: 0,
                        n_channels: 0,
                        n_samples_per_sec: 0,
                        n_avg_bytes_per_sec: 0,
                        n_block_align: 0,
                        w_bits_per_sample: 0,
                        cb_size: 0,
                    };
                    let usable = is_float(mix) || is_pcm16(mix); // :433
                    if !usable {
                        fallback.w_format_tag = WAVE_FORMAT_PCM; // :435
                        fallback.n_channels = 2; // :436
                        fallback.n_samples_per_sec = AUDIO_RATE; // :437
                        fallback.w_bits_per_sample = 16; // :438
                        fallback.n_block_align = 4; // :439
                        fallback.n_avg_bytes_per_sec = AUDIO_RATE.wrapping_mul(4); // :440
                        autoconv = true; // :441
                    }
                    let use_fmt: *const WaveFormatEx = if usable { mix } else { &fallback }; // :443
                    dev_float = usable && is_float(mix); // :444
                    fmt = if dev_float { DevFmt::F32 } else { DevFmt::I16 }; // :445
                dev_ch = (*use_fmt).n_channels as u32; // :446
                dev_rate = (*use_fmt).n_samples_per_sec; // :447
                    set_f64(&st.period_ms, def_period as f64 / 10000.0); // :448

                    // :450-451 **器は余裕を持って取り、溜めるのは target だけ。**
                    // 器が小さいと、起きるのが少し遅れたときに書く場所が無くなる
                    let want_ms = if latency_ms > 0 {
                        // :452-453
                        latency_ms as f64
                    } else {
                        2.0 * def_period as f64 / 10000.0
                    };
                    // :454-455 std::max(want_ms + 2*def, 3*def)
                    let mut buf_ms = want_ms + 2.0 * def_period as f64 / 10000.0;
                    let three = 3.0 * def_period as f64 / 10000.0;
                    if three > buf_ms {
                        buf_ms = three;
                    }
                    // :456-458 flags
                    let mut flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
                    if autoconv {
                        flags |= AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
                    }
                    // :459-460 Initialize(SHARED, flags, buf_ms hns, 0, use, nullptr)
                    hr = (vtbl!(client, IAudioClientVtbl).initialize)(
                        client,
                        AUDCLNT_SHAREMODE_SHARED,
                        flags,
                        (buf_ms * 10000.0) as i64, // REFERENCE_TIME(buf_ms*10000.0)
                        0,
                        use_fmt,
                        std::ptr::null(),
                    );
                    if failed(hr) {
                        fail("音声の開始準備", hr); // :461
                        break 'body;
                    }

                    // :463-474 **目標は周期の倍数にする** (extracted verbatim,
                    // golden test): 半端な目標にすると溜めが振動する
                    let (t, pf) = shared_target(dev_rate, want_ms, def_period);
                    target = t; // :472
                    period_frames = pf; // :473
                }

                // :477-485 the publish block
                st.exclusive_f.store(exclusive, Ordering::SeqCst); // :477
                st.dev_rate.store(dev_rate, Ordering::SeqCst); // :478
                st.dev_channels.store(dev_ch, Ordering::SeqCst); // :479
                st.dev_float.store(dev_float, Ordering::SeqCst); // :480
                st.dev_bits.store(
                    // :481 fmt == i24in32 ? 24 : (i16 ? 16 : 32)
                    if fmt == DevFmt::I24In32 {
                        24
                    } else if fmt == DevFmt::I16 {
                        16
                    } else {
                        32
                    },
                    Ordering::SeqCst,
                );
                rs.configure(AUDIO_RATE as f64, dev_rate as f64); // :482
                st.converting.store(!rs.direct(), Ordering::SeqCst); // :483
                stage.resize((CHUNK as usize + 64) * 2, 0); // :484
                mixbuf.resize(CHUNK as usize * 2, 0.0); // :485
            }

            // :488-489 SetEventHandle
            hr = (vtbl!(client, IAudioClientVtbl).set_event_handle)(client, ev);
            if failed(hr) {
                fail("イベントの登録", hr); // :489
                break 'body;
            }
            // :491-492 GetBufferSize
            hr = (vtbl!(client, IAudioClientVtbl).get_buffer_size)(client, &mut buf_frames);
            if failed(hr) {
                fail("バッファ長の取得", hr); // :492
                break 'body;
            }
            // :494-495 GetService(IAudioRenderClient)
            hr = (vtbl!(client, IAudioClientVtbl).get_service)(
                client,
                &IID_IAUDIO_RENDER_CLIENT,
                &mut render,
            );
            if failed(hr) {
                fail("書き込み口の取得", hr); // :495
                break 'body;
            }

            {
                // :497-506 デバイス側の取り分 + 再生位置
                let mut sl: i64 = 0; // :499 REFERENCE_TIME sl = 0
                if succeeded((vtbl!(client, IAudioClientVtbl).get_stream_latency)(client, &mut sl)) {
                    set_f64(&st.stream_ms, sl as f64 / 10000.0); // :500-501
                }
                // :502-505 再生位置。書いた量との差が「まだ鳴っていない量」で、
                // **ドライバが抱えている分も入る**
                if succeeded((vtbl!(client, IAudioClientVtbl).get_service)(
                    client,
                    &IID_IAUDIO_CLOCK,
                    &mut clock,
                )) {
                    (vtbl!(clock, IAudioClockVtbl).get_frequency)(clock, &mut clock_freq); // :505
                }
            }

            // :508-515 デバイスへ渡すものをそのまま書き出す（切り分け用）
            if !cap_path.is_empty() {
                if let Ok(f) = std::fs::File::create(cap_path) {
                    let mut w = std::io::BufWriter::new(f);
                    let head = [0u8; 44]; // :512 u8 head[44] = {}
                    if w.write_all(&head).is_ok() {
                        // :513 fwrite(head,1,44) — 後で書き直す
                        cap = Some(w);
                    }
                }
            }

            // :517-521 独り占めでは毎周期ちょうど器ぶんを書く。共有では target だけ溜める
            if exclusive || target == 0 || target > buf_frames {
                target = buf_frames; // :518-519
            }
            st.buffer_frames.store(buf_frames, Ordering::SeqCst); // :520
            st.target_frames.store(target, Ordering::SeqCst); // :521

            // :523-533 音声を作るスレッドは優先度を上げる。**SetThreadPriority
            // だけでは足りない** — MMCSS "Pro Audio" に登録しておく
            sys::SetThreadPriority(sys::GetCurrentThread(), sys::THREAD_PRIORITY_TIME_CRITICAL); // :529
            let task = sys::wide("Pro Audio"); // :530 L"Pro Audio"
            mmcss = sys::av_set_mm_thread_characteristics(task.as_ptr()); // :530-531
            #[allow(unused_assignments)]
            {
                let _ = mmcss_index; // the out-param value stays unused (:530 disk too)
            }
            st.mmcss.store(!mmcss.is_null(), Ordering::SeqCst); // :533 m_mmcss

            {
                // :535-683 the write/serve block
                // :538-584 write_frames lambda — 要る分だけ作り、器へ書く
                let mut write_frames = |dst: *mut u8, frames: u32| {
                    let mut at: u32 = 0; // :539
                    while at < frames {
                        // :541 min(CHUNK, frames-at)
                        let n = std::cmp::min(CHUNK, frames - at);
                        let need = rs.input_needed(n as i32); // :542
                        if need > 0 {
                            // :543-548 入れてから出す
                            if stage.len() < (need as usize) * 2 {
                                stage.resize((need as usize) * 2, 0); // :544-545
                            }
                            fill.fill(&mut stage[..(need as usize) * 2]); // :546 m_fill
                            rs.push(&stage[..(need as usize) * 2], need); // :547
                        }
                        rs.pull(&mut mixbuf[..(n as usize) * 2], n as i32); // :549

                        if fmt == DevFmt::F32 {
                            // :551-559 float へそのまま（足りないチャンネルは 0）
                            let out = dst.cast::<f32>().add((at * dev_ch) as usize);
                            for i in 0..n as usize {
                                *out.add(i * dev_ch as usize) = mixbuf[i * 2]; // :554
                                if dev_ch > 1 {
                                    *out.add(i * dev_ch as usize + 1) = mixbuf[i * 2 + 1]; // :555-556
                                }
                                let mut cch: u32 = 2;
                                while cch < dev_ch {
                                    *out.add(i * dev_ch as usize + cch as usize) = 0.0f32; // :557-558
                                    cch += 1;
                                }
                            }
                        } else if fmt == DevFmt::I24In32 {
                            // :560-570 32bit の器に左詰め。下の 8bit は 0
                            let out = dst.cast::<i32>().add((at * dev_ch) as usize);
                            for i in 0..n as usize {
                                let mut s: u32 = 0;
                                while s < 2 && s < dev_ch {
                                    let v = mixbuf[i * 2 + s as usize].clamp(-1.0f32, 1.0f32); // :565 std::clamp
                                    *out.add(i * dev_ch as usize + s as usize) =
                                        ((v * 8388607.0f32) as i32) << 8; // :566
                                    s += 1;
                                }
                                let mut cch: u32 = 2;
                                while cch < dev_ch {
                                    *out.add(i * dev_ch as usize + cch as usize) = 0; // :568-569
                                    cch += 1;
                                }
                            }
                        } else {
                            // :571-581 16bit
                            let out = dst.cast::<i16>().add((at * dev_ch) as usize);
                            for i in 0..n as usize {
                                let mut s: u32 = 0;
                                while s < 2 && s < dev_ch {
                                    let v = mixbuf[i * 2 + s as usize].clamp(-1.0f32, 1.0f32); // :575
                                    *out.add(i * dev_ch as usize + s as usize) =
                                        (v * 32767.0f32) as i16; // :576
                                    s += 1;
                                }
                                let mut cch: u32 = 2;
                                while cch < dev_ch {
                                    *out.add(i * dev_ch as usize + cch as usize) = 0; // :578-579
                                    cch += 1;
                                }
                            }
                        }
                        at += n; // :582
                    }
                };

                // :586-591 走り出しに溜める分。**満杯ではなく target まで**
                let mut data: *mut u8 = std::ptr::null_mut();
                if succeeded((vtbl!(render, IAudioRenderClientVtbl).get_buffer)(
                    render, target, &mut data,
                )) {
                    write_frames(data, target); // :588
                    st.produced.fetch_add(target as u64, Ordering::SeqCst); // :589
                    (vtbl!(render, IAudioRenderClientVtbl).release_buffer)(render, target, 0); // :590
                }

                // :593-595 Start
                hr = (vtbl!(client, IAudioClientVtbl).start)(client);
                if failed(hr) {
                    fail("再生の開始", hr); // :594
                    break 'body;
                }
                st.running.store(true, Ordering::SeqCst); // :595

                // :597-680 the event loop
                while !st.quit.load(Ordering::SeqCst) {
                    // :598-601 合図待ち（2 秒来たら終わり）
                    if sys::WaitForSingleObject(ev, 2000) != WAIT_OBJECT_0 {
                        *st.err.lock().unwrap() = "音声デバイスからの合図が来ない".to_string(); // :599
                        break; // :600
                    }

                    // :603-608 **独り占めでは padding を見ない。** 合図が来たら
                    // 裏を**まるごと**書くのが作法
                    let mut padding: u32 = 0;
                    if !exclusive
                        && failed(
                            (vtbl!(client, IAudioClientVtbl).get_current_padding)(client, &mut padding),
                        )
                    {
                        break; // :608
                    }

                    // :610-625 書いたのに、まだ鳴っていない量。ドライバの分も入る
                    if !clock.is_null() && clock_freq != 0 {
                        let mut pos: u64 = 0; // :612
                        if succeeded((vtbl!(clock, IAudioClockVtbl).get_position)(
                            clock,
                            &mut pos,
                            std::ptr::null_mut(),
                        )) {
                            // :614-615 played = pos/freq*dev_rate (f64 order)
                            let played =
                                ((pos as f64 / clock_freq as f64) * dev_rate as f64) as u64;
                            let wrote = st.produced.load(Ordering::SeqCst); // :616
                            if wrote > played
                                && st.produced.load(Ordering::SeqCst) > (buf_frames as u64) * 2
                            // :617 second load verbatim
                            {
                                let fly = wrote - played; // :618
                                st.inflight_sum.fetch_add(fly, Ordering::SeqCst); // :619
                                st.inflight_n.fetch_add(1, Ordering::SeqCst); // :620
                                if fly > st.inflight_worst.load(Ordering::SeqCst) {
                                    st.inflight_worst.store(fly, Ordering::SeqCst); // :621-622
                                }
                            }
                        }
                    }

                    // :627-631 起きたときに溜まっていた量。これが待ち時間の本体
                    st.queue_sum.fetch_add(padding as u64, Ordering::SeqCst); // :628
                    st.queue_n.fetch_add(1, Ordering::SeqCst); // :629
                    if padding as u64 > st.queue_worst.load(Ordering::SeqCst) {
                        st.queue_worst.store(padding as u64, Ordering::SeqCst); // :630-631
                    }

                    // :633-638 溜めは目標まで。満杯にすると、その分そのまま待ち時間になる
                    let mut want = buf_frames;
                    if !exclusive {
                        want = if padding >= target {
                            0
                        } else {
                            std::cmp::min(buf_frames - padding, target - padding)
                        };
                    }
                    if want == 0 {
                        continue; // :639-640
                    }

                    // :642-643 GetBuffer
                    if failed((vtbl!(render, IAudioRenderClientVtbl).get_buffer)(
                        render, want, &mut data,
                    )) {
                        break; // :643
                    }

                    // :645-648 busy accounting around write_frames
                    let t0 = sys::qpc(); // QueryPerformanceCounter
                    write_frames(data, want);
                    let t1 = sys::qpc();

                    // :650-655 cap: デバイスへ渡したバイト列をそのまま
                    if let Some(c) = cap.as_mut() {
                        let bytes = (want as usize)
                            * dev_ch as usize
                            * (if fmt == DevFmt::I16 { 2 } else { 4 }); // :651-652
                        let slice = std::slice::from_raw_parts(data, bytes); // :653 fwrite
                        let _ = c.write_all(slice);
                        cap_frames += want as u64; // :654
                    }

                    // :657-661 busy accounting
                    let took = (t1 - t0) as u64; // :657
                    st.busy_ticks.fetch_add(took, Ordering::SeqCst); // :658
                    if took > st.worst_ticks.load(Ordering::SeqCst) {
                        st.worst_ticks.store(took, Ordering::SeqCst); // :659-660
                    }
                    st.produced.fetch_add(want as u64, Ordering::SeqCst); // :661

                    // :663-677 間に合ったか。共有なら「まだ溜まっていた量 +
                    // 周期ひとつぶん」、独り占めなら「器ひとつぶん」。**走り出し
                    // の数回は数えない**
                    if st.produced.load(Ordering::SeqCst) > (buf_frames as u64) * 2 {
                        let slack: u64 = if exclusive {
                            // :671-672
                            buf_frames as u64
                        } else {
                            padding as u64 + period_frames as u64
                        };
                        if slack < st.slack_min.load(Ordering::SeqCst) {
                            st.slack_min.store(slack, Ordering::SeqCst); // :673-674
                        }
                        if (took as f64 / st.qpc_freq.load(Ordering::SeqCst) as f64)
                            > (slack as f64 / dev_rate as f64)
                        // :675
                        {
                            st.late.fetch_add(1, Ordering::SeqCst); // :676
                        }
                    }

                    (vtbl!(render, IAudioRenderClientVtbl).release_buffer)(render, want, 0); // :679
                }

                (vtbl!(client, IAudioClientVtbl).stop)(client); // :682
            }
        } // 'body
    } // !com_failed

    // 'done: :685-712 cleanup — every path, same order
    if let Some(c) = cap.as_mut() {
        // :686-701 WAV の頭を後から書く。形式はデバイスに渡したものそのまま
        let bits: u32 = if fmt == DevFmt::I16 { 16 } else { 32 }; // :688
        let bps: u32 = dev_rate.wrapping_mul(dev_ch).wrapping_mul(bits) / 8; // :689
        let data_bytes = (cap_frames * dev_ch as u64 * bits as u64 / 8) as u32; // :690
        let _ = c.seek(SeekFrom::Start(0)); // :691 fseek(cap, 0, SEEK_SET)
        let w32 = |c: &mut std::io::BufWriter<std::fs::File>, v: u32| {
            let _ = c.write_all(&v.to_le_bytes()); // :692 u8 b[4] LE
        };
        let w16 = |c: &mut std::io::BufWriter<std::fs::File>, v: u16| {
            let _ = c.write_all(&v.to_le_bytes()); // :694
        };
        let _ = c.write_all(b"RIFF"); // :695
        w32(c, 36 + data_bytes);
        let _ = c.write_all(b"WAVE");
        let _ = c.write_all(b"fmt "); // :696
        w32(c, 16);
        w16(c, if fmt == DevFmt::F32 { 3 } else { 1 }); // :697
        w16(c, dev_ch as u16); // :698
        w32(c, dev_rate); // :698
        w32(c, bps); // :698
        w16(c, (dev_ch * bits / 8) as u16); // :698
        w16(c, bits as u16); // :698
        let _ = c.write_all(b"data"); // :699
        w32(c, data_bytes); // :699
        let _ = c.flush(); // :700 fclose
    }
    if !clock.is_null() {
        (vtbl!(clock, IUnknownVtbl).release)(clock); // :702
    }
    if !mmcss.is_null() {
        sys::av_revert_mm_thread(mmcss); // :703-704
    }
    st.running.store(false, Ordering::SeqCst); // :705
    if !mix.is_null() {
        co_task_mem_free(mix as *mut c_void); // :706 CoTaskMemFree(mix)
    }
    if !render.is_null() {
        (vtbl!(render, IUnknownVtbl).release)(render); // :707
    }
    if !client.is_null() {
        (vtbl!(client, IUnknownVtbl).release)(client); // :708
    }
    if !dev.is_null() {
        (vtbl!(dev, IUnknownVtbl).release)(dev); // :709
    }
    if !en.is_null() {
        (vtbl!(en, IUnknownVtbl).release)(en); // :710
    }
    if !ev.is_null() {
        sys::CloseHandle(ev); // :711
    }
    co_uninitialize(); // :712 (also on the COM-init-failed path — disk falls
                       // through the same `done:` block; inert, thread exits)
}

// ---------------------------------------------------------------------------
// offline tests — layout/rounding/table goldens (device-free)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// struct layouts pinned against gt.exe (mingw headers, the ground-truth
    /// compiler), 2026-10-02
    #[test]
    fn layout_matches_headers() {
        assert_eq!(std::mem::size_of::<WaveFormatEx>(), 18); // gt
        assert_eq!(std::mem::size_of::<WaveFormatExtensible>(), 40);
        assert_eq!(std::mem::offset_of!(WaveFormatExtensible, format), 0);
        assert_eq!(std::mem::offset_of!(WaveFormatExtensible, samples), 18);
        assert_eq!(std::mem::offset_of!(WaveFormatExtensible, channel_mask), 20);
        assert_eq!(std::mem::offset_of!(WaveFormatExtensible, sub_format), 24);
        assert_eq!(std::mem::size_of::<PropVariant>(), 24); // gt PROPVARIANT 24
        assert_eq!(std::mem::offset_of!(PropVariant, vt), 0);
        assert_eq!(std::mem::offset_of!(PropVariant, u), 8); // pwszVal@8
        assert_eq!(std::mem::size_of::<AudioClientProperties>(), 16); // gt
        assert_eq!(std::mem::offset_of!(AudioClientProperties, cb_size), 0);
        assert_eq!(std::mem::offset_of!(AudioClientProperties, b_is_offload), 4);
        assert_eq!(std::mem::offset_of!(AudioClientProperties, e_category), 8);
        assert_eq!(std::mem::offset_of!(AudioClientProperties, options), 12);
        assert_eq!(std::mem::size_of::<PropertyKey>(), 20); // gt PROPERTYKEY 20
        assert_eq!(std::mem::offset_of!(PropertyKey, pid), 16);
        assert_eq!(std::mem::size_of::<Guid>(), 16);
    }

    /// vtable slot counts (= header method listings, all read line-by-line):
    /// any drift shifts every call below it and would execute the wrong slot
    #[test]
    fn vtbl_slot_counts() {
        let s = |n: usize| n / std::mem::size_of::<usize>();
        assert_eq!(s(std::mem::size_of::<IAudioClientVtbl>()), 15); // audioclient.h:203-251
        assert_eq!(s(std::mem::size_of::<IAudioClient2Vtbl>()), 18); // :417-433
        assert_eq!(s(std::mem::size_of::<IAudioRenderClientVtbl>()), 5); // :881-892
        assert_eq!(s(std::mem::size_of::<IAudioClockVtbl>()), 6); // :1084-1097
        assert_eq!(s(std::mem::size_of::<ImmDeviceVtbl>()), 7); // mmdeviceapi.h:299-315
        assert_eq!(s(std::mem::size_of::<ImmDeviceCollectionVtbl>()), 5); // :417-424
        assert_eq!(s(std::mem::size_of::<ImmDeviceEnumeratorVtbl>()), 8); // :584-606
        assert_eq!(s(std::mem::size_of::<IPropertyStoreVtbl>()), 8); // propsys.h:462-483
    }

    /// GUID transcription pins — every one re-printed by gt.exe 2026-10-02
    /// (three of them differ from the commonly quoted values!)
    #[test]
    fn guid_pins() {
        assert_eq!(CLSID_MM_DEVICE_ENUMERATOR, g(0xbcde0395, 0xe52f, 0x467c, [0x8e, 0x3d, 0xc4, 0x57, 0x92, 0x91, 0x69, 0x2e]));
        assert_eq!(IID_IMM_DEVICE_ENUMERATOR, g(0xa95664d2, 0x9614, 0x4f35, [0xa7, 0x46, 0xde, 0x8d, 0xb6, 0x36, 0x17, 0xe6]));
        assert_eq!(IID_IMM_DEVICE_COLLECTION, g(0x0bd7a1be, 0x7a1a, 0x44db, [0x83, 0x97, 0xcc, 0x53, 0x92, 0x38, 0x7b, 0x5e]));
        assert_eq!(IID_IMM_DEVICE, g(0xd666063f, 0x1587, 0x4e43, [0x81, 0xf1, 0xb9, 0x48, 0xe8, 0x07, 0x36, 0x3f]));
        assert_eq!(IID_I_PROPERTY_STORE, g(0x886d8eeb, 0x8cf2, 0x4446, [0x8d, 0x02, 0xcd, 0xba, 0x1d, 0xbd, 0xcf, 0x99]));
        assert_eq!(IID_IAUDIO_CLIENT, g(0x1cb9ad4c, 0xdbfa, 0x4c32, [0xb1, 0x78, 0xc2, 0xf5, 0x68, 0xa7, 0x03, 0xb2]));
        assert_eq!(IID_IAUDIO_CLIENT2, g(0x726778cd, 0xf60a, 0x4eda, [0x82, 0xde, 0xe4, 0x76, 0x10, 0xcd, 0x78, 0xaa]));
        assert_eq!(IID_IAUDIO_RENDER_CLIENT, g(0xf294acfc, 0x3146, 0x4483, [0xa7, 0xbf, 0xad, 0xdc, 0xa7, 0xc2, 0x60, 0xe2]));
        assert_eq!(IID_IAUDIO_CLOCK, g(0xcd63314f, 0x3fba, 0x4a1b, [0x81, 0x2c, 0xef, 0x96, 0x35, 0x87, 0x28, 0xe7]));
        assert_eq!(K_FRIENDLY_NAME.fmtid, g(0xa45c254e, 0xdf1c, 0x4efd, [0x80, 0x20, 0x67, 0xd1, 0x46, 0xa8, 0x50, 0xe0]));
        assert_eq!(K_FRIENDLY_NAME.pid, 14);
        assert_eq!(AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, 0x88890019u32 as i32); // gt
        assert_eq!(AUDIO_CATEGORY_MEDIA, 11); // gt
    }

    /// the full sinc table vs the mingw-built ground truth:
    /// Get-FileHash SHA1(table_cpp.bin) == 645AADA4834B09F276F16202CCBB27CC2057AACD
    /// (2026-10-02, re-reproduced byte-EXACT by a fresh gt.cpp build
    /// 2026-10-03). The bits come from the GT's own STATIC x87 sin/cos
    /// objects (build.rs); the x87 signature sin(PI)==0x3ca1a60000000000
    /// is asserted right here so a future relink to any other math (UCRT
    /// imports a bare `sin`!) flips this test instead of the audio.
    /// SMU_WASAPI_TABLE_DUMP re-dumps the raw bytes; when the golden file
    /// is present (dev machine / SMU_GT_TABLE) the FULL 8194-entry table
    /// is byte-compared, not just the pins.
    #[test]
    fn table_golden() {
        let mut rs = Resampler::new();
        rs.configure(44100.0, 48000.0);
        let x87_pi = unsafe { smu_gt_sin(3.14159265358979323846_f64) };
        eprintln!(
            "gt x87 sin(PI)=0x{:016x} rust-celestial=0x{:016x}",
            x87_pi.to_bits(),
            (3.141592653589793_f64).sin().to_bits()
        );
        assert_eq!(x87_pi.to_bits(), 0x3ca1a60000000000); // __sinl_internal signature
        assert_eq!(rs.m_tab.len(), 8194); // HALF*STEPS+2, gt tab_size
        let pins: [(usize, u32); 10] = [
            (0, 0x3f800000),
            (1, 0x3f7ffe5a),
            (255, 0x3b7ffdd7),
            (256, 0x24330e6e),
            (4095, 0xb8ae3763),
            (4096, 0xa3747b29),
            (8063, 0xb615822d),
            (8191, 0xabe36af2),
            (8192, 0x0833c3f8),
            (8193, 0x2be35cbb),
        ];
        for (i, want) in pins {
            assert_eq!(rs.m_tab[i].to_bits(), want, "tab[{i}]");
        }
        let bytes: Vec<u8> = rs.m_tab.iter().flat_map(|f| f.to_le_bytes()).collect();
        // FULL 8194-entry byte-compare against the proven C++ table when
        // the golden is on this machine (pins alone would miss a drifted
        // constant further down the range)
        let gt = std::env::var("SMU_GT_TABLE").unwrap_or_else(|_| {
            std::env::temp_dir()
                .join("opencode")
                .join("audiobt")
                .join("table_cpp.bin")
                .to_string_lossy()
                .into_owned()
        });
        match std::fs::read(&gt) {
            Ok(got) => {
                assert_eq!(got.len(), bytes.len(), "golden table length");
                let bad: Vec<usize> = (0..bytes.len()).filter(|i| got[*i] != bytes[*i]).collect();
                assert!(
                    bad.is_empty(),
                    "{}/{} bytes differ from {gt}, first {:?}",
                    bad.len(),
                    bytes.len(),
                    bad.iter().take(8).collect::<Vec<_>>()
                );
            }
            Err(_) => eprintln!("(golden {gt} absent — pins only)"),
        }
        if std::env::var("SMU_WASAPI_TABLE_DUMP").is_ok() {
            let p = std::env::temp_dir().join("opencode").join("audiobt").join("table_rs.bin");
            let _ = std::fs::write(&p, &bytes);
        }
    }

    /// exclusive pow2 period rounding — the RME lesson (audio_out.cpp:355-374).
    /// Hand-computed from the disk math; also the ledger row note
    /// 「exclusive rounds to power-of-two period」.
    #[test]
    fn exclusive_period_rounding() {
        // 10 ms @44100: frames=441 → pow2 512; (512-441)=71 vs (441-256)=185 → keep 512
        // per = 1e7*512/44100+0.5 = 116099.7732+0.5 → 116100
        assert_eq!(exclusive_period(10, 44100, 0), (512, 116100));
        // 10 ms @48000: 480 → 512 (512-480=32 < 480-256=224)
        assert_eq!(exclusive_period(10, 48000, 0).0, 512);
        // 7 ms @44100: 308.7→309 → pow2 512; (512-309)=203 > (309-256)=53 → 256
        assert_eq!(exclusive_period(7, 44100, 0).0, 256);
        // 5 ms @44100: 220.5→221 → 256 (35 < 93)
        assert_eq!(exclusive_period(5, 44100, 0).0, 256);
        // 1 ms @44100: 44.1→44 → pow2 loop stops at 64; pow2>64 false → 64
        let (fr, per) = exclusive_period(1, 44100, 0);
        assert_eq!((fr, per), (64, 14512)); // 1e7*64/44100+0.5 = 14512.4...
        // min_period growth: 1ms@44100 → per 14512 < 20000 → ×2=128, per=29025
        let (fr, per) = exclusive_period(1, 44100, 20000);
        assert_eq!((fr, per), (128, 29025));
        // latency 0 → want_ms 10.0 law (:362)
        assert_eq!(exclusive_period(0, 48000, 0).0, 512);
    }

    /// shared-mode target rounding (audio_out.cpp:463-474): the 46ms
    /// oscillation the comment describes only happens WITHOUT this law.
    #[test]
    fn shared_target_rounding() {
        // def_period 10 ms = 100000 hns → per_frames 441 @44100
        assert_eq!(shared_target(44100, 20.0, 100000), (882, 441));
        // the ledger example: 15 ms wanted → u32(1.5+0.5)=2 → 882
        assert_eq!(shared_target(44100, 15.0, 100000), (882, 441));
        assert_eq!(shared_target(44100, 25.0, 100000), (1323, 441));
        // floor to 1 period min
        assert_eq!(shared_target(44100, 4.0, 100000), (441, 441));
        // 48k device: per_frames 480, want 20 ms → 960
        assert_eq!(shared_target(48000, 20.0, 100000), (960, 480));
    }

    /// input_needed / output_available arithmetic (resampler.h:64-85),
    /// 44100→48000 step = 0.91875
    #[test]
    fn resampler_window_math() {
        let mut rs = Resampler::new();
        assert!(rs.direct()); // :177 m_direct = true (until configured)
        rs.configure(44100.0, 48000.0);
        assert!(!rs.direct());
        // last = 0.91875*479 = 440.08125 → floor 440 + 33 = 473, written 0
        assert_eq!(rs.input_needed(480), 473);
        assert_eq!(rs.input_needed(0), 0); // :68
        assert_eq!(rs.output_available(), 0); // room = -33 < 0
        let input = vec![0i16; 473 * 2];
        rs.push(&input, 473);
        assert_eq!(rs.written(), 473);
        assert_eq!(rs.input_needed(480), 0); // 473 covers it
        let mut out = vec![0f32; 480 * 2];
        rs.pull(&mut out, 480);
        assert!((rs.m_pos - 441.0).abs() < 1e-9); // 480*0.91875
        // direct passthrough: values are int/32768 verbatim
        let mut d = Resampler::new();
        d.configure(44100.0, 44100.0);
        assert!(d.direct());
        d.push(&[1i16, -1, 32767, -32768], 2);
        let mut o2 = vec![0f32; 4];
        d.pull(&mut o2, 2);
        assert_eq!(o2[0], 1.0f32 / 32768.0f32); // :90 k
        assert_eq!(o2[1], -1.0f32 / 32768.0f32);
        assert_eq!(o2[2], 32767.0f32 / 32768.0f32);
        assert_eq!(o2[3], -1.0f32);
    }

    /// is_float / is_pcm16 / make_format (audio_out.cpp:20-67). Fields of
    /// the pshpack1 structs are read unaligned (E0793 rule).
    #[test]
    fn fmt_probes() {
        let rd16 = |p: *const u8, off: usize| unsafe { (p.add(off) as *const u16).read_unaligned() };
        let rd32 = |p: *const u8, off: usize| unsafe { (p.add(off) as *const u32).read_unaligned() };
        let f32 = WaveFormatEx {
            w_format_tag: WAVE_FORMAT_IEEE_FLOAT,
            n_channels: 2,
            n_samples_per_sec: 48000,
            n_avg_bytes_per_sec: 384000,
            n_block_align: 8,
            w_bits_per_sample: 32,
            cb_size: 0,
        };
        let pcm16 = WaveFormatEx {
            w_format_tag: WAVE_FORMAT_PCM,
            n_channels: 2,
            n_samples_per_sec: 44100,
            n_avg_bytes_per_sec: 176400,
            n_block_align: 4,
            w_bits_per_sample: 16,
            cb_size: 0,
        };
        assert!(is_float(std::ptr::addr_of!(f32)));
        assert!(!is_float(std::ptr::addr_of!(pcm16)));
        assert!(is_pcm16(std::ptr::addr_of!(pcm16)));
        assert!(!is_pcm16(std::ptr::addr_of!(f32)));
        let e = make_format(44100, false, 24, 32); // :343 candidate
        let ep = std::ptr::addr_of!(e).cast::<u8>();
        assert_eq!(rd16(ep, 0), WAVE_FORMAT_EXTENSIBLE); // :51
        assert_eq!(rd16(ep, 2), 2); // :52 nChannels
        assert_eq!(rd32(ep, 4), 44100); // :53
        assert_eq!(rd16(ep, 12), 8); // :55 nBlockAlign 2*32/8
        assert_eq!(rd16(ep, 14), 32); // :54 wBitsPerSample container
        assert_eq!(rd32(ep, 8), 44100 * 8); // :56 avg
        assert_eq!(rd16(ep, 16), 22); // :57 cbSize 40-18
        assert_eq!(rd16(ep, 18), 24); // :58 wValidBitsPerSample
        assert_eq!(rd32(ep, 20), 3); // :59 FRONT_L|R
        assert_eq!(rd32(ep, 24), 1); // :61 PCM
        let ef = make_format(48000, true, 32, 32); // :344
        let efp = std::ptr::addr_of!(ef).cast::<u8>();
        assert_eq!(rd32(efp, 24), 3); // IEEE_FLOAT
        assert_eq!(rd32(efp, 4), 48000);
        // extensible probe legs of is_float/is_pcm16
        assert!(is_float(std::ptr::addr_of!(ef).cast::<WaveFormatEx>()));
        // is_pcm16's extensible leg needs wBitsPerSample==16 (audio_out.cpp:33)
        let e16 = make_format(44100, false, 16, 16); // :345 candidate
        assert!(is_pcm16(std::ptr::addr_of!(e16).cast::<WaveFormatEx>()));
        assert!(!is_pcm16(std::ptr::addr_of!(e).cast::<WaveFormatEx>())); // 24in32
    }

    /// Shared init == audio_out.h:211-236 device-less defaults (ledger:
    /// every device field explicit, no Default::default())
    #[test]
    fn shared_init() {
        let st = Shared::new();
        assert_eq!(st.dev_rate.load(Ordering::SeqCst), 44100);
        assert_eq!(st.dev_channels.load(Ordering::SeqCst), 2);
        assert_eq!(st.dev_bits.load(Ordering::SeqCst), 16);
        assert_eq!(st.slack_min.load(Ordering::SeqCst), u64::MAX);
        assert_eq!(get_f64(&st.period_ms), 0.0);
        assert!(!st.running.load(Ordering::SeqCst));
        assert_eq!(st.qpc_freq.load(Ordering::SeqCst), 1); // :236 {1}
    }
}

