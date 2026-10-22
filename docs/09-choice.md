# 选项窗口（0x15 / 0x16 / 0x17 / 0x18 / 0x1A / 0x1B）

## 调用链

```
0x15 → FUN_00436D80 → FUN_004143B0(engine+0x824, win, idx, text, x, y, w, h)
                        → FUN_0041CA00(msgwin, win, ...)
                        → msgwin[+4 + win*4] → FUN_00418560(child, idx, text, x, y, w, h)
0x16 → FUN_00436E60 → FUN_004142D0 → FUN_0041C970 → child → FUN_004182A0(child, count, flags)
0x17 → FUN_00436EC0 → FUN_004143F0 → FUN_0041CA40 → child → FUN_004183B0(child)
0x1A → FUN_00436F70：对 0..4 号子窗口各调一次 FUN_004143F0（清空）
0x1B → FUN_00436FA0：对 0..4 号子窗口洗牌
```

**注意**：`engine+0x824` 不是消息窗口本身，而是它上面的容器；`+0x25C` 才是消息窗口；
消息窗口再持有 **8** 个子窗口（`+4 .. +0x24`），脚本只用到 0..4 号（`0x1A`/`0x1B` 硬编码循环 5 次）。
所以“选项窗口”是消息窗口的子窗口，有独立的 surface（`+8`）、位置（`+0x18/+0x1C`）和尺寸（`+0x28/+0x2C`）。

## 数据结构（子窗口对象，0x1C9C 字节）

| 偏移 | 含义 |
|---|---|
| +0x14 | 已初始化 |
| +0x15 | surface 就绪 |
| +0x16 | 显示中 |
| +0x18 / +0x1C | 屏幕位置（`FUN_0041BF60` 移动，钳到 640×480 内） |
| +0x28 / +0x2C | 尺寸 |
| +0x30 | 字体对象 |
| +0x36 | 阴影开关字节（传给文字绘制的 param_8） |
| +0x38 | 选项字体高（默认 0x10 = 16） |
| +0x3C | 默认文字色（0xFFFFFF） |
| +0xE0C | 状态 0/1/2 |
| +0x1400 / +0x1404 / +0x1408 | 文字色 / 矩形填充色 / 阴影色 |
| +0x140C / +0x1410 / +0x1414 | 同上，选中态 |
| +0x1418 + i*0x20 | 第 i 个选项 |
| +0x1818 | 选项数 |
| +0x1819 / +0x181A / +0x181B | flags bit1 / bit2 / bit3 |
| +0x182C / +0x1830 | 客户区原点，默认 (0x10, 0x10) |
| +0x1834 / +0x1838 | 客户区右下限 = max(尺寸-0x10, 0x10) |

选项项 0x20 字节：`+0` used、`+1` 打开时清零、`+4/+8`、`+0x0C` CString（MFC 4.2，4 字节）、
`+0x10` RECT(left, top, right, bottom)。

## 位置计算（FUN_00418560）

```c
rect.left   = this->base_x + x;
rect.top    = this->base_y + y;
rect.right  = w + rect.left;   // 注意是 left+w，不是绝对右边界
rect.bottom = h + rect.top;
```

## 绘制（FUN_00418990 → FUN_004092A0）

```c
if (flags & 4) {                       // 居中
    x = (rect.right - n*0x38/2 + rect.left) / 2;
    y = (rect.bottom - 0x38 + rect.top) / 2;
} else if (flags & 8) {                // 右对齐
    x = win_width - 0x1E - n*0x38;  y = rect.top;
} else {
    x = rect.left;  y = rect.top;
}
FUN_00409A50(surface, &rect, fill_colour);   // 填矩形（透明面里只写 RGB，不写 alpha）
FUN_004092A0(surface, x, y, text_colour, shadow_colour, font, 0x38, text, 1);
```

`FUN_004092A0` 是引擎唯一的文字绘制原语，值得单独记：

- 把文字用 `CreateFontIndirectA`（字号 ×2）画进 1bpp DIB，再按 2×2 取样得到 0..4 级覆盖率，
  用查表 `DAT_0047227C[0..4]` 映射成 alpha —— 这就是引擎的抗锯齿。
- 写入 `RGB = text_colour`、`alpha = 覆盖率`。
- `param_8 != 0` 时在 **(x+1, y+1)** 再写一遍 `shadow_colour`、`alpha = 0x80` —— 1px 半透明投影。
- 返回这段文字的总推进宽度（供居中/换行用）。

## 本仓库的还原程度

| 项 | 状态 |
|---|---|
| 选项数、文本、矩形 | ✅ 逐字节对齐 |
| 三种对齐方式 | ✅ 照抄引擎公式（含它 `tw/4` 那个偏移） |
| 子窗口位置 `0x20` | ⚠️ 引擎里是“移动子窗口到 (x,y)”（`FUN_0041BF60`），本仓库暂按 (x,y,w,h,flag) 记事件；41 个剧本里没出现过，不影响 |
| 子窗口尺寸 | ⚠️ 推断为消息窗口尺寸 552×152（依据：选项矩形 520 + 左右各 0x10 = 552，且 `+0x1834` 的算法同源） |
| 底色 / 字色 | ⚠️ 推断。`FUN_00418420`（设置选项配色）在本版 EXE 里**没有任何调用者**，所以永远是构造函数默认值 0；
  在无 alpha 的 surface 上，`FUN_004082D0`/`FUN_00409A50` 对颜色 0 会二次填充白色（`colour==0 → 0xFFFFFF`），
  因此按“白底黑字 + 1px 50% 黑投影”渲染 |