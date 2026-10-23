// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests schema assets and error adapters.
//!
//! ## Owns
//! - SQL asset and error adapter assertions
//!
//! ## Does not own
//! - Production behavior
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use clio_types::SemanticCategory;

#[test]
fn core_sql_category_check_matches_the_whitelist() {
    // The `items` CHECK constraint lists the categories; keep it tied
    // to the single-source whitelist so a new category cannot be missed.
    let expected = SemanticCategory::WIRE
        .iter()
        .map(|category| format!("'{category}'"))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        SQL_CORE.contains(&expected),
        "sql/001_core.sql (items category CHECK) is missing the whitelist list: {expected}"
    );
}

#[test]
fn sql_assets_and_error_mappers() {
    assert!(SQL_CORE.contains("CREATE TABLE IF NOT EXISTS items"));
    assert!(SQL_CORE.contains("epistemic_kind"));
    assert!(SQL_CORE.contains("schema_version"));
    assert!(SQL_CORE.contains("ON CONFLICT (key) DO UPDATE SET"));
    assert!(SQL_CORE.contains("assoc_edges"));
    assert!(SQL_CORE.contains("memtree_nodes"));
    assert!(SQL_CORE.contains("interval_start"));
    assert!(!SQL_CORE.contains("episodic_type IN\n      (\n       'triple', 'gist', 'task', 'failure', 'temporal', 'belief', 'other'"));
    assert!(SQL_CORE.contains("'triple', 'gist', 'task', 'failure', 'temporal'"));
    assert!(SQL_CORE.contains("'schema_version', '1'"));
    assert!(SQL_CORE.contains("proposition_ciphertext"));
    // The asserted triple VALUE is sealed under the subject DEK;
    // `subject` + `predicate` stay plain because makes them the
    // identity key. There must be no plaintext `object` column to fall back to.
    assert!(SQL_CORE.contains("object_ciphertext"));
    assert!(!SQL_CORE.contains("\n  object  "));
    assert!(
        SQL_VECTORS_POSTGRES
            .contains("ALTER TABLE triples ADD COLUMN IF NOT EXISTS object_ciphertext text")
    );
    assert!(SQL_CORE.contains("triples_bank_id_tx_from_tx_until_inx"));
    assert!(SQL_CORE.contains(
        "CREATE UNIQUE INDEX IF NOT EXISTS triples_open_uniq\n\
         ON triples (bank_id, subject, predicate)\n\
         WHERE valid_until IS NULL AND tx_until IS NULL"
    ));
    assert!(
        SQL_CORE.contains("DROP INDEX IF EXISTS triples_bank_id_subject_predicate_open_both_inx")
    );
    assert!(SQL_VECTORS_POSTGRES.contains("vector"));
    assert!(SQL_VECTORS_SQLITE.contains("fts5"));
    // The vector table and its index are width-parameterized in Rust now, so
    // neither static asset may pin a width or create the vector table.
    assert!(!SQL_VECTORS_POSTGRES.contains("vector(384)"));
    assert!(!SQL_VECTORS_POSTGRES.contains("CREATE TABLE IF NOT EXISTS item_embeddings"));
    assert!(!SQL_VECTORS_SQLITE.contains("USING vec0"));
    assert!(!SQL_VECTORS_SQLITE.contains("CREATE VIRTUAL TABLE IF NOT EXISTS item_embeddings"));
    let iso = now_iso();
    assert!(!iso.is_empty(), "{iso}");
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    let stamp = now_iso8601();
    assert!(stamp.starts_with("20"), "{stamp}");
    assert!(stamp.len() >= 20, "{stamp}");
    assert_eq!(map_db_err("ctx", "boom").code, ErrorCode::Internal);
    assert_eq!(sqlite_vec_missing("x").code, ErrorCode::ConfigCorrupt);
    assert_eq!(Result::<(), &str>::Ok(()).am("ok").unwrap(), ());
    assert_eq!(
        Result::<(), &str>::Err("nope").am("bad").unwrap_err().code,
        ErrorCode::Internal
    );
    assert!(crate::vec_register::map_register_outcome(&Ok(())).is_ok());
    assert_eq!(
        crate::vec_register::map_register_outcome(&Err("boom".into()))
            .unwrap_err()
            .code,
        ErrorCode::ConfigCorrupt
    );
    assert!(crate::vec_register::interpret_auto_extension_rc(0).is_ok());
    assert!(crate::vec_register::interpret_auto_extension_rc(1).is_err());
    assert_eq!(
        crate::db_url::postgres_url_or_default(Ok("postgres://x".into())),
        "postgres://x"
    );
    assert_eq!(
        crate::db_url::postgres_url_or_default(Err(std::env::VarError::NotPresent)),
        crate::db_url::DEFAULT_DATABASE_URL
    );
}
