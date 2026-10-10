// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-only `stats` reporter: memory
//! counts by bank, tier, category, age band, and provenance class, plus
//! caller-supplied hygiene pressure counts, for operational telemetry.
//!
//! ## Owns
//! - `StatsReport` shape (`bank`, tier counts, per-category counts, age-band
//!   counts, provenance-class counts, pressure counts)
//! - `StatsInputs`: the caller-resolved half-life, clock, and pressure values
//! - Band-edge arithmetic over store-canonical UTC stamps
//! - Bank-scoped counting over the existing inspector predicates
//!
//! ## Does not own
//! - Resolving the half-life, the clock, or the pressure counts (the caller
//!   resolves them; this module reads no configuration and no clock)
//! - The sources the pressure counts come from
//! - Any mutation, cleanup, or erasure (read-only by contract)
//! - Backend SQL (`clio-store` owns the count primitives)
//!
//! ## Boundary
//!
//! Changes to the report shape should normally require changes here. Changes
//! to counting SQL, half-life resolution, or pressure sources should not
//! require changes here.
//!
//! Keep this module focused on the stats reporter.

use clio_store::{InspectSource, InspectValidity, ItemInspectFilter};
use clio_types::{
    AmError, ErrorCode, SemanticCategory, civil_from_days, days_from_civil, days_in_month,
};
use serde::Serialize;

/// One count bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Count {
    /// Bucket label.
    pub label: String,
    /// Row count.
    pub count: usize,
}

/// Bank-scoped memory counts for operational telemetry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatsReport {
    /// Bank the counts cover.
    pub bank: String,
    /// Every item row in the bank, including operationally discarded rows.
    pub items_total: usize,
    /// Currently active rows (`tx_until IS NULL`).
    pub items_active: usize,
    /// Operationally discarded rows.
    pub items_discarded: usize,
    /// Counts by tier: the semantic-versus-episodic split (semantic is
    /// category-scoped, episodic is the remainder). The word "tier" means
    /// nothing else on this surface.
    pub by_tier: Vec<Count>,
    /// Counts by semantic category (whitelist), active rows.
    pub by_category: Vec<Count>,
    /// Active rows by epistemic kind.
    pub by_epistemic_kind: Vec<Count>,
    /// Active rows by age band, in half-lives of age at the resolved unit.
    /// Each label states its span and its boundary in days.
    pub by_age_band: Vec<Count>,
    /// Active rows by provenance class: the stored `source_type` values plus
    /// the fact rung (stored NULL, labelled `fact`, never "unknown").
    pub by_provenance_class: Vec<Count>,
    /// Caller-supplied hygiene pressure counts. A `None` value is omitted
    /// rather than reported as zero; counts only, never ids or content.
    pub pressure_counts: Vec<Count>,
}

/// Hygiene pressure counts, relayed verbatim from the caller's sources.
///
/// Every field is optional: a `None` is omitted from the report instead of
/// being reported as a fabricated zero.
#[derive(Debug, Clone, Default)]
pub struct PressureCounts {
    /// Relayed as the `flagged` pressure count.
    pub flagged: Option<u64>,
    /// Relayed as the `stale summaries` pressure count.
    pub stale_summaries: Option<u64>,
    /// Relayed as the `archive candidates` pressure count.
    pub archive_candidates: Option<u64>,
}

/// Caller-resolved inputs for one `stats` run.
///
/// The half-life, the clock, and the pressure counts are resolved by the
/// caller; this module reads no configuration and no clock.
#[derive(Debug, Clone)]
pub struct StatsInputs {
    /// Resolved half-life in days; one age-band step.
    pub band_unit_days: f64,
    /// RFC 3339 UTC instant (`YYYY-MM-DDThh:mm:ssZ`) used for all age
    /// computation.
    pub now: String,
    /// Hygiene pressure counts to relay into the report; `None` leaves the
    /// pressure list empty.
    pub pressure: Option<PressureCounts>,
}

/// Count helper for a filter predicate.
fn count(store: &dyn clio_store::Store, filter: &ItemInspectFilter) -> Result<usize, AmError> {
    store.count_items(filter)
}

/// One band edge: `now` shifted back by `units` half-lives.
///
/// Edges are whole seconds (stored stamps have whole-second precision), so a
/// fractional-day half-life truncates the shift down to the second.
///
/// # Errors
/// `now` is not a UTC RFC 3339 stamp.
fn band_edge(now: &str, unit_days: f64, units: f64) -> Result<String, AmError> {
    let base = parse_utc_stamp(now)?;
    // The shift is explicitly truncated to a whole second, so the cast loses
    // no fractional part; an out-of-range shift saturates as before.
    #[allow(clippy::cast_possible_truncation)]
    let shift = (units * unit_days * 86_400.0).trunc() as i64;
    Ok(format_utc_stamp(base - shift))
}

/// Whole-second Unix-epoch seconds parsed from a UTC RFC 3339 stamp
/// (`YYYY-MM-DDThh:mm:ssZ`, an optional fractional-second tail is dropped).
///
/// # Errors
/// Not a UTC RFC 3339 stamp, or a field out of calendar range.
fn parse_utc_stamp(stamp: &str) -> Result<i64, AmError> {
    const BAD: &str = "now must be a UTC RFC 3339 stamp (YYYY-MM-DDThh:mm:ssZ)";
    let rest = stamp
        .strip_suffix('Z')
        .or_else(|| stamp.strip_suffix("+00:00"))
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, BAD))?;
    let rest = match rest.split_once('.') {
        Some((head, _fraction)) => head,
        None => rest,
    };
    let bytes = rest.as_bytes();
    let shape_ok = rest.len() == 19
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':';
    if !shape_ok {
        return Err(AmError::new(ErrorCode::InvalidArgument, BAD));
    }
    let field = |range: &str| -> Result<i64, AmError> {
        range
            .parse::<i64>()
            .map_err(|_| AmError::new(ErrorCode::InvalidArgument, BAD))
    };
    let (year, month) = (field(&rest[0..4])?, field(&rest[5..7])?);
    let (day, hour) = (field(&rest[8..10])?, field(&rest[11..13])?);
    let (minute, second) = (field(&rest[14..16])?, field(&rest[17..19])?);
    let month = u32::try_from(month).unwrap_or(0);
    let day = u32::try_from(day).unwrap_or(0);
    let calendar_ok = (1..=12).contains(&month)
        && day >= 1
        && day <= days_in_month(year, month)
        && (0..=23).contains(&hour)
        && (0..=59).contains(&minute)
        && (0..=59).contains(&second);
    if !calendar_ok {
        return Err(AmError::new(ErrorCode::InvalidArgument, BAD));
    }
    let secs_of_day = hour * 3_600 + minute * 60 + second;
    Ok(days_from_civil(year, month, day) * 86_400 + secs_of_day)
}

/// Store-canonical `YYYY-MM-DDThh:mm:ssZ` stamp from whole-second epoch
/// seconds.
fn format_utc_stamp(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// One bank-scoped filter with an explicit validity.
fn bank_filter(
    bank_id: &str,
    validity: InspectValidity,
    include_discarded: bool,
) -> ItemInspectFilter {
    ItemInspectFilter {
        bank_id: bank_id.to_owned(),
        validity,
        include_discarded,
        ..ItemInspectFilter::new(bank_id)
    }
}

/// Item totals for one bank: every row, active rows, discarded rows.
fn count_bank_totals(
    store: &dyn clio_store::Store,
    bank_id: &str,
) -> Result<(usize, usize, usize), AmError> {
    let total = bank_filter(bank_id, InspectValidity::All, true);
    let active = bank_filter(bank_id, InspectValidity::Active, false);
    let discarded = bank_filter(bank_id, InspectValidity::Discarded, true);
    Ok((
        count(store, &total)?,
        count(store, &active)?,
        count(store, &discarded)?,
    ))
}

/// Active rows by age band, in half-lives of age at the resolved unit.
///
/// The four bands partition `created_at` space, so their counts sum to the
/// active total with no gap and no overlap. Each edge is inclusive on the
/// older side and exclusive on the younger side, so an item created exactly
/// on an edge counts in the younger band.
fn count_age_bands(
    store: &dyn clio_store::Store,
    active: &ItemInspectFilter,
    unit: f64,
    now: &str,
) -> Result<Vec<Count>, AmError> {
    let edge_1 = band_edge(now, unit, 1.0)?;
    let edge_2 = band_edge(now, unit, 2.0)?;
    let edge_4 = band_edge(now, unit, 4.0)?;
    let mut by_age_band = Vec::with_capacity(4);
    for (label, time_start, time_end) in [
        (
            format!("0–1 half-life (0–{unit} days)"),
            Some(edge_1.clone()),
            None,
        ),
        (
            format!("1–2 half-lives ({unit}–{} days)", 2.0 * unit),
            Some(edge_2.clone()),
            Some(edge_1),
        ),
        (
            format!("2–4 half-lives ({}–{} days)", 2.0 * unit, 4.0 * unit),
            Some(edge_4.clone()),
            Some(edge_2),
        ),
        (
            format!("4+ half-lives ({}+ days)", 4.0 * unit),
            None,
            Some(edge_4),
        ),
    ] {
        let mut filter = active.clone();
        filter.time_start = time_start;
        filter.time_end = time_end;
        by_age_band.push(Count {
            label,
            count: count(store, &filter)?,
        });
    }
    Ok(by_age_band)
}

/// Active rows by provenance class: the stored `source_type` values plus the
/// fact rung, which is exactly the `source_type IS NULL` population compiled
/// by clio-store. Item validation keeps facts and the three belief rungs a
/// partition of every row, so the counts sum to the active total.
fn count_provenance_classes(
    store: &dyn clio_store::Store,
    active: &ItemInspectFilter,
) -> Result<Vec<Count>, AmError> {
    let mut by_provenance_class = Vec::with_capacity(4);
    for (label, rung) in [
        ("user_stated", InspectSource::UserStated),
        ("agent_inferred", InspectSource::AgentInferred),
        ("third_party", InspectSource::ThirdParty),
        ("fact", InspectSource::Fact),
    ] {
        let mut filter = active.clone();
        filter.source_type = Some(rung);
        by_provenance_class.push(Count {
            label: label.to_owned(),
            count: count(store, &filter)?,
        });
    }
    Ok(by_provenance_class)
}

/// The pressure values are the caller's; this module only relays them.
/// A `None` value is omitted rather than reported as a fabricated zero.
fn relay_pressure_counts(inputs: &StatsInputs) -> Vec<Count> {
    let mut pressure_counts = Vec::new();
    if let Some(pressure) = &inputs.pressure {
        for (label, value) in [
            ("flagged", pressure.flagged),
            ("stale summaries", pressure.stale_summaries),
            ("archive candidates", pressure.archive_candidates),
        ] {
            if let Some(value) = value {
                pressure_counts.push(Count {
                    label: label.to_owned(),
                    count: usize::try_from(value).unwrap_or(usize::MAX),
                });
            }
        }
    }
    pressure_counts
}

/// Active rows by semantic category (whitelist), with their active total.
fn count_categories(
    store: &dyn clio_store::Store,
    bank_id: &str,
) -> Result<(Vec<Count>, usize), AmError> {
    let mut by_category = Vec::new();
    let mut semantic_active = 0usize;
    for &category in SemanticCategory::WIRE {
        let mut filter = bank_filter(bank_id, InspectValidity::Active, false);
        filter.category = Some(category.to_owned());
        let n = count(store, &filter)?;
        by_category.push(Count {
            label: category.to_owned(),
            count: n,
        });
        semantic_active += n;
    }
    Ok((by_category, semantic_active))
}

/// Active rows by epistemic kind.
fn count_epistemic_kinds(
    store: &dyn clio_store::Store,
    bank_id: &str,
) -> Result<Vec<Count>, AmError> {
    let mut by_epistemic_kind = Vec::new();
    for kind in ["fact", "belief"] {
        let mut filter = bank_filter(bank_id, InspectValidity::Active, false);
        filter.epistemic_kind = Some(kind.to_owned());
        let n = count(store, &filter)?;
        by_epistemic_kind.push(Count {
            label: kind.to_owned(),
            count: n,
        });
    }
    Ok(by_epistemic_kind)
}

/// Build the report for one bank (read-only; never mutates).
///
/// # Errors
/// Storage failures, or an `inputs.now` that is not a UTC RFC 3339 stamp.
pub fn stats(
    store: &dyn clio_store::Store,
    bank_id: &str,
    inputs: &StatsInputs,
) -> Result<StatsReport, AmError> {
    if bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    let (items_total, items_active, items_discarded) = count_bank_totals(store, bank_id)?;
    let active = bank_filter(bank_id, InspectValidity::Active, false);
    let by_age_band = count_age_bands(store, &active, inputs.band_unit_days, &inputs.now)?;
    let by_provenance_class = count_provenance_classes(store, &active)?;
    let pressure_counts = relay_pressure_counts(inputs);
    let (by_category, semantic_active) = count_categories(store, bank_id)?;
    let by_epistemic_kind = count_epistemic_kinds(store, bank_id)?;
    let episodic_active = items_active.saturating_sub(semantic_active);
    Ok(StatsReport {
        bank: bank_id.to_owned(),
        items_total,
        items_active,
        items_discarded,
        by_tier: vec![
            Count {
                label: "semantic".to_owned(),
                count: semantic_active,
            },
            Count {
                label: "episodic".to_owned(),
                count: episodic_active,
            },
        ],
        by_category,
        by_epistemic_kind,
        by_age_band,
        by_provenance_class,
        pressure_counts,
    })
}

#[cfg(test)]
#[path = "stats_tests.rs"]
mod stats_tests;

#[cfg(test)]
#[path = "stats_stamp_tests.rs"]
mod stats_stamp_tests;

#[cfg(test)]
#[path = "stats_types_tests.rs"]
mod stats_types_tests;
