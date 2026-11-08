// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding `skills/clio-mcp/SKILL.md` to the
//! router policy it claims: an identity and a negative trigger, one table row
//! per catalog group, no individual tool named in that table, one copy of each
//! call rule, one retrieval-call budget, and a version check that changes
//! nothing.
//!
//! ## Owns
//! - `metadata_declares_the_identity_and_negative_trigger`
//! - `router_has_one_valid_row_per_catalog_group`
//! - `router_table_names_no_individual_tool`
//! - `every_call_rule_appears_exactly_once`
//! - `four_call_budget_appears_once_and_scopes_only_retrieval`
//! - `version_check_runs_once_and_applies_no_update`
//!
//! ## Does not own
//! - The group reference files or their contents (`group_references`)
//! - The approved group membership and guidance the table mirrors
//!   (`clio-types` `mcp_tool_groups_data`)
//! - Advertising, listing, or dispatching a tool (`clio-mcp`)
//!
//! ## Boundary
//!
//! The skill text and its one section reader are declared by the target module,
//! so this module has no working directory, clock, or network input of its own.
//! A change to the skill's router wording belongs here only when it moves one of
//! the policy sentences below; a membership change belongs in
//! `mcp_tool_groups_data`, which the table is compared against.

use clio_types::GROUP_TABLE;

use crate::{SKILL, body_under};

/// The one frontmatter `name` the skill may declare.
const SKILL_NAME: &str = "clio-mcp";

/// The one frontmatter `description` the skill may declare: one trigger
/// sentence and one negative trigger, with no further claim beside them.
const SKILL_DESCRIPTION: &str = "Use when the task needs Clio memory: store, retrieve, update, persona, tasks, graph, or maintenance. DO NOT TRIGGER on a greenfield turn that needs no prior state.";

/// The label used when a failure names the file it read.
const SKILL_LABEL: &str = "the skill `skills/clio-mcp/SKILL.md`";

/// The router table's header line, spelled exactly as the table must spell it.
const TABLE_HEADER: &str = "| Group | When you need | Read |";

/// The router table's separator line, which must follow the header directly.
const TABLE_SEPARATOR: &str = "|---|---|---|";

/// The two operations the per-turn retrieval budget scopes.
const BUDGETED_OPERATIONS: [&str; 2] = ["`retrieve`", "`compose_context`"];

/// The whole `## Turn budget` body, trimmed: one sentence with the value and a
/// second that scopes it to retrieval, so a third sentence cannot join them.
const BUDGET_BODY: &str = "At most four `retrieve` and `compose_context` calls combined are allowed per turn. This budget does not cap any other tool.";

/// The whole `## End-of-session check` body, trimmed: one read that reports and
/// applies nothing, so an added sentence cannot turn the check into a mutation.
const CHECK_BODY: &str = "At session end, run `clio version --check` once. Repeat its one result line word for word. Report a newer release when the line names one. Report the check as unavailable when the line names no release. Apply no update.";

/// Every call rule the router states, each exactly once.
const CALL_RULES: [&str; 6] = [
    "Read only the group reference file you need.",
    "Call `tool_describe(name)` for exact arguments.",
    "If the reference and `tool_describe` disagree, trust `tool_describe`.",
    "Call a listed tool directly; call a hidden tool with `tool_invoke`.",
    "If you cannot find the tool, call `tool_search`.",
    "Do not read every reference file as a discovery step.",
];

/// Claims that a client loads the skill body without being asked to.
const NO_CLIENT_LOADING: [&str; 6] = [
    "auto-load",
    "autoload",
    "automatically load",
    "loads automatically",
    "loaded automatically",
    "loads the body automatically",
];

/// Claims that the end-of-session check applies an update rather than reporting
/// one. Reporting a newer release stays allowed, so neither `update clio` nor
/// `tell the user to update` is banned.
const NO_UPDATE: [&str; 6] = [
    "apply the update",
    "apply an update",
    "install the update",
    "run the update",
    "clio update",
    "upgrade clio",
];

/// One frontmatter value by key, or a failure naming the missing key.
fn frontmatter_value(key: &str) -> &'static str {
    let prefix = format!("{key}:");
    SKILL
        .lines()
        .skip(1)
        .take_while(|line| *line != "---")
        .find_map(|line| line.strip_prefix(&prefix))
        .map_or_else(
            || {
                panic!("the skill frontmatter has no `{key}:` line: declare it so the router states its identity")
            },
            str::trim,
        )
}

/// The router table's data rows, in file order, without the header or separator.
///
/// The scan starts only after the exact header and its exact separator, so a
/// renamed column, a row placed above the header, or a second table cannot pass
/// as a router row.
fn data_rows() -> Vec<&'static str> {
    let lines: Vec<&'static str> = SKILL.lines().collect();
    let headers = lines.iter().filter(|line| **line == TABLE_HEADER).count();
    assert_eq!(
        headers, 1,
        "the skill has {headers} lines equal to `{TABLE_HEADER}` rather than one: restore the router table's three-column header verbatim, because every row is read from below that line"
    );
    let header = lines
        .iter()
        .position(|line| *line == TABLE_HEADER)
        .expect("the header count of one guarantees the header line");
    assert_eq!(
        lines.get(header + 1).copied(),
        Some(TABLE_SEPARATOR),
        "the line after `{TABLE_HEADER}` is `{:?}` rather than `{TABLE_SEPARATOR}`: keep the separator directly under the header so the header and the rows stay one table",
        lines.get(header + 1)
    );
    let first = header + 2;
    let rows: Vec<&'static str> = lines[first..]
        .iter()
        .copied()
        .take_while(|line| line.starts_with('|'))
        .collect();
    let tail = first + rows.len();
    for line in &lines[tail..] {
        assert!(
            !line.trim_start().starts_with('|'),
            "the skill has the table row `{line}` after the router table ends: keep every table row in the one table, because a second table is a second router"
        );
    }
    rows
}

/// A table cell with one pair of surrounding backticks removed.
fn unbackticked(cell: &str) -> &str {
    cell.strip_prefix('`')
        .and_then(|inner| inner.strip_suffix('`'))
        .unwrap_or(cell)
}

/// How many whole words in the skill equal `word`, case-insensitively.
fn word_count(word: &str) -> usize {
    SKILL
        .split(|character: char| !character.is_alphanumeric())
        .filter(|piece| piece.eq_ignore_ascii_case(word))
        .count()
}

#[test]
fn metadata_declares_the_identity_and_negative_trigger() {
    assert!(
        SKILL.starts_with("---\n"),
        "the skill does not open with a `---` frontmatter fence: start the file with the fence, because metadata outside the frontmatter is prose a client never reads as identity"
    );
    let name = frontmatter_value("name");
    assert_eq!(
        name, SKILL_NAME,
        "the skill's frontmatter `name` is `{name}`, not `{SKILL_NAME}`: name the skill after its directory"
    );
    let description = frontmatter_value("description");
    assert_eq!(
        description, SKILL_DESCRIPTION,
        "the frontmatter `description` is `{description}`, not `{SKILL_DESCRIPTION}`: keep the one trigger sentence and the one negative trigger, because a client reads this line first and an added claim rides along unread"
    );
    let lowered = SKILL.to_lowercase();
    for claim in NO_CLIENT_LOADING {
        assert!(
            !lowered.contains(claim),
            "the skill says `{claim}`, which claims a client loads the body by itself: remove it, because loading the skill is the caller's decision"
        );
    }
}

#[test]
fn router_has_one_valid_row_per_catalog_group() {
    let rows = data_rows();
    assert_eq!(
        rows.len(),
        GROUP_TABLE.len(),
        "the router table has {} data rows against {} catalog groups: the table must mirror `GROUP_TABLE`",
        rows.len(),
        GROUP_TABLE.len()
    );
    for (index, (row, spec)) in rows.iter().zip(GROUP_TABLE).enumerate() {
        let row_number = index + 1;
        let fields = row.split('|').count();
        assert_eq!(
            fields, 5,
            "table row {row_number} is `{row}` and splits on `|` into {fields} fields rather than 5: keep three columns between the two edge separators, because a fourth cell is a fourth column nothing here validates"
        );
        let id = spec.group.as_str();
        let opening = format!("| {id} |");
        assert!(
            row.starts_with(&opening),
            "table row {row_number} is `{row}` and does not open with `{opening}`: keep `GROUP_TABLE` order and spell the id the way `ToolGroup::as_str` does"
        );
        assert!(
            row.contains(spec.when_to_use),
            "table row {row_number} for `{id}` does not carry the catalog sentence `{}`: copy `when_to_use` from `GROUP_TABLE` verbatim",
            spec.when_to_use
        );
        let reference = format!("`references/{row_number:02}-{id}.md`");
        assert!(
            row.contains(&reference),
            "table row {row_number} for `{id}` does not point at {reference}: number each reference file by its 1-based row index"
        );
    }
}

#[test]
fn router_table_names_no_individual_tool() {
    // A scan of the bare row collides with the catalog's own wording: `correct`
    // is a word of item-write, `inspect` of item-read and belief, `summarize`
    // of item-read, `verify` and `repair` of ops, and the substrings `store`
    // (in "stored"), `import` and `export` (in "imports" and "exports") each
    // spell a tool. A whole-word scan keeps the last three and still fails on
    // the first five, so each row is scanned with its one approved sentence
    // masked out instead: the group cell, the reference cell, and any text
    // added around the sentence all still fail on a tool name.
    let tools: Vec<&'static str> = GROUP_TABLE
        .iter()
        .flat_map(|spec| spec.tools.iter().copied())
        .collect();
    for (index, (row, spec)) in data_rows().iter().zip(GROUP_TABLE).enumerate() {
        let row_number = index + 1;
        let cells: Vec<&str> = row.split('|').map(str::trim).map(unbackticked).collect();
        let masked = row.replace(spec.when_to_use, "");
        for tool in &tools {
            assert!(
                !masked.contains(tool),
                "table row {row_number} is `{row}`, which names the individual tool `{tool}` outside the catalog sentence `{}`: a row names a group and a reference file, never a tool",
                spec.when_to_use
            );
            assert!(
                !cells.contains(tool),
                "table row {row_number} is `{row}`, whose cell `{tool}` names an individual tool: keep the table at group granularity and leave `{tool}` to its group reference file"
            );
        }
    }
}

#[test]
fn every_call_rule_appears_exactly_once() {
    for rule in CALL_RULES {
        let count = SKILL.matches(rule).count();
        assert_eq!(
            count, 1,
            "the call rule `{rule}` appears {count} times: keep exactly one copy so the router states each rule once"
        );
    }
}

#[test]
fn four_call_budget_appears_once_and_scopes_only_retrieval() {
    let body = body_under(SKILL_LABEL, SKILL, "## Turn budget");
    assert_eq!(
        body, BUDGET_BODY,
        "the `## Turn budget` section reads `{body}` rather than `{BUDGET_BODY}`: keep the one budget sentence with its one value and its scope, because an added sentence, a widened scope, a changed value, or a duplicate all change what a turn may call"
    );
    for operation in BUDGETED_OPERATIONS {
        let count = SKILL.matches(operation).count();
        assert_eq!(
            count, 1,
            "the budgeted operation {operation} appears {count} times: the budget names each of the two operations once and no other"
        );
    }
    let fours = word_count("four");
    assert_eq!(
        fours, 1,
        "the word `four` appears {fours} times: one budget sentence carries the one value, so a changed or repeated budget fails here"
    );
}

#[test]
fn version_check_runs_once_and_applies_no_update() {
    let body = body_under(SKILL_LABEL, SKILL, "## End-of-session check");
    assert_eq!(
        body, CHECK_BODY,
        "the `## End-of-session check` section reads `{body}` rather than `{CHECK_BODY}`: keep the check to one read that reports and applies nothing, because an added sentence can widen it into a mutation"
    );
    let check = "`clio version --check`";
    let checks = SKILL.matches(check).count();
    assert_eq!(
        checks, 1,
        "the command {check} appears {checks} times: run the check once at session end"
    );
    let lowered = SKILL.to_lowercase();
    for claim in NO_UPDATE {
        assert!(
            !lowered.contains(claim),
            "the skill says `{claim}`, which turns the check into a mutation: remove it, because the check reports and applies no update"
        );
    }
}
