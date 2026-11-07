// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the `--discovery` flag: the
//! values it accepts, the values it refuses, and the absent flag leaving the
//! configured setting to decide.
//!
//! ## Owns
//! - The accepted `on` and `off` values
//! - The usage refusal for every other value
//!
//! ## Does not own
//! - Which tools discovery adds (see `clio-mcp`)
//! - Rendering the refusal, or opening a store (see `mcp_cli`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the flag's values.
