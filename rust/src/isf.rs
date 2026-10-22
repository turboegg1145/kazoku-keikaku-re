//! ISF 脚本：两层解密 + 字节码切分 + 虚拟机。
//!
//! 解密分两层（顺序不能反）：
//!
//! 1. 外层 "SECRETFILTER100a" 异或壳，密钥从 EXE 里 2048 字节的 `UOB0` 表实时算出来；
//! 2. 内层 `ikuar_decrypt`，按 `data[4:6]` 的魔数逐字节变换。
//!
//! 解密后的布局：
//!
//! ```text
//! u32 head_len            = 8 + 4 * 标签数
//! u32 magic (0x9795)
//! u32[n] 标签表            相对代码基址的偏移
//! 字节码：opcode(u8) + len(u8) [+ ext]，len 含头部自身
//! ```
//!
//! 指令语义见 `docs/04-isf-vm.md`。

use crate::{u16le, u32le, Error, Result};

const HEX_ENCODE_MAP: &[u8; 36] = b"G5FXIL094MPRKWCJ3OEBVA7HQ2SU8Y6TZ1ND";
const HEX_TABLE: [u8; 36] = [
    0x06, 0x21, 0x19, 0x10, 0x08, 0x01, 0x1E, 0x16, 0x1C, 0x07, 0x15, 0x13,
    0x0E, 0x23, 0x12, 0x02, 0x00, 0x17, 0x04, 0x0F, 0x0C, 0x05, 0x09, 0x22,
    0x11, 0x0A, 0x18, 0x0B, 0x1A, 0x1F, 0x1B, 0x14, 0x0D, 0x03, 0x1D, 0x20,
];

fn chr2hex(c: u8) -> usize {
    match c {
        b'0'..=b'9' => (c - b'0') as usize,
        b'a'..=b'z' => (c - b'a') as usize + 10,
        b'A'..=b'Z' => (c - b'A') as usize + 10,
        _ => 0,
    }
}

fn chr2hexcode(c: u8) -> u8 {
    HEX_TABLE[chr2hex(c) % 36]
}

/// 对应 Python 的 encode_hex：先把 >127 的值折成负数，再对 36 取模。
/// 注意 Python 的 % 是向下取整，所以这里必须用 rem_euclid。
fn encode_hex(byte: u8) -> u8 {
    let sym = if byte > 127 { byte as i32 - 256 } else { byte as i32 };
    HEX_ENCODE_MAP[sym.rem_euclid(36) as usize]
}

fn str2hex(s: &[u8]) -> usize {
    let mut v = 0usize;
    for (i, &c) in s.iter().enumerate() {
        v |= chr2hex(c) << ((s.len() - i - 1) << 2);
    }
    v
}

pub fn create_key(secret: &[u8]) -> Vec<u8> {
    let mut len = [0u8; 2];
    for i in 0..2 {
        let d = chr2hexcode(secret[0x500 + i]).wrapping_sub(chr2hexcode(secret[0x100 + i]));
        len[i] = encode_hex(d);
    }
    let klen = str2hex(&len);
    let mut key = vec![0u8; klen];
    for i in 0..klen {
        let d = chr2hexcode(secret[0x510 + i]).wrapping_sub(chr2hexcode(secret[0x110 + i]));
        key[i] = encode_hex(d);
    }
    key
}

fn update_key(secret: &[u8], key: &mut [u8], index: usize) {
    let p = (index & 0x3F) * 0x10;
    for i in 0..key.len() {
        let d = chr2hexcode(key[i]).wrapping_add(chr2hexcode(secret[p + i]));
        key[i] = encode_hex(d);
    }
}

pub fn handle_isf_xor(data: &[u8], secret: &[u8]) -> Vec<u8> {
    let mut key = create_key(secret);
    let klen = key.len();
    let mut out = data.to_vec();
    for i in 0..out.len() {
        if i % klen == 0 {
            update_key(secret, &mut key, i / klen);
        }
        out[i] ^= key[i % klen];
    }
    out
}

pub fn ikuar_decrypt(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    if out.len() < 8 {
        return out;
    }
    let ver = u16le(&out, 4);
    match ver {
        0x9795 => {
            for b in out.iter_mut().skip(8) {
                *b = (*b >> 2) | (*b << 6);
            }
        }
        0xD197 => {
            for b in out.iter_mut().skip(8) {
                *b = !*b;
            }
        }
        0xCE89 => {
            let k = out[6];
            for b in out.iter_mut().skip(8) {
                *b ^= k;
            }
        }
        _ => {}
    }
    out
}

/// 从 EXE 里找 2048 字节的 `UOB0` 密钥表。
pub fn find_secret(exe: &[u8]) -> Result<&[u8]> {
    let pos = exe
        .windows(4)
        .position(|w| w == b"UOB0")
        .ok_or_else(|| Error("EXE 里找不到 UOB0 密钥表".into()))?;
    if pos + 2048 > exe.len() {
        return Err(Error("UOB0 表被截断".into()));
    }
    Ok(&exe[pos..pos + 2048])
}

pub struct Script {
    /// 解密后的完整明文（含 8 字节头 + 标签表 + 字节码）
    pub plain: Vec<u8>,
    pub head_len: usize,
    /// 标签表，值相对代码基址
    pub labels: Vec<u32>,
}

impl Script {
    pub fn parse(raw: &[u8], secret: &[u8]) -> Result<Script> {
        if raw.len() <= 16 {
            return Err(Error("ISF 太短".into()));
        }
        let plain = ikuar_decrypt(&handle_isf_xor(&raw[..raw.len() - 16], secret));
        if plain.len() < 8 {
            return Err(Error("ISF 头部被截断".into()));
        }
        let head_len = u32le(&plain, 0) as usize;
        if head_len < 8 || head_len > plain.len() || (head_len - 8) % 4 != 0 {
            return Err(Error(format!("head_len 异常: {:#x}", head_len)));
        }
        let n = (head_len - 8) / 4;
        let labels = (0..n).map(|i| u32le(&plain, 8 + i * 4)).collect();
        Ok(Script { plain, head_len, labels })
    }
}

/// 一条指令的切分结果。
#[derive(Debug, Clone, Copy)]
pub struct Op {
    pub offset: usize,
    pub code: u8,
    pub len: usize,
}

/// 把字节码流切成指令；`end` 是流末尾。
pub fn split_ops(plain: &[u8], start: usize) -> Result<Vec<Op>> {
    let mut out = Vec::new();
    let mut p = start;
    while p < plain.len() {
        if p + 2 > plain.len() {
            return Err(Error(format!("指令头截断 @{:#x}", p)));
        }
        let code = plain[p];
        let mut len = plain[p + 1] as usize;
        let mut head = 2;
        if len & 0x80 != 0 {
            if p + 3 > plain.len() {
                return Err(Error(format!("指令头截断 @{:#x}", p)));
            }
            len = (((len << 8) | plain[p + 2] as usize) & 0x7FFF) as usize;
            head = 3;
        }
        if len < head || p + len > plain.len() {
            return Err(Error(format!("指令长度异常 @{:#x}: {}", p, len)));
        }
        out.push(Op { offset: p, code, len });
        p += len;
    }
    Ok(out)
}

pub const MAX_VARS: usize = 0x10000;
pub const MAX_RET: usize = 0x400;

/// 虚拟机需要宿主提供的东西：输入轮询和计时器。
///
/// 真引擎读的是 `GetAsyncKeyState` / `GetCursorPos` / `timeGetTime`；
/// 移植时这一层换成 UIKit / SDL / 触摸事件即可，VM 本身不用改。
pub trait Host {
    /// `0x85`：返回 (热区号, 事件)。热区 -1 = 没指到，-2 = 画面外；事件 0/2/-1。
    fn poll_input(&mut self, _mode: u8, _flags: u8) -> (i32, i32) {
        (-1, 0)
    }

    /// `0xFC`：毫秒计时器。
    fn now_ms(&mut self) -> i32 {
        0
    }

    /// `0xFB`：计时器清零。
    fn reset_timer(&mut self) {}
}

/// 无人值守宿主：不点任何东西，计时器每问一次走 16ms（相当于 60fps 一帧）。
#[derive(Default)]
pub struct Headless {
    clock: i32,
}

impl Host for Headless {
    fn now_ms(&mut self) -> i32 {
        self.clock += 16;
        self.clock
    }

    fn reset_timer(&mut self) {
        self.clock = 0;
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Text { speaker: Option<u8>, text: String, voice: Option<String> },
    Voice(String),
    Image { index: u32, name: String },
    CmdName(String),
    VoiceArchive(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Halt {
    /// `0x03`：脚本正常结束
    End,
    /// 顶层 `0x06`：返回栈空，也等于正常结束
    ReturnUnderflow,
    StepLimit,
    ReturnOverflow,
    BadLength { at: usize, len: usize },
    OutOfRange { at: usize },
    BadLabel(u32),
}

impl std::fmt::Display for Halt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Halt::End => write!(f, "END (0x03)"),
            Halt::ReturnUnderflow => write!(f, "return stack underflow"),
            Halt::StepLimit => write!(f, "step limit"),
            Halt::ReturnOverflow => write!(f, "return stack overflow"),
            Halt::BadLength { at, len } => write!(f, "bad length {} at {:#x}", len, at),
            Halt::OutOfRange { at } => write!(f, "code pointer {:#x} out of range", at),
            Halt::BadLabel(v) => write!(f, "label {} out of range", v),
        }
    }
}

/// 把 `0x2B` 的载荷拆成 (说话人, 文本)。
pub fn decode_text(payload: &[u8]) -> (Option<u8>, String) {
    if payload.len() < 4 || payload[0] != 0 || payload[1] != 3 {
        return (None, String::new());
    }
    let mut p = 2;
    let mut speaker = None;
    if payload[p] == 0xFF {
        p += 1;
    } else if payload[p] == 0x04 && p + 2 <= payload.len() {
        speaker = Some(payload[p + 1]);
        p += 2;
        if p < payload.len() && payload[p] == 0xFF {
            p += 1;
        }
    } else {
        p += 1;
    }
    (speaker, clean_text(&payload[p..]))
}

/// 0x00 0x06 = 句中换行；其余 0x00 = 结束/填充；其它控制码丢掉。
fn clean_text(raw: &[u8]) -> String {
    let mut parts = String::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        if b == 0x00 {
            let nxt = raw.get(i + 1).copied().unwrap_or(0);
            if nxt == 0x06 {
                parts.push_str(&drop_junk(&decode_cp932(&buf)));
                buf.clear();
                parts.push('\n');
                i += 2;
                continue;
            }
            break;
        }
        // 控制码（0x02 等）直接丢弃；cp932 的续字节不会落在这个区间
        if b < 0x20 {
            i += 1;
            continue;
        }
        buf.push(b);
        i += 1;
    }
    parts.push_str(&drop_junk(&decode_cp932(&buf)));
    parts
}

/// 去掉解码后残留的垃圾字符：cp932 的未定义单字节（0x80/0xA0/0xFD..0xFF）在 Python 里
/// 会变成私用区 U+F8F0..U+F8F3，在 encoding_rs 里会变成 U+FFFD，两边都当填充丢掉。
/// 必须在**解码之后**按字符过滤——这些字节也可能是合法双字节的续字节（如 あ = 82 A0）。
fn drop_junk(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            let v = c as u32;
            !(v < 0x20 || v == 0x7F || (0xE000..=0xF8FF).contains(&v) || v == 0xFFFD)
        })
        .collect()
}

pub fn decode_cp932(raw: &[u8]) -> String {
    if raw.iter().all(|&b| b < 0x80) {
        return raw.iter().map(|&b| b as char).collect();
    }
    let (s, _, _) = encoding_rs::SHIFT_JIS.decode(raw);
    s.into_owned()
}

pub struct Vm<'a> {
    plain: &'a [u8],
    base: usize,
    labels: &'a [u32],
    pub code: usize,
    pub vars: Vec<i32>,
    ret: Vec<usize>,
    pub events: Vec<Event>,
    pub steps: u64,
    pub limit: u64,
    pub unknown: std::collections::BTreeMap<u8, u64>,
    pub flags: std::collections::BTreeMap<i32, bool>,
    pub host: Box<dyn Host>,
    voice: Option<String>,
    rng: u32,
    /// 位数组（引擎 vm+0x554）：0x31 写、0x33/0x3B 读。
    pub bits: Vec<u8>,
}

impl<'a> Vm<'a> {
    pub fn new(script: &'a Script, limit: u64) -> Vm<'a> {
        Vm {
            plain: &script.plain,
            base: script.head_len,
            labels: &script.labels,
            code: script.head_len,
            vars: vec![0; MAX_VARS],
            ret: Vec::new(),
            events: Vec::new(),
            steps: 0,
            limit,
            unknown: Default::default(),
            flags: Default::default(),
            host: Box::new(Headless::default()),
            voice: None,
            rng: 0x1234_5678,
            bits: vec![0; 0x10000],
        }
    }

    pub fn texts(&self) -> impl Iterator<Item = (&Option<u8>, &String, &Option<String>)> {
        self.events.iter().filter_map(|e| match e {
            Event::Text { speaker, text, voice } => Some((speaker, text, voice)),
            _ => None,
        })
    }

    fn take_u8(&mut self) -> u8 {
        let v = self.plain[self.code];
        self.code += 1;
        v
    }

    fn take_u16(&mut self) -> u16 {
        let v = u16le(self.plain, self.code);
        self.code += 2;
        v
    }

    fn take_u32(&mut self) -> u32 {
        let v = u32le(self.plain, self.code);
        self.code += 4;
        v
    }

    fn rand(&mut self) -> u32 {
        // xorshift32，够用了
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    /// 操作数解码（引擎 FUN_00424b20）：bit31 间接、bit30 随机、bit29 符号扩展。
    fn operand(&mut self, raw: u32) -> i32 {
        let mut v = (raw & 0x3FFF_FFFF) as i64;
        if raw & 0x2000_0000 != 0 {
            v -= 0x4000_0000;
        }
        if raw & 0x8000_0000 != 0 {
            v = if (0..MAX_VARS as i64).contains(&v) { self.vars[v as usize] as i64 } else { 0 };
        }
        if raw & 0x4000_0000 != 0 && v != 0 {
            let q = v.abs();
            let r = (self.rand() as i64) % q;
            v = if v > 0 { r } else { -r };
        }
        v as i32
    }

    fn val(&mut self) -> i32 {
        let raw = self.take_u32();
        self.operand(raw)
    }

    fn set_var(&mut self, idx: u16, v: i32) {
        let i = idx as usize;
        if i < MAX_VARS {
            self.vars[i] = v;
        }
    }

    fn jump_label(&mut self, idx: u16) -> Result<()> {
        let i = idx as usize;
        if i >= self.labels.len() {
            return Err(Error(format!("label {} out of range", idx)));
        }
        self.code = self.base + self.labels[i] as usize;
        Ok(())
    }

    pub fn run(&mut self) -> Result<Halt> {
        loop {
            if self.steps >= self.limit {
                return Ok(Halt::StepLimit);
            }
            if self.code < self.base || self.code >= self.plain.len() {
                return Ok(Halt::OutOfRange { at: self.code });
            }
            let start = self.code;
            let op = self.take_u8();
            let mut len = self.take_u8() as usize;
            let mut head = 2usize;
            if len & 0x80 != 0 {
                len = (((len << 8) | self.take_u8() as usize) & 0x7FFF) as usize;
                head = 3;
            }
            let end = start + len;
            if len < head || end > self.plain.len() {
                return Ok(Halt::BadLength { at: start, len });
            }
            self.steps += 1;
            match self.exec(op, end) {
                Ok(true) => {}
                Ok(false) => self.code = end,
                Err(Halt::End) => return Ok(Halt::End),
                Err(Halt::ReturnUnderflow) => return Ok(Halt::ReturnUnderflow),
                Err(other) => return Ok(other),
            }
        }
    }

    fn exec(&mut self, op: u8, end: usize) -> std::result::Result<bool, Halt> {
        match op {
            0x00 => Ok(false),
            0x03 => Err(Halt::End),
            0x04 => {
                let l = self.take_u16();
                self.jump_label(l).map_err(|_| Halt::BadLabel(l as u32))?;
                Ok(true)
            }
            0x05 => {
                let l = self.take_u16();
                if self.ret.len() >= MAX_RET {
                    return Err(Halt::ReturnOverflow);
                }
                self.ret.push(self.code - self.base);
                self.jump_label(l).map_err(|_| Halt::BadLabel(l as u32))?;
                Ok(true)
            }
            0x06 => {
                let a = self.ret.pop().ok_or(Halt::ReturnUnderflow)?;
                self.code = self.base + a;
                Ok(true)
            }
            0x07 | 0x08 => {
                let v = self.val();
                let n = self.take_u8();
                if v >= 0 && v < n as i32 {
                    self.code += v as usize * 2;
                    let l = self.take_u16();
                    if op == 0x08 {
                        if self.ret.len() >= MAX_RET {
                            return Err(Halt::ReturnOverflow);
                        }
                        // 返回地址是「本指令末尾」
                        self.ret.push(end - self.base);
                    }
                    self.jump_label(l).map_err(|_| Halt::BadLabel(l as u32))?;
                    return Ok(true);
                }
                Ok(false)
            }
            0x2B => {
                let (spk, txt) = decode_text(&self.plain[self.code..end]);
                self.events.push(Event::Text { speaker: spk, text: txt, voice: self.voice.take() });
                Ok(false)
            }
            0x31 => {
                // FUN_00423ac0：位数组第 bit 位，mode 0 清 / 1 置 / 2 翻转
                let bit = self.take_u16() as usize;
                let mode = self.take_u8();
                if bit < self.bits.len() {
                    match mode {
                        0 => self.bits[bit] = 0,
                        1 => self.bits[bit] = 1,
                        2 => self.bits[bit] ^= 1,
                        _ => {}
                    }
                }
                Ok(false)
            }
            0x33 | 0x3B => {
                // FUN_00437750：if (bit != param) jump；0x33 传 0、0x3B 传 1
                // 即 0x33 = 位为 1 时跳，0x3B = 位为 0 时跳
                let bit = self.take_u16() as usize;
                let l = self.take_u16();
                let val = if bit < self.bits.len() { self.bits[bit] } else { 0 };
                if val == u8::from(op != 0x3B) {
                    self.jump_label(l).map_err(|_| Halt::BadLabel(l as u32))?;
                    return Ok(true);
                }
                Ok(false)
            }
            0x3A => {
                let sid = self.take_u8();
                let arg = self.take_u8();
                let l = self.take_u16();
                if self.sys_test(sid, arg) {
                    self.jump_label(l).map_err(|_| Halt::BadLabel(l as u32))?;
                    return Ok(true);
                }
                Ok(false)
            }
            0x44 => {
                let dst = self.take_u16();
                let mut acc: i32 = 0;
                let mut term: i32 = 0;
                loop {
                    let o = self.plain[self.code];
                    if o > 4 {
                        break;
                    }
                    self.code += 1;
                    let v = self.val();
                    let nxt = self.plain.get(self.code).copied().unwrap_or(0) & 0x0F;
                    if nxt > 1 {
                        match o {
                            1 => term = v.wrapping_neg(),
                            2 => term = term.wrapping_mul(v),
                            3 => term = cdiv(term, v),
                            4 => term = cmod(term, v),
                            _ => term = v,
                        }
                    } else {
                        match o {
                            0 => acc = acc.wrapping_add(v),
                            1 => acc = acc.wrapping_sub(v),
                            2 => { acc = acc.wrapping_add(v.wrapping_mul(term)); term = 0; }
                            3 => { acc = acc.wrapping_add(cdiv(term, v)); term = 0; }
                            4 => { acc = acc.wrapping_add(cmod(term, v)); term = 0; }
                            _ => {}
                        }
                    }
                }
                self.set_var(dst, acc.wrapping_add(term));
                Ok(false)
            }
            0x47 => {
                loop {
                    let a = self.val();
                    let c = self.take_u8();
                    let b = self.val();
                    let ok = match c {
                        0 => a == b,
                        1 => a < b,
                        2 => a <= b,
                        3 => a > b,
                        4 => a >= b,
                        5 => a != b,
                        _ => false,
                    };
                    if !ok {
                        return Ok(false);
                    }
                    let act = self.take_u8();
                    match act {
                        0 => {
                            let l = self.take_u16();
                            self.jump_label(l).map_err(|_| Halt::BadLabel(l as u32))?;
                            return Ok(true);
                        }
                        1 => {
                            let slot = self.take_u16();
                            let v = self.val();
                            self.set_var(slot, v);
                            return Ok(false);
                        }
                        0xFF => return Ok(false),
                        _ => return Ok(false),
                    }
                }
            }
            0x78 => {
                let s = ascii_of(&self.plain[self.code..end]);
                self.voice = Some(s.clone());
                self.events.push(Event::Voice(s));
                Ok(false)
            }
            0x85 => {
                let va = self.take_u16();
                let vb = self.take_u16();
                let mode = self.take_u8();
                let flags = self.take_u8();
                if flags & 2 == 0 {
                    let (spot, ev) = self.host.poll_input(mode, flags & 1);
                    self.set_var(va, spot);
                    self.set_var(vb, ev);
                }
                Ok(false)
            }
            0xF4 => {
                let v = self.take_u8();
                self.flags.insert(0x7FD, v != 0);
                Ok(false)
            }
            0xF5 => {
                let v = self.take_u8();
                self.flags.insert(0x83D, v != 0);
                if v == 1 {
                    self.flags.insert(0x83C, true);
                }
                Ok(false)
            }
            0xFB => {
                // 计时器清零：真引擎把 vm+0x184 置 0
                self.host.reset_timer();
                Ok(false)
            }
            0xFC => {
                let idx = self.take_u16();
                let v = self.host.now_ms();
                self.set_var(idx, v);
                Ok(false)
            }
            0x02 | 0x56 | 0xF0 => {
                let s = ascii_of(&self.plain[self.code..end]);
                match op {
                    0x02 => self.events.push(Event::CmdName(s)),
                    0x56 => {
                        let index = u32le(self.plain, self.code);
                        let name = ascii_of(&self.plain[self.code + 4..end]);
                        self.events.push(Event::Image { index, name });
                    }
                    _ => self.events.push(Event::VoiceArchive(s)),
                }
                Ok(false)
            }
            _ => {
                *self.unknown.entry(op).or_insert(0) += 1;
                Ok(false)
            }
        }
    }

    /// `0x3A`：系统条件。没模拟的一律 false。
    fn sys_test(&mut self, sid: u8, arg: u8) -> bool {
        match sid {
            0x0E => {
                // 输入状态位（+0x54D | 2）。无人值守时当作「已点击」。
                3 == arg
            }
            0x0A => arg != 0,
            _ => false,
        }
    }
}

fn cdiv(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    a.wrapping_div(b)
}

fn cmod(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    a.wrapping_rem(b)
}

fn ascii_of(p: &[u8]) -> String {
    p.iter()
        .filter(|&&b| (32..127).contains(&b))
        .map(|&b| b as char)
        .collect::<String>()
        .trim_matches('\0')
        .to_string()
}
