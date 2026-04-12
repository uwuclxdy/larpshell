use serde::{Deserialize, Serialize};

use super::AgentMode;
use std::fs;
use std::path::Path;

use crate::error::LarpshellError;
use crate::prompt::DEFAULT_EXPLAIN_PROMPT;
const OLD_EXPLAIN_PROMPT_V1: &str = include_str!("../prompts/old_explain_v1.md");
const OLD_EXPLAIN_PROMPT_V2: &str = include_str!("../prompts/old_explain_v2.md");

use super::{ActiveProvider, Config, MultiProviderConfig, explain_prompt_path};

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

type MigrationResult = Result<String, LarpshellError>;

trait Migrator {
    fn can_migrate(&self, content: &str) -> bool;
    fn migrate(&self, content: &str) -> MigrationResult;
}

struct ConfigMigrator;

impl Migrator for ConfigMigrator {
    fn can_migrate(&self, content: &str) -> bool {
        content.contains("[provider]") && content.contains("type = ")
    }

    fn migrate(&self, content: &str) -> MigrationResult {
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
            agent: AgentMode::Safe,
        };

        let new_content = toml::to_string_pretty(&new_config)?;
        Ok(new_content)
    }
}

fn migrators() -> Vec<Box<dyn Migrator>> {
    vec![Box::new(ConfigMigrator)]
}

struct ExplainPromptMigrator;

impl Migrator for ExplainPromptMigrator {
    fn can_migrate(&self, content: &str) -> bool {
        matches!(content, OLD_EXPLAIN_PROMPT_V1 | OLD_EXPLAIN_PROMPT_V2)
    }

    fn migrate(&self, _content: &str) -> MigrationResult {
        Ok(DEFAULT_EXPLAIN_PROMPT.to_string())
    }
}

fn explain_prompt_migrators() -> Vec<Box<dyn Migrator>> {
    vec![Box::new(ExplainPromptMigrator)]
}

pub fn migrate_explain_prompt() -> Result<bool, LarpshellError> {
    let explain_prompt_path = explain_prompt_path()?;

    if !explain_prompt_path.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(&explain_prompt_path)?;
    let migrators = explain_prompt_migrators();

    for migrator in migrators {
        if migrator.can_migrate(&content) {
            let new_content = migrator.migrate(&content)?;
            fs::write(&explain_prompt_path, new_content)?;
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn migrate_config(config_path: &Path) -> Result<bool, LarpshellError> {
    let content = fs::read_to_string(config_path)?;
    let migrators = migrators();

    for migrator in migrators {
        if migrator.can_migrate(&content) {
            let new_content = migrator.migrate(&content)?;
            fs::write(config_path, new_content)?;
            return Ok(true);
        }
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
