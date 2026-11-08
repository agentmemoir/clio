// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding every group reference file under
//! `skills/clio-mcp/references/` to the package contract: an exact
//! `## When to use` heading whose first paragraph is the catalog sentence, an
//! exact `## Tools` heading whose one line lists exactly the catalog row's
//! tools, one fenced example block that only calls that row's tools with one
//! JSON object each, and an exact `## Exact arguments` heading whose body
//! carries the `tool_describe` pointer. It is also the gate for the two
//! prohibitions: no schema content and no parameter table anywhere in a
//! reference file, and no restated tool count.
//!
//! ## Owns
//! - `reference_files_match_group_table`
//! - `reference_files_carry_no_schema_or_parameter_table`
//! - `reference_files_state_no_tool_count`
//! - `reference_tool_list_has_no_continuation_line`
//! - `reference_directory_holds_exactly_the_catalog_files`
//! - `no_two_references_share_a_when_to_use_paragraph`
//!
//! ## Does not own
//! - `SKILL.md` or its router policy (`router_policy`)
//! - The exported-schema validation of the examples (`reference_examples`)
//! - The approved group membership and guidance the files mirror
//!   (`clio-types` `mcp_tool_groups_data`)
//! - The prose of a reference file beyond the four parts listed above
//!
//! ## Boundary
//!
//! Every reference file is read with `include_str!`, so a tool list that drifts
//! from its catalog row fails here instead of reaching a caller, and no run can
//! see a copy other than the one that compiles. Each assertion is anchored to an
//! exact heading line or to the body under one, never to a substring of the
//! whole file, because a substring check reads a sentence that moved into
//! another section, or a key that appears anywhere, as the wording it expects.

use clio_types::{GROUP_TABLE, GroupSpec};

use crate::{body_under, paragraph_under};

/// One reference file, paired with its name, read at compile time so no run can
/// see another copy.
macro_rules! reference {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!("../../../../skills/clio-mcp/references/", $name)),
        )
    };
}

/// Every reference file the router table currently points at, paired with its
/// name, read at compile time so no run can see another copy.
const REFERENCES: [(&str, &str); 17] = [
    reference!("01-retrieval.md"),
    reference!("02-item-write.md"),
    reference!("03-item-read.md"),
    reference!("04-persona.md"),
    reference!("05-task.md"),
    reference!("06-memtree.md"),
    reference!("07-triple.md"),
    reference!("08-belief.md"),
    reference!("09-graph.md"),
    reference!("10-hygiene.md"),
    reference!("11-portability.md"),
    reference!("12-ops.md"),
    reference!("13-retention.md"),
    reference!("14-config.md"),
    reference!("15-ranking.md"),
    reference!("16-sync.md"),
    reference!("17-compliance.md"),
];

/// Words that state an argument schema: a type declaration, a properties map,
/// a required list, or a closed-object marker. A reference file names its
/// arguments nowhere but inside an example call.
const SCHEMA_KEYS: [&str; 4] = ["type:", "properties", "required", "additionalProperties"];

/// The exact heading that opens the when-to-use section of a reference file.
const WHEN_HEADING: &str = "## When to use";

/// The exact heading that opens the exact-arguments section of a reference file.
const ARGUMENTS_HEADING: &str = "## Exact arguments";

/// The exact heading that opens the tool list of a reference file.
pub(crate) const TOOLS_HEADING: &str = "## Tools";

/// The exact heading that opens the example block of a reference file.
pub(crate) const EXAMPLES_HEADING: &str = "## Examples";

/// The one group whose reference file carries no invocation example.
///
/// The compliance tools erase data, so a copied call in that file can destroy
/// the wrong record: the file names its tools and points at `tool_describe`
/// instead. The reason lives here because neither the file nor its heading says
/// it, and every other group keeps the example requirement unchanged.
const NO_EXAMPLE_GROUP: &str = "compliance";

/// The pointer to the tool that reports exact arguments.
const POINTER: &str = "tool_describe";

/// Every reference file zipped with the catalog row its position names.
///
/// The pairing is positional on purpose: a reference file is numbered by its
/// 1-based catalog row, so a reordered catalog changes which file the checks
/// read and the tool set comparison fails rather than passing on stale text. The
/// name is also compared with the name the row derives, so a hand-written list
/// that disagrees with the catalog fails here instead of reading a file the
/// router never points at.
pub(crate) fn rows() -> impl Iterator<Item = (usize, &'static str, &'static str, &'static GroupSpec)>
{
    REFERENCES
        .iter()
        .enumerate()
        .map(|(index, (name, text))| {
            let spec = GROUP_TABLE.get(index).unwrap_or_else(|| {
                panic!(
                    "the reference `{name}` has no `GROUP_TABLE` row at index {index}: number each reference file by the catalog row it serves, because the file and the row must be read as one"
                )
            });
            let derived = crate::reference_examples::derived_name(index);
            assert_eq!(
                *name,
                derived,
                "the reference read at catalog row {} is `{name}` rather than `{derived}`: name each file after its `GROUP_TABLE` row as two digits, a dash and the group id, because a hand-written name reads a file the router never points at",
                index + 1
            );
            (index, *name, *text, spec)
        })
}

/// The one line under `## Tools` that lists a reference file's tools.
///
/// The list is read from the first non-empty line only, and
/// `reference_tool_list_has_no_continuation_line` fails on a second one, so a
/// tool added on a line of its own cannot slip past the ownership rule that
/// reads this line.
pub(crate) fn tools_line<'a>(label: &str, name: &str, text: &'a str) -> &'a str {
    let lines: Vec<&'a str> = text.lines().collect();
    let heading = lines
        .iter()
        .position(|line| *line == TOOLS_HEADING)
        .unwrap_or_else(|| {
            panic!(
                "the reference `{name}` has no `{TOOLS_HEADING}` heading: give the `{label}` file one tool list, because that list ties the file to its catalog row"
            )
        });
    lines[heading + 1..]
        .iter()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| {
            panic!(
                "the reference `{name}` has no tool list under its `{TOOLS_HEADING}` heading: put the one tool list after the heading"
            )
        })
}

/// The backticked tool entries of `tools`, or a failure naming the bare entry.
pub(crate) fn listed_tools<'a>(name: &str, tools: &'a str) -> Vec<&'a str> {
    tools
        .split(',')
        .map(str::trim)
        .map(|entry| {
            entry
                .strip_prefix('`')
                .and_then(|inner| inner.strip_suffix('`'))
                .unwrap_or_else(|| {
                    panic!(
                        "the reference `{name}` has the tool entry `{entry}`: wrap every entry in one pair of backticks and separate the entries with commas, because the catalog names each tool without backticks"
                    )
                })
        })
        .collect()
}

#[test]
fn reference_files_match_group_table() {
    assert_eq!(
        REFERENCES.len(),
        GROUP_TABLE.len(),
        "the check reads {} reference files against {} catalog groups: give every `GROUP_TABLE` row one reference file, because a group with no file leaves a caller holding guidance nothing reads",
        REFERENCES.len(),
        GROUP_TABLE.len()
    );
    for (index, name, text, spec) in rows() {
        let group = spec.group.as_str();
        let row_number = index + 1;

        // The when-to-use sentence is the section body's first paragraph, so a
        // sentence that moved into another section, or a section whose heading
        // was renamed, fails instead of passing on a substring of the file.
        let when = paragraph_under(&format!("the reference `{name}`"), text, WHEN_HEADING);
        assert_eq!(
            when, spec.when_to_use,
            "the `{WHEN_HEADING}` section of `{name}` opens with `{when}` rather than the `{group}` catalog sentence `{}`: put the catalog sentence directly under that exact heading, because the file is the group's one home for that guidance",
            spec.when_to_use
        );

        // Exact tool membership belongs to the named ownership rules, which
        // report one rule at a time instead of one generic set difference. This
        // test keeps the reference count, the derived name, the when-to-use
        // sentence, the pointer, and the example.

        // The pointer lives in its own section, so a file that mentions
        // `tool_describe` in prose elsewhere no longer satisfies the contract.
        let arguments = body_under(&format!("the reference `{name}`"), text, ARGUMENTS_HEADING);
        assert!(
            arguments.contains(POINTER),
            "the `{ARGUMENTS_HEADING}` section of `{name}` reads `{arguments}`, which does not point at `{POINTER}`: keep the pointer inside that section, so a caller can read exact arguments instead of guessing"
        );

        // The compliance file is the one documented exception: its tools erase
        // data, so it names them and points at `tool_describe` rather than
        // showing a call a caller can copy. Every other file keeps the example
        // requirement unchanged.
        if group == NO_EXAMPLE_GROUP {
            continue;
        }

        let fenced = text.split_once(EXAMPLES_HEADING);
        let fenced = fenced.and_then(|(_, after)| after.split_once("```"));
        let fenced = fenced.and_then(|(_, after)| after.split_once("```"));
        assert!(
            fenced.is_some(),
            "the reference `{name}` has no fenced example block under `{EXAMPLES_HEADING}`: keep one pair of triple-backtick fences there, because a caller copies the calls inside them"
        );
        let examples = fenced.map_or("", |(fenced, _)| fenced).lines();
        for example in examples.map(str::trim).filter(|line| !line.is_empty()) {
            let (tool, rest) = example.split_once('(').unwrap_or_default();
            let args = rest.strip_suffix(')').unwrap_or(rest);
            assert!(
                spec.tools.contains(&tool),
                "the example `{example}` in `{name}` calls `{tool}`, which is not in the `{group}` catalog row (row {row_number}): name only that row's tools, because a caller copies the example and a tool outside the group is one the file never promised"
            );
            let parsed = serde_json::from_str::<serde_json::Value>(args);
            assert!(
                parsed.is_ok_and(|value| value.is_object()),
                "the example `{example}` in `{name}` passes `{args}`, which is not one JSON object: pass an object literal, because the tools take named arguments"
            );
        }
    }
}

#[test]
fn reference_files_carry_no_schema_or_parameter_table() {
    for (_, name, text, spec) in rows() {
        let group = spec.group.as_str();
        for key in SCHEMA_KEYS {
            assert!(
                !text.contains(key),
                "the reference `{name}` contains `{key}`, which is argument schema rather than guidance: keep the schema in `tool_describe` and the file to names, examples and a pointer, because a schema copied here goes stale against the tool it describes"
            );
        }
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            assert!(
                !line.trim_start().starts_with('|'),
                "the reference `{name}` line {number} reads `{line}`, a table row: state no parameter table in the `{group}` file, because a table of arguments is the schema `tool_describe` already reports and a second copy is the one that drifts"
            );
        }
    }
}

#[test]
fn reference_files_state_no_tool_count() {
    // The count is checked as a digit token outside a fenced example block, so
    // an example that happens to carry the same number does not decide the
    // result. Number words are out of scope here: a word scan would fail on
    // ordinary prose ("one of the tools") without catching a real count, so the
    // gate covers the form a count is actually written in.
    for (_, name, text, spec) in rows() {
        let group = spec.group.as_str();
        let count = spec.tools.len();
        let mut fenced = false;
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                continue;
            }
            for token in line
                .split(|character: char| !character.is_ascii_digit())
                .filter(|token| !token.is_empty())
            {
                assert!(
                    token.parse::<usize>().ok() != Some(count),
                    "the reference `{name}` line {number} reads `{line}`, whose `{token}` is the number of tools in the `{group}` group: state no tool count, because the count lives in the catalog row and a restated one goes stale the moment that row changes"
                );
            }
        }
    }
}

#[test]
fn reference_tool_list_has_no_continuation_line() {
    for (_, name, text, spec) in rows() {
        let group = spec.group.as_str();
        let tools = tools_line(group, name, text);
        let lines: Vec<&str> = text.lines().collect();
        let heading = lines
            .iter()
            .position(|line| *line == TOOLS_HEADING)
            .expect("`tools_line` guarantees the heading");
        let list = lines
            .iter()
            .position(|line| line.trim() == tools)
            .expect("`tools_line` guarantees the list line");
        assert!(
            list > heading,
            "the tool list `{tools}` in `{name}` was found above its `{TOOLS_HEADING}` heading: keep the list under the heading so the two are read as one"
        );
        let continuation = lines[list + 1..]
            .iter()
            .map(|line| line.trim())
            .find(|line| !line.is_empty());
        if let Some(line) = continuation {
            assert!(
                line.starts_with("## "),
                "the reference `{name}` continues its `{TOOLS_HEADING}` list with `{line}`: keep the list on one comma-separated line, because a second line is a second list that the set comparison above reads as one tool fewer than the file names"
            );
        }
    }
}

#[test]
fn reference_directory_holds_exactly_the_catalog_files() {
    // The files are read from the directory as well as at compile time, so a
    // file the router points at and a file the directory holds cannot disagree:
    // an extra, missing or twice-named file fails here.
    let directory = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../skills/clio-mcp/references"
    );
    let names: Vec<String> = std::fs::read_dir(directory)
        .unwrap_or_else(|error| {
            panic!("the references directory `{directory}` cannot be read: {error}")
        })
        .map(|entry| {
            entry
                .unwrap_or_else(|error| {
                    panic!("the references directory `{directory}` has an unreadable entry: {error}")
                })
                .file_name()
                .into_string()
                .unwrap_or_else(|name| {
                    panic!("the references directory `{directory}` holds `{name:?}`, a name that is not UTF-8")
                })
        })
        .collect();
    if let Err(difference) = crate::reference_examples::check_reference_set(&names) {
        panic!(
            "the references directory `{directory}` {difference}: keep exactly one reference file per catalog group, because a file nothing routes to is guidance a caller never reads and a missing one is a group with no guidance"
        );
    }
    assert_eq!(
        names.len(),
        GROUP_TABLE.len(),
        "the references directory holds {} files against {} catalog groups and the derived names already match: the directory is clean and the catalog moved, so add the row and its file together",
        names.len(),
        GROUP_TABLE.len()
    );
}

#[test]
fn no_two_references_share_a_when_to_use_paragraph() {
    // Two files that open with the same paragraph are two homes for one piece of
    // guidance, and a caller reads whichever the router sends it to. The catalog
    // sentence is compared as a whole, so a paraphrase is not a duplicate here.
    let mut seen: Vec<(String, String)> = Vec::new();
    for (_, name, text, _) in rows() {
        let when = paragraph_under(&format!("the reference `{name}`"), text, WHEN_HEADING);
        if let Some((other, _)) = seen.iter().find(|(_, paragraph)| *paragraph == when) {
            panic!(
                "the references `{other}` and `{name}` both open their `{WHEN_HEADING}` section with `{when}`: give each group its own guidance, because two files carrying one paragraph leave a caller reading the same words for two different groups"
            );
        }
        seen.push((name.to_owned(), when));
    }
}
