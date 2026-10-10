// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the configuration surface every other crate
//! reads: the config file and its profiles, the deployment overlay, the resolved
//! database and backup locations, the ranking and retention knobs, the process
//! log helpers, and the custody of the two keys — the encryption key that seals
//! the config and the sync key that seals relay traffic.
//!
//! ## Owns
//! - Loading, validating, and merging the config file, the profiles, and the
//!   deployment overlay into one `Runtime`
//! - Resolving where a machine keeps its database, its backups, and its
//!   retention store
//! - The encryption key and the sync key: generating each, reading it back,
//!   deleting it, and recording which store holds it
//! - The sealed-value format, so a caller can tell a sealed value from a plain
//!   one without opening it
//!
//! ## Does not own
//! - The database itself and every query over it (see `clio-store`)
//! - The tool that exposes this configuration to an agent (see `clio-mcp`)
//! - The command-line surface that writes a config (see `clio`)
//!
//! ## Boundary
//!
//! Changes to what a machine's configuration resolves to, or to how either key
//! is created, stored, or reported, should normally require changes here.
//! Changes to what is stored, or to how a caller reaches this crate, should
//! not.
//!
//! Keep this module focused on configuration and key custody.

#![forbid(unsafe_code)]

pub mod admission_knobs;
pub mod auth_tokens;
pub mod auth_tokens_lock;
pub mod auth_tokens_validation;
pub mod backup_path;
pub mod coding_default;
pub mod config;
pub mod db_effective;
pub mod db_path;
pub mod db_url_env;
pub mod history_knobs;
pub mod hygiene_knobs;
pub mod kms_path;
pub mod log;
pub mod mission_match;
pub mod profile;
pub mod ranking;
pub mod retention;
pub mod retention_levels;
pub mod retention_mission;
pub mod retention_resolved;
pub mod retention_schema;
pub mod retention_store;
pub mod secret;
pub mod secret_key;
pub mod secret_key_file;
pub mod secret_key_source;
pub mod secret_key_store;
pub mod secret_seal;
pub mod sync_key;
pub mod sync_key_file;
pub mod sync_key_rotation;
pub mod sync_key_store;
pub mod sync_key_value;

/// The shared keystore test credential and factory wrappers.
#[cfg(test)]
#[path = "secret_key_store_test_support.rs"]
pub(crate) mod secret_key_store_test_support;

/// The shared temp directory and injected environment the sync key tests use.
#[cfg(test)]
#[path = "sync_key_test_support.rs"]
pub(crate) mod sync_key_test_support;

mod weight_groups;


// Re-export clio-types modules so existing `crate::error` / `crate::context` paths
// inside this crate keep working without a bulk rewrite of every submodule.
pub use clio_types::context;
pub use clio_types::error;

pub use admission_knobs::{
    DEFAULT_RECENCY_TAU_DAYS, DEFAULT_THETA_ADMIT, KNOWN_ADMISSION_KEYS, default_type_prior,
    prior_for, threshold_for,
};
pub use backup_path::{
    BACKUP_DIR_ENV, BackupDirTier, ResolvedBackupDir, default_backup_dir, resolve_backup_dir,
    store_slug,
};
pub use config::{
    CONFIG_SCHEMA_VERSION, ConfigScope, DEPLOYMENT_CONFIG_ENV, DEPLOYMENT_CONFIG_FILE_NAME,
    Runtime, backup_config_file, bearer_for_effective, resolve_deployment_path,
};
pub use db_path::{ResolvedDatabase, default_sqlite_path, infer_backend, resolve_database};
pub use history_knobs::HistoryKnobs;
pub use profile::{Profile, ToolImplStatus, ToolInventoryEntry, coding_tool_inventory};
pub use retention::{
    DuplicateTolerance, INERT_FIELDS_HONORED_BY, PackFidelityDefault, ProvenanceFloorDefault,
    RETENTION_PROFILE_VERSION, RecallScope, ResolvedRetention, RetentionProfile, Verbosity,
};
pub use retention_mission::mission_rule::mission_rule_id;
pub use retention_mission::{
    MAX_MISSION_ENTRY_CHARS, MAX_MISSION_EXAMPLES_PER_LIST, MAX_MISSION_STATEMENT_CHARS,
    MAX_MISSION_TOTAL_CHARS, MISSION_DROP_PENALTY, MISSION_KEEP_BONUS, MissionEntry,
    MissionEvaluation, MissionMatchKind, ResolvedMission, ResolvedMissionEntry, RetentionMission,
};
pub use retention_schema::profile_json_schema;
pub use retention_store::{
    ProfileSource, RETENTION_PROFILES_ENV, RetentionStore, resolve_retention_path,
};
pub use secret_key::{
    KEY_FILE_ENV, KEY_FILE_NAME, KEY_FILE_ONLY_ENV, KEY_LEN, KeyLocation, LoadedKey,
    load_or_create, location,
};
pub use secret_key_file::key_file_path;
pub use secret_key_source::KeySource;
pub use secret_seal::{SEALED_PREFIX, is_sealed, is_sealed_path, open, seal};
pub use sync_key::{
    LoadedSyncKey, SYNC_KEY_FILE_ENV, SYNC_KEY_FILE_NAME, SyncKeyLocation, SyncKeyReport, delete,
    delete_with, generate, generate_with, key_file_path as sync_key_file_path, load, load_base64,
    load_with, read_key_file_base64, record_source, recorded_source, report, report_with,
};
