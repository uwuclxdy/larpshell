#[cfg(test)]
mod tests;

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

use cli::get_home_dir;
use cli::{
    PromptAction, PromptKind, execute_shell_command, parse_cli_args, print_error, print_warning,
};
use colored::*;
#[cfg(unix)]
use common::setup_terminal;
use common::{
    CTP_BLUE, CTP_OVERLAY0, CTP_YELLOW, EXIT_SIGINT, clear_line, eprint_flush, exit_with_code,
    hide_cursor, show_cursor,
};
use config::{Config, interactive_setup, load_config};
use confirmation::{
    ConfirmResult, confirm_execution, confirm_with_explain, display_command, display_explanation,
    edit_command,
};
use error::LarpshellError;
use interactive::{get_user_input, get_user_input_prefilled};
use prompt::{
    DEFAULT_EXPLAIN_PROMPT, DEFAULT_PROMPT_TEMPLATE, clean_response, create_explain_prompt,
    create_prompts, create_system_prompt, validate_explain_prompt, validate_sys_prompt,
};
use providers::create_provider;
use shell_integration::{auto_setup_shell_function, migrate_nlsh_rs_shell};
use uninstall::uninstall_larpshell;

/// Differentiates interactive (REPL) vs single-command mode.
enum CommandMode {
    Interactive,
    Single,
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

fn execute_or_print(command: &str) -> Result<(), Box<dyn std::error::Error>> {
    if std::io::stdout().is_terminal() || std::env::var("LARPSHELL_FORCE_INTERACTIVE").is_ok() {
        execute_shell_command(command)?;
    } else {
        println!("{}", command);
    }
    Ok(())
}

// ── subcommand handlers ─────────────────────────────────────────────────────

fn handle_history_subcommand(enable: bool) -> Result<(), Box<dyn std::error::Error>> {
    config::set_history_enabled(enable)?;
    if enable {
        cli::print_ok("history enabled — prompts will be saved across sessions.");
    } else {
        cli::print_ok("history disabled.");
    }
    Ok(())
}

fn handle_prompt_subcommand(
    kind: &PromptKind,
    action: &PromptAction,
) -> Result<(), Box<dyn std::error::Error>> {
    match (kind, action) {
        (PromptKind::System, PromptAction::Show) => {
            let content =
                config::load_sys_prompt().unwrap_or_else(|| DEFAULT_PROMPT_TEMPLATE.to_string());
            println!("{}", content);
        }
        (PromptKind::System, PromptAction::Edit) => {
            let path = config::get_sys_prompt_path()?;
            if !path.exists() {
                config::save_sys_prompt(DEFAULT_PROMPT_TEMPLATE)?;
            }
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
            std::process::Command::new(&editor).arg(&path).status()?;
            if let Some(saved) = config::load_sys_prompt()
                && !validate_sys_prompt(&saved)
            {
                print_warning("system prompt must contain the {request} placeholder.");
            }
        }
        (PromptKind::Explain, PromptAction::Show) => {
            let content =
                config::load_explain_prompt().unwrap_or_else(|| DEFAULT_EXPLAIN_PROMPT.to_string());
            println!("{}", content);
        }
        (PromptKind::Explain, PromptAction::Edit) => {
            let path = config::get_explain_prompt_path()?;
            if !path.exists() {
                config::save_explain_prompt(DEFAULT_EXPLAIN_PROMPT)?;
            }
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
            std::process::Command::new(&editor).arg(&path).status()?;
            if let Some(saved) = config::load_explain_prompt()
                && !validate_explain_prompt(&saved)
            {
                print_error("explain-prompt must contain the {command} placeholder.");
            }
        }
    }
    Ok(())
}

async fn handle_explain_subcommand(
    cmd_parts: Vec<String>,
    provider: &dyn providers::AIProvider,
) -> Result<(), Box<dyn std::error::Error>> {
    if cmd_parts.is_empty() {
        print_error("no command provided.");
        exit_with_code(1);
    }
    let command = cmd_parts.join(" ");
    let explanation = get_explanation(&command, provider).await?;
    if explanation.is_empty() {
        print_error("failed to generate a valid explanation.");
        return Ok(());
    }
    display_explanation(&explanation);
    Ok(())
}

// ── main ────────────────────────────────────────────────────────────────────

fn do_nlsh_rs_migration() {
    // No-op if nlsh-rs is not installed.
    let binary = get_home_dir().join(".cargo/bin/nlsh-rs");
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
async fn inner_main() -> Result<(), Box<dyn std::error::Error>> {
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

    let cli = parse_cli_args()?;

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
            if let Some(io_err) = e.downcast_ref::<std::io::Error>()
                && io_err.kind() == std::io::ErrorKind::NotFound
            {
                print_error("no API provider configured.");
                eprintln!(
                    "{}",
                    "run 'larpshell api' to set up your preferred provider.".custom_color(CTP_BLUE)
                );
                exit_with_code(1);
            }
            let err = LarpshellError::ConfigError(e.to_string());
            print_error(&err.to_string());
            exit_with_code(1);
        }
    };

    let mut provider = match create_provider(&config) {
        Ok(p) => p,
        Err(e) => {
            print_error(&e.to_string());
            exit_with_code(1);
        }
    };

    let update_task = tokio::task::spawn(update::is_update_available());

    // handle standalone explain subcommand
    if let Some(cmd_parts) = explain_parts {
        let result = handle_explain_subcommand(cmd_parts, provider.as_ref()).await;
        update::print_if_available(update_task).await;
        return result;
    }

    let interactive_mode = cli.command.is_empty() && std::io::stdin().is_terminal();

    if cli.command.is_empty() && !std::io::stdin().is_terminal() {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        let user_input = buf.trim().to_string();
        if !user_input.is_empty() {
            process_command(&user_input, provider.as_ref(), &config, CommandMode::Single).await?;
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
                get_user_input_prefilled(&initial)
            } else {
                get_user_input()
            } {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    sigint_exit = true;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(Box::new(e)),
            };
            let user_input = match raw_input {
                Some(input) => input,
                None => continue,
            };

            if user_input.starts_with('/') {
                match slash_commands::parse(&user_input) {
                    slash_commands::SlashCmd::Quit => {
                        show_cursor();
                        break;
                    }
                    slash_commands::SlashCmd::Api => match interactive_setup() {
                        Err(e) => print_error(&e.to_string()),
                        Ok(()) => match load_config() {
                            Err(e) => print_error(&format!("failed to reload config: {e}")),
                            Ok(new_config) => match create_provider(&new_config) {
                                Err(e) => print_error(&e.to_string()),
                                Ok(new_provider) => {
                                    config = new_config;
                                    provider = new_provider;
                                }
                            },
                        },
                    },
                    slash_commands::SlashCmd::Uninstall => {
                        if let Err(e) = uninstall_larpshell() {
                            print_error(&e.to_string());
                        }
                    }
                    slash_commands::SlashCmd::History { enable } => {
                        if let Err(e) = handle_history_subcommand(enable) {
                            print_error(&e.to_string());
                        }
                    }
                    slash_commands::SlashCmd::Prompt { kind, action } => {
                        if let Err(e) = handle_prompt_subcommand(&kind, &action) {
                            print_error(&e.to_string());
                        }
                    }
                    slash_commands::SlashCmd::Explain { args } => {
                        if let Err(e) = handle_explain_subcommand(args, provider.as_ref()).await
                            && !matches!(
                                e.downcast_ref::<LarpshellError>(),
                                Some(LarpshellError::Cancelled)
                            )
                        {
                            print_error(&e.to_string());
                        }
                    }
                    slash_commands::SlashCmd::Unknown(s) => {
                        print_error(&format!("unknown command '{s}'"));
                    }
                }
                continue;
            }

            match process_command(
                &user_input,
                provider.as_ref(),
                &config,
                CommandMode::Interactive,
            )
            .await
            {
                Ok(Some(p)) => {
                    // move up past the old rustyline prompt line so the new prompt overwrites it
                    eprint_flush("\x1b[1A\x1b[K");
                    prefill = Some(p);
                }
                Ok(None) => {}
                Err(e) => {
                    if !matches!(
                        e.downcast_ref::<LarpshellError>(),
                        Some(LarpshellError::Cancelled)
                    ) {
                        print_error(&e.to_string());
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
        process_command(&user_input, provider.as_ref(), &config, CommandMode::Single).await?;
    }

    update::print_if_available(update_task).await;

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = inner_main().await {
        if let Some(nl) = e.downcast_ref::<LarpshellError>() {
            nl.print();
        } else {
            print_error(&e.to_string());
        }
        exit_with_code(1);
    }
}

// ── unified command processing ──────────────────────────────────────────────

async fn process_command(
    user_input: &str,
    provider: &dyn providers::AIProvider,
    config: &Config,
    mode: CommandMode,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let model_name = config.get_provider_config()?.config.model().to_string();
    hide_cursor();
    eprint_flush(&format!(
        "{}",
        format!("using {}...", model_name).custom_color(CTP_OVERLAY0)
    ));

    let effective_sys = config::load_sys_prompt().filter(|p| validate_sys_prompt(p));
    let prompt = create_system_prompt(user_input, effective_sys.as_deref());

    let response = match &mode {
        CommandMode::Interactive => match generate_with_cancellation(provider, &prompt).await {
            Ok(res) => res,
            Err(e) => return Err(Box::new(e)),
        },
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
                    return Err(Box::new(e));
                }
            }
        }
    };

    let mut command = clean_response(&response);

    if command.trim().is_empty() {
        return Err(Box::new(LarpshellError::EmptyResponse(provider.name())));
    }

    let cancelled = 'outer: loop {
        let cmd_lines = display_command(&command);
        match confirm_with_explain(cmd_lines)? {
            ConfirmResult::Yes => {
                execute_or_print(&command)?;
                break 'outer false;
            }
            ConfirmResult::No => break 'outer false,
            ConfirmResult::Cancel => match &mode {
                CommandMode::Interactive => break 'outer true,
                CommandMode::Single => {
                    show_cursor();
                    update::print_if_resolved();
                    exit_with_code(EXIT_SIGINT);
                }
            },
            ConfirmResult::Edit => match edit_command(&command) {
                Some(new_cmd) => command = new_cmd,
                None => break 'outer false,
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
                    ConfirmResult::Cancel => match &mode {
                        CommandMode::Interactive => break 'outer true,
                        CommandMode::Single => {
                            show_cursor();
                            exit_with_code(EXIT_SIGINT);
                        }
                    },
                    ConfirmResult::Edit => match edit_command(&command) {
                        Some(new_cmd) => command = new_cmd,
                        None => break 'outer false,
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
) -> Result<String, Box<dyn std::error::Error>> {
    let effective = config::load_explain_prompt().filter(|p| validate_explain_prompt(p));
    let query = create_explain_prompt(command, effective.as_deref());

    hide_cursor();
    eprint_flush(&format!("{}", "explaining...".custom_color(CTP_OVERLAY0)));

    let result = generate_with_cancellation(provider, &query).await?;
    let cleaned = prompt::clean_explanation(&result, command);
    Ok(cleaned)
}
