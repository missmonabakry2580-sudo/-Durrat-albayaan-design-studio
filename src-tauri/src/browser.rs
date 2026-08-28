use serde_json::Value;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

/// A single reused browser window, isolated from Mona's personal browser
/// entirely — its own data directory under the app's own data folder, so
/// no cookies, history, or logged-in sessions are ever shared with (or
/// read from) her real browser profile. See docs/SECURITY.md §4.
///
/// Beyond just opening a page, Amin can read its content and click/fill
/// elements on it (see `read_page`/`click_element`/`fill_field` below),
/// via `WebviewWindow::eval_with_callback` — no separate automation
/// engine, just JS injected into this same isolated window. Every one of
/// these is `RiskTier::ConfirmHighRisk` in tools.rs: unlike a file whose
/// contents Mona already owns, a live page can be a real account (bank,
/// email-on-web, anything logged into that isolated profile) where a
/// click or a filled field has real-world consequences the instant it
/// runs. And critically: whatever text/labels a page returns is Mona's
/// data only in the sense that her browser rendered it — the words
/// themselves come from whoever controls that page. `agent.rs`'s system
/// prompt tells Claude to treat it as untrusted content to read, never as
/// instructions to follow, the same way this whole harness treats any
/// external content — a page cannot use its own text to make Amin do
/// something Mona didn't ask for.
const BROWSER_WINDOW_LABEL: &str = "amin-browser";

/// Injected once per `read_page` call. Clears any previous tagging (so ids
/// always reflect the page's current DOM, not a stale earlier read), then
/// assigns a fresh sequential `data-amin-id` to every visible, interactive
/// element so `click_element`/`fill_field` can address one by a plain
/// integer instead of Claude having to construct a CSS selector itself —
/// the integer is the only thing that ever gets spliced into a follow-up
/// script, which is what keeps that splice injection-safe.
const READ_PAGE_JS: &str = r#"(function() {
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
})()"#;

/// Parses and validates a URL before it ever reaches a window — pulled out
/// so this validation is unit-testable without a running Tauri app.
fn parse_allowed_url(url: &str) -> Result<tauri::Url, String> {
    let parsed = tauri::Url::parse(url).map_err(|e| format!("'{url}' isn't a valid URL: {e}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!(
            "only http/https URLs are allowed, got scheme: {}",
            parsed.scheme()
        ));
    }
    Ok(parsed)
}

pub fn open_url<R: Runtime>(app: &AppHandle<R>, url: &str) -> Result<(), String> {
    let parsed = parse_allowed_url(url)?;

    if let Some(window) = app.get_webview_window(BROWSER_WINDOW_LABEL) {
        window.navigate(parsed).map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    let profile_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("amin-browser-profile");

    WebviewWindowBuilder::new(app, BROWSER_WINDOW_LABEL, WebviewUrl::External(parsed))
        .title("أمين — Browser")
        .data_directory(profile_dir)
        .inner_size(1000.0, 720.0)
        .build()
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Runs `js` in the browser window and awaits the single result
/// `eval_with_callback` reports back. `eval_with_callback` only accepts an
/// `Fn`, not `FnOnce` (the underlying webview API is written to allow being
/// invoked more than once in general), even though a one-shot `eval` call
/// only ever calls back once in practice — the `Mutex<Option<_>>` is what
/// lets an `Fn` closure still consume the (non-`Clone`) oneshot sender the
/// one time it actually fires.
async fn eval_and_await<R: Runtime>(app: &AppHandle<R>, js: String) -> Result<String, String> {
    let window = app.get_webview_window(BROWSER_WINDOW_LABEL).ok_or_else(|| {
        "لا توجد صفحة مفتوحة في متصفح أمين حاليًا — افتحي رابطًا أولًا".to_string()
    })?;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx = Mutex::new(Some(tx));
    window
        .eval_with_callback(js, move |result| {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(result);
            }
        })
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "لم يصل رد من متصفح أمين".to_string())
}

/// Reads the currently-open page: its URL, title, visible text (truncated),
/// and a list of clickable/fillable elements addressed by a plain integer
/// id — see `READ_PAGE_JS`. Call this before `click_element`/`fill_field`
/// so the ids they take are the ones the current DOM actually has.
pub async fn read_page<R: Runtime>(app: &AppHandle<R>) -> Result<Value, String> {
    let raw = eval_and_await(app, READ_PAGE_JS.to_string()).await?;
    serde_json::from_str(&raw).map_err(|e| format!("تعذّر تفسير محتوى الصفحة: {e}"))
}

/// Clicks the element `read_page` tagged with `data-amin-id="{id}"`.
pub async fn click_element<R: Runtime>(app: &AppHandle<R>, id: u32) -> Result<(), String> {
    let js = format!(
        r#"(function() {{
  var el = document.querySelector('[data-amin-id="{id}"]');
  if (!el) return {{ ok: false, error: 'العنصر لم يعد موجودًا في الصفحة — اقرئي الصفحة تاني' }};
  el.scrollIntoView({{ block: 'center' }});
  el.click();
  return {{ ok: true }};
}})()"#
    );
    let raw = eval_and_await(app, js).await?;
    let result: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if result.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Ok(())
    } else {
        Err(result
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("تعذّر الضغط على العنصر")
            .to_string())
    }
}

/// Fills the element `read_page` tagged with `data-amin-id="{id}"` and
/// fires `input`/`change` so frameworks that listen for those events (React
/// and friends) pick up the value the same as if Mona had typed it — a
/// plain `.value = ...` assignment alone doesn't trigger that. `value` is
/// spliced in via `serde_json::to_string`, which produces a properly
/// escaped JS string literal — that escaping, plus `id` always being a
/// plain integer (never a caller-supplied selector), is what keeps this
/// splice safe from injection.
pub async fn fill_field<R: Runtime>(app: &AppHandle<R>, id: u32, value: &str) -> Result<(), String> {
    let value_js = serde_json::to_string(value).map_err(|e| e.to_string())?;
    let js = format!(
        r#"(function() {{
  var el = document.querySelector('[data-amin-id="{id}"]');
  if (!el) return {{ ok: false, error: 'العنصر لم يعد موجودًا في الصفحة — اقرئي الصفحة تاني' }};
  var proto = el.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
  var setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
  setter.call(el, {value_js});
  el.dispatchEvent(new Event('input', {{ bubbles: true }}));
  el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  return {{ ok: true }};
}})()"#
    );
    let raw = eval_and_await(app, js).await?;
    let result: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if result.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Ok(())
    } else {
        Err(result
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("تعذّر تعبئة الحقل")
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_http_and_https() {
        assert!(parse_allowed_url("https://example.com").is_ok());
        assert!(parse_allowed_url("http://example.com").is_ok());
    }

    #[test]
    fn rejects_a_file_url() {
        // file:// would let a caller point the "browser" at the local
        // filesystem instead of the web — not what this is for.
        assert!(parse_allowed_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn rejects_a_javascript_url() {
        assert!(parse_allowed_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn rejects_garbage_input() {
        assert!(parse_allowed_url("not a url").is_err());
    }
}
