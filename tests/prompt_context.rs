use super::*;

/// Runs the binary with its working directory set to `dir`, so repository and
/// directory context the binary collects come from `dir`, not the test runner's.
fn run_in_dir(home: &std::path::Path, dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    ensure_binary_built();
    spawn_with_retry(|| {
        Command::new(binary())
            .args(args)
            .current_dir(dir)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("NO_COLOR", "1")
            .output()
    })
}

/// A fresh git repo holding one committed `Cargo.toml`, on branch `main`.
fn git_repo_with_cargo_toml(suffix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("larpshell_prompt_ctx_{suffix}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n").unwrap();

    let git = |args: &[&str]| {
        let output = Command::new("git").args(args).current_dir(&dir).env("HOME", &dir).output().expect("git should be runnable");
        assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    };
    git(&["init", "-b", "main"]);
    git(&["-c", "user.email=test@example.com", "-c", "user.name=test", "add", "."]);
    git(&["-c", "user.email=test@example.com", "-c", "user.name=test", "commit", "-m", "fixture initial commit"]);
    dir
}

fn plain_dir(suffix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("larpshell_prompt_ctx_{suffix}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn captured_request(requests: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> String {
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "expected exactly one provider request; got {}", requests.len());
    requests[0].clone()
}

#[test]
fn system_prompt_carries_git_project_and_dir_context_inside_a_repo() {
    let dir = git_repo_with_cargo_toml("in_repo");
    let home = temp_home("prompt_ctx_repo");
    let (port, requests) = mock_ollama_scripted(&["COMMAND: echo hi"]);
    write_ollama_config(&home, port);

    let out = run_in_dir(&home, &dir, &["show disk usage"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "generation should succeed; stderr: {stderr}");

    let body = captured_request(&requests);
    assert!(body.contains("Branch: main"), "git branch missing from the system prompt");
    assert!(body.contains("fixture initial commit"), "recent commits missing from the system prompt");
    assert!(body.contains("Project type: rust"), "Cargo.toml must be detected as project type rust");
    assert!(body.contains("Cargo.toml"), "the directory summary must list cwd entries");
    assert!(
        !body.contains("{git_context}") && !body.contains("{project_type}") && !body.contains("{dir_summary}"),
        "every context placeholder must be substituted, not left verbatim"
    );
}

#[test]
fn system_prompt_renders_git_context_empty_and_project_type_none_outside_a_repo() {
    let dir = plain_dir("no_repo");
    fs::write(dir.join("marker.txt"), "x").unwrap();
    let home = temp_home("prompt_ctx_norepo");
    let (port, requests) = mock_ollama_scripted(&["COMMAND: echo hi"]);
    write_ollama_config(&home, port);

    let out = run_in_dir(&home, &dir, &["show disk usage"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "generation should succeed outside a repo; stderr: {stderr}");

    let body = captured_request(&requests);
    assert!(!body.contains("Branch: "), "no git context may render outside a repo");
    // Residual this test cannot control: the project-type walk reads temp_dir's ancestors, an open set — a marker file (Cargo.toml, package.json, …) at any of them reds the none assert with no code change.
    assert!(body.contains("Project type: none"), "no project markers means project type none");
    assert!(body.contains("marker.txt"), "the directory summary should list cwd entries");
    assert!(!body.contains("{git_context}"), "the placeholder must be substituted even when empty");
}

#[test]
fn system_prompt_bounds_the_directory_summary() {
    let dir = plain_dir("dir_bound");
    for index in 0..40 {
        fs::write(dir.join(format!("file{index:02}.txt")), "x").unwrap();
    }
    let home = temp_home("prompt_ctx_bound");
    let (port, requests) = mock_ollama_scripted(&["COMMAND: echo hi"]);
    write_ollama_config(&home, port);

    let out = run_in_dir(&home, &dir, &["show disk usage"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "generation should succeed; stderr: {stderr}");

    let body = captured_request(&requests);
    assert!(body.contains("file00.txt"), "the listing must include entries from the top");
    assert!(body.contains("file29.txt"), "the listing must carry a meaningful number of entries");
    assert!(!body.contains("file39.txt"), "a 40-entry directory must be capped, not listed whole");
    assert!(body.contains("and 10 more"), "the tail must name the 10 entries beyond the 30 shown");
    assert!(!body.contains("{dir_summary}"), "the placeholder must be substituted");
}
