use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn callbacks_return_neutral_json_for_bad_payloads_or_unbound_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("items.json");
    for payload in [
        "garbage",
        "{}",
        "{\"hook_event_name\":\"SessionStart\",\"session_id\":\"test\"}",
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_workbench"))
            .args(["codex-hook", "--state-file"])
            .arg(&state)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
        assert!(!state.exists());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_workbench"))
        .args(["codex-notify", "--state-file"])
        .arg(&state)
        .arg(r#"{"type":"agent-turn-complete","thread-id":"test","turn-id":"turn"}"#)
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}\n");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn callback_path_failures_are_observational_not_agent_blocking_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_workbench"))
        .args(["codex-notify", "{}", "--state-file", "~/unexpanded"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("signal ignored"));
}
