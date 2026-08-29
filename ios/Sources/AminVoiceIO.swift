// صوت أمين على الآيفون — دخل (SFSpeechRecognizer) وخرج (ElevenLabs ثم
// صوت النظام كبديل). Every lesson from two days of live debugging is
// baked in: silence-based finalization (the Mac's isFinal bug AND the
// web's iOS onend bug were the same disease on two platforms — never
// trust the platform's end-of-speech callback alone), speaking is
// fire-and-forget so it can never wedge the pipeline, network calls have
// hard timeouts, and refusals surface as text.
import Foundation
import AVFoundation
import Speech

@MainActor
final class VoiceIO: NSObject, ObservableObject {
    @Published var isListening = false
    @Published var partialText = ""
    @Published var statusNote = ""

    private let recognizer = SFSpeechRecognizer(locale: Locale(identifier: "ar-EG"))
    private let audioEngine = AVAudioEngine()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var silenceTimer: Timer?
    private var finished = true
    private var onFinal: ((String) -> Void)?

    private var player: AVAudioPlayer?
    private let synthesizer = AVSpeechSynthesizer()

    // MARK: - الاستماع

    func startListening(onFinal: @escaping (String) -> Void) {
        guard !isListening else { stopListening(); return }
        self.onFinal = onFinal
        SFSpeechRecognizer.requestAuthorization { [weak self] status in
            DispatchQueue.main.async {
                guard status == .authorized else {
                    self?.statusNote = "محتاج إذن التعرف على الكلام من الإعدادات"
                    return
                }
                AVAudioSession.sharedInstance().requestRecordPermission { granted in
                    DispatchQueue.main.async {
                        guard granted else {
                            self?.statusNote = "محتاج إذن المايك من الإعدادات"
                            return
                        }
                        self?.beginRecognition()
                    }
                }
            }
        }
    }

    private func beginRecognition() {
        guard let recognizer, recognizer.isAvailable else {
            statusNote = "التعرف على الكلام العربي غير متاح على الجهاز ده"
            return
        }
        stopPlayback() // مايسمعش نفسه
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playAndRecord, mode: .default,
                                    options: [.defaultToSpeaker, .allowBluetooth])
            try session.setActive(true)
        } catch {
            statusNote = "معرفتش أفتح المايك: \(error.localizedDescription)"
            return
        }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.requiresOnDeviceRecognition = false
        self.request = request

        let node = audioEngine.inputNode
        let format = node.outputFormat(forBus: 0)
        node.removeTap(onBus: 0)
        node.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak request] buffer, _ in
            request?.append(buffer)
        }
        do {
            audioEngine.prepare()
            try audioEngine.start()
        } catch {
            statusNote = "معرفتش أشغل المايك: \(error.localizedDescription)"
            return
        }
        finished = false
        isListening = true
        partialText = ""
        statusNote = "أمين بيسمعك… اسكتي لما تخلصي وهو هيبعت لوحده"

        task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            DispatchQueue.main.async {
                guard let self, !self.finished else { return }
                if let result {
                    self.partialText = result.bestTranscription.formattedString
                    if result.isFinal {
                        self.finishListening(send: true)
                    } else {
                        self.scheduleSilenceFinal()
                    }
                }
                if error != nil {
                    // خطأ حقيقي: نقفل ونبعت اللي اتسمع لحد دلوقتي بدل ما نرميه.
                    self.finishListening(send: true)
                }
            }
        }
        // سقف أمان — تسجيل من غير نهاية بيقفل نفسه بعد دقيقة.
        silenceTimer?.invalidate()
        silenceTimer = Timer.scheduledTimer(withTimeInterval: 60, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.finishListening(send: true) }
        }
    }

    private func scheduleSilenceFinal() {
        silenceTimer?.invalidate()
        guard !partialText.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        silenceTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.finishListening(send: true) }
        }
    }

    func stopListening() {
        finishListening(send: true)
    }

    private func finishListening(send: Bool) {
        guard !finished else { return }
        finished = true
        silenceTimer?.invalidate()
        audioEngine.inputNode.removeTap(onBus: 0)
        audioEngine.stop()
        request?.endAudio()
        task?.cancel()
        task = nil
        request = nil
        isListening = false
        statusNote = ""
        // حرّر جلسة الصوت عشان نطق أمين (اللي بيشتغل داخل الويب) يقدر يستخدم
        // السماعة بعد ما المايك يخلص — من غير كده الجلسة تفضل على وضع
        // التسجيل والصوت الطالع يتكتم.
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        let text = partialText.trimmingCharacters(in: .whitespacesAndNewlines)
        partialText = ""
        if send, !text.isEmpty {
            onFinal?(text)
        }
    }

    // MARK: - النطق

    func speak(_ text: String) {
        stopPlayback()
        let key = AminSettings.elevenKey.trimmingCharacters(in: .whitespaces)
        let voiceId = AminSettings.voiceId.trimmingCharacters(in: .whitespaces)
        guard !key.isEmpty, !voiceId.isEmpty else {
            speakOnDevice(text)
            return
        }
        Task { [weak self] in
            do {
                var request = URLRequest(url: URL(string:
                    "https://api.elevenlabs.io/v1/text-to-speech/\(voiceId)?output_format=mp3_44100_128")!)
                request.httpMethod = "POST"
                request.timeoutInterval = 20
                request.setValue("application/json", forHTTPHeaderField: "content-type")
                request.setValue(key, forHTTPHeaderField: "xi-api-key")
                request.httpBody = try JSONSerialization.data(withJSONObject: [
                    "text": text,
                    "model_id": "eleven_multilingual_v2",
                ])
                let (data, response) = try await URLSession.shared.data(for: request)
                if let http = response as? HTTPURLResponse, http.statusCode >= 300 {
                    await MainActor.run {
                        self?.statusNote = "صوت ElevenLabs اترفض (\(http.statusCode)) — راجعي الـVoice ID والمفتاح"
                        self?.speakOnDevice(text)
                    }
                    return
                }
                await MainActor.run { self?.playAudio(data) }
            } catch {
                await MainActor.run {
                    self?.statusNote = "تعذر الوصول لـ ElevenLabs — الصوت البديل شغال"
                    self?.speakOnDevice(text)
                }
            }
        }
    }

    private func playAudio(_ data: Data) {
        do {
            try AVAudioSession.sharedInstance().setCategory(.playback, mode: .default)
            try AVAudioSession.sharedInstance().setActive(true)
            player = try AVAudioPlayer(data: data)
            player?.play()
        } catch {
            statusNote = "معرفتش أشغل الصوت: \(error.localizedDescription)"
        }
    }

    private func speakOnDevice(_ text: String) {
        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = AVSpeechSynthesisVoice(language: "ar-SA")
        synthesizer.speak(utterance)
    }

    func stopPlayback() {
        player?.stop()
        player = nil
        synthesizer.stopSpeaking(at: .immediate)
    }
}
