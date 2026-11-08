// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `clio version` and its `--check` request: the
//! identity line, the installed skill marker the check reads, the comparison with
//! the published release, and the one JSON line that reports all of it.
//!
//! ## Owns
//! - The verb's own flags: `--check`, and `--harness <name>` as a narrowing of it
//! - The skill roots the check reads, selected the same way `clio install`
//!   selects them
//! - The installation evidence: the recorded package version and catalog hash,
//!   and the three ways it can fail to establish one
//! - The comparison rule and the four-key JSON line, with the exit code it maps to
//!
//! ## Does not own
//! - The version text's grammar and ordering (`version_check_semver`)
//! - The release request and its bounds (`version_check_release`)
//! - Which harnesses exist or which roots each one reads (`agent_harness`)
//! - Reading the marker file itself (`skill_install`, which reads it for install)
//! - Whether a path on the way to a marker may be followed at all
//!   (`skill_uninstall_guard`)
//! - Flag parsing (`cli_top_level::parse_flags`) or the verb dispatch (`main`)
//!
//! ## Boundary
//!
//! Changes to what the check reads, what it reports, or what it exits with should
//! normally require changes here. Changes to the request's bounds belong in
//! `version_check_release`, and changes to the marker's recorded shape belong in
//! `skill_marker`.
//!
//! The reported version is the one the installed marker records, never the
//! running binary's own version: a binary that answers about itself would claim
//! to be current no matter what is installed. Missing evidence skips the network
//! call entirely, so the command never asks about a release it cannot compare
//! against.
//!
//! A marker reached through a symbolic link is not evidence either, and it is
//! refused before it is read: a link can point at any file on the machine, so a
//! version read through one would describe something this package does not hold.
//! The refusal reuses the guard the removal path applies to the same file, so
//! what may be trusted is decided in one place.
//!
//! Keep this module focused on the `version` verb's check request.

use crate::agent_harness::{self, SkillRoot};
use crate::agent_harness_targets::{DirectWriteEnv, resolve_skill_root};
use crate::cli_error::{EXIT_OK, EXIT_OPERATIONAL};
use crate::cli_top_level::{
    parse_flags, print_version_help, record_flag_names, report_usage_error,
};
use crate::skill_install::{InstalledMarker, SKILL_DIR_NAME, installed_marker};
use crate::skill_marker::SKILL_MARKER_FILE;
use crate::skill_uninstall_guard::symlink_component;
use crate::version_check_release;
use crate::version_check_semver::Version;
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;

/// The portable root alone, which is what `clio install` writes with no
/// `--harness`.
const PORTABLE_ROOTS: &[SkillRoot] = &[SkillRoot::Portable];

/// What the check reports, in the order the JSON line writes the keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    /// The version the installed marker recorded, or `null` when none did.
    skill_version: Option<String>,
    /// The catalog hash the installed marker recorded, or `null`.
    catalog_hash: Option<String>,
    /// Whether the installed version is current, older than the release, or
    /// could not be compared with a release at all.
    release_status: ReleaseStatus,
    /// The published version, or `null` when none could be established.
    latest_version: Option<String>,
}

/// The three answers the check can give.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ReleaseStatus {
    /// The installed version is equal to, or newer than, the release.
    Current,
    /// A release newer than the installed version exists.
    Newer,
    /// No answer could be established, so nothing is claimed.
    Unavailable,
}

/// Why no installation evidence could be established. Every reason is
/// unavailable; they are named apart so a test can pin the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Missing {
    /// No selected root holds an installed marker.
    NoMarker,
    /// A marker is there and cannot be read, or records no version.
    UnreadableMarker,
    /// Two selected roots hold markers that disagree.
    ConflictingMarkers,
    /// The `--harness` name is not one install accepts, so no root is selected.
    UnknownHarness,
}

/// What one installation recorded, as the check reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Evidence {
    /// The version of the program that installed the package.
    pub(crate) package_version: String,
    /// The catalog hash the schema pack published at install time.
    pub(crate) catalog_hash: String,
}

/// Run `clio version` with its own arguments: the identity line, or the check.
pub(crate) fn version(args: &[String]) -> ExitCode {
    let flags = match parse_flags(args.iter().cloned(), &record_flag_names("version")) {
        Ok(flags) => flags,
        Err(error) => return report_usage_error(&error, args),
    };
    if flags.contains_key("help") {
        return print_version_help();
    }
    let harness = flags.get("harness").map(String::as_str);
    if !flags.contains_key("check") {
        // `--harness` narrows the check; alone it names nothing to do, and a
        // silently plain identity line would look like the check had run.
        if let Some(name) = harness {
            let error = crate::cli_error::CliError::usage_hint(
                format!("--harness `{name}` needs --check"),
                "run `clio version --check --harness <name>`, or drop --harness",
            );
            return report_usage_error(&error, args);
        }
        println!("{}", clio::identity());
        return ExitCode::SUCCESS;
    }
    let report = check(
        harness,
        &DirectWriteEnv::production(),
        RELEASE_ENDPOINT_FOR_CHECK,
    );
    println!("{}", report.line());
    ExitCode::from(report.exit_code())
}

/// The endpoint the command reads. Named here so the one address the process
/// reaches is visible beside the code that calls it.
const RELEASE_ENDPOINT_FOR_CHECK: &str = version_check_release::RELEASE_ENDPOINT;

/// Read the installation evidence, then the published release, and report.
///
/// The endpoint is an argument so a test can point the request at a local
/// listener; production passes the constant above.
pub(crate) fn check(harness: Option<&str>, env: &DirectWriteEnv, endpoint: &str) -> Report {
    let evidence = match installation_evidence(harness, env) {
        Ok(evidence) => evidence,
        Err(_missing) => return Report::unavailable(None),
    };
    // An installed version that is not a version cannot be compared, so the
    // request is not made: there is nothing an answer could be compared with.
    let Some(installed) = Version::parse(&evidence.package_version) else {
        return Report::unavailable(Some(&evidence));
    };
    let Some(latest) = version_check_release::latest_release(endpoint) else {
        return Report::unavailable(Some(&evidence));
    };
    let status = if installed >= latest {
        ReleaseStatus::Current
    } else {
        ReleaseStatus::Newer
    };
    Report {
        release_status: status,
        skill_version: Some(evidence.package_version),
        catalog_hash: blank_to_none(evidence.catalog_hash),
        latest_version: Some(latest.to_string()),
    }
}

/// The evidence the selected roots agree on.
///
/// # Errors
/// No root holds a marker, a marker cannot be read or records no version, two
/// roots disagree, or the harness name is not one install accepts.
pub(crate) fn installation_evidence(
    harness: Option<&str>,
    env: &DirectWriteEnv,
) -> Result<Evidence, Missing> {
    let roots = selected_roots(harness).ok_or(Missing::UnknownHarness)?;
    let mut packages: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        let path = resolve_skill_root(*root, env).ok_or(Missing::NoMarker)?;
        let package = path.join(SKILL_DIR_NAME);
        if !packages.contains(&package) {
            packages.push(package);
        }
    }
    let mut found: Vec<Evidence> = Vec::new();
    for package in packages {
        // A marker reached through a symbolic link is not this installation's
        // evidence: a link can name any file, so a version read through one would
        // describe something the package does not hold. The removal path refuses
        // the same link, and this reuses its guard rather than growing a second
        // one. A path the guard cannot inspect is refused the same way.
        if !matches!(symlink_component(&package, SKILL_MARKER_FILE), Ok(false)) {
            return Err(Missing::UnreadableMarker);
        }
        match installed_marker(&package) {
            Ok(InstalledMarker::Absent) => {}
            Ok(InstalledMarker::Read(marker)) if marker.package_version.trim().is_empty() => {
                return Err(Missing::UnreadableMarker);
            }
            Ok(InstalledMarker::Read(marker)) => {
                let evidence = Evidence {
                    package_version: marker.package_version,
                    catalog_hash: marker.catalog_hash,
                };
                if !found.contains(&evidence) {
                    found.push(evidence);
                }
            }
            Ok(InstalledMarker::Unreadable) | Err(_) => {
                return Err(Missing::UnreadableMarker);
            }
        }
    }
    match found.len() {
        0 => Err(Missing::NoMarker),
        1 => Ok(found.remove(0)),
        _ => Err(Missing::ConflictingMarkers),
    }
}

/// The roots the selected harness reads, resolved the way `clio install`
/// resolves them: the portable root alone with no harness, and the harness's own
/// roots otherwise. `None` when the name is not one install accepts.
fn selected_roots(harness: Option<&str>) -> Option<&'static [SkillRoot]> {
    match harness {
        None => Some(PORTABLE_ROOTS),
        Some(name) => {
            let record = agent_harness::resolve(name)?;
            Some(agent_harness::skill_roots(record.name))
        }
    }
}

/// A recorded field that is empty is unknown, not the empty string.
fn blank_to_none(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
}

impl Report {
    /// The unavailable answer: `current` is never claimed, and every field that
    /// states a fact the run could not establish is `null`.
    fn unavailable(evidence: Option<&Evidence>) -> Report {
        Report {
            skill_version: evidence.map(|found| found.package_version.clone()),
            catalog_hash: evidence.and_then(|found| blank_to_none(found.catalog_hash.clone())),
            release_status: ReleaseStatus::Unavailable,
            latest_version: None,
        }
    }

    /// The one compact JSON line the command prints.
    pub(crate) fn line(&self) -> String {
        serde_json::to_string(self).expect("the version report serializes")
    }

    /// The process exit code: `current` and `newer` succeed, and every
    /// unavailable answer is an operational failure.
    fn exit_code(&self) -> u8 {
        match self.release_status {
            ReleaseStatus::Current | ReleaseStatus::Newer => EXIT_OK,
            ReleaseStatus::Unavailable => EXIT_OPERATIONAL,
        }
    }
}

/// The scratch home, marker writer, and loopback listener that this module's
/// suite and `version_check_release`'s suite share. Declared once, and
/// `pub(crate)` because the sibling suite is not a child of this module.
#[cfg(test)]
#[path = "version_check_test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "version_check_tests.rs"]
mod tests;
