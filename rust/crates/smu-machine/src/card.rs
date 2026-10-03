//! SmartMedia card — full port of `src/smartmedia.{h,cpp}` (this row: the
//! NAND bus behavior on top of the W3a state half). Fields = the full
//! smartmedia.h:75-90 member set; every one explicitly initialized in
//! [`Card::new`] (ledger invariant 3 — the class has no ctor body and no
//! reset(); construction IS the reset state).
//!
//! Bus decode lives in `sh7042.rs` (Dev::CardData / Dev::CardCtrl — the
//! mu2000.cpp:981-995 windows); the Hub trait arms there call
//! [`Card::data_r`] / [`Card::data_w`] / [`Card::control_w`], and
//! `Hub::porta_pins` (mu2000.cpp:1115-1136) reads [`Card::inserted`] /
//! [`Card::write_protected`] for PA19/PA20.
//!
//! The state() leg stays EXACTLY the W3a-proven byte layout (only the
//! fields smartmedia::state touches ride the stream — 「カードの中身は
//! 入れない」, smartmedia.h:61).
//!
//! Quirk kept verbatim (smartmedia.cpp:371-374): the READ side caps the
//! queue length at 4096 *before* the resize, then `s.mem` consumes
//! `m_buf.size()` — so a >4096-byte queue from the wire desyncs the stream
//! (writer wrote n bytes, reader ate 4096). Never "fixed".

use smu_compat::StateIo;

/// origin: smartmedia.h:35 `static constexpr u32 PAGE = 512`
pub const PAGE: u32 = 512;
/// origin: smartmedia.h:35 `SPARE = 16`
pub const SPARE: u32 = 16;
/// origin: smartmedia.h:35 `PAGES_PER_BLOCK = 32`
pub const PAGES_PER_BLOCK: u32 = 32;

/// origin: smartmedia.h:67 `enum class mode : u8 { idle, read, read_id,
/// status, program, erase }` — kept as the raw u8 it serializes as
/// (smartmedia.cpp:365-367 round-trips `u8` ⇄ `mode`, and GCC's cast of an
/// out-of-range byte keeps the byte; a Rust enum could not).
pub const MODE_IDLE: u8 = 0;
/// origin: smartmedia.h:67 `read`
pub const MODE_READ: u8 = 1;
/// origin: smartmedia.h:67 `read_id`
pub const MODE_READ_ID: u8 = 2;
/// origin: smartmedia.h:67 `status`
pub const MODE_STATUS: u8 = 3;
/// origin: smartmedia.h:67 `program`
pub const MODE_PROGRAM: u8 = 4;
/// origin: smartmedia.h:67 `erase`
pub const MODE_ERASE: u8 = 5;

// ---- 名乗りの装置番号・ maker（smartmedia.cpp:19-98 anonymous namespace）----

/// origin: smartmedia.cpp:21 `constexpr u8 MAKER_TOSHIBA = 0x98`
const MAKER_TOSHIBA: u8 = 0x98;

/// origin: smartmedia.cpp:41-50 `device_code_for`
fn device_code_for(megabytes: u32) -> u8 {
    match megabytes {
        16 => 0x73,
        32 => 0x75,
        64 => 0x76,
        128 => 0x79,
        _ => 0,
    }
}

/// origin: smartmedia.cpp:54-96 `ecc256` — SmartMedia 256-byte ECC
/// (22-bit Hamming; column-parity 6 bits + XOR of odd-parity byte
/// positions, inverted).
fn ecc256(d: &[u8], out: &mut [u8; 3]) {
    let mut reg1: u8 = 0;
    let mut reg2: u8 = 0;
    let mut reg3: u8 = 0;
    for j in 0..256usize {
        let b = d[j];
        let bit = |k: u32| -> u32 { ((b >> k) & 1).into() };
        let cp = ((bit(0) ^ bit(2) ^ bit(4) ^ bit(6))
            | ((bit(1) ^ bit(3) ^ bit(5) ^ bit(7)) << 1)
            | ((bit(0) ^ bit(1) ^ bit(4) ^ bit(5)) << 2)
            | ((bit(2) ^ bit(3) ^ bit(6) ^ bit(7)) << 3)
            | ((bit(0) ^ bit(1) ^ bit(2) ^ bit(3)) << 4)
            | ((bit(4) ^ bit(5) ^ bit(6) ^ bit(7)) << 5)) as u8;
        let mut par = 0u32;
        for k in 0..8u32 {
            par ^= bit(k);
        }
        reg1 ^= cp;
        if par != 0 {
            reg3 ^= j as u8;
            reg2 ^= (j as u8) ^ 0xff; // u8(~j)
        }
    }
    let mut t1: u8 = 0;
    let mut t2: u8 = 0;
    let mut a: u8 = 0x80;
    let mut bm: u8 = 0x80;
    for _ in 0..4 {
        if reg3 & a != 0 {
            t1 |= bm;
        }
        bm >>= 1;
        if reg2 & a != 0 {
            t1 |= bm;
        }
        bm >>= 1;
        a >>= 1;
    }
    bm = 0x80;
    for _ in 0..4 {
        // `a` CONTINUES from the first loop (now 0x08) — disk flow, verbatim
        if reg3 & a != 0 {
            t2 |= bm;
        }
        bm >>= 1;
        if reg2 & a != 0 {
            t2 |= bm;
        }
        bm >>= 1;
        a >>= 1;
    }
    let c0 = !t1;
    let c1 = !t2;
    // SmartMedia の並びは、行の偶奇の 2 バイトが入れ替わる
    out[0] = c1;
    out[1] = c0;
    out[2] = ((reg1 ^ 0xff) << 2) | 0x03;
}

/// origin: smartmedia.h:50 `struct block { u32 index; std::vector<u8> bytes; }`
pub struct Block {
    pub index: u32,
    pub bytes: Vec<u8>,
}

/// origin: smartmedia.h:32-91 `class smartmedia`.
pub struct Card {
    /// origin: smartmedia.h:75 `std::vector<u8> m_data` — raw NAND layout
    /// (512 data + 16 spare per page).
    pub data: Vec<u8>,
    /// origin: smartmedia.h:76 `u32 m_pages = 0`
    pub pages: u32,
    /// origin: smartmedia.h:77 `u8 m_device_code = 0`
    pub device_code: u8,
    /// origin: smartmedia.h:78 `bool m_dirty = false`
    pub dirty: bool,
    /// origin: smartmedia.h:79 `std::vector<u8> m_dirty_blocks` — per-block
    /// rewritten mark
    pub dirty_blocks: Vec<u8>,
    /// origin: smartmedia.h:53 `bool write_protected = false`
    pub write_protected: bool,

    /// origin: smartmedia.h:82 `u8 m_ctrl = 0` — CLE/ALE/CE/WE latch.
    pub ctrl: u8,
    /// origin: smartmedia.h:83 `mode m_mode = mode::idle` — u8 carrier
    /// (see [`MODE_IDLE`]).
    pub mode: u8,
    /// origin: smartmedia.h:84 `u8 m_pointer = 0` — 00/01/50 column phase.
    pub pointer: u8,
    /// origin: smartmedia.h:85 `u8 m_addr_count = 0`.
    pub addr_count: u8,
    /// origin: smartmedia.h:86 `u32 m_column = 0` (0-527 within a page).
    pub column: u32,
    /// origin: smartmedia.h:87 `u32 m_page = 0`.
    pub page: u32,
    /// origin: smartmedia.h:88 `u32 m_id_pos = 0`.
    pub id_pos: u32,
    /// origin: smartmedia.h:89 `u8 m_last_cmd = 0`.
    pub last_cmd: u8,
    /// origin: smartmedia.h:90 `std::vector<u8> m_buf` — the 528-byte
    /// program-staging queue (state queue leg = u32 count + raw bytes).
    pub buf: Vec<u8>,
}

impl Card {
    /// Construction == the smartmedia.h:75-90 member initializers verbatim
    /// (invariant 3: every field explicit; no Default::default()).
    pub fn new() -> Card {
        Card {
            data: Vec::new(), // :75 default-empty vector
            pages: 0,         // :76 `= 0`
            device_code: 0,   // :77 `= 0`
            dirty: false,     // :78 `= false`
            dirty_blocks: Vec::new(), // :79 default-empty vector
            write_protected: false,   // :53 `= false`
            ctrl: 0,          // :82 `= 0`
            mode: MODE_IDLE,  // :83 `= mode::idle`
            pointer: 0,       // :84 `= 0`
            addr_count: 0,    // :85 `= 0`
            column: 0,        // :86 `= 0`
            page: 0,          // :87 `= 0`
            id_pos: 0,        // :88 `= 0`
            last_cmd: 0,      // :89 `= 0`
            buf: Vec::new(),  // :90 default-empty vector
        }
    }

    /// origin: smartmedia.h:69 `u32 page_bytes() const`
    #[inline]
    fn page_bytes(&self) -> u32 {
        PAGE + SPARE // :69 `return PAGE + SPARE`
    }

    /// origin: smartmedia.h:44 `bool inserted() const`
    #[inline]
    pub fn inserted(&self) -> bool {
        self.pages != 0 // :44 `return m_pages != 0`
    }

    /// origin: smartmedia.h:45 `bool dirty() const`
    #[inline]
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// origin: smartmedia.h:46 `void clear_dirty()`
    pub fn clear_dirty(&mut self) {
        self.dirty = false; // :46 `m_dirty = false`
        for b in self.dirty_blocks.iter_mut() {
            *b = 0; // :46 std::fill(..., u8(0))
        }
    }

    /// origin: smartmedia.h:54 `u32 megabytes() const`
    #[inline]
    pub fn megabytes(&self) -> u32 {
        self.pages * PAGE / (1024 * 1024) // :54
    }

    /// origin: smartmedia.h:64 `const std::vector<u8> &raw() const`
    #[inline]
    pub fn raw(&self) -> &Vec<u8> {
        &self.data
    }

    /// origin: smartmedia.h:42 `void eject()` — data only; the latch
    /// fields (m_ctrl etc.) deliberately survive, verbatim (cpp has no
    /// reset there; GT 2026-10-03: eject keeps m_ctrl).
    pub fn eject(&mut self) {
        self.data.clear(); // :42 m_data.clear()
        self.pages = 0; // :42 m_pages = 0
        self.dirty_blocks.clear(); // :42 m_dirty_blocks.clear()
        self.dirty = false; // :42 m_dirty = false
    }

    /// origin: smartmedia.cpp:100-133 `create(u32 megabytes)` — the whole
    /// image 0xFF + the SSFDC CIS head on block 0's first two pages with
    /// block-address 0000 + ECC in the spare.
    pub fn create(&mut self, megabytes: u32) -> bool {
        let code = device_code_for(megabytes); // :102
        if code == 0 {
            // :103-104 `if (!code) return false` — STATE UNTOUCHED (GT
            // create_bad: an unsupported size leaves the old card intact)
            return false;
        }
        self.pages = megabytes * 1024 * 1024 / PAGE; // :105
        self.device_code = code; // :106
        self.data = vec![0xffu8; (self.pages * self.page_bytes()) as usize]; // :107
        self.dirty_blocks = vec![1u8; (self.pages / PAGES_PER_BLOCK) as usize]; // :108
        // 物理の書式（SSFDC）。MU2000 の書式化は CIS を探してから FAT を
        // 書くので、CIS が無いと「Bad Card!」になる（:109-111）
        // firmware が見るのは CIS の頭の 10 バイトだけ（:111）
        const CIS_HEAD: [u8; 10] = [0x01, 0x03, 0xd9, 0x01, 0xff, 0x18, 0x02, 0xdf, 0x01, 0x20]; // :112
        let pb = self.page_bytes() as usize; // :113 loop bound
        for pg in 0..2usize {
            let p0 = pg * pb; // :114
            self.data[p0..p0 + PAGE as usize].fill(0xff); // :115
            self.data[p0..p0 + 10].copy_from_slice(&CIS_HEAD); // :116
            let sp = p0 + PAGE as usize; // :117
            let mut e1 = [0u8; 3]; // :118
            let mut e2 = [0u8; 3];
            ecc256(&self.data[p0..p0 + 256], &mut e1); // :119
            ecc256(&self.data[p0 + 256..p0 + PAGE as usize], &mut e2); // :120
            self.data[sp + 6] = 0x00; // :121 sp[6] = sp[7] = 0
            self.data[sp + 7] = 0x00;
            self.data[sp + 11] = 0x00; // :122 sp[11] = sp[12] = 0
            self.data[sp + 12] = 0x00;
            self.data[sp + 8] = e2[0]; // :123
            self.data[sp + 9] = e2[1];
            self.data[sp + 10] = e2[2];
            self.data[sp + 13] = e1[0]; // :124
            self.data[sp + 14] = e1[1];
            self.data[sp + 15] = e1[2];
        }
        // CIS のブロックの残りのページも、番地は 0000（:126）
        for pg in 2..PAGES_PER_BLOCK as usize {
            let sp = pg * pb + PAGE as usize; // :128
            self.data[sp + 6] = 0x00; // :129
            self.data[sp + 7] = 0x00;
            self.data[sp + 11] = 0x00;
            self.data[sp + 12] = 0x00;
        }
        self.dirty = true; // :131
        true
    }

    /// origin: smartmedia.cpp:135-164 `load` — raw image file; capacity
    /// from the file size. (DEVIATION: C++ multi-byte→wide `open_file`
    /// (cpp:24-38) == Rust's native UTF-8→UTF-16 path on Windows.)
    pub fn load(&mut self, path: &str, err: &mut String) -> bool {
        let mut f = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => {
                // :138-141 「カードのファイルを開けない」
                *err = format!("カードのファイルを開けない: {path}");
                return false;
            }
        };
        let size = match f.metadata() {
            // :142-144 fseek(END)/ftell/fseek(SET) — metadata length here
            Ok(md) => md.len() as i64,
            Err(_) => {
                *err = format!("カードのファイルを開けない: {path}");
                return false;
            }
        };
        let mut mb = 0u32; // :145-148 size → capacity loop (last match wins)
        for m in [16u32, 32, 64, 128] {
            if size as u64 == u64::from(m) * 1024 * 1024 / u64::from(PAGE) * u64::from(PAGE + SPARE) {
                mb = m;
            }
        }
        if mb == 0 {
            // :149-153 fclose + 「大きさが 16/32/64/128MB と合わない」
            *err = format!("カードのファイルの大きさが 16/32/64/128MB の SmartMedia と合わない: {path}");
            return false;
        }
        self.create(mb); // :154
        let mut got: u64 = 0; // :155 fread — consume exactly m_data.size()
        let total = self.data.len();
        let mut off = 0usize;
        use std::io::Read;
        while off < total {
            match f.read(&mut self.data[off..]) {
                Ok(0) => break,
                Ok(n) => {
                    off += n;
                    got += n as u64;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        drop(f); // :156
        if got as usize != self.data.len() {
            // :157-161 short read -> eject + 「読み切れない」
            self.eject();
            *err = format!("カードのファイルを読み切れない: {path}");
            return false;
        }
        self.clear_dirty(); // :162
        true
    }

    /// origin: smartmedia.cpp:210-224 `save` — whole raw image (C++ const
    /// method: `&self` here, file output needs no mutation).
    pub fn save(&self, path: &str, err: &mut String) -> bool {
        use std::io::Write;
        let mut f = match std::fs::File::create(path) {
            // "wb" (cpp:212) == File::create (write+create+truncate)
            Ok(f) => f,
            Err(_) => {
                // :213-217 「カードのファイルを書けない」
                *err = format!("カードのファイルを書けない: {path}");
                return false;
            }
        };
        let put = f.write(&self.data).unwrap_or(0); // :217 fwrite
        drop(f); // :219 fclose
        if put != self.data.len() {
            // :220-223 「書き切れない」
            *err = format!("カードのファイルを書き切れない: {path}");
            return false;
        }
        true
    }

    /// origin: smartmedia.cpp:166-172 `mark_dirty` (private)
    fn mark_dirty(&mut self, page: u32) {
        let b = (page / PAGES_PER_BLOCK) as usize; // :168
        if b < self.dirty_blocks.len() {
            self.dirty_blocks[b] = 1; // :169-170
        }
        self.dirty = true; // :171
    }

    /// origin: smartmedia.cpp:174-186 `take_dirty_blocks` — copy out the
    /// rewritten blocks (stops the machine; 1 block = 17KB) and clear the
    /// marks.
    pub fn take_dirty_blocks(&mut self, out: &mut Vec<Block>) {
        out.clear(); // :177
        let bytes = (PAGES_PER_BLOCK * self.page_bytes()) as usize; // :177
        for b in 0..self.dirty_blocks.len() {
            if self.dirty_blocks[b] == 0 {
                continue; // :179-180
            }
            let from = b * bytes; // :181
            out.push(Block {
                index: b as u32,
                bytes: self.data[from..from + bytes].to_vec(), // :182
            });
            self.dirty_blocks[b] = 0; // :183
        }
        self.dirty = false; // :185
    }

    /// origin: smartmedia.cpp:188-208 `write_blocks` (static) — write the
    /// taken blocks back into the card file without stopping the machine.
    /// ("r+b" cpp:192 == read+write open, NO create.)
    pub fn write_blocks(path: &str, blocks: &[Block], err: &mut String) -> bool {
        use std::io::{Seek, SeekFrom, Write};
        if blocks.is_empty() {
            return true; // :190-191
        }
        let mut f = match std::fs::OpenOptions::new().read(true).write(true).open(path) {
            Ok(f) => f,
            Err(_) => {
                // :193-197 「書き戻せない」
                *err = format!("カードのファイルに書き戻せない: {path}");
                return false;
            }
        };
        let mut ok = true; // :197
        for b in blocks {
            let at = u64::from(b.index) * u64::from(PAGES_PER_BLOCK) * u64::from(PAGE + SPARE); // :199
            if f.seek(SeekFrom::Start(at)).is_err() {
                ok = false; // :200 fseek != 0
                continue;
            }
            if f.write_all(&b.bytes).is_err() {
                // :200 fwrite != size (partial write counted as failure by
                // write_all)
                ok = false;
            }
        }
        drop(f); // :203 fclose
        if !ok {
            // :205-207 「書き戻し切れない」
            *err = format!("カードのファイルに書き戻し切れない: {path}");
        }
        ok
    }

    /// origin: mu2000.cpp:993 / smartmedia.cpp:226-229 `control_w` — pure
    /// latch store (CLE bit3, ALE bit2, CE bit0; cpp:228 `m_ctrl = v`).
    pub fn control_w(&mut self, v: u8) {
        self.ctrl = v;
    }

    /// origin: smartmedia.cpp:231-243 `data_w`
    pub fn data_w(&mut self, v: u8) {
        if !self.inserted() || (self.ctrl & 0x01) == 0 {
            return; // :233 CE low / no card ignores everything
        }
        if self.ctrl & 0x08 != 0 {
            self.command(v); // :235-236 CLE
        } else if self.ctrl & 0x04 != 0 {
            self.address(v); // :237-238 ALE
        } else if self.mode == MODE_PROGRAM {
            // :239-242 program staging
            if self.column < self.page_bytes() {
                // DEVIATION (ledger-disclosed): disk indexes m_buf[i]
                // unguarded; a state-loaded short buf would be heap-OOB
                // UB there. Rust clamps (missing byte == drop). Real
                // mid-program buf is always 528 (command 0x80 :292).
                if let Some(s) = self.buf.get_mut(self.column as usize) {
                    *s = v;
                }
                self.column += 1;
            }
        }
    }

    /// origin: smartmedia.cpp:245-274 `data_r` — NO-CARD / CE-low / idle
    /// reads 0xFF (cpp:247-248, 272; NOT 0 — the old M4 stub value).
    pub fn data_r(&mut self) -> u8 {
        if !self.inserted() || (self.ctrl & 0x01) == 0 {
            return 0xff; // :247-248
        }
        match self.mode {
            MODE_READ_ID => {
                // :250-253 maker + device code, cycling (id_pos++ % 2)
                let id = [MAKER_TOSHIBA, self.device_code];
                let v = id[(self.id_pos % 2) as usize];
                self.id_pos = self.id_pos.wrapping_add(1);
                v
            }
            MODE_STATUS => {
                // :254-256 bit7 書き込みを禁じていない、bit6 準備完了、bit0 失敗
                let mut v = 0x40u8;
                if !self.write_protected {
                    v |= 0x80;
                }
                v
            }
            MODE_READ => {
                if self.page >= self.pages {
                    return 0xff; // :258-259
                }
                let v = self.data[(self.page * self.page_bytes() + self.column) as usize]; // :260
                self.column += 1; // :261
                if self.column >= self.page_bytes() {
                    // :262-268 次のページへ続けて読む。予備だけを読む命令
                    // （50）なら次の予備から
                    self.page += 1;
                    self.column = if self.pointer == 0x50 { PAGE } else { 0 };
                    if self.pointer == 0x01 {
                        self.pointer = 0x00;
                    }
                }
                v
            }
            _ => 0xff, // :271-272 default
        }
    }

    /// origin: smartmedia.cpp:276-330 `command` (private) — the public NAND
    /// set (smartmedia.h:15): read 00/01/50, program 80+10, erase 60+D0,
    /// status 70, id 90, reset FF.
    fn command(&mut self, c: u8) {
        self.last_cmd = c; // :278
        match c {
            0xff => {
                // :280-283 リセット
                self.mode = MODE_IDLE;
                self.pointer = 0x00;
            }
            0x00 | 0x01 | 0x50 => {
                // :284-288 読む（ページの頭 / 後ろ半分 / 予備）
                self.pointer = c;
                self.mode = MODE_READ;
                self.addr_count = 0;
            }
            0x80 => {
                // :289-294 書く中身を受け取り始める
                self.mode = MODE_PROGRAM;
                self.addr_count = 0;
                self.buf = vec![0xffu8; self.page_bytes() as usize];
                self.column = if self.pointer == 0x01 {
                    256
                } else if self.pointer == 0x50 {
                    PAGE
                } else {
                    0
                };
            }
            0x10 => {
                // :295-305 書く。NAND は 1 を 0 にしかできない（AND 写し）
                if self.mode == MODE_PROGRAM && !self.write_protected && self.page < self.pages {
                    let base = (self.page * self.page_bytes()) as usize; // :297
                    for i in 0..self.page_bytes() as usize {
                        // :298-299 — DEVIATION: disk reads m_buf[i]
                        // unguarded (OOB UB if a state-load left a short
                        // buf); Rust treats a missing byte as 0xFF =
                        // no-op AND. Real buf is 528 here.
                        let v = *self.buf.get(i).unwrap_or(&0xff);
                        self.data[base + i] &= v;
                    }
                    self.mark_dirty(self.page); // :300
                }
                if self.pointer == 0x01 {
                    self.pointer = 0x00; // :302-303
                }
                self.mode = MODE_STATUS; // :304
            }
            0x60 => {
                // :306-309 消すブロックの番地を受け取り始める
                self.mode = MODE_ERASE;
                self.addr_count = 0;
            }
            0xd0 => {
                // :310-319 消す（ブロック全体）
                if self.mode == MODE_ERASE && !self.write_protected {
                    let block = self.page / PAGES_PER_BLOCK; // :312
                    let base = (block * PAGES_PER_BLOCK * self.page_bytes()) as usize; // :313
                    if base < self.data.len() {
                        let end = std::cmp::min(self.data.len(), base + (PAGES_PER_BLOCK * self.page_bytes()) as usize); // :315
                        self.data[base..end].fill(0xff);
                    }
                    self.mark_dirty(self.page); // :316 (mark even when the
                    // address ran past the card — disk semantics, GT)
                }
                self.mode = MODE_STATUS; // :318
            }
            0x70 => {
                self.mode = MODE_STATUS; // :320-322 状態
            }
            0x90 => {
                self.mode = MODE_READ_ID; // :323-326 名乗り
                self.id_pos = 0;
            }
            _ => {} // :327-328 default: keep last_cmd, mode untouched
        }
    }

    /// origin: smartmedia.cpp:332-360 `address` (private)
    fn address(&mut self, a: u8) {
        match self.mode {
            MODE_READ | MODE_PROGRAM => {
                if self.addr_count == 0 {
                    // :337-341 列の番地（pointer が 0x512/256 の位を決める）
                    let mut col = u32::from(a)
                        + if self.pointer == 0x01 {
                            256
                        } else if self.pointer == 0x50 {
                            PAGE
                        } else {
                            0
                        };
                    if col >= self.page_bytes() {
                        col = self.page_bytes() - 1; // :339-340 clamp
                    }
                    self.column = col;
                    self.page = 0; // :341
                } else {
                    // :342-345 ページ番地の字节（little-endian の 2 回目以降）
                    let shift = 8 * (u32::from(self.addr_count) - 1);
                    // wrapping_* == observed x86 GCC behavior for the
                    // >4-bytes case (C++ UB there; firmware never does it)
                    self.page = (self.page & !(0xffu32.wrapping_shl(shift)))
                        | (u32::from(a).wrapping_shl(shift));
                }
                self.addr_count += 1; // :346
            }
            MODE_ERASE => {
                // :348-356 消すときは列の番地が無く、ページの番地だけ
                let shift = 8 * u32::from(self.addr_count);
                if self.addr_count == 0 {
                    self.page = 0;
                }
                self.page = (self.page & !(0xffu32.wrapping_shl(shift)))
                    | (u32::from(a).wrapping_shl(shift));
                self.addr_count += 1;
            }
            _ => {} // :357-358 default
        }
    }

    /// origin: smartmedia.h:71 `address_cycles` — 32MB まで 3 回、64MB
    /// からは 4 回 (unused on disk; kept for the harness).
    pub fn address_cycles(&self) -> u32 {
        if self.pages > 65536 {
            4
        } else {
            3
        }
    }

    /// origin: smartmedia.cpp:362-375 `state(state_io &s)` (M5-W3a). No
    /// card DATA (smartmedia.h:61 「カードの中身は入れない」), no tag.
    pub fn state(&mut self, s: &mut StateIo) {
        s.v(&mut self.ctrl); // :364 u8
        let mut md = self.mode; // :365 `u8 md = u8(m_mode)`
        s.v(&mut md); // :366
        self.mode = md; // :367 `m_mode = mode(md)` — u8 carrier, see MODE_IDLE
        s.v(&mut self.pointer); // :368 u8
        s.v(&mut self.addr_count); // :368 u8
        s.v(&mut self.column); // :368 u32
        s.v(&mut self.page); // :368 u32
        s.v(&mut self.id_pos); // :368 u32
        s.v(&mut self.last_cmd); // :368 u8
        let mut n = self.buf.len() as u32; // :369 `u32 n = u32(m_buf.size())`
        s.v(&mut n); // :370
        if !s.writing() {
            // :371-372 read side: resize(min(n,4096)) — value-init 0 bytes,
            // all overwritten by the mem leg below (or a sticky io failure)
            self.buf.resize(std::cmp::min(n, 4096u32) as usize, 0);
        }
        if !self.buf.is_empty() {
            // :373-374 mem(m_buf.data(), m_buf.size()) — size(), NOT n:
            // the >4096 read-side desync quirk, verbatim (module doc)
            let len = self.buf.len();
            s.mem(&mut self.buf[..len]);
        }
    }
}
