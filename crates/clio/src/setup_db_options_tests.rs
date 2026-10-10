// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the advanced-setting table: that it holds the thirteen
//! settings the plan names, that no name repeats, that every one carries help,
//! and that a value is checked against what the driver accepts.
//!
//! ## Owns
//! - The table's size, names, and non-empty help
//! - The two caveats the plan says must be stated
//! - Per-setting acceptance and refusal
//!
//! ## Does not own
//! - The table itself (see `setup_db_options`)
//! - The address the settings end up in (see `setup_plan_db`)
//!
//! ## Boundary
//!
//! Tests here break when the table changes shape; they never change the
//! implementation.
//!
//! Keep this module focused on the table's contents and its checks.

use super::*;
use crate::setup_validate::check_advanced;

#[test]
fn the_table_holds_thirteen_settings() {
    assert_eq!(ADVANCED.len(), 13, "the plan names thirteen settings");
}

#[test]
fn no_setting_name_repeats() {
    let mut names: Vec<&str> = ADVANCED.iter().map(|setting| setting.name).collect();
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), before, "a repeated name would be asked twice");
}

#[test]
fn every_setting_carries_help() {
    for setting in ADVANCED {
        assert!(
            !setting.help.trim().is_empty(),
            "{} has no help",
            setting.name
        );
    }
}

#[test]
fn the_plan_names_every_setting_in_the_table() {
    for name in [
        "options",
        "application_name",
        "connect_timeout",
        "target_session_attrs",
        "channel_binding",
        "sslnegotiation",
        "keepalives",
        "keepalives_idle",
        "keepalives_interval",
        "keepalives_retries",
        "tcp_user_timeout",
        "hostaddr",
        "load_balance_hosts",
    ] {
        assert!(
            advanced(name).is_ok(),
            "{name} is named in the plan and must be in the table"
        );
    }
}

#[test]
fn an_unknown_name_names_itself_in_the_refusal() {
    let error = advanced("schema_path").expect_err("not a connection setting");
    assert!(error.contains("schema_path"), "{error}");
}

#[test]
fn a_free_text_setting_accepts_anything() {
    assert!(
        advanced("options")
            .expect("a setting")
            .check("-c search_path=app")
            .is_ok()
    );
}

#[test]
fn a_free_text_setting_still_rejects_nothing() {
    assert!(
        advanced("application_name")
            .expect("a setting")
            .check("clio setup")
            .is_ok()
    );
}

#[test]
fn a_millisecond_setting_refuses_a_non_number() {
    let error = advanced("tcp_user_timeout")
        .expect("a setting")
        .check("soon")
        .expect_err("milliseconds are a number");
    assert!(error.contains("milliseconds"), "{error}");
}

#[test]
fn sslnegotiation_accepts_direct_and_postgres() {
    let setting = advanced("sslnegotiation").expect("a setting");
    assert!(setting.check("direct").is_ok());
    assert!(setting.check("postgres").is_ok());
    assert!(setting.check("fast").is_err());
}

#[test]
fn load_balance_hosts_accepts_only_disable_and_random() {
    let setting = advanced("load_balance_hosts").expect("a setting");
    assert!(setting.check("random").is_ok());
    assert!(setting.check("disable").is_ok());
    assert!(setting.check("round-robin").is_err());
}

#[test]
fn a_value_is_trimmed_before_it_is_checked() {
    assert!(check_advanced("target_session_attrs", "  read-only  ").is_ok());
}
