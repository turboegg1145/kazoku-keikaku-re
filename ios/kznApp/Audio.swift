import AVFoundation
import Foundation

/// 音频：主线程把 `kzn_audio` 的 PCM 拉进环形缓冲，音频回调只读缓冲。
///
/// 这样 Rust 侧的 `Rc`/`RefCell` 永远不会被音频线程碰到。
final class GameAudio {
    private let engine = AVAudioEngine()
    private var node: AVAudioSourceNode!
    private let lock = NSLock()
    private var ring: [Float] = []
    private var read = 0
    private var write = 0
    private var count = 0
    private let rate: Double = 44100
    private let channels = 2

    init() {
        let capacity = Int(rate * 0.5) * channels   // 500ms
        ring = [Float](repeating: 0, count: capacity)

        let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
        node = AVAudioSourceNode { [weak self] _, _, frameCount, audioBufferList -> OSStatus in
            guard let self = self else { return noErr }
            let abl = UnsafeMutableAudioBufferListPointer(audioBufferList)
            self.lock.lock()
            for frame in 0..<Int(frameCount) {
                for ch in 0..<abl.count {
                    let buf = abl[ch]
                    let ptr = buf.mData!.assumingMemoryBound(to: Float.self)
                    if self.count >= self.channels {
                        ptr[frame] = self.ring[self.read]
                        self.read = (self.read + 1) % self.ring.count
                        self.count -= 1
                    } else {
                        ptr[frame] = 0
                    }
                }
            }
            self.lock.unlock()
            return noErr
        }

        engine.attach(node)
        engine.connect(node, to: engine.mainMixerNode, format: format)
        try? engine.start()
    }

    /// 主线程调用：从 Rust 拉 `frames` 帧，塞进环形缓冲。
    func fill(_ k: OpaquePointer?) {
        guard let k = k else { return }
        var pcm = [Int16](repeating: 0, count: 2048)
        let n = Int(kzn_audio(k, &pcm, 1024, UInt32(rate)))
        guard n > 0 else { return }
        lock.lock()
        for i in 0..<n {
            if count >= ring.count { break }
            ring[write] = Float(pcm[i]) / 32768.0
            write = (write + 1) % ring.count
            count += 1
        }
        lock.unlock()
    }
}