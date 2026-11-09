use super::*;

/// `mock_ollama` that also records each request body in arrival order, so a test
/// can assert on what the provider actually received.
fn mock_ollama_capturing(responses: &[&str]) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::Read;
    use std::io::Write;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let responses: Vec<String> = responses.iter().map(std::string::ToString::to_string).collect();
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let bodies_clone = bodies.clone();
    std::thread::spawn(move || {
        for text in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = vec![0u8; 65536];
            let read = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..read]);
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
            bodies_clone.lock().unwrap().push(body);
            let response_body = serde_json::json!({ "message": { "content": text } }).to_string();
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = stream.write_all(http.as_bytes());
        }
    });

    (port, bodies)
}

// ── conversation context tests ───────────────────────────────────────────────

#[test]
fn repl_followup_request_carries_prior_turn_by_default() {
    let home = temp_home("context_default_all");
    let (port, bodies) = mock_ollama_capturing(&["COMMAND: df -h", "COMMAND: df -h /tmp"]);
    write_ollama_config(&home, port);

    let out = run_with_stdin_interactive(&home, &[], b"show disk usage\ny\ni meant the tmp filesystem\ny\n/quit\n");
    assert!(out.status.success(), "repl session should succeed; stderr: {}", String::from_utf8_lossy(&out.stderr));

    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "expected two provider calls; bodies: {bodies:?}");
    assert!(bodies[1].contains("show disk usage"), "second request must carry the first turn's request; body: {}", bodies[1]);
    assert!(bodies[1].contains("df -h"), "second request must carry the first turn's command; body: {}", bodies[1]);
}

#[test]
fn context_single_prompt_persists_to_config() {
    let home = temp_home("context_single_prompt");
    write_ollama_config(&home, 1);

    let out = run_with_stdin_interactive(&home, &[], b"/context single-prompt\n/quit\n");
    assert!(out.status.success(), "repl session should succeed; stderr: {}", String::from_utf8_lossy(&out.stderr));

    let config = std::fs::read_to_string(home.join("config").join("larpshell").join("config.toml")).unwrap();
    assert!(config.contains("single-prompt"), "config must record the context mode; config: {config}");
}

#[test]
fn repl_agent_mode_followup_carries_prior_turn_as_messages() {
    let home = temp_home("context_agent_all");
    let (port, bodies) = mock_ollama_capturing(&["COMMAND: df -h", "COMMAND: df -h /tmp"]);
    write_ollama_config(&home, port);

    let out = run_with_stdin_interactive(&home, &[], b"/agent on\nshow disk usage\ny\ni meant the tmp filesystem\ny\n/quit\n");
    assert!(out.status.success(), "agent repl session should succeed; stderr: {}", String::from_utf8_lossy(&out.stderr));

    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "expected two provider calls; bodies: {bodies:?}");
    assert!(
        bodies[1].contains("show disk usage"),
        "second agent request must carry the first turn's request as a message; body: {}",
        bodies[1]
    );
    assert!(
        bodies[1].contains("\"role\":\"assistant\""),
        "prior turns must ride as real assistant messages, not user text; body: {}",
        bodies[1]
    );
}

#[test]
fn context_block_keeps_last_eight_turns_and_caps_fields() {
    let turns: Vec<crate::SessionTurn> = (0..10)
        .map(|i| crate::SessionTurn { request: format!("request number {i}"), command: format!("cmd-{i}"), executed: true })
        .collect();
    let block = crate::render_context_block(&turns);

    assert_eq!(block.matches("request:").count(), 8, "only the last 8 turns render; block: {block}");
    assert!(block.contains("request number 9"), "newest turn is kept; block: {block}");
    assert!(block.contains("request number 2"), "eighth-newest turn is kept; block: {block}");
    assert!(!block.contains("request number 1"), "older turns are dropped; block: {block}");
    assert!(!block.contains("request number 0"), "older turns are dropped; block: {block}");

    let long_request = "a".repeat(crate::CONTEXT_REQUEST_CAP * 2);
    let long_command = "b".repeat(crate::CONTEXT_COMMAND_CAP * 2);
    let capped = crate::render_context_block(&[crate::SessionTurn { request: long_request, command: long_command, executed: false }]);
    assert!(
        capped.contains(&format!("{}…", "a".repeat(crate::CONTEXT_REQUEST_CAP))),
        "request is capped with a trailing ellipsis; block: {capped}"
    );
    assert!(!capped.contains(&"a".repeat(crate::CONTEXT_REQUEST_CAP + 1)), "request cap is enforced; block: {capped}");
    assert!(
        capped.contains(&format!("{}…", "b".repeat(crate::CONTEXT_COMMAND_CAP))),
        "command is capped with a trailing ellipsis; block: {capped}"
    );
    assert!(!capped.contains(&"b".repeat(crate::CONTEXT_COMMAND_CAP + 1)), "command cap is enforced; block: {capped}");
    assert!(capped.contains("(not executed)"), "declined turns carry their marker; block: {capped}");
}

#[test]
fn repl_failed_command_still_records_turn() {
    let home = temp_home("context_failed_cmd");
    let (port, bodies) = mock_ollama_capturing(&["COMMAND: false", "COMMAND: df -h /tmp"]);
    write_ollama_config(&home, port);

    let out = run_with_stdin_interactive(&home, &[], b"run a failing command\ny\ntry again differently\ny\n/quit\n");
    assert!(out.status.success(), "session should continue past a failed command; stderr: {}", String::from_utf8_lossy(&out.stderr));

    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "expected two provider calls; bodies: {bodies:?}");
    assert!(bodies[1].contains("run a failing command"), "second request must carry the failed turn's request; body: {}", bodies[1]);
    assert!(bodies[1].contains("false"), "second request must carry the failed turn's command; body: {}", bodies[1]);
    assert!(bodies[1].contains("(not executed)"), "a failed command records as not executed; body: {}", bodies[1]);
}

#[test]
fn repl_declined_request_records_not_executed_turn() {
    let home = temp_home("context_declined");
    let (port, bodies) = mock_ollama_capturing(&["COMMAND: df -h", "COMMAND: df -h /tmp"]);
    write_ollama_config(&home, port);

    let out = run_with_stdin_interactive(&home, &[], b"show disk usage\nn\ni meant the tmp filesystem\ny\n/quit\n");
    assert!(out.status.success(), "declining must not end the session; stderr: {}", String::from_utf8_lossy(&out.stderr));

    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "expected two provider calls; bodies: {bodies:?}");
    assert!(bodies[1].contains("show disk usage"), "second request must carry the declined turn's request; body: {}", bodies[1]);
    assert!(bodies[1].contains("(not executed)"), "a declined request records executed=false; body: {}", bodies[1]);
}

#[test]
fn context_single_prompt_suppresses_prior_turn_non_agent() {
    let home = temp_home("context_single_nonagent");
    let (port, bodies) = mock_ollama_capturing(&["COMMAND: df -h", "COMMAND: df -h /tmp"]);
    write_ollama_config(&home, port);

    let out = run_with_stdin_interactive(&home, &[], b"/context single-prompt\nshow disk usage\ny\ni meant the tmp filesystem\ny\n/quit\n");
    assert!(out.status.success(), "repl session should succeed; stderr: {}", String::from_utf8_lossy(&out.stderr));

    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "expected two provider calls; bodies: {bodies:?}");
    assert!(!bodies[1].contains("Earlier in this session:"), "single-prompt must not carry the context block; body: {}", bodies[1]);
    assert!(!bodies[1].contains("show disk usage"), "single-prompt must not carry the prior turn's request; body: {}", bodies[1]);
}

#[test]
fn context_single_prompt_suppresses_prior_turn_agent() {
    let home = temp_home("context_single_agent");
    let (port, bodies) = mock_ollama_capturing(&["COMMAND: df -h", "COMMAND: df -h /tmp"]);
    write_ollama_config(&home, port);

    let out = run_with_stdin_interactive(
        &home,
        &[],
        b"/agent on\n/context single-prompt\nshow disk usage\ny\ni meant the tmp filesystem\ny\n/quit\n",
    );
    assert!(out.status.success(), "agent repl session should succeed; stderr: {}", String::from_utf8_lossy(&out.stderr));

    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "expected two provider calls; bodies: {bodies:?}");
    assert!(
        !bodies[1].contains("show disk usage"),
        "single-prompt agent request must not carry the prior turn's request; body: {}",
        bodies[1]
    );
    assert!(
        !bodies[1].contains("\"role\":\"assistant\""),
        "single-prompt agent request must carry no assistant history; body: {}",
        bodies[1]
    );
}
