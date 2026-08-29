//! The phone↔laptop bridge (2026-08-28, Mona's explicit demand: "عايزاك
//! تبني سيرفر يربط الاتنين ببعض... أمين اللي ع تليفوني يقدر ينفذ نفس
//! المهام اللي ع اللابتوب بالظبط").
//!
//! There is no server of ours to host, and standing one up (accounts,
//! tunnels, installs) is exactly the kind of yak-shave Mona has no
//! patience left for — so the "server" is GitHub itself, which both sides
//! can already reach with the fine-grained token she has already created
//! (Issues: Read/Write on this one repo — the exact scope this needs, no
//! wider). One pinned issue on the repo is the message channel: the phone
//! app posts her utterance as an issue comment, this module (a background
//! poller inside the Mac app) picks it up, runs it through the SAME agent
//! pipeline the laptop uses — same tools, same task list, same files,
//! same memory, same confirmation policy — and posts the reply back as
//! another comment for the phone to display and speak.
//!
//! The repo is public, so every comment body is end-to-end encrypted
//! (AES-256-GCM, key derived from a passphrase Mona types into BOTH
//! apps via PBKDF2-SHA256) — what appears publicly is ciphertext. The
//! passphrase is also the authentication: GCM's auth tag means a comment
//! that wasn't produced with her passphrase simply fails to decrypt and
//! is ignored, so a stranger commenting on the public issue cannot inject
//! commands into her Mac. Payloads carry a `kind` field ("cmd" from the
//! phone, "reply" from here) so the poller never re-processes its own
//! replies (both directions are authored by the same token account).
//!
//! Honest physical limit, disclosed in both UIs: this runs inside the Mac
//! app, so the laptop must be on and awake for the phone to get real
//! answers. That is where her files and database physically are.

use base64::Engine;
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::Manager;

const REPO_OWNER: &str = "missmonabakry2580-sudo";
const REPO_NAME: &str = "-Durrat-albayaan-design-studio";
const BRIDGE_LABEL: &str = "amin-relay";
const POLL_SECONDS: u64 = 4;
/// Fixed KDF salt — not a secret (the passphrase is); it only prevents
/// generic rainbow tables. Must match the phone app's constant exactly.
const KDF_SALT: &[u8] = b"amin-phone-bridge-v1";
const KDF_ITERATIONS: u32 = 100_000;

/// Bumped on every start/stop; a poll loop exits when the generation it
/// captured at spawn no longer matches (simplest race-free stop, no
/// JoinHandle bookkeeping).
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn derive_key(passphrase: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(passphrase.as_bytes(), KDF_SALT, KDF_ITERATIONS, &mut key);
    key
}

pub(crate) fn encrypt(key: &[u8; 32], plaintext: &str) -> Result<String, String> {
    use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
    let cipher = aes_gcm::Aes256Gcm::new(key.into());
    let nonce = aes_gcm::Aes256Gcm::generate_nonce(&mut OsRng);
    let ct = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| "encryption failed".to_string())?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

pub(crate) fn decrypt(key: &[u8; 32], data: &str) -> Option<String> {
    use aes_gcm::aead::{Aead, KeyInit};
    let raw = base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()?;
    if raw.len() < 13 {
        return None;
    }
    let (nonce, ct) = raw.split_at(12);
    let cipher = aes_gcm::Aes256Gcm::new(key.into());
    let pt = cipher.decrypt(nonce.into(), ct).ok()?;
    String::from_utf8(pt).ok()
}

/// Every relay comment body is `AMIN-MSG v1 <base64>` — the marker lets
/// the poller (and any human looking at the public issue) tell relay
/// ciphertext apart from ordinary comments instantly.
const MARKER: &str = "AMIN-MSG v1 ";

#[derive(Deserialize)]
struct RelayPayload {
    kind: String,
    id: String,
    text: String,
}

#[derive(Deserialize)]
struct IssueComment {
    id: u64,
    body: Option<String>,
}

#[derive(Deserialize)]
struct Issue {
    number: u64,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

fn auth(req: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    req.header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "amin-app")
}

/// Finds the relay issue (by its label) or creates it. The number is also
/// cached in settings so restarts don't re-search.
async fn ensure_issue(app: &tauri::AppHandle, token: &str) -> Result<u64, String> {
    if let Some(cached) = read_setting(app, crate::commands::BRIDGE_ISSUE_NUMBER_KEY) {
        if let Ok(n) = cached.parse::<u64>() {
            return Ok(n);
        }
    }
    let c = client()?;
    let url = format!(
        "https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/issues?labels={BRIDGE_LABEL}&state=open&per_page=1"
    );
    let found: Vec<Issue> = auth(c.get(&url), token)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let number = if let Some(issue) = found.first() {
        issue.number
    } else {
        let create_url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/issues");
        let body = serde_json::json!({
            "title": "[amin-relay] قناة أمين بين الهاتف واللابتوب — متقفليهاش",
            "body": "التعليقات هنا رسائل مشفّرة بين أمين على هاتف منى وأمين على اللابتوب. \
                     الريبو عام لكن المحتوى مشفّر تشفيرًا كاملًا (AES-256-GCM) — \
                     اللي باين هنا مجرد نص مشفّر، مش الكلام نفسه.",
            "labels": [BRIDGE_LABEL],
        });
        let resp = auth(c.post(&create_url), token)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("couldn't create the relay issue: {}", resp.status()));
        }
        resp.json::<Issue>().await.map_err(|e| e.to_string())?.number
    };
    write_setting(app, crate::commands::BRIDGE_ISSUE_NUMBER_KEY, &number.to_string());
    Ok(number)
}

fn read_setting(app: &tauri::AppHandle, key: &str) -> Option<String> {
    let db = app.try_state::<crate::db::Db>()?;
    let conn = db.0.lock().ok()?;
    crate::commands::get_setting(&conn, key)
}

fn write_setting(app: &tauri::AppHandle, key: &str, value: &str) {
    if let Some(db) = app.try_state::<crate::db::Db>() {
        if let Ok(conn) = db.0.lock() {
            let _ = crate::commands::set_setting(&conn, key, value);
        }
    }
}

/// Starts the poller (idempotent — a second start just supersedes the
/// first loop via the generation counter).
pub fn start(app: tauri::AppHandle) {
    let my_gen = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn(async move {
        // `since` starts now: anything said while the bridge was off is
        // stale by definition — replaying hours-old commands against her
        // Mac the moment it wakes up would be far worse than dropping them.
        let mut since = chrono::Utc::now();
        let mut last_comment_id: u64 = 0;
        loop {
            if GENERATION.load(Ordering::SeqCst) != my_gen {
                return;
            }
            if let Err(e) = tick(&app, &mut since, &mut last_comment_id).await {
                // Transient network failures are normal here (laptop on
                // flaky wifi); log locally, never crash the loop.
                eprintln!("phone_bridge: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(POLL_SECONDS)).await;
        }
    });
}

pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

async fn tick(
    app: &tauri::AppHandle,
    since: &mut chrono::DateTime<chrono::Utc>,
    last_comment_id: &mut u64,
) -> Result<(), String> {
    let token = read_setting(app, crate::commands::GITHUB_TOKEN_KEY)
        .filter(|v| !v.trim().is_empty())
        .ok_or("no github token")?;
    let passphrase = read_setting(app, crate::commands::BRIDGE_PASSPHRASE_KEY)
        .filter(|v| !v.is_empty())
        .ok_or("no bridge passphrase")?;
    let key = derive_key(&passphrase);
    let issue = ensure_issue(app, &token).await?;

    let c = client()?;
    let url = format!(
        "https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/issues/{issue}/comments?since={}&per_page=100",
        since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    );
    let comments: Vec<IssueComment> = auth(c.get(&url), &token)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    for comment in comments {
        if comment.id <= *last_comment_id {
            continue;
        }
        *last_comment_id = comment.id;
        let Some(body) = comment.body else { continue };
        let Some(encoded) = body.trim().strip_prefix(MARKER) else { continue };
        // Wrong-passphrase or stranger-authored comments fail decryption
        // and are ignored — that IS the authentication (see module doc).
        let Some(plaintext) = decrypt(&key, encoded) else { continue };
        let Ok(payload) = serde_json::from_str::<RelayPayload>(&plaintext) else { continue };
        if payload.kind != "cmd" {
            continue; // our own replies echo back through the same poll
        }
        // The full, real agent pipeline — same tools, tasks, files,
        // memory, and confirmation policy as talking to the laptop
        // directly. A failure is itself sent back as the reply, so the
        // phone never just times out in silence when the cause is known.
        let (reply_text, emotion) = match crate::commands::run_agent_turn(app, &payload.text).await {
            Ok(r) => (r.text, r.emotion),
            Err(e) => (format!("حصل خطأ عند أمين على اللابتوب: {e}"), None),
        };
        let reply = serde_json::json!({
            "kind": "reply",
            "id": uuid::Uuid::new_v4().to_string(),
            "re": payload.id,
            "text": reply_text,
            "emotion": emotion,
        });
        let encrypted = encrypt(&key, &reply.to_string())?;
        let post_url = format!(
            "https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/issues/{issue}/comments"
        );
        let resp = auth(c.post(&post_url), &token)
            .json(&serde_json::json!({ "body": format!("{MARKER}{encrypted}") }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("couldn't post the reply comment: {}", resp.status()));
        }
    }
    // GitHub's `since` filters on updated_at with minute-ish coarseness in
    // practice — walk it forward conservatively and rely on
    // last_comment_id for exact dedup.
    *since = chrono::Utc::now() - chrono::Duration::seconds(120);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_then_decrypt_round_trips() {
        let key = derive_key("كلمة سر منى");
        let ct = encrypt(&key, "افتحيلي المهام النهارده").unwrap();
        assert_eq!(decrypt(&key, &ct).unwrap(), "افتحيلي المهام النهارده");
    }

    #[test]
    fn a_wrong_passphrase_fails_to_decrypt_instead_of_garbling() {
        let ct = encrypt(&derive_key("الصح"), "رسالة").unwrap();
        assert!(decrypt(&derive_key("الغلط"), &ct).is_none());
    }

    #[test]
    fn tampered_ciphertext_is_rejected_by_the_auth_tag() {
        let key = derive_key("كلمة سر");
        let ct = encrypt(&key, "رسالة").unwrap();
        let mut raw = base64::engine::general_purpose::STANDARD.decode(&ct).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        let tampered = base64::engine::general_purpose::STANDARD.encode(raw);
        assert!(decrypt(&key, &tampered).is_none());
    }

    #[test]
    fn junk_input_is_ignored_not_a_panic() {
        let key = derive_key("x");
        assert!(decrypt(&key, "not base64 at all!!!").is_none());
        assert!(decrypt(&key, "aGVsbG8=").is_none()); // valid b64, too short
    }

    /// Cross-implementation lock: this ciphertext was produced by the
    /// PHONE app's actual WebCrypto code (mobile/index.html's
    /// bridgeEncrypt, run in a real Chromium via Playwright, 2026-08-29)
    /// with the passphrase below. If this test ever breaks, one side's
    /// crypto drifted (KDF salt/iterations, nonce layout, or base64) and
    /// the bridge would fail silently in the field — that drift must show
    /// up HERE instead.
    #[test]
    fn decrypts_real_ciphertext_produced_by_the_phone_apps_webcrypto() {
        let key = derive_key("منى-سر-الجسر-٢٠٢٦");
        let from_browser = "JgeO1PNg90vujfRf9Z9xg389qGOR78N9U6mwH1GlU4CMZArX6DNPovKPbg+miCdtfJPUeOr8JAei/tlb+3hXkv9KyZ9WFveiM/mR3CtpKEBqBaGUYLTTDYk=";
        assert_eq!(
            decrypt(&key, from_browser).unwrap(),
            r#"{"kind":"cmd","id":"test1234","text":"تجربة الجسر"}"#
        );
    }
}
