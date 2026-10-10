// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-side request fields the in-process
//! JSON surface resolves from one bank retention profile: the pack fidelity
//! channel and the wording of a refused channel name.
//!
//! ## Owns
//! - The resolver's `pack_fidelity_default` reaching the request, proven with a
//!   stand-in whose channel is `exact`, so a resolution that ignored the profile
//!   and hardcoded `flow` cannot pass
//! - An explicit channel still beating that profile default
//! - The refusal of a channel outside the closed set, with the message naming
//!   the `fidelity` argument instead of the `pack_fidelity_default` profile field
//!
//! ## Does not own
//! - The provenance floor's resolution (`surface_tests`,
//!   `provenance_surface_tests`) and the wire argument structs
//!   (`surface`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the resolved channel and its refusal wording.

use super::resolve_policy;
use crate::compose::Composer;
use crate::fixtures_tests as fx;
use crate::surface::{ReadPolicyResolver, compose_from_json};
use clio_config::retention::RetentionProfile;
use clio_config::retention_levels::PackFidelityDefault;
use serde_json::json;
use std::sync::Arc;

/// Bank read-policy stand-in whose profile names the exact channel and nothing
/// else that differs from the builtin default. `exact` is the one value a
/// hardcoded `flow` cannot produce, so a resolution that ignored this field
/// fails the test below.
struct ExactChannelBank;

impl ReadPolicyResolver for ExactChannelBank {
    fn read_policy(&self, _bank: &str) -> RetentionProfile {
        RetentionProfile {
            pack_fidelity_default: PackFidelityDefault::Exact,
            ..RetentionProfile::default()
        }
    }
}

/// The pack fidelity channel comes from the resolved profile, not from a
/// hardcoded default: the stand-in says `exact` and the builtin default says
/// `flow`, so the two paths are told apart by value and not only by source.
#[test]
fn the_resolver_profile_supplies_the_pack_fidelity_channel() {
    let (.., fidelity) =
        resolve_policy(None, None, None, "bank", Some(&ExactChannelBank)).expect("resolve");
    assert_eq!(
        fidelity,
        PackFidelityDefault::Exact,
        "the resolver's channel reaches the request, so the field is read"
    );

    let builtin = RetentionProfile::default();
    let (.., without_resolver) = resolve_policy(None, None, None, "bank", None).expect("resolve");
    assert_eq!(
        without_resolver, builtin.pack_fidelity_default,
        "the no-resolver path still applies the builtin default profile"
    );
    assert_ne!(
        fidelity, without_resolver,
        "the two stand-ins name different channels, so the check above cannot \
         pass on a hardcoded channel"
    );

    // An explicit channel still wins over the resolved profile default.
    let (.., explicit) =
        resolve_policy(None, None, Some("flow"), "bank", Some(&ExactChannelBank)).expect("resolve");
    assert_eq!(
        explicit,
        PackFidelityDefault::Flow,
        "an explicit channel beats the profile default"
    );
}

/// The channel's names are closed, so a mistyped one is refused rather than
/// packed as `flow`, and the refusal names the argument the caller sent instead
/// of the profile field the parser reads. The two published names still work,
/// so the refusal is about the value and not about the field.
#[test]
fn a_mistyped_pack_fidelity_channel_names_the_wire_argument() {
    let store = fx::store();
    let composer = Composer::new(Arc::new(fx::retriever(&store)));

    let refused = compose_from_json(
        &composer,
        &json!({
            "bank": "bank", "query": "city", "budget_tokens": 400,
            "fidelity": "verbatim"
        }),
        None,
    );
    let value = serde_json::to_value(&refused).expect("serialize");
    assert_eq!(
        value["ok"], false,
        "an unpublished channel returns no pack: {value}"
    );
    assert_eq!(
        value["code"], "invalid_argument",
        "an unpublished channel is the documented code: {value}"
    );
    assert!(
        value.get("sections").is_none(),
        "a refused channel returns no pack at all: {value}"
    );
    let message = value["message"].as_str().expect("error message");
    assert!(
        message.contains("fidelity must be flow|exact"),
        "the refusal names the wire argument: {message}"
    );
    assert!(
        !message.contains("pack_fidelity_default"),
        "the refusal does not name the stored profile field: {message}"
    );
    assert!(
        message.contains("verbatim"),
        "the refusal echoes the value the caller sent: {message}"
    );

    for published in [
        PackFidelityDefault::Flow.as_str(),
        PackFidelityDefault::Exact.as_str(),
    ] {
        let ok = compose_from_json(
            &composer,
            &json!({
                "bank": "bank", "query": "city", "budget_tokens": 400,
                "fidelity": published
            }),
            None,
        );
        let value = serde_json::to_value(&ok).expect("serialize");
        assert_eq!(value["ok"], true, "`{published}` must still pack: {value}");
    }
}
