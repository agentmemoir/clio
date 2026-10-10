// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres-backed tests for catalog
//! discovery: the app-table set a real store's schema reports, the
//! extension-dependent and extension-owned classification, discovery's
//! stability and scoping, generated-column filtering, and the owned-sequence
//! finding a snapshot refuses on.
//!
//! ## Owns
//! - The discovery acceptance scenarios, run against a real Postgres schema
//! - The shared helpers of these scenarios only
//!
//! ## Does not own
//! - The dump and restore behavior (`postgres_dump_tests`,
//!   `postgres_restore_tests`)
//! - The discovery implementation itself (`postgres_discovery`)
//!
//! ## Boundary
//!
//! Changes to what discovery reports should update the scenarios here.
//! Changes to how the tests isolate their Postgres state should not.
//!
//! Keep this module focused on discovery tests.

use super::*;
use crate::embedding_space::embedding_space_pg_space_tests::base_url;
use crate::pg_lock::lock_pg_test;
use crate::pg_scoped_schema::ScopedSchema;

/// The application-table names the store's core DDL creates, sorted.
///
/// Parsed from the DDL text itself, so the expectation moves with the schema
/// instead of rotting into a second hardcoded list.
fn core_table_names() -> Vec<String> {
    let mut names: Vec<String> = crate::migrate::SQL_CORE
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("CREATE TABLE IF NOT EXISTS ")
                .map(|rest| {
                    rest.split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                })
        })
        .collect();
    names.sort();
    names
}

/// The discovered schema of the store behind `url`, read in one transaction
/// the way a dump does.
fn discover_url(url: &str) -> DiscoveredSchema {
    let mut client = crate::pg_tls::connect(url).expect("discovery connect");
    let mut tx = client.transaction().expect("discovery transaction");
    let schema = effective_schema(&mut tx).expect("effective schema");
    let discovered = discover_schema(&mut tx, &schema).expect("discover the schema");
    tx.commit().expect("commit the discovery read");
    discovered
}

fn app_table_names(discovered: &DiscoveredSchema) -> Vec<String> {
    discovered
        .app_tables
        .iter()
        .map(|t| t.name.clone())
        .collect()
}

#[test]
fn a_real_store_schema_discovers_exactly_the_core_ddl_tables() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());
    let _store = scoped.open();

    let discovered = discover_url(scoped.url());
    assert_eq!(app_table_names(&discovered), core_table_names());
    // The vector table is created later, by Rust DDL, and carries a vector
    // column: it must never ride in app_tables.
    assert!(!app_table_names(&discovered).contains(&"item_embeddings".to_owned()));
    assert_eq!(
        discovered.extension_dependent,
        vec!["item_embeddings".to_owned()]
    );
    assert!(
        discovered.extension_owned.is_empty(),
        "{:?}",
        discovered.extension_owned
    );
}

#[test]
fn an_extension_dependent_table_is_excluded_from_app_tables() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());
    let mut client = crate::pg_tls::connect(scoped.url()).expect("connect");
    client
        .batch_execute("CREATE TABLE vec_t (x vector(3), label text);")
        .expect("create the vector table");
    drop(client);

    let discovered = discover_url(scoped.url());
    assert!(!app_table_names(&discovered).contains(&"vec_t".to_owned()));
    assert_eq!(discovered.extension_dependent, vec!["vec_t".to_owned()]);
    assert!(
        discovered.extension_owned.is_empty(),
        "{:?}",
        discovered.extension_owned
    );
}

#[test]
fn an_extension_owned_table_is_excluded_from_every_copy_class() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());
    let mut client = crate::pg_tls::connect(scoped.url()).expect("connect");
    client
        .batch_execute(
            "CREATE TABLE owned_t (x vector(3));
             ALTER EXTENSION vector ADD TABLE owned_t;",
        )
        .expect("create and hand the table to the extension");

    let discovered = discover_url(scoped.url());
    assert!(!app_table_names(&discovered).contains(&"owned_t".to_owned()));
    assert!(
        !discovered
            .extension_dependent
            .contains(&"owned_t".to_owned())
    );
    assert_eq!(discovered.extension_owned, vec!["owned_t".to_owned()]);

    // Leave the shared extension's membership as it was before the schema
    // drops out from under the table.
    client
        .batch_execute("ALTER EXTENSION vector DROP TABLE owned_t; DROP TABLE owned_t;")
        .expect("release the table from the extension");
    drop(client);
}

#[test]
fn two_discoveries_of_one_schema_are_identical_and_sorted() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());
    let _store = scoped.open();

    let first = discover_url(scoped.url());
    let second = discover_url(scoped.url());
    assert_eq!(first, second);
    let names = app_table_names(&first);
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "app tables arrive sorted by name");
}

#[test]
fn discovery_stays_inside_the_scoped_schema() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());
    let public_name = format!(
        "am_disc_public_t_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let mut client = crate::pg_tls::connect(base_url().as_str()).expect("base connect");
    client
        .batch_execute(&format!("CREATE TABLE public.{public_name} (id int);"))
        .expect("create the public decoy table");
    drop(client);
    let mut client = crate::pg_tls::connect(scoped.url()).expect("scoped connect");
    client
        .batch_execute("CREATE TABLE scoped_t (id int);")
        .expect("create the scoped table");
    drop(client);

    let discovered = discover_url(scoped.url());
    assert_eq!(
        discovered.schema,
        effective_schema_of(scoped.url()),
        "discovery reads the search path's first schema"
    );
    assert_ne!(discovered.schema, "public");
    assert!(app_table_names(&discovered).contains(&"scoped_t".to_owned()));
    assert!(
        !app_table_names(&discovered).contains(&public_name),
        "`public`'s tables must not be swept into the scoped discovery"
    );

    let mut client = crate::pg_tls::connect(base_url().as_str()).expect("base connect");
    client
        .batch_execute(&format!("DROP TABLE public.{public_name};"))
        .expect("drop the public decoy table");
    drop(client);
}

#[test]
fn generated_columns_are_left_out_of_the_column_list() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());
    let mut client = crate::pg_tls::connect(scoped.url()).expect("connect");
    client
        .batch_execute("CREATE TABLE gen_t (id int, doubled int GENERATED ALWAYS AS (id) STORED);")
        .expect("create the generated-column table");
    drop(client);

    let discovered = discover_url(scoped.url());
    let table = discovered
        .app_tables
        .iter()
        .find(|t| t.name == "gen_t")
        .expect("the generated-column table is an app table");
    assert_eq!(table.column_names(), vec!["id".to_owned()]);
}

#[test]
fn a_serial_primary_key_reports_its_owned_sequence() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());
    let mut client = crate::pg_tls::connect(scoped.url()).expect("connect");
    client
        .batch_execute("CREATE TABLE seq_t (id serial PRIMARY KEY, note text);")
        .expect("create the serial-key table");
    drop(client);

    let discovered = discover_url(scoped.url());
    assert_eq!(
        discovered.sequences,
        vec![OwnedSequence {
            sequence: "seq_t_id_seq".to_owned(),
            table: "seq_t".to_owned(),
            column: "id".to_owned(),
        }]
    );
    assert!(app_table_names(&discovered).contains(&"seq_t".to_owned()));
}

#[test]
fn an_empty_search_path_resolves_to_no_effective_schema() {
    let _guard = lock_pg_test();
    let mut client = crate::pg_tls::connect(base_url().as_str()).expect("connect");
    let mut tx = client.transaction().expect("transaction");
    tx.batch_execute("SET LOCAL search_path = '';")
        .expect("clear the search path");
    let error = effective_schema(&mut tx).expect_err("an empty search path resolves to nothing");
    assert!(
        error.message.contains("search_path"),
        "error: {}",
        error.message
    );
}

/// The schema the connection's search path resolves to, read directly.
fn effective_schema_of(url: &str) -> String {
    let mut client = crate::pg_tls::connect(url).expect("connect");
    let mut tx = client.transaction().expect("transaction");
    effective_schema(&mut tx).expect("effective schema")
}
