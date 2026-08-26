//! ElevenLabs text-to-speech — Mona explicitly asked for a more human,
//! emotionally expressive voice than the on-device AVSpeechSynthesizer
//! engine (see macos/transcriber/AminVoice.swift) can produce, and picked
//! ElevenLabs over OpenAI Audio for it. This is a genuinely different
//! trade-off from the rest of Amin's voice pipeline: the reply text leaves
//! the device to a third-party API, and it costs money per character
//! synthesized — both disclosed to her plainly, not assumed. Requires her
//! own ElevenLabs API key (see commands.rs's has/save/clear_elevenlabs_key);
//! falls back to the free, local, on-device engine when it isn't set.

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use tokio_tungstenite::tungstenite::Message;

const ELEVENLABS_TTS_URL: &str = "https://api.elevenlabs.io/v1/text-to-speech";
/// The plain streaming TTS WebSocket — not ElevenAgents/Speech Engine,
/// which both require hosting a public server of our own (see
/// docs/ARCHITECTURE.md's "Realtime voice" section for why that's ruled
/// out). Just an `xi-api-key`, same as the REST call above.
const ELEVENLABS_WS_URL: &str = "wss://api.elevenlabs.io/v1/text-to-speech";
/// "Rachel" — one of ElevenLabs' premade voices, available on every
/// account without any extra setup. Not chosen for Arabic specifically;
/// swap it once Mona picks a voice she prefers from her own ElevenLabs
/// library. `eleven_multilingual_v2` is the model, not the voice — it's
/// what lets this voice read Arabic text at all.
const DEFAULT_VOICE_ID: &str = "21m00Tcm4TlvDq8ikWAM";
const MODEL_ID: &str = "eleven_multilingual_v2";

/// Maps Claude's own `[[emotion:VALUE]]` tag (see agent::KNOWN_EMOTIONS,
/// agent::extract_emotion) to ElevenLabs' per-request `voice_settings` —
/// the concrete mechanism behind "a voice with actual emotional tone",
/// not a cosmetic label. Lower `stability` + higher `style` reads as more
/// animated/expressive delivery; higher stability + lower style reads as
/// calmer and more measured. `similarity_boost` and speaker boost stay
/// fixed — they're about matching the chosen voice's timbre, not mood.
/// Unknown/missing emotions fall back to ElevenLabs' own balanced default
/// rather than guessing.
fn voice_settings_for_emotion(emotion: Option<&str>) -> serde_json::Value {
    let (stability, style) = match emotion {
        Some("excited") => (0.30, 0.75),
        Some("happy") => (0.45, 0.55),
        Some("playful") => (0.35, 0.65),
        Some("concerned") => (0.60, 0.30),
        Some("apologetic") => (0.65, 0.25),
        Some("serious") => (0.80, 0.10),
        Some("calm") => (0.75, 0.15),
        _ => (0.50, 0.35),
    };
    serde_json::json!({
        "stability": stability,
        "similarity_boost": 0.75,
        "style": style,
        "use_speaker_boost": true,
    })
}

/// Calls ElevenLabs' TTS API and returns the raw MP3 bytes. Does not play
/// them — see `play`. `voice_id` overrides the default (Rachel, English)
/// with one Mona picked from her own ElevenLabs voice library — see
/// commands::get_elevenlabs_voice_id/save_elevenlabs_voice_id. Rachel
/// reading Arabic through the multilingual model is exactly the mangled,
/// mispronounced speech Mona reported even after the markdown/emoji
/// cleanup in agent::strip_markdown_for_speech: that cleanup fixed
/// reading literal punctuation aloud, not the underlying voice being the
/// wrong language for the text. `emotion` is Claude's own tag for this
/// reply (see voice_settings_for_emotion) — the same tag already driving
/// AminPresence's on-screen expression now also shapes delivery, one real
/// step toward emotionally expressive speech rather than a flat reading.
pub async fn synthesize(
    api_key: &str,
    text: &str,
    voice_id: Option<&str>,
    emotion: Option<&str>,
) -> Result<Vec<u8>, String> {
    let voice_id = voice_id.filter(|v| !v.trim().is_empty()).unwrap_or(DEFAULT_VOICE_ID);
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{ELEVENLABS_TTS_URL}/{voice_id}"))
        .header("xi-api-key", api_key)
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "text": text,
            "model_id": MODEL_ID,
            "voice_settings": voice_settings_for_emotion(emotion),
        }))
        .send()
        .await
        .map_err(|e| format!("couldn't reach ElevenLabs: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("ElevenLabs API error ({status}): {body}"));
    }

    response
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("couldn't read ElevenLabs audio: {e}"))
}

/// The first WebSocket message per ElevenLabs' stream-input protocol:
/// establishes voice settings and generation config for the whole
/// utterance. `text` must be non-empty per their docs even though no real
/// text goes here yet — a single space is the documented convention.
fn init_message(api_key: &str, emotion: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "text": " ",
        "voice_settings": voice_settings_for_emotion(emotion),
        "generation_config": { "chunk_length_schedule": [120, 160, 250, 290] },
        "xi-api-key": api_key,
    })
}

/// A chunk of the actual text to speak. `try_trigger_generation: true`
/// tells ElevenLabs not to wait for more text before it starts
/// synthesizing — the whole point of using this endpoint at all when
/// (today) the full reply is already known upfront rather than arriving
/// token-by-token from Claude.
fn text_message(text: &str) -> serde_json::Value {
    serde_json::json!({ "text": format!("{text} "), "try_trigger_generation": true })
}

/// Sending empty text is how this protocol says "no more input coming" —
/// ElevenLabs then finishes generating whatever's left and closes with
/// `isFinal: true`.
fn close_message() -> serde_json::Value {
    serde_json::json!({ "text": "" })
}

/// Streaming counterpart to `synthesize`: opens ElevenLabs' plain
/// stream-input WebSocket (no agent, no server of ours to host — see
/// `ELEVENLABS_WS_URL`'s doc comment) and returns the fully assembled
/// audio once ElevenLabs signals `isFinal`. Audio chunks start arriving
/// as ElevenLabs generates them rather than only after the entire file is
/// ready, which is the real point even before Claude's own replies are
/// streamed token-by-token: this function still has to wait for the last
/// chunk before returning today (see docs/ARCHITECTURE.md's "Realtime
/// voice" section — incremental *playback* while chunks arrive, and
/// barge-in, are the next slice, not yet built), but every future step
/// (streaming Claude's tokens in as `text_message`s, playing chunks as
/// `play` receives them) builds on this same connection instead of
/// starting over.
pub async fn synthesize_streaming(
    api_key: &str,
    text: &str,
    voice_id: Option<&str>,
    emotion: Option<&str>,
) -> Result<Vec<u8>, String> {
    let voice_id = voice_id.filter(|v| !v.trim().is_empty()).unwrap_or(DEFAULT_VOICE_ID);
    let url = format!("{ELEVENLABS_WS_URL}/{voice_id}/stream-input?model_id={MODEL_ID}");

    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| format!("couldn't open ElevenLabs streaming connection: {e}"))?;
    let (mut write, mut read) = ws_stream.split();

    for msg in [init_message(api_key, emotion), text_message(text), close_message()] {
        write
            .send(Message::Text(msg.to_string().into()))
            .await
            .map_err(|e| format!("couldn't send to ElevenLabs stream: {e}"))?;
    }

    let mut audio = Vec::new();
    while let Some(msg) = read.next().await {
        let msg = msg.map_err(|e| format!("ElevenLabs stream error: {e}"))?;
        let Message::Text(payload) = msg else { continue };
        let parsed: serde_json::Value = serde_json::from_str(payload.as_str())
            .map_err(|e| format!("couldn't parse ElevenLabs stream message: {e}"))?;
        if let Some(chunk) = parsed.get("audio").and_then(|v| v.as_str()) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(chunk)
                .map_err(|e| format!("couldn't decode ElevenLabs audio chunk: {e}"))?;
            audio.extend_from_slice(&bytes);
        }
        if parsed.get("isFinal").and_then(|v| v.as_bool()) == Some(true) {
            break;
        }
    }

    if audio.is_empty() {
        return Err("ElevenLabs streaming returned no audio".to_string());
    }
    Ok(audio)
}

/// Plays MP3 bytes through the system's default output via macOS's
/// built-in `afplay` CLI — ElevenLabs hands back raw audio bytes rather
/// than something the native voice engine's AVSpeechSynthesizer path
/// could take over from. Blocks until playback finishes; callers run this
/// on a background thread so the calling command can return immediately,
/// the same pattern already used for the on-device engine.
///
/// Registers its process id with `voice::set_afplay_pid` so
/// `commands::stop_speaking` can interrupt it — without this, "stop
/// speaking" would only ever reach the on-device engine and silently do
/// nothing while an ElevenLabs reply was playing.
pub fn play(audio: &[u8]) -> Result<(), String> {
    let mut path = std::env::temp_dir();
    path.push(format!("amin-speech-{}.mp3", uuid::Uuid::new_v4()));
    std::fs::write(&path, audio).map_err(|e| format!("couldn't write speech audio: {e}"))?;

    let result = match Command::new("afplay").arg(&path).spawn() {
        Ok(mut child) => {
            crate::voice::set_afplay_pid(Some(child.id()));
            let status = child.wait();
            crate::voice::set_afplay_pid(None);
            match status {
                Ok(s) if s.success() => Ok(()),
                // Killed by stop_speaking (SIGTERM) — an expected
                // interruption, not a real playback failure.
                Ok(s) if s.signal().is_some() => Ok(()),
                Ok(s) => Err(format!("afplay exited with status {s}")),
                Err(e) => Err(format!("afplay wait failed: {e}")),
            }
        }
        Err(e) => Err(format!("couldn't run afplay: {e}")),
    };

    let _ = std::fs::remove_file(&path);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_message_carries_the_api_key_and_voice_settings_not_text() {
        let msg = init_message("secret-key", Some("calm"));
        assert_eq!(msg["xi-api-key"], "secret-key");
        assert_eq!(msg["text"], " ");
        assert_eq!(msg["voice_settings"], voice_settings_for_emotion(Some("calm")));
    }

    #[test]
    fn text_message_asks_elevenlabs_to_start_generating() {
        let msg = text_message("أهلاً يا منى");
        assert_eq!(msg["text"], "أهلاً يا منى ");
        assert_eq!(msg["try_trigger_generation"], true);
    }

    #[test]
    fn close_message_is_empty_text() {
        assert_eq!(close_message()["text"], "");
    }

    #[test]
    fn excited_is_more_expressive_than_serious() {
        let excited = voice_settings_for_emotion(Some("excited"));
        let serious = voice_settings_for_emotion(Some("serious"));
        assert!(excited["stability"].as_f64().unwrap() < serious["stability"].as_f64().unwrap());
        assert!(excited["style"].as_f64().unwrap() > serious["style"].as_f64().unwrap());
    }

    #[test]
    fn unknown_or_missing_emotion_falls_back_to_a_balanced_default() {
        let missing = voice_settings_for_emotion(None);
        let unknown = voice_settings_for_emotion(Some("ecstatic"));
        assert_eq!(missing, unknown);
    }

    #[test]
    fn every_known_emotion_produces_valid_settings_in_range() {
        for emotion in [
            "happy",
            "calm",
            "concerned",
            "excited",
            "apologetic",
            "serious",
            "playful",
            "neutral",
        ] {
            let settings = voice_settings_for_emotion(Some(emotion));
            for key in ["stability", "similarity_boost", "style"] {
                let value = settings[key].as_f64().unwrap_or(-1.0);
                assert!(
                    (0.0..=1.0).contains(&value),
                    "{emotion}'s {key} out of range: {value}"
                );
            }
        }
    }
}
