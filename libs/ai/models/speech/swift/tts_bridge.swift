import AVFoundation
import Foundation

/// Accumulates the PCM the synthesizer hands back, buffer by buffer.
private final class Rendered {
    var samples: [Float] = []
    var sampleRate: Double = 0
}

/// Render `text` to mono float PCM and return an owned buffer.
///
/// Returns null on failure. The caller must release the result with
/// `apple_tts_free`. Unlike `AVSpeechSynthesizer.speak`, this never touches an
/// output device — the samples go back to Rust so Makepad's audio output owns
/// playback.
@_cdecl("apple_tts_synthesize")
public func apple_tts_synthesize(
    _ text: UnsafePointer<CChar>,
    _ voice: UnsafePointer<CChar>?,
    _ rate: Float,
    _ outLen: UnsafeMutablePointer<Int32>,
    _ outRate: UnsafeMutablePointer<Float>
) -> UnsafeMutablePointer<Float>? {
    outLen.pointee = 0
    outRate.pointee = 0

    let string = String(cString: text)
    if string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        return nil
    }

    let utterance = AVSpeechUtterance(string: string)
    if let voice, let selected = AVSpeechSynthesisVoice(identifier: String(cString: voice)) {
        utterance.voice = selected
    } else {
        utterance.voice = AVSpeechSynthesisVoice(language: "en-US")
    }
    if rate > 0 {
        utterance.rate = rate
    }

    let synthesizer = AVSpeechSynthesizer()
    let rendered = Rendered()
    let finished = DispatchSemaphore(value: 0)
    var signalled = false

    // Buffers arrive on an internal queue; a zero-length buffer terminates the run.
    synthesizer.write(utterance) { buffer in
        guard let pcm = buffer as? AVAudioPCMBuffer else { return }
        let frames = Int(pcm.frameLength)
        if frames == 0 {
            if !signalled {
                signalled = true
                finished.signal()
            }
            return
        }
        rendered.sampleRate = pcm.format.sampleRate
        if let channels = pcm.floatChannelData {
            rendered.samples.append(contentsOf: UnsafeBufferPointer(start: channels[0], count: frames))
        } else if let channels = pcm.int16ChannelData {
            let source = UnsafeBufferPointer(start: channels[0], count: frames)
            rendered.samples.append(contentsOf: source.map { Float($0) / 32768.0 })
        }
    }

    // `write` delivers its buffers through the main run loop. Blocking the main
    // thread on the semaphore therefore deadlocks and yields zero buffers — so
    // pump the run loop when we are on it, and only block when we are not.
    if Thread.isMainThread {
        let deadline = Date().addingTimeInterval(30)
        while !signalled, Date() < deadline {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02))
        }
    } else {
        _ = finished.wait(timeout: .now() + 30)
    }
    // The synthesizer must outlive its callbacks.
    withExtendedLifetime(synthesizer) {}

    if rendered.samples.isEmpty || rendered.sampleRate <= 0 {
        return nil
    }

    let count = rendered.samples.count
    let out = UnsafeMutablePointer<Float>.allocate(capacity: count)
    rendered.samples.withUnsafeBufferPointer { source in
        out.initialize(from: source.baseAddress!, count: count)
    }
    outLen.pointee = Int32(count)
    outRate.pointee = Float(rendered.sampleRate)
    return out
}

@_cdecl("apple_tts_free")
public func apple_tts_free(_ ptr: UnsafeMutablePointer<Float>?) {
    ptr?.deallocate()
}
