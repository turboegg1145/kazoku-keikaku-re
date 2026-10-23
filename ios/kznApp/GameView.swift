import SwiftUI
import Metal
import QuartzCore
import UIKit

/// 一开始跑哪个剧本。START.ISF 负责初始化（0x48 确定存档数组尺寸、读已读位图、
/// 建 6 块面），然后自己跳进第一章。
let kStartScript = "START.ISF"

/// 让 SwiftUI 的菜单按钮能拿到引擎（只有一个 GameUIView）。
weak var kGame: GameUIView?

struct GameView: UIViewRepresentable {
    func makeUIView(context: Context) -> GameUIView {
        GameUIView(frame: UIScreen.main.bounds)
    }
    func updateUIView(_ uiView: GameUIView, context: Context) {}
}

final class GameUIView: UIView {
    private let gameW = 640
    private let gameH = 480

    private var engine: OpaquePointer?
    private var display: CADisplayLink?
    private var audio: GameAudio?
    private var lastTime: CFTimeInterval = 0

    private var device: MTLDevice!
    private var queue: MTLCommandQueue!
    private var pipeline: MTLRenderPipelineState!
    private var sampler: MTLSamplerState!
    private var texture: MTLTexture!
    private var pixels = [UInt8](repeating: 0, count: 640 * 480 * 4)

    override class var layerClass: AnyClass { CAMetalLayer.self }
    private var metalLayer: CAMetalLayer { layer as! CAMetalLayer }

    override init(frame: CGRect) {
        super.init(frame: frame)
        kGame = self
        setupMetal()
        setupEngine()
        let link = CADisplayLink(target: self, selector: #selector(tick))
        link.preferredFramesPerSecond = 60
        link.add(to: .main, forMode: .common)
        display = link
    }

    required init?(coder: NSCoder) { fatalError("不支持 storyboard") }

    // MARK: - Metal

    private func setupMetal() {
        device = MTLCreateSystemDefaultDevice()
        queue = device.makeCommandQueue()
        metalLayer.device = device
        metalLayer.pixelFormat = .bgra8Unorm
        metalLayer.framebufferOnly = true
        metalLayer.isOpaque = true

        let src = """
        #include <metal_stdlib>
        using namespace metal;
        struct VOut { float4 pos [[position]]; float2 uv; };
        vertex VOut v_main(uint vid [[vertex_id]]) {
            float2 p[4] = { float2(-1,-1), float2(1,-1), float2(-1,1), float2(1,1) };
            float2 u[4] = { float2(0,1), float2(1,1), float2(0,0), float2(1,0) };
            VOut o; o.pos = float4(p[vid], 0, 1); o.uv = u[vid]; return o;
        }
        fragment float4 f_main(VOut in [[stage_in]], texture2d<float> tex [[texture(0)]], sampler s [[sampler(0)]]) {
            return tex.sample(s, in.uv);
        }
        """
        guard let lib = try? device.makeLibrary(source: src, options: nil),
              let vfn = lib.makeFunction(name: "v_main"),
              let ffn = lib.makeFunction(name: "f_main") else {
            return
        }
        let desc = MTLRenderPipelineDescriptor()
        desc.vertexFunction = vfn
        desc.fragmentFunction = ffn
        desc.colorAttachments[0].pixelFormat = .bgra8Unorm
        pipeline = try? device.makeRenderPipelineState(descriptor: desc)

        let sd = MTLSamplerDescriptor()
        sd.minFilter = .nearest
        sd.magFilter = .nearest
        sampler = device.makeSamplerState(descriptor: sd)

        let td = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba8Unorm, width: gameW, height: gameH, mipmapped: false)
        td.usage = .shaderRead
        texture = device.makeTexture(descriptor: td)
    }

    private func present() {
        guard let drawable = metalLayer.nextDrawable(),
              let cb = queue.makeCommandBuffer() else { return }
        let rp = MTLRenderPassDescriptor()
        rp.colorAttachments[0].texture = drawable.texture
        rp.colorAttachments[0].loadAction = .clear
        rp.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 1)
        rp.colorAttachments[0].storeAction = .store
        guard let enc = cb.makeRenderCommandEncoder(descriptor: rp) else { return }
        enc.setRenderPipelineState(pipeline)
        enc.setFragmentTexture(texture, index: 0)
        enc.setFragmentSamplerState(sampler, index: 0)
        enc.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
        enc.endEncoding()
        cb.present(drawable)
        cb.commit()
    }

    // MARK: - 引擎

    /// 候选字体，按顺序试。`#0` 是 ttc 集合里的字面号（NotoSansCJK 的 0 = JP）。
    /// 原版用 MS Gothic（日文 + 拉丁都有），所以第一个要求是「两种字形都得有」——
    /// DroidSansFallback 没有拉丁字形，存档界面的日期会变成方框，只能垫底。
    private func fontCandidates() -> [String] {
        var out: [String] = []
        // iOS 自带的日文字体，能读到就不用打包字体。
        for p in ["/System/Library/Fonts/Core/HiraginoSans-W3.ttc",
                  "/System/Library/Fonts/HiraginoSans-W3.ttc"] {
            if FileManager.default.fileExists(atPath: p) { out.append(p + "#0") }
        }
        if let p = Bundle.main.path(forResource: "NotoSansCJKjp-Regular", ofType: "otf") { out.append(p) }
        if let p = Bundle.main.path(forResource: "NotoSansCJK-Regular", ofType: "ttc") { out.append(p + "#0") }
        if let p = Bundle.main.path(forResource: "NotoSansJP-Regular", ofType: "otf") { out.append(p) }
        if let p = Bundle.main.path(forResource: "DroidSansFallbackFull", ofType: "ttf") { out.append(p) }
        return out
    }

    /// 完整版 ipa 里自带 `gamedata/`：在 Documents 里给它建符号链接，引擎照常按目录读。
    ///
    /// `KIZUNAR.SAV` 例外，真拷一份——存档要可写，包里的文件是只读的。
    /// 这样装完直接能玩，不用手动拷 1 GB，也不占双份空间。
    private func prepareData(_ docs: String) -> String {
        guard let bundled = Bundle.main.url(forResource: "gamedata", withExtension: nil) else {
            return docs
        }
        let fm = FileManager.default
        let names = (try? fm.contentsOfDirectory(atPath: bundled.path)) ?? []
        for n in names {
            let src = bundled.appendingPathComponent(n)
            let dst = URL(fileURLWithPath: docs).appendingPathComponent(n)
            if n.uppercased() == "KIZUNAR.SAV" {
                if !fm.fileExists(atPath: dst.path) {
                    try? fm.copyItem(at: src, to: dst)
                }
                continue
            }
            // 链接已经指对了就跳过；坏了才重建。玩家自己放的实体文件不碰。
            if let cur = try? fm.destinationOfSymbolicLink(atPath: dst.path) {
                if cur == src.path { continue }
                try? fm.removeItem(at: dst)
            } else if fm.fileExists(atPath: dst.path) {
                continue
            }
            do {
                try fm.createSymbolicLink(atPath: dst.path, withDestinationPath: src.path)
            } catch {
                // 建不了链接就老实拷一份（少数情况下沙盒会拒绝跨容器链接）。
                try? fm.copyItem(at: src, to: dst)
            }
        }
        return docs
    }

    private func setupEngine() {
        // 数据来源两种：包里的 gamedata（完整版 ipa），或 Documents（手动拷的）。
        let docs = prepareData(FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].path)
        let fonts = fontCandidates()
        guard !fonts.isEmpty else {
            print("[kzn] 找不到字体资源")
            return
        }
        // 逐个试：kzn_open 在字体解析失败时返回 null，换下一个。
        for f in fonts {
            if let e = f.withCString({ kzn_open(docs, 1, $0) }) {
                engine = e
                break
            }
            print("[kzn] 字体加载失败，换下一个: \(f)")
        }
        guard let engine = engine else {
            print("[kzn] 打不开游戏目录: \(docs)")
            return
        }
        if kzn_start(engine, kStartScript) != 0 {
            var buf = [CChar](repeating: 0, count: 256)
            kzn_error(engine, &buf, 256)
            print("[kzn] 启动失败: \(String(cString: buf))")
        }
        // 上次在「设置」里调的旋钮（原版存在注册表里，这里用 UserDefaults）。
        let defaults = UserDefaults.standard
        _ = kzn_set_volume(engine, Float(defaults.object(forKey: "kzn.volume") as? Double ?? 1.0))
        _ = kzn_set_speed(engine, Float(defaults.object(forKey: "kzn.speed") as? Double ?? 50.0))
        audio = GameAudio()
    }

    @objc private func tick(_ link: CADisplayLink) {
        guard let engine = engine else { return }
        let now = link.timestamp
        let dt = lastTime == 0 ? 16.0 : min((now - lastTime) * 1000.0, 100.0)
        lastTime = now
        kzn_tick(engine, dt)
        kzn_pump(engine, 4000)
        if kzn_frame(engine, &pixels) > 0 {
            texture.replace(
                region: MTLRegionMake2D(0, 0, gameW, gameH),
                mipmapLevel: 0,
                withBytes: pixels,
                bytesPerRow: gameW * 4)
        }
        audio?.fill(engine)
        present()
    }

    // MARK: - 触摸

    /// 视图坐标 → 640×480 游戏坐标（等比缩放 + 居中）。
    private func gamePoint(_ p: CGPoint) -> CGPoint {
        let scale = min(bounds.width / CGFloat(gameW), bounds.height / CGFloat(gameH))
        let ox = (bounds.width - CGFloat(gameW) * scale) / 2
        let oy = (bounds.height - CGFloat(gameH) * scale) / 2
        return CGPoint(x: (p.x - ox) / scale, y: (p.y - oy) / scale)
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let engine = engine, let t = touches.first else { return }
        let p = gamePoint(t.location(in: self))
        kzn_mouse(engine, Int32(p.x), Int32(p.y))
        kzn_click(engine, Int32(p.x), Int32(p.y))
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let engine = engine, let t = touches.first else { return }
        let p = gamePoint(t.location(in: self))
        kzn_mouse(engine, Int32(p.x), Int32(p.y))
        kzn_save_move(engine, Int32(p.x), Int32(p.y))
    }

    // MARK: - 存读档界面

    /// mode: 0 = 读档，1 = 存档。
    func openSave(_ mode: Int32) {
        guard let engine = engine else { return }
        _ = kzn_save_open(engine, mode)
    }

    func closeSave() {
        guard let engine = engine else { return }
        kzn_save_close(engine)
    }

    /// 回标题（原版右键菜单的「タイトルに戻る」）。
    func gotoTitle() {
        guard let engine = engine else { return }
        _ = kzn_title(engine)
    }

    func setVolume(_ v: Float) {
        guard let engine = engine else { return }
        _ = kzn_set_volume(engine, v)
    }

    func setSpeed(_ cps: Float) {
        guard let engine = engine else { return }
        _ = kzn_set_speed(engine, cps)
    }

    func flipPage(_ delta: Int32) {
        guard let engine = engine else { return }
        kzn_save_page(engine, delta)
    }

    var saveActive: Bool {
        guard let engine = engine else { return false }
        return kzn_save_active(engine) != 0
    }

    deinit {
        display?.invalidate()
        if let e = engine { kzn_free(e) }
    }
}