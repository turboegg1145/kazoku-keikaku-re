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