// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding every reference file's tool list to one owner: each published, bound business tool appears in exactly one group list, no discovery helper appears in any of them, and every listed name belongs to the group whose catalog row the file mirrors.
//!
//! ## Owns
//! - `owners`, the tool list read from every reference file
//! - `check_ownership`, the pure rule over the lists, the catalog, and the bound set
//! - The named ownership failure each rule reports
//!
//! ## Does not own
//! - Reading the reference files (`group_references`)
//! - The approved membership and guidance data (`clio-types`)
//! - The published catalog definitions and the bound set (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to an ownership rule should normally require changes here.
//! Changes to how a reference file is read should not require changes here.
//!
//! Keep this module focused on the ownership rule.

use std::collections::BTreeSet;

use clio_types::GroupSpec;

use crate::group_references::{listed_tools, rows, tools_line};

/// The helpers that serve discovery and belong to no business group.
const DISCOVERY_HELPERS: [&str; 3] = ["tool_search", "tool_describe", "tool_invoke"];

/// Every reference file's group paired with the tool names its list holds.
pub(crate) fn owners() -> Vec<(&'static str, Vec<&'static str>)> {
    rows()
        .map(|(_, name, text, spec)| {
            let group = spec.group.as_str();
            let line = tools_line(group, name, text);
            (group, listed_tools(name, line))
        })
        .collect()
}

/// The published and bound name sets the real catalog reports.
fn real_sets() -> (BTreeSet<&'static str>, BTreeSet<&'static str>) {
    let published = clio_mcp::schema::catalog_defs()
        .into_iter()
        .map(|def| def.name)
        .collect();
    let bound = clio_types::bound_tools().into_iter().collect();
    (published, bound)
}

/// Check every ownership list against the catalog rows, the published set, and the bound set.
///
/// The sets are parameters so a test can exercise a name the live catalog does not carry.
///
/// # Errors
///
/// Returns the first rule a list breaks, as a sentence that names the rule and the repair.
pub(crate) fn check_ownership(
    lists: &[(&str, Vec<&str>)],
    table: &[GroupSpec],
    published: &BTreeSet<&str>,
    bound: &BTreeSet<&str>,
) -> Result<(), String> {
    let mut owned: Vec<(&str, &str)> = Vec::new();
    for (group, names) in lists {
        let group = *group;
        for name in names.iter().copied() {
            if let Some(existing) = owned.iter().find(|entry| entry.0 == name) {
                let first = existing.1;
                return Err(format!(
                    "ownership: the tool `{name}` is owned by both `{first}` and `{group}`: give it exactly one group owner, because a second owner makes the package and the catalog disagree"
                ));
            }
            if DISCOVERY_HELPERS.contains(&name) {
                return Err(format!(
                    "ownership: the `{group}` ownership list names `{name}`, a discovery helper: keep the three helpers out of the ownership lists, because they belong to no business group"
                ));
            }
            if !published.contains(name) {
                return Err(format!(
                    "ownership: the `{group}` ownership list names `{name}`, which no published catalog definition declares: list only bound business tools, because a group owns only a tool the catalog publishes"
                ));
            }
            if !bound.contains(name) {
                return Err(format!(
                    "ownership: the `{group}` ownership list names `{name}`, which the catalog publishes but `bound_tools()` does not bind: a tool this build does not bind has no group owner"
                ));
            }
            let outside_row = table
                .iter()
                .find(|row| row.group.as_str() == group)
                .is_some_and(|row| !row.tools.contains(&name));
            if outside_row {
                return Err(format!(
                    "ownership: the `{group}` ownership list names `{name}`, which the `{group}` catalog row does not hold: list it under the one row whose tools include it, because the file and the row are one statement"
                ));
            }
            owned.push((name, group));
        }
    }
    for name in bound {
        if !owned.iter().any(|entry| entry.0 == *name) {
            return Err(format!(
                "ownership: the bound tool `{name}` has no ownership entry: give it exactly one group list, because every bound business tool needs one"
            ));
        }
    }
    Ok(())
}

#[test]
fn ownership_covers_every_bound_tool_exactly_once() {
    let lists = owners();
    let entries: usize = lists.iter().map(|(_, names)| names.len()).sum();
    assert_eq!(
        entries, 86,
        "the reference files list {entries} tool names rather than one entry for each of the 86 bound tools"
    );
    let (published, bound) = real_sets();
    assert_eq!(
        check_ownership(&lists, clio_types::GROUP_TABLE, &published, &bound),
        Ok(()),
        "the real reference tool lists break their own ownership rule"
    );
    for (group, names) in &lists {
        for name in names {
            assert!(
                !DISCOVERY_HELPERS.contains(name),
                "the `{group}` list carries `{name}`, a discovery helper"
            );
        }
    }
}

#[test]
fn an_unknown_ownership_entry_is_rejected() {
    let lists = [("retrieval", vec!["ghost_tool"])];
    let published = BTreeSet::from(["retrieve"]);
    let bound = BTreeSet::from(["retrieve"]);
    assert_eq!(
        check_ownership(&lists, clio_types::GROUP_TABLE, &published, &bound),
        Err(
            "ownership: the `retrieval` ownership list names `ghost_tool`, which no published catalog definition declares: list only bound business tools, because a group owns only a tool the catalog publishes"
                .to_owned()
        )
    );
}

#[test]
fn an_unbound_ownership_entry_is_rejected() {
    // The live published and bound sets are equal today, so a published-only name
    // needs synthetic sets: no real tool exercises this rule.
    let lists = [("retrieval", vec!["retrieve"])];
    let published = BTreeSet::from(["retrieve"]);
    let bound = BTreeSet::new();
    assert_eq!(
        check_ownership(&lists, clio_types::GROUP_TABLE, &published, &bound),
        Err(
            "ownership: the `retrieval` ownership list names `retrieve`, which the catalog publishes but `bound_tools()` does not bind: a tool this build does not bind has no group owner"
                .to_owned()
        )
    );
}

#[test]
fn a_missing_ownership_entry_is_rejected() {
    let lists = [("retrieval", vec!["retrieve"])];
    let published = BTreeSet::from(["retrieve", "store"]);
    let bound = BTreeSet::from(["retrieve", "store"]);
    assert_eq!(
        check_ownership(&lists, clio_types::GROUP_TABLE, &published, &bound),
        Err(
            "ownership: the bound tool `store` has no ownership entry: give it exactly one group list, because every bound business tool needs one"
                .to_owned()
        )
    );
}

#[test]
fn a_wrong_group_ownership_entry_is_rejected() {
    let lists = [("retrieval", vec!["store"])];
    let published = BTreeSet::from(["store"]);
    let bound = BTreeSet::from(["store"]);
    assert_eq!(
        check_ownership(&lists, clio_types::GROUP_TABLE, &published, &bound),
        Err(
            "ownership: the `retrieval` ownership list names `store`, which the `retrieval` catalog row does not hold: list it under the one row whose tools include it, because the file and the row are one statement"
                .to_owned()
        )
    );
}

#[test]
fn a_duplicate_ownership_entry_is_rejected() {
    let lists = [
        ("retrieval", vec!["retrieve"]),
        ("item-read", vec!["retrieve"]),
    ];
    let published = BTreeSet::from(["retrieve"]);
    let bound = BTreeSet::from(["retrieve"]);
    assert_eq!(
        check_ownership(&lists, clio_types::GROUP_TABLE, &published, &bound),
        Err(
            "ownership: the tool `retrieve` is owned by both `retrieval` and `item-read`: give it exactly one group owner, because a second owner makes the package and the catalog disagree"
                .to_owned()
        )
    );
}

#[test]
fn examples_and_describe_pointers_are_not_ownership_entries() {
    let text = r#"# Retrieval

## When to use

Find memory or build grounded context.

## Tools

`retrieve`, `store`

## Examples

```
retrieve({"query": "deploy steps"})
retrieve({"query": "deploy steps"})
```

## Exact arguments

Call `tool_describe("retrieve")` for the exact arguments.
"#;
    let line = tools_line("retrieval", "synthetic.md", text);
    assert_eq!(
        listed_tools("synthetic.md", line),
        vec!["retrieve", "store"]
    );
}

#[test]
fn a_listed_discovery_helper_is_rejected() {
    let lists = [("retrieval", vec!["tool_describe"])];
    let published = BTreeSet::from(["tool_describe"]);
    let bound = BTreeSet::new();
    assert_eq!(
        check_ownership(&lists, clio_types::GROUP_TABLE, &published, &bound),
        Err(
            "ownership: the `retrieval` ownership list names `tool_describe`, a discovery helper: keep the three helpers out of the ownership lists, because they belong to no business group"
                .to_owned()
        )
    );
}
