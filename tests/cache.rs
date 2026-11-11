use super::{mock_ollama_scripted, run, run_with_stdin_interactive, temp_home, write_ollama_config};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

const CACHE_FILE: &str = "translation-cache.json";
// The contract pins the max age: entries older than this are never served.
const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

fn cache_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("config").join("larpshell").join(CACHE_FILE)
}

fn seed_cache(home: &std::path::Path, request: &str, command: &str, created_at: u64) {
    let dir = home.join("config").join("larpshell");
    fs::create_dir_all(&dir).unwrap();
    let payload = serde_json::json!({
        "version": 1,
        "entries": {
            (request): { "command": command, "created_at": created_at },
        }
    });
    fs::write(dir.join(CACHE_FILE), payload.to_string()).unwrap();
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

#[test]
fn cached_request_reaches_confirmation_when_provider_unreachable() {
    let home = temp_home("cache_offline_hit");
    seed_cache(&home, "show disk usage", "echo CACHED_CMD", now_secs());
    write_ollama_config(&home, 1);

    let out = run_with_stdin_interactive(&home, &["show disk usage"], b"y");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "a cache hit must serve a dead provider; stderr: {stderr}");
    assert!(stderr.contains("cache"), "the run must say the command came from the cache; stderr: {stderr}");
    assert!(stderr.contains("echo CACHED_CMD"), "the cached command must reach the confirmation prompt; stderr: {stderr}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("CACHED_CMD"),
        "accepting the cached command must execute it; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn expired_cache_entry_is_not_served_and_is_pruned_on_load() {
    let home = temp_home("cache_offline_expired");
    seed_cache(&home, "show disk usage", "echo STALE_CMD", now_secs() - MAX_AGE_SECS - 60);
    write_ollama_config(&home, 1);

    let out = run(&home, &["show disk usage"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "an expired entry must not serve a dead provider; stderr: {stderr}");
    assert!(!stderr.contains("echo STALE_CMD"), "the expired command must not surface; stderr: {stderr}");
    let file = fs::read_to_string(cache_path(&home)).unwrap();
    assert!(!file.contains("STALE_CMD"), "the expired entry must be pruned from the file on load; file: {file}");
}

#[test]
fn accepted_command_is_cached_under_its_request() {
    let home = temp_home("cache_write_on_accept");
    let (port, _requests) = mock_ollama_scripted(&["COMMAND: echo FRESH_CMD"]);
    write_ollama_config(&home, port);

    let out = run_with_stdin_interactive(&home, &["show disk usage"], b"y");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");

    let file = fs::read_to_string(cache_path(&home)).unwrap();
    assert!(file.contains("\"show disk usage\""), "the request text must key the cache; file: {file}");
    assert!(file.contains("echo FRESH_CMD"), "the accepted command must be cached; file: {file}");
}
