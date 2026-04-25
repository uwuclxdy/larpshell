use colored::*;
use inquire::{Confirm, Text};
use serde::{Deserialize, Deserializer, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::cli::{print_ok_bold, prompt_input, prompt_input_with_default, prompt_select};
use crate::common::{CTP_GREEN, CTP_RED, clear_line};
use crate::error::LarpshellError;
mod migration;
pub use migration::migrate_from_nlsh_rs;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ActiveProvider {
    Gemini,
    Ollama,
    OpenRouter,
    #[serde(rename = "openai")]
    OpenAI,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    #[default]
    Off,
    Safe,
    On,
}

impl AgentMode {
    pub fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn is_safe(self) -> bool {
        matches!(self, Self::Safe)
    }
}

fn deserialize_agent_mode<'de, D>(deserializer: D) -> Result<AgentMode, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum AgentModeValue {
        Bool(bool),
        Mode(AgentMode),
    }

    Ok(match AgentModeValue::deserialize(deserializer)? {
        AgentModeValue::Bool(false) => AgentMode::Off,
        AgentModeValue::Bool(true) => AgentMode::On,
        AgentModeValue::Mode(mode) => mode,
    })
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(rename = "provider")]
    pub active_provider: ActiveProvider,
    #[serde(default)]
    pub providers: MultiProviderConfig,
    #[serde(default, deserialize_with = "deserialize_agent_mode")]
    pub agent: AgentMode,
}

impl Config {
    pub fn provider_config(&self) -> Result<ProviderConfig, LarpshellError> {
        match self.active_provider {
            ActiveProvider::Gemini => Ok(ProviderConfig {
                provider_type: ActiveProvider::Gemini,
                config: ProviderSpecificConfig::Gemini {
                    gemini: self.providers.gemini.clone().ok_or_else(|| {
                        LarpshellError::ConfigError(
                            "gemini config not found for active provider".to_string(),
                        )
                    })?,
                },
            }),
            ActiveProvider::Ollama => Ok(ProviderConfig {
                provider_type: ActiveProvider::Ollama,
                config: ProviderSpecificConfig::Ollama {
                    ollama: self.providers.ollama.clone().ok_or_else(|| {
                        LarpshellError::ConfigError(
                            "ollama config not found for active provider".to_string(),
                        )
                    })?,
                },
            }),
            ActiveProvider::OpenRouter => Ok(ProviderConfig {
                provider_type: ActiveProvider::OpenRouter,
                config: ProviderSpecificConfig::OpenRouter {
                    openrouter: self.providers.openrouter.clone().ok_or_else(|| {
                        LarpshellError::ConfigError(
                            "openrouter config not found for active provider".to_string(),
                        )
                    })?,
                },
            }),
            ActiveProvider::OpenAI => Ok(ProviderConfig {
                provider_type: ActiveProvider::OpenAI,
                config: ProviderSpecificConfig::OpenAI {
                    openai: self.providers.openai.clone().ok_or_else(|| {
                        LarpshellError::ConfigError(
                            "openai config not found for active provider".to_string(),
                        )
                    })?,
                },
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct MultiProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gemini: Option<GeminiConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama: Option<OllamaConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openrouter: Option<OpenRouterConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai: Option<OpenAIConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProviderConfig {
    #[serde(rename = "type")]
    pub provider_type: ActiveProvider,
    #[serde(flatten)]
    pub config: ProviderSpecificConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum ProviderSpecificConfig {
    Gemini { gemini: GeminiConfig },
    Ollama { ollama: OllamaConfig },
    OpenRouter { openrouter: OpenRouterConfig },
    OpenAI { openai: OpenAIConfig },
}

impl ProviderSpecificConfig {
    pub fn model(&self) -> &str {
        match self {
            ProviderSpecificConfig::Gemini { gemini } => &gemini.model,
            ProviderSpecificConfig::Ollama { ollama } => &ollama.model,
            ProviderSpecificConfig::OpenRouter { openrouter } => &openrouter.model,
            ProviderSpecificConfig::OpenAI { openai } => &openai.model,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GeminiConfig {
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OllamaConfig {
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OpenRouterConfig {
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub model: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OpenAIConfig {
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub model: String,
}

fn migrate_txt_prompt(md_path: &std::path::Path) {
    let txt_path = md_path.with_extension("txt");
    if txt_path.exists() && !md_path.exists() {
        let _ = fs::rename(&txt_path, md_path)
            .or_else(|_| fs::copy(&txt_path, md_path).map(|_| ()));
        if md_path.exists() {
            let _ = fs::remove_file(&txt_path);
        }
    }
}

pub fn ensure_config_dir() -> Result<PathBuf, LarpshellError> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| LarpshellError::ConfigError("failed to get config directory".to_string()))?
        .join("larpshell");
    fs::create_dir_all(&config_dir).map_err(|e| {
        LarpshellError::ConfigError(format!("failed to create config directory: {e}"))
    })?;
    Ok(config_dir)
}

fn config_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join("config.toml"))
}

pub fn sys_prompt_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join("sys-prompt.md"))
}

pub fn load_sys_prompt() -> Option<String> {
    let path = sys_prompt_path().ok()?;
    migrate_txt_prompt(&path);
    fs::read_to_string(path).ok()
}

pub fn save_sys_prompt(content: &str) -> Result<(), LarpshellError> {
    Ok(fs::write(sys_prompt_path()?, content)?)
}

pub fn explain_prompt_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join("explain-prompt.md"))
}

pub fn load_explain_prompt() -> Option<String> {
    let path = explain_prompt_path().ok()?;
    migrate_txt_prompt(&path);
    let _ = migration::migrate_explain_prompt();
    fs::read_to_string(path).ok()
}

pub fn save_explain_prompt(content: &str) -> Result<(), LarpshellError> {
    Ok(fs::write(explain_prompt_path()?, content)?)
}

pub fn agent_prompt_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join("agent-prompt.md"))
}

pub fn load_agent_prompt() -> Option<String> {
    let path = agent_prompt_path().ok()?;
    migrate_txt_prompt(&path);
    fs::read_to_string(path).ok()
}

pub fn save_agent_prompt(content: &str) -> Result<(), LarpshellError> {
    Ok(fs::write(agent_prompt_path()?, content)?)
}

pub fn agent_safe_prompt_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join("agent-safe-prompt.md"))
}

pub fn load_agent_safe_prompt() -> Option<String> {
    let path = agent_safe_prompt_path().ok()?;
    migrate_txt_prompt(&path);
    fs::read_to_string(path).ok()
}

pub fn save_agent_safe_prompt(content: &str) -> Result<(), LarpshellError> {
    Ok(fs::write(agent_safe_prompt_path()?, content)?)
}

fn history_disabled_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join(".history-disabled"))
}

pub fn history_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join(".history"))
}

pub fn history_enabled() -> bool {
    history_disabled_path().map(|p| !p.exists()).unwrap_or(true)
}

pub fn set_history_enabled(enabled: bool) -> Result<(), LarpshellError> {
    let path = history_disabled_path()?;
    if !enabled {
        fs::write(&path, "")
            .map_err(|e| LarpshellError::ConfigError(format!("failed to disable history: {e}")))?;
    } else if path.exists() {
        fs::remove_file(&path)
            .map_err(|e| LarpshellError::ConfigError(format!("failed to enable history: {e}")))?;
    }
    Ok(())
}

fn default_config() -> Config {
    Config {
        active_provider: ActiveProvider::Ollama,
        providers: MultiProviderConfig::default(),
        agent: AgentMode::Off,
    }
}

fn load_config_or_default() -> Result<Config, LarpshellError> {
    match load_config() {
        Ok(config) => Ok(config),
        Err(LarpshellError::IoError(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(default_config())
        }
        Err(error) => Err(error),
    }
}

pub fn set_agent_mode(mode: AgentMode) -> Result<(), LarpshellError> {
    let mut config = load_config_or_default()?;
    config.agent = mode;
    save_config(&config)?;
    Ok(())
}

pub fn load_config() -> Result<Config, LarpshellError> {
    let config_path = config_path()?;
    let contents = fs::read_to_string(&config_path)?;

    match toml::from_str::<Config>(&contents) {
        Ok(config) => Ok(config),
        Err(e) => {
            if migration::migrate_config(&config_path)? {
                let contents = fs::read_to_string(&config_path)?;
                Ok(toml::from_str(&contents)?)
            } else {
                Err(LarpshellError::TomlDeError(e))
            }
        }
    }
}

pub fn save_config(config: &Config) -> Result<(), LarpshellError> {
    let config_dir = ensure_config_dir()?;
    let config_path = config_dir.join("config.toml");
    let toml_string = toml::to_string_pretty(config)?;
    fs::write(&config_path, toml_string)?;
    Ok(())
}

const PROVIDER_OPTIONS: &[(&str, ActiveProvider)] = &[
    ("Gemini API", ActiveProvider::Gemini),
    ("Ollama", ActiveProvider::Ollama),
    ("OpenRouter", ActiveProvider::OpenRouter),
    ("OpenAI Compatible", ActiveProvider::OpenAI),
];

pub fn interactive_setup() -> Result<(), LarpshellError> {
    let existing_config = load_config().ok();
    let current_provider = existing_config.as_ref().map(|c| c.active_provider);
    let selection = prompt_select(
        "Select API Provider",
        &colored_provider_options(current_provider),
        0,
    )?;
    let (provider_display_name, selected_variant) = PROVIDER_OPTIONS[selection];

    let mut multi_providers = existing_config
        .as_ref()
        .map(|c| c.providers.clone())
        .unwrap_or_default();

    let should_reuse_saved =
        should_reuse_saved_credentials(&multi_providers, selected_variant, current_provider)?;

    let active_provider = if should_reuse_saved {
        selected_variant
    } else {
        let new_config = configure_provider(selected_variant, &multi_providers)?;
        apply_provider_config(&mut multi_providers, &new_config);
        new_config.provider_type
    };

    let config = Config {
        active_provider,
        providers: multi_providers,
        agent: existing_config.as_ref().map_or(AgentMode::Off, |c| c.agent),
    };

    save_config(&config)?;
    display_config_summary(&config, provider_display_name)?;

    Ok(())
}

fn colored_provider_options(current_provider: Option<ActiveProvider>) -> Vec<String> {
    PROVIDER_OPTIONS
        .iter()
        .map(|(name, variant)| {
            if Some(*variant) == current_provider {
                format!("{}", name.custom_color(CTP_GREEN))
            } else {
                name.to_string()
            }
        })
        .collect()
}

fn provider_has_saved_credentials(
    providers: &MultiProviderConfig,
    selected_variant: ActiveProvider,
) -> bool {
    match selected_variant {
        ActiveProvider::Gemini => providers.gemini.is_some(),
        ActiveProvider::Ollama => providers.ollama.is_some(),
        ActiveProvider::OpenRouter => providers.openrouter.is_some(),
        ActiveProvider::OpenAI => providers.openai.is_some(),
    }
}

fn should_reuse_saved_credentials(
    providers: &MultiProviderConfig,
    selected_variant: ActiveProvider,
    current_provider: Option<ActiveProvider>,
) -> Result<bool, LarpshellError> {
    if !provider_has_saved_credentials(providers, selected_variant)
        || Some(selected_variant) == current_provider
    {
        return Ok(false);
    }

    let result = Confirm::new("Use saved credentials?")
        .with_default(true)
        .prompt()
        .map_err(LarpshellError::InquireError)?;
    clear_line();
    Ok(result)
}

fn configure_provider(
    selected_variant: ActiveProvider,
    providers: &MultiProviderConfig,
) -> Result<ProviderConfig, LarpshellError> {
    match selected_variant {
        ActiveProvider::Gemini => configure_gemini(providers.gemini.as_ref()),
        ActiveProvider::Ollama => configure_ollama(providers.ollama.as_ref()),
        ActiveProvider::OpenRouter => configure_openrouter(providers.openrouter.as_ref()),
        ActiveProvider::OpenAI => configure_openai(providers.openai.as_ref()),
    }
}

fn apply_provider_config(providers: &mut MultiProviderConfig, config: &ProviderConfig) {
    match &config.config {
        ProviderSpecificConfig::Gemini { gemini } => {
            providers.gemini = Some(gemini.clone());
        }
        ProviderSpecificConfig::Ollama { ollama } => {
            providers.ollama = Some(ollama.clone());
        }
        ProviderSpecificConfig::OpenRouter { openrouter } => {
            providers.openrouter = Some(openrouter.clone());
        }
        ProviderSpecificConfig::OpenAI { openai } => {
            providers.openai = Some(openai.clone());
        }
    }
}

fn display_config_summary(config: &Config, provider_name: &str) -> Result<(), LarpshellError> {
    print_ok_bold("Configuration saved!");
    eprintln!();
    eprintln!("Provider: {}", provider_name);

    let provider_config = config.provider_config()?;
    match &provider_config.config {
        ProviderSpecificConfig::Gemini { gemini } => {
            eprintln!("Model: {}", gemini.model);
        }
        ProviderSpecificConfig::Ollama { ollama } => {
            eprintln!("Model: {}", ollama.model);
            eprintln!("Base URL: {}", ollama.base_url);
        }
        ProviderSpecificConfig::OpenRouter { openrouter } => {
            eprintln!("Model: {}", openrouter.model);
            eprintln!("Base URL: {}", openrouter.base_url);
        }
        ProviderSpecificConfig::OpenAI { openai } => {
            eprintln!("Model: {}", openai.model);
            eprintln!("Base URL: {}", openai.base_url);
        }
    }

    Ok(())
}

fn configure_gemini(existing: Option<&GeminiConfig>) -> Result<ProviderConfig, LarpshellError> {
    let api_key = if let Some(e) = existing {
        prompt_input_with_default("Gemini API key", &e.api_key)?
    } else {
        prompt_input("Gemini API key")?
    };

    let model_default = existing
        .map(|e| e.model.as_str())
        .unwrap_or("gemini-flash-latest");
    let model = prompt_input_with_default("Model name", model_default)?;

    Ok(ProviderConfig {
        provider_type: ActiveProvider::Gemini,
        config: ProviderSpecificConfig::Gemini {
            gemini: GeminiConfig { api_key, model },
        },
    })
}

fn configure_ollama(existing: Option<&OllamaConfig>) -> Result<ProviderConfig, LarpshellError> {
    let url_default = existing
        .map(|e| e.base_url.as_str())
        .unwrap_or("http://localhost:11434");
    let base_url = prompt_input_with_default("Ollama base URL", url_default)?;

    let model = prompt_model_name(existing.map(|e| e.model.as_str()))?;

    Ok(ProviderConfig {
        provider_type: ActiveProvider::Ollama,
        config: ProviderSpecificConfig::Ollama {
            ollama: OllamaConfig { base_url, model },
        },
    })
}

fn configure_openrouter(
    existing: Option<&OpenRouterConfig>,
) -> Result<ProviderConfig, LarpshellError> {
    let url_default = existing
        .map(|e| e.base_url.as_str())
        .unwrap_or("https://openrouter.ai/api/v1");
    let base_url = prompt_input_with_default("OpenRouter base URL", url_default)?;

    let api_key = {
        let mut text =
            Text::new("OpenRouter API key").with_help_message("Required for OpenRouter requests");
        if let Some(saved) = existing.and_then(|e| e.api_key.as_deref()) {
            text = text.with_default(saved);
        }
        text.prompt_skippable()
            .map_err(|e| LarpshellError::ConfigError(e.to_string()))?
    };

    let model_default = existing
        .map(|e| e.model.as_str())
        .unwrap_or("openrouter/auto");
    let model = prompt_input_with_default("Model name", model_default)?;

    Ok(ProviderConfig {
        provider_type: ActiveProvider::OpenRouter,
        config: ProviderSpecificConfig::OpenRouter {
            openrouter: OpenRouterConfig {
                base_url,
                api_key,
                model,
            },
        },
    })
}

fn configure_openai(existing: Option<&OpenAIConfig>) -> Result<ProviderConfig, LarpshellError> {
    let url_default = existing
        .map(|e| e.base_url.as_str())
        .unwrap_or("https://api.openai.com/v1");
    let base_url = prompt_input_with_default("API base URL", url_default)?;

    let api_key = {
        let mut text = Text::new("API key (optional for local servers)")
            .with_help_message("Leave empty for local servers like LM Studio");
        if let Some(saved) = existing.and_then(|e| e.api_key.as_deref()) {
            text = text.with_default(saved);
        }
        text.prompt_skippable()
            .map_err(|e| LarpshellError::ConfigError(e.to_string()))?
    };

    let model = prompt_model_name(existing.map(|e| e.model.as_str()))?;

    Ok(ProviderConfig {
        provider_type: ActiveProvider::OpenAI,
        config: ProviderSpecificConfig::OpenAI {
            openai: OpenAIConfig {
                base_url,
                api_key,
                model,
            },
        },
    })
}

fn prompt_model_name(default: Option<&str>) -> Result<String, LarpshellError> {
    loop {
        let model = if let Some(def) = default {
            prompt_input_with_default("Model name:", def)?
        } else {
            prompt_input("Model name:")?
        };

        if !model.trim().is_empty() {
            return Ok(model);
        }
        eprintln!("{}", "Model name cannot be empty".custom_color(CTP_RED));
    }
}
