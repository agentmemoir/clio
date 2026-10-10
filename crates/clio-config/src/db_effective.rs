// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving the database target from the
//! effective config document, which is the only place a value written into the
//! deployment config can decide which store a command opens.
//!
//! ## Owns
//! - The precedence chain: `--db` > the document's `store.database_url` > the
//!   default SQLite data path
//! - The backend pairing rule that keeps `backend` and `store.database_url`
//!   from disagreeing
//! - Refusing a configured address that names a store this build cannot open,
//!   instead of quietly falling back to SQLite
//!
//! ## Does not own
//! - The environment-only chain used to seed the document (`db_path`)
//! - Opening the store (`clio-store` owns that)
//! - Config path/value validation (`config::validate`)
//!
//! ## Boundary
//!
//! Changes to the database precedence chain should normally require changes
//! here. Changes to overlay merge order or to store opening should not require
//! changes here.
//!
//! Keep this module focused on reading one database target out of a document.
//!
//! # Precedence, and why it is not the environment chain
//!
//! Every command used to read the target from the environment alone
//! (`--db` > `CLIO_DATABASE_URL` > data directory), so no value written into
//! the deployment config could ever change which store the binary opened.
//! This module reads the effective document instead.
//!
//! The order `Runtime::effective_raw` merges in, lowest first, is: system
//! defaults (which already carry the environment chain) < the config file
//! < the process environment < the profile bundle < the profile < the
//! deployment overlay < the session overlay. So a value in the deployment
//! config — the file `clio setup` writes — outranks a `CLIO_DATABASE_URL`
//! exported in the operator's shell, while a `CLIO_DATABASE_URL` outranks the
//! config file and the defaults. Reading the document therefore gives the
//! wizard the last word, which is the point, and keeps the environment chain
//! working for anyone who has never run setup.
//!
//! The address, in order:
//!
//! 1. `--db`, passed through untouched (an operator's own value, including
//!    `sqlite::memory:`).
//! 2. `store.database_url` from the effective document.
//! 3. The default SQLite path, which is derived from the environment and so
//!    cannot be read out of the document.
//!
//! The backend, in order:
//!
//! 1. `--backend`, because a flag is the operator speaking last.
//! 2. The scheme of the resolved address, when the address came from the
//!    document and that address names Postgres.
//! 3. `backend` from the effective document, read only alongside a document
//!    address.
//! 4. The scheme of the resolved address.
//!
//! # Why the address's scheme outranks the document's `backend`
//!
//! The document's two keys are seeded together from one source, so they agree
//! whenever nothing above the defaults has touched either. They come apart the
//! moment a higher layer sets one of them: a file that sets only
//! `store.database_url` leaves `backend` at the `sqlite` default.
//!
//! Reading `backend` first would make that file open SQLite against a
//! Postgres address, which is the exact silent-fallback bug this module
//! exists to remove. An address that names Postgres is the more specific
//! statement, so it decides.
//!
//! The two keys are one statement, so they are read as one: an address passed
//! as `--db` is paired with its own scheme and never with the document's
//! `backend`. Otherwise `clio --db ./notes.db` would open a SQLite file as
//! Postgres on any machine that happens to export a `CLIO_DATABASE_URL`.
//!
//! The mirror case is left loud rather than corrected: a document that names
//! `backend: postgres` with a filesystem address resolves to Postgres against a
//! path, and the store's own open then refuses it instead of quietly using a
//! local file.

use crate::db_path::{ResolvedDatabase, default_sqlite_path, infer_backend, sqlite_file_path};
use serde_json::Value;

/// The document key that carries the database address.
const URL_POINTER: &str = "/store/database_url";

/// The document key that carries the backend name.
const BACKEND_POINTER: &str = "/backend";

/// The backend names a store can be opened as.
const BACKENDS: &[&str] = &["sqlite", "postgres"];

/// Resolve the database target from the effective config document.
///
/// `get` is the same environment lookup the rest of the resolver chain uses.
/// It is needed for the last fallback only, because the default SQLite path is
/// derived from `CLIO_DATA_DIR` / `XDG_DATA_HOME` / `HOME` and no document
/// carries it.
///
/// # Errors
/// The document's `store.database_url` or `backend` names something no store
/// can open. The refusal carries the reason, so a hand-edited config fails
/// with an explanation instead of opening a local SQLite file.
pub fn resolve_effective_database<F>(
    get: &F,
    effective: &Value,
    explicit_db: Option<&str>,
    explicit_backend: Option<&str>,
) -> Result<ResolvedDatabase, String>
where
    F: Fn(&str) -> Option<String>,
{
    let configured = text_at(effective, URL_POINTER);
    // An explicit `--db` is the operator's own value and passes through
    // untouched, which is why the document's address is not consulted for it.
    // The flag also carries its own backend, so the document's `backend` is
    // only read when the address came from the document.
    let (url, from_document) = match usable(explicit_db) {
        Some(db) => (db.to_owned(), false),
        None => match configured {
            Some(url) => {
                check_address(url)?;
                (url.to_owned(), true)
            }
            None => (
                default_sqlite_path(get).to_string_lossy().into_owned(),
                false,
            ),
        },
    };
    let backend = backend_for(
        &url,
        explicit_backend,
        from_document
            .then(|| text_at(effective, BACKEND_POINTER))
            .flatten(),
    )?;
    // The backend is settled before the scheme is stripped, because inference
    // reads the scheme. A SQLite address keeps no scheme in the result: the
    // value handed to the opener, the key-store resolver, and the backup slug is
    // the file, so all three name one path.
    let url = if backend == "sqlite" {
        sqlite_file_path(&url).to_owned()
    } else {
        url
    };
    Ok(ResolvedDatabase { url, backend })
}

/// The backend for a resolved address, paired with whatever the document says.
///
/// `document_backend` is `None` whenever the address did not come from the
/// document, which is what keeps `--db ./notes.db` a SQLite file even while a
/// `CLIO_DATABASE_URL` in the environment is seeding the document's own
/// pairing.
fn backend_for(
    url: &str,
    explicit_backend: Option<&str>,
    document_backend: Option<&str>,
) -> Result<String, String> {
    if let Some(backend) = usable(explicit_backend) {
        return Ok(backend.to_owned());
    }
    if document_backend.is_none() {
        return Ok(infer_backend(url).to_owned());
    }
    if infer_backend(url) == "postgres" {
        return Ok("postgres".to_owned());
    }
    let backend = document_backend.unwrap_or_else(|| infer_backend(url));
    if BACKENDS.contains(&backend) {
        return Ok(backend.to_owned());
    }
    Err(format!(
        "{BACKEND_POINTER} must be one of sqlite|postgres, got `{backend}`. Fix the config file, \
         or pass `--backend` to choose one for this run"
    ))
}

/// Refuse a configured address that names a store this build cannot open.
///
/// A path, `sqlite::memory:`, and a `postgres://` / `postgresql://` URL are
/// all openable. Anything else carrying a `scheme://` prefix is refused: the
/// same scheme rule `clio-store::parse_connection` applies, so the wizard and
/// the resolver cannot disagree about what a database address is.
fn check_address(url: &str) -> Result<(), String> {
    let Some(scheme) = clio_types::net::url_scheme(url) else {
        return Ok(());
    };
    if matches!(scheme.as_str(), "postgres" | "postgresql") {
        return Ok(());
    }
    Err(format!(
        "store.database_url must be a postgres:// or postgresql:// address, or a path to a SQLite \
         file, got `{url}` ({scheme}://). This build keeps memories in PostgreSQL with pgvector or \
         in a SQLite file; a {scheme}:// server cannot be opened"
    ))
}

/// A document string at `pointer`, ignoring blanks and non-strings.
fn text_at<'a>(effective: &'a Value, pointer: &str) -> Option<&'a str> {
    let raw = effective.pointer(pointer)?.as_str()?.trim();
    (!raw.is_empty()).then_some(raw)
}

/// A trimmed, non-empty flag value. An empty flag is treated as absent, so the
/// next precedence tier applies rather than resolving an unusable target.
fn usable(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains('\0'))
}

#[cfg(test)]
#[path = "db_effective_tests.rs"]
mod tests;
