//! Ikura GDL / GDT 图像编解码：GGD（24bpp）与 GG0/GG2（32bpp）。
//!
//! 两个解码器都是从 KIZUNAR.EXE 抄下来的，不是猜的：
//!
//! ```text
//! GGD    解码 FUN_0040b190，加载 FUN_0040aee0，分派 FUN_0040a7d0
//! GG0/2  解码 FUN_0040b8e0（标量）/ FUN_0040bc90（MMX），加载 FUN_0040b650
//! ```
//!
//! 操作码表见 `docs/02-formats.md` 第 3 节。

use crate::{u16le, u32le, Error, Result};

pub const MAGIC_A: [u8; 4] = [0xB9, 0xAA, 0xB3, 0xB3];
pub const MAGIC_B: &[u8; 8] = b"GGA00000";
pub const GGA_HEADER: usize = 0x30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// 24bpp，3 字节 B,G,R
    Ggd,
    /// 32bpp，4 字节 B,G,R,A
    Gga,
}

#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub width: usize,
    pub height: usize,
    pub payload: usize,
    pub family: Family,
}

pub fn parse_header(data: &[u8]) -> Result<Header> {
    if data.len() >= 8 && data[..4] == MAGIC_A {
        return Ok(Header {
            width: u16le(data, 4) as usize,
            height: u16le(data, 6) as usize,
            payload: 8,
            family: Family::Ggd,
        });
    }
    if data.len() >= GGA_HEADER && &data[..8] == MAGIC_B {
        let off = u32le(data, 0x10) as usize;
        if off != GGA_HEADER {
            return Err(Error(format!("GG0/GG2 载荷偏移异常: {:#x}", off)));
        }
        return Ok(Header {
            width: u16le(data, 8) as usize,
            height: u16le(data, 0x0A) as usize,
            payload: off,
            family: Family::Gga,
        });
    }
    Err(Error(format!("未知图像魔数: {:02x?}", &data[..data.len().min(8)])))
}

/// flags 的 bit0（字节 0x0F）表示带独立 alpha 平面。
pub fn gga_has_alpha(data: &[u8]) -> bool {
    data.len() > 0x0F && data[0x0F] & 1 != 0
}

pub struct Image {
    pub width: usize,
    pub height: usize,
    /// B,G,R 或 B,G,R,A
    pub pixels: Vec<u8>,
    pub channels: usize,
}

pub fn decode(data: &[u8]) -> Result<Image> {
    let h = parse_header(data)?;
    match h.family {
        Family::Ggd => decode_ggd(data),
        Family::Gga => decode_gga(data, None),
    }
}

fn dist(pos: usize, d: usize) -> Result<usize> {
    if d == 0 || d > pos {
        return Err(Error(format!("距离越界: d={} 在像素 {}", d, pos)));
    }
    Ok(pos - d)
}

pub fn decode_ggd(data: &[u8]) -> Result<Image> {
    let h = parse_header(data)?;
    let (w, ht) = (h.width, h.height);
    if w == 0 || ht == 0 {
        return Err(Error(format!("尺寸为 0: {}x{}", w, ht)));
    }
    let n = w * ht;
    let mut out: Vec<u8> = Vec::with_capacity(n * 3);
    let mut p = h.payload;
    while out.len() < n * 3 {
        if p >= data.len() {
            return Err(Error(format!("码流提前结束: {}/{} 像素", out.len() / 3, n)));
        }
        let b = data[p];
        p += 1;
        let x = out.len() / 3;
        match b {
            0 => {
                let k = *data.get(p).ok_or_else(|| Error("码流截断".into()))? as usize;
                p += 1;
                if x == 0 {
                    return Err(Error("首像素无法重复".into()));
                }
                let s = (x - 1) * 3;
                for _ in 0..k {
                    let px = [out[s], out[s + 1], out[s + 2]];
                    out.extend_from_slice(&px);
                }
            }
            1 | 2 => {
                let k = *data.get(p).ok_or_else(|| Error("码流截断".into()))? as usize;
                let d = if b == 1 {
                    let d = *data.get(p + 1).ok_or_else(|| Error("码流截断".into()))? as usize;
                    p += 2;
                    d
                } else {
                    let d = u16le(data, p + 1) as usize;
                    p += 3;
                    d
                };
                let mut s = dist(x, d)? * 3;
                for _ in 0..k {
                    let px = [out[s], out[s + 1], out[s + 2]];
                    out.extend_from_slice(&px);
                    s += 3;
                }
            }
            3 | 4 => {
                let (d, adv) = if b == 3 {
                    (data[p] as usize, 1)
                } else {
                    (u16le(data, p) as usize, 2)
                };
                p += adv;
                let s = dist(x, d)? * 3;
                let px = [out[s], out[s + 1], out[s + 2]];
                out.extend_from_slice(&px);
            }
            _ => {
                let k = (b - 4) as usize;
                let end = (p + 3 * k).min(data.len());
                out.extend_from_slice(&data[p..end]);
                p = end;
            }
        }
    }
    out.truncate(n * 3);
    if out.len() < n * 3 {
        // 码流跑完还差像素时补最后那个像素，保证正好 w*h
        if let Some(last) = out.len().checked_sub(3) {
            let px = [out[last], out[last + 1], out[last + 2]];
            while out.len() < n * 3 {
                out.extend_from_slice(&px);
            }
        } else {
            out.resize(n * 3, 0);
        }
    }
    Ok(Image { width: w, height: ht, pixels: out, channels: 3 })
}

/// 32bpp 复制：一次一个像素，所以重叠区间会自我复制（和 C 的循环一致）。
fn rep(out: &mut Vec<u8>, d: usize, k: usize) -> Result<()> {
    let pos = out.len() / 4;
    let mut s = dist(pos, d)? * 4;
    for _ in 0..k {
        let px = [out[s], out[s + 1], out[s + 2], out[s + 3]];
        out.extend_from_slice(&px);
        s += 4;
    }
    Ok(())
}

pub fn decode_gga(data: &[u8], width_override: Option<usize>) -> Result<Image> {
    let h = parse_header(data)?;
    let w = width_override.unwrap_or(h.width);
    let ht = h.height;
    if w == 0 || ht == 0 {
        return Err(Error(format!("尺寸为 0: {}x{}", w, ht)));
    }
    let src = &data[h.payload.min(data.len())..];
    let mut out: Vec<u8> = Vec::with_capacity(w * ht * 4);
    let mut p = 0usize;
    let get = |p: usize| -> Result<usize> {
        src.get(p).map(|&b| b as usize).ok_or_else(|| Error("码流截断".into()))
    };
    while p < src.len() {
        let op = src[p];
        p += 1;
        match op {
            0x00 => { rep(&mut out, 1, get(p)?)?; p += 1; }
            0x01 => { rep(&mut out, 1, get(p)? | (get(p + 1)? << 8))?; p += 2; }
            0x02 => { rep(&mut out, get(p)?, 1)?; p += 1; }
            0x03 => { rep(&mut out, get(p)? | (get(p + 1)? << 8), 1)?; p += 2; }
            0x04 => { rep(&mut out, get(p)?, get(p + 1)?)?; p += 2; }
            0x05 => { rep(&mut out, get(p)?, get(p + 1)? | (get(p + 2)? << 8))?; p += 3; }
            0x06 => { rep(&mut out, get(p)? | (get(p + 1)? << 8), get(p + 2)?)?; p += 3; }
            0x07 => {
                rep(&mut out, get(p)? | (get(p + 1)? << 8), get(p + 2)? | (get(p + 3)? << 8))?;
                p += 4;
            }
            0x08 => rep(&mut out, 1, 1)?,
            0x09 => rep(&mut out, w, 1)?,
            0x0A => rep(&mut out, w + 1, 1)?,
            0x0B => rep(&mut out, w.saturating_sub(1), 1)?,
            _ => {
                let k = (op - 0x0B) as usize;
                let end = (p + 4 * k).min(src.len());
                out.extend_from_slice(&src[p..end]);
                p = end;
            }
        }
    }
    // 解码器不检查 w*h：791/792 正好落在 w*h，TITLE.GG0 少一个像素，这里补齐。
    let end = w * ht * 4;
    if out.len() < end {
        if let Some(last) = out.len().checked_sub(4) {
            let px = [out[last], out[last + 1], out[last + 2], out[last + 3]];
            while out.len() < end {
                out.extend_from_slice(&px);
            }
        } else {
            out.resize(end, 0);
        }
    }
    out.truncate(end);
    Ok(Image { width: w, height: ht, pixels: out, channels: 4 })
}

/// 把 B,G,R[,A] 写成 PNG（PNG 要 R,G,B[,A]）。
pub fn write_png(path: &str, img: &Image) -> Result<()> {
    let ch = img.channels;
    // png crate 自己会加行首的 filter 字节，这里只给纯像素
    let mut raw = Vec::with_capacity(img.width * img.height * ch);
    for y in 0..img.height {
        let row = &img.pixels[y * img.width * ch..(y + 1) * img.width * ch];
        for x in 0..img.width {
            let o = x * ch;
            raw.extend_from_slice(&[row[o + 2], row[o + 1], row[o]]);
            if ch == 4 {
                raw.push(row[o + 3]);
            }
        }
    }
    let file = std::fs::File::create(path)?;
    let w = std::io::BufWriter::new(file);
    let mut enc = png::Encoder::new(w, img.width as u32, img.height as u32);
    enc.set_color(if ch == 4 { png::ColorType::Rgba } else { png::ColorType::Rgb });
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| Error(e.to_string()))?;
    writer.write_image_data(&raw).map_err(|e| Error(e.to_string()))?;
    Ok(())
}
