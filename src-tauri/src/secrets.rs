use keyring::Entry;

/// All secrets (API keys, tokens) live in the OS Keychain, never on disk and
/// never in Git. `.env.local` is for local dev convenience only — see
/// docs/SECURITY.md.
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
