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
/// speech started, 4 = speech finished (speak side); 5 = hands-free armed
/// (passively watching for the wake phrase), 6 = wake phrase heard — a
/// command session opened, 7 = the close phrase ended the command session.
type VoiceCallback = unsafe extern "C" fn(c_int, *const c_char);
type StartFn = unsafe extern "C" fn(VoiceCallback) -> c_int;
type StopFn = unsafe extern "C" fn();
type SpeakFn = unsafe extern "C" fn(*const c_char, VoiceCallback) -> c_int;
type StopSpeakingFn = unsafe extern "C" fn();
type StartHandsFreeFn = unsafe extern "C" fn(*const c_char, *const c_char, VoiceCallback) -> c_int;
type StopHandsFreeFn = unsafe extern "C" fn();
type SetHandsFreeMutedFn = unsafe extern "C" fn(c_int);

/// The loaded voice engine, once found — loaded at most once per run, then
/// reused for every push-to-talk session.
static LIBRARY: OnceLock<Library> = OnceLock::new();
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
    Ok(LIBRARY.get().expect("just set above"))
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
    // Amin's own voice must never be transcribed back at itself while
    // hands-free mode is running — see the SELF-HEARING note in
    // AminVoice.swift. This covers the on-device TTS path; the ElevenLabs
    // path (commands::speak_text) doesn't go through this callback at all
    // (playback happens over `afplay`, not the native engine), so it calls
    // `set_hands_free_muted` directly instead.
    if kind == 3 {
        set_hands_free_muted(true);
    } else if kind == 4 {
        set_hands_free_muted(false);
    }

    let _ = match kind {
        0 => app.emit("voice://partial", text),
        1 => app.emit("voice://final", text),
        3 => app.emit("voice://speaking-started", text),
        4 => app.emit("voice://speaking-finished", text),
        5 => app.emit("voice://hands-free-armed", text),
        6 => app.emit("voice://hands-free-listening", text),
        7 => app.emit("voice://hands-free-closed", text),
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

/// Stops any in-progress speech immediately. A no-op if nothing is
/// speaking or the engine was never loaded.
pub fn stop_speaking() -> Result<(), String> {
    if let Some(lib) = LIBRARY.get() {
        unsafe {
            if let Ok(stop) = lib.get::<StopSpeakingFn>(b"amin_voice_stop_speaking\0") {
                stop();
            }
        }
    }
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

/// Mutes/unmutes hands-free mode's own recognition results without
/// stopping anything — called around every `speak_text` (both the
/// on-device path, via `on_voice_event`'s kind 3/4, and the ElevenLabs
/// path directly) so Amin's own voice is never transcribed back at
/// itself. A silent no-op if hands-free mode isn't running or the engine
/// was never loaded — this is a best-effort safety measure, not something
/// that should ever fail loudly and interrupt actually speaking the reply.
pub fn set_hands_free_muted(muted: bool) {
    if let Some(lib) = LIBRARY.get() {
        unsafe {
            if let Ok(f) = lib.get::<SetHandsFreeMutedFn>(b"amin_voice_set_hands_free_muted\0") {
                f(if muted { 1 } else { 0 });
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
