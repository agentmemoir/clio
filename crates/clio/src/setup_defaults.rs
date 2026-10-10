// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for turning the effective configuration document
//! into the default value each `clio setup` question shows.
//!
//! ## Owns
//! - `Defaults`: the last known answer for every question that has one
//! - `from_effective`, which reads the effective document and parses an address
//! - The rule that an unparsable stored address is ignored, never fatal
//!
//! ## Does not own
//! - The questions themselves (see `setup_render` and `setup_steps`)
//! - Reading the runtime (stage 6 builds this value and passes it in)
//! - Validating an answer (see `setup_validate`)
//!
//! ## Boundary
//!
//! Changes to what a question defaults to should normally require changes here.
//! Changes to which questions are asked should not require changes here.
//!
//! Keep this module focused on reading the last value out of a document.
//!
//! # Why an unparsable stored address is not an error
//!
//! A config that cannot be parsed is a config someone edited by hand, and the
//! wizard's job there is to ask. Treating the failure as fatal would make setup
//! refuse to run on the very install it is most useful for, so the address is
//! ignored and the questions start blank. The stored value is left exactly as
//! it is; only the *default* is dropped.

use crate::setup_plan_db::{DatabaseChoice, HandOver};
use serde_json::Value;
use std::path::PathBuf;

/// What a question offers as its default.
///
/// `None` means the question has no last value, which is a first run rather
/// than a missing answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent stored fact; a state machine over them would be fiction"
)]
pub(crate) struct Defaults {
    /// Which database situation the last run chose.
    pub(crate) database: Option<DatabaseChoice>,
    /// The SQLite file the last run wrote.
    pub(crate) sqlite_path: Option<PathBuf>,
    /// The stored connection address, exactly as it was written.
    pub(crate) address: Option<String>,
    /// The host the stored address names.
    pub(crate) host: Option<String>,
    /// The port the stored address names, rendered for the question.
    pub(crate) port: Option<String>,
    /// The database name the stored address names.
    pub(crate) name: Option<String>,
    /// The username the stored address names.
    pub(crate) user: Option<String>,
    /// The SSL mode the stored address asks for.
    pub(crate) ssl: Option<String>,
    /// Whether a password is already stored, so the question need not show it.
    pub(crate) has_password: bool,
    /// How the last run handed the connection over.
    pub(crate) handover: Option<HandOver>,
    /// The advanced settings the stored address carries in its query string.
    pub(crate) advanced: Vec<(String, String)>,
    /// The embedding endpoint the last run wrote.
    pub(crate) embed_url: Option<String>,
    /// The embedding model the last run wrote.
    pub(crate) embed_model: Option<String>,
    /// The embedding provider the last run wrote.
    pub(crate) embed_provider: Option<String>,
    /// The rerank endpoint the last run wrote.
    pub(crate) rerank_url: Option<String>,
    /// The rerank model the last run wrote.
    pub(crate) rerank_model: Option<String>,
    /// The rerank provider the last run wrote.
    pub(crate) rerank_provider: Option<String>,
    /// Whether a hosted embedding key is already stored.
    pub(crate) has_embed_key: bool,
    /// Whether a hosted rerank key is already stored.
    pub(crate) has_rerank_key: bool,
    /// The stored extraction role.
    pub(crate) extract: Option<String>,
    /// Whether multi-host sync is in play, from the effective `/sync/omit`.
    ///
    /// `sync.omit` defaults to true, so a config that never mentions it is the
    /// single-host case and the sync-key question stays out of the plan.
    pub(crate) sync_in_play: bool,
}

impl Defaults {
    /// Defaults for a run with no config file at all.
    ///
    /// Every question is blank, which is what a first run looks like. The one
    /// exception is the SQLite path, which has a standard location the wizard
    /// always offers, and it is added by the question rather than here so the
    /// caller that knows the data directory stays the caller.
    #[must_use]
    pub(crate) fn empty() -> Self {
        Self {
            database: None,
            sqlite_path: None,
            address: None,
            host: None,
            port: None,
            name: None,
            user: None,
            ssl: None,
            has_password: false,
            handover: None,
            advanced: Vec::new(),
            embed_url: None,
            embed_model: None,
            embed_provider: None,
            rerank_url: None,
            rerank_model: None,
            rerank_provider: None,
            has_embed_key: false,
            has_rerank_key: false,
            extract: None,
            sync_in_play: false,
        }
    }

    /// Read the defaults out of the effective configuration document.
    ///
    /// A value that is present becomes the question's default. A stored address
    /// is parsed with the driver's own parser so host, port, name, and user come
    /// from the same reader the store uses; when that parse fails, every field
    /// default is dropped and the address default is kept, because the operator
    /// can still press Enter and write it back unchanged.
    #[must_use]
    pub(crate) fn from_effective(effective: &Value) -> Self {
        let mut defaults = Self::empty();
        defaults.database = text(effective, &["backend"]).and_then(|name| match name.as_str() {
            "postgres" => Some(DatabaseChoice::Existing),
            "sqlite" => Some(DatabaseChoice::Sqlite),
            _ => None,
        });
        defaults.sqlite_path = text(effective, &["store", "sqlite_path"]).map(PathBuf::from);
        // A stored backend cannot say which of the two Postgres situations it was,
        // because both write `postgres`. The address settles it: a loopback
        // address Clio generated is the managed case, and that is the only place a
        // managed one ever appears.
        //
        // The address is only read when the backend says a server is in use. The
        // merged document carries a default SQLite path in `store.database_url`
        // whether or not the file set anything, and offering that as "the address
        // you used last" would put a file path in front of an operator who has
        // never run Clio.
        if defaults.database == Some(DatabaseChoice::Sqlite) {
            defaults.database = Some(DatabaseChoice::Sqlite);
            defaults.handover = Some(HandOver::Pasted);
        } else {
            defaults.address = text(effective, &["store", "database_url"]);
            if let Some(address) = defaults.address.clone()
                && let Ok(parsed) = clio_store::parse_connection(&address)
            {
                defaults.host = Some(parsed.primary_host().to_owned());
                defaults.port = Some(parsed.primary_port().to_string());
                defaults.name.clone_from(&parsed.database);
                defaults.user.clone_from(&parsed.user);
                defaults.ssl = Some(parsed.ssl.as_str().to_owned());
                defaults.has_password = has_password(&address);
                defaults.handover = Some(HandOver::Pasted);
            }
        }
        defaults.embed_url = text(effective, &["embed", "url"]);
        defaults.embed_model = text(effective, &["embed", "model"]);
        defaults.embed_provider = text(effective, &["embed", "provider"]);
        defaults.rerank_url = text(effective, &["rerank", "url"]);
        defaults.rerank_model = text(effective, &["rerank", "model"]);
        defaults.rerank_provider = text(effective, &["rerank", "provider"]);
        defaults.has_embed_key = secret_is_set(text(effective, &["credentials", "embed_api_key"]));
        defaults.has_rerank_key =
            secret_is_set(text(effective, &["credentials", "rerank_api_key"]));
        defaults.extract = text(effective, &["extract", "role"]);
        defaults.advanced = advanced_of(defaults.address.as_deref());
        defaults.sync_in_play = !crate::setup_sync_config::single_host(effective);
        defaults
    }
}

/// Whether a stored address carries a password, without ever holding it.
///
/// The question that offers this says "keep the stored value"; it never shows
/// the value, so all this needs is the fact.
fn has_password(address: &str) -> bool {
    let Some((_, after)) = address.split_once("://") else {
        return false;
    };
    let Some((userinfo, _)) = after.split_once('@') else {
        return false;
    };
    userinfo
        .split_once(':')
        .is_some_and(|(_, password)| !password.is_empty())
}

/// Whether a stored secret is present, without ever holding it.
///
/// An empty string counts as absent, because that is what a config with no key
/// set reads as, and showing a default the operator cannot see is worse than
/// asking.
fn secret_is_set(value: Option<String>) -> bool {
    value.is_some_and(|secret| !secret.trim().is_empty())
}

/// The advanced settings a stored address carries, for the questions to reuse.
///
/// Reading them back out of the address rather than storing them separately
/// keeps one source of truth: the address is what the driver will parse, so the
/// defaults cannot disagree with it.
fn advanced_of(address: Option<&str>) -> Vec<(String, String)> {
    let Some(address) = address else {
        return Vec::new();
    };
    let Some((_, query)) = address.split_once('?') else {
        return Vec::new();
    };
    crate::setup_db_options::ADVANCED
        .iter()
        .filter_map(|setting| {
            let value = query.split('&').find_map(|pair| {
                let (name, value) = pair.split_once('=')?;
                (name == setting.name).then(|| value.to_owned())
            })?;
            Some((setting.name.to_owned(), value))
        })
        .collect()
}

/// One string at a nested path, or `None` when it is absent or not a string.
fn text(value: &Value, path: &[&str]) -> Option<String> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(key)?;
    }
    cursor.as_str().map(ToOwned::to_owned)
}

#[cfg(test)]
#[path = "setup_defaults_tests.rs"]
mod tests;
