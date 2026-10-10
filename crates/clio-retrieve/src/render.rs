// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bounded rendering of a retrieval hit's
//! stored snapshot: one function, one published cap, no truncation.
//!
//! ## Owns
//! - [`EXACT_RENDER_MAX_BYTES`], the cap every rendering is measured against
//! - [`render_exact`], the single producer of the rendering text
//!
//! ## Does not own
//! - The hit field contract and its serde shape (`clio-types` owns
//!   `RetrieveHit`)
//! - Whether a request asked for a rendering (`RetrieveRequest::include_exact`)
//! - Attaching the rendering to a hit (`finalize` owns hit assembly)
//!
//! ## Boundary
//!
//! Changes to the cap, the cap's derivation, or the rendering rule should
//! normally require changes here. Changes to hit assembly or to whether a
//! request asked for the rendering should not require changes here.
//!
//! # Deriving the cap
//!
//! The cap is measured in **bytes** of the serialized rendering, checked with
//! `String::len`, so a multi-byte character counts for the bytes it occupies.
//!
//! The snapshot shape that motivated it is the hub-distiller's own projection:
//! `unit_id`, `hub_id`, `neighbor_ids`, `neighbor_count`, and `stale_reason`
//! only when the unit is stale. Compact JSON of the four always-present keys,
//! with every id of one length `n`, renders to `86 + 10n` bytes: the ten ids'
//! characters (one `unit_id`, one `hub_id`, eight `neighbor_ids`) plus the
//! fixed key names, quotes, separators, and the `8`.
//!
//! | Id length | Rendering bytes |
//! |-----------|-----------------|
//! | 13        | 216             |
//! | 20        | 286             |
//! | 30        | 386             |
//! | 42        | 506             |
//! | 43        | 516             |
//!
//! So 512 bytes holds a first-distillation hub snapshot at the default eight
//! neighbours whenever every id in it — hub, unit, and neighbour — is 42
//! characters or fewer, and it does not hold a 43-character one. A stale unit
//! adds `"stale_reason":` plus the reason text, so a stale hub is bounded by
//! the same cap rather than by a second rule.
//!
//! That 42-character headroom is not provisional: it has been checked against a
//! measured distribution over the producers this repository ships. The hub
//! distiller at the default eight neighbours spans 13 fixtures from 176 to 616
//! bytes, median 318, and 11 of the 13 fit the cap; its whole bank is 14
//! fixtures from 136 to 616 bytes, median 273, and 12 fit. The template
//! extractor is unbounded, so its 12 fixtures run from 2 to 656 bytes, median
//! 111, and 11 fit.
//!
//! Neither producer's median is above the cap, so the common case is a
//! rendering rather than a reference: the channel is not inert, and the cap
//! stays at 512. The tail above it is by design rather than a defect. Real item
//! ids are caller-supplied (`item_id_prefix` plus `-0`), so no id length is
//! guaranteed, and a snapshot whose rendering passes the cap yields no
//! rendering at all; the pack then falls back to the hit's gist or to a marked
//! reference, so a reader can tell that fallback from content.
//!
//! # Why bytes bound tokens without any counting code
//!
//! The budget surfaces estimate tokens as `estimate_tokens`, which is
//! `chars.div_ceil(4)` over Unicode scalar values. Every UTF-8 character
//! occupies at least one byte, so a rendering of at most
//! [`EXACT_RENDER_MAX_BYTES`] bytes has at most that many characters and
//! therefore at most `EXACT_RENDER_MAX_BYTES / 4` tokens — 128 here. That
//! figure is an **upper bound** and never an underestimate, so it needs no code
//! path of its own and cannot drift from the estimator. It is reached by any
//! rendering of 509 to 512 characters, pure ASCII or multi-byte alike, because
//! the cap counts bytes while the estimator counts characters; only a rendering
//! of fewer than 509 characters lands below it. That is why the check is one
//! `len()` comparison and not a token estimate.
//!
//! # Determinism
//!
//! The same snapshot value renders to the same bytes for a fixed build.
//! `serde_json::Map` is a `BTreeMap` here, because nothing in the resolved
//! dependency graph enables the `preserve_order` feature, so object keys
//! always serialize in sorted order. Nothing else in this path reads a clock,
//! a random source, an environment variable, or an iteration order.
//!
//! Keep this module focused on one bounded rendering of one snapshot.

use serde_json::Value;

/// The cap on one rendering, in bytes of the serialized text.
///
/// See the module documentation for the derivation: it covers a
/// first-distillation hub snapshot at the default eight neighbours while every
/// id in it is 42 characters or fewer, and its token consequence is at most
/// [`EXACT_RENDER_MAX_BYTES`] / 4 = 128 tokens under `estimate_tokens`, an
/// upper bound that any rendering of 509 to 512 characters reaches.
///
/// A snapshot whose rendering is longer than this yields no rendering. The cap
/// is never met by shortening a rendering.
pub const EXACT_RENDER_MAX_BYTES: usize = 512;

/// Render one snapshot as the exact text for a hit's `exact_render` field.
///
/// Returns `None` when the whole rendering does not fit within
/// [`EXACT_RENDER_MAX_BYTES`] bytes. A caller that gets `None` has no rendering
/// at all; there is deliberately no prefix, no char-boundary cut, and no
/// ellipsis, because a shortened snapshot would read as the stored value.
///
/// The output is deterministic: for a fixed build the same snapshot value
/// serializes to the same bytes, because object keys iterate in sorted order.
///
/// Serialization of a `Value` can only fail for the two reasons `serde_json`
/// documents: a `Serialize` implementation that chooses to fail, or a map with
/// non-string keys. `Value::Object` is a `Map<String, Value>`, so neither can
/// occur here, and `None` is returned rather than `expect`ed so that a future
/// change cannot turn an unexpected failure into a panic on a read path.
#[must_use]
pub fn render_exact(snapshot: &Value) -> Option<String> {
    let rendered = serde_json::to_string(snapshot).ok()?;
    // `len` is bytes, which is the documented unit of the cap, so the check
    // needs no token estimate and no truncation path.
    (rendered.len() <= EXACT_RENDER_MAX_BYTES).then_some(rendered)
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod render_tests;
