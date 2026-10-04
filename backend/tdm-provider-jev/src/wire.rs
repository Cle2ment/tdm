//! jev (TypeSafe System One) wire types and contract mapping.
//!
//! Request: `POST <endpoint>` with `{ state, model, questions }`; `questions`
//! is a map keyed by caller-chosen ids (never sent to the model). The
//! response echoes the same keys under `answers`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tdm_core::{AnswerValue, Primitive, Question, TdmError};

/// Wire request body.
#[derive(Debug, Serialize)]
pub(crate) struct WireRequest {
    /// Decision state, passed through verbatim.
    pub state: serde_json::Value,
    /// Model identifier.
    pub model: String,
    /// Questions keyed by [`Question::id`].
    pub questions: BTreeMap<String, WireQuestion>,
}

/// Wire question shape.
#[derive(Debug, Serialize)]
pub(crate) struct WireQuestion {
    /// Primitive tag: `"choice" | "noul" | "score"`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// Judgment instructions.
    pub instructions: String,
    /// Primitive-specific criteria; omitted entirely for noul.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criteria: Option<WireCriteria>,
}

/// Criteria payload; untagged so choice emits an object and score an array.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum WireCriteria {
    /// One entry per option label; values are `null` (no extra detail needed).
    Choice(BTreeMap<String, serde_json::Value>),
    /// Ordered level descriptions.
    Score(Vec<String>),
}

/// Wire response body.
#[derive(Debug, Deserialize)]
pub(crate) struct WireResponse {
    /// Model that answered.
    pub model: String,
    /// Answers keyed by the request's question ids.
    pub answers: BTreeMap<String, WireAnswer>,
    /// Token usage.
    pub usage: WireUsage,
}

/// Wire usage counters.
#[derive(Debug, Deserialize)]
pub(crate) struct WireUsage {
    /// Input tokens consumed.
    #[serde(rename = "input_tokens")]
    pub input_tokens: u64,
    /// Output tokens produced.
    #[serde(rename = "output_tokens")]
    pub output_tokens: u64,
}

/// Wire answer; tagged by `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum WireAnswer {
    /// Boolean judgment.
    Noul {
        /// P(yes).
        noul: f64,
    },
    /// Single choice.
    Choice {
        /// Chosen option label.
        choice: String,
        /// Competing distribution over option labels.
        probabilities: BTreeMap<String, f64>,
        /// Distribution concentration.
        confidence: f64,
    },
    /// Ordered score level.
    Score {
        /// Probability-weighted score (may land between levels).
        score: f64,
        /// Level index string -> level description.
        legend: BTreeMap<String, String>,
        /// Level index string -> probability.
        probabilities: BTreeMap<String, f64>,
        /// Distribution concentration.
        confidence: f64,
    },
}

/// Maps a core [`Question`] onto its wire shape (tdm-core -> jev).
pub(crate) fn wire_question(question: &Question) -> WireQuestion {
    let (kind, criteria) = match &question.primitive {
        Primitive::Choice { options } => (
            "choice",
            Some(WireCriteria::Choice(
                options
                    .iter()
                    .map(|option| (option.clone(), serde_json::Value::Null))
                    .collect(),
            )),
        ),
        Primitive::Noul => ("noul", None),
        Primitive::Score { levels } => ("score", Some(WireCriteria::Score(levels.clone()))),
    };
    WireQuestion {
        kind,
        instructions: question.instructions.clone(),
        criteria,
    }
}

/// Maps a wire answer onto the core [`AnswerValue`] plus its confidence.
///
/// # Errors
/// [`TdmError::Quality`] when the payload violates the wire contract in a way
/// that makes the result unusable (empty score probabilities, or a
/// probabilities key absent from the legend).
pub(crate) fn core_answer(wire: &WireAnswer) -> Result<(AnswerValue, Option<f64>), TdmError> {
    let quality = |detail: String| TdmError::Quality { detail };
    match wire {
        WireAnswer::Noul { noul } => Ok((AnswerValue::Noul { probability: *noul }, None)),
        WireAnswer::Choice {
            choice,
            probabilities,
            confidence,
        } => Ok((
            AnswerValue::Choice {
                label: choice.clone(),
                distribution: probabilities
                    .iter()
                    .map(|(label, probability)| (label.clone(), *probability))
                    .collect(),
            },
            Some(*confidence),
        )),
        WireAnswer::Score {
            score,
            legend,
            probabilities,
            confidence,
        } => {
            if probabilities.is_empty() {
                return Err(quality("score answer has empty probabilities".to_owned()));
            }
            let mut distribution = Vec::with_capacity(probabilities.len());
            let mut best: Option<(&str, f64)> = None;
            for (index, probability) in probabilities {
                let label = legend.get(index).ok_or_else(|| {
                    quality(format!("score legend is missing level index {index:?}"))
                })?;
                distribution.push((label.clone(), *probability));
                if best.is_none_or(|(_, top)| *probability > top) {
                    best = Some((label.as_str(), *probability));
                }
            }
            let level = best.expect("non-empty, checked above").0.to_owned();
            Ok((
                AnswerValue::Score {
                    level,
                    weighted: *score,
                    distribution,
                },
                Some(*confidence),
            ))
        }
    }
}
