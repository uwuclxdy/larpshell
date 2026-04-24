pub mod builtins;
pub mod mcp;
pub mod tools;

use colored::*;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cli::print_warning;
use crate::common::{
    CTP_BLUE, CTP_GREEN, CTP_OVERLAY0, CTP_PRIMARY, CTP_RED, CTP_YELLOW, clear_line, clear_n_lines,
    count_visual_lines, eprint_flush, hide_cursor, show_cursor, terminal_height, terminal_width,
};
use crate::config::{
    AgentMode, Config, load_agent_prompt, load_agent_safe_prompt, load_sys_prompt,
};
use crate::error::LarpshellError;
use crate::prompt::{
    DEFAULT_AGENT_PROMPT, DEFAULT_AGENT_SAFE_PROMPT, DEFAULT_PROMPT_TEMPLATE, create_system_prompt,
    validate_sys_prompt,
};
use crate::providers::{AIProvider, ChatMessage, ChatResponse, ToolCall};
use tools::ToolRegistry;

const MAX_AGENT_ITERATIONS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalResponseKind {
    Command,
    Message,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalResponse {
    pub kind: FinalResponseKind,
    pub content: String,
}

static EXPANDED: AtomicBool = AtomicBool::new(false);
static TOOL_BLOCKS: Mutex<Vec<ToolBlock>> = Mutex::new(Vec::new());

#[derive(Clone)]
enum ToolOutcome {
    Success(String),
    Error(String),
    Denied,
}

struct ToolBlock {
    tool_call: ToolCall,
    outcome: Option<ToolOutcome>,
}

fn reset_tool_blocks() {
    if let Ok(mut blocks) = TOOL_BLOCKS.lock() {
        blocks.clear();
    }
    EXPANDED.store(false, Ordering::Relaxed);
}

fn push_tool_call(tool_call: &ToolCall) {
    if let Ok(mut blocks) = TOOL_BLOCKS.lock() {
        blocks.push(ToolBlock {
            tool_call: tool_call.clone(),
            outcome: None,
        });
    }
}

fn set_last_outcome(outcome: ToolOutcome) {
    if let Ok(mut blocks) = TOOL_BLOCKS.lock()
        && let Some(last) = blocks.last_mut()
    {
        last.outcome = Some(outcome);
    }
}

fn has_success_outcomes() -> bool {
    TOOL_BLOCKS
        .lock()
        .map(|blocks| {
            blocks
                .iter()
                .any(|b| matches!(b.outcome, Some(ToolOutcome::Success(_))))
        })
        .unwrap_or(false)
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
    CtrlE,
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
                set_last_outcome(ToolOutcome::Denied);
                render_denied_inline();
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

fn parse_final_response(text: &str) -> FinalResponse {
    let trimmed = text.trim();

    if let Some(command) = trimmed.strip_prefix("COMMAND:") {
        return FinalResponse {
            kind: FinalResponseKind::Command,
            content: command.trim().to_string(),
        };
    }

    if let Some(message) = trimmed.strip_prefix("MESSAGE:") {
        return FinalResponse {
            kind: FinalResponseKind::Message,
            content: message.trim().to_string(),
        };
    }

    FinalResponse {
        kind: FinalResponseKind::Command,
        content: trimmed.to_string(),
    }
}

fn handle_agent_response<F>(
    response: ChatResponse,
    tool_registry: &ToolRegistry,
    messages: &mut Vec<ChatMessage>,
    confirm_tool: &mut F,
) -> Result<Option<FinalResponse>, LarpshellError>
where
    F: FnMut(&ToolCall) -> ToolConfirmResult,
{
    match response {
        ChatResponse::Message(text) => Ok(Some(parse_final_response(&text))),
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
) -> Result<Option<FinalResponse>, LarpshellError>
where
    F: FnMut(&ToolCall) -> ToolConfirmResult,
{
    handle_agent_response(response, tool_registry, messages, confirm_tool)
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
        "  {} {}  {}",
        "result".custom_color(CTP_OVERLAY0),
        format!("({} {})", line_count, line_word).custom_color(CTP_GREEN),
        "ctrl+e".custom_color(CTP_OVERLAY0),
    )
}

fn expanded_output_line_string(line: &str, is_first: bool) -> String {
    let prefix = if is_first { "  └ " } else { "    " };
    format!(
        "{}{}",
        prefix.custom_color(CTP_OVERLAY0),
        line.custom_color(CTP_OVERLAY0)
    )
}

fn error_line_string(msg: &str) -> String {
    format!(
        "  {} {}",
        "error".custom_color(CTP_OVERLAY0),
        msg.custom_color(CTP_RED)
    )
}

fn tip_line_string(msg: &str) -> Option<String> {
    command_not_allowed_tip(msg)
        .map(|tip| format!("  {} {}", "tip:".custom_color(CTP_OVERLAY0).italic(), tip))
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

fn render_success_inline(output: &str, expanded: bool, cap: usize) {
    eprintln!("{}", success_summary_string(output));
    if expanded {
        let total = output.lines().count();
        let shown = total.min(cap);
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
        eprintln!("{}", tip);
    }
    eprintln!();
}

fn render_denied_inline() {
    print_warning("tool call denied.");
    eprintln!();
}

fn render_block(block: &ToolBlock, expanded: bool, is_current: bool, cap: usize) {
    eprintln!("{}", tool_line_string(&block.tool_call));
    if is_current {
        return;
    }
    match &block.outcome {
        Some(ToolOutcome::Success(output)) => render_success_inline(output, expanded, cap),
        Some(ToolOutcome::Error(msg)) => render_error_inline(msg),
        Some(ToolOutcome::Denied) => render_denied_inline(),
        None => {}
    }
}

fn compute_expanded_cap(blocks: &[ToolBlock]) -> usize {
    let width = terminal_width();
    let height = terminal_height();
    let mut non_output = 0usize;
    let mut success_count = 0usize;
    let last_idx = blocks.len().saturating_sub(1);
    for (i, block) in blocks.iter().enumerate() {
        let is_current = i == last_idx && block.outcome.is_none();
        non_output += count_visual_lines(&tool_line_string(&block.tool_call), width);
        if is_current {
            continue;
        }
        match &block.outcome {
            Some(ToolOutcome::Success(output)) => {
                non_output += count_visual_lines(&success_summary_string(output), width);
                non_output += 1;
                success_count += 1;
            }
            Some(ToolOutcome::Error(msg)) => {
                non_output += count_visual_lines(&error_line_string(msg), width);
                if let Some(tip) = tip_line_string(msg) {
                    non_output += count_visual_lines(&tip, width);
                }
                non_output += 1;
            }
            Some(ToolOutcome::Denied) => {
                non_output += 2;
            }
            None => {}
        }
    }
    if success_count == 0 {
        return usize::MAX;
    }
    let reserved = non_output + 2 /* prompt */ + 1 /* safety */;
    if reserved >= height {
        return 1;
    }
    ((height - reserved) / success_count).max(1)
}

fn block_visual_lines(block: &ToolBlock, expanded: bool, is_current: bool, cap: usize) -> usize {
    let width = terminal_width();
    let mut n = count_visual_lines(&tool_line_string(&block.tool_call), width);
    if is_current {
        return n;
    }
    match &block.outcome {
        Some(ToolOutcome::Success(output)) => {
            n += count_visual_lines(&success_summary_string(output), width);
            if expanded {
                let total = output.lines().count();
                let shown = total.min(cap);
                for (i, line) in output.lines().take(shown).enumerate() {
                    n += count_visual_lines(&expanded_output_line_string(line, i == 0), width);
                }
                if total > shown {
                    n += count_visual_lines(&more_lines_indicator(total - shown), width);
                }
            }
            n += 1;
        }
        Some(ToolOutcome::Error(msg)) => {
            n += count_visual_lines(&error_line_string(msg), width);
            if let Some(tip) = tip_line_string(msg) {
                n += count_visual_lines(&tip, width);
            }
            n += 1;
        }
        Some(ToolOutcome::Denied) => {
            n += 1;
            n += 1;
        }
        None => {}
    }
    n
}

fn redraw_all_blocks(prompt_display: &str) {
    let width = terminal_width();
    let expanded = EXPANDED.load(Ordering::Relaxed);
    let blocks_snapshot: Vec<ToolBlock> = {
        let blocks = TOOL_BLOCKS.lock().unwrap_or_else(|e| e.into_inner());
        blocks
            .iter()
            .map(|b| ToolBlock {
                tool_call: b.tool_call.clone(),
                outcome: b.outcome.clone(),
            })
            .collect()
    };
    let last_idx = blocks_snapshot.len().saturating_sub(1);
    let current_cap = compute_expanded_cap(&blocks_snapshot);

    let mut total = 0usize;
    for (i, block) in blocks_snapshot.iter().enumerate() {
        let is_current = i == last_idx && block.outcome.is_none();
        total += block_visual_lines(block, expanded, is_current, current_cap);
    }
    total += count_visual_lines(prompt_display, width);

    clear_n_lines(total);

    let new_expanded = !expanded;
    EXPANDED.store(new_expanded, Ordering::Relaxed);

    for (i, block) in blocks_snapshot.iter().enumerate() {
        let is_current = i == last_idx && block.outcome.is_none();
        render_block(block, new_expanded, is_current, current_cap);
    }

    eprint!("{prompt_display}");
    let _ = std::io::Write::flush(&mut std::io::stderr());
}

fn display_tool_call(tool_call: &ToolCall) -> usize {
    push_tool_call(tool_call);
    let tool_line = tool_line_string(tool_call);
    let lines = count_visual_lines(&tool_line, terminal_width());
    eprintln!("{tool_line}");
    lines
}

fn confirm_tool_call() -> ToolConfirmResult {
    let hint = if has_success_outcomes() {
        format!("  {}", "ctrl+e expand".custom_color(CTP_OVERLAY0))
    } else {
        String::new()
    };
    let prompt = format!(
        "  {} [{}] allow, [{}] deny, [{}] cancel{}",
        "Allow?".custom_color(CTP_YELLOW),
        "Y/Enter".custom_color(CTP_PRIMARY).bold(),
        "N".custom_color(CTP_PRIMARY).bold(),
        "Ctrl+C".custom_color(CTP_PRIMARY).bold(),
        hint,
    );
    let prompt_display = format!("{}", prompt.custom_color(CTP_BLUE));
    eprint!("{prompt_display}");
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
            Key::CtrlE => {
                if has_success_outcomes() {
                    redraw_all_blocks(&prompt_display);
                }
            }
            Key::Other | Key::Char(_) => {}
        }
    }
}

fn parse_byte(b: u8) -> Key {
    match b {
        b'\n' | b'\r' => Key::Enter,
        b'\x03' => Key::CtrlC,
        b'\x05' => Key::CtrlE,
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
    set_last_outcome(ToolOutcome::Success(result.to_string()));
    let cap = TOOL_BLOCKS
        .lock()
        .map(|blocks| compute_expanded_cap(&blocks))
        .unwrap_or(1);
    render_success_inline(result, EXPANDED.load(Ordering::Relaxed), cap);
}

fn command_not_allowed_tip(error: &str) -> Option<ColoredString> {
    let text = format!("run {} to enable all commands", "/agent on".bold());
    error
        .starts_with("command not allowed:")
        .then(|| text.italic().custom_color(CTP_OVERLAY0))
}

fn display_tool_error(error: &str) {
    set_last_outcome(ToolOutcome::Error(error.to_string()));
    render_error_inline(error);
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
    reset_tool_blocks();
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
    fn parse_final_response_defaults_to_command_without_prefix() {
        let response = parse_final_response("  ls -la  ");

        assert!(matches!(response.kind, FinalResponseKind::Command));
        assert_eq!(response.content, "ls -la");
    }
}
