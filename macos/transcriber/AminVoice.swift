// Amin's push-to-talk voice engine (macOS).
//
// STATUS: written from documented Speech/AVFoundation APIs. Compiled
// successfully in CI (.github/workflows/build-macos.yml builds this as a
// universal dylib and bundles it as a Tauri resource), loaded and called
// in-process from src-tauri/src/voice.rs via dlopen/dlsym (the `libloading`
// crate). Still NEVER RUN AGAINST A REAL MICROPHONE — there is no macOS, no
// Xcode, and no microphone in the sandbox this was written and wired up in.
// The macOS permission prompt and actual transcription accuracy remain
// unverified until tried on a real Mac. See docs/ARCHITECTURE.md's "Voice
// pipeline" section.
//
// This used to be a standalone executable, spawned as a child process from
// Rust and talked to over stdin/stdout JSON lines. Mona hit "couldn't start
// the audio engine" on a real Mac — exactly the failure mode this file's
// previous header called out as a known open risk before it was ever
// tried: a spawned CLI binary may not cleanly inherit the signed .app's
// microphone/speech-recognition TCC (privacy permission) grant the way
// code running inside that same process does. This rewrite is that fix —
// not a tweak to the subprocess version, a different architecture. The
// Swift code below now compiles to a small dylib that Amin's own process
// loads and calls directly, so every AVFoundation/Speech call executes as
// the same process macOS already prompted Mona to authorize, with no
// separate helper process and no separate TCC identity to worry about.
//
// C ABI, called from src-tauri/src/voice.rs via dlsym (there is no header —
// the two sides agree on this signature by convention, matched by hand):
//   amin_voice_start(callback) -> Int32     (0 = started, or already running)
//   amin_voice_stop()
//   amin_voice_speak(text, callback) -> Int32
//   amin_voice_stop_speaking()
// `callback` is `@convention(c) (Int32, UnsafePointer<CChar>?) -> Void`:
// kind 0 = partial transcript, 1 = final transcript, 2 = error (recognition
// side); kind 3 = speech started, 4 = speech finished (speak side, text is
// always null). The string is a NUL-terminated UTF-8 C string valid only
// for the duration of the call — the Rust side must copy it before
// returning.
//
// KNOWN LIMITATION: SFSpeechRecognizer is locale-based (one language per
// recognizer), not free code-switching — it does not natively handle the
// brief's "Arabic (Egyptian/MSA) mixed with English" the way a human
// listener would. This starts with a single locale (Arabic, Egypt) and
// that limitation is a real product conversation to have, not a bug to
// silently work around.

import Foundation
import Speech
import AVFoundation

public typealias AminVoiceCallback = @convention(c) (Int32, UnsafePointer<CChar>?) -> Void

/// One push-to-talk session at a time, matching the single `VoiceSession`
/// the Rust side manages. `amin_voice_start` is a no-op (not an error)
/// while one is already active — a key that auto-repeats while held
/// shouldn't start a second session.
private var activeTranscriber: Transcriber?

@_cdecl("amin_voice_start")
public func amin_voice_start(_ callback: @escaping AminVoiceCallback) -> Int32 {
    if activeTranscriber != nil {
        return 0
    }
    let transcriber = Transcriber(callback: callback)
    activeTranscriber = transcriber
    transcriber.start { [weak transcriber] in
        if activeTranscriber === transcriber {
            activeTranscriber = nil
        }
    }
    return 0
}

@_cdecl("amin_voice_stop")
public func amin_voice_stop() {
    activeTranscriber?.stop()
}

/// Text-to-speech for Amin's own replies (Mona asked for spoken output as
/// a priority, not just text in the chat log). Uses macOS's built-in
/// AVSpeechSynthesizer — on-device, no new API key or vendor, matching the
/// same "voice stays local" choice already made for speech recognition.
/// `text` must be a NUL-terminated UTF-8 C string. The callback receives
/// kind 3 when speech starts and kind 4 when it ends (text is always null
/// for these) — src-tauri/src/voice.rs maps those to
/// voice://speaking-started / voice://speaking-finished so the frontend
/// can track Amin's actual speaking state instead of guessing a duration.
private let speechSynthesizer = AVSpeechSynthesizer()
private var speechDelegate: SpeechDelegate?

@_cdecl("amin_voice_speak")
public func amin_voice_speak(_ text: UnsafePointer<CChar>, _ callback: @escaping AminVoiceCallback) -> Int32 {
    let utterance = AVSpeechUtterance(string: String(cString: text))
    // ar-SA: a broadly-understood Modern Standard Arabic voice. Amin's
    // replies are Arabic-first, so this covers the large majority of what
    // it says — English words mixed into a reply will still be read with
    // an Arabic accent, the same kind of single-locale limitation already
    // disclosed for speech *recognition* above, not silently pretended away.
    utterance.voice = AVSpeechSynthesisVoice(language: "ar-SA") ?? AVSpeechSynthesisVoice(language: "en-US")
    let delegate = SpeechDelegate(callback: callback)
    speechDelegate = delegate
    speechSynthesizer.delegate = delegate
    speechSynthesizer.speak(utterance)
    return 0
}

@_cdecl("amin_voice_stop_speaking")
public func amin_voice_stop_speaking() {
    speechSynthesizer.stopSpeaking(at: .immediate)
}

private final class SpeechDelegate: NSObject, AVSpeechSynthesizerDelegate {
    private let callback: AminVoiceCallback
    init(callback: @escaping AminVoiceCallback) { self.callback = callback }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didStart utterance: AVSpeechUtterance) {
        callback(3, nil)
    }
    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        callback(4, nil)
    }
    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
        callback(4, nil)
    }
}

private final class Transcriber {
    private let callback: AminVoiceCallback
    // ar-EG: Egyptian Arabic. See the code-switching limitation note above —
    // this single-locale choice is provisional, not a final decision.
    private let recognizer = SFSpeechRecognizer(locale: Locale(identifier: "ar-EG"))
    private let audioEngine = AVAudioEngine()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var finished = false
    private var onFinished: (() -> Void)?

    init(callback: @escaping AminVoiceCallback) {
        self.callback = callback
    }

    private func emit(_ kind: Int32, _ text: String) {
        text.withCString { cstr in callback(kind, cstr) }
    }

    /// Kicks off permission + recognition and returns immediately — never
    /// blocks the calling thread. `amin_voice_start` may be called from
    /// Rust's global-shortcut handler thread, which must stay responsive.
    /// Amin's own process already runs a real run loop (it's a GUI app),
    /// so dispatching to the main queue here is safe and delivered, unlike
    /// in the old standalone-CLI version which had no run loop pumping it.
    func start(onFinished: @escaping () -> Void) {
        self.onFinished = onFinished
        // Check the already-decided status first rather than calling
        // requestAuthorization unconditionally on every push-to-talk press.
        // Once Mona has answered the system prompt once, every later press
        // should go straight to listening — asking again on every single
        // press is exactly the "keeps sending me back to the permission
        // screen" bug she hit.
        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized:
            handleAuthorization(true)
        case .denied, .restricted:
            handleAuthorization(false)
        case .notDetermined:
            SFSpeechRecognizer.requestAuthorization { [weak self] status in
                DispatchQueue.main.async {
                    self?.handleAuthorization(status == .authorized)
                }
            }
        @unknown default:
            handleAuthorization(false)
        }
    }

    private func handleAuthorization(_ authorized: Bool) {
        guard authorized else {
            emit(2, "speech recognition permission was not granted")
            finish()
            return
        }
        guard let recognizer = recognizer, recognizer.isAvailable else {
            emit(2, "speech recognizer unavailable for the configured locale")
            finish()
            return
        }

        // Touching audioEngine.inputNode/.start() is what triggers the
        // *separate* microphone permission prompt the first time, and that
        // call can block the calling thread until the prompt is answered.
        // Doing this on the main thread froze Amin's whole window (the
        // spinning-wheel/beachball Mona hit) for as long as that system
        // dialog was up, since the main thread also drives the webview.
        // A background queue keeps the app responsive while she decides.
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.startEngine(recognizer: recognizer)
        }
    }

    /// Microphone access (AVFoundation/AVCaptureDevice) is a *separate* TCC
    /// permission from speech recognition (Speech framework) — Mona kept
    /// hitting the mic prompt again on every press even after the speech-
    /// recognition side was fixed to check its own status first, because
    /// that fix never touched this one. `audioEngine.start()` triggers this
    /// prompt implicitly the first time, with no way to check its status
    /// first — checking AVCaptureDevice's status explicitly here, the same
    /// pattern already used for speech recognition, is the fix.
    private func startEngine(recognizer: SFSpeechRecognizer) {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            beginRecognition(recognizer: recognizer)
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .audio) { [weak self] granted in
                if granted {
                    self?.beginRecognition(recognizer: recognizer)
                } else {
                    self?.emit(2, "microphone access was not granted")
                    self?.finish()
                }
            }
        case .denied, .restricted:
            emit(2, "microphone access was not granted")
            finish()
        @unknown default:
            emit(2, "microphone access was not granted")
            finish()
        }
    }

    private func beginRecognition(recognizer: SFSpeechRecognizer) {
        let req = SFSpeechAudioBufferRecognitionRequest()
        req.shouldReportPartialResults = true
        // Prefer on-device recognition when the OS supports it for this
        // locale, per the "voice stays local by default" principle in
        // docs/SECURITY.md. Falls back to Apple's server-based recognition
        // automatically if on-device isn't available for this locale/OS
        // version — that fallback is itself worth confirming, not assuming.
        req.requiresOnDeviceRecognition = recognizer.supportsOnDeviceRecognition
        request = req

        let inputNode = audioEngine.inputNode
        let format = inputNode.outputFormat(forBus: 0)
        inputNode.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
            req.append(buffer)
        }

        task = recognizer.recognitionTask(with: req) { [weak self] result, error in
            guard let self = self else { return }
            if let result = result {
                let text = result.bestTranscription.formattedString
                if result.isFinal {
                    self.emit(1, text)
                    self.finish()
                } else {
                    self.emit(0, text)
                }
            }
            if let error = error {
                self.emit(2, error.localizedDescription)
                self.finish()
            }
        }

        do {
            audioEngine.prepare()
            try audioEngine.start()
        } catch {
            emit(2, "couldn't start the audio engine: \(error.localizedDescription)")
            finish()
        }
    }

    /// Ends the current utterance. Safety net: if the recognizer never
    /// delivers a final result after `endAudio()` (no speech detected, a
    /// stuck recognition task), don't wait forever — whatever partial
    /// transcript already reached the frontend stays in its input box
    /// either way.
    func stop() {
        request?.endAudio()
        DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak self] in
            self?.finish()
        }
    }

    private func finish() {
        if finished { return }
        finished = true
        audioEngine.inputNode.removeTap(onBus: 0)
        audioEngine.stop()
        task?.cancel()
        onFinished?()
    }
}
