use crate::config::{OpenAIConfig, OpenRouterConfig};
use crate::error::LarpshellError;
use crate::providers::AIProvider;
use crate::providers::base::{BaseProvider, strip_url_for_display};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const DEFAULT_TEMPERATURE: f32 = 0.7;

pub struct OpenAICompatibleProvider {
    base: BaseProvider,
    base_url: String,
    api_key: Option<String>,
    model: String,
    provider_slug: &'static str,
    display_name: &'static str,
}

pub struct OpenAIProvider {
    inner: OpenAICompatibleProvider,
}

pub struct OpenRouterProvider {
    inner: OpenAICompatibleProvider,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    temperature: f32,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: MessageResponse,
}

#[derive(Deserialize)]
struct MessageResponse {
    content: String,
}

impl OpenAICompatibleProvider {
    fn new(
        base_url: String,
        api_key: Option<String>,
        model: String,
        provider_slug: &'static str,
        display_name: &'static str,
    ) -> Result<Self, LarpshellError> {
        Ok(Self {
            base: BaseProvider::new()?,
            base_url,
            api_key,
            model,
            provider_slug,
            display_name,
        })
    }

    async fn generate(&self, prompt: &str) -> Result<String, LarpshellError> {
        let normalized_base_url = self.base_url.trim_end_matches('/');
        let url = if normalized_base_url.ends_with("/v1") {
            format!("{}/chat/completions", normalized_base_url)
        } else {
            format!("{}/v1/chat/completions", normalized_base_url)
        };

        let request_body = ChatRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            temperature: DEFAULT_TEMPERATURE,
        };

        let mut request = self.base.client.post(&url).json(&request_body);
        if let Some(ref api_key) = self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .send()
            .await
            .map_err(|e| LarpshellError::from_reqwest(e, self.provider_slug))?;

        let response = BaseProvider::check_response(response, self.provider_slug).await?;

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        let content = chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| {
                LarpshellError::InvalidResponse(format!("no response from {}", self.provider_slug))
            })?;

        Ok(content)
    }

    fn name(&self) -> String {
        format!("{} ({})", self.display_name, strip_url_for_display(&self.base_url))
    }
}

impl OpenAIProvider {
    pub fn new(config: &OpenAIConfig) -> Result<Self, LarpshellError> {
        Ok(Self {
            inner: OpenAICompatibleProvider::new(
                config.base_url.clone(),
                config.api_key.clone(),
                config.model.clone(),
                "openai",
                "OpenAI",
            )?,
        })
    }
}

impl OpenRouterProvider {
    pub fn new(config: &OpenRouterConfig) -> Result<Self, LarpshellError> {
        Ok(Self {
            inner: OpenAICompatibleProvider::new(
                config.base_url.clone(),
                config.api_key.clone(),
                config.model.clone(),
                "openrouter",
                "OpenRouter",
            )?,
        })
    }
}

#[async_trait]
impl AIProvider for OpenAIProvider {
    async fn generate(&self, prompt: &str) -> Result<String, LarpshellError> {
        self.inner.generate(prompt).await
    }

    fn name(&self) -> String {
        self.inner.name()
    }
}

#[async_trait]
impl AIProvider for OpenRouterProvider {
    async fn generate(&self, prompt: &str) -> Result<String, LarpshellError> {
        self.inner.generate(prompt).await
    }

    fn name(&self) -> String {
        self.inner.name()
    }
}
