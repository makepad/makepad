import Foundation
import Speech
import AVFoundation
import CoreMedia

struct CSegment {
    var text: UnsafeMutablePointer<CChar>?
    var start_ms: Int64
    var end_ms: Int64
}

func resolveLocale(_ langStr: String) -> Locale {
    let defaults: [String: String] = [
        "en": "en-US",  "fr": "fr-FR",  "de": "de-DE",  "es": "es-ES",
        "it": "it-IT",  "pt": "pt-BR",  "zh": "zh-CN",  "ja": "ja-JP",
        "ko": "ko-KR",  "yue": "yue-CN",
    ]
    return Locale(identifier: defaults[langStr] ?? langStr)
}

func runAsyncSync<T: Sendable>(_ body: @escaping @Sendable () async throws -> T) throws -> T {
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

@_cdecl("apple_speech_transcribe")
func transcribe(
    _ samples: UnsafePointer<Float>,
    _ sampleCount: Int64,
    _ lang: UnsafePointer<CChar>,
    _ outCount: UnsafeMutablePointer<Int32>,
    _ outSegments: UnsafeMutablePointer<OpaquePointer?>
) -> Int32 {
    let langStr = String(cString: lang)
    let requestedLocale = resolveLocale(langStr)
    let count = Int(sampleCount)

    NSLog("[speech_bridge] transcribe: %d samples (%.2fs), locale=%@",
          count, Double(count) / 16000.0, requestedLocale.identifier(.bcp47))

    if count == 0 {
        outCount.pointee = 0
        outSegments.pointee = nil
        return 0
    }

    let samplesCopy = Array(UnsafeBufferPointer(start: samples, count: count))

    do {
        let segments: [(String, Int64, Int64)] = try runAsyncSync {
            let locale = await SpeechTranscriber.supportedLocale(equivalentTo: requestedLocale)
                ?? requestedLocale
            if locale.identifier(.bcp47) != requestedLocale.identifier(.bcp47) {
                NSLog("[speech_bridge] transcribe: using locale fallback %@ -> %@",
                      requestedLocale.identifier(.bcp47), locale.identifier(.bcp47))
            }
            let preset = SpeechTranscriber.Preset(
                transcriptionOptions: [],
                reportingOptions: [],
                attributeOptions: [.audioTimeRange]
            )
            let transcriber = SpeechTranscriber(locale: locale, preset: preset)
            let analyzer = SpeechAnalyzer(modules: [transcriber])

            let inputFormat = AVAudioFormat(
                commonFormat: .pcmFormatFloat32,
                sampleRate: 16000, channels: 1, interleaved: false
            )!

            let tempURL = URL(fileURLWithPath: NSTemporaryDirectory())
                .appendingPathComponent("makepad_speech_\(UUID().uuidString).caf")
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
            NSLog("[speech_bridge] analyzer: starting file analysis...")
            try await analyzer.start(inputAudioFile: inFile, finishAfterFile: true)
            NSLog("[speech_bridge] analyzer: done processing")
            NSLog("[speech_bridge] collector: waiting for results...")
            var segs: [(String, Int64, Int64)] = []
            for try await response in transcriber.results {
                let text = String(response.text.characters)
                let isFinal = response.isFinal
                NSLog("[speech_bridge] collector: final=%@, text='%@'",
                      isFinal ? "Y" : "N", text)
                if isFinal {
                    let range = response.range
                    let startMs = Int64(CMTimeGetSeconds(range.start) * 1000)
                    let endMs = Int64(CMTimeGetSeconds(
                        CMTimeAdd(range.start, range.duration)) * 1000)
                    if !text.isEmpty { segs.append((text, startMs, endMs)) }
                }
            }
            NSLog("[speech_bridge] collector: done, %d segments", segs.count)
            return segs
        }

        let n = Int32(segments.count)
        outCount.pointee = n
        if segments.isEmpty {
            outSegments.pointee = nil
            return 0
        }
        let ptr = UnsafeMutablePointer<CSegment>.allocate(capacity: segments.count)
        for (i, (text, startMs, endMs)) in segments.enumerated() {
            ptr[i] = CSegment(text: strdup(text), start_ms: startMs, end_ms: endMs)
        }
        outSegments.pointee = OpaquePointer(ptr)
        return 0
    } catch {
        NSLog("[speech_bridge] transcribe ERROR: %@", error.localizedDescription)
        outCount.pointee = 0
        outSegments.pointee = nil
        return -1
    }
}

@_cdecl("apple_speech_free_segments")
func freeSegments(_ ptr: OpaquePointer?, _ count: Int32) {
    guard let rawPtr = ptr else { return }
    let typed = UnsafeMutablePointer<CSegment>(rawPtr)
    for i in 0..<Int(count) {
        if let text = typed[i].text { free(text) }
    }
    typed.deallocate()
}

@_cdecl("apple_speech_ensure_model")
func ensureModel(_ lang: UnsafePointer<CChar>) -> Int32 {
    let langStr = String(cString: lang)
    let requestedLocale = resolveLocale(langStr)

    do {
        try runAsyncSync {
            let locale = await SpeechTranscriber.supportedLocale(equivalentTo: requestedLocale)
                ?? requestedLocale
            let supported = await SpeechTranscriber.supportedLocales
            let bcp47 = locale.identifier(.bcp47)
            guard supported.contains(where: { $0.identifier(.bcp47) == bcp47 }) else {
                throw NSError(domain: "speech_bridge", code: -2,
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
        NSLog("[speech_bridge] ensureModel error: %@", error.localizedDescription)
        return -1
    }
}
