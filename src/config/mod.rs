use inquire::{Confirm, Password, PasswordDisplayMode};
use serde::{Deserialize, Deserializer, Serialize};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::{
    map_inquire_cancel, print_error, print_ok, print_warning, prompt_input, prompt_select,
    render_config,
};
use crate::common::clear_n_lines;
use crate::confirmation::style_message_markup;
use crate::error::LarpshellError;
mod migration;
pub use migration::migrate_from_nlsh_rs;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
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
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub const fn is_safe(self) -> bool {
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
    #[serde(default = "default_verbose_tool_output")]
    pub verbose_tool_output: bool,
}

impl Config {
    pub fn provider_config(&self) -> Result<ProviderConfig, LarpshellError> {
        macro_rules! require {
            ($opt:expr, $name:literal) => {
                $opt.clone().ok_or_else(|| {
                    LarpshellError::ConfigError(
                        concat!($name, " config not found for active provider").to_string(),
                    )
                })?
            };
        }
        let (provider_type, config) = match self.active_provider {
            ActiveProvider::Gemini => (
                ActiveProvider::Gemini,
                ProviderSpecificConfig::Gemini {
                    gemini: require!(self.providers.gemini, "gemini"),
                },
            ),
            ActiveProvider::Ollama => (
                ActiveProvider::Ollama,
                ProviderSpecificConfig::Ollama {
                    ollama: require!(self.providers.ollama, "ollama"),
                },
            ),
            ActiveProvider::OpenRouter => (
                ActiveProvider::OpenRouter,
                ProviderSpecificConfig::OpenRouter {
                    openrouter: require!(self.providers.openrouter, "openrouter"),
                },
            ),
            ActiveProvider::OpenAI => (
                ActiveProvider::OpenAI,
                ProviderSpecificConfig::OpenAI {
                    openai: require!(self.providers.openai, "openai"),
                },
            ),
        };
        Ok(ProviderConfig {
            provider_type,
            config,
        })
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

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub provider_type: ActiveProvider,
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
            Self::Gemini { gemini } => &gemini.model,
            Self::Ollama { ollama } => &ollama.model,
            Self::OpenRouter { openrouter } => &openrouter.model,
            Self::OpenAI { openai } => &openai.model,
        }
    }

    pub fn base_url(&self) -> Option<&str> {
        match self {
            Self::Gemini { .. } => None,
            Self::Ollama { ollama } => Some(&ollama.base_url),
            Self::OpenRouter { openrouter } => Some(&openrouter.base_url),
            Self::OpenAI { openai } => Some(&openai.base_url),
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

fn migrate_txt_prompt(md_path: &Path) {
    let txt_path = md_path.with_extension("txt");
    if !txt_path.exists() || md_path.exists() {
        return;
    }

    let remove_source = match fs::rename(&txt_path, md_path) {
        Ok(()) => false,
        Err(rename_error) => match fs::copy(&txt_path, md_path) {
            Ok(_) => true,
            Err(copy_error) => {
                print_warning(&format!(
                    "failed to migrate {} to {}: {rename_error}; copy fallback failed: {copy_error}",
                    txt_path.display(),
                    md_path.display()
                ));
                false
            }
        },
    };

    if remove_source && let Err(error) = fs::remove_file(&txt_path) {
        print_warning(&format!(
            "failed to remove migrated prompt {}: {error}",
            txt_path.display()
        ));
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
    atomic_write(&sys_prompt_path()?, content)
}

pub fn explain_prompt_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join("explain-prompt.md"))
}

pub fn load_explain_prompt() -> Option<String> {
    let path = explain_prompt_path().ok()?;
    migrate_txt_prompt(&path);
    // migrate_explain_prompt reads the file; if it migrated, return the new
    // content directly to avoid a second read.
    if let Some(migrated) = migration::migrate_explain_prompt().ok().flatten() {
        return Some(migrated);
    }
    fs::read_to_string(path).ok()
}

pub fn save_explain_prompt(content: &str) -> Result<(), LarpshellError> {
    atomic_write(&explain_prompt_path()?, content)
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
    atomic_write(&agent_prompt_path()?, content)
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
    atomic_write(&agent_safe_prompt_path()?, content)
}

fn history_disabled_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join(".history-disabled"))
}

pub fn history_path() -> Result<PathBuf, LarpshellError> {
    Ok(ensure_config_dir()?.join(".history"))
}

pub fn history_enabled() -> bool {
    history_disabled_path().is_ok_and(|p| !p.exists())
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

const fn default_verbose_tool_output() -> bool {
    true
}

fn default_config() -> Config {
    Config {
        active_provider: ActiveProvider::Ollama,
        providers: MultiProviderConfig::default(),
        agent: AgentMode::Off,
        verbose_tool_output: default_verbose_tool_output(),
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

pub fn set_verbose_tool_output(enabled: bool) -> Result<(), LarpshellError> {
    let mut config = load_config_or_default()?;
    config.verbose_tool_output = enabled;
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

static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn atomic_temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "config".into(), |name| name.to_os_string());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        now + u128::from(ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed))
    ));
    path.with_file_name(name)
}

/// Writes `contents` to `path` atomically via an exclusive same-directory temp file.
pub(crate) fn atomic_write(path: &Path, contents: &str) -> Result<(), LarpshellError> {
    let tmp = atomic_temp_path(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| LarpshellError::ConfigError(format!("failed to create temp config: {e}")))?;
    if let Err(e) = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(&tmp);
        return Err(LarpshellError::ConfigError(format!(
            "failed to write temp config: {e}"
        )));
    }
    drop(file);
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(LarpshellError::ConfigError(format!(
            "failed to replace config atomically: {e}"
        )));
    }
    Ok(())
}

pub fn save_config(config: &Config) -> Result<(), LarpshellError> {
    let config_dir = ensure_config_dir()?;
    let config_path = config_dir.join("config.toml");
    let toml_string = toml::to_string_pretty(config)?;
    atomic_write(&config_path, &toml_string)
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
    let default_index = current_provider
        .and_then(|cp| {
            PROVIDER_OPTIONS
                .iter()
                .position(|(_, variant)| *variant == cp)
        })
        .unwrap_or(0);
    let selection = prompt_select(
        "Select API Provider",
        &provider_options_with_marker(current_provider),
        default_index,
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
        verbose_tool_output: existing_config
            .as_ref()
            .is_none_or(|c| c.verbose_tool_output),
    };

    save_config(&config)?;
    display_config_summary(&config, provider_display_name)?;

    Ok(())
}

/// Provider labels for the select menu, marking the active one with a plain-text
/// ` (current)` suffix. No ANSI is baked into the label so it neither overrides
/// the shared selected-row color nor skews the fuzzy-filter score.
fn provider_options_with_marker(current_provider: Option<ActiveProvider>) -> Vec<String> {
    PROVIDER_OPTIONS
        .iter()
        .map(|(name, variant)| {
            if Some(*variant) == current_provider {
                format!("{name} (current)")
            } else {
                (*name).to_string()
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
        ActiveProvider::OpenRouter => providers
            .openrouter
            .as_ref()
            .and_then(|config| config.api_key.as_deref())
            .is_some_and(|api_key| !api_key.trim().is_empty()),
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
        .with_render_config(render_config())
        .prompt()
        .map_err(map_inquire_cancel)?;
    // Move up over the persisted "? … Yes" answer line before erasing; a single
    // clear would only wipe the blank line inquire leaves below it.
    clear_n_lines(2);
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
    print_ok("Configuration saved!");
    eprintln!();
    eprintln!(
        "{}",
        style_message_markup(&format!("Provider: {provider_name}"))
    );

    let provider_config = config.provider_config()?;
    let specific = &provider_config.config;
    eprintln!(
        "{}",
        style_message_markup(&format!("Model: {}", specific.model()))
    );
    if let Some(url) = specific.base_url() {
        eprintln!("{}", style_message_markup(&format!("Base URL: {url}")));
    }

    Ok(())
}

fn configure_gemini(existing: Option<&GeminiConfig>) -> Result<ProviderConfig, LarpshellError> {
    let api_key = prompt_api_key("Gemini API key", existing.map(|e| e.api_key.as_str()))?;
    let model = prompt_model_name(Some(
        existing.map_or("gemini-flash-latest", |e| e.model.as_str()),
    ))?;

    Ok(ProviderConfig {
        provider_type: ActiveProvider::Gemini,
        config: ProviderSpecificConfig::Gemini {
            gemini: GeminiConfig { api_key, model },
        },
    })
}

fn configure_ollama(existing: Option<&OllamaConfig>) -> Result<ProviderConfig, LarpshellError> {
    let url_default = existing.map_or("http://localhost:11434", |e| e.base_url.as_str());
    let base_url = prompt_input("Ollama base URL", Some(url_default))?;

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
    let url_default = existing.map_or("https://openrouter.ai/api/v1", |e| e.base_url.as_str());
    let base_url = prompt_input("OpenRouter base URL", Some(url_default))?;

    let api_key = Some(prompt_api_key(
        "OpenRouter API key",
        existing.and_then(|e| e.api_key.as_deref()),
    )?);

    let model_default = existing.map_or("openrouter/auto", |e| e.model.as_str());
    let model = prompt_input("Model name", Some(model_default))?;

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
    let url_default = existing.map_or("https://api.openai.com/v1", |e| e.base_url.as_str());
    let base_url = prompt_input("API base URL", Some(url_default))?;

    let api_key = prompt_optional_api_key(
        "API key (optional for local servers)",
        "Leave empty for local servers like LM Studio",
        existing.and_then(|e| e.api_key.as_deref()),
    )?;

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

/// A masked, single-entry password prompt (no confirmation step) themed with the
/// shared render config. Empty input is allowed so callers can implement the
/// "leave blank to keep saved key" behavior.
fn masked_password<'a>(label: &'a str, help: Option<&'a str>) -> Password<'a> {
    let mut prompt = Password::new(label)
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation()
        .with_render_config(render_config());
    if let Some(help) = help {
        prompt = prompt.with_help_message(help);
    }
    prompt
}

/// A saved key is reusable only when it holds a non-blank secret. A blank saved
/// key for a required provider must be re-entered rather than silently
/// re-persisted.
fn reusable_saved_key(saved: Option<&str>) -> Option<&str> {
    saved.filter(|key| !key.trim().is_empty())
}

/// Prompts for a required API key with masked input. A non-blank saved key can
/// be kept by submitting empty; otherwise a fresh, non-empty key is required.
fn prompt_api_key(label: &str, saved: Option<&str>) -> Result<String, LarpshellError> {
    if let Some(existing) = reusable_saved_key(saved) {
        let input = masked_password(label, Some("leave blank to keep saved key"))
            .prompt()
            .map_err(map_inquire_cancel)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(existing.to_owned());
        }
        Ok(trimmed.to_owned())
    } else {
        loop {
            let input = masked_password(label, None)
                .prompt()
                .map_err(map_inquire_cancel)?;
            let trimmed = input.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_owned());
            }
            print_error("API key cannot be empty");
        }
    }
}

/// Prompts for an optional API key with masked input. When a saved key exists,
/// an empty submit retains it; with no saved key an empty submit produces `None`.
fn prompt_optional_api_key(
    label: &str,
    help: &str,
    saved: Option<&str>,
) -> Result<Option<String>, LarpshellError> {
    let help_msg;
    let help_text = if saved.is_some() {
        help_msg = format!("{help} — leave blank to keep saved key");
        help_msg.as_str()
    } else {
        help
    };
    let input = masked_password(label, Some(help_text))
        .prompt_skippable()
        .map_err(map_inquire_cancel)?;
    match input {
        Some(s) if s.trim().is_empty() => Ok(saved.map(str::to_owned)),
        Some(s) => Ok(Some(s.trim().to_owned())),
        None => Ok(saved.map(str::to_owned)),
    }
}

fn prompt_model_name(default: Option<&str>) -> Result<String, LarpshellError> {
    loop {
        let model = prompt_input("Model name", default)?;

        if !model.trim().is_empty() {
            return Ok(model);
        }
        print_error("Model name cannot be empty");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_default_index_gemini() {
        let idx = PROVIDER_OPTIONS
            .iter()
            .position(|(_, v)| *v == ActiveProvider::Gemini)
            .unwrap_or(0);
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_provider_default_index_ollama() {
        let idx = PROVIDER_OPTIONS
            .iter()
            .position(|(_, v)| *v == ActiveProvider::Ollama)
            .unwrap_or(0);
        assert_eq!(idx, 1);
    }

    #[test]
    fn test_provider_default_index_openrouter() {
        let idx = PROVIDER_OPTIONS
            .iter()
            .position(|(_, v)| *v == ActiveProvider::OpenRouter)
            .unwrap_or(0);
        assert_eq!(idx, 2);
    }

    #[test]
    fn test_provider_default_index_openai() {
        let idx = PROVIDER_OPTIONS
            .iter()
            .position(|(_, v)| *v == ActiveProvider::OpenAI)
            .unwrap_or(0);
        assert_eq!(idx, 3);
    }

    #[test]
    fn openrouter_saved_credentials_require_api_key() {
        let providers = MultiProviderConfig {
            openrouter: Some(OpenRouterConfig {
                base_url: "https://openrouter.ai/api/v1".to_string(),
                api_key: None,
                model: "openrouter/auto".to_string(),
            }),
            ..Default::default()
        };

        assert!(!provider_has_saved_credentials(
            &providers,
            ActiveProvider::OpenRouter
        ));
    }

    #[test]
    fn openrouter_saved_credentials_reject_blank_api_key() {
        let providers = MultiProviderConfig {
            openrouter: Some(OpenRouterConfig {
                base_url: "https://openrouter.ai/api/v1".to_string(),
                api_key: Some("   ".to_string()),
                model: "openrouter/auto".to_string(),
            }),
            ..Default::default()
        };

        assert!(!provider_has_saved_credentials(
            &providers,
            ActiveProvider::OpenRouter
        ));
    }

    #[test]
    fn openrouter_saved_credentials_accept_saved_api_key() {
        let providers = MultiProviderConfig {
            openrouter: Some(OpenRouterConfig {
                base_url: "https://openrouter.ai/api/v1".to_string(),
                api_key: Some("sk-or-v1-test".to_string()),
                model: "openrouter/auto".to_string(),
            }),
            ..Default::default()
        };

        assert!(provider_has_saved_credentials(
            &providers,
            ActiveProvider::OpenRouter
        ));
    }

    #[test]
    fn provider_options_mark_current_with_plain_text() {
        let opts = provider_options_with_marker(Some(ActiveProvider::Ollama));
        assert!(
            opts[1].contains("(current)"),
            "active provider labelled: {:?}",
            opts[1]
        );
        assert!(!opts[0].contains("(current)"));
        for opt in &opts {
            assert!(
                !opt.contains('\u{1b}'),
                "labels must not bake in ANSI: {opt:?}"
            );
        }
    }

    #[test]
    fn reusable_saved_key_rejects_blank() {
        assert_eq!(reusable_saved_key(Some("sk-live")), Some("sk-live"));
        assert_eq!(reusable_saved_key(Some("   ")), None);
        assert_eq!(reusable_saved_key(Some("")), None);
        assert_eq!(reusable_saved_key(None), None);
    }

    #[test]
    fn test_history_enabled_returns_bool() {
        let result = history_enabled();
        // This is a simple smoke test. The function should not panic.
        // Fail-closed behavior: if ensure_config_dir fails, returns false.
        let _ = result; // Suppress unused warning
    }
}
