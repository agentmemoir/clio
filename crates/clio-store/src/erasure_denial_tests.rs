// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the erasure-denial seam: the three-way verdict keeps an
//! unavailable authority distinguishable from a clear one, and the seam is
//! usable as a trait object for every verdict.
//!
//! ## Owns
//! - The in-test double, the refusal rule a caller applies, and the
//!   verdict-distinction assertions
//!
//! ## Does not own
//! - The seam itself (`erasure_denial`)
//! - The production authority and its journal (`clio-compliance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! The seam has no production implementation in this crate, so the double
//! stands in for every implementation: it pins the interface shape (object
//! safety and the refusal rule a caller applies) without pretending to test a
//! driver.

use clio_types::AmError;

use crate::erasure_denial::{DenialVerdict, ErasureDenial, test_salt_fingerprint};

/// A double serving one fixed verdict and one fixed generation.
struct FakeDenial {
    verdict: DenialVerdict,
    generation: u64,
}

impl ErasureDenial for FakeDenial {
    fn verdict(&self, _subject_id: &str) -> DenialVerdict {
        self.verdict
    }

    fn record_denial(&self, _subject_id: &str, _banks: &[String]) -> Result<u64, AmError> {
        Ok(self.generation)
    }

    fn salt_fingerprint(&self) -> String {
        test_salt_fingerprint().to_owned()
    }
}

/// Whether a caller refuses the affected action for `verdict`: an unavailable
/// authority refuses, and a clear one proceeds. A denial is already in force,
/// so it never proceeds as if nothing were recorded.
fn refuses(verdict: DenialVerdict) -> bool {
    !matches!(verdict, DenialVerdict::Clear)
}

#[test]
fn an_unavailable_authority_is_not_conflated_with_a_clear_verdict() {
    assert_ne!(DenialVerdict::AuthorityUnavailable, DenialVerdict::Clear);
    assert_ne!(DenialVerdict::Denied, DenialVerdict::Clear);
    assert!(refuses(DenialVerdict::AuthorityUnavailable));
    assert!(!refuses(DenialVerdict::Clear));
}

#[test]
fn the_seam_serves_every_verdict_through_a_trait_object() {
    for verdict in [
        DenialVerdict::Denied,
        DenialVerdict::Clear,
        DenialVerdict::AuthorityUnavailable,
    ] {
        let probe = FakeDenial {
            verdict,
            generation: 7,
        };
        let seam: &dyn ErasureDenial = &probe;
        assert_eq!(seam.verdict("subject-a"), verdict);
        assert_eq!(seam.salt_fingerprint(), test_salt_fingerprint());
        assert_eq!(
            seam.record_denial("subject-a", &["bank-a".to_owned()])
                .expect("the double records"),
            7,
            "the durable proof value crosses the seam"
        );
    }
}
