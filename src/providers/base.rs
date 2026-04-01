use crate::common::DEFAULT_PROVIDER_TIMEOUT_SECS;
use crate::error::LarpshellError;
use reqwest::Client;
use std::time::Duration;

/// creates a new HTTP client with default timeout.
///
/// this ensures consistent timeout behavior across all providers.
pub fn create_http_client() -> Result<Client, LarpshellError> {
    Client::builder()
        .timeout(Duration::from_secs(DEFAULT_PROVIDER_TIMEOUT_SECS))
        .build()
        .map_err(|e| LarpshellError::ConfigError(e.to_string()))
}

/// strips scheme prefix and trailing slashes from a URL for display purposes.
pub(crate) fn strip_url_for_display(url: &str) -> &str {
    url.trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches("/*")
        .trim_end_matches('/')
}

/// base provider struct containing common HTTP client.
///
/// this reduces duplication across providers by centralizing
/// the client creation and timeout configuration.
pub struct BaseProvider {
    pub(crate) client: Client,
}

impl BaseProvider {
    /// creates a new base provider with HTTP client.
    pub fn new() -> Result<Self, LarpshellError> {
        Ok(Self {
            client: create_http_client()?,
        })
    }

    /// checks an HTTP response status and returns an appropriate error for non-success codes.
    pub async fn check_response(
        response: reqwest::Response,
        provider: &str,
    ) -> Result<reqwest::Response, LarpshellError> {
        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(LarpshellError::from_http_status(
                status,
                provider,
                &error_text,
            ));
        }
        Ok(response)
    }
}
