//! 画面合成：把背景/立绘/窗框/台词/选项叠成一张 640×480。
//!
//! 之前这套逻辑散在 `main.rs` 的 `draw_message` / `draw_choices` 里，
//! iOS 端也要用同一份，所以搬进库里。

use crate::ggd;
use crate::render::{Canvas, ImageSource, Player};
use crate::text::{self, Metrics};

/// 引擎的台词字号：`CreateFontA(0x14, …)`，高 20。
pub const TEXT_SIZE: f32 = 20.0;
/// 选项窗口的等宽格：引擎 `+0x38` = 16。
pub const CHOICE_CELL: i32 = 16;

/// 一套字体 + 窗框，负责把 `Player` 的面合成成最终画面。
pub struct Composer {
    pub font: fontdue::Font,
    pub size: f32,
    /// 窗框（GGD 里的 FWIN2.GG2：552×152 的 60% 黑圆角框）。
    pub frame: Option<ggd::Image>,
}

impl Composer {
    pub fn from_bytes(bytes: Vec<u8>, size: f32, assets: &dyn ImageSource) -> crate::Result<Composer> {
        let font = fontdue::Font::from_bytes(bytes.as_slice(), fontdue::FontSettings::default())
            .map_err(|e| crate::Error(format!("字体加载失败: {}", e)))?;
        let frame = assets.image("FWIN2.GG2").or_else(|| assets.image("FWIN.GG2"));
        Ok(Composer { font, size, frame })
    }

    /// 字宽（像素）。引擎用 GDI 整数推进，这里取整对齐。
    pub fn measure(&self, ch: char, size: u16) -> i32 {
        self.font.metrics(ch, size as f32).advance_width.round().max(1.0) as i32
    }

    /// 按引擎几何排一段台词。
    pub fn layout(&self, raw: &[u8], m: &Metrics) -> text::Layout {
        text::parse(raw, m, |ch, sz| self.measure(ch, sz))
    }

    /// 窗框在屏幕上的左上角。
    pub fn frame_origin(&self, cv: &Canvas) -> (i32, i32) {
        let (fw, fh) = match self.frame.as_ref() {
            Some(f) => (f.width as i32, f.height as i32),
            None => (552, 152),
        };
        ((cv.width as i32 - fw) / 2, cv.height as i32 - fh - 8)
    }

    /// 贴窗框 + 画台词，返回排版结果（调用方可以拿它算行数/分屏）。
    ///
    /// `shown` 是打字机进度（已显示的字数），`None` 表示全部显示。
    pub fn draw_message(&self, cv: &mut Canvas, raw: &[u8], shown: Option<usize>) -> text::Layout {
        let (fw, fh) = match self.frame.as_ref() {
            Some(f) => (f.width as i32, f.height as i32),
            None => (552, 152),
        };
        let (fx, fy) = self.frame_origin(cv);
        match self.frame.as_ref() {
            Some(f) => cv.blit(f, fx as isize, fy as isize),
            None => {
                // 没有窗框时退化成半透明黑框，方便单独调排版
                for y in fy.max(0)..(fy + fh).min(cv.height as i32) {
                    for x in fx.max(0)..(fx + fw).min(cv.width as i32) {
                        cv.blend(x as usize, y as usize, 0, 0, 0, 153);
                    }
                }
            }
        }
        let m = Metrics::window(fw, fh);
        let lay = self.layout(raw, &m);
        let mut n = 0usize;
        for page in &lay.pages {
            for row in &page.rows {
                for g in &row.glyphs {
                    n += 1;
                    if let Some(limit) = shown {
                        if n > limit {
                            return lay;
                        }
                    }
                    let c = ((g.color >> 16) as u8, (g.color >> 8) as u8, g.color as u8);
                    cv.draw_glyph(
                        &self.font,
                        g.ch,
                        fx as isize + g.x as isize,
                        fy as isize + (g.y - 2) as isize,
                        g.size as f32,
                        c,
                    );
                }
            }
        }
        lay
    }

    /// 画所有打开的选项窗。位置规则照抄 `FUN_00418990`。
    pub fn draw_choices(&self, cv: &mut Canvas, p: &Player) {
        let origin = self.frame_origin(cv);
        for w in p.choices.iter() {
            if w.open == 0 {
                continue;
            }
            // 子窗口面没有 alpha：FUN_004082D0 会把 0 色二次填成白色，所以是一整块白底
            cv.fill_rect(origin.0, origin.1, w.width, w.height, 255, 255, 255, 255);
            for i in 0..w.open as usize {
                let Some((left, top, rw, rh, text)) = w.text_at(i) else { continue };
                let tw = text.chars().count() as i32 * CHOICE_CELL;
                let right = left + rw;
                let bottom = top + rh;
                let (tx, ty) = if w.flags & 4 != 0 {
                    ((right - tw / 2 + left) / 2, (bottom - CHOICE_CELL + top) / 2)
                } else if w.flags & 8 != 0 {
                    (w.width - 0x1e - tw, top)
                } else {
                    (left, top)
                };
                // 1px 50% 黑投影，再画正文（引擎 FUN_004092A0 的 param_8 分支）
                cv.draw_text(&self.font, text, (origin.0 + tx + 1) as isize, (origin.1 + ty + 1) as isize, self.size, (128, 128, 128));
                cv.draw_text(&self.font, text, (origin.0 + tx) as isize, (origin.1 + ty) as isize, self.size, (0, 0, 0));
            }
        }
    }

    /// 出一帧：背景/立绘 → 窗框+台词 → 选项。
    pub fn compose(&self, p: &Player, raw: Option<&[u8]>, shown: Option<usize>) -> Canvas {
        let mut cv = p.frame();
        if let Some(r) = raw {
            self.draw_message(&mut cv, r, shown);
        }
        self.draw_choices(&mut cv, p);
        cv
    }
}
