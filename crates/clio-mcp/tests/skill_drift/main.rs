// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for declaring the drift target's three check
//! modules and for the inputs they share: the skill text and the section
//! readers every structural assertion anchors to. The router policy lives in
//! `router_policy`; the group reference files live in `group_references`; the
//! reference file examples live in `reference_examples`.
//!
//! ## Owns
//! - The target's module declarations
//! - `SKILL`, the skill under check, read at compile time
//! - `body_under`, the single section reader all three modules call
//!
//! ## Does not own
//! - The router policy checks (`router_policy`)
//! - The group reference checks (`group_references`)
//! - The reference example checks (`reference_examples`)
//! - The wording of the skill, its frontmatter, or the prose of its reference
//!   files
//!
//! ## Boundary
//!
//! The skill and the reference files are read with `include_str!`, so no check
//! has a working directory, clock, or network input and none can read a stale
//! copy. A change to a check belongs in the module that owns it; a
//! change to how a section body is read belongs here, because all three
//! modules must read the same slice the same way.
//!
//! Every structural assertion in all three modules is anchored to an exact
//! line or an exact section body rather than to a substring, because a
//! substring check reads a widened sentence, an added column, or a second
//! table as the wording it expects. `body_under` is the only section reader,
//! so a check that needs a section body compares against exactly the lines
//! under that heading.

mod group_references;
mod reference_examples;
mod router_policy;

/// The skill under check, read at compile time so no run can see another copy.
const SKILL: &str = include_str!("../../../../skills/clio-mcp/SKILL.md");

/// The trimmed body under the exact `heading` line in `text`, up to the next
/// `## ` heading, or a failure naming `label` and the heading it lacks.
///
/// A check that needs a section body compares against this slice rather than
/// against the whole file, so a sentence that moved into another section fails
/// instead of passing on a `contains` over everything.
fn body_under(label: &str, text: &str, heading: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|line| *line == heading).map_or_else(
        || {
            panic!(
                "{label} has no `{heading}` heading: keep the section so its guidance has one home"
            )
        },
        |index| index + 1,
    );
    lines[start..]
        .iter()
        .copied()
        .take_while(|line| !line.starts_with("## "))
        .collect::<Vec<&str>>()
        .join("\n")
        .trim()
        .to_string()
}

/// The trimmed first paragraph under the exact `heading` line in `text`.
///
/// This is `body_under` cut at the first blank line, so a reference file may
/// carry further paragraphs below its catalog sentence while the sentence
/// itself is still compared as the one paragraph directly under the heading. A
/// file whose heading was renamed, or whose body opens with a different
/// sentence, fails here rather than passing on a substring of the file.
fn paragraph_under(label: &str, text: &str, heading: &str) -> String {
    body_under(label, text, heading)
        .lines()
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<&str>>()
        .join("\n")
        .trim()
        .to_string()
}
