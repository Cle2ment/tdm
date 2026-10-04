//! `tdm-provider-jev` — TypeSafe System One (jev) adapter implementing the
//! TDM [`DecisionProvider`] contract (plan §5, ADR-0003).
//!
//! # Wire mapping
//!
//! - [`tdm_core::DecisionRequest::state`] is passed through verbatim; the
//!   model id and the questions map (keyed by [`tdm_core::Question::id`],
//!   which never reaches the model's inference context) complete the request
//!   body.
//! - `Primitive::Choice` -> `criteria` object with one `null` entry per
//!   option; `Primitive::Noul` -> no `criteria` key at all; `Primitive::Score`
//!   -> `criteria` as the ordered level-description array.
//! - Answers map back to [`tdm_core::AnswerValue`]: `choice` keeps the label
//!   plus probabilities, `noul` keeps the probability (confidence stays
//!   `None`), `score` resolves the argmax level index through the wire
//!   `legend` and rewrites the distribution with level labels.
//!
//! # Error mapping
//!
//! - transport / connect / timeout -> [`tdm_core::TdmError::Network`]
//! - 429 and 5xx (including 529) -> [`tdm_core::TdmError::Server`]
//! - other 4xx -> [`tdm_core::TdmError::Client`] (422 carries
//!   `code: Some("invalid_request")`); local [`tdm_core::ValidationError`]s
//!   surface as `Client` with status 422 before any HTTP traffic is sent
//! - an HTTP-success body that fails to parse or violates the answer contract
//!   -> [`tdm_core::TdmError::Quality`] (the provider broke its contract; the
//!   caller did nothing wrong, and `Client` is reserved for 4xx statuses)
//!
//! `judge` performs exactly one attempt: retry/backoff/circuit-breaking
//! policy belongs to the runtime (M1), not to providers.

mod config;
mod provider;
mod wire;

pub use config::{JevConfig, JevConfigError};
pub use provider::JevProvider;
