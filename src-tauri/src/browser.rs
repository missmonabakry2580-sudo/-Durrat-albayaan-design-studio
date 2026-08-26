use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

/// A single reused browser window, isolated from Mona's personal browser
/// entirely — its own data directory under the app's own data folder, so
/// no cookies, history, or logged-in sessions are ever shared with (or
/// read from) her real browser profile. See docs/SECURITY.md §4.
///
/// This is intentionally minimal: "Amin can show Mona a page in an
/// isolated window." Amin reading or acting on page content (the rest of
/// "browser control") is real automation with its own security tradeoffs
/// to decide deliberately — see docs/ARCHITECTURE.md's Phase 2 notes —
/// and is not implied by this function existing.
const BROWSER_WINDOW_LABEL: &str = "amin-browser";

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
