// واجهة أمين الأصلية — نفس روح نسخة الويب اللي منى بتستخدمها: وشه في
// النص، آخر رد تحته، شريط إدخال بمايك بيبعت بالسكوت، لوحة مهام، وإعدادات
// بنفس المفاتيح الثلاثة.
import SwiftUI

@MainActor
final class AminViewModel: ObservableObject {
    @Published var lastReply = ""
    @Published var busy = false
    @Published var input = ""
    @Published var errorNote = ""

    // history as raw Anthropic message dictionaries (role/content).
    private var history: [[String: Any]] = []

    func send(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !busy else { return }
        input = ""
        errorNote = ""
        busy = true
        history.append(["role": "user", "content": trimmed])
        Task {
            do {
                let reply = try await runAminTurn(history: history)
                history.append(["role": "assistant", "content": reply.text])
                lastReply = reply.text
                busy = false
                // النطق بعد فك busy — النطق عمره ما يوقف الخط (درس التجمد).
                VoiceHolder.shared.speak(reply.text)
            } catch {
                history.removeLast()
                errorNote = error.localizedDescription
                busy = false
            }
        }
    }
}

/// Shared VoiceIO instance the view model can reach without SwiftUI plumbing.
@MainActor
enum VoiceHolder {
    static let shared = VoiceIO()
}

struct ContentView: View {
    @StateObject private var model = AminViewModel()
    @ObservedObject private var voice = VoiceHolder.shared
    @State private var showSettings = false
    @State private var showTasks = false

    var body: some View {
        VStack(spacing: 14) {
            header
            Spacer()
            face
            if !voice.statusNote.isEmpty {
                Text(voice.statusNote).font(.footnote).foregroundColor(.secondary)
            }
            if !voice.partialText.isEmpty {
                Text(voice.partialText).font(.body).foregroundColor(.secondary)
                    .multilineTextAlignment(.center).padding(.horizontal)
            }
            if !model.lastReply.isEmpty {
                ScrollView {
                    Text(model.lastReply)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal)
                }
                .frame(maxHeight: 180)
            }
            if !model.errorNote.isEmpty {
                Text(model.errorNote).font(.footnote).foregroundColor(.orange)
                    .multilineTextAlignment(.center).padding(.horizontal)
            }
            Spacer()
            inputBar
        }
        .padding(.vertical)
        .background(Color(red: 0.04, green: 0.06, blue: 0.19).ignoresSafeArea())
        .sheet(isPresented: $showSettings) { SettingsView() }
        .sheet(isPresented: $showTasks) { TasksView() }
    }

    private var header: some View {
        HStack {
            Button { showSettings = true } label: {
                Image(systemName: "gearshape.fill").font(.title3)
            }
            Button { showTasks = true } label: {
                Image(systemName: "checklist").font(.title3)
            }
            Spacer()
            VStack(alignment: .trailing) {
                Text("أمين").font(.title2).bold()
                Text(model.busy ? "بيفكر…" : "جاهز").font(.caption).foregroundColor(.secondary)
            }
        }
        .padding(.horizontal)
        .tint(.white)
    }

    private var face: some View {
        Image("amin-face")
            .resizable()
            .scaledToFill()
            .frame(width: 260, height: 260)
            .clipShape(Circle())
            .overlay(Circle().stroke(Color.white.opacity(0.2), lineWidth: 3))
            .shadow(color: voice.isListening ? .green.opacity(0.7) : .blue.opacity(0.45),
                    radius: voice.isListening ? 46 : 28)
    }

    private var inputBar: some View {
        HStack(spacing: 10) {
            Button {
                if voice.isListening {
                    voice.stopListening()
                } else {
                    voice.startListening { model.send($0) }
                }
            } label: {
                Image(systemName: "mic.fill")
                    .font(.title3)
                    .frame(width: 46, height: 46)
                    .background(voice.isListening ? Color.red : Color.white.opacity(0.12))
                    .clipShape(Circle())
            }
            TextField("اكتبي لأمين…", text: $model.input)
                .textFieldStyle(.plain)
                .padding(.horizontal, 14).padding(.vertical, 11)
                .background(Color.white.opacity(0.1))
                .clipShape(Capsule())
                .onSubmit { model.send(model.input) }
            Button { model.send(model.input) } label: {
                Image(systemName: "paperplane.fill")
                    .font(.title3)
                    .frame(width: 46, height: 46)
                    .background(Color.blue)
                    .clipShape(Circle())
            }
        }
        .tint(.white)
        .padding(.horizontal)
    }
}

struct SettingsView: View {
    @Environment(\.dismiss) private var dismiss
    @State private var anthropicKey = AminSettings.anthropicKey
    @State private var elevenKey = AminSettings.elevenKey
    @State private var voiceId = AminSettings.voiceId

    var body: some View {
        NavigationStack {
            Form {
                Section("مفتاح Anthropic (عقل أمين — إجباري)") {
                    SecureField("sk-ant-...", text: $anthropicKey)
                }
                Section("مفتاح ElevenLabs (صوته الحقيقي — اختياري)") {
                    SecureField("sk_...", text: $elevenKey)
                    TextField("Voice ID", text: $voiceId)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                }
                Section {
                    Text("المفاتيح بتتسجل على تليفونك بس. أمين معندوش أي وصول لأي تطبيق تاني على الجهاز — والتطبيقات البنكية ممنوعة عليه نهائيًا بالتصميم.")
                        .font(.footnote).foregroundColor(.secondary)
                }
            }
            .navigationTitle("الإعدادات")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("حفظ") {
                        AminSettings.anthropicKey = anthropicKey.trimmingCharacters(in: .whitespaces)
                        AminSettings.elevenKey = elevenKey.trimmingCharacters(in: .whitespaces)
                        AminSettings.voiceId = voiceId.trimmingCharacters(in: .whitespaces)
                        dismiss()
                    }
                }
            }
        }
    }
}

struct TasksView: View {
    @Environment(\.dismiss) private var dismiss
    @State private var tasks = AminStore.tasks

    var body: some View {
        NavigationStack {
            List {
                if tasks.isEmpty {
                    Text("مفيش مهام لسه — قوليها لأمين بالكلام.")
                        .foregroundColor(.secondary)
                }
                ForEach(tasks.reversed()) { task in
                    HStack {
                        Button {
                            toggle(task)
                        } label: {
                            Image(systemName: task.status == "done" ? "checkmark.circle.fill" : "circle")
                                .foregroundColor(task.status == "done" ? .green : .secondary)
                        }
                        Text(task.title)
                            .strikethrough(task.status == "done")
                            .foregroundColor(task.status == "done" ? .secondary : .primary)
                    }
                }
            }
            .navigationTitle("المهام")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("إغلاق") { dismiss() }
                }
            }
        }
    }

    private func toggle(_ task: AminTask) {
        var all = AminStore.tasks
        if let idx = all.firstIndex(where: { $0.id == task.id }) {
            all[idx].status = all[idx].status == "done" ? "open" : "done"
            AminStore.tasks = all
            tasks = all
        }
    }
}
