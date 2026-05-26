use crate::common::home_dir;
use crate::config::AgentMode;
use crate::providers::ToolDefinition;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{SearcherBuilder, Sink, SinkMatch};
use ignore::WalkBuilder;
use std::borrow::Cow;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

use super::tools::{RegisteredTool, ToolRegistry};

const MAX_FILE_SIZE: usize = 100 * 1024;
const MAX_FETCH_SIZE: usize = 100 * 1024;
const MAX_SEARCH_MATCHES: usize = 50;
const SAFE_COMMANDS: &[&str] = &[
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
    "git",
    "svn",
    "hg",
];
const DANGEROUS_FLAG_PREFIXES: &[&str] = &["--delete", "--remove", "--force"];
const DANGEROUS_ARGUMENT_TOKENS: &[&str] = &["rm", "mv", "cp", "chmod", "chown"];
const GIT_READ_ONLY_SUBCOMMANDS: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "rev-parse",
    "ls-files",
    "describe",
    "help",
];

pub(crate) fn register_builtins(registry: &mut ToolRegistry, agent_mode: AgentMode) {
    registry.register(read_file_tool());
    if matches!(agent_mode, AgentMode::On) {
        registry.register(write_file_tool());
        registry.register(edit_file_tool());
    }
    registry.register(list_files_tool());
    registry.register(search_files_tool());
    registry.register(fetch_url_tool());
    registry.register(run_command_tool(agent_mode));
}

fn read_file_tool() -> RegisteredTool {
    RegisteredTool::new(
        ToolDefinition {
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
        Box::new(|args| {
            let file_path = args["file_path"]
                .as_str()
                .ok_or("file_path must be a string")?;
            execute_read_file(file_path)
        }),
    )
}

fn execute_read_file(file_path: &str) -> Result<String, String> {
    let expanded = expand_tilde(file_path);
    let path = Path::new(expanded.as_ref());
    if !path.exists() {
        return Err(format!("file not found: {expanded}"));
    }

    let metadata = fs::metadata(path).map_err(|error| format!("cannot read file: {error}"))?;
    if metadata.len() > MAX_FILE_SIZE as u64 {
        // Read only the first MAX_FILE_SIZE bytes to avoid allocating the full file.
        let mut file =
            fs::File::open(path).map_err(|error| format!("cannot read file: {error}"))?;
        use std::io::Read;
        let mut buf = Vec::with_capacity(MAX_FILE_SIZE);
        file.by_ref()
            .take(MAX_FILE_SIZE as u64)
            .read_to_end(&mut buf)
            .map_err(|error| format!("cannot read file: {error}"))?;
        let n = buf.len();
        let partial = String::from_utf8_lossy(&buf);
        return Ok(format!(
            "{partial}\n\n[truncated — file is {} bytes, showing first {n}]",
            metadata.len()
        ));
    }

    fs::read_to_string(path).map_err(|error| format!("cannot read file: {error}"))
}

fn write_file_tool() -> RegisteredTool {
    RegisteredTool::new(
        ToolDefinition {
            name: "write_file".to_string(),
            description: "Write text content to a file, creating parent directories as needed."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute or relative path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "Text content to write"
                    }
                },
                "required": ["file_path", "content"]
            }),
        },
        Box::new(|args| {
            let file_path = args["file_path"]
                .as_str()
                .ok_or("file_path must be a string")?;
            let content = args["content"].as_str().ok_or("content must be a string")?;
            execute_write_file(file_path, content)
        }),
    )
}

fn execute_write_file(file_path: &str, content: &str) -> Result<String, String> {
    let expanded = expand_tilde(file_path);
    let path = Path::new(expanded.as_ref());
    if path.is_dir() {
        return Err(format!("cannot write to directory: {expanded}"));
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| format!("cannot create directory: {error}"))?;
    }

    fs::write(path, content).map_err(|error| format!("cannot write file: {error}"))?;
    Ok(format!("wrote {} bytes to {expanded}", content.len()))
}

fn edit_file_tool() -> RegisteredTool {
    RegisteredTool::new(
        ToolDefinition {
            name: "edit_file".to_string(),
            description: "Replace exactly one occurrence of text in a file.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute or relative path to the file to edit"
                    },
                    "old_text": {
                        "type": "string",
                        "description": "Existing text to replace"
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Replacement text"
                    }
                },
                "required": ["file_path", "old_text", "new_text"]
            }),
        },
        Box::new(|args| {
            let file_path = args["file_path"]
                .as_str()
                .ok_or("file_path must be a string")?;
            let old_text = args["old_text"]
                .as_str()
                .ok_or("old_text must be a string")?;
            let new_text = args["new_text"]
                .as_str()
                .ok_or("new_text must be a string")?;
            execute_edit_file(file_path, old_text, new_text)
        }),
    )
}

fn execute_edit_file(file_path: &str, old_text: &str, new_text: &str) -> Result<String, String> {
    if old_text.is_empty() {
        return Err("old_text must not be empty".to_string());
    }

    let expanded = expand_tilde(file_path);
    let path = Path::new(expanded.as_ref());
    if path.is_dir() {
        return Err(format!("cannot edit directory: {expanded}"));
    }

    let content = fs::read_to_string(path).map_err(|error| format!("cannot read file: {error}"))?;
    let matches = content.match_indices(old_text).count();
    if matches == 0 {
        return Err("old_text not found".to_string());
    }
    if matches > 1 {
        return Err(format!(
            "old_text matched {matches} times; provide a unique string"
        ));
    }

    let updated = content.replacen(old_text, new_text, 1);
    fs::write(path, updated).map_err(|error| format!("cannot write file: {error}"))?;
    Ok(format!("replaced 1 occurrence in {expanded}"))
}

fn list_files_tool() -> RegisteredTool {
    RegisteredTool::new(
        ToolDefinition {
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
        Box::new(|args| {
            let directory_path = args["directory_path"]
                .as_str()
                .ok_or("directory_path must be a string")?;
            execute_list_files(directory_path)
        }),
    )
}

fn execute_list_files(directory_path: &str) -> Result<String, String> {
    let expanded = expand_tilde(directory_path);
    let path = Path::new(expanded.as_ref());
    if !path.is_dir() {
        return Err(format!("not a directory: {expanded}"));
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(path).map_err(|error| format!("cannot read directory: {error}"))? {
        let entry = entry.map_err(|error| format!("error reading entry: {error}"))?;
        entries.push(entry_name(&entry)?);
    }
    entries.sort();

    Ok(entries.join("\n"))
}

fn entry_name(entry: &fs::DirEntry) -> Result<String, String> {
    let suffix = if entry
        .file_type()
        .map_err(|error| format!("error reading entry: {error}"))?
        .is_dir()
    {
        "/"
    } else {
        ""
    };

    Ok(format!("{}{}", entry.file_name().to_string_lossy(), suffix))
}

fn search_files_tool() -> RegisteredTool {
    RegisteredTool::new(
        ToolDefinition {
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
        Box::new(|args| {
            let pattern = args["pattern"].as_str().ok_or("pattern must be a string")?;
            let directory_path = args
                .get("directory_path")
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            execute_search_files(pattern, directory_path)
        }),
    )
}

fn execute_search_files(pattern: &str, directory_path: &str) -> Result<String, String> {
    let expanded = expand_tilde(directory_path);
    let path = Path::new(expanded.as_ref());
    if !path.is_dir() {
        return Err(format!("not a directory: {expanded}"));
    }

    let matcher = RegexMatcherBuilder::new()
        .fixed_strings(true)
        .build(pattern)
        .map_err(|error| format!("invalid pattern: {error}"))?;

    let walker = WalkBuilder::new(path)
        .hidden(true)
        .standard_filters(true)
        .build();

    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let mut matches: Vec<String> = Vec::new();
    let mut truncated = false;

    for entry in walker.flatten() {
        if matches.len() >= MAX_SEARCH_MATCHES {
            truncated = true;
            break;
        }

        let entry_path = entry.path();
        let is_file = entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file());
        if !is_file {
            continue;
        }

        let mut sink = MatchSink {
            path: entry_path.to_path_buf(),
            matches: &mut matches,
            truncated: &mut truncated,
        };

        let _ = searcher.search_path(&matcher, entry_path, &mut sink);
    }

    if matches.is_empty() {
        return Ok(format!("no matches found for '{pattern}'"));
    }

    if truncated {
        matches.push(format!(
            "\n[showing first {MAX_SEARCH_MATCHES} matches; refine your pattern to see more]"
        ));
    }
    Ok(matches.join("\n"))
}

struct MatchSink<'a> {
    path: PathBuf,
    matches: &'a mut Vec<String>,
    truncated: &'a mut bool,
}

impl Sink for MatchSink<'_> {
    type Error = io::Error;

    fn matched(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, io::Error> {
        if self.matches.len() >= MAX_SEARCH_MATCHES {
            *self.truncated = true;
            return Ok(false);
        }

        let line_number = mat.line_number().unwrap_or(0);
        let line = String::from_utf8_lossy(mat.bytes());
        let line = line.trim_end();
        self.matches
            .push(format!("{}:{line_number}:{line}", self.path.display()));
        Ok(true)
    }
}

fn fetch_url_tool() -> RegisteredTool {
    RegisteredTool::new(
        ToolDefinition {
            name: "fetch_url".to_string(),
            description: "Fetch text content from an HTTP or HTTPS URL.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "HTTP or HTTPS URL to fetch"
                    }
                },
                "required": ["url"]
            }),
        },
        Box::new(|args| {
            let url = args["url"].as_str().ok_or("url must be a string")?;
            execute_fetch_url(url)
        }),
    )
}

static FETCH_CLIENT: LazyLock<reqwest::blocking::Client> = LazyLock::new(|| {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("larpshell/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("failed to build HTTP client for fetch_url")
});

fn execute_fetch_url(url: &str) -> Result<String, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("url must start with http:// or https://".to_string());
    }

    let response = FETCH_CLIENT
        .get(url)
        .send()
        .map_err(|error| format!("cannot fetch URL: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("URL returned HTTP {status}"));
    }

    let content = response
        .text()
        .map_err(|error| format!("cannot read response body: {error}"))?;
    if content.len() > MAX_FETCH_SIZE {
        // Floor to the nearest char boundary at or below MAX_FETCH_SIZE so we
        // always truncate an oversize body regardless of codepoint widths.
        let cut = content
            .char_indices()
            .rev()
            .find(|(i, _)| *i <= MAX_FETCH_SIZE)
            .map_or(0, |(i, _)| i);
        let truncated = &content[..cut];
        Ok(format!(
            "{truncated}\n\n[truncated — response is {} bytes, showing first {cut}]",
            content.len()
        ))
    } else {
        Ok(content)
    }
}

fn run_command_tool(agent_mode: AgentMode) -> RegisteredTool {
    RegisteredTool::new(
        ToolDefinition {
            name: "run_command".to_string(),
            description: run_command_description(agent_mode).to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to execute"
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
        Box::new(move |args| {
            let command = args["command"].as_str().ok_or("command must be a string")?;
            let command_args = command_args(args);
            execute_run_command(agent_mode, command, &command_args)
        }),
    )
}

const fn run_command_description(agent_mode: AgentMode) -> &'static str {
    match agent_mode {
        AgentMode::Safe => "Run a restricted read-only command to gather context.",
        AgentMode::On => {
            "Run a shell command to gather context or for multi-step requests such as installing or setting up programs."
        }
        AgentMode::Off => "Run a command.",
    }
}

fn command_args(args: &serde_json::Value) -> Vec<String> {
    args.get("args")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn split_command(command: &str, args: &[String]) -> Result<(String, Vec<String>), String> {
    if !args.is_empty() {
        return Ok((command.to_string(), args.to_vec()));
    }

    let mut parts = command.split_whitespace();
    let executable = parts
        .next()
        .ok_or_else(|| "command must not be empty".to_string())?;

    Ok((
        executable.to_string(),
        parts.map(ToString::to_string).collect(),
    ))
}

fn has_shell_metacharacters(arg: &str) -> bool {
    [">", "|", ";", "&", "`"]
        .iter()
        .any(|token| arg.contains(token))
}

fn has_dangerous_flag(arg: &str) -> bool {
    DANGEROUS_FLAG_PREFIXES.iter().any(|flag| {
        arg == *flag
            || arg
                .strip_prefix(flag)
                .is_some_and(|suffix| suffix.starts_with('='))
    })
}

fn is_dangerous_argument_token(arg: &str) -> bool {
    DANGEROUS_ARGUMENT_TOKENS.contains(&arg)
}

fn is_dangerous_argument(arg: &str) -> bool {
    has_shell_metacharacters(arg) || has_dangerous_flag(arg) || is_dangerous_argument_token(arg)
}

/// Returns true when the config key in a `git -c key=value` pair can be used
/// to execute arbitrary code (pager, hooks, ssh command, aliases, etc.).
fn git_config_key_is_exec_capable(key: &str) -> bool {
    // Reject by suffix/substring; case-insensitive to match git's behaviour.
    let key_lc = key.to_ascii_lowercase();
    // Exact dangerous keys.
    const EXEC_KEYS: &[&str] = &[
        "core.pager",
        "core.sshcommand",
        "core.fsmonitor",
        "core.editor",
        "core.askpass",
    ];
    if EXEC_KEYS.contains(&key_lc.as_str()) {
        return true;
    }
    // Any alias.* key can run arbitrary commands.
    if key_lc.starts_with("alias.") {
        return true;
    }
    // Any *.pager key (e.g. pager.log, pager.diff).
    if key_lc.ends_with(".pager") {
        return true;
    }
    // Any key containing "cmd" or "command" (e.g. credential.helper, remote.*.uploadpack).
    if key_lc.contains("cmd") || key_lc.contains("command") {
        return true;
    }
    false
}

fn git_command_is_read_only(args: &[String]) -> bool {
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--version" | "version" | "help" => return true,
            "-c" => {
                // Inspect the key=value pair; reject exec-capable config keys.
                if let Some(pair) = args.get(i + 1) {
                    let key = pair.split('=').next().unwrap_or(pair);
                    if git_config_key_is_exec_capable(key) {
                        return false;
                    }
                }
                i += 2;
            }
            "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env" => {
                i += 2;
            }
            arg if arg.starts_with("--git-dir=")
                || arg.starts_with("--work-tree=")
                || arg.starts_with("--namespace=")
                || arg.starts_with("--config-env=") =>
            {
                i += 1;
            }
            arg if arg.starts_with('-') => {
                i += 1;
            }
            subcommand => return GIT_READ_ONLY_SUBCOMMANDS.contains(&subcommand),
        }
    }

    true
}

fn validate_safe_run_command(command: &str, args: &[String]) -> Result<(), String> {
    let command_base = Path::new(command)
        .file_stem()
        .and_then(|segment| segment.to_str())
        .unwrap_or(command);

    if !SAFE_COMMANDS.contains(&command_base) {
        return Err(format!("command not allowed: {command}"));
    }

    if command_base == "git" && !git_command_is_read_only(args) {
        return Err("dangerous git subcommand detected".to_string());
    }

    for arg in args {
        if is_dangerous_argument(arg) {
            return Err(format!("dangerous argument detected: {arg}"));
        }
    }

    Ok(())
}

fn execute_run_command(
    agent_mode: AgentMode,
    command: &str,
    args: &[String],
) -> Result<String, String> {
    let is_shell_expr = args.is_empty() && has_shell_metacharacters(command);

    if is_shell_expr && agent_mode.is_safe() {
        return Err(format!(
            "shell expressions not allowed in safe mode: {command}"
        ));
    }

    let (cmd, cmd_args) = if is_shell_expr {
        (
            "sh".to_string(),
            vec!["-c".to_string(), command.to_string()],
        )
    } else {
        let (c, a) = split_command(command, args)?;
        if agent_mode.is_safe() {
            validate_safe_run_command(&c, &a)?;
        }
        (c, a)
    };

    let output = Command::new(&cmd)
        .args(&cmd_args)
        .output()
        .map_err(|error| format!("failed to execute command: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("command failed: {}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn expand_tilde(path: &str) -> Cow<'_, str> {
    if path.starts_with('~') {
        let home = home_dir();
        let remaining = path.strip_prefix('~').unwrap_or("");
        let remaining = remaining.strip_prefix('/').unwrap_or(remaining);
        return Cow::Owned(home.join(remaining).to_string_lossy().into_owned());
    }
    Cow::Borrowed(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/tests")
            .join(format!("agent_builtins_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn assert_ok_trimmed(result: Result<String, String>, expected: &str) {
        assert_eq!(result.unwrap().trim(), expected);
    }

    fn assert_err_contains(result: Result<String, String>, expected: &str) {
        assert!(result.unwrap_err().contains(expected));
    }

    fn test_http_server(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer);
            stream.write_all(response.as_bytes()).unwrap();
        });
        url
    }

    fn http_response(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
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
        assert_err_contains(execute_read_file("/nonexistent/file.txt"), "file not found");
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
    fn write_file_writes_contents() {
        let dir = test_dir("write");
        let file_path = dir.join("hello.txt");

        let result = execute_write_file(file_path.to_str().unwrap(), "hello world").unwrap();

        assert!(result.contains("wrote 11 bytes"));
        assert_eq!(fs::read_to_string(file_path).unwrap(), "hello world");
    }

    #[test]
    fn write_file_creates_parent_directories() {
        let dir = test_dir("write_nested");
        let file_path = dir.join("nested/hello.txt");

        execute_write_file(file_path.to_str().unwrap(), "hello").unwrap();

        assert_eq!(fs::read_to_string(file_path).unwrap(), "hello");
    }

    #[test]
    fn write_file_rejects_directory_path() {
        let dir = test_dir("write_dir");

        assert_err_contains(
            execute_write_file(dir.to_str().unwrap(), "hello"),
            "cannot write to directory",
        );
    }

    #[test]
    fn edit_file_replaces_unique_text() {
        let dir = test_dir("edit");
        let file_path = dir.join("hello.txt");
        fs::write(&file_path, "hello world").unwrap();

        let result = execute_edit_file(file_path.to_str().unwrap(), "world", "there").unwrap();

        assert!(result.contains("replaced 1 occurrence"));
        assert_eq!(fs::read_to_string(file_path).unwrap(), "hello there");
    }

    #[test]
    fn edit_file_rejects_missing_text() {
        let dir = test_dir("edit_missing");
        let file_path = dir.join("hello.txt");
        fs::write(&file_path, "hello world").unwrap();

        assert_err_contains(
            execute_edit_file(file_path.to_str().unwrap(), "nope", "there"),
            "old_text not found",
        );
    }

    #[test]
    fn edit_file_rejects_duplicate_text() {
        let dir = test_dir("edit_duplicate");
        let file_path = dir.join("hello.txt");
        fs::write(&file_path, "hello hello").unwrap();

        assert_err_contains(
            execute_edit_file(file_path.to_str().unwrap(), "hello", "hi"),
            "old_text matched 2 times",
        );
    }

    #[test]
    fn edit_file_rejects_empty_old_text() {
        let dir = test_dir("edit_empty");
        let file_path = dir.join("hello.txt");
        fs::write(&file_path, "hello").unwrap();

        assert_err_contains(
            execute_edit_file(file_path.to_str().unwrap(), "", "hi"),
            "old_text must not be empty",
        );
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
        assert_err_contains(execute_list_files("/nonexistent/dir"), "not a directory");
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
    fn search_files_appends_truncation_notice_when_cap_hit() {
        let dir = test_dir("search_truncate");
        // One file with MAX_SEARCH_MATCHES + 5 matching lines — guarantees the cap fires.
        let content = "match\n".repeat(MAX_SEARCH_MATCHES + 5);
        fs::write(dir.join("many.txt"), &content).unwrap();

        let result = execute_search_files("match", dir.to_str().unwrap()).unwrap();
        let lines: Vec<&str> = result.lines().collect();

        // Exactly MAX_SEARCH_MATCHES match lines plus the truncation notice.
        let match_lines = lines.iter().filter(|l| l.contains("many.txt:")).count();
        assert_eq!(match_lines, MAX_SEARCH_MATCHES);
        assert!(result.contains("showing first 50 matches; refine your pattern to see more"));
    }

    #[test]
    fn fetch_url_returns_response_body() {
        let url = test_http_server(Box::leak(
            http_response("200 OK", "hello from server").into_boxed_str(),
        ));

        let result = execute_fetch_url(&url).unwrap();

        assert_eq!(result, "hello from server");
    }

    #[test]
    fn fetch_url_rejects_non_http_urls() {
        assert_err_contains(
            execute_fetch_url("file:///etc/passwd"),
            "url must start with http:// or https://",
        );
    }

    #[test]
    fn fetch_url_rejects_error_statuses() {
        let url = test_http_server(Box::leak(
            http_response("404 Not Found", "nope").into_boxed_str(),
        ));

        assert_err_contains(execute_fetch_url(&url), "URL returned HTTP 404 Not Found");
    }

    #[test]
    fn fetch_url_truncates_large_responses() {
        let body = "x".repeat(MAX_FETCH_SIZE + 1000);
        let response = http_response("200 OK", &body);
        let url = test_http_server(Box::leak(response.into_boxed_str()));

        let result = execute_fetch_url(&url).unwrap();

        assert!(result.contains("[truncated — response is 103400 bytes"));
    }

    #[test]
    fn run_command_executes_safe_commands() {
        assert_ok_trimmed(
            execute_run_command(AgentMode::Safe, "echo", &["hello world".to_string()]),
            "hello world",
        );
    }

    #[test]
    fn run_command_safe_accepts_combined_command_string() {
        assert_ok_trimmed(
            execute_run_command(AgentMode::Safe, "echo hello world", &[]),
            "hello world",
        );
    }

    #[test]
    fn run_command_rejects_unsafe_commands() {
        assert_err_contains(
            execute_run_command(AgentMode::Safe, "rm", &["-rf".to_string(), "/".to_string()]),
            "command not allowed",
        );
    }

    #[test]
    fn run_command_safe_rejects_dangerous_combined_command_string() {
        assert_err_contains(
            execute_run_command(AgentMode::Safe, "ls --force", &[]),
            "dangerous argument",
        );
    }

    #[test]
    fn run_command_rejects_dangerous_args() {
        assert_err_contains(
            execute_run_command(AgentMode::Safe, "ls", &["--force".to_string()]),
            "dangerous argument",
        );
    }

    #[test]
    fn run_command_safe_allows_benign_args_with_blocked_substrings() {
        assert_ok_trimmed(
            execute_run_command(AgentMode::Safe, "echo", &["tcp".to_string()]),
            "tcp",
        );
    }

    #[test]
    fn run_command_safe_rejects_mutating_git_subcommands() {
        assert_err_contains(
            execute_run_command(AgentMode::Safe, "git", &["init".to_string()]),
            "dangerous git subcommand",
        );
    }

    #[test]
    fn run_command_safe_allows_read_only_git_subcommands() {
        let result = execute_run_command(AgentMode::Safe, "git", &["--version".to_string()]);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("git version"));
    }

    #[test]
    fn run_command_safe_allows_git_global_option_before_read_only_subcommand() {
        let result = execute_run_command(
            AgentMode::Safe,
            "git",
            &[
                "-c".to_string(),
                "color.ui=always".to_string(),
                "--version".to_string(),
            ],
        );
        assert!(result.is_ok());
        assert!(result.unwrap().contains("git version"));
    }

    #[test]
    fn run_command_safe_allows_literal_dollar_argument() {
        assert_ok_trimmed(
            execute_run_command(AgentMode::Safe, "echo", &["$HOME".to_string()]),
            "$HOME",
        );
    }

    #[test]
    fn run_command_on_allows_arbitrary_commands() {
        assert_ok_trimmed(
            execute_run_command(AgentMode::On, "echo", &["hello world".to_string()]),
            "hello world",
        );
    }

    #[test]
    fn run_command_on_accepts_combined_command_string() {
        assert_ok_trimmed(
            execute_run_command(AgentMode::On, "echo hello world", &[]),
            "hello world",
        );
    }

    #[test]
    fn run_command_on_allows_previously_blocked_args() {
        assert_ok_trimmed(
            execute_run_command(AgentMode::On, "echo", &["--force".to_string()]),
            "--force",
        );
    }

    #[test]
    fn run_command_on_executes_compound_shell_expression() {
        assert_ok_trimmed(
            execute_run_command(AgentMode::On, "echo foo && echo bar", &[]),
            "foo
bar",
        );
    }

    #[test]
    fn run_command_safe_rejects_compound_shell_expression() {
        assert_err_contains(
            execute_run_command(AgentMode::Safe, "echo foo && echo bar", &[]),
            "shell expressions not allowed in safe mode",
        );
    }

    // ── safe-mode allowlist: write-capable / state-mutating commands ─────────

    #[test]
    fn run_command_safe_rejects_curl() {
        assert_err_contains(
            execute_run_command(
                AgentMode::Safe,
                "curl",
                &[
                    "-o".to_string(),
                    "/tmp/x".to_string(),
                    "http://example.com".to_string(),
                ],
            ),
            "command not allowed",
        );
    }

    #[test]
    fn run_command_safe_rejects_wget() {
        assert_err_contains(
            execute_run_command(
                AgentMode::Safe,
                "wget",
                &[
                    "-O".to_string(),
                    "/tmp/x".to_string(),
                    "http://example.com".to_string(),
                ],
            ),
            "command not allowed",
        );
    }

    #[test]
    fn run_command_safe_rejects_docker() {
        assert_err_contains(
            execute_run_command(AgentMode::Safe, "docker", &["ps".to_string()]),
            "command not allowed",
        );
    }

    #[test]
    fn run_command_safe_rejects_kubectl() {
        assert_err_contains(
            execute_run_command(
                AgentMode::Safe,
                "kubectl",
                &["get".to_string(), "pods".to_string()],
            ),
            "command not allowed",
        );
    }

    #[test]
    fn run_command_safe_rejects_terraform() {
        assert_err_contains(
            execute_run_command(AgentMode::Safe, "terraform", &["plan".to_string()]),
            "command not allowed",
        );
    }

    // ── regression: bug fixes ────────────────────────────────────────────────

    #[test]
    fn fetch_url_truncates_multibyte_unicode_body_at_char_boundary() {
        // Build a body whose byte length exceeds MAX_FETCH_SIZE but whose last
        // few bytes before the cap land in the middle of a multi-byte codepoint.
        // Each '€' is 3 UTF-8 bytes; fill just past the cap so the naive
        // byte-slice would panic or silently return the full string.
        let unit = '€'; // 3-byte codepoint
        let unit_bytes = unit.len_utf8();
        // Repeat enough times that the total byte length is slightly above MAX_FETCH_SIZE.
        let count = MAX_FETCH_SIZE / unit_bytes + 1;
        let body: String = std::iter::repeat_n(unit, count).collect();
        assert!(
            body.len() > MAX_FETCH_SIZE,
            "body must exceed cap for test to be meaningful"
        );

        let response = http_response("200 OK", &body);
        let url = test_http_server(Box::leak(response.into_boxed_str()));

        let result = execute_fetch_url(&url).unwrap();
        // The truncation notice must be present — the body was over the limit.
        assert!(
            result.contains("[truncated"),
            "expected truncation notice in: {result}"
        );
        // The prefix before the notice must be valid UTF-8 (no mid-codepoint cut).
        let prefix = result.split("\n\n[truncated").next().unwrap_or("");
        assert!(std::str::from_utf8(prefix.as_bytes()).is_ok());
        // The prefix must be strictly shorter than the full body.
        assert!(
            prefix.len() < body.len(),
            "prefix should be shorter than the full body"
        );
    }

    #[test]
    fn git_safe_mode_rejects_exec_capable_config_key() {
        // `core.pager` can execute arbitrary commands; must be rejected in safe mode.
        assert_err_contains(
            execute_run_command(
                AgentMode::Safe,
                "git",
                &[
                    "-c".to_string(),
                    "core.pager=evil-cmd".to_string(),
                    "log".to_string(),
                ],
            ),
            "dangerous git subcommand",
        );
    }

    #[test]
    fn git_safe_mode_rejects_alias_config_key() {
        assert_err_contains(
            execute_run_command(
                AgentMode::Safe,
                "git",
                &[
                    "-c".to_string(),
                    "alias.x=!evil".to_string(),
                    "status".to_string(),
                ],
            ),
            "dangerous git subcommand",
        );
    }

    #[test]
    fn git_safe_mode_accepts_benign_config_key_with_read_only_subcommand() {
        // `color.ui` cannot execute code; a read-only subcommand must be allowed.
        let result = execute_run_command(
            AgentMode::Safe,
            "git",
            &[
                "-c".to_string(),
                "color.ui=never".to_string(),
                "log".to_string(),
                "--oneline".to_string(),
                "-1".to_string(),
            ],
        );
        // We may not be in a git repo in CI, but the rejection must NOT happen.
        // Accept both success and a git error; reject only a "dangerous" error.
        if let Err(e) = result {
            assert!(
                !e.contains("dangerous"),
                "benign -c key should not be rejected; got: {e}"
            );
        }
    }

    #[test]
    fn read_file_truncation_notice_reports_actual_bytes_read() {
        let dir = test_dir("read_actual_bytes");
        let file_path = dir.join("big.txt");
        // Create a file larger than the cap with distinct content.
        let content = "A".repeat(MAX_FILE_SIZE + 500);
        fs::write(&file_path, &content).unwrap();

        let result = execute_read_file(file_path.to_str().unwrap()).unwrap();

        // The notice must mention the actual bytes read (≤ MAX_FILE_SIZE),
        // not the hardcoded constant as if a full read always occurred.
        assert!(result.contains("[truncated"), "expected truncation notice");
        // Extract the "showing first N" number from the notice.
        let notice = result.split("[truncated").nth(1).unwrap_or("");
        let showing_count: usize = notice
            .split("showing first ")
            .nth(1)
            .and_then(|s| s.split(']').next())
            .and_then(|s| s.trim().parse().ok())
            .expect("could not parse byte count from truncation notice");
        // Must be ≤ MAX_FILE_SIZE (not the hardcoded constant stated regardless of actual read).
        assert!(
            showing_count <= MAX_FILE_SIZE,
            "showing count {showing_count} exceeds MAX_FILE_SIZE {MAX_FILE_SIZE}"
        );
        // For an all-ASCII file, the actual read should equal the cap exactly.
        assert_eq!(
            showing_count, MAX_FILE_SIZE,
            "ASCII file should fill the buffer completely"
        );
    }
}
