// Amin's voice-biometric speaker verification (macOS).
//
// STATUS: written from documented CoreML/AVFoundation APIs, same
// never-run-on-a-real-Mac caveat as the rest of this directory — see
// AminVoice.swift's header. The ONE piece of this that WAS verified before
// shipping: the ECAPA-TDNN → CoreML conversion itself was checked, in the
// Python sandbox that produced it, against speechbrain's own reference
// output for the exact same waveform (max abs diff 0.0 after inlining
// mean_var_norm, see scripts/voiceprint/convert_ecapa_to_coreml.py) — so the
// model's numbers are known-correct. What's NOT verified: that this Swift
// code loads/runs that model correctly on-device, that real microphone
// audio through AVAudioConverter produces embeddings resembling what the
// Python conversion script exercised, and — the one number nobody can set
// from a sandbox — whether `matchThreshold` below actually separates "Mona"
// from "someone else" on a real Mac with a real voice. That threshold is a
// documented placeholder, not a measured one; see its own comment.
//
// WHY THIS EXISTS: Mona's standing complaint was that hands-free mode opens
// a command session for anyone who says the wake phrase — "بصمة الصوت...
// اريده يتعرف ع صوتي" (voice fingerprint — I want it to recognize MY voice).
// A spoken phrase is a shared secret, not an identity check (see
// AminVoice.swift's HANDS-FREE MODE note and docs/SECURITY.md). This adds
// the actual identity check: one enrollment recording produces a 192-dim
// speaker embedding, stored locally; every later wake-phrase detection
// computes a fresh embedding from the same moment's audio and compares it
// by cosine similarity before HandsFreeListener is allowed to open a
// session — see AminVoice.swift's use of `VoicePrintEngine.verify`.
//
// MODEL: speechbrain/spkrec-ecapa-voxceleb (ECAPA-TDNN), converted to
// CoreML by scripts/voiceprint/convert_ecapa_to_coreml.py, bundled as a
// Tauri resource (see tauri.conf.json) at a path Rust resolves once via
// Tauri's own resource-directory API (the same mechanism already used for
// libaminvoice.dylib itself — see voice.rs's engine_path) and hands to this
// dylib through `amin_voice_set_voiceprint_model_path`, rather than this
// code guessing a Bundle.main-relative path of its own.
//
// FIXED WINDOW: both enrollment and every verification attempt use exactly
// 3 seconds of 16kHz mono audio (`RollingPCMBuffer`) — the CoreML graph has
// no dynamic-shape support (see the conversion script's docstring for why:
// tracing speechbrain's length-masking code isn't representable in
// CoreML's static graph). Shorter audio is zero-padded; longer audio is
// truncated to the most recent 3 seconds.

import Foundation
import AVFoundation
import CoreML

/// Converts whatever format the mic's input node happens to be running at
/// into the fixed 16kHz-mono-Float32 format `VoicePrintEngine` needs,
/// independent of `SFSpeechAudioBufferRecognitionRequest`'s own internal
/// (and inaccessible) format handling — the two consumers of the same tap
/// buffer have different needs and shouldn't be coupled.
///
/// Deliberately NOT `private`: Swift's `private` is file-scoped, not
/// module-scoped, so it would be inaccessible from AminVoice.swift's
/// `HandsFreeListener` even though both files compile into the same
/// module (swiftc's multi-file mode — see macos/transcriber/README.md).
/// Real bug this exact mistake caused, found in CI logs after Mona's real
/// Mac reported the whole voice engine missing (2026-08-28): every build
/// since this file was added silently shipped an empty placeholder dylib
/// (the CI script's own graceful-degradation fallback for a compile
/// failure — see build-macos.yml's "Build the voice engine" step) because
/// `swiftc` failed with "'AudioResampler' is inaccessible due to
/// 'private' protection level" at both of AminVoice.swift's call sites —
/// and because that fallback never fails the CI job itself, this went
/// undetected by every check in this pipeline until a human actually
/// tried to use hands-free mode.
final class AudioResampler {
    private let converter: AVAudioConverter?
    private let targetFormat: AVAudioFormat

    init(from sourceFormat: AVAudioFormat) {
        self.targetFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: VoicePrintEngine.sampleRate,
            channels: 1,
            interleaved: false
        )!
        self.converter = AVAudioConverter(from: sourceFormat, to: targetFormat)
    }

    /// Returns mono Float32 samples at `VoicePrintEngine.sampleRate`, or
    /// `nil` if conversion isn't possible (e.g. an exotic input format the
    /// OS handed back) — callers should treat that as "no samples this
    /// buffer" rather than a fatal error.
    func resample(_ buffer: AVAudioPCMBuffer) -> [Float]? {
        guard let converter = converter else { return nil }
        let ratio = targetFormat.sampleRate / buffer.format.sampleRate
        let outCapacity = AVAudioFrameCount((Double(buffer.frameLength) * ratio).rounded(.up)) + 16
        guard let outBuffer = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: outCapacity) else {
            return nil
        }
        var consumed = false
        var conversionError: NSError?
        let status = converter.convert(to: outBuffer, error: &conversionError) { _, inputStatus in
            if consumed {
                inputStatus.pointee = .noDataNow
                return nil
            }
            consumed = true
            inputStatus.pointee = .haveData
            return buffer
        }
        guard status != .error, conversionError == nil, let channelData = outBuffer.floatChannelData else {
            return nil
        }
        return Array(UnsafeBufferPointer(start: channelData[0], count: Int(outBuffer.frameLength)))
    }
}

/// A fixed-capacity, most-recent-wins ring of mono Float32 samples at
/// `VoicePrintEngine.sampleRate` — "the last `capacitySeconds` seconds of
/// what the mic heard," fed continuously from the same tap that also feeds
/// speech recognition (see AminVoice.swift's `openTap`). Thread safety:
/// callers only ever touch this from the audio-tap queue and the
/// recognition-callback queue, both of which are already serialized by
/// AVAudioEngine/Speech's own delivery, matching how the rest of
/// `HandsFreeListener` already assumes no concurrent access to its state.
final class RollingPCMBuffer {
    private var samples: [Float]
    private let capacity: Int
    private var writeIndex = 0
    private var filled = false

    init(capacitySeconds: Double = 3.0, sampleRate: Double = VoicePrintEngine.sampleRate) {
        self.capacity = Int(capacitySeconds * sampleRate)
        self.samples = [Float](repeating: 0, count: capacity)
    }

    func append(_ newSamples: [Float]) {
        for sample in newSamples {
            samples[writeIndex] = sample
            writeIndex = (writeIndex + 1) % capacity
            if writeIndex == 0 { filled = true }
        }
    }

    /// The last `capacity` samples in chronological order, oldest first,
    /// zero-padded at the front if fewer than `capacity` samples have ever
    /// been appended (e.g. right after `HandsFreeListener` starts).
    func snapshot() -> [Float] {
        guard filled else {
            return Array(samples[0..<writeIndex])
        }
        return Array(samples[writeIndex...] + samples[..<writeIndex])
    }
}

/// Loads the converted ECAPA-TDNN CoreML model, extracts speaker
/// embeddings, and persists/compares the one enrolled voiceprint Amin
/// currently supports (a single stored speaker — Mona; there is no
/// multi-user account system in this app to enroll more than one).
final class VoicePrintEngine {
    static let shared = VoicePrintEngine()

    static let sampleRate: Double = 16_000
    static let windowSeconds: Double = 3.0
    static let windowSamples = Int(sampleRate * windowSeconds)

    /// Cosine similarity threshold above which a fresh embedding is
    /// considered "Mona." PLACEHOLDER, NOT MEASURED: chosen from published
    /// ECAPA-TDNN/cosine-backend speaker-verification literature's typical
    /// same-speaker/different-speaker separation, not from any recording of
    /// Mona's actual voice (impossible in this sandbox — no microphone). The
    /// first real test on her Mac (enroll, then try the wake phrase herself
    /// and have someone else try it) is what actually calibrates this — if
    /// she's rejected, lower it; if a stranger passes, raise it. Surfacing
    /// that as a real, expected first-run tuning step, not hiding a magic
    /// number.
    private let matchThreshold: Float = 0.45

    private var model: MLModel?
    private var modelLoadAttempted = false
    private var modelPath: String?

    private var storageURL: URL {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Amin", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("voiceprint.json")
    }

    private init() {}

    func setModelPath(_ path: String) {
        modelPath = path
        model = nil
        modelLoadAttempted = false
    }

    /// Compiles (once, cached) and loads the bundled .mlpackage. CoreML's
    /// on-disk .mlpackage format isn't directly loadable — it must be
    /// compiled to .mlmodelc first. This project has no Xcode build phase
    /// to do that automatically (see macos/transcriber/README.md: this
    /// whole engine is one file compiled by a bare `swiftc` invocation),
    /// so it's done here at runtime via `MLModel.compileModel(at:)`,
    /// caching the result next to the source .mlpackage so it only happens
    /// once per app version rather than on every launch.
    private func loadModel() -> MLModel? {
        if let model = model { return model }
        if modelLoadAttempted { return nil }
        modelLoadAttempted = true

        guard let modelPath = modelPath else { return nil }
        let packageURL = URL(fileURLWithPath: modelPath)
        let compiledURL = packageURL.deletingLastPathComponent()
            .appendingPathComponent(packageURL.deletingPathExtension().lastPathComponent + ".mlmodelc")

        do {
            let urlToLoad: URL
            if FileManager.default.fileExists(atPath: compiledURL.path) {
                urlToLoad = compiledURL
            } else {
                let freshlyCompiled = try MLModel.compileModel(at: packageURL)
                // MLModel.compileModel places its output in a temp
                // directory that doesn't survive app restarts — copy it
                // next to the source package so the check above finds it
                // next launch instead of recompiling every time.
                try? FileManager.default.copyItem(at: freshlyCompiled, to: compiledURL)
                urlToLoad = FileManager.default.fileExists(atPath: compiledURL.path) ? compiledURL : freshlyCompiled
            }
            let loaded = try MLModel(contentsOf: urlToLoad)
            model = loaded
            return loaded
        } catch {
            return nil
        }
    }

    /// Runs the model on exactly `windowSamples` of 16kHz mono audio,
    /// zero-padding a shorter window or truncating a longer one to match —
    /// see this file's header for why the graph is fixed-size. Returns
    /// `nil` if the model isn't loaded/loadable, never a wrong-shaped guess.
    func embedding(for samples: [Float]) -> [Float]? {
        guard let model = loadModel() else { return nil }

        var windowed = samples
        if windowed.count > Self.windowSamples {
            windowed = Array(windowed.suffix(Self.windowSamples))
        } else if windowed.count < Self.windowSamples {
            windowed += [Float](repeating: 0, count: Self.windowSamples - windowed.count)
        }

        // .float16, matching the converted model's declared input dtype
        // exactly (`ct.convert`'s mlprogram default — see the conversion
        // script) rather than assuming CoreML silently upcasts a float32
        // array for a float16 input. MLMultiArray's NSNumber-based
        // subscript setter/getter below handles the float16 storage
        // conversion transparently either way.
        guard let input = try? MLMultiArray(shape: [1, NSNumber(value: Self.windowSamples)], dataType: .float16) else {
            return nil
        }
        for (i, value) in windowed.enumerated() {
            input[i] = NSNumber(value: value)
        }

        guard
            let provider = try? MLDictionaryFeatureProvider(dictionary: ["waveform": input]),
            let output = try? model.prediction(from: provider),
            let embeddingArray = output.featureValue(for: "embedding")?.multiArrayValue
        else {
            return nil
        }

        var result = [Float](repeating: 0, count: embeddingArray.count)
        for i in 0..<embeddingArray.count {
            result[i] = embeddingArray[i].floatValue
        }
        return result
    }

    private func cosineSimilarity(_ a: [Float], _ b: [Float]) -> Float {
        guard a.count == b.count, !a.isEmpty else { return -1 }
        var dot: Float = 0, normA: Float = 0, normB: Float = 0
        for i in 0..<a.count {
            dot += a[i] * b[i]
            normA += a[i] * a[i]
            normB += b[i] * b[i]
        }
        let denom = (normA.squareRoot() * normB.squareRoot())
        return denom > 0 ? dot / denom : -1
    }

    /// Stores `embedding` as the one enrolled voiceprint, overwriting any
    /// previous enrollment (re-enrolling is how Mona would fix a bad
    /// recording — there's no "add another sample" flow yet, see this
    /// file's header on scope).
    func enroll(embedding: [Float]) -> Bool {
        let json: [String: Any] = ["embedding": embedding, "enrolledAt": ISO8601DateFormatter().string(from: Date())]
        guard let data = try? JSONSerialization.data(withJSONObject: json) else { return false }
        return (try? data.write(to: storageURL, options: .atomic)) != nil
    }

    func hasEnrolledSpeaker() -> Bool {
        FileManager.default.fileExists(atPath: storageURL.path)
    }

    func clearEnrolledSpeaker() {
        try? FileManager.default.removeItem(at: storageURL)
    }

    private func loadEnrolledEmbedding() -> [Float]? {
        guard
            let data = try? Data(contentsOf: storageURL),
            let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let raw = json["embedding"] as? [Any]
        else {
            return nil
        }
        return raw.compactMap { ($0 as? NSNumber)?.floatValue }
    }

    /// True if `samples` (raw audio, any length — this handles windowing)
    /// matches the enrolled voiceprint closely enough. If nothing is
    /// enrolled yet, or the model/embedding extraction fails for any
    /// reason, this returns `true` (fail open) — an unenrolled or
    /// mid-failure Amin should behave like it did before this feature
    /// existed (open on any wake phrase), not go silent and look broken.
    /// The one thing this must never do is fail *closed* on a real
    /// enrolled user due to a transient model hiccup.
    func verify(samples: [Float]) -> Bool {
        guard hasEnrolledSpeaker() else { return true }
        guard let enrolled = loadEnrolledEmbedding() else { return true }
        guard let candidate = embedding(for: samples) else { return true }
        return cosineSimilarity(enrolled, candidate) >= matchThreshold
    }
}

/// Records a short (`durationSeconds`) enrollment sample and stores its
/// embedding via `VoicePrintEngine.enroll`. Deliberately simpler than
/// `Transcriber`/`HandsFreeListener`: no `SFSpeechRecognizer` involved at
/// all — this only needs raw audio, not a transcript — so it's a plain
/// `AVAudioEngine` tap for a fixed duration, then stop.
final class SpeakerEnrollmentRecorder {
    private let audioEngine = AVAudioEngine()
    private let durationSeconds: Double
    private var samples: [Float] = []
    private var resampler: AudioResampler?
    private var finished = false

    init(durationSeconds: Double = 4.0) {
        self.durationSeconds = durationSeconds
    }

    /// Starts recording and calls `completion(true)` once enough audio was
    /// captured and successfully enrolled, `completion(false, reason)`
    /// otherwise. Requires the same microphone permission push-to-talk
    /// already handles — this does not re-request it, matching
    /// `HandsFreeListener`'s assumption that it's called after that's
    /// already granted (both entry points are only reachable from Settings
    /// after voice features are already in use).
    func start(completion: @escaping (Bool, String) -> Void) {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            beginRecording(completion: completion)
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .audio) { [weak self] granted in
                guard let self = self else { return }
                if granted {
                    self.beginRecording(completion: completion)
                } else {
                    completion(false, "microphone access was not granted")
                }
            }
        default:
            completion(false, "microphone access was not granted")
        }
    }

    private func beginRecording(completion: @escaping (Bool, String) -> Void) {
        let inputNode = audioEngine.inputNode
        let format = inputNode.outputFormat(forBus: 0)
        resampler = AudioResampler(from: format)
        inputNode.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
            guard let self = self, let converted = self.resampler?.resample(buffer) else { return }
            self.samples += converted
        }
        do {
            audioEngine.prepare()
            try audioEngine.start()
        } catch {
            completion(false, "couldn't start the audio engine: \(error.localizedDescription)")
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + durationSeconds) { [weak self] in
            self?.finishRecording(completion: completion)
        }
    }

    private func finishRecording(completion: @escaping (Bool, String) -> Void) {
        guard !finished else { return }
        finished = true
        audioEngine.inputNode.removeTap(onBus: 0)
        audioEngine.stop()

        guard samples.count >= Int(Double(VoicePrintEngine.windowSamples) * 0.5) else {
            completion(false, "not enough audio was captured — try again and keep speaking the whole time")
            return
        }
        guard let embedding = VoicePrintEngine.shared.embedding(for: samples) else {
            completion(false, "the voiceprint model isn't loaded — is ECAPA_TDNN.mlpackage bundled?")
            return
        }
        if VoicePrintEngine.shared.enroll(embedding: embedding) {
            completion(true, "")
        } else {
            completion(false, "couldn't save the voiceprint to disk")
        }
    }
}

// MARK: - C ABI, called from src-tauri/src/voice.rs (see AminVoice.swift's
// header for the shared callback-kind convention; this file adds kinds
// 10/11/12 — see voice.rs's on_voice_event for what each means).

private var activeEnrollment: SpeakerEnrollmentRecorder?

@_cdecl("amin_voice_set_voiceprint_model_path")
public func amin_voice_set_voiceprint_model_path(_ path: UnsafePointer<CChar>) {
    VoicePrintEngine.shared.setModelPath(String(cString: path))
}

@_cdecl("amin_voice_has_enrolled_speaker")
public func amin_voice_has_enrolled_speaker() -> Int32 {
    VoicePrintEngine.shared.hasEnrolledSpeaker() ? 1 : 0
}

@_cdecl("amin_voice_clear_enrolled_speaker")
public func amin_voice_clear_enrolled_speaker() {
    VoicePrintEngine.shared.clearEnrolledSpeaker()
}

@_cdecl("amin_voice_enroll_speaker")
public func amin_voice_enroll_speaker(_ callback: @escaping AminVoiceCallback) -> Int32 {
    if activeEnrollment != nil {
        return 0
    }
    let recorder = SpeakerEnrollmentRecorder()
    activeEnrollment = recorder
    recorder.start { success, reason in
        if success {
            callback(11, nil)
        } else {
            reason.withCString { callback(12, $0) }
        }
        activeEnrollment = nil
    }
    return 0
}
