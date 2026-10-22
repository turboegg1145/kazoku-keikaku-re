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
| ISF 脚本（两层解密 + 指令流） | ✅ 通 |
| 台词文本导出 | ✅ 62,298 条 |
| OGG / MPEG-1 | ✅ 标准格式 |
| GGD 图像（50 个成员） | ✅ **完全解开**，操作码表从 `FUN_0040b190` 抄出 |
| GG0 / GG2 容器（792 个成员） | ⚠️ 头部已知，容器结构未解 |
| 存档 SAV | ⚠️ 未解析 |

## 文档

- [引擎与运行时](docs/01-engine.md)
- [文件格式](docs/02-formats.md)
- [移植评估（Rust + iOS）](docs/03-porting.md)
- [负结果与踩坑记录](notes/negative-results.md)

## 工具

- `tools/sm2mpx.py` — SM2MPX10 归档列表/解包
- `tools/ggd.py` — GGD 图像解码 / 编码 / 出 PNG（库 + 命令行）
- `tools/ggd_batch.py` — 批量把归档里的图导出成 PNG
- `tools/ggd_probe.py` — GGD/GG2 头部与统计探针
- `tools/isf_crypto.py` — ISF 两层解密 + 指令流切分（库）
- `tools/isf_text.py` — 导出全部剧本正文（需原版 EXE 提供密钥表）

用法示例：

```
python3 tools/sm2mpx.py list GGD
python3 tools/ggd.py GGD CG2_BAO out.png
python3 tools/ggd_batch.py GGD out/
python3 tools/isf_text.py KIZUNAR.EXE ISF out/
python3 tools/ggd_probe.py GGD MODORU
```

> 本仓库只保存分析脚本与笔记，不包含游戏数据。
