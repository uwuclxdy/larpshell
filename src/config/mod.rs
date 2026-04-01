use colored::*;
use inquire::{Confirm, Text};
use serde::{Deserialize, Serialize};
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
    #[serde(rename = "openai")]
    OpenAI,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(rename = "provider")]
    pub active_provider: ActiveProvider,
    #[serde(default)]
    pub providers: MultiProviderConfig,
}

impl Config {
    pub fn get_provider_config(&self) -> Result<ProviderConfig, LarpshellError> {
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
    OpenAI { openai: OpenAIConfig },
}

impl ProviderSpecificConfig {
    pub fn model(&self) -> &str {
        match self {
            ProviderSpecificConfig::Gemini { gemini } => &gemini.model,
            ProviderSpecificConfig::Ollama { ollama } => &ollama.model,
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
pub struct OpenAIConfig {
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub model: String,
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

fn get_config_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join("config.toml"))
}

pub fn get_sys_prompt_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join("sys-prompt.txt"))
}

pub fn load_sys_prompt() -> Option<String> {
    fs::read_to_string(get_sys_prompt_path().ok()?).ok()
}

pub fn save_sys_prompt(content: &str) -> Result<(), Box<dyn std::error::Error>> {
    Ok(fs::write(get_sys_prompt_path()?, content)?)
}

pub fn get_explain_prompt_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join("explain-prompt.txt"))
}

pub fn load_explain_prompt() -> Option<String> {
    let _ = migration::migrate_explain_prompt();
    fs::read_to_string(get_explain_prompt_path().ok()?).ok()
}

pub fn save_explain_prompt(content: &str) -> Result<(), Box<dyn std::error::Error>> {
    Ok(fs::write(get_explain_prompt_path()?, content)?)
}

pub fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let config_path = get_config_path()?;
    let contents = fs::read_to_string(&config_path)?;

    match toml::from_str::<Config>(&contents) {
        Ok(config) => Ok(config),
        Err(e) => {
            if migration::migrate_config(&config_path)? {
                let contents = fs::read_to_string(&config_path)?;
                Ok(toml::from_str(&contents)?)
            } else {
                Err(Box::new(e))
            }
        }
    }
}

pub fn save_config(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = get_config_path()?;
    let toml_string = toml::to_string_pretty(config)?;
    fs::write(&config_path, toml_string)?;
    Ok(())
}

pub fn interactive_setup() -> Result<(), Box<dyn std::error::Error>> {
    let existing_config = load_config().ok();
    let current_provider = existing_config.as_ref().map(|c| c.active_provider);

    const PROVIDER_OPTIONS: &[(&str, ActiveProvider)] = &[
        ("Gemini API", ActiveProvider::Gemini),
        ("Ollama", ActiveProvider::Ollama),
        ("OpenAI Compatible", ActiveProvider::OpenAI),
    ];

    let colored_providers: Vec<String> = PROVIDER_OPTIONS
        .iter()
        .map(|(name, variant)| {
            if Some(*variant) == current_provider {
                format!("{}", name.custom_color(CTP_GREEN))
            } else {
                name.to_string()
            }
        })
        .collect();

    let selection = prompt_select("Select API Provider", &colored_providers, 0)?;
    let (provider_display_name, selected_variant) = PROVIDER_OPTIONS[selection];

    let mut multi_providers = existing_config
        .as_ref()
        .map(|c| c.providers.clone())
        .unwrap_or_default();

    let has_saved_creds = match selected_variant {
        ActiveProvider::Gemini => multi_providers.gemini.is_some(),
        ActiveProvider::Ollama => multi_providers.ollama.is_some(),
        ActiveProvider::OpenAI => multi_providers.openai.is_some(),
    };

    let has_saved = if has_saved_creds && Some(selected_variant) != current_provider {
        let result = Confirm::new("Use saved credentials?")
            .with_default(true)
            .prompt()?;
        clear_line();
        result
    } else {
        false
    };

    let (active_provider, multi_providers) = if has_saved {
        (selected_variant, multi_providers)
    } else {
        let new_config = match selected_variant {
            ActiveProvider::Gemini => configure_gemini(multi_providers.gemini.as_ref())?,
            ActiveProvider::Ollama => configure_ollama(multi_providers.ollama.as_ref())?,
            ActiveProvider::OpenAI => configure_openai(multi_providers.openai.as_ref())?,
        };

        match &new_config.config {
            ProviderSpecificConfig::Gemini { gemini } => {
                multi_providers.gemini = Some(gemini.clone());
            }
            ProviderSpecificConfig::Ollama { ollama } => {
                multi_providers.ollama = Some(ollama.clone());
            }
            ProviderSpecificConfig::OpenAI { openai } => {
                multi_providers.openai = Some(openai.clone());
            }
        }

        (new_config.provider_type, multi_providers)
    };

    let config = Config {
        active_provider,
        providers: multi_providers,
    };

    save_config(&config)?;
    display_config_summary(&config, provider_display_name)?;

    Ok(())
}

fn display_config_summary(
    config: &Config,
    provider_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    print_ok_bold("Configuration saved!");
    eprintln!();
    eprintln!("Provider: {}", provider_name);

    let provider_config = config.get_provider_config()?;
    match &provider_config.config {
        ProviderSpecificConfig::Gemini { gemini } => {
            eprintln!("Model: {}", gemini.model);
        }
        ProviderSpecificConfig::Ollama { ollama } => {
            eprintln!("Model: {}", ollama.model);
            eprintln!("Base URL: {}", ollama.base_url);
        }
        ProviderSpecificConfig::OpenAI { openai } => {
            eprintln!("Model: {}", openai.model);
            eprintln!("Base URL: {}", openai.base_url);
        }
    }

    Ok(())
}

fn configure_gemini(
    existing: Option<&GeminiConfig>,
) -> Result<ProviderConfig, Box<dyn std::error::Error>> {
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

fn configure_ollama(
    existing: Option<&OllamaConfig>,
) -> Result<ProviderConfig, Box<dyn std::error::Error>> {
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

fn configure_openai(
    existing: Option<&OpenAIConfig>,
) -> Result<ProviderConfig, Box<dyn std::error::Error>> {
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
        text.prompt_skippable()?
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

fn prompt_model_name(default: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
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
