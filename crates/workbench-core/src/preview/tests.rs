use super::*;
#[cfg(unix)]
use crate::{Engine, WorkItem};
#[cfg(unix)]
use std::time::Duration;

const META: &str = "123:456\x1f/tmp/private.sock\x1f%14\x1f789\x1f0\x1f0\x1f100\x1f30";

fn frame(before: &str, text: &str, after: &str) -> String {
    format!("\x1e{before}\x1e\n{text}\x1e{after}\x1e\n")
}

#[test]
fn test_capture_parse() {
    let text = "\x1b[32mλ🙂 done\x1b[0m\n";
    let preview = parse_capture(frame(META, text, META).as_bytes(), "%14").unwrap();
    assert_eq!(
        preview,
        PanePreview {
            text: text.into(),
            width: 100,
            height: 30
        }
    );
}

#[test]
fn test_capture_guards() {
    for after in [
        META.replace("789", "790"),
        META.replace("100", "101"),
        META.replace("123:456", "123:999"),
    ] {
        assert!(parse_capture(frame(META, "text", &after).as_bytes(), "%14").is_err());
    }
    for metadata in [
        META.replace("%14", "%15"),
        META.replace("\x1f0\x1f0\x1f", "\x1f1\x1f0\x1f"),
        META.replace("\x1f0\x1f0\x1f", "\x1f0\x1f1\x1f"),
        META.replace("100", "0"),
        META.replace("100", "bad"),
        META.replace("789", "0"),
        META.replace("/tmp/private.sock", ""),
    ] {
        assert!(parse_capture(frame(&metadata, "text", &metadata).as_bytes(), "%14").is_err());
    }
    for text in [
        Vec::new(),
        b"unframed".to_vec(),
        vec![0xff],
        frame(META, "bad\x1econtent", META).into_bytes(),
        vec![b'x'; MAX_CAPTURE + 1],
    ] {
        assert!(parse_capture(&text, "%14").is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires tmux; creates and cleans up only its own isolated server"]
async fn test_tmux_preview() {
    struct Server(std::path::PathBuf);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = std::process::Command::new("tmux")
                .arg("-S")
                .arg(&self.0)
                .arg("kill-server")
                .output();
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("preview.sock");
    let output = std::process::Command::new("tmux")
        .arg("-S")
        .arg(&socket)
        .args([
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "preview",
            "-x",
            "100",
            "-y",
            "30",
            "printf '\\033[38;2;1;2;3mpreview-marker\\033[0m\\n'; exec sleep 60",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _server = Server(socket.clone());
    let mut engine = Engine::new(directory.path().join("state.json"));
    engine.tmux.socket = Some(socket);
    let pane = engine.discover().await.unwrap().sessions[0].windows[0].panes[0]
        .id
        .clone();
    let item = WorkItem {
        id: "A".into(),
        title: String::new(),
        repository: directory.path().into(),
        workspace: directory.path().into(),
        branch: None,
        kind: crate::WorkItemKind::Implementation,
        pane_id: pane.clone(),
    };
    engine.register_work_item(item.clone()).unwrap();
    let buddy = engine.tmux.query(&[
        "new-window", "-t", "preview", "-n", "active", "-P", "-F", "#{pane_id}",
        r#"exec sh -c 'i=0; while :; do printf "\rprogress-%s" "$i"; i=$((i + 1)); sleep 0.02; done'"#,
    ]).await.unwrap().trim().to_owned();
    let focus = [
        "display-message",
        "-p",
        "-t",
        "preview",
        "#{window_id}:#{pane_id}",
    ];
    let sizes = [
        "list-panes",
        "-a",
        "-F",
        "#{pane_id}:#{pane_width}:#{pane_height}",
    ];
    let clients = ["list-clients", "-F", "#{client_name}"];
    let before = (
        engine.tmux.query(&focus).await.unwrap(),
        engine.tmux.query(&sizes).await.unwrap(),
    );
    let state = std::fs::read(engine.store.state_path()).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    tokio::time::timeout(Duration::from_secs(15), async {
        let checks = async {
            let capture = loop {
                let capture = next(&mut rx).await.unwrap();
                if capture.text.contains("preview-marker") { break capture; }
            };
            assert!(capture.text.contains("\x1b[38;2;1;2;3m"));
            assert_eq!((capture.width, capture.height), (100, 30));
            assert_eq!(engine.tmux.query(&focus).await.unwrap(), before.0);
            assert_eq!(engine.tmux.query(&sizes).await.unwrap(), before.1);
            // Even without another attached client, pausing the extra copy
            // must not stop tmux reading and updating the sibling's screen.
            let read = ["capture-pane", "-p", "-t", &buddy];
            let first = engine.tmux.query(&read).await.unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert_ne!(engine.tmux.query(&read).await.unwrap(), first);
            tokio::time::sleep(Duration::from_millis(200)).await;
            while rx.try_recv().is_ok() {}
            tokio::time::sleep(Duration::from_millis(250)).await;
            assert!(rx.try_recv().is_err(), "busy sibling woke the quiet preview");
            engine.tmux.execute(&["copy-mode".into(), "-t".into(), pane.clone()]).await.unwrap();
            let error = loop {
                if let Err(error) = next(&mut rx).await { break error; }
            };
            assert!(error.to_string().contains("copy mode"));
            engine.tmux.execute(&["send-keys".into(), "-t".into(), pane.clone(), "-X".into(), "cancel".into()]).await.unwrap();
            while next(&mut rx).await.is_err() {}
            while rx.try_recv().is_ok() {}
            let observer = engine.tmux.query(&clients).await.unwrap();
            assert_eq!(observer.lines().count(), 1);
            engine.tmux.execute(&["detach-client".into(), "-t".into(), observer.trim().into()]).await.unwrap();
            while next(&mut rx).await.is_ok() {}
            while next(&mut rx).await.is_err() {}
            assert_eq!(engine.tmux.query(&clients).await.unwrap().lines().count(), 1);
            while rx.try_recv().is_ok() {}
            engine.tmux.execute(&["kill-pane".into(), "-t".into(), pane.clone()]).await.unwrap();
            while next(&mut rx).await.is_ok() {}
        };
        tokio::select! {
            _ = checks => {},
            result = engine.stream_preview(&item, |capture| { let _ = tx.try_send(capture); }) => panic!("stream stopped: {result:?}"),
        }
    }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !engine.tmux.query(&clients).await.unwrap().trim().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read(engine.store.state_path()).unwrap(), state);
    let item = WorkItem {
        id: "B".into(),
        pane_id: buddy,
        ..item
    };
    engine.register_work_item(item.clone()).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    tokio::time::timeout(Duration::from_secs(5), async {
        let checks = async {
            assert!(next(&mut rx).await.is_ok());
            while rx.try_recv().is_ok() {}
            engine.tmux.execute(&["kill-server".into()]).await.unwrap();
            while next(&mut rx).await.is_ok() {}
        };
        tokio::select! {
            _ = checks => {},
            result = engine.stream_preview(&item, |capture| { let _ = tx.try_send(capture); }) => panic!("stream stopped: {result:?}"),
        }
    }).await.unwrap();
}

#[cfg(unix)]
async fn next(
    rx: &mut tokio::sync::mpsc::Receiver<Result<PanePreview, PreviewError>>,
) -> Result<PanePreview, PreviewError> {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap()
}
