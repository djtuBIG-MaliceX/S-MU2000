//! State serializer — transliteration of `src/state.h:27-171`.
//!
//! Ledger row M5 `state io`. One type plays both roles like the C++ class:
//! write side appends to a `&mut Vec<u8>`, read side fills from a `&[u8>`.
//! Once a read fails, `ok()` is sticky-false and every later call is a
//! no-op (`// origin: state.h:45-46`); the first failure keeps the error.
//!
//! Layout is wire-format: little-endian scalars, 8-byte NUL-padded tags —
//! identical bytes to C++. `smu-machine::state` re-exports this module
//! (dep direction: `timers::state_sync` here needs `StateIo`, so the layout
//! types live in `smu-compat`).

/// origin: state.h:27-112 `class state_io`.
pub struct StateIo<'a> {
    /// write side target; None on the read side (C++ `m_out != nullptr` :35)
    out: Option<&'a mut Vec<u8>>,
    /// read side source (C++ `m_in`/`m_len` :107-108 — len == inp.len())
    inp: &'a [u8],
    at: usize, // :108 m_at
    ok: bool,  // :109 m_ok, starts true
    version: u32, // :110 m_version
    err: String,  // :111 m_err
}

/// Fixed-width LE scalar for `StateIo::v`/`arr`. `bool` travels as one u8
/// (C++ memcpy'd a `bool` byte; `decode` follows `!= 0`).
pub trait StateScalar: Copy {
    const SIZE: usize;
    fn encode(&self, buf: &mut [u8]);
    fn decode(buf: &[u8]) -> Self;
}

macro_rules! impl_scalar {
    ($($t:ty),*) => {
        $(
            impl StateScalar for $t {
                const SIZE: usize = std::mem::size_of::<$t>();
                fn encode(&self, buf: &mut [u8]) {
                    buf[..Self::SIZE].copy_from_slice(&self.to_le_bytes());
                }
                fn decode(buf: &[u8]) -> Self {
                    <$t>::from_le_bytes(buf[..Self::SIZE].try_into().unwrap())
                }
            }
        )*
    };
}

// origin: state.h:61-65 `v<T>` — trivially copyable scalars, LE on this
// (both) little-endian toolchain. bool = u8 (see trait doc).
impl_scalar!(u8, i8, u16, i16, u32, i32, u64, i64, f32, f64);

impl StateScalar for bool {
    const SIZE: usize = 1;
    fn encode(&self, buf: &mut [u8]) {
        buf[0] = *self as u8;
    }
    fn decode(buf: &[u8]) -> Self {
        buf[0] != 0
    }
}

impl<'a> StateIo<'a> {
    /// origin: state.h:31 書く側 — `explicit state_io(std::vector<u8> &out)`
    pub fn writer(out: &'a mut Vec<u8>) -> StateIo<'a> {
        StateIo {
            out: Some(out),
            inp: b"",
            at: 0,
            ok: true,
            version: 0,
            err: String::new(),
        }
    }

    /// origin: state.h:33 読む側 — `state_io(const u8 *p, size_t n)`
    pub fn reader(inp: &'a [u8]) -> StateIo<'a> {
        StateIo {
            out: None,
            inp,
            at: 0,
            ok: true,
            version: 0,
            err: String::new(),
        }
    }

    /// origin: state.h:35 `writing`
    pub fn writing(&self) -> bool {
        self.out.is_some()
    }

    /// origin: state.h:37 `version`
    pub fn version(&self) -> u32 {
        self.version
    }

    /// origin: state.h:38 `set_version`
    pub fn set_version(&mut self, v: u32) {
        self.version = v;
    }

    /// origin: state.h:39 `ok` — sticky-false after the first failure.
    pub fn ok(&self) -> bool {
        self.ok
    }

    /// origin: state.h:40 `error` (C++ returns the `std::string`; empty = none)
    pub fn error(&self) -> &str {
        &self.err
    }

    /// origin: state.h:43-58 `raw` — write appends, read fills `p`. Short
    /// read → fail("足りない") verbatim, `p` untouched, `at` unmoved.
    pub fn raw(&mut self, p: &mut [u8]) {
        if !self.ok {
            return;
        }
        if let Some(out) = self.out.as_mut() {
            out.extend_from_slice(p);
            return;
        }
        let n = p.len();
        if self.at + n > self.inp.len() {
            self.fail("足りない");
            return;
        }
        p.copy_from_slice(&self.inp[self.at..self.at + n]);
        self.at += n;
    }

    /// origin: state.h:61-65 `v(T &x)` — one scalar, LE. On read the value
    /// is only overwritten when `raw` actually filled the buffer (C++
    /// memcpy'd through the same pointer; here decode is gated on `ok`).
    pub fn v<T: StateScalar>(&mut self, x: &mut T) {
        let mut buf = [0u8; 8];
        if self.writing() {
            T::encode(x, &mut buf[..T::SIZE]);
            self.raw(&mut buf[..T::SIZE]);
        } else {
            self.raw(&mut buf[..T::SIZE]);
            if self.ok {
                *x = T::decode(&buf[..T::SIZE]);
            }
        }
    }

    /// origin: state.h:68-78 `arr` / `stdarr` — element bytes as one raw
    /// block. Read side stages through a temp buffer to keep C++'s
    /// single all-or-nothing bounds check (state.h:51); state I/O never
    /// runs in the audio callback, so the allocation is legal (rule is
    /// audio-callback-local).
    pub fn arr<T: StateScalar>(&mut self, a: &mut [T]) {
        if self.writing() {
            for x in a.iter() {
                let mut buf = [0u8; 8];
                T::encode(x, &mut buf[..T::SIZE]);
                self.raw(&mut buf[..T::SIZE]);
            }
        } else {
            let mut buf = vec![0u8; a.len() * T::SIZE];
            self.raw(&mut buf);
            if self.ok {
                for (i, x) in a.iter_mut().enumerate() {
                    *x = T::decode(&buf[i * T::SIZE..][..T::SIZE]);
                }
            }
        }
    }

    /// origin: state.h:81 `mem` — fixed-size container (RAM etc.)
    pub fn mem(&mut self, p: &mut [u8]) {
        self.raw(p);
    }

    /// origin: state.h:84-96 `tag` — 8-byte NUL-padded, name truncated to
    /// 7 bytes; read mismatch stops the stream, naming the expected tag.
    pub fn tag(&mut self, name: &str) {
        let mut buf = [0u8; 8]; // :86 char buf[8] = {}
        let nb = name.as_bytes();
        let n = nb.len().min(7); // :87 strncpy(buf, name, 7)
        buf[..n].copy_from_slice(&nb[..n]);
        let mut got = buf; // :88-89
        self.raw(&mut got); // :90
        if !self.writing() && self.ok && got != buf {
            // :92-93 snprintf into char[64] — 63-byte cap. DEVIATION: on
            // overflow this cuts at a char boundary (C++ sliced raw bytes);
            // unreachable with in-repo tags (all short ASCII).
            let msg = format!("目印が違う（{name} のところ）");
            let mut bytes = msg.into_bytes();
            if bytes.len() > 63 {
                let mut end = 63;
                while end > 0 && (bytes[end] & 0xc0) == 0x80 {
                    end -= 1;
                }
                bytes.truncate(end);
            }
            self.fail(&String::from_utf8(bytes).unwrap());
        }
    }

    /// origin: state.h:99-104 `fail` — sticky; first error wins.
    fn fail(&mut self, why: &str) {
        self.ok = false;
        if self.err.is_empty() {
            self.err = why.to_string();
        }
    }
}

/// origin: state.h:124-149 `state_pack`. Run of ≥4 identical bytes →
/// `00 (run-4) val`; run capped at 255. A lone non-RLE 0x00 → `00 fc 00`
/// (escape for a literal zero byte); everything else literal.
pub fn state_pack(inp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(inp.len() / 4 + 64); // :127
    let mut i = 0usize;
    while i < inp.len() {
        let mut run = 1usize;
        while run < 255 && i + run < inp.len() && inp[i + run] == inp[i] {
            run += 1;
        }
        if run >= 4 {
            out.push(0x00);
            out.push((run - 4) as u8);
            out.push(inp[i]);
            i += run;
        } else if inp[i] == 0x00 {
            out.push(0x00);
            out.push(0xfc);
            out.push(0x00);
            i += 1;
        } else {
            out.push(inp[i]);
            i += 1;
        }
    }
    out
}

/// origin: state.h:151-171 `state_unpack`. `00` needs 3 bytes available or
/// the stream is corrupt (Err); `cnt == 0xfc` → one `val`; else `cnt+4`
/// copies. C++'s `bool` + out-param becomes `Result`.
pub fn state_unpack(p: &[u8]) -> Result<Vec<u8>, ()> {
    let mut out = Vec::new(); // :153 out.clear()
    let mut i = 0usize;
    while i < p.len() {
        if p[i] != 0x00 {
            out.push(p[i]);
            i += 1;
            continue;
        }
        if i + 3 > p.len() {
            return Err(());
        }
        let cnt = p[i + 1];
        let val = p[i + 2];
        i += 3;
        if cnt == 0xfc {
            out.push(val);
        } else {
            for _ in 0..(cnt as usize + 4) {
                out.push(val);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
