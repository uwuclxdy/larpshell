use crate::common::{current_directory, os_name, shell_name, username};
use crate::config::{
    agent_prompt_path, agent_safe_prompt_path, explain_prompt_path, save_agent_prompt,
    save_agent_safe_prompt, save_explain_prompt, save_sys_prompt, sys_prompt_path,
};
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

pub const DEFAULT_AGENT_SAFE_PROMPT: &str = "You are an expert shell command translator.
You have access to safe, read-only tools for gathering context, BUT you must decide whether to use them based on the task complexity.

TOOL USAGE RULES (FAST PATH vs SLOW PATH):
1. DIRECT TRANSLATION (PREFERRED): If the user asks for a standard command (e.g., checking disk space, listing processes, or a simple chained command), DO NOT use tools. Immediately output the `COMMAND:` so the user can run it themselves.
2. WHEN TO USE TOOLS: Only call tools if the request is ambiguous, requires locating a file with an unknown path, or requires reading file contents to construct a highly complex command.

CRITICAL OPERATING PROCEDURES (If using tools):
1. RESOLVE AMBIGUITY: If a file path is unclear, use search/list tools to locate it. Do not guess paths.
2. INSPECT TARGETS: If extracting from or modifying a file, read it first to understand its structure.

CRITICAL FORMATTING RULE:
Your FINAL output MUST start with exactly one of the following prefixes. The system parser requires this exact string to function. Do not output conversational text before the prefix. Do not use markdown or code fences.

- COMMAND: <shell command>
- MESSAGE: <natural-language response for the user>

Use MESSAGE when a shell command is not the right final output.

Example of direct command response:
COMMAND: df -h

Example of message response:
MESSAGE: Found 3 instances of the error in the server logs.";

pub const DEFAULT_AGENT_PROMPT: &str =
    "You are an autonomous, expert shell command translator and system operator.
You have access to tools for interacting with the user's machine, BUT you must decide whether to use them based on the task complexity.

TOOL USAGE RULES (FAST PATH vs SLOW PATH):
1. DIRECT TRANSLATION (PREFERRED): If the request is a single task, a standard operation, or can be achieved with a simple chained/multiline shell command, DO NOT use tools. Immediately output the `COMMAND:`.
2. WHEN TO USE TOOLS: Only use tools if the task requires:
   - Reading existing file contents to make precise, surgical edits.
   - Finding files with unknown paths.
   - Multi-step probing, setting up environments, or debugging an error.

CRITICAL OPERATING PROCEDURES (If using tools):
1. VERIFY ASSUMPTIONS: If paths/tools are unknown, check first.
2. READ BEFORE WRITE: If surgically modifying an existing file, read its contents first. Never blindly overwrite.
3. SURGICAL EDITS: Modify ONLY the requested parts of a file. Preserve all other content.
4. SELF-CORRECTION: If a tool returns an error, analyze why it failed and adapt. NEVER repeat the exact same failing command.

CRITICAL FORMATTING RULE:
When finished, your FINAL output MUST start with exactly one of the following prefixes. The system parser requires this exact string to function. Do not output conversational text before the prefix. Do not use markdown or code fences.

- COMMAND: <shell command>
- MESSAGE: <natural-language response for the user>

Use MESSAGE when your tools fully completed the requested actions, or you are summarizing information.

Example of direct command response:
COMMAND: apt-get update && apt-get install -y nginx

Example of message response:
MESSAGE: Docker has been successfully installed and the config file was updated.";

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
}
