//! MPEG-1 播放。
//!
//! 原版用 DirectShow 播 `0xF3` 指定的文件（`DOLOGO.MPG` / `FAM_OP.MPG` /
//! `FAM_OPHQ.MPG`），iOS 的 AVFoundation 不认 MPEG-1，所以这里自带解码器：
//! pl_mpeg（Dominic Szablewski，MIT，单文件 C）通过 `vendor/kv_movie.c` 这层
//! 扁平封装接进来，解出来的 RGBA 直接贴到 640×480 画布上，音频丢给混音器。
//!
//! 语义按原版：`0xF3` 是**阻塞**的——片头放完（或点一下跳过）才继续往下跑
//! （`SNR02` 第 14606 条播完 OP 紧接着 `0x01 SNR03`，不阻塞就会在 OP 上面
//! 开始演下一章）。

use crate::render::Canvas;
use std::ffi::c_void;
use std::os::raw::{c_char, c_double, c_int};

extern "C" {
    fn kv_open(path: *const c_char) -> *mut c_void;
    fn kv_info(
        ctx: *mut c_void,
        w: *mut c_int,
        h: *mut c_int,
        fps: *mut c_double,
        dur: *mut c_double,
        rate: *mut c_int,
    ) -> c_int;
    fn kv_decode(ctx: *mut c_void, dt: c_double) -> c_int;
    fn kv_rgba(ctx: *mut c_void, out: *mut u8, stride: c_int) -> c_int;
    fn kv_audio(ctx: *mut c_void, out: *mut i16, max_samples: c_int) -> c_int;
    fn kv_ended(ctx: *mut c_void) -> c_int;
    fn kv_close(ctx: *mut c_void);
}

pub struct Movie {
    ctx: *mut c_void,
    /// 贴在画布上的矩形（脚本给的 x/y/w/h）。
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    src_w: i32,
    src_h: i32,
    /// 解码后的当前帧，RGBA。
    rgba: Vec<u8>,
    /// 音频采样率。
    pub rate: u32,
    pub ended: bool,
    /// 播了多少秒（自己数，不依赖外部时钟）。
    elapsed: f64,
    pub name: String,
}

impl Movie {
    /// 打开 `path`，贴在 `(x, y, w, h)`。失败返回 None（脚本照常往下跑）。
    pub fn open(path: &str, x: i32, y: i32, w: i32, h: i32) -> Option<Movie> {
        // 脚本里的名字不一定带扩展名：START.ISF 写的是 `DOLOGO`，文件却叫
        // `DOLOGO.MPG`（原版自己补的）；SNR02.ISF 写的又是完整的 `FAM_OP.MPG`。
        let mut real = path.to_string();
        if !std::path::Path::new(&real).is_file() {
            for ext in [".MPG", ".mpg", ".MPEG", ".mpeg"] {
                let cand = format!("{}{}", path, ext);
                if std::path::Path::new(&cand).is_file() {
                    real = cand;
                    break;
                }
            }
        }
        let path = real.as_str();
        let c = std::ffi::CString::new(path).ok()?;
        let ctx = unsafe { kv_open(c.as_ptr()) };
        if ctx.is_null() {
            return None;
        }
        let (mut sw, mut sh, mut fps, mut dur, mut rate) = (0, 0, 0.0, 0.0, 0);
        unsafe { kv_info(ctx, &mut sw, &mut sh, &mut fps, &mut dur, &mut rate) };
        if sw <= 0 || sh <= 0 {
            unsafe { kv_close(ctx) };
            return None;
        }
        Some(Movie {
            ctx,
            x,
            y,
            w: if w > 0 { w } else { sw },
            h: if h > 0 { h } else { sh },
            src_w: sw,
            src_h: sh,
            rgba: vec![0u8; (sw * sh * 4) as usize],
            rate: rate.max(8000) as u32,
            ended: false,
            elapsed: 0.0,
            name: path.rsplit(['/', '\\']).next().unwrap_or(path).to_string(),
        })
    }

    /// 推进 `dt` 秒，顺便把解出来的音频塞进 `pcm`。
    pub fn advance(&mut self, dt: f64, pcm: &mut Vec<i16>) {
        if self.ended {
            return;
        }
        self.elapsed += dt;
        let fresh = unsafe { kv_decode(self.ctx, dt) };
        if fresh != 0 {
            unsafe { kv_rgba(self.ctx, self.rgba.as_mut_ptr(), self.src_w * 4) };
        }
        let mut buf = [0i16; 4096];
        loop {
            let n = unsafe { kv_audio(self.ctx, buf.as_mut_ptr(), 4096) };
            if n <= 0 {
                break;
            }
            pcm.extend_from_slice(&buf[..n as usize]);
        }
        if unsafe { kv_ended(self.ctx) } != 0 {
            self.ended = true;
        }
    }

    /// 贴到画布上（最近邻缩放；原版就是 640×480，基本 1:1）。
    pub fn blit(&self, cv: &mut Canvas) {
        let dw = cv.width as i32;
        let dh = cv.height as i32;
        let x0 = self.x.max(0);
        let y0 = self.y.max(0);
        let x1 = (self.x + self.w).min(dw);
        let y1 = (self.y + self.h).min(dh);
        if x1 <= x0 || y1 <= y0 || self.w <= 0 || self.h <= 0 {
            return;
        }
        for y in y0..y1 {
            let sy = (((y - self.y) as i64 * self.src_h as i64) / self.h as i64) as i32;
            let sy = sy.clamp(0, self.src_h - 1);
            for x in x0..x1 {
                let sx = (((x - self.x) as i64 * self.src_w as i64) / self.w as i64) as i32;
                let sx = sx.clamp(0, self.src_w - 1);
                let s = ((sy * self.src_w + sx) * 4) as usize;
                let d = ((y * dw + x) * 4) as usize;
                cv.pixels[d] = self.rgba[s];
                cv.pixels[d + 1] = self.rgba[s + 1];
                cv.pixels[d + 2] = self.rgba[s + 2];
                cv.pixels[d + 3] = 255;
            }
        }
    }
}

impl Drop for Movie {
    fn drop(&mut self) {
        unsafe { kv_close(self.ctx) };
    }
}
