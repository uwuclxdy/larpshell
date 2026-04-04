use crate::common::{current_directory, os_name, shell_name, username};
use crate::config::{explain_prompt_path, save_explain_prompt, save_sys_prompt, sys_prompt_path};
use crate::error::LarpshellError;

pub const DEFAULT_PROMPT_TEMPLATE: &str =
    "You are a shell command translator. Convert the user's request into a shell command for {os}.

Environment context:
- Current dir: {cwd}
- Home dir: {home}
- User: {user}
- Shell: {shell}

Rules:
- Output ONLY the command, nothing else
- No explanations, no markdown, no backticks
- If unclear, make a reasonable assumption
- Prefer simple, common commands
- Use appropriate shell syntax and commands for this environment
- Consider the current directory context when generating paths
- Use ~ for home directory when appropriate

User request: {request}";

pub fn create_system_prompt(user_request: &str, template: Option<&str>) -> String {
    let cwd = current_directory();
    let os = os_name();
    let shell = shell_name();
    let home = dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~".to_string());
    let user = username();

    let tmpl = template.unwrap_or(DEFAULT_PROMPT_TEMPLATE);

    tmpl.replace("{os}", &os)
        .replace("{cwd}", &cwd)
        .replace("{home}", &home)
        .replace("{user}", &user)
        .replace("{shell}", &shell)
        .replace("{request}", user_request)
}

pub const DEFAULT_EXPLAIN_PROMPT: &str = include_str!("prompts/explain.md");

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

pub fn clean_response(response: &str) -> String {
    let mut cleaned = response.trim();

    if let Some(after_fence) = cleaned.strip_prefix("```") {
        cleaned = after_fence
            .trim_start_matches("shell")
            .trim_start_matches("bash")
            .trim_start_matches("zsh")
            .trim_start_matches("sh");
        cleaned = cleaned.trim_end_matches("```");
    }

    cleaned.trim().to_string()
}

pub fn clean_explanation(response: &str, command: &str) -> String {
    let trimmed = response.trim();
    let cmd_trimmed = command.trim();

    // Remove leading command if present
    if let Some(after) = trimmed.strip_prefix(cmd_trimmed) {
        if after.starts_with('\n') || after.starts_with(' ') || after.is_empty() {
            after.trim_start().to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        trimmed.to_string()
    }
}

pub fn create_prompts() -> Result<(), LarpshellError> {
    let sys_path = sys_prompt_path()?;
    if !sys_path.exists() {
        save_sys_prompt(DEFAULT_PROMPT_TEMPLATE)
            .map_err(|e| LarpshellError::ConfigError(e.to_string()))?;
    }

    let explain_path = explain_prompt_path()?;
    if !explain_path.exists() {
        save_explain_prompt(DEFAULT_EXPLAIN_PROMPT)
            .map_err(|e| LarpshellError::ConfigError(e.to_string()))?;
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
}
