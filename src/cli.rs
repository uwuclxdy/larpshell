use colored::*;
use inquire::{Select, Text};
use std::env;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::common::{CTP_GREEN, CTP_RED, CTP_YELLOW};
use crate::config::AgentMode;
use crate::error::LarpshellError;

const SYMBOL_CHECK: &str = "\u{2713}";
const SYMBOL_ERROR: &str = "error:";
const SYMBOL_WARNING: &str = "warning:";

pub fn print_ok(message: &str) {
    eprintln!("{} {}", SYMBOL_CHECK.custom_color(CTP_GREEN), message);
}

pub fn print_ok_bold(message: &str) {
    eprintln!(
        "{} {}",
        SYMBOL_CHECK.custom_color(CTP_GREEN),
        message.bold()
    );
}

pub fn print_error(message: &str) {
    eprintln!("{} {}", SYMBOL_ERROR.custom_color(CTP_RED).bold(), message);
}

pub fn print_warning(message: &str) {
    eprintln!("{} {}", SYMBOL_WARNING.custom_color(CTP_YELLOW), message);
}

#[derive(Debug)]
pub struct CliArgs {
    pub command: Vec<String>,
    pub subcommand: Option<Subcommands>,
}

#[derive(Debug)]
pub enum Subcommands {
    Api,
    Uninstall,
    History {
        enable: Option<bool>,
    },
    Prompt {
        kind: PromptKind,
        action: PromptAction,
    },
    Explain {
        command: Vec<String>,
    },
    Agent {
        mode: Option<AgentMode>,
    },
}

#[derive(Debug, Clone)]
pub enum PromptKind {
    System,
    Explain,
    Agent,
    AgentSafe,
}

#[derive(Debug, Clone)]
pub enum PromptAction {
    Show,
    Edit,
    Reset,
}

pub fn parse_cli_args() -> Result<CliArgs, LarpshellError> {
    use clap::{Parser, Subcommand};

    #[derive(Parser)]
    #[command(name = "larpshell")]
    #[command(version)]
    #[command(disable_help_subcommand = true)]
    #[command(override_usage = "larpshell [REQUEST]\n       larpshell <COMMAND>")]
    struct Cli {
        #[arg(
            value_name = "REQUEST",
            help = "Natural language request to convert to a shell command"
        )]
        command: Vec<String>,

        #[command(subcommand)]
        subcommand: Option<Commands>,
    }

    #[derive(Subcommand)]
    enum Commands {
        /// Manage the API key for the active provider
        Api,
        /// Remove larpshell and its shell integration
        Uninstall,
        /// Enable or disable command history logging
        History {
            #[arg(value_enum)]
            toggle: Option<ClapHistoryToggle>,
        },
        /// View or edit system prompts
        Prompt {
            #[arg(value_enum, default_value_t = ClapPromptKind::System)]
            kind: ClapPromptKind,
            #[arg(value_enum, default_value_t = ClapPromptAction::Show)]
            action: ClapPromptAction,
        },
        /// Explain what a shell command does
        Explain { command: Vec<String> },
        /// Enable or disable agent mode
        Agent {
            #[arg(value_enum)]
            toggle: Option<ClapAgentToggle>,
        },
    }

    #[derive(clap::ValueEnum, Clone)]
    enum ClapHistoryToggle {
        On,
        Off,
    }

    #[derive(clap::ValueEnum, Clone)]
    enum ClapPromptKind {
        System,
        Explain,
        Agent,
        AgentSafe,
    }

    #[derive(clap::ValueEnum, Clone)]
    enum ClapPromptAction {
        Show,
        Edit,
        Reset,
    }

    #[derive(clap::ValueEnum, Clone)]
    enum ClapAgentToggle {
        Off,
        Safe,
        On,
    }

    let cli = Cli::parse();

    let subcommand = match cli.subcommand {
        Some(Commands::Api) => Some(Subcommands::Api),
        Some(Commands::Uninstall) => Some(Subcommands::Uninstall),
        Some(Commands::History { toggle }) => Some(Subcommands::History {
            enable: toggle.map(|t| matches!(t, ClapHistoryToggle::On)),
        }),
        Some(Commands::Prompt { kind, action }) => Some(Subcommands::Prompt {
            kind: match kind {
                ClapPromptKind::System => PromptKind::System,
                ClapPromptKind::Explain => PromptKind::Explain,
                ClapPromptKind::Agent => PromptKind::Agent,
                ClapPromptKind::AgentSafe => PromptKind::AgentSafe,
            },
            action: match action {
                ClapPromptAction::Show => PromptAction::Show,
                ClapPromptAction::Edit => PromptAction::Edit,
                ClapPromptAction::Reset => PromptAction::Reset,
            },
        }),
        Some(Commands::Explain { command }) => Some(Subcommands::Explain { command }),
        Some(Commands::Agent { toggle }) => Some(Subcommands::Agent {
            mode: toggle.map(|toggle| match toggle {
                ClapAgentToggle::Off => AgentMode::Off,
                ClapAgentToggle::Safe => AgentMode::Safe,
                ClapAgentToggle::On => AgentMode::On,
            }),
        }),
        None => None,
    };

    Ok(CliArgs {
        command: cli.command,
        subcommand,
    })
}

static CWD_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
pub(crate) static CWD_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn execute_shell_command_unlocked(command: &str) -> Result<(), LarpshellError> {
    let trimmed = command.trim();

    if trimmed.is_empty() {
        return Ok(());
    }

    // Run everything through sh -c so tilde expansion, env vars, pipes,
    // compound operators, and all other shell features work natively.
    // Append `pwd` to capture the shell's final working directory and
    // sync it back, so that `cd` (even inside compound commands) propagates
    // to the parent process.
    let seq = CWD_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let cwd_file = env::temp_dir().join(format!(".larpshell_cwd_{}_{}", std::process::id(), seq));
    let script = format!(
        "{trimmed}\n__larpshell_rc=$?\npwd > {cwd_path}\nexit $__larpshell_rc",
        cwd_path = cwd_file.display(),
    );

    Command::new("sh")
        .arg("-c")
        .arg(&script)
        .current_dir(env::current_dir()?)
        .status()?;

    // Sync the shell's final cwd back to the parent process.
    if let Ok(new_cwd) = std::fs::read_to_string(&cwd_file) {
        let new_cwd = new_cwd.trim();
        if !new_cwd.is_empty() {
            let _ = env::set_current_dir(new_cwd);
        }
    }
    let _ = std::fs::remove_file(&cwd_file);

    Ok(())
}

pub fn execute_shell_command(command: &str) -> Result<(), LarpshellError> {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    execute_shell_command_unlocked(command)
}

pub fn is_interactive_terminal() -> bool {
    if std::env::var("LARPSHELL_FORCE_INTERACTIVE").is_ok() {
        return true;
    }
    std::io::stdin().is_terminal()
}

pub fn prompt_select(
    prompt: &str,
    items: &[String],
    default: usize,
) -> Result<usize, LarpshellError> {
    let selection = Select::new(prompt, items.to_vec())
        .with_starting_cursor(default)
        .prompt()?;
    Ok(items
        .iter()
        .position(|x| x == &selection)
        .unwrap_or(default))
}

pub fn prompt_input(prompt: &str) -> Result<String, LarpshellError> {
    Ok(Text::new(prompt).prompt()?)
}

pub fn prompt_input_with_default(prompt: &str, default: &str) -> Result<String, LarpshellError> {
    Ok(Text::new(prompt).with_default(default).prompt()?)
}

pub fn home_dir() -> PathBuf {
    env::var("HOME")
        .ok()
        .or_else(|| env::var("USERPROFILE").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"))
}
