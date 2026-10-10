// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the answer scripts the wizard test suites
//! drive: one constant per route, each spelling out every line the operator
//! types.
//!
//! ## Owns
//! - The route scripts: SQLite, managed Postgres, pasted, typed, services
//! - The two config-path constants, and the environment that names nothing
//! - The flag list that answers the typed route outright
//!
//! ## Does not own
//! - Driving a run (see `setup_preview_fixtures`)
//! - Any assertion: each suite asserts on what it scripted
//!
//! ## Boundary
//!
//! Changes to the order or number of questions a route asks should normally
//! require changes here, because this is where the scripts that depend on that
//! order live. Changes to how a run is driven should not require changes here.
//!
//! Keep this module focused on the inputs a run is scripted with.
//!
//! # Why the scripts live in one place
//!
//! They are the one definition of how many questions each route asks. A suite
//! that counted for itself drifted: one question added to a route left the other
//! suites' scripts one line short, and the run then stopped with a refusal that
//! reads like a bug in the wizard. One blank line is one answer, so a reader can
//! count the questions and check the script against them.

#![allow(
    dead_code,
    reason = "a shared harness is used by several suites, not all at once"
)]

/// Where a harness run believes the deployment config lives.
///
/// A path under a directory the harness never creates, so `will_back_up` reads as
/// "there is no config file yet" without the harness touching the filesystem at
/// all. A test that wants the existing-install line says so through
/// `Harness::with_existing_config`.
pub(crate) const ABSENT_CONFIG: &str = "/nonexistent/clio-stage5/absent.json";

/// The config path a harness run believes exists.
///
/// Separate from [`ABSENT_CONFIG`] so a test can read which one it got: a fresh
/// install must print the plan's `there is no config file yet` phrase, and an
/// existing install must name a path. Asserting on which path constant was used
/// would test the harness, so the assertions are on the printed line.
pub(crate) const EXISTING_CONFIG: &str = "/nonexistent/clio-stage5/deployment.json";

/// SQLite, the shortest route: which database, and the file.
///
/// The file has a default, so the second line is a blank. One line shorter than
/// every other route, which is why a test wanting a short script takes this one.
pub(crate) const SQLITE: &str = "2\n\n";
/// Clio-managed Postgres: the choice, and nothing else.
///
/// One question, because there is one answer. A managed database is a container
/// Clio starts itself, so the host, the port, the user, the name, and the SSL
/// mode are all Clio's to pick — the port has to be one that is actually free.
/// There is no password to pick, because the managed database has none: it is
/// published on loopback and uses Postgres trust authentication. Asking the
/// operator for any of them would be asking a question whose answer the wizard
/// is going to override.
///
/// Every one of them can still be set by flag, and a flag is checked, so an
/// operator who wants a particular value is not refused it.
pub(crate) const MANAGED: &str = "1\n";
/// An existing Postgres, pasted: the choice and the hand-over.
///
/// The address is not here. It carries a password, so it is read through the
/// masked reader, which is the only way a secret is ever read; a script that tried
/// to type one would prove nothing. It is [`PASTED_ADDRESS`], and it is the first
/// value the scripted reader hands out on this route. The same rule is why
/// [`TYPED`] does not carry the password.
pub(crate) const PASTED: &str = "3\n1\n";

/// The address [`PASTED`] is completed by.
pub(crate) const PASTED_ADDRESS: &str =
    "postgres://clio:pw@db.example.com:5432/clio?sslmode=require";

/// The secrets a pasted route consumes, with whatever else the run needs after it.
///
/// The pasted address is always the first thing the masked reader hands out on
/// this route, which is what lets one helper serve every test: the address is
/// prepended, and the test's own secrets follow in the order it expects them.
pub(crate) fn pasted_secrets(rest: &[&str]) -> Vec<String> {
    std::iter::once(PASTED_ADDRESS.to_owned())
        .chain(rest.iter().map(|value| (*value).to_owned()))
        .collect()
}
/// An existing Postgres, typed: the choice, the hand-over, the host, the port,
/// the username, the name, and the SSL mode.
///
/// The password is not here: it comes from the scripted masked reader, which is
/// the only way a secret is ever read, so a script that tried to type one would
/// prove nothing.
///
/// There is no line for the advanced block, because the block is no longer asked.
/// Those thirteen settings are reachable with `--database-option`, which is also
/// how the tests that want one set it.
pub(crate) const TYPED: &str = "3\n2\ndb.example.com\n5432\nclio\n\n\n";
/// The three service questions with every default taken: the embedding role, the
/// rerank role, and extraction.
///
/// The defaults are the local sidecar, the local sidecar, and skip, so a blank
/// line at each runs the whole wizard without a credential typed anywhere.
pub(crate) const SERVICES: &str = "\n\n\n";
/// The three service questions with every service local, which is the only run
/// that starts the two model containers.
///
/// The local sidecar is option 1 at all three questions, so the answer is the same
/// number three times. The menus differ in what else they offer — reranking has a
/// `Skip` and extraction has a not-yet-built `Hosted` — but neither changes where
/// the local option sits.
pub(crate) const ALL_LOCAL: &str = "1\n1\n1\n";
/// The three service questions with reranking declined and extraction local.
///
/// Reranking's `Skip` is its last option, so it is 3 there. Embedding takes its
/// default, which is the local sidecar.
pub(crate) const NO_RERANK: &str = "\n3\n1\n";

/// An environment that names nothing.
///
/// `CLIO_DATABASE_URL` exported in the developer's shell is a real way to point Clio
/// at a database, and the wizard honours it. That is correct behaviour and it is
/// the reason a test cannot leave the decision to the ambient environment: a test
/// that inherits whatever the machine exports asserts one thing here and another
/// on a laptop with a different shell, and its result stops being a fact about
/// the code. The one test that wants the real environment asks for it by name.
pub(crate) fn no_env(_key: &str) -> Option<String> {
    None
}

/// Every flag that answers the typed route outright.
///
/// A test that is about what comes *after* the detail fields — the connection
/// test, the blame step-back, the attempt cap — should not have to count the
/// questions the re-ask repeats. Answering the route by flag removes that
/// coupling entirely: the fields are set once and a re-ask asks nothing, so the
/// script holds only the lines the test actually cares about.
///
/// There is no variant of [`TYPED`] that opens the advanced block, because the
/// block is no longer asked at all — see `ask_the_fields` in `setup_flows_db_typed`.
/// The same route, spelled as flags rather than as typed lines.
///
/// There is no variant of [`TYPED`] that opens the advanced block, because the
/// block is no longer asked at all — see `ask_the_fields` in
/// `setup_flows_db_typed`. A test that wants those settings names them with
/// `--database-option`, which is checked against the same table the block used.
pub(crate) const TYPED_BY_FLAG: [(&str, &str); 8] = [
    ("database", "existing"),
    ("database-connect", "typed"),
    ("database-host", "db.example.com"),
    ("database-port", "5432"),
    ("database-name", "clio"),
    ("database-user", "clio"),
    ("database-password", "pw"),
    ("database-ssl", "prefer"),
];

#[cfg(test)]
#[path = "setup_preview_scripts_tests.rs"]
mod tests;
