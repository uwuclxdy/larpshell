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

use std::io::IsTerminal;
use tokio_util::sync::CancellationToken;

use agent::tools::ToolRegistry;
use cli::home_dir;
use cli::{
    PromptAction, PromptKind, execute_shell_command, parse_cli_args, print_warning,
};
use colored::*;
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
use providers::create_provider;
use shell_integration::{auto_setup_shell_function, migrate_nlsh_rs_shell};
use uninstall::uninstall_larpshell;

/// Differentiates interactive (REPL) vs single-command mode.
enum CommandMode {
    Interactive,
    Single,
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
                            registry.register_mcp_tool(tool, mcp_config.name.clone());
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

fn agent_mode_status_message(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Off => "agent mode is currently off.",
        AgentMode::Safe => "agent mode is currently safe.",
        AgentMode::On => "agent mode is currently on.",
    }
}

fn agent_mode_set_message(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Off => "agent mode disabled.",
        AgentMode::Safe => {
            "agent mode set to safe — tools are enabled with restricted command execution."
        }
        AgentMode::On => {
            "agent mode set to on — tools are enabled and commands are only gated by confirmation."
        }
    }
}

// ── cancellation wrapper ────────────────────────────────────────────────────

async fn generate_with_cancellation(
    provider: &dyn providers::AIProvider,
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
        _ = cancel_token.cancelled() => {
            Err(LarpshellError::Cancelled)
        }
    };
    clear_line();
    show_cursor();
    #[cfg(unix)]
    if let Some(saved) = saved_echo {
        common::restore_terminal_echo(saved);
    }
    result
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn execute_or_print(command: &str) -> Result<(), LarpshellError> {
    if std::io::stdout().is_terminal() || std::env::var("LARPSHELL_FORCE_INTERACTIVE").is_ok() {
        execute_shell_command(command)?;
    } else {
        println!("{}", command);
    }
    Ok(())
}

// ── subcommand handlers ─────────────────────────────────────────────────────

fn handle_history_subcommand(enable: Option<bool>) -> Result<(), LarpshellError> {
    match enable {
        Some(enable) => {
            config::set_history_enabled(enable)?;
            if enable {
                cli::print_ok("command history enabled.");
            } else {
                cli::print_ok("command history disabled.");
            }
        }
        None => {
            let enabled = config::history_enabled();
            let status = if enabled { "enabled" } else { "disabled" };
            println!("command history is {status}.");
        }
    }
    Ok(())
}

fn handle_agent_subcommand(mode: Option<AgentMode>) -> Result<(), LarpshellError> {
    match mode {
        Some(mode) => {
            config::set_agent_mode(mode)?;
            cli::print_ok(agent_mode_set_message(mode));
        }
        None => {
            let mode = match config::load_config() {
                Ok(config) => config.agent,
                Err(LarpshellError::IoError(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    AgentMode::Off
                }
                Err(error) => return Err(error),
            };
            println!("{}", agent_mode_status_message(mode));
        }
    }
    Ok(())
}

fn open_in_editor(path: &std::path::Path) -> Result<(), LarpshellError> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
    std::process::Command::new(&editor).arg(path).status()?;
    Ok(())
}

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
            return Err(LarpshellError::ConfigError(spec.invalid_message.to_string()));
        }
    }
    Ok(())
}

fn reset_prompt(spec: &PromptSpec) -> Result<(), LarpshellError> {
    let path = (spec.path)()?;
    if path.exists() {
        let bak = path.with_extension(
            path.extension()
                .map(|e| format!("{}.bak", e.to_string_lossy()))
                .unwrap_or_else(|| "bak".to_string()),
        );
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

async fn handle_explain_subcommand(
    cmd_parts: Vec<String>,
    provider: &dyn providers::AIProvider,
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

fn reload_runtime_state(
    config: &mut Config,
    provider: &mut Box<dyn providers::AIProvider>,
    tool_registry: &mut Option<ToolRegistry>,
) -> Result<(), LarpshellError> {
    let new_config = load_config()
        .map_err(|e| LarpshellError::ConfigError(format!("failed to reload config: {e}")))?;
    let new_provider = create_provider(&new_config)?;
    *config = new_config;
    *provider = new_provider;
    *tool_registry = if config.agent.is_enabled() {
        Some(build_tool_registry(config.agent))
    } else {
        None
    };
    Ok(())
}

fn reload_agent_state(
    config: &mut Config,
    tool_registry: &mut Option<ToolRegistry>,
) -> Result<(), LarpshellError> {
    let new_config = load_config()
        .map_err(|e| LarpshellError::ConfigError(format!("failed to reload config: {e}")))?;
    *config = new_config;
    *tool_registry = if config.agent.is_enabled() {
        Some(build_tool_registry(config.agent))
    } else {
        None
    };
    Ok(())
}

fn handle_api_slash_command(
    config: &mut Config,
    provider: &mut Box<dyn providers::AIProvider>,
    tool_registry: &mut Option<ToolRegistry>,
) {
    if let Err(e) = interactive_setup() {
        e.print();
    } else if let Err(e) = reload_runtime_state(config, provider, tool_registry) {
        e.print();
    }
}

fn handle_agent_slash_command(
    mode: Option<AgentMode>,
    config: &mut Config,
    tool_registry: &mut Option<ToolRegistry>,
) {
    if let Err(e) = handle_agent_subcommand(mode) {
        e.print();
    } else if let Err(e) = reload_agent_state(config, tool_registry) {
        e.print();
    }
}

// ── main ────────────────────────────────────────────────────────────────────

fn do_nlsh_rs_migration() {
    // No-op if nlsh-rs is not installed.
    let binary = home_dir().join(".cargo/bin/nlsh-rs");
    if !binary.exists() {
        return;
    }

    config::migrate_from_nlsh_rs().ok();
    migrate_nlsh_rs_shell().ok();

    // Uninstall the old binary silently in the background.
    std::process::Command::new("cargo")
        .args(["uninstall", "nlsh-rs"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok();
}

/// low‑level implementation of `main` which returns a `Result`.  the
/// top‑level `main` wrapper will call this and take care of printing a
/// nicely styled error message when it fails.
async fn inner_main() -> Result<(), LarpshellError> {
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

    do_nlsh_rs_migration();

    let cli = parse_cli_args()?;

    if cli.subcommand.is_none() {
        match auto_setup_shell_function() {
            Ok(true) => {
                eprintln!(
                    "{}",
                    "restart shell or run 'source ~/.bashrc' ('source ~/.config/fish/config.fish' for fish).".custom_color(CTP_YELLOW)
                );
                exit_with_code(0);
            }
            Ok(false) => {}
            Err(_) => {}
        }
    }

    // handle subcommands that do not need a provider
    let needs_provider = matches!(cli.subcommand, Some(cli::Subcommands::Explain { .. }));
    if !needs_provider && let Some(ref command) = cli.subcommand {
        match command {
            cli::Subcommands::Api => {
                interactive_setup()?;
                return Ok(());
            }
            cli::Subcommands::Uninstall => {
                uninstall_larpshell()?;
                return Ok(());
            }
            cli::Subcommands::History { enable } => {
                handle_history_subcommand(*enable)?;
                return Ok(());
            }
            cli::Subcommands::Agent { mode } => {
                handle_agent_subcommand(*mode)?;
                return Ok(());
            }
            cli::Subcommands::Prompt { kind, action } => {
                handle_prompt_subcommand(kind, action)?;
                return Ok(());
            }
            cli::Subcommands::Explain { .. } => unreachable!(),
        }
    }

    // move cli.subcommand to extract explain parts (non-explain paths already returned above)
    let explain_parts = match cli.subcommand {
        Some(cli::Subcommands::Explain { command }) => Some(command),
        _ => None,
    };

    let mut config = match load_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            if matches!(&e, LarpshellError::IoError(io_err) if io_err.kind() == std::io::ErrorKind::NotFound)
            {
                eprintln!(
                    "{}",
                    "run 'larpshell api' to set up your preferred provider.".custom_color(CTP_BLUE)
                );
                return Err(LarpshellError::NoProviderConfigured);
            }
            return Err(e);
        }
    };

    let mut provider = match create_provider(&config) {
        Ok(p) => p,
        Err(e) => return Err(e),
    };

    let update_task = tokio::task::spawn(update::is_update_available());

    let mut tool_registry = if config.agent.is_enabled() {
        Some(build_tool_registry(config.agent))
    } else {
        None
    };

    // handle standalone explain subcommand
    if let Some(cmd_parts) = explain_parts {
        let result = handle_explain_subcommand(cmd_parts, provider.as_ref()).await;
        update::print_if_available(update_task).await;
        return result;
    }

    let interactive_mode = cli.command.is_empty() && cli::is_interactive_terminal();

    if cli.command.is_empty() && !cli::is_interactive_terminal() {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        let user_input = buf.trim().to_string();
        if !user_input.is_empty() {
            if user_input.starts_with('/') {
                match slash_commands::parse(&user_input) {
                    slash_commands::SlashCmd::Quit => {
                        update::print_if_available(update_task).await;
                        return Ok(());
                    }
                    slash_commands::SlashCmd::Api => {
                        interactive_setup()?;
                    }
                    slash_commands::SlashCmd::Agent { mode } => {
                        handle_agent_subcommand(mode)?;
                    }
                    slash_commands::SlashCmd::Uninstall => {
                        uninstall_larpshell()?;
                    }
                    slash_commands::SlashCmd::History { enable } => {
                        handle_history_subcommand(enable)?;
                    }
                    slash_commands::SlashCmd::Prompt { kind, action } => {
                        handle_prompt_subcommand(&kind, &action)?;
                    }
                    slash_commands::SlashCmd::Explain { args } => {
                        handle_explain_subcommand(args, provider.as_ref()).await?;
                    }
                    slash_commands::SlashCmd::Help => {
                        for cmd in slash_commands::COMMANDS {
                            println!("/{:<12} {}", cmd.name, cmd.description);
                        }
                    }
                    slash_commands::SlashCmd::Unknown(s) => {
                        LarpshellError::UnknownSlashCommand(s).print();
                    }
                    slash_commands::SlashCmd::InvalidArgs { command, expected } => {
                        LarpshellError::InvalidSlashArg {
                            command: command.to_string(),
                            expected: expected.to_string(),
                        }
                        .print();
                    }
                }
            } else {
                process_command(&user_input, provider.as_ref(), &config, CommandMode::Single)
                    .await?;
            }
        }
        update::print_if_available(update_task).await;
        return Ok(());
    }

    if interactive_mode {
        // interactive mode: keep running until exit signal at prompt
        let mut prefill: Option<String> = None;
        let mut sigint_exit = false;
        loop {
            interactive::reserve_preview_space();
            let raw_input = match if let Some(initial) = prefill.take() {
                user_input_prefilled(&initial)
            } else {
                user_input()
            } {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    sigint_exit = true;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(LarpshellError::IoError(e)),
            };
            let user_input = match raw_input {
                Some(input) => input,
                None => continue,
            };
            clear_line();

            if user_input.starts_with('/') {
                match slash_commands::parse(&user_input) {
                    slash_commands::SlashCmd::Quit => {
                        show_cursor();
                        break;
                    }
                    slash_commands::SlashCmd::Api => {
                        handle_api_slash_command(&mut config, &mut provider, &mut tool_registry);
                    }
                    slash_commands::SlashCmd::Agent { mode } => {
                        handle_agent_slash_command(mode, &mut config, &mut tool_registry);
                    }
                    slash_commands::SlashCmd::Uninstall => {
                        if let Err(e) = uninstall_larpshell() {
                            e.print();
                        }
                    }
                    slash_commands::SlashCmd::History { enable } => {
                        if let Err(e) = handle_history_subcommand(enable) {
                            e.print();
                        }
                    }
                    slash_commands::SlashCmd::Prompt { kind, action } => {
                        if let Err(e) = handle_prompt_subcommand(&kind, &action) {
                            e.print();
                        }
                    }
                    slash_commands::SlashCmd::Explain { args } => {
                        if let Err(e) = handle_explain_subcommand(args, provider.as_ref()).await
                            && !matches!(e, LarpshellError::Cancelled)
                        {
                            e.print();
                        }
                    }
                    slash_commands::SlashCmd::Help => {
                        for cmd in slash_commands::COMMANDS {
                            println!("/{:<12} {}", cmd.name, cmd.description);
                        }
                    }
                    slash_commands::SlashCmd::Unknown(s) => {
                        LarpshellError::UnknownSlashCommand(s).print();
                    }
                    slash_commands::SlashCmd::InvalidArgs { command, expected } => {
                        LarpshellError::InvalidSlashArg {
                            command: command.to_string(),
                            expected: expected.to_string(),
                        }
                        .print();
                    }
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

            let result = if config.agent.is_enabled() {
                let registry =
                    tool_registry.get_or_insert_with(|| build_tool_registry(config.agent));
                process_command_agent(
                    &user_input,
                    provider.as_ref(),
                    &config,
                    CommandMode::Interactive,
                    registry,
                )
                .await
            } else {
                process_command(
                    &user_input,
                    provider.as_ref(),
                    &config,
                    CommandMode::Interactive,
                )
                .await
            };

            match result {
                Ok(Some(p)) => {
                    // move up past the old rustyline prompt line so the new prompt overwrites it
                    eprint_flush("\x1b[1A\x1b[K");
                    prefill = Some(p);
                }
                Ok(None) => {}
                Err(e) => {
                    if !matches!(e, LarpshellError::Cancelled) {
                        e.print();
                    }
                }
            }
        }
        update::print_if_available(update_task).await;
        if sigint_exit {
            exit_with_code(EXIT_SIGINT);
        }
        return Ok(());
    } else {
        // single-command mode: execute once and exit
        let user_input = cli.command.join(" ");
        if config.agent.is_enabled() {
            let registry = tool_registry.get_or_insert_with(|| build_tool_registry(config.agent));
            process_command_agent(
                &user_input,
                provider.as_ref(),
                &config,
                CommandMode::Single,
                registry,
            )
            .await?;
        } else {
            process_command(&user_input, provider.as_ref(), &config, CommandMode::Single).await?;
        }
    }

    update::print_if_available(update_task).await;

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = inner_main().await {
        e.print();
        exit_with_code(1);
    }
}

// ── unified command processing ──────────────────────────────────────────────

async fn process_command(
    user_input: &str,
    provider: &dyn providers::AIProvider,
    config: &Config,
    mode: CommandMode,
) -> Result<Option<String>, LarpshellError> {
    let model_name = config.provider_config()?.config.model().to_string();
    hide_cursor();
    eprint_flush(&format!(
        "{}",
        format!("using {}...", model_name).custom_color(CTP_OVERLAY0)
    ));

    let effective_sys = config::load_sys_prompt().filter(|p| validate_sys_prompt(p));
    let prompt = create_system_prompt(user_input, effective_sys.as_deref());

    let response = match &mode {
        CommandMode::Interactive => generate_with_cancellation(provider, &prompt).await?,
        CommandMode::Single => {
            // In single mode, Ctrl+C exits immediately
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
                if let Some(saved) = saved_for_ctrlc {
                    common::restore_terminal_echo(saved);
                }
                update::print_if_resolved();
                exit_with_code(EXIT_SIGINT);
            });

            match provider.generate(&prompt).await {
                Ok(res) => {
                    clear_line();
                    show_cursor();
                    #[cfg(unix)]
                    if let Some(saved) = saved_echo {
                        common::restore_terminal_echo(saved);
                    }
                    res
                }
                Err(e) => {
                    clear_line();
                    show_cursor();
                    #[cfg(unix)]
                    if let Some(saved) = saved_echo {
                        common::restore_terminal_echo(saved);
                    }
                    return Err(e);
                }
            }
        }
    };

    let command = clean_response(&response);

    if command.trim().is_empty() {
        return Err(LarpshellError::EmptyResponse(provider.name()));
    }

    confirm_loop(command, user_input, provider, &mode, ResponseStyle::Command).await
}

async fn process_command_agent(
    user_input: &str,
    provider: &dyn providers::AIProvider,
    config: &Config,
    mode: CommandMode,
    tool_registry: &ToolRegistry,
) -> Result<Option<String>, LarpshellError> {
    let response = agent::run_agent_loop(user_input, provider, config, tool_registry).await?;

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

// ── confirmation loop ──────────────────────────────────────────────────────

async fn confirm_loop(
    mut command: String,
    user_input: &str,
    provider: &dyn providers::AIProvider,
    mode: &CommandMode,
    response_style: ResponseStyle,
) -> Result<Option<String>, LarpshellError> {
    let cancelled = 'outer: loop {
        let cmd_lines = display_response(&command, response_style);
        match confirm_with_explain(cmd_lines)? {
            ConfirmResult::Yes => {
                execute_or_print(&command)?;
                break 'outer false;
            }
            ConfirmResult::No => break 'outer false,
            ConfirmResult::Cancel => match mode {
                CommandMode::Interactive => break 'outer true,
                CommandMode::Single => {
                    show_cursor();
                    update::print_if_resolved();
                    exit_with_code(EXIT_SIGINT);
                }
            },
            ConfirmResult::Edit => match edit_command(&command) {
                Some(new_cmd) => command = new_cmd,
                None => continue 'outer,
            },
            ConfirmResult::Explain => {
                let explanation = get_explanation(&command, provider).await?;
                let expl_lines = display_explanation(&explanation);
                match confirm_execution(cmd_lines, expl_lines)? {
                    ConfirmResult::Yes => {
                        execute_or_print(&command)?;
                        break 'outer false;
                    }
                    ConfirmResult::No => break 'outer false,
                    ConfirmResult::Cancel => match mode {
                        CommandMode::Interactive => break 'outer true,
                        CommandMode::Single => {
                            show_cursor();
                            exit_with_code(EXIT_SIGINT);
                        }
                    },
                    ConfirmResult::Edit => match edit_command(&command) {
                        Some(new_cmd) => command = new_cmd,
                        None => continue 'outer,
                    },
                    ConfirmResult::Explain => break 'outer false,
                }
            }
        }
    };

    if cancelled {
        Ok(Some(user_input.to_string()))
    } else {
        Ok(None)
    }
}

// ── explanation helper ──────────────────────────────────────────────────────

async fn get_explanation(
    command: &str,
    provider: &dyn providers::AIProvider,
) -> Result<String, LarpshellError> {
    let effective = config::load_explain_prompt().filter(|p| validate_explain_prompt(p));
    let query = create_explain_prompt(command, effective.as_deref());

    hide_cursor();
    eprint_flush(&format!("{}", "explaining...".custom_color(CTP_OVERLAY0)));

    let result = generate_with_cancellation(provider, &query).await?;
    let cleaned = prompt::clean_explanation(&result, command);
    Ok(cleaned)
}
