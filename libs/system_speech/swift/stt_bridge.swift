import Foundation
import Speech
import AVFoundation
import CoreMedia

// The STT half of makepad-system-speech's Apple bridge: SpeechAnalyzer (macOS 26 /
// iOS 26) over caller-recorded 16 kHz mono PCM. Symbols are prefixed `mss_` so
// this static library can coexist with any other Swift bridge in the binary.

struct MssSegment {
    var text: UnsafeMutablePointer<CChar>?
    var start_ms: Int64
    var end_ms: Int64
}

func mssResolveLocale(_ tag: String) -> Locale {
    let defaults: [String: String] = [
        "en": "en-US",  "fr": "fr-FR",  "de": "de-DE",  "es": "es-ES",
        "it": "it-IT",  "pt": "pt-BR",  "nl": "nl-NL",  "zh": "zh-CN",
        "ja": "ja-JP",  "ko": "ko-KR",  "ru": "ru-RU",  "yue": "yue-CN",
    ]
    return Locale(identifier: defaults[tag] ?? tag.replacingOccurrences(of: "_", with: "-"))
}

func mssRunAsyncSync<T: Sendable>(_ body: @escaping @Sendable () async throws -> T) throws -> T {
    let sem = DispatchSemaphore(value: 0)
    let box = UnsafeMutablePointer<Result<T, Error>>.allocate(capacity: 1)
    box.initialize(to: .failure(NSError(domain: "uninit", code: 0)))
    DispatchQueue.global(qos: .userInitiated).async {
        Task {
            do { box.pointee = .success(try await body()) }
            catch { box.pointee = .failure(error) }
            sem.signal()
        }
    }
    sem.wait()
    let r = box.pointee
    box.deallocate()
    switch r {
    case .success(let v): return v
    case .failure(let e): throw e
    }
}

/// Recognize `sampleCount` floats of 16 kHz mono PCM. Returns 0 and an owned
/// array of `MssSegment` (release with `mss_stt_free_segments`), or a
/// negative code: -1 recognition failed, -2 locale unsupported.
@_cdecl("mss_stt_transcribe")
func mss_stt_transcribe(
    _ samples: UnsafePointer<Float>,
    _ sampleCount: Int64,
    _ lang: UnsafePointer<CChar>,
    _ wantTimestamps: Int32,
    _ outCount: UnsafeMutablePointer<Int32>,
    _ outSegments: UnsafeMutablePointer<OpaquePointer?>
) -> Int32 {
    let requestedLocale = mssResolveLocale(String(cString: lang))
    let count = Int(sampleCount)
    outCount.pointee = 0
    outSegments.pointee = nil
    if count == 0 { return 0 }

    let samplesCopy = Array(UnsafeBufferPointer(start: samples, count: count))

    do {
        let segments: [(String, Int64, Int64)] = try mssRunAsyncSync {
            let locale = await SpeechTranscriber.supportedLocale(equivalentTo: requestedLocale)
                ?? requestedLocale
            let preset = SpeechTranscriber.Preset(
                transcriptionOptions: [],
                reportingOptions: [],
                attributeOptions: wantTimestamps != 0 ? [.audioTimeRange] : []
            )
            let transcriber = SpeechTranscriber(locale: locale, preset: preset)
            let analyzer = SpeechAnalyzer(modules: [transcriber])

            let inputFormat = AVAudioFormat(
                commonFormat: .pcmFormatFloat32,
                sampleRate: 16000, channels: 1, interleaved: false
            )!

            // SpeechAnalyzer's file entry point is the one that finishes
            // deterministically after the last sample; write the PCM to a
            // temporary CAF and hand it that.
            let tempURL = URL(fileURLWithPath: NSTemporaryDirectory())
                .appendingPathComponent("makepad_mss_\(UUID().uuidString).caf")
            defer { try? FileManager.default.removeItem(at: tempURL) }

            let outFile = try AVAudioFile(forWriting: tempURL, settings: inputFormat.settings)
            let allFrames = AVAudioFrameCount(count)
            let outBuf = AVAudioPCMBuffer(pcmFormat: inputFormat, frameCapacity: allFrames)!
            outBuf.frameLength = allFrames
            samplesCopy.withUnsafeBufferPointer { buf in
                memcpy(outBuf.floatChannelData![0], buf.baseAddress!, count * MemoryLayout<Float>.size)
            }
            try outFile.write(from: outBuf)

            let inFile = try AVAudioFile(forReading: tempURL)
            try await analyzer.start(inputAudioFile: inFile, finishAfterFile: true)
            var segs: [(String, Int64, Int64)] = []
            for try await response in transcriber.results {
                let text = String(response.text.characters)
                if response.isFinal {
                    let range = response.range
                    let startMs = Int64(CMTimeGetSeconds(range.start) * 1000)
                    let endMs = Int64(CMTimeGetSeconds(CMTimeAdd(range.start, range.duration)) * 1000)
                    if !text.isEmpty { segs.append((text, startMs, endMs)) }
                }
            }
            return segs
        }

        outCount.pointee = Int32(segments.count)
        if segments.isEmpty { return 0 }
        let ptr = UnsafeMutablePointer<MssSegment>.allocate(capacity: segments.count)
        for (i, (text, startMs, endMs)) in segments.enumerated() {
            ptr[i] = MssSegment(text: strdup(text), start_ms: startMs, end_ms: endMs)
        }
        outSegments.pointee = OpaquePointer(ptr)
        return 0
    } catch {
        NSLog("[makepad-system-speech] stt transcribe: %@", error.localizedDescription)
        return -1
    }
}

@_cdecl("mss_stt_free_segments")
func mss_stt_free_segments(_ ptr: OpaquePointer?, _ count: Int32) {
    guard let rawPtr = ptr else { return }
    let typed = UnsafeMutablePointer<MssSegment>(rawPtr)
    for i in 0..<Int(count) {
        if let text = typed[i].text { free(text) }
    }
    typed.deallocate()
}

/// Make sure the on-device model for `lang` is installed (downloading it if
/// needed). 0 ready, -2 locale unsupported, -1 other failure.
@_cdecl("mss_stt_prepare")
func mss_stt_prepare(_ lang: UnsafePointer<CChar>) -> Int32 {
    let requestedLocale = mssResolveLocale(String(cString: lang))
    do {
        try mssRunAsyncSync {
            let locale = await SpeechTranscriber.supportedLocale(equivalentTo: requestedLocale)
                ?? requestedLocale
            let supported = await SpeechTranscriber.supportedLocales
            let bcp47 = locale.identifier(.bcp47)
            guard supported.contains(where: { $0.identifier(.bcp47) == bcp47 }) else {
                throw NSError(domain: "mss", code: -2,
                              userInfo: [NSLocalizedDescriptionKey: "Unsupported: \(bcp47)"])
            }
            let installed = await SpeechTranscriber.installedLocales
            if !installed.contains(where: { $0.identifier(.bcp47) == bcp47 }) {
                let preset = SpeechTranscriber.Preset(
                    transcriptionOptions: [], reportingOptions: [], attributeOptions: [])
                let t = SpeechTranscriber(locale: locale, preset: preset)
                if let dl = try await AssetInventory.assetInstallationRequest(supporting: [t]) {
                    try await dl.downloadAndInstall()
                }
            }
        }
        return 0
    } catch let e as NSError where e.code == -2 { return -2 }
    catch {
        NSLog("[makepad-system-speech] stt prepare: %@", error.localizedDescription)
        return -1
    }
}
