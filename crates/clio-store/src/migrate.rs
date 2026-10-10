// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Schema bootstrap from the portable SQL files under `sql/`.

use clio_types::{AmError, ErrorCode, civil_from_days};

/// Schema version this build expects (kept in lockstep with `SQL_CORE`).
pub const SCHEMA_VERSION: &str = "1";
/// Core portable DDL.
pub const SQL_CORE: &str = include_str!("../../../sql/001_core.sql");
/// Postgres + pgvector dialect.
pub const SQL_VECTORS_POSTGRES: &str = include_str!("../../../sql/002_vectors_postgres.sql");
/// SQLite + sqlite-vec dialect.
pub const SQL_VECTORS_SQLITE: &str = include_str!("../../../sql/002_vectors_sqlite.sql");

/// Map a driver error into `AmError`.
///
/// The Postgres driver's own `Display` for a server-side refusal is the bare
/// string `db error`. The reason an operator can act on — the constraint name,
/// the `DETAIL`, the `HINT` — sits behind that in the error's source chain, so
/// using `Display` alone left every statement-level failure reporting as
/// `items insert: db error` and nothing else (#63). This function is the single
/// mapping every statement goes through, so the walk belongs here; the connect
/// and probe paths already did it locally, which is why the same failure said
/// three different things depending on how far it got.
///
/// Errors from the SQLite backend keep their existing rendering: `rusqlite`
/// puts its message in `Display` already, so there is nothing behind it to find
/// and no test would notice a change.
#[must_use]
pub fn map_db_err<E>(context: &str, err: E) -> AmError
where
    E: std::fmt::Display + 'static,
{
    let reason = (&err as &dyn std::any::Any)
        .downcast_ref::<postgres::Error>()
        .map_or_else(|| err.to_string(), crate::pg_tls::describe);
    AmError::new(ErrorCode::Internal, format!("{context}: {reason}"))
}

/// Convert driver `Result`s into `AmError` (single coverage sink for all call sites).
pub(crate) trait IntoAmError<T> {
    fn am(self, context: &'static str) -> Result<T, AmError>;
}

impl<T, E: std::fmt::Display + 'static> IntoAmError<T> for Result<T, E> {
    fn am(self, context: &'static str) -> Result<T, AmError> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => Err(map_db_err(context, e)),
        }
    }
}

/// Fail closed when the SQLite vector extension is unavailable.
#[must_use]
pub fn sqlite_vec_missing(detail: impl std::fmt::Display) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "sqlite-vec extension is required but failed to load ({detail}); \
             install/link sqlite-vec — silent float-blob mode is not allowed"
        ),
    )
}

/// ISO-8601-ish UTC timestamp suitable for portable text columns.
#[must_use]
pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    // ponytail: no chrono dependency; millisecond epoch stamp as sortable UTC text.
    format!("{millis}")
}

/// UTC `YYYY-MM-DDThh:mm:ssZ` for the current instant.
///
/// Distinct from [`now_iso`] (epoch millis): this form sorts correctly against
/// ISO date strings such as `2024-06-01`, which is what defaults an omitted
/// triple `as_of` to “now”.
#[must_use]
pub fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
#[path = "migrate_reason_tests.rs"]
mod reason_tests;

#[cfg(test)]
mod tests {
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
            SQL_CORE
                .contains("DROP INDEX IF EXISTS triples_bank_id_subject_predicate_open_both_inx")
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
}
