use thiserror::Error;

#[derive(Error, Debug)]
pub enum LarpshellError {
    #[error("failed to connect to {provider}: {message}")]
    ConnectionFailed { provider: String, message: String },

    #[error("auth failed: invalid API key")]
    InvalidApiKey,

    #[error("auth failed: {message}")]
    AuthenticationFailed { message: String },

    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("rate limit exceeded{}", Self::rate_limit_suffix(*retry_after))]
    RateLimitExceeded { retry_after: Option<u64> },

    #[error("server error from {provider}: {message}")]
    ServerError { provider: String, message: String },

    #[error("request timeout after {seconds} seconds")]
    Timeout { seconds: u64 },

    #[error("invalid response from API: {0}")]
    InvalidResponse(String),

    #[error("network error: {0}")]
    NetworkError(String),

    #[error("config error: {0}")]
    ConfigError(String),

    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("environment variable error: {0}")]
    EnvVarError(#[from] std::env::VarError),

    #[error("toml deserialize error: {0}")]
    TomlDeError(#[from] toml::de::Error),

    #[error("toml serialize error: {0}")]
    TomlSeError(#[from] toml::ser::Error),

    #[error("user input error: {0}")]
    InquireError(#[from] inquire::InquireError),

    #[error("request cancelled")]
    Cancelled,

    #[error("empty response from {0}")]
    EmptyResponse(String),

    #[error("agent reached maximum iterations ({0}) without producing a final response")]
    AgentMaxIterations(usize),

    #[error("no command provided")]
    NoCommandProvided,

    #[error("failed to generate a valid explanation")]
    EmptyExplanation,

    #[error("no API provider configured")]
    NoProviderConfigured,

    #[error("unknown command '{0}'")]
    UnknownSlashCommand(String),

    #[error("invalid argument for /{command}: expected {expected}")]
    InvalidSlashArg { command: String, expected: String },

    #[error("expected command after '!'")]
    ExpectedCommandAfterBang,
}

impl LarpshellError {
    fn rate_limit_suffix(retry_after: Option<u64>) -> std::borrow::Cow<'static, str> {
        match retry_after {
            Some(n) => format!("; retry after {n} seconds").into(),
            None => "; please try again later".into(),
        }
    }

    fn parse_retry_after(header: Option<&str>, body: &str) -> Option<u64> {
        if let Some(header_value) = header.and_then(|v| v.parse::<u64>().ok()) {
            return Some(header_value);
        }

        if body.contains("retry") {
            body.split("retry in ")
                .nth(1)
                .and_then(|s| s.split('s').next())
                .and_then(|s| s.parse::<f64>().ok())
                .map(f64::ceil)
                .map(|seconds| seconds as u64)
        } else {
            None
        }
    }

    pub fn connection_failed(provider: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ConnectionFailed {
            provider: provider.into(),
            message: message.into(),
        }
    }

    pub fn server_error(provider: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ServerError {
            provider: provider.into(),
            message: message.into(),
        }
    }

    pub const fn timeout(seconds: u64) -> Self {
        Self::Timeout { seconds }
    }

    pub fn auth_failed(message: impl Into<String>) -> Self {
        Self::AuthenticationFailed {
            message: message.into(),
        }
    }

    pub fn from_http_status(status: reqwest::StatusCode, provider: &str, body: &str) -> Self {
        Self::from_http_status_with_retry_header(status, provider, body, None)
    }

    pub fn from_http_status_with_retry_header(
        status: reqwest::StatusCode,
        provider: &str,
        body: &str,
        retry_after_header: Option<&str>,
    ) -> Self {
        match status.as_u16() {
            401 | 403 => {
                let body_lower = body.to_lowercase();
                if body_lower.contains("api key")
                    || body_lower.contains("api_key")
                    || body_lower.contains("apikey")
                    || body_lower.contains("invalid key")
                    || body_lower.contains("missing key")
                {
                    Self::InvalidApiKey
                } else {
                    Self::auth_failed(body)
                }
            }
            404 => {
                if body.contains("model") {
                    Self::ModelNotFound(body.to_string())
                } else {
                    Self::InvalidResponse(format!("endpoint not found: {body}"))
                }
            }
            429 => {
                let retry_after = Self::parse_retry_after(retry_after_header, body);
                Self::RateLimitExceeded { retry_after }
            }
            500..=599 => Self::server_error(provider, body),
            _ => Self::InvalidResponse(format!("{status}: {body}")),
        }
    }

    pub fn from_reqwest(error: &reqwest::Error, provider: &str) -> Self {
        if error.is_timeout() {
            Self::timeout(crate::common::DEFAULT_PROVIDER_TIMEOUT_SECS)
        } else if error.is_connect() {
            Self::connection_failed(
                provider,
                "check if the service is running and the URL is correct",
            )
        } else if error.is_request() {
            Self::NetworkError("invalid request".to_string())
        } else if let Some(status) = error.status() {
            Self::from_http_status(status, provider, &error.to_string())
        } else {
            Self::NetworkError(error.to_string())
        }
    }

    /// Print the error using CLI styling (red "error:" prefix) so callers
    /// don’t have to repeat the `print_error` boilerplate.
    pub fn print(&self) {
        crate::cli::print_error(&self.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_detection_401_with_api_key_phrase() {
        let err = LarpshellError::from_http_status(
            reqwest::StatusCode::UNAUTHORIZED,
            "openai",
            "invalid api key",
        );
        assert!(matches!(err, LarpshellError::InvalidApiKey));
    }

    #[test]
    fn test_api_key_detection_403_with_api_key_phrase() {
        let err = LarpshellError::from_http_status(
            reqwest::StatusCode::FORBIDDEN,
            "gemini",
            "API_KEY authentication failed",
        );
        assert!(matches!(err, LarpshellError::InvalidApiKey));
    }

    #[test]
    fn test_api_key_detection_403_with_apikey_phrase() {
        let err = LarpshellError::from_http_status(
            reqwest::StatusCode::FORBIDDEN,
            "ollama",
            "invalid apikey provided",
        );
        assert!(matches!(err, LarpshellError::InvalidApiKey));
    }

    #[test]
    fn test_api_key_detection_case_insensitive() {
        let err = LarpshellError::from_http_status(
            reqwest::StatusCode::UNAUTHORIZED,
            "provider",
            "API KEY required",
        );
        assert!(matches!(err, LarpshellError::InvalidApiKey));
    }

    #[test]
    fn test_quota_not_misclassified_as_invalid_key() {
        let err = LarpshellError::from_http_status(
            reqwest::StatusCode::FORBIDDEN,
            "openai",
            "quota exceeded for this model. check your API usage",
        );
        assert!(!matches!(err, LarpshellError::InvalidApiKey));
        assert!(matches!(err, LarpshellError::AuthenticationFailed { .. }));
    }

    #[test]
    fn test_capacity_not_misclassified_as_invalid_key() {
        let err = LarpshellError::from_http_status(
            reqwest::StatusCode::FORBIDDEN,
            "gemini",
            "capacity not available in your region",
        );
        assert!(!matches!(err, LarpshellError::InvalidApiKey));
        assert!(matches!(err, LarpshellError::AuthenticationFailed { .. }));
    }

    #[test]
    fn test_openapi_not_misclassified_as_invalid_key() {
        let err = LarpshellError::from_http_status(
            reqwest::StatusCode::FORBIDDEN,
            "provider",
            "openapi schema validation failed",
        );
        assert!(!matches!(err, LarpshellError::InvalidApiKey));
        assert!(matches!(err, LarpshellError::AuthenticationFailed { .. }));
    }

    #[test]
    fn test_retry_after_from_header_delta_seconds() {
        let err = LarpshellError::from_http_status_with_retry_header(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "openai",
            "rate limited",
            Some("120"),
        );
        assert!(matches!(
            err,
            LarpshellError::RateLimitExceeded {
                retry_after: Some(120)
            }
        ));
    }

    #[test]
    fn test_retry_after_from_body_gemini_format() {
        let err = LarpshellError::from_http_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "gemini",
            "please retry in 30s",
        );
        assert!(matches!(
            err,
            LarpshellError::RateLimitExceeded {
                retry_after: Some(30)
            }
        ));
    }

    #[test]
    fn test_retry_after_from_body_gemini_format_float() {
        let err = LarpshellError::from_http_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "gemini",
            "please retry in 45.5s",
        );
        assert!(matches!(
            err,
            LarpshellError::RateLimitExceeded {
                retry_after: Some(46)
            }
        ));
    }

    #[test]
    fn test_retry_after_header_preferred_over_body() {
        let err = LarpshellError::from_http_status_with_retry_header(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "openai",
            "please retry in 10s",
            Some("60"),
        );
        assert!(matches!(
            err,
            LarpshellError::RateLimitExceeded {
                retry_after: Some(60)
            }
        ));
    }

    #[test]
    fn test_retry_after_none_when_not_available() {
        let err = LarpshellError::from_http_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "provider",
            "rate limited",
        );
        assert!(matches!(
            err,
            LarpshellError::RateLimitExceeded { retry_after: None }
        ));
    }
}
