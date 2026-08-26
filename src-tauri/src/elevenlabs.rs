//! ElevenLabs text-to-speech — Mona explicitly asked for a more human,
//! emotionally expressive voice than the on-device AVSpeechSynthesizer
//! engine (see macos/transcriber/AminVoice.swift) can produce, and picked
//! ElevenLabs over OpenAI Audio for it. This is a genuinely different
//! trade-off from the rest of Amin's voice pipeline: the reply text leaves
//! the device to a third-party API, and it costs money per character
//! synthesized — both disclosed to her plainly, not assumed. Requires her
//! own ElevenLabs API key (see commands.rs's has/save/clear_elevenlabs_key);
//! falls back to the free, local, on-device engine when it isn't set.

use std::process::Command;

const ELEVENLABS_TTS_URL: &str = "https://api.elevenlabs.io/v1/text-to-speech";
/// "Rachel" — one of ElevenLabs' premade voices, available on every
/// account without any extra setup. Not chosen for Arabic specifically;
/// swap it once Mona picks a voice she prefers from her own ElevenLabs
/// library. `eleven_multilingual_v2` is the model, not the voice — it's
/// what lets this voice read Arabic text at all.
const DEFAULT_VOICE_ID: &str = "21m00Tcm4TlvDq8ikWAM";
const MODEL_ID: &str = "eleven_multilingual_v2";

/// Calls ElevenLabs' TTS API and returns the raw MP3 bytes. Does not play
/// them — see `play`.
pub async fn synthesize(api_key: &str, text: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{ELEVENLABS_TTS_URL}/{DEFAULT_VOICE_ID}"))
        .header("xi-api-key", api_key)
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "text": text,
            "model_id": MODEL_ID,
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

/// Plays MP3 bytes through the system's default output via macOS's
/// built-in `afplay` CLI — ElevenLabs hands back raw audio bytes rather
/// than something the native voice engine's AVSpeechSynthesizer path
/// could take over from. Blocks until playback finishes; callers run this
/// on a background thread so the calling command can return immediately,
/// the same pattern already used for the on-device engine.
pub fn play(audio: &[u8]) -> Result<(), String> {
    let mut path = std::env::temp_dir();
    path.push(format!("amin-speech-{}.mp3", uuid::Uuid::new_v4()));
    std::fs::write(&path, audio).map_err(|e| format!("couldn't write speech audio: {e}"))?;

    let result = Command::new("afplay")
        .arg(&path)
        .status()
        .map_err(|e| format!("couldn't run afplay: {e}"))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!("afplay exited with status {status}"))
            }
        });

    let _ = std::fs::remove_file(&path);
    result
}
