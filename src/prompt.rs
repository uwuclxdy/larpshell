use crate::common::{current_directory, os_name, shell_name, username};
use crate::config::{
    agent_prompt_path, agent_safe_prompt_path, explain_prompt_path, save_agent_prompt,
    save_agent_safe_prompt, save_explain_prompt, save_sys_prompt, sys_prompt_path,
};
use crate::error::LarpshellError;

pub const DEFAULT_PROMPT_TEMPLATE: &str = include_str!("prompts/sys.md");

pub fn create_system_prompt(user_request: &str, template: Option<&str>) -> String {
    let cwd = current_directory();
    let os = os_name();
    let shell = shell_name();
    let home = dirs::home_dir().map_or_else(|| "~".to_string(), |p| p.display().to_string());
    let user = username();

    let tmpl = template.unwrap_or(DEFAULT_PROMPT_TEMPLATE);

    tmpl.replace("{os}", &os)
        .replace("{cwd}", cwd.as_str())
        .replace("{home}", home.as_str())
        .replace("{user}", user.as_str())
        .replace("{shell}", shell.as_str())
        .replace("{request}", user_request)
}

pub const DEFAULT_EXPLAIN_PROMPT: &str = include_str!("prompts/explain.md");

pub const DEFAULT_AGENT_SAFE_PROMPT: &str = include_str!("prompts/agent-safe.md");

pub const DEFAULT_AGENT_PROMPT: &str = include_str!("prompts/agent.md");

pub fn create_explain_prompt(command: &str, template: Option<&str>) -> String {
    let tmpl = template.unwrap_or(DEFAULT_EXPLAIN_PROMPT);
    tmpl.replace("{command}", command)
}

pub fn validate_sys_prompt(template: &str) -> bool {
    template.contains("{request}")
}

pub fn validate_explain_prompt(template: &str) -> bool {
    template.contains("{command}")
}

fn strip_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(after_fence) = trimmed.strip_prefix("```") else {
        return trimmed;
    };

    // Strip the optional language tag using `strip_prefix` (literal substring),
    // not `trim_start_matches` (character set).
    let rest = after_fence.strip_prefix("shell").unwrap_or(after_fence);
    let rest = rest.strip_prefix("bash").unwrap_or(rest);
    let rest = rest.strip_prefix("zsh").unwrap_or(rest);
    let rest = rest.strip_prefix("sh").unwrap_or(rest);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);

    rest.strip_suffix("```").unwrap_or(rest).trim()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLabeledResponse {
    pub message: Option<String>,
    pub command: Option<String>,
    pub has_labels: bool,
}

pub fn parse_labeled_response(text: &str) -> ParsedLabeledResponse {
    let stripped = strip_fence(text);
    let mut message_parts = Vec::new();
    let mut command_parts = Vec::new();
    let mut current_label: Option<&str> = None;
    let mut current_lines = Vec::new();
    let mut has_labels = false;

    let flush_current = |label: Option<&str>,
                         lines: &mut Vec<&str>,
                         message_parts: &mut Vec<String>,
                         command_parts: &mut Vec<String>| {
        if let Some(label) = label {
            let block = lines.join("\n").trim().to_string();
            if !block.is_empty() {
                match label {
                    "MESSAGE:" => message_parts.push(block),
                    "COMMAND:" => command_parts.push(block),
                    _ => {}
                }
            }
            lines.clear();
        }
    };

    for line in stripped.lines() {
        let trimmed = line.trim();
        let next_label = if let Some(rest) = trimmed.strip_prefix("MESSAGE:") {
            Some(("MESSAGE:", rest.trim()))
        } else {
            trimmed
                .strip_prefix("COMMAND:")
                .map(|rest| ("COMMAND:", rest.trim()))
        };

        if let Some((label, first_line)) = next_label {
            has_labels = true;
            flush_current(
                current_label,
                &mut current_lines,
                &mut message_parts,
                &mut command_parts,
            );
            current_label = Some(label);
            current_lines.push(first_line);
            continue;
        }

        if current_label.is_some() {
            current_lines.push(trimmed);
        }
    }

    flush_current(
        current_label,
        &mut current_lines,
        &mut message_parts,
        &mut command_parts,
    );

    let message = (!message_parts.is_empty()).then(|| message_parts.join("\n"));
    let command = (!command_parts.is_empty()).then(|| command_parts.join("\n"));

    ParsedLabeledResponse {
        message,
        command,
        has_labels,
    }
}

fn normalize_model_output(text: &str) -> String {
    let stripped = strip_fence(text);
    let parsed = parse_labeled_response(stripped);

    if let Some(command) = parsed.command {
        return command;
    }

    if let Some(message) = parsed.message {
        return message;
    }

    stripped.trim().to_string()
}

pub fn clean_response(response: &str) -> String {
    normalize_model_output(response)
}

pub fn clean_explanation(response: &str, command: &str) -> String {
    let trimmed = normalize_model_output(response);
    let cmd_trimmed = command.trim();

    if let Some(after) = trimmed.strip_prefix(cmd_trimmed)
        && (after.starts_with('\n') || after.starts_with(' ') || after.is_empty())
    {
        after.trim_start().to_string()
    } else {
        trimmed
    }
}

fn init_prompt_file(
    path_result: Result<std::path::PathBuf, LarpshellError>,
    default: &str,
    save: fn(&str) -> Result<(), LarpshellError>,
) -> Result<(), LarpshellError> {
    let path = path_result?;
    if !path.exists() {
        save(default)?;
    }
    Ok(())
}

pub fn create_prompts() -> Result<(), LarpshellError> {
    let prompts = [
        (
            sys_prompt_path(),
            DEFAULT_PROMPT_TEMPLATE,
            save_sys_prompt as fn(&str) -> Result<(), LarpshellError>,
        ),
        (
            explain_prompt_path(),
            DEFAULT_EXPLAIN_PROMPT,
            save_explain_prompt as fn(&str) -> Result<(), LarpshellError>,
        ),
        (
            agent_prompt_path(),
            DEFAULT_AGENT_PROMPT,
            save_agent_prompt as fn(&str) -> Result<(), LarpshellError>,
        ),
        (
            agent_safe_prompt_path(),
            DEFAULT_AGENT_SAFE_PROMPT,
            save_agent_safe_prompt as fn(&str) -> Result<(), LarpshellError>,
        ),
    ];

    for (path, default, save) in prompts {
        init_prompt_file(path, default, save)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_explain_prompt_has_command_placeholder() {
        assert!(DEFAULT_EXPLAIN_PROMPT.contains("{command}"));
    }

    #[test]
    fn validate_explain_prompt_accepts_valid_template() {
        assert!(validate_explain_prompt("explain: {command}"));
    }

    #[test]
    fn validate_explain_prompt_rejects_missing_placeholder() {
        assert!(!validate_explain_prompt("explain this command"));
    }

    #[test]
    fn validate_sys_prompt_accepts_valid_template() {
        assert!(validate_sys_prompt("do this: {request}"));
    }

    #[test]
    fn validate_sys_prompt_rejects_missing_placeholder() {
        assert!(!validate_sys_prompt("do something"));
    }

    #[test]
    fn default_agent_prompt_does_not_have_request_placeholder() {
        assert!(!DEFAULT_AGENT_PROMPT.contains("{request}"));
    }

    #[test]
    fn default_agent_safe_prompt_does_not_have_request_placeholder() {
        assert!(!DEFAULT_AGENT_SAFE_PROMPT.contains("{request}"));
    }

    #[test]
    fn create_explain_prompt_substitutes_command_in_default() {
        let result = create_explain_prompt("echo hi", None);
        assert!(result.contains("echo hi"));
        assert!(!result.contains("{command}"));
    }

    #[test]
    fn create_explain_prompt_substitutes_command_in_custom_template() {
        let result = create_explain_prompt("ls -la", Some("run: {command}"));
        assert_eq!(result, "run: ls -la");
    }

    #[test]
    fn create_explain_prompt_handles_multiword_command() {
        let result = create_explain_prompt("git log --oneline", Some("{command}"));
        assert_eq!(result, "git log --oneline");
    }

    #[test]
    fn clean_explanation_removes_leading_command() {
        let result = clean_explanation("free -h\nShows memory usage.", "free -h");
        assert_eq!(result, "Shows memory usage.");
    }

    #[test]
    fn clean_explanation_leaves_unrelated_response() {
        let result = clean_explanation("Shows memory usage.", "free -h");
        assert_eq!(result, "Shows memory usage.");
    }

    #[test]
    fn clean_explanation_handles_command_with_space() {
        let result = clean_explanation("free -h Shows memory usage.", "free -h");
        assert_eq!(result, "Shows memory usage.");
    }

    #[test]
    fn clean_response_extracts_prefixed_command_from_fenced_block() {
        let result = clean_response("```bash\nCOMMAND: ls -la\n```");
        assert_eq!(result, "ls -la");
    }

    #[test]
    fn clean_response_extracts_command_after_leading_prose() {
        let result = clean_response("Here is the command:\nCOMMAND: ls -la");
        assert_eq!(result, "ls -la");
    }

    #[test]
    fn clean_explanation_removes_repeated_command_inside_fenced_block() {
        let result = clean_explanation("```\nfree -h\nShows memory usage.\n```", "free -h");
        assert_eq!(result, "Shows memory usage.");
    }

    #[test]
    fn parse_labeled_response_preserves_multiline_message_block() {
        let parsed = parse_labeled_response("MESSAGE: first line\nsecond line\nthird line");
        assert_eq!(
            parsed,
            ParsedLabeledResponse {
                message: Some("first line\nsecond line\nthird line".to_string()),
                command: None,
                has_labels: true,
            }
        );
    }

    #[test]
    fn parse_labeled_response_extracts_message_and_command_blocks() {
        let parsed = parse_labeled_response(
            "MESSAGE: package needed by:\nfoo\nbar\nCOMMAND: sudo pacman -S webkit2gtk-4.1\necho done",
        );
        assert_eq!(
            parsed,
            ParsedLabeledResponse {
                message: Some("package needed by:\nfoo\nbar".to_string()),
                command: Some("sudo pacman -S webkit2gtk-4.1\necho done".to_string()),
                has_labels: true,
            }
        );
    }

    #[test]
    fn parse_labeled_response_trims_continuation_line_indent() {
        let parsed = parse_labeled_response("COMMAND: echo hello\n  echo world\n  pwd");
        assert_eq!(
            parsed,
            ParsedLabeledResponse {
                message: None,
                command: Some("echo hello\necho world\npwd".to_string()),
                has_labels: true,
            }
        );
    }

    #[test]
    fn parse_labeled_response_appends_repeated_message_blocks() {
        let parsed = parse_labeled_response("MESSAGE: first\nMESSAGE: second");
        assert_eq!(
            parsed,
            ParsedLabeledResponse {
                message: Some("first\nsecond".to_string()),
                command: None,
                has_labels: true,
            }
        );
    }

    #[test]
    fn clean_response_prefers_command_when_both_labels_exist() {
        let result = clean_response("MESSAGE: note\nCOMMAND: echo hello\necho world");
        assert_eq!(result, "echo hello\necho world");
    }
}
