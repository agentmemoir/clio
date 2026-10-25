// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for materializing the embedded compose
//! material (base file, arch overlay, SQL init scripts, `models/` README)
//! into the compose root and verifying every bind-mount source exists.
//!
//! ## Owns
//! - The embedded asset set (build-time `include_str!` of the checked-in files)
//! - Write-time overwrite/backup guard for user-edited materialized files,
//!   with backup failures surfaced as errors (never silent overwrites)
//! - The bind-mount source existence check with a clear missing-source error
//!
//! ## Does not own
//! - Compose-root resolution and clobber protection (see `compose_cli`)
//! - `.env` generation (see `compose_env`)
//! - The Docker invocation itself (see `compose_docker`)
//!
//! ## Boundary
//!
//! Changes to the materialized file set or the backup policy should normally
//! require changes here. Changes to Docker arguments should not require
//! changes here.
//!
//! Keep this module focused on embedded-asset materialization.

use crate::compose_docker::ComposeTarget;
use std::path::Path;

/// The base compose file, embedded at build time from the checked-in repo root.
pub(crate) const BASE_COMPOSE: &str = include_str!("../../../compose.yml");
/// Arch overlay for macOS arm64.
pub(crate) const OVERLAY_ARM64: &str = include_str!("../../../compose.arm64.yml");
/// Arch overlay for Linux `x86_64`.
pub(crate) const OVERLAY_AMD64: &str = include_str!("../../../compose.amd64.yml");
/// Postgres bind-mount init script (schema).
pub(crate) const SQL_CORE: &str = include_str!("../../../sql/001_core.sql");
/// Postgres bind-mount init script (vectors).
pub(crate) const SQL_VECTORS: &str = include_str!("../../../sql/002_vectors_postgres.sql");
/// Postgres bind-mount init script (local compliance slots).
pub(crate) const SQL_SLOTS: &str = include_str!("../../../sql/003_local_compliance_slots.sql");

/// Guidance materialized into `models/`. The GGUF itself is large and MUST
/// NOT be embedded. `clio setup` fetches it when a local extract service is
/// selected and the file is missing; the compose path needs it already present.
pub(crate) const MODELS_README: &str = "\
# models/ — extract service model directory

The `extract` service (NuExtract-1.5-tiny via llama.cpp server) expects its GGUF model file in this directory. The file is large and is not shipped with the compose material.

`clio setup` fetches it: when extraction is local and the file is missing, setup downloads the model from the Hugging Face Hub before it writes the config, so setup needs no manual step.

`clio compose up` and `make compose up` with the `local-rft` profile do not download it. Both refuse to start the extract service while the file is absent, and print the command that fetches it:

    hf download DevQuasar/numind.NuExtract-tiny-v1.5-GGUF \\
      numind.NuExtract-tiny-v1.5.Q4_K_M.gguf --local-dir ./models

The expected file name is `numind.NuExtract-tiny-v1.5.Q4_K_M.gguf` (configurable via CLIO_EXTRACT_GGUF in .env).
";

struct Asset {
    path: &'static str,
    contents: &'static str,
}

/// The full materialized asset set for one target: base compose, arch overlay,
/// the Postgres init scripts it bind-mounts, and the `models/` README.
fn assets(target: ComposeTarget) -> Vec<Asset> {
    vec![
        Asset {
            path: "compose.yml",
            contents: BASE_COMPOSE,
        },
        Asset {
            path: target.overlay(),
            contents: match target {
                ComposeTarget::MacArm64 => OVERLAY_ARM64,
                ComposeTarget::LinuxAmd64 => OVERLAY_AMD64,
            },
        },
        Asset {
            path: "sql/001_core.sql",
            contents: SQL_CORE,
        },
        Asset {
            path: "sql/002_vectors_postgres.sql",
            contents: SQL_VECTORS,
        },
        Asset {
            path: "sql/003_local_compliance_slots.sql",
            contents: SQL_SLOTS,
        },
        Asset {
            path: "models/README.md",
            contents: MODELS_README,
        },
    ]
}

/// Every relative bind-mount source the embedded compose file references
/// whatever the selection: the Postgres init scripts the managed database reads
/// on first start. The `models/` mount belongs to `extract` alone, so it is
/// required only when that service is selected, and it is checked through the
/// `extract` service's own model file rather than as a directory (see
/// `compose_model`).
pub(crate) const BIND_MOUNT_SOURCES: &[&str] = &[
    "sql/001_core.sql",
    "sql/002_vectors_postgres.sql",
    "sql/003_local_compliance_slots.sql",
];

/// Materialize the asset set into `root`, refusing to silently overwrite a
/// user-edited file: when an existing file differs from the embedded content
/// it is first copied to `<name>.bak-<unix-timestamp>` and a warning is
/// printed. Returns the materialized relative paths.
pub(crate) fn materialize(root: &Path, target: ComposeTarget) -> Result<Vec<String>, String> {
    let mut written = Vec::new();
    for asset in assets(target) {
        let path = root.join(asset.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        if let Some(backup) = backup_edited(&path, asset.contents)? {
            eprintln!(
                "warning: {} was modified by hand; saved the previous version to {}",
                asset.path,
                backup.display()
            );
        }
        std::fs::write(&path, asset.contents)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written.push(asset.path.to_string());
    }
    Ok(written)
}

/// If `path` exists with different contents, copy it to a timestamped backup
/// and return the backup path. Unchanged or unreadable files are not touched
/// (a file that cannot be read cannot be backed up). A failed backup copy is
/// an error the caller must surface, never a silent overwrite of user edits.
fn backup_edited(path: &Path, expected: &str) -> Result<Option<std::path::PathBuf>, String> {
    let Ok(existing) = std::fs::read(path) else {
        return Ok(None);
    };
    if existing.as_slice() == expected.as_bytes() {
        return Ok(None);
    }
    let backup = path.with_extension(format!(
        "{}.bak-{}",
        path.extension().unwrap_or_default().to_string_lossy(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |secs| secs.as_secs())
    ));
    std::fs::copy(path, &backup).map_err(|e| format!("cannot back up {}: {e}", path.display()))?;
    Ok(Some(backup))
}

/// Verify every relative bind-mount source the embedded compose file
/// references exists in the compose root before Docker runs, naming the first
/// missing source in the error. This is what prevents Docker from creating
/// empty directories and Postgres coming up without a schema.
pub(crate) fn verify_bind_mounts(root: &Path) -> Result<(), String> {
    for source in BIND_MOUNT_SOURCES {
        if !root.join(source).exists() {
            return Err(format!(
                "missing bind-mount source {source} in {}; `clio compose up` materializes it — \
                 do not delete it before starting Docker",
                root.display()
            ));
        }
    }
    Ok(())
}

/// The GGUF file name the extract service expects by default (matches
/// `compose.yml` and the materialized `models/` README). The name the running
/// stack actually mounts comes from Compose; see `compose_model`.
pub(crate) const GGUF_FILE_NAME: &str = "numind.NuExtract-tiny-v1.5.Q4_K_M.gguf";

#[cfg(test)]
#[path = "compose_assets_tests.rs"]
mod tests;
