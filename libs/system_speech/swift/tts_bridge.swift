import AVFoundation
import Foundation

// The TTS half of makepad-system-speech's Apple bridge: AVSpeechSynthesizer
// rendered to a PCM buffer (never to a device — Makepad's audio output owns
// playback). Symbols are prefixed `mss_`.

private final class MssRendered {
    var samples: [Float] = []
    var sampleRate: Double = 0
}

struct MssVoice {
    var id: UnsafeMutablePointer<CChar>?
    var name: UnsafeMutablePointer<CChar>?
    var language: UnsafeMutablePointer<CChar>?
    /// 0 unknown, 1 female, 2 male (AVSpeechSynthesisVoiceGender raw values).
    var gender: Int32
}

/// Render `text` to mono float PCM. `voice` is an `AVSpeechSynthesisVoice`
/// identifier or null (then `language`, a BCP-47 tag, picks the default
/// voice). `rate`/`pitch` are multipliers around 1.0. Returns null on failure;
/// release with `mss_tts_free`.
@_cdecl("mss_tts_synthesize")
public func mss_tts_synthesize(
    _ text: UnsafePointer<CChar>,
    _ voice: UnsafePointer<CChar>?,
    _ language: UnsafePointer<CChar>,
    _ rate: Float,
    _ pitch: Float,
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
        utterance.voice = AVSpeechSynthesisVoice(language: String(cString: language))
            ?? AVSpeechSynthesisVoice(language: "en-US")
    }
    // AVSpeechUtterance.rate is 0...1 with the default at 0.5; our 1.0 is that default.
    if rate > 0 && rate.isFinite {
        utterance.rate = min(max(AVSpeechUtteranceDefaultSpeechRate * rate, AVSpeechUtteranceMinimumSpeechRate),
                             AVSpeechUtteranceMaximumSpeechRate)
    }
    if pitch > 0 && pitch.isFinite {
        utterance.pitchMultiplier = min(max(pitch, 0.5), 2.0)
    }

    let synthesizer = AVSpeechSynthesizer()
    let rendered = MssRendered()
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

    // `write` delivers its buffers through the MAIN run loop, whichever thread
    // called it (pumping the caller's own loop was tried and delivers
    // nothing). Every UI app pumps main, so a worker just waits on the
    // semaphore; a caller ON main must pump instead of blocking, or it
    // deadlocks itself. A headless tool calling from a worker must keep its
    // main thread in CFRunLoopRun — see makepad-ai-hub's speech-roundtrip.
    if Thread.isMainThread {
        let deadline = Date().addingTimeInterval(30)
        while !signalled, Date() < deadline {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02))
        }
    } else {
        _ = finished.wait(timeout: .now() + 30)
    }
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

@_cdecl("mss_tts_free")
public func mss_tts_free(_ ptr: UnsafeMutablePointer<Float>?) {
    ptr?.deallocate()
}

/// Installed voices as an owned array of `MssVoice`; release with
/// `mss_tts_free_voices`.
@_cdecl("mss_tts_voices")
public func mss_tts_voices(_ outCount: UnsafeMutablePointer<Int32>) -> OpaquePointer? {
    let voices = AVSpeechSynthesisVoice.speechVoices()
    outCount.pointee = Int32(voices.count)
    if voices.isEmpty { return nil }
    let ptr = UnsafeMutablePointer<MssVoice>.allocate(capacity: voices.count)
    for (i, v) in voices.enumerated() {
        ptr[i] = MssVoice(
            id: strdup(v.identifier),
            name: strdup(v.name),
            language: strdup(v.language),
            gender: Int32(v.gender.rawValue)
        )
    }
    return OpaquePointer(ptr)
}

@_cdecl("mss_tts_free_voices")
public func mss_tts_free_voices(_ ptr: OpaquePointer?, _ count: Int32) {
    guard let rawPtr = ptr else { return }
    let typed = UnsafeMutablePointer<MssVoice>(rawPtr)
    for i in 0..<Int(count) {
        if let s = typed[i].id { free(s) }
        if let s = typed[i].name { free(s) }
        if let s = typed[i].language { free(s) }
    }
    typed.deallocate()
}
