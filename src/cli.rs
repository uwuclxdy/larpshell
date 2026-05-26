use colored::Colorize;
use inquire::{Select, Text};
use std::env;
use std::fs::OpenOptions;
use std::io::IsTerminal;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::common::{CTP_GREEN, CTP_RED, CTP_YELLOW};
use crate::config::AgentMode;
use crate::confirmation::style_message_markup;
use crate::error::LarpshellError;

const SYMBOL_CHECK: &str = "\u{2713}";
const SYMBOL_ERROR: &str = "error:";
const SYMBOL_WARNING: &str = "warning:";

pub fn print_ok(message: &str) {
    eprintln!(
        "{} {}",
        SYMBOL_CHECK.custom_color(CTP_GREEN),
        style_message_markup(message)
    );
}

pub fn print_ok_bold(message: &str) {
    eprintln!(
        "{} {}",
        SYMBOL_CHECK.custom_color(CTP_GREEN),
        style_message_markup(message).bold()
    );
}

pub fn print_error(message: &str) {
    eprintln!(
        "{} {}",
        SYMBOL_ERROR.custom_color(CTP_RED).bold(),
        style_message_markup(message)
    );
}

pub fn print_warning(message: &str) {
    eprintln!(
        "{} {}",
        SYMBOL_WARNING.custom_color(CTP_YELLOW),
        style_message_markup(message)
    );
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
    Verbose {
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

#[derive(clap::Parser)]
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

// clap requires real `ValueEnum` variants, so this vocabulary is spelled out
// rather than sourced from `vocab` directly. The `#[cfg(test)] clap_*_matches_vocab`
// guards at the bottom of this file assert each enum's `ValueEnum` value strings
// equal the matching `vocab` slice, turning any drift into a test failure.
#[derive(clap::Subcommand)]
enum Commands {
    /// Manage the API key for the active provider
    Api,
    /// Remove larpshell and its shell integration
    Uninstall,
    /// Enable or disable command history logging
    History {
        #[arg(value_enum)]
        toggle: Option<ClapBoolToggle>,
    },
    /// Enable or disable verbose agent tool output
    Verbose {
        #[arg(value_enum)]
        toggle: Option<ClapBoolToggle>,
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
enum ClapBoolToggle {
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

pub fn parse_cli_args() -> CliArgs {
    use clap::Parser;

    let cli = Cli::parse();

    let subcommand = match cli.subcommand {
        Some(Commands::Api) => Some(Subcommands::Api),
        Some(Commands::Uninstall) => Some(Subcommands::Uninstall),
        Some(Commands::History { toggle }) => Some(Subcommands::History {
            enable: toggle.map(|toggle| matches!(toggle, ClapBoolToggle::On)),
        }),
        Some(Commands::Verbose { toggle }) => Some(Subcommands::Verbose {
            enable: toggle.map(|toggle| matches!(toggle, ClapBoolToggle::On)),
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

    CliArgs {
        command: cli.command,
        subcommand,
    }
}

static CWD_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
pub static CWD_LOCK: Mutex<()> = Mutex::new(());

pub fn execute_shell_command_unlocked(command: &str) -> Result<(), LarpshellError> {
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
    // Mix in nanosecond timestamp for unpredictability — an attacker who
    // knows the PID cannot pre-create a symlink at the path because the
    // nanos component is not guessable without a timing oracle.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let cwd_file = env::temp_dir().join(format!(
        ".larpshell_cwd_{}_{}_{:08x}",
        std::process::id(),
        seq,
        nanos,
    ));
    // Create exclusively (O_EXCL) so a pre-planted symlink at this exact path
    // is never followed — the open fails rather than writing through it.
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&cwd_file)?;
    let script = format!(
        "__larpshell_cwd_file=$1\nreadonly __larpshell_cwd_file\n{trimmed}\n__larpshell_rc=$?\npwd > \"$__larpshell_cwd_file\"\nexit $__larpshell_rc"
    );

    let status = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .arg("larpshell")
        .arg(&cwd_file)
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

    if status.success() {
        Ok(())
    } else {
        Err(LarpshellError::IoError(std::io::Error::other(format!(
            "sh exited with status {status}"
        ))))
    }
}

pub fn execute_shell_command(command: &str) -> Result<(), LarpshellError> {
    let _guard = CWD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    let selected = Select::new(prompt, items.to_vec())
        .with_starting_cursor(default)
        .raw_prompt()?;
    Ok(selected.index)
}

pub fn prompt_input(prompt: &str, default: Option<&str>) -> Result<String, LarpshellError> {
    let mut text = Text::new(prompt);
    if let Some(d) = default {
        text = text.with_default(d);
    }
    Ok(text.prompt()?)
}

pub use crate::common::home_dir;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocab;
    use clap::{CommandFactory, ValueEnum};

    /// Value strings clap derives for a `ValueEnum`, in declaration order.
    fn enum_values<T: ValueEnum>() -> Vec<String> {
        T::value_variants()
            .iter()
            .map(|v| {
                v.to_possible_value()
                    .expect("variant has no possible value")
                    .get_name()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn clap_bool_toggle_matches_vocab() {
        assert_eq!(enum_values::<ClapBoolToggle>(), vocab::BOOL_TOGGLES);
    }

    #[test]
    fn clap_agent_toggle_matches_vocab() {
        assert_eq!(enum_values::<ClapAgentToggle>(), vocab::AGENT_TOGGLES);
    }

    #[test]
    fn clap_prompt_kind_matches_vocab() {
        assert_eq!(enum_values::<ClapPromptKind>(), vocab::PROMPT_KINDS);
    }

    #[test]
    fn clap_prompt_action_matches_vocab() {
        assert_eq!(enum_values::<ClapPromptAction>(), vocab::PROMPT_ACTIONS);
    }

    #[test]
    fn clap_subcommands_match_vocab() {
        let names: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|sub| sub.get_name().to_string())
            .collect();
        // Order differs from the bash first-word list, so compare as sets.
        for &want in vocab::SUBCOMMANDS {
            assert!(
                names.iter().any(|n| n == want),
                "clap subcommand {want:?} missing from CLI"
            );
        }
        assert_eq!(
            names.len(),
            vocab::SUBCOMMANDS.len(),
            "clap subcommand count {names:?} differs from vocab {:?}",
            vocab::SUBCOMMANDS
        );
    }
}
