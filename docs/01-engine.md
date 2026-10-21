# 引擎与运行时（静态分析）

## 结论速览

- 引擎：D.O. 自研 **GDT**（Game Development Toolkit）。证据：字符串 `GDTMIDI`、类名 `CGDTWnd` / `CGDTPSData` / `CGDTPropertySheetSystem`、`(C) 2005 D.O. corp.`
- 架构：Win32 + MFC 4.2 静态链接；GDI 绘制；DirectShow 播 MPEG；DirectSound / ACM 处理音频。
- 资源：自定义归档 **SM2MPX10**；脚本 **ISF**；图像 **GGD/GG2**；音频 OGG Vorbis；视频 MPEG-1。
- 文本：原版 Shift-JIS；汉化版由 `kzn_sc.dll` 在运行时替换文本（未覆盖原文件）。

## 关键证据（REA / Ghidra）

| 项 | 地址 | 说明 |
|---|---|---|
| 归档魔数比较 | `0x430db0` | 读 8 字节并与 `SM2MPX10` 比较 |
| 归档打开 | `0x430ae0` | 解析 0x20 字节头 + 20 字节目录项；type 2 = SM2MPX10 |
| 文件类型分派 | `0x43d2e0` | case 0=ISF, 1=GGD, 2=ogg, 3=MID, 4=GGD ... |
| 游戏对象构造 | `0x433d60` | 打开 ISF 归档；创建若干容器/管理器 |
| 启动配置 | `0x445f90` | 读注册表 `SOFTWARE\DO\KIZUNAR` 与 `.SUF/.ini` |
| 主窗口类名 | `0x4457b0` | `sprintf("ScriptMagic:%s")` 生成窗口类名（不是脚本系统） |

## 依赖（导入表/字符串）

- `GDI32`: BitBlt, StretchBlt, CreateDIBSection, SetDIBitsToDevice, TextOutA/ExtTextOutA
- `DSOUND.dll`, `WINMM.dll`, `acm*`（ADPCM）
- DirectShow: `EC_*` 事件、`mciSendCommandA`、MPEG 播放
- MFC 4.2：`CWinApp`/`CWnd`/`CFile`/`CPropertySheet` 等

## 汉化补丁

- `kzn_sc.dll` 是 dwing 加壳的小型 DLL（节名 `dwing0/1/2`），入口自解压后 LoadLibrary/GetProcAddress。
- 行为推测：hook 文本输出/字体创建，按 `isf_sc.bin` 的映射替换为简体中文；`kzn_sc.ini` 可配置字体。
- `isf_sc.bin` 为长度/索引混合的加密块序列，暂未完全解析（不影响原版引擎评估）。

## ISF 结论（已解决）

ISF 是**两层加密的字节码**，不是加密文本：

1. 外层 `SECRETFILTER100a` 异或壳，密钥由 EXE 内 2048 字节的 `UOB0` 表实时算出（本作文件偏移 `0x74218`）
2. 内层 `ikuar_decrypt`，按 `data[4:6]` 的版本魔数做循环右移/取反/异或
3. 解出后是 `head_len + 偏移表 + 指令流`，指令按 `opcode(u8) + len(u8)` 切分

细节见 [文件格式](02-formats.md#2-isf-脚本)。

## 待办

- [x] 定位 ISF 解码入口（已定位到算法并复现）
- [x] 确认 ISF 是字节码还是加密文本（字节码）
- [ ] 解析 GGD 图像压缩（下一步：从脚本 `0x56` 的处理函数往下追）
- [ ] 解析 SAV 存档
- [ ] 补全 93 种 opcode 的语义（已确认 7 种，覆盖高频指令）
