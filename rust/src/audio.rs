//! 音频：Ogg Vorbis 解码 + 三路混音（BGM / 语音 / SE）。
//!
//! 归档里所有音频都是标准 Ogg Vorbis（44.1kHz 立体声），没有私有编码，
//! 所以这里只做「解码成 PCM」和「按引擎的语义混音」两件事。
//! 输出层（CoreAudio / cpal）不在这里，保持核心可移植。

use std::sync::Arc;

/// 一段解码好的 PCM（交错排列，i16）。
pub struct Track {
    pub rate: u32,
    pub channels: usize,
    pub samples: Vec<i16>,
    /// 帧数（一帧 = 所有声道各一个采样）。
    pub frames: usize,
}

impl Track {
    pub fn duration(&self) -> f64 {
        self.frames as f64 / self.rate as f64
    }

    /// 取第 frame 帧的第 ch 个声道；单声道会复制到所有声道。
    #[inline]
    fn at(&self, frame: usize, ch: usize) -> f32 {
        if self.channels == 1 {
            self.samples[frame] as f32 / 32768.0
        } else {
            self.samples[frame * self.channels + ch.min(self.channels - 1)] as f32 / 32768.0
        }
    }
}

/// 解码一整段 Ogg Vorbis。
pub fn decode_ogg(data: &[u8]) -> Result<Track, String> {
    let mut reader = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(data))
        .map_err(|e| format!("ogg: {e}"))?;
    let rate = reader.ident_hdr.audio_sample_rate;
    let channels = reader.ident_hdr.audio_channels as usize;
    let mut samples = Vec::new();
    while let Some(pkt) = reader.read_dec_packet_itl().map_err(|e| format!("vorbis: {e}"))? {
        samples.extend_from_slice(&pkt);
    }
    let frames = if channels > 0 { samples.len() / channels } else { 0 };
    Ok(Track { rate, channels, samples, frames })
}

/// 淡入淡出状态。引擎的 BGM 切换都是「N 毫秒内线性过渡」。
#[derive(Clone, Copy)]
struct Fade {
    from: f32,
    to: f32,
    total: u32,
    done: u32,
}

/// 一路正在播放的音源。
pub struct Voice {
    pub track: Arc<Track>,
    /// 当前帧位置（浮点，便于重采样）。
    pos: f64,
    /// 基础音量 0..=1（引擎 0..=100 → 除 100）。
    pub volume: f32,
    /// 是否循环（BGM）。
    pub looping: bool,
    fade: Option<Fade>,
    /// 播到第几帧算结束。
    pub stop_at: Option<usize>,
    pub done: bool,
}

impl Voice {
    pub fn new(track: Arc<Track>, volume: f32, looping: bool) -> Voice {
        Voice { track, pos: 0.0, volume, looping, fade: None, stop_at: None, done: false }
    }

    /// 从当前音量线性过渡到 to，用 ms 毫秒。
    pub fn fade_to(&mut self, to: f32, ms: u32) {
        let frames = (ms as f64 * self.track.rate as f64 / 1000.0) as u32;
        self.fade = Some(Fade { from: self.volume, to, total: frames.max(1), done: 0 });
    }

    fn current_volume(&mut self) -> f32 {
        if let Some(f) = self.fade.as_mut() {
            f.done = f.done.saturating_add(1);
            if f.done >= f.total {
                self.volume = f.to;
                self.fade = None;
            } else {
                let t = f.done as f32 / f.total as f32;
                self.volume = f.from + (f.to - f.from) * t;
            }
        }
        self.volume
    }

    /// 混入 out（交错，chans 声道），out_rate 为输出采样率。
    fn mix_into(&mut self, out: &mut [i16], out_rate: u32, chans: usize) {
        if self.done {
            return;
        }
        let step = self.track.rate as f64 / out_rate as f64;
        let frames = out.len() / chans;
        for i in 0..frames {
            if let Some(limit) = self.stop_at {
                if self.pos as usize >= limit {
                    self.done = true;
                    break;
                }
            }
            if self.pos as usize >= self.track.frames {
                if self.looping && self.track.frames > 0 {
                    self.pos -= self.track.frames as f64;
                } else {
                    self.done = true;
                    break;
                }
            }
            let vol = self.current_volume();
            // 线性插值，避免 44.1k→48k 时的锯齿
            let f0 = self.pos.floor() as usize;
            let frac = (self.pos - f0 as f64) as f32;
            let f1 = if f0 + 1 < self.track.frames { f0 + 1 } else { f0 };
            for c in 0..chans {
                let a = self.track.at(f0, c);
                let b = self.track.at(f1, c);
                let v = (a + (b - a) * frac) * vol;
                let cur = out[i * chans + c] as f32 / 32768.0 + v;
                out[i * chans + c] = (cur.clamp(-1.0, 1.0) * 32767.0) as i16;
            }
            self.pos += step;
        }
    }
}

/// 混音器：引擎只有 BGM 一路、语音一路、SE 若干路。
#[derive(Default)]
pub struct Mixer {
    pub bgm: Option<Voice>,
    /// 正在淡出的旧 BGM（切换时交叉淡化用）。
    pub bgm_old: Option<Voice>,
    pub voice: Option<Voice>,
    pub se: Vec<Voice>,
    pub master: f32,
}

impl Mixer {
    pub fn new() -> Mixer {
        Mixer { master: 1.0, ..Default::default() }
    }

    /// 换 BGM；fade_ms > 0 时旧曲淡出、新曲淡入。
    pub fn play_bgm(&mut self, track: Arc<Track>, looping: bool, fade_ms: u32) {
        if let Some(old) = self.bgm.take() {
            if fade_ms > 0 {
                let mut old = old;
                old.looping = false;
                old.fade_to(0.0, fade_ms);
                let n = old.pos as usize + fade_ms as usize * old.track.rate as usize / 1000;
                old.stop_at = Some(n);
                self.bgm_old = Some(old);
            }
        }
        let mut v = Voice::new(track, if fade_ms > 0 { 0.0 } else { 1.0 }, looping);
        if fade_ms > 0 {
            v.fade_to(1.0, fade_ms);
        }
        self.bgm = Some(v);
    }

    pub fn stop_bgm(&mut self, fade_ms: u32) {
        if let Some(mut v) = self.bgm.take() {
            if fade_ms == 0 {
                return;
            }
            v.looping = false;
            v.fade_to(0.0, fade_ms);
            let n = v.pos as usize + fade_ms as usize * v.track.rate as usize / 1000;
            v.stop_at = Some(n);
            self.bgm_old = Some(v);
        }
    }

    pub fn play_voice(&mut self, track: Arc<Track>) {
        self.voice = Some(Voice::new(track, 1.0, false));
    }

    pub fn stop_voice(&mut self) {
        self.voice = None;
    }

    pub fn play_se(&mut self, track: Arc<Track>, volume: f32) {
        self.se.push(Voice::new(track, volume, false));
    }

    /// 生成 frames 帧的 PCM（交错 chans 声道）。
    pub fn render(&mut self, frames: usize, out_rate: u32, chans: usize) -> Vec<i16> {
        let mut out = vec![0i16; frames * chans];
        if let Some(v) = self.bgm.as_mut() {
            v.mix_into(&mut out, out_rate, chans);
        }
        if let Some(v) = self.bgm_old.as_mut() {
            v.mix_into(&mut out, out_rate, chans);
            if v.done {
                self.bgm_old = None;
            }
        }
        if let Some(v) = self.voice.as_mut() {
            v.mix_into(&mut out, out_rate, chans);
            if v.done {
                self.voice = None;
            }
        }
        for v in self.se.iter_mut() {
            v.mix_into(&mut out, out_rate, chans);
        }
        self.se.retain(|v| !v.done);
        if self.master != 1.0 {
            for s in out.iter_mut() {
                *s = (*s as f32 * self.master) as i16;
            }
        }
        out
    }
}

/// 写一个最小 WAV（16bit PCM），给验证用。
pub fn write_wav(path: &str, samples: &[i16], rate: u32, chans: usize) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data_len = (samples.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&(chans as u16).to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * chans as u32 * 2).to_le_bytes())?;
    f.write_all(&((chans * 2) as u16).to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}