use super::*;

#[test]
fn profile_timeout_secs_kills_a_slow_provider_at_the_configured_seconds() {
    let home = temp_home("timeout_secs_slow");
    let port = mock_ollama_slow(&["COMMAND: echo SLOW_CMD"], std::time::Duration::from_secs(3));
    write_ollama_config_with_timeout(&home, port, Some(1));

    let out = run(&home, &["show disk usage"]);
    assert!(!out.status.success(), "a 3s response must die at the configured 1s timeout");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("request timeout after 1 seconds"), "the timeout error must name the configured value; stderr: {stderr}");
}

#[test]
fn no_timeout_secs_lets_a_slow_provider_finish() {
    let home = temp_home("timeout_none_slow");
    let port = mock_ollama_slow(&["COMMAND: echo SLOW_CMD"], std::time::Duration::from_secs(31));
    write_ollama_config_with_timeout(&home, port, None);

    let out = run(&home, &["show disk usage"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "without timeout_secs nothing may kill a 31s response; stderr: {stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("SLOW_CMD"), "the translated command should reach stdout; stdout: {stdout}");
}

#[test]
fn config_rejects_a_zero_timeout_secs() {
    let home = temp_home("timeout_secs_zero");
    write_ollama_config_with_timeout(&home, 1, Some(0));

    let out = run(&home, &["show disk usage"]);
    assert!(!out.status.success(), "timeout_secs = 0 must fail at config load, not brick every request");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("timeout_secs must be at least 1 second"), "the load error must name the field; stderr: {stderr}");
}
