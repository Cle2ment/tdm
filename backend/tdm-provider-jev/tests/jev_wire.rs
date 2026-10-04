//! wiremock-driven integration tests for the jev provider.
//!
//! No live network, no real API key: every test talks to an in-process mock
//! server speaking the pinned System One wire contract.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tdm_core::{
    Answer, AnswerValue, DecisionProvider, DecisionRequest, Primitive, Question, TdmError,
};
use tdm_provider_jev::{JevConfig, JevConfigError, JevProvider};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn test_provider(uri: String) -> JevProvider {
    JevProvider::new(JevConfig {
        endpoint: uri,
        api_key: "test-key".to_owned(),
        ..JevConfig::default()
    })
}

/// Mock server with request recording enabled (default) for
/// `received_requests` assertions.
async fn tracked_server() -> MockServer {
    MockServer::start().await
}

/// A contract-valid jev response covering all three primitives.
fn ok_response_body() -> Value {
    json!({
        "model": "jev-latest",
        "answers": {
            "c1": {
                "type": "choice",
                "choice": "b",
                "probabilities": { "a": 0.3, "b": 0.7 },
                "confidence": 0.4
            },
            "n1": { "type": "noul", "noul": 0.87 },
            "s1": {
                "type": "score",
                "score": 1.9,
                "legend": { "0": "low", "1": "high" },
                "probabilities": { "0": 0.1, "1": 0.9 },
                "confidence": 0.8
            }
        },
        "usage": { "input_tokens": 10, "output_tokens": 5 }
    })
}

/// One batch exercising all three primitives.
fn sample_request() -> DecisionRequest {
    DecisionRequest {
        state: json!({ "repo": "tdm" }),
        questions: vec![
            Question {
                id: "c1".into(),
                instructions: "pick one".into(),
                primitive: Primitive::Choice {
                    options: vec!["a".into(), "b".into()],
                },
            },
            Question {
                id: "n1".into(),
                instructions: "is it safe?".into(),
                primitive: Primitive::Noul,
            },
            Question {
                id: "s1".into(),
                instructions: "rate it".into(),
                primitive: Primitive::Score {
                    levels: vec!["low".into(), "high".into()],
                },
            },
        ],
    }
}

async fn mounted_ok() -> (MockServer, JevProvider) {
    let server = tracked_server().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_response_body()))
        .mount(&server)
        .await;
    let provider = test_provider(server.uri());
    (server, provider)
}

/// Server answering every POST with `status` + `body`.
/// The server is returned alongside the provider so callers keep it alive
/// for the test's duration — dropping it frees the port, which a parallel
/// test's server can then claim (observed as cross-test contamination on
/// Linux CI).
async fn mounted_status(status: u16, body: &str) -> (MockServer, JevProvider) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(&server)
        .await;
    let provider = test_provider(server.uri());
    (server, provider)
}

// ---------------------------------------------------------------------------
// 1. Golden request shape per primitive
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn golden_request_shape_covers_all_primitives() {
    let (server, provider) = mounted_ok().await;
    provider.judge(sample_request()).await.unwrap();

    let requests = server.received_requests().await.expect("tracking enabled");
    assert_eq!(requests.len(), 1, "exactly one HTTP request");
    let sent = &requests[0];
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.url.path(), "/");
    assert_eq!(
        sent.headers
            .get("authorization")
            .expect("authorization header")
            .to_str()
            .unwrap(),
        "Bearer test-key"
    );

    let body: Value = serde_json::from_slice(&sent.body).unwrap();
    assert_eq!(body["model"], "jev-latest");
    let questions = body["questions"].as_object().expect("questions is a map");
    assert_eq!(questions.len(), 3);
    for id in ["c1", "n1", "s1"] {
        assert!(questions.contains_key(id), "questions keyed by id: {id}");
    }

    let choice = &questions["c1"];
    assert_eq!(choice["type"], "choice");
    assert_eq!(choice["instructions"], "pick one");
    let criteria = choice["criteria"].as_object().expect("choice criteria map");
    assert_eq!(criteria.len(), 2);
    assert!(
        criteria.values().all(Value::is_null),
        "choice criteria values are null"
    );

    let noul = &questions["n1"];
    assert_eq!(noul["type"], "noul");
    assert!(noul.get("criteria").is_none(), "noul omits criteria");

    let score = &questions["s1"];
    assert_eq!(score["type"], "score");
    assert_eq!(
        score["criteria"],
        json!(["low", "high"]),
        "score criteria is the ordered level array"
    );
}

// ---------------------------------------------------------------------------
// 2. Response mapping per primitive
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn maps_responses_for_all_primitives() {
    let (_server, provider) = mounted_ok().await;
    let result = provider.judge(sample_request()).await.unwrap();
    assert_eq!(result.answers.len(), 3);

    let by_id: BTreeMap<&str, &Answer> = result
        .answers
        .iter()
        .map(|answer| (answer.id.as_str(), answer))
        .collect();

    let noul = by_id["n1"];
    assert_eq!(noul.value, AnswerValue::Noul { probability: 0.87 });
    assert_eq!(noul.confidence, None, "noul carries no confidence");

    let choice = by_id["c1"];
    assert_eq!(
        choice.value,
        AnswerValue::Choice {
            label: "b".into(),
            distribution: vec![("a".into(), 0.3), ("b".into(), 0.7)],
        }
    );
    assert_eq!(choice.confidence, Some(0.4));

    let score = by_id["s1"];
    assert_eq!(
        score.value,
        AnswerValue::Score {
            level: "high".into(),
            weighted: 1.9,
            // Probabilities remapped from index keys through the legend.
            distribution: vec![("low".into(), 0.1), ("high".into(), 0.9)],
        }
    );
    assert_eq!(score.confidence, Some(0.8));

    assert_eq!(result.usage.input_tokens, 10);
    assert_eq!(result.usage.output_tokens, 5);
    assert_eq!(result.provider.id, "jev");
    assert_eq!(result.provider.model.as_deref(), Some("jev-latest"));
}

// ---------------------------------------------------------------------------
// 3. Error mapping
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn maps_401_to_client_error() {
    let (_server, provider) = mounted_status(401, r#"{"error":"unauthorized"}"#).await;
    let error = provider.judge(sample_request()).await.unwrap_err();
    let retryable = error.is_retryable();
    assert!(
        matches!(
            &error,
            TdmError::Client { status: 401, code: None, message }
                if message.contains("unauthorized")
        ),
        "got {error:?}"
    );
    assert!(!retryable);
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_422_to_client_error_with_invalid_request_code() {
    let body = "x".repeat(800);
    let (_server, provider) = mounted_status(422, &body).await;
    let error = provider.judge(sample_request()).await.unwrap_err();
    let retryable = error.is_retryable();
    assert!(
        matches!(
            &error,
            TdmError::Client { status: 422, code, .. }
                if code.as_deref() == Some("invalid_request")
        ),
        "got {error:?}"
    );
    if let TdmError::Client { message, .. } = &error {
        let chars = message.chars().count();
        assert!(
            (500..=540).contains(&chars),
            "body must be truncated to ~500 chars, got {chars}"
        );
    }
    assert!(!retryable);
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_429_to_retryable_server_error() {
    let (_server, provider) = mounted_status(429, r#"{"error":"rate limited"}"#).await;
    let error = provider.judge(sample_request()).await.unwrap_err();
    assert!(
        matches!(&error, TdmError::Server { status: 429, .. }),
        "got {error:?}"
    );
    assert!(error.is_retryable());
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_529_to_retryable_server_error() {
    let (_server, provider) = mounted_status(529, "overloaded").await;
    let error = provider.judge(sample_request()).await.unwrap_err();
    assert!(
        matches!(&error, TdmError::Server { status: 529, .. }),
        "got {error:?}"
    );
    assert!(error.is_retryable());
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_500_to_retryable_server_error() {
    let (_server, provider) = mounted_status(500, "boom").await;
    let error = provider.judge(sample_request()).await.unwrap_err();
    let retryable = error.is_retryable();
    assert!(
        matches!(
            &error,
            TdmError::Server { status: 500, message }
                if message.contains("boom")
        ),
        "got {error:?}"
    );
    assert!(retryable);
}

/// Chosen mapping (documented in crate docs): an HTTP-200 body that fails to
/// parse means the provider broke its contract, so `Quality` — `Client` stays
/// reserved for 4xx statuses, and it is not retryable either.
#[tokio::test(flavor = "multi_thread")]
async fn maps_malformed_success_body_to_quality() {
    let (_server, provider) = mounted_status(200, "Ceci n'est pas du JSON").await;
    let error = provider.judge(sample_request()).await.unwrap_err();
    assert!(matches!(&error, TdmError::Quality { .. }), "got {error:?}");
    assert!(!error.is_retryable());
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_connect_failure_to_network() {
    // Reserve a port, then drop the listener so connect() is refused at once.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let provider = test_provider(format!("http://127.0.0.1:{port}"));
    let error = provider.judge(sample_request()).await.unwrap_err();
    assert!(matches!(&error, TdmError::Network { .. }), "got {error:?}");
    assert!(error.is_retryable());
}

// ---------------------------------------------------------------------------
// 4. Local validation happens before any HTTP traffic
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn invalid_choice_rejected_without_http() {
    let (server, provider) = mounted_ok().await;
    let request = DecisionRequest {
        state: json!({}),
        questions: vec![Question {
            id: "bad".into(),
            instructions: "pick".into(),
            primitive: Primitive::Choice { options: vec![] },
        }],
    };
    let error = provider.judge(request).await.unwrap_err();
    assert!(
        matches!(
            &error,
            TdmError::Client { status: 422, code, .. }
                if code.as_deref() == Some("invalid_request")
        ),
        "got {error:?}"
    );
    let requests = server.received_requests().await.expect("tracking enabled");
    assert!(requests.is_empty(), "no HTTP request must be made");
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_instructions_rejected_without_http() {
    let (server, provider) = mounted_ok().await;
    let request = DecisionRequest {
        state: json!({}),
        questions: vec![Question {
            id: "q".into(),
            instructions: String::new(),
            primitive: Primitive::Noul,
        }],
    };
    let error = provider.judge(request).await.unwrap_err();
    assert!(
        matches!(&error, TdmError::Client { status: 422, .. }),
        "got {error:?}"
    );
    let requests = server.received_requests().await.expect("tracking enabled");
    assert!(requests.is_empty());
}

/// Wire answers are keyed by question id; duplicates would make answers
/// unattributable, so they are rejected locally (Client 422, no HTTP).
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_question_ids_rejected_without_http() {
    let (server, provider) = mounted_ok().await;
    let request = DecisionRequest {
        state: json!({}),
        questions: vec![
            Question {
                id: "same".into(),
                instructions: "one".into(),
                primitive: Primitive::Noul,
            },
            Question {
                id: "same".into(),
                instructions: "two".into(),
                primitive: Primitive::Noul,
            },
        ],
    };
    let error = provider.judge(request).await.unwrap_err();
    assert!(
        matches!(
            &error,
            TdmError::Client { status: 422, code, .. }
                if code.as_deref() == Some("invalid_request")
        ),
        "got {error:?}"
    );
    let requests = server.received_requests().await.expect("tracking enabled");
    assert!(requests.is_empty());
}

// ---------------------------------------------------------------------------
// 5. health()
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn health_reports_ok_on_roundtrip() {
    let (server, provider) = mounted_ok().await;
    let report = provider.health().await;
    assert!(report.ok);
    assert!(report.detail.is_none());
    assert_eq!(report.version.as_deref(), Some("jev-latest"));

    let requests = server.received_requests().await.expect("tracking enabled");
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["questions"]["health"]["type"], "noul");
}

#[tokio::test(flavor = "multi_thread")]
async fn health_reports_failure_with_detail() {
    // Bare mock server: no mounted mock -> wiremock answers 404.
    let server = MockServer::start().await;
    let provider = test_provider(server.uri());
    let report = provider.health().await;
    assert!(!report.ok);
    assert!(report.detail.is_some());
    assert_eq!(report.version, None);
}

// ---------------------------------------------------------------------------
// Provider metadata / redaction / config-from-env
// ---------------------------------------------------------------------------

#[test]
fn provider_metadata_and_debug_redaction() {
    let provider = JevProvider::new(JevConfig {
        api_key: "super-secret-key".into(),
        ..JevConfig::default()
    });
    assert_eq!(provider.id(), "jev");
    let caps = provider.capabilities();
    assert_eq!(caps.primitives.len(), 3);
    assert!(caps.batch);
    assert_eq!(caps.max_state_bytes, 1 << 20);

    for debug in [
        format!("{provider:?}"),
        format!(
            "{:?}",
            JevConfig {
                api_key: "super-secret-key".into(),
                ..JevConfig::default()
            }
        ),
    ] {
        assert!(
            !debug.contains("super-secret-key"),
            "api key must never render: {debug}"
        );
        assert!(debug.contains("[redacted]"));
    }
}

#[test]
fn from_env_prefers_tdm_var_and_falls_back() {
    // SAFETY: no other test in this binary reads these variables; cargo runs
    // this process's tests on threads that only this test touches env on.
    unsafe {
        std::env::remove_var("TDM_JEV_API_KEY");
        std::env::remove_var("TYPESAFE_API_KEY");
    }
    assert!(matches!(
        JevConfig::from_env(),
        Err(JevConfigError::MissingApiKey)
    ));

    unsafe { std::env::set_var("TYPESAFE_API_KEY", "legacy") };
    assert_eq!(JevConfig::from_env().unwrap().api_key, "legacy");

    unsafe { std::env::set_var("TDM_JEV_API_KEY", "primary") };
    assert_eq!(JevConfig::from_env().unwrap().api_key, "primary");

    unsafe {
        std::env::remove_var("TDM_JEV_API_KEY");
        std::env::remove_var("TYPESAFE_API_KEY");
    }
}
