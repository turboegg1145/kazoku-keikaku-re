# 先行资料（provenance）

分析过程中参考过的公开资料，只用来对照格式，**没有直接搬运代码**：

- `https://pypi.org/project/sm2mpx/` — SM2MPX10 归档
- `https://orin.page/wiki/Ikura_GDL` — Ikura GDL 引擎总览
- `/Ikura_GDL_(archive)` `/ISF` `/Ikura_GDL_(config)` `/Ikura_GDL_(save)` — 同站格式页
- `https://github.com/satan53x/SExtractor` 的 `tools/IKURA/` — ISF 加解密与文本导入导出

`tools/isf_crypto.py` 里的 `HEX_TABLE` / `HEX_ENCODE_MAP` 两个常量与 SExtractor 的实现一致（格式常量，无法回避）。其余代码为本仓库自行实现。

> 这些资料只覆盖 ISF 与归档，**没有覆盖 GGD 图像压缩**，所以图像仍需自己破。

下载的参考文件放在 `ref/`，已加入 `.gitignore`（来源许可不明，不入库）。
