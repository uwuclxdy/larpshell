pub mod mcp;
pub mod tools;

use colored::*;

use crate::cli::print_warning;
use crate::common::{
    CTP_BLUE, CTP_GREEN, CTP_OVERLAY0, CTP_PRIMARY, CTP_RED, CTP_YELLOW, clear_line,
    count_visual_lines, current_directory, eprint_flush, hide_cursor, os_name, shell_name,
    show_cursor, terminal_width, username,
};
use crate::config::{AgentMode, Config, load_agent_prompt, load_agent_safe_prompt};
use crate::error::LarpshellError;
use crate::prompt::{DEFAULT_AGENT_PROMPT, DEFAULT_AGENT_SAFE_PROMPT};
use crate::providers::{AIProvider, ChatMessage, ChatResponse, ToolCall};
use tools::ToolRegistry;

const MAX_AGENT_ITERATIONS: usize = 10;

fn substitute_agent_prompt(template: &str, user_request: &str) -> String {
    let cwd = current_directory();
    let os = os_name();
    let shell = shell_name();
    let home = dirs::home_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "~".to_string());
    let user = username();

    template
        .replace("{cwd}", &cwd)
        .replace("{home}", &home)
        .replace("{user}", &user)
        .replace("{shell}", &shell)
        .replace("{os}", &os)
        .replace("{request}", user_request)
}

fn build_agent_system_prompt(agent_mode: AgentMode, user_request: &str) -> String {
    let template = match agent_mode {
        AgentMode::On => load_agent_prompt().unwrap_or_else(|| DEFAULT_AGENT_PROMPT.to_string()),
        AgentMode::Safe | AgentMode::Off => {
            load_agent_safe_prompt().unwrap_or_else(|| DEFAULT_AGENT_SAFE_PROMPT.to_string())
        }
    };
    substitute_agent_prompt(&template, user_request)
}

pub enum ToolConfirmResult {
    Allow,
    Deny,
    Cancel,
}

enum Key {
    Enter,
    Char(char),
    CtrlC,
    Other,
}

fn string_argument<'a>(
    arguments: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: &'a str,
) -> &'a str {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or(default)
}

fn run_command_preview(arguments: &serde_json::Map<String, serde_json::Value>) -> String {
    let command = string_argument(arguments, "command", "");
    let args = arguments
        .get("args")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let full_command = if args.is_empty() {
        command.to_string()
    } else {
        format!("{command} {args}")
    };
    format!("{} {}", "run".custom_color(CTP_BLUE), full_command.italic())
}

fn generic_tool_preview(
    tool_name: &str,
    arguments: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let parts = arguments
        .iter()
        .map(|(key, value)| {
            let value_str = match value {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            format!("{key}: {value_str}")
        })
        .collect::<Vec<_>>();

    if parts.is_empty() {
        format!("{}", tool_name.bold())
    } else {
        format!("{} with {}", tool_name.bold(), parts.join(", "))
    }
}

fn format_tool_preview(
    tool_name: &str,
    arguments: &serde_json::Map<String, serde_json::Value>,
) -> String {
    match tool_name {
        "run_command" => run_command_preview(arguments),
        "read_file" => format!(
            "{} {}",
            "read".custom_color(CTP_BLUE),
            string_argument(arguments, "file_path", "").italic()
        ),
        "list_files" => format!(
            "{} in {}",
            "list files".custom_color(CTP_BLUE),
            string_argument(arguments, "directory_path", "").italic()
        ),
        "search_files" => format!(
            "{} for {} in {}",
            "search".custom_color(CTP_BLUE),
            string_argument(arguments, "pattern", "").italic(),
            string_argument(arguments, "directory_path", ".").italic()
        ),
        _ => generic_tool_preview(tool_name, arguments),
    }
}

fn execute_tool_call(tool_registry: &ToolRegistry, tool_call: &ToolCall) -> String {
    let result = tool_registry.execute(&tool_call.name, tool_call.arguments.clone());
    match &result {
        Ok(output) => display_tool_result(output),
        Err(error) => display_tool_error(error),
    }

    match result {
        Ok(output) => output,
        Err(error) => format!("Error: {error}"),
    }
}

fn append_tool_messages(
    messages: &mut Vec<ChatMessage>,
    tool_call: &ToolCall,
    result: impl Into<String>,
) {
    messages.push(ChatMessage::assistant_tool_calls(vec![tool_call.clone()]));
    messages.push(ChatMessage::tool_result(&tool_call.id, result.into()));
}

fn denied_tool_result() -> &'static str {
    "Tool call denied by user. Try a different approach or produce the final command."
}

fn initial_agent_messages(user_input: &str, config: &Config) -> Vec<ChatMessage> {
    let system_prompt = build_agent_system_prompt(config.agent, user_input);
    vec![
        ChatMessage::system(system_prompt),
        ChatMessage::user(user_input),
    ]
}

fn show_agent_start(config: &Config) -> Result<(), LarpshellError> {
    let model_name = config.provider_config()?.config.model().to_string();
    hide_cursor();
    eprint_flush(&format!(
        "{}",
        format!("using {} (agent)...", model_name).custom_color(CTP_OVERLAY0)
    ));
    Ok(())
}

fn clear_agent_status() {
    clear_line();
    show_cursor();
}

async fn next_agent_response(
    provider: &dyn AIProvider,
    messages: &[ChatMessage],
    tool_definitions: &[crate::providers::ToolDefinition],
) -> Result<ChatResponse, LarpshellError> {
    let response = provider
        .generate_with_tools(messages, tool_definitions)
        .await;
    clear_agent_status();
    response
}

fn handle_tool_calls<F>(
    tool_calls: &[ToolCall],
    tool_registry: &ToolRegistry,
    messages: &mut Vec<ChatMessage>,
    confirm_tool: &mut F,
) -> Result<(), LarpshellError>
where
    F: FnMut(&ToolCall) -> ToolConfirmResult,
{
    for tool_call in tool_calls {
        display_tool_call(tool_call);

        match confirm_tool(tool_call) {
            ToolConfirmResult::Allow => {
                let result_text = execute_tool_call(tool_registry, tool_call);
                append_tool_messages(messages, tool_call, result_text);
            }
            ToolConfirmResult::Deny => {
                print_warning("tool call denied.");
                append_tool_messages(messages, tool_call, denied_tool_result());
            }
            ToolConfirmResult::Cancel => return Err(LarpshellError::Cancelled),
        }
    }

    Ok(())
}

fn show_next_iteration_prompt(iteration: usize) {
    if iteration < MAX_AGENT_ITERATIONS - 1 {
        hide_cursor();
        eprint_flush(&format!("{}", "thinking...".custom_color(CTP_OVERLAY0)));
    }
}

fn handle_agent_response<F>(
    response: ChatResponse,
    tool_registry: &ToolRegistry,
    messages: &mut Vec<ChatMessage>,
    confirm_tool: &mut F,
) -> Result<Option<String>, LarpshellError>
where
    F: FnMut(&ToolCall) -> ToolConfirmResult,
{
    match response {
        ChatResponse::Message(text) => Ok(Some(text)),
        ChatResponse::ToolCalls(tool_calls) => {
            handle_tool_calls(&tool_calls, tool_registry, messages, confirm_tool)?;
            Ok(None)
        }
    }
}

fn max_iterations_error() -> LarpshellError {
    LarpshellError::AgentMaxIterations(MAX_AGENT_ITERATIONS)
}

fn agent_context(
    user_input: &str,
    config: &Config,
    tool_registry: &ToolRegistry,
) -> Result<(Vec<ChatMessage>, Vec<crate::providers::ToolDefinition>), LarpshellError> {
    show_agent_start(config)?;
    Ok((
        initial_agent_messages(user_input, config),
        tool_registry.definitions(),
    ))
}

fn continue_after_response(iteration: usize) {
    show_next_iteration_prompt(iteration);
}

fn agent_iteration_error() -> LarpshellError {
    max_iterations_error()
}

async fn provider_response(
    provider: &dyn AIProvider,
    messages: &[ChatMessage],
    tool_definitions: &[crate::providers::ToolDefinition],
) -> Result<ChatResponse, LarpshellError> {
    next_agent_response(provider, messages, tool_definitions).await
}

fn tool_response<F>(
    response: ChatResponse,
    tool_registry: &ToolRegistry,
    messages: &mut Vec<ChatMessage>,
    confirm_tool: &mut F,
) -> Result<Option<String>, LarpshellError>
where
    F: FnMut(&ToolCall) -> ToolConfirmResult,
{
    handle_agent_response(response, tool_registry, messages, confirm_tool)
}

fn display_tool_call(tool_call: &ToolCall) -> usize {
    let width = terminal_width();
    let mut lines = 0;

    // Create user-friendly preview
    if let Some(arguments) = tool_call.arguments.as_object() {
        let preview = format_tool_preview(&tool_call.name, arguments);
        let preview_line = format!("  {} {}", "tool".custom_color(CTP_OVERLAY0), preview);
        eprintln!("{preview_line}");
        lines += count_visual_lines(&preview_line, width);
    } else {
        // Fallback for tools with no arguments
        let tool_line = format!(
            "  {}  {}",
            "tool".custom_color(CTP_OVERLAY0),
            tool_call.name.custom_color(CTP_BLUE).bold()
        );
        eprintln!("{tool_line}");
        lines += count_visual_lines(&tool_line, width);
    }

    lines
}

fn confirm_tool_call() -> ToolConfirmResult {
    let prompt = format!(
        "  {} [{}] allow, [{}] deny, [{}] cancel",
        "Allow?".custom_color(CTP_YELLOW),
        "Y/Enter".custom_color(CTP_PRIMARY).bold(),
        "N".custom_color(CTP_PRIMARY).bold(),
        "Ctrl+C".custom_color(CTP_PRIMARY).bold()
    );
    eprint!("{}", prompt.custom_color(CTP_BLUE));
    let _ = std::io::Write::flush(&mut std::io::stderr());

    #[cfg(unix)]
    {
        use nix::sys::termios::FlushArg;
        let _ = nix::sys::termios::tcflush(std::io::stdin(), FlushArg::TCIFLUSH);
    }

    loop {
        match read_key() {
            Key::Enter | Key::Char('y') | Key::Char('Y') => {
                clear_line();
                return ToolConfirmResult::Allow;
            }
            Key::Char('n') | Key::Char('N') => {
                clear_line();
                return ToolConfirmResult::Deny;
            }
            Key::CtrlC => {
                clear_line();
                return ToolConfirmResult::Cancel;
            }
            Key::Other | Key::Char(_) => {}
        }
    }
}

fn parse_byte(b: u8) -> Key {
    match b {
        b'\n' | b'\r' => Key::Enter,
        b'\x03' => Key::CtrlC,
        ch @ 32..=126 => Key::Char(ch as char),
        _ => Key::Other,
    }
}

fn read_key() -> Key {
    #[cfg(unix)]
    {
        use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};

        let stdin = std::io::stdin();
        if let Ok(original) = tcgetattr(&stdin) {
            let mut raw = original.clone();
            raw.local_flags
                .remove(LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG);

            if tcsetattr(&stdin, SetArg::TCSANOW, &raw).is_ok() {
                let mut buffer = [0u8; 1];
                let read_result =
                    if std::io::Read::read(&mut stdin.lock(), &mut buffer).unwrap_or(0) == 0 {
                        Key::Other
                    } else {
                        parse_byte(buffer[0])
                    };
                let _ = tcsetattr(&stdin, SetArg::TCSANOW, &original);
                return read_result;
            }

            let _ = tcsetattr(&stdin, SetArg::TCSANOW, &original);
        }
    }

    let mut buffer = [0u8; 1];
    if std::io::Read::read(&mut std::io::stdin().lock(), &mut buffer).unwrap_or(0) == 0 {
        return Key::Other;
    }

    parse_byte(buffer[0])
}

fn display_tool_result(result: &str) {
    let line_count = result.lines().count();
    let line_word = if line_count == 1 { "line" } else { "lines" };
    eprintln!(
        "  {} {}",
        "result".custom_color(CTP_OVERLAY0),
        format!("({} {})", line_count, line_word).custom_color(CTP_GREEN)
    );
    eprintln!();
}

fn command_not_allowed_tip(error: &str) -> Option<ColoredString> {
    let text = format!("run {} to enable all commands", "/agent on".bold());
    error
        .starts_with("command not allowed:")
        .then(|| text.italic().custom_color(CTP_OVERLAY0))
}

fn display_tool_error(error: &str) {
    eprintln!(
        "  {} {}",
        "error".custom_color(CTP_OVERLAY0),
        error.custom_color(CTP_RED)
    );
    if let Some(tip) = command_not_allowed_tip(error) {
        eprintln!("  {} {}", "tip:".custom_color(CTP_OVERLAY0).italic(), tip);
    }
    eprintln!();
}

async fn run_agent_loop_with_confirm<F>(
    user_input: &str,
    provider: &dyn AIProvider,
    config: &Config,
    tool_registry: &ToolRegistry,
    mut confirm_tool: F,
) -> Result<String, LarpshellError>
where
    F: FnMut(&ToolCall) -> ToolConfirmResult,
{
    let (mut messages, tool_definitions) = agent_context(user_input, config, tool_registry)?;

    for iteration in 0..MAX_AGENT_ITERATIONS {
        let response = match provider_response(provider, &messages, &tool_definitions).await {
            Ok(response) => response,
            Err(error) => return Err(error),
        };

        if let Some(text) =
            tool_response(response, tool_registry, &mut messages, &mut confirm_tool)?
        {
            return Ok(text);
        }

        continue_after_response(iteration);
    }

    Err(agent_iteration_error())
}

pub async fn run_agent_loop(
    user_input: &str,
    provider: &dyn AIProvider,
    config: &Config,
    tool_registry: &ToolRegistry,
) -> Result<String, LarpshellError> {
    run_agent_loop_with_confirm(user_input, provider, config, tool_registry, |_| {
        confirm_tool_call()
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ActiveProvider, Config, MultiProviderConfig, OllamaConfig, ProviderSpecificConfig,
    };
    use crate::providers::{ChatResponse, Role, ToolDefinition};
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::Mutex;

    struct MockProvider {
        responses: Mutex<VecDeque<ChatResponse>>,
        captured_messages: Mutex<Vec<Vec<ChatMessage>>>,
    }

    impl MockProvider {
        fn new(responses: Vec<ChatResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                captured_messages: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AIProvider for MockProvider {
        async fn generate(&self, _prompt: &str) -> Result<String, LarpshellError> {
            unreachable!("generate should not be called")
        }

        async fn generate_with_tools(
            &self,
            messages: &[ChatMessage],
            _tools: &[ToolDefinition],
        ) -> Result<ChatResponse, LarpshellError> {
            self.captured_messages
                .lock()
                .unwrap()
                .push(messages.to_vec());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| LarpshellError::InvalidResponse("missing mock response".to_string()))
        }

        fn name(&self) -> String {
            "mock".to_string()
        }
    }

    fn test_config() -> Config {
        Config {
            active_provider: ActiveProvider::Ollama,
            providers: MultiProviderConfig {
                ollama: Some(OllamaConfig {
                    base_url: "http://localhost:11434".to_string(),
                    model: "llama3".to_string(),
                }),
                ..Default::default()
            },
            agent: AgentMode::Safe,
        }
    }

    fn make_test_directory(name: &str) -> std::path::PathBuf {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        let path = std::env::temp_dir().join(format!(
            "larpshell_agent_{name}_{}",
            NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn build_agent_system_prompt_includes_request() {
        let prompt = substitute_agent_prompt(DEFAULT_AGENT_SAFE_PROMPT, "list the rust files");

        assert!(prompt.contains("User request: list the rust files"));
        assert!(prompt.contains("Current dir:"));
        assert!(prompt.contains("Shell:"));
    }

    #[test]
    fn build_agent_system_prompt_for_on_mentions_run_command() {
        let prompt = substitute_agent_prompt(DEFAULT_AGENT_PROMPT, "inspect the environment");

        assert!(prompt.contains("use the run_command tool"));
        assert!(prompt.contains("iterative probing"));
    }

    #[test]
    fn build_agent_system_prompt_for_safe_is_conservative() {
        let prompt = substitute_agent_prompt(DEFAULT_AGENT_SAFE_PROMPT, "inspect the environment");

        assert!(prompt.contains("Use tools conservatively"));
        assert!(!prompt.contains("use the run_command tool"));
    }

    #[tokio::test]
    async fn run_agent_loop_returns_message_without_tool_calls() {
        let provider = MockProvider::new(vec![ChatResponse::Message("ls -la".to_string())]);
        let tool_registry = ToolRegistry::with_builtins(AgentMode::Safe);

        let command = run_agent_loop_with_confirm(
            "list files",
            &provider,
            &test_config(),
            &tool_registry,
            |_| ToolConfirmResult::Allow,
        )
        .await
        .unwrap();

        assert_eq!(command, "ls -la");
    }

    #[tokio::test]
    async fn run_agent_loop_executes_tool_call_and_returns_follow_up_message() {
        let directory = make_test_directory("list_files");
        fs::write(directory.join("hello.txt"), "hello").unwrap();

        let provider = MockProvider::new(vec![
            ChatResponse::ToolCalls(vec![crate::providers::ToolCall {
                id: "tool-1".to_string(),
                name: "list_files".to_string(),
                arguments: serde_json::json!({
                    "directory_path": directory.display().to_string()
                }),
            }]),
            ChatResponse::Message("cat hello.txt".to_string()),
        ]);
        let tool_registry = ToolRegistry::with_builtins(AgentMode::Safe);

        let command = run_agent_loop_with_confirm(
            "show me the file",
            &provider,
            &test_config(),
            &tool_registry,
            |_| ToolConfirmResult::Allow,
        )
        .await
        .unwrap();

        assert_eq!(command, "cat hello.txt");

        let captured_messages = provider.captured_messages.lock().unwrap();
        assert_eq!(captured_messages.len(), 2);
        assert!(
            captured_messages[1]
                .iter()
                .any(|message| message.role == Role::Assistant && message.tool_calls.is_some())
        );
        assert!(
            captured_messages[1]
                .iter()
                .any(|message| message.role == Role::Tool && message.content.is_some())
        );
    }

    #[tokio::test]
    async fn run_agent_loop_returns_max_iterations_error() {
        let tool_call = crate::providers::ToolCall {
            id: "tool-1".to_string(),
            name: "search_files".to_string(),
            arguments: serde_json::json!({ "pattern": "main" }),
        };
        let responses = std::iter::repeat_n(
            ChatResponse::ToolCalls(vec![tool_call]),
            MAX_AGENT_ITERATIONS,
        )
        .collect();
        let provider = MockProvider::new(responses);
        let tool_registry = ToolRegistry::with_builtins(AgentMode::Safe);

        let error = run_agent_loop_with_confirm(
            "find main",
            &provider,
            &test_config(),
            &tool_registry,
            |_| ToolConfirmResult::Deny,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            LarpshellError::AgentMaxIterations(MAX_AGENT_ITERATIONS)
        ));
    }

    #[test]
    fn test_config_uses_ollama_provider() {
        let provider_config = test_config().provider_config().unwrap();
        assert!(matches!(
            provider_config.config,
            ProviderSpecificConfig::Ollama { .. }
        ));
    }

    #[test]
    fn format_tool_preview_creates_user_friendly_messages() {
        use serde_json::json;

        fn plain(tool: &str, args: &serde_json::Map<String, serde_json::Value>) -> String {
            let preview = format_tool_preview(tool, args);
            String::from_utf8_lossy(&strip_ansi_escapes::strip(&preview)).into_owned()
        }

        fn plain_tip(error: &str) -> Option<String> {
            command_not_allowed_tip(error)
                .map(|tip| String::from_utf8_lossy(&strip_ansi_escapes::strip(&*tip)).into_owned())
        }

        // Test run_command with simple command
        let mut args = serde_json::Map::new();
        args.insert("command".to_string(), json!("ls"));
        let preview = plain("run_command", &args);
        assert!(preview.contains("run"));
        assert!(preview.contains("ls"));

        // Test run_command with command and args
        let mut args = serde_json::Map::new();
        args.insert("command".to_string(), json!("grep"));
        args.insert("args".to_string(), json!(["pattern", "file.txt"]));
        let preview = plain("run_command", &args);
        assert!(preview.contains("run"));
        assert!(preview.contains("grep pattern file.txt"));

        // Test read_file
        let mut args = serde_json::Map::new();
        args.insert("file_path".to_string(), json!("/home/user/file.txt"));
        let preview = plain("read_file", &args);
        assert_eq!(preview, "read /home/user/file.txt");

        // Test list_files
        let mut args = serde_json::Map::new();
        args.insert("directory_path".to_string(), json!("/home/user"));
        let preview = plain("list_files", &args);
        assert_eq!(preview, "list files in /home/user");

        // Test search_files
        let mut args = serde_json::Map::new();
        args.insert("pattern".to_string(), json!("main"));
        args.insert("directory_path".to_string(), json!("/src"));
        let preview = plain("search_files", &args);
        assert_eq!(preview, "search for main in /src");

        // Test unknown tool
        let mut args = serde_json::Map::new();
        args.insert("param1".to_string(), json!("value1"));
        args.insert("param2".to_string(), json!("value2"));
        let preview = plain("unknown_tool", &args);
        assert_eq!(preview, "unknown_tool with param1: value1, param2: value2");

        let tip = plain_tip("command not allowed: rm").unwrap();
        assert!(tip.contains("run /agent on to enable all commands"));

        assert!(plain_tip("dangerous argument detected: --force").is_none());
    }
}
