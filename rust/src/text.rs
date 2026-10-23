//! 文字窗文本流解释器。
//!
//! 台词（0x2B）的正文不是纯字符串，而是一段带控制码的流。引擎在
//! FUN_00418D40（消息窗每帧更新）里逐字节解释它，本模块是它的还原。
//!
//! 关键点：**小于 0x14 的字节是控制码**，其余字节是 cp932 文字。
//! 所以 0x00 是「回车」，0x06 是「换行」，两者总是成对出现（CRLF）。
//!
//! | 码 | 参数 | 含义 |
//! |---|---|---|
//! | 0x00 | — | 回车：x 回到左边距，y 不动 |
//! | 0x01 | u8 索引 + 3 字节 BGR | 改颜色（0 正文 / 1 阴影 / 2 描边） |
//! | 0x02 | — | 等待玩家点击（翻页） |
//! | 0x03 | — | 清窗 / 回到顶部（新的一屏） |
//! | 0x04 | u8 | 单字节参数（语速之类） |
//! | 0x05 | — | 本条台词结束 |
//! | 0x06 | — | 换行：y += 行距 + 4，放不下就翻页等待 |
//! | 0x07 | u8 | 音效/其它单字节参数 |
//! | 0x08 | u32 | 跳转/长参数 |
//! | 0x09 | u8 | 单字节参数 |
//! | 0x0A | 3 字节 | ruby（注音）参数 |
//! | 0x13 | 名字串 | 说话人名（最多 13 字节） |
//! | 0xFF | — | 进入绘制模式（后面才是正文） |
//!
//! 几何来自引擎默认值：文字区 (16,16)-(宽-16,高-16)，行距 = 字高 + 4，
//! 其中字高是窗口基类的 +0x38（默认 16，消息窗没改过）。

/// 文字区几何（引擎里的 +0x182C/+0x1830/+0x1834/+0x1838 与 +0x38）。
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    /// 行高（引擎 +0x38，默认 16）。
    pub line_h: i32,
}

impl Metrics {
    /// 消息窗默认值：552×152 的窗口，四边各留 16。
    pub fn window(w: i32, h: i32) -> Metrics {
        Metrics { left: 16, top: 16, right: w - 16, bottom: h - 16, line_h: 16 }
    }

    /// 下一行的 y。
    pub fn next_y(&self, y: i32) -> i32 {
        y + self.line_h + 4
    }

    /// 引擎的「还放得下吗」判断：y + 2 + 行高 < 下边界。
    pub fn fits(&self, y: i32) -> bool {
        y + 2 + self.line_h < self.bottom
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics::window(552, 152)
    }
}

/// 一个字符的落点。
#[derive(Clone, Debug)]
pub struct Glyph {
    pub ch: char,
    /// 左上角（引擎在 y 上多加了 2 像素基线偏移）。
    pub x: i32,
    pub y: i32,
    pub size: u16,
    pub color: u32,
}

/// 屏幕上的一行。
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub y: i32,
    pub x: i32,
    pub text: String,
    pub glyphs: Vec<Glyph>,
}

/// 一屏（翻页之间）。
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub rows: Vec<Row>,
    /// 这一屏结束时是否要等玩家点击（0x02 或自动翻页）。
    pub wait: bool,
}

/// 解析结果。
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub pages: Vec<Page>,
    /// 说话人名（0x13），已按引擎的 13 字节上限截断。
    pub name: String,
    /// 三个颜色：正文 / 阴影 / 描边。
    pub colors: [u32; 3],
    /// 当前字号（0x01 里能改，默认 20）。
    pub size: u16,
    /// 一共要等几次点击。
    pub waits: usize,
}

impl Layout {
    /// 纯文本（所有行拼起来，用换行分隔）——日志、字幕、存档摘要都用它。
    pub fn plain(&self) -> String {
        let mut s = String::new();
        for (pi, p) in self.pages.iter().enumerate() {
            if pi > 0 {
                s.push('\n');
            }
            for r in &p.rows {
                s.push_str(&r.text);
                s.push('\n');
            }
        }
        s
    }
}

/// 默认字宽：全角 = 字号，半角 = 字号/2。渲染时应该换成真实字体度量。
pub fn default_measure(ch: char, size: u16) -> i32 {
    let c = ch as u32;
    let half = (0xFF61..=0xFF9F).contains(&c)   // 半角片假名
        || c < 0x1100                            // ASCII / 拉丁
        || (0x2000..0x2070).contains(&c);        // 一般标点
    if half {
        (size / 2) as i32
    } else {
        size as i32
    }
}

/// 从 cp932 流里取一个字符，返回 (字符, 占用字节数)。
fn take_char(raw: &[u8], i: usize) -> (char, usize) {
    let b = raw[i];
    if b < 0x80 {
        return (b as char, 1);
    }
    let lead = (0x81..=0x9F).contains(&b) || (0xE0..=0xFC).contains(&b);
    let n = if lead && i + 1 < raw.len() { 2 } else { 1 };
    let end = (i + n).min(raw.len());
    let (s, _, _) = encoding_rs::SHIFT_JIS.decode(&raw[i..end]);
    let ch = s.chars().next().unwrap_or('\u{FFFD}');
    (ch, n)
}

fn color_of(b: u8, g: u8, r: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// 解释一段台词正文。
///
/// measure 用来量字宽（真实渲染传字体度量，日志用 default_measure）。
pub fn parse(raw: &[u8], m: &Metrics, measure: impl Fn(char, u16) -> i32) -> Layout {
    let mut out = Layout { colors: [0xFFFFFF, 0, 0], size: 20, ..Default::default() };
    let mut pages: Vec<Page> = Vec::new();
    let mut page = Page::default();
    let mut row = Row { y: m.top, x: m.left, ..Default::default() };
    let mut x = m.left;
    let mut y = m.top;
    let mut i = 0usize;

    // 结束当前行：把 row 收进 page。
    macro_rules! flush_row {
        () => {
            if !row.text.is_empty() {
                row.x = m.left;
                page.rows.push(row.clone());
                row.text.clear();
                row.glyphs.clear();
            }
            row.y = y;
        };
    }

    // 换行；放不下就翻页（引擎 FUN_00419990 返回 1 的那条路）。
    macro_rules! newline {
        () => {{
            flush_row!();
            x = m.left;
            y = m.next_y(y);
            if !m.fits(y) {
                page.wait = true;
                out.waits += 1;
                pages.push(page.clone());
                page = Page::default();
                y = m.top;
                x = m.left;
            }
            row.y = y;
        }};
    }

    while i < raw.len() {
        let b = raw[i];
        if b == 0xFF {
            // 进入绘制模式：引擎里这个字节不进文字流
            i += 1;
            continue;
        }
        if b >= 0x14 {
            let (ch, n) = take_char(raw, i);
            let w = measure(ch, out.size);
            if x + w > m.right && x > m.left {
                newline!();
            }
            row.glyphs.push(Glyph { ch, x, y: y + 2, size: out.size, color: out.colors[0] });
            row.text.push(ch);
            x += w;
            i += n;
            continue;
        }
        match b {
            0x00 => {
                // 回车
                x = m.left;
                i += 1;
            }
            0x01 => {
                // 改颜色：<u8 索引><u8 B><u8 G><u8 R>
                if i + 4 < raw.len() {
                    let idx = (raw[i + 1] as usize).min(2);
                    out.colors[idx] = color_of(raw[i + 2], raw[i + 3], raw[i + 4]);
                }
                i += 5;
            }
            0x02 => {
                // 等点击（翻页）
                page.wait = true;
                out.waits += 1;
                i += 1;
            }
            0x03 => {
                // 清窗：把已经写满的一屏收起来，回到顶部
                flush_row!();
                if !page.rows.is_empty() {
                    page.wait = true;
                    out.waits += 1;
                    pages.push(page.clone());
                }
                page = Page::default();
                y = m.top;
                x = m.left;
                row.y = y;
                i += 1;
            }
            0x04 | 0x07 | 0x09 => i += 2,
            0x05 => i += 1,
            0x06 => {
                newline!();
                i += 1;
            }
            0x08 => i += 5,
            0x0A => i += 4,
            0x13 => {
                // 说话人名，最多 28 字节，引擎只保留 13 字节
                let mut j = i + 1;
                let mut name = Vec::new();
                while j < raw.len() && raw[j] != 0 && name.len() < 28 {
                    name.push(raw[j]);
                    j += 1;
                }
                name.truncate(13);
                out.name = crate::isf::decode_cp932(&name);
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    flush_row!();
    if !page.rows.is_empty() || pages.is_empty() {
        // 台词打完引擎一定会等一次点击（FUN_00434710 之后进入等待态）
        page.wait = true;
        out.waits += 1;
        pages.push(page);
    }
    out.pages = pages;
    out
}

/// 把解析结果压成一段文本（不含位置），给日志用。
pub fn flatten(l: &Layout) -> String {
    let mut s = String::new();
    for p in &l.pages {
        for r in &p.rows {
            s.push_str(&r.text);
            s.push('\n');
        }
    }
    s.trim_end().to_string()
}
