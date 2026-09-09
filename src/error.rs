use thiserror::Error;

use std::time::Duration;

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

    #[error("rate limited: {message}")]
    RateLimit {
        message: String,
        retry_after: Option<Duration>,
    },

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

impl Error {
    pub fn rate_limit(message: impl Into<String>) -> Self {
        Self::RateLimit {
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn rate_limit_after(message: impl Into<String>, retry_after: Option<Duration>) -> Self {
        Self::RateLimit {
            message: message.into(),
            retry_after,
        }
    }
}

/// Parse `Retry-After` as delta-seconds or an HTTP date. Malformed/past values fall back to the
/// router's short default cooldown.
pub(crate) fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    let value = value?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let timestamp = chrono::DateTime::parse_from_rfc2822(value)
        .map(|date| date.timestamp())
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(value, "%a, %d %b %Y %H:%M:%S GMT")
                .map(|date| date.and_utc().timestamp())
        })
        .ok()?;
    let seconds = timestamp.saturating_sub(chrono::Utc::now().timestamp());
    (seconds > 0).then(|| Duration::from_secs(seconds as u64))
}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::Ws(Box::new(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_seconds_are_preserved() {
        assert_eq!(parse_retry_after(Some("37")), Some(Duration::from_secs(37)));
        assert_eq!(parse_retry_after(Some("invalid")), None);
    }

    #[test]
    fn retry_after_http_date_is_supported() {
        let value = (chrono::Utc::now() + chrono::TimeDelta::seconds(120))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let seconds = parse_retry_after(Some(&value)).unwrap().as_secs();
        assert!((118..=120).contains(&seconds), "got {seconds}s");
    }
}
