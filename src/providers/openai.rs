use crate::config::OpenAIConfig;
use crate::error::LarpshellError;
use crate::providers::AIProvider;
use crate::providers::base::{BaseProvider, strip_url_for_display};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const DEFAULT_TEMPERATURE: f32 = 0.7;

pub struct OpenAIProvider {
    base: BaseProvider,
    base_url: String,
    api_key: Option<String>,
    model: String,
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

impl OpenAIProvider {
    pub fn new(config: &OpenAIConfig) -> Result<Self, LarpshellError> {
        Ok(Self {
            base: BaseProvider::new()?,
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        })
    }
}

#[async_trait]
impl AIProvider for OpenAIProvider {
    async fn generate(&self, prompt: &str) -> Result<String, LarpshellError> {
        let url = if self.base_url.ends_with("/v1") {
            format!("{}/chat/completions", self.base_url)
        } else {
            format!("{}/v1/chat/completions", self.base_url)
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
            .map_err(|e| LarpshellError::from_reqwest(e, "openai"))?;

        let response = BaseProvider::check_response(response, "openai").await?;

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        let content = chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| {
                LarpshellError::InvalidResponse("no response from openai".to_string())
            })?;

        Ok(content)
    }
    fn name(&self) -> String {
        format!("OpenAI ({})", strip_url_for_display(&self.base_url))
    }
}
