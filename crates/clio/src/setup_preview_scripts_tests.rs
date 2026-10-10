// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the answer scripts the wizard test
//! suites drive: that each route script asks exactly the answers it claims to,
//! that the environment helper names nothing, and that the flag list covers the
//! typed route.
//!
//! ## Owns
//! - Tests for the route scripts, the secrets helper and the flag list
//! - The pinned shape of each script's line count and content
//!
//! ## Does not own
//! - Driving a setup run (`setup_preview_fixtures`)
//! - What the wizard prints in response to a script
//!
//! ## Boundary
//!
//! Tests here break when a script gains or loses a line, or when the typed route
//! changes which fields a flag can answer. They never change the wizard.
//!
//! Keep this module focused on the inputs a run is scripted with.

/// The environment helper must name nothing at all, so a suite never inherits
/// whatever the machine exported and asserts a fact about the machine.
///
/// The prefixed database variable is named first because it is the one an
/// operator most likely to have exported.
#[test]
fn the_naming_nothing_environment_names_nothing() {
    for key in [
        "CLIO_DATABASE_URL",
        "CLIO_DATA_DIR",
        "CLIO_KEY_FILE",
        "DATABASE_URL",
        "",
    ] {
        assert_eq!(super::no_env(key), None, "for key `{key}`");
    }
}

/// A pasted route reads its address from the masked reader before anything the
/// test asked for, because one helper serves every pasted-route test on that
/// assumption. The rest must keep the order the test passed.
#[test]
fn pasted_secrets_leads_with_the_address_then_keeps_the_rest_in_order() {
    let secrets = super::pasted_secrets(&["first", "second"]);
    assert_eq!(secrets.len(), 3, "one address plus two secrets");
    let address = super::PASTED_ADDRESS;
    assert_eq!(secrets.first().map(String::as_str), Some(address));
    assert_eq!(&secrets[1..], ["first".to_owned(), "second".to_owned()]);
}

/// The pasted route asks only the choice and the hand-over, so a script carrying
/// a third line would type it into a question the wizard does not ask.
#[test]
fn the_pasted_route_script_asks_two_questions() {
    let script = super::PASTED;
    assert_eq!(script.lines().count(), 2, "{script}");
    assert_eq!(super::pasted_secrets(&[]).len(), 1, "the address alone");
}

/// The typed route asks every field except the password, which comes from the
/// masked reader. Each answer is one line, so the script's length is the number
/// of questions.
#[test]
fn the_typed_route_script_asks_one_line_per_field() {
    let script = super::TYPED;
    assert_eq!(script.lines().count(), 7, "{script}");
    assert!(
        !script.contains("pw"),
        "the password is read through the masked reader, never typed: {script}"
    );
}

/// The managed route asks one question, and the SQLite route asks two with the
/// second blank because the file has a default.
#[test]
fn the_short_routes_ask_only_what_they_claim() {
    let managed = super::MANAGED;
    let sqlite = super::SQLITE;
    assert_eq!(managed.lines().count(), 1, "{managed}");
    assert_eq!(sqlite.lines().count(), 2, "{sqlite}");
    assert!(sqlite.ends_with('\n'), "{sqlite}");
}

/// The service scripts are one line per service question, and the defaults are a
/// blank line rather than an omitted one.
#[test]
fn the_service_scripts_ask_three_questions_each() {
    let services = super::SERVICES;
    let all_local = super::ALL_LOCAL;
    let no_rerank = super::NO_RERANK;
    assert_eq!(services.lines().count(), 3, "{services}");
    assert_eq!(all_local.lines().count(), 3, "{all_local}");
    assert_eq!(no_rerank.lines().count(), 3, "{no_rerank}");
    assert!(no_rerank.starts_with('\n'), "{no_rerank}");
}

/// The two config-path constants must stay distinct, because a suite reads which
/// one it got from the wording the wizard prints for each. Neither may exist on
/// disk, or `will_back_up` reads as an install that is already there.
#[test]
fn the_two_config_paths_are_distinct_and_absent_from_the_filesystem() {
    assert_ne!(super::ABSENT_CONFIG, super::EXISTING_CONFIG);
    for path in [super::ABSENT_CONFIG, super::EXISTING_CONFIG] {
        assert!(path.starts_with('/'), "{path} is absolute");
        assert!(
            !std::path::Path::new(path).exists(),
            "{path} must not exist, or will_back_up reads as an install"
        );
    }
}

/// The flag list must answer every field the typed route asks, and must still
/// reach the typed route itself.
#[test]
fn the_flag_list_covers_every_typed_route_field() {
    let flags = super::TYPED_BY_FLAG;
    let names: Vec<&str> = flags.map(|(name, _)| name).to_vec();
    for expected in [
        "database",
        "database-connect",
        "database-host",
        "database-port",
        "database-name",
        "database-user",
        "database-password",
        "database-ssl",
    ] {
        assert!(names.contains(&expected), "missing flag `{expected}`");
    }
    let values: Vec<&str> = flags.map(|(_, value)| value).to_vec();
    assert!(
        values.contains(&"typed"),
        "the typed route is reachable by flag alone"
    );
}
