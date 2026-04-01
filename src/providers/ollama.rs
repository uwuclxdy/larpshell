use crate::config::OllamaConfig;
use crate::error::LarpshellError;
use crate::providers::AIProvider;
use crate::providers::base::{BaseProvider, strip_url_for_display};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub struct OllamaProvider {
    base: BaseProvider,
    base_url: String,
    model: String,
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

impl OllamaProvider {
    pub fn new(config: &OllamaConfig) -> Result<Self, LarpshellError> {
        Ok(Self {
            base: BaseProvider::new()?,
            base_url: config.base_url.clone(),
            model: config.model.clone(),
        })
    }
}

#[async_trait]
impl AIProvider for OllamaProvider {
    async fn generate(&self, prompt: &str) -> Result<String, LarpshellError> {
        let url = format!("{}/api/generate", self.base_url);

        let request_body = OllamaRequest {
            model: self.model.clone(),
            prompt: prompt.to_string(),
            stream: false,
        };

        let response = self
            .base
            .client
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| LarpshellError::from_reqwest(e, "ollama"))?;

        let response = BaseProvider::check_response(response, "ollama").await?;

        let ollama_response: OllamaResponse = response
            .json()
            .await
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        Ok(ollama_response.response)
    }
    fn name(&self) -> String {
        format!("Ollama ({})", strip_url_for_display(&self.base_url))
    }
}
