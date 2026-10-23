# iOS 端（iOS 14 + TrollStore）

引擎本体在 `../rust/`，这里只有一层很薄的壳：

```
kznApp/
  App.swift              SwiftUI 入口
  GameView.swift         Metal 贴 640×480 帧 + CADisplayLink 主循环 + 触摸
  Audio.swift            AVAudioEngine + 环形缓冲（音频线程不碰 Rust）
  kzn.h                  C ABI 声明（对应 rust/src/capi.rs）
  kzn-Bridging-Header.h  Swift 桥接
  Info.plist
project.yml              XcodeGen 工程描述
scripts/build-rust.sh    交叉编译 libkzn.a
```

## 构建

```bash
../ios/scripts/build-rust.sh          # -> rust/target/aarch64-apple-ios/release/libkzn.a
cd ios && xcodegen generate           # -> Kzn.xcodeproj
xcodebuild -project Kzn.xcodeproj -scheme Kzn -sdk iphoneos \
  CODE_SIGNING_ALLOWED=NO build
```

GitHub Actions 已经配好（`.github/workflows/ios.yml`），推上去就会产出未签名的
`Kzn.ipa`，直接用 TrollStore 装。

## 放游戏数据

App 从 **Documents 目录**读游戏文件（`Info.plist` 开了 `UIFileSharingEnabled`，
所以「文件」App 里能看到 Kzn 文件夹，直接拖进去）：

```
Documents/
  KIZUNAR.EXE     ← 需要它里面的 UOB0 密钥表来解 ISF
  GGD  GGD2  DATA  ISF  SE  VOICE1  VOICE2  WMSC
  KIZUNAR.SAV     ← 可选，放进去就能读原版存档
```

字体放 App 包里（`DroidSansFallbackFull.ttf`，和 `kzn shot` 用的是同一个），
放在 target 的 Copy Bundle Resources 里即可。

## 现在的限制

- 起始剧本写死 `SNR00.ISF`（`GameView.swift` 的 `kStartScript`）。
- 标题菜单（`START.ISF` 的 `0x48` 存读档）还没接，所以暂时不能从标题进游戏。
- 触摸只有「点一下推进」，没有右键/滚轮/Ctrl 快进。