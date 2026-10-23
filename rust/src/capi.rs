//! C ABI：给 iOS（Swift）和任何非 Rust 前端调用。
//!
//! 设计原则：Rust 侧拥有全部状态，前端只负责三件事——
//! 定时调 `kzn_tick` + `kzn_pump`、把 `kzn_frame` 的 RGBA 贴到屏幕上、
//! 把 `kzn_audio` 的 PCM 塞进音频回调。
//!
//! 字符串一律 UTF-8；返回 `c_int` 的函数，0 表示成功（或「没有内容」），负数表示错误。

use std::collections::HashMap;
use std::rc::Rc;
use std::ffi::{c_char, CStr};
use std::os::raw::c_int;

use crate::assets::Assets;
use crate::compose::Composer;
use crate::engine::{Engine, Wait};
use crate::isf::{Script, Vm};

/// 前端看到的状态码（和 `Wait` 一一对应）。
pub const KZN_RUN: c_int = 0;
pub const KZN_TEXT: c_int = 1;
pub const KZN_INPUT: c_int = 2;
pub const KZN_CHOICE: c_int = 3;
pub const KZN_HALT: c_int = 4;
pub const KZN_FAILED: c_int = 5;
/// `0xE5` 的模态对话框开着（存档 / 读档 / 设置），前端应该显示存读档界面。
pub const KZN_DIALOG: c_int = 6;
/// `0xF3` 的 MPEG 正在播（点一下跳过）。
pub const KZN_MOVIE: c_int = 7;

pub struct Kzn {
    assets: Assets,
    composer: Composer,
    engine: Option<Engine>,
    steps: u64,
    error: Option<String>,
    /// 游戏目录：`0x48` 从这里读写 `KIZUNAR.SAV`。
    dir: String,
    /// 存读档界面（打开时盖在游戏画面上）。
    save_ui: Option<crate::saveui::SaveScreen>,
    /// 本地时区相对 UTC 的秒数（东京 = 32400），存档日期用。
    offset_secs: i64,
}

fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

fn put(s: &str, out: *mut c_char, cap: c_int) -> c_int {
    if out.is_null() || cap <= 1 {
        return 0;
    }
    let bytes = s.as_bytes();
    let n = bytes.len().min(cap as usize - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, out, n);
        *out.add(n) = 0;
    }
    n as c_int
}

/// 打开游戏目录，返回句柄（失败返回 NULL）。`audio = 0` 时不加载音频归档。
#[no_mangle]
pub extern "C" fn kzn_open(dir: *const c_char, audio: c_int, font: *const c_char) -> *mut Kzn {
    let (Some(dir), Some(font)) = (cstr(dir), cstr(font)) else {
        return std::ptr::null_mut();
    };
    let Ok(assets) = Assets::open(dir, audio != 0) else {
        return std::ptr::null_mut();
    };
    // 字体路径可以写成 `xxx.ttc#2` 选集合里的第几个字面（NotoSansCJK 0 = JP）。
    let (font_path, face) = match font.rsplit_once('#') {
        Some((p, n)) => (p, n.parse::<u32>().unwrap_or(0)),
        None => (font, 0),
    };
    let Ok(font_data) = std::fs::read(font_path) else {
        return std::ptr::null_mut();
    };
    let Ok(composer) = Composer::from_bytes_idx(font_data, crate::compose::TEXT_SIZE, &assets, face) else {
        return std::ptr::null_mut();
    };
    let k = Kzn {
        assets,
        composer,
        engine: None,
        steps: 5_000_000,
        error: None,
        dir: dir.to_string(),
        save_ui: None,
        offset_secs: 9 * 3600,
    };
    Box::into_raw(Box::new(k))
}

#[no_mangle]
pub extern "C" fn kzn_free(k: *mut Kzn) {
    if !k.is_null() {
        drop(unsafe { Box::from_raw(k) });
    }
}

fn load_mods(assets: &Assets) -> HashMap<String, Rc<Script>> {
    let mut out = HashMap::new();
    let Some(arc) = assets.isf.as_ref() else { return out };
    for e in &arc.entries {
        if !e.name.to_uppercase().ends_with(".ISF") {
            continue;
        }
        if let Ok(b) = arc.bytes(e) {
            if let Ok(s) = Script::parse(&b, &assets.secret) {
                out.insert(e.name.split('.').next().unwrap_or(&e.name).to_uppercase(), Rc::new(s));
            }
        }
    }
    out
}

/// 载入剧本并开跑。返回 0 = 成功。
#[no_mangle]
pub extern "C" fn kzn_start(k: *mut Kzn, script: *const c_char) -> c_int {
    let Some(k) = (unsafe { k.as_mut() }) else { return -1 };
    let Some(name) = cstr(script) else { return -1 };
    let Some(arc) = k.assets.isf.as_ref() else {
        k.error = Some("ISF 归档没打开".into());
        return -1;
    };
    let Some(e) = arc.find(name) else {
        k.error = Some(format!("ISF 里没有 {}", name));
        return -1;
    };
    let bytes = match arc.bytes(e) {
        Ok(b) => b,
        Err(err) => {
            k.error = Some(err.to_string());
            return -1;
        }
    };
    let script = match Script::parse(&bytes, &k.assets.secret) {
        Ok(s) => s,
        Err(err) => {
            k.error = Some(err.to_string());
            return -1;
        }
    };
    let stem = name.split('.').next().unwrap_or(name).to_string();
    let mods = load_mods(&k.assets);
    let vm = Vm::new(&script, k.steps).with_modules(mods, &stem);
    k.engine = Some(Engine::new(vm).with_dir(&k.dir));
    k.error = None;
    0
}

/// 推进时间（毫秒）：打字机、特效、音频淡化都靠它。
#[no_mangle]
pub extern "C" fn kzn_tick(k: *mut Kzn, dt_ms: f64) {
    if let Some(k) = unsafe { k.as_mut() } {
        if let Some(e) = k.engine.as_mut() {
            e.tick(dt_ms);
        }
    }
}

/// 跑一批指令。前端一般每帧调一次。
#[no_mangle]
pub extern "C" fn kzn_pump(k: *mut Kzn, budget: u32) {
    if let Some(k) = unsafe { k.as_mut() } {
        // 脚本要开对话框：把界面拉起来，然后停在这里等玩家。
        // 原版是 FUN_0043F9A0 里的 DoModal，脚本线程就卡在 0xE5 上。
        if let Some(e) = k.engine.as_ref() {
            if let Some(m) = e.dialog {
                if k.save_ui.is_none() {
                    match m {
                        0 | 1 => {
                            let mode = if m == 0 {
                                crate::saveui::Mode::Save
                            } else {
                                crate::saveui::Mode::Load
                            };
                            k.save_ui = Some(crate::saveui::SaveScreen::open(&k.dir, mode));
                        }
                        _ => {
                            // mode >= 2 是设置对话框（DoModal 参数 mode-2 是页号）。
                            // 设置项还没复刻，先直接放行，别把脚本卡死。
                            if let Some(e) = k.engine.as_mut() {
                                e.close_dialog(false);
                            }
                        }
                    }
                }
                return;
            }
        }
        if k.save_ui.is_some() {
            return;
        }
        if let Some(e) = k.engine.as_mut() {
            e.pump(&k.assets, &k.assets, budget.max(1) as usize);
        }
    }
}

/// 当前状态码。
#[no_mangle]
pub extern "C" fn kzn_state(k: *mut Kzn) -> c_int {
    let Some(k) = (unsafe { k.as_ref() }) else { return -1 };
    let Some(e) = k.engine.as_ref() else { return -1 };
    match e.wait {
        Wait::Run => KZN_RUN,
        Wait::Text => KZN_TEXT,
        Wait::Input => KZN_INPUT,
        Wait::Choice(_) => KZN_CHOICE,
        Wait::Dialog(_) => KZN_DIALOG,
        Wait::Movie => KZN_MOVIE,
        Wait::Halt(_) => KZN_HALT,
        Wait::Failed(_) => KZN_FAILED,
    }
}

/// 打开存读档界面：`mode` 0 = 读档，1 = 存档。返回 1 表示已打开。
#[no_mangle]
pub extern "C" fn kzn_save_open(k: *mut Kzn, mode: c_int) -> c_int {
    let Some(k) = (unsafe { k.as_mut() }) else { return 0 };
    if k.engine.is_none() {
        return 0;
    }
    let m = if mode == 1 { crate::saveui::Mode::Save } else { crate::saveui::Mode::Load };
    k.save_ui = Some(crate::saveui::SaveScreen::open(&k.dir, m));
    1
}

/// 关闭存读档界面。
#[no_mangle]
pub extern "C" fn kzn_save_close(k: *mut Kzn) {
    if let Some(k) = unsafe { k.as_mut() } {
        k.save_ui = None;
    }
}

/// 主音量 0..1（原版设置对话框的「音量」）。写的是混音器的 master，
/// 脚本自己用 `0x71/0x72` 调的轨道音量是另一层，两者相乘。
#[no_mangle]
pub extern "C" fn kzn_set_volume(k: *mut Kzn, v: f32) -> c_int {
    let Some(k) = (unsafe { k.as_mut() }) else { return 0 };
    let Some(e) = k.engine.as_mut() else { return 0 };
    e.mixer.master = v.clamp(0.0, 1.0);
    1
}

#[no_mangle]
pub extern "C" fn kzn_volume(k: *mut Kzn) -> f32 {
    let Some(k) = (unsafe { k.as_ref() }) else { return 1.0 };
    k.engine.as_ref().map(|e| e.mixer.master).unwrap_or(1.0)
}

/// 打字速度（字符/秒，原版设置对话框的「文字速度」）。
#[no_mangle]
pub extern "C" fn kzn_set_speed(k: *mut Kzn, cps: f32) -> c_int {
    let Some(k) = (unsafe { k.as_mut() }) else { return 0 };
    let Some(e) = k.engine.as_mut() else { return 0 };
    e.speed = cps.clamp(1.0, 400.0);
    1
}

#[no_mangle]
pub extern "C" fn kzn_speed(k: *mut Kzn) -> f32 {
    let Some(k) = (unsafe { k.as_ref() }) else { return 50.0 };
    k.engine.as_ref().map(|e| e.speed).unwrap_or(50.0)
}

/// 回标题（原版右键菜单的「タイトルに戻る」）。
#[no_mangle]
pub extern "C" fn kzn_title(k: *mut Kzn) -> c_int {
    let Some(k) = (unsafe { k.as_mut() }) else { return 0 };
    let Some(e) = k.engine.as_mut() else { return 0 };
    k.save_ui = None;
    e.goto_title();
    1
}

/// 存读档界面是否开着。
#[no_mangle]
pub extern "C" fn kzn_save_active(k: *mut Kzn) -> c_int {
    let Some(k) = (unsafe { k.as_mut() }) else { return 0 };
    if k.save_ui.is_some() { 1 } else { 0 }
}

/// 鼠标移动：更新当前高亮格。
#[no_mangle]
pub extern "C" fn kzn_save_move(k: *mut Kzn, x: i32, y: i32) {
    let Some(k) = (unsafe { k.as_mut() }) else { return };
    let Some(s) = k.save_ui.as_mut() else { return };
    if let Some(crate::saveui::Action::Hover(i)) = s.hit(x, y) {
        s.cursor = i;
    }
}

/// 翻页：`delta` -1 / +1。
#[no_mangle]
pub extern "C" fn kzn_save_page(k: *mut Kzn, delta: c_int) {
    let Some(k) = (unsafe { k.as_mut() }) else { return };
    let Some(s) = k.save_ui.as_mut() else { return };
    let n = crate::saveui::PAGES as i32;
    s.page = ((s.page as i32 + delta).rem_euclid(n)) as usize;
}

/// 处理一次点击。返回 1 表示界面吃掉了这次点击（不传给剧本）。
fn save_click(k: &mut Kzn, x: i32, y: i32) -> bool {
    let mut done = false;
    // 读档确认后要重进 LOAD.ISF（原版 FUN_0043F9A0 的返回码 10 分支）。
    let mut reload = false;
    {
        let Some(s) = k.save_ui.as_mut() else { return false };
        let Some(a) = s.hit(x, y) else { return true };
        match a {
            crate::saveui::Action::Hover(i) => {
                s.cursor = i;
            }
            crate::saveui::Action::Prev => {
                s.page = (s.page + crate::saveui::PAGES - 1) % crate::saveui::PAGES;
            }
            crate::saveui::Action::Next => {
                s.page = (s.page + 1) % crate::saveui::PAGES;
            }
            crate::saveui::Action::Cancel => {
                done = true;
            }
            crate::saveui::Action::Ok | crate::saveui::Action::Pick(_) => {
                if let crate::saveui::Action::Pick(i) = a {
                    s.cursor = i;
                }
                match s.mode {
                    crate::saveui::Mode::Save => {
                        let Some(e) = k.engine.as_ref() else { return true };
                        let (raw, shown) = match e.line.as_ref() {
                            Some(l) if l.total > 0 => (Some(&l.raw[..]), Some(l.shown as usize)),
                            _ => (None, None),
                        };
                        let frame = k.composer.compose(&e.player, raw, shown);
                        let off = k.offset_secs;
                        let vm = &e.vm;
                        match s.save(vm, &frame, off) {
                            // 存档不重进剧本：原版的返回码 10 只在读档时出现。
                            Ok(()) => done = true,
                            Err(err) => s.message = format!("保存失败：{}", err),
                        }
                    }
                    crate::saveui::Mode::Load => {
                        let Some(e) = k.engine.as_mut() else { return true };
                        match s.load(&mut e.vm) {
                            Ok(()) => {
                                done = true;
                                reload = true;
                            }
                            Err(err) => s.message = format!("读取失败：{}", err),
                        }
                    }
                }
            }
        }
    }
    if done {
        k.save_ui = None;
        if let Some(e) = k.engine.as_mut() {
            e.close_dialog(reload);
        }
    }
    true
}
#[no_mangle]
pub extern "C" fn kzn_frame(k: *mut Kzn, out: *mut u8) -> c_int {
    let Some(k) = (unsafe { k.as_mut() }) else { return -1 };
    let Some(e) = k.engine.as_ref() else { return -1 };
    if out.is_null() {
        return -1;
    }
    let (raw, shown) = match e.line.as_ref() {
        Some(l) if l.total > 0 => (Some(&l.raw[..]), Some(l.shown as usize)),
        _ => (None, None),
    };
    let mut cv = k.composer.compose(&e.player, raw, shown);
    // 0xF3 的片子盖在最上面（原版是 DirectShow 自己的窗口，永远在前）。
    e.composite_movie(&mut cv);
    if let Some(s) = k.save_ui.as_ref() {
        s.draw(&mut cv, &k.composer.font);
    }
    let n = cv.pixels.len();
    unsafe { std::ptr::copy_nonoverlapping(cv.pixels.as_ptr(), out, n) };
    n as c_int
}

/// 鼠标移动（热区高亮用）。
#[no_mangle]
pub extern "C" fn kzn_mouse(k: *mut Kzn, x: i32, y: i32) {
    if let Some(k) = unsafe { k.as_mut() } {
        if let Some(e) = k.engine.as_mut() {
            e.st.borrow_mut().mouse = (x, y);
        }
    }
}

/// 点一下。返回 1 = 推进了（或选了选项），0 = 没反应。
#[no_mangle]
pub extern "C" fn kzn_click(k: *mut Kzn, x: i32, y: i32) -> c_int {
    let Some(k) = (unsafe { k.as_mut() }) else { return 0 };
    if k.save_ui.is_some() {
        return if save_click(k, x, y) { 1 } else { 0 };
    }
    let Some(e) = k.engine.as_mut() else { return 0 };
    if e.click(x, y, &k.assets, &k.assets) { 1 } else { 0 }
}

/// 取 `frames` 帧立体声 PCM（交错 i16）。返回写入的采样数。
#[no_mangle]
pub extern "C" fn kzn_audio(k: *mut Kzn, out: *mut i16, frames: u32, rate: u32) -> c_int {
    let Some(k) = (unsafe { k.as_mut() }) else { return -1 };
    let Some(e) = k.engine.as_mut() else { return -1 };
    if out.is_null() {
        return -1;
    }
    let pcm = e.mixer.render(frames as usize, rate.max(8000), 2);
    let n = pcm.len();
    unsafe { std::ptr::copy_nonoverlapping(pcm.as_ptr(), out, n) };
    n as c_int
}

/// 当前台词（已显示的部分），UTF-8。返回字节数。
#[no_mangle]
pub extern "C" fn kzn_text(k: *mut Kzn, out: *mut c_char, cap: c_int) -> c_int {
    let Some(k) = (unsafe { k.as_ref() }) else { return 0 };
    let Some(e) = k.engine.as_ref() else { return 0 };
    match e.line.as_ref() {
        Some(l) => put(&l.visible(), out, cap),
        None => put("", out, cap),
    }
}

/// 第 w 个选项窗打开了几个选项。
#[no_mangle]
pub extern "C" fn kzn_choice_count(k: *mut Kzn, w: c_int) -> c_int {
    let Some(k) = (unsafe { k.as_ref() }) else { return 0 };
    let Some(e) = k.engine.as_ref() else { return 0 };
    e.player.choices.get(w as usize).map(|c| c.open as c_int).unwrap_or(0)
}

/// 第 w 个窗口第 i 个选项的文字。
#[no_mangle]
pub extern "C" fn kzn_choice_text(k: *mut Kzn, w: c_int, i: c_int, out: *mut c_char, cap: c_int) -> c_int {
    let Some(k) = (unsafe { k.as_ref() }) else { return 0 };
    let Some(e) = k.engine.as_ref() else { return 0 };
    match e.player.choices.get(w as usize).and_then(|c| c.text_at(i as usize)) {
        Some((_, _, _, _, t)) => put(t, out, cap),
        None => put("", out, cap),
    }
}

/// 最近一次错误（UTF-8）。
#[no_mangle]
pub extern "C" fn kzn_error(k: *mut Kzn, out: *mut c_char, cap: c_int) -> c_int {
    let Some(k) = (unsafe { k.as_ref() }) else { return 0 };
    match k.error.as_deref() {
        Some(s) => put(s, out, cap),
        None => put("", out, cap),
    }
}

/// 台词总字符数（打字机进度用）。
#[no_mangle]
pub extern "C" fn kzn_text_total(k: *mut Kzn) -> c_int {
    let Some(k) = (unsafe { k.as_ref() }) else { return 0 };
    k.engine.as_ref().and_then(|e| e.line.as_ref()).map(|l| l.total as c_int).unwrap_or(0)
}

/// 第 w 个窗口第 i 个选项在屏幕上的矩形（x, y, w, h）。返回 1 = 有。
#[no_mangle]
pub extern "C" fn kzn_choice_rect(k: *mut Kzn, w: c_int, i: c_int, out: *mut i32) -> c_int {
    let Some(k) = (unsafe { k.as_ref() }) else { return 0 };
    let Some(e) = k.engine.as_ref() else { return 0 };
    if out.is_null() {
        return 0;
    }
    match e.player.choice_rect(w as usize, i as usize) {
        Some((x, y, rw, rh)) => {
            unsafe {
                *out = x;
                *out.add(1) = y;
                *out.add(2) = rw;
                *out.add(3) = rh;
            }
            1
        }
        None => 0,
    }
}
