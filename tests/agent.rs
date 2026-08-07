use super::*;

fn clean_home(suffix: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/tests")
        .join(format!("larpshell_test_clean_{suffix}"));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_clean_home(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    ensure_binary_built();
    let config_dir = home.join(".config");
    std::process::Command::new(binary())
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run larpshell")
}

fn clean_home_config_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".config").join("larpshell").join("config.toml")
}

fn stderr_text(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout_text(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn assert_no_shell_bootstrap(out: &std::process::Output) {
    let stderr = stderr_text(out);
    assert!(
        !stderr.contains("restart shell or run 'source ~/.bashrc'"),
        "subcommand should not trigger shell bootstrap; stderr: {stderr}"
    );
}

fn assert_agent_mode_written(home: &std::path::Path, expected: &str) {
    let contents = fs::read_to_string(clean_home_config_path(home)).unwrap();
    assert!(
        contents.contains(expected),
        "config contents were: {contents}"
    );
}

fn assert_success(out: &std::process::Output) {
    assert!(
        out.status.success(),
        "expected success, stderr: {}",
        stderr_text(out)
    );
}

fn assert_file_contains(path: &std::path::Path, expected: &str) {
    let contents = fs::read_to_string(path).unwrap();
    assert!(
        contents.contains(expected),
        "file contents were: {contents}"
    );
}

fn assert_clean_home_agent_bootstrap(suffix: &str, args: &[&str], expected: &str) {
    let home = clean_home(suffix);
    let out = run_clean_home(&home, args);
    assert_success(&out);
    assert_agent_mode_written(&home, expected);
}

fn assert_prompt_show_uses_default(suffix: &str, args: &[&str], placeholder: &str) {
    let home = clean_home(suffix);
    let out = run_clean_home(&home, args);
    assert_success(&out);
    assert!(stdout_text(&out).contains(placeholder));
}

fn run_clean_home_with_editor(
    home: &std::path::Path,
    args: &[&str],
    editor: &std::path::Path,
) -> std::process::Output {
    ensure_binary_built();
    let config_dir = home.join(".config");
    std::process::Command::new(binary())
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("EDITOR", editor)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run larpshell")
}

fn agent_safe_prompt_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".config")
        .join("larpshell")
        .join("agent-safe-prompt.md")
}

fn make_noop_editor(home: &std::path::Path) -> std::path::PathBuf {
    let editor_script = home.join("fake-editor.sh");
    fs::write(&editor_script, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&editor_script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&editor_script, perms).unwrap();
    }
    editor_script
}

const SAFE_BOOTSTRAPPED: &str = "agent = \"safe\"";
const ON_BOOTSTRAPPED: &str = "agent = \"on\"";
const OFF_BOOTSTRAPPED: &str = "agent = \"off\"";

#[test]
fn agent_on_safe_off_subcommand_updates_config() {
    let home = temp_home("agent_toggle");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);

    let config_path = home.join("config").join("larpshell").join("config.toml");

    let out = run(&home, &["agent", "on"]);
    assert!(out.status.success());
    assert_file_contains(&config_path, "agent = \"on\"");

    let out = run(&home, &["agent", "safe"]);
    assert!(out.status.success());
    assert_file_contains(&config_path, "agent = \"safe\"");

    let out = run(&home, &["agent", "off"]);
    assert!(out.status.success());
    assert_file_contains(&config_path, "agent = \"off\"");
}

#[test]
fn agent_safe_subcommand_bootstraps_missing_config() {
    let home = temp_home("agent_safe_no_config");
    let config_path = home.join("config").join("larpshell").join("config.toml");

    let out = run(&home, &["agent", "safe"]);
    assert_success(&out);
    assert_file_contains(&config_path, "agent = \"safe\"");
}

#[test]
fn agent_safe_subcommand_bootstraps_missing_config_on_clean_home() {
    assert_clean_home_agent_bootstrap(
        "agent_safe_clean_home",
        &["agent", "safe"],
        SAFE_BOOTSTRAPPED,
    );
}

#[test]
fn agent_status_subcommand_does_not_require_shell_bootstrap() {
    let home = clean_home("agent_status_clean_home");

    let out = run_clean_home(&home, &["agent"]);
    assert_success(&out);
    assert_no_shell_bootstrap(&out);
}

#[test]
fn prompt_agent_safe_show_uses_default_on_clean_home() {
    let home = clean_home("prompt_agent_safe_clean_home");

    let out = run_clean_home(&home, &["prompt", "agent-safe", "show"]);
    assert_success(&out);
    assert!(!stdout_text(&out).contains("{request}"));
    assert_no_shell_bootstrap(&out);
}

#[test]
fn prompt_agent_safe_edit_creates_prompt_file_on_clean_home() {
    let home = clean_home("prompt_agent_safe_edit_clean_home");
    let editor_script = make_noop_editor(&home);

    let out = run_clean_home_with_editor(&home, &["prompt", "agent-safe", "edit"], &editor_script);
    assert_success(&out);

    let contents = fs::read_to_string(agent_safe_prompt_path(&home)).unwrap();
    assert!(!contents.contains("{request}"));
    assert!(contents.contains("safe, read-only tools"));
}

#[test]
fn agent_on_subcommand_bootstraps_missing_config_on_clean_home() {
    assert_clean_home_agent_bootstrap("agent_on_clean_home", &["agent", "on"], ON_BOOTSTRAPPED);
}

#[test]
fn agent_off_subcommand_bootstraps_missing_config_on_clean_home() {
    assert_clean_home_agent_bootstrap("agent_off_clean_home", &["agent", "off"], OFF_BOOTSTRAPPED);
}

#[test]
fn prompt_agent_show_uses_default_on_clean_home() {
    assert_prompt_show_uses_default(
        "prompt_agent_clean_home",
        &["prompt", "agent", "show"],
        "interacting with the user's machine",
    );
}

#[test]
fn prompt_system_show_uses_default_on_clean_home() {
    assert_prompt_show_uses_default(
        "prompt_system_clean_home",
        &["prompt", "system", "show"],
        "{request}",
    );
}

#[test]
fn prompt_explain_show_uses_default_on_clean_home() {
    assert_prompt_show_uses_default(
        "prompt_explain_clean_home",
        &["prompt", "explain", "show"],
        "{command}",
    );
}

#[test]
fn history_status_commands_still_work_with_shell_bootstrap_home() {
    let home = temp_home("history_control");

    let out = run(&home, &["history", "on"]);
    assert!(out.status.success());

    let out = run(&home, &["history", "off"]);
    assert!(out.status.success());
}

#[test]
fn tool_output_subcommand_updates_config() {
    let home = temp_home("tool_output_control");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);

    let config_path = home.join("config").join("larpshell").join("config.toml");

    let out = run(&home, &["verbose", "off"]);
    assert_success(&out);
    assert_file_contains(&config_path, "verbose_tool_output = false");

    let out = run(&home, &["verbose", "on"]);
    assert_success(&out);
    assert_file_contains(&config_path, "verbose_tool_output = true");
}

#[test]
fn verbose_slash_command_takes_effect_immediately() {
    let home = temp_home("verbose_slash");
    let port = mock_ollama(&[
        r#"CHAT_JSON:{"message":{"content":"","tool_calls":[{"function":{"name":"search_files","arguments":{"pattern":"Cargo","directory_path":"."}}}]}}"#,
        "MESSAGE: done",
    ]);
    write_ollama_config(&home, port);

    let out = run_with_stdin_interactive(
        &home,
        &[],
        b"/agent safe\n/verbose off\nfind Cargo\n/quit\n",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("verbose tool output: off"),
        "expected verbose off message; stderr: {stderr}"
    );
    assert!(
        stderr.contains("result ("),
        "expected tool summary; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("Cargo.toml"),
        "verbose output should not show result lines after /verbose off; stderr: {stderr}"
    );

    let config_path = home.join("config").join("larpshell").join("config.toml");
    assert_file_contains(&config_path, "verbose_tool_output = false");
}

#[test]
fn agent_slash_command_parsed_in_interactive() {
    let home = temp_home("agent_slash");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);

    let out = run_with_stdin(&home, &[], b"/agent safe\n/quit\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("agent mode: safe"),
        "expected agent safe message; stderr: {stderr}"
    );

    let config_path = home.join("config").join("larpshell").join("config.toml");
    assert_file_contains(&config_path, "agent = \"safe\"");
}

#[test]
fn agent_multiline_message_does_not_indent_continuation_lines() {
    let home = temp_home("agent_multiline_message");
    let port = mock_ollama(&["MESSAGE: package needed by:
larpshell
COMMAND: echo done"]);
    let config_path = home.join("config").join("larpshell").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        &config_path,
        format!(
            "provider = \"ollama\"\nagent = \"safe\"\n\n[providers.ollama]\nbase_url = \"http://127.0.0.1:{port}\"\nmodel = \"test\"\n"
        ),
    )
    .unwrap();

    let out = run_with_stdin(&home, &[], b"install package\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("● package needed by:\nlarpshell"),
        "expected multiline message without indent; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("● package needed by:\n  larpshell"),
        "message continuation line should not be indented; stderr: {stderr}"
    );
}

#[test]
fn agent_off_by_default_in_config() {
    let home = temp_home("agent_default");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);

    let config_path = home.join("config").join("larpshell").join("config.toml");
    let contents = fs::read_to_string(&config_path).unwrap();
    assert!(
        !contents.contains("agent = \"on\"") && !contents.contains("agent = \"safe\""),
        "fresh config should not have agent enabled"
    );
}
