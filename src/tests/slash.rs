use super::*;

#[test]
fn quit_command_exits_zero() {
    let home = temp_home("slash_quit");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);
    let out = run_with_stdin(&home, &[], b"/quit\n");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn unknown_slash_command_prints_error() {
    let home = temp_home("slash_unknown");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);
    let out = run_with_stdin(&home, &[], b"/notacommand\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown command") && stderr.contains("/notacommand"),
        "stderr: {stderr}"
    );
}
