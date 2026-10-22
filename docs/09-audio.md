# 音频

## 结论：全是标准 Ogg Vorbis

| 归档 | 数量 | 命名 | 参数 |
|---|---|---|---|
| WMSC | 27 | `02.OGG` … `27.OGG`、`MUON.OGG` | 44100 Hz 立体声，BGM |
| SE | 136 | `SE001.OGG` … | 22050 Hz 单声道，音效 |
| VOICE1 | 11215 | `ANA0001.OGG` …（角色前缀） | 22050 Hz 单声道，语音 |
| VOICE2 | 11081 | `KDM0001.OGG` … | 22050 Hz 单声道，语音 |

没有私有编码、没有加密、没有 ADPCM——`DATA` 里那 3 个 `KA_*.BIN` 是 RIFF/ACM ADPCM，
但只是 UI 音（打字声、自动模式提示音、返回音），可以直接用系统解码器或者手写 IMA/MS ADPCM。

所以音频这块**不需要逆向**，只需要「解码 + 按引擎语义混音」。

## 混音模型

引擎的音频只有四类：

- **BGM**：单路，可循环，切换时有淡入淡出
- **语音**：单路，新语音打断旧语音
- **SE**：多路叠加（有 slot 概念，`0x74` 载入到 slot、`0x75` 播放、`0x76` 停止、`0x7B` 循环）
- **主音量**

`rust/src/audio.rs` 实现了 `Mixer`：`bgm` / `bgm_old`（交叉淡化）/ `voice` / `se: Vec<Voice>`，
`render(frames, out_rate, chans) -> Vec<i16>` 拉模式输出，方便以后直接接到 CoreAudio 的
render callback 上（iOS 端只负责喂缓冲区，核心不含任何平台代码）。

重采样用线性插值：源是 22050/44100，输出通常是 48000，不插值会有明显锯齿。

## 指令对照

| 指令 | 事件 | 语义 |
|---|---|---|
| `0x70` | `Bgm` | 播放 BGM（带模式：循环 / 单次） |
| `0x71` | `BgmVolume` | 循环标志 + 音量 |
| `0x72` | `BgmLevel` | 淡入淡出电平 |
| `0x73` | — | 停止 BGM（`BgmStop`） |
| `0x74` | `SeLoad` | 载入 SE 到 slot |
| `0x75` | `SePlay` | 播放 slot |
| `0x76` | `SeStop` | 停止 slot |
| `0x77` | `Voice` | 播放语音 |
| `0x7B` | `SeRepeat` | SE 循环 |

## 验证

```
kzn audio <归档> <成员> [--wav out.wav]
```

实测：

```
02.OGG: 324.61 秒  44100 Hz  2 声道  峰值 32768
SE001.OGG: 21.97 秒  22050 Hz  1 声道  峰值 2960
ANA0001.OGG: 5.02 秒  22050 Hz  1 声道  峰值 7431
```