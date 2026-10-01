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
        .env(
            "TMUX",
            format!("{},1,0", directory.path().join("absent.sock").display()),
        )
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
    let home_listed = Command::new(env!("CARGO_BIN_EXE_workbench"))
        .args(["list", "--state-file"])
        .arg(&path)
        .env("HOME", "/work")
        .env(
            "TMUX",
            format!("{},1,0", directory.path().join("absent.sock").display()),
        )
        .output()
        .unwrap();
    assert!(home_listed.status.success());
    let text = String::from_utf8(home_listed.stdout).unwrap();
    assert!(text.contains("Repository: ~/my repo"));
    assert!(text.contains("Workspace: ~/task workspace"));
    assert_eq!(fs::read(&path).unwrap(), original);
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

#[test]
fn short_description_limit_and_alias_work_through_the_cli_without_data_loss() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("items.json");
    let attempt = |id: &str, flag: &str, description: &str| {
        Command::new(env!("CARGO_BIN_EXE_workbench"))
            .args([
                "register",
                "--id",
                id,
                "--kind",
                "implementation",
                "--repository",
                "/work",
                "--workspace",
                "/work",
                "--pane",
                "%1",
                flag,
                description,
                "--state-file",
            ])
            .arg(&path)
            .output()
            .unwrap()
    };
    let limit = workbench_core::MAX_SHORT_DESCRIPTION_CHARS;
    let too_long = "🙂".repeat(limit + 1);
    let failed = attempt("long", "--short-description", &too_long);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("at most 120 characters"));
    assert!(!path.exists());
    for (id, flag) in [("new", "--short-description"), ("legacy", "--title")] {
        let output = attempt(id, flag, &"🙂".repeat(limit));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stored = fs::read(&path).unwrap();
    for flag in ["--short-description", "--title"] {
        assert!(!attempt("long", flag, &too_long).status.success());
        assert_eq!(fs::read(&path).unwrap(), stored);
    }
    let items = workbench_core::Engine::new(&path).work_items().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item.title.chars().count() == limit));
}
