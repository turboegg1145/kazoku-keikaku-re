//! 主循环：把虚拟机、渲染层、音频混音器串成一台能跑的机器。
//!
//! 引擎的节奏是「帧驱动」：每帧跑一批指令，遇到要等玩家的地方就交还控制权。
//! 等待的判定完全来自 VM 自己的 `blocked` 标记（对应引擎里没调 `FUN_00434EB0`），
//! 所以这里不需要再猜脚本的意图。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::audio::{decode_ogg, Mixer, Track};
use crate::isf::{Event, Halt, Host, Vm};
use crate::render::{Canvas, ImageSource, Player};

/// 每帧最多执行多少条指令。脚本里的等待是「循环轮询」，必须有上限兜底。
pub const STEP_BUDGET: usize = 20_000;

/// 鼠标/键盘状态。主循环每帧写，VM 的 `0x85` 通过 Host 读。
#[derive(Default)]
pub struct InputState {
    /// 当前光标位置（屏幕坐标）。
    pub mouse: (i32, i32),
    /// 本帧有没有按下左键（被 `0x85` 读走后清掉）。
    pub clicked: bool,
    /// 按住 Ctrl（引擎里是「快进」）。
    pub ctrl: bool,
    /// 毫秒时钟（`0xFC` 读它）。
    pub clock: i32,
    /// 热区矩形（屏幕坐标）。
    pub spots: Vec<(i32, i32, i32, i32)>,
}

impl InputState {
    /// 光标命中哪个热区：-2 = 画面外，-1 = 没命中，>=0 = 热区号。
    pub fn hit(&self) -> i32 {
        let (mx, my) = self.mouse;
        if mx < 0 || my < 0 || mx >= 640 || my >= 480 {
            return -2;
        }
        for (i, r) in self.spots.iter().enumerate() {
            if mx >= r.0 && my >= r.1 && mx < r.0 + r.2 && my < r.1 + r.3 {
                return i as i32;
            }
        }
        -1
    }
}

/// 给 VM 用的宿主：把输入状态桥接给 `0x85` / `0xFC`。
pub struct EngineHost {
    pub st: Rc<RefCell<InputState>>,
}

impl Host for EngineHost {
    fn poll_input(&mut self, _mode: u8, _flags: u8) -> (i32, i32) {
        let mut s = self.st.borrow_mut();
        if !s.clicked {
            return (-1, 0);
        }
        // 一次点击只算一次，避免脚本在一帧里循环轮询时被重复消费。
        s.clicked = false;
        (s.hit(), 2)
    }

    fn now_ms(&mut self) -> i32 {
        self.st.borrow().clock
    }
}

/// 音频资源来源（归档读取）。
pub trait AudioSource {
    /// `archive` 取 "WMSC" / "SE" / "VOICE1" / "VOICE2"。
    fn ogg(&self, archive: &str, name: &str) -> Option<Vec<u8>>;
}

/// 当前这句台词。
#[derive(Clone, Debug)]
pub struct Line {
    pub speaker: Option<u8>,
    pub text: String,
    pub voice: Option<String>,
    /// 已经显示的字符数（浮点，按时间累积）。
    pub shown: f32,
    /// 总字符数。
    pub total: usize,
}

impl Line {
    pub fn new(speaker: Option<u8>, text: String, voice: Option<String>) -> Line {
        let total = text.chars().count();
        Line { speaker, text, voice, shown: 0.0, total }
    }

    pub fn done(&self) -> bool {
        self.shown >= self.total as f32
    }

    /// 已经显示出来的部分。
    pub fn visible(&self) -> String {
        self.text.chars().take(self.shown as usize).collect()
    }

    pub fn finish(&mut self) {
        self.shown = self.total as f32;
    }
}

/// 主循环停在哪儿。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wait {
    /// 还能继续跑（本帧指令预算用完）。
    Run,
    /// 台词在逐字显示，VM 已经阻塞。
    Text,
    /// 等一次点击推进（没有选项窗）。
    Input,
    /// 有选项窗打开，等玩家点某一项。
    Choice(usize),
    /// 脚本结束。
    Halt(Halt),
    /// VM 内部错误（长度/越界/标签）。
    Failed(String),
}

pub struct Engine {
    pub vm: Vm,
    pub player: Player,
    pub mixer: Mixer,
    pub st: Rc<RefCell<InputState>>,
    /// 毫秒时钟。
    pub clock: f64,
    pub wait: Wait,
    /// 当前台词。
    pub line: Option<Line>,
    /// 出现过的所有台词（按顺序，给日志/回顾用）。
    pub log: Vec<Line>,
    /// 出现过的所有选项窗：(窗口号, 选项文本)。
    pub choice_log: Vec<(usize, Vec<String>)>,
    /// 打字速度（字符/秒）。引擎默认值未确认，这里取 50。
    pub speed: f32,
    /// 已出现过的台词数 / 选项数。
    pub lines: usize,
    pub choices: usize,
    /// 结束原因。
    pub halt: Option<Halt>,
    /// `0x78` 登记的语音名，会附到下一句台词上。
    pub pending_voice: Option<String>,
    /// `0x74` 装进各声道的 SE 名字。
    pub se_names: HashMap<i32, String>,
    /// 语音用哪个归档（`0x84` 之类会切）。
    pub voice_archive: String,
    /// 解码缓存。
    cache: HashMap<String, Arc<Track>>,
    /// 主音量（0..=1）。
    pub volume: f32,
    /// 连续多少帧「跑了指令但没有任何事件」。用来识别脚本的空转等待。
    pub idle: u32,
}

impl Engine {
    pub fn new(mut vm: Vm) -> Engine {
        let st = Rc::new(RefCell::new(InputState::default()));
        vm.host = Box::new(EngineHost { st: st.clone() });
        vm.block_on_input = true;
        // 消息窗口本身就是一个热区（原版里点它推进台词）。
        {
            let (mx, my) = Player::new().msg_origin();
            st.borrow_mut().spots = vec![(mx, my, 552, 152)];
        }
        Engine {
            vm,
            player: Player::new(),
            mixer: Mixer::new(),
            st,
            clock: 0.0,
            wait: Wait::Run,
            line: None,
            log: Vec::new(),
            choice_log: Vec::new(),
            speed: 50.0,
            lines: 0,
            choices: 0,
            halt: None,
            pending_voice: None,
            se_names: HashMap::new(),
            voice_archive: "VOICE1".to_string(),
            cache: HashMap::new(),
            volume: 1.0,
            idle: 0,
        }
    }

    /// 推进时间（毫秒）。台词打字靠它。
    pub fn tick(&mut self, dt_ms: f64) {
        self.clock += dt_ms;
        self.st.borrow_mut().clock = self.clock as i32;
        self.player.tick(dt_ms);
        if let Wait::Text = self.wait {
            if let Some(l) = self.line.as_mut() {
                l.shown += self.speed * (dt_ms as f32) / 1000.0;
                if l.done() {
                    l.finish();
                    self.wait = self.blocked_wait();
                }
            }
        }
    }

    /// VM 阻塞时该报哪种等待。
    fn blocked_wait(&self) -> Wait {
        if let Some(l) = self.line.as_ref() {
            if !l.done() {
                return Wait::Text;
            }
        }
        for (w, cw) in self.player.choices.iter().enumerate() {
            if cw.open > 0 {
                return Wait::Choice(w);
            }
        }
        Wait::Input
    }

    /// 跑一帧：最多 `budget` 条指令，遇到阻塞就停。
    pub fn pump(&mut self, src: &dyn ImageSource, media: &dyn AudioSource, budget: usize) {
        if let Wait::Halt(_) = self.wait {
            return;
        }
        if self.vm.blocked {
            self.wait = self.blocked_wait();
            return;
        }
        let mut n = 0;
        let mut idle = 0u32;
        while n < budget {
            match self.vm.step() {
                Ok(Some(h)) => {
                    self.halt = Some(h.clone());
                    self.wait = Wait::Halt(h);
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    self.wait = Wait::Failed(e.to_string());
                    return;
                }
            }
            n += 1;
            let before = self.lines + self.choices;
            self.drain(src, media);
            if self.lines + self.choices != before {
                idle = 0;
                self.idle = 0;
            }
            if self.vm.blocked {
                self.wait = self.blocked_wait();
                return;
            }
            if self.vm.poll_wait {
                idle += 1;
                // 连续轮询却没有任何新事件：脚本在等玩家，把控制权交出去。
                if idle >= 32 {
                    self.wait = self.blocked_wait();
                    return;
                }
            }
        }
        if idle == 0 {
            self.idle = self.idle.saturating_add(1);
        }
        self.wait = Wait::Run;
    }

    /// 把 VM 这一批事件分发给渲染层和音频层。
    fn drain(&mut self, src: &dyn ImageSource, media: &dyn AudioSource) {
        let events = self.vm.drain_events();
        for ev in events {
            match &ev {
                Event::Text { speaker, text, voice, .. } => {
                    self.lines += 1;
                    let l = Line::new(*speaker, text.clone(), voice.clone());
                    self.log.push(l.clone());
                    self.line = Some(l);
                    if let Some(v) = voice.clone() {
                        self.play_voice(media, &v);
                    }
                }
                Event::Voice(name) => {
                    self.pending_voice = Some(name.clone());
                }
                Event::VoiceArchive(a) => {
                    self.voice_archive = a.clone();
                }
                Event::Bgm { name, mode } => self.play_bgm(media, name, *mode),
                Event::BgmVolume { looping, volume } => {
                    if let Some(v) = self.mixer.bgm.as_mut() {
                        v.looping = *looping;
                        v.volume = (*volume as f32 / 100.0).clamp(0.0, 1.0) * self.volume;
                    }
                }
                Event::BgmLevel(v) => {
                    if *v == 0 {
                        self.mixer.stop_bgm(300);
                    } else if let Some(b) = self.mixer.bgm.as_mut() {
                        b.volume = (*v as f32 / 100.0).clamp(0.0, 1.0) * self.volume;
                    }
                }
                Event::BgmStop => self.mixer.stop_bgm(300),
                Event::SeLoad { name, slot } => {
                    self.se_names.insert(*slot, name.clone());
                }
                Event::SePlay { slot, .. } => {
                    if let Some(name) = self.se_names.get(slot).cloned() {
                        if let Some(t) = self.load(media, "SE", &name) {
                            self.mixer.play_se(*slot, t, self.volume);
                        }
                    }
                }
                Event::SeStop { slot } => self.mixer.stop_se(*slot),
                Event::SeRepeat { slot, value } => {
                    if *value == 0 {
                        self.mixer.stop_se(*slot);
                    }
                }
                Event::VoicePlay(v) => {
                    // 0x77：0 = 停，其它 = 继续/开始
                    if *v == 0 {
                        self.mixer.stop_voice();
                    }
                }
                Event::VoiceParam { value } => {
                    if *value == 0 {
                        self.mixer.stop_voice();
                    }
                }
                Event::ChoiceOpen { window, count, .. } => {
                    self.choices += 1;
                    self.player.apply(&ev, src);
                    let w = *window as usize;
                    if let Some(cw) = self.player.choices.get(w) {
                        let opts: Vec<String> = (0..(*count as usize).min(cw.items.len()))
                            .filter_map(|i| cw.text_at(i).map(|(_, _, _, _, t)| t.to_string()))
                            .collect();
                        self.choice_log.push((w, opts));
                    }
                }
                _ => {
                    self.player.apply(&ev, src);
                }
            }
        }
    }

    fn play_bgm(&mut self, media: &dyn AudioSource, name: &str, mode: u8) {
        if let Some(t) = self.load(media, "WMSC", name) {
            let looping = mode != 1;
            self.mixer.play_bgm(t, looping, 0);
        }
    }

    fn play_voice(&mut self, media: &dyn AudioSource, name: &str) {
        let arch = self.voice_archive.clone();
        if let Some(t) = self.load(media, &arch, name) {
            self.mixer.play_voice(t);
        }
    }

    fn load(&mut self, media: &dyn AudioSource, archive: &str, name: &str) -> Option<Arc<Track>> {
        let mut key = name.to_string();
        if !key.to_ascii_uppercase().ends_with(".OGG") {
            key.push_str(".OGG");
        }
        let cache_key = format!("{}/{}", archive, key);
        if let Some(t) = self.cache.get(&cache_key) {
            return Some(t.clone());
        }
        let bytes = media.ogg(archive, &key)?;
        match decode_ogg(&bytes) {
            Ok(t) => {
                let t = Arc::new(t);
                self.cache.insert(cache_key, t.clone());
                Some(t)
            }
            Err(_) => None,
        }
    }

    /// 玩家点了一下。返回是否被消费。
    pub fn click(&mut self, x: i32, y: i32, src: &dyn ImageSource, media: &dyn AudioSource) -> bool {
        {
            let mut s = self.st.borrow_mut();
            s.mouse = (x, y);
            s.clicked = true;
        }
        match self.wait.clone() {
            Wait::Text => {
                if let Some(l) = self.line.as_mut() {
                    l.finish();
                }
                self.wait = self.blocked_wait();
                true
            }
            Wait::Choice(w) => {
                let _ = w;
                match self.player.hit_choice(x, y) {
                    Some((_, i)) => {
                        // 引擎里点击先更新高亮（win+0x181C），再由 0x18 的确认模式写进变量。
                        self.vm.hover = i as i32;
                        self.vm.unblock();
                        self.pump(src, media, STEP_BUDGET);
                        true
                    }
                    None => false,
                }
            }
            Wait::Input => {
                if self.vm.blocked {
                    self.vm.unblock();
                } else {
                    self.st.borrow_mut().clicked = true;
                }
                self.pump(src, media, STEP_BUDGET);
                true
            }
            _ => false,
        }
    }

    /// 无人值守推进：台词直接显示完，选项永远选第 0 项。
    /// 返回 false 表示已经没东西可推进了。
    pub fn auto_advance(&mut self, src: &dyn ImageSource, media: &dyn AudioSource) -> bool {
        match self.wait.clone() {
            Wait::Text => {
                if let Some(l) = self.line.as_mut() {
                    l.finish();
                }
                self.wait = self.blocked_wait();
                true
            }
            Wait::Input => {
                if self.vm.blocked {
                    self.vm.unblock();
                } else {
                    let mut s = self.st.borrow_mut();
                    s.mouse = (336, 396);
                    s.clicked = true;
                }
                self.pump(src, media, STEP_BUDGET);
                true
            }
            Wait::Choice(_) => {
                self.vm.hover = 0;
                self.vm.unblock();
                self.pump(src, media, STEP_BUDGET);
                true
            }
            Wait::Run => {
                // 本帧预算跑完了但 VM 还没阻塞（过场、动画循环）：继续跑。
                // 连续空转说明脚本在等「鼠标移上去」这类外部状态，
                // 原版靠消息循环在帧之间更新，这里直接模拟成「光标停在第 0 项」。
                if self.idle >= 1 && self.vm.hover < 0 {
                    self.vm.hover = 0;
                }
                let before = self.vm.steps;
                self.pump(src, media, STEP_BUDGET);
                self.vm.steps != before
            }
            _ => false,
        }
    }

    /// 当前画面（还没画文字窗）。
    pub fn frame(&self) -> Canvas {
        self.player.frame()
    }
}