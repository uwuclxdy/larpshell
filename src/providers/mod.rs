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

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Function name of the tool call this result answers. Carried internally so
    /// providers that match tool results by name (Gemini's `functionResponse.name`)
    /// can recover it; OpenAI/Ollama match by `tool_call_id` and ignore this.
    /// Skipped in serde so those providers' payloads stay byte-identical.
    #[serde(skip)]
    pub tool_call_name: Option<String>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: Some(content.into()), tool_calls: None, tool_call_id: None, tool_call_name: None }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: Some(content.into()), tool_calls: None, tool_call_id: None, tool_call_name: None }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, tool_call_name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            tool_call_name: Some(tool_call_name.into()),
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self { role: Role::Assistant, content: None, tool_calls: Some(tool_calls), tool_call_id: None, tool_call_name: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "function")]
    pub name: String,
    pub arguments: serde_json::Value,
    /// Provider-specific opaque token preserved across multi-turn conversations
    /// (currently used by Gemini's `thought_signature`). Skipped in serde since
    /// it's only carried internally between provider calls.
    #[serde(skip)]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

    /// Default implementation of `generate_with_tools` that falls back to single-shot text generation.
    ///
    /// **Limitation:** This default is single-shot only. It filters messages to extract only `User`
    /// and `System` roles, silently discarding all assistant turns, tool calls, and tool results.
    /// The resulting text is joined and passed to `generate()` without any tool definitions.
    ///
    /// Providers that support multi-turn tool use should override this method to preserve
    /// conversation state and pass tools to their LLM API.
    async fn generate_with_tools(&self, messages: &[ChatMessage], _tools: &[ToolDefinition]) -> Result<ChatResponse, LarpshellError> {
        let prompt = messages
            .iter()
            .filter(|message| message.role == Role::User || message.role == Role::System)
            .filter_map(|message| message.content.as_deref())
            .collect::<Vec<_>>()
            .join("\n\n");
        let result = self.generate(&prompt).await?;
        Ok(ChatResponse::Message(result))
    }
}

pub fn create_provider(config: &Config) -> Result<Box<dyn AIProvider>, LarpshellError> {
    match config.provider_config()? {
        ProviderSpecificConfig::Gemini(config) => Ok(Box::new(gemini::GeminiProvider::new(&config)?)),
        ProviderSpecificConfig::Ollama(config) => Ok(Box::new(ollama::OllamaProvider::new(&config)?)),
        ProviderSpecificConfig::OpenRouter(config) => Ok(Box::new(openai::OpenAICompatibleProvider::openrouter(&config)?)),
        ProviderSpecificConfig::OpenAI(config) => Ok(Box::new(openai::OpenAICompatibleProvider::openai(&config)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chat_message_user_sets_role_and_serializes_content() {
        let message = ChatMessage::user("hello");

        assert_eq!(message.role, Role::User);
        assert_eq!(message.content.as_deref(), Some("hello"));
        assert_eq!(message.tool_calls, None);
        assert_eq!(message.tool_call_id, None);
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            json!({
                "role": "user",
                "content": "hello"
            })
        );
    }

    #[test]
    fn chat_message_system_sets_role_and_serializes_content() {
        let message = ChatMessage::system("system prompt");

        assert_eq!(message.role, Role::System);
        assert_eq!(message.content.as_deref(), Some("system prompt"));
        assert_eq!(message.tool_calls, None);
        assert_eq!(message.tool_call_id, None);
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            json!({
                "role": "system",
                "content": "system prompt"
            })
        );
    }

    #[test]
    fn chat_message_tool_result_sets_tool_metadata_and_serializes_content() {
        let message = ChatMessage::tool_result("call-1", "search", "done");

        assert_eq!(message.role, Role::Tool);
        assert_eq!(message.content.as_deref(), Some("done"));
        assert_eq!(message.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(message.tool_call_name.as_deref(), Some("search"));
        assert_eq!(message.tool_calls, None);
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            json!({
                "role": "tool",
                "content": "done",
                "tool_call_id": "call-1"
            })
        );
    }

    #[test]
    fn chat_message_assistant_tool_calls_sets_calls_and_skips_content() {
        let tool_calls = vec![ToolCall {
            id: String::from("call-1"),
            name: String::from("search"),
            arguments: json!({ "query": "rust" }),
            thought_signature: None,
        }];
        let message = ChatMessage::assistant_tool_calls(tool_calls.clone());

        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content, None);
        assert_eq!(message.tool_calls, Some(tool_calls));
        assert_eq!(message.tool_call_id, None);
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            json!({
                "role": "assistant",
                "tool_calls": [{
                    "id": "call-1",
                    "function": "search",
                    "arguments": { "query": "rust" }
                }]
            })
        );
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
    fn tool_call_serializes_function_name_and_json_arguments() {
        let tool_call = ToolCall {
            id: String::from("call-1"),
            name: String::from("search"),
            arguments: json!({ "query": "rust" }),
            thought_signature: None,
        };

        assert_eq!(
            serde_json::to_value(&tool_call).unwrap(),
            json!({
                "id": "call-1",
                "function": "search",
                "arguments": { "query": "rust" }
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
        let tool_calls =
            vec![ToolCall { id: String::from("call-1"), name: String::from("search"), arguments: json!({}), thought_signature: None }];
        let response = ChatResponse::ToolCalls(tool_calls.clone());

        assert_eq!(response, ChatResponse::ToolCalls(tool_calls));
    }

    #[test]
    fn default_generate_with_tools_extracts_user_messages() {
        let messages = [
            ChatMessage::system("you are helpful"),
            ChatMessage::user("list files"),
            ChatMessage::tool_result("tc_1", "list_files", "file1.txt"),
        ];

        let prompt: String = messages
            .iter()
            .filter(|message| message.role == Role::User || message.role == Role::System)
            .filter_map(|message| message.content.as_deref())
            .collect::<Vec<_>>()
            .join("\n\n");

        assert_eq!(prompt, "you are helpful\n\nlist files");
    }
}
