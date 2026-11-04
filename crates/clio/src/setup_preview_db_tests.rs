// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the database block's rows: the parts a
//! parsed address expands into, and the fallback row shown when the driver's
//! parser refuses the address.
//!
//! ## Owns
//! - The block's rows under test for `setup_preview_db`
//!
//! ## Does not own
//! - The rest of the preview (see `setup_preview_tests`)
//!
//! ## Boundary
//!
//! Changes to the database block's rows should normally update tests here.
//! Changes to the other blocks should not.
//!
//! Keep this module focused on one address's expansion.

use super::*;
use serde_json::json;

/// The rows the block draws for one address.
fn rows(address: &str) -> Vec<Change> {
    let entries = vec![
        ("backend".to_owned(), json!("postgres")),
        ("store.database_url".to_owned(), json!(address)),
    ];
    block(&entries, watched(), &|_| None).expect("an address entry")
}

/// The one row named `name`.
fn row<'a>(rows: &'a [Change], name: &str) -> &'a Change {
    rows.iter()
        .find(|change| change.path == name)
        .unwrap_or_else(|| panic!("the {name} row"))
}

#[test]
fn a_refused_database_address_is_shown_without_its_password() {
    let refused = rows("postgres://user:pw@host:notaport/db");
    assert_eq!(
        row(&refused, "Database address").value,
        "postgres://***@host:notaport/db"
    );

    let file = rows("/var/lib/clio/memories.db");
    assert_eq!(
        row(&file, "Database address").value,
        "/var/lib/clio/memories.db"
    );
}

#[test]
fn a_parsed_database_address_is_shown_as_its_parts() {
    let parsed = rows("postgres://clio:pw@db.example.com:5432/clio?sslmode=verify-full");
    assert_eq!(row(&parsed, "Database host").value, "db.example.com");
    assert_eq!(row(&parsed, "Database port").value, "5432");
    assert_eq!(row(&parsed, "Database name").value, "clio");
    assert_eq!(row(&parsed, "Database username").value, "clio");
    assert_eq!(row(&parsed, "Database password").value, "(set)");
    assert_eq!(row(&parsed, "Database SSL mode").value, "verify-full");
}
