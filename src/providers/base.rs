use crate::common::DEFAULT_PROVIDER_TIMEOUT_SECS;
use crate::error::LarpshellError;
use reqwest::Client;
use std::time::Duration;

/// Creates a new HTTP client with default timeout.
///
/// Ensures consistent timeout behavior across all providers.
pub fn create_http_client() -> Result<Client, LarpshellError> {
    Client::builder()
        .timeout(Duration::from_secs(DEFAULT_PROVIDER_TIMEOUT_SECS))
        .build()
        .map_err(|e| LarpshellError::ConfigError(e.to_string()))
}

/// Strips scheme prefix and trailing slashes from a URL for display purposes.
pub fn strip_url_for_display(url: &str) -> &str {
    url.trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
}

/// Base provider struct containing a shared HTTP client.
///
/// Reduces duplication across providers by centralizing
/// client creation and timeout configuration.
pub struct BaseProvider {
    pub(crate) client: Client,
}

impl BaseProvider {
    /// Creates a new base provider with an HTTP client.
    pub fn new() -> Result<Self, LarpshellError> {
        Ok(Self {
            client: create_http_client()?,
        })
    }

    /// Sends an HTTP request and checks the response status.
    ///
    /// Combines the common pattern of sending a request, handling reqwest errors,
    /// and validating the HTTP status code.
    pub async fn send_json(
        request: reqwest::RequestBuilder,
        provider: &str,
    ) -> Result<reqwest::Response, LarpshellError> {
        let response = request
            .send()
            .await
            .map_err(|e| LarpshellError::from_reqwest(&e, provider))?;
        Self::check_response(response, provider).await
    }

    /// Checks an HTTP response status and returns an appropriate error for non-success codes.
    pub async fn check_response(
        response: reqwest::Response,
        provider: &str,
    ) -> Result<reqwest::Response, LarpshellError> {
        if !response.status().is_success() {
            let status = response.status();
            let retry_after_header = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(LarpshellError::from_http_status_with_retry_header(
                status,
                provider,
                &error_text,
                retry_after_header.as_deref(),
            ));
        }
        Ok(response)
    }
}
