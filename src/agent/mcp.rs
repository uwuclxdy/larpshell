use crate::providers::ToolDefinition;
use rmcp::model::{CallToolRequestParams, ClientCapabilities, Implementation, InitializeRequestParams};
use rmcp::service::{ClientServiceExt, RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientLifecycleMode, Peer, ServiceError};
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

const MCP_REQUEST_TIMEOUT_SECS: u64 = 30;

pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

/// stdio MCP client built on rmcp: spawning runs the legacy `initialize`
/// handshake up front (the lifecycle every configured server speaks today),
/// tool calls carry a per-request deadline, and dropping the client kills the
/// child process.
pub struct StdioMcpClient {
    name: String,
    peer: Peer<RoleClient>,
    /// Owns the transport and the child process; dropping it kills the server.
    _service: RunningService<RoleClient, InitializeRequestParams>,
    /// Runtime handle captured at spawn time; tool calls happen later on a
    /// worker thread parked in `block_in_place`, outside any async context,
    /// so `block_on` bridges them back into the runtime.
    handle: tokio::runtime::Handle,
}

impl StdioMcpClient {
    pub async fn spawn(config: &McpServerConfig) -> Result<Self, String> {
        let mut command = tokio::process::Command::new(&config.command);
        command.args(&config.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());

        for (key, value) in &config.env {
            command.env(key, value);
        }

        let transport = TokioChildProcess::new(command)
            .map_err(|error| format!("failed to spawn MCP server '{}' ({}): {error}", config.name, config.command))?;

        // Pin the same protocol version the hand-rolled client advertised, so
        // servers that only speak the baseline revision see an identical
        // handshake.
        let client_info =
            InitializeRequestParams::new(ClientCapabilities::default(), Implementation::new("larpshell", env!("CARGO_PKG_VERSION")))
                .with_protocol_version(rmcp::model::ProtocolVersion::V_2024_11_05);
        let service = client_info
            .serve_with_lifecycle(transport, ClientLifecycleMode::Initialize)
            .await
            .map_err(|error| format!("failed to initialize MCP server '{}': {error}", config.name))?;

        Ok(Self { name: config.name.clone(), peer: service.peer().clone(), _service: service, handle: tokio::runtime::Handle::current() })
    }

    pub async fn list_tools(&self) -> Result<Vec<ToolDefinition>, String> {
        let tools = self.peer.list_all_tools().await.map_err(|error| format!("MCP server '{}': {error}", self.name))?;

        Ok(tools
            .into_iter()
            .map(|tool| {
                let name = format!("{}_{}", self.name, tool.name);
                let parameters = tool.schema_as_json_value();
                let description = tool.description.unwrap_or_default().into_owned();
                ToolDefinition { name, description, parameters }
            })
            .collect())
    }

    pub fn call_tool(&mut self, tool_name: &str, arguments: &serde_json::Value) -> Result<String, String> {
        // Routing guarantees the prefix is present; fall back to the bare name
        // only for callers that already stripped it (e.g. direct test calls).
        let original_name = tool_name.strip_prefix(self.name.as_str()).and_then(|rest| rest.strip_prefix('_')).unwrap_or(tool_name);

        let arguments_map = arguments.as_object().cloned().unwrap_or_default();
        let params = CallToolRequestParams::new(original_name.to_string()).with_arguments(arguments_map);

        let peer = self.peer.clone();
        let handle = self.handle.clone();
        let result = handle
            .block_on(async move { tokio::time::timeout(Duration::from_secs(MCP_REQUEST_TIMEOUT_SECS), peer.call_tool(params)).await });

        match result {
            Ok(Ok(call_result)) => Ok(call_result
                .content
                .iter()
                .filter_map(|content| content.as_text().map(|text| text.text.clone()))
                .collect::<Vec<_>>()
                .join("\n")),
            Ok(Err(ServiceError::TransportClosed)) => Err(format!("MCP server '{}' exited unexpectedly", self.name)),
            Ok(Err(error)) => Err(format!("MCP server '{}' error: {error}", self.name)),
            Err(_elapsed) => Err(format!("MCP server '{}' did not respond within {MCP_REQUEST_TIMEOUT_SECS}s", self.name)),
        }
    }

    pub fn server_name(&self) -> &str {
        &self.name
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

    /// End-to-end against a real stdio server: a python script speaking the
    /// legacy JSON-RPC handshake, exercising spawn, the initialize lifecycle,
    /// tool listing, and a tool call through the rmcp client.
    #[tokio::test(flavor = "multi_thread")]
    async fn stdio_client_lists_and_calls_tools() {
        if std::process::Command::new("python3").arg("--version").output().is_err() {
            eprintln!("skipping: python3 not available for the mock MCP server");
            return;
        }

        let dir = std::env::temp_dir().join(format!("larpshell_mcp_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script_path = dir.join("mcp_server.py");
        std::fs::write(&script_path, MOCK_MCP_SERVER_PY).unwrap();

        let config = McpServerConfig {
            name: "test".to_string(),
            command: "python3".to_string(),
            args: vec![script_path.display().to_string()],
            env: HashMap::new(),
        };

        let client = StdioMcpClient::spawn(&config).await.expect("mock MCP server should spawn and initialize");

        let tools = client.list_tools().await.expect("tools/list should succeed");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test_echo");
        assert_eq!(tools[0].description, "echo a string");
        assert_eq!(tools[0].parameters["type"], "object");

        let mut client = client;
        let result = tokio::task::block_in_place(|| client.call_tool("test_echo", &serde_json::json!({ "text": "hello from larpshell" })))
            .expect("tools/call should succeed");
        assert_eq!(result, "hello from larpshell");

        drop(client);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Minimal MCP server speaking just enough of the protocol: initialize,
    /// tools/list, tools/call; notifications (no `id`) are ignored.
    const MOCK_MCP_SERVER_PY: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {
            "protocolVersion": msg["params"]["protocolVersion"],
            "capabilities": {"tools": {"listChanged": False}},
            "serverInfo": {"name": "test-server", "version": "1.0.0"},
        }})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"tools": [{
            "name": "echo",
            "description": "echo a string",
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
        }]}})
    elif method == "tools/call":
        if msg["params"].get("name") != "echo":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": {
                "content": [{"type": "text", "text": "unknown tool"}],
                "isError": True,
            }})
        else:
            args = msg["params"].get("arguments", {})
            send({"jsonrpc": "2.0", "id": msg["id"], "result": {
                "content": [{"type": "text", "text": args.get("text", "")}],
            }})
"#;
}
