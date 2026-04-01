use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};

mod edit;
mod explain;
mod slash;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/larpshell")
}

fn run(home: &std::path::Path, args: &[&str]) -> std::process::Output {
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
