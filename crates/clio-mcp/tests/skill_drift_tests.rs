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
//! - The wording of the skill, its frontmatter, or its reference files
//! - The approved group membership and guidance the table mirrors
//!   (`clio-types` `mcp_tool_groups_data`)
//! - Advertising, listing, or dispatching a tool (`clio-mcp`)
//!
//! ## Boundary
//!
//! The skill is read with `include_str!`, so this check has no working
//! directory, clock, or network input and cannot read a stale copy. A change to
//! the skill's router wording belongs here only when it moves one of the policy
//! sentences above; a membership change belongs in `mcp_tool_groups_data`,
//! which the table is compared against.

use clio_types::GROUP_TABLE;

/// The skill under check, read at compile time so no run can see another copy.
const SKILL: &str = include_str!("../../../skills/clio-mcp/SKILL.md");

/// The two operations the per-turn retrieval budget scopes.
const BUDGETED_OPERATIONS: [&str; 2] = ["`retrieve`", "`compose_context`"];

/// The one sentence that states the per-turn retrieval budget.
const BUDGET_SENTENCE: &str =
    "At most four `retrieve` and `compose_context` calls combined are allowed per turn.";

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
const NO_CLIENT_LOADING: [&str; 4] = [
    "auto-load",
    "autoload",
    "automatically loads",
    "loads the body automatically",
];

/// Claims that the end-of-session check changes the installation.
const NO_UPDATE: [&str; 5] = [
    "apply the update",
    "install the update",
    "run the update",
    "upgrade clio",
    "update clio",
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
fn data_rows() -> Vec<&'static str> {
    SKILL
        .lines()
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .filter(|line| !line.starts_with("| Group |") && !line.starts_with("|---"))
        .collect()
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
    assert_eq!(
        frontmatter_value("name"),
        "clio-mcp",
        "the skill's frontmatter `name` is `{}`, not `clio-mcp`: name the skill after its directory",
        frontmatter_value("name")
    );
    let description = frontmatter_value("description");
    let trigger = "DO NOT TRIGGER on a greenfield turn that needs no prior state.";
    assert!(
        description.contains(trigger),
        "the frontmatter `description` is `{description}` and lacks `{trigger}`: keep the negative trigger in the description a client reads first"
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
        17,
        "the router table has {} data rows rather than 17: give every catalog group exactly one row",
        rows.len()
    );
    assert_eq!(
        rows.len(),
        GROUP_TABLE.len(),
        "the router table has {} data rows against {} catalog groups: the table must mirror `GROUP_TABLE`",
        rows.len(),
        GROUP_TABLE.len()
    );
    for (index, (row, spec)) in rows.iter().zip(GROUP_TABLE).enumerate() {
        let row_number = index + 1;
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
    let tools: Vec<&'static str> = GROUP_TABLE
        .iter()
        .flat_map(|spec| spec.tools.iter().copied())
        .collect();
    for (index, row) in data_rows().iter().enumerate() {
        let row_number = index + 1;
        let cells: Vec<&str> = row.split('|').map(str::trim).map(unbackticked).collect();
        for tool in &tools {
            assert!(
                !cells.contains(tool),
                "table row {row_number} is `{row}`, whose cell `{tool}` names an individual tool: keep the table at group granularity and leave `{tool}` to its group reference file"
            );
            let backticked = format!("`{tool}`");
            assert!(
                !row.contains(&backticked),
                "table row {row_number} is `{row}`, which names the individual tool `{tool}`: a row names a group and a reference file, never a tool"
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
    let budgets = SKILL.matches(BUDGET_SENTENCE).count();
    assert_eq!(
        budgets, 1,
        "the budget sentence `{BUDGET_SENTENCE}` appears {budgets} times: keep one sentence so the cap has one value"
    );
    for operation in BUDGETED_OPERATIONS {
        let count = SKILL.matches(operation).count();
        assert_eq!(
            count, 1,
            "the budgeted operation {operation} appears {count} times: the budget names each of the two operations once and no other"
        );
    }
    let scope = "This budget does not cap any other tool.";
    assert!(
        SKILL.contains(scope),
        "the skill lacks `{scope}`: state the budget's scope so no reader applies it to another tool"
    );
    let fours = word_count("four");
    assert_eq!(
        fours, 1,
        "the word `four` appears {fours} times: one budget sentence carries the one value, so a changed or repeated budget fails here"
    );
}

#[test]
fn version_check_runs_once_and_applies_no_update() {
    let check = "`clio version --check`";
    let checks = SKILL.matches(check).count();
    assert_eq!(
        checks, 1,
        "the command {check} appears {checks} times: run the check once at session end"
    );
    let no_update = "Apply no update.";
    assert!(
        SKILL.contains(no_update),
        "the skill lacks `{no_update}`: state that the check reads and changes nothing"
    );
    let lowered = SKILL.to_lowercase();
    for claim in NO_UPDATE {
        assert!(
            !lowered.contains(claim),
            "the skill says `{claim}`, which turns the check into a mutation: remove it, because the check reports and applies no update"
        );
    }
}
