//! Deterministic mock [`DecisionProvider`] — the backbone of unit tests, CI,
//! and the M1 conformance suite.
//!
//! Answers are a pure function of `(question id, instructions, primitive)`
//! derived with a stable non-cryptographic FNV-1a hash: no network, no RNG, no
//! wall clock. The same request always yields the exact same
//! [`DecisionResult`].
//!
//! # Deterministic synthesis
//!
//! With `h = FNV-1a64(id bytes ++ 0x00 ++ instructions bytes)`:
//!
//! - [`Primitive::Noul`]: `probability = 0.05 + (h % 91) / 100.0`, i.e. uniform
//!   over the 91 steps in `[0.05, 0.95]`. Confidence is `None` (noul never
//!   carries confidence).
//! - [`Primitive::Choice`]: winner index `= h % options.len()`. Distribution:
//!   the winner gets `0.7`, every other option shares the remaining `0.3`
//!   uniformly (a degenerate single-option choice gets `P = 1.0`). Confidence
//!   is the distribution concentration: winner probability minus the uniform
//!   share of the rest (`0.7 - 0.3 / (n - 1)`; `1.0` for a single option).
//! - [`Primitive::Score`]: analogous over `levels` (winner `0.7`, the rest
//!   share `0.3`; levels are validated as `2..=10`, so there is no degenerate
//!   case). `weighted` is the probability-weighted 0-based level index.
//!   Confidence uses the same concentration formula.
//!
//! Usage is synthetic: `input_tokens = len(json(state ++ questions)) / 4`,
//! `output_tokens = 16`, `latency_ms = 0`. Provider meta is `id = "mock"`,
//! `model = Some("mock-1")`.
//!
//! # Answer precedence
//!
//! `judge` resolves each batch in this order:
//!
//! 1. Error mode ([`MockProvider::with_error`]): *every* `judge` returns that
//!    exact [`TdmError`] — for retry / circuit-breaker tests — before anything
//!    else is consulted. `health` is unaffected.
//! 2. Request validation ([`Question::validate`]): any violation aborts the
//!    whole batch with [`TdmError::Client`] status 422, mirroring the jev
//!    provider convention.
//! 3. Canned answers ([`MockProvider::with_answers`]): an entry keyed by
//!    question id is returned verbatim; its confidence is derived from the
//!    distribution concentration (`None` for noul).
//! 4. Deterministic synthesis for every other question.
//!
//! # Examples
//!
//! ```
//! use tdm_core::TdmError;
//! use tdm_provider_mock::MockProvider;
//!
//! // Deterministic answers for everything...
//! let provider = MockProvider::new();
//! // ...or canned overrides by question id...
//! let canned = MockProvider::new().with_answers(std::collections::HashMap::new());
//! // ...or a hard error for retry / circuit tests.
//! let broken = MockProvider::new().with_error(TdmError::Network {
//!     message: "down".into(),
//! });
//! # let _ = (provider, canned, broken);
//! ```

use std::collections::HashMap;

use async_trait::async_trait;
use tdm_core::{
    Answer, AnswerValue, Capabilities, DecisionProvider, DecisionRequest, DecisionResult,
    HealthReport, Primitive, PrimitiveKind, ProviderMeta, Question, TdmError, Usage,
};

/// Probability assigned to the deterministic distribution winner.
const WINNER_PROBABILITY: f64 = 0.7;
/// Probability mass shared uniformly by the non-winner entries.
const REST_MASS: f64 = 0.3;

/// Deterministic mock [`DecisionProvider`]. See the crate docs for the exact
/// synthesis mapping and answer precedence.
#[derive(Debug, Clone)]
pub struct MockProvider {
    canned: HashMap<String, AnswerValue>,
    error: Option<TdmError>,
    capabilities: Capabilities,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MockProvider {
    /// A provider whose answers are a pure function of the request.
    #[must_use]
    pub fn new() -> Self {
        Self {
            canned: HashMap::new(),
            error: None,
            capabilities: Capabilities {
                primitives: vec![
                    PrimitiveKind::Choice,
                    PrimitiveKind::Noul,
                    PrimitiveKind::Score,
                ],
                batch: true,
                max_state_bytes: 16 * 1024 * 1024,
            },
        }
    }

    /// Canned answers keyed by question id; they take precedence over
    /// deterministic synthesis (error mode still wins over both).
    #[must_use]
    pub fn with_answers(mut self, answers: HashMap<String, AnswerValue>) -> Self {
        self.canned = answers;
        self
    }

    /// Error mode: every `judge` returns exactly this error. `health` is
    /// unaffected.
    #[must_use]
    pub fn with_error(mut self, error: TdmError) -> Self {
        self.error = Some(error);
        self
    }

    /// One answer for `question`: canned override, else deterministic synthesis.
    fn answer_for(&self, question: &Question) -> Answer {
        if let Some(value) = self.canned.get(&question.id) {
            return Answer {
                id: question.id.clone(),
                value: value.clone(),
                confidence: confidence_of(value),
            };
        }

        let hash = hash_question(question);
        let value = match &question.primitive {
            Primitive::Noul => AnswerValue::Noul {
                probability: 0.05 + (hash % 91) as f64 / 100.0,
            },
            Primitive::Choice { options } => {
                let winner = (hash % options.len() as u64) as usize;
                AnswerValue::Choice {
                    label: options[winner].clone(),
                    distribution: distribution_over(options, winner),
                }
            }
            Primitive::Score { levels } => {
                let winner = (hash % levels.len() as u64) as usize;
                let distribution = distribution_over(levels, winner);
                let weighted = distribution
                    .iter()
                    .enumerate()
                    .map(|(i, (_, p))| i as f64 * p)
                    .sum();
                AnswerValue::Score {
                    level: levels[winner].clone(),
                    weighted,
                    distribution,
                }
            }
        };

        Answer {
            id: question.id.clone(),
            confidence: confidence_of(&value),
            value,
        }
    }
}

#[async_trait]
impl DecisionProvider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn judge(&self, req: DecisionRequest) -> Result<DecisionResult, TdmError> {
        // 1. Error mode simulates a failing provider before anything else.
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        // 2. Request-shape validation aborts the whole batch (422, like jev).
        for question in &req.questions {
            if let Err(validation) = question.validate() {
                return Err(TdmError::Client {
                    status: 422,
                    code: Some("validation".into()),
                    message: format!("question {}: {validation}", question.id),
                });
            }
        }

        let answers: Vec<Answer> = req.questions.iter().map(|q| self.answer_for(q)).collect();
        let payload = serde_json::to_string(&(&req.state, &req.questions)).unwrap_or_default();

        Ok(DecisionResult {
            answers,
            usage: Usage {
                input_tokens: (payload.len() / 4) as u64,
                output_tokens: 16,
            },
            provider: ProviderMeta {
                id: "mock".into(),
                model: Some("mock-1".into()),
            },
            latency_ms: 0,
        })
    }

    async fn health(&self) -> HealthReport {
        HealthReport {
            ok: true,
            latency_ms: 0,
            detail: None,
            version: Some("mock-1".into()),
        }
    }
}

/// FNV-1a 64-bit hash: stable, non-cryptographic, dependency-free.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Hash seed for a question: `id`, a NUL separator, then `instructions`. The
/// separator keeps `(id, instructions)` pairs unambiguous.
fn hash_question(question: &Question) -> u64 {
    let mut bytes = Vec::with_capacity(question.id.len() + question.instructions.len() + 1);
    bytes.extend_from_slice(question.id.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(question.instructions.as_bytes());
    fnv1a64(&bytes)
}

/// Distribution over `labels` with `labels[winner]` at
/// [`WINNER_PROBABILITY`] and the rest sharing [`REST_MASS`] uniformly. A
/// single label degenerates to `P = 1.0`.
fn distribution_over(labels: &[String], winner: usize) -> Vec<(String, f64)> {
    if labels.len() == 1 {
        return vec![(labels[0].clone(), 1.0)];
    }
    let rest = REST_MASS / (labels.len() - 1) as f64;
    labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let probability = if i == winner {
                WINNER_PROBABILITY
            } else {
                rest
            };
            (label.clone(), probability)
        })
        .collect()
}

/// Distribution concentration used as confidence: winner probability minus the
/// uniform share of the remaining mass. Degenerate single entry → `1.0`.
fn concentration(distribution: &[(String, f64)]) -> Option<f64> {
    match distribution.len() {
        0 => None,
        1 => Some(1.0),
        n => {
            let max = distribution
                .iter()
                .map(|(_, p)| *p)
                .fold(f64::NEG_INFINITY, f64::max);
            Some(max - (1.0 - max) / (n - 1) as f64)
        }
    }
}

/// Confidence for an answer value: `None` for noul, else the concentration of
/// its distribution.
fn confidence_of(value: &AnswerValue) -> Option<f64> {
    match value {
        AnswerValue::Noul { .. } => None,
        AnswerValue::Choice { distribution, .. } | AnswerValue::Score { distribution, .. } => {
            concentration(distribution)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(id: &str, primitive: Primitive) -> Question {
        Question {
            id: id.into(),
            instructions: format!("judge {id}"),
            primitive,
        }
    }

    fn noul_question(id: &str) -> Question {
        question(id, Primitive::Noul)
    }

    fn choice_question(id: &str, options: &[&str]) -> Question {
        question(
            id,
            Primitive::Choice {
                options: options.iter().map(|s| s.to_string()).collect(),
            },
        )
    }

    fn score_question(id: &str, levels: &[&str]) -> Question {
        question(
            id,
            Primitive::Score {
                levels: levels.iter().map(|s| s.to_string()).collect(),
            },
        )
    }

    fn empty_request(questions: Vec<Question>) -> DecisionRequest {
        DecisionRequest {
            state: serde_json::json!(null),
            questions,
        }
    }

    #[tokio::test]
    async fn judge_is_deterministic() {
        let provider = MockProvider::new();
        let req = DecisionRequest {
            state: serde_json::json!({ "repo": "tdm", "files_changed": 3 }),
            questions: vec![
                choice_question("c1", &["a", "b", "c"]),
                noul_question("n1"),
                score_question("s1", &["low", "mid", "high"]),
            ],
        };

        let first = provider.clone().judge(req.clone()).await.unwrap();
        let second = provider.judge(req).await.unwrap();
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn all_primitives_produce_in_range_values() {
        let provider = MockProvider::new();

        for i in 0..64u32 {
            let id = format!("q{i}");
            let request = |q: Question| DecisionRequest {
                state: serde_json::json!({ "i": i }),
                questions: vec![q],
            };

            let noul = provider.judge(request(noul_question(&id))).await.unwrap();
            let AnswerValue::Noul { probability } = &noul.answers[0].value else {
                panic!("expected noul answer, got {:?}", noul.answers[0].value);
            };
            assert!(
                (0.05..=0.95).contains(probability),
                "noul probability out of range: {probability}"
            );
            assert_eq!(noul.answers[0].confidence, None);

            let choice = provider
                .judge(request(choice_question(&id, &["a", "b", "c", "d"])))
                .await
                .unwrap();
            let AnswerValue::Choice {
                label,
                distribution,
            } = &choice.answers[0].value
            else {
                panic!("expected choice answer");
            };
            assert!(distribution.iter().any(|(option, _)| option == label));
            let sum: f64 = distribution.iter().map(|(_, p)| p).sum();
            assert!(
                (sum - 1.0).abs() < 1e-9,
                "choice distribution sums to {sum}"
            );
            let winner = distribution
                .iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert_eq!(winner.0, *label);
            assert!((winner.1 - WINNER_PROBABILITY).abs() < 1e-9);

            let score = provider
                .judge(request(score_question(
                    &id,
                    &["l0", "l1", "l2", "l3", "l4"],
                )))
                .await
                .unwrap();
            let AnswerValue::Score {
                level,
                weighted,
                distribution,
            } = &score.answers[0].value
            else {
                panic!("expected score answer");
            };
            assert!(distribution.iter().any(|(candidate, _)| candidate == level));
            let sum: f64 = distribution.iter().map(|(_, p)| p).sum();
            assert!((sum - 1.0).abs() < 1e-9, "score distribution sums to {sum}");
            let expected_weighted: f64 = distribution
                .iter()
                .enumerate()
                .map(|(i, (_, p))| i as f64 * p)
                .sum();
            assert!(
                (weighted - expected_weighted).abs() < 1e-12,
                "weighted {weighted} does not match distribution"
            );
            assert!((0.0..=4.0).contains(weighted));
        }
    }

    #[tokio::test]
    async fn single_option_choice_degenerates_to_certainty() {
        let provider = MockProvider::new();
        let result = provider
            .judge(empty_request(vec![choice_question("one", &["only"])]))
            .await
            .unwrap();

        let AnswerValue::Choice {
            label,
            distribution,
        } = &result.answers[0].value
        else {
            panic!("expected choice answer");
        };
        assert_eq!(label, "only");
        assert_eq!(distribution.len(), 1);
        assert_eq!(distribution[0].0, "only");
        assert_eq!(distribution[0].1, 1.0);
        assert_eq!(result.answers[0].confidence, Some(1.0));
    }

    #[tokio::test]
    async fn canned_answers_override_and_unknown_ids_fall_back() {
        let provider = MockProvider::new().with_answers(HashMap::from([(
            "known".to_string(),
            AnswerValue::Noul { probability: 0.42 },
        )]));

        let result = provider
            .judge(empty_request(vec![
                noul_question("known"),
                noul_question("unknown"),
            ]))
            .await
            .unwrap();

        assert_eq!(
            result.answers[0].value,
            AnswerValue::Noul { probability: 0.42 }
        );
        assert_eq!(result.answers[0].confidence, None);

        // Unknown ids fall back to the same answer pure synthesis would give.
        let bare = MockProvider::new();
        let fallback = bare
            .judge(empty_request(vec![noul_question("unknown")]))
            .await
            .unwrap();
        assert_eq!(result.answers[1].value, fallback.answers[0].value);
        assert_eq!(result.answers[1].confidence, fallback.answers[0].confidence);
    }

    #[tokio::test]
    async fn error_mode_returns_exact_error_and_health_is_unaffected() {
        let error = TdmError::Server {
            status: 503,
            message: "synthetic outage".into(),
        };
        // Error mode wins even over a canned answer.
        let provider = MockProvider::new()
            .with_answers(HashMap::from([(
                "q".to_string(),
                AnswerValue::Noul { probability: 0.5 },
            )]))
            .with_error(error.clone());

        let err = provider
            .judge(empty_request(vec![noul_question("q")]))
            .await
            .unwrap_err();
        assert_eq!(err, error);

        let health = provider.health().await;
        assert!(health.ok);
        assert_eq!(health.latency_ms, 0);
        assert_eq!(health.version.as_deref(), Some("mock-1"));
    }

    #[tokio::test]
    async fn ids_are_echoed_in_request_order() {
        let provider = MockProvider::new();
        let ids = ["alpha", "beta", "gamma", "delta"];
        let questions: Vec<Question> = ids.iter().map(|id| noul_question(id)).collect();

        let result = provider.judge(empty_request(questions)).await.unwrap();

        let echoed: Vec<&str> = result.answers.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(echoed, ids);
    }

    #[tokio::test]
    async fn invalid_questions_fail_with_client_422() {
        let provider = MockProvider::new();
        let cases = vec![
            choice_question("bad-options", &[]),
            score_question("bad-levels", &["l0"]),
            Question {
                id: "bad-instructions".into(),
                instructions: String::new(),
                primitive: Primitive::Noul,
            },
        ];

        for q in cases {
            let err = provider
                .judge(empty_request(vec![q.clone()]))
                .await
                .unwrap_err();
            match err {
                TdmError::Client {
                    status,
                    code,
                    message,
                } => {
                    assert_eq!(status, 422);
                    assert_eq!(code.as_deref(), Some("validation"));
                    assert!(message.contains(&q.id), "message should name the question");
                }
                other => panic!("expected Client error for {}, got {other:?}", q.id),
            }
        }

        // A single invalid question aborts the whole batch.
        let err = provider
            .judge(empty_request(vec![
                noul_question("fine"),
                score_question("broken", &["l0"]),
            ]))
            .await
            .unwrap_err();
        assert!(
            matches!(err, TdmError::Client { status: 422, .. }),
            "expected 422, got {err:?}"
        );
    }

    #[tokio::test]
    async fn capabilities_meta_and_usage() {
        let provider = MockProvider::new();

        assert_eq!(provider.id(), "mock");
        let caps = provider.capabilities();
        assert_eq!(
            caps.primitives,
            vec![
                PrimitiveKind::Choice,
                PrimitiveKind::Noul,
                PrimitiveKind::Score,
            ]
        );
        assert!(caps.batch);
        assert_eq!(caps.max_state_bytes, 16 * 1024 * 1024);

        let state = serde_json::json!({ "k": "v" });
        let questions = vec![noul_question("q")];
        let expected_input = serde_json::to_string(&(&state, &questions)).unwrap().len() as u64 / 4;

        let result = provider
            .judge(DecisionRequest { state, questions })
            .await
            .unwrap();

        assert_eq!(
            result.provider,
            ProviderMeta {
                id: "mock".into(),
                model: Some("mock-1".into()),
            }
        );
        assert_eq!(result.latency_ms, 0);
        assert_eq!(result.usage.output_tokens, 16);
        assert_eq!(result.usage.input_tokens, expected_input);
    }
}
