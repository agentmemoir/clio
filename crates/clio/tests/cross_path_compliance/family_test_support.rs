// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the ledger expectations of the cross-path
//! compliance proof: the stored families the scope ledger names, the count each
//! family holds for the subject under test, and the named omission the ledger
//! gives every family it cannot attribute to that subject.
//!
//! ## Owns
//! - `LEDGER_FAMILIES`, the stored families the ledger names
//! - `FamilyFixture`, the seeded counts and the expected ledger omissions
//! - `assert_every_family`, the manifest check the export scenario runs
//!
//! ## Does not own
//! - The rows the every-family fixture writes (see `family_fixtures`)
//! - The plain item fixture, the scratch directory, and the store openers (see
//!   `fixtures`)
//! - The export route and the scenario that drives it (see
//!   `scenario_export_erase_restore`)
//!
//! ## Boundary
//!
//! Changes to which families the proof covers, or to the counts and omissions
//! it expects, should normally require changes here. Changes to a scenario's
//! assertions should not.
//!
//! Keep this module focused on the ledger expectations and the check that
//! pins them.

use serde_json::Value;

/// The stored families the scope ledger names, in its report order. The export
/// manifest names the same nine.
pub const LEDGER_FAMILIES: [&str; 9] = [
    "items",
    "triples",
    "assoc_edges",
    "persona_stable",
    "persona_preferences",
    "tasks",
    "failures",
    "beliefs",
    "tombstones",
];

/// The item rows the fixture writes for the subject under test: one semantic
/// item plus the three carrier items the triple, task, and failure rows are
/// written under.
const SUBJECT_ITEMS: u64 = 4;

/// The item rows the other subject of the same bank contributes. The ledger
/// measures the item family store-wide, so this is the remainder the subject's
/// own selection leaves out.
const CONTROL_ITEMS: u64 = 1;

/// The rows the one seeded belief writes: the belief itself and its first
/// confidence entry. The ledger measures both tables as one family.
const BELIEF_ROWS: u64 = 2;

/// What the every-family fixture wrote, and what the scope ledger therefore
/// expects the export manifest to report for the subject under test.
#[derive(Debug, Clone)]
pub struct FamilyFixture {
    /// The subject's row count per family, in the ledger's report order.
    counts: Vec<(&'static str, u64)>,
    /// The named ledger omission per family, when the ledger carries one.
    omissions: Vec<(&'static str, Option<String>)>,
}

impl FamilyFixture {
    /// The counts and the ledger omissions the every-family fixture produces.
    ///
    /// The item family is measured store-wide, so the other subject's item is
    /// the remainder the subject's own selection leaves out. The belief family
    /// is measured over the belief and its confidence-entry tables, so the one
    /// seeded belief leaves its confidence entry out. The three families the
    /// ledger links by a stored subject column select every row they hold.
    pub(crate) fn expected() -> Self {
        let measured_items = SUBJECT_ITEMS + CONTROL_ITEMS;
        let counts = vec![
            ("items", SUBJECT_ITEMS),
            ("triples", 1),
            ("assoc_edges", 0),
            ("persona_stable", 0),
            ("persona_preferences", 0),
            ("tasks", 1),
            ("failures", 1),
            ("beliefs", 1),
            ("tombstones", 0),
        ];
        let omissions = vec![
            (
                "items",
                Some(format!(
                    "items: store-wide {measured_items} items rows measured: {SUBJECT_ITEMS} \
                     selected for this subject, {CONTROL_ITEMS} not selected by this mapping"
                )),
            ),
            ("triples", None),
            (
                "assoc_edges",
                Some(
                    "assoc_edges: no subject column: bank-scoped graph edges between item ids \
                     (store-wide 1 rows measured)"
                        .to_owned(),
                ),
            ),
            (
                "persona_stable",
                Some(
                    "persona_stable: no subject column: the persona document is bank-scoped \
                     (store-wide 1 rows measured)"
                        .to_owned(),
                ),
            ),
            (
                "persona_preferences",
                Some(
                    "persona_preferences: no subject column: preferences are bank-scoped, not \
                     per subject (store-wide 1 rows measured)"
                        .to_owned(),
                ),
            ),
            ("tasks", None),
            ("failures", None),
            (
                "beliefs",
                Some(format!(
                    "beliefs: store-wide {BELIEF_ROWS} beliefs rows measured: 1 selected for this \
                     subject, {} not selected by this mapping",
                    BELIEF_ROWS - 1
                )),
            ),
            (
                "tombstones",
                Some(
                    "tombstones: compliance_tombstones store a one-way subject_hash only"
                        .to_owned(),
                ),
            ),
        ];
        Self { counts, omissions }
    }
}

/// Assert the export manifest carries every family the ledger names, with the
/// count the fixture seeded, and the ledger's own omission for each family.
///
/// The manifest must name the nine families and no other, every count must
/// equal the seeded count, and each family must carry exactly the omission the
/// ledger names for it: none for a family the ledger selects completely, and
/// the ledger's own reason for every family it cannot attribute to the subject.
/// The manifest also names omissions for the store slots the ledger does not
/// count, so only the nine counted families are checked here.
pub fn assert_every_family(manifest: &Value, fixture: &FamilyFixture) {
    let counts = manifest["counts_by_type"]
        .as_array()
        .expect("the manifest counts_by_type");
    let labels: Vec<&str> = counts
        .iter()
        .map(|entry| entry[0].as_str().expect("a family label"))
        .collect();
    assert_eq!(
        labels,
        LEDGER_FAMILIES.to_vec(),
        "the manifest names the ledger's families: {manifest}"
    );
    for (family, want) in &fixture.counts {
        let entry = counts
            .iter()
            .find(|entry| entry[0].as_str() == Some(*family))
            .unwrap_or_else(|| panic!("family {family} is missing: {manifest}"));
        assert_eq!(
            entry[1].as_u64(),
            Some(*want),
            "family {family} carries the seeded count: {manifest}"
        );
    }
    let omissions = manifest["omissions"]
        .as_array()
        .expect("the manifest omissions");
    for (family, expected) in &fixture.omissions {
        let prefix = format!("{family}: ");
        let named: Vec<&str> = omissions
            .iter()
            .filter_map(Value::as_str)
            .filter(|text| text.starts_with(&prefix))
            .collect();
        match expected {
            Some(text) => assert_eq!(
                named,
                vec![text.as_str()],
                "family {family} carries the ledger's own omission: {manifest}"
            ),
            None => assert!(
                named.is_empty(),
                "family {family} carries no omission: {named:?} in {manifest}"
            ),
        }
    }
}
