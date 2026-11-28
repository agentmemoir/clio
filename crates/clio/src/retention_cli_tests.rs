// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the `clio retention` command surface:
//! how each subcommand decodes its flags and reads or writes the retention
//! store.
//!
//! ## Owns
//! - `retention get|set|schema` subcommand behaviour and its refusals
//! - the profile read from `--profile` and `--profile-file`, including the
//!   input cap
//! - the store round-trip through the file `--profiles` names
//!
//! ## Does not own
//! - the help screens and exit codes (see `retention_cli_help_tests`)
//! - the capped read of the profile file (see `cli_read_cap`)
//! - profile mapping numbers or the storage format (see `clio-config`)
//!
//! ## Boundary
//!
//! Changes to a subcommand's decoding, its refusals, or its store interaction
//! should normally require changes here. Changes to a help screen or to the
//! shared capped read should not.
//!
//! Keep this module focused on the retention subcommands' behaviour.

use super::*;
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
fn get_refuses_a_missing_profiles_file() {
    let path = tmp_store("missing");
    let path_str = path.to_string_lossy().into_owned();
    let error = run("get", &flags(&[("profiles", &path_str)]))
        .expect_err("a configured missing file is not a stored profile");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("profiles file does not exist"));
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
    assert!(err.message.contains("is not readable"), "{}", err.message);
    assert!(
        err.message.contains("/nonexistent/retention-profile.json"),
        "the refusal must name the path it could not read: {}",
        err.message
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn set_rejects_a_profile_file_over_the_input_cap() {
    let path = tmp_store("oversized");
    let mut body = vec![b' '; crate::cli_read_cap::MAX_INPUT_BYTES + 1];
    body[0] = b'{';
    body[1] = b'}';
    std::fs::write(&path, &body).expect("write oversized profile");
    let path_str = path.to_string_lossy().into_owned();
    let store = tmp_store("oversized-store");
    let store_str = store.to_string_lossy().into_owned();
    let err = run(
        "set",
        &flags(&[("profiles", &store_str), ("profile-file", &path_str)]),
    )
    .expect_err("oversized profile file");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("4 MiB input cap"), "{}", err.message);
    // The hint names the fix, so a message that stops at the cap is incomplete.
    assert!(
        err.message.contains(PROFILE_TOO_LARGE_HINT),
        "the refusal must offer the alternative: {}",
        err.message
    );
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
fn profile_from_flags_parses_inline_json() {
    let parsed =
        profile_from_flags(&flags(&[("profile", r#"{"verbosity":"selective"}"#)])).expect("valid");
    assert_eq!(parsed.verbosity, clio::retention::Verbosity::Selective);
    assert!(profile_from_flags(&flags(&[("profile", "{")])).is_err());
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
    assert!(reason.contains("--profile-file"), "{reason}");
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
