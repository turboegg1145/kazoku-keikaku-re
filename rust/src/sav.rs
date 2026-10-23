//! `KIZUNAR.SAV` 存档容器（布局见 `docs/06-save.md`）。
//!
//! ```text
//! 0x00  u32 offset[30]   块偏移表；块 i = [offset[i], offset[i+1])
//!       块 0..26 = 27 个存档格，块 27/28 = 引擎保留块，块 29 = 已读文本位图
//! ```
//!
//! 存档格固定 60181 字节：SYSTEMTIME + 160×120 缩略图 + 标题 + 位数组 + 变量数组。

use crate::{Error, Result};

pub const TABLE_N: usize = 30;
pub const SLOT_N: usize = 27;
pub const TABLE_BYTES: usize = TABLE_N * 4;

pub const THUMB_W: usize = 160;
pub const THUMB_H: usize = 120;
pub const THUMB_HDR: usize = 40;
pub const THUMB_SIZE: usize = THUMB_HDR + THUMB_W * THUMB_H * 3; // 0xE128

pub const TITLE_SIZE: usize = 0xA0;
pub const EXTRA_SIZE: usize = 20;
pub const BITS_N: usize = 1025; // 0x30 0x0400 -> n+1
pub const BITS_BYTES: usize = (BITS_N + 7) / 8; // 129
pub const VARS_N: usize = 542; // 0x40 0x021D -> n+1
pub const TAIL_SIZE: usize = 0x30;
pub const SLOT_SIZE: usize =
    0x10 + THUMB_SIZE + TITLE_SIZE + EXTRA_SIZE + BITS_BYTES + VARS_N * 4 + TAIL_SIZE;

pub const SYSTEM_BLOCKS: [usize; 2] = [27, 28];
pub const GLOBAL_BLOCK: usize = 29;
pub const GLOBAL_BITS_N: usize = 30001; // 0xA5 0x7530 -> n+1

/// 一个存档格。`time` 是 Win32 `SYSTEMTIME` 的 8 个 u16。
#[derive(Clone)]
pub struct Slot {
    pub time: [u16; 8],
    pub thumb: Vec<u8>,
    pub title: String,
    pub extra: [u8; EXTRA_SIZE],
    pub bits: Vec<u8>,
    pub vars: Vec<i32>,
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            time: [0; 8],
            thumb: vec![0; THUMB_SIZE],
            title: String::new(),
            extra: [0; EXTRA_SIZE],
            bits: vec![0; BITS_BYTES],
            vars: vec![0; VARS_N],
        }
    }
}

impl Slot {
    pub fn used(&self) -> bool {
        self.time.iter().any(|&x| x != 0)
    }

    /// 读档界面的排序键：日期优先，其次时分秒。
    pub fn sort_key(&self) -> (u32, u32) {
        let t = &self.time;
        (
            t[0] as u32 * 0x10000 + t[1] as u32 * 0x100 + t[3] as u32,
            t[4] as u32 * 0x10000 + t[5] as u32 * 0x100 + t[6] as u32,
        )
    }
}

#[derive(Clone, Default)]
pub struct SaveFile {
    pub slots: Vec<Slot>,
    pub system: Vec<(usize, Vec<u8>)>,
    pub global_bits: Vec<u8>,
}

impl SaveFile {
    pub fn new() -> Self {
        SaveFile { slots: vec![Slot::default(); SLOT_N], system: Vec::new(), global_bits: Vec::new() }
    }

    pub fn newest(&self) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, s) in self.slots.iter().enumerate() {
            if !s.used() {
                continue;
            }
            best = match best {
                None => Some(i),
                Some(b) if s.sort_key() > self.slots[b].sort_key() => Some(i),
                other => other,
            };
        }
        best
    }

    pub fn system_block(&self, idx: usize) -> &[u8] {
        self.system.iter().find(|(i, _)| *i == idx).map(|(_, b)| &b[..]).unwrap_or(&[])
    }
}

pub fn parse(data: &[u8]) -> Result<SaveFile> {
    if data.len() < TABLE_BYTES {
        return Err(Error(format!("SAV 太短：{} 字节", data.len())));
    }
    let mut off = [0usize; TABLE_N];
    for (i, o) in off.iter_mut().enumerate() {
        *o = crate::u32le(data, i * 4) as usize;
    }
    // 空块的偏移是 0，所以块的结束位置要看「下一个非零偏移」
    let mut ends = [data.len(); TABLE_N];
    let mut nxt = data.len();
    for i in (0..TABLE_N).rev() {
        ends[i] = nxt;
        if off[i] != 0 {
            nxt = off[i];
        }
    }
    let mut sf = SaveFile::new();
    for i in 0..TABLE_N {
        let (start, end) = (off[i], ends[i]);
        if start == 0 || start >= end || end > data.len() {
            continue;
        }
        let blob = &data[start..end];
        if i < SLOT_N {
            sf.slots[i] = parse_slot(blob)?;
        } else if SYSTEM_BLOCKS.contains(&i) {
            sf.system.push((i, blob.to_vec()));
        } else if i == GLOBAL_BLOCK && blob.len() >= 4 {
            let n = crate::u32le(blob, 0) as usize;
            sf.global_bits = blob[4..(4 + n).min(blob.len())].to_vec();
        }
    }
    Ok(sf)
}

fn parse_slot(blob: &[u8]) -> Result<Slot> {
    if blob.len() < SLOT_SIZE {
        return Err(Error(format!("存档格太短：{}（应为 {}）", blob.len(), SLOT_SIZE)));
    }
    let mut s = Slot::default();
    for i in 0..8 {
        s.time[i] = crate::u16le(blob, i * 2);
    }
    s.thumb = blob[0x10..0x10 + THUMB_SIZE].to_vec();
    let raw = &blob[0xE138..0xE138 + TITLE_SIZE];
    let cut = raw.iter().position(|&b| b == 0).unwrap_or(TITLE_SIZE);
    s.title = encoding_rs::SHIFT_JIS.decode(&raw[..cut]).0.into_owned();
    let mut p = 0xE138 + TITLE_SIZE;
    s.extra.copy_from_slice(&blob[p..p + EXTRA_SIZE]);
    p += EXTRA_SIZE;
    s.bits = blob[p..p + BITS_BYTES].to_vec();
    p += BITS_BYTES;
    s.vars = (0..VARS_N).map(|i| crate::u32le(blob, p + i * 4) as i32).collect();
    Ok(s)
}

fn build_slot(s: &Slot) -> Vec<u8> {
    let mut out = Vec::with_capacity(SLOT_SIZE);
    for &v in &s.time {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut thumb = s.thumb.clone();
    thumb.resize(THUMB_SIZE, 0);
    out.extend_from_slice(&thumb);
    let (title, _, _) = encoding_rs::SHIFT_JIS.encode(&s.title);
    let mut t = vec![0u8; TITLE_SIZE];
    let n = title.len().min(TITLE_SIZE);
    t[..n].copy_from_slice(&title[..n]);
    out.extend_from_slice(&t);
    out.extend_from_slice(&s.extra);
    let mut bits = s.bits.clone();
    bits.resize(BITS_BYTES, 0);
    out.extend_from_slice(&bits);
    for i in 0..VARS_N {
        let v = s.vars.get(i).copied().unwrap_or(0);
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&vec![0u8; TAIL_SIZE]);
    out
}

pub fn build(sf: &SaveFile) -> Vec<u8> {
    let mut blocks: Vec<Vec<u8>> = sf.slots.iter().map(|s| if s.used() { build_slot(s) } else { Vec::new() }).collect();
    blocks.push(sf.system_block(27).to_vec());
    blocks.push(sf.system_block(28).to_vec());
    if sf.global_bits.is_empty() {
        blocks.push(Vec::new());
    } else {
        let mut b = (sf.global_bits.len() as u32).to_le_bytes().to_vec();
        b.extend_from_slice(&sf.global_bits);
        blocks.push(b);
    }
    pack_blocks(&blocks)
}

pub fn pack_blocks(blocks: &[Vec<u8>]) -> Vec<u8> {
    let mut off = Vec::with_capacity(TABLE_N);
    let mut body = Vec::new();
    let mut pos = TABLE_BYTES;
    for b in blocks {
        if b.is_empty() {
            off.push(0u32);
        } else {
            off.push(pos as u32);
            body.extend_from_slice(b);
            pos += b.len();
        }
    }
    while off.len() < TABLE_N {
        off.push(0);
    }
    let mut out = Vec::with_capacity(pos);
    for v in off {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&body);
    out
}

/// 读 30 项块偏移表。
pub fn table(data: &[u8]) -> [u32; TABLE_N] {
    let mut t = [0u32; TABLE_N];
    for (i, v) in t.iter_mut().enumerate() {
        *v = crate::u32le(data, i * 4);
    }
    t
}

/// 取第 `i` 块的原始字节；空块返回空 vec。
pub fn block(data: &[u8], i: usize) -> Vec<u8> {
    let t = table(data);
    let start = t[i] as usize;
    if start == 0 || start >= data.len() {
        return Vec::new();
    }
    let mut end = data.len();
    for j in i + 1..TABLE_N {
        if t[j] != 0 {
            end = (t[j] as usize).min(data.len());
            break;
        }
    }
    if end <= start {
        return Vec::new();
    }
    data[start..end].to_vec()
}

/// 块 27/28 的头 4 字节 = 容量（`0x48` 写入）。
///
/// 原版 `0x49`/`0x4A` 的 `start` 从**头部之后**算起：`FUN_00423F10` /
/// `FUN_00424CA0` 一进来就先读 4 字节，用它夹住 `start`/`count`，然后才写数据。
pub fn global_capacity(blk: &[u8]) -> usize {
    if blk.len() < 4 {
        0
    } else {
        (crate::u32le(blk, 0) & 0xFFFF) as usize
    }
}

/// `0x48` 新建块 27/28 时的字节布局（`FUN_00438A90`）。
///
/// 位数组：`4 + ceil((((n+7)>>3)+4)/4)*4`；变量数组：`4 + (n*4+4)`。
/// 512 位 → 72 字节，528 个变量 → 2120 字节，和真实存档完全一致。
pub fn new_global_block(n: usize, vars: bool) -> Vec<u8> {
    let payload = if vars { n * 4 + 4 } else { ((n + 7) >> 3) + 4 };
    let len = 4 + payload.div_ceil(4) * 4;
    let mut b = vec![0u8; len];
    b[0..4].copy_from_slice(&(n as u32).to_le_bytes());
    b
}

/// 引擎按字节覆盖，实际编解码的位数向上取整到 8 的倍数。
pub fn bit_total(nbits: usize) -> usize {
    ((nbits + 7) / 8) * 8
}

pub fn rle_decode(data: &[u8], nbits: usize) -> Vec<u8> {
    let nbits = bit_total(nbits);
    let mut out = vec![0u8; (nbits + 7) / 8];
    let (mut i, mut bit) = (0usize, 0usize);
    while i < data.len() && bit < nbits {
        let b0 = data[i];
        let cls = ((b0 & 0x60) >> 5) as usize;
        let mut n = (b0 & 0x1F) as usize;
        i += 1;
        for _ in 0..cls {
            if i >= data.len() {
                return out;
            }
            n = (n << 8) | data[i] as usize;
            i += 1;
        }
        let val = b0 & 0x80 != 0;
        for _ in 0..n {
            if bit >= nbits {
                break;
            }
            if val {
                out[bit >> 3] |= 1 << (bit & 7);
            }
            bit += 1;
        }
    }
    out
}

pub fn rle_encode(bits: &[u8], nbits: usize) -> Vec<u8> {
    let nbits = bit_total(nbits);
    let mut out = Vec::new();
    let get = |i: usize| -> u8 {
        if i >> 3 < bits.len() {
            (bits[i >> 3] >> (i & 7)) & 1
        } else {
            0
        }
    };
    let mut i = 0usize;
    while i < nbits {
        let v = get(i);
        let mut j = i;
        while j < nbits && get(j) == v {
            j += 1;
        }
        let mut n = j - i;
        while n > 0 {
            if n < 0x20 {
                out.push((v << 7) | n as u8);
                n = 0;
            } else if n < 0x2000 {
                out.push((v << 7) | 0x20 | (n >> 8) as u8);
                out.push((n & 0xFF) as u8);
                n = 0;
            } else if n < 0x200000 {
                out.push((v << 7) | 0x40 | (n >> 16) as u8);
                out.push(((n >> 8) & 0xFF) as u8);
                out.push((n & 0xFF) as u8);
                n = 0;
            } else {
                let chunk = n.min(0x1FFF_FFFF);
                out.push((v << 7) | 0x60 | (chunk >> 24) as u8);
                out.push(((chunk >> 16) & 0xFF) as u8);
                out.push(((chunk >> 8) & 0xFF) as u8);
                out.push((chunk & 0xFF) as u8);
                n -= chunk;
            }
        }
        i = j;
    }
    out
}

/// 把 `w*h*3` 的 RGB 缩到 160×120，包成 BITMAPINFOHEADER + BGR（顶朝下）。
pub fn thumb_from_rgb(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; THUMB_SIZE];
    let mut put = |o: usize, v: u32| out[o..o + 4].copy_from_slice(&v.to_le_bytes());
    put(0, THUMB_HDR as u32);
    put(4, THUMB_W as u32);
    put(8, (-(THUMB_H as i32)) as u32);
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&24u16.to_le_bytes());
    for y in 0..THUMB_H {
        let sy = (y * h / THUMB_H).min(h - 1);
        for x in 0..THUMB_W {
            let sx = (x * w / THUMB_W).min(w - 1);
            let s = (sy * w + sx) * 3;
            let d = THUMB_HDR + (y * THUMB_W + x) * 3;
            out[d] = rgb[s + 2];
            out[d + 1] = rgb[s + 1];
            out[d + 2] = rgb[s];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_size_is_fixed() {
        assert_eq!(SLOT_SIZE, 60181);
        assert_eq!(THUMB_SIZE, 0xE128);
    }

    #[test]
    fn empty_roundtrip() {
        let sf = SaveFile::new();
        let d = build(&sf);
        assert_eq!(d.len(), TABLE_BYTES);
        let back = parse(&d).unwrap();
        assert!(back.slots.iter().all(|s| !s.used()));
        assert_eq!(back.newest(), None);
    }

    #[test]
    fn slot_roundtrip() {
        let mut sf = SaveFile::new();
        let s = &mut sf.slots[4];
        s.time = [2026, 10, 4, 22, 21, 30, 15, 0];
        s.title = "第三章".into();
        s.vars[0] = 7;
        s.vars[541] = -12345;
        s.bits[0] = 0b1010_0101;
        let d = build(&sf);
        assert_eq!(d.len(), TABLE_BYTES + SLOT_SIZE);
        let back = parse(&d).unwrap();
        assert_eq!(back.slots[4].title, "第三章");
        assert_eq!(back.slots[4].vars[541], -12345);
        assert_eq!(back.slots[4].bits[0], 0b1010_0101);
        assert_eq!(back.newest(), Some(4));
    }

    #[test]
    fn global_block_layout_matches_real_save() {
        // 真实 KIZUNAR.SAV：块 27 = 72 字节（头 512），块 28 = 2120 字节（头 528）。
        let b = new_global_block(512, false);
        assert_eq!(b.len(), 72);
        assert_eq!(global_capacity(&b), 512);
        let v = new_global_block(528, true);
        assert_eq!(v.len(), 2120);
        assert_eq!(global_capacity(&v), 528);
    }

    #[test]
    fn rebuild_keeps_other_blocks() {
        let mut sf = SaveFile::new();
        sf.slots[2].time = [2026, 9, 0, 19, 21, 0, 0, 0];
        sf.global_bits = rle_encode(&[0u8; 3751], 30001);
        let d = build(&sf);
        let mut blocks: Vec<Vec<u8>> = (0..TABLE_N).map(|i| block(&d, i)).collect();
        assert!(blocks[27].is_empty() && blocks[28].is_empty());
        blocks[27] = new_global_block(512, false);
        blocks[28] = new_global_block(528, true);
        let out = pack_blocks(&blocks);
        let back = parse(&out).unwrap();
        assert_eq!(back.global_bits.len(), 3);
        assert_eq!(back.system_block(27).len(), 72);
        assert_eq!(back.system_block(28).len(), 2120);
        assert_eq!(back.slots[2].time[0], 2026);
    }

    #[test]
    fn rle_roundtrip() {
        for n in [0usize, 32, 800, 1600, 30001] {
            let bits = vec![0u8; (n + 7) / 8];
            let enc = rle_encode(&bits, n);
            assert_eq!(rle_decode(&enc, n), bits);
        }
        assert_eq!(rle_encode(&[0u8; 3751], 30001), vec![0x40, 0x75, 0x38]);
    }
}