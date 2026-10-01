//! CPU address bus — transliteration of `src/compat/membus.h`.
//!
//! Semantics that MUST stay bit-identical (do not "fix" them):
//! - Region ranges are **inclusive of `end`** (`a >= start && a <= end`).
//! - Fast-path RAM test is `u32(a - start) <= len` with `len = end - start`
//!   (so it accepts len+1 bytes, and sub-start addresses wrap unsigned — membus.h:185).
//! - Big-endian everywhere (SH-2). Width promotion/demotion tables are copied
//!   function-for-function from membus.h; a device with only a wider handler sees
//!   zero-filled values exactly as the C++ shifts do (reads discard low byte of
//!   a shifted word, writes place v at the addressed lane).
//!
//! Layout mirrors the C++ object: hot scalar copies of the program ROM and the
//! first writable region (membus.h:150-178 "ここは 1 命令ごとに通る場所").

/// origin: membus.h:39-44 — handler signatures take the absolute (16-bit-unit
/// for SWP30-attached spaces? no: absolute byte address) address.
pub type R8 = Box<dyn FnMut(u32) -> u8>;
pub type R16 = Box<dyn FnMut(u32) -> u16>;
pub type R32 = Box<dyn FnMut(u32) -> u32>;
pub type W8 = Box<dyn FnMut(u32, u8)>;
pub type W16 = Box<dyn FnMut(u32, u16)>;
pub type W32 = Box<dyn FnMut(u32, u32)>;

/// origin: membus.h:37-45 `struct device`
#[derive(Default)]
pub struct Device {
    pub start: u32,
    pub end: u32,
    pub r8: Option<R8>,
    pub r16: Option<R16>,
    pub r32: Option<R32>,
    pub w8: Option<W8>,
    pub w16: Option<W16>,
    pub w32: Option<W32>,
}

/// origin: membus.h:30-34 `struct region`. `base` points into memory owned by
/// the Machine (program ROM / work RAM vectors) whose lifetime the Machine
/// guarantees outlives the bus; set once during wiring, never repointed while
/// running — mirrors the C++ raw `u8 *base`.
struct Region {
    start: u32,
    end: u32,
    base: std::ptr::NonNull<u8>,
    writable: bool,
}

/// origin: membus.h:26 `class mem_bus`
///
/// SAFETY: the Machine is single-threaded around the bus (the optional slave
/// SWP30 thread never touches it); region pointers are stable by construction.
pub struct MemBus {
    regions: Vec<Region>,
    devices: Vec<Device>,
    // 熱い領域の写し (membus.h:220-222), filled by build_pages()
    hot_r: *const u8,
    hot_r_end: u32,
    hot_w: *mut u8,
    hot_w_start: u32,
    hot_w_len: u32,
}

// SAFETY: see struct doc. The Machine owns the pointee and is itself the only
// mover of the bus.
unsafe impl Send for MemBus {}

impl Default for MemBus {
    fn default() -> Self {
        Self::new()
    }
}

impl MemBus {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            devices: Vec::new(),
            hot_r: std::ptr::null(),
            hot_r_end: 0,
            hot_w: std::ptr::null_mut(),
            hot_w_start: 0,
            hot_w_len: 0,
        }
    }

    /// origin: membus.h:47-51 add_region
    ///
    /// # Safety
    /// `base..base+(end-start+1)` must remain valid and stable for the bus's
    /// lifetime, and mutable aliasing must follow Machine single-owner rules.
    pub unsafe fn add_region(&mut self, start: u32, end: u32, base: *mut u8, writable: bool) {
        self.regions.push(Region {
            start,
            end,
            base: std::ptr::NonNull::new(base).expect("region base null"),
            writable,
        });
        self.build_pages();
    }

    /// origin: membus.h:53-57 add_device
    pub fn add_device(&mut self, d: Device) {
        self.devices.push(d);
        self.build_pages();
    }

    // ---- origin: membus.h:161-178 build_pages
    fn build_pages(&mut self) {
        self.hot_r = std::ptr::null();
        self.hot_w = std::ptr::null_mut();
        self.hot_r_end = 0;
        self.hot_w_start = 0;
        self.hot_w_len = 0;
        for r in &self.regions {
            // 0 から始まる読み出し専用の大きな領域（プログラム ROM）
            if r.start == 0 && self.hot_r.is_null() {
                self.hot_r = r.base.as_ptr();
                self.hot_r_end = r.end;
            }
            // 最初の書ける領域（ワーク RAM）
            if r.writable && self.hot_w.is_null() {
                self.hot_w = r.base.as_ptr();
                self.hot_w_start = r.start;
                self.hot_w_len = r.end.wrapping_sub(r.start);
            }
        }
    }

    // ---- origin: membus.h:181-188 fast (two scalar range tests, inclusive)
    #[inline(always)]
    fn fast(&self, a: u32) -> *const u8 {
        if a <= self.hot_r_end {
            // SAFETY: hot_r was set from a valid region covering [0, hot_r_end]
            unsafe { self.hot_r.add(a as usize) }
        } else if a.wrapping_sub(self.hot_w_start) <= self.hot_w_len {
            // SAFETY: RAM region covers start..start+len inclusive (mirrors C++)
            unsafe { self.hot_w.add(a.wrapping_sub(self.hot_w_start) as usize) }
        } else {
            std::ptr::null()
        }
    }

    /// origin: membus.h:190-195 fast_w
    #[inline(always)]
    fn fast_w(&self, a: u32) -> *mut u8 {
        if a.wrapping_sub(self.hot_w_start) <= self.hot_w_len {
            unsafe { self.hot_w.add(a.wrapping_sub(self.hot_w_start) as usize) }
        } else {
            std::ptr::null_mut()
        }
    }

    /// origin: membus.h:197-202 find_read (any region, inclusive end)
    fn find_read(&self, a: u32) -> *const u8 {
        for r in &self.regions {
            if a >= r.start && a <= r.end {
                return unsafe { r.base.as_ptr().add(a.wrapping_sub(r.start) as usize) };
            }
        }
        std::ptr::null()
    }

    /// origin: membus.h:204-209 find_write (writable regions only)
    fn find_write(&self, a: u32) -> *mut u8 {
        for r in self.regions.iter() {
            if r.writable && a >= r.start && a <= r.end {
                return unsafe { r.base.as_ptr().add(a.wrapping_sub(r.start) as usize) };
            }
        }
        std::ptr::null_mut()
    }

    /// origin: membus.h:211-216 find_dev
    fn find_dev(&mut self, a: u32) -> Option<&mut Device> {
        for d in &mut self.devices {
            if a >= d.start && a <= d.end {
                return Some(d);
            }
        }
        None
    }

    // ---- 読み出し origin: membus.h:61-71 read_byte
    pub fn read_byte(&mut self, a: u32) -> u8 {
        let p = self.fast(a);
        if !p.is_null() {
            return unsafe { *p };
        }
        let p = self.find_read(a);
        if !p.is_null() {
            return unsafe { *p };
        } // 半端に掛かった領域
        if let Some(d) = self.find_dev(a) {
            if let Some(r8) = &mut d.r8 {
                return r8(a);
            }
            if let Some(r16) = &mut d.r16 {
                return (r16(a & !1) >> if a & 1 != 0 { 0 } else { 8 }) as u8;
            }
            if let Some(r32) = &mut d.r32 {
                return (r32(a & !3) >> ((3 - (a & 3)) * 8)) as u8;
            }
        }
        0
    }

    /// origin: membus.h:73-84 read_word
    pub fn read_word(&mut self, a: u32) -> u16 {
        let a = a & !1;
        let p = self.fast(a);
        if !p.is_null() {
            return unsafe { (u16::from(*p) << 8) | u16::from(*p.add(1)) };
        }
        let p = self.find_read(a);
        if !p.is_null() {
            return unsafe { (u16::from(*p) << 8) | u16::from(*p.add(1)) };
        }
        if let Some(d) = self.find_dev(a) {
            if let Some(r16) = &mut d.r16 {
                return r16(a);
            }
            if let Some(r32) = &mut d.r32 {
                return (r32(a & !3) >> if a & 2 != 0 { 0 } else { 16 }) as u16;
            }
            if let Some(r8) = &mut d.r8 {
                let hi = r8(a) as u16;
                let lo = r8(a + 1) as u16;
                return (hi << 8) | lo;
            }
        }
        0
    }

    /// origin: membus.h:86-97 read_dword
    pub fn read_dword(&mut self, a: u32) -> u32 {
        let a = a & !3;
        let p = self.fast(a);
        if !p.is_null() {
            return unsafe {
                (u32::from(*p) << 24)
                    | (u32::from(*p.add(1)) << 16)
                    | (u32::from(*p.add(2)) << 8)
                    | u32::from(*p.add(3))
            };
        }
        let p = self.find_read(a);
        if !p.is_null() {
            return unsafe {
                (u32::from(*p) << 24)
                    | (u32::from(*p.add(1)) << 16)
                    | (u32::from(*p.add(2)) << 8)
                    | u32::from(*p.add(3))
            };
        }
        if let Some(d) = self.find_dev(a) {
            if let Some(r32) = &mut d.r32 {
                return r32(a);
            }
        }
        let hi = self.read_word(a) as u32;
        let lo = self.read_word(a + 2) as u32;
        (hi << 16) | lo
    }

    /// origin: membus.h:101-110 write_byte
    pub fn write_byte(&mut self, a: u32, v: u8) {
        let p = self.fast_w(a);
        if !p.is_null() {
            unsafe { *p = v };
            return;
        }
        let p = self.find_write(a);
        if !p.is_null() {
            unsafe { *p = v };
            return;
        }
        if let Some(d) = self.find_dev(a) {
            if let Some(w8) = &mut d.w8 {
                w8(a, v);
                return;
            }
            if let Some(w16) = &mut d.w16 {
                w16(a & !1, if a & 1 != 0 { v as u16 } else { (v as u16) << 8 });
                return;
            }
            if let Some(w32) = &mut d.w32 {
                w32(a & !3, (v as u32) << ((3 - (a & 3)) * 8));
                return;
            }
        }
    }

    /// origin: membus.h:112-122 write_word
    pub fn write_word(&mut self, a: u32, v: u16) {
        let a = a & !1;
        let p = self.fast_w(a);
        if !p.is_null() {
            unsafe {
                *p = (v >> 8) as u8;
                *p.add(1) = v as u8;
            }
            return;
        }
        let p = self.find_write(a);
        if !p.is_null() {
            unsafe {
                *p = (v >> 8) as u8;
                *p.add(1) = v as u8;
            }
            return;
        }
        if let Some(d) = self.find_dev(a) {
            if let Some(w16) = &mut d.w16 {
                w16(a, v);
                return;
            }
            if let Some(w32) = &mut d.w32 {
                w32(a & !3, if a & 2 != 0 { v as u32 } else { (v as u32) << 16 });
                return;
            }
            if let Some(w8) = &mut d.w8 {
                w8(a, (v >> 8) as u8);
                w8(a + 1, v as u8);
                return;
            }
        }
    }

    /// origin: membus.h:124-140 write_dword
    pub fn write_dword(&mut self, a: u32, v: u32) {
        let a = a & !3;
        let p = self.fast_w(a);
        if !p.is_null() {
            unsafe {
                *p = (v >> 24) as u8;
                *p.add(1) = (v >> 16) as u8;
                *p.add(2) = (v >> 8) as u8;
                *p.add(3) = v as u8;
            }
            return;
        }
        let p = self.find_write(a);
        if !p.is_null() {
            unsafe {
                *p = (v >> 24) as u8;
                *p.add(1) = (v >> 16) as u8;
                *p.add(2) = (v >> 8) as u8;
                *p.add(3) = v as u8;
            }
            return;
        }
        if let Some(d) = self.find_dev(a) {
            if let Some(w32) = &mut d.w32 {
                w32(a, v);
                return;
            }
        }
        self.write_word(a, (v >> 16) as u16);
        self.write_word(a + 2, v as u16);
    }

    // ---- origin: membus.h:143-147 hot accessors (kept for the JIT-era M9 API)
    pub fn hot_rom(&self) -> *const u8 {
        self.hot_r
    }
    pub fn hot_rom_end(&self) -> u32 {
        self.hot_r_end
    }
    pub fn hot_ram(&self) -> *mut u8 {
        self.hot_w
    }
    pub fn hot_ram_start(&self) -> u32 {
        self.hot_w_start
    }
    pub fn hot_ram_len(&self) -> u32 {
        self.hot_w_len
    }
}

#[cfg(test)]
mod tests;
