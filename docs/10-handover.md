# 交接说明（2026-10-23）

> 这份文档是给「下一次会话」或者「另一个人」看的。目标只有一个：**不用重新读历史，
> 直接知道做到哪、下一步做什么、坑在哪。**

## 0. 一句话现状

引擎的**解析层和虚拟机已经 100% 还原**（归档 / 图像 / 脚本 / 存档 / 音频格式），
**渲染层做到了能出正确画面**，**主循环还没写**。也就是说：零件齐全，差一个把零件转起来的壳。

## 1. 仓库

```
/home/eggdog/项目/kazoku-keikaku-re/
├── rust/            Rust 实现（主实现）
│   ├── src/
│   │   ├── sm2mpx.rs   SM2MPX10 归档
│   │   ├── ggd.rs      GGD/GG0/GG2 图像解码
│   │   ├── isf.rs      ISF 解密 + 指令流 + 虚拟机（约 1800 行）
│   │   ├── sav.rs      KIZUNAR.SAV 存档
│   │   ├── render.rs   面合成器（128 块面、blend、裁剪）
│   │   ├── audio.rs    Ogg Vorbis 解码 + 混音器
│   │   └── main.rs     CLI：info/png/raw/hashes/shot/run/dump/sav/audio
│   └── Cargo.toml
├── tools/           Python 对照实现（用来对拍，不是主实现）
│   ├── isf_vm.py       Python 版虚拟机
│   ├── isf_dump.py     批量导出
│   ├── sav.py          存档读写
│   └── ggd.py / sm2mpx.py
├── docs/            见第 8 节
└── notes/           负结果与踩坑
```

最近提交：

```
a93d0ac 0x18 选项输入四种模式还原；VM 拆出单步 step()/drain_events()
d1f96ce 音频：Ogg Vorbis + 混音器 + kzn audio
3b7af33 选项窗口还原（docs/09-choice.md）
2dadeae 0x37 系统功能 20 个分支
```

## 2. 怎么编译 / 怎么验证

```bash
cd /home/eggdog/项目/kazoku-keikaku-re/rust
cargo build --release        # 产物 rust/target/release/kzn
```

游戏目录（WSL 路径，注意有全角括号和波浪号）：

```
/mnt/e/BaiduNetdiskDownload/gal/【PC汉化ADV】家族計画 ～追憶～/家族計画 ～追憶～
```

常用命令：

```bash
K=./rust/target/release/kzn
G=/mnt/e/BaiduNetdiskDownload/gal/【PC汉化ADV】家族計画 ～追憶～/家族計画 ～追憶～

# 归档信息
$K info $G
# 导出图片
$K png $G/GGD BG01AC /tmp/a.png
# 批量跑全部剧本，导出事件流
$K dump $G/KIZUNAR.EXE $G/ISF /tmp/rs --steps 200000
# 截图（--lines N 取前 N 句台词；--at N 只取第 N 句；--choice N 取第 N 个选项窗）
$K shot $G SNR01.ISF /tmp/shots --lines 3 --ascii
$K shot $G SNR01.ISF /tmp/shots --choice 0 --ascii
# 存档
$K sav $G/KIZUNAR.SAV
# 音频
$K audio $G/WMSC 02.OGG --wav /tmp/02.wav
```

**回归基线**（改完必须还是这些数）：

```
$K dump ... → TOTAL steps=7000065 texts=89683   （41 个剧本）
$K shot $G SNR01.ISF ... --lines 1 → 亮像素=25473
$K shot ... --choice 0 → 白底面板 + 两行黑字
```

## 3. 已经 100% 确认的东西（别再重查）

| 模块 | 结论 | 文档 |
|---|---|---|
| 归档 | SM2MPX10，`0x20 + i*20` 表项，12 字节名 + off + size | 02 |
| 图像 | GGD 24bpp / GG0·GG2 32bpp BGRA，操作码表来自 `FUN_0040b190`/`FUN_0040b8e0` | 02 |
| 脚本 | `SECRETFILTER100a` XOR + `ikuar_decrypt` 两层；551,127 条指令 | 04 |
| 指令 | **93/93 条在用指令全部实现**，运行时 unknown 为空 | 05 |
| 存档 | 30 项块表；0..26 = 27 格；27/28 = 全局位/变量；29 = 已读位图 RLE | 06 |
| 音频 | 全是标准 Ogg Vorbis（44100 立体声 BGM / 22050 单声道 SE·语音） | 07 |
| 渲染 | 128 块面 + 滚动 + blend；上半屏与原始 GG0 逐像素 0 差异 | 01 |
| 选项 | 0x15/0x16/0x17/0x18/0x1A/0x1B 全部语义 | 09 |

## 4. 关键结构速查（最容易忘的）

### 4.1 引擎窗口层级

```
engine+0x824            窗口容器
  └─ +0x25C             消息窗口（552×152，贴 FWIN2.GG2）
       └─ +4..+0x24     8 个子窗口（脚本只用 0..4 号）
            每个子窗口：+0x08 surface、+0x18/+0x1C 位置、+0x28/+0x2C 尺寸
                        +0x1418+i*0x20 选项表（32 项）
                        +0x1818 选项数、+0x1819/1A/1B flags
                        +0x182C/+0x1830 客户区原点（默认 0x10,0x10）
```

### 4.2 引擎的「阻塞」原语（**非常重要**）

```
VM 对象 +0x20 = 待执行指令号
  = -1  → 下一帧取新指令
  ≠ -1  → 下一帧重新执行同一条

FUN_00434EB0 = mov [ecx+0x20], -1    // 「这条做完了」
```

**哪个指令处理函数不调用 `FUN_00434EB0`，脚本就卡在那条指令上，每帧重跑。**
这就是引擎的等待机制（0x18 鼠标模式、0x85 输入轮询都是靠这个）。

Rust 侧的对应实现：`Vm::step()` 里 `Ok(true)` = 不推进 PC（等价于阻塞），
`Ok(false)` = `code = end`（推进）。`block_on_input` 开关控制是否真的阻塞
（批量 dump 时必须关，否则原地死循环）。

### 4.3 选项结果怎么回传脚本

```
0x18 <u8 窗口> <u16 槽> <u8 标志>
  标志&3 = 0 鼠标模式（阻塞） / 1 键盘模式 / 2 取消写 -1 / 3 确认
  确认时：FUN_00424A60(obj, 槽, 当前高亮) → 就是写 VM 变量数组 vars[槽]
```

所以脚本后面用 `IF 变量0E == ...` 读结果。
`Vm::hover` 就是「当前高亮」（引擎 +0x181C），-2 = 没选中。

### 4.4 文字绘制原语 `FUN_004092A0`

```
draw_text(surface, x, y, 文字色, 阴影色, 字体, 字高, 文本, 阴影开关)
```

- 2× 字号画进 1bpp DIB，再 2×2 取样得到 0..4 级覆盖率，查表 `DAT_0047227C` 得 alpha（引擎的抗锯齿）
- `阴影开关 != 0` 时在 (x+1, y+1) 再画一遍，alpha 固定 0x80
- 返回总推进宽度（居中/换行用）

## 5. 还没做的（按建议顺序）

### ① 主循环 `engine.rs`（下一步，最高优先）

要写的东西：

```rust
pub enum Wait {
    Input,                        // 台词显示完，等点击
    Choice { window: u8 },        // 等选项
    Timed { ms: f32 },            // 过渡/动画
}
pub struct Engine {
    vm: isf::Vm, player: render::Player, mixer: audio::Mixer,
    assets: Assets, wait: Option<Wait>, clock_ms: f64, halted: Option<isf::Halt>,
}
```

- `run_until_wait()`：`vm.step()` 循环 → `drain_events()` → 交给 player/mixer → 遇到需要等的事件就 break
- `tick(dt_ms)`：推进时钟、处理 `Timed`、拉音频
- 鼠标：`hover` 命中测试（选项矩形 → `vm.hover`），点击 → 解除 `Input`/`Choice`

### ② 文字窗几何 / 控制码

- ✅ 已做（见 `docs/13-text.md`）：控制码全解（`00` 回车 / `06` 换行 / `02` 等点击 / `03` 清窗 / `13` 人名 / `FF` 绘制模式…）、
  文字区 `(16,16)-(宽-16,高-16)`、行距 = 字高(16) + 4、字体 20px/字重 900、白色正文。
  `rust/src/text.rs` 负责解析排版，`kzn text` 可查，`kzn shot` 已按真几何画字。
- 待办：`0x01` 阴影/描边色、`0x0A` ruby 注音（全篇 3 处）、`0xD6`/`0x25` 的另一个文本类、`0xE0`–`0xE5` 文本对象。

### ③ 特效与过渡

- `0x60` 特效（`DATA/EFF*.BIN`，16 张 200×300 24bpp BMP）
- `0x63` 过渡种类（`0x57` 的 cmd 已经分派到 `FUN_0040dd90`）
- `0xF3` MPEG 播片（`DOLOGO.MPG` / `FAM_OP.MPG` / `FAM_OPHQ.MPG`）

### ④ 输入热区 `0x85`

需要把点击区域表建出来。目前 `do_input` 只有骨架。

### ⑤ 存读档菜单 `0x48`

`FUN_00438A90`，7045 行，只在 `START.ISF` 里。纯界面还原，逻辑复杂但都看得到。

### ⑥ 音频接线

`audio.rs` 和事件都齐了，缺的是主循环里的驱动 + 平台输出层。

### ⑦ iOS 外壳

- Rust 核心编静态库（`aarch64-apple-ios`）
- Swift 外壳 + CoreAudio render callback 直接调 `Mixer::render`
- GitHub Actions 打包（**本机是 Windows，没有 Xcode**）
- TrollStore 安装（目标 iOS 14）

## 6. 已知的「推断项」（不是确认，别当成事实）

| 项 | 状态 |
|---|---|
| 选项窗底色/字色 | **推断**。`FUN_00418420`（设置选项配色）在本版 EXE 里**没有任何调用者**，永远是构造函数默认值 0；无 alpha 的 surface 上颜色 0 会被 `FUN_004082D0` 二次填成白色，所以按「白底黑字 + 1px 50% 黑投影」渲染 |
| 选项子窗口尺寸 | **推断**为 552×152（依据：选项矩形 520 + 左右各 0x10 = 552，且 `+0x1834` 算法同源） |
| 消息窗口屏幕位置 | **推断**为底部居中 `((640-552)/2, 480-152-8)` |
| `0x20` 操作数 | 引擎里是「移动子窗口到 (x,y)」（`FUN_0041BF60`），本仓库暂按 `(x,y,w,h,flag)` 记事件；41 个剧本里没出现过，不影响 |

## 7. 环境坑（踩过的）

- **REA MCP 会掉会话**：`batch_decompile` 报 `No app is open` 时，重新 `await mcp_rea.open_binary({ path: "/tmp/kz.exe" })`。
  如果反复掉，直接看 `/tmp/kz.asm`（`objdump -M intel -d kz.exe`，16 万行）。
- **`/tmp` 会被清**：`/tmp/kz.exe`、`/tmp/kz.asm`、`/tmp/revenv` 都可能消失，重新生成即可：
  ```bash
  cp "$G/KIZUNAR.EXE" /tmp/kz.exe
  objdump -M intel -d /tmp/kz.exe > /tmp/kz.asm
  ```
- **Python 依赖**：`tools/isf_vm.py` 只用标准库，`python3` 直接能跑；`capstone`/`pefile` 只在别的脚本里用。
- **Rust 侧不要用模板字符串写代码**（本项目所有代码都用 `Edit` + 显式字符串拼接，避免反引号问题）。
- **编码**：Python `cp932` 会把 `0xFF` 映射成 `U+F8F3`（PUA），Rust `encoding_rs::SHIFT_JIS` 映射成 `U+FFFD`，两边都要过滤。

## 8. 文档索引

| 文件 | 内容 |
|---|---|
| `docs/01-engine.md` | 引擎总览、窗口/面模型 |
| `docs/02-formats.md` | 归档、图像、脚本加密 |
| `docs/03-porting.md` | 移植评估（Rust + iOS） |
| `docs/04-isf-vm.md` | ISF 虚拟机、指令编码 |
| `docs/05-opcodes-todo.md` | 93 条指令全表与还原进度 |
| `docs/06-save.md` | 存档格式 |
| `docs/07-audio.md` | 音频格式、BGM/语音语义、混音器 |
| `docs/08-sysfunc.md` | `0x37` 系统功能 20 个分支 |
| `docs/09-choice.md` | 选项窗口（调用链、坐标系、对齐公式） |
| `notes/negative-results.md` | 走不通的路，别再走 |

## 9. 下一步第一个动作

```bash
cd /home/eggdog/项目/kazoku-keikaku-re/rust
cargo build --release
```

确认基线没坏（第 2 节那三条），然后新建 `src/engine.rs`，按第 5 节 ① 的骨架写主循环。