// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the co-activation policy seam of
//! `hub_distill`: that a `set_policy` call in this process governs a wave
//! started after it, and that one wave never reads two different knob sets.
//!
//! ## Owns
//! - The ranking-policy tests for `HubDistillEngine`
//! - The mid-wave policy-change test that pins the one-snapshot contract
//!
//! ## Does not own
//! - The hub fixture helpers (`store`, `seed_hub`, `engine`, `policy`, `ctx`,
//!   declared in `hub_distill_tests`)
//! - The distill pipeline and its admission outcomes (`hub_distill_tests`)
//! - The read-only scans that consume a policy (`hub_distill_scan`)
//!
//! ## Boundary
//!
//! Split out of `hub_distill_tests.rs` to keep that file inside the 450-line
//! limit. Changes to how a policy change reaches a wave belong here; changes to
//! what a scan does with a policy belong in `hub_distill_scan`.
//!
//! Keep this module focused on the policy seam of hub distillation.

use super::hub_distill_tests::{ctx, engine, engine_at, policy, seed_hub, store};
use crate::hub_distill::HubDistillEngine;
use clio_admission::{AdmissionEvent, EventSink};
use clio_config::retention::ResolvedRetention;
use clio_store::{AssocPolicy, SqliteStore};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};

/// Three neighbours give every node degree 3, so a limit of 2 selects the hub
/// and the default of 16 does not.
fn seed_triangle(store: &Arc<SqliteStore>, bank: &str) {
    seed_hub(store, bank, "hub", &["n1", "n2", "n3"], 0.15);
}

#[test]
fn set_policy_governs_a_later_wave_without_rebuilding_the_engine() {
    let store = store();
    seed_triangle(&store, "bank");
    let engine = engine(&store, 0.0, 16);

    // The engine was built with the default limit, so degree 3 is not a hub.
    let hubs = engine.eligible_hubs("bank").unwrap();
    assert!(hubs.is_empty(), "eligible under the default: {hubs:?}");

    // A wave started after the change uses the new value; no second engine and
    // no second conversion are involved.
    engine.set_policy(policy(2));
    assert_eq!(
        engine.eligible_hubs("bank").unwrap(),
        vec!["hub".to_owned()]
    );
    let report = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(report.distilled.len(), 1);
    assert!(
        report.summary().contains("eligible=1,distilled=1"),
        "summary: {}",
        report.summary()
    );
}

#[test]
fn set_policy_above_the_degree_makes_the_hub_ineligible_again() {
    let store = store();
    seed_triangle(&store, "bank");
    let engine = engine(&store, 0.0, 2);
    assert_eq!(
        engine.eligible_hubs("bank").unwrap(),
        vec!["hub".to_owned()]
    );

    engine.set_policy(policy(16));
    let hubs = engine.eligible_hubs("bank").unwrap();
    assert!(hubs.is_empty(), "a higher limit kept a hub: {hubs:?}");
}

#[test]
fn w_min_gates_the_same_scan_the_degree_limit_uses() {
    let store = store();
    seed_triangle(&store, "bank");
    // The same limit that admitted the hub, but a prune threshold above every
    // edge's effective weight, so no edge counts toward the degree.
    let engine = engine(&store, 0.0, 2);
    assert_eq!(engine.eligible_hubs("bank").unwrap().len(), 1);

    engine.set_policy(AssocPolicy {
        w_min: 0.99,
        ..policy(2)
    });
    let hubs = engine.eligible_hubs("bank").unwrap();
    assert!(hubs.is_empty(), "w_min kept a hub: {hubs:?}");
}

#[test]
fn a_default_policy_leaves_eligibility_unchanged() {
    let store = store();
    seed_triangle(&store, "bank");
    let engine = engine(&store, 0.0, 16);
    engine.set_policy(AssocPolicy::default());
    let hubs = engine.eligible_hubs("bank").unwrap();
    assert!(hubs.is_empty(), "the default made a hub: {hubs:?}");
    assert_eq!(AssocPolicy::default(), policy(16));
}

/// The decay knob decides eligibility on its own, with the clock injected.
///
/// `seed_hub` stamps every edge at millis 0, so a clock 60 days later decays
/// the default 30-day half-life by a factor of four: 0.15 becomes 0.0375, under
/// the default prune threshold of 0.05. Lowering the threshold brings the same
/// edges back. Both numbers come from the injected clock, so this result does
/// not depend on when the suite runs — which is why it belongs here and not in
/// the end-to-end harness, where elapsed real time is not controllable.
#[test]
fn the_decay_knob_alone_can_add_or_remove_every_hub() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let engine = engine_at(&store, 0.0, 2, 60 * 86_400_000);

    // Decayed to 0.0375, which the default 0.05 threshold drops.
    let hubs = engine.eligible_hubs("bank").unwrap();
    assert!(hubs.is_empty(), "decay kept a hub: {hubs:?}");

    engine.set_policy(AssocPolicy {
        w_min: 0.01,
        ..policy(2)
    });
    assert_eq!(
        engine.eligible_hubs("bank").unwrap(),
        vec!["hub".to_owned()]
    );
}

/// Replaces the engine's policy at the first admission decision of a wave.
///
/// That emit lands after one hub's neighbor scan and before the next hub's, so
/// the flip sits between the two points where a wave reads the policy. The wave
/// is synchronous, so this is a fixed point in the wave, not an elapsed time.
#[derive(Debug, Default)]
struct FlipWMinOnce {
    /// Filled after the engine is shared, because `set_sink` needs `&mut self`
    /// and a `Weak` needs the `Arc` to exist first.
    engine: OnceLock<Weak<HubDistillEngine>>,
    flipped: AtomicBool,
}

impl FlipWMinOnce {
    fn arm(&self, engine: &Arc<HubDistillEngine>) {
        self.engine
            .set(Arc::downgrade(engine))
            .expect("sink armed once");
    }
}

impl EventSink for FlipWMinOnce {
    fn emit(&self, _event: AdmissionEvent) {
        if self.flipped.swap(true, Ordering::SeqCst) {
            return;
        }
        // Above every seeded edge weight (0.15), so a scan under this policy
        // finds no neighbors at all.
        if let Some(engine) = self.engine.get().and_then(Weak::upgrade) {
            engine.set_policy(AssocPolicy {
                w_min: 0.9,
                ..policy(2)
            });
        }
    }
}

#[test]
fn one_wave_never_mixes_two_knob_sets() {
    let store = store();
    seed_hub(&store, "bank", "hub_a", &["n1", "n2", "n3"], 0.15);
    seed_hub(&store, "bank", "hub_b", &["n4", "n5", "n6"], 0.15);
    let mut engine = engine(&store, 0.0, 2);
    let sink = Arc::new(FlipWMinOnce::default());
    engine.set_sink(Arc::clone(&sink) as Arc<dyn EventSink + Send + Sync>);
    let engine = Arc::new(engine);
    sink.arm(&engine);

    // Degree 3 per hub against limit 2, so both hubs are eligible. The policy
    // flips while hub_a is admitted, which is after the wave's own eligibility
    // scan and before hub_b's neighbor scan.
    let report = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(
        report.eligible,
        vec!["hub_a".to_owned(), "hub_b".to_owned()]
    );
    assert_eq!(
        report.distilled.len(),
        2,
        "hub_b lost its neighbors to a policy change mid-wave: {report:?}"
    );
}
