// Which renderer shows Amin's identity — "one Amin core, two renderers"
// (see docs/ARCHITECTURE.md). This is a display preference with zero
// security or audit relevance, so unlike hands-free mode or the API keys
// it's kept in localStorage rather than the Rust-side settings DB: it never
// needs to be inspectable from the audit log, and every other durable
// setting already goes through a Tauri command specifically because it
// *does* carry a trust decision. Persists across restarts the same way
// (WKWebView/WebView2 both keep localStorage in the app's own data dir).
export type VisualMode = "3d" | "portrait";

const STORAGE_KEY = "amin.visualMode";
const DEFAULT_MODE: VisualMode = "portrait";

export function getStoredVisualMode(): VisualMode {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw === "3d" || raw === "portrait" ? raw : DEFAULT_MODE;
  } catch {
    return DEFAULT_MODE;
  }
}

export function setStoredVisualMode(mode: VisualMode): void {
  try {
    localStorage.setItem(STORAGE_KEY, mode);
  } catch {
    // Private-browsing-style storage rejection — the toggle still works
    // for the rest of this session via React state, it just won't be
    // remembered next launch. Not worth surfacing as an error.
  }
}
