use crate::tests;
use std::fs;

// `larpshell prompt explain show` → exits 0, stdout contains {command} placeholder.
#[test]
fn prompt_explain_show_prints_default() {
    let home = tests::temp_home("explain_show_default");
    let out = tests::run(&home, &["prompt", "explain", "show"]);

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("{command}"),
        "default explain prompt must contain {{command}}, got: {stdout}"
    );
}

// `larpshell prompt explain show` with a saved custom file → outputs the file contents.
#[test]
fn prompt_explain_show_uses_saved_custom_prompt() {
    let home = tests::temp_home("explain_show_custom");
    let config_dir = home.join("config").join("larpshell");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("explain-prompt.md"), "describe: {command}").unwrap();

    let out = tests::run(&home, &["prompt", "explain", "show"]);

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), "describe: {command}");
}

// `larpshell prompt system show` command.
#[test]
fn prompt_system_show() {
    let home = tests::temp_home("sys_show");
    let out = tests::run(&home, &["prompt", "system", "show"]);

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("{request}"),
        "system prompt must contain {{request}}, got: {stdout}"
    );
    assert!(
        !stdout.contains("{command}"),
        "system prompt must not contain {{command}}"
    );
}

// `larpshell prompt` (no args, both defaults) is equivalent to `larpshell prompt system show`.
#[test]
fn prompt_bare_equals_system_show() {
    let home = tests::temp_home("prompt_bare");
    let explicit = tests::run(&home, &["prompt", "system", "show"]);
    let bare = tests::run(&home, &["prompt"]);

    assert!(explicit.status.success());
    assert!(bare.status.success());
    assert_eq!(bare.stdout, explicit.stdout);
}

// `larpshell prompt explain show` and `larpshell prompt system show` produce different output.
#[test]
fn explain_and_system_prompts_differ() {
    let home = tests::temp_home("prompts_differ");
    let explain_out = tests::run(&home, &["prompt", "explain", "show"]);
    let system_out = tests::run(&home, &["prompt", "system", "show"]);

    assert_ne!(explain_out.stdout, system_out.stdout);
}

// `larpshell explain echo hi` with no provider configured → exits non-zero, reports the error.
#[test]
fn explain_subcommand_without_provider_exits_error() {
    let home = tests::temp_home("explain_no_provider");
    let out = tests::run(&home, &["explain", "echo", "hi"]);

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no API provider configured"),
        "stderr: {stderr}"
    );
}

// `larpshell explain` with no arguments → exits non-zero (no provider → config error first).
#[test]
fn explain_subcommand_no_args_exits_error() {
    let home = tests::temp_home("explain_no_args");
    let out = tests::run(&home, &["explain"]);

    assert!(!out.status.success());
}

// ── explain subcommand (single-run) ─────────────────────────────────────────

// `larpshell explain echo hello` → explanation text from mock appears in stderr.
#[test]
fn explain_subcommand_with_mock_shows_explanation_in_stderr() {
    let home = tests::temp_home("explain_mock_stderr");
    let port = tests::mock_ollama(&["SAFE: echoes hello to stdout"]);
    tests::write_ollama_config(&home, port);

    let out = tests::run(&home, &["explain", "echo", "hello"]);

    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("echoes hello to stdout"),
        "explanation not found in stderr: {stderr}"
    );
}

// ── main command flow ────────────────────────────────────────────────────────

// Single-run (`larpshell <request>`): mock returns a command → command is executed.
#[test]
fn single_run_with_mock_executes_command() {
    let home = tests::temp_home("single_run_mock");
    let port = tests::mock_ollama(&["echo from_mock"]);
    tests::write_ollama_config(&home, port);

    let out = tests::run(&home, &["list files in current directory"]);

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "echo from_mock"
    );
}

// Interactive mode (piped stdin): mock returns a command → binary executes it,
// output appears in stdout, then EOF causes a clean exit.
#[test]
fn interactive_mode_with_piped_input_executes_command() {
    let home = tests::temp_home("interactive_mock");
    let port = tests::mock_ollama(&["echo interactive_ok"]);
    tests::write_ollama_config(&home, port);

    let out = tests::run_with_stdin(&home, &[], b"list files\n");

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("interactive_ok"),
        "executed command output not in stdout: {stdout}"
    );
}
