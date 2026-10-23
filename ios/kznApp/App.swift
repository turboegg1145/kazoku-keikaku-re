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

/// 右上角的小菜单。原版是右键/系统菜单（セーブ / ロード / プロパティ /
/// タイトルに戻る / 終了），iOS 上没有右键，就收成一个 ☰ 按钮。
struct MenuBar: View {
    @State private var open = false
    @State private var confirmQuit = false

    var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            Button(open ? "✕" : "☰") { open.toggle() }
            if open {
                Button("存档") { kGame?.openSave(1) }
                Button("读档") { kGame?.openSave(0) }
                if kGame?.saveActive == true {
                    HStack(spacing: 6) {
                        Button("◀") { kGame?.flipPage(-1) }
                        Button("▶") { kGame?.flipPage(1) }
                        Button("关闭") { kGame?.closeSave() }
                    }
                }
                Button("回标题") { kGame?.gotoTitle() }
                Button("退出") { confirmQuit = true }
            }
        }
        .font(.system(size: 15, weight: .semibold))
        .buttonStyle(.borderedProminent)
        .padding(10)
        .background(Color.black.opacity(0.45))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .padding(12)
        .alert(isPresented: $confirmQuit) {
            Alert(
                title: Text("退出游戏"),
                message: Text("ゲームを終了します。よろしいですか？"),
                primaryButton: .destructive(Text("退出")) { exit(0) },
                secondaryButton: .cancel(Text("取消"))
            )
        }
    }
}