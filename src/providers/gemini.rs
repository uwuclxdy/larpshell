use crate::config::GeminiConfig;
use crate::error::LarpshellError;
use crate::providers::base::BaseProvider;
use crate::providers::{AIProvider, ChatMessage, ChatResponse, Role, ToolCall, ToolDefinition};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";

pub struct GeminiProvider {
    base: BaseProvider,
    api_key: String,
    model: String,
}

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<Content>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiToolDeclaration>>,
}

#[derive(Serialize, Deserialize)]
struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<Part>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<FunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_response: Option<FunctionResponse>,
    /// Opaque token from the model. Must be echoed back unchanged on the
    /// next turn alongside the functionCall, or the API rejects the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct FunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct FunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolDeclaration {
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<Candidate>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    content: Option<ContentResponse>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ContentResponse {
    parts: Option<Vec<Part>>,
}

#[derive(Deserialize)]
struct GeminiErrorResponse {
    error: GeminiErrorDetail,
}

#[derive(Deserialize)]
struct GeminiErrorDetail {
    code: u16,
    message: String,
}

impl GeminiProvider {
    pub fn new(config: &GeminiConfig) -> Result<Self, LarpshellError> {
        Ok(Self {
            base: BaseProvider::new()?,
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        })
    }

    fn generate_url(&self) -> String {
        format!(
            "{}/v1beta/models/{}:generateContent",
            GEMINI_BASE_URL, self.model
        )
    }

    fn prompt_request(prompt: &str) -> GeminiRequest {
        GeminiRequest {
            contents: vec![Content {
                role: None,
                parts: vec![Part {
                    text: Some(prompt.to_string()),
                    function_call: None,
                    function_response: None,
                    thought_signature: None,
                }],
            }],
            system_instruction: None,
            tools: None,
        }
    }

    fn parse_generate_error(
        status: reqwest::StatusCode,
        response_text: &str,
        retry_after_header: Option<&str>,
    ) -> LarpshellError {
        if let Ok(error_response) = serde_json::from_str::<GeminiErrorResponse>(response_text) {
            return LarpshellError::from_http_status_with_retry_header(
                reqwest::StatusCode::from_u16(error_response.error.code).unwrap_or(status),
                "gemini",
                &error_response.error.message,
                retry_after_header,
            );
        }

        LarpshellError::from_http_status_with_retry_header(
            status,
            "gemini",
            response_text,
            retry_after_header,
        )
    }

    fn parse_generate_response(response_text: &str) -> Result<GeminiResponse, LarpshellError> {
        serde_json::from_str(response_text)
            .map_err(|error| LarpshellError::InvalidResponse(error.to_string()))
    }

    /// Returns the first candidate after rejecting blocked content, or an error
    /// if there are no candidates / the candidates list is empty / the content
    /// was blocked.
    fn first_unblocked_candidate(
        candidates: &Option<Vec<Candidate>>,
    ) -> Result<&Candidate, LarpshellError> {
        let candidates = candidates.as_ref().ok_or_else(|| {
            LarpshellError::InvalidResponse("no candidates in response".to_string())
        })?;

        let candidate = candidates
            .first()
            .ok_or_else(|| LarpshellError::InvalidResponse("empty candidates list".to_string()))?;

        if let Some(err) = Self::check_blocked(candidate) {
            return Err(err);
        }

        Ok(candidate)
    }

    /// Joins the text across all parts, erroring if the result is empty.
    fn join_part_text(parts: &[Part]) -> Result<String, LarpshellError> {
        let text: String = parts
            .iter()
            .filter_map(|part| part.text.as_deref())
            .collect::<Vec<_>>()
            .join("");
        if text.is_empty() {
            return Err(LarpshellError::InvalidResponse(
                "no text in response".to_string(),
            ));
        }
        Ok(text)
    }

    fn extract_generate_text(gemini_response: GeminiResponse) -> Result<String, LarpshellError> {
        let candidate = Self::first_unblocked_candidate(&gemini_response.candidates)?;

        let content = candidate
            .content
            .as_ref()
            .ok_or_else(|| LarpshellError::InvalidResponse("no content in response".to_string()))?;

        let parts = content
            .parts
            .as_ref()
            .ok_or_else(|| LarpshellError::InvalidResponse("no parts in content".to_string()))?;

        Self::join_part_text(parts)
    }

    async fn request_generate(
        &self,
        request_body: &GeminiRequest,
    ) -> Result<GeminiResponse, LarpshellError> {
        let response = self
            .base
            .client
            .post(self.generate_url())
            .header("x-goog-api-key", &self.api_key)
            .json(request_body)
            .send()
            .await
            .map_err(|e| LarpshellError::from_reqwest(&e, "gemini"))?;

        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let response_text = response
            .text()
            .await
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        if !status.is_success() {
            return Err(Self::parse_generate_error(
                status,
                &response_text,
                retry_after.as_deref(),
            ));
        }

        Self::parse_generate_response(&response_text)
    }

    fn message_content(message: &ChatMessage) -> Result<Content, LarpshellError> {
        let role = match message.role {
            Role::User | Role::Tool => Some("user".to_string()),
            Role::Assistant => Some("model".to_string()),
            Role::System => None,
        };

        let parts = if let Some(tool_calls) = message.tool_calls.as_ref() {
            tool_calls
                .iter()
                .map(|tool_call| Part {
                    text: None,
                    function_call: Some(FunctionCall {
                        name: tool_call.name.clone(),
                        args: tool_call.arguments.clone(),
                    }),
                    function_response: None,
                    thought_signature: tool_call.thought_signature.clone(),
                })
                .collect()
        } else if message.role == Role::Tool {
            // Gemini matches a functionResponse to its functionCall by name, so
            // use the original function name; fall back to the id only if the
            // name is absent.
            let name = message
                .tool_call_name
                .as_deref()
                .or(message.tool_call_id.as_deref())
                .ok_or_else(|| {
                    LarpshellError::InvalidResponse(
                        "tool message is missing tool_call_name and tool_call_id".to_string(),
                    )
                })?;
            vec![Part {
                text: None,
                function_call: None,
                function_response: Some(FunctionResponse {
                    name: name.to_string(),
                    response: serde_json::json!({
                        "result": message.content.clone().unwrap_or_default()
                    }),
                }),
                thought_signature: None,
            }]
        } else {
            vec![Part {
                text: message.content.clone(),
                function_call: None,
                function_response: None,
                thought_signature: None,
            }]
        };

        Ok(Content { role, parts })
    }

    fn system_instruction(messages: &[ChatMessage]) -> Option<Content> {
        let text = messages
            .iter()
            .filter(|message| message.role == Role::System)
            .filter_map(|message| message.content.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        (!text.is_empty()).then(|| Content {
            role: None,
            parts: vec![Part {
                text: Some(text),
                function_call: None,
                function_response: None,
                thought_signature: None,
            }],
        })
    }

    fn tool_declarations(tools: &[ToolDefinition]) -> Option<Vec<GeminiToolDeclaration>> {
        (!tools.is_empty()).then(|| {
            vec![GeminiToolDeclaration {
                function_declarations: tools
                    .iter()
                    .map(|tool| GeminiFunctionDeclaration {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        parameters: tool.parameters.clone(),
                    })
                    .collect(),
            }]
        })
    }

    fn check_blocked(candidate: &Candidate) -> Option<LarpshellError> {
        let reason = candidate.finish_reason.as_deref()?;
        if matches!(reason, "SAFETY" | "RECITATION") {
            Some(LarpshellError::InvalidResponse(format!(
                "content blocked by gemini: {}",
                reason.to_lowercase()
            )))
        } else {
            None
        }
    }

    fn extract_tool_parts(candidate: &Candidate) -> Result<&Vec<Part>, LarpshellError> {
        candidate
            .content
            .as_ref()
            .and_then(|content| content.parts.as_ref())
            .ok_or_else(|| LarpshellError::InvalidResponse("no parts in response".to_string()))
    }

    fn extract_tool_calls(parts: &[Part]) -> Vec<ToolCall> {
        parts
            .iter()
            .filter_map(|part| {
                part.function_call
                    .as_ref()
                    .map(|call| (call, part.thought_signature.clone()))
            })
            .enumerate()
            .map(|(index, (function_call, thought_signature))| ToolCall {
                id: format!("gemini_tc_{index}"),
                name: function_call.name.clone(),
                arguments: function_call.args.clone(),
                thought_signature,
            })
            .collect()
    }

    fn extract_chat_response(
        gemini_response: GeminiResponse,
    ) -> Result<ChatResponse, LarpshellError> {
        let candidate = Self::first_unblocked_candidate(&gemini_response.candidates)?;

        let parts = Self::extract_tool_parts(candidate)?;
        let tool_calls = Self::extract_tool_calls(parts);

        if !tool_calls.is_empty() {
            return Ok(ChatResponse::ToolCalls(tool_calls));
        }

        Self::join_part_text(parts).map(ChatResponse::Message)
    }
}

#[async_trait]
impl AIProvider for GeminiProvider {
    async fn generate(&self, prompt: &str) -> Result<String, LarpshellError> {
        let request_body = Self::prompt_request(prompt);
        let response = self.request_generate(&request_body).await?;
        Self::extract_generate_text(response)
    }

    async fn generate_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<ChatResponse, LarpshellError> {
        let request_body = GeminiRequest {
            contents: messages
                .iter()
                .filter(|message| message.role != Role::System)
                .map(Self::message_content)
                .collect::<Result<Vec<_>, _>>()?,
            system_instruction: Self::system_instruction(messages),
            tools: Self::tool_declarations(tools),
        };

        let response = self.request_generate(&request_body).await?;
        Self::extract_chat_response(response)
    }

    fn name(&self) -> String {
        format!("Gemini ({})", self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_tool_call_response_deserializes() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "read_file",
                            "args": {"file_path": "/tmp/test.txt"}
                        }
                    }]
                },
                "finishReason": "STOP"
            }]
        }"#;
        let response: GeminiResponse = serde_json::from_str(json).unwrap();
        let candidates = response.candidates.unwrap();
        let part = &candidates[0]
            .content
            .as_ref()
            .unwrap()
            .parts
            .as_ref()
            .unwrap()[0];
        assert!(part.text.is_none());
        let function_call = part.function_call.as_ref().unwrap();
        assert_eq!(function_call.name, "read_file");
    }

    #[test]
    fn gemini_text_response_deserializes() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "echo hello"}]
                },
                "finishReason": "STOP"
            }]
        }"#;
        let response: GeminiResponse = serde_json::from_str(json).unwrap();
        let candidates = response.candidates.unwrap();
        let part = &candidates[0]
            .content
            .as_ref()
            .unwrap()
            .parts
            .as_ref()
            .unwrap()[0];
        assert_eq!(part.text.as_deref(), Some("echo hello"));
        assert!(part.function_call.is_none());
    }

    #[test]
    fn extract_chat_response_safety_block_returns_blocked_error() {
        let json = r#"{
            "candidates": [{
                "finishReason": "SAFETY"
            }]
        }"#;
        let response: GeminiResponse = serde_json::from_str(json).unwrap();
        let err = GeminiProvider::extract_chat_response(response).unwrap_err();
        match err {
            LarpshellError::InvalidResponse(msg) => {
                assert!(
                    msg.contains("content blocked by gemini") && msg.contains("safety"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected InvalidResponse, got {other:?}"),
        }
    }

    #[test]
    fn gemini_request_serializes_system_instruction() {
        let messages = [
            ChatMessage::system("Use tools before answering".to_string()),
            ChatMessage::user("show disk usage".to_string()),
        ];
        let request = GeminiRequest {
            contents: messages
                .iter()
                .filter(|message| message.role != Role::System)
                .map(GeminiProvider::message_content)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            system_instruction: GeminiProvider::system_instruction(&messages),
            tools: None,
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(
            json["systemInstruction"]["parts"][0]["text"],
            "Use tools before answering"
        );
        assert_eq!(json["contents"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn gemini_generate_error_preserves_retry_after_header() {
        let err = GeminiProvider::parse_generate_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"code":429,"message":"quota exceeded"}}"#,
            Some("17"),
        );

        match err {
            LarpshellError::RateLimitExceeded { retry_after } => {
                assert_eq!(retry_after, Some(17));
            }
            other => panic!("expected RateLimitExceeded, got {other:?}"),
        }
    }
}
