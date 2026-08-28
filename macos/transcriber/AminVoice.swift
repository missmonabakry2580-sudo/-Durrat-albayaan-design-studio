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
//   amin_voice_start_hands_free(wakePhrase, closePhrase, callback) -> Int32
//   amin_voice_stop_hands_free()
//   amin_voice_set_hands_free_speaking(text: UnsafePointer<CChar>?)
// `callback` is `@convention(c) (Int32, UnsafePointer<CChar>?) -> Void`:
// kind 0 = partial transcript, 1 = final transcript, 2 = error (recognition
// side); kind 3 = speech started (text is the sentence about to be spoken —
// see `amin_voice_set_hands_free_speaking`), 4 = speech finished (text
// always null); kind 5 = hands-free armed (passively watching for the wake
// phrase, text is always null), 6 = wake phrase heard — a command session
// just opened (text always null), 7 = the close phrase ended the command
// session (text always null; any command text before the close phrase was
// already sent as a normal kind-1 final), 8 = hands-free timed out after
// passiveModeTimeoutSeconds of no wake phrase (text always null; the audio
// engine is still technically running at this instant — see armPassive's
// comment for why the actual teardown happens on the Rust/frontend side
// instead of here), 9 = a real barge-in — Mona started talking over Amin's
// own reply (text is what she said; see HandsFreeListener's
// isLikelySelfEcho and armPassive/listenForCommand for why this fires
// instead of the recognition being discarded as an echo). 10 = the wake
// phrase was heard but rejected because the enrolled voiceprint didn't
// match (text always null; see VoicePrint.swift). 11 = speaker enrollment
// succeeded (text always null). 12 = speaker enrollment failed (text is
// why). The string is a NUL-terminated UTF-8 C string valid only for the
// duration of the call — the Rust side must copy it before returning.
//
// KNOWN LIMITATION: SFSpeechRecognizer is locale-based (one language per
// recognizer), not free code-switching — it does not natively handle the
// brief's "Arabic (Egyptian/MSA) mixed with English" the way a human
// listener would. This starts with a single locale (Arabic, Egypt) and
// that limitation is a real product conversation to have, not a bug to
// silently work around.
//
// HANDS-FREE MODE: Mona asked for Amin to be reachable without pressing
// anything — she says a wake phrase, Amin listens, she says a closing
// phrase (or just goes quiet) when she's done. The privacy trade-off she'd
// actually care about is real and disclosed, not hidden: while this mode
// is on, the microphone stays open continuously (macOS's own orange mic
// indicator will show the whole time), and the wake-phrase-watching phase
// is HARD-REQUIRED to run on-device (`requiresOnDeviceRecognition` forced
// true) — if the OS/locale can't do on-device recognition, hands-free mode
// refuses to start rather than silently streaming continuous audio to
// Apple's servers just to watch for a phrase. A spoken phrase is a shared
// secret, not an identity check — anyone in earshot who knows it can open
// a session. Voice-print/speaker verification (the actual fix for that) is
// a separate, not-yet-built phase — see docs/SECURITY.md.
//
// SELF-HEARING / BARGE-IN: the microphone stays live while Amin talks, so
// without some defense it would happily transcribe its own TTS voice
// coming out of the speakers — best case a wasted round trip, worst case
// Amin accidentally hearing its own reply contain the close phrase and
// closing the session on itself. Two layers, not one:
//
//   1. `openTap` enables `AVAudioInputNode.setVoiceProcessingEnabled`
//      (Apple's VoIP-grade acoustic echo cancellation) on the mic input,
//      which cancels the echo of whatever's coming out of the output
//      hardware — best-effort: non-fatal if the OS refuses it, and not
//      guaranteed to fully cancel every audio path.
//   2. `amin_voice_set_hands_free_speaking` (voice.rs's fix, called around
//      every speak_text call, both the on-device and ElevenLabs paths)
//      tells `HandsFreeListener` the actual sentence Amin is saying, not
//      just a mute flag. `isLikelySelfEcho` compares each recognized
//      utterance against that sentence: a close match (still leaking
//      through despite AEC) is discarded exactly like the old blanket
//      mute; a clearly different utterance is a real barge-in — Mona
//      talking over it on purpose — which stops playback (kind 9, handled
//      Rust-side in `on_voice_event`) and gets treated as her next command
//      instead of silently dropped. The audio engine and recognition task
//      keep running underneath the whole time; nothing needs restarting
//      when Amin stops talking (real or interrupted).
//
// VOICE-BIOMETRIC SPEAKER VERIFICATION (VoicePrint.swift): a spoken wake
// phrase is a shared secret, not an identity check — anyone in earshot who
// knows it could open a session, which is exactly the gap Mona flagged
// ("بصمة الصوت... اريده يتعرف ع صوتي"). `HandsFreeListener` now keeps a
// rolling 3-second buffer of the raw mic audio (`RollingPCMBuffer`,
// resampled to the 16kHz mono ECAPA-TDNN expects) alongside the existing
// speech-recognition tap, and — the moment the wake phrase is heard —
// checks that buffer against Mona's enrolled voiceprint
// (`VoicePrintEngine.verify`) before opening a session. A mismatch is
// treated exactly like not having heard the wake phrase at all (kind 10,
// stays passive); nothing enrolled yet, or the model failing to load for
// any reason, fails OPEN (behaves like before this feature existed) rather
// than silently locking Mona out of her own app — see `verify`'s doc
// comment.

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

/// One hands-free session at a time, matching the single `HandsFreeSession`
/// the Rust side manages (see voice.rs).
private var activeHandsFree: HandsFreeListener?

@_cdecl("amin_voice_start_hands_free")
public func amin_voice_start_hands_free(
    _ wakePhrase: UnsafePointer<CChar>,
    _ closePhrase: UnsafePointer<CChar>,
    _ callback: @escaping AminVoiceCallback
) -> Int32 {
    if activeHandsFree != nil {
        return 0
    }
    let listener = HandsFreeListener(
        wakePhrase: String(cString: wakePhrase),
        closePhrase: String(cString: closePhrase),
        callback: callback
    )
    activeHandsFree = listener
    listener.start()
    return 0
}

@_cdecl("amin_voice_stop_hands_free")
public func amin_voice_stop_hands_free() {
    activeHandsFree?.stop()
    activeHandsFree = nil
}

/// See the SELF-HEARING / BARGE-IN note above — called by voice.rs around
/// every speak_text call with the sentence Amin is about to say (or `nil`/
/// empty once it's done) so `HandsFreeListener` can tell its own echo
/// apart from a real barge-in instead of just muting everything. A no-op
/// if hands-free mode isn't running.
@_cdecl("amin_voice_set_hands_free_speaking")
public func amin_voice_set_hands_free_speaking(_ text: UnsafePointer<CChar>?) {
    activeHandsFree?.setSpeakingText(text.map { String(cString: $0) })
}

/// Text-to-speech for Amin's own replies (Mona asked for spoken output as
/// a priority, not just text in the chat log). Uses macOS's built-in
/// AVSpeechSynthesizer — on-device, no new API key or vendor, matching the
/// same "voice stays local" choice already made for speech recognition.
/// `text` must be a NUL-terminated UTF-8 C string. The callback receives
/// kind 3 (carrying that same text) when speech starts and kind 4 (text
/// always null) when it ends — src-tauri/src/voice.rs maps those to
/// voice://speaking-started / voice://speaking-finished so the frontend
/// can track Amin's actual speaking state instead of guessing a duration,
/// and also forwards kind 3's text into `set_hands_free_speaking` — see
/// this file's SELF-HEARING / BARGE-IN note.
private let speechSynthesizer = AVSpeechSynthesizer()
private var speechDelegate: SpeechDelegate?

/// `AVSpeechSynthesisVoice(language: "ar-SA")` returns whatever Apple
/// designates as *the* default voice for that language — on a Mac that
/// has never had a better Arabic voice downloaded, that's the low-quality
/// "Compact" one bundled with the OS, not the noticeably more natural
/// "Enhanced"/"Premium" voices Apple offers as a free (no account, no key)
/// download via System Settings → Accessibility → Spoken Content. Mona
/// found the Compact voice's speech "بشعة" (ugly) — this picks the best
/// quality tier actually installed among Arabic voices instead of
/// blindly taking Apple's default, so downloading a better voice there
/// (still entirely free, still on-device) actually gets used the moment
/// it's installed, no app change needed.
private func bestArabicVoice() -> AVSpeechSynthesisVoice? {
    let arabicVoices = AVSpeechSynthesisVoice.speechVoices().filter { $0.language.hasPrefix("ar") }
    func rank(_ voice: AVSpeechSynthesisVoice) -> Int {
        switch voice.quality {
        case .premium: return 2
        case .enhanced: return 1
        case .default: return 0
        @unknown default: return 0
        }
    }
    return arabicVoices.max(by: { rank($0) < rank($1) })
}

@_cdecl("amin_voice_speak")
public func amin_voice_speak(_ text: UnsafePointer<CChar>, _ callback: @escaping AminVoiceCallback) -> Int32 {
    let textString = String(cString: text)
    let utterance = AVSpeechUtterance(string: textString)
    // English words mixed into an Arabic reply will still be read with an
    // Arabic accent regardless of voice quality tier — the same single-
    // locale limitation already disclosed for speech *recognition* above,
    // not silently pretended away.
    utterance.voice = bestArabicVoice()
        ?? AVSpeechSynthesisVoice(language: "ar-SA")
        ?? AVSpeechSynthesisVoice(language: "en-US")
    // `textString` is threaded through so the kind-3 callback carries the
    // actual sentence, not null — see amin_voice_set_hands_free_speaking.
    let delegate = SpeechDelegate(text: textString, callback: callback)
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
    private let text: String
    private let callback: AminVoiceCallback
    init(text: String, callback: @escaping AminVoiceCallback) {
        self.text = text
        self.callback = callback
    }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didStart utterance: AVSpeechUtterance) {
        text.withCString { callback(3, $0) }
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
            // requestAccess's completion fires on an arbitrary system queue
            // (documented Apple behavior), not necessarily the same
            // .global(qos: .userInitiated) queue beginRecognition otherwise
            // always runs on via the .authorized branch above. Mona hit
            // exactly this on a real Mac, on exactly her first-ever grant on
            // a fresh build: "couldn't start the audio engine" (CoreAudio
            // error 268451843) right after tapping Allow — AVAudioEngine is
            // documented as needing a consistent calling context, and this
            // was the one path that didn't provide one. Explicitly hopping
            // back to that same queue here, instead of calling straight out
            // of the arbitrary completion queue, is the fix.
            AVCaptureDevice.requestAccess(for: .audio) { [weak self] granted in
                guard let self = self else { return }
                guard granted else {
                    self.emit(2, "microphone access was not granted")
                    self.finish()
                    return
                }
                DispatchQueue.global(qos: .userInitiated).async {
                    self.beginRecognition(recognizer: recognizer)
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

/// Continuous "say a phrase to open, say a phrase to close" listening — see
/// the HANDS-FREE MODE note in this file's header for the privacy trade-off.
/// Unlike `Transcriber` (one push-to-talk utterance, then done), this keeps
/// the audio engine running and cycles between two phases for as long as
/// hands-free mode is enabled: passively watching for the wake phrase
/// (nothing is sent anywhere as a command), then — once heard — capturing
/// one or more command utterances until the close phrase is heard or
/// `stop()` is called explicitly.
private final class HandsFreeListener {
    private let callback: AminVoiceCallback
    private let wakePhrase: String
    private let closePhrase: String
    private let recognizer = SFSpeechRecognizer(locale: Locale(identifier: "ar-EG"))
    private let audioEngine = AVAudioEngine()
    private var currentRequest: SFSpeechAudioBufferRecognitionRequest?
    private var currentTask: SFSpeechRecognitionTask?
    private var mode: Mode = .passive
    private var stopped = false
    /// See the SELF-HEARING / BARGE-IN note in this file's header. Holds
    /// the sentence Amin is currently speaking, or `nil` when it isn't
    /// speaking. Non-nil doesn't mean "ignore everything" the way the old
    /// blanket mute did — `isLikelySelfEcho` compares each recognized
    /// utterance against this text to tell an echo of it apart from Mona
    /// genuinely talking over it.
    private var currentlySpeakingText: String?

    /// When continuous passive (wake-phrase-only) listening last began, or
    /// `nil` while an active command session is open. Found in the field
    /// on 2026-08-28: Mona left hands-free on, moved to unrelated work
    /// (typing an official government letter in Chrome), and only then
    /// noticed the mic had stayed hot the whole time via macOS's own
    /// privacy indicator — a real trust problem for someone whose actual
    /// job involves confidential correspondence, not a hypothetical one.
    /// `armPassive` checks this against `passiveModeTimeoutSeconds` and,
    /// past it, emits kind 8 instead of re-arming — see its own comment for
    /// why the Rust/frontend layer does the actual stopping rather than
    /// this class calling `stop()` on itself.
    private var passiveModeStartedAt: Date?
    private let passiveModeTimeoutSeconds: TimeInterval = 15 * 60

    /// See this file's VOICE-BIOMETRIC SPEAKER VERIFICATION header note and
    /// VoicePrint.swift. Fed continuously from the same tap `openTap`
    /// installs for speech recognition; read at the moment the wake phrase
    /// is heard.
    private let voiceBuffer = RollingPCMBuffer()
    private var voiceResampler: AudioResampler?

    private enum Mode: Equatable { case passive, active }

    func setSpeakingText(_ text: String?) {
        currentlySpeakingText = (text?.isEmpty == false) ? text : nil
    }

    init(wakePhrase: String, closePhrase: String, callback: @escaping AminVoiceCallback) {
        self.wakePhrase = wakePhrase
        self.closePhrase = closePhrase
        self.callback = callback
    }

    private func emit(_ kind: Int32, _ text: String = "") {
        text.withCString { cstr in callback(kind, cstr) }
    }

    /// Case-folds and strips Arabic diacritics (tashkeel) so "يا أمِين" and
    /// "يا أمين" are treated as the same phrase — SFSpeechRecognizer's
    /// output isn't guaranteed to be diacritic-free.
    private func normalize(_ text: String) -> String {
        let diacritics = CharacterSet(
            charactersIn: "\u{064B}\u{064C}\u{064D}\u{064E}\u{064F}\u{0650}\u{0651}\u{0652}\u{0670}"
        )
        let stripped = text.unicodeScalars.filter { !diacritics.contains($0) }
        return String(String.UnicodeScalarView(stripped))
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
    }

    private func heard(_ phrase: String, in text: String) -> Bool {
        !phrase.isEmpty && normalize(text).contains(normalize(phrase))
    }

    /// Best-effort: drops everything from where the close phrase starts
    /// onward, so "افتحيلي كذا خلاص يا أمين" becomes "افتحيلي كذا" instead
    /// of sending the closing instruction itself as a command. Diacritic
    /// stripping can shift character offsets slightly, so this is an
    /// approximate cut, not an exact one — good enough for "drop the
    /// trailing close phrase", which is all this needs.
    private func textBeforePhrase(_ phrase: String, in text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedText = normalize(trimmed)
        guard let range = normalizedText.range(of: normalize(phrase)) else { return trimmed }
        let cut = min(normalizedText.distance(from: normalizedText.startIndex, to: range.lowerBound), trimmed.count)
        let idx = trimmed.index(trimmed.startIndex, offsetBy: cut)
        return String(trimmed[..<idx]).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// True if `heard` is very likely Amin's own TTS voice leaking back
    /// through the mic despite `openTap`'s echo cancellation, rather than
    /// Mona actually talking over it. Not exact-match: real speech
    /// recognition of echo-cancelled, speaker-reflected audio won't
    /// transcribe identically to Amin's own sentence even when it IS an
    /// echo, so containment alone would under-catch — a word-overlap ratio
    /// is the fallback for that case. Deliberately biased toward "treat as
    /// echo" on a tie: a missed barge-in just means Mona repeats herself
    /// (harmless, familiar); a false barge-in makes Amin cut itself off
    /// having heard nothing, which is a stranger, more confusing failure.
    private func isLikelySelfEcho(_ heard: String) -> Bool {
        guard let speaking = currentlySpeakingText, !speaking.isEmpty else { return false }
        let heardNorm = normalize(heard)
        if heardNorm.isEmpty { return true }
        let speakingNorm = normalize(speaking)
        if speakingNorm.contains(heardNorm) { return true }
        let heardWords = Set(heardNorm.split(separator: " "))
        guard !heardWords.isEmpty else { return true }
        let speakingWords = Set(speakingNorm.split(separator: " "))
        let overlap = heardWords.intersection(speakingWords).count
        return Double(overlap) / Double(heardWords.count) > 0.6
    }

    func start() {
        guard let recognizer = recognizer, recognizer.supportsOnDeviceRecognition else {
            emit(2, "hands-free mode needs on-device speech recognition, which isn't available here")
            return
        }
        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized:
            beginAudioEngine(recognizer: recognizer)
        case .notDetermined:
            SFSpeechRecognizer.requestAuthorization { [weak self] status in
                DispatchQueue.main.async {
                    if status == .authorized {
                        self?.beginAudioEngine(recognizer: recognizer)
                    } else {
                        self?.emit(2, "speech recognition permission was not granted")
                    }
                }
            }
        case .denied, .restricted:
            emit(2, "speech recognition permission was not granted")
        @unknown default:
            emit(2, "speech recognition permission was not granted")
        }
    }

    private func beginAudioEngine(recognizer: SFSpeechRecognizer) {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            openTap(recognizer: recognizer)
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .audio) { [weak self] granted in
                if granted {
                    self?.openTap(recognizer: recognizer)
                } else {
                    self?.emit(2, "microphone access was not granted")
                }
            }
        case .denied, .restricted:
            emit(2, "microphone access was not granted")
        @unknown default:
            emit(2, "microphone access was not granted")
        }
    }

    /// Installs the tap and starts the audio engine once, up front — unlike
    /// `Transcriber`, whose engine starts and stops per utterance, this
    /// keeps recording continuously across both phases and every command
    /// utterance; only the recognition request/task underneath is swapped
    /// out (`runRecognition`) each time one finishes.
    private func openTap(recognizer: SFSpeechRecognizer) {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self, !self.stopped else { return }
            let inputNode = self.audioEngine.inputNode
            // REVERTED 2026-08-28 — real bug found in the field: Mona
            // reported having to speak unusually loudly for hands-free to
            // hear her at all ("لازم اصرخ لحد ما يرد عليا") after this
            // shipped. `setVoiceProcessingEnabled(true)` turns on Apple's
            // VoIP-style pipeline (echo cancellation, but also automatic
            // gain control and noise suppression tuned for close-talking
            // phone-call audio) for the *entire* hands-free session, not
            // just the brief windows where Amin's own voice is actually
            // playing back — a real, plausible explanation for degraded
            // recognition at normal conversational volume the rest of the
            // time. Barge-in's real-time interruption now relies solely on
            // isLikelySelfEcho's text comparison (see that method) instead
            // of acoustic echo cancellation — a real, if less precise,
            // fallback this code already anticipated, not a new gap. Being
            // heard normally the other 99% of the time matters more than a
            // barge-in feature Mona hadn't even confirmed working yet.
            let format = inputNode.outputFormat(forBus: 0)
            self.voiceResampler = AudioResampler(from: format)
            inputNode.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
                guard let self = self else { return }
                self.currentRequest?.append(buffer)
                if let converted = self.voiceResampler?.resample(buffer) {
                    self.voiceBuffer.append(converted)
                }
            }
            do {
                self.audioEngine.prepare()
                try self.audioEngine.start()
            } catch {
                self.emit(2, "couldn't start the audio engine: \(error.localizedDescription)")
                return
            }
            self.armPassive(recognizer: recognizer)
        }
    }

    /// Phase 1: watching for the wake phrase only. Nothing here is a
    /// command — partial/final transcripts are checked locally and never
    /// forwarded as kind 0/1, so nothing reaches Amin's input box until she
    /// actually opens a session.
    ///
    /// Also where the inactivity timeout is checked (see
    /// `passiveModeStartedAt`'s comment): past `passiveModeTimeoutSeconds`
    /// of continuous passive listening with no wake phrase heard, this
    /// emits kind 8 and returns without re-arming, instead of calling
    /// `stop()` itself. Deliberately: the audio engine/tap stay running for
    /// the brief moment until the frontend's `voice://hands-free-timeout`
    /// handler calls the real `stop_hands_free` Tauri command — the same
    /// single, already-correct teardown path a manual toggle-off uses —
    /// rather than this class tearing itself down and risking a
    /// double-stop (`removeTap`/`audioEngine.stop()` called twice) if that
    /// command then runs anyway against stale `HandsFreeSession` state.
    private func armPassive(recognizer: SFSpeechRecognizer) {
        guard !stopped else { return }
        if let startedAt = passiveModeStartedAt {
            if Date().timeIntervalSince(startedAt) > passiveModeTimeoutSeconds {
                emit(8)
                return
            }
        } else {
            passiveModeStartedAt = Date()
        }
        mode = .passive
        emit(5)
        runRecognition(recognizer: recognizer, onDeviceOnly: true) { [weak self] text, isFinal in
            guard let self = self else { return }
            if self.currentlySpeakingText != nil {
                if self.isLikelySelfEcho(text) {
                    if isFinal { self.armPassive(recognizer: recognizer) }
                    return
                }
                // Amin is talking (a proactive reply, not inside an open
                // session) and this doesn't look like its own echo — treat
                // it as Mona actually trying to get its attention.
                if isFinal, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    self.emit(9, text)
                    self.openActiveSession(recognizer: recognizer)
                }
                return
            }
            if self.heard(self.wakePhrase, in: text) {
                if VoicePrintEngine.shared.verify(samples: self.voiceBuffer.snapshot()) {
                    self.openActiveSession(recognizer: recognizer)
                    return
                }
                // Enrolled voiceprint didn't match whoever just said the
                // wake phrase — treat exactly like not having heard it.
                // Only re-arm at isFinal (not every partial) so a still-
                // growing transcript that keeps containing the phrase
                // doesn't keep tearing down and restarting recognition.
                self.emit(10)
                if isFinal { self.armPassive(recognizer: recognizer) }
                return
            }
            if isFinal {
                self.armPassive(recognizer: recognizer)
            }
        }
    }

    /// Phase 2: a command session is open. Every finalized utterance is
    /// sent as a normal kind-1 final (the frontend auto-sends it to the
    /// agent while a hands-free session is open, the same event a manual
    /// tap-to-toggle final uses otherwise) — this keeps listening for
    /// follow-up utterances afterward instead of ending, until the close
    /// phrase is heard.
    private func openActiveSession(recognizer: SFSpeechRecognizer) {
        guard !stopped else { return }
        // Real engagement — reset the inactivity clock so it measures idle
        // passive time since this moment, not since hands-free first
        // turned on.
        passiveModeStartedAt = nil
        mode = .active
        emit(6)
        listenForCommand(recognizer: recognizer)
    }

    private func listenForCommand(recognizer: SFSpeechRecognizer) {
        guard !stopped else { return }
        // Command utterances go through Apple's server recognizer when
        // on-device isn't available for this locale — same fallback
        // `Transcriber` uses for ordinary push-to-talk, and the same
        // trade-off already disclosed there. Only the passive wake-phrase
        // phase above hard-requires on-device.
        runRecognition(recognizer: recognizer, onDeviceOnly: recognizer.supportsOnDeviceRecognition) { [weak self] text, isFinal in
            guard let self = self else { return }
            if self.currentlySpeakingText != nil {
                if self.isLikelySelfEcho(text) {
                    if isFinal { self.listenForCommand(recognizer: recognizer) }
                    return
                }
                // A real barge-in: Mona started talking over Amin's own
                // reply. Emitting kind 9 (rather than a normal kind-1
                // final) lets the Rust side stop playback immediately,
                // before the frontend even sees the text, instead of
                // waiting for a fresh stop_speaking round trip afterward.
                if isFinal, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    self.emit(9, text)
                    self.listenForCommand(recognizer: recognizer)
                }
                return
            }
            // Gated to isFinal — checking the close phrase against every
            // partial result closed the session the instant a still-growing
            // transcript happened to contain it, sometimes before Mona had
            // even finished her sentence. The wake phrase above stays
            // partial-triggered deliberately (that responsiveness is the
            // point of a wake word); closing an already-open session is the
            // one direction where waiting the extra beat for a final result
            // is worth it.
            if isFinal, self.heard(self.closePhrase, in: text) {
                let remainder = self.textBeforePhrase(self.closePhrase, in: text)
                if !remainder.isEmpty {
                    self.emit(1, remainder)
                }
                self.emit(7)
                self.armPassive(recognizer: recognizer)
                return
            }
            if isFinal {
                if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    self.emit(1, text)
                }
                self.listenForCommand(recognizer: recognizer)
            } else {
                self.emit(0, text)
            }
        }
    }

    /// Starts one recognition request/task against the already-running
    /// audio engine. Identity-checks `currentTask` in the completion
    /// handler so a late callback from a task we've already moved past
    /// (e.g. one `armPassive`/`openActiveSession` superseded) is ignored
    /// instead of re-triggering a phase transition a second time.
    private func runRecognition(
        recognizer: SFSpeechRecognizer,
        onDeviceOnly: Bool,
        onUpdate: @escaping (String, Bool) -> Void
    ) {
        let req = SFSpeechAudioBufferRecognitionRequest()
        req.shouldReportPartialResults = true
        req.requiresOnDeviceRecognition = onDeviceOnly
        currentRequest = req

        var task: SFSpeechRecognitionTask?
        task = recognizer.recognitionTask(with: req) { [weak self] result, error in
            guard let self = self, !self.stopped, self.currentTask === task else { return }
            if let result = result {
                onUpdate(result.bestTranscription.formattedString, result.isFinal)
            }
            if let error = error {
                self.emit(2, error.localizedDescription)
                // A dropped recognition task (a transient Speech framework
                // error) shouldn't silently end hands-free mode — re-arm
                // whichever phase we were in rather than going dark.
                if self.mode == .passive {
                    self.armPassive(recognizer: recognizer)
                } else {
                    self.listenForCommand(recognizer: recognizer)
                }
            }
        }
        currentTask = task
    }

    func stop() {
        stopped = true
        currentRequest?.endAudio()
        currentTask?.cancel()
        audioEngine.inputNode.removeTap(onBus: 0)
        audioEngine.stop()
    }
}
