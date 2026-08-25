use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

/// The one push-to-talk session, if a recording is currently in progress.
/// Plain `std::process` + `std::thread`, deliberately not async — this runs
/// from a global-shortcut key-event handler, whose execution context isn't
/// guaranteed to have a Tokio runtime entered, so it must not depend on one.
pub struct VoiceSession(pub Mutex<Option<Child>>);

impl VoiceSession {
    pub fn new() -> Self {
        VoiceSession(Mutex::new(None))
    }
}

impl Default for VoiceSession {
    fn default() -> Self {
        Self::new()
    }
}

/// One line of the native transcriber helper's stdout protocol. See
/// macos/transcriber/main.swift for the producer side of this contract.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TranscriberMessage {
    Partial { text: String },
    Final { text: String },
    Error { message: String },
    #[serde(other)]
    Other,
}

/// Where the compiled native helper is expected to live, bundled as a
/// Tauri resource. **Not shipped yet** — see docs/ARCHITECTURE.md's "Voice
/// pipeline" section for the build step that produces it. Until then,
/// `start_listening` fails with a clear "not built yet" error rather than
/// a confusing file-not-found one.
fn helper_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve("amin-transcriber", tauri::path::BaseDirectory::Resource)
        .map_err(|e| e.to_string())
}

/// Starts push-to-talk listening: spawns the native transcriber helper and
/// forwards its partial/final transcript lines to the frontend as
/// `voice://partial`, `voice://final`, `voice://error` events. A second
/// call while already listening is a no-op, not an error (a key that
/// auto-repeats while held shouldn't spawn a second helper).
pub fn start_listening(app: AppHandle, session: tauri::State<'_, VoiceSession>) -> Result<(), String> {
    {
        let guard = session.0.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Ok(());
        }
    }

    let path = helper_path(&app)?;
    if !path.exists() {
        return Err(format!(
            "the voice transcriber isn't built yet (expected at {}) — see docs/ARCHITECTURE.md \"Voice pipeline\"",
            path.display()
        ));
    }

    let mut child = Command::new(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start the voice transcriber: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .expect("stdout was requested as piped above");

    let app_for_thread = app.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let message = match serde_json::from_str::<TranscriberMessage>(&line) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let _ = match message {
                TranscriberMessage::Partial { text } => app_for_thread.emit("voice://partial", text),
                TranscriberMessage::Final { text } => app_for_thread.emit("voice://final", text),
                TranscriberMessage::Error { message } => app_for_thread.emit("voice://error", message),
                TranscriberMessage::Other => Ok(()),
            };
        }
    });

    let mut guard = session.0.lock().map_err(|e| e.to_string())?;
    *guard = Some(child);
    Ok(())
}

/// Stops push-to-talk listening: sends the helper its stop signal over
/// stdin and waits for it to exit (it's expected to flush a final
/// transcript line first). A no-op if nothing is listening.
pub fn stop_listening(session: tauri::State<'_, VoiceSession>) -> Result<(), String> {
    let mut child = {
        let mut guard = session.0.lock().map_err(|e| e.to_string())?;
        match guard.take() {
            Some(child) => child,
            None => return Ok(()),
        }
    };

    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(b"stop\n");
    }
    let _ = child.wait();
    Ok(())
}
