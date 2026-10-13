// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the database plan: that a choice produces the address it
//! should, that the two config keys are always written together, and that the
//! managed address carries no password while a typed one is still redacted.
//!
//! ## Owns
//! - The paired `entries` vector, asserted exactly
//! - Which services each choice starts
//! - The passwordless managed address, percent-encoding of a typed value, and
//!   the refusal of an unusable address
//!
//! ## Does not own
//! - The plan itself (see `setup_plan_db`)
//! - Writing the config (see `setup_config`)
//!
//! ## Boundary
//!
//! Tests here break when a choice stops producing the entries it should; they
//! never change the implementation.
//!
//! Keep this module focused on the plan's observable output.

use super::*;
use clio_store::SslChoice;

fn managed_plan() -> DatabasePlan {
    managed(
        "127.0.0.1",
        "34310",
        "clio",
        "clio",
        SslChoice::Prefer,
        None,
    )
    .expect("a managed plan assembles")
}

fn existing_plan() -> DatabasePlan {
    existing(
        "postgres://u:p@db.example.com:5432/clio".to_owned(),
        SslChoice::VerifyFull,
        Vec::new(),
        None,
    )
    .expect("an existing plan records the address")
}

#[test]
fn a_managed_plan_names_both_keys() {
    let plan = managed_plan();
    assert_eq!(
        plan.entries(),
        vec![
            ("backend".to_owned(), json!("postgres")),
            (
                "store.database_url".to_owned(),
                json!("postgres://clio@127.0.0.1:34310/clio?sslmode=prefer")
            ),
        ]
    );
}

#[test]
fn an_existing_plan_names_both_keys() {
    let plan = existing_plan();
    assert_eq!(
        plan.entries(),
        vec![
            ("backend".to_owned(), json!("postgres")),
            (
                "store.database_url".to_owned(),
                json!("postgres://u:p@db.example.com:5432/clio")
            ),
        ]
    );
}

#[test]
fn a_sqlite_plan_also_names_the_file() {
    let plan = sqlite(PathBuf::from("/data/clio.db"), None);
    assert_eq!(
        plan.entries(),
        vec![
            ("backend".to_owned(), json!("sqlite")),
            ("store.database_url".to_owned(), json!("/data/clio.db")),
            ("store.sqlite_path".to_owned(), json!("/data/clio.db")),
        ]
    );
}

/// The silent-SQLite regression at the plan level, both directions. If
/// `entries` ever drops either key, this fails on the missing one and not on
/// the key it happened to keep.
#[test]
fn the_backend_and_the_address_are_always_written_together() {
    for plan in [
        managed_plan(),
        existing_plan(),
        sqlite(PathBuf::from("/d/clio.db"), None),
    ] {
        let entries = plan.entries();
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        assert!(
            names.contains(&"backend"),
            "{names:?} is missing the backend"
        );
        assert!(
            names.contains(&"store.database_url"),
            "{names:?} is missing the address"
        );
    }
}

#[test]
fn switching_to_a_file_still_writes_the_backend() {
    // The failure this pins: a plan that wrote only the file path would leave
    // `backend: postgres` in the config and open a Postgres server for a store
    // that is a file.
    let plan = sqlite(
        PathBuf::from("/data/clio.db"),
        Some("/old/clio.db".to_owned()),
    );
    assert_eq!(plan.backend_name(), "sqlite");
    assert!(plan.entries().iter().any(|(name, _)| name == "backend"));
}

#[test]
fn a_managed_plan_keeps_the_store_it_replaces() {
    let plan = managed(
        "127.0.0.1",
        "5432",
        "clio",
        "clio",
        SslChoice::Prefer,
        Some("/old/clio.db".to_owned()),
    )
    .expect("assembles");
    assert_eq!(plan.previous_store.as_deref(), Some("/old/clio.db"));
}

#[test]
fn only_the_managed_choice_starts_a_database() {
    assert!(managed_plan().starts_database());
    assert!(!existing_plan().starts_database());
    assert!(!sqlite(PathBuf::from("/d/clio.db"), None).starts_database());
}

#[test]
fn only_the_managed_choice_names_the_postgres_service() {
    assert_eq!(managed_plan().database_services(), &["postgres"]);
    let existing_services = existing_plan().database_services();
    assert!(existing_services.is_empty(), "{existing_services:?}");
    let sqlite_services = sqlite(PathBuf::from("/d/clio.db"), None).database_services();
    assert!(sqlite_services.is_empty(), "{sqlite_services:?}");
}

#[test]
fn a_managed_address_carries_no_password() {
    let address = &managed_plan().address;
    assert!(!address.contains("generated-value-1"), "{address}");
    assert!(address.starts_with("postgres://clio@"), "{address}");
    let (_, after_scheme) = address
        .split_once("://")
        .expect("the address names a scheme");
    let (userinfo, _) = after_scheme
        .split_once('@')
        .expect("the address names a host");
    assert!(
        !userinfo.contains(':'),
        "the userinfo holds a password: {address}"
    );
}

#[test]
fn a_description_of_a_passwordless_address_is_not_redacted() {
    // The managed address has no password, so a redaction mark on it would
    // claim a secret where there is none.
    let described = managed_plan().describe();
    assert!(!described.contains("***"), "{described}");
    assert!(described.contains("Postgres"), "{described}");
}

#[test]
fn a_description_redacts_a_typed_password() {
    let plan = existing(
        "postgres://u:s3cret@h:5432/clio".to_owned(),
        SslChoice::VerifyFull,
        vec![],
        None,
    )
    .expect("records the address");
    assert!(!plan.describe().contains("s3cret"), "{}", plan.describe());
}

#[test]
fn a_file_description_names_the_file() {
    let plan = sqlite(PathBuf::from("/data/clio.db"), None);
    assert_eq!(plan.describe(), "SQLite file at /data/clio.db");
}

#[test]
fn a_managed_address_names_the_host_port_and_database() {
    let address = managed_address("127.0.0.1", "34310", "clio", "clio", SslChoice::VerifyFull)
        .expect("assembles");
    assert_eq!(
        address,
        "postgres://clio@127.0.0.1:34310/clio?sslmode=verify-full"
    );
}

#[test]
fn a_managed_address_that_the_driver_refuses_is_not_returned() {
    let error = managed_address("", "", "", "clio", SslChoice::Prefer)
        .expect_err("an empty host makes no address");
    assert!(error.contains("no host"), "{error}");
}

#[test]
fn a_typed_password_is_percent_encoded() {
    let address = typed_address(
        "h",
        "5432",
        "clio",
        "clio",
        "a b&c=d",
        SslChoice::Prefer,
        None,
        &[],
    )
    .expect("assembles");
    assert!(
        address.starts_with("postgres://clio:a%20b%26c%3Dd@h:5432/clio?"),
        "{address}"
    );
}

#[test]
fn a_typed_advanced_value_is_percent_encoded() {
    let password = format!("{:016x}", rand::random::<u64>());
    let address = typed_address(
        "h",
        "5432",
        "clio",
        "clio",
        &password,
        SslChoice::Prefer,
        None,
        &[Extra {
            name: "options",
            value: "a b&c=d".to_owned(),
        }],
    )
    .expect("assembles");
    assert!(address.contains("&options=a%20b%26c%3Dd"), "{address}");
}

#[test]
fn an_empty_advanced_value_adds_nothing() {
    let password = format!("{:016x}", rand::random::<u64>());
    let address = typed_address(
        "h",
        "5432",
        "clio",
        "clio",
        &password,
        SslChoice::Prefer,
        None,
        &[Extra {
            name: "options",
            value: "  ".to_owned(),
        }],
    )
    .expect("assembles");
    assert!(!address.contains("options"), "{address}");
}

#[test]
fn the_ssl_mode_is_the_first_query_parameter() {
    let password = format!("{:016x}", rand::random::<u64>());
    let address = typed_address(
        "h",
        "5432",
        "clio",
        "clio",
        &password,
        SslChoice::Disable,
        None,
        &[],
    )
    .expect("assembles");
    assert!(address.contains("?sslmode=disable"), "{address}");
}

#[test]
fn a_typed_root_certificate_is_percent_encoded_into_the_address() {
    let password = format!("{:016x}", rand::random::<u64>());
    let address = typed_address(
        "h",
        "5432",
        "clio",
        "clio",
        &password,
        SslChoice::VerifyFull,
        Some("/etc/my ca.pem"),
        &[],
    )
    .expect("assembles");
    assert!(
        address.contains("&sslrootcert=%2Fetc%2Fmy%20ca.pem"),
        "{address}"
    );
}

#[test]
fn an_empty_root_certificate_adds_nothing() {
    let password = format!("{:016x}", rand::random::<u64>());
    let address = typed_address(
        "h",
        "5432",
        "clio",
        "clio",
        &password,
        SslChoice::VerifyFull,
        Some("  "),
        &[],
    )
    .expect("assembles");
    assert!(!address.contains("sslrootcert"), "{address}");
}

#[test]
fn a_root_certificate_is_not_added_for_a_mode_that_does_not_check_it() {
    // A root in an address whose mode never checks the certificate would be a
    // value that reaches the store and is never used. It is left out.
    for mode in [SslChoice::Disable, SslChoice::EncryptNoVerify] {
        let password = format!("{:016x}", rand::random::<u64>());
        let address = typed_address(
            "h",
            "5432",
            "clio",
            "clio",
            &password,
            mode,
            Some("/etc/ca.pem"),
            &[],
        )
        .expect("assembles");
        assert!(
            !address.contains("sslrootcert"),
            "{}: {address}",
            mode.as_str()
        );
    }
}

#[test]
fn the_managed_flag_spellings_are_accepted() {
    assert_eq!(
        DatabaseChoice::parse("managed"),
        Some(DatabaseChoice::Managed)
    );
    assert_eq!(
        DatabaseChoice::parse("docker"),
        Some(DatabaseChoice::Managed)
    );
}

#[test]
fn the_sqlite_flag_spellings_are_accepted() {
    assert_eq!(
        DatabaseChoice::parse("sqlite"),
        Some(DatabaseChoice::Sqlite)
    );
    assert_eq!(DatabaseChoice::parse("file"), Some(DatabaseChoice::Sqlite));
}

#[test]
fn the_existing_flag_spellings_are_accepted() {
    assert_eq!(
        DatabaseChoice::parse("existing"),
        Some(DatabaseChoice::Existing)
    );
    assert_eq!(
        DatabaseChoice::parse("external"),
        Some(DatabaseChoice::Existing)
    );
}

#[test]
fn an_unknown_choice_is_refused() {
    assert_eq!(DatabaseChoice::parse("mysql"), None);
    assert_eq!(DatabaseChoice::parse(""), None);
}

#[test]
fn the_paste_flag_spellings_are_accepted() {
    assert_eq!(HandOver::parse("pasted"), Some(HandOver::Pasted));
    assert_eq!(HandOver::parse("paste"), Some(HandOver::Pasted));
}

#[test]
fn the_type_flag_spellings_are_accepted() {
    assert_eq!(HandOver::parse("typed"), Some(HandOver::Typed));
    assert_eq!(HandOver::parse("details"), Some(HandOver::Typed));
}

#[test]
fn an_unknown_hand_over_is_refused() {
    assert_eq!(HandOver::parse("import"), None);
}

#[test]
fn each_choice_names_itself_for_the_question() {
    assert_eq!(
        DatabaseChoice::Managed.label(),
        "Clio Manages Postgres in Docker"
    );
    assert_eq!(
        DatabaseChoice::Sqlite.label(),
        "A SQLite file on this machine"
    );
    assert_eq!(
        DatabaseChoice::Existing.label(),
        "A Postgres you already have"
    );
}

#[test]
fn each_hand_over_names_itself_for_the_question() {
    assert_eq!(HandOver::Pasted.label(), "Paste a connection string");
    assert_eq!(HandOver::Typed.label(), "Type the details");
}

#[test]
fn an_existing_plan_refuses_an_address_it_cannot_open() {
    let error = existing("mysql://h/db".to_owned(), SslChoice::Prefer, vec![], None)
        .expect_err("a mysql server cannot be opened");
    assert!(error.contains("mysql://"), "{error}");
}
