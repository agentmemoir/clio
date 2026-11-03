// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! SQLite file-path helpers.

use clio_types::{AmError, ErrorCode};
use std::path::Path;

pub(crate) fn create_parent_dir(parent: &Path) -> Result<(), AmError> {
    std::fs::create_dir_all(parent).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("create sqlite parent dir `{}`: {e}", parent.display()),
        )
    })
}

pub(crate) fn ensure_parent(path: &Path) -> Result<(), AmError> {
    let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return Ok(());
    };
    create_parent_dir(parent)
}

/// Leave the database file owner-only, creating it when it is missing.
///
/// The mode is set in the `open` call that creates the file, so no window
/// exists in which the process umask's default mode is in place, and an
/// existing file that a previous build created more widely readable is
/// tightened too. Only the file is touched: the directory keeps its mode.
///
/// # Errors
/// The file cannot be created or its permissions cannot be changed.
#[cfg(unix)]
pub(crate) fn ensure_owner_only(path: &Path) -> Result<(), AmError> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|e| {
            AmError::new(
                ErrorCode::Internal,
                format!("create sqlite file `{}` owner-only: {e}", path.display()),
            )
        })?;
    crate::local_kms_migrate::tighten_permissions(path)
}

/// Permissions are already process-scoped on non-Unix hosts.
///
/// # Errors
/// Never.
#[cfg(not(unix))]
pub(crate) fn ensure_owner_only(_path: &Path) -> Result<(), AmError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn ensure_parent_noop_for_bare_filename() {
        assert!(ensure_parent(Path::new("bare.db")).is_ok());
    }

    #[test]
    fn create_parent_dir_errors_when_path_is_a_file() {
        let base = std::env::temp_dir().join(format!("am-file-{}", std::process::id()));
        std::fs::write(&base, b"x").unwrap();
        let err = create_parent_dir(&base).unwrap_err();
        assert!(err.message.contains(&base.display().to_string()));
        let _ = std::fs::remove_file(&base);
    }

    /// The file's permission bits, on a host that has them.
    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("stat the database file")
            .permissions()
            .mode()
            & 0o777
    }

    /// A scratch directory removed when the guard drops.
    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(tag: &str) -> Scratch {
        Scratch(crate::test_unique::scratch_dir(&format!(
            "clio-store-owner-only-{tag}"
        )))
    }

    #[cfg(unix)]
    #[test]
    fn ensure_owner_only_creates_a_missing_file_at_0600() {
        let dir = scratch("missing");
        let path = dir.0.join("clio.db");
        ensure_owner_only(&path).expect("leave a missing file owner-only");
        assert!(path.exists(), "the missing file must be created");
        assert_eq!(mode_of(&path), 0o600, "a created file must be owner-only");
    }

    #[cfg(unix)]
    #[test]
    fn ensure_owner_only_tightens_an_existing_world_readable_file() {
        let dir = scratch("tighten");
        let path = dir.0.join("clio.db");
        std::fs::write(&path, b"").expect("write the database file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("make the file world-readable");
        ensure_owner_only(&path).expect("tighten the existing file");
        assert_eq!(mode_of(&path), 0o600, "an existing file must be tightened");
    }
}
