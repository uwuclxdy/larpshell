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

#[derive(Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaTool>>,
}

#[derive(Serialize)]
struct OllamaChatMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Serialize)]
struct OllamaTool {
    r#type: String,
    function: OllamaToolFunction,
}

#[derive(Serialize)]
struct OllamaToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct OllamaToolCall {
    function: OllamaToolCallFunction,
}

#[derive(Serialize, Deserialize)]
struct OllamaToolCallFunction {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: OllamaChatResponseMessage,
}

#[derive(Deserialize)]
struct OllamaChatResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OllamaToolCall>>,
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

        let request = self.base.client.post(&url).json(&request_body);

        let response = BaseProvider::send_json(request, "ollama").await?;

        let ollama_response: OllamaResponse = response
            .json()
            .await
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        Ok(ollama_response.response)
    }

    async fn generate_with_tools(
        &self,
        messages: &[crate::providers::ChatMessage],
        tools: &[crate::providers::ToolDefinition],
    ) -> Result<crate::providers::ChatResponse, LarpshellError> {
        use crate::providers::ChatResponse;

        let url = format!("{}/api/chat", self.base_url);

        let ollama_messages: Vec<OllamaChatMessage> = messages
            .iter()
            .map(|message| OllamaChatMessage {
                role: message.role.as_str(),
                content: message.content.clone(),
                tool_calls: message.tool_calls.as_ref().map(|tool_calls| {
                    tool_calls
                        .iter()
                        .map(|tool_call| OllamaToolCall {
                            function: OllamaToolCallFunction {
                                name: tool_call.name.clone(),
                                arguments: tool_call.arguments.clone(),
                            },
                        })
                        .collect()
                }),
            })
            .collect();

        let ollama_tools = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|tool| OllamaTool {
                        r#type: "function".to_string(),
                        function: OllamaToolFunction {
                            name: tool.name.clone(),
                            description: tool.description.clone(),
                            parameters: tool.parameters.clone(),
                        },
                    })
                    .collect(),
            )
        };

        let request_body = OllamaChatRequest {
            model: self.model.clone(),
            messages: ollama_messages,
            stream: false,
            tools: ollama_tools,
        };

        let request = self.base.client.post(&url).json(&request_body);

        let response = BaseProvider::send_json(request, "ollama").await?;

        let chat_response: OllamaChatResponse = response
            .json()
            .await
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        if let Some(tool_calls) = &chat_response.message.tool_calls
            && !tool_calls.is_empty()
        {
            let calls = tool_calls
                .iter()
                .enumerate()
                .map(|(index, tool_call)| crate::providers::ToolCall {
                    id: format!("ollama_tc_{index}"),
                    name: tool_call.function.name.clone(),
                    arguments: tool_call.function.arguments.clone(),
                    thought_signature: None,
                })
                .collect();
            return Ok(ChatResponse::ToolCalls(calls));
        }

        let content = chat_response
            .message
            .content
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                LarpshellError::InvalidResponse("no content in ollama response".to_string())
            })?;

        Ok(ChatResponse::Message(content))
    }

    fn name(&self) -> String {
        format!("Ollama ({})", strip_url_for_display(&self.base_url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_chat_response_with_tool_call_deserializes() {
        let json = r#"{
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {
                        "name": "read_file",
                        "arguments": {
                            "file_path": "/tmp/test.txt"
                        }
                    }
                }]
            }
        }"#;

        let response: OllamaChatResponse = serde_json::from_str(json).unwrap();
        let tool_calls = response.message.tool_calls.unwrap();
        assert_eq!(tool_calls[0].function.name, "read_file");
        assert_eq!(
            tool_calls[0].function.arguments["file_path"],
            "/tmp/test.txt"
        );
    }

    #[test]
    fn ollama_chat_response_text_only_deserializes() {
        let json = r#"{
            "message": {
                "role": "assistant",
                "content": "echo hello"
            }
        }"#;

        let response: OllamaChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.message.content.as_deref(), Some("echo hello"));
        assert!(response.message.tool_calls.is_none());
    }

    #[test]
    fn ollama_missing_content_is_none() {
        let json = r#"{"message": {"role": "assistant"}}"#;
        let response: OllamaChatResponse = serde_json::from_str(json).unwrap();
        // content absent → None; the provider must error, not return empty string
        assert!(response.message.content.is_none());
    }

    #[test]
    fn ollama_empty_content_filtered_to_none() {
        let json = r#"{"message": {"role": "assistant", "content": ""}}"#;
        let response: OllamaChatResponse = serde_json::from_str(json).unwrap();
        // filter(|s| !s.is_empty()) turns "" into None → error path
        assert!(response.message.content.filter(|s| !s.is_empty()).is_none());
    }
}
