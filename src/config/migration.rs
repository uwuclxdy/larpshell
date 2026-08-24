use serde::{Deserialize, Serialize};

use super::AgentMode;
use std::fs;
use std::path::Path;

use crate::error::LarpshellError;
use crate::prompt::DEFAULT_EXPLAIN_PROMPT;
const OLD_EXPLAIN_PROMPT_V1: &str = include_str!("../prompts/old/explain_v1.md");
const OLD_EXPLAIN_PROMPT_V2: &str = include_str!("../prompts/old/explain_v2.md");

use super::{
    ActiveProvider, Config, GeminiConfig, OllamaConfig, OpenAIConfig, OpenRouterConfig, ProviderProfile, ProviderSpecificConfig,
    atomic_write, config_base_dir, default_verbose_tool_output, deserialize_agent_mode, ensure_config_dir, explain_prompt_path,
};

/// Pre-profile `[providers.<kind>]` tables, one slot per provider type.
/// Only the migration parsers read this shape anymore.
#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct MultiProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gemini: Option<GeminiConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama: Option<OllamaConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openrouter: Option<OpenRouterConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai: Option<OpenAIConfig>,
}

impl MultiProviderConfig {
    fn into_profiles(self) -> Vec<ProviderProfile> {
        let mut profiles = Vec::new();
        if let Some(config) = self.gemini {
            profiles.push(ProviderProfile { name: "gemini".to_string(), config: ProviderSpecificConfig::Gemini(config) });
        }
        if let Some(config) = self.ollama {
            profiles.push(ProviderProfile { name: "ollama".to_string(), config: ProviderSpecificConfig::Ollama(config) });
        }
        if let Some(config) = self.openrouter {
            profiles.push(ProviderProfile { name: "openrouter".to_string(), config: ProviderSpecificConfig::OpenRouter(config) });
        }
        if let Some(config) = self.openai {
            profiles.push(ProviderProfile { name: "openai".to_string(), config: ProviderSpecificConfig::OpenAI(config) });
        }
        profiles
    }
}

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
            return Err(LarpshellError::ConfigError(format!("unknown provider type in config: {other}")));
        }
    };

    let new_config = Config {
        active_provider: active_provider.as_str().to_string(),
        providers: old_config.providers.into_profiles(),
        agent: AgentMode::Off,
        verbose_tool_output: true,
    };

    Ok(toml::to_string_pretty(&new_config)?)
}

/// The previous larpshell shape: bare `provider = "<kind>"` plus one
/// `[providers.<kind>]` table per configured type.
#[derive(Debug, Deserialize)]
struct OldLarpshellConfig {
    provider: ActiveProvider,
    #[serde(default)]
    providers: MultiProviderConfig,
    #[serde(default, deserialize_with = "deserialize_agent_mode")]
    agent: AgentMode,
    #[serde(default = "default_verbose_tool_output")]
    verbose_tool_output: bool,
}

/// Distinguishes the old per-type tables from the new `[[providers]]` array:
/// only the old shape contains a dotted `[providers.`.
fn can_migrate_old_larpshell(content: &str) -> bool {
    content.contains("[providers.")
}

fn migrate_old_larpshell_content(content: &str) -> Result<String, LarpshellError> {
    let old_config: OldLarpshellConfig = toml::from_str(content)?;

    let new_config = Config {
        active_provider: old_config.provider.as_str().to_string(),
        providers: old_config.providers.into_profiles(),
        agent: old_config.agent,
        verbose_tool_output: old_config.verbose_tool_output,
    };

    Ok(toml::to_string_pretty(&new_config)?)
}

fn can_migrate_explain_prompt(content: &str) -> bool {
    matches!(content, OLD_EXPLAIN_PROMPT_V1 | OLD_EXPLAIN_PROMPT_V2)
}

/// Migrates the explain prompt if it matches a known old version.
/// Returns `Some(new_content)` when migration ran (caller need not re-read),
/// `None` when no migration was needed, or an error.
pub fn migrate_explain_prompt() -> Result<Option<String>, LarpshellError> {
    let explain_prompt_path = explain_prompt_path()?;

    if !explain_prompt_path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&explain_prompt_path)?;

    if can_migrate_explain_prompt(&content) {
        atomic_write(&explain_prompt_path, DEFAULT_EXPLAIN_PROMPT)?;
        return Ok(Some(DEFAULT_EXPLAIN_PROMPT.to_string()));
    }

    Ok(None)
}

pub fn migrate_config(config_path: &Path) -> Result<bool, LarpshellError> {
    let content = fs::read_to_string(config_path)?;

    if can_migrate_config(&content) {
        let new_content = migrate_config_content(&content)?;
        atomic_write(config_path, &new_content)?;
        return Ok(true);
    }

    if can_migrate_old_larpshell(&content) {
        let new_content = migrate_old_larpshell_content(&content)?;
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
    // nlsh-rs used the platform's native config dir (Library/Application Support on macOS)
    let old_base = dirs::config_dir().ok_or_else(|| LarpshellError::ConfigError("failed to get config directory".to_string()))?;
    let old_dir = old_base.join("nlsh-rs");

    if !old_dir.exists() {
        return Ok(false);
    }

    // larpshell now uses XDG config semantics on all unix
    let new_dir =
        config_base_dir().ok_or_else(|| LarpshellError::ConfigError("failed to get config directory".to_string()))?.join("larpshell");
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

/// Copies all files from `old_dir` to `new_dir` when the new dir lacks a
/// `config.toml`, then removes the old dir. When both dirs have `config.toml`,
/// the new dir wins (no clobber). No-op when `old_dir` doesn't exist.
fn relocate_config_dir(old_dir: &Path, new_dir: &Path) -> Result<bool, LarpshellError> {
    if !old_dir.exists() {
        return Ok(false);
    }

    if !new_dir.join("config.toml").exists() {
        fs::create_dir_all(new_dir)?;
        for entry in fs::read_dir(old_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::copy(entry.path(), new_dir.join(entry.file_name()))?;
            }
        }
    }

    let _ = fs::remove_dir_all(old_dir);
    Ok(true)
}

/// Moves config from the macOS Application Support location
/// (`~/Library/Application Support/larpshell`) to the XDG config directory.
/// On Linux the old path never exists, so it is always a no-op.
pub fn migrate_macos_config_dir() -> Result<bool, LarpshellError> {
    let home = dirs::home_dir().ok_or_else(|| LarpshellError::ConfigError("failed to get home directory".to_string()))?;
    let old_dir = home.join("Library/Application Support/larpshell");
    let new_dir = ensure_config_dir()?;
    relocate_config_dir(&old_dir, &new_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

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
        assert_eq!(config.agent, AgentMode::Off, "migrated config must not opt users into agent mode");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn old_larpshell_config_migrates_per_type_tables_to_profiles() {
        let old = r#"
provider = "openrouter"
agent = "safe"
verbose_tool_output = false

[providers.ollama]
base_url = "http://localhost:11434"
model = "llama3"

[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
api_key = "sk-or-test"
model = "openrouter/auto"
"#;
        let path = std::env::temp_dir().join("larpshell_old_config_migration_test.toml");
        fs::write(&path, old).unwrap();

        let migrated = migrate_config(&path).unwrap();
        assert!(migrated, "expected migration to run");

        let content = fs::read_to_string(&path).unwrap();
        let config: Config = toml::from_str(&content).unwrap();
        assert_eq!(config.active_provider, "openrouter");
        assert_eq!(config.providers.len(), 2);
        assert!(config.providers.iter().any(|p| p.name == "ollama" && matches!(p.config, ProviderSpecificConfig::Ollama(_))));
        assert!(config.providers.iter().any(|p| p.name == "openrouter" && matches!(p.config, ProviderSpecificConfig::OpenRouter(_))));
        assert_eq!(config.agent, AgentMode::Safe, "agent mode must survive the migration");
        assert!(!config.verbose_tool_output, "verbose_tool_output must survive the migration");
        assert!(config.provider_config().is_ok(), "active provider must resolve after migration");

        let _ = fs::remove_file(&path);
    }

    fn migrate_tmp(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("larpshell_migrate_{test_name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn relocate_config_dir_moves_files_when_new_is_empty() {
        let tmp = migrate_tmp("empty");
        let old_dir = tmp.join("old");
        let new_dir = tmp.join("new");

        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("config.toml"), "provider = \"ollama\"").unwrap();

        let migrated = relocate_config_dir(&old_dir, &new_dir).unwrap();
        assert!(migrated, "expected migration to run");
        assert!(new_dir.join("config.toml").exists(), "config should be in new dir");
        assert!(!old_dir.exists(), "old dir should be removed");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn relocate_config_dir_preserves_new_when_both_exist() {
        let tmp = migrate_tmp("clobber");
        let old_dir = tmp.join("old");
        let new_dir = tmp.join("new");

        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("config.toml"), "old").unwrap();
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("config.toml"), "new").unwrap();

        let migrated = relocate_config_dir(&old_dir, &new_dir).unwrap();
        assert!(migrated, "old dir existed, migration ran");
        assert_eq!(fs::read_to_string(new_dir.join("config.toml")).unwrap(), "new", "new dir must not be clobbered");
        assert!(!old_dir.exists(), "old dir should still be cleaned up");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn relocate_config_dir_noop_when_old_missing() {
        let tmp = migrate_tmp("noop");
        let old_dir = tmp.join("nonexistent");
        let new_dir = tmp.join("new");

        let migrated = relocate_config_dir(&old_dir, &new_dir).unwrap();
        assert!(!migrated, "expected no migration");

        let _ = fs::remove_dir_all(&tmp);
    }
}
