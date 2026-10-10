// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the database choice as a value: which of the
//! three situations the operator is in, the address each one produces, and the
//! config entries that make the store actually open there.
//!
//! ## Owns
//! - `DatabaseChoice` and `HandOver`, with the flag spellings each accepts
//! - `DatabasePlan`, the assembled address plus everything apply needs
//! - `entries`, which writes `backend` and `store.database_url` together
//! - Percent-encoding on assembly, and the driver's parse as the final check
//!
//! ## Does not own
//! - Asking the questions (stage 5 owns that)
//! - Writing the config and pruning the fields a new choice no longer wants
//!   (see `setup_config`)
//! - Starting containers, which is the compose helper's job
//!
//! ## Boundary
//!
//! Changes to what a choice turns into should normally require changes here.
//! Changes to the questions, the preview, or the write should not require
//! changes here.
//!
//! Keep this module focused on turning a choice into an address and a set of
//! config entries.

use crate::setup_db_fields::ssl_query;
use clio_store::SslChoice;
use serde_json::{Value, json};
use std::path::PathBuf;

/// The three database situations, which are three and not two.
///
/// Clio does not care where a server runs or who runs it, so a hosted database
/// and one you run yourself are the same problem: connect to an address that
/// already exists. Splitting them would teach a false lesson.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DatabaseChoice {
    /// Clio creates the database and starts it in Docker.
    Managed,
    /// A SQLite file, with no Docker at all.
    Sqlite,
    /// A Postgres that already exists.
    Existing,
}

impl DatabaseChoice {
    /// Read a choice from its flag spelling.
    ///
    /// Both the menu word and the flag word are accepted, so `--database
    /// docker` and `--database managed` mean the same thing.
    #[must_use]
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "managed" | "docker" => Some(Self::Managed),
            "sqlite" | "file" => Some(Self::Sqlite),
            "existing" | "external" => Some(Self::Existing),
            _ => None,
        }
    }

    /// The word the question offers, and the summary names.
    ///
    /// The one place a choice is spelled in English, so the option the operator
    /// reads and the line the applied summary prints cannot be two different
    /// sentences.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Managed => "Clio Manages Postgres in Docker",
            Self::Sqlite => "A SQLite file on this machine",
            Self::Existing => "A Postgres you already have",
        }
    }
}

/// How the operator wants to hand over a connection to an existing server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandOver {
    /// One long address, kept exactly as given.
    Pasted,
    /// The fields, with the password masked as it is typed.
    Typed,
}

impl HandOver {
    /// Read a hand-over from its flag spelling.
    #[must_use]
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pasted" | "paste" => Some(Self::Pasted),
            "typed" | "details" => Some(Self::Typed),
            _ => None,
        }
    }

    /// The word the question offers.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Pasted => "Paste a connection string",
            Self::Typed => "Type the details",
        }
    }
}

/// One advanced connection setting, on the typed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Extra {
    /// The name the driver spells it with.
    pub name: &'static str,
    /// The value as typed. Encoded on assembly, because the driver reads these
    /// out of a query string.
    pub value: String,
}

/// Everything apply needs to turn a choice into a working store.
///
/// Two fields have no reader in this stage: the advanced settings, and the store
/// this plan replaces. Stage 6 is the only caller that wants them — the extras
/// go to the applied summary, and the previous store names the switch warning —
/// so they are held rather than dropped. The allowance is on the fields alone,
/// so a genuinely dead method still warns.
#[allow(dead_code, reason = "read by the stage 6 apply path")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DatabasePlan {
    /// Which of the three situations this is.
    pub choice: DatabaseChoice,
    /// The connection address, or the SQLite file path.
    pub address: String,
    /// The SQLite file, when the choice is a file.
    pub sqlite_path: Option<PathBuf>,
    /// The encryption mode the address asks for.
    pub ssl: SslChoice,
    /// The advanced settings the operator answered.
    pub extra: Vec<Extra>,
    /// The store this plan replaces, for the switch warning.
    pub previous_store: Option<String>,
}

impl DatabasePlan {
    /// The config entries this plan writes, in the order they are applied.
    ///
    /// `backend` and `store.database_url` are always written together. They are
    /// one statement, and writing either alone is the silent-SQLite bug: a
    /// config that kept `backend: sqlite` beside a Postgres address made every
    /// command open a local file while the status command read the address and
    /// reported Postgres. Two commands, two different stores, one config.
    ///
    /// A SQLite file adds `store.sqlite_path` as well, so switching back to
    /// Postgres later has a file path to remove. Removing what a new choice no
    /// longer wants is the watched-field prune, which stage 6 owns.
    #[must_use]
    pub(crate) fn entries(&self) -> Vec<(String, Value)> {
        let mut entries = vec![
            ("backend".to_owned(), json!(self.backend_name())),
            ("store.database_url".to_owned(), json!(self.address)),
        ];
        if let Some(path) = &self.sqlite_path {
            entries.push((
                "store.sqlite_path".to_owned(),
                json!(path.to_string_lossy()),
            ));
        }
        entries
    }

    /// The Compose services this choice needs started.
    ///
    /// `postgres` carries no `profiles:` key, so it is Compose's default data
    /// plane and `docker compose --profile … up -d` starts it whatever the
    /// profile says. Naming the services explicitly is the other half of that
    /// fix, and this is the half that leaves `postgres` out when it was not
    /// chosen.
    #[must_use]
    pub(crate) fn database_services(&self) -> &'static [&'static str] {
        match self.choice {
            DatabaseChoice::Managed => &["postgres"],
            DatabaseChoice::Sqlite | DatabaseChoice::Existing => &[],
        }
    }

    /// Whether this choice starts a database container at all.
    #[allow(dead_code, reason = "the stage 6 start path")]
    #[must_use]
    pub(crate) fn starts_database(&self) -> bool {
        matches!(self.choice, DatabaseChoice::Managed)
    }

    /// The backend name this plan opens.
    #[must_use]
    pub(crate) fn backend_name(&self) -> &'static str {
        match self.choice {
            DatabaseChoice::Sqlite => "sqlite",
            DatabaseChoice::Managed | DatabaseChoice::Existing => "postgres",
        }
    }

    /// A one-line description with no password in it, for previews.
    ///
    /// The address is shown through the store's own redaction, because a typed
    /// password is inside it. The managed address carries no password at all, so
    /// redaction leaves it exactly as written.
    #[must_use]
    pub(crate) fn describe(&self) -> String {
        let shown = clio_ops::redact_credentials(&self.address);
        match self.choice {
            DatabaseChoice::Sqlite => format!("SQLite file at {shown}"),
            _ => format!("Postgres at {shown} ({})", self.ssl.as_str()),
        }
    }
}

/// The address for a database Clio creates itself.
///
/// No password, because the managed database has none: `compose.yml` sets the
/// image's trust authentication and the port is published on loopback only, so
/// the bind is the boundary. The driver still parses the result, so an address
/// that cannot be opened is refused here rather than at the first write.
///
/// # Errors
/// A field is empty, or the driver refuses the assembled address.
pub(crate) fn managed_address(
    host: &str,
    port: &str,
    database: &str,
    user: &str,
    ssl: SslChoice,
) -> Result<String, String> {
    let address = format!(
        "postgres://{user}@{host}:{port}/{database}?{}",
        ssl_query(ssl)
    );
    check_openable(&address)?;
    Ok(address)
}

/// The address for a database the operator typed in field by field.
///
/// # Errors
/// A field is empty, or the driver refuses the assembled address.
#[allow(
    clippy::too_many_arguments,
    reason = "an address is assembled from its fields; grouping them would only move the same values"
)]
pub(crate) fn typed_address(
    host: &str,
    port: &str,
    database: &str,
    user: &str,
    password: &str,
    ssl: SslChoice,
    root: Option<&str>,
    extra: &[Extra],
) -> Result<String, String> {
    let mut address = format!(
        "postgres://{}:{}@{host}:{port}/{database}?{}",
        encode(user),
        encode(password),
        ssl_query(ssl)
    );
    // The supplied root rides in the address, spelled the way libpq spells it,
    // so the store reads it back with everything else the connection needs. It
    // is only meaningful for a mode that checks the certificate, so a mode that
    // does not check one never carries it.
    if matches!(ssl, SslChoice::Prefer | SslChoice::VerifyFull)
        && let Some(root) = root.filter(|path| !path.trim().is_empty())
    {
        address.push('&');
        address.push_str(crate::setup_db_fields::SSL_ROOT_PARAM);
        address.push('=');
        address.push_str(&encode(root));
    }
    for setting in extra {
        if setting.value.trim().is_empty() {
            continue;
        }
        address.push('&');
        address.push_str(setting.name);
        address.push('=');
        address.push_str(&encode(&setting.value));
    }
    check_openable(&address)?;
    Ok(address)
}

/// Refuse an address no store can open, naming the address as well as the reason.
///
/// One gate, not two: this is the same check the questions run, so the wizard
/// cannot assemble an address its own validator would refuse, and a message
/// that reached the operator during a check does not change shape on the way
/// into the plan.
///
/// The driver's parser alone is not enough. It accepts
/// `postgres://user:pw@:/?sslmode=prefer` and reads the empty host as "the
/// local socket", which is a connection to a database nobody named.
fn check_openable(address: &str) -> Result<(), String> {
    crate::setup_validate::check_connection(address).map_err(|reason| {
        format!("these answers do not make a usable address ({reason}): {address}")
    })
}

/// The plan for a database Clio creates itself.
///
/// # Errors
/// The answers do not make an address the driver will open. Refusing here
/// keeps a plan from ever carrying an address that cannot be connected to,
/// which is the failure the whole resolver change exists to prevent.
pub(crate) fn managed(
    host: &str,
    port: &str,
    database: &str,
    user: &str,
    ssl: SslChoice,
    previous_store: Option<String>,
) -> Result<DatabasePlan, String> {
    let address = managed_address(host, port, database, user, ssl)?;
    Ok(DatabasePlan {
        choice: DatabaseChoice::Managed,
        address,
        sqlite_path: None,
        ssl,
        extra: Vec::new(),
        previous_store,
    })
}

/// The plan for a database the operator already has.
///
/// The address has already been through [`super::setup_validate::check_connection`]
/// by the time it gets here, so this only records it.
///
/// # Errors
/// A field is empty, or the address is not one the driver will open.
pub(crate) fn existing(
    address: String,
    ssl: SslChoice,
    extra: Vec<Extra>,
    previous_store: Option<String>,
) -> Result<DatabasePlan, String> {
    check_openable(&address)?;
    Ok(DatabasePlan {
        choice: DatabaseChoice::Existing,
        address,
        sqlite_path: None,
        ssl,
        extra,
        previous_store,
    })
}

/// The plan for a SQLite file.
///
/// No Docker, no password, and no address to connect to: the file path is the
/// address. `previous_store` is kept so a switch away from a database can name
/// what is being left behind.
#[must_use]
pub(crate) fn sqlite(path: PathBuf, previous_store: Option<String>) -> DatabasePlan {
    DatabasePlan {
        choice: DatabaseChoice::Sqlite,
        address: path.to_string_lossy().into_owned(),
        sqlite_path: Some(path),
        ssl: SslChoice::Prefer,
        extra: Vec::new(),
        previous_store,
    }
}

/// Percent-encode everything outside the RFC 3986 unreserved set.
///
/// Only the unreserved set passes through, so a space, an `&`, an `=`, or a `?`
/// in a value cannot end the value early or start a parameter of its own. A
/// multi-byte character is encoded byte by byte, which is what the driver
/// expects.
fn encode(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            other => {
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "setup_plan_db_tests.rs"]
mod tests;
