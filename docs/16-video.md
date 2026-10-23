# 播片（`0xF3` / MPEG-1）

## 为什么不能用系统解码器

原版是 Win32 + DirectShow 播 MPEG-1（`FUN_0043D890`）。iOS 的 AVFoundation
**完全不认 MPEG-1**（连 MPEG-2 都不行），所以只有两条路：

1. 打包时把 `.MPG` 转成 H.264 —— 但片源在玩家自己的游戏目录里，CI 拿不到，
   而且转完要额外占几百 MB，和「原版文件原样放」的目标也冲突。
2. **自带解码器** —— 选这条。

用的是 [pl_mpeg](https://github.com/phoboslab/pl_mpeg)（Dominic Szablewski，MIT，
单文件 C，4400 行）：MPEG-PS 解复用 + MPEG-1 视频 + MP2 音频，正好覆盖这三个文件。

## 接线

```
rust/vendor/pl_mpeg.h    原版单文件库（未改）
rust/vendor/kv_movie.c   薄封装：把 plm_* 摊平成 kv_open/kv_decode/kv_rgba/kv_audio
rust/build.rs            cc::Build 编进 libkzn.a（iOS 上自动用 xcrun clang）
rust/src/video.rs        Movie：开文件、按时钟解码、贴到画布
```

pl_mpeg 的帧是 Y/Cr/Cb 平面，`plm_frame_to_rgba` 转成 RGBA 后直接 `memcpy` 到
`Movie::rgba`，再按脚本给的 `(x, y, w, h)` 最近邻缩放到 640×480 画布上
（原版就是 640×480，基本 1:1）。pl_mpeg 不写 alpha，封装里补成 255。

## 阻塞语义（关键）

`0xF3` 在脚本里是**阻塞**的。证据是 `SNR02.ISF` 的结尾：

```
14606  0xF3  0,0 640x480 FAM_OP.MPG
14607  0x01  SNR03
14608  0xF3  0,0 640x480 FAM_OPHQ.MPG
14609  0x01  SNR03
```

播完 OP 立刻 `0x01 SNR03`。如果播片是异步的，下一章就会盖在 OP 上开始演。
所以原版 `FUN_0043D890` 建完 DirectShow 图之后是等 `EC_COMPLETE` 的，
Rust 侧对应 `isf.rs` 的 `block_requested = true`（和 `0xE5` 模态框同一套机制）。

引擎里：

- `Engine::tick` 按帧推进 `Movie::advance`，解出的 PCM 丢给 `Mixer::push_movie`
- `Engine::pump` 在播片期间直接返回 `Wait::Movie`（状态码 7），不跑脚本
- 放完（`plm_has_ended`）或玩家点一下 → `finish_movie()` → `vm.unblock()` 继续

## 文件名

脚本里的名字**不一定带扩展名**：

| 脚本 | 字符串 | 实际文件 |
|---|---|---|
| `START.ISF` 第 202 条 | `DOLOGO` | `DOLOGO.MPG` |
| `SNR02.ISF` 第 14606 条 | `FAM_OP.MPG` | `FAM_OP.MPG` |

所以 `Movie::open` 先按原样找，找不到再依次试 `.MPG / .mpg / .MPEG / .mpeg`
（原版也是这么补的）。文件不在就当没有这段，直接 `unblock`，不会把脚本卡死。

## 音频

MPEG-1 Layer II，`plm_samples_t` 给的是 float 交错立体声。封装里转成 i16，
主线程解码时塞进 `Mixer` 的 `MovieVoice`（VecDeque，最多留 4 秒），
`render` 时和 BGM / 语音 / SE 一起混，采样率不同就线性插值。

解码和混音**都在主线程**：pl_mpeg 不是线程安全的，音频线程只读环形缓冲
（`ios/kznApp/Audio.swift` 的 `GameAudio`）。

## 实测

```
$ kzn abi <游戏目录> START.ISF --frames 120
ABI 跑完 120 帧：状态 7，点击 0 次，音频峰值 26482
```

第 120 帧（约 1.9 秒）还在放 `DOLOGO.MPG`：状态 7 = `KZN_MOVIE`，
画面就是 D.O. 的 logo（640×480、24fps、7.92 秒）。

```
$ kzn abi <游戏目录> START.ISF --frames 700
ABI 跑完 700 帧：状态 2，点击 198 次
```

片头放完（7.92s ≈ 495 帧）自动解除阻塞，标题画面正常出现（亮像素 66741，
和没有播片时的基线一致）。

跳过：`kzn abi ... --frames 260 --skip 100` → 第 100 帧点一下，
状态立刻回到 2，画面是标题而不是片子。

## 没做的

- 原版 DirectShow 窗口的右键菜单（暂停/全屏切换）没有；点一下就是跳过。
- 不支持拖进度条 —— 脚本播片本来也没有这种交互。
