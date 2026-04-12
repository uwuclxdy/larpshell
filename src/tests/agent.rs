use super::*;

#[test]
fn agent_on_safe_off_subcommand_updates_config() {
    let home = temp_home("agent_toggle");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);

    let config_path = home.join("config").join("larpshell").join("config.toml");

    let out = run(&home, &["agent", "on"]);
    assert!(out.status.success());
    let contents = fs::read_to_string(&config_path).unwrap();
    assert!(contents.contains("agent = \"on\""));

    let out = run(&home, &["agent", "safe"]);
    assert!(out.status.success());
    let contents = fs::read_to_string(&config_path).unwrap();
    assert!(contents.contains("agent = \"safe\""));

    let out = run(&home, &["agent", "off"]);
    assert!(out.status.success());
    let contents = fs::read_to_string(&config_path).unwrap();
    assert!(contents.contains("agent = \"off\""));
}

#[test]
fn agent_slash_command_parsed_in_interactive() {
    let home = temp_home("agent_slash");
    let port = mock_ollama(&[]);
    write_ollama_config(&home, port);

    let out = run_with_stdin(&home, &[], b"/agent safe\n/quit\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("agent mode set to safe"),
        "expected agent safe message; stderr: {stderr}"
    );

    let config_path = home.join("config").join("larpshell").join("config.toml");
    let contents = fs::read_to_string(&config_path).unwrap();
    assert!(
        contents.contains("agent = \"safe\""),
        "config should have agent = \"safe\" after /agent safe"
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
