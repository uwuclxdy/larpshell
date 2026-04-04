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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
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
        assert_eq!(message.content, "hello");
        assert_eq!(message.tool_calls, None);
        assert_eq!(message.tool_call_id, None);
    }

    #[test]
    fn chat_message_system_sets_role_and_content() {
        let message = ChatMessage::system("system prompt");

        assert_eq!(message.role, Role::System);
        assert_eq!(message.content, "system prompt");
        assert_eq!(message.tool_calls, None);
        assert_eq!(message.tool_call_id, None);
    }

    #[test]
    fn chat_message_tool_result_sets_tool_metadata() {
        let message = ChatMessage::tool_result("call-1", "done");

        assert_eq!(message.role, Role::Tool);
        assert_eq!(message.content, "done");
        assert_eq!(message.tool_call_id, Some(String::from("call-1")));
        assert_eq!(message.tool_calls, None);
    }

    #[test]
    fn chat_message_assistant_tool_calls_sets_calls() {
        let tool_calls = vec![ToolCall {
            id: String::from("call-1"),
            name: String::from("search"),
            arguments: String::from("{\"query\":\"rust\"}"),
        }];
        let message = ChatMessage::assistant_tool_calls(tool_calls.clone());

        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content, "");
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

        match response {
            ChatResponse::Message(content) => assert_eq!(content, "hello"),
            ChatResponse::ToolCalls(_) => panic!("expected message response"),
        }
    }

    #[test]
    fn chat_response_tool_calls_variant_contains_calls() {
        let response = ChatResponse::ToolCalls(vec![ToolCall {
            id: String::from("call-1"),
            name: String::from("search"),
            arguments: String::from("{}"),
        }]);

        match response {
            ChatResponse::ToolCalls(tool_calls) => {
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].name, "search");
            }
            ChatResponse::Message(_) => panic!("expected tool call response"),
        }
    }
}
