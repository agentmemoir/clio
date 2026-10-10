// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Runtime retention profile accessor tests.

use crate::runtime::{McpOpenOptions, McpState};
use crate::runtime_open::load_retention_store_from;
use crate::runtime_retention::retention_test_support::RetentionStoreFixture;
use clio_config::retention::{RetentionProfile, Verbosity};
use clio_config::retention_store::{ProfileSource, RetentionStore};
use std::sync::Arc;

fn state() -> RetentionStoreFixture {
    RetentionStoreFixture::open(&McpOpenOptions::default())
}

#[test]
fn resolved_retention_defaults_to_builtin() {
    let s = state();
    let resolved = s.resolved_retention("bank-a");
    assert!(resolved.theta_delta().abs() < 1e-12);
    assert!(resolved.novelty_floor().abs() < 1e-12);
    assert_eq!(resolved.profile_version(), 3);
    assert!(resolved.retention_curve(), "curve defaults on");
    assert!((resolved.retention_floor() - 0.25).abs() < 1e-12);
    let (_, source) = s.retention_effective("bank-a");
    assert_eq!(source, ProfileSource::Builtin);
}

#[test]
fn resolved_retention_exposes_the_bank_curve_policy() {
    let s = state();
    s.set_retention(
        "bank-a",
        RetentionProfile {
            retention_curve: false,
            retention_floor: 0.5,
            ..RetentionProfile::default()
        },
    )
    .expect("set");
    let resolved = s.resolved_retention("bank-a");
    assert!(!resolved.retention_curve());
    assert!((resolved.retention_floor() - 0.5).abs() < 1e-12);
    // A bank without an override keeps the builtin default.
    let other = s.resolved_retention("bank-b");
    assert!(other.retention_curve());
    assert!((other.retention_floor() - 0.25).abs() < 1e-12);
}

#[test]
fn set_retention_changes_only_the_named_bank() {
    let s = state();
    let persisted = s
        .set_retention(
            "bank-a",
            RetentionProfile {
                verbosity: Verbosity::Selective,
                ..RetentionProfile::default()
            },
        )
        .expect("set");
    assert!(persisted, "the fixture's configured store path is written");
    assert!(s.resolved_retention("bank-a").theta_delta() > 0.0);
    assert!(s.resolved_retention("bank-b").theta_delta().abs() < 1e-12);
    assert_eq!(s.retention_effective("bank-a").1, ProfileSource::Bank);

    let deploy = s
        .set_retention_deployment(RetentionProfile {
            verbosity: Verbosity::Selective,
            ..RetentionProfile::default()
        })
        .expect("deploy");
    assert!(deploy);
    assert_eq!(s.retention_effective("bank-b").1, ProfileSource::Deployment);
    assert!(s.resolved_retention("bank-b").theta_delta() > 0.0);

    // Invalid profiles are rejected before they reach the store.
    assert!(
        s.set_retention(
            "bank-a",
            RetentionProfile {
                version: 7,
                ..RetentionProfile::default()
            }
        )
        .is_err()
    );

    // `permissive` MUST NOT become a deployment default.
    assert!(
        s.set_retention_deployment(RetentionProfile {
            verbosity: Verbosity::Permissive,
            ..RetentionProfile::default()
        })
        .is_err()
    );
    assert_eq!(
        s.retention_effective("bank-b").1,
        ProfileSource::Deployment,
        "rejected default must not apply"
    );
}

#[test]
fn set_retention_path_persists_and_reloads() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("am-mcp-retention-{nanos}.json"));

    let mut writer = McpState::open(&McpOpenOptions::default()).expect("state");
    Arc::get_mut(&mut writer)
        .expect("unique state")
        .set_retention_path(path.clone());
    let persisted = writer
        .set_retention(
            "bank-a",
            RetentionProfile {
                verbosity: Verbosity::Selective,
                ..RetentionProfile::default()
            },
        )
        .expect("set");
    assert!(persisted);

    let mut reader = McpState::open(&McpOpenOptions::default()).expect("state");
    Arc::get_mut(&mut reader)
        .expect("unique state")
        .set_retention_path(path.clone());
    let (profile, source) = reader.retention_effective("bank-a");
    assert_eq!(source, ProfileSource::Bank);
    assert_eq!(profile.verbosity, Verbosity::Selective);
    assert!(reader.resolved_retention("bank-a").theta_delta() > 0.0);

    let _ = std::fs::remove_file(path);
}

#[test]
fn retention_store_loader_handles_both_path_arms() {
    assert_eq!(load_retention_store_from(None), RetentionStore::default());

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let missing = std::env::temp_dir().join(format!("am-mcp-retention-missing-{nanos}.json"));
    assert_eq!(
        load_retention_store_from(Some(missing)),
        RetentionStore::default()
    );

    let corrupt = std::env::temp_dir().join(format!("am-mcp-retention-corrupt-{nanos}.json"));
    std::fs::write(&corrupt, "{not json").expect("write");
    assert_eq!(
        load_retention_store_from(Some(corrupt.clone())),
        RetentionStore::default()
    );
    let _ = std::fs::remove_file(corrupt);
}

#[test]
fn failed_persist_rolls_back_the_in_memory_store() {
    let dir = std::env::temp_dir().join(format!(
        "am-mcp-retention-rollback-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let mut s = McpState::open(&McpOpenOptions::default()).expect("state");
    Arc::get_mut(&mut s)
        .expect("unique state")
        .set_retention_path(dir.clone());

    // First set fails on save; the caller sees the error and the store stays empty.
    assert!(
        s.set_retention("bank-a", RetentionProfile::default())
            .is_err()
    );
    assert_eq!(
        s.retention_effective("bank-a").1,
        ProfileSource::Builtin,
        "failed persist must leave no override"
    );

    // Same guarantee for the deployment default.
    assert!(
        s.set_retention_deployment(RetentionProfile::default())
            .is_err()
    );
    assert_eq!(
        s.retention_effective("bank-b").1,
        ProfileSource::Builtin,
        "failed persist must leave no deployment default"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn successful_persist_keeps_the_stored_change() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("am-mcp-retention-ok-{nanos}"));
    std::fs::create_dir_all(&dir).expect("dir");
    let mut s = McpState::open(&McpOpenOptions::default()).expect("state");
    Arc::get_mut(&mut s)
        .expect("unique state")
        .set_retention_path(dir.join("profiles.json"));
    let persisted = s
        .set_retention(
            "bank-a",
            RetentionProfile {
                verbosity: Verbosity::Selective,
                ..RetentionProfile::default()
            },
        )
        .expect("set");
    assert!(persisted);
    assert_eq!(s.retention_effective("bank-a").1, ProfileSource::Bank);
    let _ = std::fs::remove_dir_all(dir);
}
