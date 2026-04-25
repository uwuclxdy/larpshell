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
    messages: Vec<RequestMessage>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
}

#[derive(Serialize)]
struct RequestMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<RequestToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct RequestToolCall {
    id: String,
    r#type: String,
    function: RequestToolCallFunction,
}

#[derive(Serialize)]
struct RequestToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct OpenAITool {
    r#type: String,
    function: OpenAIFunction,
}

#[derive(Serialize)]
struct OpenAIFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize)]
struct ChatResponseBody {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ResponseToolCall>>,
}

#[derive(Deserialize)]
struct ResponseToolCall {
    id: String,
    function: ResponseToolCallFunction,
}

#[derive(Deserialize)]
struct ResponseToolCallFunction {
    name: String,
    arguments: String,
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

    fn chat_completions_url(&self) -> String {
        let normalized = self.base_url.trim_end_matches('/');
        if normalized.ends_with("/v1") {
            format!("{normalized}/chat/completions")
        } else {
            format!("{normalized}/v1/chat/completions")
        }
    }

    async fn generate(&self, prompt: &str) -> Result<String, LarpshellError> {
        let url = self.chat_completions_url();

        let request_body = ChatRequest {
            model: self.model.clone(),
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: Some(prompt.to_string()),
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: DEFAULT_TEMPERATURE,
            tools: None,
        };

        let mut request = self.base.client.post(&url).json(&request_body);
        if let Some(ref api_key) = self.api_key {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }

        let response = BaseProvider::send_json(request, self.provider_slug).await?;

        let body: ChatResponseBody = response
            .json()
            .await
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        body.choices
            .first()
            .and_then(|choice| choice.message.content.clone())
            .ok_or_else(|| {
                LarpshellError::InvalidResponse(format!("no response from {}", self.provider_slug))
            })
    }

    async fn generate_with_tools(
        &self,
        messages: &[crate::providers::ChatMessage],
        tools: &[crate::providers::ToolDefinition],
    ) -> Result<crate::providers::ChatResponse, LarpshellError> {
        use crate::providers::{ChatResponse, Role};

        let url = self.chat_completions_url();

        let request_messages = messages
            .iter()
            .map(|message| RequestMessage {
                role: match message.role {
                    Role::System => "system".to_string(),
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                    Role::Tool => "tool".to_string(),
                },
                content: message.content.clone(),
                tool_calls: message.tool_calls.as_ref().map(|tool_calls| {
                    tool_calls
                        .iter()
                        .map(|tool_call| RequestToolCall {
                            id: tool_call.id.clone(),
                            r#type: "function".to_string(),
                            function: RequestToolCallFunction {
                                name: tool_call.name.clone(),
                                arguments: tool_call.arguments.to_string(),
                            },
                        })
                        .collect()
                }),
                tool_call_id: message.tool_call_id.clone(),
            })
            .collect();

        let openai_tools = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|tool| OpenAITool {
                        r#type: "function".to_string(),
                        function: OpenAIFunction {
                            name: tool.name.clone(),
                            description: tool.description.clone(),
                            parameters: tool.parameters.clone(),
                        },
                    })
                    .collect(),
            )
        };

        let request_body = ChatRequest {
            model: self.model.clone(),
            messages: request_messages,
            temperature: DEFAULT_TEMPERATURE,
            tools: openai_tools,
        };

        let mut request = self.base.client.post(&url).json(&request_body);
        if let Some(ref api_key) = self.api_key {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }

        let response = BaseProvider::send_json(request, self.provider_slug).await?;

        let body: ChatResponseBody = response
            .json()
            .await
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        let choice = body.choices.first().ok_or_else(|| {
            LarpshellError::InvalidResponse(format!("no response from {}", self.provider_slug))
        })?;

        if let Some(ref tool_calls) = choice.message.tool_calls
            && !tool_calls.is_empty()
        {
            let calls = tool_calls
                .iter()
                .map(|tool_call| {
                    let arguments =
                        serde_json::from_str(&tool_call.function.arguments).unwrap_or_default();
                    crate::providers::ToolCall {
                        id: tool_call.id.clone(),
                        name: tool_call.function.name.clone(),
                        arguments,
                        thought_signature: None,
                    }
                })
                .collect();
            return Ok(ChatResponse::ToolCalls(calls));
        }

        let content = choice.message.content.clone().ok_or_else(|| {
            LarpshellError::InvalidResponse(format!("no content from {}", self.provider_slug))
        })?;

        Ok(ChatResponse::Message(content))
    }

    fn name(&self) -> String {
        format!(
            "{} ({})",
            self.display_name,
            strip_url_for_display(&self.base_url)
        )
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

    async fn generate_with_tools(
        &self,
        messages: &[crate::providers::ChatMessage],
        tools: &[crate::providers::ToolDefinition],
    ) -> Result<crate::providers::ChatResponse, LarpshellError> {
        self.inner.generate_with_tools(messages, tools).await
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

    async fn generate_with_tools(
        &self,
        messages: &[crate::providers::ChatMessage],
        tools: &[crate::providers::ToolDefinition],
    ) -> Result<crate::providers::ChatResponse, LarpshellError> {
        self.inner.generate_with_tools(messages, tools).await
    }

    fn name(&self) -> String {
        self.inner.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ToolDefinition;

    #[test]
    fn chat_request_with_tools_serializes_correctly() {
        let tools = [ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" }
                },
                "required": ["file_path"]
            }),
        }];

        let openai_tools: Vec<OpenAITool> = tools
            .iter()
            .map(|tool| OpenAITool {
                r#type: "function".to_string(),
                function: OpenAIFunction {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: tool.parameters.clone(),
                },
            })
            .collect();

        let json = serde_json::to_value(&openai_tools).unwrap();
        assert_eq!(json[0]["type"], "function");
        assert_eq!(json[0]["function"]["name"], "read_file");
    }

    #[test]
    fn tool_call_response_deserializes() {
        let json = r#"{
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"file_path\":\"/tmp/test.txt\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;
        let response: ChatResponseBody = serde_json::from_str(json).unwrap();
        let choice = &response.choices[0];
        assert!(choice.message.tool_calls.is_some());
        let tool_call = &choice.message.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tool_call.id, "call_abc123");
        assert_eq!(tool_call.function.name, "read_file");
    }

    #[test]
    fn text_response_deserializes() {
        let json = r#"{
            "choices": [{
                "message": {
                    "content": "echo hello",
                    "tool_calls": null
                },
                "finish_reason": "stop"
            }]
        }"#;
        let response: ChatResponseBody = serde_json::from_str(json).unwrap();
        assert_eq!(
            response.choices[0].message.content.as_deref(),
            Some("echo hello")
        );
        assert!(response.choices[0].message.tool_calls.is_none());
    }
}
