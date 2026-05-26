use crate::config::AgentMode;
use crate::providers::ToolDefinition;
use std::sync::Mutex;

use super::builtins;

type ToolExecutor = Box<dyn Fn(&serde_json::Value) -> Result<String, String> + Send + Sync>;

pub enum RegisteredTool {
    /// A builtin tool with an owned executor closure.
    Builtin {
        definition: ToolDefinition,
        executor: ToolExecutor,
    },
    /// An MCP-backed tool; execution is always routed through `mcp_clients`
    /// by name-prefix, so no executor is stored here.
    Mcp { definition: ToolDefinition },
}

impl RegisteredTool {
    pub fn new(definition: ToolDefinition, executor: ToolExecutor) -> Self {
        Self::Builtin {
            definition,
            executor,
        }
    }

    pub fn definition(&self) -> &ToolDefinition {
        match self {
            Self::Builtin { definition, .. } | Self::Mcp { definition } => definition,
        }
    }

    pub fn execute(&self, args: &serde_json::Value) -> Result<String, String> {
        match self {
            Self::Builtin { executor, .. } => executor(args),
            Self::Mcp { definition } => Err(format!(
                "MCP tool '{}' must be dispatched via mcp_clients",
                definition.name
            )),
        }
    }
}

struct McpClientEntry {
    // avoids a per-call `format!` allocation in the agent loop
    prefix: String,
    client: Mutex<crate::agent::mcp::StdioMcpClient>,
}

pub struct ToolRegistry {
    tools: Vec<RegisteredTool>,
    mcp_clients: Vec<McpClientEntry>,
}

impl ToolRegistry {
    pub const fn new() -> Self {
        Self {
            tools: Vec::new(),
            mcp_clients: Vec::new(),
        }
    }

    pub fn register(&mut self, tool: RegisteredTool) {
        self.tools.push(tool);
    }

    /// Registers an MCP-backed tool's definition. Execution is routed through
    /// `mcp_clients` by name-prefix; this entry is definition-only.
    pub fn register_mcp_tool(&mut self, definition: ToolDefinition) {
        self.tools.push(RegisteredTool::Mcp { definition });
    }

    pub fn add_mcp_client(&mut self, client: crate::agent::mcp::StdioMcpClient) {
        let mut prefix = client.server_name().to_owned();
        prefix.push('_');

        // Reject server names that are a prefix of an existing server name (or vice-versa),
        // because `try_execute_mcp_tool` routes by `starts_with` and would mis-route tool
        // calls (e.g. "git_status" would match both "git_" and "github_" if "git" is
        // registered before "github"). Panic at registration so the misconfiguration is
        // caught during startup rather than silently mislabelling calls at runtime.
        for existing in &self.mcp_clients {
            let a = &existing.prefix;
            let b = &prefix;
            if a.starts_with(b.as_str()) || b.starts_with(a.as_str()) {
                panic!(
                    "MCP server name collision: '{}' and '{}' share a prefix — \
                     one would shadow the other during tool routing",
                    a.trim_end_matches('_'),
                    b.trim_end_matches('_'),
                );
            }
        }

        self.mcp_clients.push(McpClientEntry {
            prefix,
            client: Mutex::new(client),
        });
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| tool.definition().clone())
            .collect()
    }

    pub fn execute(&self, name: &str, args: &serde_json::Value) -> Result<String, String> {
        if let Some(result) = self.try_execute_mcp_tool(name, args) {
            return result;
        }

        self.execute_builtin_tool(name, args)
    }

    fn try_execute_mcp_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Option<Result<String, String>> {
        for entry in &self.mcp_clients {
            if name.starts_with(&entry.prefix) {
                let mut client = entry
                    .client
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                return Some(client.call_tool(name, args));
            }
        }

        None
    }

    fn execute_builtin_tool(&self, name: &str, args: &serde_json::Value) -> Result<String, String> {
        self.tools
            .iter()
            .find(|tool| tool.definition().name == name)
            .ok_or_else(|| format!("unknown tool: {name}"))
            .and_then(|tool| tool.execute(args))
    }

    pub fn with_builtins(agent_mode: AgentMode) -> Self {
        let mut registry = Self::new();
        builtins::register_builtins(&mut registry, agent_mode);
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/tests")
            .join(format!("agent_tools_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn assert_err_contains(result: Result<String, String>, expected: &str) {
        assert!(result.unwrap_err().contains(expected));
    }

    fn assert_has_tool_names(registry: &ToolRegistry, expected_names: &[&str]) {
        let names: Vec<_> = registry
            .definitions()
            .iter()
            .map(|definition| definition.name.clone())
            .collect();

        for name in expected_names {
            assert!(names.contains(&name.to_string()));
        }
    }

    #[test]
    fn tool_registry_with_builtins_registers_expected_builtin_names() {
        let registry = ToolRegistry::with_builtins(AgentMode::On);
        let names = registry
            .definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "list_files",
                "search_files",
                "fetch_url",
                "run_command"
            ]
        );
    }

    #[test]
    fn registry_with_builtins_has_five_safe_tools() {
        let registry = ToolRegistry::with_builtins(AgentMode::Safe);
        assert_eq!(registry.definitions().len(), 5);
        assert_has_tool_names(
            &registry,
            &[
                "read_file",
                "list_files",
                "search_files",
                "fetch_url",
                "run_command",
            ],
        );
    }

    #[test]
    fn registry_with_builtins_has_seven_on_tools() {
        let registry = ToolRegistry::with_builtins(AgentMode::On);
        assert_eq!(registry.definitions().len(), 7);
        assert_has_tool_names(
            &registry,
            &[
                "read_file",
                "write_file",
                "edit_file",
                "list_files",
                "search_files",
                "fetch_url",
                "run_command",
            ],
        );
    }

    #[test]
    fn registry_execute_calls_correct_tool() {
        let dir = test_dir("registry_exec");
        fs::write(dir.join("test.txt"), "hello").unwrap();
        let registry = ToolRegistry::with_builtins(AgentMode::Safe);

        let result = registry
            .execute(
                "read_file",
                &serde_json::json!({"file_path": dir.join("test.txt").to_str().unwrap()}),
            )
            .unwrap();

        assert_eq!(result, "hello");
    }

    #[test]
    fn registry_execute_unknown_tool_returns_error() {
        let registry = ToolRegistry::with_builtins(AgentMode::Safe);
        assert_err_contains(
            registry.execute("nonexistent", &serde_json::json!({})),
            "unknown tool",
        );
    }
}
