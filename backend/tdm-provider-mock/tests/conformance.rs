//! Conformance battery for the deterministic mock provider.
//!
//! Runs the full reusable battery (`tdm-conformance`) against
//! [`MockProvider`] — both in pure deterministic mode and with a canned
//! answer override in place.

use std::collections::HashMap;

use tdm_conformance::{NOUL_CASE_ID, assert_compliant};
use tdm_core::AnswerValue;
use tdm_provider_mock::MockProvider;

#[tokio::test]
async fn deterministic_mock_is_fully_conformant() {
    assert_compliant(&MockProvider::new()).await;
}

/// A canned answer for the battery's stable noul question id takes
/// precedence over deterministic synthesis; the provider must stay
/// conformant regardless.
#[tokio::test]
async fn canned_mock_is_fully_conformant() {
    let canned = HashMap::from([(
        NOUL_CASE_ID.to_owned(),
        AnswerValue::Noul { probability: 0.62 },
    )]);
    assert_compliant(&MockProvider::new().with_answers(canned)).await;
}
