use crate::providers::ToolDefinition;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

pub struct RegisteredTool {
    pub definition: ToolDefinition,
    executor: Box<dyn Fn(serde_json::Value) -> Result<String, String> + Send + Sync>,
}

impl RegisteredTool {
    pub fn execute(&self, args: serde_json::Value) -> Result<String, String> {
        (self.executor)(args)
    }
}

pub struct ToolRegistry {
    tools: Vec<RegisteredTool>,
    mcp_clients: Vec<Mutex<crate::agent::mcp::StdioMcpClient>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            mcp_clients: Vec::new(),
        }
    }

    pub fn register(&mut self, tool: RegisteredTool) {
        self.tools.push(tool);
    }

    pub fn register_mcp_tool(&mut self, definition: ToolDefinition, _server_name: String) {
        // Register the definition so it appears in definitions() sent to the LLM.
        // Execution is always routed through mcp_clients, never through this executor.
        self.tools.push(RegisteredTool {
            definition,
            executor: Box::new(|_| unreachable!("MCP tools are executed via mcp_clients")),
        });
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
        for client_mutex in &self.mcp_clients {
            let prefix = {
                let client = client_mutex.lock().unwrap();
                format!("{}_", client.server_name())
            };
            if name.starts_with(&prefix) {
                let mut client = client_mutex.lock().unwrap();
                return client.call_tool(name, args);
            }
        }

        self.tools
            .iter()
            .find(|tool| tool.definition.name == name)
            .ok_or_else(|| format!("unknown tool: {name}"))
            .and_then(|tool| tool.execute(args))
    }

    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(read_file_tool());
        registry.register(list_files_tool());
        registry.register(search_files_tool());
        registry.register(run_command_tool());
        registry
    }
}

const MAX_FILE_SIZE: usize = 100 * 1024;
const MAX_SEARCH_MATCHES: usize = 50;

fn read_file_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "read_file".to_string(),
            description: "Read the contents of a file at the given path.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute or relative path to the file to read"
                    }
                },
                "required": ["file_path"]
            }),
        },
        executor: Box::new(|args| {
            let file_path = args["file_path"]
                .as_str()
                .ok_or("file_path must be a string")?;
            execute_read_file(file_path)
        }),
    }
}

fn execute_read_file(file_path: &str) -> Result<String, String> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Err(format!("file not found: {file_path}"));
    }

    let metadata = fs::metadata(path).map_err(|error| format!("cannot read file: {error}"))?;
    if metadata.len() as usize > MAX_FILE_SIZE {
        let content =
            fs::read_to_string(path).map_err(|error| format!("cannot read file: {error}"))?;
        let truncated = content.get(..MAX_FILE_SIZE).unwrap_or(&content);
        Ok(format!(
            "{truncated}\n\n[truncated — file is {} bytes, showing first {MAX_FILE_SIZE}]",
            metadata.len()
        ))
    } else {
        fs::read_to_string(path).map_err(|error| format!("cannot read file: {error}"))
    }
}

fn list_files_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "list_files".to_string(),
            description: "List files and directories in the given directory path.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "directory_path": {
                        "type": "string",
                        "description": "Path to the directory to list"
                    }
                },
                "required": ["directory_path"]
            }),
        },
        executor: Box::new(|args| {
            let directory_path = args["directory_path"]
                .as_str()
                .ok_or("directory_path must be a string")?;
            execute_list_files(directory_path)
        }),
    }
}

fn execute_list_files(directory_path: &str) -> Result<String, String> {
    let path = Path::new(directory_path);
    if !path.is_dir() {
        return Err(format!("not a directory: {directory_path}"));
    }

    let read_dir = fs::read_dir(path).map_err(|error| format!("cannot read directory: {error}"))?;
    let mut entries = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|error| format!("error reading entry: {error}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false)
        {
            entries.push(format!("{name}/"));
        } else {
            entries.push(name);
        }
    }
    entries.sort();
    Ok(entries.join("\n"))
}

fn search_files_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "search_files".to_string(),
            description: "Search for a text pattern across files in a directory. Returns matching lines with file paths and line numbers.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Text pattern to search for (literal string match)"
                    },
                    "directory_path": {
                        "type": "string",
                        "description": "Directory to search in (defaults to current directory)"
                    }
                },
                "required": ["pattern"]
            }),
        },
        executor: Box::new(|args| {
            let pattern = args["pattern"].as_str().ok_or("pattern must be a string")?;
            let directory_path = args
                .get("directory_path")
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            execute_search_files(pattern, directory_path)
        }),
    }
}

fn execute_search_files(pattern: &str, directory_path: &str) -> Result<String, String> {
    let path = Path::new(directory_path);
    if !path.is_dir() {
        return Err(format!("not a directory: {directory_path}"));
    }

    let mut matches = Vec::new();
    search_recursive(path, pattern, &mut matches);

    if matches.is_empty() {
        return Ok(format!("no matches found for '{pattern}'"));
    }

    let total = matches.len();
    if total > MAX_SEARCH_MATCHES {
        matches.truncate(MAX_SEARCH_MATCHES);
        matches.push(format!(
            "\n[showing {MAX_SEARCH_MATCHES} of {total} matches]"
        ));
    }

    Ok(matches.join("\n"))
}

fn search_recursive(dir: &Path, pattern: &str, matches: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        if matches.len() >= MAX_SEARCH_MATCHES {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') || name_str == "node_modules" || name_str == "target" {
                continue;
            }
            search_recursive(&path, pattern, matches);
        } else if path.is_file() {
            let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
            let text_extensions = [
                "rs", "toml", "json", "yaml", "yml", "md", "txt", "sh", "py", "js", "ts", "html",
                "css", "c", "h", "cpp", "go", "java", "rb", "conf", "cfg", "ini", "xml", "csv",
                "sql", "lua", "zig", "nix",
            ];
            if !extension.is_empty() && !text_extensions.contains(&extension) {
                continue;
            }

            search_file(&path, pattern, matches);
        }
    }
}

fn search_file(path: &Path, pattern: &str, matches: &mut Vec<String>) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };

    let display_path = path.display();
    for (index, line) in content.lines().enumerate() {
        if matches.len() >= MAX_SEARCH_MATCHES {
            return;
        }
        if line.contains(pattern) {
            matches.push(format!("{}:{}:{}", display_path, index + 1, line));
        }
    }
}

fn run_command_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "run_command".to_string(),
            description: "Run a safe read-only command to gather context. Only informational commands are allowed (no file modifications, deletions, or destructive operations).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to execute (must be safe and read-only)"
                    },
                    "args": {
                        "type": "array",
                        "items": {
                            "type": "string"
                        },
                        "description": "Optional arguments for the command"
                    }
                },
                "required": ["command"]
            }),
        },
        executor: Box::new(|args| {
            let command = args["command"]
                .as_str()
                .ok_or("command must be a string")?;
            let command_args = args
                .get("args")
                .and_then(|value| value.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            execute_run_command(command, &command_args)
        }),
    }
}

fn execute_run_command(command: &str, args: &[String]) -> Result<String, String> {
    // List of safe commands that are allowed
    let safe_commands = [
        "ls",
        "cat",
        "echo",
        "grep",
        "find",
        "ps",
        "whoami",
        "uname",
        "date",
        "pwd",
        "env",
        "printenv",
        "which",
        "whereis",
        "file",
        "stat",
        "id",
        "groups",
        "hostname",
        "uptime",
        "free",
        "df",
        "du",
        "top",
        "htop",
        "vmstat",
        "iostat",
        "mpstat",
        "sar",
        "netstat",
        "ss",
        "ip",
        "ifconfig",
        "route",
        "ping",
        "traceroute",
        "mtr",
        "dig",
        "nslookup",
        "host",
        "curl",
        "wget",
        "git",
        "svn",
        "hg",
        "docker",
        "podman",
        "kubectl",
        "aws",
        "gcloud",
        "az",
        "terraform",
        "ansible",
        "vault",
        "consul",
    ];

    // Check if the command is in the safe list
    let command_base = Path::new(command)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(command);

    if !safe_commands.contains(&command_base) {
        return Err(format!("command not allowed: {}", command));
    }

    // Additional safety checks for potentially dangerous arguments
    for arg in args {
        // Block common dangerous patterns
        if arg.contains("--delete")
            || arg.contains("--remove")
            || arg.contains("--force")
            || arg.contains(">")
            || arg.contains("|")
            || arg.contains(";")
            || arg.contains("&")
            || arg.contains("`")
            || arg.contains("$")
            || arg.contains("rm")
            || arg.contains("mv")
            || arg.contains("cp")
            || arg.contains("chmod")
            || arg.contains("chown")
        {
            return Err(format!("dangerous argument detected: {}", arg));
        }
    }

    // Execute the command
    let output = Command::new(command)
        .args(args)
        .output()
        .map_err(|error| format!("failed to execute command: {}", error))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("command failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.to_string())
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

    #[test]
    fn registry_with_builtins_has_four_tools() {
        let registry = ToolRegistry::with_builtins();
        assert_eq!(registry.definitions().len(), 4);
        let names: Vec<_> = registry
            .definitions()
            .iter()
            .map(|definition| definition.name.clone())
            .collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"list_files".to_string()));
        assert!(names.contains(&"search_files".to_string()));
        assert!(names.contains(&"run_command".to_string()));
    }

    #[test]
    fn read_file_returns_contents() {
        let dir = test_dir("read");
        let file_path = dir.join("hello.txt");
        fs::write(&file_path, "hello world").unwrap();

        let result = execute_read_file(file_path.to_str().unwrap());

        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn read_file_missing_returns_error() {
        let result = execute_read_file("/nonexistent/file.txt");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("file not found"));
    }

    #[test]
    fn read_file_truncates_large_files() {
        let dir = test_dir("read_large");
        let file_path = dir.join("big.txt");
        let content = "x".repeat(MAX_FILE_SIZE + 1000);
        fs::write(&file_path, &content).unwrap();

        let result = execute_read_file(file_path.to_str().unwrap()).unwrap();

        assert!(result.contains("[truncated"));
    }

    #[test]
    fn list_files_returns_sorted_entries() {
        let dir = test_dir("list");
        fs::write(dir.join("beta.txt"), "").unwrap();
        fs::write(dir.join("alpha.txt"), "").unwrap();
        fs::create_dir_all(dir.join("gamma")).unwrap();

        let result = execute_list_files(dir.to_str().unwrap()).unwrap();
        let lines: Vec<&str> = result.lines().collect();

        assert_eq!(lines, vec!["alpha.txt", "beta.txt", "gamma/"]);
    }

    #[test]
    fn list_files_not_a_dir_returns_error() {
        let result = execute_list_files("/nonexistent/dir");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a directory"));
    }

    #[test]
    fn search_files_finds_pattern() {
        let dir = test_dir("search");
        fs::write(
            dir.join("code.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        fs::write(dir.join("other.rs"), "fn other() {}\n").unwrap();

        let result = execute_search_files("println", dir.to_str().unwrap()).unwrap();

        assert!(result.contains("println"));
        assert!(result.contains("code.rs:2:"));
    }

    #[test]
    fn search_files_no_matches() {
        let dir = test_dir("search_empty");
        fs::write(dir.join("code.rs"), "fn main() {}\n").unwrap();

        let result = execute_search_files("nonexistent_pattern", dir.to_str().unwrap()).unwrap();

        assert!(result.contains("no matches found"));
    }

    #[test]
    fn registry_execute_calls_correct_tool() {
        let dir = test_dir("registry_exec");
        fs::write(dir.join("test.txt"), "hello").unwrap();
        let registry = ToolRegistry::with_builtins();

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
        let registry = ToolRegistry::with_builtins();
        let result = registry.execute("nonexistent", serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown tool"));
    }

    #[test]
    fn run_command_executes_safe_commands() {
        let result = execute_run_command("echo", &["hello world".to_string()]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().trim(), "hello world");
    }

    #[test]
    fn run_command_rejects_unsafe_commands() {
        let result = execute_run_command("rm", &["-rf".to_string(), "/".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("command not allowed"));
    }

    #[test]
    fn run_command_rejects_dangerous_args() {
        let result = execute_run_command("ls", &["--force".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("dangerous argument"));
    }
}
