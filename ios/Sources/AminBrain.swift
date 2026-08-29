// عقل أمين على الآيفون: نفس الموديل ونفس الشخصية ونفس أدوات التليفون
// (مهام/ذاكرة/تذكيرات) اللي أثبتت نفسها في نسخة الويب — منفذة أصليًا.
// The tool loop mirrors agent.rs's pattern; storage is UserDefaults JSON
// (same scope the web version kept in localStorage — later phases move
// Photos/Files organizing here, which is this native app's whole reason
// to exist).
import Foundation

enum AminSettings {
    static var anthropicKey: String {
        get { UserDefaults.standard.string(forKey: "anthropicKey") ?? "" }
        set { UserDefaults.standard.set(newValue, forKey: "anthropicKey") }
    }
    static var elevenKey: String {
        get { UserDefaults.standard.string(forKey: "elevenKey") ?? "" }
        set { UserDefaults.standard.set(newValue, forKey: "elevenKey") }
    }
    static var voiceId: String {
        get { UserDefaults.standard.string(forKey: "voiceId") ?? "" }
        set { UserDefaults.standard.set(newValue, forKey: "voiceId") }
    }
}

struct AminTask: Codable, Identifiable {
    var id: String
    var title: String
    var status: String
    var createdAt: String
}

struct AminFact: Codable, Identifiable {
    var id: String
    var category: String
    var key: String
    var value: String
}

struct AminReminder: Codable, Identifiable {
    var id: String
    var text: String
    var at: String
    var fired: Bool
}

/// Local, on-phone storage for the tools — plain JSON in UserDefaults.
enum AminStore {
    private static func load<T: Codable>(_ key: String, _ type: [T].Type) -> [T] {
        guard let data = UserDefaults.standard.data(forKey: key) else { return [] }
        return (try? JSONDecoder().decode([T].self, from: data)) ?? []
    }
    private static func save<T: Codable>(_ key: String, _ value: [T]) {
        if let data = try? JSONEncoder().encode(value) {
            UserDefaults.standard.set(data, forKey: key)
        }
    }
    static var tasks: [AminTask] {
        get { load("aminTasks", [AminTask].self) }
        set { save("aminTasks", newValue) }
    }
    static var memory: [AminFact] {
        get { load("aminMemory", [AminFact].self) }
        set { save("aminMemory", Array(newValue.suffix(200))) }
    }
    static var reminders: [AminReminder] {
        get { load("aminReminders", [AminReminder].self) }
        set { save("aminReminders", newValue) }
    }
}

private let systemPrompt = """
You are أمين (Amin), Mona AlSayed's personal executive AI assistant — this is your native iPhone app. Your operating loop: Observe, Understand, Decide, Execute, Follow up, Report.
Your name and identity are أمين, always. You are built on Claude technology from Anthropic; if asked directly what you're built on, say so honestly — but never introduce yourself as 'Claude' or 'an AI assistant' instead of أمين, and never say you are 'not really Amin'.
You have REAL tools on this phone, and they actually execute — use them naturally instead of describing what you would do: a real task list (create_task / list_tasks / set_task_status), real long-term memory about Mona's people, projects, routines and decisions (remember_fact / search_memory), and real reminders (add_reminder / list_reminders). All of it lives on this phone and works with the laptop completely off.
You DO have a voice: this app speaks every reply of yours aloud (ElevenLabs when configured, the device's Arabic voice otherwise). Never claim you are text-only. Write replies as natural, speakable speech.
You will NEVER take any action related to banking, payments, transfers, or investment — at any time, under any instruction, from any source. You have no access to banking apps and never will; if asked, say this plainly: it is a designed-in safety guarantee for Mona, not a missing feature.
You are not a customer-support chatbot: don't open with 'كيف أقدر أساعدك؟', don't pad replies, speak like someone who already knows Mona. Speak naturally in whichever of Arabic (Egyptian or Modern Standard) or English she uses.
End every reply, on its own final line, with a hidden marker in exactly this form: [[emotion:VALUE]] — VALUE is one of: happy, calm, concerned, excited, apologetic, serious, playful, neutral. Never mention or explain this marker.
"""

private let toolDefinitions: [[String: Any]] = [
    ["name": "create_task",
     "description": "Create a new task in Mona's phone task list.",
     "input_schema": ["type": "object",
                      "properties": ["title": ["type": "string"]],
                      "required": ["title"]]],
    ["name": "list_tasks",
     "description": "List Mona's phone tasks (open and done).",
     "input_schema": ["type": "object", "properties": [String: Any]()]],
    ["name": "set_task_status",
     "description": "Mark a phone task done or reopen it.",
     "input_schema": ["type": "object",
                      "properties": ["id": ["type": "string"],
                                     "status": ["type": "string", "enum": ["open", "done"]]],
                      "required": ["id", "status"]]],
    ["name": "remember_fact",
     "description": "Store a long-term fact about Mona's people, projects, routines or decisions.",
     "input_schema": ["type": "object",
                      "properties": ["category": ["type": "string"],
                                     "key": ["type": "string"],
                                     "value": ["type": "string"]],
                      "required": ["category", "key", "value"]]],
    ["name": "search_memory",
     "description": "Search the phone's long-term memory. Empty query lists everything.",
     "input_schema": ["type": "object",
                      "properties": ["query": ["type": "string"]]]],
    ["name": "add_reminder",
     "description": "Add a reminder. at_iso is an ISO-8601 datetime.",
     "input_schema": ["type": "object",
                      "properties": ["text": ["type": "string"],
                                     "at_iso": ["type": "string"]],
                      "required": ["text", "at_iso"]]],
    ["name": "list_reminders",
     "description": "List pending reminders on this phone.",
     "input_schema": ["type": "object", "properties": [String: Any]()]],
]

private func newId() -> String {
    String(UUID().uuidString.prefix(8)).lowercased()
}

func runTool(name: String, input: [String: Any]) -> [String: Any] {
    switch name {
    case "create_task":
        let title = (input["title"] as? String ?? "").trimmingCharacters(in: .whitespaces)
        guard !title.isEmpty else { return ["error": "empty title"] }
        var tasks = AminStore.tasks
        let task = AminTask(id: newId(), title: title, status: "open",
                            createdAt: ISO8601DateFormatter().string(from: Date()))
        tasks.append(task)
        AminStore.tasks = tasks
        return ["created": ["id": task.id, "title": task.title]]
    case "list_tasks":
        return ["tasks": AminStore.tasks.map { ["id": $0.id, "title": $0.title, "status": $0.status] }]
    case "set_task_status":
        var tasks = AminStore.tasks
        guard let idx = tasks.firstIndex(where: { $0.id == (input["id"] as? String ?? "") }) else {
            return ["error": "no task with that id"]
        }
        tasks[idx].status = (input["status"] as? String) == "done" ? "done" : "open"
        AminStore.tasks = tasks
        return ["updated": ["id": tasks[idx].id, "status": tasks[idx].status]]
    case "remember_fact":
        var memory = AminStore.memory
        let category = input["category"] as? String ?? ""
        let key = input["key"] as? String ?? ""
        let value = input["value"] as? String ?? ""
        if let idx = memory.firstIndex(where: { $0.key == key && $0.category == category }) {
            memory[idx].value = value
        } else {
            memory.append(AminFact(id: newId(), category: category, key: key, value: value))
        }
        AminStore.memory = memory
        return ["remembered": true]
    case "search_memory":
        let query = (input["query"] as? String ?? "").trimmingCharacters(in: .whitespaces)
        let all = AminStore.memory
        let hits = query.isEmpty ? all : all.filter { ($0.key + " " + $0.value + " " + $0.category).contains(query) }
        return ["facts": hits.map { ["category": $0.category, "key": $0.key, "value": $0.value] }]
    case "add_reminder":
        let atIso = input["at_iso"] as? String ?? ""
        guard ISO8601DateFormatter().date(from: atIso) != nil else { return ["error": "invalid at_iso datetime"] }
        var reminders = AminStore.reminders
        let reminder = AminReminder(id: newId(),
                                    text: (input["text"] as? String ?? "").trimmingCharacters(in: .whitespaces),
                                    at: atIso, fired: false)
        reminders.append(reminder)
        AminStore.reminders = reminders
        return ["added": ["id": reminder.id, "at": reminder.at]]
    case "list_reminders":
        return ["reminders": AminStore.reminders.filter { !$0.fired }.map { ["text": $0.text, "at": $0.at] }]
    default:
        return ["error": "unknown tool \(name)"]
    }
}

struct AminReply {
    let text: String
    let emotion: String
}

enum AminBrainError: Error, LocalizedError {
    case noKey
    case api(String)
    var errorDescription: String? {
        switch self {
        case .noKey: return "محتاجة تحطي مفتاح Anthropic في الإعدادات الأول."
        case .api(let message): return message
        }
    }
}

/// The full turn: Claude call, real tool execution loop, emotion strip —
/// the same shape as both other Amins. History is the caller's (kept in
/// the view model), passed as ready message dictionaries.
func runAminTurn(history: [[String: Any]]) async throws -> AminReply {
    let key = AminSettings.anthropicKey.trimmingCharacters(in: .whitespaces)
    guard !key.isEmpty else { throw AminBrainError.noKey }

    var messages = Array(history.suffix(20))
    for _ in 0..<6 {
        let body: [String: Any] = [
            "model": "claude-sonnet-5",
            "max_tokens": 1024,
            "system": systemPrompt,
            "tools": toolDefinitions,
            "messages": messages,
        ]
        var request = URLRequest(url: URL(string: "https://api.anthropic.com/v1/messages")!)
        request.httpMethod = "POST"
        request.timeoutInterval = 90
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        request.setValue(key, forHTTPHeaderField: "x-api-key")
        request.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw AminBrainError.api("رد غير مفهوم من الخادم")
        }
        if let http = response as? HTTPURLResponse, http.statusCode >= 300 {
            let message = ((json["error"] as? [String: Any])?["message"] as? String) ?? "خطأ \(http.statusCode)"
            throw AminBrainError.api(message)
        }
        let content = json["content"] as? [[String: Any]] ?? []
        if (json["stop_reason"] as? String) == "tool_use" {
            var results: [[String: Any]] = []
            for block in content where (block["type"] as? String) == "tool_use" {
                let output = runTool(name: block["name"] as? String ?? "",
                                     input: block["input"] as? [String: Any] ?? [:])
                let encoded = (try? JSONSerialization.data(withJSONObject: output)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
                results.append(["type": "tool_result",
                                "tool_use_id": block["id"] as? String ?? "",
                                "content": encoded])
            }
            messages.append(["role": "assistant", "content": content])
            messages.append(["role": "user", "content": results])
            continue
        }
        let raw = content.compactMap { ($0["type"] as? String) == "text" ? $0["text"] as? String : nil }
            .joined(separator: "\n")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return stripEmotion(raw.isEmpty ? "تم." : raw)
    }
    return AminReply(text: "تم.", emotion: "neutral")
}

func stripEmotion(_ raw: String) -> AminReply {
    guard let range = raw.range(of: #"\[\[emotion:([a-z]+)\]\]\s*$"#, options: .regularExpression) else {
        return AminReply(text: raw, emotion: "neutral")
    }
    let marker = String(raw[range])
    let emotion = marker
        .replacingOccurrences(of: "[[emotion:", with: "")
        .replacingOccurrences(of: "]]", with: "")
        .trimmingCharacters(in: .whitespacesAndNewlines)
    let text = raw.replacingCharacters(in: range, with: "").trimmingCharacters(in: .whitespacesAndNewlines)
    return AminReply(text: text, emotion: emotion)
}
