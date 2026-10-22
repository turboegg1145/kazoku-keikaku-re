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
| **ISF 虚拟机（可执行）** | ✅ **41 个脚本全部跑到自然结束**，导出 60,917 条台词 |
| 台词文本导出 | ✅ 62,298 条 |
| OGG / MPEG-1 | ✅ 标准格式 |
| GGD 图像（50 个成员，24bpp） | ✅ **完全解开**，操作码表从 `FUN_0040b190` 抄出 |
| GG0 / GG2 图像（792 个成员，32bpp BGRA） | ✅ **完全解开**，操作码表从 `FUN_0040b8e0` 抄出 |
| 存档 SAV | ⚠️ 未解析（重写版可自定格式）|

## 文档

- [引擎与运行时](docs/01-engine.md)
- [文件格式](docs/02-formats.md)
- [ISF 虚拟机](docs/04-isf-vm.md)
- [移植评估（Rust + iOS）](docs/03-porting.md)
- [负结果与踩坑记录](notes/negative-results.md)

## 工具

- `tools/sm2mpx.py` — SM2MPX10 归档列表/解包
- `tools/ggd.py` — GGD / GG0 / GG2 图像解码 / 编码 / 出 PNG（库 + 命令行）
- `tools/ggd_batch.py` — 批量把归档里的图导出成 PNG
- `tools/test_ggd.py` — 不依赖游戏数据的自检（6 项，全部通过）
- `tools/isf_crypto.py` — ISF 两层解密 + 指令流切分（库）
- `tools/isf_text.py` — 导出全部剧本正文（需原版 EXE 提供密钥表）
- `tools/isf_dis.py` — ISF 反汇编（带 opcode 助记符）
- `tools/isf_vm.py` — **可执行的 ISF 虚拟机**（Python 参考实现，能跑完整剧本）

用法示例：

```
python3 tools/sm2mpx.py list GGD
python3 tools/ggd.py GGD CG2_BAO out.png
python3 tools/ggd_batch.py GGD out/
python3 tools/test_ggd.py
python3 tools/isf_vm.py KIZUNAR.EXE ISF SNR00.ISF --out snr00.txt
python3 tools/isf_text.py KIZUNAR.EXE ISF out/
```

> 本仓库只保存分析脚本与笔记，不包含游戏数据。
