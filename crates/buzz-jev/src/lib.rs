//! Typed client for the Jev systemone classifier (`POST /v1/systemone`).
//!
//! Phase 1 of the shadow-mode plan: this crate only asks and parses. It never
//! acts on an answer; callers decide what a judgment is worth.
//!
//! Two rules are enforced by construction:
//!
//! - **Argmax, never `score`.** A score answer's wire `score` is the
//!   probability-weighted mean index (1.63 is not a level). [`Answer::Score`]
//!   carries the argmax of `probabilities` and has no `score` field at all.
//! - **No silent trust.** A choice that is not the argmax of its own
//!   probabilities, a probability outside `[0, 1]`, or a missing answer is a
//!   [`JevError::ShapeAnomaly`], not a value.
//!
//! The key comes only from [`API_KEY_ENV`]; a missing key is an error, never a
//! default. Every call has a hard [`TIMEOUT`].

mod wire;

use std::fmt;
use std::time::Duration;

use reqwest::header::{HeaderValue, AUTHORIZATION};
use reqwest::StatusCode;

pub use wire::{argmax, argmax_index, Answer, Judgment, NoulCriteria, Question, Request, Usage};

/// Production endpoint.
pub const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
/// Environment variable holding the bearer key.
pub const API_KEY_ENV: &str = "JEV_API_KEY";
/// Hard deadline for one call: connect, request, and full response body.
pub const TIMEOUT: Duration = Duration::from_secs(2);
/// Longest error-body excerpt kept in [`JevError::Rejected`].
const BODY_EXCERPT: usize = 512;

/// Everything that can go wrong asking Jev.
#[derive(Debug, thiserror::Error)]
pub enum JevError {
    /// The key is unset, blank, or not a valid header value.
    #[error("Jev API key missing or malformed; set {API_KEY_ENV}")]
    MissingKey,
    /// No complete response within [`TIMEOUT`].
    #[error("Jev did not answer within {TIMEOUT:?}")]
    Timeout,
    /// Connection, TLS, or protocol failure before a response arrived.
    #[error("transport error calling Jev: {0}")]
    Transport(#[source] reqwest::Error),
    /// HTTP 401 or 403: the key was refused.
    #[error("Jev refused the API key (HTTP {0})")]
    Unauthorized(u16),
    /// HTTP 429.
    #[error("Jev rate limit hit (HTTP 429)")]
    RateLimited,
    /// Any other 4xx, e.g. 422 for a malformed question.
    #[error("Jev rejected the request (HTTP {status}): {body}")]
    Rejected {
        /// HTTP status.
        status: u16,
        /// First bytes of the response body.
        body: String,
    },
    /// HTTP 5xx or another non-success status.
    #[error("Jev server error (HTTP {0})")]
    Server(u16),
    /// The body is not JSON of the systemone response shape.
    #[error("Jev response did not decode: {0}")]
    Decode(#[source] serde_json::Error),
    /// The body decoded but is internally inconsistent.
    #[error("Jev shape anomaly in {question:?}: {detail}")]
    ShapeAnomaly {
        /// Question id the anomaly was found in.
        question: String,
        /// What was wrong.
        detail: String,
    },
}

/// Systemone client. Cheap to clone; reuses connections.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoint: String,
    auth: HeaderValue,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("endpoint", &self.endpoint)
            .field("auth", &"<redacted>")
            .finish()
    }
}

impl Client {
    /// Production client keyed from [`API_KEY_ENV`].
    pub fn from_env() -> Result<Self, JevError> {
        Self::new(ENDPOINT, &std::env::var(API_KEY_ENV).unwrap_or_default())
    }

    /// Client for an explicit endpoint and key. A blank key is rejected.
    pub fn new(endpoint: impl Into<String>, api_key: &str) -> Result<Self, JevError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(JevError::MissingKey);
        }
        let mut auth = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| JevError::MissingKey)?;
        auth.set_sensitive(true);
        let http = reqwest::Client::builder()
            .build()
            .map_err(JevError::Transport)?;
        Ok(Self {
            http,
            endpoint: endpoint.into(),
            auth,
        })
    }

    /// Ask the questions in `request` and return the validated judgment.
    ///
    /// Fails with [`JevError::Timeout`] if the whole exchange, body included,
    /// takes longer than [`TIMEOUT`].
    pub async fn judge(&self, request: &Request) -> Result<Judgment, JevError> {
        let exchange = async {
            let response = self
                .http
                .post(&self.endpoint)
                .header(AUTHORIZATION, self.auth.clone())
                .json(request)
                .send()
                .await
                .map_err(JevError::Transport)?;
            let status = response.status();
            let body = response.bytes().await.map_err(JevError::Transport)?;
            Ok::<_, JevError>((status, body))
        };
        let (status, body) = tokio::time::timeout(TIMEOUT, exchange)
            .await
            .map_err(|_| JevError::Timeout)??;
        check_status(status, &body)?;
        let judgment = Judgment::from_slice(&body)?;
        judgment.check_against(request)?;
        Ok(judgment)
    }
}

fn check_status(status: StatusCode, body: &[u8]) -> Result<(), JevError> {
    let code = status.as_u16();
    match code {
        _ if status.is_success() => Ok(()),
        401 | 403 => Err(JevError::Unauthorized(code)),
        429 => Err(JevError::RateLimited),
        400..=499 => {
            let text = String::from_utf8_lossy(body);
            Err(JevError::Rejected {
                status: code,
                body: text.chars().take(BODY_EXCERPT).collect(),
            })
        }
        _ => Err(JevError::Server(code)),
    }
}
