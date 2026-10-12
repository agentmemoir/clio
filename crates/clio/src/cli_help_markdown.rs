// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Markdown help-page renderer and the
//! page-name/link map it reads: a command record in, Markdown text out.
//!
//! ## Owns
//! - `render_command_page`, one record's Markdown page: the title, summary,
//!   usage fence, description paragraphs, arguments, options with kinds and
//!   defaults, the accepted globals, examples, aliases, and See-also links
//! - `PageMap`, the deterministic collision-checked command-name → page-file
//!   map with its index link, its parent link, and its See-also targets
//! - `table_cell` and `code_fence`, the shared Markdown escaping helpers
//!
//! ## Does not own
//! - The record model and its fields (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The decision of which pages exist and the writing of files (the help
//!   surface that calls this module owns that)
//!
//! ## Boundary
//!
//! Changes to how a Markdown help page is written should normally require
//! changes here. Changes to one command's flags should not.
//!
//! Keep this module focused on the Markdown rendering itself.

use crate::cli_command_registry;
use crate::cli_command_spec::{CommandSpec, Flag, ValueKind};
use std::collections::BTreeMap;

/// The index page's file name: the fallback a derived parent links to when it
/// has no page of its own.
const INDEX_PAGE: &str = "index.md";

/// One command's Markdown help page.
///
/// The page is the title, the summary, the usage fence, one paragraph per
/// note, the arguments table, the options table, the accepted globals' table,
/// the examples, the aliases, and the See-also links. A section with nothing
/// to show is omitted, so no heading stands over an empty list. A shared flag
/// is defined once in the registry and rendered where needed: an
/// `accepted_extra` name is looked up in the declared globals and never
/// re-declared here.
///
/// # Panics
/// Panics when `accepted_extra` names something that is not a declared global
/// flag, the same fail-closed rule `options_help` applies: an unvalidated
/// record must not render a flag the registry never declared.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn render_command_page(spec: &CommandSpec, pages: &PageMap) -> String {
    let mut page = String::new();
    page.push_str("# clio ");
    page.push_str(spec.name);
    page.push_str("\n\n");
    page.push_str(spec.summary);
    page.push('\n');
    push_usage(&mut page, spec);
    push_description(&mut page, spec);
    push_arguments(&mut page, spec);
    let documented: Vec<&Flag> = spec.documented.iter().collect();
    push_options(&mut page, "Options", &documented);
    let globals: Vec<&Flag> = spec
        .accepted_extra
        .iter()
        .map(|name| {
            cli_command_registry::global_flag(name)
                .expect("accepted_extra names a declared global flag")
        })
        .collect();
    push_options(&mut page, "Global options", &globals);
    push_examples(&mut page, spec);
    push_aliases(&mut page, spec);
    push_see_also(&mut page, spec, pages);
    page
}

/// The usage section: the usage line and every alternate inside one fenced
/// code block.
#[cfg_attr(not(test), allow(dead_code))]
fn push_usage(page: &mut String, spec: &CommandSpec) {
    let mut usage = spec.usage_line();
    for alternate in spec.usage_alternates {
        usage.push('\n');
        usage.push_str(alternate);
    }
    page.push_str("\n## Usage\n\n");
    page.push_str(&code_fence(&usage));
}

/// The description section: one paragraph per note.
#[cfg_attr(not(test), allow(dead_code))]
fn push_description(page: &mut String, spec: &CommandSpec) {
    if spec.notes.is_empty() {
        return;
    }
    page.push_str("\n## Description\n\n");
    for (position, note) in spec.notes.iter().enumerate() {
        if position > 0 {
            page.push('\n');
        }
        page.push_str(note);
        page.push('\n');
    }
}

/// The arguments section: one table row per positional.
#[cfg_attr(not(test), allow(dead_code))]
fn push_arguments(page: &mut String, spec: &CommandSpec) {
    if spec.positionals.is_empty() {
        return;
    }
    page.push_str("\n## Arguments\n\n");
    page.push_str("| Argument | Required | Description |\n");
    page.push_str("| --- | --- | --- |\n");
    for positional in spec.positionals {
        let required = if positional.required { "yes" } else { "no" };
        page.push_str("| `");
        page.push_str(&table_cell(positional.label));
        page.push_str("` | ");
        page.push_str(required);
        page.push_str(" | ");
        page.push_str(&table_cell(positional.description));
        page.push_str(" |\n");
    }
}

/// One options table: `heading` is `Options` for the record's own flags and
/// `Global options` for the accepted globals, and both share the same row
/// shape. A flag whose long spelling is already shown prints once.
#[cfg_attr(not(test), allow(dead_code))]
fn push_options(page: &mut String, heading: &str, flags: &[&Flag]) {
    if flags.is_empty() {
        return;
    }
    page.push_str("\n## ");
    page.push_str(heading);
    page.push_str("\n\n");
    page.push_str("| Option | Kind | Default | Description |\n");
    page.push_str("| --- | --- | --- | --- |\n");
    let mut shown: Vec<&str> = Vec::new();
    for flag in flags {
        if shown.contains(&flag.long) {
            continue;
        }
        shown.push(flag.long);
        let default = flag
            .default
            .map(|default| format!("`{}`", table_cell(default)))
            .unwrap_or_default();
        page.push_str("| `");
        page.push_str(&table_cell(&flag_option_token(flag)));
        page.push_str("` | ");
        page.push_str(&table_cell(&flag_kind(flag)));
        page.push_str(" | ");
        page.push_str(&default);
        page.push_str(" | ");
        page.push_str(&table_cell(flag.description));
        page.push_str(" |\n");
    }
}

/// One flag's option cell: the long spelling, then the value token the usage
/// line prints — the placeholder override when one is set, bracketed when the
/// value may be omitted.
fn flag_option_token(flag: &Flag) -> String {
    let mut token = format!("--{}", flag.long);
    if let Some(value) = flag.value {
        let placeholder = flag.placeholder_override.unwrap_or(value.placeholder);
        if flag.value_optional {
            token.push_str(" [");
            token.push_str(placeholder);
            token.push(']');
        } else {
            token.push(' ');
            token.push_str(placeholder);
        }
    }
    token
}

/// One flag's kind cell: the value kind as the catalog spells it, with an
/// enum's allowed values, and empty for a boolean flag.
fn flag_kind(flag: &Flag) -> String {
    let Some(value) = flag.value else {
        return String::new();
    };
    match value.kind {
        ValueKind::Text => String::from("text"),
        ValueKind::Uint => String::from("uint"),
        ValueKind::Float => String::from("float"),
        ValueKind::Enum(allowed) => format!("enum: {}", allowed.join(", ")),
    }
}

/// The examples section: one comment line and one fenced command per example.
#[cfg_attr(not(test), allow(dead_code))]
fn push_examples(page: &mut String, spec: &CommandSpec) {
    if spec.examples.is_empty() {
        return;
    }
    page.push_str("\n## Examples\n\n");
    for (position, example) in spec.examples.iter().enumerate() {
        if position > 0 {
            page.push('\n');
        }
        if let Some(comment) = example.comment {
            page.push_str(comment);
            page.push('\n');
        }
        page.push_str(&code_fence(example.command));
    }
}

/// The aliases section: the alternate spellings that reach this command.
#[cfg_attr(not(test), allow(dead_code))]
fn push_aliases(page: &mut String, spec: &CommandSpec) {
    if spec.aliases.is_empty() {
        return;
    }
    page.push_str("\n## Aliases\n\n");
    let spellings: Vec<String> = spec
        .aliases
        .iter()
        .map(|alias| format!("`{alias}`"))
        .collect();
    page.push_str(&spellings.join(", "));
    page.push('\n');
}

/// The See-also section: one link per target the page map returns.
#[cfg_attr(not(test), allow(dead_code))]
fn push_see_also(page: &mut String, spec: &CommandSpec, pages: &PageMap) {
    let links = pages.see_also(spec.name);
    if links.is_empty() {
        return;
    }
    page.push_str("\n## See also\n\n");
    for (label, target) in links {
        page.push_str("- [");
        page.push_str(&label);
        page.push_str("](");
        page.push_str(&target);
        page.push_str(")\n");
    }
}

/// Any table cell's content: raw newlines become spaces, because a GFM table
/// cell cannot hold one, and every literal pipe becomes `\|`, so the cell
/// cannot break the table. The escape is applied even inside code spans,
/// where GFM still honors it in table cells.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn table_cell(text: &str) -> String {
    text.replace(['\r', '\n'], " ").replace('|', "\\|")
}

/// `content` inside one fenced code block whose fence is longer than any
/// backtick run the content holds: a backslash cannot protect a backtick
/// inside a code span, so the fence length is the only protection, and a
/// fence shorter than the content's longest run would close the block early.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn code_fence(content: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for character in content.chars() {
        if character == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let ticks = "`".repeat(3.max(longest + 1));
    format!("{ticks}\n{content}\n{ticks}\n")
}

/// The page-file map over a set of command names: deterministic,
/// collision-checked filenames, plus the index link, the parent link, and the
/// See-also link targets a page reads.
///
/// The map holds only `BTreeMap` and sorted input, so nothing it returns ever
/// follows registration or hash order.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct PageMap {
    pages: BTreeMap<String, String>,
}

impl PageMap {
    /// Build the map for `names`, sorted, so the output is stable for the same
    /// name set whatever order the names arrive in.
    ///
    /// A page file is the command name with spaces as hyphens plus `.md`. When
    /// two names reach the same file, the later one in sorted order gains
    /// `-2`, then `-3`, and so on, so both stay addressable.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new<'a, I>(names: I) -> Self
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut ordered: Vec<&str> = names.into_iter().collect();
        ordered.sort_unstable();
        let mut pages = BTreeMap::new();
        let mut taken: Vec<String> = vec![String::from(INDEX_PAGE)];
        for name in ordered {
            let stem = name.replace(' ', "-");
            let mut suffix = 1;
            loop {
                let file = if suffix == 1 {
                    format!("{stem}.md")
                } else {
                    format!("{stem}-{suffix}.md")
                };
                if !taken.contains(&file) {
                    taken.push(file.clone());
                    pages.insert(String::from(name), file);
                    break;
                }
                suffix += 1;
            }
        }
        Self { pages }
    }

    /// The page file for `name`, when the map holds one.
    pub(crate) fn page(&self, name: &str) -> Option<&str> {
        self.pages.get(name).map(String::as_str)
    }

    /// The index page's file name, the link target every page-less name falls
    /// back to.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn index_page() -> &'static str {
        INDEX_PAGE
    }

    /// The link target for `name`'s derived parent — the name without its last
    /// token: the parent's page when the map holds one, else the index page,
    /// because a parent that is only a group prefix has no page of its own.
    /// `None` when `name` is a single token and has no parent.
    pub(crate) fn parent_link(&self, name: &str) -> Option<String> {
        let (parent, _) = name.rsplit_once(' ')?;
        Some(self.page(parent).unwrap_or(Self::index_page()).to_owned())
    }

    /// The See-also link targets for `name`: its parent's link first, then the
    /// sibling commands — pages whose own derived parent is the same parent,
    /// so a deeper name that only shares the prefix is left out — that hold
    /// pages, sorted. Each pair is `(label, target)`, the label the command
    /// name as typed and the target always a page the map holds or the index
    /// page.
    pub(crate) fn see_also(&self, name: &str) -> Vec<(String, String)> {
        let Some((parent, _)) = name.rsplit_once(' ') else {
            return Vec::new();
        };
        let target = self
            .parent_link(name)
            .expect("a multi-token name has a parent link");
        let mut links = vec![(String::from(parent), target)];
        for (other, file) in self.pages.range(parent.to_owned()..) {
            let is_sibling = other
                .rsplit_once(' ')
                .is_some_and(|(other_parent, _)| other_parent == parent);
            if *other != name && is_sibling {
                links.push((other.clone(), file.clone()));
            }
        }
        links
    }
}

#[cfg(test)]
#[path = "cli_help_markdown_tests.rs"]
mod tests;
