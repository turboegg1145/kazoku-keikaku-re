# 画面特效 `0x60`

## 指令格式

```
0x60 <int 种类> <int 面号> <int 时长ms> <int 取反> <int 附加> <13 字节文件名>
```

实测（全篇 367 次）种类**全是 0**，面号 1/3，时长 300/500/1500ms，取反 0/1。

## 调用链

```
0x43A250  读 5 个整数 + 13 字节名字
  → FUN_00414FA0(engine+0x824, 种类, 面号, 时长, 取反, 附加, 名字)
      surface = engine+0x58 + (面号 & 0x7F) * 4
      → FUN_00415010(engine+0x824, 显示面, 种类, surface, 时长, 取反, 附加, 名字)
          kind 0 → FUN_004150F0
          kind 1 → FUN_00415830
          kind 2 → FUN_00415F40
```

## 蒙版文件（`DATA/EFF*.BIN`、`DATA/SEF*.BIN`）

**是标准 8bpp BMP**，不是自定义格式：

```
0x00 "BM"
0x02 u32 文件长度 0x13038 = 77880
0x0A u32 像素偏移 0x436 = 1078   （14 + 40 + 1024 调色板）
0x0E u32 DIB 头长度 0x28
0x12 i32 宽 320
0x16 i32 高 240
0x1C u16 位深 8
0x36 256 项调色板（每项 4 字节）
0x436 像素索引，自下而上，行按 4 字节对齐
```

**调色板完全不用**——引擎只把每个像素的**索引字节当 alpha 用**。
所以一张特效图就是一张 320×240 的灰度蒙版，运行时按 2× 放大到 640×480。

## 合成公式（`FUN_004156C0` 纯 C 版 / `FUN_00415580` MMX 版，语义相同）

```c
progress = (elapsed * 1024) / duration - 512;   // 0ms → -512，结束 → +512
if (progress > 255) progress = 255;

m = mask[i];
if (m != 0) {
    if (invert) m = (byte)(-m);                 // 0xFF 取反加一
    a = m + progress;
    if (a < 0) a = 1;
    else if (a > 254) a = 255;
} else {
    a = 0;                                      // 蒙版为 0 的像素不参与
}
dst.rgb = (目标面.rgb * a + 显示面.rgb * (255 - a)) >> 8;
```

也就是「用蒙版做遮罩，把显示面溶解到目标面」，`a=255` 时完全是目标面。

`elapsed` 每帧按 `timeGetTime()` 差值累加，单帧最多算 200ms（防止卡顿时跳变）。

## 本仓库的还原

`rust/src/render.rs`：

- `decode_mask(data) -> (w, h, Vec<u8>)`：解 8bpp BMP，统一成从上到下。
- `Effect { mask, from, to, start, dur, invert }`，`Player::apply` 收到 `Event::Effect` 时快照显示面。
- `Player::frame()` 在返回前按上面的公式合成；`Player::tick(dt)` 推进时钟。

验证：

```
$ kzn play "$G" SNR00.ISF --lines 500 --no-audio
SNR00.ISF: 台词 500 句，选项 1 个，帧 998，指令 55398
特效 5 次: ["SEF03.BIN(1500ms,面1)"]

$ kzn mask "$G" SEF03.BIN --png /tmp/sef03.png
SEF03.BIN: 320x240  非零 76800 (100.0%)  全不透明 2000 (2.6%)
  最多的灰度: [(255, 2000), (250, 789), (244, 764), (254, 756), (169, 735)]
```

## 还没做的

种类 1（`FUN_00415830`）和种类 2（`FUN_00415F40`）的算法还没解。
全篇 367 次 `0x60` 里没出现过，等遇到再补。

---

# 画面抖动 `0x63`

## 指令格式

```
0x63 <u16 种类> <u8 a> <u8 b> <i32 时长ms>
```

调用 `FUN_00413090(engine+0x824, 种类, a, b, 时长)` 只负责**登记**，
真正的位移在每帧的 `FUN_00413119` 里算。

## 算法

引擎**不抖像素**，它改的是**滚动原点**（后面贴图时整体平移），
所以画面边缘会出现「露底」——原版就是这样，不是 bug。

```c
steps    = kind * 2;                    // 种类 = 来回次数（一个来回 = 2 次）
step_ms  = duration / steps;
idx      = (now - start) / step_ms;     // 0,1,2,...
if (idx >= steps) 抖动结束;
sign     = (idx % 2 == 0) ? +1 : -1;    // 每隔 step_ms 翻一次符号
offset   = (sign * a, sign * b);
```

也就是「左右横跳 kind 个来回」，每次停留 duration / (kind*2) 毫秒。

## 实测操作数分布（全篇 26 次）

| 种类 | a | b | 时长 | 含义 |
|---|---|---|---|---|
| 10 | 5 | 0 | 500 | 水平抖 10 个来回，每次 25ms，幅度 5px（最常用） |
| 10 | 3 | 0 | 300 | 同上，幅度 3px |
| 10 | 1 | 0 | 100 | 轻微抖 |
| 2 | 0 | 30 | 100 | 垂直抖 2 个来回，幅度 30px（重击感） |
| 2 | 0 | 5 | 100 | 垂直轻抖 |
| 2 | 5 | 0 | 100 | 水平短抖 |

## 本仓库的还原

`rust/src/render.rs`：

- `Shake { a, b, kind, dur, start }`，`Player::apply` 收到 `Event::Transition` 时登记，
  `player.shakes` 计数。
- `Player::display_frame()` 按上面的公式给 `off[display]` 加偏移；
  `frame()` 再在这之上叠 `0x60` 特效（先抖后溶解，和引擎顺序一致）。

验证：

```
$ kzn play GAME SNR05.ISF --lines 1500 --no-audio
SNR05.ISF: 台词 1500 句，选项 6 个，帧 2998，指令 241235，结束 Text
画面抖动 16 次
特效 6 次: ["SEF04.BIN(300ms,面3)", "EFF03.BIN(500ms,面1,反)"]
```
