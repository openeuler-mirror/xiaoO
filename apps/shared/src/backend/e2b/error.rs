use reqwest::StatusCode;
use std::error::Error as StdError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum E2bFailureKind {
    Dns,
    Connect,
    Tls,
    Timeout,
    HttpStatus,
    BodyInterrupted,
    Protocol,
    Unknown,
}

impl E2bFailureKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::Connect => "connect",
            Self::Tls => "tls",
            Self::Timeout => "timeout",
            Self::HttpStatus => "http_status",
            Self::BodyInterrupted => "body_interrupted",
            Self::Protocol => "protocol",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct E2bFailure {
    pub(crate) kind: E2bFailureKind,
    pub(crate) message: String,
    pub(crate) retryable: bool,
    details: String,
}

impl E2bFailure {
    pub(crate) fn from_reqwest(context: &str, error: &reqwest::Error) -> Self {
        let chain = error_chain(error);
        let lowercase = chain.to_ascii_lowercase();
        let kind = if error.is_timeout() {
            E2bFailureKind::Timeout
        } else if lowercase.contains("dns")
            || lowercase.contains("failed to lookup address")
            || lowercase.contains("name or service not known")
        {
            E2bFailureKind::Dns
        } else if lowercase.contains("tls")
            || lowercase.contains("certificate")
            || lowercase.contains("handshake")
        {
            E2bFailureKind::Tls
        } else if error.is_body() || error.is_decode() {
            E2bFailureKind::BodyInterrupted
        } else if error.is_connect() {
            E2bFailureKind::Connect
        } else {
            E2bFailureKind::Unknown
        };
        let retryable = matches!(
            kind,
            E2bFailureKind::Dns
                | E2bFailureKind::Connect
                | E2bFailureKind::Tls
                | E2bFailureKind::Timeout
                | E2bFailureKind::BodyInterrupted
                | E2bFailureKind::Unknown
        );
        Self {
            kind,
            message: format!("{context}: {error}"),
            retryable,
            details: chain,
        }
    }

    pub(crate) fn from_status(context: &str, status: StatusCode, message: String) -> Self {
        Self {
            kind: E2bFailureKind::HttpStatus,
            message: format!("{context} failed with HTTP {status}: {message}"),
            retryable: status == StatusCode::TOO_MANY_REQUESTS
                || status == StatusCode::BAD_GATEWAY
                || status == StatusCode::SERVICE_UNAVAILABLE
                || status == StatusCode::GATEWAY_TIMEOUT
                || status.is_server_error(),
            details: String::new(),
        }
    }

    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self {
            kind: E2bFailureKind::Protocol,
            message: message.into(),
            retryable: false,
            details: String::new(),
        }
    }

    pub(crate) fn retryable_protocol(message: impl Into<String>) -> Self {
        Self {
            kind: E2bFailureKind::Protocol,
            message: message.into(),
            retryable: true,
            details: String::new(),
        }
    }

    pub(crate) fn body_interrupted(message: impl Into<String>) -> Self {
        Self {
            kind: E2bFailureKind::BodyInterrupted,
            message: message.into(),
            retryable: true,
            details: String::new(),
        }
    }

    pub(crate) fn log(
        &self,
        operation: &'static str,
        sandbox_id: Option<&str>,
        attempt: Option<usize>,
        elapsed_ms: u128,
    ) {
        tracing::warn!(
            operation,
            sandbox_id = sandbox_id.unwrap_or(""),
            attempt = attempt.unwrap_or_default(),
            elapsed_ms,
            failure_kind = self.kind.as_str(),
            retryable = self.retryable,
            error = %self.message,
            caused_by = %self.details,
            "E2B request failed"
        );
    }
}

fn error_chain(error: &reqwest::Error) -> String {
    let mut messages = Vec::new();
    let mut source = error.source();
    while let Some(current) = source {
        messages.push(current.to_string());
        source = current.source();
    }
    if messages.is_empty() {
        "none".to_string()
    } else {
        messages.join(" <- ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_statuses_are_classified() {
        assert!(
            E2bFailure::from_status("create", StatusCode::TOO_MANY_REQUESTS, "".into()).retryable
        );
        assert!(E2bFailure::from_status("create", StatusCode::BAD_GATEWAY, "".into()).retryable);
        assert!(!E2bFailure::from_status("create", StatusCode::UNAUTHORIZED, "".into()).retryable);
    }
}
