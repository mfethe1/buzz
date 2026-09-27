//! Request and response types for `POST /v1/systemone`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Body of a `POST /v1/systemone` call: `{model, state, questions}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Model id, e.g. `jev-latest`.
    pub model: String,
    /// Free-form facts the model judges. Keys are yours; values are any JSON.
    pub state: BTreeMap<String, serde_json::Value>,
    /// Question id → question. Each answer comes back under the same id.
    pub questions: BTreeMap<String, Question>,
}

/// One typed question. The wire tag is the `type` field.
///
/// `instructions` and every criteria entry are the API's `EntryType`: a
/// string, object, array, or null, so they are raw JSON here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// Yes/no; the answer is the probability of yes.
    Noul {
        /// The question.
        instructions: serde_json::Value,
        /// Optional descriptions of what yes and no mean.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    /// Pick one option from a named set.
    Choice {
        /// The question.
        instructions: serde_json::Value,
        /// Option id → description.
        criteria: BTreeMap<String, serde_json::Value>,
    },
    /// Position on an ordered list of levels (index 0 first).
    Score {
        /// The question.
        instructions: serde_json::Value,
        /// Level descriptions, lowest first.
        criteria: Vec<serde_json::Value>,
    },
}

/// What a yes and a no mean for a [`Question::Noul`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    /// Meaning of yes.
    #[serde(rename = "true")]
    pub yes: serde_json::Value,
    /// Meaning of no.
    #[serde(rename = "false")]
    pub no: serde_json::Value,
}

/// Token accounting returned with every response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Usage {
    /// Prompt tokens billed.
    pub input_tokens: u64,
    /// Completion tokens billed.
    pub output_tokens: u64,
}

/// A validated answer. Every value was checked against the response's own
/// probabilities; nothing here was taken on the model's word alone.
///
/// A score answer carries the argmax level, never the wire `score` field.
/// `score` is the probability-weighted mean index (e.g. 1.63), not a level id,
/// and this type has no field or accessor for it:
///
/// ```compile_fail,E0026
/// fn read(a: buzz_jev::Answer) {
///     if let buzz_jev::Answer::Score { score, .. } = a { let _ = score; }
/// }
/// ```
///
/// The same pattern with the real field compiles, so the failure above can
/// only be the missing `score`:
///
/// ```
/// fn read(a: buzz_jev::Answer) {
///     if let buzz_jev::Answer::Score { index, .. } = a { let _ = index; }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// Probability that the answer is yes, in `[0, 1]`.
    Noul {
        /// P(yes).
        p_yes: f64,
    },
    /// The chosen option and its probability mass.
    Choice {
        /// Option id; always an argmax of `probabilities`.
        option: String,
        /// Probability mass of `option`.
        mass: f64,
    },
    /// The argmax level of a score question and its probability mass.
    Score {
        /// Level index (argmax of `probabilities`).
        index: usize,
        /// Probability mass of `index`.
        mass: f64,
    },
}

/// A parsed, validated systemone response: `.answers` and `.usage`.
#[derive(Debug, Clone, PartialEq)]
pub struct Judgment {
    /// Resolved model id, e.g. `jev-1.13.0`.
    pub model: String,
    /// Question id → validated answer.
    pub answers: BTreeMap<String, Answer>,
    /// Token accounting.
    pub usage: Usage,
}
