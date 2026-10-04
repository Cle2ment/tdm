//! `tdm-conformance` — the reusable conformance battery every
//! [`DecisionProvider`] must pass (plan §5: all providers run the same
//! conformance suite — the backbone of replaceability).
//!
//! The battery is transport-agnostic: it drives a provider purely through the
//! trait surface ([`DecisionProvider::health`], [`DecisionProvider::capabilities`],
//! [`DecisionProvider::judge`]) and records one [`CaseResult`] per case, so
//! failures read as replaceability defects rather than harness artifacts.
//!
//! # Cases
//!
//! 1. `health_ok` — `health()` reports `ok = true`.
//! 2. `capabilities_advertised` — non-empty advertised primitives, and the
//!    batch flag is consistent with behavior. Documented batch semantics: a
//!    provider with `batch = false` may *either* answer a multi-question
//!    request (e.g. by degrading to sequential judging) *or* reject it with
//!    an explicit [`TdmError`] — both are compliant. A provider with
//!    `batch = true` must answer the multi-question request.
//! 3. `noul_in_range` — noul probability in `0.0..=1.0`, confidence `None`.
//! 4. `choice_structure` — label ∈ options; distribution keys exactly the
//!    option set; probabilities sum to `1.0 ± 0.01`; confidence `Some` in
//!    `0.0..=1.0`.
//! 5. `score_structure` — level ∈ levels; `weighted` within
//!    `0.0..=len(levels)-1` (the 0-based level-index scale); distribution
//!    covers all levels and sums to `1.0 ± 0.01`.
//! 6. `ids_echoed_in_order` — a 3-question batch (one per primitive) is
//!    answered with the same ids in the same order.
//! 7. `usage_present` — token usage is present on a successful judgment.
//! 8. `provider_meta_stable` — `result.provider.id` equals
//!    [`DecisionProvider::id`].
//! 9. `invalid_question_rejected` — a question violating
//!    [`Question::validate`] is rejected with [`TdmError::Client`], never a
//!    panic or an acceptance.
//! 10. `deterministic_state_isolation` — two calls with different states
//!     echo only their own request ids; no answer identity leaks across
//!     calls.
//!
//! Cases 3–6 are primitive-scoped: a primitive the provider does not
//! advertise is skipped (recorded as passed with a `skipped:` detail), so
//! the battery stays reusable for partial providers. Both providers shipped
//! in this workspace advertise all three primitives and execute every case.
//!
//! # Examples
//!
//! ```ignore
//! use tdm_conformance::assert_compliant;
//! use tdm_provider_mock::MockProvider;
//!
//! assert_compliant(&MockProvider::new()).await;
//! ```

use std::collections::BTreeSet;

use tdm_core::{
    AnswerValue, DecisionProvider, DecisionRequest, Primitive, PrimitiveKind, Question, TdmError,
};

/// Question id of the noul structure case (`noul_in_range`); stable so
/// fixture builders can pre-seed canned answers against it.
pub const NOUL_CASE_ID: &str = "conf-noul-1";
/// Question id of the choice structure case (`choice_structure`).
pub const CHOICE_CASE_ID: &str = "conf-choice-1";
/// Question id of the score structure case (`score_structure`).
pub const SCORE_CASE_ID: &str = "conf-score-1";

/// Result of a single conformance case.
#[derive(Debug, Clone)]
pub struct CaseResult {
    /// Stable case name (e.g. `"noul_in_range"`).
    pub name: &'static str,
    /// Whether the case passed.
    pub passed: bool,
    /// Human-readable evidence: why it failed, or what was observed.
    pub detail: Option<String>,
}

impl CaseResult {
    fn pass(name: &'static str, detail: Option<String>) -> Self {
        Self {
            name,
            passed: true,
            detail,
        }
    }

    fn fail(name: &'static str, detail: String) -> Self {
        Self {
            name,
            passed: false,
            detail: Some(detail),
        }
    }
}

/// Full outcome of the battery: one [`CaseResult`] per case, in battery
/// order.
#[derive(Debug, Clone)]
pub struct ConformanceReport {
    /// Per-case results.
    pub cases: Vec<CaseResult>,
}

/// Runs the full battery against any [`DecisionProvider`].
pub async fn run(provider: &dyn DecisionProvider) -> ConformanceReport {
    ConformanceReport {
        cases: vec![
            case_health_ok(provider).await,
            case_capabilities_advertised(provider).await,
            case_noul_in_range(provider).await,
            case_choice_structure(provider).await,
            case_score_structure(provider).await,
            case_ids_echoed_in_order(provider).await,
            case_usage_present(provider).await,
            case_provider_meta_stable(provider).await,
            case_invalid_question_rejected(provider).await,
            case_deterministic_state_isolation(provider).await,
        ],
    }
}

/// Runs the full battery and panics with a readable report unless every case
/// passed.
///
/// # Panics
/// If any case fails; the panic message lists every failing case with its
/// detail.
pub async fn assert_compliant(provider: &dyn DecisionProvider) {
    let report = run(provider).await;
    if report.cases.iter().all(|case| case.passed) {
        return;
    }
    let mut message = String::from("provider failed the conformance battery:\n");
    for case in report.cases.iter().filter(|case| !case.passed) {
        message.push_str(&format!("  FAIL {}\n", case.name));
        if let Some(detail) = &case.detail {
            message.push_str(&format!("        {detail}\n"));
        }
    }
    panic!("{message}");
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A sample primitive of `kind`, sized for the probes (3 options / 3 levels).
fn sample_primitive(kind: PrimitiveKind) -> Primitive {
    match kind {
        PrimitiveKind::Choice => Primitive::Choice {
            options: vec!["alpha".into(), "beta".into(), "gamma".into()],
        },
        PrimitiveKind::Noul => Primitive::Noul,
        PrimitiveKind::Score => Primitive::Score {
            levels: vec!["low".into(), "medium".into(), "high".into()],
        },
    }
}

fn question(id: &str, primitive: Primitive) -> Question {
    Question {
        id: id.to_owned(),
        instructions: format!("conformance probe {id}"),
        primitive,
    }
}

/// A single-question request over `state = {"conformance": true}`.
fn single_request(id: &str, primitive: Primitive) -> DecisionRequest {
    DecisionRequest {
        state: serde_json::json!({ "conformance": true }),
        questions: vec![question(id, primitive)],
    }
}

fn advertises(provider: &dyn DecisionProvider, kind: PrimitiveKind) -> bool {
    provider.capabilities().primitives.contains(&kind)
}

/// The first advertised primitive, for primitive-agnostic cases.
fn first_primitive(provider: &dyn DecisionProvider) -> Option<PrimitiveKind> {
    provider.capabilities().primitives.first().copied()
}

fn distribution_keys(distribution: &[(String, f64)]) -> BTreeSet<&str> {
    distribution
        .iter()
        .map(|(label, _)| label.as_str())
        .collect()
}

fn distribution_sum(distribution: &[(String, f64)]) -> f64 {
    distribution
        .iter()
        .map(|(_, probability)| probability)
        .sum()
}

fn sums_to_one(distribution: &[(String, f64)]) -> bool {
    (0.99..=1.01).contains(&distribution_sum(distribution))
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// 1. `health_ok`.
async fn case_health_ok(provider: &dyn DecisionProvider) -> CaseResult {
    let report = provider.health().await;
    if report.ok {
        CaseResult::pass(
            "health_ok",
            Some(format!("ok=true, latency_ms={}", report.latency_ms)),
        )
    } else {
        CaseResult::fail(
            "health_ok",
            format!("health() reported ok=false: {:?}", report.detail),
        )
    }
}

/// 2. `capabilities_advertised` — non-empty primitives, batch flag consistent
///    with the two-question probe (see the crate docs for the `batch = false`
///    contract).
async fn case_capabilities_advertised(provider: &dyn DecisionProvider) -> CaseResult {
    const NAME: &str = "capabilities_advertised";
    let caps = provider.capabilities();
    let Some(kind) = caps.primitives.first().copied() else {
        return CaseResult::fail(NAME, "capabilities().primitives is empty".into());
    };
    let request = DecisionRequest {
        state: serde_json::json!({ "probe": "capabilities" }),
        questions: vec![
            question("conf-caps-a", sample_primitive(kind)),
            question("conf-caps-b", sample_primitive(kind)),
        ],
    };
    match provider.judge(request).await {
        Ok(result) => {
            if result.answers.len() != 2 {
                return CaseResult::fail(
                    NAME,
                    format!(
                        "multi-question request returned {} answers, expected 2",
                        result.answers.len()
                    ),
                );
            }
            if caps.batch {
                CaseResult::pass(NAME, Some("batch=true: 2-question batch answered".into()))
            } else {
                CaseResult::pass(
                    NAME,
                    Some(
                        "batch=false: multi-question request answered (sequential degradation)"
                            .into(),
                    ),
                )
            }
        }
        Err(error) if caps.batch => CaseResult::fail(
            NAME,
            format!("batch=true but the 2-question batch failed: {error}"),
        ),
        // Documented compliant alternative for `batch = false` providers: an
        // explicit typed rejection.
        Err(error) => CaseResult::pass(
            NAME,
            Some(format!(
                "batch=false: rejected the multi-question batch explicitly: {error}"
            )),
        ),
    }
}

/// 3. `noul_in_range`.
async fn case_noul_in_range(provider: &dyn DecisionProvider) -> CaseResult {
    const NAME: &str = "noul_in_range";
    if !advertises(provider, PrimitiveKind::Noul) {
        return CaseResult::pass(NAME, Some("skipped: noul is not advertised".into()));
    }
    let result = match provider
        .judge(single_request(NOUL_CASE_ID, Primitive::Noul))
        .await
    {
        Ok(result) => result,
        Err(error) => return CaseResult::fail(NAME, format!("judge failed: {error}")),
    };
    if result.answers.len() != 1 {
        return CaseResult::fail(
            NAME,
            format!("expected exactly 1 answer, got {}", result.answers.len()),
        );
    }
    let answer = &result.answers[0];
    let AnswerValue::Noul { probability } = &answer.value else {
        return CaseResult::fail(
            NAME,
            format!("expected a noul answer, got {:?}", answer.value),
        );
    };
    if !(0.0..=1.0).contains(probability) {
        return CaseResult::fail(
            NAME,
            format!("noul probability {probability} outside 0.0..=1.0"),
        );
    }
    if answer.confidence.is_some() {
        return CaseResult::fail(
            NAME,
            format!("noul must carry no confidence, got {:?}", answer.confidence),
        );
    }
    CaseResult::pass(
        NAME,
        Some(format!("probability={probability:.3}, confidence=None")),
    )
}

/// 4. `choice_structure`.
async fn case_choice_structure(provider: &dyn DecisionProvider) -> CaseResult {
    const NAME: &str = "choice_structure";
    if !advertises(provider, PrimitiveKind::Choice) {
        return CaseResult::pass(NAME, Some("skipped: choice is not advertised".into()));
    }
    let options = vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()];
    let result = match provider
        .judge(single_request(
            CHOICE_CASE_ID,
            Primitive::Choice {
                options: options.clone(),
            },
        ))
        .await
    {
        Ok(result) => result,
        Err(error) => return CaseResult::fail(NAME, format!("judge failed: {error}")),
    };
    if result.answers.len() != 1 {
        return CaseResult::fail(
            NAME,
            format!("expected exactly 1 answer, got {}", result.answers.len()),
        );
    }
    let answer = &result.answers[0];
    let AnswerValue::Choice {
        label,
        distribution,
    } = &answer.value
    else {
        return CaseResult::fail(
            NAME,
            format!("expected a choice answer, got {:?}", answer.value),
        );
    };
    if !options.iter().any(|option| option == label) {
        return CaseResult::fail(
            NAME,
            format!("chosen label {label:?} is not one of the options"),
        );
    }
    let keys = distribution_keys(distribution);
    let expected: BTreeSet<&str> = options.iter().map(String::as_str).collect();
    if keys != expected {
        return CaseResult::fail(
            NAME,
            format!("distribution keys {keys:?} do not exactly match the options {expected:?}"),
        );
    }
    if !sums_to_one(distribution) {
        return CaseResult::fail(
            NAME,
            format!(
                "distribution sums to {}, expected 1.0 ± 0.01",
                distribution_sum(distribution)
            ),
        );
    }
    let Some(confidence) = answer.confidence else {
        return CaseResult::fail(
            NAME,
            "choice answer must carry Some confidence, got None".into(),
        );
    };
    if !(0.0..=1.0).contains(&confidence) {
        return CaseResult::fail(NAME, format!("confidence {confidence} outside 0.0..=1.0"));
    }
    CaseResult::pass(
        NAME,
        Some(format!("label={label:?}, confidence={confidence}")),
    )
}

/// 5. `score_structure`.
async fn case_score_structure(provider: &dyn DecisionProvider) -> CaseResult {
    const NAME: &str = "score_structure";
    if !advertises(provider, PrimitiveKind::Score) {
        return CaseResult::pass(NAME, Some("skipped: score is not advertised".into()));
    }
    let levels = vec!["low".to_owned(), "medium".to_owned(), "high".to_owned()];
    let result = match provider
        .judge(single_request(
            SCORE_CASE_ID,
            Primitive::Score {
                levels: levels.clone(),
            },
        ))
        .await
    {
        Ok(result) => result,
        Err(error) => return CaseResult::fail(NAME, format!("judge failed: {error}")),
    };
    if result.answers.len() != 1 {
        return CaseResult::fail(
            NAME,
            format!("expected exactly 1 answer, got {}", result.answers.len()),
        );
    }
    let answer = &result.answers[0];
    let AnswerValue::Score {
        level,
        weighted,
        distribution,
    } = &answer.value
    else {
        return CaseResult::fail(
            NAME,
            format!("expected a score answer, got {:?}", answer.value),
        );
    };
    if !levels.iter().any(|candidate| candidate == level) {
        return CaseResult::fail(
            NAME,
            format!("chosen level {level:?} is not one of the levels"),
        );
    }
    let max_weighted = (levels.len() - 1) as f64;
    if !(0.0..=max_weighted).contains(weighted) {
        return CaseResult::fail(
            NAME,
            format!("weighted {weighted} outside 0.0..={max_weighted} (0-based level-index scale)"),
        );
    }
    let covered = distribution_keys(distribution);
    for expected in &levels {
        if !covered.contains(expected.as_str()) {
            return CaseResult::fail(
                NAME,
                format!("distribution does not cover level {expected:?}"),
            );
        }
    }
    if !sums_to_one(distribution) {
        return CaseResult::fail(
            NAME,
            format!(
                "distribution sums to {}, expected 1.0 ± 0.01",
                distribution_sum(distribution)
            ),
        );
    }
    CaseResult::pass(NAME, Some(format!("level={level:?}, weighted={weighted}")))
}

/// 6. `ids_echoed_in_order`.
async fn case_ids_echoed_in_order(provider: &dyn DecisionProvider) -> CaseResult {
    const NAME: &str = "ids_echoed_in_order";
    let all_primitives = [
        PrimitiveKind::Choice,
        PrimitiveKind::Noul,
        PrimitiveKind::Score,
    ];
    if !all_primitives
        .iter()
        .all(|kind| advertises(provider, *kind))
    {
        return CaseResult::pass(
            NAME,
            Some("skipped: this case needs all three primitives advertised".into()),
        );
    }
    let ids = ["conf-echo-choice", "conf-echo-noul", "conf-echo-score"];
    let request = DecisionRequest {
        state: serde_json::json!({ "conformance": "echo" }),
        questions: vec![
            question(ids[0], sample_primitive(PrimitiveKind::Choice)),
            question(ids[1], sample_primitive(PrimitiveKind::Noul)),
            question(ids[2], sample_primitive(PrimitiveKind::Score)),
        ],
    };
    let result = match provider.judge(request).await {
        Ok(result) => result,
        Err(error) => return CaseResult::fail(NAME, format!("judge failed: {error}")),
    };
    let echoed: Vec<&str> = result
        .answers
        .iter()
        .map(|answer| answer.id.as_str())
        .collect();
    if echoed != ids {
        return CaseResult::fail(
            NAME,
            format!("answers echoed ids {echoed:?}, expected {ids:?} in the same order"),
        );
    }
    CaseResult::pass(NAME, Some(format!("echoed {ids:?} in order")))
}

/// 7. `usage_present` — usage fields are `u64` by type, so existence means a
///    successful judgment; the detail records the observed values.
async fn case_usage_present(provider: &dyn DecisionProvider) -> CaseResult {
    const NAME: &str = "usage_present";
    let Some(kind) = first_primitive(provider) else {
        return CaseResult::fail(NAME, "no advertised primitive to judge with".into());
    };
    match provider
        .judge(single_request("conf-usage", sample_primitive(kind)))
        .await
    {
        Ok(result) => CaseResult::pass(
            NAME,
            Some(format!(
                "usage present: input_tokens={}, output_tokens={}",
                result.usage.input_tokens, result.usage.output_tokens
            )),
        ),
        Err(error) => CaseResult::fail(NAME, format!("judge failed: {error}")),
    }
}

/// 8. `provider_meta_stable`.
async fn case_provider_meta_stable(provider: &dyn DecisionProvider) -> CaseResult {
    const NAME: &str = "provider_meta_stable";
    let Some(kind) = first_primitive(provider) else {
        return CaseResult::fail(NAME, "no advertised primitive to judge with".into());
    };
    match provider
        .judge(single_request("conf-meta", sample_primitive(kind)))
        .await
    {
        Ok(result) => {
            if result.provider.id == provider.id() {
                CaseResult::pass(
                    NAME,
                    Some(format!("result.provider.id == {:?}", provider.id())),
                )
            } else {
                CaseResult::fail(
                    NAME,
                    format!(
                        "result.provider.id = {:?}, but provider.id() = {:?}",
                        result.provider.id,
                        provider.id()
                    ),
                )
            }
        }
        Err(error) => CaseResult::fail(NAME, format!("judge failed: {error}")),
    }
}

/// 9. `invalid_question_rejected`.
async fn case_invalid_question_rejected(provider: &dyn DecisionProvider) -> CaseResult {
    const NAME: &str = "invalid_question_rejected";
    let Some(kind) = first_primitive(provider) else {
        return CaseResult::fail(
            NAME,
            "no advertised primitive to build an invalid question from".into(),
        );
    };
    let request = DecisionRequest {
        state: serde_json::json!({}),
        questions: vec![Question {
            id: "conf-invalid".into(),
            // The one violation that works for every primitive: empty
            // instructions fail Question::validate() unconditionally.
            instructions: String::new(),
            primitive: sample_primitive(kind),
        }],
    };
    match provider.judge(request).await {
        Ok(_) => CaseResult::fail(
            NAME,
            "provider accepted a question that fails Question::validate()".into(),
        ),
        Err(error @ TdmError::Client { status, .. }) => CaseResult::pass(
            NAME,
            Some(format!("rejected with Client (status {status}): {error}")),
        ),
        Err(other) => CaseResult::fail(NAME, format!("expected TdmError::Client, got {other}")),
    }
}

/// 10. `deterministic_state_isolation`.
async fn case_deterministic_state_isolation(provider: &dyn DecisionProvider) -> CaseResult {
    const NAME: &str = "deterministic_state_isolation";
    let Some(kind) = first_primitive(provider) else {
        return CaseResult::fail(NAME, "no advertised primitive to judge with".into());
    };
    let call_a = provider
        .judge(DecisionRequest {
            state: serde_json::json!({ "isolation": "A" }),
            questions: vec![
                question("conf-iso-a1", sample_primitive(kind)),
                question("conf-iso-a2", sample_primitive(kind)),
            ],
        })
        .await;
    let call_b = provider
        .judge(DecisionRequest {
            state: serde_json::json!({ "isolation": "B" }),
            questions: vec![
                question("conf-iso-b1", sample_primitive(kind)),
                question("conf-iso-b2", sample_primitive(kind)),
            ],
        })
        .await;

    let (a, b) = match (call_a, call_b) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(error), _) | (_, Err(error)) => {
            return CaseResult::fail(NAME, format!("judge failed: {error}"));
        }
    };
    let ids_a: Vec<&str> = a.answers.iter().map(|answer| answer.id.as_str()).collect();
    let ids_b: Vec<&str> = b.answers.iter().map(|answer| answer.id.as_str()).collect();
    if ids_a != ["conf-iso-a1", "conf-iso-a2"] {
        return CaseResult::fail(
            NAME,
            format!(
                "first call echoed {ids_a:?}; answers must reference only their own request ids"
            ),
        );
    }
    if ids_b != ["conf-iso-b1", "conf-iso-b2"] {
        return CaseResult::fail(
            NAME,
            format!(
                "second call echoed {ids_b:?}; answers must reference only their own request ids"
            ),
        );
    }
    CaseResult::pass(
        NAME,
        Some(format!(
            "distinct states, disjoint ids: call A echoed {ids_a:?}, call B echoed {ids_b:?}"
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tdm_core::{Answer, Capabilities, DecisionResult, HealthReport, ProviderMeta, Usage};

    /// Minimal hand-rolled provider emitting textbook-valid answers, used to
    /// validate the battery itself from both sides.
    struct FakeProvider {
        id: &'static str,
        reported_meta_id: &'static str,
        capabilities: Capabilities,
    }

    impl FakeProvider {
        fn conforming() -> Self {
            Self {
                id: "fake",
                reported_meta_id: "fake",
                capabilities: Capabilities {
                    primitives: vec![
                        PrimitiveKind::Choice,
                        PrimitiveKind::Noul,
                        PrimitiveKind::Score,
                    ],
                    batch: true,
                    max_state_bytes: 1 << 20,
                },
            }
        }

        /// Judge results carry a provider meta id that differs from `id()`.
        fn with_meta_mismatch() -> Self {
            Self {
                reported_meta_id: "someone-else",
                ..Self::conforming()
            }
        }

        fn answer_for(&self, question: &Question) -> Answer {
            let value = match &question.primitive {
                Primitive::Noul => AnswerValue::Noul { probability: 0.75 },
                Primitive::Choice { options } => AnswerValue::Choice {
                    label: options[0].clone(),
                    distribution: options
                        .iter()
                        .map(|option| (option.clone(), 1.0 / options.len() as f64))
                        .collect(),
                },
                Primitive::Score { levels } => AnswerValue::Score {
                    level: levels[0].clone(),
                    weighted: 0.0,
                    distribution: levels
                        .iter()
                        .map(|level| (level.clone(), 1.0 / levels.len() as f64))
                        .collect(),
                },
            };
            let confidence = (!matches!(value, AnswerValue::Noul { .. })).then_some(0.5);
            Answer {
                id: question.id.clone(),
                value,
                confidence,
            }
        }
    }

    #[async_trait]
    impl DecisionProvider for FakeProvider {
        fn id(&self) -> &str {
            self.id
        }

        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        async fn judge(&self, req: DecisionRequest) -> Result<DecisionResult, TdmError> {
            for question in &req.questions {
                question.validate().map_err(|error| TdmError::Client {
                    status: 422,
                    code: None,
                    message: error.to_string(),
                })?;
            }
            let answers = req.questions.iter().map(|q| self.answer_for(q)).collect();
            Ok(DecisionResult {
                answers,
                usage: Usage {
                    input_tokens: 11,
                    output_tokens: 7,
                },
                provider: ProviderMeta {
                    id: self.reported_meta_id.to_owned(),
                    model: Some("fake-1".into()),
                },
                latency_ms: 0,
            })
        }

        async fn health(&self) -> HealthReport {
            HealthReport {
                ok: true,
                latency_ms: 0,
                detail: None,
                version: Some("fake-1".into()),
            }
        }
    }

    #[tokio::test]
    async fn conforming_fake_provider_passes_every_case() {
        let report = run(&FakeProvider::conforming()).await;
        let failures: Vec<&CaseResult> = report.cases.iter().filter(|case| !case.passed).collect();
        assert!(failures.is_empty(), "unexpected failures: {failures:?}");
        assert_eq!(report.cases.len(), 10);
    }

    #[tokio::test]
    async fn battery_detects_provider_meta_mismatch() {
        let report = run(&FakeProvider::with_meta_mismatch()).await;
        let failed: Vec<&str> = report
            .cases
            .iter()
            .filter(|case| !case.passed)
            .map(|case| case.name)
            .collect();
        assert_eq!(failed, ["provider_meta_stable"]);
    }
}
