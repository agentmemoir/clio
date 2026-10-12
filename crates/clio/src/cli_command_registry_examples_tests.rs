// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the record metadata and completeness
//! contract `check_metadata` applies — the flag-metadata rules, the positional
//! rules, and the example rules — together with the example scanner and its
//! dump seam.
//!
//! ## Owns
//! - `check_metadata`'s flag, positional, and example rejection cases, the
//!   whole-registry gate over `all()`, and the declared globals' flag metadata
//! - The example scanner's accept and reject cases, and the JSON dump shape
//!
//! ## Does not own
//! - The registry's lookups and the alias contract (`cli_command_registry_tests`)
//! - The help-topic contract (`cli_command_registry_help_topics_tests`)
//! - The records themselves (the tranche test modules)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the metadata, completeness, and example tests.

use super::{check_example, check_examples, examples_dump_json, split_line};
use crate::cli_command_registry::{
    check_flag_metadata, check_metadata, global_bool_names, global_flag, global_value_names,
};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Positional, ValueKind};

/// A clone of a registered record, so a test can vary one of its fields.
fn registered(name: &str) -> CommandSpec {
    crate::cli_command_registry_test_support::record(name).clone()
}

/// Every documented flag in the real registry, and every declared global,
/// carries complete metadata: a description, and for a value flag a placeholder
/// and a non-empty kind. This is the flag half of `check_metadata`; its
/// completeness half turns green as the record tranches land their data.
#[test]
fn validate_accepts_the_registry_flag_metadata() {
    let specs: Vec<&CommandSpec> = crate::cli_command_registry::all().collect();
    for spec in &specs {
        for flag in spec.documented {
            check_flag_metadata(flag).unwrap_or_else(|error| panic!("`{}`: {error}", spec.name));
        }
    }
    for name in global_value_names().chain(global_bool_names()) {
        let flag = global_flag(name).expect("a declared global");
        check_flag_metadata(flag).unwrap_or_else(|error| panic!("global `{name}`: {error}"));
    }
}

/// The whole registry passes the completeness contract: `check_metadata` over
/// every record covers the flag rules, the positional rules, and every
/// record's examples. The per-tranche tests supplement this gate; they never
/// replace it.
#[test]
fn check_metadata_accepts_the_whole_registry() {
    let specs: Vec<&CommandSpec> = crate::cli_command_registry::all().collect();
    assert_eq!(check_metadata(&specs), Ok(()));
}

/// A documented flag with a blank description fails the metadata check, and the
/// error names the owning record.
#[test]
fn check_metadata_rejects_a_flag_with_a_blank_description() {
    const DOCUMENTED: &[Flag] = &[Flag::value("limit", "N")];
    let spec = CommandSpec {
        documented: DOCUMENTED,
        ..registered("get")
    };
    let error = check_metadata(&[&spec]).expect_err("a blank description is rejected");
    assert!(error.contains("--limit"), "{error}");
    assert!(error.contains("get"), "{error}");
}

/// A value flag with a blank placeholder fails the metadata check.
#[test]
fn check_metadata_rejects_a_value_flag_with_a_blank_placeholder() {
    const DOCUMENTED: &[Flag] = &[Flag::text("limit", "", "Page size.")];
    let spec = CommandSpec {
        documented: DOCUMENTED,
        ..registered("get")
    };
    let error = check_metadata(&[&spec]).expect_err("a blank placeholder is rejected");
    assert!(error.contains("--limit"), "{error}");
}

/// An `Enum` value kind with an empty allowed-value list fails the check.
#[test]
fn check_metadata_rejects_an_enum_with_an_empty_allowed_list() {
    const DOCUMENTED: &[Flag] = &[Flag::choice("scope", "SCOPE", "Scope.", &[])];
    let spec = CommandSpec {
        documented: DOCUMENTED,
        ..registered("get")
    };
    let error = check_metadata(&[&spec]).expect_err("an empty allowed list is rejected");
    assert!(error.contains("--scope"), "{error}");
}

/// The declared globals carry the enum lists their consumers parse:
/// `clio_store`'s backend parser accepts the two store backends, and
/// `cli_output::resolve_output` accepts the two output modes.
#[test]
fn the_global_enums_name_the_values_their_consumers_parse() {
    let backend = global_flag("backend").expect("backend is declared");
    let output = global_flag("output").expect("output is declared");
    assert_eq!(
        backend.value.expect("backend takes a value").kind,
        ValueKind::Enum(&["sqlite", "postgres"]),
        "global --backend"
    );
    assert_eq!(
        output.value.expect("output takes a value").kind,
        ValueKind::Enum(&["json", "text"]),
        "global --output"
    );
}

/// A positional with a blank description fails the completeness contract; the
/// error names the record and the positional.
#[test]
fn check_metadata_rejects_a_positional_with_a_blank_description() {
    const POSITIONALS: &[Positional] = &[Positional {
        label: "ID",
        required: true,
        description: "",
    }];
    let spec = CommandSpec {
        positionals: POSITIONALS,
        ..registered("get")
    };
    let error = check_metadata(&[&spec]).expect_err("a blank description is rejected");
    assert!(error.contains("get"), "{error}");
    assert!(error.contains("ID"), "{error}");
}

/// A positional with a blank label fails the completeness contract.
#[test]
fn check_metadata_rejects_a_positional_with_a_blank_label() {
    const POSITIONALS: &[Positional] = &[Positional {
        label: "",
        required: true,
        description: "An id.",
    }];
    let spec = CommandSpec {
        positionals: POSITIONALS,
        ..registered("get")
    };
    let error = check_metadata(&[&spec]).expect_err("a blank label is rejected");
    assert!(error.contains("blank label"), "{error}");
    assert!(error.contains("get"), "{error}");
}

/// The shape the slices must produce — a described positional and two valid
/// examples — passes the whole completeness contract.
#[test]
fn check_metadata_accepts_a_complete_record() {
    const POSITIONALS: &[Positional] = &[Positional {
        label: "ID",
        required: true,
        description: "The memory id.",
    }];
    const EXAMPLES: &[Example] = &[
        Example {
            command: "clio get m1 --gist",
            comment: None,
        },
        Example {
            command: "clio get m1",
            comment: Some("plain text"),
        },
    ];
    let spec = CommandSpec {
        positionals: POSITIONALS,
        examples: EXAMPLES,
        ..registered("get")
    };
    assert_eq!(check_metadata(&[&spec]), Ok(()));
}

/// Fewer than two or more than four examples fails, naming the record; none
/// and one are the missing cases.
#[test]
fn check_examples_rejects_a_count_outside_two_to_four() {
    const ONE: &[Example] = &[Example {
        command: "clio get m1",
        comment: None,
    }];
    const FIVE: &[Example] = &[ONE[0]; 5];
    for examples in [&[][..], ONE, FIVE] {
        let spec = CommandSpec {
            examples,
            ..registered("get")
        };
        let error = check_examples(&spec).expect_err("a bad count is rejected");
        assert!(error.contains("get"), "{error}");
        assert!(error.contains("2 to 4"), "{error}");
    }
}

/// An example naming a command the registry does not know is rejected, and the
/// failure quotes the example text.
#[test]
fn check_example_rejects_an_unknown_command() {
    let example = Example {
        command: "clio frobnicate m1",
        comment: None,
    };
    let error = check_example(&registered("get"), &example).expect_err("an unknown command fails");
    assert!(error.contains("frobnicate"), "{error}");
    assert!(error.contains("example"), "{error}");
}

/// An example naming a real command the carrying record does not own is
/// rejected: the failure names the record and the command it refuses to carry.
#[test]
fn check_example_rejects_an_example_the_record_does_not_own() {
    let example = Example {
        command: "clio backup verify itm-sample-1",
        comment: None,
    };
    let error = check_example(&registered("get"), &example).expect_err("a foreign command fails");
    assert!(error.contains("backup verify"), "{error}");
    assert!(error.contains("does not carry"), "{error}");
    assert!(error.contains("get"), "{error}");
}

/// An example naming a flag the command does not accept is rejected.
#[test]
fn check_example_rejects_a_flag_the_command_does_not_accept() {
    let example = Example {
        command: "clio get m1 --json",
        comment: None,
    };
    let error = check_example(&registered("get"), &example).expect_err("a malformed flag fails");
    assert!(error.contains("--json"), "{error}");
    assert!(error.contains("get"), "{error}");
}

/// An example passing more positionals than the command takes is rejected, and
/// the failure names the extra argument.
#[test]
fn check_example_rejects_an_extra_positional_argument() {
    let example = Example {
        command: "clio get m1 m2",
        comment: None,
    };
    let error = check_example(&registered("get"), &example).expect_err("an extra argument fails");
    assert!(error.contains("m2"), "{error}");
}

/// An example omitting a required positional is rejected, and the failure
/// names the positional.
#[test]
fn check_example_rejects_a_missing_required_positional() {
    let example = Example {
        command: "clio get --gist",
        comment: None,
    };
    let error =
        check_example(&registered("get"), &example).expect_err("a missing positional fails");
    assert!(error.contains("ID"), "{error}");
}

/// A blank example, or one that does not start with `clio `, is rejected.
#[test]
fn check_example_rejects_a_line_without_the_clio_prefix() {
    for command in ["get m1", "clio", ""] {
        let example = Example {
            command,
            comment: None,
        };
        let error =
            check_example(&registered("get"), &example).expect_err("a bad prefix is rejected");
        assert!(error.contains("get"), "{error}: {command}");
    }
}

/// An example whose global flags lead the command still resolves and parses.
#[test]
fn check_example_accepts_a_leading_global_flag() {
    let example = Example {
        command: "clio --db ./x.db get m1",
        comment: None,
    };
    assert_eq!(check_example(&registered("get"), &example), Ok(()));
}

/// An example naming a multi-token command resolves through its whole name.
#[test]
fn check_example_resolves_a_multi_token_command() {
    let example = Example {
        command: "clio task get t1",
        comment: None,
    };
    let spec = crate::cli_command_registry_test_support::record("task get");
    assert_eq!(check_example(spec, &example), Ok(()));
}

/// The documented bare `--bind` spelling parses: a value-optional flag may
/// appear with no value, and a following flag is never taken as its value.
#[test]
fn check_example_accepts_a_bare_value_optional_flag() {
    let spec = registered("mcp http");
    let bare = Example {
        command: "clio mcp http --bind",
        comment: None,
    };
    assert_eq!(check_example(&spec, &bare), Ok(()));
    let followed = Example {
        command: "clio mcp http --bind --sessions",
        comment: None,
    };
    assert_eq!(check_example(&spec, &followed), Ok(()));
}

/// A value-optional flag given twice is the duplicate the real grammar
/// refuses, and a token after `--` is a positional: the strip re-spells
/// neither, so `check_example` refuses both spellings.
#[test]
fn check_example_rejects_a_repeated_or_delimited_bare_flag() {
    let spec = registered("mcp http");
    for command in ["clio mcp http --bind --bind x", "clio mcp http -- --bind"] {
        let example = Example {
            command,
            comment: None,
        };
        let error = check_example(&spec, &example).expect_err("a duplicate or delimiter fails");
        assert!(error.contains("--bind"), "{error}: {command}");
    }
}

/// The documented `--file` alternate of `admit` parses: the file flag stands
/// in for the text positional, while a run with neither still fails.
#[test]
fn check_example_accepts_the_admit_file_alternate() {
    let spec = registered("admit");
    let alternate = Example {
        command: "clio admit --file ops.json",
        comment: None,
    };
    assert_eq!(check_example(&spec, &alternate), Ok(()));
    let without = Example {
        command: "clio admit --bank work",
        comment: None,
    };
    let error = check_example(&spec, &without).expect_err("no text and no file fails");
    assert!(error.contains("TEXT"), "{error}");
}

/// The documented `--latest` selector of the backup commands parses: the
/// toggle stands in for the id positional, while a plain flag run still
/// requires the id.
#[test]
fn check_example_accepts_the_backup_latest_selector() {
    let restore = registered("backup restore");
    let latest = Example {
        command: "clio backup restore --latest --dry-run",
        comment: None,
    };
    assert_eq!(check_example(&restore, &latest), Ok(()));
    let verify = registered("backup verify");
    let verify_latest = Example {
        command: "clio backup verify --latest",
        comment: None,
    };
    assert_eq!(check_example(&verify, &verify_latest), Ok(()));
    let no_id = Example {
        command: "clio backup restore --yes",
        comment: None,
    };
    let error = check_example(&restore, &no_id).expect_err("no id and no selector fails");
    assert!(error.contains("ID"), "{error}");
}

/// The tokenizer groups quoted text, keeps escaped quotes and spaces, keeps an
/// empty quoted token, and refuses a quote left open.
#[test]
fn the_tokenizer_handles_quotes_and_escapes() {
    let joined = |line: &str| split_line(line).expect("tokenizes").join("|");
    assert_eq!(joined("clio get \"m one\" --json"), "clio|get|m one|--json");
    assert_eq!(joined("clio get 'm one'"), "clio|get|m one");
    assert_eq!(joined("clio get m\\ one"), "clio|get|m one");
    assert_eq!(joined("clio recall 'a\\b'"), "clio|recall|a\\b");
    assert_eq!(joined("clio get m\\"), "clio|get|m\\");
    assert_eq!(joined("clio get \"\\\"m\\\"\""), "clio|get|\"m\"");
    assert_eq!(joined("clio get \"\""), "clio|get|");
    assert!(split_line("clio get \"m one").is_err());
}

/// `check_example` maps the tokenizer's open-quote refusal to a failure that
/// names the record and quotes the example text.
#[test]
fn check_example_rejects_an_unbalanced_quote() {
    let example = Example {
        command: "clio get \"m one",
        comment: None,
    };
    let error = check_example(&registered("get"), &example).expect_err("an open quote is rejected");
    assert!(error.contains("get"), "{error}");
    assert!(error.contains("clio get \"m one"), "{error}");
}

/// The dump writes one object per example in record order, with `record`,
/// `command`, and `comment` keys and quoted text escaped.
#[test]
fn the_examples_dump_writes_one_json_object_per_example() {
    const EXAMPLES: &[Example] = &[
        Example {
            command: "clio get m1 --json",
            comment: None,
        },
        Example {
            command: "clio get \"m one\"",
            comment: Some("or the gist"),
        },
    ];
    let spec = CommandSpec {
        examples: EXAMPLES,
        ..registered("get")
    };
    let json = examples_dump_json(&[&spec]);
    assert_eq!(
        json,
        concat!(
            "[{\"record\": \"get\", \"command\": \"clio get m1 --json\", \"comment\": null}, ",
            "{\"record\": \"get\", \"command\": \"clio get \\\"m one\\\"\", \"comment\": \"or the gist\"}]"
        )
    );
}

/// With `CLIO_EXAMPLES_DUMP` set, the seam writes the registry's examples as
/// the dump shape to that path, the written file matches the dump JSON, and it
/// parses as JSON; unset, it does nothing.
#[test]
fn the_examples_dump_seam_writes_the_registry_when_gated() {
    let Ok(path) = std::env::var("CLIO_EXAMPLES_DUMP") else {
        return;
    };
    let specs: Vec<&CommandSpec> = crate::cli_command_registry::all().collect();
    let expected = examples_dump_json(&specs);
    std::fs::write(&path, &expected).expect("the dump path is writable");
    let written = std::fs::read_to_string(&path).expect("the dump path is readable");
    assert_eq!(written, expected);
    serde_json::from_str::<serde_json::Value>(&written).expect("the dump parses as JSON");
}
