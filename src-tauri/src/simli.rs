//! Simli (simli.ai) real-time talking-avatar API — Portrait Mode's engine
//! for real lip-sync tied to Amin's actual voice (see
//! docs/ARCHITECTURE.md's "Visual modes" section for why this was chosen
//! over per-reply video rendering, which real measurement showed costs
//! ~$0.86 and ~260s per reply and is unusable for live conversation).
//!
//! This module only does the one thing that has to happen in Rust: create
//! a session token using Mona's API key (the key itself never leaves this
//! process — only the short-lived session token goes to the frontend).
//! Everything else — the WebSocket signaling, the RTCPeerConnection, and
//! streaming audio into it — has to run in the webview: WebRTC is a
//! browser API with no equivalent Rust implementation in this codebase,
//! and Simli's own protocol carries both the WebRTC signaling *and* the
//! raw audio frames over the same WebSocket (confirmed against Simli's
//! own docs), so splitting audio-sending into Rust while leaving
//! signaling in the frontend isn't possible — they're the same socket.
//! See src/lib/simli/simliClient.ts for that half.
//!
//! Exact endpoint/request shape below is taken directly from Simli's
//! published OpenAPI spec (docs.simli.com/api-reference/openapi.yaml),
//! not guessed — a wrong field name here would fail visibly (a non-2xx
//! response), not silently, so this is the one place worth being precise.

const SIMLI_TOKEN_URL: &str = "https://api.simli.ai/compose/token";

#[derive(serde::Deserialize)]
struct SimliTokenResponse {
    session_token: Option<String>,
    detail: Option<String>,
}

/// Requests a new session token for `face_id`. The token is short-lived
/// and scoped to one session — safe to hand to the frontend, unlike the
/// long-lived API key this function consumes and never returns.
pub async fn start_session(api_key: &str, face_id: &str) -> Result<String, String> {
    if face_id.trim().is_empty() {
        return Err("مفيش Simli face ID متحدد — اختاري preset أو دخلي الـ custom face ID في الإعدادات".to_string());
    }
    let client = reqwest::Client::new();
    let response = client
        .post(SIMLI_TOKEN_URL)
        .header("x-simli-api-key", api_key)
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "faceId": face_id,
            "apiVersion": "v2",
            "sessionAggregator": serde_json::Value::Null,
            "handleSilence": true,
            "maxSessionLength": 3600,
            "maxIdleTime": 300,
            "startFrame": 0,
            "audioInputFormat": "pcm16",
        }))
        .send()
        .await
        .map_err(|e| format!("تعذّر الوصول لـ Simli: {e}"))?;

    let status = response.status();
    let body: SimliTokenResponse = response
        .json()
        .await
        .map_err(|e| format!("رد Simli غير متوقع (HTTP {status}): {e}"))?;

    // The HTTP status is the only reliable success signal here — verified
    // against Simli's real, live endpoint (not just their docs): a bad
    // API key comes back as HTTP 401 with `detail: "INVALID_API_KEY"`,
    // but the body still carries a non-empty, real-looking session_token
    // string regardless. Checking only the body (as their published error
    // example, `{"session_token":"FAIL TOKEN",...}`, would suggest) would
    // have silently treated this exact failure as success.
    if !status.is_success() {
        return Err(format!(
            "Simli رفض إنشاء الجلسة (HTTP {status}): {}",
            body.detail.unwrap_or_else(|| "لا تفاصيل".to_string())
        ));
    }
    match body.session_token {
        Some(token) if !token.is_empty() && token != "FAIL TOKEN" => Ok(token),
        _ => Err(format!(
            "Simli رد بنجاح لكن من غير session token صالح (HTTP {status}): {}",
            body.detail.unwrap_or_else(|| "لا تفاصيل".to_string())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_face_id_is_rejected_before_any_network_call() {
        let err = start_session("fake-key", "").await.unwrap_err();
        assert!(err.contains("face ID"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn whitespace_only_face_id_is_rejected() {
        let err = start_session("fake-key", "   ").await.unwrap_err();
        assert!(err.contains("face ID"), "unexpected error: {err}");
    }

    /// Hits Simli's real, live endpoint with a deliberately invalid key —
    /// no real credentials involved, and never run in CI (no network
    /// there, and this repo's CI doesn't run `cargo test` at all — see
    /// .github/workflows/build-macos.yml). Exists because manually
    /// running exactly this call against the real server on 2026-08-28 is
    /// what caught a real bug: Simli returns HTTP 401 with a non-empty,
    /// real-looking `session_token` for a bad key, which the body-only
    /// check this function used to have would have silently accepted as
    /// success. `cargo test -- --ignored` to re-run this by hand.
    #[tokio::test]
    #[ignore]
    async fn a_real_invalid_key_is_rejected_by_the_live_endpoint() {
        let err = start_session("invalid-test-key-for-protocol-verification", "f0ba4efe-7946-45de-9955-c04a04c367b9")
            .await
            .unwrap_err();
        assert!(err.contains("رفض"), "expected a rejection message, got: {err}");
    }
}
