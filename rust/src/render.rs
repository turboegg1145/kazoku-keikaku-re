//! 软件渲染器：面（surface）合成 + alpha 混合贴图 + 字形光栅化。
//!
//! 上层是 Player：照着引擎的绘制指令（0x50/0x51/0x56/0x21/0x57）维护
//! 128 块离屏面，0 号面就是屏幕。它只负责「摆位」，不负责「动」——
//! 淡入淡出/擦除这类过渡在静态帧里等价于一次性贴完，过渡曲线留给平台层。
//!
//! 几何语义全部来自 KIZUNAR.EXE：
//!
//! ```text
//! 0x50 FUN_0040c3c0  分配面 <idx> <w> <h>
//! 0x51 FUN_0040d2e0  选显示面 + 滚动偏移 <idx> <x> <y>
//! 0x56 FUN_0040d3a0  把图装进面 <idx> <名字>
//! 0x57 FUN_004398c0  绘制（cmd 决定目标面与过渡方式）
//! ```

use crate::ggd::Image;
use crate::isf::Event;

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    /// RGBA8
    pub pixels: Vec<u8>,
    /// 整块不透明（alpha 全 255）。贴图时可以走 memcpy 快路径。
    pub opaque: bool,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Canvas {
        Canvas { width, height, pixels: vec![0; width * height * 4], opaque: false }
    }

    pub fn fill(&mut self, r: u8, g: u8, b: u8, a: u8) {
        for px in self.pixels.chunks_exact_mut(4) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = a;
        }
        self.opaque = a == 255;
    }

    /// 把 B,G,R,A 的图贴到 (dx, dy)，按 alpha 混合。
    pub fn blit(&mut self, img: &Image, dx: isize, dy: isize) {
        for y in 0..img.height {
            let ty = dy + y as isize;
            if ty < 0 || ty >= self.height as isize {
                continue;
            }
            for x in 0..img.width {
                let tx = dx + x as isize;
                if tx < 0 || tx >= self.width as isize {
                    continue;
                }
                let s = (y * img.width + x) * img.channels;
                let b = img.pixels[s] as u32;
                let g = img.pixels[s + 1] as u32;
                let r = img.pixels[s + 2] as u32;
                let a = if img.channels == 4 && img.alpha { img.pixels[s + 3] as u32 } else { 255 };
                if a == 0 {
                    continue;
                }
                self.blend(tx as usize, ty as usize, r, g, b, a);
            }
        }
    }

    pub fn blend(&mut self, x: usize, y: usize, r: u32, g: u32, b: u32, a: u32) {
        let o = (y * self.width + x) * 4;
        let dst_a = self.pixels[o + 3] as u32;
        if dst_a == 255 {
            // 目标本来就不透明：一次乘法就够，不用算 out_a
            if a == 255 {
                self.pixels[o] = r as u8;
                self.pixels[o + 1] = g as u8;
                self.pixels[o + 2] = b as u8;
            } else if a != 0 {
                let inv = 255 - a;
                self.pixels[o] = ((r * a + self.pixels[o] as u32 * inv + 127) / 255) as u8;
                self.pixels[o + 1] = ((g * a + self.pixels[o + 1] as u32 * inv + 127) / 255) as u8;
                self.pixels[o + 2] = ((b * a + self.pixels[o + 2] as u32 * inv + 127) / 255) as u8;
            }
            return;
        }
        // source-over，全程 0..255 定点
        let out_a = a + dst_a * (255 - a) / 255;
        if out_a == 0 {
            return;
        }
        for (i, src) in [r, g, b].iter().enumerate() {
            let dst = self.pixels[o + i] as u32;
            self.pixels[o + i] =
                ((src * a * 255 + dst * dst_a * (255 - a)) / (out_a * 255)) as u8;
        }
        self.pixels[o + 3] = out_a as u8;
    }

    /// 写一行字，返回这行的宽度（像素）。`y` 是行顶。
    pub fn draw_text(
        &mut self,
        font: &fontdue::Font,
        text: &str,
        x: isize,
        y: isize,
        size: f32,
        color: (u8, u8, u8),
    ) -> f32 {
        let mut pen = x as f32;
        for ch in text.chars() {
            let (m, bitmap) = font.rasterize(ch, size);
            if m.width > 0 && m.height > 0 {
                let gx = pen as isize + m.xmin as isize;
                let gy = y + size as isize - m.height as isize - m.ymin as isize;
                for row in 0..m.height {
                    for col in 0..m.width {
                        let cov = bitmap[row * m.width + col] as u32;
                        if cov == 0 {
                            continue;
                        }
                        let tx = gx + col as isize;
                        let ty = gy + row as isize;
                        if tx < 0 || ty < 0 || tx >= self.width as isize || ty >= self.height as isize {
                            continue;
                        }
                        self.blend(tx as usize, ty as usize, color.0 as u32, color.1 as u32, color.2 as u32, cov);
                    }
                }
            }
            pen += m.advance_width;
        }
        pen - x as f32
    }

    /// 按像素宽度折行。日文可以任意断行，只要别让标点跑到行首。
    pub fn wrap(&self, font: &fontdue::Font, text: &str, size: f32, max_width: f32) -> Vec<String> {
        const NO_LINE_START: &str = "、。，．・：；？！ー」』）】〕｝〉》…　";
        let mut lines = Vec::new();
        let mut cur = String::new();
        let mut w = 0.0f32;
        for ch in text.chars() {
            if ch == '\n' {
                lines.push(std::mem::take(&mut cur));
                w = 0.0;
                continue;
            }
            let adv = font.metrics(ch, size).advance_width;
            if w + adv > max_width && !cur.is_empty() {
                if NO_LINE_START.contains(ch) && cur.chars().count() > 1 {
                    // 标点不能打头：把上一行的最后一个字一起带下来
                    let mut cs: Vec<char> = cur.chars().collect();
                    let last = cs.pop().unwrap();
                    cur = cs.into_iter().collect();
                    lines.push(std::mem::take(&mut cur));
                    cur.push(last);
                    w = font.metrics(last, size).advance_width;
                } else {
                    lines.push(std::mem::take(&mut cur));
                    w = 0.0;
                }
            }
            cur.push(ch);
            w += adv;
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }

    /// 统计画上去的像素（含只有 alpha 的部分——文本框是纯黑带 alpha）。
    pub fn stats(&self) -> (usize, (usize, usize, usize, usize)) {
        let mut n = 0;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for y in 0..self.height {
            for x in 0..self.width {
                let o = (y * self.width + x) * 4;
                if self.pixels[o] | self.pixels[o + 1] | self.pixels[o + 2] != 0 || self.pixels[o + 3] > 8 {
                    n += 1;
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if n == 0 {
            return (0, (0, 0, 0, 0));
        }
        (n, (x0, y0, x1, y1))
    }

    /// 只统计「亮」像素（文字），返回 (数量, bbox)。用来判断字有没有溢出文本框。
    pub fn bright_bbox(&self, threshold: u8) -> (usize, (usize, usize, usize, usize)) {
        let mut n = 0;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for y in 0..self.height {
            for x in 0..self.width {
                let o = (y * self.width + x) * 4;
                if self.pixels[o] > threshold && self.pixels[o + 1] > threshold && self.pixels[o + 2] > threshold {
                    n += 1;
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if n == 0 {
            return (0, (0, 0, 0, 0));
        }
        (n, (x0, y0, x1, y1))
    }

    /// 把画面缩成 ASCII，方便在没有图形界面的环境里「看一眼」。
    pub fn ascii(&self, cols: usize, rows: usize) -> String {
        const RAMP: &[u8] = b" .:-=+*#%@";
        let mut s = String::new();
        for ry in 0..rows {
            for rx in 0..cols {
                let x0 = rx * self.width / cols;
                let x1 = ((rx + 1) * self.width / cols).max(x0 + 1);
                let y0 = ry * self.height / rows;
                let y1 = ((ry + 1) * self.height / rows).max(y0 + 1);
                let mut sum = 0u32;
                let mut n = 0u32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let o = (y * self.width + x) * 4;
                        let l = (self.pixels[o] as u32 * 30
                            + self.pixels[o + 1] as u32 * 59
                            + self.pixels[o + 2] as u32 * 11)
                            / 100;
                        sum += l;
                        n += 1;
                    }
                }
                let v = (sum / n.max(1)) as usize * (RAMP.len() - 1) / 255;
                s.push(RAMP[v] as char);
            }
            s.push('\n');
        }
        s
    }

    /// 把 B,G,R(,A) 的图像整张转成 RGBA 画布。
    pub fn from_image(img: &Image) -> Canvas {
        let mut cv = Canvas::new(img.width, img.height);
        let has_a = img.channels == 4 && img.alpha;
        let mut opaque = true;
        for i in 0..img.width * img.height {
            let s = i * img.channels;
            cv.pixels[i * 4] = img.pixels[s + 2];
            cv.pixels[i * 4 + 1] = img.pixels[s + 1];
            cv.pixels[i * 4 + 2] = img.pixels[s];
            let a = if has_a { img.pixels[s + 3] } else { 255 };
            cv.pixels[i * 4 + 3] = a;
            if a != 255 {
                opaque = false;
            }
        }
        cv.opaque = opaque;
        cv
    }

    /// 把 src 的 (sx,sy,w,h) 贴到 (dx,dy)，越界自动裁剪。
    /// 填一块矩形（越界自动裁剪）。
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: u8, g: u8, b: u8, a: u8) {
        for yy in y..y + h {
            if yy < 0 || yy as usize >= self.height {
                continue;
            }
            for xx in x..x + w {
                if xx < 0 || xx as usize >= self.width {
                    continue;
                }
                let i = (yy as usize * self.width + xx as usize) * 4;
                self.pixels[i] = r;
                self.pixels[i + 1] = g;
                self.pixels[i + 2] = b;
                self.pixels[i + 3] = a;
            }
        }
    }

    pub fn blit_rect(&mut self, src: &Canvas, sx: i32, sy: i32, w: i32, h: i32, dx: i32, dy: i32) {
        self.blit_rect_alpha(src, sx, sy, w, h, dx, dy, 255);
    }

    /// 同上，但整体再乘一个不透明度 level/255（对应 0x14/0x15 的擦除等级）。
    pub fn blit_rect_alpha(
        &mut self,
        src: &Canvas,
        sx: i32,
        sy: i32,
        w: i32,
        h: i32,
        dx: i32,
        dy: i32,
        level: u32,
    ) {
        if level == 0 || w <= 0 || h <= 0 {
            return;
        }
        // 裁剪出源/目标各自的有效区间
        let x0 = sx.max(0);
        let y0 = sy.max(0);
        let x1 = (sx + w).min(src.width as i32);
        let y1 = (sy + h).min(src.height as i32);
        let tx0 = (dx + (x0 - sx)).max(0);
        let ty0 = (dy + (y0 - sy)).max(0);
        let tw = (x1 - x0).min(self.width as i32 - tx0);
        let th = (y1 - y0).min(self.height as i32 - ty0);
        if tw <= 0 || th <= 0 {
            return;
        }
        // 快路径：整块不透明 + 不透明度 100% → 直接按行拷贝
        if level == 255 && src.opaque {
            for j in 0..th {
                let so = ((y0 + j) as usize * src.width + x0 as usize) * 4;
                let dof = ((ty0 + j) as usize * self.width + tx0 as usize) * 4;
                let n = tw as usize * 4;
                self.pixels[dof..dof + n].copy_from_slice(&src.pixels[so..so + n]);
            }
            self.opaque = self.opaque
                && tx0 == 0
                && ty0 == 0
                && tw as usize == self.width
                && th as usize == self.height;
            return;
        }
        for j in 0..th {
            let syy = (y0 + j) as usize;
            let ty = (ty0 + j) as usize;
            for i in 0..tw {
                let sxx = (x0 + i) as usize;
                let tx = (tx0 + i) as usize;
                let o = (syy * src.width + sxx) * 4;
                let a = src.pixels[o + 3] as u32 * level / 255;
                if a == 0 {
                    continue;
                }
                self.blend(
                    tx,
                    ty,
                    src.pixels[o] as u32,
                    src.pixels[o + 1] as u32,
                    src.pixels[o + 2] as u32,
                    a,
                );
            }
        }
        self.opaque = false;
    }

    /// 量一段文字的宽度（像素），用于居中排版。
    pub fn text_width(font: &fontdue::Font, text: &str, size: f32) -> f32 {
        text.chars().map(|c| font.metrics(c, size).advance_width).sum()
    }
    /// 复制一块矩形区域成新画布（用于把大画面按滚动偏移裁成 640×480）。
    pub fn crop(&self, sx: i32, sy: i32, w: usize, h: usize) -> Canvas {
        let mut cv = Canvas::new(w, h);
        cv.blit_rect(self, sx, sy, w as i32, h as i32, 0, 0);
        cv
    }

    pub fn write_png(&self, path: &str) -> crate::Result<()> {
        let file = std::fs::File::create(path)?;
        let w = std::io::BufWriter::new(file);
        let mut enc = png::Encoder::new(w, self.width as u32, self.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| crate::Error(e.to_string()))?;
        writer
            .write_image_data(&self.pixels)
            .map_err(|e| crate::Error(e.to_string()))?;
        Ok(())
    }
}

/// 图片来源：把成员名解析成一张解码好的图（GGD / GGD2 归档）。
pub trait ImageSource {
    fn image(&self, name: &str) -> Option<Image>;
    /// 原始数据成员（DATA 归档里的 EFF*.BIN / SEF*.BIN 等）。
    fn data(&self, _name: &str) -> Option<Vec<u8>> {
        None
    }
}

/// `0x60` 画面特效：用一张 8bpp 蒙版图做「显示面 → 目标面」的溶解。
///
/// 引擎实现 `FUN_004150F0` / `FUN_004156C0`：
/// ```text
/// m = 蒙版[i]                                  // 0 = 不参与，保持显示面
/// if m != 0 {
///     if invert { m = -m }                     // 0xFF 取反加一
///     a = clamp(m + progress, 1, 255)          // progress = t*1024/dur - 512
/// }
/// dst = (目标 * a + 显示面 * (255 - a)) >> 8
/// ```
pub struct Effect {
    pub name: String,
    /// 640×480 的蒙版（源图 320×240，按 2× 放大）。
    pub mask: Vec<u8>,
    /// 起始画面（显示面快照，特效期间不变）。
    pub from: Canvas,
    /// 目标面号。
    pub to: usize,
    pub start: f64,
    pub dur: f64,
    pub invert: bool,
}

/// 解 8bpp BMP（特效蒙版）：返回 (宽, 高, 每像素 1 字节，从上到下)。
pub fn decode_mask(data: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    if data.len() < 0x36 || &data[..2] != b"BM" {
        return None;
    }
    let off = u32::from_le_bytes(data[0x0A..0x0E].try_into().ok()?) as usize;
    let w = i32::from_le_bytes(data[0x12..0x16].try_into().ok()?);
    let h = i32::from_le_bytes(data[0x16..0x1A].try_into().ok()?);
    let bpp = u16::from_le_bytes(data[0x1C..0x1E].try_into().ok()?);
    if bpp != 8 || w <= 0 || h == 0 {
        return None;
    }
    let (w, h, top_down) = (w as usize, h.unsigned_abs() as usize, h < 0);
    let stride = (w + 3) & !3;
    if off + stride * h > data.len() {
        return None;
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let sy = if top_down { y } else { h - 1 - y };
        let row = &data[off + sy * stride..off + sy * stride + w];
        out[y * w..(y + 1) * w].copy_from_slice(row);
    }
    Some((w, h, out))
}

/// 一个选择窗口（引擎里 window 对象的 5 个槽之一）。
#[derive(Clone, Default)]
pub struct ChoiceWindow {
    /// 0x20 设的屏幕位置（引擎 win+0x18/0x1c）。
    pub x: i32,
    pub y: i32,
    /// 客户区原点（引擎 win+0x182c/0x1830，默认 0x10,0x10）。
    pub base: (i32, i32),
    /// 0x16 打开时的选项数；0 = 关着。
    pub open: u8,
    /// 0x16 的 flags：bit1 画文字、bit2 居中、bit3 右对齐。
    pub flags: u8,
    /// 窗口宽高（引擎 win+0x28/+0x2C）；默认 552x152，和消息窗口一致。
    pub width: i32,
    pub height: i32,
    /// 0x15 登记的选项：(相对 x, y, w, h, 文本)。
    pub items: Vec<(i32, i32, i32, i32, String)>,
}

impl Player {
    /// 消息窗口在屏幕上的左上角。引擎把 552×152 的窗口摆在底部居中、离底 8px。
    pub fn msg_origin(&self) -> (i32, i32) {
        ((640 - 552) / 2, 480 - 152 - 8)
    }

    /// 第 w 个窗口的第 i 个选项在**屏幕**上的矩形（不含滚动）。
    pub fn choice_rect(&self, w: usize, i: usize) -> Option<(i32, i32, i32, i32)> {
        let cw = self.choices.get(w)?;
        let (x, y, rw, rh, _) = cw.text_at(i)?;
        let (ox, oy) = self.msg_origin();
        Some((ox + cw.x + x, oy + cw.y + y, rw, rh))
    }

    /// 屏幕坐标落在哪个选项上：返回 (窗口号, 选项号)。
    pub fn hit_choice(&self, x: i32, y: i32) -> Option<(usize, usize)> {
        for (w, cw) in self.choices.iter().enumerate() {
            if cw.open == 0 {
                continue;
            }
            for i in 0..cw.open as usize {
                if let Some((rx, ry, rw_, rh_)) = self.choice_rect(w, i) {
                    if x >= rx && y >= ry && x < rx + rw_ && y < ry + rh_ {
                        return Some((w, i));
                    }
                }
            }
        }
        None
    }
}

impl ChoiceWindow {
    /// 选项在**窗口内**的矩形：客户区原点 + 0x15 给的相对坐标。
    /// 引擎 FUN_00418560 存的就是 base+param，FUN_00418990 直接拿它当窗口坐标用。
    pub fn text_at(&self, i: usize) -> Option<(i32, i32, i32, i32, &str)> {
        let it = self.items.get(i)?;
        Some((self.base.0 + it.0, self.base.1 + it.1, it.2, it.3, &it.4))
    }
}
/// 面合成器：把引擎的绘制指令流还原成 640×480 的画面。
pub struct Player {
    /// 128 块面，0 号是默认显示面。
    pub surf: Vec<Option<Canvas>>,
    /// 0x51 设的滚动偏移。
    pub off: Vec<(i32, i32)>,
    /// 当前显示面（引擎 window+0xc）。
    pub display: usize,
    /// 0x56 注册的图片名。
    pub images: std::collections::HashMap<u32, String>,
    /// 归档里找不到的图片名（每名只记一次）。
    pub missing: Vec<String>,
    /// 出现过的绘制 cmd 与次数（用来盯「还没实现的过渡」）。
    pub cmds: std::collections::BTreeMap<u8, u64>,
    /// 0x21 装进窗口对象的图（5 个窗口各最多 4 张）。
    pub window: Vec<Vec<String>>,
    /// 0x53/0x55 设的窗口底色。
    pub win_color: (u8, u8, u8),
    /// 5 个选择窗口（引擎 window 对象的 5 个槽）。
    pub choices: Vec<ChoiceWindow>,
    pub draws: u64,
    /// 毫秒时钟（特效进度用）。
    pub clock: f64,
    /// 正在播的 0x60 特效。
    pub effect: Option<Effect>,
    /// 播过的特效名（按顺序）。
    pub effects: Vec<String>,
}

impl Default for Player {
    fn default() -> Player {
        Player::new()
    }
}

impl Player {
    pub fn new() -> Player {
        let mut surf: Vec<Option<Canvas>> = (0..128).map(|_| None).collect();
        surf[0] = Some(Canvas::new(640, 480));
        Player {
            surf,
            off: vec![(0, 0); 128],
            display: 0,
            images: Default::default(),
            missing: Vec::new(),
            cmds: Default::default(),
            window: vec![Vec::new(); 5],
            win_color: (0, 0, 0),
            choices: (0..5)
                .map(|_| ChoiceWindow {
                    base: (0x10, 0x10),
                    width: 552,
                    height: 152,
                    ..Default::default()
                })
                .collect(),
            draws: 0,
            clock: 0.0,
            effect: None,
            effects: Vec::new(),
        }
    }

    /// 推进时间（毫秒），特效进度靠它。
    pub fn tick(&mut self, dt_ms: f64) {
        self.clock += dt_ms;
    }

    /// 当前画面（显示面按滚动偏移裁成 640×480），有特效时叠上特效。
    pub fn frame(&self) -> Canvas {
        let mut base = self.display_frame();
        if let Some(e) = self.effect.as_ref() {
            let prog = if e.dur > 0.0 {
                (((self.clock - e.start).max(0.0) * 1024.0 / e.dur) as i32) - 512
            } else {
                512
            };
            let to = match self.surf.get(e.to).and_then(|s| s.as_ref()) {
                Some(c) => c.crop(0, 0, 640, 480),
                None => return base,
            };
            for i in 0..(base.width * base.height).min(e.mask.len()) {
                let m = e.mask[i] as i32;
                if m == 0 {
                    continue;
                }
                let mm = if e.invert { (256 - m) & 0xFF } else { m };
                let a = {
                    let v = mm + prog;
                    if v < 0 {
                        1
                    } else if v > 254 {
                        255
                    } else {
                        v
                    }
                } as u32;
                let ia = 255 - a;
                let d = i * 4;
                for c in 0..3 {
                    let s = to.pixels[d + c] as u32;
                    let b = base.pixels[d + c] as u32;
                    base.pixels[d + c] = (((s * a) >> 8) + ((b * ia) >> 8)) as u8;
                }
                base.pixels[d + 3] = 255;
            }
        }
        base
    }

    /// 只取显示面（不含特效）。
    pub fn display_frame(&self) -> Canvas {
        let (ox, oy) = self.off[self.display];
        match self.surf[self.display].as_ref() {
            Some(c) => c.crop(ox, oy, 640, 480),
            None => Canvas::new(640, 480),
        }
    }

    /// 直接拿显示面的引用（偏移为 0 且显示面就是 0 号时可当屏幕用）。
    pub fn screen(&self) -> Option<&Canvas> {
        self.surf[self.display].as_ref()
    }

    /// 0x56：把图装进面。面已经够大就原地贴，不够大就换一块更大的再贴。
    fn put_image(&mut self, idx: usize, img: &Image) {
        let fits = match self.surf[idx].as_ref() {
            Some(c) => c.width >= img.width && c.height >= img.height,
            None => false,
        };
        if !fits {
            let (w, h) = match self.surf[idx].as_ref() {
                Some(c) => (c.width.max(img.width), c.height.max(img.height)),
                None => (img.width, img.height),
            };
            let mut nc = Canvas::new(w, h);
            if let Some(old) = self.surf[idx].take() {
                nc.blit_rect(&old, 0, 0, old.width as i32, old.height as i32, 0, 0);
            }
            self.surf[idx] = Some(nc);
        }
        let src = Canvas::from_image(img);
        if let Some(dst) = self.surf[idx].as_mut() {
            dst.blit_rect(&src, 0, 0, img.width as i32, img.height as i32, 0, 0);
        }
    }

    /// 面到面：把 src 的矩形贴到 dst 的 (dx,dy)。
    #[allow(clippy::too_many_arguments)]
    pub fn blit(
        &mut self,
        dst: usize,
        src: usize,
        sx: i32,
        sy: i32,
        w: i32,
        h: i32,
        dx: i32,
        dy: i32,
        level: u32,
    ) {
        if dst == src || dst >= 128 || src >= 128 {
            return;
        }
        let s = match self.surf[src].take() {
            Some(c) => c,
            None => return,
        };
        // 单独跑一个剧本时，启动脚本（START.ISF）里分配的面还不存在。
        // 引擎里这些面一直活着，所以这里按需补一块 640×480 的。
        if self.surf[dst].is_none() {
            let mut cv = Canvas::new(640, 480);
            cv.fill(0, 0, 0, 255);
            self.surf[dst] = Some(cv);
        }
        if let Some(d) = self.surf[dst].as_mut() {
            d.blit_rect_alpha(&s, sx, sy, w, h, dx, dy, level);
        }
        self.surf[src] = Some(s);
    }

    /// 吃一条事件；返回 true 表示这条事件动过画面。
    pub fn apply(&mut self, ev: &Event, src: &dyn ImageSource) -> bool {
        match ev {
            Event::Surface { index, w, h } => {
                let idx = (*index & 0x7F) as usize;
                if *w > 0 && *h > 0 {
                    let mut cv = Canvas::new(*w as usize, *h as usize);
                    cv.fill(0, 0, 0, 255);
                    self.surf[idx] = Some(cv);
                } else {
                    self.surf[idx] = None;
                }
                true
            }
            Event::SurfaceRect { index, x, y } => {
                let idx = (*index & 0x7F) as usize;
                self.off[idx] = (*x, *y);
                self.display = idx;
                true
            }
            Event::Image { index, name } => {
                self.images.insert(*index, name.clone());
                let idx = (*index & 0x7F) as usize;
                match src.image(name) {
                    Some(img) => self.put_image(idx, &img),
                    None => {
                        if !self.missing.contains(name) {
                            self.missing.push(name.clone());
                        }
                    }
                }
                true
            }
            Event::SurfaceImage { surface, names, .. } => {
                let i = (*surface as usize).min(4);
                self.window[i] = names.clone();
                true
            }
            Event::Choice { window, index, x, y, w, h, text } => {
                let wi = (*window as usize).min(4);
                let idx = *index as usize;
                let win = &mut self.choices[wi];
                while win.items.len() <= idx {
                    win.items.push((0, 0, 0, 0, String::new()));
                }
                win.items[idx] = (*x, *y, *w, *h, text.clone());
                true
            }
            Event::ChoiceOpen { window, count, flags } => {
                let wi = (*window as usize).min(4);
                self.choices[wi].open = (*count).clamp(0, 32) as u8;
                self.choices[wi].flags = *flags;
                true
            }
            Event::ChoiceClose { window } => {
                let wi = (*window as usize).min(4);
                self.choices[wi].open = 0;
                true
            }
            Event::ChoiceClear => {
                for w in self.choices.iter_mut() {
                    w.items.clear();
                }
                true
            }
            Event::SurfaceParam { surface, a, b, .. } => {
                // 0x20 <窗口> <x> <y> …：移动窗口
                let wi = (*surface as usize).min(4);
                self.choices[wi].x = *a;
                self.choices[wi].y = *b;
                true
            }
            Event::WindowRect { r, g, b, .. } => {
                self.win_color = (*r, *g, *b);
                true
            }
            Event::WindowFx { r, g, b, .. } => {
                self.win_color = (*r, *g, *b);
                true
            }
            Event::Draw { cmd, img, x, y, w, h, dur, dx, dy, extra } => {
                self.draws += 1;
                *self.cmds.entry(*cmd).or_insert(0) += 1;
                let s = (*img & 0x7F) as usize;
                let d = self.display;
                match cmd {
                    // 0/1：面到面。第 6 个操作数是目标面。
                    0 | 1 => {
                        let dst = (*dur & 0x7F) as usize;
                        self.blit(dst, s, *x, *y, *w, *h, *dx, *dy, 255);
                    }
                    // 0x14/0x15：按等级把源面混到画面上（擦除/淡入）。
                    0x14 | 0x15 => {
                        let lvl = extra.first().copied().unwrap_or(255).clamp(0, 255) as u32;
                        self.blit(d, s, *x, *y, *w, *h, *dx, *dy, lvl);
                    }
                    // 其余 cmd 都是「过渡到最终状态」，静态帧等于贴满。
                    _ => self.blit(d, s, *x, *y, *w, *h, *dx, *dy, 255),
                }
                true
            }
            Event::Effect { args, name } => {
                // 0x60 <种类> <面> <时长> <取反> <附加> <蒙版名>
                // 只见到种类 0（FUN_004150F0）；1/2 是别的算法，先记下来不猜。
                if args[0] != 0 {
                    self.missing.push(format!("effect-kind{}", args[0]));
                    return false;
                }
                let to = (args[1] & 0x7F) as usize;
                let dur = args[2].max(1) as f64;
                let invert = args[3] != 0;
                let Some(data) = src.data(name) else {
                    self.missing.push(name.clone());
                    return false;
                };
                let Some((w, h, idx)) = decode_mask(&data) else {
                    self.missing.push(name.clone());
                    return false;
                };
                // 引擎把 320×240 的蒙版按 2× 放大到 640×480。
                let mut mask = vec![0u8; 640 * 480];
                for y in 0..480usize {
                    let sy = (y * h / 480).min(h - 1);
                    for x in 0..640usize {
                        let sx = (x * w / 640).min(w - 1);
                        mask[y * 640 + x] = idx[sy * w + sx];
                    }
                }
                let from = self.display_frame();
                self.effects.push(format!("{}({}ms,面{}{})", name, dur as i64, to, if invert { ",反" } else { "" }));
                self.effect = Some(Effect {
                    name: name.clone(),
                    mask,
                    from,
                    to,
                    start: self.clock,
                    dur,
                    invert,
                });
                true
            }
            _ => false,
        }
    }
}