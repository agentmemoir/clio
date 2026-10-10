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

#[cfg(test)]
mod tests {
    use super::*;

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
}
