// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Retention CLI tests: subcommand behavior, exit codes, and store round-trip.

use super::*;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

fn flags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn tmp_store(name: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-cli-retention-{name}-{n}.json"))
}

#[test]
fn schema_prints_the_published_mapping_tables() {
    let out = run("schema", &HashMap::new()).expect("schema");
    assert!(out.contains("x-verbosity-mapping"));
    assert!(out.contains("x-duplicate-tolerance-write-mapping"));
    assert!(out.contains("x-retention-profile-version"));
    assert!(!out.contains("\"title\""));
}

#[test]
fn set_then_get_round_trips_through_the_store_file() {
    let path = tmp_store("roundtrip");
    let path_str = path.to_string_lossy().into_owned();
    let set = run(
        "set",
        &flags(&[
            ("profiles", &path_str),
            ("bank", "bank-a"),
            ("profile", r#"{"verbosity":"selective"}"#),
        ]),
    )
    .expect("set");
    assert!(set.contains("\"persisted\": true"));
    assert!(set.contains("accepted_but_inert"));

    let get = run(
        "get",
        &flags(&[("profiles", &path_str), ("bank", "bank-a")]),
    )
    .expect("get");
    assert!(get.contains("\"source\": \"bank\""));
    assert!(get.contains("\"verbosity\": \"selective\""));
    assert!(get.contains("honored_by"));

    // A bank with no override resolves to the builtin default.
    let other = run(
        "get",
        &flags(&[("profiles", &path_str), ("bank", "bank-b")]),
    )
    .expect("get b");
    assert!(other.contains("\"source\": \"builtin\""));

    let _ = std::fs::remove_file(path);
}

#[test]
fn set_reads_a_profile_file_and_the_deployment_scope() {
    let path = tmp_store("deploy");
    let path_str = path.to_string_lossy().into_owned();
    let profile_file = tmp_store("profile");
    std::fs::write(&profile_file, r#"{"verbosity":"selective"}"#).expect("write profile");
    let profile_str = profile_file.to_string_lossy().into_owned();

    let out = run(
        "set",
        &flags(&[
            ("profiles", &path_str),
            ("scope", "deployment"),
            ("profile-file", &profile_str),
        ]),
    )
    .expect("set deployment");
    assert!(out.contains("\"scope\": \"deployment\""));
    assert!(out.contains("\"bank\": null"));

    let get = run(
        "get",
        &flags(&[("profiles", &path_str), ("bank", "bank-z")]),
    )
    .expect("get z");
    assert!(get.contains("\"source\": \"deployment\""));
    assert!(get.contains("\"verbosity\": \"selective\""));

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(profile_file);
}

#[test]
fn set_rejects_missing_and_invalid_inputs() {
    let path = tmp_store("errors");
    let path_str = path.to_string_lossy().into_owned();
    assert_eq!(
        run("set", &flags(&[("profiles", &path_str)]))
            .expect_err("missing profile")
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        run(
            "set",
            &flags(&[("profiles", &path_str), ("profile", "{not json")])
        )
        .expect_err("bad json")
        .code,
        ErrorCode::InvalidType
    );
    assert_eq!(
        run(
            "set",
            &flags(&[
                ("profiles", &path_str),
                ("profile", r#"{"verbosity":"verbose"}"#)
            ])
        )
        .expect_err("bad verbosity")
        .code,
        ErrorCode::InvalidType
    );
    assert_eq!(
        run(
            "set",
            &flags(&[
                ("profiles", &path_str),
                ("profile", "{}"),
                ("scope", "global")
            ])
        )
        .expect_err("bad scope")
        .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        run(
            "set",
            &flags(&[
                ("profiles", &path_str),
                ("profile", "{}"),
                ("scope", "bank"),
                ("bank", "")
            ])
        )
        .expect_err("empty bank")
        .code,
        ErrorCode::MissingBank
    );
    assert_eq!(
        run(
            "set",
            &flags(&[
                ("profiles", &path_str),
                ("profile", r#"{"theta_offsets":{"persona":0.9}}"#)
            ])
        )
        .expect_err("oversize offset")
        .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        run("get", &flags(&[("profiles", &path_str), ("bank", "")]))
            .expect_err("empty bank get")
            .code,
        ErrorCode::MissingBank
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn set_rejects_an_unreadable_profile_file() {
    let path = tmp_store("unreadable");
    let path_str = path.to_string_lossy().into_owned();
    let err = run(
        "set",
        &flags(&[
            ("profiles", &path_str),
            ("profile-file", "/nonexistent/retention-profile.json"),
        ]),
    )
    .expect_err("unreadable profile file");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("failed to read profile file"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn unknown_subcommand_is_an_error_when_run_directly() {
    assert_eq!(
        run("bogus", &HashMap::new()).expect_err("unknown sub").code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn missing_store_configuration_is_an_error() {
    // `run` is reached only for known subcommands; an explicit path always
    // works, so the unconfigured branch is exercised through `open_runtime`.
    let err = open_runtime(&HashMap::new()).expect_err("no store configured");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn exit_codes_distinguish_usage_from_failure() {
    assert_eq!(
        retention(vec!["bogus".to_owned()]),
        ExitCode::from(2),
        "unknown subcommand is a usage error"
    );
    assert_eq!(
        retention(vec!["set".to_owned()]),
        ExitCode::from(1),
        "missing profile is a runtime failure"
    );
    assert_eq!(retention(vec!["schema".to_owned()]), ExitCode::SUCCESS);
    // `retention` with no subcommand at all is a usage error too.
    assert_eq!(retention(Vec::<String>::new()), ExitCode::from(2));
}

#[test]
fn profile_from_flags_parses_inline_json() {
    let parsed =
        profile_from_flags(&flags(&[("profile", r#"{"verbosity":"selective"}"#)])).expect("valid");
    assert_eq!(parsed.verbosity, clio::retention::Verbosity::Selective);
    assert!(profile_from_flags(&flags(&[("profile", "{")])).is_err());
}

/// Each subcommand's help is the registry's usage line plus what that subcommand
/// does, so `clio retention get --help` cannot run the command instead.
///
/// The claim under test is the one the reported defect broke: `--help` was never
/// read here, so `retention get --help` reported the retention store missing.
#[test]
fn each_subcommands_help_carries_its_own_usage_and_prose() {
    for (sub, expected) in [
        ("get", "Shows the effective per-bank profile"),
        ("set", "Stores a validated profile"),
        ("schema", "Prints the retention profile JSON Schema"),
    ] {
        let text = super::retention_help(sub);
        assert!(
            text.contains(&format!("clio retention {sub}")),
            "`clio retention {sub} --help` must print its usage line: {text}"
        );
        assert!(
            text.contains(expected),
            "`clio retention {sub} --help` must say what it does: {text}"
        );
        // A help screen that never names a flag reads as a command that takes none.
        if sub != "schema" {
            assert!(
                text.contains("--"),
                "`clio retention {sub} --help` names no flag: {text}"
            );
        }
    }
}

/// The refusal carries the message and the hint on separate lines and exits 2.
///
/// An error that exits 0 is the defect this replaced: an unrecognised
/// subcommand used to report success.
#[test]
fn a_refusal_names_the_hint_and_exits_two() {
    assert_eq!(
        super::eprintln_error(
            "unknown retention command `bogus`",
            "try `clio retention get`"
        ),
        ExitCode::from(2)
    );
}

/// A help spelling where a subcommand belongs prints the subcommand list and
/// exits 0, so `clio retention --help` and `clio help retention` answer the
/// question instead of refusing `--help` as an unknown command.
#[test]
fn a_top_level_help_spelling_prints_the_subcommand_list() {
    for spelling in ["--help", "help", "-h"] {
        assert_eq!(
            retention(vec![spelling.to_owned()]),
            ExitCode::SUCCESS,
            "clio retention {spelling} must print help"
        );
    }
}

/// A profile is also read from the file `--profile-file` names, and an inline
/// `--profile` wins when both are given.
///
/// The inline case alone is covered beside this; the file case and the precedence
/// are not, and precedence is the part a reader cannot check from the help text.
#[test]
fn a_profile_comes_from_the_file_it_names_and_the_inline_value_wins() {
    let path = tmp_store("profile-from-file");
    std::fs::write(&path, r#"{"verbosity":"permissive"}"#).expect("write");
    let path_str = path.to_string_lossy().into_owned();

    let from_file = super::profile_from_flags(&flags(&[("profile-file", &path_str)]))
        .expect("profile from file");
    assert_eq!(
        from_file.verbosity,
        clio::retention::Verbosity::Permissive,
        "{from_file:?}"
    );

    // Both given: the inline value is the one typed last, so it is the one read.
    let both = super::profile_from_flags(&flags(&[
        ("profile", r#"{"verbosity":"selective"}"#),
        ("profile-file", &path_str),
    ]))
    .expect("both");
    assert_eq!(
        both.verbosity,
        clio::retention::Verbosity::Selective,
        "{both:?}"
    );

    // A file that is not there is a refusal that names the path, not a default.
    let missing = super::profile_from_flags(&flags(&[(
        "profile-file",
        "/nonexistent/clio/profile.json",
    )]))
    .expect_err("no such file");
    let reason = format!("{missing}");
    assert!(reason.contains("profile file"), "{reason}");
    assert!(
        reason.contains("/nonexistent/clio/profile.json"),
        "{reason}"
    );

    // Malformed JSON is a refusal too, not a silently empty profile.
    let broken = super::profile_from_flags(&flags(&[("profile", "{")])).expect_err("bad json");
    assert!(
        format!("{broken}").contains("invalid retention profile"),
        "{broken}"
    );

    let _ = std::fs::remove_file(path);
}

/// A profile given as neither is refused, and the refusal names both ways to give
/// one, so the caller does not have to read the usage line to find out.
#[test]
fn a_profile_given_as_neither_is_refused_with_both_ways_to_give_one() {
    let reason = format!(
        "{}",
        super::profile_from_flags(&HashMap::new()).expect_err("no profile at all")
    );
    assert!(reason.contains("--profile "), "{reason}");
    assert!(reason.contains("--profile-file"), "{reason}");
}

#[test]
fn an_undeclared_retention_flag_is_a_usage_error() {
    assert_eq!(
        retention(vec!["get".to_owned(), "--bogus".to_owned()]),
        ExitCode::from(2),
        "an unknown flag fails closed instead of running with defaults"
    );
}

/// A bare `help` token is one of the spellings the surface answers, and the help
/// scan runs before the parser, so a help request is never turned into a usage
/// error. `retention get help` printed help on master and must keep doing so.
#[test]
fn a_bare_help_token_prints_the_subcommand_help() {
    for sub in ["get", "set", "schema"] {
        assert_eq!(
            retention(vec![sub.to_owned(), "help".to_owned()]),
            ExitCode::SUCCESS,
            "{sub}"
        );
    }
}
