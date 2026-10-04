//! The jev-backed [`DecisionProvider`] implementation.

use std::{collections::BTreeMap, time::Instant};

use async_trait::async_trait;
use tdm_core::{
    Answer, Capabilities, DecisionProvider, DecisionRequest, DecisionResult, HealthReport,
    PrimitiveKind, ProviderMeta, TdmError, Usage,
};

use crate::{
    config::JevConfig,
    wire::{WireQuestion, WireRequest, core_answer, wire_question},
};

/// 422 machine-readable code per the jev error contract.
const INVALID_REQUEST_CODE: &str = "invalid_request";
/// Response-body snippet budget inside error messages.
const BODY_SNIPPET_CHARS: usize = 500;

/// TypeSafe System One (jev) provider adapter.
///
/// One attempt per call — retry/backoff/circuit-breaking belongs to the
/// runtime (M1). [`Debug`] never leaks the API key (see [`JevConfig`]).
#[derive(Debug, Clone)]
pub struct JevProvider {
    config: JevConfig,
    http: reqwest::Client,
    capabilities: Capabilities,
}

impl JevProvider {
    /// Builds a provider from an explicit config.
    ///
    /// # Panics
    /// Only if the reqwest client backend fails to initialize, which in
    /// practice means a broken TLS installation.
    pub fn new(config: JevConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("reqwest client must build with valid configuration");
        Self {
            config,
            http,
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

    /// Sends one wire request and returns the parsed response plus the
    /// measured HTTP latency (request start -> body fully received).
    ///
    /// # Errors
    /// [`TdmError`] per the taxonomy documented at crate level.
    async fn send(&self, body: WireRequest) -> Result<(crate::wire::WireResponse, u64), TdmError> {
        let started = Instant::now();
        let response = self
            .http
            .post(&self.config.endpoint)
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| TdmError::Network {
                message: format!("jev request failed: {error}"),
            })?;
        let status = response.status();
        let text = response.text().await.map_err(|error| TdmError::Network {
            message: format!("jev body read failed: {error}"),
        })?;
        let latency_ms = started.elapsed().as_millis() as u64;
        if !status.is_success() {
            return Err(status_error(status.as_u16(), &text));
        }
        let parsed: crate::wire::WireResponse = serde_json::from_str(&text).map_err(|error| {
            // Deliberate mapping: an HTTP-success body that fails to parse
            // is the provider breaking its contract, not a caller mistake.
            // `Client` stays reserved for 4xx statuses; this is `Quality`.
            TdmError::Quality {
                detail: format!("jev returned an unparseable response body: {error}"),
            }
        })?;
        Ok((parsed, latency_ms))
    }
}

/// Maps a non-2xx HTTP status plus body onto the [`TdmError`] taxonomy.
fn status_error(status: u16, body: &str) -> TdmError {
    let message = format!(
        "jev responded {status}: {}",
        truncate_body(body, BODY_SNIPPET_CHARS)
    );
    if status == 429 || status >= 500 {
        TdmError::Server { status, message }
    } else if (400..500).contains(&status) {
        TdmError::Client {
            status,
            code: (status == 422).then(|| INVALID_REQUEST_CODE.to_owned()),
            message,
        }
    } else {
        // 1xx/3xx after redirect resolution: the endpoint is not behaving
        // like System One — provider contract violation.
        TdmError::Quality {
            detail: format!("jev responded with unexpected status {status}: {message}"),
        }
    }
}

/// Truncates a response body to `max_chars` characters (boundary-safe).
fn truncate_body(body: &str, max_chars: usize) -> String {
    if body.chars().count() <= max_chars {
        return body.to_owned();
    }
    let mut snippet: String = body.chars().take(max_chars).collect();
    snippet.push('…');
    snippet
}

#[async_trait]
impl DecisionProvider for JevProvider {
    fn id(&self) -> &str {
        "jev"
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn judge(&self, req: DecisionRequest) -> Result<DecisionResult, TdmError> {
        let DecisionRequest { state, questions } = req;

        // Validate everything locally before anything goes on the wire.
        for question in &questions {
            if let Err(error) = question.validate() {
                return Err(TdmError::Client {
                    status: 422,
                    code: Some(INVALID_REQUEST_CODE.to_owned()),
                    message: format!("question {:?}: {error}", question.id),
                });
            }
        }
        // The wire contract keys answers by question id, so duplicates would
        // make answers unattributable — reject instead of silently merging.
        let mut wire_questions: BTreeMap<String, WireQuestion> = BTreeMap::new();
        for question in &questions {
            if wire_questions
                .insert(question.id.clone(), wire_question(question))
                .is_some()
            {
                return Err(TdmError::Client {
                    status: 422,
                    code: Some(INVALID_REQUEST_CODE.to_owned()),
                    message: format!("duplicate question id {:?}", question.id),
                });
            }
        }

        tracing::debug!(
            provider = "jev",
            questions = questions.len(),
            "judging batch"
        );
        let (wire, latency_ms) = self
            .send(WireRequest {
                state,
                model: self.config.model.clone(),
                questions: wire_questions,
            })
            .await?;

        // One answer per request question, in the same order.
        let mut answers = Vec::with_capacity(questions.len());
        for question in &questions {
            let wire_answer = wire
                .answers
                .get(&question.id)
                .ok_or_else(|| TdmError::Quality {
                    detail: format!(
                        "jev response is missing an answer for question {:?}",
                        question.id
                    ),
                })?;
            let (value, confidence) = core_answer(wire_answer)?;
            answers.push(Answer {
                id: question.id.clone(),
                value,
                confidence,
            });
        }

        Ok(DecisionResult {
            answers,
            usage: Usage {
                input_tokens: wire.usage.input_tokens,
                output_tokens: wire.usage.output_tokens,
            },
            provider: ProviderMeta {
                id: "jev".to_owned(),
                model: Some(wire.model),
            },
            latency_ms,
        })
    }

    async fn health(&self) -> HealthReport {
        let started = Instant::now();
        let mut questions = BTreeMap::new();
        questions.insert(
            "health".to_owned(),
            WireQuestion {
                kind: "noul",
                instructions: "Health probe: return the probability that the state's `probe` \
                               field is true."
                    .to_owned(),
                criteria: None,
            },
        );
        let request = WireRequest {
            state: serde_json::json!({ "probe": true }),
            model: self.config.model.clone(),
            questions,
        };
        match self.send(request).await {
            Ok((wire, _)) => HealthReport {
                ok: true,
                latency_ms: started.elapsed().as_millis() as u64,
                detail: None,
                // The response's model id is the only version signal jev gives.
                version: Some(wire.model),
            },
            Err(error) => HealthReport {
                ok: false,
                latency_ms: started.elapsed().as_millis() as u64,
                detail: Some(error.to_string()),
                version: None,
            },
        }
    }
}
