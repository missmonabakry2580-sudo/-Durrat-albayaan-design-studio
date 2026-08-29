//! All secrets (API keys, tokens) were meant to live in the OS Keychain,
//! never on disk and never in Git — that principle still holds for
//! anything added later. This specific module is temporarily unused:
//! `commands.rs`'s `save_api_key`/`has_api_key` moved the one secret Amin
//! currently has (the Anthropic key) to the local settings table after a
//! reproducible failure on a real Mac — `set_secret` reported success every
//! time, yet `get_secret` reliably came back `keyring::Error::NoEntry` ("No
//! matching entry found in secure storage") moments later in the same
//! running session. Checked the `keyring` crate's own macOS backend source
//! to rule out an ambiguous-duplicate-item explanation; found nothing else
//! to try without a second real Mac to reproduce on. Mona chose, knowingly,
//! to accept local-disk storage for now rather than stay blocked — see
//! docs/SECURITY.md. Kept here rather than deleted: this is still the right
//! approach for any future secret once the Keychain issue is understood.
#![allow(dead_code)]

use keyring::Entry;

const SERVICE: &str = "com.monaalsayedstudio.amin";

pub fn set_secret(key_name: &str, value: &str) -> Result<(), String> {
    Entry::new(SERVICE, key_name)
        .map_err(|e| e.to_string())?
        .set_password(value)
        .map_err(|e| e.to_string())
}

pub fn has_secret(key_name: &str) -> bool {
    Entry::new(SERVICE, key_name)
        .and_then(|e| e.get_password())
        .is_ok()
}

/// Read back for outbound calls — e.g. `agent::send_message` reading the
/// Anthropic key.
pub fn get_secret(key_name: &str) -> Result<String, String> {
    Entry::new(SERVICE, key_name)
        .map_err(|e| e.to_string())?
        .get_password()
        .map_err(|e| e.to_string())
}

pub fn clear_secret(key_name: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE, key_name).map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
