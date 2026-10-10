// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the injected environment and the three
//! database-address readers under test, so every case in this target hands the
//! same inputs to the same set of paths and reads back one comparable answer
//! from each.
//!
//! ## Owns
//! - `InjectedEnv`, a lookup built from pairs, which reads and writes no global
//! - `read_all`, which runs the three readers over one injected environment
//! - `assert_fallback`, the one comparison the absent-value cases share
//!
//! ## Does not own
//! - The cases and what each asserts (see the test modules beside this one)
//! - The precedence any reader applies (see the config and store crates)
//! - Which variable names a database (see the config crate's `db_url_env`)
//!
//! ## Boundary
//!
//! Changes to which readers take part in the parity check should normally
//! require changes here. Changes to what a reader does with a value should not
//! require changes here.
//!
//! Keep this module focused on the injected inputs and the readers.

use clio_config::Runtime;
use clio_config::db_path::resolve_database;
use clio_store::{StoreBackend, StoreOpenOptions};
use serde_json::{Value, json};

/// The prefixed name, spelled out rather than read from the crate's constant.
///
/// A case that used the constant would agree with a renamed constant by
/// construction, so the rename this target guards would not be caught here. The
/// constant is pinned to this literal in a separate test instead.
pub const PREFIXED: &str = "CLIO_DATABASE_URL";

/// The name this variable carried before the rename.
///
/// It is an input to a negative case. Nothing in this target reads it.
pub const UNPREFIXED: &str = "DATABASE_URL";

/// A first address. Fake on purpose: no real host, role or password is here.
pub const FIRST: &str = "postgres://first:fake@127.0.0.1:1/first";

/// A second address, unlike the first in every character that matters.
pub const SECOND: &str = "postgres://second:fake@127.0.0.1:2/second";

/// The address the old spelling carried, in a negative case.
pub const OLD: &str = "postgres://old:fake@127.0.0.1:3/old";

/// What every path answers when no name is set at all.
///
/// Not a host-supplied value: the lookup answers nothing for `CLIO_DATA_DIR`,
/// `XDG_DATA_HOME` and `HOME` as well as for the database name, so no directory
/// is available and the relative default is the answer on every machine.
pub const FALLBACK: &str = "clio.db";

/// One injected environment, answered from a table instead of the process.
///
/// `std::env::set_var` is not used anywhere in this target. Every test in one
/// binary shares one process environment, so a write there races every other
/// test running beside it, and the code under test was given an injected lookup
/// precisely so this test does not have to.
pub struct InjectedEnv {
    entries: Vec<(String, String)>,
}

impl InjectedEnv {
    /// An environment that answers exactly the given pairs and nothing else.
    #[must_use]
    pub fn new(pairs: &[(&str, &str)]) -> Self {
        Self {
            entries: pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    /// An environment that answers nothing for any name.
    #[must_use]
    pub fn none() -> Self {
        Self::new(&[])
    }

    /// The value this environment holds for `key`, trimmed is never applied
    /// here: deciding what counts as absent belongs to the code under test.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, value)| value.clone())
    }
}

/// What each reader answered for one injected environment.
#[derive(Debug)]
pub struct Answers {
    /// The address the config overlay carries.
    pub overlay_url: String,
    /// The backend the config overlay carries.
    pub overlay_backend: String,
    /// The address the database path resolver returns.
    pub resolver_url: String,
    /// The backend the database path resolver infers.
    pub resolver_backend: String,
    /// The address the store bootstrap finds with a document that carries none.
    pub bootstrap_env_tier_url: String,
    /// The address the store bootstrap finds with the effective document.
    pub bootstrap_effective_url: String,
    /// The backend the store bootstrap chose with a document that carries none.
    pub bootstrap_backend: StoreBackend,
    /// The backend the store bootstrap chose with the effective document.
    pub bootstrap_effective_backend: StoreBackend,
}

/// A document carrying the resolved backend and no address of its own.
///
/// The bootstrap's environment tier is the only tier left when the document
/// names no address, so this is the only document that can reach it.
fn document_without_address(effective: &Value) -> Value {
    json!({ "backend": effective["backend"].clone() })
}

/// Run the three readers over one injected environment.
///
/// The lookup is a single `Fn` closure, which satisfies all three signatures:
/// the two configuration readers take a shared reference to a non-mutating
/// lookup, and the store bootstrap takes a mutable reference to a boxed one.
#[must_use]
pub fn read_all(env: &InjectedEnv) -> Answers {
    let get = move |key: &str| env.get(key);

    // The config overlay. Booting the runtime with this lookup means the system
    // defaults, the overlay and the deployment path all come from it and from
    // nothing on this machine.
    //
    // One thing does not. The runtime resolves the retention path from the real
    // process environment instead of from this lookup, so a machine with
    // `CLIO_RETENTION_PROFILES` set loads its own retention store. That store
    // sits beside the configuration rather than inside it: it holds retention
    // profiles and never an address or a backend, and it is not one of the
    // layers the effective document is merged from. So it cannot reach either
    // value read below, and the address is a fact about the code.
    let effective = Runtime::with_env(&get).effective_raw();

    // The database path resolver, with no explicit flag to outrank the variable.
    let resolved = resolve_database(&get, None, None);

    // The store bootstrap, twice. Given the effective document it must reproduce
    // that document's address. Given a document carrying no address it must
    // fall through to its own environment read, which is the only way that read
    // is ever reached. Both documents carry the backend the resolver inferred,
    // so the two answers differ only in where the address came from.
    let without_address = document_without_address(&effective);
    let mut own_env = get;
    let from_env = StoreOpenOptions::from_effective(&without_address, &mut own_env)
        .expect("a document naming a known backend always resolves");
    let mut effective_env = get;
    let from_effective = StoreOpenOptions::from_effective(&effective, &mut effective_env)
        .expect("the effective document always names a known backend");

    Answers {
        overlay_url: effective["store"]["database_url"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        overlay_backend: effective["backend"].as_str().unwrap_or_default().to_owned(),
        resolver_url: resolved.url,
        resolver_backend: resolved.backend,
        bootstrap_env_tier_url: from_env.database_url,
        bootstrap_effective_url: from_effective.database_url,
        bootstrap_backend: from_env.backend,
        bootstrap_effective_backend: from_effective.backend,
    }
}

/// Assert that every path answered the one absent-value default.
///
/// The three paths that carry a default must carry the same one. The bootstrap's
/// own environment tier must carry nothing, because a document with no address
/// has no default of its own to invent: the default can only reach it through
/// the effective document. Asserting the empty string is what keeps the two
/// bootstrap answers from being mistaken for two different databases.
pub fn assert_fallback(answers: &Answers, case: &str) {
    assert_eq!(answers.resolver_url, FALLBACK, "{case}: the resolver");
    assert_eq!(answers.overlay_url, FALLBACK, "{case}: the config overlay");
    assert_eq!(
        answers.bootstrap_effective_url, FALLBACK,
        "{case}: the store bootstrap given the effective document"
    );
    assert_eq!(
        answers.bootstrap_effective_backend,
        StoreBackend::Sqlite,
        "{case}: the effective document's own default backend"
    );
    assert_eq!(
        answers.overlay_backend, "sqlite",
        "{case}: the overlay backend"
    );
    assert_eq!(
        answers.resolver_backend, "sqlite",
        "{case}: the resolver backend"
    );
    assert_eq!(
        answers.bootstrap_backend,
        StoreBackend::Sqlite,
        "{case}: the bootstrap backend"
    );
    assert!(
        answers.bootstrap_env_tier_url.is_empty(),
        "{case}: the bootstrap's own environment tier answered {} from a document \
         that carries no address, so it read a name that was never set",
        answers.bootstrap_env_tier_url
    );
}
