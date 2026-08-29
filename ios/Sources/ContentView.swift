// أمين الأصلي = تطبيق الويب الكامل (كل القدرات المختبَرة: قاعدة البيانات،
// إدارة الحسابات، الكود، المراقبة…) داخل WKWebView، مع صوت أبل الأصلي
// (SFSpeechRecognizer) مدمج بدل صوت الويب اللي بيعاند على iOS. أي تعديل في
// نسخة الويب بيوصل هنا تلقائيًا لأننا بنحمّل نفس الصفحة المنشورة.
import SwiftUI
import WebKit

// الصفحة المنشورة على GitHub Pages. علامة native=1 بتخلي صفحة الويب تخفي
// مايكها المكسور وتفعّل جسر الصوت الأصلي (window.__aminNativeSend).
private let AMIN_WEB_URL = "https://missmonabakry2580-sudo.github.io/-Durrat-albayaan-design-studio/?native=1"

struct WebViewContainer: UIViewRepresentable {
    let webView: WKWebView
    func makeUIView(context: Context) -> WKWebView { webView }
    func updateUIView(_ uiView: WKWebView, context: Context) {}
}

@MainActor
final class AminWeb: ObservableObject {
    let webView: WKWebView

    init() {
        let cfg = WKWebViewConfiguration()
        cfg.allowsInlineMediaPlayback = true
        cfg.mediaTypesRequiringUserActionForPlayback = []
        cfg.websiteDataStore = .default() // localStorage بيفضل محفوظ بين الجلسات
        let wv = WKWebView(frame: .zero, configuration: cfg)
        wv.scrollView.bounces = false
        wv.allowsBackForwardNavigationGestures = false
        webView = wv
        if let url = URL(string: AMIN_WEB_URL) {
            webView.load(URLRequest(url: url))
        }
    }

    // بيحقن النص المسموع في صفحة الويب ويشغّل الإرسال — نفس مسار الكتابة.
    func sendVoice(_ text: String) {
        let escaped = text
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
            .replacingOccurrences(of: "\n", with: " ")
            .replacingOccurrences(of: "\r", with: " ")
        webView.evaluateJavaScript(
            "window.__aminNativeSend && window.__aminNativeSend(\"\(escaped)\")",
            completionHandler: nil
        )
    }

    func reload() {
        if let url = URL(string: AMIN_WEB_URL) {
            webView.load(URLRequest(url: url))
        }
    }
}

struct ContentView: View {
    @StateObject private var web = AminWeb()
    @StateObject private var voice = VoiceIO()

    var body: some View {
        ZStack(alignment: .bottomLeading) {
            WebViewContainer(webView: web.webView)
                .ignoresSafeArea()

            VStack(alignment: .leading, spacing: 8) {
                if !voice.partialText.isEmpty {
                    Text(voice.partialText)
                        .font(.footnote)
                        .foregroundColor(.white)
                        .padding(8)
                        .background(Color.black.opacity(0.6))
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                        .padding(.horizontal, 12)
                }
                Button {
                    if voice.isListening {
                        voice.stopListening()
                    } else {
                        voice.startListening { text in web.sendVoice(text) }
                    }
                } label: {
                    Image(systemName: "mic.fill")
                        .font(.title2)
                        .foregroundColor(.white)
                        .frame(width: 58, height: 58)
                        .background(voice.isListening ? Color.red : Color.blue)
                        .clipShape(Circle())
                        .shadow(color: .black.opacity(0.4), radius: 6)
                }
                .padding(.leading, 16)
                .padding(.bottom, 92) // فوق شريط الإدخال بتاع صفحة الويب
            }
        }
    }
}
