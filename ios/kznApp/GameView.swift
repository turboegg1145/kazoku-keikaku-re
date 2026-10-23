import SwiftUI
import Metal
import QuartzCore
import UIKit

/// 一开始跑哪个剧本。正式版应该进 START.ISF（标题画面），
/// 但标题菜单的 0x48 存读档还没接，先用第一章验证画面/声音/点击。
let kStartScript = "SNR00.ISF"

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

    private func setupEngine() {
        // TrollStore 安装后，游戏数据放 Documents（用文件 App / AFC 拷进去）。
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].path
        let font = Bundle.main.path(forResource: "DroidSansFallbackFull", ofType: "ttf")
            ?? Bundle.main.path(forResource: "NotoSansJP-Regular", ofType: "otf")
        guard let font = font else {
            print("[kzn] 找不到字体资源")
            return
        }
        engine = kzn_open(docs, 1, font)
        guard let engine = engine else {
            print("[kzn] 打不开游戏目录: \(docs)")
            return
        }
        if kzn_start(engine, kStartScript) != 0 {
            var buf = [CChar](repeating: 0, count: 256)
            kzn_error(engine, &buf, 256)
            print("[kzn] 启动失败: \(String(cString: buf))")
        }
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
    }

    deinit {
        display?.invalidate()
        if let e = engine { kzn_free(e) }
    }
}