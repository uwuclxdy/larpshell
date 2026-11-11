use super::{mock_ollama_scripted, run, temp_home, write_ollama_config};

#[test]
fn dry_run_prints_the_translation_once_and_never_executes_it() {
    let home = temp_home("dry_run_flag");
    let (port, _requests) = mock_ollama_scripted(&["COMMAND: echo DRY_CMD"]);
    write_ollama_config(&home, port);

    let out = run(&home, &["--dry-run", "show disk usage"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "--dry-run must exit 0; stderr: {stderr}");
    assert_eq!(stderr.matches("echo DRY_CMD").count(), 1, "the translated command must print exactly once; stderr: {stderr}");
    assert!(stdout.is_empty(), "--dry-run must emit nothing on stdout; stdout: {stdout}");
}
