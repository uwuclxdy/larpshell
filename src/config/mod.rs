use inquire::{Password, PasswordDisplayMode};
use serde::{Deserialize, Deserializer, Serialize};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::{map_inquire_cancel, print_error, print_ok, print_warning, prompt_input, prompt_select, render_config};
use crate::confirmation::style_message_markup;
use crate::error::LarpshellError;
mod migration;
pub use migration::{migrate_from_nlsh_rs, migrate_macos_config_dir};

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActiveProvider {
    Gemini,
    Ollama,
    OpenRouter,
    #[serde(rename = "openai")]
    OpenAI,
}

impl ActiveProvider {
    /// Serde wire name, also the default profile name for the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gemini => "gemini",
            Self::Ollama => "ollama",
            Self::OpenRouter => "openrouter",
            Self::OpenAI => "openai",
        }
    }

    /// Label shown in setup menus and status output.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Gemini => "Gemini API",
            Self::Ollama => "Ollama",
            Self::OpenRouter => "OpenRouter",
            Self::OpenAI => "OpenAI Compatible",
        }
    }
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
    pub active_provider: String,
    #[serde(default)]
    pub providers: Vec<ProviderProfile>,
    #[serde(default, deserialize_with = "deserialize_agent_mode")]
    pub agent: AgentMode,
    #[serde(default = "default_verbose_tool_output")]
    pub verbose_tool_output: bool,
}

impl Config {
    /// The active profile's kind-specific settings.
    pub fn provider_config(&self) -> Result<ProviderSpecificConfig, LarpshellError> {
        let profile = self
            .providers
            .iter()
            .find(|profile| profile.name == self.active_provider)
            .ok_or_else(|| LarpshellError::ConfigError(format!("config not found for active provider \"{}\"", self.active_provider)))?;
        Ok(profile.config.clone())
    }
}

/// One saved provider instance. `name` is the toggling key; `config` carries
/// `kind` plus the kind-specific fields, flattened into the same TOML table.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProviderProfile {
    pub name: String,
    #[serde(flatten)]
    pub config: ProviderSpecificConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ProviderSpecificConfig {
    Gemini(GeminiConfig),
    Ollama(OllamaConfig),
    OpenRouter(OpenRouterConfig),
    #[serde(rename = "openai")]
    OpenAI(OpenAIConfig),
}

impl ProviderSpecificConfig {
    pub fn provider_type(&self) -> ActiveProvider {
        match self {
            Self::Gemini(_) => ActiveProvider::Gemini,
            Self::Ollama(_) => ActiveProvider::Ollama,
            Self::OpenRouter(_) => ActiveProvider::OpenRouter,
            Self::OpenAI(_) => ActiveProvider::OpenAI,
        }
    }

    pub fn model(&self) -> &str {
        match self {
            Self::Gemini(config) => &config.model,
            Self::Ollama(config) => &config.model,
            Self::OpenRouter(config) => &config.model,
            Self::OpenAI(config) => &config.model,
        }
    }

    pub fn base_url(&self) -> Option<&str> {
        match self {
            Self::Gemini(_) => None,
            Self::Ollama(config) => Some(&config.base_url),
            Self::OpenRouter(config) => Some(&config.base_url),
            Self::OpenAI(config) => Some(&config.base_url),
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
        print_warning(&format!("failed to remove migrated prompt {}: {error}", txt_path.display()));
    }
}

/// Resolves the config base directory with XDG semantics on all unix
/// (`XDG_CONFIG_HOME` when absolute, else `~/.config`), matching `dirs::config_dir()`
/// on Linux. On non-unix, delegates to `dirs::config_dir()` unchanged.
fn config_base_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|p| PathBuf::from(p).is_absolute())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
    }
    #[cfg(not(unix))]
    {
        dirs::config_dir()
    }
}

pub fn ensure_config_dir() -> Result<PathBuf, LarpshellError> {
    let config_dir =
        config_base_dir().ok_or_else(|| LarpshellError::ConfigError("failed to get config directory".to_string()))?.join("larpshell");
    fs::create_dir_all(&config_dir).map_err(|e| LarpshellError::ConfigError(format!("failed to create config directory: {e}")))?;
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
        fs::write(&path, "").map_err(|e| LarpshellError::ConfigError(format!("failed to disable history: {e}")))?;
    } else if path.exists() {
        fs::remove_file(&path).map_err(|e| LarpshellError::ConfigError(format!("failed to enable history: {e}")))?;
    }
    Ok(())
}

const fn default_verbose_tool_output() -> bool {
    true
}

fn default_config() -> Config {
    Config {
        active_provider: String::new(),
        providers: Vec::new(),
        agent: AgentMode::Off,
        verbose_tool_output: default_verbose_tool_output(),
    }
}

fn load_config_or_default() -> Result<Config, LarpshellError> {
    match load_config() {
        Ok(config) => Ok(config),
        Err(LarpshellError::IoError(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(default_config()),
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
    let mut name = path.file_name().map_or_else(|| "config".into(), |name| name.to_os_string());
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    name.push(format!(".{}.{}.tmp", std::process::id(), now + u128::from(ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed))));
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
    if let Err(e) = file.write_all(contents.as_bytes()).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&tmp);
        return Err(LarpshellError::ConfigError(format!("failed to write temp config: {e}")));
    }
    drop(file);
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(LarpshellError::ConfigError(format!("failed to replace config atomically: {e}")));
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

const ADD_NEW_PROVIDER: &str = "+ add new provider";

/// `/api` entry point: pick a saved profile to reconfigure, or add a new one.
/// The touched profile becomes active.
pub fn interactive_setup() -> Result<(), LarpshellError> {
    // A malformed config must not be overwritten by the setup flow: only a
    // missing file starts fresh.
    let existing_config = match load_config() {
        Ok(config) => Some(config),
        Err(LarpshellError::IoError(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut profiles = existing_config.as_ref().map(|c| c.providers.clone()).unwrap_or_default();
    let current_index = existing_config.as_ref().and_then(|c| c.profiles_active_index());

    let mut items: Vec<String> = profiles.iter().map(profile_menu_label).collect();
    items.push(ADD_NEW_PROVIDER.to_string());
    let selection = prompt_select("Select provider", &items, current_index.unwrap_or(0))?;

    let (active_name, active_config) = if selection == items.len() - 1 {
        let new_profile = configure_new_profile(&profiles)?;
        profiles.push(new_profile.clone());
        (new_profile.name, new_profile.config)
    } else {
        let profile = &mut profiles[selection];
        let existing = profile.config.clone();
        profile.config = configure_provider(existing.provider_type(), Some(&existing))?;
        (profile.name.clone(), profile.config.clone())
    };

    let config = Config {
        active_provider: active_name.clone(),
        providers: profiles,
        agent: existing_config.as_ref().map_or(AgentMode::Off, |c| c.agent),
        verbose_tool_output: existing_config.as_ref().is_none_or(|c| c.verbose_tool_output),
    };

    save_config(&config)?;
    display_config_summary(&active_config, &active_name);

    Ok(())
}

impl Config {
    fn profiles_active_index(&self) -> Option<usize> {
        self.providers.iter().position(|p| p.name == self.active_provider)
    }
}

/// `/provider` entry point: select a saved profile to activate.
pub fn interactive_provider_switch() -> Result<(), LarpshellError> {
    let mut config = load_config_or_default()?;
    if config.providers.is_empty() {
        return Err(LarpshellError::ConfigError("no saved providers — run 'larpshell api' first".to_string()));
    }

    let items: Vec<String> = config.providers.iter().map(|p| profile_menu_label_with_current(p, &config.active_provider)).collect();
    let selection = prompt_select("Switch to provider", &items, config.profiles_active_index().unwrap_or(0))?;
    config.active_provider = config.providers[selection].name.clone();
    save_config(&config)?;
    print_ok(&format!("switched to provider \"{}\"", config.active_provider));

    Ok(())
}

/// Activates a saved profile by name without any prompt.
pub fn set_active_provider(name: &str) -> Result<(), LarpshellError> {
    let mut config = load_config_or_default()?;
    if !config.providers.iter().any(|profile| profile.name == name) {
        return Err(LarpshellError::ConfigError(format!("no provider named \"{name}\"")));
    }
    config.active_provider = name.to_string();
    save_config(&config)
}

fn profile_menu_label(profile: &ProviderProfile) -> String {
    format!("{} — {} ({})", profile.name, profile.config.provider_type().display_name(), profile.config.model())
}

fn profile_menu_label_with_current(profile: &ProviderProfile, active_name: &str) -> String {
    let label = profile_menu_label(profile);
    if profile.name == active_name { format!("{label} (current)") } else { label }
}

/// First name for a new profile of this kind: the kind string, then
/// `<kind>-2`, `<kind>-3`, … while taken.
fn unique_profile_name(kind: ActiveProvider, profiles: &[ProviderProfile]) -> String {
    let base = kind.as_str();
    if !profiles.iter().any(|p| p.name == base) {
        return base.to_string();
    }
    for n in 2.. {
        let candidate = format!("{base}-{n}");
        if !profiles.iter().any(|p| p.name == candidate) {
            return candidate;
        }
    }
    unreachable!("integer suffix space is infinite")
}

fn configure_new_profile(profiles: &[ProviderProfile]) -> Result<ProviderProfile, LarpshellError> {
    let kind_labels: Vec<String> = PROVIDER_OPTIONS.iter().map(|(label, _)| (*label).to_string()).collect();
    let selection = prompt_select("Select provider type", &kind_labels, 0)?;
    let kind = PROVIDER_OPTIONS[selection].1;

    let default_name = unique_profile_name(kind, profiles);
    let name = loop {
        let input = prompt_input("Profile name", Some(&default_name))?;
        if input.is_empty() {
            print_error("Profile name cannot be empty");
            continue;
        }
        if profiles.iter().any(|p| p.name == input) {
            print_error("A provider with this name already exists");
            continue;
        }
        break input;
    };

    Ok(ProviderProfile { name, config: configure_provider(kind, None)? })
}

fn configure_provider(kind: ActiveProvider, existing: Option<&ProviderSpecificConfig>) -> Result<ProviderSpecificConfig, LarpshellError> {
    match kind {
        ActiveProvider::Gemini => configure_gemini(existing.and_then(|c| match c {
            ProviderSpecificConfig::Gemini(config) => Some(config),
            _ => None,
        })),
        ActiveProvider::Ollama => configure_ollama(existing.and_then(|c| match c {
            ProviderSpecificConfig::Ollama(config) => Some(config),
            _ => None,
        })),
        ActiveProvider::OpenRouter => configure_openrouter(existing.and_then(|c| match c {
            ProviderSpecificConfig::OpenRouter(config) => Some(config),
            _ => None,
        })),
        ActiveProvider::OpenAI => configure_openai(existing.and_then(|c| match c {
            ProviderSpecificConfig::OpenAI(config) => Some(config),
            _ => None,
        })),
    }
}

fn display_config_summary(config: &ProviderSpecificConfig, name: &str) {
    print_ok("Configuration saved!");
    eprintln!();
    eprintln!("{}", style_message_markup(&format!("Provider: {name}")));
    eprintln!("{}", style_message_markup(&format!("Model: {}", config.model())));
    if let Some(url) = config.base_url() {
        eprintln!("{}", style_message_markup(&format!("Base URL: {url}")));
    }
}

fn configure_gemini(existing: Option<&GeminiConfig>) -> Result<ProviderSpecificConfig, LarpshellError> {
    let api_key = prompt_api_key("Gemini API key", existing.map(|e| e.api_key.as_str()))?;
    let model = prompt_model_name(Some(existing.map_or("gemini-flash-latest", |e| e.model.as_str())))?;

    Ok(ProviderSpecificConfig::Gemini(GeminiConfig { api_key, model }))
}

fn configure_ollama(existing: Option<&OllamaConfig>) -> Result<ProviderSpecificConfig, LarpshellError> {
    let url_default = existing.map_or("http://localhost:11434", |e| e.base_url.as_str());
    let base_url = prompt_input("Ollama base URL", Some(url_default))?;

    let model = prompt_model_name(existing.map(|e| e.model.as_str()))?;

    Ok(ProviderSpecificConfig::Ollama(OllamaConfig { base_url, model }))
}

fn configure_openrouter(existing: Option<&OpenRouterConfig>) -> Result<ProviderSpecificConfig, LarpshellError> {
    let url_default = existing.map_or("https://openrouter.ai/api/v1", |e| e.base_url.as_str());
    let base_url = prompt_input("OpenRouter base URL", Some(url_default))?;

    let api_key = Some(prompt_api_key("OpenRouter API key", existing.and_then(|e| e.api_key.as_deref()))?);

    let model_default = existing.map_or("openrouter/auto", |e| e.model.as_str());
    let model = prompt_input("Model name", Some(model_default))?;

    Ok(ProviderSpecificConfig::OpenRouter(OpenRouterConfig { base_url, api_key, model }))
}

fn configure_openai(existing: Option<&OpenAIConfig>) -> Result<ProviderSpecificConfig, LarpshellError> {
    let url_default = existing.map_or("https://api.openai.com/v1", |e| e.base_url.as_str());
    let base_url = prompt_input("API base URL", Some(url_default))?;

    let api_key = prompt_optional_api_key(
        "API key (optional for local servers)",
        "Leave empty for local servers like LM Studio",
        existing.and_then(|e| e.api_key.as_deref()),
    )?;

    let model = prompt_model_name(existing.map(|e| e.model.as_str()))?;

    Ok(ProviderSpecificConfig::OpenAI(OpenAIConfig { base_url, api_key, model }))
}

/// A masked, single-entry password prompt (no confirmation step) themed with the
/// shared render config. Empty input is allowed so callers can implement the
/// "leave blank to keep saved key" behavior.
fn masked_password<'a>(label: &'a str, help: Option<&'a str>) -> Password<'a> {
    let mut prompt =
        Password::new(label).with_display_mode(PasswordDisplayMode::Masked).without_confirmation().with_render_config(render_config());
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
        let input = masked_password(label, Some("leave blank to keep saved key")).prompt().map_err(map_inquire_cancel)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(existing.to_owned());
        }
        Ok(trimmed.to_owned())
    } else {
        loop {
            let input = masked_password(label, None).prompt().map_err(map_inquire_cancel)?;
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
fn prompt_optional_api_key(label: &str, help: &str, saved: Option<&str>) -> Result<Option<String>, LarpshellError> {
    let help_msg;
    let help_text = if saved.is_some() {
        help_msg = format!("{help} — leave blank to keep saved key");
        help_msg.as_str()
    } else {
        help
    };
    let input = masked_password(label, Some(help_text)).prompt_skippable().map_err(map_inquire_cancel)?;
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

    fn ollama_profile(name: &str) -> ProviderProfile {
        ProviderProfile {
            name: name.to_string(),
            config: ProviderSpecificConfig::Ollama(OllamaConfig {
                base_url: "http://localhost:11434".to_string(),
                model: "llama3".to_string(),
            }),
        }
    }

    #[test]
    fn provider_kind_wire_names_match_kind_strings() {
        assert_eq!(ActiveProvider::Gemini.as_str(), "gemini");
        assert_eq!(ActiveProvider::Ollama.as_str(), "ollama");
        assert_eq!(ActiveProvider::OpenRouter.as_str(), "openrouter");
        assert_eq!(ActiveProvider::OpenAI.as_str(), "openai");
    }

    #[test]
    fn provider_profile_roundtrips_through_toml() {
        let profile = ProviderProfile {
            name: "home-ollama".to_string(),
            config: ProviderSpecificConfig::Ollama(OllamaConfig {
                base_url: "http://localhost:11434".to_string(),
                model: "llama3".to_string(),
            }),
        };
        let config = Config {
            active_provider: "home-ollama".to_string(),
            providers: vec![profile],
            agent: AgentMode::Off,
            verbose_tool_output: true,
        };

        let toml_string = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_string).unwrap();

        assert_eq!(parsed.active_provider, "home-ollama");
        assert_eq!(parsed.providers.len(), 1);
        let parsed_profile = &parsed.providers[0];
        assert_eq!(parsed_profile.name, "home-ollama");
        assert!(matches!(parsed_profile.config, ProviderSpecificConfig::Ollama(_)));
        assert_eq!(parsed_profile.config.model(), "llama3");
        assert_eq!(parsed_profile.config.base_url(), Some("http://localhost:11434"));
    }

    #[test]
    fn openai_profile_with_api_key_deserializes_as_openai_not_openrouter() {
        let toml_string = r#"
provider = "my-openai"

[[providers]]
name = "my-openai"
kind = "openai"
base_url = "https://api.openai.com/v1"
api_key = "sk-test"
model = "gpt-4"
"#;

        let config: Config = toml::from_str(toml_string).unwrap();
        let provider_config = config.provider_config().unwrap();
        assert_eq!(provider_config.provider_type(), ActiveProvider::OpenAI);
        match provider_config {
            ProviderSpecificConfig::OpenAI(config) => assert_eq!(config.api_key.as_deref(), Some("sk-test")),
            other => panic!("expected OpenAI config, got {other:?}"),
        }
    }

    #[test]
    fn provider_config_resolves_by_active_name() {
        let config = Config {
            active_provider: "home-ollama".to_string(),
            providers: vec![ollama_profile("home-ollama"), ollama_profile("office-ollama")],
            agent: AgentMode::Off,
            verbose_tool_output: true,
        };

        let provider_config = config.provider_config().unwrap();
        assert_eq!(provider_config.provider_type(), ActiveProvider::Ollama);
        assert_eq!(provider_config.model(), "llama3");
    }

    #[test]
    fn provider_config_errors_for_unknown_active_name() {
        let config = Config {
            active_provider: "missing".to_string(),
            providers: vec![ollama_profile("home-ollama")],
            agent: AgentMode::Off,
            verbose_tool_output: true,
        };

        match config.provider_config().unwrap_err() {
            LarpshellError::ConfigError(message) => {
                assert!(message.contains("missing"));
                assert!(message.contains("config not found"));
            }
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    #[test]
    fn unique_profile_name_appends_suffix_when_taken() {
        let profiles = vec![ollama_profile("ollama")];
        assert_eq!(unique_profile_name(ActiveProvider::Ollama, &profiles), "ollama-2");
        assert_eq!(unique_profile_name(ActiveProvider::Ollama, &[]), "ollama");
    }

    #[test]
    fn profile_menu_label_marks_current_with_plain_text() {
        let label = profile_menu_label_with_current(&ollama_profile("home-ollama"), "home-ollama");
        assert!(label.contains("(current)"));
        assert!(!label.contains('\u{1b}'), "labels must not bake in ANSI: {label:?}");
        let other = profile_menu_label_with_current(&ollama_profile("office-ollama"), "home-ollama");
        assert!(!other.contains("(current)"));
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
