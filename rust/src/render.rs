//! 一个最小的软件渲染器：RGBA 画布 + alpha 混合贴图 + 字形光栅化。
//!
//! 只做「把一张背景、一个文本框、几行字拼成 640×480」这件事，
//! 不做缩放/旋转/特效——那些交给后面的平台层（iOS 上直接用 CoreGraphics 也行）。

use crate::ggd::Image;

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    /// RGBA8
    pub pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Canvas {
        Canvas { width, height, pixels: vec![0; width * height * 4] }
    }

    pub fn fill(&mut self, r: u8, g: u8, b: u8, a: u8) {
        for px in self.pixels.chunks_exact_mut(4) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = a;
        }
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
