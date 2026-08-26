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
    pub name: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

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

pub fn list<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<WorkspaceEntry>, String> {
    let root = workspace_root(app)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        entries.push(WorkspaceEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
            size_bytes: metadata.len(),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

pub fn read<R: Runtime>(app: &AppHandle<R>, relative: &str) -> Result<String, String> {
    let root = workspace_root(app)?;
    let path = resolve_within_workspace(&root, relative)?;
    fs::read_to_string(&path).map_err(|e| format!("couldn't read '{relative}': {e}"))
}

pub fn write<R: Runtime>(app: &AppHandle<R>, relative: &str, contents: &str) -> Result<(), String> {
    let root = workspace_root(app)?;
    // The target may not exist yet, so resolve containment against its
    // parent directory (which must already be inside the workspace) and
    // reattach the file name, rather than requiring the file itself to
    // pre-exist before it can be written.
    let parent_relative = Path::new(relative).parent().unwrap_or(Path::new(""));
    let file_name = Path::new(relative)
        .file_name()
        .ok_or_else(|| format!("'{relative}' isn't a valid file name"))?;
    let parent = if parent_relative.as_os_str().is_empty() {
        root.canonicalize().map_err(|e| e.to_string())?
    } else {
        resolve_within_workspace(&root, &parent_relative.to_string_lossy())?
    };
    fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let path = parent.join(file_name);
    fs::write(&path, contents).map_err(|e| format!("couldn't write '{relative}': {e}"))
}

pub fn delete<R: Runtime>(app: &AppHandle<R>, relative: &str) -> Result<(), String> {
    let root = workspace_root(app)?;
    let path = resolve_within_workspace(&root, relative)?;
    fs::remove_file(&path).map_err(|e| format!("couldn't delete '{relative}': {e}"))
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
