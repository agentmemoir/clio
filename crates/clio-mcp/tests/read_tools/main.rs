// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the entry point of the pressure-measurement
//! target: open each backend, build the synthetic fixture, and run the common
//! measurement over it.
//!
//! ## Owns
//! - The target's module declarations
//! - The SQLite test that always runs and the Postgres test that returns early
//!   when the shared database URL is not a Postgres URL
//!
//! ## Does not own
//! - The fixture contents (`fixture`)
//! - The timing method and the p95 ceilings (`measure`)
//! - The trigger measurements (`measure_trigger`)
//!
//! ## Boundary
//!
//! Changes to which backends are measured should normally require changes here.
//! Changes to the fixture or the timing method should not require changes here.
//!
//! Keep this module focused on the target's entry point and backend selection.

#[path = "fixture.rs"]
mod fixture;
#[path = "measure.rs"]
mod measure;
#[path = "measure_trigger.rs"]
mod measure_trigger;

/// The pressure read is measured on SQLite in memory, always.
#[test]
fn pressure_measure_sqlite_latency_is_measured_against_a_generous_bound() {
    let state = fixture::open_sqlite();
    let fixture = fixture::build(&state);
    measure::run("sqlite", &fixture, &measure::Bounds::sqlite());
    measure_trigger::run(
        "sqlite",
        &fixture,
        &measure_trigger::TriggerBounds::sqlite(),
    );
}

/// The same measurement over the shared Postgres server, when one is offered
/// through `CLIO_DATABASE_URL`. A missing URL fails closed unless the operator
/// opted out; the gate prints its own note, which never prints the URL itself.
#[test]
fn pressure_measure_postgres_latency_is_measured_against_a_generous_bound() {
    let Some(database_url) = fixture::postgres_url() else {
        return;
    };
    // The shared database is store-global; run under the process-wide lock and
    // inside a private schema so parallel Postgres suites cannot see these rows.
    let _guard = clio_store::lock_pg_test();
    let schema = clio_store::ScopedSchema::new(&database_url);
    let state = fixture::open_postgres(schema.url());
    let fixture = fixture::build(&state);
    measure::run("postgres", &fixture, &measure::Bounds::postgres());
    measure_trigger::run(
        "postgres",
        &fixture,
        &measure_trigger::TriggerBounds::postgres(),
    );
}
