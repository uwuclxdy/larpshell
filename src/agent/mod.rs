pub mod builtins;
pub mod mcp;
pub mod tools;

use colored::Colorize;

use crate::cli::print_warning;
use crate::common::{
    CTP_BLUE, CTP_GREEN, CTP_OVERLAY0, CTP_PRIMARY, CTP_RED, CTP_YELLOW, clear_line, eprint_flush,
    hide_cursor, show_cursor,
};
use crate::config::{
    AgentMode, Config, load_agent_prompt, load_agent_safe_prompt, load_sys_prompt,
};
use crate::confirmation::style_message_markup;
use crate::error::LarpshellError;
use crate::prompt::{
    DEFAULT_AGENT_PROMPT, DEFAULT_AGENT_SAFE_PROMPT, DEFAULT_PROMPT_TEMPLATE, create_system_prompt,
    parse_labeled_response, validate_sys_prompt,
};
use crate::providers::{AIProvider, ChatMessage, ChatResponse, ToolCall};
use tools::ToolRegistry;

const MAX_AGENT_ITERATIONS: usize = 25;
const TOOL_OUTPUT_LINE_CAP: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalResponseKind {
    Command,
    Message,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalResponse {
    pub kind: FinalResponseKind,
    pub content: String,
    pub message: Option<String>,
    pub command: Option<String>,
}

fn compose_agent_system_prompt(
    agent_prompt: &str,
    user_request: &str,
    system_template: &str,
) -> String {
    let system_prompt = create_system_prompt(user_request, Some(system_template));
    format!("{agent_prompt}\n\n{system_prompt}")
}

fn build_agent_system_prompt(agent_mode: AgentMode, user_request: &str) -> String {
    let agent_prompt = match agent_mode {
        AgentMode::On => load_agent_prompt().unwrap_or_else(|| DEFAULT_AGENT_PROMPT.to_string()),
        AgentMode::Safe | AgentMode::Off => {
            load_agent_safe_prompt().unwrap_or_else(|| DEFAULT_AGENT_SAFE_PROMPT.to_string())
        }
    };
    let system_template = load_sys_prompt()
        .filter(|template| validate_sys_prompt(template))
        .unwrap_or_else(|| DEFAULT_PROMPT_TEMPLATE.to_string());

    compose_agent_system_prompt(&agent_prompt, user_request, &system_template)
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
        "write_file" => format!(
            "{} {}",
            "write".custom_color(CTP_BLUE),
            string_argument(arguments, "file_path", "").italic()
        ),
        "edit_file" => format!(
            "{} {}",
            "edit".custom_color(CTP_BLUE),
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
        "fetch_url" => format!(
            "{} {}",
            "fetch".custom_color(CTP_BLUE),
            string_argument(arguments, "url", "").italic()
        ),
        _ => generic_tool_preview(tool_name, arguments),
    }
}

fn execute_tool_call(
    tool_registry: &ToolRegistry,
    tool_call: &ToolCall,
    verbose_tool_output: bool,
) -> String {
    let result = tool_registry.execute(&tool_call.name, &tool_call.arguments);
    match &result {
        Ok(output) => render_success_inline(output, verbose_tool_output),
        Err(error) => render_error_inline(error),
    }

    match result {
        Ok(output) => output,
        Err(error) => format!("Error: {error}"),
    }
}

const fn denied_tool_result() -> &'static str {
    "Tool call denied by user. Try a different approach or produce the final response."
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
    eprint_flush(
        &format!("using {model_name} (agent)...")
            .custom_color(CTP_OVERLAY0)
            .to_string(),
    );
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
    verbose_tool_output: bool,
    confirm_tool: &mut F,
) -> Result<(), LarpshellError>
where
    F: FnMut(&ToolCall) -> ToolConfirmResult,
{
    // Batch all tool calls into one assistant message to avoid consecutive
    // assistant messages, which violates the alternating-role contract of
    // OpenAI/Ollama APIs.
    let mut pending_messages = vec![ChatMessage::assistant_tool_calls(tool_calls.to_vec())];

    for tool_call in tool_calls {
        display_tool_call(tool_call);

        match confirm_tool(tool_call) {
            ToolConfirmResult::Allow => {
                let result_text = execute_tool_call(tool_registry, tool_call, verbose_tool_output);
                pending_messages.push(ChatMessage::tool_result(&tool_call.id, result_text));
            }
            ToolConfirmResult::Deny => {
                render_denied_inline();
                pending_messages.push(ChatMessage::tool_result(
                    &tool_call.id,
                    denied_tool_result(),
                ));
            }
            ToolConfirmResult::Cancel => return Err(LarpshellError::Cancelled),
        }
    }

    messages.extend(pending_messages);

    Ok(())
}

fn show_next_iteration_prompt(iteration: usize) {
    if iteration < MAX_AGENT_ITERATIONS - 1 {
        hide_cursor();
        eprint_flush(&"thinking...".custom_color(CTP_OVERLAY0).to_string());
    }
}

fn parse_final_response(text: &str) -> FinalResponse {
    let trimmed = text.trim();
    let parsed = parse_labeled_response(trimmed);

    if parsed.has_labels {
        if let Some(command) = parsed.command {
            return FinalResponse {
                kind: FinalResponseKind::Command,
                content: command.clone(),
                message: parsed.message,
                command: Some(command),
            };
        }

        if let Some(message) = parsed.message {
            return FinalResponse {
                kind: FinalResponseKind::Message,
                content: message.clone(),
                message: Some(message),
                command: None,
            };
        }
    }

    let content = crate::prompt::normalize_model_output(trimmed);
    FinalResponse {
        kind: FinalResponseKind::Message,
        message: Some(content.clone()),
        content,
        command: None,
    }
}

fn handle_agent_response<F>(
    response: ChatResponse,
    tool_registry: &ToolRegistry,
    messages: &mut Vec<ChatMessage>,
    verbose_tool_output: bool,
    confirm_tool: &mut F,
) -> Result<Option<FinalResponse>, LarpshellError>
where
    F: FnMut(&ToolCall) -> ToolConfirmResult,
{
    match response {
        ChatResponse::Message(text) => Ok(Some(parse_final_response(&text))),
        ChatResponse::ToolCalls(tool_calls) => {
            handle_tool_calls(
                &tool_calls,
                tool_registry,
                messages,
                verbose_tool_output,
                confirm_tool,
            )?;
            Ok(None)
        }
    }
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

fn tool_line_string(tool_call: &ToolCall) -> String {
    if let Some(arguments) = tool_call.arguments.as_object() {
        let preview = format_tool_preview(&tool_call.name, arguments);
        format!("  {} {}", "tool".custom_color(CTP_OVERLAY0), preview)
    } else {
        format!(
            "  {}  {}",
            "tool".custom_color(CTP_OVERLAY0),
            tool_call.name.custom_color(CTP_BLUE).bold()
        )
    }
}

fn success_summary_string(output: &str) -> String {
    let line_count = output.lines().count();
    let line_word = if line_count == 1 { "line" } else { "lines" };
    format!(
        "  {} {}",
        "result".custom_color(CTP_OVERLAY0),
        format!("({line_count} {line_word})").custom_color(CTP_GREEN),
    )
}

fn expanded_output_line_string(line: &str, is_first: bool) -> String {
    let prefix = if is_first { "  └ " } else { "    " };
    format!(
        "{}{}",
        prefix.custom_color(CTP_OVERLAY0),
        style_message_markup(line).custom_color(CTP_OVERLAY0)
    )
}

fn error_line_string(msg: &str) -> String {
    format!(
        "  {} {}",
        "error".custom_color(CTP_OVERLAY0),
        style_message_markup(msg).custom_color(CTP_RED)
    )
}

fn tip_line_string(msg: &str) -> Option<String> {
    command_not_allowed_tip(msg).map(|tip| {
        format!(
            "  {} {}",
            "tip:".custom_color(CTP_OVERLAY0).italic(),
            style_message_markup(&tip)
        )
    })
}

fn more_lines_indicator(hidden: usize) -> String {
    let word = if hidden == 1 { "line" } else { "lines" };
    format!(
        "    {}",
        format!("... {hidden} more {word}")
            .custom_color(CTP_OVERLAY0)
            .italic()
    )
}

fn render_success_inline(output: &str, verbose_tool_output: bool) {
    eprintln!("{}", success_summary_string(output));
    if verbose_tool_output {
        let total = output.lines().count();
        let shown = total.min(TOOL_OUTPUT_LINE_CAP);
        for (i, line) in output.lines().take(shown).enumerate() {
            eprintln!("{}", expanded_output_line_string(line, i == 0));
        }
        if total > shown {
            eprintln!("{}", more_lines_indicator(total - shown));
        }
    }
    eprintln!();
}

fn render_error_inline(msg: &str) {
    eprintln!("{}", error_line_string(msg));
    if let Some(tip) = tip_line_string(msg) {
        eprintln!("{tip}");
    }
    eprintln!();
}

fn render_denied_inline() {
    print_warning("tool call denied.");
    eprintln!();
}

fn display_tool_call(tool_call: &ToolCall) {
    eprintln!("{}", tool_line_string(tool_call));
}

fn confirm_tool_call() -> ToolConfirmResult {
    let prompt = format!(
        "  {} [{}] allow, [{}] deny, [{}] cancel",
        "Allow?".custom_color(CTP_YELLOW),
        "Y/Enter".custom_color(CTP_PRIMARY).bold(),
        "N".custom_color(CTP_PRIMARY).bold(),
        "Ctrl+C".custom_color(CTP_PRIMARY).bold(),
    );
    let prompt_display = prompt.custom_color(CTP_BLUE).to_string();
    eprint!("{prompt_display}");
    let _ = std::io::Write::flush(&mut std::io::stderr());

    #[cfg(unix)]
    {
        use nix::sys::termios::FlushArg;
        let _ = nix::sys::termios::tcflush(std::io::stdin(), FlushArg::TCIFLUSH);
    }

    loop {
        match read_key() {
            Key::Enter | Key::Char('y' | 'Y') => {
                clear_line();
                return ToolConfirmResult::Allow;
            }
            Key::Char('n' | 'N') => {
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

const fn parse_byte(b: u8) -> Key {
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
                // I/O error is treated as EOF: read_key cannot propagate errors, and
                // returning Key::Other lets the confirm loop spin until valid input arrives.
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
    // Same EOF-on-error rationale as above.
    if std::io::Read::read(&mut std::io::stdin().lock(), &mut buffer).unwrap_or(0) == 0 {
        return Key::Other;
    }

    parse_byte(buffer[0])
}

fn command_not_allowed_tip(error: &str) -> Option<String> {
    error
        .starts_with("command not allowed:")
        .then(|| "run **/agent on** to enable all commands".to_string())
}

async fn run_agent_loop_with_confirm<F>(
    user_input: &str,
    provider: &dyn AIProvider,
    config: &Config,
    tool_registry: &ToolRegistry,
    mut confirm_tool: F,
) -> Result<FinalResponse, LarpshellError>
where
    F: FnMut(&ToolCall) -> ToolConfirmResult,
{
    let (mut messages, tool_definitions) = agent_context(user_input, config, tool_registry)?;

    for iteration in 0..MAX_AGENT_ITERATIONS {
        let response = next_agent_response(provider, &messages, &tool_definitions).await?;

        if let Some(text) = handle_agent_response(
            response,
            tool_registry,
            &mut messages,
            config.verbose_tool_output,
            &mut confirm_tool,
        )? {
            return Ok(text);
        }

        show_next_iteration_prompt(iteration);
    }

    Err(LarpshellError::AgentMaxIterations(MAX_AGENT_ITERATIONS))
}

pub async fn run_agent_loop(
    user_input: &str,
    provider: &dyn AIProvider,
    config: &Config,
    tool_registry: &ToolRegistry,
) -> Result<FinalResponse, LarpshellError> {
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
            verbose_tool_output: true,
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
    fn build_agent_system_prompt_prepends_agent_prompt_to_system_prompt() {
        let prompt = compose_agent_system_prompt(
            DEFAULT_AGENT_SAFE_PROMPT,
            "list the rust files",
            DEFAULT_PROMPT_TEMPLATE,
        );
        let expected_system_prompt =
            create_system_prompt("list the rust files", Some(DEFAULT_PROMPT_TEMPLATE));

        assert!(prompt.contains("User request: list the rust files"));
        assert!(prompt.contains("Current dir:"));
        assert!(prompt.contains("Shell:"));
        assert!(prompt.contains("safe, read-only tools"));
        assert!(prompt.contains("You are a shell command translator."));
        assert_eq!(
            prompt,
            format!("{DEFAULT_AGENT_SAFE_PROMPT}\n\n{expected_system_prompt}")
        );
    }

    #[test]
    fn build_agent_system_prompt_for_on_mentions_iterative_probing() {
        let prompt = compose_agent_system_prompt(
            DEFAULT_AGENT_PROMPT,
            "inspect the environment",
            DEFAULT_PROMPT_TEMPLATE,
        );

        assert!(prompt.contains("interacting with the user's machine"));
        assert!(prompt.contains("Multi-step probing"));
        assert!(prompt.starts_with(DEFAULT_AGENT_PROMPT));
    }

    #[test]
    fn build_agent_system_prompt_for_safe_is_conservative() {
        let prompt = compose_agent_system_prompt(
            DEFAULT_AGENT_SAFE_PROMPT,
            "inspect the environment",
            DEFAULT_PROMPT_TEMPLATE,
        );

        assert!(prompt.contains("safe, read-only tools"));
        assert!(!prompt.contains("use the run_command tool"));
    }

    #[tokio::test]
    async fn run_agent_loop_returns_command_without_tool_calls() {
        let provider =
            MockProvider::new(vec![ChatResponse::Message("COMMAND: ls -la".to_string())]);
        let tool_registry = ToolRegistry::with_builtins(AgentMode::Safe);

        let response = run_agent_loop_with_confirm(
            "list files",
            &provider,
            &test_config(),
            &tool_registry,
            |_| ToolConfirmResult::Allow,
        )
        .await
        .unwrap();

        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Command,
                content: "ls -la".to_string(),
                message: None,
                command: Some("ls -la".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn run_agent_loop_returns_message_without_tool_calls() {
        let provider = MockProvider::new(vec![ChatResponse::Message(
            "MESSAGE: no command needed".to_string(),
        )]);
        let tool_registry = ToolRegistry::with_builtins(AgentMode::Safe);

        let response = run_agent_loop_with_confirm(
            "say hi",
            &provider,
            &test_config(),
            &tool_registry,
            |_| ToolConfirmResult::Allow,
        )
        .await
        .unwrap();

        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Message,
                content: "no command needed".to_string(),
                message: Some("no command needed".to_string()),
                command: None,
            }
        );
    }

    #[tokio::test]
    async fn run_agent_loop_returns_message_and_command_from_single_response() {
        let provider = MockProvider::new(vec![ChatResponse::Message(
            "MESSAGE: package needed by:\nlarpshell\nCOMMAND: sudo pacman -S webkit2gtk-4.1"
                .to_string(),
        )]);
        let tool_registry = ToolRegistry::with_builtins(AgentMode::Safe);

        let response = run_agent_loop_with_confirm(
            "install package",
            &provider,
            &test_config(),
            &tool_registry,
            |_| ToolConfirmResult::Allow,
        )
        .await
        .unwrap();

        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Command,
                content: "sudo pacman -S webkit2gtk-4.1".to_string(),
                message: Some("package needed by:\nlarpshell".to_string()),
                command: Some("sudo pacman -S webkit2gtk-4.1".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn run_agent_loop_preserves_multiline_command_payload() {
        let provider = MockProvider::new(vec![ChatResponse::Message(
            "COMMAND: echo one\necho two".to_string(),
        )]);
        let tool_registry = ToolRegistry::with_builtins(AgentMode::Safe);

        let response = run_agent_loop_with_confirm(
            "echo twice",
            &provider,
            &test_config(),
            &tool_registry,
            |_| ToolConfirmResult::Allow,
        )
        .await
        .unwrap();

        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Command,
                content: "echo one\necho two".to_string(),
                message: None,
                command: Some("echo one\necho two".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn run_agent_loop_preserves_multiline_message_payload() {
        let provider = MockProvider::new(vec![ChatResponse::Message(
            "MESSAGE: first line\nsecond line".to_string(),
        )]);
        let tool_registry = ToolRegistry::with_builtins(AgentMode::Safe);

        let response = run_agent_loop_with_confirm(
            "describe thing",
            &provider,
            &test_config(),
            &tool_registry,
            |_| ToolConfirmResult::Allow,
        )
        .await
        .unwrap();

        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Message,
                content: "first line\nsecond line".to_string(),
                message: Some("first line\nsecond line".to_string()),
                command: None,
            }
        );
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
                thought_signature: None,
            }]),
            ChatResponse::Message("COMMAND: cat hello.txt".to_string()),
        ]);
        let tool_registry = ToolRegistry::with_builtins(AgentMode::Safe);

        let response = run_agent_loop_with_confirm(
            "show me the file",
            &provider,
            &test_config(),
            &tool_registry,
            |_| ToolConfirmResult::Allow,
        )
        .await
        .unwrap();

        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Command,
                content: "cat hello.txt".to_string(),
                message: None,
                command: Some("cat hello.txt".to_string()),
            }
        );

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
            thought_signature: None,
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
    fn parse_final_response_treats_unprefixed_text_as_message() {
        let response = parse_final_response("here is what I found");
        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Message,
                content: "here is what I found".to_string(),
                message: Some("here is what I found".to_string()),
                command: None,
            }
        );
    }

    #[test]
    fn parse_final_response_extracts_multiline_message_from_fenced_block() {
        let response = parse_final_response("```\nMESSAGE: no command needed\nsecond line\n```");
        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Message,
                content: "no command needed\nsecond line".to_string(),
                message: Some("no command needed\nsecond line".to_string()),
                command: None,
            }
        );
    }

    #[test]
    fn parse_final_response_extracts_multiline_command_after_leading_prose() {
        let response = parse_final_response("Done.\nCOMMAND: ls -la\npwd");
        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Command,
                content: "ls -la\npwd".to_string(),
                message: None,
                command: Some("ls -la\npwd".to_string()),
            }
        );
    }

    #[test]
    fn parse_final_response_prefers_command_when_message_and_command_exist() {
        let response = parse_final_response("MESSAGE: note\nCOMMAND: ls -la");
        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Command,
                content: "ls -la".to_string(),
                message: Some("note".to_string()),
                command: Some("ls -la".to_string()),
            }
        );
    }

    #[test]
    fn parse_final_response_appends_repeated_message_blocks() {
        let response = parse_final_response("MESSAGE: first\nMESSAGE: second");
        assert_eq!(
            response,
            FinalResponse {
                kind: FinalResponseKind::Message,
                content: "first\nsecond".to_string(),
                message: Some("first\nsecond".to_string()),
                command: None,
            }
        );
    }

    #[test]
    fn parse_final_response_parses_multiline_command_prefix() {
        let response = parse_final_response("  COMMAND:   echo hello\n  echo world  ");

        assert!(matches!(response.kind, FinalResponseKind::Command));
        assert_eq!(response.content, "echo hello\necho world");
    }

    #[test]
    fn parse_final_response_parses_multiline_message_prefix() {
        let response = parse_final_response("  MESSAGE:   done\nnext step  ");

        assert!(matches!(response.kind, FinalResponseKind::Message));
        assert_eq!(response.content, "done\nnext step");
    }

    #[test]
    fn parse_final_response_defaults_to_message_without_prefix() {
        let response = parse_final_response("  ls -la  ");

        assert!(matches!(response.kind, FinalResponseKind::Message));
        assert_eq!(response.content, "ls -la");
    }

    #[test]
    fn parse_final_response_parses_command_prefix() {
        let response = parse_final_response("  COMMAND:   echo hello  ");

        assert!(matches!(response.kind, FinalResponseKind::Command));
        assert_eq!(response.content, "echo hello");
    }

    #[test]
    fn parse_final_response_parses_message_prefix() {
        let response = parse_final_response("  MESSAGE:   done  ");

        assert!(matches!(response.kind, FinalResponseKind::Message));
        assert_eq!(response.content, "done");
    }

    #[test]
    fn parse_final_response_defaults_to_message_without_prefix_again() {
        let response = parse_final_response("  ls -la  ");

        assert!(matches!(response.kind, FinalResponseKind::Message));
        assert_eq!(response.content, "ls -la");
    }

    #[test]
    fn handle_tool_calls_does_not_append_messages_when_cancelled() {
        let tool_calls = vec![
            crate::providers::ToolCall {
                id: "tool-1".to_string(),
                name: "list_files".to_string(),
                arguments: serde_json::json!({
                    "directory_path": std::env::temp_dir().display().to_string()
                }),
                thought_signature: None,
            },
            crate::providers::ToolCall {
                id: "tool-2".to_string(),
                name: "terminal_width".to_string(),
                arguments: serde_json::json!({}),
                thought_signature: None,
            },
        ];
        let tool_registry = ToolRegistry::with_builtins(AgentMode::Safe);
        let mut messages = vec![ChatMessage::user("show me files")];
        let mut confirmations =
            vec![ToolConfirmResult::Allow, ToolConfirmResult::Cancel].into_iter();

        let error = handle_tool_calls(
            &tool_calls,
            &tool_registry,
            &mut messages,
            true,
            &mut |_| confirmations.next().unwrap(),
        )
        .unwrap_err();

        assert!(matches!(error, LarpshellError::Cancelled));
        assert_eq!(messages, vec![ChatMessage::user("show me files")]);
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
        assert!(tip.contains("run **/agent on** to enable all commands"));

        assert!(plain_tip("dangerous argument detected: --force").is_none());
    }
}
