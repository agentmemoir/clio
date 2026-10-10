// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//!
//! # Responsibility
//!
//! This module is responsible for tests proving a stored retention profile
//! whose provenance floor names no rung resolves to the strictest rung instead
//! of no floor, and for the blast radius of that one unreadable value.
//!
//! ## Owns
//! - The strictest-rung fallback for an unreadable stored floor, per bank and per
//!   deployment default
//! - The intactness of every other field and bank sharing the same file
//! - The unchanged fail-open policy for a file that is genuinely malformed
//!
//! ## Does not own
//! - The ladder's wire names and the request-side refusal (see `retention_levels`)
//! - Bank resolution order and profile pruning (see `retention_store`)
//!
//! ## Boundary
//!
//! Changes to how a stored floor degrades should normally require changes here.
//! Changes to the ladder or to request-side validation should not.
//!
//! Keep this module focused on the stored floor's fail-closed behavior.

use super::retention_store_tests::tmp;
use super::*;
use crate::retention::{
    DuplicateTolerance, PackFidelityDefault, ProvenanceFloorDefault, Verbosity,
};

/// A stored floor naming no rung resolves to the strictest rung rather than to
/// no floor, because "no floor" would widen every read of that bank to every
/// rung while the operator believed the floor still applied.
#[test]
fn stored_floor_naming_no_rung_resolves_to_the_strictest_rung() {
    for raw in [r#""trusted""#, r#""Verified""#, r#""thirdparty""#, "3"] {
        let path = tmp(&format!("rung-{raw}.json"));
        fs::write(
            &path,
            format!(r#"{{"banks": {{"bank-a": {{"provenance_floor_default": {raw}}}}}}}"#),
        )
        .expect("write");
        let store = RetentionStore::load_lossy(&path);
        let profile = store
            .bank_override("bank-a")
            .expect("the profile still loads");
        assert_eq!(
            profile.provenance_floor_default,
            Some(ProvenanceFloorDefault::Verified),
            "`{raw}` must fail the floor closed to `verified`, never to no floor"
        );
        assert_eq!(
            store
                .effective("bank-a")
                .0
                .resolve()
                .provenance_floor_default(),
            Some(ProvenanceFloorDefault::Verified),
            "the resolved snapshot carries the strictest rung for `{raw}`"
        );
        let _ = fs::remove_file(&path);
    }
}

/// A floor that is absent, or explicitly `null`, still means no floor. The
/// fallback exists for a value this build cannot read, not to invent a floor
/// where the operator stated none.
#[test]
fn an_absent_or_null_stored_floor_still_means_no_floor() {
    let path = tmp("absent.json");
    fs::write(
        &path,
        r#"{"banks": {"plain": {"verbosity": "selective"}, "explicit-null": {"provenance_floor_default": null}}}"#,
    )
    .expect("write");
    let store = RetentionStore::load_lossy(&path);
    assert_eq!(
        store
            .bank_override("plain")
            .expect("plain")
            .provenance_floor_default,
        None,
        "an absent floor means no floor"
    );
    assert_eq!(
        store
            .bank_override("explicit-null")
            .expect("explicit null")
            .provenance_floor_default,
        None,
        "an explicit null means no floor"
    );
    let _ = fs::remove_file(&path);
}

/// The blast radius of one unreadable floor is its own profile. Before, the bad
/// value failed the whole document, so every bank's override and the deployment
/// default in that file were discarded together and the floor disappeared.
#[test]
fn one_unreadable_floor_leaves_the_rest_of_the_file_intact() {
    let path = tmp("blast.json");
    fs::write(
        &path,
        r#"{
            "deployment": {"verbosity": "selective", "provenance_floor_default": "inferred"},
            "banks": {
                "broken": {
                    "verbosity": "permissive",
                    "duplicate_tolerance_write": "strict",
                    "retention_floor": 0.75,
                    "pack_fidelity_default": "exact",
                    "maintenance_trigger": false,
                    "provenance_floor_default": "trusted"
                },
                "intact": {"verbosity": "selective", "provenance_floor_default": "third_party"}
            }
        }"#,
    )
    .expect("write");
    let store = RetentionStore::load_lossy(&path);

    // The same profile keeps every one of its other fields.
    let broken = store.bank_override("broken").expect("broken survives");
    assert_eq!(broken.verbosity, Verbosity::Permissive);
    assert_eq!(broken.duplicate_tolerance_write, DuplicateTolerance::Strict);
    assert!((broken.retention_floor - 0.75).abs() < 1e-12);
    assert_eq!(broken.pack_fidelity_default, PackFidelityDefault::Exact);
    assert!(!broken.maintenance_trigger);
    assert_eq!(
        broken.provenance_floor_default,
        Some(ProvenanceFloorDefault::Verified),
        "only the unreadable floor changed"
    );

    // The other bank in the same file is untouched.
    let intact = store.bank_override("intact").expect("intact survives");
    assert_eq!(intact.verbosity, Verbosity::Selective);
    assert_eq!(
        intact.provenance_floor_default,
        Some(ProvenanceFloorDefault::ThirdParty),
        "a readable rung is not rewritten"
    );

    // And so is the deployment default, which the same failure used to discard.
    let deployment = store.deployment.as_ref().expect("deployment survives");
    assert_eq!(deployment.verbosity, Verbosity::Selective);
    assert_eq!(
        deployment.provenance_floor_default,
        Some(ProvenanceFloorDefault::Inferred)
    );
    assert_eq!(store.effective("intact").1, ProfileSource::Bank);
    let _ = fs::remove_file(&path);
}

/// An unreadable floor on the deployment default degrades to the strictest rung
/// too, so a bank with no override is not silently left unfiltered.
#[test]
fn a_deployment_default_floor_naming_no_rung_fails_closed() {
    let path = tmp("deployment.json");
    fs::write(
        &path,
        r#"{"deployment": {"verbosity": "selective", "provenance_floor_default": "assumed"}, "banks": {}}"#,
    )
    .expect("write");
    let store = RetentionStore::load_lossy(&path);
    let (profile, source) = store.effective("any-bank");
    assert_eq!(
        source,
        ProfileSource::Deployment,
        "the default still applies"
    );
    assert_eq!(profile.verbosity, Verbosity::Selective);
    assert_eq!(
        profile.provenance_floor_default,
        Some(ProvenanceFloorDefault::Verified),
        "the deployment floor fails closed rather than vanishing"
    );
    let _ = fs::remove_file(&path);
}

/// The fallback is applied only while reading a stored file. A file that is
/// genuinely malformed JSON is still `ConfigCorrupt`, and `load_lossy` still
/// degrades it to the builtin store, so the fail-open policy for an unreadable
/// file is unchanged by the stricter floor.
#[test]
fn a_malformed_store_file_still_degrades_to_the_builtin_store() {
    for text in [
        r#"{"banks": {"a": {"provenance_floor_default": "trusted"}"#,
        "{not json",
    ] {
        let path = tmp("malformed.json");
        fs::write(&path, text).expect("write");
        assert_eq!(
            RetentionStore::load(&path).expect_err("truncated").code,
            ErrorCode::ConfigCorrupt,
            "a truncated file is still corrupt: {text}"
        );
        assert_eq!(
            RetentionStore::load_lossy(&path),
            RetentionStore::default(),
            "an unreadable file still degrades to the builtin store: {text}"
        );
        assert_eq!(
            RetentionStore::load_lossy(&path).effective("a").1,
            ProfileSource::Builtin,
            "the degraded store falls back to the builtin profile: {text}"
        );
        let _ = fs::remove_file(&path);
    }
}

/// A floor that this build can read survives a save and load round trip, so the
/// fallback never rewrites a file into a different one.
#[test]
fn a_readable_stored_floor_round_trips_unchanged() {
    let path = tmp("roundtrip.json");
    fs::write(
        &path,
        r#"{"banks": {"repo:/tmp/floor": {"verbosity": "selective", "provenance_floor_default": "stated"}}}"#,
    )
    .expect("write");
    let store = RetentionStore::load_lossy(&path);
    store.save(&path).expect("save");
    assert_eq!(
        RetentionStore::load_lossy(&path).bank_override("repo:/tmp/floor"),
        store.bank_override("repo:/tmp/floor"),
        "a readable floor reads back exactly as it was stored"
    );
    assert!(
        fs::read_to_string(&path)
            .expect("read")
            .contains(r#""provenance_floor_default": "stated""#),
        "the saved file still names the rung it was given"
    );
    let _ = fs::remove_file(&path);
}
