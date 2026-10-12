// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Markdown help-page renderer: the page sections and
//! their order, the table-cell and code-fence escaping, the shared global
//! flags rendered from the registry, and the page map's deterministic,
//! collision-checked filenames and link targets.
//!
//! ## Owns
//! - Assertions over `render_command_page`, `table_cell`, `code_fence`, and
//!   `PageMap`
//!
//! ## Does not own
//! - The rendering under test (the `cli_help_markdown` module)
//! - The registry records' own invariants (see `cli_command_registry_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the renderer's page bytes and link targets.

use super::*;
use crate::cli_command_spec::{Example, Positional, Section};

/// A flag with a documented default value.
const fn with_default(mut flag: Flag, default: &'static str) -> Flag {
    flag.default = Some(default);
    flag
}

/// A flag whose value may be omitted.
const fn with_optional_value(mut flag: Flag) -> Flag {
    flag.value_optional = true;
    flag
}

static TEST_ALIASES: &[&str] = &["remember"];

static TEST_POSITIONALS: &[Positional] = &[Positional {
    label: "TEXT",
    required: true,
    description: "The text to save | verbatim.",
}];

static TEST_NOTES: &[&str] = &[
    "The item is saved with the current time.",
    "Pipes in a description | stay in one cell.",
];

static TEST_FLAGS: &[Flag] = &[
    with_default(Flag::text("kind", "KIND", "The kind of item."), "note"),
    Flag::choice(
        "backend",
        "sqlite|postgres",
        "The store backend.",
        &["sqlite", "postgres"],
    ),
    Flag::toggle("now", "Saves without waiting."),
    with_optional_value(Flag::text("file", "PATH", "The file to read from.")),
    Flag::text("kind", "KIND", "The kind of item."),
];

static TEST_EXAMPLES: &[Example] = &[
    Example {
        command: "clio memory add hello",
        comment: Some("Save one line:"),
    },
    Example {
        command: "clio memory add `tick`",
        comment: None,
    },
];

static TEST_ALTERNATES: &[&str] = &["clio memory add --file PATH"];

/// A rich record: aliases, notes, a positional, four flags, globals,
/// alternates, and examples.
fn memory_add_spec() -> CommandSpec {
    CommandSpec {
        name: "memory add",
        aliases: TEST_ALIASES,
        summary: "Adds one memory item.",
        notes: TEST_NOTES,
        positionals: TEST_POSITIONALS,
        documented: TEST_FLAGS,
        accepted_extra: &["db", "yes"],
        flag_aliases: &[],
        usage_override: None,
        usage_alternates: TEST_ALTERNATES,
        examples: TEST_EXAMPLES,
        tool: None,
        section: Section::Write,
    }
}

/// A bare record: nothing but the name, the summary, and one accepted global.
fn memory_list_spec() -> CommandSpec {
    CommandSpec {
        name: "memory list",
        aliases: &[],
        summary: "Lists memory items.",
        notes: &[],
        positionals: &[],
        documented: &[],
        accepted_extra: &["db"],
        flag_aliases: &[],
        usage_override: None,
        usage_alternates: &[],
        examples: &[],
        tool: None,
        section: Section::Read,
    }
}

/// A single-token record, so every section but the title is empty.
fn get_spec() -> CommandSpec {
    CommandSpec {
        name: "get",
        aliases: &[],
        summary: "Reads one item.",
        notes: &[],
        positionals: &[],
        documented: &[],
        accepted_extra: &[],
        flag_aliases: &[],
        usage_override: None,
        usage_alternates: &[],
        examples: &[],
        tool: None,
        section: Section::Read,
    }
}

/// The map over a `memory` group: the parent and two siblings hold pages.
fn group_map() -> PageMap {
    PageMap::new(["memory", "memory add", "memory list"])
}

fn render(spec: &CommandSpec, pages: &PageMap) -> String {
    render_command_page(spec, pages)
}

/// The one line of `page` holding `needle`, for row-level assertions.
fn line_of(page: &str, needle: &str) -> String {
    page.lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line holds {needle:?} in:\n{page}"))
        .to_owned()
}

#[test]
fn the_page_opens_with_the_command_title_and_summary() {
    let page = render(&memory_add_spec(), &group_map());
    let opening: Vec<&str> = page.lines().take(3).collect();
    assert_eq!(
        opening,
        vec!["# clio memory add", "", "Adds one memory item."]
    );
}

#[test]
fn the_usage_section_fences_the_usage_line_and_every_alternate() {
    let page = render(&memory_add_spec(), &group_map());
    assert!(page.contains(
        "\n## Usage\n\n```\nclio memory add TEXT [--kind KIND] \
         [--backend sqlite|postgres] [--now] [--file [PATH]] [--kind KIND]\n\
         clio memory add --file PATH\n```\n"
    ));
}

#[test]
fn each_note_renders_as_its_own_paragraph() {
    let page = render(&memory_add_spec(), &group_map());
    assert!(page.contains(
        "\n## Description\n\nThe item is saved with the current time.\n\n\
         Pipes in a description | stay in one cell.\n"
    ));
    assert!(!render(&memory_list_spec(), &group_map()).contains("## Description"));
}

#[test]
fn the_arguments_table_escapes_pipes_and_marks_required() {
    let page = render(&memory_add_spec(), &group_map());
    assert!(page.contains("## Arguments"));
    assert_eq!(
        line_of(&page, "| `TEXT` |"),
        "| `TEXT` | yes | The text to save \\| verbatim. |"
    );
}

#[test]
fn each_own_flag_renders_once_with_kind_default_and_description() {
    let page = render(&memory_add_spec(), &group_map());
    assert!(page.contains("## Options"));
    assert_eq!(
        line_of(&page, "`--kind KIND` | text"),
        "| `--kind KIND` | text | `note` | The kind of item. |"
    );
    // A documented-twice flag prints once.
    assert_eq!(
        page.matches("| `--kind KIND` | text |").count(),
        1,
        "the duplicate kind flag prints once"
    );
    assert_eq!(
        line_of(&page, "`--backend sqlite"),
        "| `--backend sqlite\\|postgres` | enum: sqlite, postgres |  | \
         The store backend. |"
    );
    assert_eq!(
        line_of(&page, "`--now` |"),
        "| `--now` |  |  | Saves without waiting. |"
    );
    assert_eq!(
        line_of(&page, "`--file [PATH]` |"),
        "| `--file [PATH]` | text |  | The file to read from. |"
    );
}

#[test]
fn an_accepted_global_renders_from_its_shared_registry_definition() {
    let page = render(&memory_add_spec(), &group_map());
    assert!(page.contains("## Global options"));
    assert_eq!(
        line_of(&page, "`--db PATH` |"),
        "| `--db PATH` | text |  | Overrides the store address that \
         `db_target::resolve_target` opens; config and environment decide it \
         when absent. |"
    );
    assert_eq!(
        line_of(&page, "`--yes` |"),
        "| `--yes` |  |  | Answers yes to the destructive-confirmation gate \
         that `cli_confirm::confirmed` applies. |"
    );
}

#[test]
fn a_shared_flag_is_defined_once_and_rendered_on_every_page_that_accepts_it() {
    let add = render(&memory_add_spec(), &group_map());
    let list = render(&memory_list_spec(), &group_map());
    assert_eq!(
        line_of(&add, "`--db PATH` |"),
        line_of(&list, "`--db PATH` |")
    );
}

#[test]
fn each_example_renders_its_comment_and_an_adaptive_fence() {
    let page = render(&memory_add_spec(), &group_map());
    assert!(page.contains(
        "\n## Examples\n\nSave one line:\n```\nclio memory add hello\n```\n\n\
         ```\nclio memory add `tick`\n```\n"
    ));
}

#[test]
fn the_aliases_render_and_a_command_without_them_omits_the_section() {
    let page = render(&memory_add_spec(), &group_map());
    assert!(page.contains("\n## Aliases\n\n`remember`\n"));
    assert!(!render(&memory_list_spec(), &group_map()).contains("## Aliases"));
}

#[test]
fn the_see_also_links_the_parent_page_and_its_siblings_in_order() {
    let page = render(&memory_add_spec(), &group_map());
    assert!(
        page.contains("\n## See also\n\n- [memory](memory.md)\n- [memory list](memory-list.md)\n")
    );
    assert_eq!(
        group_map().see_also("memory add"),
        vec![
            (String::from("memory"), String::from("memory.md")),
            (String::from("memory list"), String::from("memory-list.md")),
        ]
    );
}

/// The map over a nested `sync` group: the group page, its direct children,
/// and a deeper `sync key` group whose names only share the outer prefix.
fn nested_sync_map() -> PageMap {
    PageMap::new([
        "sync",
        "sync key export",
        "sync key status",
        "sync pull",
        "sync push",
    ])
}

/// A grandchild shares the page's prefix but its own derived parent differs,
/// so it must not be listed as a sibling of the shallower page.
#[test]
fn see_also_excludes_a_deeper_name_whose_own_parent_differs() {
    assert_eq!(
        nested_sync_map().see_also("sync push"),
        vec![
            (String::from("sync"), String::from("sync.md")),
            (String::from("sync pull"), String::from("sync-pull.md")),
        ],
        "sync key export and sync key status are children of sync key, not siblings"
    );
}

/// The deeper page still lists its own parent — the group without a page falls
/// back to the index — and its true sibling, never the outer group's children.
#[test]
fn a_deeper_page_lists_its_own_parent_and_sibling_only() {
    assert_eq!(
        nested_sync_map().see_also("sync key export"),
        vec![
            (String::from("sync key"), String::from("index.md")),
            (
                String::from("sync key status"),
                String::from("sync-key-status.md"),
            ),
        ],
    );
}

/// A rendered page's See-also section never links a grandchild as a sibling.
#[test]
fn a_rendered_page_never_links_a_grandchild_as_a_sibling() {
    let pages = PageMap::new(["memory", "memory add", "memory key export"]);
    let page = render(&memory_add_spec(), &pages);
    assert!(page.contains("- [memory](memory.md)"));
    assert!(
        !page.contains("memory key export"),
        "a child of memory key is not a sibling of memory add: {page}"
    );
}

#[test]
fn a_derived_parent_without_a_page_links_the_index() {
    let pages = PageMap::new(["memory add"]);
    let page = render(&memory_add_spec(), &pages);
    assert!(page.contains("\n## See also\n\n- [memory](index.md)\n"));
}

#[test]
fn a_single_token_command_has_no_parent_and_no_see_also() {
    let pages = group_map();
    assert_eq!(pages.parent_link("get"), None);
    assert_eq!(pages.see_also("get"), Vec::new());
    assert!(!render(&get_spec(), &pages).contains("## See also"));
}

#[test]
fn the_page_map_names_are_deterministic_and_collision_checked() {
    let forward = PageMap::new(["memory add", "memory-add"]);
    let reversed = PageMap::new(["memory-add", "memory add"]);
    assert_eq!(forward.page("memory add"), Some("memory-add.md"));
    assert_eq!(forward.page("memory-add"), Some("memory-add-2.md"));
    assert_eq!(forward.page("memory add"), reversed.page("memory add"));
    assert_eq!(forward.page("memory-add"), reversed.page("memory-add"));
    assert_eq!(PageMap::index_page(), "index.md");
}

#[test]
fn a_table_cell_holds_no_raw_newline_and_no_unescaped_pipe() {
    assert_eq!(table_cell("a | b\nc\rd"), "a \\| b c d");
}

#[test]
fn a_code_fence_is_always_longer_than_any_backtick_run_in_its_content() {
    assert_eq!(code_fence("plain"), "```\nplain\n```\n");
    assert_eq!(code_fence("a ``` b"), "````\na ``` b\n````\n");
}

#[test]
fn rendering_the_same_record_twice_is_identical_and_holds_no_host_path() {
    let first = render(&memory_add_spec(), &group_map());
    let second = render(&memory_add_spec(), &group_map());
    assert_eq!(first, second);
    assert!(!first.contains(env!("CARGO_MANIFEST_DIR")));
    assert!(!first.contains("/home/"));
}
