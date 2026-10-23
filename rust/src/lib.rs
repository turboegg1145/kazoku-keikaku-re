//! GDT / Ikura GDL 引擎的数据层与脚本虚拟机。
//!
//! 三个模块对应 `docs/02-formats.md` 和 `docs/04-isf-vm.md`：
//!
//! - `sm2mpx`：SM2MPX10 归档容器
//! - `ggd`：GGD（24bpp）/ GG0、GG2（32bpp）图像解码
//! - `isf`：ISF 脚本解密 + 虚拟机

pub mod audio;
pub mod engine;
pub mod ggd;
pub mod render;
pub mod isf;
pub mod sav;
pub mod sm2mpx;

/// 一个错误类型就够了：这些解析器要么成功，要么带上一条人话说明。
#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Error(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// 小工具：从字节流里按小端取数。
pub fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
