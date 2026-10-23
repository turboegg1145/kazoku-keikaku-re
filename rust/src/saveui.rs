//! 存读档界面。
//!
//! 原版是 Win32 对话框（`.rsrc` 里的 DIALOG 182/183，标题 `Load`）：
//! 9 行「データN + `2026/10/23 22:31`」，右侧 160×120 缩略图。
//! 这里按同一套语义重画；**存档格字节布局和 PC 版完全一致**（见 `docs/06-save.md`）。
//!
//! 和原版一致的地方：
//!
//! - 27 格 = 3 页 × 9 格，格子编号 `page*9 + i`；
//! - 日期字符串是 `sprintf("%4d/%2d/%2d %02d:%02d")`（`FUN_00426850` 里那串 `%s/%s/%s %s:%s`）；
//! - 扩展段固定写 5 个 `00 00 00 00`；尾部固定 0x30 个 0；
//! - 存盘走 `SAVE.TMP` → 换名，和 `FUN_00438A90` 一样。

use crate::isf::Vm;
use crate::render::Canvas;
use crate::sav::{self, SaveFile, Slot, SLOT_N};

/// 每页格子数（引擎里 `page*9 + i`，见 `FUN_00426EA3` 的 `lea eax,[eax+eax*8]`）。
pub const PER_PAGE: usize = 9;
/// 总页数。
pub const PAGES: usize = SLOT_N / PER_PAGE;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Save,
    Load,
}

/// 玩家在存读档界面上做的一个动作。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// 鼠标移到第 i 格。
    Hover(usize),
    /// 点了第 i 格（原版双击/确定）。
    Pick(usize),
    Prev,
    Next,
    Ok,
    Cancel,
}

pub struct SaveScreen {
    pub mode: Mode,
    pub page: usize,
    pub cursor: usize,
    pub file: SaveFile,
    /// `KIZUNAR.SAV` 的完整路径。
    pub path: String,
    /// 最近一次操作的结果，画在底部。
    pub message: String,
}

/// 对话框 182 的控件几何（DLU）换算到 640×480 后的位置。
pub const PANEL: (i32, i32, i32, i32) = (56, 36, 528, 408);
const ROW_X: i32 = 72;
const ROW_Y: i32 = 84;
const ROW_W: i32 = 300;
const ROW_H: i32 = 30;
const ROW_STEP: i32 = 34;
const PREVIEW: (i32, i32) = (396, 84);
const BTN_OK: (i32, i32, i32, i32) = (292, 402, 96, 30);
const BTN_CANCEL: (i32, i32, i32, i32) = (400, 402, 96, 30);

fn inside(r: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    x >= r.0 && y >= r.1 && x < r.0 + r.2 && y < r.1 + r.3
}

/// 第 i 行（当前页）的矩形。
pub fn row_rect(i: usize) -> (i32, i32, i32, i32) {
    (ROW_X, ROW_Y + i as i32 * ROW_STEP, ROW_W, ROW_H)
}

/// 原版的日期文本：`%4d/%2d/%2d %02d:%02d`。
pub fn date_text(t: &[u16; 8]) -> String {
    format!("{:4}/{:2}/{:2} {:02}:{:02}", t[0], t[1], t[3], t[4], t[5])
}

/// 未使用的格子显示这个（对应 `0x47EE84` 那串占位文本）。
pub const EMPTY_TEXT: &str = "------------/--/-- --:--";

/// 把存档里的 40 字节 BITMAPINFOHEADER + BGR 解成画布。
pub fn thumb_canvas(thumb: &[u8]) -> Option<Canvas> {
    if thumb.len() < sav::THUMB_HDR + sav::THUMB_W * sav::THUMB_H * 3 {
        return None;
    }
    let mut cv = Canvas::new(sav::THUMB_W, sav::THUMB_H);
    for y in 0..sav::THUMB_H {
        for x in 0..sav::THUMB_W {
            let s = sav::THUMB_HDR + (y * sav::THUMB_W + x) * 3;
            let d = (y * sav::THUMB_W + x) * 4;
            cv.pixels[d] = thumb[s + 2];
            cv.pixels[d + 1] = thumb[s + 1];
            cv.pixels[d + 2] = thumb[s];
            cv.pixels[d + 3] = 255;
        }
    }
    cv.opaque = true;
    Some(cv)
}

/// 把当前画面缩成 160×120 缩略图（`sav::thumb_from_rgb` 要 RGB24）。
pub fn thumb_from(cv: &Canvas) -> Vec<u8> {
    let mut rgb = vec![0u8; cv.width * cv.height * 3];
    for i in 0..cv.width * cv.height {
        rgb[i * 3] = cv.pixels[i * 4];
        rgb[i * 3 + 1] = cv.pixels[i * 4 + 1];
        rgb[i * 3 + 2] = cv.pixels[i * 4 + 2];
    }
    sav::thumb_from_rgb(&rgb, cv.width, cv.height)
}

/// 公历日期换算（Howard Hinnant 的 civil_from_days）。
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 现在的时间，写成 Win32 `SYSTEMTIME` 的 8 个 u16。
///
/// `offset_secs` 是相对 UTC 的本地时区偏移（东京 = 32400）。
pub fn now_time(offset_secs: i64) -> [u16; 8] {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + offset_secs;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 是星期四（Win32 的 wDayOfWeek：0 = 星期日）
    let dow = (days + 4).rem_euclid(7) as u16;
    [y as u16, m as u16, dow, d as u16, (rem / 3600) as u16, (rem % 3600 / 60) as u16, (rem % 60) as u16, 0]
}

impl SaveScreen {
    /// 打开游戏目录下的 `KIZUNAR.SAV`。文件不存在就是 27 个空格子。
    pub fn open(dir: &str, mode: Mode) -> SaveScreen {
        let path = format!("{}/KIZUNAR.SAV", dir);
        let file = std::fs::read(&path)
            .ok()
            .and_then(|d| sav::parse(&d).ok())
            .unwrap_or_else(SaveFile::new);
        SaveScreen { mode, page: 0, cursor: 0, file, path, message: String::new() }
    }

    pub fn slot_index(&self) -> usize {
        self.page * PER_PAGE + self.cursor
    }

    pub fn slot(&self, i: usize) -> Option<&Slot> {
        self.file.slots.get(i)
    }

    /// 命中测试。返回鼠标落在哪个控件上。
    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        for i in 0..PER_PAGE {
            if inside(row_rect(i), x, y) {
                return Some(Action::Hover(i));
            }
        }
        if inside(BTN_OK, x, y) {
            return Some(Action::Ok);
        }
        if inside(BTN_CANCEL, x, y) {
            return Some(Action::Cancel);
        }
        if inside((ROW_X, ROW_Y + PER_PAGE as i32 * ROW_STEP, 120, 24), x, y) {
            return Some(Action::Prev);
        }
        if inside((ROW_X + 130, ROW_Y + PER_PAGE as i32 * ROW_STEP, 120, 24), x, y) {
            return Some(Action::Next);
        }
        None
    }

    /// 把当前 VM 状态写进一格并落盘。
    pub fn save(&mut self, vm: &Vm, frame: &Canvas, offset_secs: i64) -> crate::Result<()> {
        let i = self.slot_index();
        let mut s = Slot::default();
        s.time = now_time(offset_secs);
        s.thumb = thumb_from(frame);
        s.title = date_text(&s.time);
        vm.fill_slot(&mut s);
        self.file.slots[i] = s;
        self.flush()?;
        self.message = format!("已保存到 データ{}", i + 1);
        Ok(())
    }

    /// 从一格读回 VM 的位数组与变量数组。
    ///
    /// 原版的存档格里**没有剧本位置**（`FUN_00426EA3` 写档时不写 PC，尾部 0x30 全是 0），
    /// 所以这里也只恢复旗标——和 PC 版行为一致。
    pub fn load(&mut self, vm: &mut Vm) -> crate::Result<()> {
        let i = self.slot_index();
        let s = self.file.slots[i].clone();
        if !s.used() {
            self.message = format!("データ{} 是空的", i + 1);
            return Ok(());
        }
        vm.apply_slot(&s);
        self.message = format!("已读取 データ{}", i + 1);
        Ok(())
    }

    /// 写回 `KIZUNAR.SAV`：先写 `SAVE.TMP` 再换名，和原版一样。
    pub fn flush(&self) -> crate::Result<()> {
        let data = sav::build(&self.file);
        let tmp = self.path.replace("KIZUNAR.SAV", "SAVE.TMP");
        std::fs::write(&tmp, &data)?;
        if std::fs::rename(&tmp, &self.path).is_err() {
            let _ = std::fs::remove_file(&self.path);
            std::fs::rename(&tmp, &self.path)?;
        }
        Ok(())
    }

    /// 画一帧。
    pub fn draw(&self, cv: &mut Canvas, font: &fontdue::Font) {
        let (px, py, pw, ph) = PANEL;
        cv.fill_rect(px, py, pw, ph, 24, 28, 36, 232);
        cv.fill_rect(px, py, pw, 26, 60, 70, 96, 255);
        let title = match self.mode {
            Mode::Save => "Save",
            Mode::Load => "Load",
        };
        cv.draw_text(font, title, (px + 8) as isize, (py + 4) as isize, 18.0, (235, 235, 240));

        for i in 0..PER_PAGE {
            let (x, y, w, h) = row_rect(i);
            let idx = self.page * PER_PAGE + i;
            let sel = i == self.cursor;
            if sel {
                cv.fill_rect(x, y, w, h, 70, 96, 150, 255);
            } else {
                cv.fill_rect(x, y, w, h, 40, 46, 58, 255);
            }
            let label = format!("データ{}", idx + 1);
            cv.draw_text(font, &label, (x + 8) as isize, (y + 5) as isize, 16.0, (230, 230, 235));
            let s = &self.file.slots[idx];
            let date = if s.used() { date_text(&s.time) } else { EMPTY_TEXT.to_string() };
            let c = if s.used() { (200, 220, 200) } else { (120, 126, 136) };
            cv.draw_text(font, &date, (x + 96) as isize, (y + 5) as isize, 16.0, c);
        }

        // 右侧预览：160×120 缩略图 + 日期
        let (vx, vy) = PREVIEW;
        cv.fill_rect(vx - 4, vy - 4, sav::THUMB_W as i32 + 8, sav::THUMB_H as i32 + 8, 60, 70, 96, 255);
        let s = &self.file.slots[self.slot_index()];
        if let Some(t) = thumb_canvas(&s.thumb) {
            cv.blit_rect(&t, 0, 0, sav::THUMB_W as i32, sav::THUMB_H as i32, vx, vy);
        }
        let date = if s.used() { date_text(&s.time) } else { "（空）".to_string() };
        cv.draw_text(font, &date, vx as isize, (vy + sav::THUMB_H as i32 + 8) as isize, 16.0, (230, 230, 235));

        let by = ROW_Y + PER_PAGE as i32 * ROW_STEP;
        cv.fill_rect(ROW_X, by, 120, 24, 40, 46, 58, 255);
        cv.draw_text(font, "◀ 上一页", (ROW_X + 8) as isize, (by + 3) as isize, 15.0, (220, 220, 226));
        cv.fill_rect(ROW_X + 130, by, 120, 24, 40, 46, 58, 255);
        cv.draw_text(font, "下一页 ▶", (ROW_X + 138) as isize, (by + 3) as isize, 15.0, (220, 220, 226));
        let page = format!("{}/{}", self.page + 1, PAGES);
        cv.draw_text(font, &page, (ROW_X + 268) as isize, (by + 3) as isize, 15.0, (170, 176, 190));

        let (ox, oy, ow, oh) = BTN_OK;
        cv.fill_rect(ox, oy, ow, oh, 70, 96, 150, 255);
        cv.draw_text(font, "确定", (ox + 26) as isize, (oy + 5) as isize, 16.0, (240, 240, 245));
        let (cx, cy, cw, ch) = BTN_CANCEL;
        cv.fill_rect(cx, cy, cw, ch, 46, 52, 66, 255);
        cv.draw_text(font, "取消", (cx + 26) as isize, (cy + 5) as isize, 16.0, (220, 220, 226));

        if !self.message.is_empty() {
            cv.draw_text(font, &self.message, (px + 8) as isize, (py + ph - 22) as isize, 14.0, (200, 210, 220));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_matches_engine_format() {
        // FUN_00426850 里的 sprintf("%4d/%2d/%2d %02d:%02d")
        let t = [2026, 10, 5, 23, 22, 31, 15, 0];
        assert_eq!(date_text(&t), "2026/10/23 22:31");
    }

    #[test]
    fn civil_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20688), (2026, 8, 23));
        assert_eq!(civil_from_days(20749), (2026, 10, 23));
    }

    #[test]
    fn hit_test_rows() {
        let s = SaveScreen {
            mode: Mode::Load,
            page: 0,
            cursor: 0,
            file: SaveFile::new(),
            path: String::new(),
            message: String::new(),
        };
        assert_eq!(s.hit(ROW_X + 5, ROW_Y + 5), Some(Action::Hover(0)));
        assert_eq!(s.hit(ROW_X + 5, ROW_Y + ROW_STEP + 5), Some(Action::Hover(1)));
        assert_eq!(s.hit(10, 10), None);
    }
}