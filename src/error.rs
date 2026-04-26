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

    #[error("rate limit exceeded{}", retry_after.map_or("; please try again later".to_string(), |n| format!("; retry after {n} seconds")))]
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

    #[error("no command provided.")]
    NoCommandProvided,

    #[error("failed to generate a valid explanation.")]
    EmptyExplanation,

    #[error("no API provider configured.")]
    NoProviderConfigured,

    #[error("unknown command '{0}'")]
    UnknownSlashCommand(String),

    #[error("invalid argument for /{command}: expected {expected}")]
    InvalidSlashArg { command: String, expected: String },

    #[error("expected command after '!'")]
    ExpectedCommandAfterBang,
}

impl LarpshellError {
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
        match status.as_u16() {
            401 | 403 => {
                if body.contains("key") || body.contains("api") || body.contains("token") {
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
                let retry_after = if body.contains("retry") {
                    body.split("retry in ")
                        .nth(1)
                        .and_then(|s| s.split('s').next())
                        .and_then(|s| s.parse::<f64>().ok())
                        .map(f64::ceil)
                        .map(|seconds| seconds as u64)
                } else {
                    None
                };
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
