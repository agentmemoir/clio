// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the part of the reference contract the
//! headings cannot carry: every call a reference file shows must be valid input
//! for the tool it names, as the exported schema pack declares that tool, and
//! the compliance reference must name its destructive tools while showing no
//! call to one.
//!
//! It also owns the two small functions that make the negative cases provable:
//! `check_reference_set`, so a missing, extra or duplicated reference file can
//! be shown to fail, and `validate_example`, so an invalid example can be shown
//! to fail against the same schema the reference files are held to.
//!
//! ## Owns
//! - `derived_name`, the one place a reference file name is derived
//! - `check_reference_set`, the reference set comparison
//! - `validate_example`, the exported-pack validation of one call
//! - `every_reference_example_validates_against_the_exported_pack`
//! - `an_invalid_example_is_rejected`
//! - `an_incomplete_reference_set_is_rejected`
//! - `compliance_reference_names_its_destructive_tools_and_shows_no_call`
//!
//! ## Does not own
//! - The heading, tool-list and pointer checks (`group_references`)
//! - Catalog membership and guidance (`clio-types` `mcp_tool_groups_data`)
//! - The exported pack itself (`clio_mcp::schema::schema_pack`)
//!
//! ## Boundary
//!
//! The pack is the only schema source here, so an example is held to the schema
//! a caller receives, not to a second copy of it. A reference file is read from
//! the references directory, and each pure function takes its input as a
//! parameter, so a synthetic set or example can be shown to fail without a
//! broken file in the tree.

use clio_types::GROUP_TABLE;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::body_under;
use crate::group_references::{EXAMPLES_HEADING, TOOLS_HEADING};

/// The references directory, resolved from this crate's manifest directory so
/// the check reads the files beside the sources it was built from.
const REFERENCES_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../skills/clio-mcp/references"
);

/// The group whose reference file documents the one safety exception.
const COMPLIANCE_GROUP: &str = "compliance";

/// The confirm-gated tools this build must still publish, checked against the
/// derived set so a tool that lost its `confirm` property fails here instead of
/// shrinking the guard below without a word.
const CONFIRM_GATED: [&str; 10] = [
    "correct",
    "discard",
    "erase_request",
    "hygiene_clean",
    "reindex",
    "repair",
    "retention_profile_set",
    "shared_discard",
    "subject_request_retry",
    "sync_ack_skip",
];

/// The file name the catalog row at 0-based `row` names: `{row:02}-{group}.md`.
///
/// The name is derived from the row, so a hand-written list that disagrees with
/// the catalog fails rather than passing on a name nothing reads.
pub(crate) fn derived_name(row: usize) -> String {
    let spec = &GROUP_TABLE[row];
    format!("{:02}-{}.md", row + 1, spec.group.as_str())
}

/// Every file name the catalog derives, in catalog order.
fn derived_names() -> Vec<String> {
    (0..GROUP_TABLE.len()).map(derived_name).collect()
}

/// Whether `names` is exactly one file per catalog row: no missing name, no
/// extra name, and no name twice.
pub(crate) fn check_reference_set(names: &[String]) -> Result<(), String> {
    let expected = derived_names();
    let mut sorted = names.to_vec();
    sorted.sort_unstable();
    for pair in sorted.windows(2) {
        if pair[0] == pair[1] {
            return Err(format!("names the file `{}` twice", pair[0]));
        }
    }
    let missing: Vec<&String> = expected
        .iter()
        .filter(|name| !sorted.contains(name))
        .collect();
    let extra: Vec<&String> = sorted
        .iter()
        .filter(|name| !expected.contains(name))
        .collect();
    if missing.is_empty() && extra.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "is missing {missing:?} and holds the extra {extra:?} against the {} names the catalog derives",
            expected.len()
        ))
    }
}

/// Every reference file on disk, as `(name, text)`, in file-name order.
fn references() -> Vec<(String, String)> {
    let directory = Path::new(REFERENCES_DIR);
    let entries = std::fs::read_dir(directory).unwrap_or_else(|error| {
        panic!("the references directory `{REFERENCES_DIR}` cannot be read: {error}")
    });
    let mut names: Vec<String> = entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| {
                    panic!("the references directory `{REFERENCES_DIR}` has an unreadable entry: {error}")
                })
                .file_name()
                .into_string()
                .unwrap_or_else(|name| {
                    panic!(
                        "the references directory `{REFERENCES_DIR}` holds `{}`, a name that is not UTF-8",
                        name.display()
                    )
                })
        })
        .collect();
    names.sort_unstable();
    names
        .into_iter()
        .map(|name| {
            let path: PathBuf = directory.join(&name);
            let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!("the reference `{}` cannot be read: {error}", path.display())
            });
            (name, text)
        })
        .collect()
}

/// The exported `inputSchema` of every catalog tool, compiled once, offline.
///
/// One build per tool, because a build is slow and the same schema answers every
/// example of that tool. The build is offline exactly as a client holds the
/// pack, so a schema that needs a fetch fails here rather than at a caller.
fn validators() -> &'static BTreeMap<String, jsonschema::Validator> {
    static VALIDATORS: OnceLock<BTreeMap<String, jsonschema::Validator>> = OnceLock::new();
    VALIDATORS.get_or_init(|| {
        let pack = clio_mcp::schema::schema_pack();
        let tools = pack
            .get("tools")
            .and_then(Value::as_array)
            .expect("the exported pack publishes a tools array");
        let mut compiled = BTreeMap::new();
        for tool in tools {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .expect("every published tool has a name");
            let schema = tool
                .get("inputSchema")
                .unwrap_or_else(|| panic!("the exported pack publishes no `{name}` inputSchema"));
            let validator = jsonschema::options()
                .offline()
                .build(schema)
                .unwrap_or_else(|error| {
                    panic!("`{name}`: inputSchema does not compile offline: {error}")
                });
            compiled.insert(name.to_owned(), validator);
        }
        compiled
    })
}

/// Validate one call of `tool` with `args` against the exported pack.
///
/// The catalog is checked first, so a tool no group lists fails here rather than
/// validating against nothing. The message names the tool, the arguments and the
/// first fault, because the negative case asserts this value and a reference
/// check prints it.
fn validate_example(tool: &str, args: &Value) -> Result<(), String> {
    if !GROUP_TABLE.iter().any(|spec| spec.tools.contains(&tool)) {
        return Err(format!(
            "`{tool}` is not in the catalog, so no group promises it"
        ));
    }
    let validator = validators()
        .get(tool)
        .ok_or_else(|| format!("the exported pack publishes no `{tool}` schema"))?;
    validator.validate(args).map_err(|error| {
        format!(
            "`{tool}({args})` is not valid input at `{}`: {error}",
            error.instance_path().as_str()
        )
    })
}

/// The `(tool, arguments)` calls in the one fenced block under `## Examples`.
///
/// A file with no `## Examples` heading returns no call, because the compliance
/// reference carries none on purpose. A heading whose block is missing, or a
/// line that is not `tool(object)`, is a failure.
fn examples(label: &str, text: &str) -> Result<Vec<(String, Value)>, String> {
    if !text.lines().any(|line| line == EXAMPLES_HEADING) {
        return Ok(Vec::new());
    }
    let body = body_under(label, text, EXAMPLES_HEADING);
    let block = body
        .split_once("```")
        .and_then(|(_, after)| after.split_once("```"))
        .map(|(block, _)| block)
        .ok_or_else(|| {
            format!("the `{EXAMPLES_HEADING}` section of {label} holds no fenced block")
        })?;
    let mut calls = Vec::new();
    for example in block.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (tool, rest) = example
            .split_once('(')
            .ok_or_else(|| format!("the example `{example}` in {label} is not a call"))?;
        let args = rest
            .strip_suffix(')')
            .ok_or_else(|| format!("the example `{example}` in {label} does not close its call"))?;
        let parsed = serde_json::from_str::<Value>(args).map_err(|error| {
            format!(
                "the example `{example}` in {label} passes `{args}`, which is not JSON: {error}"
            )
        })?;
        if !parsed.is_object() {
            return Err(format!(
                "the example `{example}` in {label} passes `{args}`, which is not one JSON object"
            ));
        }
        calls.push((tool.to_owned(), parsed));
    }
    Ok(calls)
}

/// Every tool whose exported input schema declares a `confirm` property.
fn confirm_gated() -> Vec<String> {
    let pack = clio_mcp::schema::schema_pack();
    let mut gated: Vec<String> = pack
        .get("tools")
        .and_then(Value::as_array)
        .expect("the exported pack publishes a tools array")
        .iter()
        .filter(|tool| tool["inputSchema"]["properties"].get("confirm").is_some())
        .filter_map(|tool| tool["name"].as_str())
        .map(str::to_owned)
        .collect();
    gated.sort_unstable();
    gated
}

/// The compliance reference: its name, its catalog row and its text.
fn compliance() -> (String, &'static clio_types::GroupSpec, String) {
    let row = GROUP_TABLE
        .iter()
        .position(|spec| spec.group.as_str() == COMPLIANCE_GROUP)
        .unwrap_or_else(|| {
            panic!("the catalog has no `{COMPLIANCE_GROUP}` row: the safety exception belongs to that group")
        });
    let name = derived_name(row);
    let path = Path::new(REFERENCES_DIR).join(&name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("the reference `{}` cannot be read: {error}", path.display())
    });
    (name, &GROUP_TABLE[row], text)
}

#[test]
fn every_reference_example_validates_against_the_exported_pack() {
    let mut validated = 0_usize;
    for (name, text) in references() {
        let label = format!("the reference `{name}`");
        let calls = examples(&label, &text).unwrap_or_else(|error| panic!("{error}"));
        for (tool, args) in calls {
            validate_example(&tool, &args).unwrap_or_else(|error| panic!("{label}: {error}"));
            validated += 1;
        }
    }
    assert!(
        validated >= GROUP_TABLE.len(),
        "the reference files show {validated} calls: keep at least one example call under `{EXAMPLES_HEADING}` for every reference file outside the `{COMPLIANCE_GROUP}` exception, because a caller copies the call it reads"
    );
}

#[test]
fn an_invalid_example_is_rejected() {
    // Fixtures, never reference content: the check above would fail on either of
    // these, so this test proves the same call rejects them. Without it a
    // validator that accepted everything would pass the check above.
    let unknown = serde_json::json!({ "no_such_argument": 1 });
    let error = validate_example("get", &unknown)
        .expect_err("an argument outside the exported schema is accepted");
    assert!(
        error.contains("get"),
        "the refusal `{error}` does not name the tool it refused"
    );
    let mistyped = serde_json::json!({ "item_id": 7 });
    validate_example("get", &mistyped)
        .expect_err("a number where the schema declares a string is accepted");
    let outside = serde_json::json!({});
    let error = validate_example("no_such_tool", &outside)
        .expect_err("a tool outside the catalog is accepted");
    assert!(
        error.contains("no_such_tool"),
        "the refusal `{error}` does not name the tool it refused"
    );
}

#[test]
fn an_incomplete_reference_set_is_rejected() {
    let complete = derived_names();
    assert_eq!(
        check_reference_set(&complete),
        Ok(()),
        "the derived set is not one file per catalog row"
    );
    let mut missing = complete.clone();
    missing.pop();
    check_reference_set(&missing).expect_err("a missing reference file is accepted");
    let mut extra = complete.clone();
    extra.push("18-extra.md".to_owned());
    check_reference_set(&extra).expect_err("an extra reference file is accepted");
    let mut twice = complete.clone();
    twice.push(complete[0].clone());
    check_reference_set(&twice).expect_err("a duplicated reference name is accepted");
}

#[test]
fn compliance_reference_names_its_destructive_tools_and_shows_no_call() {
    let gated = confirm_gated();
    for tool in CONFIRM_GATED {
        assert!(
            gated.iter().any(|name| name == tool),
            "the exported pack declares no `confirm` property for `{tool}`, which this build must gate: a tool that lost its gate is one a caller can run without agreeing to it"
        );
    }

    let (name, spec, text) = compliance();
    let label = format!("the reference `{name}`");
    let tools = body_under(&label, &text, TOOLS_HEADING);
    let mut named = 0_usize;
    for tool in &gated {
        if !spec.tools.contains(&tool.as_str()) {
            continue;
        }
        named += 1;
        assert!(
            tools.contains(&format!("`{tool}`")),
            "the `{TOOLS_HEADING}` line of `{name}` does not name `{tool}`, a `{COMPLIANCE_GROUP}` tool that declares `confirm`: name every gated tool of the group, because the file is where a caller learns the tools exist"
        );
    }
    assert!(
        named > 0,
        "no `{COMPLIANCE_GROUP}` tool declares `confirm`, so the guard below checks nothing"
    );

    for tool in &gated {
        assert!(
            !text.contains(&format!("{tool}(")),
            "the reference `{name}` shows `{tool}(`, a call to a gated tool: name the tool and point at `tool_describe` instead, because a copied call there can erase data"
        );
    }
    assert!(
        !text.contains('{'),
        "the reference `{name}` carries a `{{`, an argument object: show no argument value for a `{COMPLIANCE_GROUP}` tool, because a copied value can erase the wrong data"
    );
}
