use tauri::{AppHandle, Emitter, State};

use crate::brief::DeltaBrief;
use crate::confirmation::{self, PendingAction, PendingConfirmation};
use crate::db::Db;
use crate::files::WorkspaceEntry;
use crate::followups::FollowUp;
use crate::policy::{self, AutonomyLevel, RiskTier};
use crate::tasks::Task;
use crate::voice::{HandsFreeSession, VoiceSession};
use crate::{
    agent, audio_level, audit, brief, browser, elevenlabs, files, followups, memory, notify, simli,
    tasks, tools, verification, voice,
};

const ANTHROPIC_KEY_NAME: &str = "anthropic_api_key";
/// Optional — Amin's voice falls back to the free, local, on-device engine
/// (macos/transcriber/AminVoice.swift) whenever this isn't set. See
/// elevenlabs.rs for why this is a distinct, disclosed trade-off (cost,
/// and the reply text leaving the device) rather than the default.
const ELEVENLABS_KEY_NAME: &str = "elevenlabs_api_key";
/// Which ElevenLabs voice to speak replies with — see
/// elevenlabs::synthesize's doc comment for why the hardcoded default
/// (Rachel, English) mangles Arabic and needs to be Mona's own choice
/// from her ElevenLabs voice library, not guessed by this app.
const ELEVENLABS_VOICE_ID_KEY: &str = "elevenlabs_voice_id";
/// The one ElevenLabs pronunciation dictionary Amin uses — see
/// elevenlabs::{create,add_rule_to}_pronunciation_dictionary and
/// docs/ARCHITECTURE.md's pronunciation-dictionary section. Stored as a
/// pair (dictionary id + its current version) rather than just the id —
/// see PronunciationDictionary's own doc comment for why the version
/// matters just as much.
const ELEVENLABS_PRONUNCIATION_DICT_ID_KEY: &str = "elevenlabs_pronunciation_dict_id";
const ELEVENLABS_PRONUNCIATION_DICT_VERSION_KEY: &str = "elevenlabs_pronunciation_dict_version";
/// Optional — Portrait Mode's real-time talking avatar (see
/// docs/ARCHITECTURE.md's "Visual modes" section and simli.rs). Stored the
/// same way as the two keys above (local settings table, not the OS
/// Keychain — see has_api_key's doc comment for why) for the same reason:
/// consistency with an already-disclosed, already-accepted trade-off,
/// not a new decision made for this key specifically. Entered once in
/// Settings, never typed into a chat message or committed to Git.
const SIMLI_KEY_NAME: &str = "simli_api_key";
/// Which Simli avatar (faceId) to animate — a free preset while Mona is
/// only proving the integration works at all, then her own custom face
/// (built from src/assets/amin-identity.jpg) once she upgrades. See
/// simli.rs for why this is never hardcoded.
const SIMLI_FACE_ID_KEY: &str = "simli_face_id";

/// Hands-free mode settings — see voice::start_hands_free and
/// AminVoice.swift's `HandsFreeListener`. Off by default, every launch, on
/// purpose: it means the microphone stays open continuously while enabled,
/// which is a real privacy trade-off Mona opts into explicitly each
/// session, never something that persists or silently resumes on its own
/// — see `get_hands_free_settings`'s doc comment.
const WAKE_PHRASE_KEY: &str = "wake_phrase";
const CLOSE_PHRASE_KEY: &str = "close_phrase";
const DEFAULT_WAKE_PHRASE: &str = "يا أمين";
/// Deliberately shares no words with `DEFAULT_WAKE_PHRASE` — an earlier
/// default, "خلاص يا أمين", literally *contained* the wake phrase as a
/// substring. Since AminVoice.swift's `heard()` matches by substring
/// containment (see its own doc comment on why: diacritics/exact-boundary
/// matching is too brittle for speech recognition output), saying the
/// close phrase in one breath also satisfied the wake-phrase check, then
/// the tail end of that same breath ("يا أمين") could itself satisfy the
/// *new* active session's close-phrase check moments later — Mona hit
/// this directly: hands-free mode opened and immediately closed again
/// without ever actually listening to a command. See
/// save_hands_free_phrases' substring-overlap validation below, which now
/// rejects this class of phrase pair for any custom phrases too.
const DEFAULT_CLOSE_PHRASE: &str = "كفاية كده";

fn set_setting(conn: &rusqlite::Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        rusqlite::params![key, value, chrono::Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn get_setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .ok()
}

/// Long-term conversation memory, across app restarts — see
/// schema.sql's `conversation_history` and agent::Conversation's doc
/// comment. Called once at startup (lib.rs) to seed the in-memory
/// conversation; a malformed row is skipped rather than failing the
/// whole load.
pub fn load_conversation_history(conn: &rusqlite::Connection) -> Vec<agent::ChatMessage> {
    let Ok(mut stmt) = conn.prepare("SELECT role, content FROM conversation_history ORDER BY id ASC")
    else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let role: String = row.get(0)?;
        let content: String = row.get(1)?;
        Ok((role, content))
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok)
        .filter_map(|(role, content)| {
            serde_json::from_str::<serde_json::Value>(&content)
                .ok()
                .map(|content| agent::ChatMessage { role, content })
        })
        .collect()
}

/// Persists one conversation turn and keeps the table trimmed to a rolling
/// window — long-term memory across restarts, not an unbounded transcript.
/// Best-effort: a failure here shouldn't break the actual conversation
/// turn, so errors are swallowed rather than propagated.
fn persist_turn(conn: &rusqlite::Connection, msg: &agent::ChatMessage) {
    let _ = conn.execute(
        "INSERT INTO conversation_history (ts, role, content) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            chrono::Utc::now().to_rfc3339(),
            msg.role,
            serde_json::to_string(&msg.content).unwrap_or_default(),
        ],
    );
    let _ = conn.execute(
        "DELETE FROM conversation_history WHERE id NOT IN (
            SELECT id FROM conversation_history ORDER BY id DESC LIMIT 200
        )",
        [],
    );
}

/// Pushes `msg` onto the in-memory history, persists it, and re-applies
/// the sliding-window cap — the one place every conversation-mutating
/// call site should go through, so persistence can never be forgotten at
/// one of them.
fn push_turn(conn: &rusqlite::Connection, turns: &mut Vec<agent::ChatMessage>, msg: agent::ChatMessage) {
    persist_turn(conn, &msg);
    turns.push(msg);
    agent::trim_history(turns);
}

#[derive(serde::Serialize)]
pub struct AppInfo {
    name: &'static str,
    version: &'static str,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: "Amin",
        version: env!("CARGO_PKG_VERSION"),
    }
}

// The Anthropic key was originally stored in the OS Keychain via
// secrets.rs (see that module for the generic wrapper, still used
// elsewhere). On at least one real Mac, saving it there reported success
// every time yet reading it back — moments later, same running session —
// reliably came back "No matching entry found in secure storage". That's
// not a permissions prompt or an ambiguous-item issue (checked the
// keyring crate's own macOS backend source for both); it looks like a
// genuine environment-specific Keychain fault this app can't work around,
// and there's no second Mac available to root-cause it further. Mona
// chose, knowingly, to store it in the local settings table instead (the
// same table already reliably holding autonomy_level/kill_switch) so Amin
// is actually usable now, rather than staying blocked on an unresolved
// OS-level issue. Trade-off, stated plainly: this is local-disk storage
// with the same protection as any other file only her logged-in macOS
// account can read — not Keychain's at-rest encryption. See
// docs/SECURITY.md.
#[tauri::command]
pub fn has_api_key(db: State<Db>) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(get_setting(&conn, ANTHROPIC_KEY_NAME)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false))
}

#[tauri::command]
pub fn save_api_key(key: String, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, ANTHROPIC_KEY_NAME, key.trim())?;
    audit::record(
        &conn,
        "user",
        "save_api_key",
        RiskTier::TrustedDelegation,
        audit::Decision::Confirmed,
        None,
        None,
    )
}

#[tauri::command]
pub fn clear_api_key(db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM settings WHERE key = ?1", [ANTHROPIC_KEY_NAME])
        .map_err(|e| e.to_string())?;
    audit::record(
        &conn,
        "user",
        "clear_api_key",
        RiskTier::TrustedDelegation,
        audit::Decision::Confirmed,
        None,
        None,
    )
}

#[tauri::command]
pub fn has_elevenlabs_key(db: State<Db>) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(get_setting(&conn, ELEVENLABS_KEY_NAME)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false))
}

#[tauri::command]
pub fn save_elevenlabs_key(key: String, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, ELEVENLABS_KEY_NAME, key.trim())?;
    audit::record(
        &conn,
        "user",
        "save_elevenlabs_key",
        RiskTier::TrustedDelegation,
        audit::Decision::Confirmed,
        None,
        None,
    )
}

#[tauri::command]
pub fn clear_elevenlabs_key(db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM settings WHERE key = ?1", [ELEVENLABS_KEY_NAME])
        .map_err(|e| e.to_string())?;
    audit::record(
        &conn,
        "user",
        "clear_elevenlabs_key",
        RiskTier::TrustedDelegation,
        audit::Decision::Confirmed,
        None,
        None,
    )
}

#[tauri::command]
pub fn has_simli_key(db: State<Db>) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(get_setting(&conn, SIMLI_KEY_NAME)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false))
}

#[tauri::command]
pub fn save_simli_key(key: String, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, SIMLI_KEY_NAME, key.trim())?;
    audit::record(
        &conn,
        "user",
        "save_simli_key",
        RiskTier::TrustedDelegation,
        audit::Decision::Confirmed,
        None,
        None,
    )
}

#[tauri::command]
pub fn clear_simli_key(db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM settings WHERE key = ?1", [SIMLI_KEY_NAME])
        .map_err(|e| e.to_string())?;
    audit::record(
        &conn,
        "user",
        "clear_simli_key",
        RiskTier::TrustedDelegation,
        audit::Decision::Confirmed,
        None,
        None,
    )
}

#[tauri::command]
pub fn get_simli_face_id(db: State<Db>) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(get_setting(&conn, SIMLI_FACE_ID_KEY).unwrap_or_default())
}

#[tauri::command]
pub fn save_simli_face_id(face_id: String, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, SIMLI_FACE_ID_KEY, face_id.trim())
}

#[tauri::command]
pub fn get_elevenlabs_voice_id(db: State<Db>) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(get_setting(&conn, ELEVENLABS_VOICE_ID_KEY).unwrap_or_default())
}

/// Real bug from a real Mac screenshot (2026-08-28): Mona pasted her actual
/// ElevenLabs API key into this field instead of a Voice ID — an easy
/// mistake, since both fields sit right next to each other and both are
/// opaque pasted strings. The result was a confusing `404 voice_not_found`
/// naming the API key itself as the "voice", every ElevenLabs call failing,
/// and a silent fallback to the on-device voice she then (reasonably) read
/// as "the voice is broken" rather than "the wrong string is in the wrong
/// box." Every real ElevenLabs API key starts with `sk_`; no real Voice ID
/// ever does — this is a cheap, reliable check that turns that whole
/// failure mode into an immediate, specific error instead of a mystery.
#[tauri::command]
pub fn save_elevenlabs_voice_id(voice_id: String, db: State<Db>) -> Result<(), String> {
    let voice_id = voice_id.trim();
    if voice_id.starts_with("sk_") {
        return Err(
            "الكود ده شكله مفتاح API (ElevenLabs API key) مش Voice ID — الـ Voice ID بتاخديه من صفحة Voices في حسابك، مش من صفحة الـ API keys".to_string(),
        );
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, ELEVENLABS_VOICE_ID_KEY, voice_id)
}

/// The saved pronunciation-dictionary id, or empty if
/// create_amin_pronunciation_dictionary has never been run — drives
/// Settings' "create" vs. "already set up" wording.
#[tauri::command]
pub fn get_pronunciation_dictionary_id(db: State<Db>) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(get_setting(&conn, ELEVENLABS_PRONUNCIATION_DICT_ID_KEY).unwrap_or_default())
}

/// Creates Amin's ElevenLabs pronunciation dictionary from
/// elevenlabs::default_pronunciation_rules() (Mona's own real-world
/// findings — see docs/ARCHITECTURE.md) and saves the resulting id +
/// version. A real, live API call — needs her own ElevenLabs key already
/// saved; there is no way to create this on her behalf without it, and
/// no way to verify it actually improves pronunciation without her
/// listening to the result (see that same doc section's honesty note on
/// what could/couldn't be tested from this sandbox).
#[tauri::command]
pub async fn create_amin_pronunciation_dictionary(db: State<'_, Db>) -> Result<(), String> {
    let key = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        get_setting(&conn, ELEVENLABS_KEY_NAME).filter(|v| !v.trim().is_empty())
    };
    let Some(key) = key else {
        return Err("محتاجة تحطي مفتاح ElevenLabs الأول قبل ما تنشئي قاموس النطق".to_string());
    };
    let dict = elevenlabs::create_pronunciation_dictionary(
        &key,
        "Amin Arabic Pronunciation",
        &elevenlabs::default_pronunciation_rules(),
    )
    .await?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, ELEVENLABS_PRONUNCIATION_DICT_ID_KEY, &dict.id)?;
    set_setting(&conn, ELEVENLABS_PRONUNCIATION_DICT_VERSION_KEY, &dict.version_id)
}

/// Adds one new word/correct-pronunciation pair to Amin's existing
/// dictionary — the ongoing mechanism Mona asked for ("أي كلمة جديدة تُنطق
/// غلط... نضيفها للقاموس نفسه بدل تغيير الصوت"), rather than a one-time
/// fixed list. Updates the stored version_id to the new one — see
/// PronunciationDictionary's doc comment for why an unclaimed old version
/// would silently drop this new rule from every later request.
#[tauri::command]
pub async fn add_pronunciation_rule(word: String, correct_pronunciation: String, db: State<'_, Db>) -> Result<(), String> {
    let (key, dictionary_id) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        (
            get_setting(&conn, ELEVENLABS_KEY_NAME).filter(|v| !v.trim().is_empty()),
            get_setting(&conn, ELEVENLABS_PRONUNCIATION_DICT_ID_KEY).filter(|v| !v.trim().is_empty()),
        )
    };
    let Some(key) = key else {
        return Err("محتاجة تحطي مفتاح ElevenLabs الأول".to_string());
    };
    let Some(dictionary_id) = dictionary_id else {
        return Err("لسه مفيش قاموس نطق — أنشئيه الأول".to_string());
    };
    let rule = elevenlabs::PronunciationRule { string_to_replace: word, alias: correct_pronunciation };
    let dict = elevenlabs::add_pronunciation_rules(&key, &dictionary_id, &[rule]).await?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, ELEVENLABS_PRONUNCIATION_DICT_VERSION_KEY, &dict.version_id)
}

#[tauri::command]
pub fn get_autonomy_level(db: State<Db>) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let level = get_setting(&conn, "autonomy_level").unwrap_or_else(|| {
        AutonomyLevel::default().as_str().to_string()
    });
    Ok(level)
}

#[tauri::command]
pub fn set_autonomy_level(level: String, db: State<Db>) -> Result<(), String> {
    let parsed = AutonomyLevel::parse(&level)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, "autonomy_level", parsed.as_str())?;
    audit::record(
        &conn,
        "user",
        "set_autonomy_level",
        RiskTier::TrustedDelegation,
        audit::Decision::Confirmed,
        Some(parsed.as_str()),
        None,
    )
}

#[tauri::command]
pub fn is_halted(db: State<Db>) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(get_setting(&conn, "kill_switch").as_deref() == Some("on"))
}

#[tauri::command]
pub fn set_kill_switch(active: bool, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, "kill_switch", if active { "on" } else { "off" })?;
    audit::record(
        &conn,
        "user",
        if active { "kill_switch_on" } else { "kill_switch_off" },
        RiskTier::ConfirmHighRisk,
        audit::Decision::Executed,
        None,
        None,
    )
}

#[tauri::command]
pub fn classify_action(domain: String) -> String {
    policy::classify(&domain).as_str().to_string()
}

/// A reply from Amin's Agent Core, ready for the frontend. `emotion`, when
/// present, is the tone Claude tagged its own reply with (see
/// agent::extract_emotion) — Mona never sees the raw marker, only this
/// parsed-out field, meant to drive Amin's presence now and a future
/// hologram/avatar face's expression later.
#[derive(serde::Serialize)]
pub struct AgentReply {
    pub text: String,
    pub emotion: Option<String>,
}

impl AgentReply {
    fn new(text: String) -> Self {
        let (text, emotion) = agent::extract_emotion(&text);
        AgentReply { text, emotion }
    }
}

/// Send one turn to Amin's Agent Core. This is the one place Mona's
/// non-negotiable instruction — "any step Amin wants to take waits for my
/// explicit word before it runs" — is actually enforced at runtime, not
/// just described in the system prompt:
///
/// 1. If a ConfirmHighRisk tool call is already pending from the previous
///    turn, this message IS Mona's answer to it — `resolve_pending_action`
///    reads it as approve/deny/unclear and never starts a new turn until
///    that's settled.
/// 2. Otherwise, Claude gets the real tool registry (`tools.rs`). If it
///    calls a tool classified Auto/TrustedDelegation, that tool runs right
///    away (logged either way), then Claude gets one follow-up call to
///    narrate the result in plain language. If it calls a ConfirmHighRisk
///    tool, nothing runs yet — the call is stored as pending and this
///    returns a confirmation request instead.
///
/// Every branch audits its outcome, including "proposed and waiting" —
/// see `audit::Decision::Proposed`.
#[tauri::command]
pub async fn send_agent_message(
    message: String,
    app: AppHandle,
    db: State<'_, Db>,
    conversation: State<'_, agent::Conversation>,
    pending: State<'_, PendingConfirmation>,
) -> Result<AgentReply, String> {
    let halted = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        get_setting(&conn, "kill_switch").as_deref() == Some("on")
    };
    if halted {
        return Err("Amin is halted — turn the kill switch off to resume.".to_string());
    }

    // Read from the local settings table, not the OS Keychain — see
    // has_api_key's doc comment for why (a reproducible Keychain read
    // failure on Mona's real Mac that a real Keychain error message
    // confirmed was "No matching entry found in secure storage" despite
    // save_api_key reporting success every time).
    let api_key = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        get_setting(&conn, ANTHROPIC_KEY_NAME).filter(|v| !v.trim().is_empty())
    }
    .ok_or_else(|| "لسه محتاجة تحطي مفتاح الاتصال بأنثروبيك — من قسم الأمان والاستقلالية.".to_string())?;

    let existing_pending = {
        let guard = pending.0.lock().map_err(|e| e.to_string())?;
        guard.clone()
    };

    if let Some(action) = existing_pending {
        return resolve_pending_action(&app, &db, &conversation, &pending, &api_key, action, &message).await;
    }

    let tool_defs = tools::tool_definitions();

    let (history, memory_block) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
        push_turn(&conn, &mut turns, agent::ChatMessage::user_text(&message));
        let block = agent::memory_prompt_block(&memory::list(&conn, None).unwrap_or_default());
        (turns.clone(), block)
    };

    let response = match agent::send_message(&api_key, &history, &tool_defs, &memory_block).await {
        Ok(r) => r,
        Err(e) => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let _ = audit::record(
                &conn,
                "amin",
                "agent_message",
                RiskTier::Auto,
                audit::Decision::Blocked,
                Some(&e),
                None,
            );
            return Err(e);
        }
    };

    if let Some(refusal) = response.refusal_error() {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let _ = audit::record(
            &conn,
            "amin",
            "agent_message",
            RiskTier::Auto,
            audit::Decision::Blocked,
            Some(&refusal),
            None,
        );
        return Err(refusal);
    }

    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
        push_turn(
            &conn,
            &mut turns,
            agent::ChatMessage::assistant_content(response.as_assistant_content()),
        );
    }

    let claude_text = response.text();

    let Some((tool_id, tool_name, tool_input)) = response.first_tool_use() else {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let _ = audit::record(
            &conn,
            "amin",
            "agent_message",
            RiskTier::Auto,
            audit::Decision::Executed,
            None,
            None,
        );
        let reply = AgentReply::new(claude_text);
        return if reply.text.is_empty() {
            Err("Amin returned an empty response".to_string())
        } else {
            Ok(reply)
        };
    };

    let tool_id = tool_id.to_string();
    let tool_name = tool_name.to_string();
    let tool_input = tool_input.clone();
    let risk = tools::risk_for(&tool_name);
    let description = tools::describe(&tool_name, &tool_input);

    if risk == RiskTier::ConfirmHighRisk {
        {
            let mut guard = pending.0.lock().map_err(|e| e.to_string())?;
            *guard = Some(PendingAction {
                tool_use_id: tool_id,
                name: tool_name.clone(),
                input: tool_input,
                proposed_at: chrono::Utc::now(),
            });
        }
        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let _ = audit::record(
                &conn,
                "amin",
                &tool_name,
                risk,
                audit::Decision::Proposed,
                Some(&description),
                None,
            );
        }

        let (claude_text, emotion) = agent::extract_emotion(&claude_text);
        let prefix = if claude_text.is_empty() {
            String::new()
        } else {
            format!("{claude_text}\n\n")
        };
        return Ok(AgentReply {
            text: format!(
                "{prefix}⏸️ {description}\n\nمستني كلمتك يا مُنى — قولي \"موافقة\" أو \"نفذ\" عشان أكمل، أو \"إلغاء\" لو غيرتِ رأيك."
            ),
            emotion,
        });
    }

    // Auto / TrustedDelegation: safe to run immediately — still logged,
    // and still narrated back rather than executed silently. `execute` locks
    // the DB itself only where it needs it, never across an `.await` — see
    // its doc comment — so it's called here without holding a guard first.
    let exec_result = tools::execute(&app, &db, &tool_name, &tool_input).await;
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let (details, decision) = match &exec_result {
            Ok(_) => (description.clone(), audit::Decision::Executed),
            Err(e) => (format!("{description} — خطأ: {e}"), audit::Decision::Blocked),
        };
        let _ = audit::record(&conn, "amin", &tool_name, risk, decision, Some(&details), None);
    }

    let result_text = match &exec_result {
        Ok(v) => v.to_string(),
        Err(e) => format!("Error: {e}"),
    };

    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
        push_turn(&conn, &mut turns, agent::ChatMessage::tool_result(&tool_id, &result_text, None));
    }

    let fallback = verification::verified_outcome(&exec_result, &description);
    narrate(&api_key, &db, &conversation, &tool_defs, fallback).await
}

/// Mona's reply to an already-pending ConfirmHighRisk tool call. Reads her
/// message with `confirmation::interpret` rather than assuming any
/// non-empty reply means yes — an unclear message re-states what's pending
/// and waits again instead of guessing.
async fn resolve_pending_action(
    app: &AppHandle,
    db: &State<'_, Db>,
    conversation: &State<'_, agent::Conversation>,
    pending: &State<'_, PendingConfirmation>,
    api_key: &str,
    action: PendingAction,
    message: &str,
) -> Result<AgentReply, String> {
    let tool_defs = tools::tool_definitions();
    let description = tools::describe(&action.name, &action.input);
    let risk = tools::risk_for(&action.name);

    // An approval is scoped to this one proposal, not a standing
    // permission — if too long has passed, Mona has likely moved on, and a
    // reply that happens to contain an approval word for something else
    // entirely must never fire this old action by accident. Expire it and
    // make her re-ask, regardless of what her reply says.
    if confirmation::is_expired(&action, chrono::Utc::now()) {
        {
            let mut guard = pending.0.lock().map_err(|e| e.to_string())?;
            *guard = None;
        }
        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let _ = audit::record(
                &conn,
                "amin",
                &action.name,
                risk,
                audit::Decision::Declined,
                Some(&format!("{description} — انتهت صلاحية الموافقة")),
                None,
            );
        }
        return Ok(AgentReply::new(format!(
            "الطلب ده قديم شوية وانتهت صلاحيته: ⏸️ {description}\n\nلو لسه محتاجة تنفيذه، قوليلي تاني من الأول."
        )));
    }

    match confirmation::interpret(message) {
        confirmation::Reply::Unclear => Ok(AgentReply::new(format!(
            "لسه مستني تأكيدك على:\n⏸️ {description}\n\nقولي \"موافقة\" أو \"نفذ\" عشان أكمل، أو \"إلغاء\" لو غيرتِ رأيك."
        ))),
        confirmation::Reply::Deny => {
            {
                let mut guard = pending.0.lock().map_err(|e| e.to_string())?;
                *guard = None;
            }
            {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                let _ = audit::record(
                    &conn,
                    "user",
                    &action.name,
                    risk,
                    audit::Decision::Declined,
                    Some(&description),
                    None,
                );
            }
            {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
                push_turn(
                    &conn,
                    &mut turns,
                    agent::ChatMessage::tool_result(
                        &action.tool_use_id,
                        "Mona declined this action. Do not perform it.",
                        Some(message),
                    ),
                );
            }
            narrate(api_key, db, conversation, &tool_defs, format!("تمام، اتلغى: {description}")).await
        }
        confirmation::Reply::Approve => {
            {
                let mut guard = pending.0.lock().map_err(|e| e.to_string())?;
                *guard = None;
            }
            let exec_result = tools::execute(app, db, &action.name, &action.input).await;
            {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                let (details, decision) = match &exec_result {
                    Ok(_) => (description.clone(), audit::Decision::Executed),
                    Err(e) => (format!("{description} — خطأ: {e}"), audit::Decision::Blocked),
                };
                let _ = audit::record(&conn, "user", &action.name, risk, decision, Some(&details), None);
            }
            let result_text = match &exec_result {
                Ok(v) => v.to_string(),
                Err(e) => format!("Error: {e}"),
            };
            {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
                push_turn(
                    &conn,
                    &mut turns,
                    agent::ChatMessage::tool_result(&action.tool_use_id, &result_text, Some(message)),
                );
            }
            let fallback = verification::verified_outcome(&exec_result, &description);
            narrate(api_key, db, conversation, &tool_defs, fallback).await
        }
    }
}

/// One follow-up call to Claude after a tool has run (or been declined), so
/// Mona gets a natural sentence about the outcome instead of raw tool JSON.
/// `fallback_text` covers the (rare) case this follow-up call itself fails
/// or comes back empty — the action already ran and is already audited by
/// this point, so a narration hiccup is reported gently, not as a failure
/// of the whole turn.
async fn narrate(
    api_key: &str,
    db: &State<'_, Db>,
    conversation: &State<'_, agent::Conversation>,
    tool_defs: &[serde_json::Value],
    fallback_text: String,
) -> Result<AgentReply, String> {
    let (history, memory_block) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let turns = conversation.0.lock().map_err(|e| e.to_string())?;
        let block = agent::memory_prompt_block(&memory::list(&conn, None).unwrap_or_default());
        (turns.clone(), block)
    };

    let response = match agent::send_message(api_key, &history, tool_defs, &memory_block).await {
        Ok(r) => r,
        Err(_) => return Ok(AgentReply::new(fallback_text)),
    };

    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
        push_turn(
            &conn,
            &mut turns,
            agent::ChatMessage::assistant_content(response.as_assistant_content()),
        );
    }

    let text = response.text();
    Ok(if text.is_empty() {
        AgentReply::new(fallback_text)
    } else {
        AgentReply::new(text)
    })
}

/// Reset Amin's conversation memory — both the in-memory working context
/// and the long-term `conversation_history` persisted to disk. An
/// explicit "New conversation" action is the one deliberate way to make
/// Amin genuinely forget, since otherwise it now remembers across app
/// restarts by design (see agent::Conversation). Does not touch the audit
/// log itself — nothing risky happened.
#[tauri::command]
pub fn clear_agent_conversation(conversation: State<'_, agent::Conversation>, db: State<Db>) -> Result<(), String> {
    let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
    turns.clear();
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM conversation_history", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(serde::Serialize)]
pub struct AuditEntry {
    pub id: String,
    pub ts: String,
    pub actor: String,
    pub action: String,
    pub risk_tier: String,
    pub decision: String,
    pub details: Option<String>,
    pub evidence: Option<String>,
}

#[tauri::command]
pub fn list_audit_log(limit: i64, db: State<Db>) -> Result<Vec<AuditEntry>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, ts, actor, action, risk_tier, decision, details, evidence
             FROM audit_log ORDER BY ts DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok(AuditEntry {
                id: row.get(0)?,
                ts: row.get(1)?,
                actor: row.get(2)?,
                action: row.get(3)?,
                risk_tier: row.get(4)?,
                decision: row.get(5)?,
                details: row.get(6)?,
                evidence: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Start push-to-talk listening from the UI (a mic button), independent of
/// the global keyboard shortcut in lib.rs — both call the same
/// `voice::start_listening`.
#[tauri::command]
pub fn start_voice_capture(
    app: AppHandle,
    session: State<VoiceSession>,
    hands_free: State<HandsFreeSession>,
) -> Result<(), String> {
    // The two native engines each open their own AVAudioEngine — running
    // both at once would fight over the same microphone input rather than
    // cleanly cooperate. Hands-free mode already covers this case anyway
    // (no need to also tap the mic button while it's armed).
    if hands_free.is_active() {
        return Err("الاستماع الحر شغّال دلوقتي — قفليه الأول لو عايزة تستخدمي زرار المايك".to_string());
    }
    voice::start_listening(app, session)
}

#[tauri::command]
pub fn stop_voice_capture(session: State<VoiceSession>) -> Result<(), String> {
    voice::stop_listening(session)
}

/// Loads the saved pronunciation dictionary (id + version, see
/// elevenlabs::PronunciationDictionary), if Amin has one — `None` before
/// create_amin_pronunciation_dictionary has ever been run.
fn load_pronunciation_dictionary(conn: &rusqlite::Connection) -> Option<elevenlabs::PronunciationDictionary> {
    let id = get_setting(conn, ELEVENLABS_PRONUNCIATION_DICT_ID_KEY).filter(|v| !v.trim().is_empty())?;
    let version_id = get_setting(conn, ELEVENLABS_PRONUNCIATION_DICT_VERSION_KEY).filter(|v| !v.trim().is_empty())?;
    Some(elevenlabs::PronunciationDictionary { id, version_id })
}

/// Developer Mode debug info (Mona's explicit request, 2026-08-28, item 8):
/// original text / TTS text / pronunciation_dictionary_id / model_id /
/// language_code for whichever engine actually spoke this reply. Fired on
/// every speak_text call, on-device fallback included (with the
/// ElevenLabs-only fields as `null`) — the frontend's Developer Mode panel
/// decides whether to show it, not this function.
fn emit_tts_debug(
    app: &AppHandle,
    original_text: &str,
    tts_text: &str,
    pronunciation_dictionary_id: Option<&str>,
    model_id: Option<&str>,
    language_code: Option<&str>,
) {
    let _ = app.emit(
        "voice://tts-debug",
        serde_json::json!({
            "original_text": original_text,
            "tts_text": tts_text,
            "pronunciation_dictionary_id": pronunciation_dictionary_id,
            "model_id": model_id,
            "language_code": language_code,
        }),
    );
}

/// Speaks Amin's reply aloud. Prefers ElevenLabs (a more expressive,
/// human-sounding voice Mona explicitly asked for) when she's added her
/// own ElevenLabs key; otherwise falls back to the free, local, on-device
/// engine (voice::speak) — never a hard error just because the optional
/// upgrade isn't configured.
#[tauri::command]
pub async fn speak_text(
    app: AppHandle,
    text: String,
    emotion: Option<String>,
    db: State<'_, Db>,
) -> Result<(), String> {
    let original_text = text.clone();
    // Claude's replies are written for the chat UI, which renders markdown
    // (**bold**, bullets, links...) as formatting — a speech engine just
    // reads the punctuation aloud, which is what surfaced as the on-device
    // Arabic voice "breaking up letters" and mis-spelling everything. Only
    // the spoken copy is cleaned; the chat log keeps the original text.
    let text = agent::strip_markdown_for_speech(&text);

    let (eleven_key, voice_id, pronunciation_dictionary, anthropic_key) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        (
            get_setting(&conn, ELEVENLABS_KEY_NAME).filter(|v| !v.trim().is_empty()),
            get_setting(&conn, ELEVENLABS_VOICE_ID_KEY),
            load_pronunciation_dictionary(&conn),
            get_setting(&conn, ANTHROPIC_KEY_NAME).filter(|v| !v.trim().is_empty()),
        )
    };

    let Some(key) = eleven_key else {
        // No ElevenLabs dictionary mechanism reaches the on-device engine
        // (see docs/ARCHITECTURE.md's pronunciation-dictionary section) —
        // this narrow, hand-written fix is the only protection this path
        // has.
        let on_device_text = agent::fix_pronunciation_for_speech(&text);
        emit_tts_debug(&app, &original_text, &on_device_text, None, None, None);
        return voice::speak(app, &on_device_text);
    };

    // Automatic full diacritization (see agent::diacritize_arabic_text) —
    // the general-case fix for ElevenLabs mispronunciation Mona asked for,
    // so she never has to hear a bad word and report it by hand for it to
    // get fixed. Best-effort: no Anthropic key, a network error, or a
    // malformed response all fall back to speaking the plain text rather
    // than blocking speech on this extra call.
    let text = match &anthropic_key {
        Some(k) => agent::diacritize_arabic_text(k, &text).await.unwrap_or(text),
        None => text,
    };

    emit_tts_debug(
        &app,
        &original_text,
        &text,
        pronunciation_dictionary.as_ref().map(|d| d.id.as_str()),
        Some(elevenlabs::model_id()),
        None, // language_code: never sent — see elevenlabs.rs's MODEL_ID audit comment.
    );

    // The streaming WebSocket (audio arrives as ElevenLabs generates it,
    // rather than only after the whole file renders — see
    // docs/ARCHITECTURE.md's "Realtime voice" section) is what Amin should
    // use going forward, but it's new and network-shaped differently than
    // the plain REST call (a dropped/blocked WebSocket connection is a
    // different failure mode than an HTTP request failing). Falling back
    // to the REST endpoint before giving up on ElevenLabs entirely avoids
    // a regression: something that worked over plain HTTPS shouldn't stop
    // working just because the WebSocket path had a bad day.
    let audio = match elevenlabs::synthesize_streaming(
        &key,
        &text,
        voice_id.as_deref(),
        emotion.as_deref(),
        pronunciation_dictionary.as_ref(),
    )
    .await
    {
        Ok(a) => a,
        Err(streaming_err) => {
            match elevenlabs::synthesize(
                &key,
                &text,
                voice_id.as_deref(),
                emotion.as_deref(),
                pronunciation_dictionary.as_ref(),
            )
            .await
            {
                Ok(a) => a,
                Err(e) => {
                    // Both ElevenLabs paths failed (bad key, quota,
                    // network) — fall back to the on-device voice rather
                    // than staying silent.
                    let _ = app.emit(
                        "voice://error",
                        format!("ElevenLabs: {e} (streaming also failed: {streaming_err})"),
                    );
                    let on_device_text = agent::fix_pronunciation_for_speech(&text);
                    emit_tts_debug(&app, &original_text, &on_device_text, None, None, None);
                    return voice::speak(app, &on_device_text);
                }
            }
        }
    };

    // ElevenLabs playback doesn't go through the native engine's callback
    // (on_voice_event), unlike the on-device path — tell hands-free mode
    // what's being said directly here, same as that callback would, so
    // HandsFreeListener can still tell its own echoed voice apart from a
    // real barge-in (see AminVoice.swift's SELF-HEARING note and
    // isLikelySelfEcho).
    voice::set_hands_free_speaking(Some(&text));
    let _ = app.emit("voice://speaking-started", text.clone());
    // Real-time mouth movement for whichever visual mode the frontend is
    // showing (see audio_level.rs) — decoded from the exact same MP3 bytes
    // afplay is about to play, not a separate/approximate source. Cloned
    // before the afplay thread below takes ownership of the original.
    audio_level::spawn_level_emitter(app.clone(), audio.clone());
    let app_for_thread = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = elevenlabs::play(&audio) {
            let _ = app_for_thread.emit("voice://error", e);
        }
        voice::set_hands_free_speaking(None);
        let _ = app_for_thread.emit("voice://speaking-finished", "");
    });
    Ok(())
}

#[tauri::command]
pub fn stop_speaking() -> Result<(), String> {
    voice::stop_speaking()
}

/// Starts recording Mona's voiceprint enrollment (~4 seconds of speech —
/// see VoicePrint.swift's `SpeakerEnrollmentRecorder`). Result arrives
/// asynchronously as `voice://speaker-enrolled` / `voice://speaker-
/// enrollment-failed`, not as this command's return value, since it takes
/// several seconds and the frontend needs to show live progress either way.
#[tauri::command]
pub fn start_speaker_enrollment(app: AppHandle) -> Result<(), String> {
    voice::enroll_speaker(app)
}

/// Whether a voiceprint is already enrolled — drives Settings' "record my
/// voice" vs. "re-record" wording.
#[tauri::command]
pub fn has_enrolled_speaker(app: AppHandle) -> bool {
    voice::has_enrolled_speaker(app)
}

/// Deletes the enrolled voiceprint. Hands-free mode goes back to opening on
/// any wake phrase (no speaker check) until Mona enrolls again.
#[tauri::command]
pub fn clear_enrolled_speaker(app: AppHandle) -> Result<(), String> {
    voice::clear_enrolled_speaker(app)
}

/// A free, shared preset face ("Doctor" — one of Simli's own published
/// example faces, see docs.simli.com/api-reference/preset-faces) — used
/// only while Mona proves the Simli integration itself works, per her own
/// explicit instruction not to pay for a custom face until then. Once she
/// upgrades and builds a real Amin face from amin-identity.jpg, saving
/// that face ID via save_simli_face_id overrides this default.
const SIMLI_DEFAULT_PRESET_FACE_ID: &str = "f0ba4efe-7946-45de-9955-c04a04c367b9";

/// Starts a new Simli session and returns the short-lived session token —
/// never the API key itself — for the frontend's WebRTC client
/// (src/lib/simli/simliClient.ts) to open the signaling WebSocket with.
/// See simli.rs's doc comment for why the WebRTC/audio-streaming half
/// can't live here in Rust.
#[tauri::command]
pub async fn start_simli_session(db: State<'_, Db>) -> Result<String, String> {
    let (api_key, face_id) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let key = get_setting(&conn, SIMLI_KEY_NAME).filter(|v| !v.trim().is_empty());
        let face = get_setting(&conn, SIMLI_FACE_ID_KEY)
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| SIMLI_DEFAULT_PRESET_FACE_ID.to_string());
        (key, face)
    };
    let Some(api_key) = api_key else {
        return Err("مفيش Simli API key متحط في الإعدادات — دخّليه الأول".to_string());
    };
    simli::start_session(&api_key, &face_id).await
}

/// Synthesizes `text` as raw 16kHz/16-bit/mono PCM for the frontend to
/// stream into an already-open Simli session — a separate command from
/// speak_text because the two paths need the audio in fundamentally
/// different shapes (a played file vs. raw bytes over a WebSocket), not
/// because they use a different voice: same ElevenLabs key, same voice
/// ID, same emotion tagging, same "one Amin voice" either way.
#[tauri::command]
pub async fn synthesize_pcm_for_simli(
    text: String,
    emotion: Option<String>,
    db: State<'_, Db>,
) -> Result<Vec<u8>, String> {
    // No local fix_pronunciation_for_speech step here (unlike speak_text's
    // on-device fallback) — this path always goes through ElevenLabs, so
    // the pronunciation dictionary loaded below is the single, real
    // mechanism, not a second hand-written text substitution racing it.
    let text = agent::strip_markdown_for_speech(&text);

    let (eleven_key, voice_id, pronunciation_dictionary, anthropic_key) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        (
            get_setting(&conn, ELEVENLABS_KEY_NAME).filter(|v| !v.trim().is_empty()),
            get_setting(&conn, ELEVENLABS_VOICE_ID_KEY),
            load_pronunciation_dictionary(&conn),
            get_setting(&conn, ANTHROPIC_KEY_NAME).filter(|v| !v.trim().is_empty()),
        )
    };
    let Some(key) = eleven_key else {
        // Unlike speak_text, there is no on-device fallback here: the
        // native AVSpeechSynthesizer path never hands Rust any audio
        // bytes at all (see voice.rs), so there is nothing to send Simli
        // without ElevenLabs configured. Disclosed plainly rather than
        // silently producing silence.
        return Err(
            "Portrait Mode مع Simli محتاج مفتاح ElevenLabs متحط — الصوت المحلي (بدون ElevenLabs) مفيش منه بيانات صوت تتبعت لـ Simli".to_string(),
        );
    };
    // Same automatic full diacritization as speak_text — one Amin voice,
    // one pronunciation fix, regardless of which visual mode is showing.
    let text = match &anthropic_key {
        Some(k) => agent::diacritize_arabic_text(k, &text).await.unwrap_or(text),
        None => text,
    };
    elevenlabs::synthesize_pcm16(&key, &text, voice_id.as_deref(), emotion.as_deref(), pronunciation_dictionary.as_ref()).await
}

#[derive(serde::Serialize)]
pub struct HandsFreeSettings {
    pub enabled: bool,
    pub wake_phrase: String,
    pub close_phrase: String,
}

/// `enabled` is deliberately always `false` here — never read from the
/// persisted `hands_free_enabled` setting `set_hands_free_mode` still
/// writes below. A real, found-in-the-field bug: `lib.rs`'s `setup` never
/// calls `voice::start_hands_free` on launch, so a persisted "on" from a
/// previous session used to make this command tell the frontend hands-free
/// was running when the native listener never actually restarted — the
/// toggle lied. Rather than fix that by making hands-free silently
/// auto-resume a live microphone on every app launch (exactly the kind of
/// "was this listening the whole time without me noticing" mona was
/// alarmed by, mid-way through official government correspondence, on
/// 2026-08-28), the safer call is: it never resumes on its own. She always
/// has to turn it on again this session, deliberately, every time.
#[tauri::command]
pub fn get_hands_free_settings(db: State<Db>) -> Result<HandsFreeSettings, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(HandsFreeSettings {
        enabled: false,
        wake_phrase: get_setting(&conn, WAKE_PHRASE_KEY).unwrap_or_else(|| DEFAULT_WAKE_PHRASE.to_string()),
        close_phrase: get_setting(&conn, CLOSE_PHRASE_KEY)
            .unwrap_or_else(|| DEFAULT_CLOSE_PHRASE.to_string()),
    })
}

/// Rejects empty, identical, or substring-overlapping wake/close phrase
/// pairs. Pulled out of `save_hands_free_phrases` as a pure function so the
/// substring-overlap case — the exact bug behind "hands-free opens and
/// immediately closes again" (see DEFAULT_CLOSE_PHRASE's doc comment) — is
/// directly unit-testable without a database.
fn validate_hands_free_phrases(wake: &str, close: &str) -> Result<(), String> {
    if wake.is_empty() || close.is_empty() {
        return Err("العبارتين لازم ميكونوش فاضيين".to_string());
    }
    if wake == close {
        return Err("لازم عبارة الفتح وعبارة القفل يكونوا مختلفين عن بعض".to_string());
    }
    // AminVoice.swift matches phrases by substring containment (diacritics
    // make exact matching too brittle) — if one phrase contains the other,
    // saying the longer one satisfies both checks in the same breath and
    // hands-free mode opens and immediately closes again.
    let (wake_lower, close_lower) = (wake.to_lowercase(), close.to_lowercase());
    if wake_lower.contains(&close_lower) || close_lower.contains(&wake_lower) {
        return Err(
            "عبارة الفتح وعبارة القفل لازم متكونش وحدة منهم جزء من التانية (زي \"يا أمين\" و\"خلاص يا أمين\") — دي بتسبب فتح وقفل فوري من غير ما يسمعك".to_string(),
        );
    }
    Ok(())
}

/// Saves custom wake/close phrases. Doesn't itself start or stop hands-free
/// mode — a change while it's already running takes effect the next time
/// it's (re)enabled via `set_hands_free_mode`, same as any other setting.
#[tauri::command]
pub fn save_hands_free_phrases(wake_phrase: String, close_phrase: String, db: State<Db>) -> Result<(), String> {
    let wake = wake_phrase.trim();
    let close = close_phrase.trim();
    validate_hands_free_phrases(wake, close)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, WAKE_PHRASE_KEY, wake)?;
    set_setting(&conn, CLOSE_PHRASE_KEY, close)?;
    Ok(())
}

#[cfg(test)]
mod hands_free_phrase_tests {
    use super::validate_hands_free_phrases;

    #[test]
    fn rejects_the_original_default_pair_that_bit_mona() {
        // The literal old defaults: "خلاص يا أمين" contains "يا أمين".
        assert!(validate_hands_free_phrases("يا أمين", "خلاص يا أمين").is_err());
    }

    #[test]
    fn rejects_overlap_in_either_direction() {
        assert!(validate_hands_free_phrases("خلاص يا أمين", "يا أمين").is_err());
    }

    #[test]
    fn accepts_the_new_non_overlapping_defaults() {
        assert!(validate_hands_free_phrases("يا أمين", "كفاية كده").is_ok());
    }

    #[test]
    fn rejects_empty_or_identical_phrases() {
        assert!(validate_hands_free_phrases("", "كفاية كده").is_err());
        assert!(validate_hands_free_phrases("يا أمين", "").is_err());
        assert!(validate_hands_free_phrases("يا أمين", "يا أمين").is_err());
    }
}

/// Turns hands-free mode on or off for this session only — deliberately
/// not persisted (see `get_hands_free_settings`'s doc comment for why: it
/// never silently resumes a live microphone on the next launch). If the
/// native side then fails asynchronously (e.g. on-device recognition
/// unavailable, mic/speech permission denied), that failure only surfaces
/// later as a `voice://error` event, same unresolved gap as the rest of
/// this voice pipeline (see AminVoice.swift's header). Not yet verified on
/// a real Mac.
#[tauri::command]
pub fn set_hands_free_mode(
    enabled: bool,
    app: AppHandle,
    db: State<Db>,
    hands_free: State<HandsFreeSession>,
    voice_session: State<VoiceSession>,
) -> Result<(), String> {
    if enabled && voice_session.is_active() {
        return Err("زرار المايك شغّال دلوقتي — سيبيه يخلص الأول".to_string());
    }
    let (wake_phrase, close_phrase) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        (
            get_setting(&conn, WAKE_PHRASE_KEY).unwrap_or_else(|| DEFAULT_WAKE_PHRASE.to_string()),
            get_setting(&conn, CLOSE_PHRASE_KEY).unwrap_or_else(|| DEFAULT_CLOSE_PHRASE.to_string()),
        )
    };
    if enabled {
        voice::start_hands_free(app, hands_free, &wake_phrase, &close_phrase)
    } else {
        voice::stop_hands_free(hands_free)
    }
}

/// Create a task manually (from the Tasks panel's own form).
#[tauri::command]
pub fn create_task(title: String, db: State<Db>) -> Result<Task, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let task = tasks::create(&conn, &title, "manual")?;
    let _ = audit::record(
        &conn,
        "user",
        "create_task",
        policy::classify("create_task"),
        audit::Decision::Executed,
        Some(&task.title),
        None,
    );
    Ok(task)
}

/// Quick Capture: one text box, no status/source picking — for jotting
/// something down fast (typed or, once voice is wired up, spoken) without
/// interrupting whatever else Mona is doing.
#[tauri::command]
pub fn quick_capture(text: String, db: State<Db>) -> Result<Task, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let task = tasks::create(&conn, &text, "quick_capture")?;
    let _ = audit::record(
        &conn,
        "user",
        "quick_capture",
        policy::classify("quick_capture"),
        audit::Decision::Executed,
        Some(&task.title),
        None,
    );
    Ok(task)
}

#[tauri::command]
pub fn list_tasks(status: Option<String>, db: State<Db>) -> Result<Vec<Task>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    tasks::list(&conn, status.as_deref())
}

#[tauri::command]
pub fn set_task_status(id: String, status: String, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    tasks::set_status(&conn, &id, &status)?;
    let _ = audit::record(
        &conn,
        "user",
        "set_task_status",
        policy::classify("set_task_status"),
        audit::Decision::Executed,
        Some(&format!("{id} -> {status}")),
        None,
    );
    Ok(())
}

/// File access spans Mona's whole home directory (broadened from one
/// dedicated folder at her explicit request — see files.rs's doc comment
/// for the containment check every one of these still goes through). These
/// four Tauri commands are the direct, Mona-initiated path from the Notes
/// panel's own UI (she typed the content and clicked Save herself, which is
/// its own confirmation) — the risk tiers recorded here are audit-log
/// labels, not a gate. When Claude proposes the same operation through the
/// agent tool-use path instead, it goes through `tools::execute`, and
/// `tools::risk_for` — not this file — is what actually withholds it behind
/// Mona's confirmation: every file tool there is ConfirmHighRisk, including
/// a plain read or listing, precisely because that broadened scope now
/// reaches anything on her machine.
#[tauri::command]
pub fn list_workspace_files(app: AppHandle) -> Result<Vec<WorkspaceEntry>, String> {
    files::list(&app, "", false).map(|(entries, _truncated)| entries)
}

#[tauri::command]
pub fn read_workspace_file(app: AppHandle, path: String) -> Result<String, String> {
    files::read(&app, &path)
}

#[tauri::command]
pub fn write_workspace_file(app: AppHandle, path: String, contents: String, db: State<Db>) -> Result<(), String> {
    let result = files::write(&app, &path, &contents);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let _ = audit::record(
        &conn,
        "user",
        "write_workspace_file",
        RiskTier::TrustedDelegation,
        if result.is_ok() {
            audit::Decision::Executed
        } else {
            audit::Decision::Blocked
        },
        Some(&path),
        None,
    );
    result
}

#[tauri::command]
pub fn delete_workspace_file(app: AppHandle, path: String, db: State<Db>) -> Result<(), String> {
    let result = files::delete(&app, &path);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let _ = audit::record(
        &conn,
        "user",
        "delete_workspace_file",
        policy::classify("delete_workspace_file"),
        if result.is_ok() {
            audit::Decision::Executed
        } else {
            audit::Decision::Blocked
        },
        Some(&path),
        None,
    );
    result
}

/// Opens a URL in Amin's own isolated browser window — never Mona's
/// personal browser/profile. See browser.rs and docs/SECURITY.md §4. This
/// is TrustedDelegation, not Auto: showing a page is low-risk, but it's
/// still Amin reaching an external destination, worth a line in the audit
/// log.
#[tauri::command]
pub fn open_browser_url(app: AppHandle, url: String, db: State<Db>) -> Result<(), String> {
    let result = browser::open_url(&app, &url);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let _ = audit::record(
        &conn,
        "user",
        "open_browser_url",
        RiskTier::TrustedDelegation,
        if result.is_ok() {
            audit::Decision::Executed
        } else {
            audit::Decision::Blocked
        },
        Some(&url),
        None,
    );
    result
}

pub(crate) fn task_title(conn: &rusqlite::Connection, task_id: &str) -> String {
    conn.query_row(
        "SELECT title FROM tasks WHERE id = ?1",
        rusqlite::params![task_id],
        |row| row.get(0),
    )
    .unwrap_or_else(|_| task_id.to_string())
}

/// Follow-up Engine (local only for now — see followups.rs's module doc
/// for why "sent" doesn't mean an email went out yet — except for the one
/// real local channel, a native OS notification, which this and
/// escalate_follow_up below both use).
#[tauri::command]
pub fn create_follow_up(app: AppHandle, task_id: String, due_at: String, db: State<Db>) -> Result<FollowUp, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let follow_up = followups::create(&conn, &task_id, &due_at)?;
    let _ = audit::record(
        &conn,
        "user",
        "create_follow_up",
        RiskTier::Auto,
        audit::Decision::Executed,
        Some(&format!("task {task_id} due {due_at}")),
        None,
    );
    // Parsed comparison, not a raw string one — chrono's own to_rfc3339()
    // uses a "+00:00" suffix while due_at may arrive as "...Z" (e.g. from
    // JS's Date.toISOString()); those represent the same instants but
    // don't compare correctly as strings.
    let already_due = chrono::DateTime::parse_from_rfc3339(&follow_up.due_at)
        .map(|due| due <= chrono::Utc::now())
        .unwrap_or(false);
    if already_due {
        notify::send(&app, "أمين — متابعة", &task_title(&conn, &task_id));
    }
    Ok(follow_up)
}

#[tauri::command]
pub fn list_follow_ups(task_id: Option<String>, db: State<Db>) -> Result<Vec<FollowUp>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    followups::list(&conn, task_id.as_deref())
}

#[tauri::command]
pub fn list_due_follow_ups(db: State<Db>) -> Result<Vec<FollowUp>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    followups::list_due(&conn, chrono::Utc::now())
}

#[tauri::command]
pub fn escalate_follow_up(app: AppHandle, id: String, db: State<Db>) -> Result<FollowUp, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let follow_up = followups::escalate(&conn, &id)?;
    let _ = audit::record(
        &conn,
        "amin",
        "escalate_follow_up",
        RiskTier::TrustedDelegation,
        audit::Decision::Executed,
        Some(&format!("{id} -> {}", follow_up.escalation_stage)),
        None,
    );
    let title = task_title(&conn, &follow_up.task_id);
    let stage_label = match follow_up.escalation_stage.as_str() {
        "firm" => "تذكير",
        "escalate_to_user" => "محتاجة انتباهك",
        _ => "متابعة",
    };
    notify::send(&app, &format!("أمين — {stage_label}"), &title);
    Ok(follow_up)
}

#[tauri::command]
pub fn set_follow_up_status(id: String, status: String, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    followups::set_status(&conn, &id, &status)?;
    let _ = audit::record(
        &conn,
        "user",
        "set_follow_up_status",
        RiskTier::Auto,
        audit::Decision::Executed,
        Some(&format!("{id} -> {status}")),
        None,
    );
    Ok(())
}

/// Local Delta Brief (Phase 3 slice that needs no Gmail/Calendar) — a
/// "what changed" summary of Amin's own local activity. See brief.rs.
#[tauri::command]
pub fn generate_delta_brief(db: State<Db>) -> Result<DeltaBrief, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    brief::generate(&conn)
}

#[derive(serde::Serialize)]
pub struct PendingActionSummary {
    pub tool_name: String,
    pub description: String,
    pub proposed_at: String,
    pub expired: bool,
}

/// Developer/Diagnostics Mode's read into `PendingConfirmation` — the same
/// state `send_agent_message`/`resolve_pending_action` act on, exposed
/// read-only so a diagnostics screen can show Mona (or a developer) exactly
/// what Amin is currently waiting on approval for, if anything, without
/// having to reconstruct it from the audit log by hand. Never mutates the
/// pending slot — only `resolve_pending_action` does that.
#[tauri::command]
pub fn get_pending_action(pending: State<PendingConfirmation>) -> Result<Option<PendingActionSummary>, String> {
    let guard = pending.0.lock().map_err(|e| e.to_string())?;
    Ok(guard.as_ref().map(|action| PendingActionSummary {
        tool_name: action.name.clone(),
        description: tools::describe(&action.name, &action.input),
        proposed_at: action.proposed_at.to_rfc3339(),
        expired: confirmation::is_expired(action, chrono::Utc::now()),
    }))
}
