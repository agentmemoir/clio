// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for making everything the wizard would reach for
//! injectable, so a test can script a success or a refusal without a server, a
//! network, or a terminal.
//!
//! ## Owns
//! - The two probe signatures and the `Probes` that carries them
//! - The masked-reader signature, as the one the render helper takes
//! - The constructors a scripted test uses
//!
//! ## Does not own
//! - Running a probe and reporting it (see `setup_connect`)
//! - Building the real probes from `clio-store` and `clio-index`: stage 6 owns
//!   that wiring, and doing it here would make this module depend on a transport
//!   it does not use
//! - Building the real masked reader (see `setup_secrets`)
//!
//! ## Boundary
//!
//! Changes to what a probe or a reader returns should normally require changes
//! here. Changes to how a probe is run or reported should not.
//!
//! Keep this module focused on the seams, with no probe and no reader of its own.
//!
//! # Why the real probes are not built here
//!
//! A probe that built its own connector would be a second implementation of the
//! one the store uses, and the two would eventually disagree: setup would report
//! a database healthy while the first real write failed. So the real closures are
//! the store's own probe and the index's own endpoint, and stage 6 passes them
//! in.
//!
//! # Why one signature for every secret
//!
//! The database password and every API key go through the same reader. One
//! signature means one seam, so a test cannot accidentally script the API key
//! through a reader the wizard never calls, and cannot leave the password going
//! through a different one that a real terminal would use.

#[cfg(test)]
use crate::setup_secrets::SecretReader;

/// Open a database address and describe it.
pub(crate) type DatabaseProbe = Box<dyn Fn(&str) -> Result<String, String> + Send>;

/// Ask a service endpoint whether it answers, with an optional API key.
pub(crate) type EndpointProbe = Box<dyn Fn(&str, Option<&str>) -> Result<String, String> + Send>;

/// The two probes the wizard runs, injected.
///
/// A database probe opens the database the operator just named and reports what
/// is there. An endpoint probe asks a configured service whether it answers.
/// Both are plain closures so a stage 5 test can script either outcome.
pub(crate) struct Probes {
    /// Open a database address and describe it.
    pub(crate) database: DatabaseProbe,
    /// Ask a service endpoint whether it answers, with an optional API key.
    pub(crate) endpoint: EndpointProbe,
}

impl Probes {
    /// The probes, from two closures.
    pub(crate) fn new<D, E>(database: D, endpoint: E) -> Self
    where
        D: Fn(&str) -> Result<String, String> + Send + 'static,
        E: Fn(&str, Option<&str>) -> Result<String, String> + Send + 'static,
    {
        Self {
            database: Box::new(database),
            endpoint: Box::new(endpoint),
        }
    }

    /// Both probes succeed, reporting fixed text.
    ///
    /// The common case in a test that is about navigation or rendering rather
    /// than about a refusal, where a scripted failure would be noise in the
    /// captured output.
    #[cfg(test)]
    pub(crate) fn ok() -> Self {
        Self::new(
            |address: &str| Ok(format!("connected to {address}")),
            |url: &str, _| Ok(format!("{url} answered")),
        )
    }

    /// Run the database probe.
    ///
    /// # Errors
    /// Whatever the probe refused with, unchanged.
    pub(crate) fn test_database(&self, address: &str) -> Result<String, String> {
        (self.database)(address)
    }

    /// Run the endpoint probe.
    ///
    /// # Errors
    /// Whatever the probe refused with, unchanged.
    pub(crate) fn test_endpoint(&self, url: &str, api_key: Option<&str>) -> Result<String, String> {
        (self.endpoint)(url, api_key)
    }
}

/// A masked reader that hands out scripted values in order.
///
/// The last value repeats once the script runs out, so a test that re-asks a
/// question does not have to script the repeat. A script of zero values refuses
/// instead, which is the honest failure: a test that expected a secret and
/// scripted none has a bug, and silently returning an empty string would let it
/// pass with a config that has no password.
#[cfg(test)]
pub(crate) fn scripted_secrets(values: Vec<String>) -> Box<SecretReader> {
    let mut queue = values.into_iter();
    let mut last: Option<String> = None;
    Box::new(move |_out: &mut dyn std::io::Write, _label: &str| {
        let next = queue.next().or_else(|| last.clone());
        match next {
            Some(value) => {
                last = Some(value.clone());
                Ok(value)
            }
            None => Err("the scripted reader had no value left to give".to_owned()),
        }
    })
}

#[cfg(test)]
#[path = "setup_test_seam_tests.rs"]
mod tests;
