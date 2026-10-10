// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the value surface of the stats types: the
//! serialized report wire shape and the derived value behaviour of the
//! caller-supplied input types.
//!
//! ## Owns
//! - The serialization contract for `StatsReport`: the field names on the
//!   wire and the `Count { label, count }` entry shape
//! - The default, clone, and debug behaviour of `PressureCounts` and
//!   `StatsInputs` as it shows up in the report
//!
//! ## Does not own
//! - The report counts themselves (see `stats_tests.rs`)
//! - Stamp parsing and band-edge shifts (see `stats_stamp_tests.rs`)
//!
//! ## Boundary
//!
//! Changes to the report wire shape or the input types' value surface should
//! normally require changes here. Changes to counting or stamp arithmetic
//! should not require changes here.
//!
//! Keep this module focused on stats type surface tests.

use crate::stats::{Count, PressureCounts, StatsInputs, stats};
use crate::test_unique::unique;
use clio_store::{DekProvider, LocalDevKms, SqliteStore};
use std::sync::Arc;

fn store() -> SqliteStore {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open_in_memory(kms).unwrap()
}

/// Caller inputs with the default half-life, one fixed instant, and the
/// given pressure section.
fn inputs(pressure: PressureCounts) -> StatsInputs {
    StatsInputs {
        band_unit_days: 30.0,
        now: "2026-03-01T00:00:00Z".into(),
        pressure: Some(pressure),
    }
}

#[test]
fn stats_report_serializes_with_count_shape_and_field_names() {
    let store = store();
    let bank = unique("b");
    let report = stats(
        &store,
        &bank,
        &inputs(PressureCounts {
            flagged: Some(2),
            stale_summaries: None,
            archive_candidates: None,
        }),
    )
    .unwrap();
    let value = serde_json::to_value(&report).unwrap();
    let mut names: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "bank",
            "by_age_band",
            "by_category",
            "by_epistemic_kind",
            "by_provenance_class",
            "by_tier",
            "items_active",
            "items_discarded",
            "items_total",
            "pressure_counts",
        ],
        "{names:?}"
    );
    // Every count section keeps the two-key Count shape: label and count.
    let sections = [
        "by_tier",
        "by_category",
        "by_epistemic_kind",
        "by_age_band",
        "by_provenance_class",
        "pressure_counts",
    ];
    for section in sections {
        for entry in value[section].as_array().unwrap() {
            let mut keys: Vec<&str> = entry
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, vec!["count", "label"], "{section}");
        }
    }
    assert_eq!(
        value["pressure_counts"][0],
        serde_json::json!({"label": "flagged", "count": 2})
    );
}

#[test]
fn default_pressure_counts_relay_no_section() {
    let store = store();
    let bank = unique("b");
    let report = stats(&store, &bank, &inputs(PressureCounts::default())).unwrap();
    // An every-None pressure section is omitted, never a fabricated zero.
    assert_eq!(report.pressure_counts, [] as [Count; 0]);
}

#[test]
fn cloned_pressure_counts_relay_the_same_values() {
    let store = store();
    let bank = unique("b");
    let pressure = PressureCounts {
        flagged: Some(3),
        stale_summaries: Some(0),
        archive_candidates: None,
    };
    let original = stats(&store, &bank, &inputs(pressure.clone())).unwrap();
    let cloned = stats(&store, &bank, &inputs(pressure)).unwrap();
    assert_eq!(original.pressure_counts, cloned.pressure_counts);
    assert_eq!(
        original.pressure_counts,
        vec![
            Count {
                label: "flagged".into(),
                count: 3,
            },
            Count {
                label: "stale summaries".into(),
                count: 0,
            },
        ]
    );
}

#[test]
fn cloned_stats_inputs_produce_the_same_report() {
    let store = store();
    let bank = unique("b");
    let inputs = inputs(PressureCounts::default());
    let clone = inputs.clone();
    let from_clone = stats(&store, &bank, &clone).unwrap();
    let from_original = stats(&store, &bank, &inputs).unwrap();
    assert_eq!(from_clone, from_original);
}

#[test]
fn stats_input_types_debug_output_names_the_fields() {
    let inputs = inputs(PressureCounts {
        flagged: Some(1),
        stale_summaries: None,
        archive_candidates: None,
    });
    let text = format!("{inputs:?}");
    assert!(text.contains("band_unit_days: 30.0"), "{text}");
    assert!(text.contains("now:"), "{text}");
    assert!(text.contains("pressure:"), "{text}");
    let pressure_text = format!("{:?}", PressureCounts::default());
    assert!(pressure_text.contains("flagged: None"), "{pressure_text}");
    assert!(
        pressure_text.contains("stale_summaries: None"),
        "{pressure_text}"
    );
    assert!(
        pressure_text.contains("archive_candidates: None"),
        "{pressure_text}"
    );
}
