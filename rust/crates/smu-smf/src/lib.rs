//! SMF (MIDI file) reader — mirror of `src/smf.cpp` (ledger row `smf`).
//! Used by render/blocktime bins. Remember: test MIDIs must carry tempo events.
//!
//! license:BSD-3-Clause
//!
//! SMF の読み込み。smf.h の説明を参照。(origin: smf.cpp:3)

/// origin: smf.h:18-28 — `struct event`。
/// `time` 秒、`bytes` MIDI バイト列、`port` 出し先 (既定 0)。
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub time: f64,     // origin: smf.h:19 秒
    pub bytes: Vec<u8>,// origin: smf.h:20
    pub port: u8,      // origin: smf.h:27 既定 0
}

impl Event {
    /// 明示初期化コンストラクタ（invariant 3: 暗黙の既定値に頼らない）。
    /// origin: smf.h:27 の既定 port=0 を هنا で明示。
    pub fn new(time: f64, bytes: Vec<u8>) -> Self {
        Event { time, bytes, port: 0 }
    }
}

/// トラック名から口を読む（issue #63）。origin: smf.cpp:14-53。
/// 「PartA」「Part A」「Part-A」「A01」「A1」「A-1」「A 16」、
/// 2 桁の番号のあとに区切りと名前が続く「A01-FrHorn 2」（大文字小文字は問わない）。
/// 読めなければ -1。
pub fn port_from_track_name(raw: &[u8]) -> i32 {
    // origin: smf.cpp:16-19  NUL を飛ばして小文字化（C ロケール so A-Z only）
    let mut s: Vec<u8> = Vec::new();
    for &c in raw.iter() {
        if c != 0 {
            s.push(if c.is_ascii_uppercase() { c + 32 } else { c }); // std::tolower(u8(c))
        }
    }
    // origin: smf.cpp:20-21  末尾の空白・タブ落とし
    while let Some(&b) = s.last() {
        if b == b' ' || b == b'\t' {
            s.pop();
        } else {
            break;
        }
    }
    // origin: smf.cpp:22-24  先頭の空白スキップ
    let mut i: usize = 0;
    while i < s.len() && s[i] == b' ' {
        i += 1;
    }
    // origin: smf.cpp:25  auto sep = [&]() { ' ', '-', '_' を飛ばす }
    // （C++ lambda の等価形 as macro: &mut capture の别名問題を回避）
    macro_rules! sep {
        () => {
            while i < s.len() && (s[i] == b' ' || s[i] == b'-' || s[i] == b'_') {
                i += 1;
            }
        };
    }
    // origin: smf.cpp:26-31  "part" 接頭辞
    let mut part = false;
    if &s[i..std::cmp::min(i + 4, s.len())] == b"part" {
        part = true;
        i += 4;
        sep!();
    }
    // origin: smf.cpp:32-34  a-d の一文字
    if i >= s.len() || s[i] < b'a' || s[i] > b'd' {
        return -1;
    }
    let port = (s[i] - b'a') as i32;
    i += 1;
    sep!();
    // origin: smf.cpp:36-38  後ろは空（Part の形だけ）か、1-16 の番号
    if i == s.len() {
        return if part { port } else { -1 };
    }
    // origin: smf.cpp:39-43  最大 3 桁の番号
    let mut n: i32 = 0;
    let mut digits: i32 = 0;
    while i < s.len() && s[i] >= b'0' && s[i] <= b'9' && digits < 3 {
        n = n * 10 + (s[i] - b'0') as i32;
        i += 1;
        digits += 1;
    }
    // origin: smf.cpp:44-45
    if digits == 0 || n < 1 || n > 16 {
        return -1;
    }
    // origin: smf.cpp:46-47
    if i == s.len() {
        return port;
    }
    // origin: smf.cpp:48-51  「A01-FrHorn 2」型は 2 桁 + 区切りのときだけ
    if digits == 2 && (s[i] == b'-' || s[i] == b' ' || s[i] == b'_' || s[i] == b':') {
        return port;
    }
    // origin: smf.cpp:52
    -1
}

/// origin: smf.h:34-40 — ファイルの口をエミュの口（0-3 = A-D）に割り当てる。
/// C++ の既定引数 `usb = false`（smf.h:34）は 2 引数版 [`mu_port_d`] として mirror
/// （呼び出し側: render.cpp:545 は 3 引数、autotest.mm:427 は 2 引数）。
pub fn mu_port(port: u8, fold: bool, usb: bool) -> i32 {
    let n: i32 = if usb { 4 } else { 2 }; // origin: smf.h:36
    let p = port as i32;                   // u8 → int 昇格 (smf.h:37)
    if p < n {
        return p;
    }
    if fold { p % n } else { -1 }          // origin: smf.h:39
}

/// `mu_port(port, fold, /*usb=*/false)` の既定形。origin: smf.h:34 の既定引数。
pub fn mu_port_d(port: u8, fold: bool) -> i32 {
    mu_port(port, fold, false)
}

// origin: smf.cpp:55-58  無名名前空間の be32/be16
#[inline]
fn be32(p: &[u8]) -> u32 {
    (u32::from(p[0]) << 24) | (u32::from(p[1]) << 16) | (u32::from(p[2]) << 8) | u32::from(p[3])
}

#[inline]
fn be16(p: &[u8]) -> u16 {
    (u16::from(p[0]) << 8) | u16::from(p[1])
}

/// SMF を (秒, バイト列) の並びに開く。format 0/1 の両方に対応する。
/// origin: smf.cpp:61-182。C++ と同じ「渡された out に append」语义。
///
/// C++ deviation (documented): C++ は ftell のバイト数を fread し不足なら
/// 「読めない」を返す（smf.cpp:66-70）。Rust は read_to_end で EOF まで読む
/// （通常ファイルでは同一。読み込み中に縮むファイルのみ挙動差 = 非現実的）。
pub fn load(path: &str, out: &mut Vec<Event>, err: &mut String) -> bool {
    // origin: smf.cpp:63-64
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            *err = format!("MIDI ファイルを開けない: {path}");
            return false;
        }
    };
    // origin: smf.cpp:65-71  (ftell 長 fread → 全文読み)
    let mut d: Vec<u8> = Vec::new();
    if std::io::Read::read_to_end(&mut f, &mut d).is_err() {
        *err = "MIDI ファイルを読めない".to_string(); // origin: smf.cpp:69
        return false;
    }

    // origin: smf.cpp:73-75
    if d.len() < 14 || &d[0..4] != b"MThd" {
        *err = "MThd がない。標準 MIDI ファイルではないらしい".to_string();
        return false;
    }
    // origin: smf.cpp:76-78
    let ntrk: u16 = be16(&d[10..12]);
    let div: u16 = be16(&d[12..14]);
    if div & 0x8000 != 0 {
        *err = "SMPTE 単位の MIDI には未対応".to_string();
        return false;
    }

    // origin: smf.cpp:80-82  まずは全トラックを (tick, バイト列) で集める
    struct Raw {
        tick: u64,
        bytes: Vec<u8>,
        tempo: bool,
        usec: u32,
        port: u8,
    }
    let mut all: Vec<Raw> = Vec::new();

    // origin: smf.cpp:84
    let mut pos: usize = 8usize.wrapping_add(be32(&d[4..8]) as usize);
    // origin: smf.cpp:85  for (u16 t = 0; t < ntrk && pos + 8 <= d.size(); t++)
    let mut t: u16 = 0;
    while t < ntrk && pos.wrapping_add(8) <= d.len() {
        // origin: smf.cpp:86  MTrk でなければ break（エラーにせず打ち切り）
        if &d[pos..pos + 4] != b"MTrk" {
            break;
        }
        // origin: smf.cpp:87-90
        let len = be32(&d[pos + 4..pos + 8]) as usize;
        let p0 = pos + 8;
        let mut p: usize = p0;
        let end = std::cmp::min(p0.wrapping_add(len), d.len());
        pos = p0.wrapping_add(len);

        // origin: smf.cpp:92-98
        let mut tick: u64 = 0;
        let mut running: u8 = 0;
        // トラックごとの出し先。`FF 21 01 pp` で決まる。無ければ 0。(origin: smf.cpp:94-95)
        let mut port: u8 = 0;
        let mut explicit_port = false;
        // origin: smf.cpp:99  while (p < end)
        while p < end {
            // origin: smf.cpp:100-105  可変長 delta（u64、溢れはラップ）
            let mut delta: u64 = 0;
            while p < end {
                delta = delta.wrapping_shl(7) | u64::from(d[p] & 0x7f);
                let b = d[p];
                p += 1;
                if b & 0x80 == 0 {
                    break;
                }
            }
            tick = tick.wrapping_add(delta);
            // origin: smf.cpp:106  delta だけで終わったトラックは打ち切り
            if p >= end {
                break;
            }

            // origin: smf.cpp:108-110  ランニングステータス
            let mut status = d[p];
            if status < 0x80 {
                status = running;
            } else {
                p += 1;
            }

            // origin: smf.cpp:112-149  メタイベント
            if status == 0xff {
                // smf.cpp:113 の d[p++] は p==end のとき C++ は無検査で読む
                // （end < size なら実バイト、end==size なら UB）。Rust は境界内は同じ
                // 実バイト、track 外 EOF 超過のみ 0（deviation: UB 領域のみ、正常入力不変）。
                let ty = *d.get(p).unwrap_or(&0); // origin: smf.cpp:113 u8 type = d[p++]
                p += 1;
                // origin: smf.cpp:114-115  meta 長 VLQ
                let mut l: u64 = 0;
                while p < end {
                    l = l.wrapping_shl(7) | u64::from(d[p] & 0x7f);
                    let b = d[p];
                    p += 1;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                // origin: smf.cpp:116-118  テンポ（FF 51 03）。payload を get() で読む
                // （C++ の d[p..p+2] 無検査 read と同じ実バイト。EOF 超過のみ 0）
                if ty == 0x51 && l == 3 {
                    let b0 = u32::from(*d.get(p).unwrap_or(&0));
                    let b1 = u32::from(*d.get(p + 1).unwrap_or(&0));
                    let b2 = u32::from(*d.get(p + 2).unwrap_or(&0));
                    all.push(Raw {
                        tick,
                        bytes: Vec::new(),
                        tempo: true,
                        usec: (b0 << 16) | (b1 << 8) | b2,
                        port: 0,
                    });
                }
                // origin: smf.cpp:119-122  ポート指定 FF 21 01 pp
                if ty == 0x21 && l == 1 {
                    port = *d.get(p).unwrap_or(&0);
                    explicit_port = true;
                }
                // origin: smf.cpp:123-128  ヤマハ固有 FF 7F 04 43 00 01 pp（issue #63）
                if ty == 0x7f
                    && l == 4
                    && p + 4 <= end
                    && d[p] == 0x43
                    && d[p + 1] == 0x00
                    && d[p + 2] == 0x01
                {
                    port = d[p + 3];
                    explicit_port = true;
                }
                // origin: smf.cpp:129-133  トラック名 FF 03（明示ポートが無効なときだけ）
                if ty == 0x03 && !explicit_port && (1..=64).contains(&l) {
                    // C++ std::min(p + l, end)。p+l の折返しは C++ では逆範囲 UB →
                    // Rust は空区間にクランプ（deviation: UB 領域のみ）
                    let hi = std::cmp::max(p, std::cmp::min(p.wrapping_add(l as usize), end));
                    let tp = port_from_track_name(&d[p..hi]);
                    if tp >= 0 {
                        port = tp as u8;
                    }
                }
                // origin: smf.cpp:134-146  機器名 FF 09 で口を言う流儀
                if ty == 0x09 && (1..=32).contains(&l) {
                    let hi = std::cmp::max(p, std::cmp::min(p.wrapping_add(l as usize), end));
                    let mut name: Vec<u8> = d[p..hi].to_vec();
                    // smf.cpp:137  末尾の ' ' と NUL 落とし
                    while let Some(&b) = name.last() {
                        if b == b' ' || b == 0 {
                            name.pop();
                        } else {
                            break;
                        }
                    }
                    // smf.cpp:138  小文字化
                    for c in name.iter_mut() {
                        if c.is_ascii_uppercase() {
                            *c += 32;
                        }
                    }
                    // smf.cpp:139-145  「A」〜「D」か「Port 1」〜「Port 4」だけ見る
                    if name.len() == 1 && name[0] >= b'a' && name[0] <= b'd' {
                        port = name[0] - b'a';
                        explicit_port = true;
                    } else if name.len() == 6
                        && &name[0..5] == b"port "
                        && name[5] >= b'1'
                        && name[5] <= b'4'
                    {
                        port = name[5] - b'1';
                        explicit_port = true;
                    }
                }
                // origin: smf.cpp:147-148  p += l（end を越えても加算 — まるごと飛ばす）
                p = p.wrapping_add(l as usize);
                continue;
            }

            // origin: smf.cpp:150-159  システムエクスクルーシブ
            if status == 0xf0 || status == 0xf7 {
                let mut l: u64 = 0;
                while p < end {
                    l = l.wrapping_shl(7) | u64::from(d[p] & 0x7f);
                    let b = d[p];
                    p += 1;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                // 0xf0 は先頭に 0xf0 を付ける。0xf7（続き）は付けない (smf.cpp:154)
                let mut b: Vec<u8> = Vec::new();
                if status == 0xf0 {
                    b.push(0xf0);
                }
                let hi = std::cmp::max(p, std::cmp::min(p.wrapping_add(l as usize), end));
                b.extend_from_slice(&d[p..hi]); // smf.cpp:155  end でクランプ
                p = p.wrapping_add(l as usize); // smf.cpp:156  クランプしない
                all.push(Raw { tick, bytes: b, tempo: false, usec: 0, port });
                continue;
            }

            // origin: smf.cpp:161-165  チャンネル（および system byte もこの道に落ちる）
            running = status;
            let n: i32 = if (status & 0xf0) == 0xc0 || (status & 0xf0) == 0xd0 {
                1
            } else {
                2
            };
            let mut b: Vec<u8> = vec![status];
            let mut i = 0;
            while i < n && p < end {
                b.push(d[p]);
                p += 1;
                i += 1;
            }
            all.push(Raw { tick, bytes: b, tempo: false, usec: 0, port });
        }
        t += 1;
    }

    // origin: smf.cpp:169-170  stable_sort（同 tick は挿入順 = トラック順maintained）
    all.sort_by(|a, b| a.tick.cmp(&b.tick));

    // origin: smf.cpp:172-181  テンポを追いながら秒に直す。既定 120 BPM。
    // 演算順は C++ と同一: sec += (double)(tick-last) * us_per_beat / ((double)div * 1e6)
    let mut sec: f64 = 0.0;
    let mut us_per_beat: f64 = 500000.0;
    let mut last: u64 = 0;
    for e in all.into_iter() {
        sec += (e.tick.wrapping_sub(last) as f64) * us_per_beat / (f64::from(div) * 1e6);
        last = e.tick;
        if e.tempo {
            us_per_beat = f64::from(e.usec);
            continue; // origin: smf.cpp:178  テンポ自体は out に出さない
        }
        out.push(Event { time: sec, bytes: e.bytes, port: e.port }); // origin: smf.cpp:179
    }
    true // origin: smf.cpp:181
}

#[cfg(test)]
mod tests;
