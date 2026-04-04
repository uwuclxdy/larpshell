mod base;
mod gemini;
mod ollama;
mod openai;

use crate::config::{Config, ProviderSpecificConfig};
use crate::error::LarpshellError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatResponse {
    Message(String),
    ToolCalls(Vec<ToolCall>),
}

#[async_trait]
pub trait AIProvider: Send + Sync {
    async fn generate(&self, prompt: &str) -> Result<String, LarpshellError>;
    fn name(&self) -> String;
}

pub fn create_provider(config: &Config) -> Result<Box<dyn AIProvider>, LarpshellError> {
    let provider = config.get_provider_config()?;
    match &provider.config {
        ProviderSpecificConfig::Gemini { gemini } => {
            Ok(Box::new(gemini::GeminiProvider::new(gemini)?))
        }
        ProviderSpecificConfig::Ollama { ollama } => {
            Ok(Box::new(ollama::OllamaProvider::new(ollama)?))
        }
        ProviderSpecificConfig::OpenRouter { openrouter } => {
            Ok(Box::new(openai::OpenRouterProvider::new(openrouter)?))
        }
        ProviderSpecificConfig::OpenAI { openai } => {
            Ok(Box::new(openai::OpenAIProvider::new(openai)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chat_message_user_sets_role_and_content() {
        let message = ChatMessage::user("hello");

        assert_eq!(message.role, Role::User);
        assert_eq!(message.content.as_deref(), Some("hello"));
        assert_eq!(message.tool_calls, None);
        assert_eq!(message.tool_call_id, None);
    }

    #[test]
    fn chat_message_system_sets_role_and_content() {
        let message = ChatMessage::system("system prompt");

        assert_eq!(message.role, Role::System);
        assert_eq!(message.content.as_deref(), Some("system prompt"));
        assert_eq!(message.tool_calls, None);
        assert_eq!(message.tool_call_id, None);
    }

    #[test]
    fn chat_message_tool_result_sets_tool_metadata() {
        let message = ChatMessage::tool_result("call-1", "done");

        assert_eq!(message.role, Role::Tool);
        assert_eq!(message.content.as_deref(), Some("done"));
        assert_eq!(message.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(message.tool_calls, None);
    }

    #[test]
    fn chat_message_assistant_tool_calls_sets_calls() {
        let tool_calls = vec![ToolCall {
            id: String::from("call-1"),
            name: String::from("search"),
            arguments: json!({ "query": "rust" }),
        }];
        let message = ChatMessage::assistant_tool_calls(tool_calls.clone());

        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content, None);
        assert_eq!(message.tool_calls, Some(tool_calls));
        assert_eq!(message.tool_call_id, None);
    }

    #[test]
    fn tool_definition_serializes_expected_fields() {
        let definition = ToolDefinition {
            name: String::from("search"),
            description: String::from("Search the web"),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                }
            }),
        };

        assert_eq!(
            serde_json::to_value(&definition).unwrap(),
            json!({
                "name": "search",
                "description": "Search the web",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    }
                }
            })
        );
    }

    #[test]
    fn chat_response_message_variant_contains_text() {
        let response = ChatResponse::Message(String::from("hello"));

        assert_eq!(response, ChatResponse::Message(String::from("hello")));
    }

    #[test]
    fn chat_response_tool_calls_variant_contains_calls() {
        let tool_calls = vec![ToolCall {
            id: String::from("call-1"),
            name: String::from("search"),
            arguments: json!({}),
        }];
        let response = ChatResponse::ToolCalls(tool_calls.clone());

        assert_eq!(response, ChatResponse::ToolCalls(tool_calls));
    }
}
