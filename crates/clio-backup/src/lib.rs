// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the crate root: the backup archive container,
//! manifest, member-name gate, snapshot inventory, and prune public surface.
//!
//! ## Owns
//! - Module declarations and public re-exports for the backup format
//! - The `MANIFEST_MEMBER` / `KEY_MEMBER` names and the snapshot file extension
//! - The re-export of prune selection and deletion, which the CLI renders
//!
//! ## Does not own
//! - Database snapshotting or restore (see `clio-store`)
//! - Backup-directory path resolution (see `clio-config` `backup_path`)
//! - Member staging and the atomic publish (see `staging`, `atomic_publish`)
//! - CLI rendering or exit codes (see `clio`)
//!
//! ## Boundary
//!
//! Changes to the public archive API should normally require changes here.
//! Changes to container bytes or manifest fields should not require changes here.
//!
//! Keep this module focused on the backup-format public surface.
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::cast_possible_truncation)]
#![forbid(unsafe_code)]

pub mod archive_name;
pub mod atomic_publish;
pub mod container;
pub mod create;
pub mod dump_meta;
pub mod extract;
pub mod head_sink;
pub mod inventory;
mod key_member;
pub mod manifest;
pub mod name;
pub mod prune;
pub mod prune_delete;
pub mod prune_kind;
pub mod staging;
pub mod staging_journal;
pub mod verify;
mod verify_members;

mod tar_header;

/// The create/verify/inventory test fixtures, shared by their test modules.
#[cfg(test)]
#[path = "backup_fixtures_tests.rs"]
pub(crate) mod fixtures;

/// Archive restore-readiness: the members an archive carries come back out
/// byte-exact and a damaged key member is caught.

/// Hostile archives and hostile manifest fields, as `list` and `verify` meet
/// them: member order abuse, repeated member names, and an over-cap manifest.

/// The hand-built Postgres archive fixtures the archive suites share.
#[cfg(test)]
#[path = "postgres_archive_fixtures.rs"]
mod postgres_archive_fixtures;

/// Postgres archives: a hand-built dump is assembled, extracted, and verified
/// member by member.

/// The duplicate-member-name hostile cases: the reader's once-only rule from
/// the outside, and the writer's own rule.

pub use archive_name::{archive_file_name, snapshot_id, snapshot_id_of};
pub use atomic_publish::AtomicFile;
pub use container::{
    ArchiveReader, ArchiveWriter, MAX_MANIFEST_BYTES, MemberDigest, MemberHeader, ReaderBounds,
    declared_size_suspect,
};
pub use create::{CreateReport, CreateRequest, KeySource, StagedMember, create_archive};
pub use dump_meta::{
    DUMP_FORMAT_VERSION, DUMP_KIND, DUMP_META_MEMBER, DUMP_TABLES_PREFIX, DumpColumn, DumpMeta,
    DumpTable, MAX_DUMP_META_BYTES,
};
pub use extract::{ExtractedArchive, ExtractedMember, extract_members};
pub use inventory::{
    SnapshotEntry, SnapshotRow, latest_snapshot_id, list_snapshot_rows, list_snapshots,
    read_manifest,
};
pub use manifest::{
    BackupManifest, COMPRESSION, CountsInfo, FORMAT_VERSION, KIND, KeysInfo, PayloadInfo,
    SnapshotInfo, StoreInfo,
};
pub use name::{MAX_NAME_BYTES, validate_member_name};
pub use prune::{
    ArchiveRef, KeepReason, KeptArchive, PrunePlan, PruneRefusal, UnreadableArchive,
    refusal_message, select,
};
pub use prune_delete::{CandidateOutcome, CandidateState, PruneReport, assess, delete};
pub use prune_kind::LinkedPaths;
pub use staging::StagedFile;
pub use staging::{stage_bytes, stage_copy};
pub use staging_journal::{JournalSweepReport, sweep_staging_journals};
pub use verify::{MemberCheck, VerifyReport, verify_archive};

/// The manifest's member name; always the first archive member.
pub const MANIFEST_MEMBER: &str = "clio-backup-manifest.json";

/// The key-store member name; a SQLite key-store database copied as it stands.
///
/// What the archive carries is a key store whose per-subject keys are wrapped
/// under the Clio encryption key. The Clio key itself is never in the file, so
/// this member is ciphertext and does not by itself open anything.
///
/// This phase only names the member; the snapshot writer produces it.
pub const KEY_MEMBER: &str = "keys.kms.db";

/// Suffix of a snapshot archive file in the backup directory.
///
/// A snapshot file is named `<store-slug>-<UTC timestamp>-<8 hex>.cliobak.tar.gz`.
pub const SNAPSHOT_EXTENSION: &str = ".cliobak.tar.gz";
