//! Shared filesystem helpers for writing credential-bearing files.
//!
//! Config files, MCP client configs, and (later) `.env` / cursor files all hold
//! API tokens, so they must be written atomically (no torn writes that could
//! leave a half-written or world-readable file) and with private `0600`
//! permissions on Unix. These helpers centralize that policy so every caller
//! gets the same guarantees.

use anyhow::{Context, Result};
use std::{fs, io::Write, path::Path};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// Atomically writes `contents` to `path` with private (`0600`) permissions on
/// Unix.
///
/// The bytes are first written to a sibling temporary file (created with the
/// restricted mode so the token is never momentarily world-readable), `fsync`ed
/// to durable storage, and then `rename`d over `path`. The rename is atomic on
/// POSIX filesystems, so concurrent readers see either the old file or the new
/// one, never a partial write. If the rename fails the temporary file is removed
/// so a token-bearing temp file is never left behind.
///
/// `path` must have a parent directory and a UTF-8 file name. The parent
/// directory is expected to already exist; callers that need to create it should
/// use [`restrict_directory_permissions`] after `create_dir_all`.
pub fn write_atomic_0600(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().context("path must have a parent directory")?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("path must have a valid UTF-8 file name")?;
    let temp_path = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));

    write_private_temp_file(&temp_path, contents)?;
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error).with_context(|| {
            format!(
                "failed to atomically replace {} with {}",
                path.display(),
                temp_path.display()
            )
        });
    }

    Ok(())
}

#[cfg(unix)]
fn write_private_temp_file(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("failed to open temporary file {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("failed to write temporary file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync temporary file {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_temp_file(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open temporary file {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("failed to write temporary file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync temporary file {}", path.display()))?;
    Ok(())
}

/// Restricts `path` (a directory) to owner-only (`0700`) permissions on Unix.
///
/// On non-Unix platforms this is a no-op. This is the companion to
/// [`write_atomic_0600`]: callers create the parent directory with
/// `fs::create_dir_all` and then tighten its permissions so the private files
/// inside cannot be reached by other users.
#[cfg(unix)]
pub fn restrict_directory_permissions(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to set directory permissions on {}", path.display()))
}

#[cfg(not(unix))]
pub fn restrict_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

/// Creates `path`'s parent directory (if any) and restricts it to owner-only
/// (`0700`) permissions on Unix, best-effort.
///
/// Unlike [`restrict_directory_permissions`], a failure to tighten the
/// permissions is ignored (the directory may be pre-existing and shared, e.g.
/// `~/.cursor`); only the `create_dir_all` failure is surfaced. This mirrors the
/// behavior the MCP installer relies on.
pub fn ensure_parent_private(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        #[cfg(unix)]
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn writes_contents_atomically() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("secret.toml");

        write_atomic_0600(&path, b"token = \"abc\"").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"token = \"abc\"");
    }

    #[test]
    fn overwrites_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("secret.toml");

        write_atomic_0600(&path, b"old").unwrap();
        write_atomic_0600(&path, b"new").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn uses_private_file_permissions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("secret.toml");

        write_atomic_0600(&path, b"token").unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn failed_rename_cleans_up_temp_file() {
        let dir = tempdir().unwrap();
        // A directory at `path` makes the rename-over-target fail.
        let path = dir.path().join("target");
        fs::create_dir(&path).unwrap();
        let temp_path = dir
            .path()
            .join(format!(".target.tmp.{}", std::process::id()));

        assert!(write_atomic_0600(&path, b"token").is_err());
        assert!(
            !temp_path.exists(),
            "failed rename should not leave a token-bearing temporary file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn restrict_directory_permissions_sets_0700() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("nested");
        fs::create_dir(&nested).unwrap();

        restrict_directory_permissions(&nested).unwrap();

        let mode = fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_parent_private_creates_and_restricts() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("file.json");

        ensure_parent_private(&path).unwrap();

        let parent = path.parent().unwrap();
        assert!(parent.is_dir());
        let mode = fs::metadata(parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}
