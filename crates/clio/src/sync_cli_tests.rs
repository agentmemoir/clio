// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio sync` commands: the single-host explanation,
//! multi-host status, push/pull/serve flag decoding, and the text views.
//!
//! ## Owns
//! - Behavior and view tests for the sync verbs
//!
//! ## Does not own
//! - The command implementation under test (`sync_cli`)
//! - Sync protocol behavior (see `clio-sync`)
//!
//! ## Boundary
//!
//! Keep this module focused on behavior and view tests for the sync verbs.

use super::*;
use crate::cli_args::{self, ParsedArgs};
use crate::cli_read_test_support::state;
use clio_mcp::runtime::McpState;

/// Run one `sync` verb against injected state and a chosen deployment kind.
fn run_sync(
    args: &[&str],
    single_host: bool,
    state: Option<&Arc<McpState>>,
    is_tty: bool,
) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty,
            no_color: true,
            env: &env,
        };
        run(
            &mut io,
            tokens(args),
            single_host,
            state,
            Some(&crate::sigpipe::Serving::for_test()),
        )
    };
    (
        code,
        String::from_utf8(out).expect("utf8"),
        String::from_utf8(err).expect("utf8"),
    )
}

fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn parsed_args(verb: &str, args: &[&str]) -> ParsedArgs {
    cli_args::parse(&spec(verb), tokens(args)).expect("parse")
}

#[test]
fn status_on_a_single_host_explains_and_exits_zero() {
    let (code, out, err) = run_sync(&["status"], true, None, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["single_host"], json!(true), "{out}");
    assert_eq!(payload["sync"], json!("omitted"), "{out}");
    assert!(
        payload["explanation"]
            .as_str()
            .unwrap()
            .contains("single-host exception")
    );

    let (_, text, _) = run_sync(&["status"], true, None, true);
    assert!(text.contains("single-host"), "{text}");
    assert!(text.contains("local store is authoritative"), "{text}");
}

#[test]
fn status_on_a_multi_host_reports_cursors() {
    let s = state();
    let (code, out, err) = run_sync(&["status"], false, Some(&s), false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert!(payload["device_id"].as_str().is_some(), "{payload}");
    assert!(payload["banks"].as_array().is_some(), "{payload}");

    let (_, text, _) = run_sync(&["status"], false, Some(&s), true);
    assert!(text.starts_with("sync status (device"), "{text}");
}

#[test]
fn status_opens_state_through_the_shared_resolver() {
    // `injected = None` exercises the real open path over an in-memory DB.
    let (code, out, err) = run_sync(&["status", "--db", "sqlite::memory:"], false, None, false);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert!(payload["device_id"].as_str().is_some(), "{payload}");

    let (code, _, err) = run_sync(
        &["status", "--backend", "mysql", "--db", "x"],
        false,
        None,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{err}");
}

#[test]
fn push_and_pull_require_a_remote() {
    let s = state();
    for verb in ["push", "pull"] {
        let (code, _, err) = run_sync(&[verb], false, Some(&s), false);
        assert_eq!(code, ExitCode::from(2), "{verb}: {err}");
        assert!(err.contains("--remote"), "{verb}: {err}");
    }
}

#[test]
fn build_decodes_transfer_flags_and_the_token_environment_fallback() {
    let parsed = parsed_args(
        "sync push",
        &[
            "--remote",
            "127.0.0.1:34311",
            "--mode",
            "bidirectional",
            "--banks",
            "demo, other",
            "--token",
            "argv-token",
        ],
    );
    let (tool, args) = build("sync push", &parsed, &|_key| None).expect("build");
    assert_eq!(tool, "sync_push");
    assert_eq!(args["remote"], json!("127.0.0.1:34311"));
    assert_eq!(args["mode"], json!("bidirectional"));
    assert_eq!(args["banks"], json!(["demo", "other"]));
    assert_eq!(args["token"], json!("argv-token"));

    // The env fallback is used only when the flag is absent.
    let parsed = parsed_args("sync pull", &["--remote", "h:1"]);
    let (tool, args) = build("sync pull", &parsed, &|key| {
        (key == "CLIO_SYNC_TOKEN").then(|| format!("env-{key}"))
    })
    .expect("build");
    assert_eq!(tool, "sync_pull");
    assert_eq!(args["token"], json!("env-CLIO_SYNC_TOKEN"));
}

/// A key supplied only through the removed `CLIO_SYNC_KEY` environment variable
/// never reaches the transfer tool arguments, so no direct supply path can
/// populate a transfer key or override the stored custody key.
#[test]
fn a_legacy_environment_key_never_becomes_transfer_material() {
    let parsed = parsed_args("sync push", &["--remote", "h:1"]);
    let (_, args) = build("sync push", &parsed, &|key| {
        (key == "CLIO_SYNC_KEY").then(|| "legacy-key".to_owned())
    })
    .expect("build");
    assert!(args.get("sync_key").is_none(), "{args}");
}

/// The removed `--sync-key` flag fails at the parser with the flag named and a
/// hint to the current key route, so an old command line cannot silently supply
/// a key and the operator is pointed at the store or key file instead.
#[test]
fn the_removed_sync_key_flag_is_rejected_by_the_parser() {
    let s = state();
    let (code, out, err) = run_sync(
        &["push", "--remote", "h:1", "--sync-key", "old-key"],
        false,
        Some(&s),
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--sync-key"), "{err}");
    assert!(err.contains("CLIO_SYNC_KEY_FILE"), "{err}");
    assert!(err.contains("keygen"), "{err}");
}

/// Both TTL arms run, and neither is measured against the clock.
///
/// The sleep arm used to be reached by a test that moved to another file when the
/// render views were split out, which left the line counting against this file's
/// own tests with nothing reaching it. One millisecond is the smallest TTL that is
/// not zero, so the arm is taken without making the suite wait, and the zero arm
/// returns at once.
///
/// No elapsed time is asserted in either arm. `wait_for_ttl` has no clock seam, so
/// a bound here would be a claim about the machine rather than about the code: one
/// deschedule longer than the bound fails on a loaded runner while the code stands.
#[test]
fn wait_for_ttl_returns_for_a_zero_and_a_non_zero_ttl() {
    wait_for_ttl(Duration::ZERO, &crate::sigpipe::Serving::for_test());
    wait_for_ttl(
        Duration::from_millis(1),
        &crate::sigpipe::Serving::for_test(),
    );
}

#[test]
fn build_rejects_a_bad_mode_and_an_empty_bank_list() {
    let parsed = parsed_args("sync push", &["--remote", "h:1", "--mode", "bogus"]);
    let error = build("sync push", &parsed, &|_| None).expect_err("bad mode");
    assert_eq!(error.exit_code(), ExitCode::from(2));

    let parsed = parsed_args("sync status", &["--banks", " , "]);
    let error = build("sync status", &parsed, &|_| None).expect_err("empty banks");
    assert_eq!(error.exit_code(), ExitCode::from(2));
}

#[test]
fn build_decodes_serve_flags() {
    let parsed = parsed_args(
        "sync serve",
        &[
            "--bind",
            "127.0.0.1:0",
            "--banks",
            "demo",
            "--ttl-secs",
            "0",
            "--dev",
        ],
    );
    let (tool, args) = build("sync serve", &parsed, &|_| None).expect("build");
    assert_eq!(tool, "sync_serve");
    assert_eq!(args["bind"], json!("127.0.0.1:0"));
    assert_eq!(args["banks"], json!(["demo"]));
    assert_eq!(args["ttl_secs"], json!(0));
    assert_eq!(args["dev"], json!(true));
}

#[test]
fn serve_with_dev_and_zero_ttl_runs_and_exits() {
    let s = state();
    let (code, out, err) = run_sync(
        &["serve", "--dev", "--ttl-secs", "0"],
        false,
        Some(&s),
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert!(payload["addr"].as_str().is_some(), "{payload}");
    assert_eq!(payload["ttl_secs"], json!(0), "{payload}");

    let (_, text, _) = run_sync(
        &["serve", "--dev", "--ttl-secs", "0"],
        false,
        Some(&s),
        true,
    );
    assert!(text.contains("sync serve listening on"), "{text}");
    wait_for_ttl(Duration::ZERO, &crate::sigpipe::Serving::for_test());
}

#[test]
fn serve_refuses_an_anonymous_bind_outside_dev_mode() {
    let s = state();
    let (code, _, err) = run_sync(&["serve"], false, Some(&s), false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("auth material"), "{err}");
}

#[test]
fn unknown_subcommand_and_missing_subcommand_exit_two() {
    let (code, _, err) = run_sync(&["bogus"], true, None, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("unknown `sync` subcommand `bogus`"), "{err}");

    let (code, _, err) = run_sync(&[], true, None, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("requires a subcommand"), "{err}");
}

#[test]
fn help_exits_zero_for_the_group_and_a_verb() {
    let (code, out, err) = run_sync(&["--help"], true, None, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("`sync` subcommands:"), "{out}");
    assert!(out.contains("clio sync status"), "{out}");

    let (code, out, err) = run_sync(&["status", "--help"], true, None, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("usage: clio sync status"), "{out}");
    assert!(out.contains("calls the `sync_status` tool"), "{out}");
}

#[test]
fn spec_lists_the_expected_flags() {
    assert!(spec("sync status").value_flags().contains(&"banks"));
    assert!(!spec("sync status").value_flags().contains(&"remote"));
    assert!(!spec("sync push").value_flags().contains(&"sync-key"));
    assert!(
        spec("sync serve").value_flags().contains(&"token-file"),
        "serve keeps its scoped credential file"
    );
    assert!(
        !spec("sync serve").value_flags().contains(&"token"),
        "serve no longer takes the removed single token"
    );
    assert!(spec("sync serve").bool_flags().contains(&"dev"));
    assert!(!spec("sync serve").bool_flags().contains(&"dry-run"));
}

#[test]
fn sync_entrypoint_covers_the_wrapper_and_single_host_detection() {
    assert_eq!(sync(vec!["--help".to_owned()]), ExitCode::SUCCESS);
    let _ = config_single_host();
}

#[test]
fn the_entrypoint_routes_the_key_verbs_to_the_key_surface() {
    // `sync` hands `key` and `keygen` to the key surface before its own parser,
    // so these reach the key verbs rather than `sync`'s serve flags.
    assert_eq!(sync(vec!["key".to_owned()]), ExitCode::from(2));
    assert_eq!(
        sync(vec![
            "keygen".to_owned(),
            "--store".to_owned(),
            "bogus".to_owned()
        ]),
        ExitCode::from(2)
    );
    assert_eq!(
        sync(vec!["key".to_owned(), "--help".to_owned()]),
        ExitCode::SUCCESS
    );
}

#[test]
fn status_accepts_banks_and_rejects_bad_flags_and_output() {
    let s = state();
    let (code, out, err) = run_sync(&["status", "--banks", "demo"], false, Some(&s), false);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");

    let (code, _, err) = run_sync(&["status", "--bogus"], true, None, false);
    assert_eq!(code, ExitCode::from(2), "{err}");

    let (code, _, err) = run_sync(&["status", "--output", "xml"], true, None, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
}

#[test]
fn single_host_status_rejects_bad_args_like_multi_host() {
    // Usage validation runs before the single-host short-circuit.
    let (code, _, err) = run_sync(&["status", "unexpected_arg"], true, None, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("unexpected extra argument"), "{err}");

    // An explicitly empty `--banks` is refused by the parser's one rule.
    let (code, _, err) = run_sync(&["status", "--banks", ""], true, None, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--banks must not be empty"), "{err}");
}

#[test]
fn unknown_subcommand_suggests_a_close_verb() {
    let (code, _, err) = run_sync(&["statuss"], true, None, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("did you mean `clio sync status`?"), "{err}");
}

/// The removed `--token` on serve fails at the parser with the flag named and the
/// suggester's own hint to the scoped `--token-file`, so an old serve command line
/// cannot silently start a server on the removed shared token and the operator is
/// pointed at the credential store instead.
#[test]
fn the_removed_serve_token_flag_is_rejected_by_the_parser() {
    let s = state();
    let (code, out, err) = run_sync(
        &["serve", "--token", "old-token", "--dev", "--ttl-secs", "0"],
        false,
        Some(&s),
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("unknown flag `--token`"), "{err}");
    assert!(err.contains("did you mean `--token-file`?"), "{err}");
}

/// A token supplied only through `CLIO_SYNC_TOKEN` never reaches the serve
/// arguments: the environment rung supplied the removed single token, so no
/// environment value can populate a server credential.
#[test]
fn the_token_environment_variable_never_reaches_serve_arguments() {
    let parsed = parsed_args("sync serve", &["--dev"]);
    let (_, args) = build("sync serve", &parsed, &|key| {
        (key == "CLIO_SYNC_TOKEN").then(|| "env-token".to_owned())
    })
    .expect("build");
    assert!(args.get("token").is_none(), "{args}");
}

#[test]
fn serve_passes_a_token_file_and_the_client_verbs_reject_it() {
    let parsed = parsed_args("sync serve", &["--token-file", "/tmp/store.json", "--dev"]);
    let (_, args) = build("sync serve", &parsed, &|_| None).expect("build");
    assert_eq!(args["token_file"], json!("/tmp/store.json"));
    assert!(
        !spec("sync push").value_flags().contains(&"token-file"),
        "the client verbs must not gain the server-only flag"
    );
    assert!(
        cli_args::parse(
            &spec("sync push"),
            tokens(&["--remote", "h:1", "--token-file", "x"])
        )
        .is_err()
    );
}

/// Serve's `--help` keeps advertising the scoped credential file and its
/// start-time reload rule, and never advertises the removed single token or the
/// environment rung that supplied it.
#[test]
fn serve_help_documents_the_token_file_and_never_the_removed_token() {
    let (code, out, err) = run_sync(&["serve", "--help"], false, None, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("usage: clio sync serve"), "{out}");
    assert!(out.contains("--token-file PATH"), "{out}");
    assert!(
        out.contains("revocation takes effect at the next start"),
        "{out}"
    );
    assert!(!out.contains("[--token T]"), "{out}");
    assert!(!out.contains("CLIO_SYNC_TOKEN"), "{out}");
    assert!(!out.contains("legacy single token"), "{out}");
}
