// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for durable deployment-overlay persistence: it
//! coordinates the submodules that resolve the file path, load and open sealed
//! values, save and seal values, back up a replaced file, and write owner-only.
//!
//! ## Owns
//! - The persistence surface other crates import
//! - The test modules for deployment-overlay persistence
//!
//! ## Does not own
//! - Path resolution (see `path`)
//! - The load-side seal gate (see `load`)
//! - The save-side sealing (see `save`)
//! - Backup naming and layout (see `backup`)
//! - Owner-only file and directory writes (see `write`)
//!
//! ## Boundary
//!
//! Adding a persistence entry point should normally require changes here.
//! Changing how an existing step works belongs in its own submodule.
//!
//! Keep this module focused on the persistence surface and its tests.
//!
//! # Sealing
//!
//! `save` is the one place the file is serialized, so it is the one place a
//! secret becomes unreadable on the way out, and `load` is the one place it
//! becomes readable again. Every write path goes through them — `config_set`
//! at `scope=deployment`, `flush_deployment_overlay`, and `clio setup` — so
//! none of them can accidentally write a secret in the clear.
//!
//! The overlay in memory always holds plaintext, so masking, validation, and
//! every existing consumer behave exactly as they did before sealing existed.

mod backup;
mod load;
mod path;
mod save;
mod write;

pub use backup::backup_config_file;
pub(crate) use load::ensure_overwritable;
pub use load::load_overlay_file;
pub(crate) use path::resolve_deployment_path_with;
pub use path::{
    DEPLOYMENT_CONFIG_ENV, DEPLOYMENT_CONFIG_FILE_NAME, default_config_dir, resolve_deployment_path,
};
pub use save::save_overlay_file;
pub use write::{restrict_dir, write_private_atomic};

// These names were in scope here before the split; the test modules below
// reach them through `use super::*`.
#[cfg(test)]
pub(crate) use crate::config::merge::Overlay;
#[cfg(test)]
pub(crate) use crate::error::ErrorCode;
#[cfg(test)]
pub(crate) use backup::BACKUP_DIR_NAME;
#[cfg(test)]
pub(crate) use std::fs;
#[cfg(test)]
pub(crate) use std::path::{Path, PathBuf};
#[cfg(test)]
pub(crate) use write::temp_sibling;

#[cfg(test)]
#[path = "persist_seal_gate_tests.rs"]
mod seal_gate_tests;
#[cfg(test)]
#[path = "persist_seal_tests.rs"]
mod seal_tests;
#[cfg(test)]
#[path = "persist_seal_write_tests.rs"]
mod seal_write_tests;
#[cfg(test)]
#[path = "persist_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "persist_db_agreement_tests.rs"]
mod persist_db_agreement_tests;
