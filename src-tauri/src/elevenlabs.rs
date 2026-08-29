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
/// Audit finding, 2026-08-28 (Mona: Arabic pronunciation "سيئ جدًا... معظم
/// الجمل تُنطق بشكل غير طبيعي" — bad, most sentences pronounced
/// unnaturally), checked against ElevenLabs' own docs rather than assumed:
///
///  - `eleven_multilingual_v2` (the previous value here) DOES list Arabic
///    among its 29 supported languages — the model itself wasn't the
///    fundamental problem. The likely dominant cause is DEFAULT_VOICE_ID
///    (Rachel, an English-trained voice) being what actually spoke, not a
///    model limitation — see the real bug this session found and fixed the
///    same day: Mona's ElevenLabs API key had been pasted into the Voice
///    ID *setting*, so every request silently used this fallback the whole
///    time instead of whatever Arabic voice she'd meant to pick.
///  - `language_code` — the parameter Mona specifically asked about — is
///    explicitly documented as NOT supported by `eleven_multilingual_v2`
///    ("This parameter is not supported for multilingual_v2 models"), so
///    it was never a fixable gap on that model, not an oversight.
///  - `eleven_v3` (what this constant is now) is ElevenLabs' newest,
///    highest-quality model, explicitly documented as supporting 70+
///    languages including Arabic — the two other checked models
///    (`eleven_multilingual_v2`, `eleven_flash_v2_5`) both cap out lower.
///    Its docs describe emotional-delivery "audio tags" ("[excited]",
///    "[whispers]") as embeddable in the text as an ADDITION to
///    `voice_settings`, not a replacement — so `voice_settings_for_emotion`
///    below still applies unchanged.
///
/// **What's NOT verified — no ElevenLabs API key in this sandbox to test
/// with, and Mona's own instruction was explicit not to count an API
/// response as proof of correct pronunciation**: whether `eleven_v3`
/// actually sounds better for Arabic in practice, whether it works over
/// `synthesize_streaming`'s stream-input WebSocket (undocumented in what
/// could be checked — if it doesn't, the existing streaming→REST→on-device
/// fallback chain in `commands::speak_text` already covers that, so this
/// is a safe experiment either way, not a gamble on a single path), and
/// whether it's priced or access-gated differently on her plan. The one
/// thing this audit is confident fixing regardless of the model: Mona
/// still needs to set a real Arabic voice ID from her own ElevenLabs
/// Voice Library — no model choice fixes an English-trained voice reading
/// Arabic, and this sandbox has no access to her account's voice library
/// to pick one on her behalf.
const MODEL_ID: &str = "eleven_v3";

/// The model_id every synthesis call actually sends — exposed so
/// commands::speak_text's Developer Mode debug event reports the real
/// value instead of a second, easily-drifting hardcoded copy.
pub fn model_id() -> &'static str {
    MODEL_ID
}

const PRONUNCIATION_DICTIONARIES_URL: &str = "https://api.elevenlabs.io/v1/pronunciation-dictionaries";

/// A saved ElevenLabs pronunciation dictionary — see
/// commands::{create,add_rule_to}_pronunciation_dictionary. `version_id`
/// changes every time a rule is added (ElevenLabs versions the whole
/// dictionary, not individual rules), which is why this is stored as a
/// pair rather than just the dictionary id: a stale version_id still
/// resolves (old versions stay accessible) but silently omits every rule
/// added after it, so keeping the two in lockstep matters.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PronunciationDictionary {
    pub id: String,
    pub version_id: String,
}

/// One rule for `create_pronunciation_dictionary`/`add_pronunciation_rules`.
/// Alias-only (a plain-text substitution ElevenLabs then reads normally),
/// not phoneme/IPA — Mona's own real-world finding ("التشكيل يحل أخطاء
/// النطق في ElevenLabs" — diacritization fixes ElevenLabs' pronunciation
/// errors) is exactly an alias-rule use case: replacing plain "منى" with
/// the fully-vocalized "مُنَى" is a text substitution, not a phonemic
/// instruction, and alias rules are the one rule type ElevenLabs documents
/// with no model-specific restriction — unlike phoneme/IPA rules, which
/// have no established Arabic precedent to build on here.
pub struct PronunciationRule {
    pub string_to_replace: String,
    pub alias: String,
}

impl PronunciationRule {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "string_to_replace": self.string_to_replace,
            "type": "alias",
            "alias": self.alias,
            // Word-boundary matching, not substring: without this, a rule
            // for "منى" would also fire inside "يتمنى"/"تتمنى" ("to wish",
            // unrelated root) — the exact bug this codebase's own
            // agent::fix_pronunciation_for_speech already had to guard
            // against by hand for the same word.
            "word_boundaries": true,
        })
    }
}

/// Amin's own name-pronunciation rules, from Mona's real listening test —
/// see docs/ARCHITECTURE.md's pronunciation-dictionary section for why
/// alias rules (not phoneme/IPA) and why these specific six words.
pub fn default_pronunciation_rules() -> Vec<PronunciationRule> {
    [
        ("منى", "مُنى"),
        ("أمين", "أَمِين"),
        ("درة البيان", "دُرَّةُ البَيَان"),
        ("المعبيلة", "المَعْبِيلَة"),
        ("عُمان", "عُمَان"),
        ("متابعتك", "مُتابَعَتِكِ"),
    ]
    .into_iter()
    .map(|(word, alias)| PronunciationRule {
        string_to_replace: word.to_string(),
        alias: alias.to_string(),
    })
    .collect()
}

/// Creates a new ElevenLabs pronunciation dictionary from `rules` — see
/// docs/api-reference/pronunciation-dictionaries/create-from-rules.
/// Returns the id/version_id pair to store (commands::
/// create_amin_pronunciation_dictionary saves it as a setting) and attach
/// to every later TTS request via `PronunciationDictionary`.
pub async fn create_pronunciation_dictionary(
    api_key: &str,
    name: &str,
    rules: &[PronunciationRule],
) -> Result<PronunciationDictionary, String> {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{PRONUNCIATION_DICTIONARIES_URL}/add-from-rules"))
        .header("xi-api-key", api_key)
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "name": name,
            "rules": rules.iter().map(PronunciationRule::to_json).collect::<Vec<_>>(),
        }))
        .send()
        .await
        .map_err(|e| format!("couldn't reach ElevenLabs: {e}"))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("ElevenLabs API error ({status}): {body}"));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("couldn't parse ElevenLabs response: {e} (body: {body})"))?;
    let id = parsed["id"].as_str().ok_or("ElevenLabs response had no dictionary id")?;
    let version_id = parsed["version_id"].as_str().ok_or("ElevenLabs response had no version_id")?;
    Ok(PronunciationDictionary { id: id.to_string(), version_id: version_id.to_string() })
}

/// Adds `rules` to an already-created dictionary — see docs/api-reference/
/// pronunciation-dictionaries/add-rules. Returns the NEW version_id, which
/// must replace whatever was stored before (see `PronunciationDictionary`'s
/// doc comment for why an old version_id silently omits the new rules
/// rather than erroring).
pub async fn add_pronunciation_rules(
    api_key: &str,
    dictionary_id: &str,
    rules: &[PronunciationRule],
) -> Result<PronunciationDictionary, String> {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{PRONUNCIATION_DICTIONARIES_URL}/{dictionary_id}/add-rules"))
        .header("xi-api-key", api_key)
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "rules": rules.iter().map(PronunciationRule::to_json).collect::<Vec<_>>(),
        }))
        .send()
        .await
        .map_err(|e| format!("couldn't reach ElevenLabs: {e}"))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("ElevenLabs API error ({status}): {body}"));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("couldn't parse ElevenLabs response: {e} (body: {body})"))?;
    let id = parsed["id"].as_str().unwrap_or(dictionary_id);
    let version_id = parsed["version_id"].as_str().ok_or("ElevenLabs response had no version_id")?;
    Ok(PronunciationDictionary { id: id.to_string(), version_id: version_id.to_string() })
}

/// Builds the `pronunciation_dictionary_locators` array ElevenLabs expects
/// on a TTS request — shared by all three synthesis paths below so the
/// shape only needs to be right in one place.
fn locators_json(dictionary: Option<&PronunciationDictionary>) -> Option<serde_json::Value> {
    dictionary.map(|d| {
        serde_json::json!([{ "pronunciation_dictionary_id": d.id, "version_id": d.version_id }])
    })
}

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
    pronunciation_dictionary: Option<&PronunciationDictionary>,
) -> Result<Vec<u8>, String> {
    let voice_id = voice_id.filter(|v| !v.trim().is_empty()).unwrap_or(DEFAULT_VOICE_ID);
    let client = reqwest::Client::new();
    let mut body = serde_json::json!({
        "text": text,
        "model_id": MODEL_ID,
        "voice_settings": voice_settings_for_emotion(emotion),
    });
    if let Some(locators) = locators_json(pronunciation_dictionary) {
        body["pronunciation_dictionary_locators"] = locators;
    }
    let response = client
        .post(format!("{ELEVENLABS_TTS_URL}/{voice_id}"))
        .header("xi-api-key", api_key)
        .header("content-type", "application/json")
        .json(&body)
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

/// Same call as `synthesize`, but asks ElevenLabs for raw, headerless
/// 16-bit/16kHz/mono PCM (`output_format=pcm_16000`) instead of MP3 — the
/// exact format Simli's real-time avatar requires audio in (confirmed
/// against Simli's own docs: "raw PCM, Int16, 16000 Hz, mono"). Requesting
/// this natively from ElevenLabs, rather than decoding MP3 and resampling
/// it ourselves (see audio_level.rs's decode path, used only for the 3D
/// avatar's loudness meter), avoids a resampling step entirely — one less
/// place for a subtle pitch/speed bug to hide.
pub async fn synthesize_pcm16(
    api_key: &str,
    text: &str,
    voice_id: Option<&str>,
    emotion: Option<&str>,
    pronunciation_dictionary: Option<&PronunciationDictionary>,
) -> Result<Vec<u8>, String> {
    let voice_id = voice_id.filter(|v| !v.trim().is_empty()).unwrap_or(DEFAULT_VOICE_ID);
    let client = reqwest::Client::new();
    let mut body = serde_json::json!({
        "text": text,
        "model_id": MODEL_ID,
        "voice_settings": voice_settings_for_emotion(emotion),
    });
    if let Some(locators) = locators_json(pronunciation_dictionary) {
        body["pronunciation_dictionary_locators"] = locators;
    }
    let response = client
        .post(format!("{ELEVENLABS_TTS_URL}/{voice_id}?output_format=pcm_16000"))
        .header("xi-api-key", api_key)
        .header("content-type", "application/json")
        .json(&body)
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
        .map_err(|e| format!("couldn't read ElevenLabs PCM audio: {e}"))
}

/// The first WebSocket message per ElevenLabs' stream-input protocol:
/// establishes voice settings and generation config for the whole
/// utterance. `text` must be non-empty per their docs even though no real
/// text goes here yet — a single space is the documented convention.
fn init_message(
    api_key: &str,
    emotion: Option<&str>,
    pronunciation_dictionary: Option<&PronunciationDictionary>,
) -> serde_json::Value {
    let mut msg = serde_json::json!({
        "text": " ",
        "voice_settings": voice_settings_for_emotion(emotion),
        "generation_config": { "chunk_length_schedule": [120, 160, 250, 290] },
        "xi-api-key": api_key,
    });
    if let Some(locators) = locators_json(pronunciation_dictionary) {
        msg["pronunciation_dictionary_locators"] = locators;
    }
    msg
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
    pronunciation_dictionary: Option<&PronunciationDictionary>,
) -> Result<Vec<u8>, String> {
    let voice_id = voice_id.filter(|v| !v.trim().is_empty()).unwrap_or(DEFAULT_VOICE_ID);
    let url = format!("{ELEVENLABS_WS_URL}/{voice_id}/stream-input?model_id={MODEL_ID}");

    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| format!("couldn't open ElevenLabs streaming connection: {e}"))?;
    let (mut write, mut read) = ws_stream.split();

    for msg in [init_message(api_key, emotion, pronunciation_dictionary), text_message(text), close_message()] {
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
///
/// Kills whatever `afplay` might already be running first (see
/// `voice::kill_current_afplay`'s comment) — a real bug, a real Mac,
/// Mona: "كلام الـ3D بيتداخل صوته... كإن في صوتين داخلين جوه بعض". Without
/// this, two overlapping `speak_text` calls each spawn their own `afplay`
/// and both play at once; this guarantees at most one ever runs.
pub fn play(audio: &[u8]) -> Result<(), String> {
    crate::voice::kill_current_afplay();

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
        let msg = init_message("secret-key", Some("calm"), None);
        assert_eq!(msg["xi-api-key"], "secret-key");
        assert_eq!(msg["text"], " ");
        assert_eq!(msg["voice_settings"], voice_settings_for_emotion(Some("calm")));
        assert!(msg.get("pronunciation_dictionary_locators").is_none());
    }

    #[test]
    fn init_message_attaches_pronunciation_dictionary_locators_when_given() {
        let dict = PronunciationDictionary { id: "dict1".to_string(), version_id: "v1".to_string() };
        let msg = init_message("secret-key", None, Some(&dict));
        assert_eq!(
            msg["pronunciation_dictionary_locators"],
            serde_json::json!([{ "pronunciation_dictionary_id": "dict1", "version_id": "v1" }])
        );
    }

    #[test]
    fn default_pronunciation_rules_use_word_boundaries_and_alias_type() {
        let rules = default_pronunciation_rules();
        assert!(rules.iter().any(|r| r.string_to_replace == "منى" && r.alias == "مُنى"));
        for rule in &rules {
            let json = rule.to_json();
            assert_eq!(json["type"], "alias");
            assert_eq!(json["word_boundaries"], true);
        }
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
