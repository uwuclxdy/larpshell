use crate::config::{ActiveProvider, AgentMode, Config, ProviderSpecificConfig};
use crate::error::LarpshellError;
use crate::providers::create_provider;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use toml::from_str;

mod agent;
mod edit;
mod explain;
mod slash;

fn binary() -> PathBuf {
    if let Some(path) = TEST_BINARY_OVERRIDE.lock().unwrap().clone() {
        return PathBuf::from(path);
    }

    if let Some(path) = option_env!("CARGO_BIN_EXE_larpshell") {
        return PathBuf::from(path);
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/larpshell")
}

fn ensure_binary_built() {
    if TEST_BINARY_OVERRIDE.lock().unwrap().is_some()
        || option_env!("CARGO_BIN_EXE_larpshell").is_some()
    {
        return;
    }

    static BUILD_ONCE: OnceLock<()> = OnceLock::new();
    BUILD_ONCE.get_or_init(|| {
        let status = Command::new("cargo")
            .args(["build", "--bin", "larpshell"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("failed to build larpshell test binary");
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
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/tests")
        .join(format!("larpshell_test_{suffix}"));
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
    let responses: Vec<String> = responses.iter().map(|s| s.to_string()).collect();

    std::thread::spawn(move || {
        for text in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf);
            let body = format!(r#"{{"response":"{}"}}"#, text);
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
        format!(
            "provider = \"ollama\"\n\n[providers.ollama]\nbase_url = \"http://127.0.0.1:{port}\"\nmodel = \"test\"\n"
        ),
    )
    .unwrap();
}

/// Like `run_with_stdin`, but sets `LARPSHELL_FORCE_INTERACTIVE=1` so that confirmation
/// prompts and the edit UI actually read from the piped stdin instead of being
/// auto-approved. The piped bytes represent raw key presses (Y/N/Enter/escape
/// sequences for arrow keys, etc.).
fn run_with_stdin_interactive(
    home: &std::path::Path,
    args: &[&str],
    stdin_data: &[u8],
) -> std::process::Output {
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

    child
        .wait_with_output()
        .expect("failed to wait for larpshell")
}

/// Like `run`, but feeds `stdin_data` into the binary's stdin before closing it.
fn run_with_stdin(
    home: &std::path::Path,
    args: &[&str],
    stdin_data: &[u8],
) -> std::process::Output {
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

    child
        .wait_with_output()
        .expect("failed to wait for larpshell")
}

// ── history subcommand tests ─────────────────────────────────────────────────

#[test]
fn history_on_prints_confirmation() {
    let home = temp_home("history_on");
    let out = run(&home, &["history", "on"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "history on should exit 0; stderr: {stderr}"
    );
    assert!(
        stderr.contains("history") && (stderr.contains("on") || stderr.contains("enabled")),
        "expected confirmation message; stderr: {stderr}"
    );
    let disabled_flag = home
        .join("config")
        .join("larpshell")
        .join(".history-disabled");
    assert!(
        !disabled_flag.exists(),
        ".history-disabled must not exist after 'history on'"
    );
}

#[test]
fn history_off_creates_disabled_flag_and_prints_confirmation() {
    let home = temp_home("history_off");

    let out = run(&home, &["history", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "history off should exit 0; stderr: {stderr}"
    );
    assert!(
        stderr.contains("history") && (stderr.contains("off") || stderr.contains("disabled")),
        "expected confirmation message; stderr: {stderr}"
    );
    let disabled_flag = home
        .join("config")
        .join("larpshell")
        .join(".history-disabled");
    assert!(
        disabled_flag.exists(),
        ".history-disabled must exist after 'history off'"
    );
}

#[test]
fn history_enabled_by_default() {
    let home = temp_home("history_default");
    let disabled_flag = home
        .join("config")
        .join("larpshell")
        .join(".history-disabled");
    assert!(
        !disabled_flag.exists(),
        ".history-disabled must not exist in a fresh home"
    );
}

// ── provider config tests ───────────────────────────────────────────────────

#[test]
fn openrouter_config_parsing_and_resolution_succeeds() {
    let config_toml = r#"
provider = "openrouter"

[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
api_key = "test-openrouter-key"
model = "openrouter/auto"
"#;

    let config: Config = from_str(config_toml).expect("openrouter TOML should parse");
    assert_eq!(config.active_provider, ActiveProvider::OpenRouter);

    let provider_config = config
        .provider_config()
        .expect("openrouter provider config should resolve");

    assert_eq!(provider_config.provider_type, ActiveProvider::OpenRouter);
    match provider_config.config {
        ProviderSpecificConfig::OpenRouter { openrouter } => {
            assert_eq!(openrouter.base_url, "https://openrouter.ai/api/v1");
            assert_eq!(openrouter.api_key.as_deref(), Some("test-openrouter-key"));
            assert_eq!(openrouter.model, "openrouter/auto");
        }
        other => panic!("expected OpenRouter config, got {other:?}"),
    }
}

#[test]
fn openrouter_missing_provider_config_returns_config_error() {
    let config_toml = r#"
provider = "openrouter"
"#;

    let config: Config = from_str(config_toml).expect("openrouter provider enum should parse");
    let error = config
        .provider_config()
        .expect_err("missing openrouter config should return an error");

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

[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
api_key = "test-openrouter-key"
model = "openrouter/auto"
"#;

    let config: Config = from_str(config_toml).expect("openrouter TOML should parse");
    let provider = create_provider(&config)
        .expect("openrouter provider should be created when support is implemented");

    assert!(
        provider.name().contains("OpenRouter"),
        "expected provider name to identify OpenRouter, got {}",
        provider.name()
    );
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
    eprintln!("stderr for debug: {:?}", stderr);
    // should include the styled prefix and the friendly message
    assert!(stderr.contains("error:") && stderr.contains("failed to connect"));
}

// ── agent config tests ─────────────────────────────────────────────────────

#[test]
fn agent_field_defaults_to_off() {
    let config_toml = r#"
provider = "ollama"

[providers.ollama]
base_url = "http://localhost:11434"
model = "test"
"#;
    let config: Config = from_str(config_toml).expect("should parse without agent field");
    assert_eq!(config.agent, AgentMode::Off);
}

#[test]
fn agent_field_parses_mode_values() {
    for (value, expected) in [
        ("off", AgentMode::Off),
        ("safe", AgentMode::Safe),
        ("on", AgentMode::On),
    ] {
        let config_toml = format!(
            "provider = \"ollama\"\nagent = \"{value}\"\n\n[providers.ollama]\nbase_url = \"http://localhost:11434\"\nmodel = \"test\"\n"
        );
        let config: Config = from_str(&config_toml).expect("should parse agent mode");
        assert_eq!(config.agent, expected);
    }
}

#[test]
fn agent_field_parses_legacy_bool_values() {
    for (value, expected) in [("false", AgentMode::Off), ("true", AgentMode::On)] {
        let config_toml = format!(
            "provider = \"ollama\"\nagent = {value}\n\n[providers.ollama]\nbase_url = \"http://localhost:11434\"\nmodel = \"test\"\n"
        );
        let config: Config = from_str(&config_toml).expect("should parse legacy bool agent");
        assert_eq!(config.agent, expected);
    }
}

#[test]
fn agent_subcommand_on_prints_confirmation() {
    let home = temp_home("agent_on");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);
    let out = run(&home, &["agent", "on"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "agent on should exit 0; stderr: {stderr}"
    );
    assert!(
        stderr.contains("agent mode set to on"),
        "expected confirmation message; stderr: {stderr}"
    );
    let config_path = home.join("config").join("larpshell").join("config.toml");
    let contents = fs::read_to_string(config_path).unwrap();
    assert!(
        contents.contains("agent = \"on\""),
        "config should have agent = \"on\""
    );
}

#[test]
fn agent_subcommand_safe_prints_confirmation() {
    let home = temp_home("agent_safe");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);
    let out = run(&home, &["agent", "safe"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "agent safe should exit 0; stderr: {stderr}"
    );
    assert!(
        stderr.contains("agent mode set to safe"),
        "expected confirmation message; stderr: {stderr}"
    );
    let config_path = home.join("config").join("larpshell").join("config.toml");
    let contents = fs::read_to_string(config_path).unwrap();
    assert!(
        contents.contains("agent = \"safe\""),
        "config should have agent = \"safe\""
    );
}

#[test]
fn agent_subcommand_off_prints_confirmation() {
    let home = temp_home("agent_off");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);
    run(&home, &["agent", "on"]);
    let out = run(&home, &["agent", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "agent off should exit 0; stderr: {stderr}"
    );
    assert!(
        stderr.contains("agent mode disabled"),
        "expected confirmation message; stderr: {stderr}"
    );
    let config_path = home.join("config").join("larpshell").join("config.toml");
    let contents = fs::read_to_string(config_path).unwrap();
    assert!(
        contents.contains("agent = \"off\""),
        "config should have agent = \"off\""
    );
}
