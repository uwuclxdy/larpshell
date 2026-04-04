use crate::cli::{PromptAction, PromptKind};

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

pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "api",
        description: "configure API provider",
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
        name: "prompt",
        description: "manage system or explain prompts",
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
    Api,
    Uninstall,
    History { enable: bool },
    Prompt {
        kind: PromptKind,
        action: PromptAction,
    },
    Explain {
        args: Vec<String>,
    },
    Quit,
    Unknown(String),
}

pub fn parse(input: &str) -> SlashCmd {
    let mut parts = input.split_whitespace();
    match parts.next() {
        Some("/api") => SlashCmd::Api,
        Some("/uninstall") => SlashCmd::Uninstall,
        Some("/quit") => SlashCmd::Quit,
        Some("/history") => {
            let enable = matches!(parts.next(), Some("on"));
            SlashCmd::History { enable }
        }
        Some("/explain") => SlashCmd::Explain {
            args: parts.map(|s| s.to_string()).collect(),
        },
        Some("/prompt") => {
            let kind = match parts.next() {
                Some("explain") => PromptKind::Explain,
                _ => PromptKind::System,
            };
            let action = match parts.next() {
                Some("edit") => PromptAction::Edit,
                _ => PromptAction::Show,
            };
            SlashCmd::Prompt { kind, action }
        }
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
        assert_eq!(values, ["system", "explain"]);
    }

    #[test]
    fn arg_completions_prompt_partial_kind_filters() {
        let (start, candidates) = arg_completions("/prompt s").unwrap();
        assert_eq!(start, 8);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].value, "system");
    }

    #[test]
    fn arg_completions_prompt_kind_complete_returns_actions() {
        let (start, candidates) = arg_completions("/prompt system ").unwrap();
        assert_eq!(start, 15);
        let values: Vec<_> = candidates.iter().map(|c| c.value).collect();
        assert_eq!(values, ["show", "edit"]);
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
