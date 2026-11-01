// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the erasure-denial wiring of the shipping sync routes:
//! the CLI serve verb and the bound tool both serve through the journal this
//! deployment resolves — a bank a recorded denial covers is refused while an
//! unrelated bank still serves, a damaged journal refuses every pull, a
//! journal file that does not exist yet is clear — a deployment that cannot
//! name its journal is refused rather than served unguarded, and the `sync
//! pull` verb refuses before any transport work when the journal its state
//! names is not the one the deployment resolves.
//!
//! ## Owns
//! - Behavior coverage for the serve routes' denial-aware branch and for their
//!   fail-closed start
//! - The verb's own start-and-answer contract, which needs a resolved journal
//! - The pull verb's refusal when its transfer cannot resolve a journal
//! - The scratch journal fixture these wiring tests share
//!
//! ## Does not own
//! - The server's own denial behavior (`clio-sync` `server_denial_tests`)
//! - The verb decoding and refusal shapes (`sync_cli_tests`)
//! - The probe and its construction (`clio-compliance` `journal_denial_tests`)
//!
//! ## Boundary
//!
//! These tests break when a serve route's authority wiring changes; they never
//! change it.
//!
//! Keep this module focused on the serve routes' denial wiring.

use super::*;
use crate::sync_cli::sync_cli_serve::start_serve;
use clio_compliance::JournalDenialProbe;
use clio_store::ErasureDenial;
use clio_sync::denial::denied_bank_marker;
use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One scratch journal path, removed on drop: the crate's fixture pattern —
/// the pid and a nanosecond stamp keep parallel tests apart.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let file = format!("am-sync-cli-{name}-{}-{nanos}.json", std::process::id());
        Self(std::env::temp_dir().join(file))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Seed bytes that are not a journal, restricted to the owner where mode
    /// bits exist (the shape `journal_denial_tests` seeds its damaged one with).
    fn damage(&self) {
        std::fs::write(&self.0, b"not a journal").expect("seed the damaged journal");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o600))
                .expect("restrict the seeded journal");
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The deployment environment naming `journal` as this host's erasure journal.
fn journal_env(journal: &Scratch) -> impl Fn(&str) -> Option<String> + '_ {
    move |key: &str| {
        (key == clio_config::ERASURE_JOURNAL_FILE_ENV)
            .then(|| journal.path().to_string_lossy().into_owned())
    }
}

/// Record a bank-wide denial through the probe, so the journal on disk holds it
/// the way the erase path would.
fn record_bank_denial(journal: &Scratch, bank: &str) {
    JournalDenialProbe::new(journal.path().to_path_buf())
        .record_denial(&denied_bank_marker(bank), &[bank.to_owned()])
        .expect("record the bank denial");
}

/// One pull of `bank`; returns the status and the decoded body.
fn pull(addr: &str, bank: &str) -> (u16, Value) {
    let body = serde_json::to_vec(&json!({ "bank": bank, "cursor": "", "limit": 10 }))
        .expect("serialize pull");
    let head = format!(
        "POST /sync/pull HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&body))
        .expect("send the pull");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read the response");
    let text = String::from_utf8_lossy(&raw);
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = text.find("\r\n\r\n").map_or(text.len(), |p| p + 4);
    let value = serde_json::from_str(text[body_start..].trim()).expect("decode the body");
    (status, value)
}

/// The serve arguments one `--dev --bind 127.0.0.1:0 --banks b-denied,b-open`
/// command line decodes to, built through the verb's own decoder.
fn serve_args(banks: &str) -> Value {
    let parsed = parsed_args(
        "sync serve",
        &["--dev", "--bind", "127.0.0.1:0", "--banks", banks],
    );
    let (tool, args) = build("sync serve", &parsed, &|_| None).expect("build");
    assert_eq!(tool, "sync_serve");
    args
}

/// The first line the verb answers with, read on a helper thread with a bound
/// so a verb that never answers fails the test instead of hanging it (the same
/// reader a process-level harness for a started server uses).
fn answer(reader: std::io::PipeReader) -> String {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufReader::new(reader).read_line(&mut line);
        let _ = tx.send(line);
    });
    rx.recv_timeout(Duration::from_secs(30)).unwrap_or_default()
}

/// A resolved journal path serves the server through the denial authority: a
/// bank a recorded denial covers is refused with the erased-subject reason while
/// an unrelated bank still serves. The server is started through the same
/// function the `sync serve` verb runs, so this pins the production branch.
#[test]
fn a_configured_journal_refuses_a_denied_bank_and_serves_the_rest() {
    let journal = Scratch::new("serve-denial");
    record_bank_denial(&journal, "b-denied");
    let get = journal_env(&journal);
    let (server, payload) = start_serve(&state(), &serve_args("b-denied,b-open"), &get)
        .expect("the serve process starts");
    assert!(payload["addr"].as_str().is_some(), "{payload}");

    let (status, value) = pull(server.addr(), "b-denied");
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "erased_subject", "{value}");
    let (status, value) = pull(server.addr(), "b-open");
    assert_eq!(status, 200, "an unrelated bank still serves: {value}");
    server.stop();
}

/// A damaged journal is an unreadable authority: the server starts, but every
/// pull it would have to clear is refused as the server's own failure, never
/// served as if the journal were clear.
#[test]
fn a_damaged_journal_refuses_every_covered_pull() {
    let journal = Scratch::new("serve-damaged");
    journal.damage();
    let get = journal_env(&journal);
    let (server, payload) = start_serve(&state(), &serve_args("b-denied,b-open"), &get)
        .expect("the serve process starts");
    assert!(payload["addr"].as_str().is_some(), "{payload}");

    for bank in ["b-denied", "b-open"] {
        let (status, value) = pull(server.addr(), bank);
        assert_eq!(status, 500, "{bank}: {value}");
        assert_eq!(value["code"], "config_corrupt", "{bank}: {value}");
    }
    server.stop();
}

/// A journal path that resolves but holds no file yet is a fresh deployment
/// (a missing journal is a clear one): the server serves every bank it covers.
#[test]
fn a_resolved_journal_path_with_no_file_serves_the_banks() {
    let journal = Scratch::new("serve-absent");
    let get = journal_env(&journal);
    let (server, payload) = start_serve(&state(), &serve_args("b-denied,b-open"), &get)
        .expect("the serve process starts");
    assert!(payload["addr"].as_str().is_some(), "{payload}");

    for bank in ["b-denied", "b-open"] {
        let (status, value) = pull(server.addr(), bank);
        assert_eq!(status, 200, "no recorded denial covers {bank}: {value}");
    }
    server.stop();
}

/// A deployment that cannot name its journal is refused before any bind: a
/// serve process without the authority would ignore every recorded denial, so
/// the refusal is the answer and the override that fixes it is named.
#[test]
fn an_unresolvable_journal_path_refuses_to_start() {
    let env = |_key: &str| -> Option<String> { None };
    let error = start_serve(&state(), &serve_args("b-denied,b-open"), &env)
        .err()
        .expect("a serve route must not start without its authority");
    assert_eq!(error.exit_code(), ExitCode::from(1), "{error:?}");
    assert_eq!(error.code(), "config_corrupt", "{error:?}");
    let message = error.message();
    assert!(
        message.contains(clio_config::ERASURE_JOURNAL_FILE_ENV),
        "the refusal names the override that fixes it: {message}"
    );
}

/// The bound tool's own start refuses the same way when this deployment cannot
/// name a journal: an MCP serve route without the authority would ignore every
/// recorded denial, so the refusal is its answer too.
#[test]
fn the_bound_serve_tool_refuses_to_start_without_a_resolvable_journal() {
    let env = |_key: &str| -> Option<String> { None };
    let error = clio_mcp::sync_tools::sync_serve_with(&state(), &json!({ "dev": true }), &env)
        .expect_err("a serve route must not start without its authority");
    assert_eq!(
        error.code,
        clio_types::ErrorCode::ConfigCorrupt,
        "{error:?}"
    );
    assert!(
        error
            .message
            .contains(clio_config::ERASURE_JOURNAL_FILE_ENV),
        "the refusal names the override that fixes it: {error}"
    );
}

/// The verb starts, answers with the bound address, and exits once its TTL is
/// spent — with a journal path that resolves, which is the configuration every
/// serve start now needs.
#[test]
fn serve_with_dev_and_zero_ttl_runs_and_exits() {
    let s = state();
    let journal = Scratch::new("serve-surface");
    let get = journal_env(&journal);
    let (code, out, err) = run_sync_env(
        &["serve", "--dev", "--ttl-secs", "0"],
        false,
        Some(&s),
        false,
        &get,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert!(payload["addr"].as_str().is_some(), "{payload}");
    assert_eq!(payload["ttl_secs"], json!(0), "{payload}");

    let (_, text, _) = run_sync_env(
        &["serve", "--dev", "--ttl-secs", "0"],
        false,
        Some(&s),
        true,
        &get,
    );
    assert!(text.contains("sync serve listening on"), "{text}");
}

/// The verb's own dispatch carries the journal authority: through `run`, a pull
/// for a bank a recorded denial covers is refused as erased and an unrelated
/// bank still serves. A dispatch that lost the authority would serve both.
///
/// The verb holds the process for its TTL, so the served window is ended by
/// `release_serve_hold` once the two pulls are done rather than by waiting the
/// TTL out: nothing here is timed.
#[test]
fn the_serve_verb_serves_under_the_journals_denial_authority() {
    let journal = Scratch::new("serve-verb");
    record_bank_denial(&journal, "b-denied");
    let get = journal_env(&journal);
    let s = state();
    let (reader, writer) = std::io::pipe().expect("pipe");
    let (denied, open, code, err) = std::thread::scope(|scope| {
        let verb = scope.spawn(move || {
            let (mut out, mut err) = (writer, Vec::new());
            let mut input = std::io::empty();
            let code = {
                let mut io = CliIo {
                    out: &mut out,
                    err: &mut err,
                    stdin: &mut input,
                    is_tty: false,
                    no_color: true,
                    env: &get,
                };
                run(
                    &mut io,
                    tokens(&[
                        "serve",
                        "--dev",
                        "--bind",
                        "127.0.0.1:0",
                        "--banks",
                        "b-denied,b-open",
                        "--ttl-secs",
                        "30",
                    ]),
                    false,
                    Some(&s),
                    Some(&crate::sigpipe::Serving::for_test()),
                )
            };
            (code, String::from_utf8(err).expect("utf8"))
        });
        // The answer is written once the server is bound, so it is the moment
        // the started server is reachable.
        let line = answer(reader);
        let payload: Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|error| panic!("the serve verb must answer with json: {error}"));
        let addr = payload["addr"].as_str().expect("the bound address");
        let denied = pull(addr, "b-denied");
        let open = pull(addr, "b-open");
        release_serve_hold();
        let (code, err) = verb.join().expect("the serve verb thread");
        (denied, open, code, err)
    });

    assert_eq!(denied.0, 403, "{}", denied.1);
    assert_eq!(denied.1["code"], "erased_subject", "{}", denied.1);
    assert_eq!(open.0, 200, "an unrelated bank still serves: {}", open.1);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
}

/// The bound tool's own serve run carries the same authority: the server it
/// starts refuses the denied bank's pull and serves the unrelated bank's.
#[test]
fn the_bound_serve_tool_serves_under_the_journals_denial_authority() {
    let journal = Scratch::new("serve-tool");
    record_bank_denial(&journal, "b-denied");
    let get = journal_env(&journal);
    let payload = clio_mcp::sync_tools::sync_serve_with(
        &state(),
        &json!({ "dev": true, "ttl_secs": 30, "banks": ["b-denied", "b-open"] }),
        &get,
    )
    .expect("the tool starts the server");
    let addr = payload["addr"].as_str().expect("the bound address");

    let (status, value) = pull(addr, "b-denied");
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "erased_subject", "{value}");
    let (status, value) = pull(addr, "b-open");
    assert_eq!(status, 200, "an unrelated bank still serves: {value}");
}

/// The `sync pull` verb runs its in-process transfer under the journal its
/// state names: a state built from an effective document naming a journal the
/// deployment does not resolve refuses the verb before any transport work,
/// naming the override that fixes it. A transfer that ignored the authority
/// would have gone to the wire instead.
#[test]
fn the_pull_verb_refuses_a_journal_the_deployment_does_not_resolve() {
    let journal = Scratch::new("pull-document");
    let s = McpState::open_with_effective(
        &McpOpenOptions::default(),
        &json!({ "erasure_journal_file": journal.path().display().to_string() }),
    )
    .expect("state");
    let (code, out, err) = run_sync(&["pull", "--remote", "127.0.0.1:1"], false, Some(&s), false);
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(
        err.contains("differs from the deployment resolution"),
        "the refusal names the divergence: {err}"
    );
    assert!(
        err.contains(clio_config::ERASURE_JOURNAL_FILE_ENV),
        "the refusal names the override that fixes it: {err}"
    );
}
