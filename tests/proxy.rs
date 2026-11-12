use super::{run, temp_home};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// One-shot forward proxy: records the first request line, then answers with a
/// valid openai-compatible chat completion so the run completes. Nothing listens
/// on the provider port the config points at, so a successful run is only
/// reachable through this proxy.
fn mock_proxy(body: &'static str) -> (u16, Arc<Mutex<Option<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(None));
    let seen_clone = seen.clone();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = vec![0u8; 8192];
        let read = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..read]);
        *seen_clone.lock().unwrap() = Some(request.lines().next().unwrap_or("").to_string());
        let http = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(http.as_bytes());
    });
    (port, seen)
}

fn write_proxy_config(home: &std::path::Path, proxy_port: u16, target_port: u16) {
    let config_dir = home.join("config").join("larpshell");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        format!(
            "provider = \"openai\"\nproxy = \"http://127.0.0.1:{proxy_port}\"\n\n[[providers]]\nname = \"openai\"\nkind = \"openai\"\nbase_url = \"http://127.0.0.1:{target_port}/v1\"\nmodel = \"test\"\n"
        ),
    )
    .unwrap();
}

fn run_with_env(home: &std::path::Path, args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    super::ensure_binary_built();
    super::spawn_with_retry(|| {
        let mut command = std::process::Command::new(super::binary());
        command.args(args).env("HOME", home).env("XDG_CONFIG_HOME", home.join("config")).env("NO_COLOR", "1");
        for (key, value) in envs {
            command.env(key, value);
        }
        command.output()
    })
}

#[test]
fn proxy_config_field_routes_provider_requests_through_the_override() {
    let home = temp_home("proxy_override");
    let body: &'static str = r#"{"choices":[{"message":{"content":"COMMAND: echo PROXIED"}}]}"#;
    let (proxy_port, seen) = mock_proxy(body);
    write_proxy_config(&home, proxy_port, 1);

    let out = run(&home, &["show disk usage"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the provider request must ride the configured proxy; stderr: {stderr}");
    let request = seen.lock().unwrap().clone().expect("the mock proxy must see the provider request");
    assert!(request.contains("127.0.0.1:1/"), "the proxied request must target the provider base URL; request line: {request}");
}

#[test]
fn proxy_config_override_wins_over_the_system_proxy_env() {
    let home = temp_home("proxy_env_override");
    let body: &'static str = r#"{"choices":[{"message":{"content":"COMMAND: echo PROXIED"}}]}"#;
    let (proxy_port, seen) = mock_proxy(body);
    write_proxy_config(&home, proxy_port, 1);

    // The system proxy env points at a dead port; the config field must win.
    let out = run_with_env(&home, &["show disk usage"], &[("HTTP_PROXY", "http://127.0.0.1:1"), ("http_proxy", "http://127.0.0.1:1")]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the config proxy must override the system proxy env; stderr: {stderr}");
    let request = seen.lock().unwrap().clone().expect("the mock proxy must see the provider request");
    assert!(request.contains("127.0.0.1:1/"), "the proxied request must target the provider base URL; request line: {request}");
}

#[test]
fn proxy_field_survives_a_config_save() {
    let home = temp_home("proxy_save_roundtrip");
    let config_dir = home.join("config").join("larpshell");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"provider = "home-ollama"
proxy = "http://127.0.0.1:9"

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

    let out = run(&home, &["provider", "office-ollama"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "provider switch must exit 0; stderr: {stderr}");

    let contents = fs::read_to_string(config_dir.join("config.toml")).unwrap();
    assert!(contents.contains("proxy = \"http://127.0.0.1:9\""), "a config save must not drop the proxy field; config: {contents}");
}
