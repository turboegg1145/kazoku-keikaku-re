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
    @State private var showSettings = false

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
                Button("设置") { showSettings = true }
                Button("回标题") { kGame?.gotoTitle() }
                Button("退出") { confirmQuit = true }
            }
        }
        .font(.system(size: 15, weight: .semibold))
        .buttonStyle(BorderlessButtonStyle())
        .foregroundColor(.white)
        .padding(10)
        .background(Color.black.opacity(0.45))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .padding(12)
        .sheet(isPresented: $showSettings) { SettingsView() }
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

/// 设置。原版是 Win32 属性页（CMyPropertyPage4..8），里面能调的就是
/// 文字速度、音量、画面模式这些；iOS 上画面模式由系统决定，所以只留两个旋钮。
struct SettingsView: View {
    @AppStorage("kzn.volume") private var volume: Double = 1.0
    @AppStorage("kzn.speed") private var speed: Double = 50.0
    @Environment(\.presentationMode) private var presentation

    var body: some View {
        NavigationView {
            Form {
                Section(header: Text("音量")) {
                    HStack {
                        Image(systemName: "speaker.fill")
                        Slider(value: $volume, in: 0...1)
                        Image(systemName: "speaker.wave.3.fill")
                    }
                }
                Section(header: Text("文字速度")) {
                    Slider(value: $speed, in: 10...200, step: 5)
                    Text("每分钟约 \(Int(speed * 60)) 字")
                        .font(.footnote)
                        .foregroundColor(.secondary)
                }
                Section {
                    Button("恢复默认") {
                        volume = 1.0
                        speed = 50.0
                    }
                }
            }
            .navigationTitle("设置")
            .navigationBarItems(trailing: Button("完成") { presentation.wrappedValue.dismiss() })
            .onChange(of: volume) { v in kGame?.setVolume(Float(v)) }
            .onChange(of: speed) { v in kGame?.setSpeed(Float(v)) }
            .onAppear {
                kGame?.setVolume(Float(volume))
                kGame?.setSpeed(Float(speed))
            }
        }
    }
}