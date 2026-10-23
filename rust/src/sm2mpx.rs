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
//!
//! 打开时只读目录（几十 KB），成员按需 seek 读取。
//! 之前是整份读进内存，VOICE1+VOICE2 一共 400MB，在机械盘上要等 40 秒。

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

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

enum Backing {
    /// 整份在内存里（测试、或本来就小的归档）。
    Mem(Vec<u8>),
    /// 按需读取。RefCell 只为了在 &self 上做 seek+read。
    File(RefCell<File>),
}

pub struct Archive {
    backing: Backing,
    pub entries: Vec<Entry>,
    /// 归档文件长度（懒加载时用来做越界检查）。
    len: usize,
}

impl Archive {
    /// 打开归档：只读目录，不读数据区。
    pub fn open(path: &str) -> Result<Archive> {
        let mut f = File::open(path).map_err(|e| Error(format!("{}: {}", path, e)))?;
        let len = f.metadata().map(|m| m.len() as usize).unwrap_or(0);
        let mut head = [0u8; HEADER_SIZE];
        f.read_exact(&mut head)
            .map_err(|e| Error(format!("{}: 读文件头失败: {}", path, e)))?;
        if &head[..8] != MAGIC {
            return Err(Error(format!("{}: 不是 SM2MPX10 归档", path)));
        }
        let count = u32le(&head, 8) as usize;
        if HEADER_SIZE + count * ENTRY_SIZE > len {
            return Err(Error(format!("{}: 目录项数 {} 超出文件长度", path, count)));
        }
        let mut dir = vec![0u8; count * ENTRY_SIZE];
        f.read_exact(&mut dir)
            .map_err(|e| Error(format!("{}: 读目录失败: {}", path, e)))?;
        let entries = parse_entries(&dir, count, len)?;
        Ok(Archive { backing: Backing::File(RefCell::new(f)), entries, len })
    }

    /// 从内存解析（测试用）。
    pub fn parse(data: Vec<u8>) -> Result<Archive> {
        if data.len() < HEADER_SIZE || &data[..8] != MAGIC {
            return Err(Error("不是 SM2MPX10 归档".into()));
        }
        let count = u32le(&data, 8) as usize;
        if HEADER_SIZE + count * ENTRY_SIZE > data.len() {
            return Err(Error(format!("目录项数 {} 超出文件长度", count)));
        }
        let len = data.len();
        let entries = parse_entries(&data[HEADER_SIZE..HEADER_SIZE + count * ENTRY_SIZE], count, len)?;
        Ok(Archive { backing: Backing::Mem(data), entries, len })
    }

    /// 取成员内容。
    pub fn bytes(&self, e: &Entry) -> Result<Vec<u8>> {
        match &self.backing {
            Backing::Mem(d) => Ok(d[e.offset..e.offset + e.size].to_vec()),
            Backing::File(f) => {
                let mut f = f.borrow_mut();
                f.seek(SeekFrom::Start(e.offset as u64))?;
                let mut buf = vec![0u8; e.size];
                f.read_exact(&mut buf)?;
                Ok(buf)
            }
        }
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

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn parse_entries(dir: &[u8], count: usize, file_len: usize) -> Result<Vec<Entry>> {
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let base = i * ENTRY_SIZE;
        let raw = &dir[base..base + 12];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(12);
        let name = decode_cp932_name(&raw[..end]);
        let offset = u32le(dir, base + 12) as usize;
        let size = u32le(dir, base + 16) as usize;
        if offset + size > file_len {
            return Err(Error(format!("目录项越界: {} @{:#x}+{:#x}", name, offset, size)));
        }
        entries.push(Entry { name, offset, size });
    }
    Ok(entries)
}

/// 目录里的名字是 cp932；本作全是 ASCII，非 ASCII 字节退化成 U+FFFD。
fn decode_cp932_name(raw: &[u8]) -> String {
    if raw.iter().all(|&b| b < 0x80) {
        return raw.iter().map(|&b| b as char).collect();
    }
    let (s, _, _) = encoding_rs::SHIFT_JIS.decode(raw);
    s.into_owned()
}