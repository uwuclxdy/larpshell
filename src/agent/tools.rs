use crate::providers::ToolDefinition;
use std::fs;
use std::path::Path;

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
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: RegisteredTool) {
        self.tools.push(tool);
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| tool.definition.clone())
            .collect()
    }

    pub fn execute(&self, name: &str, args: serde_json::Value) -> Result<String, String> {
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
        let truncated: String = content.chars().take(MAX_FILE_SIZE).collect();
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
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') || name_str == "node_modules" || name_str == "target" {
                continue;
            }
            if matches.len() < MAX_SEARCH_MATCHES {
                search_recursive(&path, pattern, matches);
            }
        } else if path.is_file() {
            if matches.len() >= MAX_SEARCH_MATCHES {
                return;
            }

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
    fn registry_with_builtins_has_three_tools() {
        let registry = ToolRegistry::with_builtins();
        assert_eq!(registry.definitions().len(), 3);
        let names: Vec<_> = registry
            .definitions()
            .iter()
            .map(|definition| definition.name.clone())
            .collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"list_files".to_string()));
        assert!(names.contains(&"search_files".to_string()));
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
}
