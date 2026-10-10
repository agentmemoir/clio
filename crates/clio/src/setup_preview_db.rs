// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the database section of the preview: the rows
//! that expand a connection address into the parts it was assembled from, and
//! the states those rows carry.
//!
//! ## Owns
//! - The address's expansion into host, port, name, username, password, SSL mode
//! - The state of one part row, which is the state of the address it came from
//! - The password's display form, which only the plan can decide
//! - The encryption-and-locality label for the address
//! - The watched rows the database scope drops
//!
//! ## Does not own
//! - Which blocks exist, or their order (see `setup_preview`)
//! - The embedding and reranking rows (see `setup_preview`)
//! - The change model and the layout (see `setup_report`)
//!
//! ## Boundary
//!
//! Changes to how the database address is shown should normally require changes
//! here. Changes to which sections the preview has, or to how it is drawn,
//! should not require changes here.
//!
//! Keep this module focused on reading one address apart.
//!
//! # Why the address is shown as its parts
//!
//! The applied summary counts config entries, and the preview has to show every
//! entry it counts so the two can never disagree silently. `backend` and
//! `store.database_url` are two entries, but one address read as a single line
//! tells the operator nothing they can check — a wrong port inside a long string
//! is invisible. So the address is expanded through the store's own parser into
//! host, port, database name, username, password, and SSL mode. The parts come
//! from the string that is written, read through the same parser the driver will
//! use, so the expansion cannot disagree with it.
//!
//! # Why the password row is conditional
//!
//! The row appears only when the address about to be written actually carries a
//! password. Clio's own Postgres has none, and a row claiming a password that
//! does not exist would send an operator looking for one. A hosted address does
//! carry one, and the row is what confirms the value arrived — shown as `(set)`,
//! never as characters, because a preview gets pasted into issues.

use crate::setup_report::{self as report, Change, ChangeState};
use serde_json::Value;

/// The database scope: the paths whose rows this module builds.
const SCOPE: &[&str] = &["backend", "store.database_url", "store.sqlite_path"];

/// Whether a path belongs to the database block.
pub(crate) fn is_database(path: &str) -> bool {
    path.starts_with("backend") || path.starts_with("store.")
}

/// The database block, with the address expanded into the parts it was assembled
/// from.
///
/// `None` when the run is not writing an address, which is the SQLite case: there
/// the path is the answer and the generic rows already say so.
pub(crate) fn block(
    entries: &[(String, Value)],
    scope: &[&'static str],
    current: &dyn Fn(&str) -> Option<Value>,
) -> Option<Vec<Change>> {
    let address = entries
        .iter()
        .find(|(path, _)| path == "store.database_url")
        .map(|(_, value)| value.as_str().unwrap_or_default().to_owned())?;
    let state = state_of("store.database_url", entries, current);
    let mut changes = vec![Change {
        path: report::display_name("backend"),
        value: backend_of(entries),
        state: state_of("backend", entries, current),
        note: None,
    }];
    // The address itself is a secret path, so the generic display would render it
    // as a length. The parts below say the same thing and say it usefully, so the
    // address row is replaced rather than repeated.
    match clio_store::parse_connection(&address) {
        Ok(parsed) => {
            changes.push(part("Database host", parsed.primary_host(), state, None));
            changes.push(part(
                "Database port",
                &parsed.primary_port().to_string(),
                state,
                None,
            ));
            changes.push(part(
                "Database name",
                parsed.database.as_deref().unwrap_or_default(),
                state,
                None,
            ));
            changes.push(part(
                "Database username",
                parsed.user.as_deref().unwrap_or_default(),
                state,
                None,
            ));
            // Only when the address actually carries one. Clio's own Postgres has
            // none, and a row claiming a password that does not exist would send
            // an operator looking for one. A hosted address does carry one, and
            // saying so is the confirmation that the value arrived.
            if has_password(&address) {
                changes.push(part(
                    "Database password",
                    &crate::setup_preview::secret_set(),
                    state,
                    Some("not shown".to_owned()),
                ));
            }
            changes.push(part(
                "Database SSL mode",
                parsed.ssl.as_str(),
                state,
                Some(locality_of(&parsed, parsed.ssl.as_str())),
            ));
        }
        // A SQLite install puts the file in the same field a Postgres address
        // occupies, because the store reads one address for both backends. The
        // driver's parser refuses a path by design — it only speaks Postgres — so
        // the file is shown as the address it is rather than as a redacted
        // non-answer.
        Err(_) => changes.push(Change {
            path: report::display_name("store.database_url"),
            value: address.clone(),
            state,
            note: Some("a file on this machine".to_owned()),
        }),
    }
    changes.extend(dropped(scope, entries, current));
    Some(changes)
}

/// The paths the database scope watches, whether or not a run writes them.
pub(crate) fn watched() -> &'static [&'static str] {
    SCOPE
}

/// One part row, which takes the state of the address it was read from.
///
/// The parts are not config entries, so each one takes the address's state: a new
/// address means every part of it is new, and reporting them any other way would
/// claim a part is unchanged when the string it lives in is being replaced.
fn part(name: &str, value: &str, state: ChangeState, note: Option<String>) -> Change {
    Change {
        path: name.to_owned(),
        value: value.to_owned(),
        state,
        note,
    }
}

/// Whether the address carries a password in its userinfo.
///
/// The colon is the separator: `postgres://user:password@host/db` has one and
/// `postgres://user@host/db` does not. Everything after the first `:` is the
/// password, so a password that itself contains a colon still satisfies this.
///
/// Read off the string rather than off the plan, because the plan no longer
/// knows: a pasted hosted address and a typed one both arrive as one string, and
/// the row has to follow the address that is actually written.
fn has_password(address: &str) -> bool {
    let Some(sep) = address.find("://") else {
        return false;
    };
    let userinfo = &address[sep + 3..];
    userinfo
        .find('@')
        .is_some_and(|at| userinfo[..at].contains(':'))
}

/// The encryption and locality of the address, as the driver sees them.
///
/// A loopback address never leaves the machine, so it is labelled by where it
/// goes rather than by whether it is encrypted; anything else is verified
/// (`verify-full`) or deliberately unverified (`encrypt-no-verify`), and is
/// labelled by whether the server certificate is checked.
fn locality_of(parsed: &clio_store::Connection, ssl: &str) -> String {
    let encryption = match ssl {
        "disable" => "not encrypted",
        "prefer" => "prefer",
        "encrypt-no-verify" => "encrypted, not verified",
        _ => "encrypted",
    };
    let place = if parsed.is_local() {
        "this machine only"
    } else {
        "remote"
    };
    format!("{encryption}, {place}")
}

/// One `New` or `Unchanged` row for a config path.
///
/// `Removed` when the run is not writing that path at all, which is what lets
/// the address's parts be grouped under one state without each re-deciding it.
fn state_of(
    path: &str,
    entries: &[(String, Value)],
    current: &dyn Fn(&str) -> Option<Value>,
) -> ChangeState {
    let Some((_, value)) = entries.iter().find(|(planned, _)| planned == path) else {
        return ChangeState::Removed;
    };
    match current(path) {
        Some(existing) if existing == *value => ChangeState::Unchanged,
        _ => ChangeState::New,
    }
}

/// The `backend` value the run writes, read from the entries.
fn backend_of(entries: &[(String, Value)]) -> String {
    entries
        .iter()
        .find(|(path, _)| path == "backend")
        .map(|(_, value)| value.as_str().unwrap_or_default().to_owned())
        .unwrap_or_default()
}

/// The watched rows the database scope drops, minus the address row itself.
///
/// The address is excluded because [`block`] already shows it as its parts, and
/// showing it again as a removal would claim the run deletes a field it is
/// rewriting.
fn dropped(
    scope: &[&'static str],
    entries: &[(String, Value)],
    current: &dyn Fn(&str) -> Option<Value>,
) -> Vec<Change> {
    scope
        .iter()
        .filter(|path| **path != "store.database_url")
        .filter(|path| !entries.iter().any(|(planned, _)| planned == **path))
        .filter(|path| current(path).is_some())
        .map(|path| Change {
            path: report::display_name(path),
            value: String::new(),
            state: ChangeState::Removed,
            note: Some("will be removed".to_owned()),
        })
        .collect()
}
