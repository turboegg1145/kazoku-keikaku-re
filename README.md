# 家族計画 ～追憶～ / KIZUNAR (GDT) 逆向笔记

目标：用 REA 分析 PC 版《家族計画～追憶～》的 GDT 引擎与数据格式，评估用 Rust 重写引擎并移植 iOS 的可行性。

- 样本：`E:\BaiduNetdiskDownload\gal\【PC汉化ADV】家族計画 ～追憶～\家族計画 ～追憶～`
- 主程序：`KIZUNAR.EXE`（PE32, x86, MFC, 2005 D.O.）
- 汉化：`kzn_sc.exe` + `kzn_sc.dll` + `isf_sc.bin` + `ggd_sc/`
- 工具：REA 6.7.0（Ghidra 12.1.4）

## 进度

| 模块 | 状态 |
|---|---|
| SM2MPX10 归档 | ✅ 全部解析（842 个成员） |
| ISF 脚本（两层解密 + 指令流） | ✅ 通，163 个 opcode 全部定位 |
| **ISF 虚拟机（可执行）** | ✅ 41/41，含 `0x01/0x02/0x03` 模块调用，主线跑通 `SNR00→…→TITLE` |
| **Rust 重写（`rust/`）** | ✅ 归档 + 图像 + 虚拟机 + 模块系统，41/41 剧本事件流与 Python 版逐字节一致 |
| 软件渲染器（Rust） | ✅ **自动取真实背景** + 文本框 + 折行台词 → 640×480 PNG（`kzn shot --at N`）|
| 台词文本导出 | ✅ 62,298 条 |
| OGG / MPEG-1 | ✅ 标准格式 |
| GGD 图像（50 个成员，24bpp） | ✅ **完全解开**，操作码表从 `FUN_0040b190` 抄出 |
| GG0 / GG2 图像（792 个成员，32bpp BGRA） | ✅ **完全解开**，操作码表从 `FUN_0040b8e0` 抄出 |
| **存档 SAV（`KIZUNAR.SAV`）** | ✅ **完全解析**：30 项块表 + 27 格 + 位图 RLE，真实存档逐字节回环通过（`docs/06-save.md`）|
| 全局旗标落盘（`0x49/0x4A` ↔ 块 27/28） | ✅ 反编译确认偏移与打包方式，虚拟机直接读写这两块 |
| 指令覆盖 | ✅ **93 / 93 条在用指令全部还原**（100% 指令次数，运行时 `unknown` 为空），见 `docs/05-opcodes-todo.md` |
| 主循环 | ✅ `engine.rs`：台词/选项阻塞、打字机、音频接线（`docs/11-engine.md`）|
| 画面特效 | ✅ `0x60` 蒙版溶解 + `0x63` 抖动（`docs/12-effects.md`）|
| 文字窗 | ✅ 文本流控制码全解 + 真几何排版（`docs/13-text.md`，`kzn text` 可查）|
| 存读档界面 | ✅ 27 格 × 3 页 + 缩略图 + 日期，存档格字节与原版一致（`docs/14-saveui.md`）|
| 标题菜单 | ✅ 四块木牌（始めから/続きから/物置/終わる）全部对上脚本分支（`docs/15-menu.md`）|
| `0xE5` 模态对话框 | ✅ 阻塞语义 + 关窗后重置 VM 载入 `LOAD.ISF`（28 章节目录表）|
| 鉴赏室 | ✅ `BGMLOAD`（BGM 列表）/ `CGMODE`（CG 网格）/ `SEXMODE`（场景回放）都能跑 |
| 字体 | ✅ Noto Sans CJK JP（原版 MS Gothic 的度量仍不同，见下）|
| iOS | ⬜ 代码齐了（Metal + CoreAudio + ☰ 菜单 + 设置），**但还没在 CI 上真编过一次** |

已知与原版不同：字体（MS Gothic → Noto Sans CJK JP）、`0xF3` 的 MPEG 片头/片尾不播、
设置是 iOS 原生表单而不是 Win32 属性页。

## 文档

**先看 [交接说明](docs/10-handover.md)** —— 现状、下一步、坑，一页看完。

- [交接说明](docs/10-handover.md)
- [引擎与运行时](docs/01-engine.md)
- [文件格式](docs/02-formats.md)
- [ISF 虚拟机](docs/04-isf-vm.md)
- [移植评估（Rust + iOS）](docs/03-porting.md)
- [指令全表与还原进度](docs/05-opcodes-todo.md)
- [存档格式](docs/06-save.md)
- [音频](docs/07-audio.md)
- [系统功能 0x37](docs/08-sysfunc.md)
- [选项窗口](docs/09-choice.md)
- [主循环与阻塞](docs/11-engine.md)
- [画面特效 0x60 / 抖动 0x63](docs/12-effects.md)
- [文字窗：文本流与控制码](docs/13-text.md)
- [存读档界面](docs/14-saveui.md)
- [标题菜单 / 读档流程 / 原生菜单](docs/15-menu.md)
- [负结果与踩坑记录](notes/negative-results.md)

## iOS

引擎编成静态库（`libkzn.a`），Swift 壳只负责 Metal 贴图、CoreAudio 出声、触摸转发：

- 接口：`rust/src/capi.rs`（`kzn_open/start/tick/pump/frame/click/audio/…`），声明在 `ios/kznApp/kzn.h`
- 壳：`ios/`（XcodeGen 工程 + GitHub Actions 出未签名 `Kzn.ipa`，TrollStore 直装）
- 详见 [ios/README.md](ios/README.md)
- 自检：`kzn abi <游戏目录> <剧本> --frames 600` 完全走 C ABI 跑一遍并出一张 PNG

## 工具

- `tools/sm2mpx.py` — SM2MPX10 归档列表/解包
- `tools/ggd.py` — GGD / GG0 / GG2 图像解码 / 编码 / 出 PNG（库 + 命令行）
- `tools/ggd_batch.py` — 批量把归档里的图导出成 PNG
- `tools/test_ggd.py` — 不依赖游戏数据的自检（6 项，全部通过）
- `tools/sav.py` — `KIZUNAR.SAV` 解析 / 构造 / 位图 RLE（`--selftest` 对真实存档做逐字节回环）
- `tools/test_sav.py` — 存档格式自检（全部通过）
- `tools/isf_crypto.py` — ISF 两层解密 + 指令流切分（库）
- `tools/isf_text.py` — 导出全部剧本正文（需原版 EXE 提供密钥表）
- `tools/isf_dis.py` — ISF 反汇编（带 opcode 助记符）
- `tools/isf_vm.py` — **可执行的 ISF 虚拟机**（Python 参考实现，能跑完整剧本）
- `tools/isf_dump.py` — 把 41 个脚本的事件流（台词/语音/图像/命令）导出成文本
- **`rust/` — Rust 重写版**（`cargo build --release`，见 `docs/03-porting.md` §1.5）

用法示例：

```
python3 tools/sm2mpx.py list GGD
python3 tools/ggd.py GGD CG2_BAO out.png
python3 tools/ggd_batch.py GGD out/
python3 tools/test_ggd.py
python3 tools/isf_vm.py KIZUNAR.EXE ISF SNR00.ISF --out snr00.txt
python3 tools/isf_dump.py KIZUNAR.EXE ISF out/script --steps 300000
python3 tools/isf_text.py KIZUNAR.EXE ISF out/
python3 tools/sav.py KIZUNAR.SAV
python3 tools/test_sav.py
```

> 本仓库只保存分析脚本与笔记，不包含游戏数据。
