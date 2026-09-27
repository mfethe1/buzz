//! Request and response types for `POST /v1/systemone`, plus the parse step
//! that turns a raw response into a validated [`Judgment`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::JevError;

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

#[derive(Deserialize)]
struct WireResponse {
    model: String,
    answers: BTreeMap<String, WireAnswer>,
    usage: Usage,
}

// `score`, `confidence` and `legend` are deliberately absent: serde skips
// fields a struct does not declare, so they are never read.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WireAnswer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
    },
    Score {
        probabilities: BTreeMap<String, f64>,
    },
}

/// Mass differences below this are ties (the API rounds to 2 decimals).
const TIE_EPSILON: f64 = 1e-9;

fn anomaly(question: &str, detail: impl Into<String>) -> JevError {
    JevError::ShapeAnomaly {
        question: question.to_owned(),
        detail: detail.into(),
    }
}

fn check_probability(question: &str, p: f64) -> Result<f64, JevError> {
    if p.is_finite() && (0.0..=1.0).contains(&p) {
        Ok(p)
    } else {
        Err(anomaly(question, format!("probability {p} outside [0, 1]")))
    }
}

/// Key with the highest probability, and its mass. Ties go to the first key
/// in sorted order. Errors if the map is empty or holds a value outside
/// `[0, 1]`.
pub fn argmax<'a>(
    question: &str,
    probabilities: &'a BTreeMap<String, f64>,
) -> Result<(&'a str, f64), JevError> {
    let mut best: Option<(&str, f64)> = None;
    for (key, &p) in probabilities {
        let p = check_probability(question, p)?;
        if best.is_none_or(|(_, m)| p > m + TIE_EPSILON) {
            best = Some((key, p));
        }
    }
    best.ok_or_else(|| anomaly(question, "empty probabilities"))
}

/// Argmax level index of a score question's probabilities (keys `"0"`,
/// `"1"`, ...). A winner that is not an in-range index is an anomaly, never
/// clamped to a valid level.
pub fn argmax_index(
    question: &str,
    probabilities: &BTreeMap<String, f64>,
) -> Result<(usize, f64), JevError> {
    let (key, mass) = argmax(question, probabilities)?;
    match key.parse::<usize>() {
        Ok(index) if index < probabilities.len() => Ok((index, mass)),
        _ => Err(anomaly(
            question,
            format!("argmax key {key:?} is not a level index"),
        )),
    }
}

fn parse_answer(question: &str, wire: WireAnswer) -> Result<Answer, JevError> {
    match wire {
        WireAnswer::Noul { noul } => Ok(Answer::Noul {
            p_yes: check_probability(question, noul)?,
        }),
        WireAnswer::Choice {
            choice,
            probabilities,
        } => {
            let (winner, top) = argmax(question, &probabilities)?;
            match probabilities.get(&choice) {
                Some(&mass) if mass + TIE_EPSILON >= top => Ok(Answer::Choice {
                    option: choice,
                    mass,
                }),
                _ => Err(anomaly(
                    question,
                    format!("choice {choice:?} is not the argmax {winner:?} ({top})"),
                )),
            }
        }
        WireAnswer::Score { probabilities } => {
            let (index, mass) = argmax_index(question, &probabilities)?;
            Ok(Answer::Score { index, mass })
        }
    }
}

impl Judgment {
    /// Parse and validate a raw response body.
    pub fn from_slice(body: &[u8]) -> Result<Self, JevError> {
        let wire: WireResponse = serde_json::from_slice(body).map_err(JevError::Decode)?;
        let answers = wire
            .answers
            .into_iter()
            .map(|(id, a)| parse_answer(&id, a).map(|a| (id, a)))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            model: wire.model,
            answers,
            usage: wire.usage,
        })
    }

    /// Every asked question must come back, with the type that was asked.
    pub fn check_against(&self, request: &Request) -> Result<(), JevError> {
        for (id, question) in &request.questions {
            let matches = matches!(
                (question, self.answers.get(id)),
                (Question::Noul { .. }, Some(Answer::Noul { .. }))
                    | (Question::Choice { .. }, Some(Answer::Choice { .. }))
                    | (Question::Score { .. }, Some(Answer::Score { .. }))
            );
            if !matches {
                return Err(anomaly(id, "asked question missing or of the wrong type"));
            }
        }
        Ok(())
    }
}
