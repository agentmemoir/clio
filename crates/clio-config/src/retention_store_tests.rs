// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing bank-scoped retention profile
//! storage: resolution order, set validation, save and load round trips, lossy
//! pruning, and the retention path selection.
//!
//! ## Owns
//! - The shared `tmp()` scratch-path helper used by the sibling retention store
//!   test modules
//! - The bank / deployment / builtin / coding-default resolution order tests
//! - Set validation and save/load error tests
//! - The lossy load pruning tests and the explicit-or-env path selection tests
//!
//! ## Does not own
//! - The store implementation itself (see `retention_store`)
//! - Stored marker conversion (see `retention_store_version_tests`)
//! - The stored provenance floor (see `retention_store_floor_tests`)
//!
//! ## Boundary
//!
//! A change to how the store saves, loads, or resolves profiles should
//! normally require changes here. A change to profile validation rules should
//! not, unless the store's handling of an invalid profile changed.
//!
//! Keep this module focused on storage and resolution behavior.

use super::*;
use crate::retention::{
    DuplicateTolerance, PackFidelityDefault, ProvenanceFloorDefault, RETENTION_PROFILE_VERSION,
    Verbosity,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// A scratch path that is unique per call and removed when the guard drops.
///
/// Self-cleaning so a panicked test leaves nothing behind. Sibling retention
/// store test modules build paths from it with `Deref`/`AsRef<Path>` and never
/// name the guard type.
pub(super) fn tmp(name: &str) -> TempPath {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    TempPath(std::env::temp_dir().join(format!("am-retention-{name}-{n}")))
}

pub(super) struct TempPath(PathBuf);

impl std::ops::Deref for TempPath {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<std::path::Path> for TempPath {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        if self.0.is_dir() {
            let _ = fs::remove_dir_all(&self.0);
        } else if self.0.is_file() {
            let _ = fs::remove_file(&self.0);
        }
        // A nested scratch path such as `nested/store.json` leaves parent
        // directories behind; remove them while they are ours and empty.
        let mut parent = self.0.parent();
        while let Some(dir) = parent {
            let ours = dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("am-retention-"));
            if !ours || fs::remove_dir(dir).is_err() {
                break;
            }
            parent = dir.parent();
        }
    }
}

fn selective() -> RetentionProfile {
    RetentionProfile {
        verbosity: Verbosity::Selective,
        duplicate_tolerance_write: DuplicateTolerance::Strict,
        ..RetentionProfile::default()
    }
}

#[test]
fn resolution_order_is_bank_then_deployment_then_builtin() {
    let mut store = RetentionStore::default();
    let (profile, source) = store.effective("bank-a");
    assert_eq!(profile, RetentionProfile::default());
    assert_eq!(source, ProfileSource::Builtin);
    assert_eq!(source.as_str(), "builtin");
    assert!(store.bank_override("bank-a").is_none());

    store.set_deployment(selective()).expect("deployment");
    let (profile, source) = store.effective("bank-a");
    assert_eq!(profile, selective());
    assert_eq!(source, ProfileSource::Deployment);
    assert_eq!(source.as_str(), "deployment");

    store
        .set_bank("bank-a", RetentionProfile::default())
        .expect("bank");
    let (profile, source) = store.effective("bank-a");
    assert_eq!(profile, RetentionProfile::default());
    assert_eq!(source, ProfileSource::Bank);
    assert_eq!(source.as_str(), "bank");
    // Bank B still sees the deployment default, never bank A's override.
    assert_eq!(store.effective("bank-b").1, ProfileSource::Deployment);
    assert_eq!(
        store.bank_override("bank-a"),
        Some(&RetentionProfile::default())
    );

    assert!(store.clear_bank("bank-a"));
    assert!(!store.clear_bank("bank-a"));
    assert_eq!(store.effective("bank-a").1, ProfileSource::Deployment);
}

#[test]
fn set_rejects_empty_bank_and_invalid_profile() {
    let mut store = RetentionStore::default();
    assert_eq!(
        store
            .set_bank("  ", RetentionProfile::default())
            .expect_err("empty bank")
            .code,
        ErrorCode::MissingBank
    );
    let mut bad = RetentionProfile::default();
    bad.theta_offsets.insert("nope".into(), 0.1);
    assert_eq!(
        store.set_bank("bank-a", bad.clone()).expect_err("bad").code,
        ErrorCode::UnknownPath
    );
    assert_eq!(
        store.set_deployment(bad).expect_err("bad deploy").code,
        ErrorCode::UnknownPath
    );
    assert!(store.banks.is_empty());
    assert!(store.deployment.is_none());
}

#[test]
fn set_deployment_rejects_permissive_but_banks_may_use_it() {
    let mut store = RetentionStore::default();
    let permissive = RetentionProfile {
        verbosity: Verbosity::Permissive,
        ..RetentionProfile::default()
    };
    assert_eq!(
        store
            .set_deployment(permissive.clone())
            .expect_err("permissive default")
            .code,
        ErrorCode::InvalidArgument
    );
    assert!(store.deployment.is_none());
    // The same level stays legal for a single bank.
    store
        .set_bank("bank-a", permissive)
        .expect("bank scope is allowed");
    assert_eq!(store.effective("bank-a").1, ProfileSource::Bank);
}

#[test]
fn load_missing_is_empty_and_corrupt_errors() {
    let missing = tmp("missing.json");
    assert_eq!(
        RetentionStore::load(&missing).expect("missing"),
        RetentionStore::default()
    );

    let bad = tmp("bad.json");
    fs::write(&bad, "not-json").expect("write");
    assert_eq!(
        RetentionStore::load(&bad).expect_err("parse").code,
        ErrorCode::ConfigCorrupt
    );
    // A directory cannot be read as a file.
    let dir = tmp("dir");
    fs::create_dir_all(&dir).expect("dir");
    assert_eq!(
        RetentionStore::load(&dir).expect_err("read").code,
        ErrorCode::ConfigCorrupt
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn save_round_trips_and_reports_io_errors() {
    let path = tmp("nested/store.json");
    let mut store = RetentionStore::default();
    store.set_bank("bank-a", selective()).expect("set");
    store
        .set_deployment(RetentionProfile::default())
        .expect("deploy");
    store.save(&path).expect("save");
    let back = RetentionStore::load(&path).expect("load");
    assert_eq!(back, store);

    let blocker = tmp("blocker");
    fs::write(&blocker, "x").expect("blocker");
    assert_eq!(
        store
            .save(&blocker.join("child.json"))
            .expect_err("mkdir")
            .code,
        ErrorCode::Internal
    );
    let dir = tmp("outdir");
    fs::create_dir_all(&dir).expect("dir");
    assert_eq!(
        store.save(&dir).expect_err("write").code,
        ErrorCode::Internal
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_file(&path);
}

#[test]
fn load_lossy_drops_invalid_profiles_and_falls_back() {
    // Unreadable file degrades to the builtin-only store.
    let bad = tmp("lossy-bad.json");
    fs::write(&bad, "{not json").expect("write");
    assert_eq!(RetentionStore::load_lossy(&bad), RetentionStore::default());

    // Invalid stored profiles are pruned; valid ones survive. A stored
    // `permissive` deployment default is dropped too (it MUST NOT be a default).
    let path = tmp("lossy.json");
    fs::write(
        &path,
        r#"{
            "deployment": {"version": 99},
            "banks": {
                "bad": {"verbosity": "selective", "theta_offsets": {"nope": 0.1}},
                "good": {"verbosity": "selective"}
            }
        }"#,
    )
    .expect("write");
    let store = RetentionStore::load_lossy(&path);
    assert!(store.deployment.is_none());
    assert!(store.bank_override("bad").is_none());
    assert_eq!(
        store.bank_override("good").expect("good").verbosity,
        Verbosity::Selective
    );
    assert_eq!(store.effective("bad").1, ProfileSource::Builtin);

    let permissive_default = tmp("lossy-permissive.json");
    fs::write(
        &permissive_default,
        r#"{"deployment": {"verbosity": "permissive"}, "banks": {}}"#,
    )
    .expect("write");
    let store = RetentionStore::load_lossy(&permissive_default);
    assert!(
        store.deployment.is_none(),
        "permissive default must be dropped"
    );
    assert_eq!(store.effective("any-bank").1, ProfileSource::Builtin);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(&permissive_default);
}

#[test]
fn resolve_path_prefers_explicit_then_env() {
    let explicit = PathBuf::from("/tmp/explicit-retention.json");
    assert_eq!(
        resolve_retention_path(Some(explicit.clone())),
        Some(explicit.clone())
    );
    assert_eq!(
        resolve_retention_path_with(Some(explicit.clone()), || Some("/tmp/env.json".into())),
        Some(explicit)
    );
    assert_eq!(
        resolve_retention_path_with(None, || Some(std::ffi::OsString::from("/tmp/env.json"))),
        Some(PathBuf::from("/tmp/env.json"))
    );
    assert_eq!(resolve_retention_path_with(None, || None), None);
}

#[test]
fn new_repo_banks_resolve_the_coding_default() {
    let store = RetentionStore::default();
    let (profile, source) = store.effective("repo:/Users/me/proj");
    assert_eq!(source, ProfileSource::CodingDefault);
    assert_eq!(source.as_str(), "coding_default");
    assert_eq!(profile, crate::coding_default::coding_default_profile());
    assert!(
        profile.mission.is_some(),
        "coding default carries a mission"
    );
    assert_eq!(profile.version, 3);

    // Non-repo banks keep the builtin default.
    let (profile, source) = store.effective("bank-a");
    assert_eq!(source, ProfileSource::Builtin);
    assert!(profile.mission.is_none());

    // An operator-set deployment default outranks the coding default.
    let mut store = RetentionStore::default();
    store.set_deployment(selective()).expect("deployment");
    let (_, source) = store.effective("repo:/Users/me/proj");
    assert_eq!(source, ProfileSource::Deployment);

    // An operator-set bank override outranks everything and is never rewritten.
    store
        .set_bank("repo:/Users/me/proj", selective())
        .expect("override");
    let (profile, source) = store.effective("repo:/Users/me/proj");
    assert_eq!(source, ProfileSource::Bank);
    assert_eq!(profile, selective());
}

/// an upgrade converts the stored v1 marker to the current version while every
/// value the stored profile carried stays as stored, and a fresh repo bank
/// picks up the coding default.
#[test]
fn upgrade_converts_the_stored_marker_and_defaults_new_repo_banks() {
    let dir = tmp("upgrade");
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("profiles.json");
    fs::write(
        &path,
        r#"{"banks": {"repo:/tmp/exists": {"verbosity": "selective", "duplicate_tolerance_write": "strict", "version": 1}}}"#,
    )
    .expect("write v1 store");
    let store = RetentionStore::load_lossy(&path);
    let (profile, source) = store.effective("repo:/tmp/exists");
    assert_eq!(source, ProfileSource::Bank);
    assert_eq!(
        profile.version, RETENTION_PROFILE_VERSION,
        "a stored v1 marker is converted to the current version"
    );
    assert_eq!(profile.verbosity, Verbosity::Selective);
    assert_eq!(
        profile.duplicate_tolerance_write,
        DuplicateTolerance::Strict
    );
    assert!(profile.mission.is_none(), "stored profile gains no mission");
    // The same store resolves a NEW repo bank to the coding default.
    let (_, new_source) = store.effective("repo:/tmp/fresh");
    assert_eq!(new_source, ProfileSource::CodingDefault);
    // And a save/load round-trip persists the converted marker and every value.
    store.save(&path).expect("save");
    let reloaded = RetentionStore::load_lossy(&path);
    assert_eq!(
        reloaded.bank_override("repo:/tmp/exists"),
        store.bank_override("repo:/tmp/exists")
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A stored version-3 profile keeps its new field values through load, prune,
/// and a save/load round trip.
#[test]
fn stored_version_three_profile_with_new_fields_survives_load_and_prune() {
    let dir = tmp("v3");
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("profiles.json");
    fs::write(
        &path,
        r#"{"banks": {"repo:/tmp/v3": {"version": 3, "retention_curve": false, "retention_floor": 0.5, "pack_fidelity_default": "exact", "provenance_floor_default": "stated", "maintenance_trigger": false}}}"#,
    )
    .expect("write v3 store");
    let store = RetentionStore::load(&path).expect("v3 store loads");
    let profile = store
        .bank_override("repo:/tmp/v3")
        .expect("v3 profile kept");
    assert_eq!(profile.version, 3);
    assert!(!profile.retention_curve);
    assert!((profile.retention_floor - 0.5).abs() < 1e-12);
    assert_eq!(profile.pack_fidelity_default, PackFidelityDefault::Exact);
    assert_eq!(
        profile.provenance_floor_default,
        Some(ProvenanceFloorDefault::Stated)
    );
    assert!(!profile.maintenance_trigger);
    // Pruning keeps a valid v3 profile, and saving it back preserves every value.
    let pruned = RetentionStore::load_lossy(&path);
    assert_eq!(pruned.bank_override("repo:/tmp/v3"), Some(profile));
    pruned.save(&path).expect("save");
    assert_eq!(
        RetentionStore::load_lossy(&path).bank_override("repo:/tmp/v3"),
        Some(profile)
    );
    let _ = fs::remove_dir_all(&dir);
}
