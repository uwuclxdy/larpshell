use crate::config::{ActiveProvider, AgentMode, Config, ProviderSpecificConfig};
use crate::error::LarpshellError;
use crate::providers::create_provider;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;

static BUILD_ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
use toml::from_str;

mod agent;
mod cli;
mod confirmation;
mod edit;
mod error;
mod explain;
mod interactive;
mod slash;

static TARGET_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

// `CARGO_TARGET_DIR` or `build.target-dir` can relocate artifacts off
// `CARGO_MANIFEST_DIR/target`, so resolve the real location via `cargo metadata`.
fn target_dir() -> PathBuf {
    TARGET_DIR
        .get_or_init(|| {
            let output = Command::new("cargo")
                .args(["metadata", "--format-version", "1", "--no-deps"])
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .output()
                .expect("failed to run `cargo metadata`");
            assert!(output.status.success(), "`cargo metadata` failed: {}", String::from_utf8_lossy(&output.stderr));
            let meta: serde_json::Value = serde_json::from_slice(&output.stdout).expect("failed to parse `cargo metadata`");
            meta["target_directory"].as_str().map(PathBuf::from).expect("`cargo metadata` is missing `target_directory`")
        })
        .clone()
}

fn binary() -> PathBuf {
    if let Some(path) = TEST_BINARY_OVERRIDE.lock().unwrap().clone() {
        return PathBuf::from(path);
    }

    if let Some(path) = option_env!("CARGO_BIN_EXE_larpshell") {
        return PathBuf::from(path);
    }

    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    target_dir().join(profile).join("larpshell")
}

fn ensure_binary_built() {
    if TEST_BINARY_OVERRIDE.lock().unwrap().is_some() || option_env!("CARGO_BIN_EXE_larpshell").is_some() {
        return;
    }

    BUILD_ONCE.get_or_init(|| {
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--bin", "larpshell"]);
        if !cfg!(debug_assertions) {
            cmd.arg("--release");
        }
        let status = cmd.current_dir(env!("CARGO_MANIFEST_DIR")).status().expect("failed to build larpshell test binary");
        assert!(status.success(), "cargo build --bin larpshell failed");
    });
}

static TEST_BINARY_OVERRIDE: Mutex<Option<OsString>> = Mutex::new(None);

struct TestBinaryOverrideGuard {
    original: Option<OsString>,
}

impl TestBinaryOverrideGuard {
    fn set(path: &str) -> Self {
        let mut override_path = TEST_BINARY_OVERRIDE.lock().unwrap();
        let original = override_path.replace(OsString::from(path));
        drop(override_path);
        Self { original }
    }
}

impl Drop for TestBinaryOverrideGuard {
    fn drop(&mut self) {
        *TEST_BINARY_OVERRIDE.lock().unwrap() = self.original.take();
    }
}

#[test]
fn binary_prefers_explicit_test_override() {
    let _guard = TestBinaryOverrideGuard::set("/tmp/larpshell-test-bin");
    assert_eq!(binary(), PathBuf::from("/tmp/larpshell-test-bin"));
}

#[test]
fn ensure_binary_built_skips_nested_cargo_when_override_is_present() {
    let _guard = TestBinaryOverrideGuard::set("/tmp/larpshell-test-bin");
    ensure_binary_built();
}

fn run(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    ensure_binary_built();
    Command::new(binary())
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run larpshell")
}

fn temp_home(suffix: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tests").join(format!("larpshell_test_{suffix}"));
    // Pre-create the bash completion marker so auto_setup_shell_function sees it as
    // already installed and returns Ok(false) instead of exiting the process early.
    let completion_dir = dir.join(".local/share/bash-completion/completions");
    fs::create_dir_all(&completion_dir).unwrap();
    fs::write(completion_dir.join("larpshell"), "").unwrap();
    dir
}

// ── mock-server helpers ──────────────────────────────────────────────────────

/// Starts a local TCP server that responds to each accepted connection with the
/// next Ollama-format JSON response from `responses`, then closes the socket.
/// Returns the bound port so tests can point the config at it.
fn mock_ollama(responses: &[&str]) -> u16 {
    use std::io::Read;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let responses: Vec<String> = responses.iter().map(std::string::ToString::to_string).collect();

    std::thread::spawn(move || {
        for text in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = vec![0u8; 8192];
            let read = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..read]);
            let body = if let Some(raw_json) = text.strip_prefix("CHAT_JSON:") {
                raw_json.to_string()
            } else if request.starts_with("POST /api/chat ") {
                serde_json::json!({
                    "message": {
                        "content": text,
                    }
                })
                .to_string()
            } else {
                serde_json::json!({ "response": text }).to_string()
            };
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(http.as_bytes());
        }
    });

    port
}

fn write_ollama_config(home: &std::path::Path, port: u16) {
    let config_dir = home.join("config").join("larpshell");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        format!("provider = \"ollama\"\n\n[providers.ollama]\nbase_url = \"http://127.0.0.1:{port}\"\nmodel = \"test\"\n"),
    )
    .unwrap();
}

/// Like `run_with_stdin`, but sets `LARPSHELL_FORCE_INTERACTIVE=1` so that confirmation
/// prompts and the edit UI actually read from the piped stdin instead of being
/// auto-approved. The piped bytes represent raw key presses (Y/N/Enter/escape
/// sequences for arrow keys, etc.).
fn run_with_stdin_interactive(home: &std::path::Path, args: &[&str], stdin_data: &[u8]) -> std::process::Output {
    ensure_binary_built();
    let mut child = Command::new(binary())
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("NO_COLOR", "1")
        .env("LARPSHELL_FORCE_INTERACTIVE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn larpshell");

    let bytes = stdin_data.to_vec();
    let mut pipe = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        let _ = pipe.write_all(&bytes);
    });

    child.wait_with_output().expect("failed to wait for larpshell")
}

/// Like `run`, but feeds `stdin_data` into the binary's stdin before closing it.
fn run_with_stdin(home: &std::path::Path, args: &[&str], stdin_data: &[u8]) -> std::process::Output {
    ensure_binary_built();
    let mut child = Command::new(binary())
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn larpshell");

    let bytes = stdin_data.to_vec();
    let mut pipe = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        let _ = pipe.write_all(&bytes);
        // dropping `pipe` closes stdin → binary sees EOF
    });

    child.wait_with_output().expect("failed to wait for larpshell")
}

// ── history subcommand tests ─────────────────────────────────────────────────

#[test]
fn history_on_prints_confirmation() {
    let home = temp_home("history_on");
    let out = run(&home, &["history", "on"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "history on should exit 0; stderr: {stderr}");
    assert!(
        stderr.contains("history") && (stderr.contains("on") || stderr.contains("enabled")),
        "expected confirmation message; stderr: {stderr}"
    );
    let disabled_flag = home.join("config").join("larpshell").join(".history-disabled");
    assert!(!disabled_flag.exists(), ".history-disabled must not exist after 'history on'");
}

#[test]
fn history_off_creates_disabled_flag_and_prints_confirmation() {
    let home = temp_home("history_off");

    let out = run(&home, &["history", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "history off should exit 0; stderr: {stderr}");
    assert!(
        stderr.contains("history") && (stderr.contains("off") || stderr.contains("disabled")),
        "expected confirmation message; stderr: {stderr}"
    );
    let disabled_flag = home.join("config").join("larpshell").join(".history-disabled");
    assert!(disabled_flag.exists(), ".history-disabled must exist after 'history off'");
}

#[test]
fn history_enabled_by_default() {
    let home = temp_home("history_default");
    let disabled_flag = home.join("config").join("larpshell").join(".history-disabled");
    assert!(!disabled_flag.exists(), ".history-disabled must not exist in a fresh home");
}

// ── provider config tests ───────────────────────────────────────────────────

#[test]
fn openrouter_config_parsing_and_resolution_succeeds() {
    let config_toml = r#"
provider = "openrouter"

[[providers]]
name = "openrouter"
kind = "openrouter"
base_url = "https://openrouter.ai/api/v1"
api_key = "test-openrouter-key"
model = "openrouter/auto"
"#;

    let config: Config = from_str(config_toml).expect("openrouter TOML should parse");
    assert_eq!(config.active_provider, "openrouter");

    let provider_config = config.provider_config().expect("openrouter provider config should resolve");

    assert_eq!(provider_config.provider_type(), ActiveProvider::OpenRouter);
    match provider_config {
        ProviderSpecificConfig::OpenRouter(openrouter) => {
            assert_eq!(openrouter.base_url, "https://openrouter.ai/api/v1");
            assert_eq!(openrouter.api_key.as_deref(), Some("test-openrouter-key"));
            assert_eq!(openrouter.model, "openrouter/auto");
        }
        other => panic!("expected OpenRouter config, got {other:?}"),
    }
}

#[test]
fn anthropic_config_parsing_and_resolution_succeeds() {
    let config_toml = r#"
provider = "anthropic"

[[providers]]
name = "anthropic"
kind = "anthropic"
api_key = "sk-ant-test"
model = "claude-sonnet-4-6"
"#;

    let config: Config = from_str(config_toml).expect("anthropic TOML should parse");
    assert_eq!(config.active_provider, "anthropic");

    let provider_config = config.provider_config().expect("anthropic provider config should resolve");

    assert_eq!(provider_config.provider_type(), ActiveProvider::Anthropic);
    match provider_config {
        ProviderSpecificConfig::Anthropic(anthropic) => {
            assert_eq!(anthropic.api_key, "sk-ant-test");
            assert_eq!(anthropic.model, "claude-sonnet-4-6");
        }
        other => panic!("expected Anthropic config, got {other:?}"),
    }
}

#[test]
fn lmstudio_config_parsing_and_resolution_succeeds() {
    let config_toml = r#"
provider = "lmstudio"

[[providers]]
name = "lmstudio"
kind = "lmstudio"
base_url = "http://localhost:1234"
model = "llama-3.2-1b"
"#;

    let config: Config = from_str(config_toml).expect("lmstudio TOML should parse");
    assert_eq!(config.active_provider, "lmstudio");

    let provider_config = config.provider_config().expect("lmstudio provider config should resolve");

    assert_eq!(provider_config.provider_type(), ActiveProvider::LMStudio);
    match provider_config {
        ProviderSpecificConfig::LMStudio(lmstudio) => {
            assert_eq!(lmstudio.base_url, "http://localhost:1234");
            assert_eq!(lmstudio.model, "llama-3.2-1b");
        }
        other => panic!("expected LMStudio config, got {other:?}"),
    }
}

#[test]
fn create_provider_with_anthropic_config_reports_anthropic_name() {
    let config_toml = r#"
provider = "anthropic"

[[providers]]
name = "anthropic"
kind = "anthropic"
api_key = "sk-ant-test"
model = "claude-sonnet-4-6"
"#;

    let config: Config = from_str(config_toml).expect("anthropic TOML should parse");
    let provider = create_provider(&config).expect("anthropic provider should be created");

    assert!(provider.name().contains("Anthropic"), "expected provider name to identify Anthropic, got {}", provider.name());
}

#[test]
fn create_provider_with_lmstudio_config_reports_lm_studio_name() {
    let config_toml = r#"
provider = "lmstudio"

[[providers]]
name = "lmstudio"
kind = "lmstudio"
base_url = "http://localhost:1234"
model = "llama-3.2-1b"
"#;

    let config: Config = from_str(config_toml).expect("lmstudio TOML should parse");
    let provider = create_provider(&config).expect("lmstudio provider should be created");

    assert!(provider.name().contains("LM Studio"), "expected provider name to identify LM Studio, got {}", provider.name());
}

#[test]
fn openrouter_missing_provider_config_returns_config_error() {
    let config_toml = r#"
provider = "openrouter"
"#;

    let config: Config = from_str(config_toml).expect("openrouter provider enum should parse");
    let error = config.provider_config().expect_err("missing openrouter config should return an error");

    match error {
        LarpshellError::ConfigError(message) => {
            assert!(message.contains("openrouter"));
            assert!(message.contains("config not found"));
        }
        other => panic!("expected config error, got {other:?}"),
    }
}

#[test]
fn create_provider_with_openrouter_config_reports_openrouter_name() {
    let config_toml = r#"
provider = "openrouter"

[[providers]]
name = "openrouter"
kind = "openrouter"
base_url = "https://openrouter.ai/api/v1"
api_key = "test-openrouter-key"
model = "openrouter/auto"
"#;

    let config: Config = from_str(config_toml).expect("openrouter TOML should parse");
    let provider = create_provider(&config).expect("openrouter provider should be created when support is implemented");

    assert!(provider.name().contains("OpenRouter"), "expected provider name to identify OpenRouter, got {}", provider.name());
}

// ── provider switch subcommand tests ────────────────────────────────────────

fn write_two_profile_config(home: &std::path::Path) {
    let config_dir = home.join("config").join("larpshell");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"provider = "home-ollama"

[[providers]]
name = "home-ollama"
kind = "ollama"
base_url = "http://127.0.0.1:11434"
model = "llama3"

[[providers]]
name = "office-ollama"
kind = "ollama"
base_url = "http://192.168.1.50:11434"
model = "llama3"
"#,
    )
    .unwrap();
}

#[test]
fn provider_switch_by_name_updates_config() {
    let home = temp_home("provider_switch");
    write_two_profile_config(&home);

    let out = run(&home, &["provider", "office-ollama"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "provider switch should exit 0; stderr: {stderr}");
    assert!(stderr.contains("office-ollama"), "expected confirmation; stderr: {stderr}");

    let config_path = home.join("config").join("larpshell").join("config.toml");
    let contents = fs::read_to_string(config_path).unwrap();
    assert!(contents.contains("provider = \"office-ollama\""), "config should have switched provider: {contents}");
    assert!(contents.contains("name = \"home-ollama\""), "switching must not drop the sibling profile; config: {contents}");
}

#[test]
fn provider_switch_unknown_name_errors() {
    let home = temp_home("provider_switch_unknown");
    write_two_profile_config(&home);

    let out = run(&home, &["provider", "nope"]);
    assert!(!out.status.success(), "unknown provider name should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no provider named"), "expected lookup error; stderr: {stderr}");
}

// ── openai-compatible provider tests ────────────────────────────────────────

/// Starts a one-shot server answering with `status` + `body` (raw JSON for the
/// response payload) and recording the first request line.
fn mock_openai_with_status(status: u16, body: &'static str) -> (u16, std::sync::Arc<std::sync::Mutex<Option<String>>>) {
    use std::io::Read;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));

    let seen_clone = seen.clone();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = vec![0u8; 8192];
        let read = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..read]);
        let request_line = request.lines().next().unwrap_or("").to_string();
        *seen_clone.lock().unwrap() = Some(request_line);

        let reason = if status == 200 { "OK" } else { "Error" };
        let http = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(http.as_bytes());
    });

    (port, seen)
}

fn write_openai_config(home: &std::path::Path, port: u16) {
    let config_dir = home.join("config").join("larpshell");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        format!(
            "provider = \"openai\"\n\n[[providers]]\nname = \"openai\"\nkind = \"openai\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\nmodel = \"gpt-test\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn openai_compat_generation_hits_v1_chat_completions_path() {
    let home = temp_home("openai_path");
    let (port, seen) = mock_openai_with_status(200, r#"{"choices":[{"message":{"content":"COMMAND: ls -la"}}]}"#);
    write_openai_config(&home, port);

    let out = run(&home, &["show disk usage"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "generation should succeed; stderr: {stderr}");

    let request_line = seen.lock().unwrap().clone().expect("mock should have seen a request");
    assert!(request_line.starts_with("POST /v1/chat/completions "), "base URL /v1 must survive URL joining; got {request_line:?}");
}

#[test]
fn openai_compat_401_prints_auth_failed() {
    let home = temp_home("openai_401");
    let (port, _seen) = mock_openai_with_status(401, r#"{"error":{"message":"invalid api key"}}"#);
    write_openai_config(&home, port);

    let out = run(&home, &["show disk usage"]);
    assert!(!out.status.success(), "401 should fail the run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("auth failed") && stderr.contains("invalid API key"), "expected friendly auth failure; stderr: {stderr}");
}

// ── error formatting tests ──────────────────────────────────────────────────

#[test]
fn connection_failure_prints_pretty_error() {
    let home = temp_home("conn_fail");
    // write config pointing at port unlikely to be open
    write_ollama_config(&home, 1);

    let out = run(&home, &["show disk usage"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    eprintln!("stderr for debug: {stderr:?}");
    // should include the styled prefix and the friendly message
    assert!(stderr.contains("error:") && stderr.contains("failed to connect"));
}

// ── agent config tests ─────────────────────────────────────────────────────

#[test]
fn agent_field_defaults_to_off() {
    let config_toml = r#"
provider = "ollama"

[[providers]]
name = "ollama"
kind = "ollama"
base_url = "http://localhost:11434"
model = "test"
"#;
    let config: Config = from_str(config_toml).expect("should parse without agent field");
    assert_eq!(config.agent, AgentMode::Off);
    assert!(config.verbose_tool_output);
}

#[test]
fn agent_field_parses_mode_values() {
    for (value, expected) in [("off", AgentMode::Off), ("safe", AgentMode::Safe), ("on", AgentMode::On)] {
        let config_toml = format!(
            "provider = \"ollama\"\nagent = \"{value}\"\n\n[[providers]]\nname = \"ollama\"\nkind = \"ollama\"\nbase_url = \"http://localhost:11434\"\nmodel = \"test\"\n"
        );
        let config: Config = from_str(&config_toml).expect("should parse agent mode");
        assert_eq!(config.agent, expected);
    }
}

#[test]
fn agent_field_parses_legacy_bool_values() {
    for (value, expected) in [("false", AgentMode::Off), ("true", AgentMode::On)] {
        let config_toml = format!(
            "provider = \"ollama\"\nagent = {value}\n\n[[providers]]\nname = \"ollama\"\nkind = \"ollama\"\nbase_url = \"http://localhost:11434\"\nmodel = \"test\"\n"
        );
        let config: Config = from_str(&config_toml).expect("should parse legacy bool agent");
        assert_eq!(config.agent, expected);
    }
}

#[test]
fn verbose_tool_output_field_parses_false() {
    let config_toml = r#"
provider = "ollama"
verbose_tool_output = false

[[providers]]
name = "ollama"
kind = "ollama"
base_url = "http://localhost:11434"
model = "test"
"#;
    let config: Config = from_str(config_toml).expect("should parse verbose tool output");
    assert!(!config.verbose_tool_output);
}

#[test]
fn agent_subcommand_on_prints_confirmation() {
    let home = temp_home("agent_on");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);
    let out = run(&home, &["agent", "on"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "agent on should exit 0; stderr: {stderr}");
    assert!(stderr.contains("agent mode: on"), "expected confirmation message; stderr: {stderr}");
    let config_path = home.join("config").join("larpshell").join("config.toml");
    let contents = fs::read_to_string(config_path).unwrap();
    assert!(contents.contains("agent = \"on\""), "config should have agent = \"on\"");
}

#[test]
fn agent_subcommand_safe_prints_confirmation() {
    let home = temp_home("agent_safe");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);
    let out = run(&home, &["agent", "safe"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "agent safe should exit 0; stderr: {stderr}");
    assert!(stderr.contains("agent mode: safe"), "expected confirmation message; stderr: {stderr}");
    let config_path = home.join("config").join("larpshell").join("config.toml");
    let contents = fs::read_to_string(config_path).unwrap();
    assert!(contents.contains("agent = \"safe\""), "config should have agent = \"safe\"");
}

#[test]
fn agent_subcommand_off_prints_confirmation() {
    let home = temp_home("agent_off");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);
    run(&home, &["agent", "on"]);
    let out = run(&home, &["agent", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "agent off should exit 0; stderr: {stderr}");
    assert!(stderr.contains("agent mode: off"), "expected confirmation message; stderr: {stderr}");
    let config_path = home.join("config").join("larpshell").join("config.toml");
    let contents = fs::read_to_string(config_path).unwrap();
    assert!(contents.contains("agent = \"off\""), "config should have agent = \"off\"");
}

#[test]
fn atomic_write_produces_correct_content_and_no_temp_file() {
    use crate::config::atomic_write;
    // Use a unique subdir so parallel test runs don't collide.
    let dir = std::env::temp_dir().join(format!("larpshell_test_{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join("config.toml");
    let content = "provider = \"ollama\"\n";

    atomic_write(&target, content).expect("atomic_write should succeed");

    assert_eq!(fs::read_to_string(&target).unwrap(), content, "target should contain what was written");
    assert!(!dir.join("config.tmp").exists(), "temp file must not be left behind after a successful write");
    let _ = fs::remove_dir_all(&dir);
}
