//! Adds a "DISPLAY" tab to Fight Night Champion's Settings screen, in the
//! player's own copy of `databigs/xenon/fe.big`. Same result as
//! `tools/display_tab/aptpatch_display.py`, byte for byte.
//!
//! The Settings screen is the Apt (Flash-like) movie `gamesettingdata.big`
//! inside fe.big. The patch
//!  * appends new strings to the movie's constant file (the items array moves to the end),
//!  * copies the movie's frame-0 action to the end of its data file with a bigger
//!    ConstantPool and our code before the final End, then points frame 0 at the copy,
//!  * re-packs the member (RefPack-compressed: the game decompresses in place, so a member
//!    must not be bigger compressed than uncompressed) and appends it to fe.big,
//!    repointing that file's table entry.
//!
//! The values of the new rows come from the game exe (project/src/fnc_settings_menu.cpp:
//! category 200, settings 201 and up). The original file is kept as fe.big.orig and every
//! run patches from it, so running again is safe.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const CATEGORY: u32 = 200;
const TAB_NAME: &str = "DISPLAY";
/// (title, labels) -> setting ids 201, 202, ...; must match kRows in fnc_settings_menu.cpp.
const ROWS: &[(&str, &[&str])] = &[
    (
        "FRAME RATE (RESTART)",
        &[
            "30 FPS", "60 FPS", "120 FPS", "144 FPS", "165 FPS", "180 FPS", "240 FPS",
        ],
    ),
    ("VSYNC", &["OFF", "ON"]),
    ("RESOLUTION SCALE (RESTART)", &["1X", "2X", "3X"]),
    ("ANTI-ALIASING", &["OFF", "FXAA", "FXAA EXTREME"]),
    ("FULLSCREEN", &["OFF", "ON"]),
];
const CONTEXTS: &[&str] = &[
    "FROM_MAIN_MENU",
    "FROM_PAUSE_MENU",
    "FROM_MOVIE_PAUSE_MENU",
    "FROM_TRAINING_PAUSE_MENU",
    "FROM_PLAY_NOW_FLOW",
    "FROM_LEGACY",
    "FROM_AMATEUR_TOURNAMENT",
];
const MEMBER: &str = "gamesettingdata.big";

fn u32be(b: &[u8], o: usize) -> Result<u32> {
    let s = b
        .get(o..o + 4)
        .with_context(|| format!("read past the end at 0x{o:X}"))?;
    Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}
fn p32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&p32(v));
}

// ------------------------------------------------------------------ RefPack

pub fn unrefpack(b: &[u8]) -> Result<Vec<u8>> {
    if b.len() < 5 || b[1] != 0xFB {
        return Ok(b.to_vec());
    }
    let flags = b[0];
    let mut p = 2;
    let size = if flags & 0x80 != 0 {
        p += 4;
        u32::from_be_bytes([b[2], b[3], b[4], b[5]]) as usize
    } else {
        p += 3;
        ((b[2] as usize) << 16) | ((b[3] as usize) << 8) | b[4] as usize
    };
    if flags & 0x01 != 0 {
        p += if flags & 0x80 != 0 { 4 } else { 3 };
    }
    let mut out: Vec<u8> = Vec::with_capacity(size);
    let get = |p: usize| -> Result<u8> { b.get(p).copied().context("RefPack data is cut short") };
    while p < b.len() {
        let c0 = b[p];
        let (lit, ln, off);
        if c0 < 0x80 {
            let c1 = get(p + 1)? as usize;
            p += 2;
            lit = (c0 & 3) as usize;
            ln = ((c0 as usize >> 2) & 7) + 3;
            off = ((c0 as usize & 0x60) << 3) + c1 + 1;
        } else if c0 < 0xC0 {
            let (c1, c2) = (get(p + 1)? as usize, get(p + 2)? as usize);
            p += 3;
            lit = c1 >> 6;
            ln = (c0 as usize & 0x3F) + 4;
            off = ((c1 & 0x3F) << 8) + c2 + 1;
        } else if c0 < 0xE0 {
            let (c1, c2, c3) = (
                get(p + 1)? as usize,
                get(p + 2)? as usize,
                get(p + 3)? as usize,
            );
            p += 4;
            lit = (c0 & 3) as usize;
            ln = ((c0 as usize & 0x0C) << 6) + c3 + 5;
            off = ((c0 as usize & 0x10) << 12) + (c1 << 8) + c2 + 1;
        } else if c0 < 0xFC {
            let n = ((c0 as usize & 0x1F) << 2) + 4;
            p += 1;
            out.extend_from_slice(b.get(p..p + n).context("RefPack data is cut short")?);
            p += n;
            continue;
        } else {
            let n = (c0 & 3) as usize;
            p += 1;
            out.extend_from_slice(b.get(p..p + n).context("RefPack data is cut short")?);
            break;
        }
        out.extend_from_slice(b.get(p..p + lit).context("RefPack data is cut short")?);
        p += lit;
        if off > out.len() {
            bail!("RefPack data is damaged");
        }
        for _ in 0..ln {
            let v = out[out.len() - off];
            out.push(v);
        }
    }
    Ok(out)
}

/// Greedy LZ with hash chains (same choices as tools/display_tab/refpack_enc.py).
pub fn refpack(data: &[u8]) -> Result<Vec<u8>> {
    let n = data.len();
    let mut out = vec![0x10, 0xFB, (n >> 16) as u8, (n >> 8) as u8, n as u8];
    let mut heads: HashMap<[u8; 3], Vec<usize>> = HashMap::new();
    let key = |k: usize| [data[k], data[k + 1], data[k + 2]];
    let mut lit_start = 0usize;
    let mut i = 0usize;
    let flush = |out: &mut Vec<u8>, lit_start: &mut usize, upto: usize| {
        while upto - *lit_start >= 4 {
            let k = 112.min((upto - *lit_start) & !3);
            out.push(0xE0 | ((k - 4) >> 2) as u8);
            out.extend_from_slice(&data[*lit_start..*lit_start + k]);
            *lit_start += k;
        }
    };
    while i < n {
        let (mut best_len, mut best_off) = (0usize, 0usize);
        if i + 3 <= n
            && let Some(list) = heads.get(&key(i))
        {
            let from = list.len().saturating_sub(48);
            for &p in list[from..].iter().rev() {
                let off = i - p;
                if off > 131072 {
                    break;
                }
                let mut l = 3;
                let m = 1028.min(n - i);
                while l < m && data[p + l] == data[i + l] {
                    l += 1;
                }
                let ok = (l >= 3 && off <= 1024) || (l >= 4 && off <= 16384) || l >= 5;
                if ok && l > best_len {
                    best_len = l;
                    best_off = off;
                }
                if best_len >= 1028 {
                    break;
                }
            }
        }
        if best_len > 0 {
            flush(&mut out, &mut lit_start, i);
            let lit = i - lit_start;
            if lit > 3 {
                bail!("RefPack: internal error");
            }
            let mut l = best_len;
            let off = best_off - 1;
            if l <= 10 && best_off <= 1024 {
                out.push((((off >> 3) & 0x60) | ((l - 3) << 2) | lit) as u8);
                out.push(off as u8);
            } else if l <= 67 && best_off <= 16384 {
                out.push((0x80 | (l - 4)) as u8);
                out.push(((lit << 6) | (off >> 8)) as u8);
                out.push(off as u8);
            } else {
                l = l.min(1028);
                out.push((0xC0 | ((off >> 12) & 0x10) | (((l - 5) >> 6) & 0x0C) | lit) as u8);
                out.push((off >> 8) as u8);
                out.push(off as u8);
                out.push((l - 5) as u8);
            }
            out.extend_from_slice(&data[lit_start..i]);
            for k in i..(i + l).min(n.saturating_sub(2)) {
                heads.entry(key(k)).or_default().push(k);
            }
            i += l;
            lit_start = i;
        } else {
            if i + 3 <= n {
                heads.entry(key(i)).or_default().push(i);
            }
            i += 1;
        }
    }
    flush(&mut out, &mut lit_start, n);
    let rest = n - lit_start;
    out.push(0xFC | rest as u8);
    out.extend_from_slice(&data[lit_start..]);
    if unrefpack(&out)? != data {
        bail!("RefPack: round trip failed");
    }
    Ok(out)
}

// ------------------------------------------------------------------ archives

struct EbEntry {
    name: String,
    offset: u64,
    size: u32,
}

/// The table of an "EB" v3 archive (fe.big).
fn eb_index(f: &mut File) -> Result<Vec<EbEntry>> {
    let mut h = [0u8; 0x30];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut h)?;
    let n = u32be(&h, 4)? as usize;
    let (name_off, name_size) = (u32be(&h, 0xC)? as u64, u32be(&h, 0x10)? as usize);
    let name_len = h[0x14] as usize;
    let mut tab = vec![0u8; n * 16];
    f.read_exact(&mut tab)?;
    let mut names = vec![0u8; name_size];
    f.seek(SeekFrom::Start(name_off))?;
    f.read_exact(&mut names)?;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let rec = names
            .get(i * name_len..(i + 1) * name_len)
            .context("fe.big name table is damaged")?;
        let raw = &rec[2..];
        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        out.push(EbEntry {
            name: String::from_utf8_lossy(&raw[..end]).into_owned(),
            offset: (u32be(&tab, i * 16)? as u64) << 4,
            size: u32be(&tab, i * 16 + 8)?,
        });
    }
    Ok(out)
}

/// BIGF members as (name, bytes).
type Members = Vec<(String, Vec<u8>)>;

fn bigf_read(b: &[u8]) -> Result<(Members, Vec<u8>)> {
    if b.get(0..4) != Some(b"BIGF") {
        bail!("{MEMBER} is not a BIGF archive");
    }
    let n = u32be(b, 8)? as usize;
    let mut p = 16;
    let mut mem = Vec::with_capacity(n);
    for _ in 0..n {
        let (off, size) = (u32be(b, p)? as usize, u32be(b, p + 4)? as usize);
        p += 8;
        let e = p + b[p..]
            .iter()
            .position(|&c| c == 0)
            .context("bad BIGF name")?;
        let name = String::from_utf8_lossy(&b[p..e]).into_owned();
        p = e + 1;
        mem.push((
            name,
            b.get(off..off + size).context("bad BIGF member")?.to_vec(),
        ));
    }
    let footer = b
        .get(p..u32be(b, 12)? as usize)
        .context("bad BIGF header")?
        .to_vec();
    Ok((mem, footer))
}

fn bigf_write(mem: &[(String, Vec<u8>)], footer: &[u8]) -> Vec<u8> {
    const ALIGN: usize = 0x40;
    let up = |v: usize| v.div_ceil(ALIGN) * ALIGN;
    let dir_len = 16 + mem.iter().map(|(n, _)| 8 + n.len() + 1).sum::<usize>();
    let hdr_size = dir_len + footer.len();
    let mut pos = up(hdr_size);
    let mut offs = Vec::with_capacity(mem.len());
    for (_, data) in mem {
        offs.push(pos);
        if !data.is_empty() {
            pos = up(pos + data.len());
        }
    }
    let total = offs.last().copied().unwrap_or(0) + mem.last().map(|m| m.1.len()).unwrap_or(0);
    let mut h = b"BIGF".to_vec();
    h.extend_from_slice(&(total as u32).to_le_bytes());
    h.extend_from_slice(&p32(mem.len() as u32));
    h.extend_from_slice(&p32(hdr_size as u32));
    for ((name, data), off) in mem.iter().zip(&offs) {
        h.extend_from_slice(&p32(*off as u32));
        h.extend_from_slice(&p32(data.len() as u32));
        h.extend_from_slice(name.as_bytes());
        h.push(0);
    }
    h.extend_from_slice(footer);
    let mut out = h;
    for ((_, data), off) in mem.iter().zip(&offs) {
        if *off > out.len() {
            out.resize(*off, 0);
        }
        out.extend_from_slice(data);
    }
    out.truncate(total);
    out
}

// ------------------------------------------------------------------ Apt bytecode

/// Opcodes whose operands start at the next 4-byte boundary.
const ALIGNED: &[u8] = &[
    0x9B, 0x8E, 0x88, 0x9D, 0x99, 0x96, 0x83, 0x8C, 0x87, 0x8B, 0x81, 0x9F, 0x94, 0xA1, 0xB8, 0xA4,
    0xA6, 0xA5, 0xA7,
];
/// Opcodes without operands.
const PLAIN: &[u8] = &[
    0x00, 0x04, 0x05, 0x06, 0x07, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14,
    0x15, 0x17, 0x18, 0x1C, 0x1D, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x29, 0x2A, 0x2B, 0x2C,
    0x30, 0x34, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
    0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x58,
    0x59, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x8B, 0x94, 0x9A, 0x9E, 0xFF,
];

#[derive(Default)]
struct Walk {
    /// Offsets of anonymous DefineFunction (0x9B) ops, in order.
    anon_functions: Vec<usize>,
    /// The last op seen at the top level was End.
    ended: bool,
}

/// Walks an action stream the way the game reads it. Returns the offset after
/// the top-level End.
fn walk(a: &[u8], start: usize, limit: Option<usize>, depth: u32, w: &mut Walk) -> Result<usize> {
    let mut p = start;
    let lim = limit.unwrap_or(a.len());
    while p < lim {
        let op = a[p];
        let pos = p;
        p += 1;
        if ALIGNED.contains(&op) {
            p = (p + 3) & !3;
        }
        match op {
            0x88 | 0x96 | 0x83 => p += 8,
            0x8E => {
                let size = u32be(a, p + 16)? as usize;
                p += 28;
                walk(a, p, Some(p + size), depth + 1, w)?;
                p += size;
                continue;
            }
            0x9B => {
                // Anonymous: no name, or an empty one.
                let nm = u32be(a, p)? as usize;
                if nm == 0 || cstr(a, nm).is_some_and(|n| n.is_empty()) {
                    w.anon_functions.push(pos);
                }
                let size = u32be(a, p + 12)? as usize;
                p += 24;
                walk(a, p, Some(p + size), depth + 1, w)?;
                p += size;
                continue;
            }
            0x99 | 0x9D | 0xB8 | 0xA1 | 0xA4 | 0xA5 | 0xA6 | 0xA7 | 0x8C | 0x87 | 0x81 | 0x9F
            | 0xB4 | 0xB7 => p += 4,
            0xA2 | 0xAE | 0xAF | 0xB0 | 0xB1 | 0xB2 | 0xB3 | 0xB5 | 0xB9 => p += 1,
            0xA3 | 0xB6 => p += 2,
            _ if PLAIN.contains(&op) => {}
            _ => bail!("unknown Apt opcode 0x{op:02X} at 0x{pos:X}"),
        }
        if depth == 0 {
            w.ended = op == 0x00;
            if w.ended {
                return Ok(p);
            }
        }
    }
    Ok(p)
}

struct Asm<'a> {
    b: Vec<u8>,
    base: usize,
    ix: &'a dyn Fn(&str) -> Result<usize>,
}

impl Asm<'_> {
    fn pos(&self) -> usize {
        self.base + self.b.len()
    }
    fn op(&mut self, o: u8) {
        self.b.push(o);
    }
    fn align(&mut self) {
        while !self.pos().is_multiple_of(4) {
            self.b.push(0);
        }
    }
    fn konst(&mut self, s: &str) -> Result<()> {
        let i = (self.ix)(s)?;
        if i < 256 {
            self.b.extend_from_slice(&[0xA2, i as u8]);
        } else {
            self.b.push(0xA3);
            self.b.extend_from_slice(&(i as u16).to_be_bytes());
        }
        Ok(())
    }
    fn byte_op(&mut self, o: u8, s: &str) -> Result<()> {
        let i = (self.ix)(s)?;
        if i >= 256 {
            bail!("constant '{s}' is out of byte range");
        }
        self.b.extend_from_slice(&[o, i as u8]);
        Ok(())
    }
    fn var(&mut self, s: &str) -> Result<()> {
        self.byte_op(0xAE, s) // PushValueOfVar
    }
    fn member(&mut self, s: &str) -> Result<()> {
        self.byte_op(0xAF, s) // GetNamedMember
    }
    fn int(&mut self, v: u32) {
        match v {
            0 => self.op(0x59),
            1 => self.op(0x5A),
            2..=127 => self.b.extend_from_slice(&[0xB5, v as u8]),
            _ => {
                self.b.push(0xB6);
                self.b.extend_from_slice(&(v as u16).to_be_bytes());
            }
        }
    }
    /// Anonymous DefineFunction (0x9B) whose body is written by `body`.
    fn function(
        &mut self,
        name_ptr: u32,
        params_ptr: u32,
        body: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
        self.b.push(0x9B);
        self.align();
        let hdr = self.b.len();
        for v in [name_ptr, 0, params_ptr, 0] {
            self.b.extend_from_slice(&p32(v));
        }
        self.b
            .extend_from_slice(&[0x98, 0x76, 0x54, 0x32, 0x12, 0x34, 0x56, 0x78]);
        let start = self.b.len();
        body(self)?;
        let size = (self.b.len() - start) as u32;
        put32(&mut self.b, hdr + 12, size);
        Ok(())
    }
}

fn cstr(b: &[u8], o: usize) -> Option<&[u8]> {
    let rest = b.get(o..)?;
    let e = rest.iter().position(|&c| c == 0)?;
    Some(&rest[..e])
}

/// Patches the movie's data file (member "0") and constant file (member "1").
/// Returns (data, const, offset of the new frame action).
fn patch_apt(data: &[u8], konst: &[u8]) -> Result<(Vec<u8>, Vec<u8>, usize)> {
    let mut data = data.to_vec();
    let mut konst = konst.to_vec();
    // --- constant file: append strings + a new, longer items array
    let n_items = u32be(&konst, 0x18)? as usize;
    let items_off = u32be(&konst, 0x1C)? as usize;
    let mut items = konst
        .get(items_off..items_off + n_items * 8)
        .context("bad constant table")?
        .to_vec();
    let mut names: Vec<&str> = vec![TAB_NAME];
    names.extend(ROWS.iter().map(|(t, _)| *t));
    names.extend(ROWS.iter().flat_map(|(_, ls)| ls.iter().copied()));
    let mut new_strings: Vec<(&str, usize)> = Vec::new();
    for s in names {
        if new_strings.iter().any(|(x, _)| *x == s) {
            continue;
        }
        new_strings.push((s, konst.len()));
        konst.extend_from_slice(s.as_bytes());
        konst.push(0);
    }
    while !konst.len().is_multiple_of(4) {
        konst.push(0);
    }
    for (_, off) in &new_strings {
        items.extend_from_slice(&p32(1));
        items.extend_from_slice(&p32(*off as u32));
    }
    let new_count = new_strings.len();
    let klen = konst.len() as u32;
    put32(&mut konst, 0x1C, klen);
    put32(&mut konst, 0x18, (n_items + new_count) as u32);
    konst.extend_from_slice(&items);

    // --- data file: the frame-0 action
    let movie = u32be(&konst, 0x14)? as usize;
    if u32be(&data, movie + 4)? != 0x0987_6543 {
        bail!("{MEMBER}: unexpected movie layout");
    }
    let frames = u32be(&data, movie + 20)? as usize;
    let (icnt, iptr) = (
        u32be(&data, frames)? as usize,
        u32be(&data, frames + 4)? as usize,
    );
    let mut act_item = None;
    for k in 0..icnt {
        let it = u32be(&data, iptr + 4 * k)? as usize;
        if u32be(&data, it)? == 1 {
            act_item = Some(it);
        }
    }
    let act_item = act_item.context("frame 0 has no action")?;
    let old = u32be(&data, act_item + 4)? as usize;
    if data.get(old) != Some(&0x88) {
        bail!("the frame action does not start with a ConstantPool");
    }
    let pool_n = u32be(&data, old + 4)? as usize;
    let pool_ptr = u32be(&data, old + 8)? as usize;
    let pool: Vec<u32> = (0..pool_n)
        .map(|i| u32be(&data, pool_ptr + 4 * i))
        .collect::<Result<_>>()?;
    let mut w = Walk::default();
    let end = walk(&data, old, None, 0, &mut w)? - 1;
    if !w.ended || data[end] != 0x00 {
        bail!("the frame action does not end with End");
    }
    let fn_off = *w
        .anon_functions
        .first()
        .context("no function to copy the layout from")?;
    let q = (fn_off + 1 + 3) & !3;
    let (name_ptr, params_ptr) = (u32be(&data, q)?, u32be(&data, q + 8)?);

    let konst_ref = &konst;
    let pool_ref = &pool;
    let new_ref = &new_strings;
    let pool_ix = move |s: &str| -> Result<usize> {
        if let Some(k) = new_ref.iter().position(|(x, _)| *x == s) {
            return Ok(pool_n + k);
        }
        for (i, &ci) in pool_ref.iter().enumerate() {
            let ci = ci as usize;
            let t = u32be(konst_ref, items_off + ci * 8)?;
            let v = u32be(konst_ref, items_off + ci * 8 + 4)? as usize;
            if t == 1 && cstr(konst_ref, v) == Some(s.as_bytes()) {
                return Ok(i);
            }
        }
        bail!("the Settings screen has no constant '{s}'")
    };

    while !data.len().is_multiple_of(8) {
        data.push(0);
    }
    let new = data.len();
    if (old + 12) % 4 != (new + 12) % 4 {
        bail!("unexpected alignment of the frame action");
    }
    let mut a = Asm {
        b: Vec::new(),
        base: new,
        ix: &pool_ix,
    };
    a.op(0x88);
    a.align();
    let patch_at = a.b.len();
    a.b.extend_from_slice(&[0; 8]);
    a.b.extend_from_slice(&data[old + 12..end]); // the original statements, verbatim

    // categories[200] = new Object(); .name = "DISPLAY"; .settings = [201, ...]
    a.var("categories")?;
    a.int(CATEGORY);
    a.int(0);
    a.konst("Object")?;
    a.op(0x40);
    a.op(0x4F);
    a.var("categories")?;
    a.int(CATEGORY);
    a.op(0x4E);
    a.konst("name")?;
    a.konst(TAB_NAME)?;
    a.op(0x4F);
    a.var("categories")?;
    a.int(CATEGORY);
    a.op(0x4E);
    a.konst("settings")?;
    let ids: Vec<u32> = (0..ROWS.len() as u32).map(|i| CATEGORY + 1 + i).collect();
    for sid in ids.iter().rev() {
        a.int(*sid);
    }
    a.int(ids.len() as u32);
    a.op(0x42);
    a.op(0x4F);
    // initSettings(id, title, "toggle", labelsFn, valuesFn, true)
    for (sid, (title, labels)) in ids.iter().zip(ROWS) {
        a.op(0x73); // isEnabled = true
        let n = labels.len() as u32;
        a.function(name_ptr, params_ptr, |a| {
            for v in (0..n).rev() {
                a.int(v);
            }
            a.int(n);
            a.konst("Array")?;
            a.op(0x40);
            a.op(0x3E);
            Ok(())
        })?;
        a.function(name_ptr, params_ptr, |a| {
            for s in labels.iter().rev() {
                a.konst(s)?;
            }
            a.int(n);
            a.konst("Array")?;
            a.op(0x40);
            a.op(0x3E);
            Ok(())
        })?;
        a.konst("toggle")?;
        a.konst(title)?;
        a.int(*sid);
        a.int(6);
        a.byte_op(0xB0, "initSettings")?;
    }
    // context[Settings.eContext.X].push(200)
    for c in CONTEXTS {
        a.int(CATEGORY);
        a.int(1);
        a.var("context")?;
        a.op(0x71);
        a.member("Settings")?;
        a.member("eContext")?;
        a.member(c)?;
        a.op(0x4E);
        a.byte_op(0xB2, "push")?;
    }
    a.op(0x00); // End
    let code = std::mem::take(&mut a.b);
    drop(a);

    data.extend_from_slice(&code);
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    let idx_ptr = data.len();
    for ci in &pool {
        data.extend_from_slice(&p32(*ci));
    }
    for k in 0..new_count {
        data.extend_from_slice(&p32((n_items + k) as u32));
    }
    put32(&mut data, new + patch_at, (pool_n + new_count) as u32);
    put32(&mut data, new + patch_at + 4, idx_ptr as u32);
    put32(&mut data, act_item + 4, new as u32);

    // The new action must read cleanly from start to End.
    let mut w = Walk::default();
    walk(&data, new, None, 0, &mut w)?;
    if !w.ended {
        bail!("the patched frame action does not end with End");
    }
    Ok((data, konst, new))
}

/// True when this constant file already has the DISPLAY tab.
fn already_patched(konst: &[u8]) -> bool {
    let needle = ROWS[2].0.as_bytes();
    konst.windows(needle.len()).any(|w| w == needle)
}

/// Patches `fe_big` in place. Ok(false) when it already has the tab.
/// The Settings screen member of an fe.big: (table index, members, footer,
/// index of member "0", index of member "1").
fn read_member(path: &Path) -> Result<(usize, Members, Vec<u8>, usize, usize)> {
    let mut f = File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let idx = eb_index(&mut f)?;
    let i = idx
        .iter()
        .position(|e| e.name == MEMBER)
        .with_context(|| format!("{MEMBER} not found in fe.big"))?;
    let mut big = vec![0u8; idx[i].size as usize];
    f.seek(SeekFrom::Start(idx[i].offset))?;
    f.read_exact(&mut big)?;
    let (mem, footer) = bigf_read(&big)?;
    let find = |n: &str| mem.iter().position(|m| m.0 == n);
    let d0 = find("0").context("member 0 missing")?;
    let d1 = find("1").context("member 1 missing")?;
    Ok((i, mem, footer, d0, d1))
}

/// Patches `fe_big` in place. Ok(false) when it already has the tab.
pub fn patch_fe_big(fe_big: &Path) -> Result<bool> {
    let orig = fe_big.with_file_name(format!(
        "{}.orig",
        fe_big.file_name().unwrap_or_default().to_string_lossy()
    ));
    // Skip only a current menu; older DISPLAY tabs must be rebuilt from .orig.
    {
        let (_, mem, _, _, d1) = read_member(fe_big)?;
        let konst = unrefpack(&mem[d1].1)?;
        if already_patched(&konst)
            && ROWS.iter().flat_map(|(_, labels)| *labels).all(|label| {
                konst.windows(label.len()).any(|w| w == label.as_bytes())
            })
        {
            return Ok(false);
        }
    }
    if !orig.exists() {
        fs::copy(fe_big, &orig).context("could not back up fe.big")?;
    }
    // Always patch from the original.
    let (i, mut mem, footer, d0, d1) = read_member(&orig)?;
    let data = unrefpack(&mem[d0].1)?;
    let konst = unrefpack(&mem[d1].1)?;
    if already_patched(&konst) {
        bail!(
            "fe.big.orig already has the DISPLAY tab; restore the original fe.big from your disc"
        );
    }
    let (data, konst, _) = patch_apt(&data, &konst)?;
    mem[d0].1 = refpack(&data)?;
    mem[d1].1 = refpack(&konst)?;
    let newbig = bigf_write(&mem, &footer);

    let tmp = fe_big.with_file_name("fe.big.tmp");
    fs::copy(&orig, &tmp).context("could not write fe.big (is the disk full?)")?;
    let mut t = OpenOptions::new().read(true).write(true).open(&tmp)?;
    let end = t.seek(SeekFrom::End(0))?;
    let pad = ((16 - end % 16) % 16) as usize;
    t.write_all(&vec![0u8; pad])?;
    let at = end + pad as u64;
    t.write_all(&newbig)?;
    t.write_all(&vec![0u8; (16 - newbig.len() % 16) % 16])?;
    let ent = 0x30 + i as u64 * 16;
    let mut e = [0u8; 16];
    t.seek(SeekFrom::Start(ent))?;
    t.read_exact(&mut e)?;
    e[0..4].copy_from_slice(&p32((at >> 4) as u32));
    e[8..12].copy_from_slice(&p32(newbig.len() as u32));
    t.seek(SeekFrom::Start(ent))?;
    t.write_all(&e)?;
    let new_size = t.seek(SeekFrom::End(0))?;
    let mut hs = [0u8; 4];
    t.seek(SeekFrom::Start(0x1C))?;
    t.read_exact(&mut hs)?;
    if u32::from_be_bytes(hs) as u64 == end {
        // Header field 0x1C is the archive size.
        t.seek(SeekFrom::Start(0x1C))?;
        t.write_all(&p32(new_size as u32))?;
    }
    t.sync_all()?;
    drop(t);
    fs::rename(&tmp, fe_big).context("could not replace fe.big")?;
    Ok(true)
}
