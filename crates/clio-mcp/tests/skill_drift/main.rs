// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for declaring the drift target's five check
//! modules and for the inputs they share: the skill text and the section
//! readers every structural assertion anchors to. The router policy lives in
//! `router_policy`; the group reference files live in `group_references`; the
//! reference file examples live in `reference_examples`; the ownership rule
//! lives in `ownership`; the catalog hash check lives in `catalog_hash`.
//!
//! ## Owns
//! - The target's module declarations
//! - `SKILL`, the skill under check: the embedded first asset, so the text the
//!   check reads is the text the crate ships
//! - `body_under`, the section reader the group reference, reference example,
//!   and router policy checks call
//! - The catalog hash drift check (`catalog_hash`)
//!
//! ## Does not own
//! - The router policy checks (`router_policy`)
//! - The group reference checks (`group_references`)
//! - The reference example checks (`reference_examples`)
//! - The ownership rule over the reference tool lists (`ownership`)
//! - The wording of the skill, its frontmatter, or the prose of its reference
//!   files
//!
//! ## Boundary
//!
//! The skill and the reference files are the assets `build.rs` embeds, so no
//! check has a working directory, clock, or network input and none can read a
//! stale copy. A change to a check belongs in the module that owns it; a change to
//! how a section body is read belongs here, because the group reference,
//! reference example, and router policy checks must read the same slice the
//! same way.
//!
//! Every structural assertion in these checks is anchored to an exact line or
//! an exact section body rather than to a substring, because a substring check
//! reads a widened sentence, an added column, or a second table as the wording
//! it expects. `body_under` is the only function that reads a section body, and
//! `paragraph_under` cuts it at the first blank line: the group reference,
//! reference example, and router policy modules call it, `ownership` reads the
//! tool list line through `tools_line` instead, and `catalog_hash` reads no
//! section at all.

mod catalog_hash;
mod group_references;
mod ownership;
mod reference_examples;
mod router_policy;

/// The skill under check: the router file, the first asset `build.rs` embedded,
/// so no run can see a copy other than the one the crate ships.
const SKILL: &str = clio_mcp::skill_assets::SKILL_ASSETS[0].1;

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
