//! SM2MPX10 归档（GDT 引擎通用容器）。
//!
//! ```text
//! 0x00  8   魔数 "SM2MPX10"
//! 0x08  u32 条目数
//! 0x0C  u32 目录区字节数（用不到）
//! 0x10  12  归档内部名
//! 0x1C  u32 常量 0x20
//! 0x20  ... 目录项，每项 20 字节：
//!           0x00 12  文件名（cp932，NUL 填充）
//!           0x0C u32 数据偏移
//!           0x10 u32 数据长度
//! ```

use crate::{u32le, Error, Result};

pub const MAGIC: &[u8; 8] = b"SM2MPX10";
const HEADER_SIZE: usize = 0x20;
const ENTRY_SIZE: usize = 20;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub offset: usize,
    pub size: usize,
}

pub struct Archive {
    data: Vec<u8>,
    pub entries: Vec<Entry>,
}

impl Archive {
    pub fn open(path: &str) -> Result<Archive> {
        Archive::parse(std::fs::read(path)?)
    }

    pub fn parse(data: Vec<u8>) -> Result<Archive> {
        if data.len() < HEADER_SIZE || &data[..8] != MAGIC {
            return Err(Error("不是 SM2MPX10 归档".into()));
        }
        let count = u32le(&data, 8) as usize;
        if HEADER_SIZE + count * ENTRY_SIZE > data.len() {
            return Err(Error(format!("目录项数 {} 超出文件长度", count)));
        }
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let base = HEADER_SIZE + i * ENTRY_SIZE;
            let raw = &data[base..base + 12];
            let end = raw.iter().position(|&b| b == 0).unwrap_or(12);
            let name = decode_cp932_name(&raw[..end]);
            let offset = u32le(&data, base + 12) as usize;
            let size = u32le(&data, base + 16) as usize;
            if offset + size > data.len() {
                return Err(Error(format!("目录项越界: {} @{:#x}+{:#x}", name, offset, size)));
            }
            entries.push(Entry { name, offset, size });
        }
        Ok(Archive { data, entries })
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        let e = self.find(name)?;
        Some(&self.data[e.offset..e.offset + e.size])
    }

    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// 按「去掉扩展名」的名字查，GGD/GG0/GG2 同名的情况由调用方挑。
    pub fn find_stem(&self, stem: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| {
            let s = e.name.rsplit_once('.').map(|(a, _)| a).unwrap_or(&e.name);
            s.eq_ignore_ascii_case(stem)
        })
    }

    pub fn bytes(&self, e: &Entry) -> &[u8] {
        &self.data[e.offset..e.offset + e.size]
    }
}

/// 目录里的名字是 cp932；本作全是 ASCII，非 ASCII 字节退化成 U+FFFD。
fn decode_cp932_name(raw: &[u8]) -> String {
    if raw.iter().all(|&b| b < 0x80) {
        return raw.iter().map(|&b| b as char).collect();
    }
    let (s, _, _) = encoding_rs::SHIFT_JIS.decode(raw);
    s.into_owned()
}
