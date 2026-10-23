//! 资源集合：把游戏目录里的归档按名字查出来，给渲染层和音频层共用。

use crate::engine::AudioSource;
use crate::ggd;
use crate::render::ImageSource;
use crate::sm2mpx::Archive;

/// 一个游戏目录里所有归档。除 `GGD` 外都允许缺失（试玩版/删减版）。
pub struct Assets {
    pub ggd: Archive,
    pub ggd2: Option<Archive>,
    /// 特效蒙版（`EFF*.BIN` / `SEF*.BIN`）。
    pub data: Option<Archive>,
    pub isf: Option<Archive>,
    pub se: Option<Archive>,
    pub voice1: Option<Archive>,
    pub voice2: Option<Archive>,
    pub wmsc: Option<Archive>,
    /// ISF 的异或密钥表（从 KIZUNAR.EXE 里找 "UOB0" 算出来）。
    pub secret: Vec<u8>,
}

impl Assets {
    /// 打开一个游戏目录。`audio = false` 时不碰音频归档（省启动时间）。
    pub fn open(dir: &str, audio: bool) -> crate::Result<Assets> {
        let o = |n: &str| Archive::open(&format!("{}/{}", dir, n)).ok();
        let exe = std::fs::read(format!("{}/KIZUNAR.EXE", dir))?;
        let secret = crate::isf::find_secret(&exe)?.to_vec();
        Ok(Assets {
            secret,
            ggd: Archive::open(&format!("{}/GGD", dir))?,
            ggd2: o("GGD2"),
            data: o("DATA"),
            isf: o("ISF"),
            se: if audio { o("SE") } else { None },
            voice1: if audio { o("VOICE1") } else { None },
            voice2: if audio { o("VOICE2") } else { None },
            wmsc: if audio { o("WMSC") } else { None },
        })
    }

    pub fn arc(&self, name: &str) -> Option<&Archive> {
        match name {
            "GGD" => Some(&self.ggd),
            "GGD2" => self.ggd2.as_ref(),
            "DATA" => self.data.as_ref(),
            "ISF" => self.isf.as_ref(),
            "SE" => self.se.as_ref(),
            "VOICE1" => self.voice1.as_ref(),
            "VOICE2" => self.voice2.as_ref(),
            "WMSC" => self.wmsc.as_ref(),
            _ => None,
        }
    }
}

impl ImageSource for Assets {
    fn data(&self, name: &str) -> Option<Vec<u8>> {
        let a = self.data.as_ref()?;
        let e = a.find(name).or_else(|| a.find_stem(name))?;
        a.bytes(e).ok()
    }

    fn image(&self, name: &str) -> Option<ggd::Image> {
        for arc in [Some(&self.ggd), self.ggd2.as_ref()].into_iter().flatten() {
            if let Some(e) = arc.find(name).or_else(|| arc.find_stem(name)) {
                return arc.bytes(e).ok().and_then(|b| ggd::decode(&b).ok());
            }
        }
        None
    }
}

impl AudioSource for Assets {
    fn ogg(&self, archive: &str, name: &str) -> Option<Vec<u8>> {
        let a = self.arc(archive)?;
        let e = a.find(name).or_else(|| a.find_stem(name))?;
        a.bytes(e).ok()
    }
}
