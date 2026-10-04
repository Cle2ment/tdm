//! Conformance battery for the jev provider, run against an in-process mock
//! System One server (no live network, no real API key).

use serde_json::{Value, json};
use tdm_conformance::assert_compliant;
use tdm_core::{DecisionProvider, DecisionRequest, Primitive, Question, TdmError};
use tdm_provider_jev::{JevConfig, JevProvider};
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{method, path},
};

/// Answers every incoming question with a contract-valid System One payload
/// keyed by the question's own id, mirroring the response shapes in
/// `jev_wire.rs`:
///
/// - `choice` -> uniform distribution over the requested options, first
///   option wins, confidence 0.5
/// - `score` -> uniform distribution over the requested levels, weighted
///   mid-scale, confidence 0.5
/// - `noul` (and anything unrecognized) -> P(yes) = 0.75
struct SystemOneResponder;

impl Respond for SystemOneResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let mut answers = serde_json::Map::new();
        if let Some(questions) = body.get("questions").and_then(Value::as_object) {
            for (id, question) in questions {
                answers.insert(id.clone(), canned_answer(question));
            }
        }
        ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-latest",
            "answers": Value::Object(answers),
            "usage": { "input_tokens": 10, "output_tokens": 5 },
        }))
    }
}

/// One contract-valid wire answer for `question`.
fn canned_answer(question: &Value) -> Value {
    match question.get("type").and_then(Value::as_str) {
        Some("choice") => {
            let options: Vec<&str> = question
                .get("criteria")
                .and_then(Value::as_object)
                .map(|criteria| criteria.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let share = 1.0 / options.len().max(1) as f64;
            let probabilities: serde_json::Map<String, Value> = options
                .iter()
                .map(|option| ((*option).to_owned(), json!(share)))
                .collect();
            json!({
                "type": "choice",
                "choice": options.first().copied().unwrap_or_default(),
                "probabilities": Value::Object(probabilities),
                "confidence": 0.5,
            })
        }
        Some("score") => {
            let levels: Vec<&str> = question
                .get("criteria")
                .and_then(Value::as_array)
                .map(|levels| levels.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let share = 1.0 / levels.len().max(1) as f64;
            let mut legend = serde_json::Map::new();
            let mut probabilities = serde_json::Map::new();
            for (index, level) in levels.iter().enumerate() {
                legend.insert(index.to_string(), json!(level));
                probabilities.insert(index.to_string(), json!(share));
            }
            json!({
                "type": "score",
                "score": (levels.len().saturating_sub(1) as f64) / 2.0,
                "legend": Value::Object(legend),
                "probabilities": Value::Object(probabilities),
                "confidence": 0.5,
            })
        }
        _ => json!({ "type": "noul", "noul": 0.75 }),
    }
}

fn jev_at(uri: String) -> JevProvider {
    JevProvider::new(JevConfig {
        endpoint: uri,
        api_key: "test-key".to_owned(),
        ..JevConfig::default()
    })
}

async fn tracked_responder_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(SystemOneResponder)
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn jev_provider_is_fully_conformant() {
    let server = tracked_responder_server().await;
    assert_compliant(&jev_at(server.uri())).await;
}

/// The conformance battery's invalid-question leg must never reach the wire:
/// the jev provider rejects `Question::validate()` violations pre-HTTP with
/// `TdmError::Client` (pattern from `jev_wire.rs`).
#[tokio::test(flavor = "multi_thread")]
async fn invalid_question_is_rejected_before_any_http_request() {
    let server = tracked_responder_server().await;
    let provider = jev_at(server.uri());

    let error = provider
        .judge(DecisionRequest {
            state: json!({}),
            questions: vec![Question {
                id: "conf-invalid".into(),
                instructions: String::new(),
                primitive: Primitive::Noul,
            }],
        })
        .await
        .unwrap_err();

    assert!(
        matches!(&error, TdmError::Client { status: 422, .. }),
        "expected Client 422, got {error:?}"
    );
    let requests = server.received_requests().await.expect("tracking enabled");
    assert!(
        requests.is_empty(),
        "invalid question must be rejected before any HTTP request"
    );
}
