// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the table of advanced Postgres connection
//! settings `clio setup` can write, and for checking a value against what the
//! driver accepts for that setting.
//!
//! ## Owns
//! - The ordered `ADVANCED` list: name, accepted values, kind, one-line help
//! - The two caveats that cost an operator an hour otherwise: `direct` needs
//!   PostgreSQL 17, and `channel_binding=require` fails on an Ed25519
//!   certificate
//! - Per-setting validation, so a value the driver would reject is refused at
//!   the question rather than at first use
//!
//! ## Does not own
//! - The connection address itself (see `setup_plan_db`)
//! - The connection parse (see `clio-store`, which owns the driver's parser)
//! - Which settings a particular database needs
//!
//! ## Boundary
//!
//! Changes to the set of settings offered, their accepted values, or their help
//! text should normally require changes here. Changes to address assembly
//! should not require changes here.
//!
//! Keep this module focused on describing and checking advanced settings.
//!
//! # Why every one of these is in the table
//!
//! The test each line was held to: a setting the driver would silently drop is
//! worse than no setting, because the config then looks deliberate when it is
//! not. Every entry below is parsed and acted on by the driver, which is why it
//! is offered at all. The free-text ones are offered because a server the
//! wizard cannot describe is a server the wizard cannot reach.

/// What kind of value a setting takes, which is what its check reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind {
    /// Anything, including an empty value meaning "the driver's default".
    Text,
    /// A whole number of seconds, above zero.
    Seconds,
    /// A whole number that may be zero.
    Count,
    /// A whole number of milliseconds, above zero.
    Millis,
}

/// One advanced connection setting.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Setting {
    /// The name the driver spells it with, used as the query parameter.
    pub name: &'static str,
    /// The values the driver accepts, empty when it accepts free text.
    pub values: &'static [&'static str],
    /// The shape of the value, for the check.
    pub kind: ValueKind,
    /// One line the help shows.
    pub help: &'static str,
    /// A second line, only when a combination or a certificate can bite.
    pub caveat: Option<&'static str>,
}

impl Setting {
    /// Check one value against this setting.
    ///
    /// An empty value is always accepted and means "leave the driver's own
    /// default alone", which is what pressing Enter at the question produces.
    ///
    /// # Errors
    /// The value is outside the accepted list, or it is not a number of the
    /// right kind. The message names what the driver accepts, so the operator
    /// is not left guessing what to type.
    pub(crate) fn check(&self, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(());
        }
        if !self.values.is_empty() && !self.values.contains(&value) {
            return Err(format!(
                "`{}` must be {}, got `{value}`",
                self.name,
                self.values.join("|")
            ));
        }
        match self.kind {
            ValueKind::Text => Ok(()),
            ValueKind::Seconds => whole(value, self.name, 1, "seconds"),
            ValueKind::Count => whole(value, self.name, 0, "a count"),
            ValueKind::Millis => whole(value, self.name, 1, "milliseconds"),
        }
    }
}

/// A whole number at or above `floor`, named in the error message.
fn whole(value: &str, name: &str, floor: u64, unit: &str) -> Result<(), String> {
    match value.parse::<u64>() {
        Ok(parsed) if parsed >= floor => Ok(()),
        _ => Err(format!(
            "`{name}` must be a whole number of {unit} of at least {floor}, got `{value}`"
        )),
    }
}

/// Every advanced setting, in the order the questions ask them.
pub(crate) const ADVANCED: &[Setting] = &[
    Setting {
        name: "options",
        values: &[],
        kind: ValueKind::Text,
        help: "Server runtime settings, such as a custom schema search path.",
        caveat: None,
    },
    Setting {
        name: "application_name",
        values: &[],
        kind: ValueKind::Text,
        help: "Shows up in the database's own activity list.",
        caveat: Some(
            "Worth setting on a shared cloud database, so your own connections are findable in \
             the provider's dashboard.",
        ),
    },
    Setting {
        name: "connect_timeout",
        values: &[],
        kind: ValueKind::Seconds,
        help: "How long to wait before giving up.",
        caveat: Some("The driver's own default is 30, which is slow on a bad address."),
    },
    Setting {
        name: "target_session_attrs",
        values: &["any", "read-write", "read-only"],
        kind: ValueKind::Text,
        help: "Refuse to land on a replica.",
        caveat: Some("Matters on a provider that keeps a read replica."),
    },
    Setting {
        name: "channel_binding",
        values: &["disable", "prefer", "require"],
        kind: ValueKind::Text,
        help: "Ties the password to the encrypted channel.",
        caveat: Some(
            "`require` FAILS against a server certificate signed with Ed25519, because that \
             signature algorithm has no tls-server-end-point digest. If that is the only thing \
             wrong, use `prefer`.",
        ),
    },
    Setting {
        name: "sslnegotiation",
        values: &["postgres", "direct"],
        kind: ValueKind::Text,
        help: "`direct` skips the old handshake step.",
        caveat: Some(
            "`direct` needs PostgreSQL 17 or newer, and the driver refuses it together with \
             sslmode `prefer`.",
        ),
    },
    Setting {
        name: "keepalives",
        values: &["on", "off"],
        kind: ValueKind::Text,
        help: "Detect a dead connection.",
        caveat: None,
    },
    Setting {
        name: "keepalives_idle",
        values: &[],
        kind: ValueKind::Seconds,
        help: "How long before the first keepalive.",
        caveat: None,
    },
    Setting {
        name: "keepalives_interval",
        values: &[],
        kind: ValueKind::Seconds,
        help: "Gap between keepalives.",
        caveat: None,
    },
    Setting {
        name: "keepalives_retries",
        values: &[],
        kind: ValueKind::Count,
        help: "How many to try before reporting the connection dead.",
        caveat: None,
    },
    Setting {
        name: "tcp_user_timeout",
        values: &[],
        kind: ValueKind::Millis,
        help: "Upper bound on unacknowledged data.",
        caveat: Some("For a network that drops silently rather than refusing."),
    },
    Setting {
        name: "hostaddr",
        values: &[],
        kind: ValueKind::Text,
        help: "Connect by address when the name does not resolve.",
        caveat: None,
    },
    Setting {
        name: "load_balance_hosts",
        values: &["disable", "random"],
        kind: ValueKind::Text,
        help: "With several hosts, try them in a random order.",
        caveat: None,
    },
];

/// The setting with this name.
///
/// # Errors
/// No setting goes by that name, so the wizard can refuse rather than write a
/// parameter the driver would ignore.
pub(crate) fn advanced(name: &str) -> Result<&'static Setting, String> {
    ADVANCED
        .iter()
        .find(|setting| setting.name == name)
        .ok_or_else(|| format!("`{name}` is not an advanced connection setting"))
}

#[cfg(test)]
#[path = "setup_db_options_tests.rs"]
mod tests;
