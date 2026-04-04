pub mod mcp;
pub mod tools;

use colored::*;

use crate::cli::print_warning;
use crate::common::{
    CTP_BLUE, CTP_GREEN, CTP_OVERLAY0, CTP_PRIMARY, CTP_RED, CTP_TEXT, CTP_YELLOW, clear_line,
    count_visual_lines, eprint_flush, get_current_directory, get_os, get_shell, get_terminal_width,
    get_username, hide_cursor, show_cursor,
};
use crate::config::Config;
use crate::error::LarpshellError;
use crate::providers::{AIProvider, ChatMessage, ChatResponse, ToolCall};
use tools::ToolRegistry;

const MAX_AGENT_ITERATIONS: usize = 10;

const AGENT_SYSTEM_PROMPT: &str =
    "You are a shell command translator with access to tools for gathering context.
You may call tools to read files, list directories, or search for patterns
before producing your final shell command.

When you have enough context, respond with ONLY the shell command (no markdown,
no explanations, no backticks) — the same rules as without tools.

Environment context:
- Current dir: {cwd}
- Home dir: {home}
- User: {user}
- Shell: {shell}
- OS: {os}

User request: {request}";

fn build_agent_system_prompt(user_request: &str) -> String {
    let cwd = get_current_directory();
    let os = get_os();
    let shell = get_shell();
    let home = dirs::home_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "~".to_string());
    let user = get_username();

    AGENT_SYSTEM_PROMPT
        .replace("{cwd}", &cwd)
        .replace("{home}", &home)
        .replace("{user}", &user)
        .replace("{shell}", &shell)
        .replace("{os}", &os)
        .replace("{request}", user_request)
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

fn display_tool_call(tool_call: &ToolCall) -> usize {
    let width = get_terminal_width();
    let mut lines = 0;

    let tool_line = format!(
        "  {}  {}",
        "tool".custom_color(CTP_OVERLAY0),
        tool_call.name.custom_color(CTP_BLUE).bold()
    );
    eprintln!("{tool_line}");
    lines += count_visual_lines(&tool_line, width);

    if let Some(arguments) = tool_call.arguments.as_object() {
        for (key, value) in arguments {
            let value_str = match value {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            let argument_line = format!(
                "  {}  {}: {}",
                "args".custom_color(CTP_OVERLAY0),
                key.custom_color(CTP_TEXT),
                value_str.custom_color(CTP_TEXT).bold()
            );
            eprintln!("{argument_line}");
            lines += count_visual_lines(&argument_line, width);
        }
    }

    lines
}

fn confirm_tool_call() -> ToolConfirmResult {
    let prompt = format!(
        "\n  {} [{}] allow, [{}] deny, [{}] cancel",
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
        let _ = nix::sys::termios::tcflush(&std::io::stdin(), FlushArg::TCIFLUSH);
    }

    loop {
        match read_key() {
            Key::Enter | Key::Char('y') | Key::Char('Y') => {
                eprintln!();
                return ToolConfirmResult::Allow;
            }
            Key::Char('n') | Key::Char('N') => {
                eprintln!();
                return ToolConfirmResult::Deny;
            }
            Key::CtrlC => {
                eprintln!();
                return ToolConfirmResult::Cancel;
            }
            Key::Other | Key::Char(_) => {}
        }
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
                        match buffer[0] {
                            b'\n' | b'\r' => Key::Enter,
                            b'\x03' => Key::CtrlC,
                            ch @ 32..=126 => Key::Char(ch as char),
                            _ => Key::Other,
                        }
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

    match buffer[0] {
        b'\n' | b'\r' => Key::Enter,
        b'\x03' => Key::CtrlC,
        ch @ 32..=126 => Key::Char(ch as char),
        _ => Key::Other,
    }
}

fn display_tool_result(result: &str) {
    let truncated = if result.len() > 500 {
        format!("{}...\n  [truncated for display]", &result[..500])
    } else {
        result.to_string()
    };
    eprintln!(
        "  {} {}",
        "result".custom_color(CTP_OVERLAY0),
        truncated.custom_color(CTP_GREEN)
    );
}

fn display_tool_error(error: &str) {
    eprintln!(
        "  {} {}",
        "error".custom_color(CTP_OVERLAY0),
        error.custom_color(CTP_RED)
    );
}

async fn run_agent_loop_with_confirm<F>(
    user_input: &str,
    provider: &dyn AIProvider,
    config: &Config,
    tool_registry: &ToolRegistry,
    mut confirm_tool: F,
) -> Result<String, Box<dyn std::error::Error>>
where
    F: FnMut(&ToolCall) -> ToolConfirmResult,
{
    let model_name = config.get_provider_config()?.config.model().to_string();
    hide_cursor();
    eprint_flush(&format!(
        "{}",
        format!("using {} (agent)...", model_name).custom_color(CTP_OVERLAY0)
    ));

    let system_prompt = build_agent_system_prompt(user_input);
    let mut messages = vec![
        ChatMessage::system(system_prompt),
        ChatMessage::user(user_input),
    ];
    let tool_definitions = tool_registry.definitions();

    for iteration in 0..MAX_AGENT_ITERATIONS {
        let response = match provider
            .generate_with_tools(&messages, &tool_definitions)
            .await
        {
            Ok(response) => response,
            Err(error) => {
                clear_line();
                show_cursor();
                return Err(Box::new(error));
            }
        };

        clear_line();
        show_cursor();

        match response {
            ChatResponse::Message(text) => return Ok(text),
            ChatResponse::ToolCalls(tool_calls) => {
                for tool_call in &tool_calls {
                    display_tool_call(tool_call);

                    match confirm_tool(tool_call) {
                        ToolConfirmResult::Allow => {
                            let result =
                                tool_registry.execute(&tool_call.name, tool_call.arguments.clone());
                            match &result {
                                Ok(output) => display_tool_result(output),
                                Err(error) => display_tool_error(error),
                            }
                            let result_text = match result {
                                Ok(output) => output,
                                Err(error) => format!("Error: {error}"),
                            };
                            messages
                                .push(ChatMessage::assistant_tool_calls(vec![tool_call.clone()]));
                            messages.push(ChatMessage::tool_result(&tool_call.id, result_text));
                        }
                        ToolConfirmResult::Deny => {
                            print_warning("tool call denied.");
                            messages
                                .push(ChatMessage::assistant_tool_calls(vec![tool_call.clone()]));
                            messages.push(ChatMessage::tool_result(
                                &tool_call.id,
                                "Tool call denied by user. Try a different approach or produce the final command.",
                            ));
                        }
                        ToolConfirmResult::Cancel => {
                            return Err(Box::new(LarpshellError::Cancelled));
                        }
                    }
                }

                if iteration < MAX_AGENT_ITERATIONS - 1 {
                    hide_cursor();
                    eprint_flush(&format!("{}", "thinking...".custom_color(CTP_OVERLAY0)));
                }
            }
        }
    }

    Err(Box::new(LarpshellError::AgentMaxIterations(
        MAX_AGENT_ITERATIONS,
    )))
}

pub async fn run_agent_loop(
    user_input: &str,
    provider: &dyn AIProvider,
    config: &Config,
    tool_registry: &ToolRegistry,
) -> Result<String, Box<dyn std::error::Error>> {
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
            agent: true,
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
        let prompt = build_agent_system_prompt("list the rust files");

        assert!(prompt.contains("User request: list the rust files"));
        assert!(prompt.contains("Current dir:"));
        assert!(prompt.contains("Shell:"));
    }

    #[tokio::test]
    async fn run_agent_loop_returns_message_without_tool_calls() {
        let provider = MockProvider::new(vec![ChatResponse::Message("ls -la".to_string())]);
        let tool_registry = ToolRegistry::with_builtins();

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
        let tool_registry = ToolRegistry::with_builtins();

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
        let tool_registry = ToolRegistry::with_builtins();

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
            error.downcast_ref::<LarpshellError>(),
            Some(LarpshellError::AgentMaxIterations(MAX_AGENT_ITERATIONS))
        ));
    }

    #[test]
    fn test_config_uses_ollama_provider() {
        let provider_config = test_config().get_provider_config().unwrap();
        assert!(matches!(
            provider_config.config,
            ProviderSpecificConfig::Ollama { .. }
        ));
    }
}
