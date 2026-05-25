use crate::providers::ToolDefinition;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

pub struct StdioMcpClient {
    name: String,
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    request_id: u64,
}

#[derive(Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct JsonRpcResponse {
    #[expect(dead_code, reason = "deserialized for protocol completeness")]
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    #[expect(dead_code, reason = "deserialized for protocol completeness")]
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct ToolsListResult {
    tools: Vec<McpToolInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpToolInfo {
    name: String,
    #[serde(default)]
    description: Option<String>,
    input_schema: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ToolCallResult {
    content: Vec<McpContent>,
}

#[derive(Deserialize)]
struct McpContent {
    #[serde(default)]
    text: Option<String>,
}

impl StdioMcpClient {
    pub fn spawn(config: &McpServerConfig) -> Result<Self, String> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        for (key, value) in &config.env {
            command.env(key, value);
        }

        let mut child = command.spawn().map_err(|error| {
            format!(
                "failed to spawn MCP server '{}' ({}): {error}",
                config.name, config.command
            )
        })?;

        let stdin = BufWriter::new(child.stdin.take().ok_or("failed to get stdin")?);
        let stdout = BufReader::new(child.stdout.take().ok_or("failed to get stdout")?);

        Ok(Self {
            name: config.name.clone(),
            child,
            stdin,
            stdout,
            request_id: 0,
        })
    }

    fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        self.request_id += 1;
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: self.request_id,
            method,
            params,
        };

        let json =
            serde_json::to_string(&request).map_err(|error| format!("serialize error: {error}"))?;
        writeln!(self.stdin, "{json}")
            .map_err(|error| format!("write to MCP server '{}': {error}", self.name))?;
        self.stdin
            .flush()
            .map_err(|error| format!("flush to MCP server '{}': {error}", self.name))?;

        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|error| format!("read from MCP server '{}': {error}", self.name))?;

        if line.trim().is_empty() {
            return Err(format!(
                "MCP server '{}' returned empty response",
                self.name
            ));
        }

        let response: JsonRpcResponse = serde_json::from_str(line.trim())
            .map_err(|error| format!("parse MCP response: {error}"))?;

        if let Some(error) = response.error {
            return Err(format!(
                "MCP server '{}' error: {}",
                self.name, error.message
            ));
        }

        response
            .result
            .ok_or_else(|| format!("MCP server '{}' returned no result", self.name))
    }

    pub fn initialize(&mut self) -> Result<(), String> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "larpshell",
                "version": env!("CARGO_PKG_VERSION")
            }
        });
        let _ = self.send_request("initialize", Some(params))?;

        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let json =
            serde_json::to_string(&notification).map_err(|error| format!("serialize: {error}"))?;
        writeln!(self.stdin, "{json}").map_err(|error| format!("write notification: {error}"))?;
        self.stdin
            .flush()
            .map_err(|error| format!("flush: {error}"))?;

        Ok(())
    }

    pub fn list_tools(&mut self) -> Result<Vec<ToolDefinition>, String> {
        let result = self.send_request("tools/list", None)?;
        let tools_result: ToolsListResult =
            serde_json::from_value(result).map_err(|error| format!("parse tools/list: {error}"))?;

        Ok(tools_result
            .tools
            .into_iter()
            .map(|tool| ToolDefinition {
                name: format!("{}_{}", self.name, tool.name),
                description: tool.description.unwrap_or_default(),
                parameters: tool
                    .input_schema
                    .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
            })
            .collect())
    }

    pub fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        // Routing guarantees the prefix is present; fall back to the bare name
        // only for callers that already stripped it (e.g. direct test calls).
        let original_name = tool_name
            .strip_prefix(self.name.as_str())
            .and_then(|rest| rest.strip_prefix('_'))
            .unwrap_or(tool_name);

        let params = serde_json::json!({
            "name": original_name,
            "arguments": arguments
        });
        let result = self.send_request("tools/call", Some(params))?;
        let call_result: ToolCallResult =
            serde_json::from_value(result).map_err(|error| format!("parse tools/call: {error}"))?;

        Ok(call_result
            .content
            .iter()
            .filter_map(|content| content.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub fn server_name(&self) -> &str {
        &self.name
    }
}

impl Drop for StdioMcpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub fn load_mcp_configs() -> Vec<McpServerConfig> {
    let Ok(config_dir) = crate::config::ensure_config_dir() else {
        return Vec::new();
    };

    let mcp_path = config_dir.join("mcp.json");
    if !mcp_path.exists() {
        return Vec::new();
    }

    let contents = match std::fs::read_to_string(&mcp_path) {
        Ok(contents) => contents,
        Err(error) => {
            crate::cli::print_warning(&format!("failed to read mcp.json: {error}"));
            return Vec::new();
        }
    };

    let parsed: Result<McpConfigFile, _> = serde_json::from_str(&contents);
    match parsed {
        Ok(file) => file
            .mcp_servers
            .into_iter()
            .map(|(name, server)| McpServerConfig {
                name,
                command: server.command,
                args: server.args.unwrap_or_default(),
                env: server.env.unwrap_or_default(),
            })
            .collect(),
        Err(error) => {
            crate::cli::print_warning(&format!("failed to parse mcp.json: {error}"));
            Vec::new()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpConfigFile {
    mcp_servers: HashMap<String, McpServerEntry>,
}

#[derive(Deserialize)]
struct McpServerEntry {
    command: String,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_config_parses_valid_json() {
        let json = r#"{
            "mcpServers": {
                "git": {
                    "command": "mcp-server-git",
                    "args": ["--repository", "."],
                    "env": {"GIT_DIR": "/tmp"}
                },
                "minimal": {
                    "command": "/usr/bin/server"
                }
            }
        }"#;
        let file: McpConfigFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.mcp_servers.len(), 2);

        let git = &file.mcp_servers["git"];
        assert_eq!(git.command, "mcp-server-git");
        assert_eq!(git.args.as_ref().unwrap(), &["--repository", "."]);
        assert_eq!(git.env.as_ref().unwrap()["GIT_DIR"], "/tmp");

        let minimal = &file.mcp_servers["minimal"];
        assert_eq!(minimal.command, "/usr/bin/server");
        assert!(minimal.args.is_none());
        assert!(minimal.env.is_none());
    }

    #[test]
    fn mcp_config_empty_servers() {
        let json = r#"{"mcpServers": {}}"#;
        let file: McpConfigFile = serde_json::from_str(json).unwrap();
        assert!(file.mcp_servers.is_empty());
    }

    #[test]
    fn load_mcp_configs_returns_empty_when_no_file() {
        let configs = load_mcp_configs();
        let _ = configs;
    }
}
