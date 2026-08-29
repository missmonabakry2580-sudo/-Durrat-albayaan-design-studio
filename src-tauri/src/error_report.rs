//! Automatic error reporting to GitHub, as an issue on Amin's own repo —
//! Mona, explicit request (2026-08-28): "أنا عايزاك تبني طريقه في الكود ان
//! لما يكون في خطأ في النظام مبني يصير تواصل مباشر بين امين و بينك يبلغك
//! الخطأ عشان تصلحه" (build a way so that when there's an error in the
//! system, Amin communicates directly with you [Claude] to report the
//! error so you can fix it). Direct, instant Claude-to-app communication
//! isn't something this architecture can offer — there is no standing
//! server here for the app to call into. What IS real and buildable: the
//! app files a GitHub issue on its own repo the moment a real backend
//! error happens, labeled so it's easy to find, and a scheduled check (see
//! docs/ARCHITECTURE.md's "Automatic error reporting" section) picks new
//! ones up without Mona having to notice, screenshot, or describe anything
//! herself. Optional end to end: nothing here ever runs, and no issue is
//! ever filed, unless she's pasted her own GitHub token into Settings.
//!
//! Deliberately conservative about what gets reported: this is for real,
//! actionable backend failures (an API call that failed after every retry,
//! a subsystem refusing to start), never for expected/user-facing
//! conditions (a bad API key format, a declined confirmation, a voice
//! mismatch) — those already have their own clear message shown to her
//! directly and would just be noise here.

use std::sync::Mutex;
use std::time::{Duration, Instant};

const REPO_OWNER: &str = "missmonabakry2580-sudo";
const REPO_NAME: &str = "-Durrat-albayaan-design-studio";
const ISSUES_URL: &str = "https://api.github.com/repos/missmonabakry2580-sudo/-Durrat-albayaan-design-studio/issues";
const REPORT_LABEL: &str = "amin-auto-report";

/// Never file more than one issue for the same `category` within this
/// window — a repeatedly-failing subsystem (e.g. every single TTS call
/// while a network outage lasts) must not flood the repo with duplicate
/// issues. Keyed by category, not by exact message, since the same root
/// cause usually produces near-identical text every time.
const DEDUP_WINDOW: Duration = Duration::from_secs(60 * 60);

static LAST_REPORTED: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

fn recently_reported(category: &str) -> bool {
    let mut guard = match LAST_REPORTED.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    guard.retain(|(_, at)| at.elapsed() < DEDUP_WINDOW);
    guard.iter().any(|(c, _)| c == category)
}

fn mark_reported(category: &str) {
    if let Ok(mut guard) = LAST_REPORTED.lock() {
        guard.push((category.to_string(), Instant::now()));
    }
}

/// Fires a real backend failure at GitHub as a new issue — fully
/// best-effort: no token saved, a network failure, or any GitHub API error
/// here is swallowed silently (logged to stderr only) rather than
/// interrupting whatever real operation was already failing when this was
/// called. `category` is a short, stable identifier (e.g.
/// "elevenlabs_tts", "agent_api", "voice_engine_load") used only for the
/// dedup window and the issue title, not shown to Mona anywhere.
/// Fire-and-forget wrapper (2026-08-28 code review finding): `report` was
/// being awaited inline on speech/agent failure paths, so a network outage
/// — the very condition most likely to have caused the failure being
/// reported — could stall the caller (e.g. delay `speak_text`'s on-device
/// fallback) for as long as the GitHub POST took to fail. Reporting an
/// error must never make the error's handling worse: spawn it and move on.
pub fn report_in_background(app: &tauri::AppHandle, category: &str, detail: &str) {
    let app = app.clone();
    let category = category.to_string();
    let detail = detail.to_string();
    tauri::async_runtime::spawn(async move {
        report(&app, &category, &detail).await;
    });
}

pub async fn report(app: &tauri::AppHandle, category: &str, detail: &str) {
    if recently_reported(category) {
        return;
    }
    let token = {
        use tauri::Manager;
        let Some(db) = app.try_state::<crate::db::Db>() else { return };
        let Ok(conn) = db.0.lock() else { return };
        crate::commands::get_setting(&conn, crate::commands::GITHUB_TOKEN_KEY)
    };
    let Some(token) = token.filter(|v| !v.trim().is_empty()) else { return };

    mark_reported(category);

    let title = format!("[amin-auto-report] {category}");
    let body = format!(
        "أبلغ عنه أمين تلقائيًا — مش من منى.\n\n\
         **الفئة:** `{category}`\n\
         **نسخة أمين:** {version}\n\
         **الوقت:** {time}\n\n\
         **التفاصيل:**\n```\n{detail}\n```",
        version = env!("CARGO_PKG_VERSION"),
        time = chrono::Utc::now().to_rfc3339(),
    );

    let payload = serde_json::json!({
        "title": title,
        "body": body,
        "labels": [REPORT_LABEL],
    });

    // 10s cap for the same reason diacritize_arabic_text has its 8s one:
    // the default reqwest client has NO timeout, and a black-holed
    // connection would otherwise hold this task open indefinitely.
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    else {
        return;
    };
    let result = client
        .post(ISSUES_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "amin-app")
        .json(&payload)
        .send()
        .await;

    match result {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => {
            eprintln!(
                "error_report: GitHub API returned {} for {REPO_OWNER}/{REPO_NAME}",
                resp.status()
            );
        }
        Err(e) => {
            eprintln!("error_report: couldn't reach GitHub: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_category_has_never_been_reported() {
        assert!(!recently_reported("a_category_no_test_has_used_before"));
    }

    #[test]
    fn marking_a_category_makes_it_count_as_recently_reported() {
        mark_reported("dedup_test_category");
        assert!(recently_reported("dedup_test_category"));
    }

    #[test]
    fn different_categories_dedup_independently() {
        mark_reported("dedup_test_category_a");
        assert!(!recently_reported("dedup_test_category_b_never_marked"));
    }
}
