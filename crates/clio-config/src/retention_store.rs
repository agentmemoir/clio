// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for bank-scoped storage of retention profiles and the
//! bank override → deployment default → builtin default resolution order.
//!
//! ## Owns
//! - `RetentionStore` (in-memory bank/deployment maps) and `ProfileSource`
//! - Durable load/save of the store file and its path resolution
//! - Lossy load: an unreadable or invalid stored profile degrades to the next level
//! - The conversion of an older stored profile marker to the current version,
//!   with the carve-out that leaves a version 1 profile carrying a mission alone
//! - The fail-closed fallback for a stored provenance floor naming no rung
//!
//! ## Does not own
//! - The profile object shape and its strictness mapping (see `retention`)
//! - Admission scoring that consumes the resolved snapshot (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to bank-scoped storage or resolution order should normally require changes here.
//! Changes to profile fields or mapping numbers should not require changes here.
//!
//! Keep this module focused on profile storage and resolution.
//!
//! ## Resolution order
//!
//! A bank resolution is a bank override, else the deployment default, else — for a
//! NEW per-repo (`repo:`-prefixed) coding-agent bank with no override and no
//! deployment default — the builtin coding default, else the builtin default. A
//! missing or invalid stored profile never fails admission open or closed: the
//! next level down applies. The coding default never rewrites a stored profile:
//! it applies only where nothing is stored, so it changes nothing that is stored.
//! A stored profile whose version marker is older keeps every value it holds and
//! only has that marker converted; a version 1 profile carrying a mission is
//! left unconverted for the typed reader to refuse (see `load`).

use crate::coding_default::coding_default_profile;
use crate::error::{AmError, ErrorCode};
use crate::log;
use crate::retention::{
    RETENTION_PROFILE_VERSION, RetentionProfile, Verbosity, fail_closed_stored_floor,
};
use crate::retention_mission::log_coding_default_once;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Env var naming the durable retention profile store file.
pub const RETENTION_PROFILES_ENV: &str = "CLIO_RETENTION_PROFILES";

/// Prefix of a per-repository bank id. Banks whose name starts with this prefix
/// are the coding-agent per-repo banks; a `repo:` bank with no stored override
/// and no deployment default is a NEW bank and resolves the coding default.
pub const REPO_BANK_PREFIX: &str = "repo:";

/// Where an effective profile came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSource {
    /// A bank-scoped override.
    Bank,
    /// The deployment default.
    Deployment,
    /// The builtin coding-agent default for a new per-repo bank.
    CodingDefault,
    /// The builtin default profile.
    Builtin,
}

impl ProfileSource {
    /// Wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bank => "bank",
            Self::Deployment => "deployment",
            Self::CodingDefault => "coding_default",
            Self::Builtin => "builtin",
        }
    }
}

/// Bank-scoped retention profiles plus an optional deployment default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionStore {
    /// Deployment-wide default applied to banks without an override.
    #[serde(default)]
    pub deployment: Option<RetentionProfile>,
    /// Per-bank overrides.
    #[serde(default)]
    pub banks: BTreeMap<String, RetentionProfile>,
}

impl RetentionStore {
    /// Resolve the effective profile for a bank plus where it came from.
    ///
    /// Order: bank override → deployment default → coding default (new
    /// `repo:`-prefixed banks only) → builtin default. Applying the coding
    /// default is logged once per bank per process with the profile version.
    #[must_use]
    pub fn effective(&self, bank: &str) -> (RetentionProfile, ProfileSource) {
        if let Some(profile) = self.banks.get(bank) {
            return (profile.clone(), ProfileSource::Bank);
        }
        if let Some(profile) = &self.deployment {
            return (profile.clone(), ProfileSource::Deployment);
        }
        if bank.starts_with(REPO_BANK_PREFIX) {
            log_coding_default_once(bank, coding_default_profile().version);
            return (coding_default_profile(), ProfileSource::CodingDefault);
        }
        (RetentionProfile::default(), ProfileSource::Builtin)
    }

    /// Stored bank override, if any.
    #[must_use]
    pub fn bank_override(&self, bank: &str) -> Option<&RetentionProfile> {
        self.banks.get(bank)
    }

    /// Store a validated bank override.
    ///
    /// # Errors
    /// Empty bank name, or an invalid profile.
    pub fn set_bank(&mut self, bank: &str, profile: RetentionProfile) -> Result<(), AmError> {
        if bank.trim().is_empty() {
            return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
        }
        profile.validate()?;
        self.banks.insert(bank.to_owned(), profile);
        Ok(())
    }

    /// Store a validated deployment default.
    ///
    /// `permissive` is rejected as a deployment default: it MUST NOT silently
    /// loosen admission for every bank without an override.
    ///
    /// # Errors
    /// An invalid profile, or `verbosity=permissive`.
    pub fn set_deployment(&mut self, profile: RetentionProfile) -> Result<(), AmError> {
        profile.validate()?;
        if profile.verbosity == Verbosity::Permissive {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "permissive is not allowed as a deployment default; set it per bank instead",
            ));
        }
        self.deployment = Some(profile);
        Ok(())
    }

    /// Remove a bank override; `true` when one was present.
    pub fn clear_bank(&mut self, bank: &str) -> bool {
        self.banks.remove(bank).is_some()
    }

    /// Load a store file. A missing file yields the empty (builtin-only) store.
    ///
    /// A stored profile carrying a version 1 or 2 marker is converted to the current
    /// version before it is typed, and `rewrite_stored_versions` logs the bank it
    /// belonged to once the typed read succeeds. A version 1 profile carrying a
    /// mission is left unconverted: the typed read keeps its version 1 marker, so
    /// validation refuses it and the lossy read prunes it, exactly as the base
    /// reader did. Only the marker changes: a field the older file omits reads the
    /// documented default, and every field it holds is read exactly as written.
    ///
    /// The rewrite reads only integer markers: a version marker on another JSON
    /// lane, such as the float `1.0` or the string `"1"`, is not converted, so the
    /// typed read refuses the document and the whole store load fails closed.
    ///
    /// A stored provenance floor that names no rung is not an error: it resolves
    /// to the strictest rung and `rewrite_stored_floors` logs the bank it
    /// belonged to, so the degradation is visible rather than silent. A document
    /// that is malformed for any other reason still errors, and the caller still
    /// degrades to the builtin store.
    ///
    /// A later [`RetentionStore::save`] writes the converted marker and the
    /// rewritten floor back to the same file, so the next write after a start-up
    /// replaces the operator's original text for those values on disk; the log
    /// line above is the only notice.
    ///
    /// # Errors
    /// Unreadable file or a JSON document that is not a store object.
    pub fn load(path: &Path) -> Result<Self, AmError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path).map_err(|e| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("failed to read retention profiles {}: {e}", path.display()),
            )
        })?;
        let mut parsed: Value = serde_json::from_str(&text).map_err(|e| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("retention profiles parse error: {e}"),
            )
        })?;
        let conversions = rewrite_stored_versions(&mut parsed);
        rewrite_stored_floors(&mut parsed);
        let store: RetentionStore = serde_json::from_value(parsed).map_err(|e| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("retention profiles parse error: {e}"),
            )
        })?;
        let to = RETENTION_PROFILE_VERSION.to_string();
        for (bank, from) in conversions {
            let from = from.to_string();
            log::info(
                "stored retention profile version converted to the current version",
                &[("bank", bank.as_str()), ("from", &from), ("to", &to)],
            );
        }
        Ok(store)
    }

    /// Load a store file, dropping profiles that fail validation.
    ///
    /// Unreadable or unparsable files degrade to the empty store. Either way the
    /// result is a usable store, never an error: a corrupt stored profile must not
    /// block admission.
    #[must_use]
    pub fn load_lossy(path: &Path) -> Self {
        match Self::load(path) {
            Ok(store) => store.pruned(),
            Err(e) => {
                log::warn(
                    "retention profiles unreadable; using builtin defaults",
                    &[("reason", e.code.as_str())],
                );
                Self::default()
            }
        }
    }

    /// Drop profiles that do not validate, keeping the rest.
    #[must_use]
    pub fn pruned(self) -> Self {
        let deployment = self.deployment.filter(|p| {
            let ok = p.validate().is_ok() && p.verbosity != Verbosity::Permissive;
            if !ok {
                log::warn(
                    "stored deployment retention profile invalid; using builtin default",
                    &[],
                );
            }
            ok
        });
        let mut banks = BTreeMap::new();
        for (bank, profile) in self.banks {
            if profile.validate().is_ok() {
                banks.insert(bank, profile);
            } else {
                log::warn(
                    "stored bank retention profile invalid; using the next level down",
                    &[("bank", bank.as_str())],
                );
            }
        }
        Self { deployment, banks }
    }

    /// Persist the store as pretty JSON, creating parent directories.
    ///
    /// The version each profile holds is what is written, so a store assembled by
    /// [`RetentionStore::load`] persists its converted markers on the next save.
    ///
    /// # Errors
    /// I/O failures creating the parent directory or writing the file.
    ///
    /// # Panics
    /// Panics if the store fails to serialize (it is `Serialize` by construction).
    pub fn save(&self, path: &Path) -> Result<(), AmError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| {
                AmError::new(
                    ErrorCode::Internal,
                    format!("failed to create {}: {e}", parent.display()),
                )
            })?;
        }
        let text = serde_json::to_string_pretty(self).expect("retention store serializes");
        fs::write(path, text).map_err(|e| {
            AmError::new(
                ErrorCode::Internal,
                format!("failed to write retention profiles {}: {e}", path.display()),
            )
        })
    }
}

/// Name under which a stored provenance floor sits in the file.
const FLOOR_KEY: &str = "provenance_floor_default";

/// Name under which a stored profile version sits in the file.
const VERSION_KEY: &str = "version";

/// Name under which a stored mission sits in the file.
const MISSION_KEY: &str = "mission";

/// Bank name reported for the store's deployment default.
const DEPLOYMENT_LABEL: &str = "deployment";

/// Rewrite every stored profile whose version marker is older than the current
/// version to the current version, in place, and return the banks it touched so
/// the caller can log them after the typed read succeeds.
///
/// Versions 1 and 2 differ from the current version only by fields the current
/// reader defaults in: version 2 added `mission`, version 3 added the five
/// defaulted retention fields, and no field changed meaning. A stored profile
/// therefore resolves to exactly the same values whichever of those markers it
/// carries, and stamping the current marker only makes the file say what is
/// already true of every value it holds — except a version 1 profile that
/// carries a mission, which is left alone (see `rewrite_stored_version`).
///
/// The rewrite happens on the raw document, before it is typed as a store, so
/// the typed read that follows never sees an older marker. Any other marker
/// (missing, current, or one this build does not know) is left alone for the
/// existing read or refusal path.
fn rewrite_stored_versions(parsed: &mut Value) -> Vec<(String, u64)> {
    let mut converted = Vec::new();
    if let Some(deployment) = parsed.get_mut("deployment") {
        converted.extend(rewrite_stored_version(DEPLOYMENT_LABEL, deployment));
    }
    if let Some(banks) = parsed.get_mut("banks").and_then(Value::as_object_mut) {
        for (bank, profile) in banks.iter_mut() {
            converted.extend(rewrite_stored_version(bank.as_str(), profile));
        }
    }
    converted
}

/// Rewrite one profile's older version marker; return `(bank, from marker)` when
/// it did.
///
/// Version 2 converts. Version 1 converts only when the profile carries no
/// mission: the mission field was added in version 2, so a version 1 file that
/// holds one is not a shape this build shipped, and stamping it current would
/// activate a mission the base reader refused and pruned.
fn rewrite_stored_version(bank: &str, profile: &mut Value) -> Option<(String, u64)> {
    let raw = profile.get(VERSION_KEY).and_then(Value::as_u64)?;
    let converts = match raw {
        2 => true,
        1 => profile
            .get(MISSION_KEY)
            .is_none_or(serde_json::Value::is_null),
        _ => false,
    };
    if !converts {
        return None;
    }
    if let Some(object) = profile.as_object_mut() {
        object.insert(
            VERSION_KEY.to_owned(),
            Value::from(RETENTION_PROFILE_VERSION),
        );
    }
    Some((bank.to_owned(), raw))
}

/// Rewrite every stored provenance floor that names no rung to the strictest
/// rung, in place, and log one line per bank it touched.
///
/// The rewrite happens on the raw document, before it is typed as a store, so
/// the typed read that follows never sees the unreadable value. A floor that
/// names one of the four rungs, or is absent or `null`, is left alone and says
/// nothing. Rewriting only this one key is what keeps the blast radius to its
/// own profile: every other field, and every other bank in the same file, is
/// still read exactly as written.
fn rewrite_stored_floors(parsed: &mut Value) {
    if let Some(deployment) = parsed.get_mut("deployment") {
        rewrite_stored_floor(DEPLOYMENT_LABEL, deployment);
    }
    if let Some(banks) = parsed.get_mut("banks").and_then(Value::as_object_mut) {
        for (bank, profile) in banks.iter_mut() {
            rewrite_stored_floor(bank.as_str(), profile);
        }
    }
}

/// Rewrite one profile's floor if it names no rung, and say which bank it was.
fn rewrite_stored_floor(bank: &str, profile: &mut Value) {
    let Some(raw) = profile.get(FLOOR_KEY).cloned() else {
        return;
    };
    let Some(fallback) = fail_closed_stored_floor(&raw) else {
        return;
    };
    if let Some(object) = profile.as_object_mut() {
        object.insert(
            FLOOR_KEY.to_owned(),
            Value::String(fallback.as_str().to_owned()),
        );
    }
    log::warn(
        "stored provenance floor names no rung; using the strictest rung",
        &[("bank", bank), ("used", fallback.as_str())],
    );
}

/// Resolve the retention store path: explicit override, else `CLIO_RETENTION_PROFILES`.
#[must_use]
pub fn resolve_retention_path(explicit: Option<PathBuf>) -> Option<PathBuf> {
    resolve_retention_path_with(explicit, || std::env::var_os(RETENTION_PROFILES_ENV))
}

/// Injectable variant of [`resolve_retention_path`] (one codegen path for coverage).
#[must_use]
pub(crate) fn resolve_retention_path_with(
    explicit: Option<PathBuf>,
    env: impl FnOnce() -> Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p);
    }
    env().map(PathBuf::from)
}

#[cfg(test)]
#[path = "retention_store_tests.rs"]
mod retention_store_tests;

#[cfg(test)]
#[path = "retention_store_floor_tests.rs"]
mod retention_store_floor_tests;

#[cfg(test)]
#[path = "retention_store_version_tests.rs"]
mod retention_store_version_tests;
