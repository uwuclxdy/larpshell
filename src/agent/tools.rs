use crate::config::AgentMode;
use crate::providers::ToolDefinition;
use std::sync::Mutex;

use super::builtins;

pub struct RegisteredTool {
    pub definition: ToolDefinition,
    executor: Box<dyn Fn(serde_json::Value) -> Result<String, String> + Send + Sync>,
}

impl RegisteredTool {
    pub fn new(
        definition: ToolDefinition,
        executor: Box<dyn Fn(serde_json::Value) -> Result<String, String> + Send + Sync>,
    ) -> Self {
        Self {
            definition,
            executor,
        }
    }

    pub fn execute(&self, args: serde_json::Value) -> Result<String, String> {
        (self.executor)(args)
    }
}

pub struct ToolRegistry {
    tools: Vec<RegisteredTool>,
    mcp_clients: Vec<Mutex<crate::agent::mcp::StdioMcpClient>>,
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

    /// Registers an MCP-backed tool's definition. Execution is always routed
    /// through `mcp_clients`, so the executor closure is unreachable.
    pub fn register_mcp_tool(&mut self, definition: ToolDefinition) {
        self.tools.push(RegisteredTool::new(
            definition,
            Box::new(|_| unreachable!("MCP tools are executed via mcp_clients")),
        ));
    }

    pub fn add_mcp_client(&mut self, client: crate::agent::mcp::StdioMcpClient) {
        self.mcp_clients.push(Mutex::new(client));
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| tool.definition.clone())
            .collect()
    }

    pub fn execute(&self, name: &str, args: serde_json::Value) -> Result<String, String> {
        if let Some(result) = self.try_execute_mcp_tool(name, &args) {
            return result;
        }

        self.execute_builtin_tool(name, args)
    }

    fn try_execute_mcp_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Option<Result<String, String>> {
        for client_mutex in &self.mcp_clients {
            let mut client = client_mutex
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let prefix = format!("{}_", client.server_name());
            if name.starts_with(&prefix) {
                return Some(client.call_tool(name, args));
            }
        }

        None
    }

    fn execute_builtin_tool(&self, name: &str, args: serde_json::Value) -> Result<String, String> {
        self.tools
            .iter()
            .find(|tool| tool.definition.name == name)
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
                serde_json::json!({"file_path": dir.join("test.txt").to_str().unwrap()}),
            )
            .unwrap();

        assert_eq!(result, "hello");
    }

    #[test]
    fn registry_execute_unknown_tool_returns_error() {
        let registry = ToolRegistry::with_builtins(AgentMode::Safe);
        assert_err_contains(
            registry.execute("nonexistent", serde_json::json!({})),
            "unknown tool",
        );
    }
}
