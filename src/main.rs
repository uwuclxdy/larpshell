#[cfg(test)]
mod tests;

mod agent;
mod cli;
mod common;
mod config;
mod confirmation;
mod error;
mod interactive;
mod prompt;
mod providers;
mod shell_integration;
mod slash_commands;
mod uninstall;
mod update;

use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use agent::tools::ToolRegistry;
use cli::home_dir;
use cli::{PromptAction, PromptKind, execute_shell_command, parse_cli_args, print_warning};
use colored::Colorize;
#[cfg(unix)]
use common::setup_terminal;
use common::{
    CTP_BLUE, CTP_OVERLAY0, CTP_YELLOW, EXIT_SIGINT, clear_line, eprint_flush, exit_with_code,
    hide_cursor, show_cursor,
};
use config::{AgentMode, Config, interactive_setup, load_config};
use confirmation::{
    ConfirmResult, ResponseStyle, confirm_execution, confirm_with_explain, display_explanation,
    display_response, edit_command,
};
use error::LarpshellError;
use interactive::{user_input, user_input_prefilled};
use prompt::{
    DEFAULT_AGENT_PROMPT, DEFAULT_AGENT_SAFE_PROMPT, DEFAULT_EXPLAIN_PROMPT,
    DEFAULT_PROMPT_TEMPLATE, clean_response, create_explain_prompt, create_prompts,
    create_system_prompt, validate_explain_prompt, validate_sys_prompt,
};
use providers::{AIProvider, create_provider};
use shell_integration::{auto_setup_shell_function, migrate_nlsh_rs_shell};
use slash_commands::SlashCmd;
use uninstall::uninstall_larpshell;

/// Differentiates interactive (REPL) vs single-command mode.
#[derive(Clone, Copy, PartialEq)]
enum CommandMode {
    Interactive,
    Single,
}

/// How larpshell was invoked. Selected once at startup from CLI args + TTY state.
enum RunMode {
    /// Positional args: `larpshell show disk usage`. Run once and exit.
    OneShot(String),
    /// No args, TTY attached: drop into the REPL.
    Repl,
    /// No args, stdin is a pipe: read one request (or slash command) and exit.
    Piped,
}

/// Mutable runtime state that some slash commands (`/api`, `/agent`) can swap.
struct Runtime {
    config: Config,
    provider: Box<dyn AIProvider>,
    tool_registry: Option<ToolRegistry>,
    update_task: Option<JoinHandle<bool>>,
}

enum SlashOutcome {
    Continue,
    Quit,
}

// ── main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error.print();
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), LarpshellError> {
    setup_environment();
    do_nlsh_rs_migration();

    let cli = parse_cli_args();

    if cli.subcommand.is_none() {
        try_auto_install_shell();
    }

    if let Some(ref sub) = cli.subcommand
        && !matches!(sub, cli::Subcommands::Explain { .. })
    {
        return dispatch_provider_less_subcommand(sub);
    }

    let mut runtime = Runtime::create()?;

    let result = match cli.subcommand {
        Some(cli::Subcommands::Explain { command }) => {
            handle_explain_subcommand(command, runtime.provider.as_ref()).await
        }
        _ => match select_run_mode(&cli.command) {
            RunMode::OneShot(input) => {
                process_user_input(&input, &mut runtime, CommandMode::Single)
                    .await
                    .map(drop)
            }
            RunMode::Piped => run_piped(&mut runtime).await,
            RunMode::Repl => run_repl(&mut runtime).await,
        },
    };

    runtime.finish_update().await;
    result
}

// ── startup ─────────────────────────────────────────────────────────────────

fn setup_environment() {
    #[cfg(unix)]
    setup_terminal();

    if std::io::stderr().is_terminal() {
        colored::control::set_override(true);
    }

    create_prompts().ok();

    if !validate_sys_prompt(
        config::load_sys_prompt()
            .as_deref()
            .unwrap_or(DEFAULT_PROMPT_TEMPLATE),
    ) {
        print_warning("system prompt must contain {request} placeholder — using default.");
    }

    if !validate_explain_prompt(
        config::load_explain_prompt()
            .as_deref()
            .unwrap_or(DEFAULT_EXPLAIN_PROMPT),
    ) {
        print_warning("explain prompt must contain {command} placeholder — using default.");
    }
}

fn do_nlsh_rs_migration() {
    let binary = home_dir().join(".cargo/bin/nlsh-rs");
    if !binary.exists() {
        return;
    }

    config::migrate_from_nlsh_rs().ok();
    migrate_nlsh_rs_shell().ok();

    std::process::Command::new("cargo")
        .args(["uninstall", "nlsh-rs"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok();
}

fn try_auto_install_shell() {
    if matches!(auto_setup_shell_function(), Ok(true)) {
        eprintln!(
            "{}",
            "restart shell or run 'source ~/.bashrc' ('source ~/.config/fish/config.fish' for fish).".custom_color(CTP_YELLOW)
        );
        exit_with_code(0);
    }
}

// ── run mode dispatch ───────────────────────────────────────────────────────

fn select_run_mode(command: &[String]) -> RunMode {
    if !command.is_empty() {
        RunMode::OneShot(command.join(" "))
    } else if cli::is_interactive_terminal() {
        RunMode::Repl
    } else {
        RunMode::Piped
    }
}

// ── piped/stdin mode ────────────────────────────────────────────────────────

async fn run_piped(runtime: &mut Runtime) -> Result<(), LarpshellError> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let input = buf.trim();

    if input.is_empty() {
        return Ok(());
    }

    if input.starts_with('/') {
        dispatch_slash_command(slash_commands::parse(input), runtime, CommandMode::Single).await?;
        return Ok(());
    }

    process_user_input(input, runtime, CommandMode::Single)
        .await
        .map(drop)
}

// ── REPL mode ───────────────────────────────────────────────────────────────

async fn run_repl(runtime: &mut Runtime) -> Result<(), LarpshellError> {
    let mut prefill: Option<String> = None;
    let mut sigint_exit = false;

    loop {
        interactive::reserve_preview_space();
        let user_input = match read_next_input(prefill.take()) {
            Ok(Some(input)) => input,
            Ok(None) => continue,
            Err(ReplInputError::SigInt) => {
                sigint_exit = true;
                break;
            }
            Err(ReplInputError::Eof) => break,
            Err(ReplInputError::Io(error)) => return Err(LarpshellError::IoError(error)),
        };
        clear_line();

        if user_input.starts_with('/') {
            let cmd = slash_commands::parse(&user_input);
            match dispatch_slash_command(cmd, runtime, CommandMode::Interactive).await {
                Ok(SlashOutcome::Quit) => {
                    show_cursor();
                    break;
                }
                Ok(SlashOutcome::Continue) => {}
                Err(error) => print_unless_cancelled(&error),
            }
            continue;
        }

        if let Some(cmd) = user_input.strip_prefix("! ") {
            execute_shell_command(cmd)?;
            continue;
        }
        if user_input == "!" {
            LarpshellError::ExpectedCommandAfterBang.print();
            continue;
        }

        match process_user_input(&user_input, runtime, CommandMode::Interactive).await {
            Ok(Some(resubmit)) => {
                // Overwrite the old rustyline prompt line with the new prefilled one.
                eprint_flush("\x1b[1A\x1b[K");
                prefill = Some(resubmit);
            }
            Ok(None) => {}
            Err(error) => print_unless_cancelled(&error),
        }
    }

    runtime.finish_update().await;
    if sigint_exit {
        exit_with_code(EXIT_SIGINT);
    }
    Ok(())
}

enum ReplInputError {
    SigInt,
    Eof,
    Io(std::io::Error),
}

fn read_next_input(prefill: Option<String>) -> Result<Option<String>, ReplInputError> {
    let raw = match prefill {
        Some(initial) => user_input_prefilled(&initial),
        None => user_input(),
    };
    match raw {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
            Err(ReplInputError::SigInt)
        }
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Err(ReplInputError::Eof),
        Err(error) => Err(ReplInputError::Io(error)),
    }
}

fn print_unless_cancelled(error: &LarpshellError) {
    if !matches!(error, LarpshellError::Cancelled) {
        error.print();
    }
}

// ── slash command dispatch ──────────────────────────────────────────────────

async fn dispatch_slash_command(
    cmd: SlashCmd,
    runtime: &mut Runtime,
    command_mode: CommandMode,
) -> Result<SlashOutcome, LarpshellError> {
    match cmd {
        SlashCmd::Quit => return Ok(SlashOutcome::Quit),
        SlashCmd::Unknown(name) => return Err(LarpshellError::UnknownSlashCommand(name)),
        SlashCmd::InvalidArgs { command, expected } => {
            return Err(LarpshellError::InvalidSlashArg {
                command: command.to_string(),
                expected: expected.to_string(),
            });
        }
        SlashCmd::Api => {
            interactive_setup()?;
            if command_mode == CommandMode::Interactive {
                runtime.reload_all()?;
            }
        }
        SlashCmd::Agent { mode: agent_mode } => {
            handle_agent_subcommand(agent_mode)?;
            if command_mode == CommandMode::Interactive {
                runtime.reload_agent()?;
            }
        }
        SlashCmd::Uninstall => uninstall_larpshell()?,
        SlashCmd::History { enable } => handle_history_subcommand(enable)?,
        SlashCmd::Prompt { kind, action } => handle_prompt_subcommand(&kind, &action)?,
        SlashCmd::Explain { args } => {
            handle_explain_subcommand(args, runtime.provider.as_ref()).await?;
        }
        SlashCmd::Help => print_slash_command_help(),
    }
    Ok(SlashOutcome::Continue)
}

fn print_slash_command_help() {
    for command in slash_commands::COMMANDS {
        println!("/{:<12} {}", command.name, command.description);
    }
}

// ── provider-less subcommands ───────────────────────────────────────────────

fn dispatch_provider_less_subcommand(sub: &cli::Subcommands) -> Result<(), LarpshellError> {
    match sub {
        cli::Subcommands::Api => interactive_setup(),
        cli::Subcommands::Uninstall => uninstall_larpshell(),
        cli::Subcommands::History { enable } => handle_history_subcommand(*enable),
        cli::Subcommands::Agent { mode } => handle_agent_subcommand(*mode),
        cli::Subcommands::Prompt { kind, action } => handle_prompt_subcommand(kind, action),
        cli::Subcommands::Explain { .. } => unreachable!("Explain needs a provider"),
    }
}

// ── Runtime ─────────────────────────────────────────────────────────────────

impl Runtime {
    fn create() -> Result<Self, LarpshellError> {
        let config = match load_config() {
            Ok(config) => config,
            Err(LarpshellError::IoError(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!(
                    "{}",
                    "run 'larpshell api' to set up your preferred provider.".custom_color(CTP_BLUE)
                );
                return Err(LarpshellError::NoProviderConfigured);
            }
            Err(error) => return Err(error),
        };
        let provider = create_provider(&config)?;
        let tool_registry = Self::build_registry(config.agent);
        let update_task = Some(tokio::task::spawn(update::is_update_available()));
        Ok(Self {
            config,
            provider,
            tool_registry,
            update_task,
        })
    }

    fn reload_all(&mut self) -> Result<(), LarpshellError> {
        let config = reload_config()?;
        self.provider = create_provider(&config)?;
        self.tool_registry = Self::build_registry(config.agent);
        self.config = config;
        Ok(())
    }

    fn reload_agent(&mut self) -> Result<(), LarpshellError> {
        let config = reload_config()?;
        self.tool_registry = Self::build_registry(config.agent);
        self.config = config;
        Ok(())
    }

    fn build_registry(mode: AgentMode) -> Option<ToolRegistry> {
        mode.is_enabled().then(|| build_tool_registry(mode))
    }

    async fn finish_update(&mut self) {
        if let Some(task) = self.update_task.take() {
            update::print_if_available(task).await;
        }
    }
}

fn reload_config() -> Result<Config, LarpshellError> {
    load_config()
        .map_err(|error| LarpshellError::ConfigError(format!("failed to reload config: {error}")))
}

fn build_tool_registry(agent_mode: AgentMode) -> ToolRegistry {
    let mut registry = ToolRegistry::with_builtins(agent_mode);
    for mcp_config in agent::mcp::load_mcp_configs() {
        match agent::mcp::StdioMcpClient::spawn(&mcp_config) {
            Ok(mut client) => {
                if let Err(error) = client.initialize() {
                    print_warning(&format!("MCP server '{}': {error}", mcp_config.name));
                    continue;
                }
                match client.list_tools() {
                    Ok(tools) => {
                        for tool in tools {
                            registry.register_mcp_tool(tool);
                        }
                    }
                    Err(error) => {
                        print_warning(&format!("MCP server '{}': {error}", mcp_config.name));
                    }
                }
                registry.add_mcp_client(client);
            }
            Err(error) => print_warning(&error),
        }
    }
    registry
}

// ── subcommand handlers ─────────────────────────────────────────────────────

fn handle_history_subcommand(enable: Option<bool>) -> Result<(), LarpshellError> {
    if let Some(on) = enable {
        config::set_history_enabled(on)?;
        cli::print_ok(if on {
            "command history enabled."
        } else {
            "command history disabled."
        });
    } else {
        let status = if config::history_enabled() {
            "enabled"
        } else {
            "disabled"
        };
        println!("command history is {status}.");
    }
    Ok(())
}

fn handle_agent_subcommand(mode: Option<AgentMode>) -> Result<(), LarpshellError> {
    if let Some(mode) = mode {
        config::set_agent_mode(mode)?;
        cli::print_ok(agent_mode_status_message(mode));
    } else {
        let mode = match config::load_config() {
            Ok(config) => config.agent,
            Err(LarpshellError::IoError(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                AgentMode::Off
            }
            Err(error) => Err(error)?,
        };
        println!("{}", agent_mode_status_message(mode));
    }
    Ok(())
}

const fn agent_mode_status_message(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Off => "agent mode: off",
        AgentMode::Safe => "agent mode: safe (restricted to read-only commands)",
        AgentMode::On => "agent mode: on",
    }
}

async fn handle_explain_subcommand(
    cmd_parts: Vec<String>,
    provider: &dyn AIProvider,
) -> Result<(), LarpshellError> {
    if cmd_parts.is_empty() {
        return Err(LarpshellError::NoCommandProvided);
    }
    let command = cmd_parts.join(" ");
    let explanation = get_explanation(&command, provider).await?;
    if explanation.is_empty() {
        return Err(LarpshellError::EmptyExplanation);
    }
    display_explanation(&explanation);
    Ok(())
}

// ── prompt subcommand ───────────────────────────────────────────────────────

struct PromptSpec {
    path: fn() -> Result<std::path::PathBuf, LarpshellError>,
    load: fn() -> Option<String>,
    save: fn(&str) -> Result<(), LarpshellError>,
    default: &'static str,
    validate: fn(&str) -> bool,
    invalid_message: &'static str,
    warn_only: bool,
}

fn prompt_spec(kind: &PromptKind) -> PromptSpec {
    match kind {
        PromptKind::System => PromptSpec {
            path: config::sys_prompt_path,
            load: config::load_sys_prompt,
            save: config::save_sys_prompt,
            default: DEFAULT_PROMPT_TEMPLATE,
            validate: validate_sys_prompt,
            invalid_message: "system prompt must contain the {request} placeholder.",
            warn_only: true,
        },
        PromptKind::Explain => PromptSpec {
            path: config::explain_prompt_path,
            load: config::load_explain_prompt,
            save: config::save_explain_prompt,
            default: DEFAULT_EXPLAIN_PROMPT,
            validate: validate_explain_prompt,
            invalid_message: "explain-prompt must contain the {command} placeholder.",
            warn_only: false,
        },
        PromptKind::Agent => PromptSpec {
            path: config::agent_prompt_path,
            load: config::load_agent_prompt,
            save: config::save_agent_prompt,
            default: DEFAULT_AGENT_PROMPT,
            validate: validate_sys_prompt,
            invalid_message: "agent prompt must contain the {request} placeholder.",
            warn_only: true,
        },
        PromptKind::AgentSafe => PromptSpec {
            path: config::agent_safe_prompt_path,
            load: config::load_agent_safe_prompt,
            save: config::save_agent_safe_prompt,
            default: DEFAULT_AGENT_SAFE_PROMPT,
            validate: validate_sys_prompt,
            invalid_message: "agent-safe prompt must contain the {request} placeholder.",
            warn_only: true,
        },
    }
}

fn handle_prompt_subcommand(
    kind: &PromptKind,
    action: &PromptAction,
) -> Result<(), LarpshellError> {
    let spec = prompt_spec(kind);
    match action {
        PromptAction::Show => show_prompt(&spec),
        PromptAction::Edit => edit_prompt(&spec)?,
        PromptAction::Reset => reset_prompt(&spec)?,
    }
    Ok(())
}

fn show_prompt(spec: &PromptSpec) {
    let content = (spec.load)().unwrap_or_else(|| spec.default.to_string());
    println!("{content}");
}

fn edit_prompt(spec: &PromptSpec) -> Result<(), LarpshellError> {
    let path = (spec.path)()?;
    if !path.exists() {
        (spec.save)(spec.default)?;
    }
    open_in_editor(&path)?;
    if let Some(saved) = (spec.load)()
        && !(spec.validate)(&saved)
    {
        if spec.warn_only {
            print_warning(spec.invalid_message);
        } else {
            return Err(LarpshellError::ConfigError(
                spec.invalid_message.to_string(),
            ));
        }
    }
    Ok(())
}

fn reset_prompt(spec: &PromptSpec) -> Result<(), LarpshellError> {
    let path = (spec.path)()?;
    if path.exists() {
        let bak = path.with_extension(path.extension().map_or_else(
            || "bak".to_string(),
            |ext| format!("{ext}.bak", ext = ext.to_string_lossy()),
        ));
        std::fs::rename(&path, &bak).map_err(LarpshellError::IoError)?;
        (spec.save)(spec.default)?;
        cli::print_ok(&format!(
            "Reset to default (backup saved as {})",
            bak.file_name().unwrap_or_default().to_string_lossy()
        ));
    } else {
        (spec.save)(spec.default)?;
        cli::print_ok("Reset to default.");
    }
    Ok(())
}

fn open_in_editor(path: &std::path::Path) -> Result<(), LarpshellError> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
    std::process::Command::new(&editor).arg(path).status()?;
    Ok(())
}

// ── command processing ──────────────────────────────────────────────────────

async fn process_user_input(
    user_input: &str,
    runtime: &mut Runtime,
    mode: CommandMode,
) -> Result<Option<String>, LarpshellError> {
    if runtime.config.agent.is_enabled() {
        let registry = runtime
            .tool_registry
            .get_or_insert_with(|| build_tool_registry(runtime.config.agent));
        process_command_agent(
            user_input,
            runtime.provider.as_ref(),
            &runtime.config,
            mode,
            registry,
        )
        .await
    } else {
        process_command(user_input, runtime.provider.as_ref(), &runtime.config, mode).await
    }
}

async fn process_command(
    user_input: &str,
    provider: &dyn AIProvider,
    config: &Config,
    mode: CommandMode,
) -> Result<Option<String>, LarpshellError> {
    let model_name = config.provider_config()?.config.model().to_string();
    hide_cursor();
    eprint_status(&format!("using {model_name}..."));

    let effective_sys = config::load_sys_prompt().filter(|prompt| validate_sys_prompt(prompt));
    let prompt = create_system_prompt(user_input, effective_sys.as_deref());

    let response = match &mode {
        CommandMode::Interactive => generate_with_cancellation(provider, &prompt).await?,
        CommandMode::Single => generate_single_shot(provider, &prompt).await?,
    };

    let command = clean_response(&response);
    if command.trim().is_empty() {
        return Err(LarpshellError::EmptyResponse(provider.name()));
    }

    confirm_loop(command, user_input, provider, &mode, ResponseStyle::Command).await
}

async fn process_command_agent(
    user_input: &str,
    provider: &dyn AIProvider,
    config: &Config,
    mode: CommandMode,
    tool_registry: &ToolRegistry,
) -> Result<Option<String>, LarpshellError> {
    let response = agent::run_agent_loop(user_input, provider, config, tool_registry).await?;
    let message = response
        .message
        .as_deref()
        .map(str::trim)
        .filter(|msg| !msg.is_empty());
    let command = response
        .command
        .as_deref()
        .map(str::trim)
        .filter(|cmd| !cmd.is_empty());

    if let Some(message) = message {
        display_response(message, ResponseStyle::Message);
        if let Some(command) = command {
            return confirm_loop(
                command.to_string(),
                user_input,
                provider,
                &mode,
                ResponseStyle::Command,
            )
            .await;
        }
        return Ok(None);
    }

    if let Some(command) = command {
        return confirm_loop(
            command.to_string(),
            user_input,
            provider,
            &mode,
            ResponseStyle::Command,
        )
        .await;
    }

    match response.kind {
        agent::FinalResponseKind::Command => {
            let command = clean_response(&response.content);
            if command.trim().is_empty() {
                return Err(LarpshellError::EmptyResponse(provider.name()));
            }
            confirm_loop(command, user_input, provider, &mode, ResponseStyle::Command).await
        }
        agent::FinalResponseKind::Message => {
            let message = response.content.trim().to_string();
            if message.is_empty() {
                return Err(LarpshellError::EmptyResponse(provider.name()));
            }
            display_response(&message, ResponseStyle::Message);
            Ok(None)
        }
    }
}

// ── generation with cancellation ────────────────────────────────────────────

async fn generate_with_cancellation(
    provider: &dyn AIProvider,
    prompt: &str,
) -> Result<String, LarpshellError> {
    #[cfg(unix)]
    let saved_echo = common::disable_terminal_echo();

    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    let ctrl_c = tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_clone.cancel();
    });
    let result = tokio::select! {
        res = provider.generate(prompt) => {
            ctrl_c.abort();
            res
        }
        () = cancel_token.cancelled() => Err(LarpshellError::Cancelled),
    };
    clear_line();
    show_cursor();
    #[cfg(unix)]
    if let Some(saved) = saved_echo.as_ref() {
        common::restore_terminal_echo(saved);
    }
    result
}

/// In single-command mode, Ctrl-C exits the process immediately with
/// `EXIT_SIGINT` after restoring the terminal and printing any pending update.
async fn generate_single_shot(
    provider: &dyn AIProvider,
    prompt: &str,
) -> Result<String, LarpshellError> {
    #[cfg(unix)]
    let saved_echo = common::disable_terminal_echo();
    #[cfg(unix)]
    let saved_for_ctrlc = saved_echo.clone();

    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_clone.cancel();
        eprintln!();
        #[cfg(unix)]
        if let Some(saved) = saved_for_ctrlc.as_ref() {
            common::restore_terminal_echo(saved);
        }
        update::print_if_resolved();
        exit_with_code(EXIT_SIGINT);
    });

    let result = provider.generate(prompt).await;
    clear_line();
    show_cursor();
    #[cfg(unix)]
    if let Some(saved) = saved_echo.as_ref() {
        common::restore_terminal_echo(saved);
    }
    result
}

// ── confirmation loop ───────────────────────────────────────────────────────

async fn confirm_loop(
    mut command: String,
    user_input: &str,
    provider: &dyn AIProvider,
    mode: &CommandMode,
    response_style: ResponseStyle,
) -> Result<Option<String>, LarpshellError> {
    let cancelled = 'outer: loop {
        let cmd_lines = display_response(&command, response_style);
        match confirm_with_explain(cmd_lines) {
            ConfirmResult::Yes => {
                execute_or_print(&command)?;
                break 'outer false;
            }
            ConfirmResult::No => break 'outer false,
            ConfirmResult::Cancel => match mode {
                CommandMode::Interactive => break 'outer true,
                CommandMode::Single => exit_on_sigint(),
            },
            ConfirmResult::Edit => {
                if let Some(new_cmd) = edit_command(&command) {
                    command = new_cmd;
                }
            }
            ConfirmResult::Explain => {
                let explanation = get_explanation(&command, provider).await?;
                let expl_lines = display_explanation(&explanation);
                match confirm_execution(cmd_lines, expl_lines) {
                    ConfirmResult::Yes => {
                        execute_or_print(&command)?;
                        break 'outer false;
                    }
                    ConfirmResult::No | ConfirmResult::Explain => break 'outer false,
                    ConfirmResult::Cancel => match mode {
                        CommandMode::Interactive => break 'outer true,
                        CommandMode::Single => exit_on_sigint(),
                    },
                    ConfirmResult::Edit => {
                        if let Some(new_cmd) = edit_command(&command) {
                            command = new_cmd;
                        }
                    }
                }
            }
        }
    };

    Ok(cancelled.then(|| user_input.to_string()))
}

fn exit_on_sigint() -> ! {
    show_cursor();
    update::print_if_resolved();
    exit_with_code(EXIT_SIGINT);
}

fn execute_or_print(command: &str) -> Result<(), LarpshellError> {
    if std::io::stdout().is_terminal() || std::env::var("LARPSHELL_FORCE_INTERACTIVE").is_ok() {
        execute_shell_command(command)?;
    } else {
        println!("{command}");
    }
    Ok(())
}

fn eprint_status(message: &str) {
    eprint_flush(&message.custom_color(CTP_OVERLAY0).to_string());
}

// ── explanation helper ──────────────────────────────────────────────────────

async fn get_explanation(
    command: &str,
    provider: &dyn AIProvider,
) -> Result<String, LarpshellError> {
    let effective = config::load_explain_prompt().filter(|prompt| validate_explain_prompt(prompt));
    let query = create_explain_prompt(command, effective.as_deref());

    hide_cursor();
    eprint_status("explaining...");

    let result = generate_with_cancellation(provider, &query).await?;
    Ok(prompt::clean_explanation(&result, command))
}
