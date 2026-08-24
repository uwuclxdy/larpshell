use crate::common::DEFAULT_PROVIDER_TIMEOUT_SECS;
use crate::config::{GeminiConfig, OllamaConfig, OpenAIConfig, OpenRouterConfig};
use crate::error::LarpshellError;
use crate::providers::{AIProvider, ChatMessage, ChatResponse, Role, ToolCall, ToolDefinition};
use async_trait::async_trait;
use genai::adapter::AdapterKind;
use genai::chat::{
    ChatMessage as GenaiChatMessage, ChatRequest, ContentPart, MessageContent, Tool, ToolCall as GenaiToolCall, ToolName, ToolResponse,
};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ServiceTarget};
use std::time::Duration;

/// All four provider kinds ride the genai adapters; the only per-kind state is
/// the adapter selection, the base URL, and the display label.
pub struct GenaiProvider {
    client: Client,
    model: String,
    provider_slug: &'static str,
    /// Display suffix: the model for Gemini, the host for URL-based kinds.
    display_suffix: String,
    display_name: &'static str,
}

impl GenaiProvider {
    pub fn gemini(config: &GeminiConfig) -> Result<Self, LarpshellError> {
        let client = build_client(AdapterKind::Gemini, None, Some(&config.api_key))?;
        Ok(Self {
            client,
            model: config.model.clone(),
            provider_slug: "gemini",
            display_suffix: config.model.clone(),
            display_name: "Gemini",
        })
    }

    pub fn ollama(config: &OllamaConfig) -> Result<Self, LarpshellError> {
        // The ollama adapter concatenates `{base_url}api/chat` without a
        // separator, so a bare-host base URL must carry its trailing slash.
        let base_url = format!("{}/", config.base_url.trim_end_matches('/'));
        let client = build_client(AdapterKind::Ollama, Some(&base_url), None)?;
        Ok(Self {
            client,
            model: config.model.clone(),
            provider_slug: "ollama",
            display_suffix: strip_url_for_display(&config.base_url).to_string(),
            display_name: "Ollama",
        })
    }

    pub fn openrouter(config: &OpenRouterConfig) -> Result<Self, LarpshellError> {
        let client = build_client(AdapterKind::OpenRouter, Some(&config.base_url), config.api_key.as_deref())?;
        Ok(Self {
            client,
            model: config.model.clone(),
            provider_slug: "openrouter",
            display_suffix: strip_url_for_display(&config.base_url).to_string(),
            display_name: "OpenRouter",
        })
    }

    pub fn openai(config: &OpenAIConfig) -> Result<Self, LarpshellError> {
        let client = build_client(AdapterKind::OpenAI, Some(&config.base_url), config.api_key.as_deref())?;
        Ok(Self {
            client,
            model: config.model.clone(),
            provider_slug: "openai",
            display_suffix: strip_url_for_display(&config.base_url).to_string(),
            display_name: "OpenAI Compatible",
        })
    }

    fn chat_request(messages: &[ChatMessage], tools: &[ToolDefinition]) -> ChatRequest {
        let mut system_parts: Vec<String> = Vec::new();
        let mut genai_messages: Vec<GenaiChatMessage> = Vec::new();

        for message in messages {
            match message.role {
                Role::System => {
                    if let Some(content) = message.content.as_deref() {
                        system_parts.push(content.to_string());
                    }
                }
                Role::User => {
                    genai_messages.push(GenaiChatMessage::user(message.content.clone().unwrap_or_default()));
                }
                Role::Assistant => {
                    if let Some(tool_calls) = message.tool_calls.as_ref() {
                        // Thought signatures (Gemini thinking models) travel in
                        // the tool call and must be re-emitted before the calls
                        // on the next turn.
                        let genai_calls: Vec<GenaiToolCall> = tool_calls
                            .iter()
                            .map(|tool_call| GenaiToolCall {
                                call_id: tool_call.id.clone(),
                                fn_name: tool_call.name.clone(),
                                fn_arguments: tool_call.arguments.clone(),
                                thought_signatures: None,
                            })
                            .collect();
                        let signatures: Vec<String> = tool_calls.iter().filter_map(|call| call.thought_signature.clone()).collect();
                        genai_messages.push(GenaiChatMessage::assistant_tool_calls_with_thoughts(genai_calls, signatures));
                    } else {
                        genai_messages.push(GenaiChatMessage::assistant(message.content.clone().unwrap_or_default()));
                    }
                }
                Role::Tool => {
                    genai_messages.push(GenaiChatMessage::tool(MessageContent::from_parts(vec![ContentPart::ToolResponse(
                        ToolResponse {
                            call_id: message.tool_call_id.clone().unwrap_or_default(),
                            // Gemini matches tool results by function name; the
                            // other adapters correlate by call id and ignore it.
                            fn_name: message.tool_call_name.clone(),
                            content: message.content.clone().unwrap_or_default(),
                        },
                    )])));
                }
            }
        }

        let genai_tools = tools
            .iter()
            .map(|tool| Tool {
                name: ToolName::Custom(tool.name.clone()),
                description: Some(tool.description.clone()),
                schema: Some(tool.parameters.clone()),
                strict: None,
                config: None,
            })
            .collect::<Vec<_>>();

        ChatRequest {
            system: (!system_parts.is_empty()).then(|| system_parts.join("\n")),
            messages: genai_messages,
            tools: (!genai_tools.is_empty()).then_some(genai_tools),
            previous_response_id: None,
            store: None,
        }
    }
}

#[async_trait]
impl AIProvider for GenaiProvider {
    async fn generate(&self, prompt: &str) -> Result<String, LarpshellError> {
        let response = self
            .client
            .exec_chat(&self.model, ChatRequest::from_user(prompt), None)
            .await
            .map_err(|error| map_genai_error(error, self.provider_slug))?;

        let text = response.content.texts().join("");
        if text.is_empty() {
            return Err(LarpshellError::InvalidResponse(format!("no response from {}", self.provider_slug)));
        }
        Ok(text)
    }

    async fn generate_with_tools(&self, messages: &[ChatMessage], tools: &[ToolDefinition]) -> Result<ChatResponse, LarpshellError> {
        let request = Self::chat_request(messages, tools);
        let response =
            self.client.exec_chat(&self.model, request, None).await.map_err(|error| map_genai_error(error, self.provider_slug))?;

        let tool_calls: Vec<ToolCall> = response
            .content
            .tool_calls()
            .iter()
            .map(|tool_call| ToolCall {
                id: tool_call.call_id.clone(),
                name: tool_call.fn_name.clone(),
                arguments: tool_call.fn_arguments.clone(),
                thought_signature: tool_call.thought_signatures.as_ref().and_then(|signatures| signatures.first().cloned()),
            })
            .collect();

        if !tool_calls.is_empty() {
            return Ok(ChatResponse::ToolCalls(tool_calls));
        }

        let text = response.content.texts().join("");
        if text.is_empty() {
            return Err(LarpshellError::InvalidResponse(format!("no content from {}", self.provider_slug)));
        }
        Ok(ChatResponse::Message(text))
    }

    fn name(&self) -> String {
        format!("{} ({})", self.display_name, self.display_suffix)
    }
}

/// One reqwest client shared across adapters; `with_reqwest` injects it into
/// genai so the provider timeout applies to every call.
fn build_client(kind: AdapterKind, base_url: Option<&str>, api_key: Option<&str>) -> Result<Client, LarpshellError> {
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(DEFAULT_PROVIDER_TIMEOUT_SECS))
        .build()
        .map_err(|error| LarpshellError::ConfigError(error.to_string()))?;

    let base = base_url.map(str::to_string);
    // An empty key is meaningful: it sends `Bearer ` with no credential, which
    // keyless local servers (LM Studio) accept. Never resolve to `None`, which
    // genai turns into a missing-key error before any request leaves.
    let key = api_key.unwrap_or_default().to_string();

    let mut builder = Client::builder().with_adapter_kind(kind).with_reqwest(http_client);
    if let Some(base) = base {
        builder = builder.with_service_target_resolver_fn(
            move |mut target: ServiceTarget| -> std::result::Result<ServiceTarget, genai::resolver::Error> {
                target.endpoint = Endpoint::from_owned(base.clone());
                Ok(target)
            },
        );
    }
    builder = builder.with_auth_resolver_fn(move |_model: ModelIden| -> std::result::Result<Option<AuthData>, genai::resolver::Error> {
        Ok(Some(AuthData::from_single(key.clone())))
    });

    Ok(builder.build())
}

fn map_genai_error(error: genai::Error, provider: &str) -> LarpshellError {
    match error {
        genai::Error::HttpError { status, body, .. } => LarpshellError::from_http_status_with_retry_header(status, provider, &body, None),
        genai::Error::WebAdapterCall { webc_error, .. } | genai::Error::WebModelCall { webc_error, .. } => match webc_error {
            genai::webc::Error::Reqwest(reqwest_error) => LarpshellError::from_reqwest(&reqwest_error, provider),
            other => LarpshellError::InvalidResponse(format!("{provider} error: {other}")),
        },
        other => LarpshellError::InvalidResponse(format!("{provider} error: {other}")),
    }
}

/// Strips scheme prefix and trailing slashes from a URL for display purposes.
fn strip_url_for_display(url: &str) -> &str {
    url.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderSpecificConfig;

    #[test]
    fn chat_request_maps_tool_call_and_result_messages() {
        let messages = vec![
            ChatMessage::system("use tools".to_string()),
            ChatMessage::user("go".to_string()),
            ChatMessage::assistant_tool_calls(vec![ToolCall {
                id: "call-1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({ "query": "rust" }),
                thought_signature: None,
            }]),
            ChatMessage::tool_result("call-1", "search", "found"),
        ];
        let tools = [ToolDefinition {
            name: "search".to_string(),
            description: "search".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }];

        let request = GenaiProvider::chat_request(&messages, &tools);

        assert_eq!(request.system.as_deref(), Some("use tools"));
        assert_eq!(request.messages.len(), 3);
        let tools = request.tools.expect("tools should be attached");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].schema.as_ref().unwrap()["type"], "object");
    }

    #[test]
    fn chat_request_carries_thought_signatures_before_tool_calls() {
        let messages = vec![ChatMessage::assistant_tool_calls(vec![ToolCall {
            id: "call-1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({}),
            thought_signature: Some("sig-1".to_string()),
        }])];

        let request = GenaiProvider::chat_request(&messages, &[]);
        let message = &request.messages[0];
        let parts = message.content.parts();
        assert!(matches!(parts[0], ContentPart::ThoughtSignature(_)));
        assert!(matches!(parts[1], ContentPart::ToolCall(_)));
    }

    #[test]
    fn strip_url_for_display_strips_scheme_and_slashes() {
        assert_eq!(strip_url_for_display("http://localhost:11434/"), "localhost:11434");
        assert_eq!(strip_url_for_display("https://api.openai.com/v1"), "api.openai.com/v1");
    }

    #[test]
    fn provider_kind_mapping_covers_all_four_kinds() {
        // Guards the ProviderSpecificConfig -> constructor match in
        // providers/mod.rs: every kind must have a GenaiProvider constructor.
        let gemini = ProviderSpecificConfig::Gemini(GeminiConfig { api_key: "k".into(), model: "m".into() });
        let ollama = ProviderSpecificConfig::Ollama(OllamaConfig { base_url: "http://localhost:11434".into(), model: "m".into() });
        let openrouter =
            ProviderSpecificConfig::OpenRouter(OpenRouterConfig { base_url: "https://x/v1".into(), api_key: None, model: "m".into() });
        let openai = ProviderSpecificConfig::OpenAI(OpenAIConfig { base_url: "https://x/v1".into(), api_key: None, model: "m".into() });

        match gemini {
            ProviderSpecificConfig::Gemini(config) => assert!(GenaiProvider::gemini(&config).is_ok()),
            _ => unreachable!(),
        }
        match ollama {
            ProviderSpecificConfig::Ollama(config) => assert!(GenaiProvider::ollama(&config).is_ok()),
            _ => unreachable!(),
        }
        match openrouter {
            ProviderSpecificConfig::OpenRouter(config) => assert!(GenaiProvider::openrouter(&config).is_ok()),
            _ => unreachable!(),
        }
        match openai {
            ProviderSpecificConfig::OpenAI(config) => assert!(GenaiProvider::openai(&config).is_ok()),
            _ => unreachable!(),
        }
    }
}
