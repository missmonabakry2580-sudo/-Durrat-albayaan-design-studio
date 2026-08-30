// متصفح أمين داخل التطبيق — نفس قدرة أمين على اللابتوب بالظبط
// (src-tauri/src/browser.rs)، لكن أصلية على الآيفون: نافذة متصفح منفصلة
// منى بتشوفها وتقدر تسجّل دخولها بنفسها، وأمين بيفتح/يقرأ/يضغط/يكتب فيها
// عن طريق حقن JS في نفس الصفحة — من غير أي محرك أتمتة خارجي.
//
// ده اللي بيخلّي أمين يشتغل فعليًا جوّه البوابة التعليمية ومنصة المنظرة
// الوزارية: يفتحها، يقرأ عناصرها، يملا الحقول، يضغط الأزرار. أي كلام
// بترجعه الصفحة = محتوى غير موثوق يتقري، مش أوامر تتنفّذ.
//
// كل ضغط/كتابة بيتأكد من منى في طبقة الويب الأول (زي متصفح اللابتوب،
// RiskTier::ConfirmHighRisk). الحظر البنكي بيفضل ساري مهما كان.
import Foundation
import WebKit
import UIKit

final class AminBrowser: NSObject, WKScriptMessageHandlerWithReply, WKNavigationDelegate {
    private var webView: WKWebView?
    private weak var browserVC: AminBrowserViewController?
    private var pendingOpenReply: ((Any?, String?) -> Void)?
    private var openTimeoutTimer: Timer?

    // MARK: - نقطة الدخول من الويب

    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping (Any?, String?) -> Void
    ) {
        guard let body = message.body as? [String: Any],
              let action = body["action"] as? String else {
            replyHandler(nil, "طلب غير صالح")
            return
        }
        let args = body["args"] as? [String: Any] ?? [:]
        switch action {
        case "browse_open":    open(args, replyHandler)
        case "browse_read":    read(replyHandler)
        case "browse_click":   click(args, replyHandler)
        case "browse_fill":    fill(args, replyHandler)
        case "browse_current": current(replyHandler)
        case "browse_close":   closeBrowser(replyHandler)
        default:
            replyHandler(nil, "أمر متصفح غير معروف: \(action)")
        }
    }

    // MARK: - إدارة النافذة

    private func ensureWebView() -> WKWebView {
        if let wv = webView { return wv }
        let cfg = WKWebViewConfiguration()
        cfg.websiteDataStore = .default() // تسجيلات الدخول بتفضل محفوظة بين الجلسات
        cfg.allowsInlineMediaPlayback = true
        let wv = WKWebView(frame: .zero, configuration: cfg)
        wv.navigationDelegate = self
        wv.allowsBackForwardNavigationGestures = true
        webView = wv
        return wv
    }

    private func topViewController() -> UIViewController? {
        let scenes = UIApplication.shared.connectedScenes
        let windowScene = scenes.first { $0.activationState == .foregroundActive } as? UIWindowScene
        var top = windowScene?.windows.first { $0.isKeyWindow }?.rootViewController
            ?? windowScene?.windows.first?.rootViewController
        while let presented = top?.presentedViewController { top = presented }
        return top
    }

    private func presentIfNeeded() {
        if browserVC != nil { return }
        guard let top = topViewController() else { return }
        let wv = ensureWebView()
        let vc = AminBrowserViewController(webView: wv, onClose: { [weak self] in
            self?.browserVC = nil
        })
        vc.modalPresentationStyle = .fullScreen
        browserVC = vc
        top.present(vc, animated: true)
    }

    private func validURL(_ raw: String) -> URL? {
        var s = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if s.isEmpty { return nil }
        if !s.lowercased().hasPrefix("http://") && !s.lowercased().hasPrefix("https://") {
            s = "https://" + s
        }
        guard let u = URL(string: s),
              let scheme = u.scheme?.lowercased(),
              scheme == "http" || scheme == "https" else { return nil }
        return u
    }

    // MARK: - فتح صفحة (بيرجّع لما تحمّل أو بعد مهلة)

    private func open(_ args: [String: Any], _ reply: @escaping (Any?, String?) -> Void) {
        guard let url = validURL(args["url"] as? String ?? "") else {
            reply(nil, "محتاج رابط صحيح http/https."); return
        }
        let wv = ensureWebView()
        presentIfNeeded()
        // لو كان في فتح معلّق قديم، نقفله بهدوء ونبدأ الجديد.
        finishPendingOpen(nil, "طلب فتح جديد ألغى القديم")
        pendingOpenReply = reply
        openTimeoutTimer?.invalidate()
        openTimeoutTimer = Timer.scheduledTimer(withTimeInterval: 12, repeats: false) { [weak self] _ in
            let u = self?.webView?.url?.absoluteString ?? url.absoluteString
            self?.finishPendingOpen([
                "ok": true, "url": u,
                "note": "الصفحة لسه بتحمّل — استخدمي browse_read لما تجهز."
            ], nil)
        }
        wv.load(URLRequest(url: url))
    }

    private func finishPendingOpen(_ ok: Any?, _ err: String?) {
        openTimeoutTimer?.invalidate(); openTimeoutTimer = nil
        guard let r = pendingOpenReply else { return }
        pendingOpenReply = nil
        r(ok, err)
    }

    // MARK: - WKNavigationDelegate

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        browserVC?.setURL(webView.url?.absoluteString ?? "")
        finishPendingOpen([
            "ok": true,
            "url": webView.url?.absoluteString ?? "",
            "title": webView.title ?? ""
        ], nil)
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        browserVC?.setURL(webView.url?.absoluteString ?? "")
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        finishPendingOpen(nil, "فشل فتح الصفحة: \(error.localizedDescription)")
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        finishPendingOpen(nil, "فشل فتح الصفحة: \(error.localizedDescription)")
    }

    // MARK: - قراءة/ضغط/كتابة (حقن JS في نفس الصفحة)

    private func eval(_ js: String, _ reply: @escaping (Any?, String?) -> Void) {
        guard let wv = webView, browserVC != nil else {
            reply(nil, "مفيش صفحة مفتوحة — افتحي رابط الأول بـ browse_open."); return
        }
        wv.evaluateJavaScript(js) { result, error in
            if let error = error {
                reply(nil, "تعذّر تنفيذ العملية في الصفحة: \(error.localizedDescription)")
                return
            }
            reply(result ?? NSNull(), nil)
        }
    }

    private func read(_ reply: @escaping (Any?, String?) -> Void) {
        eval(Self.readPageJS, reply)
    }

    private func click(_ args: [String: Any], _ reply: @escaping (Any?, String?) -> Void) {
        guard let id = intId(args["id"]) else {
            reply(nil, "محتاج رقم العنصر (id) من آخر browse_read."); return
        }
        eval(Self.clickJS(id), reply)
    }

    private func fill(_ args: [String: Any], _ reply: @escaping (Any?, String?) -> Void) {
        guard let id = intId(args["id"]) else {
            reply(nil, "محتاج رقم العنصر (id) من آخر browse_read."); return
        }
        let value = args["value"] as? String ?? ""
        eval(Self.fillJS(id, value), reply)
    }

    private func current(_ reply: @escaping (Any?, String?) -> Void) {
        guard let wv = webView else { reply(["open": false], nil); return }
        reply([
            "open": browserVC != nil,
            "url": wv.url?.absoluteString ?? "",
            "title": wv.title ?? ""
        ], nil)
    }

    private func closeBrowser(_ reply: @escaping (Any?, String?) -> Void) {
        browserVC?.dismiss(animated: true)
        browserVC = nil
        reply(["closed": true], nil)
    }

    private func intId(_ v: Any?) -> Int? {
        if let i = v as? Int { return i }
        if let d = v as? Double { return Int(d) }
        if let s = v as? String { return Int(s) }
        return nil
    }

    // MARK: - سكربتات JS (منسوخة من نموذج اللابتوب browser.rs)

    // بيمسح أي وسم قديم (عشان الأرقام تعكس DOM الحالي)، وبيدي رقم متسلسل
    // data-amin-id لكل عنصر تفاعلي ظاهر، وبيرجّع رابط الصفحة وعنوانها ونصها
    // (مقصوص) وقائمة العناصر. الرقم الصحيح بس هو اللي بيتحطّ في سكربت
    // الضغط/الكتابة بعدين — وده اللي بيخلّي الحقن آمن.
    static let readPageJS = """
    (function() {
      document.querySelectorAll('[data-amin-id]').forEach(function(el) {
        el.removeAttribute('data-amin-id');
      });
      var nodes = document.querySelectorAll(
        'a[href], button, input, textarea, select, [role="button"], [onclick]'
      );
      var elements = [];
      var id = 0;
      for (var i = 0; i < nodes.length && elements.length < 150; i++) {
        var el = nodes[i];
        var style = window.getComputedStyle(el);
        if (style.display === 'none' || style.visibility === 'hidden') continue;
        var rect = el.getBoundingClientRect();
        if (rect.width === 0 && rect.height === 0) continue;
        el.setAttribute('data-amin-id', String(id));
        var label = (el.innerText || el.value || el.getAttribute('placeholder') ||
          el.getAttribute('aria-label') || el.getAttribute('alt') || '').trim().slice(0, 120);
        elements.push({
          id: id,
          tag: el.tagName.toLowerCase(),
          type: el.getAttribute('type') || null,
          label: label,
          href: el.tagName === 'A' ? el.href : null
        });
        id++;
      }
      var text = document.body ? document.body.innerText.trim().slice(0, 6000) : '';
      return { url: location.href, title: document.title, text: text, elements: elements };
    })()
    """

    static func clickJS(_ id: Int) -> String {
        return """
        (function() {
          var el = document.querySelector('[data-amin-id="\(id)"]');
          if (!el) return { ok: false, error: 'العنصر لم يعد موجودًا — اقرئي الصفحة تاني بـ browse_read' };
          el.scrollIntoView({ block: 'center' });
          el.click();
          return { ok: true };
        })()
        """
    }

    static func fillJS(_ id: Int, _ value: String) -> String {
        return """
        (function() {
          var el = document.querySelector('[data-amin-id="\(id)"]');
          if (!el) return { ok: false, error: 'العنصر لم يعد موجودًا — اقرئي الصفحة تاني بـ browse_read' };
          var proto = el.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
          var setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
          setter.call(el, \(jsStringLiteral(value)));
          el.dispatchEvent(new Event('input', { bubbles: true }));
          el.dispatchEvent(new Event('change', { bubbles: true }));
          return { ok: true };
        })()
        """
    }

    // بيحوّل نص لـ JS string literal آمن (بهروب كامل) عبر JSONSerialization —
    // القيمة الوحيدة اللي بتتحقن من المستخدم، فلازم تكون مهرّبة صح.
    static func jsStringLiteral(_ s: String) -> String {
        if let data = try? JSONSerialization.data(withJSONObject: [s], options: []),
           let arr = String(data: data, encoding: .utf8), arr.count >= 2 {
            return String(arr.dropFirst().dropLast()) // يشيل [ و ]
        }
        return "\"\""
    }
}

// نافذة المتصفح اللي منى بتشوفها: شريط علوي (إغلاق/رجوع/تحديث + الرابط)
// وتحته صفحة الويب. أمين بيشتغل على نفس webView عن طريق حقن JS.
final class AminBrowserViewController: UIViewController {
    private let webView: WKWebView
    private let onClose: () -> Void
    private let urlLabel = UILabel()

    init(webView: WKWebView, onClose: @escaping () -> Void) {
        self.webView = webView
        self.onClose = onClose
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not used") }

    func setURL(_ s: String) { urlLabel.text = s }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground

        let closeBtn = UIButton(type: .system)
        closeBtn.setTitle("إغلاق", for: .normal)
        closeBtn.addTarget(self, action: #selector(closeTapped), for: .touchUpInside)

        let backBtn = UIButton(type: .system)
        backBtn.setTitle("‹", for: .normal)
        backBtn.titleLabel?.font = .systemFont(ofSize: 26, weight: .semibold)
        backBtn.addTarget(self, action: #selector(backTapped), for: .touchUpInside)

        let reloadBtn = UIButton(type: .system)
        reloadBtn.setTitle("↻", for: .normal)
        reloadBtn.titleLabel?.font = .systemFont(ofSize: 20)
        reloadBtn.addTarget(self, action: #selector(reloadTapped), for: .touchUpInside)

        urlLabel.font = .systemFont(ofSize: 12)
        urlLabel.textColor = .secondaryLabel
        urlLabel.lineBreakMode = .byTruncatingMiddle
        urlLabel.setContentHuggingPriority(.defaultLow, for: .horizontal)

        let bar = UIStackView(arrangedSubviews: [closeBtn, backBtn, reloadBtn, urlLabel])
        bar.axis = .horizontal
        bar.spacing = 12
        bar.alignment = .center
        bar.isLayoutMarginsRelativeArrangement = true
        bar.layoutMargins = UIEdgeInsets(top: 8, left: 14, bottom: 8, right: 14)
        bar.translatesAutoresizingMaskIntoConstraints = false

        let separator = UIView()
        separator.backgroundColor = .separator
        separator.translatesAutoresizingMaskIntoConstraints = false

        webView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(bar)
        view.addSubview(separator)
        view.addSubview(webView)

        NSLayoutConstraint.activate([
            bar.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            bar.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            bar.trailingAnchor.constraint(equalTo: view.trailingAnchor),

            separator.topAnchor.constraint(equalTo: bar.bottomAnchor),
            separator.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            separator.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            separator.heightAnchor.constraint(equalToConstant: 0.5),

            webView.topAnchor.constraint(equalTo: separator.bottomAnchor),
            webView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            webView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
    }

    @objc private func closeTapped() {
        onClose()
        dismiss(animated: true)
    }

    @objc private func backTapped() {
        if webView.canGoBack { webView.goBack() }
    }

    @objc private func reloadTapped() {
        webView.reload()
    }
}
