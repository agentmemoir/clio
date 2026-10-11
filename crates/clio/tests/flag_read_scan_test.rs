// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the literal-read scan over the flag-reading
//! modules: it finds every production flag read whose key is not a string
//! literal - constructed names, table-driven reads, constants - and asserts
//! that each one carries a reasoned exemption and the route test that exercises
//! it. An undecidable read fails the scan.
//!
//! ## Owns
//! - `non_literal_reads`, the bounded line scan over the named file list
//! - `READ_EXEMPTIONS`, the reasoned exemptions for today's non-literal reads
//! - `RESERVED_SURFACE_POLICY`, the reserved-surface override policy those
//!   flags stay documented exemptions under
//! - The printed exemption list, separate from the parser-parity check
//! - The self-tests: the per-line census, the exact exemption match, the
//!   undecidable path, and the route-check resolution
//!
//! ## Does not own
//! - The both-direction help/parser parity (`cli_help_usage_tests`)
//! - The usage lines, records, or parsers the exemptions reason about
//!
//! ## Boundary
//!
//! Changes to a non-literal read, or to an exemption for one, should normally
//! require changes here. Changes to a usage line or a record should not.
//!
//! Keep this module focused on the scan and its exemption table.

use std::path::{Path, PathBuf};

/// One non-literal flag read: where it sits and what kind it is.
#[derive(Debug)]
struct LiteralRead {
    file: &'static str,
    line: usize,
    kind: &'static str,
    text: String,
}

/// One reasoned exemption for a non-literal read the scan may pass over.
#[derive(Debug)]
struct ReadExemption {
    file: &'static str,
    needle: &'static str,
    reason: &'static str,
    route_check: &'static str,
}

/// A reserved surface whose flags are documented exemptions from the
/// registry-backed read set, with the route test that exercises it.
struct ReservedSurface {
    surface: &'static str,
    reason: &'static str,
    route_check: &'static str,
}

/// The production files the scan reads, with each path derived from
/// `CARGO_MANIFEST_DIR` rather than the process working directory. The bound is
/// explicit and short rather than a tree walk: the read engine, the shared read
/// helpers, the six read groups whose records own their flags, the export
/// fallback, and the reserved-surface files that refuse store flags in a loop.
const READ_SCAN_FILES: &[&str] = &[
    "src/cli_read.rs",
    "src/cli_read_args.rs",
    "src/cli_read_core.rs",
    "src/cli_read_history.rs",
    "src/cli_read_belief.rs",
    "src/cli_read_graph.rs",
    "src/cli_read_workspace.rs",
    "src/cli_read_hygiene.rs",
    "src/cli_write_portability.rs",
    "src/auth_cli.rs",
    "src/backup_list_cli.rs",
    "src/backup_prune_cli.rs",
    "src/sync_key_cli.rs",
];

/// The non-literal reads the scan finds today, pinned so the number cannot
/// shrink unnoticed: an added read fails as undecidable until an exemption with
/// a reason and a route check exists, and a removed one changes this count and
/// forces the same review.
const EXPECTED_LITERAL_READS: usize = 11;

/// The reasons the scan may pass over a non-literal read in a named file. Each
/// entry names the route test that exercises the read, so an exemption is a
/// claim with evidence rather than a silence.
const READ_EXEMPTIONS: &[ReadExemption] = &[
    ReadExemption {
        file: "src/cli_read_core.rs",
        needle: "parsed.value(flag)",
        reason: "table-driven read over the literal floor table and the inspect filter table; every name in each table is documented by the command's record",
        route_check: "cli_read_tests_floors_tests::recall_all_four_floors_use_the_documented_key_names, cli_read::tests::inspect_filters_and_individual_flags",
    },
    ReadExemption {
        file: "src/cli_read_belief.rs",
        needle: "parsed.value(term)",
        reason: "table-driven read over the static TRIPLE_TERMS table, the same table that names the positionals; the triple query record documents all three names",
        route_check: "cli_read_belief::tests::triple_query_positionals_flags_and_text",
    },
    ReadExemption {
        file: "src/cli_read_args.rs",
        needle: "parsed.value(flag)",
        reason: "the required-flags helper reads only names its own list already proved required, and that list comes from the record or the authored row",
        route_check: "cli_read_args::tests::every_missing_required_flag_is_named_at_once",
    },
    ReadExemption {
        file: "src/cli_read_args.rs",
        needle: "parsed.value(name)",
        reason: "the shared flag accessor: the name comes from its caller, and every caller passes a literal",
        route_check: "cli_write_portability::tests::export_import_round_trip_is_idempotent",
    },
    ReadExemption {
        file: "src/cli_write_portability.rs",
        needle: "parsed.value(flag)",
        reason: "table-driven read over the export filter table and the include-flag helper; the authored export row documents each name, and the verb's record arrives in a later phase",
        route_check: "cli_write_portability::tests::export_import_round_trip_is_idempotent",
    },
    ReadExemption {
        file: "src/auth_cli.rs",
        needle: "parsed.value(flag)",
        reason: "table-driven refusal over the literal store-flag list; --db/--backend are declared globals and --bank/--actor are the verb's own flags",
        route_check: "auth_cli::tests::the_store_flags_a_verb_cannot_honour_are_refused",
    },
    ReadExemption {
        file: "src/backup_list_cli.rs",
        needle: "parsed.value(flag)",
        reason: "table-driven refusal over the literal store-flag list; the backup surface owns its FlagSpec, so its flags stay documented exemptions until its override records arrive in a later phase",
        route_check: "backup_list_cli::tests::list_refuses_the_store_flags_it_cannot_honor",
    },
    ReadExemption {
        file: "src/backup_prune_cli.rs",
        needle: "parsed.value(flag)",
        reason: "table-driven refusal over the literal store-flag list; the backup surface owns its FlagSpec, so its flags stay documented exemptions until its override records arrive in a later phase",
        route_check: "backup_prune_cli::tests::prune_refuses_the_store_flags_it_cannot_honor",
    },
    ReadExemption {
        file: "src/sync_key_cli.rs",
        needle: "parsed.value(flag)",
        reason: "table-driven refusal over the literal store-flag list; the sync key surface owns its FlagSpec, so its flags stay documented exemptions until its override records arrive in a later phase",
        route_check: "sync_key_cli::tests::the_store_flags_are_refused_by_every_key_verb",
    },
];

/// The reserved surfaces' override policy, printed with the exemption table:
/// each reads its flags through its own `FlagSpec` rather than a registry
/// record, so its flags stay documented exemptions until its override records
/// arrive in a later phase. Their documented flag lists live in
/// `cli_help_surfaces::SURFACE_USAGE`, and each entry names the route test that
/// exercises the reads.
const RESERVED_SURFACE_POLICY: &[ReservedSurface] = &[
    ReservedSurface {
        surface: "backup",
        reason: "owns its FlagSpec; its flags stay documented exemptions until its override records arrive in a later phase",
        route_check: "backup_cli::tests::cli_t10_backup_dir_creates_the_fallback_dir_owner_only, backup_create_cli::tests::create_writes_one_archive_and_reports_what_it_holds, backup_list_cli::tests::list_refuses_the_store_flags_it_cannot_honor, backup_prune_cli::tests::prune_refuses_the_store_flags_it_cannot_honor, backup_restore_cli::tests::a_dry_run_plans_and_changes_nothing, backup_verify_cli::tests::verify_refuses_the_store_flags_and_a_foreign_backend",
    },
    ReservedSurface {
        surface: "sync key",
        reason: "owns its FlagSpec; its flags stay documented exemptions until its override records arrive in a later phase",
        route_check: "sync_key_cli::tests::the_store_flags_are_refused_by_every_key_verb",
    },
    ReservedSurface {
        surface: "auth token",
        reason: "owns its FlagSpec; its flags stay documented exemptions until its override records arrive in a later phase",
        route_check: "auth_cli::tests::the_store_flags_a_verb_cannot_honour_are_refused",
    },
];

/// The read-call heads that take a flag name, matched with optional
/// whitespace before the parenthesis. `flag(` must open on `parsed`, the only
/// receiver these modules use. `parsed.bool_before` is not one: its argument
/// is a positional index.
const READ_HEADS: &[&str] = &["parsed.values_of", "parsed.value", "parsed.has", "flag"];

/// Every flag read in `source` whose key is not a string literal, counting
/// each read call on a line: a second read collocated with a first is its own
/// finding.
///
/// A comment line is skipped: these modules quote calls in their docs. The key
/// expression decides the kind - `format!` is a constructed name, an
/// all-uppercase identifier is a constant, and anything else is a table-driven
/// read (`flag`, `term`, `name`).
fn non_literal_reads(file: &'static str, source: &str) -> Vec<LiteralRead> {
    let mut found = Vec::new();
    for (index, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for (text, key) in read_calls(line) {
            let kind = if key.contains("format!") {
                "constructed"
            } else if is_constant(&key) {
                "constant"
            } else {
                "table-driven"
            };
            found.push(LiteralRead {
                file,
                line: index + 1,
                kind,
                text,
            });
        }
    }
    found
}

/// Every flag-read call on a line, each as its rendered text and its key
/// expression. The rendered text is canonical - no space before the
/// parenthesis, `flag` rendered with its `parsed` receiver - so a call matches
/// its exemption however it was spaced in the source.
fn read_calls(line: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for head in READ_HEADS {
        let mut from = 0;
        while let Some(step) = line[from..].find(head) {
            from += step + head.len();
            let rest = line[from..].trim_start();
            if !rest.starts_with('(') {
                continue;
            }
            let mut key = &rest[1..];
            if *head == "flag" {
                let Some(rest) = key.trim_start().strip_prefix("parsed") else {
                    continue;
                };
                let Some(rest) = rest.trim_start().strip_prefix(',') else {
                    continue;
                };
                key = rest;
            }
            let key = key.split(')').next().unwrap_or("").trim();
            if key.starts_with('"') {
                // A literal key is decidable; the parity check owns it.
                continue;
            }
            let text = if *head == "flag" {
                format!("flag(parsed, {key})")
            } else {
                format!("{head}({key})")
            };
            found.push((text, key.to_string()));
        }
    }
    found
}

/// Whether the key expression starts with a `SCREAMING_CASE` identifier.
fn is_constant(key: &str) -> bool {
    let ident: String = key
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    !ident.is_empty()
        && ident.chars().any(|c| c.is_ascii_uppercase())
        && !ident.chars().any(|c| c.is_ascii_lowercase())
}

/// The exemption for one found read, or `None` when the scan cannot decide it.
/// The needle must equal the rendered call exactly, so one read cannot be
/// exempted under another read's reason.
fn exemption_for(file: &str, text: &str) -> Option<&'static ReadExemption> {
    READ_EXEMPTIONS
        .iter()
        .find(|entry| entry.file == file && text == entry.needle)
}

/// The scan's verdict for each finding: the exemption that covers it, or the
/// undecidable site. Separate from the main test so the self-test can exercise
/// the undecidable path.
fn classify(findings: &[LiteralRead]) -> (Vec<(&'static ReadExemption, String)>, Vec<String>) {
    let mut exempted = Vec::new();
    let mut undecidable = Vec::new();
    for finding in findings {
        let (file, line, kind, text) = (finding.file, finding.line, finding.kind, &finding.text);
        let site = format!("{file}:{line} [{kind}] {text}");
        match exemption_for(finding.file, &finding.text) {
            Some(entry) => exempted.push((entry, site)),
            None => undecidable.push(site),
        }
    }
    (exempted, undecidable)
}

/// The literal-read scan: every flag read whose key is not a string literal is
/// listed with a reason and the route test that exercises it, printed
/// separately from the both-direction parser parity. An undecidable read - one
/// with no registered exemption - fails.
///
/// The reserved surfaces are documented exemptions: each reads its flags
/// through its own `FlagSpec` instead of a registry record, so they stay
/// documented exemptions until their override records arrive in a later phase.
#[test]
fn flag_read_scan_exempts_every_non_literal_read_with_a_reason() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut findings: Vec<LiteralRead> = Vec::new();
    for file in READ_SCAN_FILES {
        let path = root.join(file);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("the scan cannot read {}: {error}", path.display()));
        findings.extend(non_literal_reads(file, &source));
    }

    let (exempted, undecidable) = classify(&findings);

    // The exemption list, printed separately from the parser-parity check.
    println!("literal-read exemptions ({}):", exempted.len());
    for (entry, site) in &exempted {
        println!("  {site}");
        println!("    reason: {}", entry.reason);
        println!("    route check: {}", entry.route_check);
    }
    println!("reserved-surface override policy:");
    for surface in RESERVED_SURFACE_POLICY {
        println!("  {}", surface.surface);
        println!("    reason: {}", surface.reason);
        println!("    route check: {}", surface.route_check);
    }

    assert!(
        undecidable.is_empty(),
        "these flag reads have no registered exemption, so the scan cannot \
         decide them:\n{}",
        undecidable.join("\n")
    );
    // Coverage bookkeeping, so the exemption table cannot silently shrink.
    assert_eq!(
        findings.len(),
        EXPECTED_LITERAL_READS,
        "the non-literal read surface changed; review the exemption table"
    );
    // No dead exemption: every entry must excuse a read the scan really finds.
    for entry in READ_EXEMPTIONS {
        assert!(
            exempted
                .iter()
                .any(|(used, _)| used.file == entry.file && used.needle == entry.needle),
            "the exemption for {} at `{}` excuses no read the scan finds",
            entry.file,
            entry.needle
        );
    }
}

/// The detector must catch the three kinds it exists for and pass a literal
/// key, so an empty undecidable list is evidence the scan can see rather than
/// evidence it found nothing.
#[test]
fn flag_read_scan_classifies_the_three_kinds_and_skips_literal_keys() {
    let source = concat!(
        "let a = parsed.value(&format!(\"--{name}\"));\n",
        "let b = parsed.value(FLAG_CONST);\n",
        "let c = parsed.has(flag);\n",
        "let d = parsed.value(\"literal\");\n",
        "// parsed.value(flag);\n",
    );
    let found = non_literal_reads("src/example.rs", source);
    let kinds: Vec<&str> = found.iter().map(|read| read.kind).collect();
    assert_eq!(
        kinds,
        ["constructed", "constant", "table-driven"],
        "{found:?}"
    );
}

/// The census: two reads on one line are both counted, whitespace before the
/// parenthesis is still a read, and a needle cannot excuse a longer call whose
/// text merely contains it.
#[test]
fn flag_read_scan_counts_every_read_on_a_line_and_pins_the_exemption_boundary() {
    let source = concat!(
        "let a = one(parsed.value(flag), parsed.has(term));\n",
        "let b = parsed.value (flag2);\n",
        "let c = flag (parsed, FLAG_CONST);\n",
    );
    let found = non_literal_reads("src/example.rs", source);
    assert_eq!(
        found
            .iter()
            .map(|read| read.text.as_str())
            .collect::<Vec<_>>(),
        [
            "parsed.value(flag)",
            "parsed.has(term)",
            "parsed.value(flag2)",
            "flag(parsed, FLAG_CONST)"
        ],
        "{found:?}"
    );
    // The needle `parsed.value(flag)` must not excuse `parsed.value(flag_filter)`.
    assert!(exemption_for("src/cli_read_core.rs", "parsed.value(flag_filter)").is_none());
    assert!(exemption_for("src/cli_read_core.rs", "parsed.value(flag)").is_some());
}

/// The failure modes: an unregistered read lands undecidable, the policy
/// table carries its reasons, and every route check names a real test.
#[test]
fn flag_read_scan_self_tests_the_undecidable_path_the_policy_and_the_route_checks() {
    let unregistered = non_literal_reads("src/example.rs", "let x = parsed.value(other);\n");
    assert_eq!(unregistered.len(), 1, "{unregistered:?}");
    let (exempted, undecidable) = classify(&unregistered);
    assert!(exempted.is_empty(), "{exempted:?}");
    assert_eq!(undecidable.len(), 1, "{undecidable:?}");

    assert_eq!(RESERVED_SURFACE_POLICY.len(), 3);
    for surface in RESERVED_SURFACE_POLICY {
        assert!(!surface.reason.is_empty(), "{}", surface.surface);
        assert!(!surface.route_check.is_empty(), "{}", surface.surface);
    }
    let checks = READ_EXEMPTIONS
        .iter()
        .map(|entry| entry.route_check)
        .chain(RESERVED_SURFACE_POLICY.iter().map(|s| s.route_check));
    for route in checks.flat_map(|checks| checks.split(", ")) {
        assert!(test_fn_exists(route), "route check {route} names no test");
    }
}

/// Whether a route check resolves: its last path segment must name a test
/// function declared somewhere in the crate's own Rust files.
fn test_fn_exists(route: &str) -> bool {
    let name = route.rsplit("::").next().unwrap_or(route);
    let wanted = format!("fn {name}(");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in ["src", "tests"] {
        push_rs_files(&root.join(dir), &mut files);
    }
    files
        .iter()
        .any(|path| std::fs::read_to_string(path).is_ok_and(|source| source.contains(&wanted)))
}

/// Every Rust file under `dir`, at any depth.
fn push_rs_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            push_rs_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}
