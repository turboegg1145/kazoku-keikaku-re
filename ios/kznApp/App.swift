import SwiftUI

@main
struct KznApp: App {
    var body: some Scene {
        WindowGroup {
            ZStack(alignment: .topTrailing) {
                GameView()
                    .ignoresSafeArea()
                    .statusBarHidden(true)
                MenuBar()
            }
        }
    }
}

/// 右上角的小菜单。原版是右键/系统菜单，iOS 上没有右键，就放三个按钮。
struct MenuBar: View {
    @State private var open = false

    var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            HStack(spacing: 6) {
                Button("存档") { kGame?.openSave(1); open = true }
                Button("读档") { kGame?.openSave(0); open = true }
            }
            if open {
                HStack(spacing: 6) {
                    Button("◀") { kGame?.flipPage(-1) }
                    Button("▶") { kGame?.flipPage(1) }
                    Button("关闭") { kGame?.closeSave(); open = false }
                }
            }
        }
        .font(.system(size: 15, weight: .semibold))
        .buttonStyle(.borderedProminent)
        .padding(10)
        .background(Color.black.opacity(0.45))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .padding(12)
    }
}