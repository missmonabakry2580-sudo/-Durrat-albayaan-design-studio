use libloading::{Library, Symbol};
use std::ffi::{c_char, c_int, CStr};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};

/// Whether a push-to-talk session is currently in progress. Plain
/// `std::sync`, deliberately not async — this runs from a global-shortcut
/// key-event handler, whose execution context isn't guaranteed to have a
/// Tokio runtime entered, so it must not depend on one.
pub struct VoiceSession(Mutex<bool>);

impl VoiceSession {
    pub fn new() -> Self {
        VoiceSession(Mutex::new(false))
    }

    pub fn is_active(&self) -> bool {
        self.0.lock().map(|g| *g).unwrap_or(false)
    }
}

impl Default for VoiceSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether hands-free mode (wake phrase opens a session, close phrase or
/// silence ends it — see AminVoice.swift's `HandsFreeListener`) is
/// currently running. Separate from `VoiceSession`: the two are mutually
/// exclusive at the UI level (see commands.rs) but are tracked
/// independently since they call into different native entry points.
pub struct HandsFreeSession(Mutex<bool>);

impl HandsFreeSession {
    pub fn new() -> Self {
        HandsFreeSession(Mutex::new(false))
    }

    pub fn is_active(&self) -> bool {
        self.0.lock().map(|g| *g).unwrap_or(false)
    }
}

impl Default for HandsFreeSession {
    fn default() -> Self {
        Self::new()
    }
}

/// `kind` as passed by `AminVoice.swift`'s C callback: 0 = partial
/// transcript, 1 = final transcript, 2 = error (recognition side); 3 =
/// speech started (text is the sentence Amin is about to speak — see
/// `set_hands_free_speaking`), 4 = speech finished (text always null); 5 =
/// hands-free armed (passively watching for the wake phrase), 6 = wake
/// phrase heard — a command session opened, 7 = the close phrase ended the
/// command session, 8 = hands-free timed out from inactivity, 9 = a real
/// barge-in — Mona started talking over Amin's own reply (text is what she
/// said), 10 = the wake phrase was heard but rejected — the enrolled
/// voiceprint didn't match whoever said it (see VoicePrint.swift), 11 =
/// speaker enrollment succeeded, 12 = speaker enrollment failed (text is
/// why).
type VoiceCallback = unsafe extern "C" fn(c_int, *const c_char);
type StartFn = unsafe extern "C" fn(VoiceCallback) -> c_int;
type StopFn = unsafe extern "C" fn();
type SpeakFn = unsafe extern "C" fn(*const c_char, VoiceCallback) -> c_int;
type StopSpeakingFn = unsafe extern "C" fn();
type StartHandsFreeFn = unsafe extern "C" fn(*const c_char, *const c_char, VoiceCallback) -> c_int;
type StopHandsFreeFn = unsafe extern "C" fn();
type SetHandsFreeSpeakingFn = unsafe extern "C" fn(*const c_char);
type SetVoiceprintModelPathFn = unsafe extern "C" fn(*const c_char);
type HasEnrolledSpeakerFn = unsafe extern "C" fn() -> c_int;
type ClearEnrolledSpeakerFn = unsafe extern "C" fn();
type EnrollSpeakerFn = unsafe extern "C" fn(VoiceCallback) -> c_int;

/// The loaded voice engine, once found — loaded at most once per run, then
/// reused for every push-to-talk session.
static LIBRARY: OnceLock<Library> = OnceLock::new();
/// PID of the `afplay` process currently playing an ElevenLabs reply, if
/// any — set by `elevenlabs::play`, read by `stop_speaking` below. The
/// ElevenLabs path doesn't go through the native engine at all (playback
/// is a plain child process, not AVSpeechSynthesizer), so without this
/// `stop_speaking` would only ever reach the on-device voice and silently
/// do nothing while an ElevenLabs reply is playing.
static AFPLAY_PID: Mutex<Option<u32>> = Mutex::new(None);

pub fn set_afplay_pid(pid: Option<u32>) {
    if let Ok(mut guard) = AFPLAY_PID.lock() {
        *guard = pid;
    }
}

/// Mirrors AFPLAY_PID immediately above, for the audio-level emitter thread
/// audio_level::spawn_level_emitter starts alongside this utterance's
/// afplay playback (see commands::speak_text). `stop_speaking` flips
/// whatever flag is registered here so that thread stops sending
/// voice://audio-level events the instant playback is interrupted, instead
/// of continuing to animate the 3D avatar's mouth after Amin's voice has
/// actually gone silent.
static AUDIO_LEVEL_CANCEL: Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> = Mutex::new(None);

pub fn set_audio_level_cancel(flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) {
    if let Ok(mut guard) = AUDIO_LEVEL_CANCEL.lock() {
        if let Some(old) = guard.take() {
            old.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        *guard = flag;
    }
}
/// Set on the first `start_listening` call so the plain C callback below
/// (which, being `extern "C"`, cannot capture any Rust state) has a way to
/// reach the app and emit events. Amin only ever runs one app instance, so
/// one static handle is all this needs.
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Where the compiled voice engine is expected to live, bundled as a Tauri
/// resource. See macos/transcriber/README.md for the build step that
/// produces it and why it's a dylib loaded in-process rather than a
/// spawned helper.
fn engine_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve("libaminvoice.dylib", tauri::path::BaseDirectory::Resource)
        .map_err(|e| e.to_string())
}

/// Where the converted ECAPA-TDNN speaker-verification model is bundled —
/// see VoicePrint.swift's header and scripts/voiceprint/convert_ecapa_to_coreml.py.
/// Resolved the same way as `engine_path` (a Tauri resource, not a
/// Bundle.main-relative guess made from inside the dylib) and handed to
/// Swift once via `amin_voice_set_voiceprint_model_path`.
fn voiceprint_model_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve("ECAPA_TDNN.mlpackage", tauri::path::BaseDirectory::Resource)
        .map_err(|e| e.to_string())
}

fn engine(app: &AppHandle) -> Result<&'static Library, String> {
    if let Some(lib) = LIBRARY.get() {
        return Ok(lib);
    }

    let path = engine_path(app)?;
    if !path.exists() {
        return Err(format!(
            "the voice engine isn't built yet (expected at {}) — see macos/transcriber/README.md",
            path.display()
        ));
    }

    // Safety: libaminvoice.dylib is built by this repo's own CI step
    // (.github/workflows/build-macos.yml) from
    // macos/transcriber/AminVoice.swift immediately before bundling — never
    // a third-party or user-supplied file.
    let lib = unsafe { Library::new(&path) }
        .map_err(|e| format!("couldn't load the voice engine: {e}"))?;
    // Someone else may have raced us and already set it; either way,
    // LIBRARY now holds a library, which is all the caller needs.
    let _ = LIBRARY.set(lib);
    let lib = LIBRARY.get().expect("just set above");

    // Best-effort: if the model isn't bundled or the path can't be
    // resolved, VoicePrintEngine.verify simply fails open (see its doc
    // comment) — never worth failing the whole engine load over.
    if let Ok(model_path) = voiceprint_model_path(app) {
        if model_path.exists() {
            if let Some(path_str) = model_path.to_str() {
                unsafe {
                    if let Ok(set_path) = lib.get::<SetVoiceprintModelPathFn>(b"amin_voice_set_voiceprint_model_path\0") {
                        if let Ok(c_path) = std::ffi::CString::new(path_str) {
                            set_path(c_path.as_ptr());
                        }
                    }
                }
            }
        }
    }
    Ok(lib)
}

/// Fire-and-forget system-sound cue, entirely independent of ElevenLabs/
/// afplay's `AFPLAY_PID` bookkeeping (see `kill_current_afplay`) — this is
/// never "Amin speaking" and must never be tracked, muted, or killed as
/// part of that logic. Deliberately best-effort: a missing sound file or a
/// spawn failure should never do anything more than silently skip the
/// chime, since the chime itself only exists to make the real voice
/// pipeline's state audible, not to be a critical part of it.
fn play_chime(system_sound_name: &str) {
    let path = format!("/System/Library/Sounds/{system_sound_name}.aiff");
    let _ = std::process::Command::new("afplay").arg(path).spawn();
}

/// Forwards a partial/final/error event from the (in-process) voice engine
/// to the frontend. Runs on whatever thread the Speech framework's
/// recognition task happens to call back on.
unsafe extern "C" fn on_voice_event(kind: c_int, text: *const c_char) {
    let Some(app) = APP_HANDLE.get() else { return };
    let text = if text.is_null() {
        String::new()
    } else {
        // Safety: AminVoice.swift documents this pointer as a NUL-terminated
        // UTF-8 C string valid only for the duration of this call — copy it
        // now rather than holding onto it.
        unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
    };
    // Amin's own voice must never be blindly acted on while hands-free mode
    // is running — see the SELF-HEARING note in AminVoice.swift. This
    // covers the on-device TTS path; the ElevenLabs path
    // (commands::speak_text) doesn't go through this callback at all
    // (playback happens over `afplay`, not the native engine), so it calls
    // `set_hands_free_speaking` directly instead. Passing the actual text
    // (not just a mute flag) is what lets `HandsFreeListener` tell a real
    // barge-in apart from hearing its own reply — see its
    // `isLikelySelfEcho`.
    if kind == 3 {
        set_hands_free_speaking(Some(&text));
    } else if kind == 4 {
        set_hands_free_speaking(None);
    } else if kind == 9 {
        // A genuine barge-in: stop Amin's own playback immediately, before
        // even notifying the frontend — every millisecond here is a
        // millisecond it keeps talking over her. The ElevenLabs path's
        // afplay process and the on-device AVSpeechSynthesizer are both
        // covered by this same call (see stop_speaking's doc comment).
        let _ = stop_speaking();
    }

    // Real bug found 2026-08-28: Mona reported hands-free "never responds"
    // with zero visible sign of anything happening — no error, no change.
    // Two real, separate gaps turned out to compound: (1) `armed`'s only
    // visual signal was a brow/eye blendshape at 0.15-0.2 intensity, well
    // below the ~0.5+ this session already found necessary for this
    // model's small blendshape deltas to read as visible at all (see this
    // file's "small blendshape deltas" note in ARCHITECTURE.md); (2) once
    // the chat UI was removed for the voice-only redesign, there was no
    // replacement feedback at all for "Amin just started listening" — not
    // even the old input box's live partial transcript. A short system
    // chime the instant hands-free actually arms is unambiguous, doesn't
    // depend on ElevenLabs/network/API keys (so it still fires even if
    // those are broken), and turns "did it even start?" from a guess into
    // something she can literally hear within a second of toggling it on.
    if kind == 5 {
        play_chime("Pop");
    } else if kind == 1 {
        play_chime("Tink");
    }

    let _ = match kind {
        0 => app.emit("voice://partial", text),
        1 => app.emit("voice://final", text),
        3 => app.emit("voice://speaking-started", text),
        4 => app.emit("voice://speaking-finished", text),
        5 => app.emit("voice://hands-free-armed", text),
        6 => app.emit("voice://hands-free-listening", text),
        7 => app.emit("voice://hands-free-closed", text),
        // AminVoice.swift's HandsFreeListener stopped re-arming on its own
        // after its inactivity timeout, but deliberately didn't tear down
        // its own audio engine/tap (see its comment on this — re-entrant
        // native calls from inside its own recognition callback). The
        // frontend's listener for this event is what actually finishes the
        // job, by calling the normal set_hands_free_mode(false) command —
        // the same already-correct stop path a manual toggle-off uses.
        8 => app.emit("voice://hands-free-timeout", text),
        9 => app.emit("voice://hands-free-barge-in", text),
        10 => app.emit("voice://hands-free-voice-rejected", text),
        11 => app.emit("voice://speaker-enrolled", text),
        12 => app.emit("voice://speaker-enrollment-failed", text),
        _ => app.emit("voice://error", text),
    };
}

/// Starts push-to-talk listening: loads the voice engine on first use and
/// calls straight into it — no subprocess, no stdin/stdout protocol. A
/// second call while already listening is a no-op, not an error (a key
/// that auto-repeats while held shouldn't start a second session).
pub fn start_listening(app: AppHandle, session: tauri::State<'_, VoiceSession>) -> Result<(), String> {
    let mut listening = session.0.lock().map_err(|e| e.to_string())?;
    if *listening {
        return Ok(());
    }

    let _ = APP_HANDLE.set(app.clone());
    let lib = engine(&app)?;

    let rc = unsafe {
        let start: Symbol<StartFn> = lib
            .get(b"amin_voice_start\0")
            .map_err(|e| format!("voice engine is missing amin_voice_start: {e}"))?;
        start(on_voice_event)
    };
    if rc != 0 {
        return Err(format!("the voice engine failed to start (code {rc})"));
    }

    *listening = true;
    Ok(())
}

/// Stops push-to-talk listening: calls into the voice engine to end the
/// current utterance. A no-op if nothing is listening.
pub fn stop_listening(session: tauri::State<'_, VoiceSession>) -> Result<(), String> {
    let mut listening = session.0.lock().map_err(|e| e.to_string())?;
    if !*listening {
        return Ok(());
    }

    if let Some(lib) = LIBRARY.get() {
        unsafe {
            if let Ok(stop) = lib.get::<StopFn>(b"amin_voice_stop\0") {
                stop();
            }
        }
    }

    *listening = false;
    Ok(())
}

/// Speaks `text` aloud through the same in-process voice engine (macOS's
/// on-device AVSpeechSynthesizer — see AminVoice.swift). Fires
/// `voice://speaking-started` / `voice://speaking-finished` so the
/// frontend can track Amin's real speaking state instead of guessing a
/// fixed duration.
pub fn speak(app: AppHandle, text: &str) -> Result<(), String> {
    let _ = APP_HANDLE.set(app.clone());
    let lib = engine(&app)?;
    let c_text = std::ffi::CString::new(text).map_err(|e| e.to_string())?;

    let rc = unsafe {
        let speak: Symbol<SpeakFn> = lib
            .get(b"amin_voice_speak\0")
            .map_err(|e| format!("voice engine is missing amin_voice_speak: {e}"))?;
        speak(c_text.as_ptr(), on_voice_event)
    };
    if rc != 0 {
        return Err(format!("the voice engine failed to speak (code {rc})"));
    }
    Ok(())
}

/// Kills whatever `afplay` process `AFPLAY_PID` currently points at, if
/// any, and clears the slot. A real bug from a real Mac (2026-08-28, Mona:
/// "كلام الـ3D بيتداخل صوته... كإن في صوتين داخلين جوه بعض" — the 3D's
/// speech overlaps, like two voices inside each other): `AFPLAY_PID` only
/// ever tracked the *latest* spawned process, so if `speak_text` somehow
/// ran twice in close succession (e.g. Enter and a button both firing off
/// one stale `agentBusy` read before either state update had committed —
/// a real React race, not confirmed as *the* trigger here but a real one
/// nonetheless), nothing ever stopped the first `afplay` before the second
/// one started — both played the same reply on top of each other. Calling
/// this immediately before spawning a new `afplay` (see
/// `elevenlabs::play`) guarantees at most one is ever running, regardless
/// of what caused a second `speak_text` call to happen.
pub fn kill_current_afplay() {
    let pid = AFPLAY_PID.lock().ok().and_then(|mut guard| guard.take());
    if let Some(pid) = pid {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
    }
}

/// Stops any in-progress speech immediately — whichever voice is actually
/// speaking, on-device or ElevenLabs. A no-op if nothing is speaking.
/// Also clears hands-free's "Amin is currently saying X" state — without
/// this, a manual stop mid-sentence would leave `HandsFreeListener`
/// comparing new speech against a reply that no longer exists, until the
/// on-device path's own kind-4 callback happened to arrive (which a forced
/// stop can race).
pub fn stop_speaking() -> Result<(), String> {
    if let Some(lib) = LIBRARY.get() {
        unsafe {
            if let Ok(stop) = lib.get::<StopSpeakingFn>(b"amin_voice_stop_speaking\0") {
                stop();
            }
        }
    }
    kill_current_afplay();
    set_audio_level_cancel(None);
    set_hands_free_speaking(None);
    Ok(())
}

/// Starts hands-free mode: Mona says `wake_phrase` to open a session, then
/// `close_phrase` (or just going quiet) to end it — see the HANDS-FREE MODE
/// note in AminVoice.swift for the privacy trade-off (continuous mic use,
/// on-device-only wake-phrase watching). A second call while already
/// running is a no-op, not an error, matching `start_listening`'s pattern.
pub fn start_hands_free(
    app: AppHandle,
    session: tauri::State<'_, HandsFreeSession>,
    wake_phrase: &str,
    close_phrase: &str,
) -> Result<(), String> {
    let mut active = session.0.lock().map_err(|e| e.to_string())?;
    if *active {
        return Ok(());
    }

    let _ = APP_HANDLE.set(app.clone());
    let lib = engine(&app)?;
    let c_wake = std::ffi::CString::new(wake_phrase).map_err(|e| e.to_string())?;
    let c_close = std::ffi::CString::new(close_phrase).map_err(|e| e.to_string())?;

    let rc = unsafe {
        let start: Symbol<StartHandsFreeFn> = lib
            .get(b"amin_voice_start_hands_free\0")
            .map_err(|e| format!("voice engine is missing amin_voice_start_hands_free: {e}"))?;
        start(c_wake.as_ptr(), c_close.as_ptr(), on_voice_event)
    };
    if rc != 0 {
        return Err(format!("hands-free mode failed to start (code {rc})"));
    }

    *active = true;
    Ok(())
}

/// Tells hands-free mode what Amin is currently saying (`Some(text)`) or
/// that it's finished (`None`) — called around every `speak_text` (both
/// the on-device path, via `on_voice_event`'s kind 3/4, and the ElevenLabs
/// path directly). `HandsFreeListener` uses this to tell its own echoed
/// voice apart from Mona genuinely talking over it (a real barge-in) —
/// see AminVoice.swift's `isLikelySelfEcho`. A silent no-op if hands-free
/// mode isn't running or the engine was never loaded — this is a
/// best-effort safety measure, not something that should ever fail loudly
/// and interrupt actually speaking the reply.
pub fn set_hands_free_speaking(text: Option<&str>) {
    if let Some(lib) = LIBRARY.get() {
        unsafe {
            if let Ok(f) = lib.get::<SetHandsFreeSpeakingFn>(b"amin_voice_set_hands_free_speaking\0") {
                match text {
                    Some(t) => {
                        if let Ok(c_text) = std::ffi::CString::new(t) {
                            f(c_text.as_ptr());
                        }
                    }
                    None => f(std::ptr::null()),
                }
            }
        }
    }
}

/// Stops hands-free mode entirely (not just the current command session —
/// the passive wake-phrase watch too). A no-op if it isn't running.
pub fn stop_hands_free(session: tauri::State<'_, HandsFreeSession>) -> Result<(), String> {
    let mut active = session.0.lock().map_err(|e| e.to_string())?;
    if !*active {
        return Ok(());
    }

    if let Some(lib) = LIBRARY.get() {
        unsafe {
            if let Ok(stop) = lib.get::<StopHandsFreeFn>(b"amin_voice_stop_hands_free\0") {
                stop();
            }
        }
    }

    *active = false;
    Ok(())
}

/// Records ~4 seconds of Mona's speech and stores its voiceprint — see
/// VoicePrint.swift's `SpeakerEnrollmentRecorder`. Result arrives
/// asynchronously as `voice://speaker-enrolled` / `voice://speaker-
/// enrollment-failed` through the same `on_voice_event` dispatcher every
/// other voice callback uses (kinds 11/12).
pub fn enroll_speaker(app: AppHandle) -> Result<(), String> {
    let _ = APP_HANDLE.set(app.clone());
    let lib = engine(&app)?;
    let rc = unsafe {
        let enroll: Symbol<EnrollSpeakerFn> = lib
            .get(b"amin_voice_enroll_speaker\0")
            .map_err(|e| format!("voice engine is missing amin_voice_enroll_speaker: {e}"))?;
        enroll(on_voice_event)
    };
    if rc != 0 {
        return Err(format!("speaker enrollment failed to start (code {rc})"));
    }
    Ok(())
}

/// Whether a voiceprint is currently enrolled. Loads the engine on demand
/// (Settings can call this before any voice feature has run yet) — returns
/// `false`, not an error, if the engine isn't built/loadable, matching
/// `VoicePrintEngine.verify`'s own fail-open stance.
pub fn has_enrolled_speaker(app: AppHandle) -> bool {
    let Ok(lib) = engine(&app) else { return false };
    unsafe {
        match lib.get::<HasEnrolledSpeakerFn>(b"amin_voice_has_enrolled_speaker\0") {
            Ok(f) => f() != 0,
            Err(_) => false,
        }
    }
}

/// Deletes the enrolled voiceprint, if any — hands-free mode goes back to
/// opening on any wake phrase (the pre-voiceprint behavior) until Mona
/// enrolls again.
pub fn clear_enrolled_speaker(app: AppHandle) -> Result<(), String> {
    let lib = engine(&app)?;
    unsafe {
        if let Ok(clear) = lib.get::<ClearEnrolledSpeakerFn>(b"amin_voice_clear_enrolled_speaker\0") {
            clear();
        }
    }
    Ok(())
}
