use crate::cli::{PromptAction, PromptKind};
use crate::config::AgentMode;

pub struct SlashCommand {
    pub name: &'static str,
    #[allow(dead_code)] // used in Task 4: preview drawing
    pub description: &'static str,
}

pub struct ArgChoice {
    pub value: &'static str,
    pub description: &'static str,
}

static PROMPT_KINDS: &[ArgChoice] = &[
    ArgChoice {
        value: "system",
        description: "system prompt",
    },
    ArgChoice {
        value: "explain",
        description: "explain prompt",
    },
    ArgChoice {
        value: "agent",
        description: "agent (unrestricted) prompt",
    },
    ArgChoice {
        value: "agent-safe",
        description: "agent (safe/restricted) prompt",
    },
];

static PROMPT_ACTIONS: &[ArgChoice] = &[
    ArgChoice {
        value: "show",
        description: "print current value",
    },
    ArgChoice {
        value: "edit",
        description: "open in editor",
    },
    ArgChoice {
        value: "reset",
        description: "restore default and back up current",
    },
];

static HISTORY_TOGGLES: &[ArgChoice] = &[
    ArgChoice {
        value: "on",
        description: "save prompts across sessions",
    },
    ArgChoice {
        value: "off",
        description: "stop saving history",
    },
];

static TOOL_OUTPUT_TOGGLES: &[ArgChoice] = &[
    ArgChoice {
        value: "on",
        description: "show expanded agent tool output",
    },
    ArgChoice {
        value: "off",
        description: "show tool output summaries only",
    },
];

static AGENT_TOGGLES: &[ArgChoice] = &[
    ArgChoice {
        value: "off",
        description: "disable agent mode",
    },
    ArgChoice {
        value: "safe",
        description: "enable restricted agent tool mode",
    },
    ArgChoice {
        value: "on",
        description: "enable unrestricted agent tool mode",
    },
];

pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "api",
        description: "configure API provider",
    },
    SlashCommand {
        name: "agent",
        description: "enable or disable agent mode",
    },
    SlashCommand {
        name: "explain",
        description: "explain a shell command",
    },
    SlashCommand {
        name: "history",
        description: "enable or disable prompt history",
    },
    SlashCommand {
        name: "verbose",
        description: "enable or disable verbose agent tool output",
    },
    SlashCommand {
        name: "prompt",
        description: "manage system, explain, or agent prompts",
    },
    SlashCommand {
        name: "help",
        description: "list available slash commands",
    },
    SlashCommand {
        name: "quit",
        description: "exit interactive mode",
    },
    SlashCommand {
        name: "uninstall",
        description: "uninstall larpshell",
    },
];

/// Returns `(replacement_start, candidates)` for argument completion at the
/// current cursor position. `replacement_start` is the byte offset in `line`
/// where the replacement should begin.
pub fn arg_completions(line: &str) -> Option<(usize, Vec<&'static ArgChoice>)> {
    if !line.starts_with('/') {
        return None;
    }
    let space_pos = line.find(' ')?;
    let cmd = &line[..space_pos];
    let after_cmd = &line[space_pos + 1..];

    let (prior_args, partial) = if line.ends_with(' ') {
        (after_cmd.split_whitespace().collect::<Vec<_>>(), "")
    } else {
        let mut parts: Vec<&str> = after_cmd.split_whitespace().collect();
        let partial = parts.pop().unwrap_or("");
        (parts, partial)
    };

    let pool: &[ArgChoice] = match cmd {
        "/history" => match prior_args.len() {
            0 => HISTORY_TOGGLES,
            _ => return None,
        },
        "/verbose" => match prior_args.len() {
            0 => TOOL_OUTPUT_TOGGLES,
            _ => return None,
        },
        "/agent" => match prior_args.len() {
            0 => AGENT_TOGGLES,
            _ => return None,
        },
        "/prompt" => match prior_args.len() {
            0 => PROMPT_KINDS,
            1 => PROMPT_ACTIONS,
            _ => return None,
        },
        _ => return None,
    };

    let candidates: Vec<&'static ArgChoice> = pool
        .iter()
        .filter(|c| c.value.starts_with(partial))
        .collect();

    if candidates.is_empty() {
        return None;
    }

    Some((line.len() - partial.len(), candidates))
}

/// Returns commands whose `/name` is a prefix of `typed`, or `typed` is a prefix of `/name`.
/// Returns nothing if `typed` is empty or does not start with `/`.
pub fn filter(typed: &str) -> Vec<&'static SlashCommand> {
    if !typed.starts_with('/') {
        return vec![];
    }
    // Extract just the command part (first word, strip leading '/')
    let typed_cmd = typed[1..].split_whitespace().next().unwrap_or("");
    COMMANDS
        .iter()
        .filter(|cmd| cmd.name.starts_with(typed_cmd) || typed_cmd.starts_with(cmd.name))
        .collect()
}

#[derive(Debug)]
pub enum SlashCmd {
    Agent {
        mode: Option<AgentMode>,
    },
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
        args: Vec<String>,
    },
    Help,
    Quit,
    Unknown(String),
    InvalidArgs {
        command: &'static str,
        expected: &'static str,
    },
}

fn parse_agent_mode_strict(
    arg: Option<&str>,
) -> Result<Option<AgentMode>, (&'static str, &'static str)> {
    match arg {
        None => Ok(None),
        Some("off") => Ok(Some(AgentMode::Off)),
        Some("safe") => Ok(Some(AgentMode::Safe)),
        Some("on") => Ok(Some(AgentMode::On)),
        Some(_) => Err(("agent", "off, safe, or on")),
    }
}

fn parse_bool_toggle_strict(
    command: &'static str,
    arg: Option<&str>,
) -> Result<Option<bool>, (&'static str, &'static str)> {
    match arg {
        None => Ok(None),
        Some("on") => Ok(Some(true)),
        Some("off") => Ok(Some(false)),
        Some(_) => Err((command, "on or off")),
    }
}

fn parse_prompt_kind(arg: Option<&str>) -> PromptKind {
    match arg {
        Some("explain") => PromptKind::Explain,
        Some("agent-safe") => PromptKind::AgentSafe,
        Some("agent") => PromptKind::Agent,
        _ => PromptKind::System,
    }
}

fn parse_prompt_action(arg: Option<&str>) -> PromptAction {
    match arg {
        Some("edit") => PromptAction::Edit,
        Some("reset") => PromptAction::Reset,
        _ => PromptAction::Show,
    }
}

pub fn parse(input: &str) -> SlashCmd {
    let mut parts = input.split_whitespace();
    match parts.next() {
        Some("/agent") => match parse_agent_mode_strict(parts.next()) {
            Ok(mode) => SlashCmd::Agent { mode },
            Err((cmd, expected)) => SlashCmd::InvalidArgs {
                command: cmd,
                expected,
            },
        },
        Some("/api") => SlashCmd::Api,
        Some("/uninstall") => SlashCmd::Uninstall,
        Some("/help") => SlashCmd::Help,
        Some("/quit") => SlashCmd::Quit,
        Some("/history") => match parse_bool_toggle_strict("history", parts.next()) {
            Ok(enable) => SlashCmd::History { enable },
            Err((cmd, expected)) => SlashCmd::InvalidArgs {
                command: cmd,
                expected,
            },
        },
        Some("/verbose") => match parse_bool_toggle_strict("verbose", parts.next()) {
            Ok(enable) => SlashCmd::Verbose { enable },
            Err((cmd, expected)) => SlashCmd::InvalidArgs {
                command: cmd,
                expected,
            },
        },
        Some("/explain") => SlashCmd::Explain {
            args: parts.map(std::string::ToString::to_string).collect(),
        },
        Some("/prompt") => SlashCmd::Prompt {
            kind: parse_prompt_kind(parts.next()),
            action: parse_prompt_action(parts.next()),
        },
        _ => SlashCmd::Unknown(input.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_empty_input_returns_nothing() {
        assert!(filter("").is_empty());
    }

    #[test]
    fn filter_slash_returns_all_commands() {
        assert_eq!(filter("/").len(), COMMANDS.len());
    }

    #[test]
    fn filter_prefix_matches_command() {
        let results = filter("/p");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "prompt");
    }

    #[test]
    fn filter_full_name_matches_self() {
        let results = filter("/quit");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "quit");
    }

    #[test]
    fn filter_with_args_matches_base_command() {
        let results = filter("/prompt system edit");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "prompt");
    }

    #[test]
    fn filter_unknown_returns_nothing() {
        assert!(filter("/zzz").is_empty());
    }

    #[test]
    fn parse_quit() {
        assert!(matches!(parse("/quit"), SlashCmd::Quit));
    }

    #[test]
    fn parse_api() {
        assert!(matches!(parse("/api"), SlashCmd::Api));
    }

    #[test]
    fn parse_uninstall() {
        assert!(matches!(parse("/uninstall"), SlashCmd::Uninstall));
    }

    #[test]
    fn parse_explain_with_args() {
        let cmd = parse("/explain ls -la");
        match cmd {
            SlashCmd::Explain { args } => assert_eq!(args, vec!["ls", "-la"]),
            _ => panic!("expected Explain"),
        }
    }

    #[test]
    fn parse_prompt_defaults_to_system_show() {
        match parse("/prompt") {
            SlashCmd::Prompt { kind, action } => {
                assert!(matches!(kind, crate::cli::PromptKind::System));
                assert!(matches!(action, crate::cli::PromptAction::Show));
            }
            _ => panic!("expected Prompt"),
        }
    }

    #[test]
    fn parse_prompt_explain_edit() {
        match parse("/prompt explain edit") {
            SlashCmd::Prompt { kind, action } => {
                assert!(matches!(kind, crate::cli::PromptKind::Explain));
                assert!(matches!(action, crate::cli::PromptAction::Edit));
            }
            _ => panic!("expected Prompt"),
        }
    }

    #[test]
    fn parse_agent_on() {
        match parse("/agent on") {
            SlashCmd::Agent { mode } => assert_eq!(mode, Some(AgentMode::On)),
            _ => panic!("expected Agent"),
        }
    }

    #[test]
    fn parse_agent_safe() {
        match parse("/agent safe") {
            SlashCmd::Agent { mode } => assert_eq!(mode, Some(AgentMode::Safe)),
            _ => panic!("expected Agent"),
        }
    }

    #[test]
    fn parse_agent_off() {
        match parse("/agent off") {
            SlashCmd::Agent { mode } => assert_eq!(mode, Some(AgentMode::Off)),
            _ => panic!("expected Agent"),
        }
    }

    #[test]
    fn parse_agent_no_arg_shows_status() {
        match parse("/agent") {
            SlashCmd::Agent { mode } => assert_eq!(mode, None),
            _ => panic!("expected Agent"),
        }
    }

    #[test]
    fn parse_tool_output_off() {
        match parse("/verbose off") {
            SlashCmd::Verbose { enable } => assert_eq!(enable, Some(false)),
            _ => panic!("expected Verbose"),
        }
    }

    #[test]
    fn parse_tool_output_no_arg_shows_status() {
        match parse("/verbose") {
            SlashCmd::Verbose { enable } => assert_eq!(enable, None),
            _ => panic!("expected Verbose"),
        }
    }

    #[test]
    fn arg_completions_agent_returns_toggles() {
        let (start, candidates) = arg_completions("/agent ").unwrap();
        assert_eq!(start, 7);
        let values: Vec<_> = candidates.iter().map(|candidate| candidate.value).collect();
        assert_eq!(values, ["off", "safe", "on"]);
    }

    #[test]
    fn filter_includes_agent() {
        let results = filter("/a");
        let names: Vec<_> = results.iter().map(|command| command.name).collect();
        assert!(names.contains(&"agent"), "filter /a should include agent");
    }

    #[test]
    fn arg_completions_tool_output_returns_toggles() {
        let (start, candidates) = arg_completions("/verbose ").unwrap();
        assert_eq!(start, 9);
        let values: Vec<_> = candidates.iter().map(|candidate| candidate.value).collect();
        assert_eq!(values, ["on", "off"]);
    }

    #[test]
    fn parse_unknown_command() {
        match parse("/foo") {
            SlashCmd::Unknown(s) => assert_eq!(s, "/foo"),
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn arg_completions_non_slash_returns_none() {
        assert!(arg_completions("prompt").is_none());
    }

    #[test]
    fn arg_completions_no_space_returns_none() {
        assert!(arg_completions("/prompt").is_none());
    }

    #[test]
    fn arg_completions_unknown_command_returns_none() {
        assert!(arg_completions("/api ").is_none());
    }

    #[test]
    fn arg_completions_prompt_space_returns_kinds() {
        let (start, candidates) = arg_completions("/prompt ").unwrap();
        assert_eq!(start, 8);
        let values: Vec<_> = candidates.iter().map(|c| c.value).collect();
        assert_eq!(values, ["system", "explain", "agent", "agent-safe"]);
    }

    #[test]
    fn arg_completions_prompt_partial_kind_filters() {
        let (start, candidates) = arg_completions("/prompt s").unwrap();
        assert_eq!(start, 8);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].value, "system");
    }

    #[test]
    fn arg_completions_prompt_agent_prefix_filters() {
        let (start, candidates) = arg_completions("/prompt agent").unwrap();
        assert_eq!(start, 8);
        let values: Vec<_> = candidates.iter().map(|c| c.value).collect();
        assert_eq!(values, ["agent", "agent-safe"]);
    }

    #[test]
    fn parse_prompt_agent_edit() {
        match parse("/prompt agent edit") {
            SlashCmd::Prompt { kind, action } => {
                assert!(matches!(kind, PromptKind::Agent));
                assert!(matches!(action, PromptAction::Edit));
            }
            _ => panic!("expected Prompt"),
        }
    }

    #[test]
    fn parse_prompt_agent_safe_show() {
        match parse("/prompt agent-safe") {
            SlashCmd::Prompt { kind, action } => {
                assert!(matches!(kind, PromptKind::AgentSafe));
                assert!(matches!(action, PromptAction::Show));
            }
            _ => panic!("expected Prompt"),
        }
    }

    #[test]
    fn arg_completions_prompt_kind_complete_returns_actions() {
        let (start, candidates) = arg_completions("/prompt system ").unwrap();
        assert_eq!(start, 15);
        let values: Vec<_> = candidates.iter().map(|c| c.value).collect();
        assert_eq!(values, ["show", "edit", "reset"]);
    }

    #[test]
    fn arg_completions_prompt_partial_action_filters() {
        let (start, candidates) = arg_completions("/prompt system e").unwrap();
        assert_eq!(start, 15);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].value, "edit");
    }

    #[test]
    fn arg_completions_no_match_returns_none() {
        assert!(arg_completions("/prompt zzz").is_none());
    }

    #[test]
    fn arg_completions_too_many_args_returns_none() {
        assert!(arg_completions("/prompt system edit ").is_none());
    }
}
