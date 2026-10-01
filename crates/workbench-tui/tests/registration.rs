use std::{fs, process::Command};

fn register(path: &std::path::Path, id: &str, kind: &str, pane: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_workbench"))
        .args([
            "register",
            "--id",
            id,
            "--kind",
            kind,
            "--repository",
            "/work/my repo",
            "--workspace",
            "/work/task workspace",
            "--pane",
            pane,
            "--title",
            "Fix retries — λ",
            "--state-file",
        ])
        .arg(path)
        .output()
        .unwrap()
}

#[test]
fn registrations_survive_independent_processes_without_tmux_or_a_terminal() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested/work-items.json");
    for (id, kind, pane) in [
        ("ABC-123", "implementation", "%14"),
        ("PR #1842", "external-review", "%21"),
    ] {
        let output = register(&path, id, kind, pane);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let listed = Command::new(env!("CARGO_BIN_EXE_workbench"))
        .args(["list", "--state-file"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(listed.status.success());
    let text = String::from_utf8(listed.stdout).unwrap();
    for expected in [
        "ABC-123",
        "PR #1842",
        "UNKNOWN",
        "Implementation",
        "External Review",
        "%14",
        "%21",
        "/work/my repo",
        "/work/task workspace",
        "Fix retries — λ",
    ] {
        assert!(text.contains(expected), "missing {expected:?}: {text}");
    }
    let original = fs::read(&path).unwrap();
    let duplicate = register(&path, "ABC-123", "implementation", "%15");
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already registered"));
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn corrupt_state_and_invalid_registration_exit_with_errors_without_losing_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("items.json");
    assert!(
        !register(&path, "ABC-123", "implementation", "not-a-pane")
            .status
            .success()
    );
    assert!(!path.exists());
    fs::write(&path, "{broken").unwrap();
    let output = register(&path, "ABC-123", "implementation", "%14");
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");
}
