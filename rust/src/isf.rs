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
    ///
    /// `spots` 是本次参与命中的槽矩形（引擎 FUN_00440B40 会先把 +0x7E8 设成 mode，
    /// 再调 FUN_00440910 在 0..mode 里找第一个命中的）。`want_confirm` 对应 flags 的 bit0。
    fn poll_input(
        &mut self,
        _spots: &[(i32, i32, i32, i32)],
        _want_confirm: bool,
    ) -> (i32, i32) {
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

    /// `0x48`：读出整个 `KIZUNAR.SAV`。没有文件就返回 `None`。
    fn save_read(&mut self) -> Option<Vec<u8>> {
        None
    }

    /// `0x48`：原子替换写回 `KIZUNAR.SAV`（原版是写 `SAVE.TMP` 再 `MoveFile`）。
    fn save_write(&mut self, _data: &[u8]) {}
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
    /// 台词。raw 是原始载荷（含控制码），给 text 模块排版用。
    Text { speaker: Option<u8>, text: String, voice: Option<String>, raw: Vec<u8> },
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
    /// `0x13`：19 字节参数块（全作只出现 1 次，用途未明）。
    Raw13(Vec<u8>),
    /// `0x20 <u8 面> <x> <y> <w> <h> <u8 标志>`：FUN_00414170（只用到前两个参数）。
    SurfaceParam { surface: u8, a: i32, b: i32, c: i32, d: i32, flag: u8 },
    /// `0x21 <u8 面> <u8 模式> <名字…>`：按名字装图（FUN_00437040，最多 4 个字符串）。
    SurfaceImage { surface: u8, mode: u8, names: Vec<String> },
    /// `0x23 <u8 序号> <a> <b> <u8 标志>`：FUN_00422230 / FUN_00422260。
    ObjParam { index: u8, a: i32, b: i32, flag: u8 },
    /// `0x25 <u8 面> <u8 标志> <文本>`：往画面上写文本（FUN_004141E0）。
    SurfaceText { surface: u8, flag: u8, text: String },
    /// `0x26 <u8 面> <开/关>`：FUN_0041C8F0，同时把 vm+0x548 记成开关状态。
    SurfaceFlag { surface: u8, on: i32 },
    /// `0x27 <u8 面> <值>`：FUN_0041C910。
    Surface27 { surface: u8, value: i32 },
    /// `0x29 <u8 面>`：FUN_0041C950。
    Surface29(u8),
    /// `0x2A <u8 面>`：FUN_0041C9A0。
    Surface2A(u8),
    /// `0x73`：停 BGM（FUN_0043AB60，只在该通道状态为 2 时动）。
    BgmStop,
    /// `0x77 <u8 参数>`：语音播放/跳过（FUN_0043AF20，会看 Ctrl 键状态）。
    VoicePlay(u8),
    /// `0x89 <序号 0..63>`：FUN_00440760 选一个索引。
    SelectIndex(i32),
    /// `0xF7 <文本>`：写进 vm+0x190 的 79 字节缓冲（FUN_004346A0）。
    TextBuf(String),
    /// `0x32 <u16 起> <u16 止> <u8 值>`：位数组区间填充（FUN_00423B40）。
    BitsFill { from: u16, to: u16, value: u8 },
    /// `0x37 <功能号> <参数>`：系统功能（FUN_004378F0 里 0..0x13 的大 switch）。
    SysFunc { kind: i32, arg: i32 },
    /// `0x45 <u16 起> <u16 止> <值>`：变量数组区间填充（FUN_00424A80）。
    VarsFill { from: u16, to: u16, value: i32 },
    /// `0x48 <u16 位数组容量> <u16 变量数组容量>`：确定存档块 27/28 的尺寸。
    ///
    /// 原版 `FUN_00438A90`：容量优先取自 `KIZUNAR.SAV` 里已有的块 27/28 头 4 字节；
    /// 文件里没有这两块（新装游戏）时，才按脚本声明的尺寸重建整个存档文件。
    SaveMenu { bits: u16, vars: u16 },
    /// `0x4B <u16 参数> <u8 条件号>`：第二组系统条件（FUN_004392A0）。
    SysCond { arg: u16, kind: u8 },
    /// `0x80 <u8 面> <名字>`：UI 文本资源（FUN_00422A00）。
    UiText { surface: u8, name: String },
    /// `0x86`：把输入状态里的 1/2/13 号键清掉（FUN_00440850）。
    InputReset,
    /// `0x87`：空指令（原版处理函数直接 return）。
    Nop87,
    /// `0x88 <u8 组> <8 个位/变量号>`：登记条件组（64 组 × 9 字节，最后一字节是有效标志）。
    CondGroup { index: u8, slots: [i32; 8] },
    /// `0x8F`：把 64 个条件组的有效标志全清掉。
    CondGroupClear,
    /// `0x84 <槽> <left> <top> <right> <bottom> ...`：定义绘制槽，也就是命中热区。
    Slot { index: usize, rect: (i32, i32, i32, i32), image: u16 },
    /// `0xA0`：FUN_00414760（清空「面文本」列表）。
    SaveMenuDraw,
    /// `0xA1 <6 字节>`：FUN_00414770 配色。
    Color6([u8; 6]),
    /// `0xD6 <u8 面> <u8 标志> <文本>`：文字窗口写文本（FUN_00414590 / FUN_00414560）。
    WindowText { surface: u8, flag: u8, text: String },
    /// `0xE0 <u8 对象> <文本>`：新建/替换一个文本对象（最多 5 个）。
    TextObjNew { index: u8, text: String },
    /// `0xE1 <u8 对象> <u8 行> <文本>`：设某一行（FUN_00424ED0）。
    TextObjLine { index: u8, line: u8, text: String },
    /// `0xE2/0xE3 <u8 对象> <u8 行> <u8 列> <u16 值> <文本>`：设某个格子。
    TextObjCell { index: u8, line: u8, col: u8, value: u16, text: String },
    /// `0xE4`：整块设置（3 个字符串 + 8 个参数，全作只 1 处）。
    TextObjFull(Vec<u8>),
    /// `0xE5 <u8>`：FUN_0043F9A0。
    TextObjFlag(u8),
    /// `0xF3 <x> <y> <w> <h> <文件名>`：播 MPEG（FUN_0043D890）。
    Movie { x: i32, y: i32, w: i32, h: i32, name: String },
    /// `0xF1 <毫秒>`：设一个截止时间（vm+0x7B4 = timeGetTime() + n）。
    Deadline(i32),
    /// `0xF2`：空指令（原版处理函数直接 return 1）。
    NopF2,
    /// `0xFA <值>`：FUN_00434590。
    FuncFa(i32),
    /// `0x7F`：把 vm+0x7E0 的缓冲拷到 vm+0x8E4（lstrcpyA）。
    BufCopy,
    /// `0xF8 <u8 面> <u8 掩码>`：三个颜色通道的开关（FUN_004144D0）。
    ColorChannels { surface: u8, mask: u8 },
    /// `0xF9 <5 字节>`：FUN_004144A0。
    Color5([u8; 5]),
    /// `0x60 <5 个参数> <特效文件名>`：在画面上播特效（FUN_00414FA0，名字进 13 字节缓冲）。
    Effect { args: [i32; 5], name: String },
    /// `0x63 <u16 种类> <u8 a> <u8 b> <时长>`：画面转场/擦除（FUN_00413090）。
    Transition { kind: u16, a: u8, b: u8, dur: i32 },
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
    /// 渲染用：已经拿到足够的台词，主动停下（省时间省内存）。
    TextLimit,
    /// 渲染用：已经遇到第 N 个选项窗口，主动停下。
    ChoiceLimit,
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
            Halt::TextLimit => write!(f, "已经取够台词"),
            Halt::ChoiceLimit => write!(f, "已经拿到选项窗口"),
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
///
/// 前 4 个字段是 RECT（left / top / right / bottom），不是 x/y/宽/高。
/// 反编译 FUN_0043B6F0 是连读 4 个 u32 直接存进去，而命中测试 FUN_00440910
/// 用 PtInRect((RECT*)(slot+0), pt) 判定——所以 TITLE 里的
///   84 00 | 4c 02 00 00 | 34 01 00 00 | 67 02 00 00 | c7 01 00 00
/// 表示 588..615 × 308..455 的竖条，不是「x=588 宽=615」。
#[derive(Clone, Copy, Default)]
pub struct Slot {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
    pub layer: u8,
    pub image: u16,
    /// 0x84 尾巴上的 3 个字节（引擎 +0x16/+0x15/+0x14），含义还没钉死。
    pub f6: u8,
    pub f5: u8,
    pub f4: u8,
}

impl Slot {
    pub fn w(&self) -> i32 {
        (self.r - self.l).max(0)
    }
    pub fn h(&self) -> i32 {
        (self.b - self.t).max(0)
    }
    /// 给命中测试用的 (x, y, 宽, 高)。
    pub fn rect(&self) -> (i32, i32, i32, i32) {
        (self.l, self.t, self.w(), self.h())
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
}

/// 0x37 系统功能维护的全局开关（引擎里的 DAT_004891b8..004891ee / 004892e4/e5）。
///
/// 语义见 docs/07-sysfunc.md：这些字节被输入处理、文字绘制、以及 0x3A 系统条件读，
/// 所以必须真的改，不能只记事件。
#[derive(Clone, Debug)]
pub struct SysFlags {
    /// 下标 = 引擎全局地址的低 12 位：0x4891B8 → 0xB8，engine+0x801 → 0x801。
    /// 和 Python 侧同一套编号，方便逐项对拍。
    pub g: [u32; 0x1000],
    /// engine+0x81C 里音频对象是否就绪（0x37 的 bVar1）。
    pub audio_ready: bool,
    /// DAT_0048940C：文字绘制模式（0 = 整串画，1 = 逐字画）。
    pub text_mode: i32,
}

impl SysFlags {
    pub fn new() -> SysFlags {
        SysFlags { g: [0; 0x1000], audio_ready: true, text_mode: 0 }
    }

    #[inline]
    pub fn get(&self, key: usize) -> u32 {
        self.g[key & 0xFFF]
    }

    #[inline]
    pub fn set(&mut self, key: usize, v: u32) {
        self.g[key & 0xFFF] = v;
    }
}

pub struct Vm {
    plain: std::rc::Rc<Vec<u8>>,
    base: usize,
    labels: std::rc::Rc<Vec<u32>>,
    pub code: usize,
    pub vars: Vec<i32>,
    /// 当前高亮的选项下标（引擎 win+0x181C）；-2 = 没选中，-1 = 取消。
    pub hover: i32,
    /// 主循环驱动时置 true：0x18 的鼠标模式会卡住 PC 等输入（引擎行为）。
    /// 批量 dump 时置 false，否则会原地死循环。
    pub block_on_input: bool,
    /// 上一条 step() 是不是「没推进 PC、原地等输入」（引擎里没调 FUN_00434EB0）。
    /// 主循环靠它判断该不该把控制权交回给玩家。
    pub blocked: bool,
    /// 阻塞时这条指令的结束偏移：解除阻塞就是跳到它（= 引擎补一次 FUN_00434EB0）。
    pub blocked_end: usize,
    /// 指令处理函数请求「卡住」的标记，step() 用它决定是否回退 PC。
    block_requested: bool,
    /// 上一条 0x85 是不是「轮询了但没输入」。脚本靠循环轮询等点击，
    /// 主循环靠这个标记判断「它其实在等人」。
    pub poll_wait: bool,
    /// 最近执行过的指令 (地址, 操作码)，只保留最后 64 条，死循环排查用。
    pub recent: std::collections::VecDeque<(usize, u8)>,
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
    /// 引擎 vm+0x54B（0x37 kind 0x10 写）。
    pub b54b: u8,
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
    /// 上一次 `0x85` 参与命中的槽数（引擎 +0x7E8）。
    pub slot_count: u8,
    /// 每条台词出现时的背景名（槽 0 的图像解析结果）。
    pub text_bg: Vec<Option<String>>,
    /// 存档块 27（全局位数组的落盘副本），`0x49/0x4A` 直接读写它。
    pub sys27: Vec<u8>,
    /// 存档块 28（全局变量数组的落盘副本）。
    pub sys28: Vec<u8>,
    /// 引擎 `+0x7D8`：存档位数组容量，VM 启动时从块 27 头 4 字节读出。
    pub sav_bits: i32,
    /// 引擎 `+0x7DC`：存档变量数组容量，VM 启动时从块 28 头 4 字节读出。
    pub sav_vars: i32,
    /// 是否已经尝试过从存档里取容量（等价于原版构造函数里的那两次读取）。
    sav_ready: bool,
    /// `0xF1` 设的等待时长（引擎 vm+0x7B4 存的是 timeGetTime()+n）。
    pub deadline: i32,
    /// 收到这么多句台词就停（渲染用；None = 不停）。
    pub stop_texts: Option<usize>,
    /// 已经产生的台词数。
    pub text_count: usize,
    /// 遇到这么多选项窗口就停（渲染用）。
    pub stop_choices: Option<usize>,
    /// 已经产生的选项窗口数。
    pub choice_count: usize,
    /// 0x37 维护的全局开关。
    pub sys: SysFlags,
}

impl Vm {
    pub fn new(script: &Script, limit: u64) -> Vm {
        Vm {
            plain: script.plain.clone(),
            base: script.head_len,
            labels: script.labels.clone(),
            code: script.head_len,
            vars: vec![0; MAX_VARS],
            // dump 模式下不模拟输入：等价于「永远选第 0 项」，这样能走完一条完整路线
            hover: 0,
            block_on_input: false,
            blocked: false,
            blocked_end: 0,
            block_requested: false,
            poll_wait: false,
            recent: std::collections::VecDeque::with_capacity(64),
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
            b54b: 0,
            b54c: 0,
            b54d: 0,
            b54e: 0,
            auto: true,
            visited: Default::default(),
            stop_on_repeat: false,
            images: Default::default(),
            slots: vec![Slot::default(); 64],
            slot_count: 0,
            text_bg: Vec::new(),
            sys27: Vec::new(),
            sys28: Vec::new(),
            sav_bits: 0,
            sav_vars: 0,
            sav_ready: false,
            deadline: 0,
            stop_texts: None,
            text_count: 0,
            stop_choices: None,
            choice_count: 0,
            sys: SysFlags::new(),
        }
    }

    /// 从当前位置读一个 NUL 结尾的 cp932 串（对应引擎的 FUN_004349E0）。
    fn take_str(&mut self, end: usize) -> String {
        if self.code >= end {
            return String::new();
        }
        let raw = &self.plain[self.code..end];
        let n = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        // 无效字节 encoding_rs 会解成 U+FFFD；这边直接丢掉，和 Python 的 errors="ignore" 对齐
        let s: String = decode_cp932(&raw[..n])
            .chars()
            .filter(|&c| c != '\u{FFFD}' && !('\u{E000}'..='\u{F8FF}').contains(&c))
            .collect();
        self.code = (self.code + n + 1).min(end);
        s
    }

    /// 把存档里的块 27/28 装进来（对应引擎构造函数里那两次「读块 27/28 头 4 字节」）。
    ///
    /// 原版 `FUN_00434350` / `FUN_00434470` 在 VM 构造时就把容量读进 `+0x7D8` / `+0x7DC`，
    /// 所以 `0x48` 里那段「重建存档」只有在新装游戏（文件里没有块 27/28）时才会走到。
    pub fn load_system(&mut self, sf: &crate::sav::SaveFile) {
        self.sys27 = sf.system_block(27).to_vec();
        self.sys28 = sf.system_block(28).to_vec();
        self.sav_bits = crate::sav::global_capacity(&self.sys27) as i32;
        self.sav_vars = crate::sav::global_capacity(&self.sys28) as i32;
        self.sav_ready = true;
    }

    /// 懒初始化：从宿主那边读一次 `KIZUNAR.SAV`，把块 27/28 的容量装进 `+0x7D8/+0x7DC`。
    /// 对应原版 VM 构造函数里的 `FUN_00434350` / `FUN_00434470`。
    fn sav_ensure(&mut self) {
        if self.sav_ready {
            return;
        }
        self.sav_ready = true;
        let Some(data) = self.host.save_read() else {
            return;
        };
        let t = crate::sav::table(&data);
        let n = data.len() as u32;
        let ok = |o: u32| o != 0 && o.saturating_add(4) <= n;
        if !(ok(t[27]) && ok(t[28])) {
            return;
        }
        self.sys27 = crate::sav::block(&data, 27);
        self.sys28 = crate::sav::block(&data, 28);
        self.sav_bits = crate::sav::global_capacity(&self.sys27) as i32;
        self.sav_vars = crate::sav::global_capacity(&self.sys28) as i32;
    }

    /// `0x48 <u16 位数组容量> <u16 变量数组容量>`（`FUN_00438A90`）。
    ///
    /// 两步：① 容量已经有（从存档里读到了）就什么都不做；
    /// ② 否则按脚本声明的尺寸重建 `KIZUNAR.SAV`：块 0..26 和 29 原样搬运，
    /// 块 27/28 换成 `4 字节容量 + 全 0`。
    fn save_init(&mut self, bits: u16, vars: u16) {
        self.sav_ensure();
        if self.sav_bits >= 1 || self.sav_vars >= 1 {
            return;
        }
        let old = self.host.save_read();
        let mut blocks: Vec<Vec<u8>> = match &old {
            Some(d) => (0..crate::sav::TABLE_N).map(|i| crate::sav::block(d, i)).collect(),
            None => vec![Vec::new(); crate::sav::TABLE_N],
        };
        blocks[27] = crate::sav::new_global_block(bits as usize, false);
        blocks[28] = crate::sav::new_global_block(vars as usize, true);
        let out = crate::sav::pack_blocks(&blocks);
        self.host.save_write(&out);
        self.sys27 = blocks[27].clone();
        self.sys28 = blocks[28].clone();
        self.sav_bits = bits as i32;
        self.sav_vars = vars as i32;
    }

    /// `0x4A`：从块 27/28 读回全局数组。
    ///
    /// 两层夹紧和原版一致：先按 `+0x7D8/+0x7DC`（`0x49` 在调用前就夹过），
    /// 再按块头 4 字节的容量；数据从**头部之后**开始（`FUN_00423F10` 一进门先读 4 字节）。
    fn global_load(&mut self, start: i32, count: i32, which: u8) {
        self.sav_ensure();
        let cap = if which == 0 { self.sav_bits } else { self.sav_vars };
        if start < 0 || count <= 0 || start >= cap {
            return;
        }
        let start = start as usize;
        let count = (count as usize).min(cap as usize - start);
        if which == 0 {
            let blk = std::mem::take(&mut self.sys27);
            let n = crate::sav::global_capacity(&blk).min(self.bits_n);
            if start < n {
                let end = (start + count).min(n);
                for i in start..end {
                    let at = 4 + (i >> 3);
                    let bit = match blk.get(at) {
                        Some(&b) => (b >> (i & 7)) & 1,
                        None => 0,
                    };
                    self.bits[i] = bit;
                }
            }
            self.sys27 = blk;
        } else if which == 1 {
            let blk = std::mem::take(&mut self.sys28);
            let n = crate::sav::global_capacity(&blk).min(self.vars_n).min(self.vars.len());
            if start < n {
                let end = (start + count).min(n);
                for i in start..end {
                    let at = 4 + i * 4;
                    let v = match blk.get(at..at + 4) {
                        Some(b) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                        None => 0,
                    };
                    self.vars[i] = v;
                }
            }
            self.sys28 = blk;
        }
    }

    /// `0x49`：把全局数组写回块 27/28（存档时随文件一起落盘）。
    ///
    /// 位数组是**按字节整块打包**（`FUN_00423F10`）：从 `start/8` 那一字节开始，
    /// 每字节 8 位都从内存数组重新取，所以 `start` 不对齐时也会把前面的位一起覆盖。
    fn global_store(&mut self, start: i32, count: i32, which: u8) {
        self.sav_ensure();
        let cap = if which == 0 { self.sav_bits } else { self.sav_vars };
        if start < 0 || count <= 0 || start >= cap {
            return;
        }
        let start = start as usize;
        let count = (count as usize).min(cap as usize - start);
        if which == 0 {
            let blk = std::mem::take(&mut self.sys27);
            let n = crate::sav::global_capacity(&blk).min(self.bits_n);
            if start < n {
                let end = (start + count).min(n);
                let b1 = end.div_ceil(8);
                for b in start / 8..b1 {
                    let mut v = 0u8;
                    for k in 0..8 {
                        let i = b * 8 + k;
                        if i < self.bits_n && self.bits[i] != 0 {
                            v |= 1 << k;
                        }
                    }
                    let at = 4 + b;
                    if self.sys27.len() <= at {
                        self.sys27.resize(at + 1, 0);
                    }
                    self.sys27[at] = v;
                }
            }
            self.sys27 = blk;
        } else if which == 1 {
            let blk = std::mem::take(&mut self.sys28);
            let n = crate::sav::global_capacity(&blk).min(self.vars_n).min(self.vars.len());
            if start < n {
                let end = (start + count).min(n);
                for i in start..end {
                    let at = 4 + i * 4;
                    if self.sys28.len() < at + 4 {
                        self.sys28.resize(at + 4, 0);
                    }
                    self.sys28[at..at + 4].copy_from_slice(&self.vars[i].to_le_bytes());
                }
            }
            self.sys28 = blk;
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
            Event::Text { speaker, text, voice, .. } => Some((speaker, text, voice)),
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
            if let Some(h) = self.step()? {
                return Ok(h);
            }
        }
    }

    /// 执行一条指令。返回 Some(halt) 表示 VM 停下了，None 表示还能继续。
    ///
    /// 游戏主循环用它做单步驱动：跑一条指令、把新事件交给渲染/音频层，
    /// 遇到需要等待的指令（台词、过渡、选项）就 break 出去等时间/输入。
    pub fn step(&mut self) -> Result<Option<Halt>> {
        self.blocked = false;
        self.poll_wait = false;
        if self.steps >= self.limit {
            return Ok(Some(Halt::StepLimit));
        }
        if self.code < self.base || self.code >= self.plain.len() {
            return Ok(Some(Halt::OutOfRange { at: self.code }));
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
            return Ok(Some(Halt::BadLength { at: start, len }));
        }
        self.steps += 1;
        if self.recent.len() >= 64 {
            self.recent.pop_front();
        }
        self.recent.push_back((start, op));
        match self.exec(op, end) {
            Ok(true) => {
                if self.block_requested {
                    // 这条指令要等输入：把 PC 退回去，下次 step() 重新执行它。
                    // 引擎里对应的就是「没调 FUN_00434EB0」，指令号不变。
                    self.block_requested = false;
                    self.code = start;
                    self.blocked = true;
                    self.blocked_end = end;
                }
                Ok(None)
            }
            Ok(false) => {
                self.code = end;
                Ok(None)
            }
            Err(Halt::End) => Ok(Some(Halt::End)),
            Err(Halt::ReturnUnderflow) => Ok(Some(Halt::ReturnUnderflow)),
            Err(other) => Ok(Some(other)),
        }
    }

    /// 解除输入阻塞：跳过那条卡住的指令（引擎里是输入处理函数补调 FUN_00434EB0）。
    pub fn unblock(&mut self) {
        if self.blocked {
            self.code = self.blocked_end;
            self.blocked = false;
        }
    }

    /// 取走目前累积的事件（主循环每帧清一次，避免长时间运行把内存吃满）。
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
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
                let raw = self.plain[self.code..end].to_vec();
                self.events.push(Event::Text { speaker: spk, text: txt, voice: self.voice.take(), raw });
                self.text_count += 1;
                if let Some(n) = self.stop_texts {
                    if self.text_count >= n {
                        return Err(Halt::TextLimit);
                    }
                }
                // 引擎的 0x2B（0x437530）写完文本就直接返回，不调 FUN_00434EB0，
                // 也就是「这句台词挂着，等玩家点」。这是全篇最主要的阻塞点。
                if self.block_on_input {
                    self.block_requested = true;
                    return Ok(true);
                }
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
                // 84 <u8 slot> <i32 left> <i32 top> <i32 right> <i32 bottom>
                //    <u8 layer> <u16 img> <u8 f6> <u8 f5> <u8 f4>
                let slot = (self.take_u8() & 0x3F) as usize;
                let l = self.take_u32() as i32;
                let t = self.take_u32() as i32;
                let r = self.take_u32() as i32;
                let b = self.take_u32() as i32;
                let layer = self.take_u8();
                let image = self.take_u16();
                let f6 = self.take_u8();
                let f5 = self.take_u8();
                let f4 = self.take_u8();
                if slot < self.slots.len() {
                    self.slots[slot] = Slot { l, t, r, b, layer, image, f6, f5, f4 };
                }
                self.events.push(Event::Slot { index: slot, rect: (l, t, r, b), image });
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
            0x13 => {
                // FUN_00436B40：19 个字节，用途未明
                let mut v = Vec::with_capacity(19);
                for _ in 0..19 {
                    v.push(self.take_u8());
                }
                self.events.push(Event::Raw13(v));
                Ok(false)
            }
            0x20 => {
                // FUN_00436FD0 → FUN_00414170：<面> <x> <y> <w> <h> <标志>
                let surface = self.take_u8();
                let a = self.val();
                let b = self.val();
                let c = self.val();
                let d = self.val();
                let flag = self.take_u8();
                self.events.push(Event::SurfaceParam { surface, a, b, c, d, flag });
                Ok(false)
            }
            0x21 => {
                // FUN_00437040：<面> <模式> + 最多 4 个 NUL 结尾的名字
                let surface = self.take_u8();
                let mode = self.take_u8();
                let mut names = Vec::new();
                while self.code < end {
                    names.push(self.take_str(end));
                }
                self.events.push(Event::SurfaceImage { surface, mode, names });
                Ok(false)
            }
            0x23 => {
                // FUN_00437250：<序号> <a> <b> <标志>
                let index = self.take_u8();
                let a = self.val();
                let b = self.val();
                let flag = self.take_u8();
                self.events.push(Event::ObjParam { index, a, b, flag });
                Ok(false)
            }
            0x25 => {
                // FUN_00437350 → FUN_004141E0：<面> <标志> <文本>
                let surface = self.take_u8();
                let flag = self.take_u8();
                let text = self.take_str(end);
                self.code = end;
                self.events.push(Event::SurfaceText { surface, flag, text });
                Ok(false)
            }
            0x26 => {
                // FUN_004373B0 → FUN_0041C8F0：<面> <开关>
                let surface = self.take_u8();
                let on = self.val();
                self.events.push(Event::SurfaceFlag { surface, on });
                Ok(false)
            }
            0x27 => {
                // FUN_00437410 → FUN_0041C910：<面> <值>
                let surface = self.take_u8();
                let value = self.val();
                self.events.push(Event::Surface27 { surface, value });
                Ok(false)
            }
            0x29 => {
                // FUN_004374B0 → FUN_0041C950：<面>
                let surface = self.take_u8();
                self.events.push(Event::Surface29(surface));
                Ok(false)
            }
            0x2A => {
                // FUN_004374F0 → FUN_0041C9A0：<面>
                let surface = self.take_u8();
                self.events.push(Event::Surface2A(surface));
                Ok(false)
            }
            0x32 => {
                // FUN_004376F0：<u16 起> <u16 止> <u8 值> → 位数组区间填充
                let from = self.take_u16();
                let to = self.take_u16();
                let value = self.take_u8();
                for i in from as usize..=to as usize {
                    if i < self.bits.len() {
                        self.bits[i] = value & 1;
                    }
                }
                self.events.push(Event::BitsFill { from, to, value });
                Ok(false)
            }
            0x37 => {
                // FUN_004378F0：<功能号> <参数>，0..0x13 的大 switch
                let kind = (self.val() & 0xFF) as u8;
                let arg = (self.val() & 0xFF) as u8;
                self.sys_func(kind, arg);
                self.events.push(Event::SysFunc { kind: kind as i32, arg: arg as i32 });
                Ok(false)
            }
            0x45 => {
                // FUN_004388A0：<u16 起> <u16 止> <值> → 变量数组区间填充
                let from = self.take_u16();
                let to = self.take_u16();
                let value = self.val();
                let end_i = (to as usize).min(self.vars.len().saturating_sub(1));
                for i in from as usize..=end_i {
                    self.vars[i] = value;
                }
                self.events.push(Event::VarsFill { from, to, value });
                Ok(false)
            }
            0x48 => {
                // FUN_00438A90：<u16 位数组容量> <u16 变量数组容量>，只出现在 START.ISF。
                let bits = self.take_u16();
                let vars = self.take_u16();
                self.code = end;
                self.save_init(bits, vars);
                self.events.push(Event::SaveMenu { bits, vars });
                Ok(false)
            }
            0x4B => {
                // FUN_004392A0：<u16 参数> <u8 条件号>，结果写进 0x47 的条件
                let arg = self.take_u16();
                let kind = self.take_u8();
                self.events.push(Event::SysCond { arg, kind });
                Ok(false)
            }
            0x80 => {
                // FUN_0043B520 → FUN_00422A00：<面> <名字>
                let surface = self.take_u8();
                let name = self.take_str(end);
                self.code = end;
                self.events.push(Event::UiText { surface, name });
                Ok(false)
            }
            0x86 => {
                // FUN_00440850：清 1/2/13 号键的输入状态
                self.events.push(Event::InputReset);
                Ok(false)
            }
            0x87 => {
                // FUN_004434F0：原版就是空的
                self.events.push(Event::Nop87);
                Ok(false)
            }
            0x88 => {
                // FUN_0043B8E0：<组号> <8 个 0..63 的位/变量号>
                let index = self.take_u8() & 0x3F;
                let mut slots = [0i32; 8];
                for v in slots.iter_mut() {
                    *v = self.val() & 0x3F;
                }
                self.events.push(Event::CondGroup { index, slots });
                Ok(false)
            }
            0x8F => {
                // FUN_0043BB90：清全部条件组的有效标志
                self.events.push(Event::CondGroupClear);
                Ok(false)
            }
            0xA0 => {
                // FUN_0043BD10 → FUN_00414760
                self.events.push(Event::SaveMenuDraw);
                Ok(false)
            }
            0xA1 => {
                // FUN_0043BD40 → FUN_00414770：6 个字节
                let mut v = [0u8; 6];
                for b in v.iter_mut() {
                    *b = self.take_u8();
                }
                self.events.push(Event::Color6(v));
                Ok(false)
            }
            0xD6 => {
                // FUN_0043C9B0：<面> <标志> <文本>
                let surface = self.take_u8();
                let flag = self.take_u8();
                let text = self.take_str(end);
                self.code = end;
                self.events.push(Event::WindowText { surface, flag, text });
                Ok(false)
            }
            0xE0 => {
                // FUN_0043CAE0：<对象> <文本>
                let index = self.take_u8();
                let text = self.take_str(end);
                self.code = end;
                self.events.push(Event::TextObjNew { index, text });
                Ok(false)
            }
            0xE1 => {
                // FUN_0043CBD0：<对象> <行> <文本>
                let index = self.take_u8();
                let line = self.take_u8();
                let text = self.take_str(end);
                self.code = end;
                self.events.push(Event::TextObjLine { index, line, text });
                Ok(false)
            }
            0xE2 | 0xE3 => {
                // FUN_0043CC90 / FUN_0043CE20：<对象> <行> <列> <u16 值> <文本>
                let index = self.take_u8();
                let line = self.take_u8();
                let col = self.take_u8();
                let value = self.take_u16();
                let text = self.take_str(end);
                self.code = end;
                self.events.push(Event::TextObjCell { index, line, col, value, text });
                Ok(false)
            }
            0xE4 => {
                // FUN_0043CF60：整块设置（3 个字符串 + 8 个参数）
                let v = self.plain[self.code..end].to_vec();
                self.code = end;
                self.events.push(Event::TextObjFull(v));
                Ok(false)
            }
            0xE5 => {
                // FUN_0043D170 → FUN_0043F9A0
                let v = self.take_u8();
                self.events.push(Event::TextObjFlag(v));
                Ok(false)
            }
            0xF1 => {
                // FUN_0043D860：vm+0x7B4 = timeGetTime() + n
                let ms = self.val();
                self.deadline = ms;
                self.events.push(Event::Deadline(ms));
                Ok(false)
            }
            0xF2 => {
                // FUN_0043DDE0：空指令
                self.events.push(Event::NopF2);
                Ok(false)
            }
            0xF3 => {
                // FUN_0043D890：<x> <y> <w> <h> <文件名> → 播 MPEG
                let x = self.val();
                let y = self.val();
                let w = self.val();
                let h = self.val();
                let name = self.take_str(end);
                self.code = end;
                self.events.push(Event::Movie { x, y, w, h, name });
                Ok(false)
            }
            0xF8 => {
                // FUN_0043DB50：<面> <掩码> → 三个颜色通道
                let surface = self.take_u8();
                let mask = self.take_u8();
                self.events.push(Event::ColorChannels { surface, mask });
                Ok(false)
            }
            0xF9 => {
                // FUN_0043DBE0：<面> <a> <b> <c> <d>
                let mut v = [0u8; 5];
                for b in v.iter_mut() {
                    *b = self.take_u8();
                }
                self.events.push(Event::Color5(v));
                Ok(false)
            }
            0xFA => {
                // FUN_0043DC70 → FUN_00434590
                let v = self.val();
                self.events.push(Event::FuncFa(v));
                Ok(false)
            }
            0x7F => {
                // FUN_0043B4F0：lstrcpyA(vm+0x8E4, vm+0x7E0)
                self.events.push(Event::BufCopy);
                Ok(false)
            }
            0x60 => {
                // FUN_0043A250：5 个操作数 + 特效文件名（进 13 字节缓冲）
                let mut args = [0i32; 5];
                for v in args.iter_mut() {
                    *v = self.val();
                }
                let name = self.take_str(end);
                self.code = end;
                self.events.push(Event::Effect { args, name });
                Ok(false)
            }
            0x63 => {
                // FUN_0043A530：<u16 种类> <u8 a> <u8 b> <时长>；返回 0 才前进
                let kind = self.take_u16();
                let a = self.take_u8();
                let b = self.take_u8();
                let dur = self.val();
                self.events.push(Event::Transition { kind, a, b, dur });
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
                self.choice_count += 1;
                if let Some(n) = self.stop_choices {
                    if self.choice_count >= n {
                        return Err(Halt::ChoiceLimit);
                    }
                }
                Ok(false)
            }
            0x17 => {
                // FUN_00436EC0 → FUN_004183B0：复位窗口
                let window = self.take_u8();
                self.events.push(Event::ChoiceClose { window });
                Ok(false)
            }
            0x18 => {
                // FUN_00436F00 → FUN_004186C0：<u8 窗口> <u16 槽> <u8 标志>
                //
                // 标志低两位是模式（FUN_004186C0 的 switch）：
                //   0 鼠标：等待点击，**不推进 PC**（引擎里 FUN_00434EB0 不被调用）
                //   1 键盘：清掉当前选择
                //   2 取消：把 -1 写进槽
                //   3 确认：把当前高亮项写进槽（无选中写 -2）
                let window = self.take_u8();
                let slot = self.take_u16();
                let flags = self.take_u8();
                match flags & 3 {
                    0 => {
                        if self.block_on_input {
                            self.hover = -2;
                        }
                        self.events.push(Event::ChoiceInput { window, value: slot, flags });
                        if self.block_on_input {
                            self.block_requested = true;
                            return Ok(true); // 阻塞：step() 会把 PC 退回去
                        }
                    }
                    1 => {
                        if self.block_on_input {
                            self.hover = -2;
                        }
                    }
                    2 => {
                        if (slot as usize) < self.vars.len() {
                            self.vars[slot as usize] = -1;
                        }
                    }
                    _ => {
                        if (slot as usize) < self.vars.len() {
                            self.vars[slot as usize] = self.hover;
                        }
                    }
                }
                self.events.push(Event::ChoiceInput { window, value: slot, flags });
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
            0x73 => {
                // FUN_0043AB60：停 BGM
                self.events.push(Event::BgmStop);
                Ok(false)
            }
            0x77 => {
                // FUN_0043AF20：语音播放/跳过
                let v = self.take_u8();
                self.events.push(Event::VoicePlay(v));
                Ok(false)
            }
            0x89 => {
                // FUN_0043BA30 → FUN_00440760：<序号 0..63>
                let idx = self.val() & 0x3F;
                self.events.push(Event::SelectIndex(idx));
                Ok(false)
            }
            0xF7 => {
                // FUN_0043DB10 → FUN_004346A0：<文本>
                let text = self.take_str(end);
                self.code = end;
                self.events.push(Event::TextBuf(text));
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
                // FUN_0043B800：85 <u16 var_a> <u16 var_b> <u8 槽数> <u8 flags>
                //   槽数 = 引擎 +0x7E8，只有 0..槽数-1 号槽参与命中
                //   flags bit0 = 要确认键（param_4），bit1 = 只更新悬停（FUN_004407A0）
                let va = self.take_u16();
                let vb = self.take_u16();
                let mode = self.take_u8();
                let flags = self.take_u8();
                self.slot_count = mode;
                if flags & 2 == 0 {
                    let n = (mode as usize).min(self.slots.len());
                    let mut rects = [(0i32, 0i32, 0i32, 0i32); 64];
                    for i in 0..n {
                        rects[i] = self.slots[i].rect();
                    }
                    let (spot, ev) = if self.auto {
                        // 无人值守：当作点了第一个热区，把剧本一路推到底。
                        if n == 0 { (-1, 0) } else { (0, 2) }
                    } else {
                        self.host.poll_input(&rects[..n], flags & 1 != 0)
                    };
                    if ev == 0 {
                        // 没有输入：脚本会自己循环回来，主循环把它当「等玩家」。
                        self.poll_wait = true;
                    }
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
    /// `0x37` 系统功能（FUN_004378F0）。两个操作数在引擎里都被截成字节。
    ///
    /// 参数有上限：0/1/8 必须 < 3，2..0xd / 0xf / 0x10 / 0x11 / 0x13 必须 < 2，
    /// 0xe 必须 < 4；越界直接返回、什么都不做。
    fn sys_func(&mut self, kind: u8, arg: u8) {
        let limit = match kind {
            0x00 | 0x01 | 0x08 => 3,
            0x02..=0x07 | 0x09..=0x0D | 0x0F..=0x11 | 0x13 => 2,
            0x0E => 4,
            _ => 0xFF,
        };
        if kind < 0x14 && arg >= limit {
            return;
        }
        let a = arg as u32;
        let ready = self.sys.audio_ready;
        match kind {
            0x00 => match arg {
                0 => {
                    self.sys.set(0xBC, 1);
                    self.sys.set(0xC0, 1);
                    self.sys.set(0xC4, 1);
                }
                1 => {
                    self.sys.set(0xBC, 1);
                    self.sys.set(0xC0, 1);
                    self.sys.set(0xC4, 0);
                    if ready && self.sys.get(0xB8) != 0 {
                        if self.sys.text_mode == 0 {
                            self.sys.set(0xD4, 0);
                            self.sys.set(0xAC, a);
                            self.sys.set(0xB8, 1);
                            return;
                        }
                        if self.sys.text_mode > 3 {
                            self.sys.set(0xAC, a);
                            self.sys.set(0xBC, 1);
                            self.sys.set(0xC0, 1);
                            self.sys.set(0xC4, 0);
                            return;
                        }
                    }
                    self.sys.set(0xB8, 0);
                    self.sys.set(0xAC, a);
                    self.sys.set(0xD4, 1);
                    return;
                }
                2 => {
                    self.sys.set(0xBC, 0);
                    self.sys.set(0xC0, 0);
                    self.sys.set(0xC4, 0);
                    self.sys.set(0xAC, a);
                    return;
                }
                _ => {}
            },
            0x01 => {
                if self.sys.get(0xAC) == 2 {
                    return;
                }
                if arg == 0 && ready {
                    self.sys.set(0xD4, 0);
                } else {
                    self.sys.set(0xD4, 0);
                    if self.sys.get(0xBC) != 0 || self.sys.get(0xC4) != 0 {
                        self.sys.set(0xD4, 1);
                    }
                    if arg == 1 {
                        self.sys.set(0xB8, 0);
                        return;
                    }
                }
                if ready && (self.sys.get(0xC0) != 0 || self.sys.get(0xC4) != 0) {
                    self.sys.set(0xB8, 1);
                    return;
                }
                self.sys.set(0xB8, 0);
                return;
            }
            0x02 => {
                self.sys.set(0xC8, (arg == 0) as u32);
                return;
            }
            0x03 => {
                if ready && arg == 0 && (self.sys.get(0xB8) != 0 || self.sys.get(0xAC) != 2) {
                    self.sys.set(0xD8, 1);
                    self.sys.set(0xCC, 1);
                } else {
                    self.sys.set(0xCC, 0);
                    self.sys.set(0xD8, 0);
                }
                return;
            }
            0x04 => {
                if ready && arg == 0 {
                    self.sys.set(0xDC, 1);
                    self.sys.set(0xD0, 1);
                } else {
                    self.sys.set(0xD0, 0);
                    self.sys.set(0xDC, 0);
                }
                return;
            }
            0x05 => {
                self.sys.set(0xE0, (ready && arg == 0) as u32);
                return;
            }
            0x06 => {
                let v = if self.sys.get(0xB8) != 0 && arg == 0 { (self.sys.get(0xD8) != 0) as u32 } else { 0 };
                self.sys.set(0xCC, v);
                return;
            }
            0x07 => {
                let v = if self.sys.get(0xD4) != 0 && arg == 0 { (self.sys.get(0xDC) != 0) as u32 } else { 0 };
                self.sys.set(0xD0, v);
                return;
            }
            0x08 => {
                // FUN_00435C60：vm+0x549 = 模式；arg==1 时清 vm+0x54D 的 bit1
                match arg {
                    0 => {
                        self.b549 = 1;
                        self.b54d &= !2;
                        self.events.push(Event::SeStop { slot: 0x20 });
                    }
                    1 => self.b549 = 0,
                    2 => {
                        self.b549 = 1;
                        self.b54d &= !2;
                    }
                    _ => {}
                }
                return;
            }
            0x09 => {
                self.sys.set(0xE4, (arg == 0) as u32);
                return;
            }
            0x0A => {
                self.sys.set(0xE8, (arg != 0) as u32);
                return;
            }
            0x0C => {
                self.b54a = arg;
                return;
            }
            0x0D => return, // 窗口激活模式，无画面意义
            0x0E => {
                // FUN_00435D70：bit1 置位时同时清 vm+0x549
                if arg & 2 != 0 {
                    self.b549 = 0;
                }
                self.b54d = arg;
                return;
            }
            0x0F => {
                self.sys.set(0x801, (arg != 0) as u32);
                return;
            }
            0x10 => {
                self.b54b = (arg != 0) as u8;
                return;
            }
            0x11 => {
                self.sys.set(0xEC, (arg != 0) as u32);
                return;
            }
            0x13 => {
                self.sys.set(0x2E5, (arg != 0) as u32);
                return;
            }
            0x15 => {
                self.sys.set(0xED, (arg != 0) as u32);
                return;
            }
            0x16 => {
                self.sys.set(0x2E4, (arg != 0) as u32);
                return;
            }
            0x17 => {
                self.sys.set(0xEE, a);
                return;
            }
            _ => {},
        }
    }

    /// `0x3A`：按引擎系统状态跳转（FUN_004383D0）。和 Python 侧逐条对齐。
    fn sys_test(&mut self, sid: u8, arg: u8) -> bool {
        let g = |k: usize| self.sys.get(k);
        match sid {
            0x00 => g(0xAC) == arg as u32,
            0x01 => {
                // 鼠标按键边沿：0 = 刚按下，1 = 刚松开，2 = 按下中
                let (d4, b8) = (g(0xD4), g(0xB8));
                match arg {
                    0 => d4 == 0 && b8 != 0,
                    1 => d4 != 0 && b8 == 0,
                    _ => d4 != 0 && b8 != 0,
                }
            }
            0x02..=0x07 => {
                let key = match sid {
                    0x02 => 0xC8,
                    0x03 => 0xD8,
                    0x04 => 0xDC,
                    0x05 => 0xE0,
                    0x06 => 0xCC,
                    _ => 0xD0,
                };
                g(key) == if arg == 0 { 1 } else { 0 }
            }
            0x08 => {
                // FUN_00435C40(vm,0)：输入被锁（+0x54C）时返回 1，否则返回 +0x549
                let held = if self.b54c != 0 { 1 } else { self.b549 };
                (held != 0) == (arg == 0)
            }
            0x0A => arg != 0,
            0x0B => (g(0xA1) != 0) == (arg != 0),
            0x0C => (self.b54a == 1) == (arg == 0),
            // 文字管理器空闲（0）≠ 1/2；无人值守时按「瞬间打完」处理
            0x0D => false,
            0x0E => {
                let mut al = self.b54d;
                if self.b54e == 1 && self.b54c == 0 {
                    al |= 2;
                }
                al == arg
            }
            0x0F => (g(0x801) != 0) == (arg != 0),
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
