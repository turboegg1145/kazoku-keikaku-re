# iOS 端（iOS 14+ / TrollStore）

引擎本体在 `../rust/`，这里只有一层很薄的壳：

```
kznApp/
  App.swift              SwiftUI 入口 + ☰ 菜单（存/读/设置/回标题/退出）
  GameView.swift         Metal 贴 640×480 帧 + CADisplayLink 主循环 + 触摸
  Audio.swift            AVAudioEngine + 环形缓冲（音频线程不碰 Rust）
  kzn.h                  C ABI 声明（对应 rust/src/capi.rs）
  kzn-Bridging-Header.h  Swift 桥接
  Info.plist
project.yml              XcodeGen 工程描述（deploymentTarget 14.0）
scripts/build-rust.sh    交叉编译 libkzn.a
```

## 构建

本地（需要 macOS + Xcode）：

```bash
./scripts/build-rust.sh               # -> rust/target/aarch64-apple-ios/release/libkzn.a
cd .. && cd ios && xcodegen generate  # -> Kzn.xcodeproj
xcodebuild -project Kzn.xcodeproj -scheme Kzn -sdk iphoneos \
  CODE_SIGNING_ALLOWED=NO build
```

Windows 上没法编 Xcode，所以走 GitHub Actions（`.github/workflows/ios.yml`）：
推 `main` 自动跑，也可以在 Actions 页面手动 `Run workflow`。产物是**未签名**的
`Kzn.ipa`（Artifacts → `Kzn-ipa`），TrollStore 直接装。

第一次真编出来的结果（2026-10-23）：

| 项 | 值 |
|---|---|
| 架构 | arm64（Mach-O 64-bit） |
| MinimumOSVersion | **14.0** |
| 链接 SDK | 17.5（Xcode 15.4） |
| ipa 大小 | 13.7 MB（其中字体 16.4 MB 未压缩） |
| 包名 | `com.kazoku.kzn` |

CI 里有两个坑已经钉死，别随手改：

- **XcodeGen 必须 2.42.0**。更新的版本写 Xcode 16 的工程格式（`objectVersion 77`），
  runner 上的 Xcode 15.4 打不开，报 *"future Xcode project file format (77)"*。
- **不要用 iOS 15+ 的 SwiftUI API**（`.borderedProminent`、`.tint`、`.task`、
  `AsyncImage`、`NavigationStack`）。编译器会直接报 *only available in iOS 15.0 or newer*。

## 装到手机

1. 从 Actions 下载 `Kzn-ipa`，解压得到 `Kzn.ipa`。
2. TrollStore → `+` → 选 ipa → 安装（TrollStore 会自己 ldid 签名，不用开发者证书）。
3. 打开一次 App，让它建出 Documents 目录（第一次会是黑屏/报错，正常）。

## 放游戏数据

App 从 **Documents 目录**读游戏文件（`Info.plist` 开了 `UIFileSharingEnabled` 和
`LSSupportsOpeningDocumentsInPlace`，所以「文件」App 里能看到 `Kzn` 文件夹，
直接把 PC 版目录里的文件拖进去）：

```
Documents/
  KIZUNAR.EXE     ← 需要它里面的 UOB0 密钥表来解 ISF
  GGD  GGD2  DATA  ISF  SE  VOICE1  VOICE2  WMSC
  KIZUNAR.SAV     ← 可选，放进去就能读原版存档（格式完全一致）
```

也可以用 Filza / AFC 直接拷到
`/var/mobile/Containers/Data/Application/<Kzn 的 UUID>/Documents/`。

## 字体

原版是 MS Gothic，iOS 上没有。优先级：

1. 系统自带 **Hiragino Sans W3**（`/System/Library/Fonts/Core/HiraginoSans-W3.ttc`）
2. App 包里 CI 下载的 **Noto Sans CJK JP**（`NotoSansCJKjp-Regular.otf`）
3. 兜底 DroidSansFallback（**没有拉丁字母**，日期会变方块，只在前面全挂时才用）

`kzn_open` 的字体参数支持 `路径#faceIndex` 选 `.ttc` 里的第几个字体。

## 现在的限制

- `0xF3` 的 MPEG 片头（`DOLOGO.MPG`）和 OP（`FAM_OP.MPG`）**不播**：iOS 的
  AVFoundation 解不了 MPEG-1，需要在打包时转码，还没做。
- 设置是 iOS 原生表单（音量 / 文字速度），不是 Win32 属性页；壁纸功能没做。
- 原版右键菜单里的「既読スキップ」等项，改成了 ☰ 菜单里的按钮。
