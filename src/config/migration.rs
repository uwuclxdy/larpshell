use serde::{Deserialize, Serialize};

use super::AgentMode;
use std::fs;
use std::path::Path;

use crate::error::LarpshellError;
use crate::prompt::DEFAULT_EXPLAIN_PROMPT;
const OLD_EXPLAIN_PROMPT_V1: &str = include_str!("../prompts/old/explain_v1.md");
const OLD_EXPLAIN_PROMPT_V2: &str = include_str!("../prompts/old/explain_v2.md");

use super::{ActiveProvider, Config, MultiProviderConfig, atomic_write, explain_prompt_path};

#[derive(Debug, Serialize, Deserialize)]
struct V1ProviderSection {
    #[serde(rename = "type")]
    provider_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct V1Config {
    provider: V1ProviderSection,
    #[serde(default)]
    providers: MultiProviderConfig,
}

fn can_migrate_config(content: &str) -> bool {
    content.contains("[provider]") && content.contains("type = ")
}

fn migrate_config_content(content: &str) -> Result<String, LarpshellError> {
    let old_config: V1Config = toml::from_str(content)?;

    let active_provider = match old_config.provider.provider_type.as_str() {
        "gemini" => ActiveProvider::Gemini,
        "ollama" => ActiveProvider::Ollama,
        "openai" => ActiveProvider::OpenAI,
        other => {
            return Err(LarpshellError::ConfigError(format!(
                "unknown provider type in config: {other}"
            )));
        }
    };

    let new_config = Config {
        active_provider,
        providers: old_config.providers,
        agent: AgentMode::Off,
        verbose_tool_output: true,
    };

    Ok(toml::to_string_pretty(&new_config)?)
}

fn can_migrate_explain_prompt(content: &str) -> bool {
    matches!(content, OLD_EXPLAIN_PROMPT_V1 | OLD_EXPLAIN_PROMPT_V2)
}

pub fn migrate_explain_prompt() -> Result<bool, LarpshellError> {
    let explain_prompt_path = explain_prompt_path()?;

    if !explain_prompt_path.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(&explain_prompt_path)?;

    if can_migrate_explain_prompt(&content) {
        atomic_write(&explain_prompt_path, DEFAULT_EXPLAIN_PROMPT)?;
        return Ok(true);
    }

    Ok(false)
}

pub fn migrate_config(config_path: &Path) -> Result<bool, LarpshellError> {
    let content = fs::read_to_string(config_path)?;

    if can_migrate_config(&content) {
        let new_content = migrate_config_content(&content)?;
        atomic_write(config_path, &new_content)?;
        return Ok(true);
    }

    Ok(false)
}

/// Copies `~/.config/nlsh-rs/` into `~/.config/larpshell/` then deletes the
/// old directory.  Skips the copy if larpshell already has a config, but still
/// removes the old dir.  Returns `Ok(false)` immediately when there is nothing
/// to do (no `~/.config/nlsh-rs/` present).
pub fn migrate_from_nlsh_rs() -> Result<bool, LarpshellError> {
    let config_base = dirs::config_dir()
        .ok_or_else(|| LarpshellError::ConfigError("failed to get config directory".to_string()))?;
    let old_dir = config_base.join("nlsh-rs");

    if !old_dir.exists() {
        return Ok(false);
    }

    let new_dir = config_base.join("larpshell");
    if !new_dir.join("config.toml").exists() {
        fs::create_dir_all(&new_dir)?;
        for entry in fs::read_dir(&old_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::copy(entry.path(), new_dir.join(entry.file_name()))?;
            }
        }
    }

    let _ = fs::remove_dir_all(&old_dir);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn v1_config_migrates_agent_to_off() {
        let v1 = r#"
[provider]
type = "ollama"

[ollama]
base_url = "http://localhost:11434"
model = "llama3"
"#;
        let path = std::env::temp_dir().join("larpshell_migration_test.toml");
        fs::write(&path, v1).unwrap();

        let migrated = migrate_config(&path).unwrap();
        assert!(migrated, "expected migration to run");

        let content = fs::read_to_string(&path).unwrap();
        let config: Config = toml::from_str(&content).unwrap();
        assert_eq!(
            config.agent,
            AgentMode::Off,
            "migrated config must not opt users into agent mode"
        );

        let _ = fs::remove_file(&path);
    }
}
