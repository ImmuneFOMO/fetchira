use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config: {0}")]
    Config(String),

    #[error(transparent)]
    Transport(#[from] reqwest::Error),

    #[error(transparent)]
    Web(#[from] wreq::Error),

    // Boxed: tungstenite's error is 136 bytes and would bloat every Result.
    #[error(transparent)]
    Ws(Box<tokio_tungstenite::tungstenite::Error>),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Db(#[from] sqlx::Error),

    #[error("rate limited: {0}")]
    RateLimit(String),

    #[error("quota exhausted: {0}")]
    QuotaExceeded(String),

    #[error("{provider} returned {status}: {body}")]
    Provider {
        provider: &'static str,
        status: u16,
        body: String,
    },

    #[error("{0} produced an unexpected response shape")]
    BadResponse(&'static str),

    #[error("missing required argument: {0}")]
    MissingArg(&'static str),

    #[error("{0} does not support this capability")]
    Unsupported(&'static str),

    #[error("no available account for {0}")]
    NoCandidate(&'static str),

    #[error("forced provider {0} has no available account")]
    ProviderForced(String),

    #[error("{0} timed out")]
    Timeout(&'static str),

    #[error("{0}")]
    Schema(String),
}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::Ws(Box::new(e))
    }
}
