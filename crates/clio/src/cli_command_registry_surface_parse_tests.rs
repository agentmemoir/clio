// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests how the reserved-surface records' accepted sets meet the
//! real parsers and helpers, and how dispatch routes the surfaces' trailing
//! arguments and refusals.
//!
//! ## Owns
//! - The parser and helper cases: `parse_setup_args` over every accepted
//!   name, `parse_flags` through the record-derived name lists, and the
//!   records' parser specs against the real surface parsers
//! - The version and health dispatch's acceptance of trailing arguments, the
//!   `ops` dispatcher's refusal and help exits with its unknown-flag parity,
//!   and the `compose` dispatcher's unknown-flag refusal
//!
//! ## Does not own
//! - Byte equality of each record's `usage_line` with the authored surface
//!   row and the records' summaries and prose against their authored rows
//!   (`cli_command_registry_surface_tests`)
//! - The registry lookups and `validate` itself (`cli_command_registry_tests`)
//! - The reserved surfaces' end-to-end process contract
//!   (`reserved_surface_args_harness`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the surfaces' parse and dispatch routing.

use std::process::ExitCode;

use crate::cli_command_registry_test_support::record;

/// Every flag name a usage line writes, bracket and placeholder stripped.
fn flag_names(line: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("--") {
        let after = &rest[start + 2..];
        let end = after
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
            .unwrap_or(after.len());
        names.push(&after[..end]);
        rest = &after[end..];
    }
    names
}

/// `crate::dispatch` over one surface name, so a test exercises the same
/// routing the binary's argument list reaches.
fn dispatched(name: &str, args: &[&str]) -> ExitCode {
    crate::dispatch(name, args.iter().map(|arg| (*arg).to_owned()).collect())
}

/// `mcp schema-export` carries its own help prose, and the record-derived
/// name list is the list its handler parses with: both value spellings are
/// accepted and an unknown flag is refused.
#[test]
fn schema_export_carries_its_help_prose_and_matches_its_flag_table() {
    let spec = record("mcp schema-export");
    assert_eq!(
        spec.notes,
        [
            "--out FILE writes the pack to a file; --path is an accepted alias.\nWith neither flag the pack is written to stdout."
        ]
    );
    let names = crate::cli_command_registry_surface::parse_flag_names("mcp schema-export");
    assert_eq!(names, ["out", "path"]);
    let bare =
        crate::cli_top_level::parse_flags(["--out".to_owned(), "pack.json".to_owned()], &names)
            .expect("--out is documented");
    assert_eq!(bare.get("out").map(String::as_str), Some("pack.json"));
    let explicit = crate::cli_top_level::parse_flags(["--path=pack.json".to_owned()], &names)
        .expect("--path is the accepted alias");
    assert_eq!(explicit.get("path").map(String::as_str), Some("pack.json"));
    assert!(
        crate::cli_top_level::parse_flags(["--bogus".to_owned()], &names).is_err(),
        "an unknown flag is refused"
    );
}

/// The two transports keep the accepted set their records state: the
/// documented flags plus the hidden `--shared-bank`, which the usage line
/// never prints. Bare, `--bind` is present with no value (the auto-scan);
/// with a value it binds that address and never consumes a following flag.
#[test]
fn the_transports_keep_their_hidden_flag_and_bind_forms() {
    for name in ["mcp stdio", "mcp http"] {
        let spec = record(name);
        let names = crate::cli_command_registry_surface::parse_flag_names(name);
        for flag in spec.documented {
            assert!(names.contains(&flag.long), "{name} accepts --{}", flag.long);
        }
        assert!(
            names.contains(&"shared-bank"),
            "{name} keeps --shared-bank, which the usage line never prints"
        );
        assert!(
            !spec.usage_line().contains("shared-bank"),
            "{name} does not print --shared-bank"
        );
    }
    let names = crate::cli_command_registry_surface::parse_flag_names("mcp http");
    let parse = |args: &[&str]| {
        crate::cli_top_level::parse_flags(args.iter().map(std::string::ToString::to_string), &names)
    };
    let bare = parse(&["--bind"]).expect("bind is documented");
    assert_eq!(bare.get("bind").map(String::as_str), Some("true"));
    let explicit = parse(&["--bind", "127.0.0.1:4000"]).expect("an explicit value is accepted");
    assert_eq!(
        explicit.get("bind").map(String::as_str),
        Some("127.0.0.1:4000")
    );
    let inline = parse(&["--bind=127.0.0.1:4000"]).expect("an inline value is the same flag");
    assert_eq!(
        inline.get("bind").map(String::as_str),
        Some("127.0.0.1:4000")
    );
    let followed =
        parse(&["--bind", "--db", "x"]).expect("a bare --bind does not consume a following flag");
    assert_eq!(followed.get("bind").map(String::as_str), Some("true"));
    assert_eq!(followed.get("db").map(String::as_str), Some("x"));
}

/// Each `ops` record's derived name list parses every documented non-global
/// flag it names and refuses an unknown flag, the dispatcher refuses that
/// same unknown flag with a usage error before any store is opened, and
/// `--confirm` stays a bare switch on the two commands it guards.
#[test]
fn the_ops_records_match_their_subcommand_flag_sets() {
    for sub in ["diagnose", "verify", "doctor", "repair", "reindex"] {
        let full = format!("ops {sub}");
        let spec = record(&full);
        let names = crate::cli_command_registry_surface::parse_flag_names(&full);
        for flag in spec
            .documented
            .iter()
            .filter(|flag| crate::cli_command_registry::global_flag(flag.long).is_none())
        {
            let takes_value = flag.value.is_some();
            let args: Vec<String> = if takes_value {
                vec![format!("--{}", flag.long), "x".to_owned()]
            } else {
                vec![format!("--{}", flag.long)]
            };
            let parsed = crate::cli_top_level::parse_flags(args, &names).expect(flag.long);
            let want = if takes_value { "x" } else { "true" };
            assert_eq!(parsed.get(flag.long).map(String::as_str), Some(want));
        }
        assert!(
            crate::cli_top_level::parse_flags(["--frobnicate".to_owned()], &names).is_err(),
            "{full} refuses an unknown flag"
        );
        assert_eq!(
            crate::ops_cli::ops_with([sub.to_owned(), "--frobnicate".to_owned()], &|_| None::<
                String,
            >),
            ExitCode::from(2),
            "{full} dispatcher refuses the same unknown flag"
        );
        if matches!(sub, "repair" | "reindex") {
            let confirm = spec
                .documented
                .iter()
                .find(|flag| flag.long == "confirm")
                .expect("confirm is documented");
            assert!(confirm.value.is_none(), "{full} takes --confirm bare");
        }
    }
}

/// The `compose` records share the `up` derivation the dispatcher gates on,
/// and the dispatcher refuses an unknown flag with a usage error before any
/// Docker probe runs, so a record that loses a flag fails both here and at
/// the parse gate.
#[test]
fn the_compose_records_match_their_dispatcher_refusal() {
    for name in ["compose up", "compose down"] {
        let names = crate::cli_command_registry_surface::parse_flag_names(name);
        assert!(names.contains(&"dir"), "{name} accepts --dir");
        assert!(names.contains(&"services"), "{name} accepts --services");
        assert!(
            crate::cli_top_level::parse_flags(["--frobnicate".to_owned()], &names).is_err(),
            "{name} refuses an unknown flag"
        );
    }
    for sub in ["up", "down"] {
        assert_eq!(
            crate::compose_cli::compose([sub.to_owned(), "--frobnicate".to_owned()]),
            ExitCode::from(2),
            "compose {sub} dispatcher refuses the same unknown flag"
        );
    }
}

/// The `ops` dispatcher keeps its no-store refusals: an unknown subcommand
/// exits 2 and the help branch exits 0, over an injected lookup that reads
/// no host environment.
#[test]
fn the_ops_dispatcher_keeps_its_refusal_and_help_exits() {
    assert_eq!(
        crate::ops_cli::ops_with(["frobnicate".to_owned()], &|_| None::<String>),
        ExitCode::from(2)
    );
    assert_eq!(
        crate::ops_cli::ops_with(["--help".to_owned()], &|_| None::<String>),
        ExitCode::SUCCESS
    );
}

/// The version and health dispatch ignores its trailing arguments: a stray
/// token or an unknown flag is accepted and left unread, and the command still
/// answers. A help spelling renders the record's help instead.
#[test]
fn version_and_health_accept_trailing_tokens_and_answer() {
    for name in ["version", "health"] {
        assert_eq!(
            dispatched(name, &[]),
            ExitCode::SUCCESS,
            "{name} with no trailing arguments runs"
        );
        assert_eq!(
            dispatched(name, &["stray"]),
            ExitCode::SUCCESS,
            "{name} ignores a stray token"
        );
        assert_eq!(
            dispatched(name, &["--frobnicate"]),
            ExitCode::SUCCESS,
            "{name} ignores an unknown flag"
        );
        for help in ["--help", "-h"] {
            assert_eq!(
                dispatched(name, &[help]),
                ExitCode::SUCCESS,
                "{help} prints {name}'s record help"
            );
        }
    }
}

/// The help record documents `--json` and `--markdown`, and the dispatch reads
/// the spellings from the record: only a documented boolean selects a mode.
#[test]
fn help_documents_json_and_the_dispatch_reads_it_from_the_record() {
    let spec = record("help");
    let booleans: Vec<&str> = spec
        .documented
        .iter()
        .filter(|flag| flag.value.is_none())
        .map(|flag| flag.long)
        .collect();
    assert_eq!(
        booleans,
        ["json", "markdown"],
        "the help record's boolean flags"
    );
    assert!(
        spec.positionals
            .iter()
            .any(|positional| positional.label == "TOPIC" && !positional.required),
        "the help record holds the optional TOPIC positional"
    );
    assert!(crate::cli_top_level::wants_json_catalog(&[
        "--json".to_owned()
    ]));
    assert!(!crate::cli_top_level::wants_json_catalog(
        &["-h".to_owned()]
    ));
    assert!(!crate::cli_top_level::wants_json_catalog(&[]));
}

/// `status` keeps its accepted set: the record's parser spec is exactly the
/// names the real surface parser accepts, and the record-derived name list
/// gates `--dir` through the shared parser in both value spellings.
#[test]
fn status_matches_the_real_surface_parser() {
    let parser = record("status").parser_spec();
    // The spec orders the globals first, then the surface's own flags.
    assert_eq!(
        parser.value_flags(),
        ["db", "backend", "bank", "actor", "output", "dir"]
    );
    assert_eq!(parser.bool_flags(), ["no-input", "yes"]);

    let names = crate::cli_top_level::record_flag_names("status");
    let bare = crate::cli_top_level::parse_flags(["--dir".to_owned(), "/tmp".to_owned()], &names)
        .expect("--dir is documented");
    assert_eq!(bare.get("dir").map(String::as_str), Some("/tmp"));
    let explicit = crate::cli_top_level::parse_flags(["--dir=/tmp".to_owned()], &names)
        .expect("an explicit value is the same flag");
    assert_eq!(explicit.get("dir").map(String::as_str), Some("/tmp"));
    assert!(
        crate::cli_top_level::parse_flags(["--frobnicate".to_owned()], &names).is_err(),
        "an unknown flag is refused"
    );
}

/// Setup's record states its real accepted set: every flag `parse_setup_args`
/// accepts is a `Flag` entry the record carries, the pinned usage line names
/// every documented one, and the constructed per-service names are explicit.
#[test]
fn setup_states_its_accepted_set_and_matches_its_parser() {
    let spec = record("setup");
    let line = spec.usage_line();
    let line_flags = flag_names(&line);

    // Every documented entry is named by the line.
    for flag in spec.documented {
        assert!(
            line_flags.contains(&flag.long),
            "--{} is accepted but the line does not name it",
            flag.long
        );
    }
    // Every flag the line names is one the record accepts.
    for name in &line_flags {
        assert!(
            spec.documented.iter().any(|flag| flag.long == *name),
            "--{name} is named but the setup parser does not accept it"
        );
    }
    // The flags the wizard constructs from the service role names are named
    // explicitly, not left to the construction route.
    for role in ["embed", "rerank"] {
        for suffix in ["provider", "url", "model", "key"] {
            let constructed = format!("{role}-{suffix}");
            assert!(
                line_flags.contains(&constructed.as_str()),
                "--{constructed} is a constructed name the line must carry"
            );
        }
    }
    // The parser reads the record: booleans bare, value flags with a value.
    for flag in spec.documented {
        let takes_value = flag.value.is_some();
        let args: Vec<String> = if takes_value {
            vec![format!("--{}", flag.long), "x".to_owned()]
        } else {
            vec![format!("--{}", flag.long)]
        };
        let parsed = crate::setup_flags::parse_setup_args(&args).expect(flag.long);
        let want = if takes_value { "x" } else { "true" };
        assert_eq!(
            parsed.get(flag.long).map(String::as_str),
            Some(want),
            "{}",
            flag.long
        );
    }
    // The surface's own `--help` spelling is accepted beside the record's
    // entries, though the pinned line does not name it.
    let helped =
        crate::setup_flags::parse_setup_args(&["--help".to_owned()]).expect("--help is accepted");
    assert_eq!(helped.get("help").map(String::as_str), Some("true"));
}

/// `install` and `uninstall` require `--harness`, and the record-derived name
/// list gates it through the shared parser in both value spellings.
#[test]
fn install_and_uninstall_require_harness_and_match_the_shared_parser() {
    for name in ["install", "uninstall"] {
        let spec = record(name);
        assert_eq!(spec.usage_line(), format!("clio {name} --harness <name>"));
        assert_eq!(spec.required_flags(), ["harness"]);
        let parser = spec.parser_spec();
        assert_eq!(
            parser.value_flags(),
            ["db", "backend", "bank", "actor", "output", "harness"]
        );
        assert_eq!(parser.bool_flags(), ["no-input", "yes"]);
    }

    let names = crate::cli_top_level::record_flag_names("install");
    let bare =
        crate::cli_top_level::parse_flags(["--harness".to_owned(), "claude".to_owned()], &names)
            .expect("--harness is documented");
    assert_eq!(bare.get("harness").map(String::as_str), Some("claude"));
    let explicit = crate::cli_top_level::parse_flags(["--harness=claude".to_owned()], &names)
        .expect("an explicit value is the same flag");
    assert_eq!(explicit.get("harness").map(String::as_str), Some("claude"));
    assert!(
        crate::cli_top_level::parse_flags(["--bogus".to_owned()], &names).is_err(),
        "an unknown flag is refused"
    );
}
