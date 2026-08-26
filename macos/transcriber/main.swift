// Amin's push-to-talk transcriber helper (macOS).
//
// STATUS: written from documented Speech/AVFoundation APIs, now wired into
// the CI build (.github/workflows/build-macos.yml compiles this as a
// universal binary and bundles it as a Tauri resource) and into
// tauri.conf.json + src-tauri/Info.plist (NSMicrophoneUsageDescription /
// NSSpeechRecognitionUsageDescription). Still NEVER RUN AGAINST A REAL
// MICROPHONE — there is no macOS, no Xcode, and no microphone in the
// sandbox this was written and wired up in. The macOS permission prompt,
// the subprocess-TCC risk noted below, and actual transcription accuracy
// are all unverified until tried on a real Mac. See docs/ARCHITECTURE.md's
// "Voice pipeline" section.
//
// Protocol: reads nothing until a "stop\n" line arrives on stdin, at which
// point it finishes the current utterance and exits. Meanwhile it writes
// one JSON object per line to stdout:
//   {"type":"partial","text":"..."}   — an in-progress transcription
//   {"type":"final","text":"..."}     — the finished transcription
//   {"type":"error","message":"..."}  — authorization denied, no mic, etc.
//
// Build (on macOS, once Xcode command line tools are installed):
//   swiftc -O main.swift -o amin-transcriber
// Then place the resulting binary where src-tauri/src/voice.rs expects it
// (bundled as a Tauri resource — see tauri.conf.json once that's wired up).
//
// KNOWN OPEN RISK (check this first): a standalone CLI binary spawned as a
// child process may not cleanly inherit microphone/speech-recognition TCC
// permission prompts the way code running inside the signed .app bundle
// does. If `SFSpeechRecognizer.requestAuthorization` or the audio engine
// silently fails/denies here, the likely fix is moving this logic in-process
// (a Rust<->Swift FFI bridge) rather than a separate executable — don't sink
// hours into subprocess permission debugging before trying that alternative.
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

func emit(_ dict: [String: String]) {
    guard let data = try? JSONSerialization.data(withJSONObject: dict),
          let line = String(data: data, encoding: .utf8) else { return }
    print(line)
    fflush(stdout)
}

func emitPartial(_ text: String) { emit(["type": "partial", "text": text]) }
func emitFinal(_ text: String) { emit(["type": "final", "text": text]) }
func emitError(_ message: String) { emit(["type": "error", "message": message]) }

final class Transcriber {
    // ar-EG: Egyptian Arabic. See the code-switching limitation note above —
    // this single-locale choice is provisional, not a final decision.
    private let recognizer = SFSpeechRecognizer(locale: Locale(identifier: "ar-EG"))
    private let audioEngine = AVAudioEngine()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private let doneGroup = DispatchGroup()

    func run() {
        let authGroup = DispatchGroup()
        authGroup.enter()
        var authorized = false
        SFSpeechRecognizer.requestAuthorization { status in
            authorized = (status == .authorized)
            authGroup.leave()
        }
        authGroup.wait()

        guard authorized else {
            emitError("speech recognition permission was not granted")
            return
        }
        guard let recognizer = recognizer, recognizer.isAvailable else {
            emitError("speech recognizer unavailable for the configured locale")
            return
        }

        let req = SFSpeechAudioBufferRecognitionRequest()
        req.shouldReportPartialResults = true
        // Prefer on-device recognition when the OS supports it for this
        // locale, per the "voice stays local by default" principle in
        // docs/SECURITY.md. Falls back to Apple's server-based recognition
        // automatically if on-device isn't available for this locale/OS
        // version — that fallback is itself worth confirming, not assuming.
        req.requiresOnDeviceRecognition = recognizer.supportsOnDeviceRecognition
        self.request = req

        let inputNode = audioEngine.inputNode
        let format = inputNode.outputFormat(forBus: 0)
        inputNode.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
            req.append(buffer)
        }

        doneGroup.enter()
        task = recognizer.recognitionTask(with: req) { [weak self] result, error in
            guard let self = self else { return }
            if let result = result {
                let text = result.bestTranscription.formattedString
                if result.isFinal {
                    emitFinal(text)
                    self.finish()
                } else {
                    emitPartial(text)
                }
            }
            if let error = error {
                emitError(error.localizedDescription)
                self.finish()
            }
        }

        do {
            audioEngine.prepare()
            try audioEngine.start()
        } catch {
            emitError("couldn't start the audio engine: \(error.localizedDescription)")
            finish()
            return
        }

        // Block the main thread until a "stop" line arrives on stdin or the
        // recognition task finishes on its own (silence timeout, error).
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            while let line = readLine(strippingNewline: true) {
                if line == "stop" {
                    self?.request?.endAudio()
                    // Safety net: if the recognizer never delivers a final
                    // result after endAudio() (no speech detected, a stuck
                    // recognition task), don't hang forever — the Rust side
                    // is blocked in child.wait() for this exact process to
                    // exit. Whatever partial transcript already reached the
                    // frontend stays in its input box either way.
                    DispatchQueue.global().asyncAfter(deadline: .now() + 8) {
                        self?.finish()
                    }
                    break
                }
            }
        }

        doneGroup.wait()
    }

    private var finished = false
    private func finish() {
        if finished { return }
        finished = true
        audioEngine.inputNode.removeTap(onBus: 0)
        audioEngine.stop()
        task?.cancel()
        doneGroup.leave()
    }
}

Transcriber().run()
