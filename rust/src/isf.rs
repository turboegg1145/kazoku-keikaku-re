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

#[derive(Clone)]
pub struct Script {
    /// 解密后的完整明文（含 8 字节头 + 标签表 + 字节码）
    pub plain: std::rc::Rc<Vec<u8>>,
    pub head_len: usize,
    /// 标签表，值相对代码基址
    pub labels: std::rc::Rc<Vec<u32>>,
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
        let labels: Vec<u32> = (0..n).map(|i| u32le(&plain, 8 + i * 4)).collect();
        Ok(Script {
            plain: std::rc::Rc::new(plain),
            head_len,
            labels: std::rc::Rc::new(labels),
        })
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

    /// `0xA5`：取存档块 29 的 RLE 数据（已读文本位图）。没有存档就返回 None。
    fn load_seen(&mut self) -> Option<Vec<u8>> {
        None
    }

    /// `0xA6`：文字窗口参数。
    fn set_text_rect(&mut self, _a: u8, _b: u16, _c: u16) {}
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
    /// `0x01`：跳到另一个剧本（场景切换，不返回）。
    JumpScript(String),
    /// `0x70 <名字> <u8 mode>`：播放 BGM（WMSC 归档里的 OGG）。
    /// mode 0 = 循环，1 = 单次，2 = 循环且从当前位置继续。
    Bgm { name: String, mode: u8 },
    /// `0x71 <u8 loop> <u32 音量>`：调整当前 BGM。
    BgmVolume { looping: bool, volume: u32 },
    /// `0x72 <u32 音量>`：0 表示停掉 BGM。
    BgmLevel(u32),
    /// `0x15 <u8 窗口> <u8 序号> <x> <y> <w> <h> <cp932 文本>`：登记一个选项。
    /// 目标函数 FUN_00418560 把文本拷进窗口对象的 32 项表（每项 0x20 字节）。
    Choice { window: u8, index: u8, x: i32, y: i32, w: i32, h: i32, text: String },
    /// `0x16 <u8 窗口> <选项数> <u8 标志>`：打开选项窗口（FUN_004182A0）。
    ChoiceOpen { window: u8, count: i32, flags: u8 },
    /// `0x17 <u8 窗口>`：提交/结束这次选择（FUN_004183B0，清标志并重排）。
    ChoiceClose { window: u8 },
    /// `0x18 <u8 窗口> <u16 值> <u8 标志>`：选择输入模式（FUN_004186C0）。
    /// 标志低 2 位：0 = 鼠标、1 = 键盘、2 = 取消、3 = 确认；值存进窗口对象 +0x1824。
    ChoiceInput { window: u8, value: u16, flags: u8 },
    /// `0x1A`：0..4 号窗口的 32 项选项表全部清空（FUN_004185D0）。
    ChoiceClear,
    /// `0x1B`：0..4 号窗口的选项矩形随机打乱（FUN_004185F0，用 _rand() 洗牌）。
    ChoiceShuffle,
    /// `0x74 <名字> <序号>`：把音效装进声道（FUN_0043ABB0，名字存在 vm+0x596+序号*13）。
    SeLoad { name: String, slot: i32 },
    /// `0x75 <序号> <参数>`：播放该声道（FUN_0043ACF0）。
    SePlay { slot: i32, param: i32 },
    /// `0x76 <序号>`：停掉该声道（FUN_0043AEB0 → FUN_00405B70）。
    SeStop { slot: i32 },
    /// `0x79 <参数>`：语音声道（0x20）的播放参数，0 表示停（FUN_0043B1A0）。
    VoiceParam { value: i32 },
    /// `0x7B <序号> <参数>`：设置声道重复次数，0 表示停（FUN_0043B230）。
    SeRepeat { slot: i32, value: i32 },
    /// `0x7E <u8 位/变量> <u16 序号>`：把「语音声道还在放吗」写进位数组/变量数组（FUN_0043B430）。
    FlagFromVoice { which: u8, index: u16 },
    /// `0x81 <u8 模式>`：UI 模式字节（FUN_00422BC0）。
    UiMode(u8),
    /// `0x49 <start> <count> <x> <u8 which>`：把全局数组写回存档块 27/28。
    /// `which` 0 = 位数组（块 27），1 = 变量数组（块 28）；第三个操作数引擎读了但没用。
    GlobalSave { start: i32, count: i32, x: i32, which: u8 },
    /// `0x4A <start> <count> <x> <u8 which>`：从存档块 27/28 读回全局数组。
    GlobalLoad { start: i32, count: i32, x: i32, which: u8 },
    /// `0x50 <idx> <w> <h>`：分配一块离屏画布（FUN_0040C3C0）。
    Surface { index: i32, w: i32, h: i32 },
    /// `0x51 <idx> <x> <y>`：设置离屏画布贴在画面上的位置（FUN_0040D2E0）。
    SurfaceRect { index: i32, x: i32, y: i32 },
    /// `0x53 <surface> <r> <g> <b>`：文字窗口底色（FUN_0040D350）。
    WindowRect { surface: i32, r: u8, g: u8, b: u8 },
    /// `0x55 <value> <r> <g> <b> <u8 flag>`：窗口淡入淡出/特效（FUN_00439680）。
    /// `value` 为 0 时只改颜色；非 0 时按毫秒做过渡。
    WindowFx { value: i32, r: u8, g: u8, b: u8, flag: u8 },
    /// `0x57`：把图像画到画面上。`img` 是 `0x56` 注册表里的序号，
    /// `(x,y,w,h)` 是源矩形，`(dx,dy)` 是目标位置，`dur` 是过渡时长（毫秒）。
    Draw { cmd: u8, img: i32, x: i32, y: i32, w: i32, h: i32, dur: i32, dx: i32, dy: i32,
           extra: Vec<i32> },
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
    /// `--once`：剧本跳转回到了已经走过的剧本（一轮通关结束）。
    ScriptLoop(String),
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
            Halt::ScriptLoop(n) => write!(f, "回到已走过的剧本 {}", n),
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

/// 绘制槽（engine+0xE0，64 项 × 0x1C 字节）。
#[derive(Clone, Copy, Default)]
pub struct Slot {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub layer: u8,
    pub image: u16,
}

pub struct Vm {
    plain: std::rc::Rc<Vec<u8>>,
    base: usize,
    labels: std::rc::Rc<Vec<u32>>,
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
    /// 位数组的有效位数（引擎 `0x30 <u16 n>` 设成 n+1；构造时 1024）。
    pub bits_n: usize,
    /// 变量数组的有效个数（引擎 `0x40 <u16 n>` 设成 n+1；构造时 256）。
    pub vars_n: usize,
    /// 已读文本位图（引擎 vm+0x564）：`0xA5 <u32 n>` 设成 n+1 并从存档块 29 载入。
    pub seen: Vec<u8>,
    pub seen_n: usize,
    /// `0x02 CALLMOD` 能调用的其他模块（名字 -> 已解密脚本）。
    pub modules: std::collections::HashMap<String, std::rc::Rc<Script>>,
    /// 当前模块名。
    pub module: String,
    /// 模块调用栈：(返回后模块名, 返回偏移)。对应引擎 vm+0x4F/vm+0x12C。
    mod_stack: Vec<(String, usize)>,
    /// 输入状态字节（引擎 vm+0x549/0x54A/0x54C/0x54D/0x54E），0x3A 读它们。
    pub b549: u8,
    pub b54a: u8,
    pub b54c: u8,
    pub b54d: u8,
    pub b54e: u8,
    /// 无人值守：0x85 每次轮询都当作「点了第一个热区」。
    pub auto: bool,
    /// 已经执行过的剧本（`0x01` 跳转）。`stop_on_repeat` 为真时，回到走过的剧本就停。
    pub visited: std::collections::HashSet<String>,
    pub stop_on_repeat: bool,
    /// 图像注册表：`0x56 <u32 idx> <名字>`。
    pub images: std::collections::HashMap<u32, String>,
    /// 绘制槽：`0x84`。
    pub slots: Vec<Slot>,
    /// 每条台词出现时的背景名（槽 0 的图像解析结果）。
    pub text_bg: Vec<Option<String>>,
    /// 存档块 27（全局位数组的落盘副本），`0x49/0x4A` 直接读写它。
    pub sys27: Vec<u8>,
    /// 存档块 28（全局变量数组的落盘副本）。
    pub sys28: Vec<u8>,
}

impl Vm {
    pub fn new(script: &Script, limit: u64) -> Vm {
        Vm {
            plain: script.plain.clone(),
            base: script.head_len,
            labels: script.labels.clone(),
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
            bits_n: 1024,
            vars_n: 256,
            seen: vec![0; 0x10000],
            seen_n: 1024,
            modules: Default::default(),
            module: String::new(),
            mod_stack: Vec::new(),
            b549: 0,
            b54a: 0,
            b54c: 0,
            b54d: 0,
            b54e: 0,
            auto: true,
            visited: Default::default(),
            stop_on_repeat: false,
            images: Default::default(),
            slots: vec![Slot::default(); 64],
            text_bg: Vec::new(),
            sys27: Vec::new(),
            sys28: Vec::new(),
        }
    }

    /// 把存档里的块 27/28 装进来（对应引擎打开 KIZUNAR.SAV 后读到的那两块）。
    pub fn load_system(&mut self, sf: &crate::sav::SaveFile) {
        self.sys27 = sf.system_block(27).to_vec();
        self.sys28 = sf.system_block(28).to_vec();
    }

    /// `0x4A`：从块 27/28 读回全局数组。位数组按 LSB-first 原样存放，
    /// 变量数组是裸 u32，都从块首算起。
    fn global_load(&mut self, start: i32, count: i32, which: u8) {
        if start < 0 || count <= 0 {
            return;
        }
        let start = start as usize;
        let count = count as usize;
        if which == 0 {
            if start >= self.bits_n {
                return;
            }
            let base = start / 8;
            let end = (start + count).min(self.bits_n);
            for i in start..end {
                let at = base + (i / 8 - base);
                let bit = match self.sys27.get(at) {
                    Some(&b) => (b >> (i & 7)) & 1,
                    None => 0,
                };
                self.bits[i] = bit;
            }
        } else if which == 1 {
            if start >= self.vars_n {
                return;
            }
            let end = (start + count).min(self.vars_n).min(self.vars.len());
            for i in start..end {
                let at = i * 4;
                let v = match self.sys28.get(at..at + 4) {
                    Some(b) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    None => 0,
                };
                self.vars[i] = v;
            }
        }
    }

    /// `0x49`：把全局数组写回块 27/28（存档时随文件一起落盘）。
    fn global_store(&mut self, start: i32, count: i32, which: u8) {
        if start < 0 || count <= 0 {
            return;
        }
        let start = start as usize;
        let count = count as usize;
        if which == 0 {
            if start >= self.bits_n {
                return;
            }
            let base = start / 8;
            let end = (start + count).min(self.bits_n);
            let need = base + (end.div_ceil(8) - base);
            if self.sys27.len() < need {
                self.sys27.resize(need, 0);
            }
            for i in start..end {
                let at = base + (i / 8 - base);
                if self.bits[i] != 0 {
                    self.sys27[at] |= 1 << (i & 7);
                } else {
                    self.sys27[at] &= !(1 << (i & 7));
                }
            }
        } else if which == 1 {
            if start >= self.vars_n {
                return;
            }
            let end = (start + count).min(self.vars_n).min(self.vars.len());
            let need = end * 4;
            if self.sys28.len() < need {
                self.sys28.resize(need, 0);
            }
            for i in start..end {
                self.sys28[i * 4..i * 4 + 4].copy_from_slice(&self.vars[i].to_le_bytes());
            }
        }
    }

    /// 当前背景：槽 0 绑定的图像号，去注册表里查名字。
    fn current_bg(&self) -> Option<String> {
        let s = self.slots.first()?;
        self.images.get(&(s.image as u32)).cloned()
    }

    /// 挂上可调用模块表并记录当前模块名。
    pub fn with_modules(
        mut self,
        mods: std::collections::HashMap<String, std::rc::Rc<Script>>,
        name: &str,
    ) -> Vm {
        self.modules = mods;
        self.module = name.to_string();
        self.visited.insert(name.to_string());
        self
    }

    /// 切换模块：等价于引擎 FUN_00434B50 的「重定位标签 + PC 归零」。
    /// 变量和位数组是全局的，不清。
    fn load_module(&mut self, name: &str) -> bool {
        match self.modules.get(name) {
            Some(s) => {
                self.plain = s.plain.clone();
                self.labels = s.labels.clone();
                self.base = s.head_len;
                self.code = s.head_len;
                self.module = name.to_string();
                true
            }
            None => false,
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
        let v = u16le(&self.plain, self.code);
        self.code += 2;
        v
    }

    fn take_u32(&mut self) -> u32 {
        let v = u32le(&self.plain, self.code);
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
            0x01 => {
                // FUN_004363A0：跳转剧本。先把模块深度/返回栈清零，再载入新剧本。
                let name = ascii_of(&self.plain[self.code..end]);
                self.events.push(Event::JumpScript(name.clone()));
                if self.stop_on_repeat && !self.visited.insert(name.clone()) {
                    return Err(Halt::ScriptLoop(name));
                }
                self.ret.clear();
                self.mod_stack.clear();
                Ok(self.load_module(&name))
            }
            0x02 => {
                // FUN_004363E0：调用模块，返回地址 = 本指令末尾。
                let name = ascii_of(&self.plain[self.code..end]);
                self.events.push(Event::CmdName(name.clone()));
                if self.modules.contains_key(&name) {
                    let ret = end - self.base;
                    let caller = self.module.clone();
                    self.load_module(&name);
                    self.mod_stack.push((caller, ret));
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            0x03 => {
                // FUN_00436460：模块返回；栈空即整个脚本结束。
                match self.mod_stack.pop() {
                    Some((name, ret)) => {
                        self.load_module(&name);
                        self.code = self.base + ret;
                        Ok(true)
                    }
                    None => Err(Halt::End),
                }
            }
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
                self.text_bg.push(self.current_bg());
                self.events.push(Event::Text { speaker: spk, text: txt, voice: self.voice.take() });
                Ok(false)
            }
            0x30 => {
                // FUN_00437660：声明位数组尺寸 = 操作数 + 1
                let n = self.take_u16() as usize + 1;
                self.bits_n = n;
                Ok(false)
            }
            0x40 => {
                // FUN_004386a0：声明变量数组尺寸 = 操作数 + 1
                let n = self.take_u16() as usize + 1;
                self.vars_n = n;
                Ok(false)
            }
            0xA5 => {
                // FUN_0043bf90：声明已读文本位图尺寸，并从 KIZUNAR.SAV 的块 29 载入。
                // 存档不存在不算错（引擎里 FUN_0043e1d0 直接返回 1）。
                let n = crate::u32le(&self.plain, self.code) as usize + 1;
                self.code += 4;
                self.seen_n = n;
                if let Some(raw) = self.host.load_seen() {
                    let packed = crate::sav::rle_decode(&raw, n);
                    for i in 0..n.min(self.seen.len()) {
                        self.seen[i] = (packed[i >> 3] >> (i & 7)) & 1;
                    }
                }
                Ok(false)
            }
            0xA6 => {
                // FUN_0043c000：文字窗口参数 <u8> <u16> <u16>
                let a = self.take_u8();
                let b = self.take_u16();
                let c = self.take_u16();
                self.host.set_text_rect(a, b, c);
                Ok(false)
            }
            0x84 => {
                // FUN_0043B6F0：定义绘制槽（64 项，engine+0xE0）
                // 84 <u8 slot> <u32 x> <u32 y> <u32 w> <u32 h> <u8 layer> <u16 img> <u8> <u8> <u8>
                let slot = (self.take_u8() & 0x3F) as usize;
                let x = self.take_u32();
                let y = self.take_u32();
                let w = self.take_u32();
                let h = self.take_u32();
                let layer = self.take_u8();
                let image = self.take_u16();
                let _f6 = self.take_u8();
                let _f5 = self.take_u8();
                let _f4 = self.take_u8();
                if slot < self.slots.len() {
                    self.slots[slot] = Slot { x, y, w, h, layer, image };
                }
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
            0x70 => {
                // FUN_0043A690：BGM。NUL 结尾的名字 + 模式 u8。
                let raw = ascii_of(&self.plain[self.code..end]);
                self.code += raw.len() + 1;
                let mode = self.take_u8();
                // 引擎用 _splitpath 取主名再拼 ".OGG"（脚本里写的是 "25.wav" 这种）
                let stem = raw.rsplit_once('.').map(|(a, _)| a).unwrap_or(&raw);
                self.events.push(Event::Bgm { name: format!("{}.OGG", stem), mode });
                Ok(false)
            }
            0x71 => {
                // FUN_0043A9A0：<u8 循环> <操作数 音量>
                let looping = self.take_u8() == 0;
                let volume = self.val() as u32;
                self.events.push(Event::BgmVolume { looping, volume });
                Ok(false)
            }
            0x72 => {
                // FUN_0043AAF0：音量操作数，< 1 表示停
                let level = self.val() as u32;
                self.events.push(Event::BgmLevel(level));
                Ok(false)
            }
            0x15 => {
                // FUN_00436D80：选项文本 + 相对窗口基准的矩形
                let window = self.take_u8();
                let index = self.take_u8();
                let x = self.val();
                let y = self.val();
                let w = self.val();
                let h = self.val();
                let mut raw = &self.plain[self.code..end];
                if let Some(p) = raw.iter().position(|&b| b == 0) {
                    raw = &raw[..p];
                }
                let text = decode_cp932(raw);
                self.code = end;
                self.events.push(Event::Choice { window, index, x, y, w, h, text });
                Ok(false)
            }
            0x16 => {
                // FUN_00436E60 → FUN_004182A0：打开选项窗口
                let window = self.take_u8();
                let count = self.val();
                let flags = self.take_u8();
                self.events.push(Event::ChoiceOpen { window, count, flags });
                Ok(false)
            }
            0x17 => {
                // FUN_00436EC0 → FUN_004183B0：复位窗口
                let window = self.take_u8();
                self.events.push(Event::ChoiceClose { window });
                Ok(false)
            }
            0x18 => {
                // FUN_00436F00 → FUN_004186C0：<u8 窗口> <u16 值> <u8 标志>
                let window = self.take_u8();
                let value = self.take_u16();
                let flags = self.take_u8();
                self.events.push(Event::ChoiceInput { window, value, flags });
                Ok(false)
            }
            0x1A => {
                // FUN_00436F70：0..4 号窗口的选项表全清
                self.events.push(Event::ChoiceClear);
                Ok(false)
            }
            0x1B => {
                // FUN_00436FA0：0..4 号窗口的选项矩形随机洗牌
                self.events.push(Event::ChoiceShuffle);
                Ok(false)
            }
            0x49 => {
                // FUN_00439000：把全局数组的一段写回存档块 27/28。
                let start = self.val();
                let count = self.val();
                let x = self.val();
                let which = self.take_u8();
                self.global_store(start, count, which);
                self.events.push(Event::GlobalSave { start, count, x, which });
                Ok(false)
            }
            0x4A => {
                // FUN_00439060：从存档块 27/28 读回全局数组的一段。
                let start = self.val();
                let count = self.val();
                let x = self.val();
                let which = self.take_u8();
                self.global_load(start, count, which);
                self.events.push(Event::GlobalLoad { start, count, x, which });
                Ok(false)
            }
            0x50 => {
                // FUN_00439480：<序号> <宽> <高>
                let index = self.val();
                let w = self.val();
                let h = self.val();
                self.events.push(Event::Surface { index, w, h });
                Ok(false)
            }
            0x51 => {
                // FUN_004394E0：<序号> <x> <y>
                let index = self.val();
                let x = self.val();
                let y = self.val();
                self.events.push(Event::SurfaceRect { index, x, y });
                Ok(false)
            }
            0x53 => {
                // FUN_00439550：<窗口> <r> <g> <b>
                let surface = self.val();
                let r = self.take_u8();
                let g = self.take_u8();
                let b = self.take_u8();
                self.events.push(Event::WindowRect { surface, r, g, b });
                Ok(false)
            }
            0x55 => {
                // FUN_00439680：<时长> <r> <g> <b> <u8 标志>
                let value = self.val();
                let r = self.take_u8();
                let g = self.take_u8();
                let b = self.take_u8();
                let flag = self.take_u8();
                self.events.push(Event::WindowFx { value, r, g, b, flag });
                Ok(false)
            }
            0x57 => {
                // FUN_004398C0：8 个操作数 + 按 cmd 追加的额外操作数
                let cmd = self.take_u8();
                let mut f = [0i32; 8];
                for v in f.iter_mut() {
                    *v = self.val();
                }
                let extra_n = match cmd {
                    0x0F..=0x12 => 2,
                    0x14 | 0x15 => 1,
                    0x1C | 0x1D => 4,
                    _ => 0,
                };
                let mut extra = Vec::new();
                for _ in 0..extra_n {
                    extra.push(self.val());
                }
                self.events.push(Event::Draw {
                    cmd, img: f[0], x: f[1], y: f[2], w: f[3], h: f[4], dur: f[5],
                    dx: f[6], dy: f[7], extra,
                });
                Ok(false)
            }
            0x74 => {
                // FUN_0043ABB0：先把声道停掉，再按名字从归档里装音效
                let raw = ascii_of(&self.plain[self.code..end]);
                self.code += raw.len() + 1;
                let slot = self.val();
                self.events.push(Event::SeLoad { name: raw, slot });
                Ok(false)
            }
            0x75 => {
                // FUN_0043ACF0：<序号> <参数>
                let slot = self.val();
                let param = self.val();
                self.events.push(Event::SePlay { slot, param });
                Ok(false)
            }
            0x76 => {
                // FUN_0043AEB0：停掉声道
                let slot = self.val();
                self.events.push(Event::SeStop { slot });
                Ok(false)
            }
            0x79 => {
                // FUN_0043B1A0：语音声道 0x20 的参数
                let value = self.val();
                self.events.push(Event::VoiceParam { value });
                Ok(false)
            }
            0x7B => {
                // FUN_0043B230：<序号> <重复次数/参数>
                let slot = self.val();
                let value = self.val();
                self.events.push(Event::SeRepeat { slot, value });
                Ok(false)
            }
            0x7E => {
                // FUN_0043B430：<u8 位/变量> <u16 序号>
                let which = self.take_u8();
                let index = self.take_u16();
                self.events.push(Event::FlagFromVoice { which, index });
                Ok(false)
            }
            0x81 => {
                // FUN_0043B570：<u8 模式>
                let mode = self.take_u8();
                self.events.push(Event::UiMode(mode));
                Ok(false)
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
                    let (spot, ev) = if self.auto {
                        // 无人值守：当作点了第一个热区，把剧本一路推到底。
                        (0, 2)
                    } else {
                        self.host.poll_input(mode, flags & 1)
                    };
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
            0x56 => {
                let index = u32le(&self.plain, self.code);
                let name = ascii_of(&self.plain[self.code + 4..end]);
                self.images.insert(index, name.clone());
                self.events.push(Event::Image { index, name });
                Ok(false)
            }
            0xF0 => {
                let s = ascii_of(&self.plain[self.code..end]);
                self.events.push(Event::VoiceArchive(s));
                Ok(false)
            }
            _ => {
                *self.unknown.entry(op).or_insert(0) += 1;
                Ok(false)
            }
        }
    }

    /// 把位数组打包成存档里的字节串（`ceil(bits_n/8)` 字节，LSB 优先）。
    pub fn pack_bits(&self) -> Vec<u8> {
        let mut out = vec![0u8; (self.bits_n + 7) / 8];
        for i in 0..self.bits_n.min(self.bits.len()) {
            if self.bits[i] != 0 {
                out[i >> 3] |= 1 << (i & 7);
            }
        }
        out
    }

    /// 从存档格恢复位数组与变量数组（尺寸以格里的长度为准）。
    pub fn apply_slot(&mut self, slot: &crate::sav::Slot) {
        self.bits_n = slot.bits.len() * 8;
        for i in 0..self.bits_n.min(self.bits.len()) {
            self.bits[i] = (slot.bits[i >> 3] >> (i & 7)) & 1;
        }
        self.vars_n = slot.vars.len();
        for (i, &v) in slot.vars.iter().enumerate().take(self.vars.len()) {
            self.vars[i] = v;
        }
    }

    /// 把当前位数组与变量数组写进一个存档格。
    pub fn fill_slot(&self, slot: &mut crate::sav::Slot) {
        slot.bits = self.pack_bits();
        slot.vars = self.vars.iter().take(self.vars_n).copied().collect();
        slot.vars.resize(crate::sav::VARS_N, 0);
    }

    /// `0x3A`：系统条件（FUN_004383D0，跳转表 0x438658，共 16 个系统号）。
    /// 每个分支都是「拿一个全局/引擎字段和操作数比」；默认值全 0（无人操作、无存档）。
    fn sys_test(&mut self, sid: u8, arg: u8) -> bool {
        match sid {
            0x00 => 0 == arg as i32,
            0x01 => false, // 鼠标边沿：d4=0、b8=0 时三个分支都不成立
            0x02..=0x07 => 0 == (if arg == 0 { 1 } else { 0 }),
            0x08 => {
                let held = if self.b54c != 0 { 1 } else { self.b549 };
                (held != 0) == (arg == 0)
            }
            0x0A => arg != 0,
            0x0B => false == (arg != 0),
            0x0C => (self.b54a == 1) == (arg == 0),
            0x0D => false, // 文字管理器空闲（0）≠ 1/2
            0x0E => {
                let mut al = self.b54d;
                if self.b54e == 1 && self.b54c == 0 {
                    al |= 2;
                }
                al == arg
            }
            0x0F => false == (arg != 0),
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
