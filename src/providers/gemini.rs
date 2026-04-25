use crate::config::GeminiConfig;
use crate::error::LarpshellError;
use crate::providers::AIProvider;
use crate::providers::base::{BaseProvider, strip_url_for_display};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";

pub struct GeminiProvider {
    base: BaseProvider,
    base_url: String,
    api_key: String,
    model: String,
}

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<Content>,
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
            base_url: GEMINI_BASE_URL.to_string(),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        })
    }
}

#[async_trait]
impl AIProvider for GeminiProvider {
    async fn generate(&self, prompt: &str) -> Result<String, LarpshellError> {
        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.base_url, self.model, self.api_key
        );

        let request_body = GeminiRequest {
            contents: vec![Content {
                role: None,
                parts: vec![Part {
                    text: Some(prompt.to_string()),
                    function_call: None,
                    function_response: None,
                    thought_signature: None,
                }],
            }],
            tools: None,
        };

        let response = self
            .base
            .client
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| LarpshellError::from_reqwest(e, "gemini"))?;

        let status = response.status();
        let response_text = response
            .text()
            .await
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        if !status.is_success() {
            if let Ok(error_response) = serde_json::from_str::<GeminiErrorResponse>(&response_text)
            {
                return Err(LarpshellError::from_http_status(
                    reqwest::StatusCode::from_u16(error_response.error.code).unwrap_or(status),
                    "gemini",
                    &error_response.error.message,
                ));
            }

            return Err(LarpshellError::from_http_status(
                status,
                "gemini",
                &response_text,
            ));
        }

        let gemini_response: GeminiResponse = serde_json::from_str(&response_text)
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        let candidates = gemini_response.candidates.ok_or_else(|| {
            LarpshellError::InvalidResponse("no candidates in response".to_string())
        })?;

        let candidate = candidates
            .first()
            .ok_or_else(|| LarpshellError::InvalidResponse("empty candidates list".to_string()))?;

        if let Some(finish_reason) = &candidate.finish_reason
            && (finish_reason == "SAFETY" || finish_reason == "RECITATION")
        {
            let reason = finish_reason.to_lowercase();
            return Err(LarpshellError::InvalidResponse(format!(
                "content blocked by gemini: {reason}"
            )));
        }

        let content = candidate
            .content
            .as_ref()
            .ok_or_else(|| LarpshellError::InvalidResponse("no content in response".to_string()))?;

        let parts = content
            .parts
            .as_ref()
            .ok_or_else(|| LarpshellError::InvalidResponse("no parts in content".to_string()))?;

        let text = parts
            .first()
            .and_then(|part| part.text.clone())
            .ok_or_else(|| LarpshellError::InvalidResponse("no text in response".to_string()))?;

        Ok(text)
    }

    async fn generate_with_tools(
        &self,
        messages: &[crate::providers::ChatMessage],
        tools: &[crate::providers::ToolDefinition],
    ) -> Result<crate::providers::ChatResponse, LarpshellError> {
        use crate::providers::{ChatResponse, Role};

        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.base_url, self.model, self.api_key
        );

        let contents: Vec<Content> = messages
            .iter()
            .filter(|message| message.role != Role::System)
            .map(|message| {
                let role = match message.role {
                    Role::User => Some("user".to_string()),
                    Role::Assistant => Some("model".to_string()),
                    Role::Tool => Some("user".to_string()),
                    Role::System => None,
                };

                let parts = if let Some(ref tool_calls) = message.tool_calls {
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
                    vec![Part {
                        text: None,
                        function_call: None,
                        function_response: Some(FunctionResponse {
                            name: message.tool_call_id.clone().unwrap_or_default(),
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

                Content { role, parts }
            })
            .collect();

        let gemini_tools = if tools.is_empty() {
            None
        } else {
            Some(vec![GeminiToolDeclaration {
                function_declarations: tools
                    .iter()
                    .map(|tool| GeminiFunctionDeclaration {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        parameters: tool.parameters.clone(),
                    })
                    .collect(),
            }])
        };

        let request_body = GeminiRequest {
            contents,
            tools: gemini_tools,
        };

        let response = self
            .base
            .client
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| LarpshellError::from_reqwest(e, "gemini"))?;

        let status = response.status();
        let response_text = response
            .text()
            .await
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        if !status.is_success() {
            if let Ok(error_response) = serde_json::from_str::<GeminiErrorResponse>(&response_text)
            {
                return Err(LarpshellError::from_http_status(
                    reqwest::StatusCode::from_u16(error_response.error.code).unwrap_or(status),
                    "gemini",
                    &error_response.error.message,
                ));
            }
            return Err(LarpshellError::from_http_status(
                status,
                "gemini",
                &response_text,
            ));
        }

        let gemini_response: GeminiResponse = serde_json::from_str(&response_text)
            .map_err(|e| LarpshellError::InvalidResponse(e.to_string()))?;

        let candidates = gemini_response.candidates.ok_or_else(|| {
            LarpshellError::InvalidResponse("no candidates in response".to_string())
        })?;

        let candidate = candidates
            .first()
            .ok_or_else(|| LarpshellError::InvalidResponse("empty candidates list".to_string()))?;

        let parts = candidate
            .content
            .as_ref()
            .and_then(|content| content.parts.as_ref())
            .ok_or_else(|| LarpshellError::InvalidResponse("no parts in response".to_string()))?;

        let tool_calls: Vec<crate::providers::ToolCall> = parts
            .iter()
            .filter_map(|part| {
                part.function_call
                    .as_ref()
                    .map(|call| (call, part.thought_signature.clone()))
            })
            .enumerate()
            .map(
                |(index, (function_call, thought_signature))| crate::providers::ToolCall {
                    id: format!("gemini_tc_{index}"),
                    name: function_call.name.clone(),
                    arguments: function_call.args.clone(),
                    thought_signature,
                },
            )
            .collect();

        if !tool_calls.is_empty() {
            return Ok(ChatResponse::ToolCalls(tool_calls));
        }

        let text = parts
            .first()
            .and_then(|part| part.text.clone())
            .ok_or_else(|| LarpshellError::InvalidResponse("no text in response".to_string()))?;

        Ok(ChatResponse::Message(text))
    }

    fn name(&self) -> String {
        format!("Gemini ({})", strip_url_for_display(&self.base_url))
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
}
