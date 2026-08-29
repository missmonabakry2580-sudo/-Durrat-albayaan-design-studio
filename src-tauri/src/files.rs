use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

/// Amin's file access root. Originally this was one dedicated folder
/// (`~/Documents/Amin`); Mona explicitly asked to broaden that to "كل
/// الملفات" (all files) on 2026-08-26. Read pragmatically — not as the
/// literal filesystem root, which would expose OS/system files no legitimate
/// task ever needs — this is her whole home directory. Every function here
/// still re-derives this root and re-validates containment against it; there
/// is no code path that accepts an arbitrary absolute path from the caller.
/// See docs/SECURITY.md for why, given this much broader surface,
/// `tools::risk_for` now gates *every* file tool (including plain reads and
/// listing, not just writes/deletes) behind Mona's explicit confirmation —
/// a file's contents leaving her machine in a tool_result, to a third-party
/// API, is itself a "step" her own instruction says must wait for her word.
#[derive(Serialize)]
pub struct WorkspaceEntry {
    /// Relative to the home folder, e.g. "Desktop/old stuff/file.txt" — not
    /// just a bare file name — so a nested entry from a recursive listing
    /// can be passed straight back into read/write/delete/move/mkdir
    /// without Mona or Claude having to reconstruct the path themselves.
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

/// Caps on `list`'s recursion — a real home directory can be enormous, and
/// this return value goes straight into a Claude tool_result (and, once
/// approved, a confirmation card Mona actually has to read). Depth 3 and
/// 500 entries is enough to survey "what's a mess in here" without either
/// blowing up the response or asking Mona to review an unreadable wall of
/// text before she can approve anything.
const MAX_LIST_DEPTH: u32 = 3;
const MAX_LIST_ENTRIES: usize = 500;

fn workspace_root<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .home_dir()
        .map_err(|e| format!("couldn't resolve the home folder: {e}"))
}

/// Resolves `relative` against the workspace root and rejects anything
/// that would escape it (`..`, an absolute path, a symlink pointing
/// outside). This is the one check every read/write/delete goes through —
/// see the tests below for the exact traversal attempts it blocks.
fn resolve_within_workspace(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty() {
        return Err("a file path is required".to_string());
    }

    // The file may not exist yet (e.g. a new write) — canonicalize the root
    // (the home directory, which always exists) and manually resolve the
    // rest, rather than requiring the whole path to already be on disk.
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("couldn't resolve your home folder: {e}"))?;

    let mut resolved = canonical_root.clone();
    for component in Path::new(relative).components() {
        use std::path::Component;
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "'{relative}' isn't inside your home folder"
                ));
            }
        }
    }

    // If the target already exists (e.g. via a symlink), canonicalize it
    // for real and re-check containment — a symlink inside the workspace
    // could otherwise point somewhere else entirely.
    if let Ok(existing) = resolved.canonicalize() {
        if !existing.starts_with(&canonical_root) {
            return Err(format!(
                "'{relative}' isn't inside your home folder"
            ));
        }
        return Ok(existing);
    }

    if !resolved.starts_with(&canonical_root) {
        return Err(format!("'{relative}' isn't inside your home folder"));
    }

    Ok(resolved)
}

/// Hidden/system entries that are real and readable but never a legitimate
/// Amin-workspace file — noise Mona should never have to look at. This is
/// a *display* filter only: it does not touch `resolve_within_workspace`,
/// so none of these become any less reachable if a task genuinely needs
/// one by exact name; it only keeps them out of the plain listing.
const HIDDEN_ENTRY_PREFIXES: &[&str] = &[".", "$RECYCLE.BIN"];
const HIDDEN_ENTRY_NAMES: &[&str] = &["System Volume Information", "lost+found"];

fn is_hidden_from_listing(name: &str) -> bool {
    HIDDEN_ENTRY_PREFIXES.iter().any(|p| name.starts_with(p))
        || HIDDEN_ENTRY_NAMES.contains(&name)
}

/// Lists `relative` (empty string = the home folder itself). With
/// `recursive`, walks up to `MAX_LIST_DEPTH` levels down, stopping early
/// and noting it in `truncated` once `MAX_LIST_ENTRIES` is hit — the caller
/// (tools.rs) surfaces that flag rather than silently returning a partial
/// picture as if it were complete.
pub fn list<R: Runtime>(
    app: &AppHandle<R>,
    relative: &str,
    recursive: bool,
) -> Result<(Vec<WorkspaceEntry>, bool), String> {
    let root = workspace_root(app)?;
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    let start = if relative.is_empty() {
        canonical_root.clone()
    } else {
        resolve_within_workspace(&root, relative)?
    };

    let mut entries = Vec::new();
    let mut truncated = false;
    let mut stack = vec![(start, 0u32)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(read_dir) = fs::read_dir(&dir) else { continue };
        for entry in read_dir {
            if entries.len() >= MAX_LIST_ENTRIES {
                truncated = true;
                break;
            }
            // A single unreadable entry must not take down the whole
            // listing — a real home directory has all sorts of special
            // files (dangling symlinks, permission-restricted items,
            // sockets) that a broad Documents-folder scope never had to
            // deal with. Skip what can't be read rather than erroring the
            // entire call.
            let Ok(entry) = entry else { continue };
            let name = entry.file_name().to_string_lossy().to_string();
            if is_hidden_from_listing(&name) {
                continue;
            }
            let Ok(metadata) = entry.metadata().or_else(|_| entry.path().symlink_metadata()) else {
                continue;
            };
            let path = entry.path();
            let Ok(relative_path) = path.strip_prefix(&canonical_root) else { continue };
            let is_dir = metadata.is_dir();
            entries.push(WorkspaceEntry {
                path: relative_path.to_string_lossy().to_string(),
                is_dir,
                size_bytes: metadata.len(),
            });
            if recursive && is_dir && depth < MAX_LIST_DEPTH {
                stack.push((path, depth + 1));
            }
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((entries, truncated))
}

pub fn read<R: Runtime>(app: &AppHandle<R>, relative: &str) -> Result<String, String> {
    let root = workspace_root(app)?;
    let path = resolve_within_workspace(&root, relative)?;
    fs::read_to_string(&path).map_err(|e| format!("couldn't read '{relative}': {e}"))
}

pub fn write<R: Runtime>(app: &AppHandle<R>, relative: &str, contents: &str) -> Result<(), String> {
    let root = workspace_root(app)?;
    let path = resolve_destination(&root, relative)?;
    fs::write(&path, contents).map_err(|e| format!("couldn't write '{relative}': {e}"))
}

pub fn delete<R: Runtime>(app: &AppHandle<R>, relative: &str) -> Result<(), String> {
    let root = workspace_root(app)?;
    let path = resolve_within_workspace(&root, relative)?;
    if path.is_dir() {
        fs::remove_dir_all(&path).map_err(|e| format!("couldn't delete '{relative}': {e}"))
    } else {
        fs::remove_file(&path).map_err(|e| format!("couldn't delete '{relative}': {e}"))
    }
}

/// Resolves a destination path the same way `write`'s target resolves —
/// the destination may not exist yet, so containment is checked against
/// its parent, not the (not-yet-real) path itself.
fn resolve_destination(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let parent_relative = Path::new(relative).parent().unwrap_or(Path::new(""));
    let file_name = Path::new(relative)
        .file_name()
        .ok_or_else(|| format!("'{relative}' isn't a valid destination path"))?;
    let parent = if parent_relative.as_os_str().is_empty() {
        root.canonicalize().map_err(|e| e.to_string())?
    } else {
        resolve_within_workspace(root, &parent_relative.to_string_lossy())?
    };
    fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    Ok(parent.join(file_name))
}

/// Moves or renames a file/folder — same operation either way, since
/// `fs::rename` handles both, as long as source and destination both stay
/// inside the workspace.
pub fn mv<R: Runtime>(app: &AppHandle<R>, from: &str, to: &str) -> Result<(), String> {
    let root = workspace_root(app)?;
    let source = resolve_within_workspace(&root, from)?;
    let destination = resolve_destination(&root, to)?;
    fs::rename(&source, &destination).map_err(|e| format!("couldn't move '{from}' to '{to}': {e}"))
}

pub fn create_dir<R: Runtime>(app: &AppHandle<R>, relative: &str) -> Result<(), String> {
    let root = workspace_root(app)?;
    if relative.is_empty() {
        return Err("a folder path is required".to_string());
    }
    let mut resolved = root.canonicalize().map_err(|e| e.to_string())?;
    for component in Path::new(relative).components() {
        use std::path::Component;
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("'{relative}' isn't inside your home folder"));
            }
        }
    }
    fs::create_dir_all(&resolved).map_err(|e| format!("couldn't create '{relative}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_within_workspace_blocks_parent_dir_traversal() {
        let tmp = std::env::temp_dir().join(format!("amin-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let err = resolve_within_workspace(&tmp, "../../etc/passwd");
        assert!(err.is_err());
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn resolve_within_workspace_blocks_absolute_paths() {
        let tmp = std::env::temp_dir().join(format!("amin-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let err = resolve_within_workspace(&tmp, "/etc/passwd");
        assert!(err.is_err());
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn resolve_within_workspace_allows_a_plain_relative_path() {
        let tmp = std::env::temp_dir().join(format!("amin-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let resolved = resolve_within_workspace(&tmp, "notes/today.txt").unwrap();
        assert!(resolved.starts_with(tmp.canonicalize().unwrap()));
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn resolve_within_workspace_rejects_an_empty_path() {
        let tmp = std::env::temp_dir().join(format!("amin-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        assert!(resolve_within_workspace(&tmp, "").is_err());
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    #[cfg(unix)]
    fn resolve_within_workspace_blocks_a_symlink_pointing_outside() {
        let base = std::env::temp_dir().join(format!("amin-test-{}", uuid::Uuid::new_v4()));
        let workspace = base.join("workspace");
        let outside = base.join("outside");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "not for Amin").unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join("escape")).unwrap();

        let err = resolve_within_workspace(&workspace, "escape/secret.txt");
        assert!(err.is_err(), "a symlink out of the workspace must be rejected");

        fs::remove_dir_all(&base).ok();
    }
}
