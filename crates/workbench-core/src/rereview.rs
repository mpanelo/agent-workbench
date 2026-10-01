//! Reconstruct captured text against immutable Git base blobs. Never read a
//! live file when marking it reviewed, write Git objects, or apply stored patches
//! to a checkout. Existing M7 snapshots require no migration.
use std::{ffi::OsString, path::Path};

use tokio::time::timeout;

use crate::git::{
    GitClient, MAX_FILES, MAX_LINE_BYTES, MAX_OUTPUT, MAX_PATCH_LINES, MAX_TOTAL_PATCH,
    REVIEW_TIMEOUT, diff_args, patch_counts, text,
};
use crate::{ChangeKind, ChangedFile, GitError, ReviewSession};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    mode: String,
    content: Vec<u8>,
}

impl GitClient {
    pub(crate) async fn changes_since_review(
        &self,
        session: &ReviewSession,
    ) -> Result<Vec<ChangedFile>, GitError> {
        timeout(REVIEW_TIMEOUT, async {
            let mut changes = Vec::new();
            let mut snapshots: Vec<_> = session.reviewed.values().collect();
            snapshots.sort_by(|a, b| a.path.cmp(&b.path));
            let mut total = 0;
            for snapshot in snapshots {
                let current = session.diff.files.iter().find(|file| file.path == snapshot.path);
                if current == Some(snapshot) {
                    continue;
                }
                let delta = self.compare_snapshot(session, snapshot, current).await;
                // Unavailable text must not look reviewed. Keep full review
                // usable, but clearly decline a since-review line count.
                let delta = match delta {
                    Ok(delta) => delta,
                    Err(error) => Some(notice(snapshot, format!(
                        "Since-review text unavailable: {error}\nUse c for the full diff or an external viewer; no line count is claimed.\n"
                    ))),
                };
                if let Some(delta) = delta {
                    total += delta.patch.len();
                    if changes.len() >= MAX_FILES || total > MAX_TOTAL_PATCH {
                        return Err(GitError::TooLarge);
                    }
                    changes.push(delta);
                }
            }
            Ok(changes)
        }).await.map_err(|_| GitError::TimedOut)?
    }

    async fn base_entry(
        &self,
        session: &ReviewSession,
        path: &Path,
    ) -> Result<Option<Entry>, GitError> {
        let args: Vec<OsString> = ["ls-tree", "-z", &session.diff.base_revision, "--"]
            .into_iter()
            .map(OsString::from)
            .chain([path.as_os_str().to_owned()])
            .collect();
        let tree = self.run(&session.diff.workspace, &args, false).await?;
        if tree.is_empty() {
            return Ok(None);
        }
        let record = text(&tree)?.strip_suffix('\0').ok_or_else(invalid_patch)?;
        let (metadata, name) = record.split_once('\t').ok_or_else(invalid_patch)?;
        if Path::new(name) != path {
            return Err(invalid_patch());
        }
        let fields: Vec<_> = metadata.split(' ').collect();
        if fields.len() != 3 || !valid_mode(fields[0]) {
            return Err(invalid_patch());
        }
        let content = if fields[0] == "160000" {
            format!("Subproject commit {}\n", fields[2]).into_bytes()
        } else {
            let args = ["cat-file", "blob", fields[2]].map(OsString::from);
            self.run(&session.diff.workspace, &args, false).await?
        };
        Ok(Some(Entry {
            mode: fields[0].into(),
            content,
        }))
    }

    async fn captured_entry(
        &self,
        session: &ReviewSession,
        file: &ChangedFile,
    ) -> Result<Option<Entry>, GitError> {
        let base = if matches!(file.kind, ChangeKind::Added | ChangeKind::Untracked) {
            None
        } else {
            self.base_entry(session, file.old_path.as_deref().unwrap_or(&file.path))
                .await?
        };
        reconstruct(base, &file.patch)
    }

    async fn compare_snapshot(
        &self,
        session: &ReviewSession,
        snapshot: &ChangedFile,
        current: Option<&ChangedFile>,
    ) -> Result<Option<ChangedFile>, GitError> {
        if snapshot.additions.is_none() || current.is_some_and(|file| file.additions.is_none()) {
            if let Some(current) = current
                && binary_identity(snapshot).is_some()
                && binary_identity(snapshot) == binary_identity(current)
            {
                return Ok(None);
            }
            return Ok(Some(notice(snapshot,
                "Binary content or metadata changed since review; use an external viewer. No text line count is available.\n".into())));
        }
        let before = self.captured_entry(session, snapshot).await?;
        let after = match current {
            Some(file) => self.captured_entry(session, file).await?,
            None => {
                // A path can have become the source of a new rename, which is
                // no longer present under its original name in the name list.
                if session
                    .diff
                    .files
                    .iter()
                    .any(|file| file.old_path.as_deref() == Some(&snapshot.path))
                {
                    None
                } else {
                    self.base_entry(session, &snapshot.path).await?
                }
            }
        };
        if before == after {
            return Ok(None);
        }
        // Private temporary regular files hold symlink target text / submodule
        // pointers too. Never follow links or consult working-tree filters.
        let directory = tempfile::tempdir().map_err(GitError::Io)?;
        for (name, entry) in [("reviewed", &before), ("current", &after)] {
            if let Some(entry) = entry {
                std::fs::write(directory.path().join(name), &entry.content)
                    .map_err(GitError::Io)?;
            }
        }
        let left = if before.is_some() {
            "reviewed"
        } else {
            "/dev/null"
        };
        let right = if after.is_some() {
            "current"
        } else {
            "/dev/null"
        };
        let bytes = self
            .run(
                directory.path(),
                &diff_args(&["--no-index", "--", left, right]),
                true,
            )
            .await?;
        let (additions, deletions) = patch_counts(text(&bytes)?);
        let mut patch = String::new();
        let old_mode = before
            .as_ref()
            .map(|entry| entry.mode.as_str())
            .unwrap_or("absent");
        let new_mode = after
            .as_ref()
            .map(|entry| entry.mode.as_str())
            .unwrap_or("absent");
        if old_mode != new_mode {
            patch.push_str(&format!("File mode: {old_mode} -> {new_mode}\n"));
        }
        // Stable labels never expose temporary paths, and diff only content:
        // no patch-of-a-patch or original base-only edits in this view.
        let mut in_hunk = false;
        for line in text(&bytes)?.split_inclusive('\n') {
            if line.starts_with("diff --git ") {
                in_hunk = false;
                patch.push_str(&format!("diff since review: {:?}\n", snapshot.path));
            } else if !in_hunk && line.starts_with("--- ") {
                patch.push_str(&format!("--- reviewed/{:?}\n", snapshot.path));
            } else if !in_hunk && line.starts_with("+++ ") {
                patch.push_str(&format!("+++ current/{:?}\n", snapshot.path));
            } else {
                if line.starts_with("@@ ") {
                    in_hunk = true;
                }
                patch.push_str(line);
            }
        }
        if patch.is_empty() {
            patch.push_str("File metadata changed since review.\n");
        }
        if patch.lines().count() > MAX_PATCH_LINES
            || patch.lines().any(|line| line.len() > MAX_LINE_BYTES)
        {
            return Err(GitError::TooLarge);
        }
        Ok(Some(ChangedFile {
            path: snapshot.path.clone(),
            old_path: None,
            kind: match (&before, &after) {
                (None, Some(_)) => ChangeKind::Added,
                (Some(_), None) => ChangeKind::Deleted,
                _ if old_mode != new_mode => ChangeKind::TypeChanged,
                _ => ChangeKind::Modified,
            },
            additions,
            deletions,
            patch,
        }))
    }
}

fn notice(snapshot: &ChangedFile, patch: String) -> ChangedFile {
    ChangedFile {
        path: snapshot.path.clone(),
        old_path: None,
        kind: ChangeKind::Modified,
        additions: None,
        deletions: None,
        patch,
    }
}

fn binary_identity(file: &ChangedFile) -> Option<(&str, &str)> {
    let mut id = None;
    let mut mode = "100644";
    for line in file.patch.lines() {
        if let Some(value) = line
            .strip_prefix("new file mode ")
            .or_else(|| line.strip_prefix("new mode "))
        {
            mode = value;
        }
        if let Some(value) = line.strip_prefix("index ") {
            let mut fields = value.split(' ');
            id = fields.next()?.split_once("..").map(|(_, new)| new);
            if let Some(value) = fields.next() {
                mode = value;
            }
        }
    }
    id.filter(|id| matches!(id.len(), 40 | 64) && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(|id| (id, mode))
}

fn valid_mode(mode: &str) -> bool {
    matches!(mode, "100644" | "100755" | "120000" | "160000")
}

fn invalid_patch() -> GitError {
    GitError::Invalid("saved patch cannot be reconstructed against its base".into())
}

/// Interpret hunk content only. Patch paths are never used for file operations.
fn reconstruct(mut entry: Option<Entry>, patch: &str) -> Result<Option<Entry>, GitError> {
    let all_lines: Vec<_> = patch.split_inclusive('\n').collect();
    let mut starts: Vec<_> = all_lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| line.starts_with("diff --git ").then_some(i))
        .collect();
    if starts.is_empty() {
        return Err(invalid_patch());
    }
    starts.push(all_lines.len());
    for section in starts.windows(2) {
        let lines = &all_lines[section[0]..section[1]];
        let mut mode = entry
            .as_ref()
            .map(|entry| entry.mode.clone())
            .unwrap_or_else(|| "100644".into());
        let mut deleted = false;
        for line in lines.iter().take_while(|line| !line.starts_with("@@ ")) {
            let header = line.trim_end_matches('\n');
            if let Some(new_mode) = header
                .strip_prefix("new file mode ")
                .or_else(|| header.strip_prefix("new mode "))
            {
                if !valid_mode(new_mode) {
                    return Err(invalid_patch());
                }
                mode = new_mode.into();
            }
            if header.starts_with("deleted file mode ") {
                deleted = true;
            }
            if header.starts_with("Binary files ") || header == "GIT binary patch" {
                return Err(invalid_patch());
            }
        }
        let source = entry
            .as_ref()
            .map(|entry| entry.content.as_slice())
            .unwrap_or(&[]);
        let content = apply_hunks(source, lines)?;
        if deleted && !content.is_empty() {
            return Err(invalid_patch());
        }
        entry = if deleted {
            None
        } else {
            Some(Entry { mode, content })
        };
    }
    Ok(entry)
}

fn range(value: &str) -> Result<(usize, usize), GitError> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    Ok((
        start.parse().map_err(|_| invalid_patch())?,
        count.parse().map_err(|_| invalid_patch())?,
    ))
}

fn apply_hunks(source: &[u8], lines: &[&str]) -> Result<Vec<u8>, GitError> {
    let original: Vec<_> = source.split_inclusive(|byte| *byte == b'\n').collect();
    let mut output = Vec::new();
    let mut cursor = 0;
    let mut output_lines = 0;
    let mut index = 0;
    let mut seen_hunk = false;
    while index < lines.len() {
        let Some(header) = lines[index].strip_prefix("@@ -") else {
            if seen_hunk
                && lines[index]
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| matches!(byte, b'+' | b'-' | b' ' | b'\\'))
            {
                return Err(invalid_patch());
            }
            index += 1;
            continue;
        };
        seen_hunk = true;
        let (old, rest) = header.split_once(" +").ok_or_else(invalid_patch)?;
        let (new, _) = rest.split_once(" @@").ok_or_else(invalid_patch)?;
        let (old_start, old_count) = range(old)?;
        let (new_start, new_count) = range(new)?;
        let start = if old_count == 0 {
            old_start
        } else {
            old_start.checked_sub(1).ok_or_else(invalid_patch)?
        };
        if start < cursor || start > original.len() {
            return Err(invalid_patch());
        }
        for line in &original[cursor..start] {
            output.extend_from_slice(line);
            output_lines += 1;
        }
        cursor = start;
        let new_position = if new_count == 0 {
            new_start
        } else {
            new_start.checked_sub(1).ok_or_else(invalid_patch)?
        };
        if output_lines != new_position {
            return Err(invalid_patch());
        }
        index += 1;
        let (mut consumed, mut produced) = (0, 0);
        while consumed < old_count || produced < new_count {
            let line = *lines.get(index).ok_or_else(invalid_patch)?;
            let prefix = *line.as_bytes().first().ok_or_else(invalid_patch)?;
            let mut bytes = &line.as_bytes()[1..];
            index += 1;
            if lines
                .get(index)
                .is_some_and(|line| line.starts_with("\\ No newline at end of file"))
            {
                bytes = bytes.strip_suffix(b"\n").ok_or_else(invalid_patch)?;
                index += 1;
            }
            match prefix {
                b' ' | b'-' => {
                    if original.get(cursor).copied() != Some(bytes) {
                        return Err(invalid_patch());
                    }
                    cursor += 1;
                    consumed += 1;
                    if prefix == b' ' {
                        output.extend_from_slice(bytes);
                        produced += 1;
                        output_lines += 1;
                    }
                }
                b'+' => {
                    output.extend_from_slice(bytes);
                    produced += 1;
                    output_lines += 1;
                }
                _ => return Err(invalid_patch()),
            }
            if consumed > old_count || produced > new_count || output.len() > MAX_OUTPUT {
                return Err(invalid_patch());
            }
        }
    }
    for line in &original[cursor..] {
        output.extend_from_slice(line);
    }
    if output.len() > MAX_OUTPUT {
        return Err(GitError::TooLarge);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Engine, ReviewStatus,
        git::tests::{fixture, git},
    };
    use std::fs;

    fn mark_all(engine: &Engine, session: &mut ReviewSession) {
        let paths: Vec<_> = session
            .diff
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect();
        for path in paths {
            engine.set_file_reviewed(session, &path, true).unwrap();
        }
    }

    #[tokio::test]
    async fn only_corrections_are_shown_after_restart_and_remarking_uses_seen_capture() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        let produced = "agent implementation\nkeep this implementation\nfix this\n";
        fs::write(root.join("source.txt"), produced).unwrap();
        fs::write(root.join("new λ.txt"), "new implementation\n").unwrap();
        let mut review = engine.open_review("A", None).await.unwrap();
        mark_all(&engine, &mut review);
        let saved = fs::read(engine.store.review_path()).unwrap();
        let index = fs::read(root.join(".git/index")).unwrap();
        fs::write(
            root.join("source.txt"),
            produced.replace("fix this", "corrected"),
        )
        .unwrap();
        fs::write(root.join("never reviewed.txt"), "brand new\n").unwrap();
        let restarted = Engine::new(dir.path().join("items.json"));
        let mut fresh = restarted.open_review("A", None).await.unwrap();
        let changes = fresh.changes_since_review();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, Path::new("source.txt"));
        assert_eq!(
            (changes[0].additions, changes[0].deletions),
            (Some(1), Some(1))
        );
        assert!(changes[0].patch.contains("-fix this\n+corrected\n"));
        assert!(!changes[0].patch.contains("-old\n"));
        assert!(!changes[0].patch.contains("+agent implementation\n"));
        assert_eq!(fresh.progress(), (1, 3));
        assert_eq!(fs::read(engine.store.review_path()).unwrap(), saved);
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        fs::write(root.join("source.txt"), "unseen later correction\n").unwrap();
        restarted
            .set_file_reviewed(&mut fresh, Path::new("source.txt"), true)
            .unwrap();
        assert!(fresh.changes_since_review().is_empty());
        let next = restarted.open_review("A", None).await.unwrap();
        assert!(
            next.changes_since_review()[0]
                .patch
                .contains("-corrected\n")
        );
        assert!(
            next.changes_since_review()[0]
                .patch
                .contains("+unseen later correction\n")
        );
    }

    #[tokio::test]
    async fn reverted_and_removed_paths_remain_visible_and_can_be_acknowledged() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        fs::write(root.join("source.txt"), "reviewed modification\n").unwrap();
        fs::write(root.join("untracked.txt"), "reviewed new file\n").unwrap();
        let mut review = engine.open_review("A", None).await.unwrap();
        mark_all(&engine, &mut review);
        fs::write(root.join("source.txt"), "one\nold\n").unwrap();
        fs::remove_file(root.join("untracked.txt")).unwrap();
        let mut fresh = engine.open_review("A", None).await.unwrap();
        assert!(fresh.diff.files.is_empty());
        assert_eq!(fresh.changed_after_review(), 2);
        assert!(
            fresh.changes_since_review()[0]
                .patch
                .contains("-reviewed modification\n")
        );
        assert!(fresh.changes_since_review()[0].patch.contains("+old\n"));
        assert_eq!(fresh.changes_since_review()[1].kind, ChangeKind::Deleted);
        for path in ["source.txt", "untracked.txt"] {
            assert_eq!(
                fresh.status(Path::new(path)),
                ReviewStatus::ChangedAfterReview
            );
            engine
                .set_file_reviewed(&mut fresh, Path::new(path), true)
                .unwrap();
        }
        assert!(
            engine
                .open_review("A", None)
                .await
                .unwrap()
                .changes_since_review()
                .is_empty()
        );
        // New edits after acknowledgment are genuinely unreviewed.
        fs::write(root.join("source.txt"), "another change\n").unwrap();
        assert_eq!(
            engine
                .open_review("A", None)
                .await
                .unwrap()
                .status(Path::new("source.txt")),
            ReviewStatus::Unreviewed
        );
    }

    #[tokio::test]
    async fn renames_deletions_recreation_and_staging_identical_text_are_compared_as_content() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        git(&root, &["mv", "--", "rename source.txt", "renamed λ.txt"]);
        fs::remove_file(root.join("-delete.txt")).unwrap();
        fs::write(root.join("new.txt"), "same text\n").unwrap();
        git(&root, &["rm", "--cached", "--", "source.txt"]);
        fs::write(root.join("source.txt"), "recreated\n").unwrap();
        let mut review = engine.open_review("A", None).await.unwrap();
        mark_all(&engine, &mut review);
        fs::write(
            root.join("renamed λ.txt"),
            "preserve this renamed file\ncorrection\n",
        )
        .unwrap();
        fs::write(root.join("-delete.txt"), "restored\n").unwrap();
        fs::write(root.join("source.txt"), "recreated correction\n").unwrap();
        git(&root, &["add", "--", "new.txt"]);
        let fresh = engine.open_review("A", None).await.unwrap();
        assert_eq!(fresh.changed_after_review(), 3);
        assert!(fresh.is_reviewed(Path::new("new.txt")));
        for (path, expected) in [
            ("renamed λ.txt", "+correction\n"),
            ("-delete.txt", "+restored\n"),
            ("source.txt", "-recreated\n+recreated correction\n"),
        ] {
            let file = fresh
                .changes_since_review()
                .iter()
                .find(|file| file.path == Path::new(path))
                .unwrap();
            assert!(file.additions.is_some(), "{}", file.patch);
            assert!(file.patch.contains(expected), "{}", file.patch);
        }
        // A reviewed path renamed again is shown as removed, plus a never-
        // reviewed destination in the full diff, not silently lost.
        git(&root, &["mv", "--", "renamed λ.txt", "renamed again.txt"]);
        let fresh = engine.open_review("A", None).await.unwrap();
        assert_eq!(
            fresh
                .changes_since_review()
                .iter()
                .find(|file| file.path == Path::new("renamed λ.txt"))
                .unwrap()
                .kind,
            ChangeKind::Deleted
        );
    }

    #[tokio::test]
    async fn binary_changes_have_no_fictional_line_count_and_staging_alone_keeps_review() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        fs::write(root.join("new.bin"), b"reviewed\0binary").unwrap();
        let mut review = engine.open_review("A", None).await.unwrap();
        mark_all(&engine, &mut review);
        git(&root, &["add", "--", "new.bin"]);
        assert!(
            engine
                .open_review("A", None)
                .await
                .unwrap()
                .is_reviewed(Path::new("new.bin"))
        );
        fs::write(root.join("new.bin"), b"corrected\0binary").unwrap();
        let fresh = engine.open_review("A", None).await.unwrap();
        let file = &fresh.changes_since_review()[0];
        assert_eq!((file.additions, file.deletions), (None, None));
        assert!(
            file.patch
                .contains("Binary content or metadata changed since review")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_targets_and_mode_only_changes_never_read_external_content() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        fs::write(dir.path().join("outside"), "private external payload\n").unwrap();
        symlink(dir.path().join("outside"), root.join("link")).unwrap();
        fs::write(root.join("script"), "run\n").unwrap();
        let mut review = engine.open_review("A", None).await.unwrap();
        mark_all(&engine, &mut review);
        fs::remove_file(root.join("link")).unwrap();
        symlink("different-target", root.join("link")).unwrap();
        fs::set_permissions(root.join("script"), fs::Permissions::from_mode(0o755)).unwrap();
        let fresh = engine.open_review("A", None).await.unwrap();
        let link = fresh
            .changes_since_review()
            .iter()
            .find(|file| file.path == Path::new("link"))
            .unwrap();
        assert!(link.patch.contains("+different-target\n"));
        assert!(!link.patch.contains("private external payload"));
        let script = fresh
            .changes_since_review()
            .iter()
            .find(|file| file.path == Path::new("script"))
            .unwrap();
        assert_eq!((script.additions, script.deletions), (Some(0), Some(0)));
        assert!(script.patch.contains("100644 -> 100755"));
    }

    #[test]
    fn hunks_support_multiple_ranges_crlf_no_final_newline_and_patch_like_text() {
        let base = Entry {
            mode: "100644".into(),
            content: b"one\r\n-- old\r\nthree\r\nfour\r\ntail".to_vec(),
        };
        let patch = "diff --git a/x b/x\n@@ -1,2 +1,2 @@\n one\r\n--- old\r\n+++ new diff --git literal\r\n@@ -5 +5 @@\n-tail\n\\ No newline at end of file\n+new tail\n\\ No newline at end of file\n";
        let after = reconstruct(Some(base), patch).unwrap().unwrap();
        assert_eq!(
            after.content,
            b"one\r\n++ new diff --git literal\r\nthree\r\nfour\r\nnew tail"
        );
        let added = reconstruct(
            None,
            "diff --git a/x b/x\nnew file mode 100644\n@@ -0,0 +1 @@\n+new\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(added.content, b"new\n");
        let empty = reconstruct(
            None,
            "diff --git a/x b/x\nnew file mode 100644\nindex 0000..1234\n",
        )
        .unwrap()
        .unwrap();
        assert!(empty.content.is_empty());
    }

    #[test]
    fn malformed_hunks_are_rejected_instead_of_inventing_reviewed_content() {
        for patch in [
            "not a patch",
            "diff --git a/x b/x\n@@ -9 +9 @@\n-old\n+new\n",
            "diff --git a/x b/x\n@@ -1 +1 @@\n-wrong\n+new\n",
            "diff --git a/x b/x\n@@ -1,2 +1 @@\n-old\n+new\n",
            "diff --git a/x b/x\nnew mode 999999\n",
        ] {
            assert!(
                reconstruct(
                    Some(Entry {
                        mode: "100644".into(),
                        content: b"old\n".to_vec()
                    }),
                    patch
                )
                .is_err(),
                "{patch}"
            );
        }
    }

    #[tokio::test]
    async fn real_git_hunks_reconstruct_exact_bytes_and_preserve_header_like_content() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        let base = (0..30).map(|i| format!("line {i}\r\n")).collect::<String>();
        fs::write(root.join("source.txt"), &base).unwrap();
        git(&root, &["add", "--", "source.txt"]);
        git(&root, &["commit", "-m", "Synthetic multiline base"]);
        let reviewed = base
            .replace("line 1", "-- patch-like diff --git text")
            .replace("line 29\r\n", "tail without newline");
        fs::write(root.join("source.txt"), &reviewed).unwrap();
        let mut session = engine.open_review("A", None).await.unwrap();
        let captured = engine
            .git
            .captured_entry(&session, &session.diff.files[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(captured.content, reviewed.as_bytes());
        mark_all(&engine, &mut session);
        let corrected = reviewed.replace(
            "-- patch-like diff --git text",
            "++ corrected diff --git text",
        );
        fs::write(root.join("source.txt"), corrected).unwrap();
        let fresh = engine.open_review("A", None).await.unwrap();
        let delta = &fresh.changes_since_review()[0];
        assert!(
            delta.patch.contains("--- patch-like diff --git text\r\n"),
            "{}",
            delta.patch
        );
        assert!(
            delta.patch.contains("+++ corrected diff --git text\r\n"),
            "{}",
            delta.patch
        );
        assert!(delta.additions.is_some());
    }

    #[tokio::test]
    async fn invalid_snapshot_text_is_explicitly_unavailable_and_read_only() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        fs::write(root.join("source.txt"), "reviewed\n").unwrap();
        let mut session = engine.open_review("A", None).await.unwrap();
        mark_all(&engine, &mut session);
        let state_path = engine.store.review_path();
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        let patch = state["reviews"][0]["files"][0]["patch"]
            .as_str()
            .unwrap()
            .replace("-old\n", "-invalid context\n");
        state["reviews"][0]["files"][0]["patch"] = patch.into();
        fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
        let saved = fs::read(&state_path).unwrap();
        fs::write(root.join("source.txt"), "corrected\n").unwrap();
        let fresh = engine.open_review("A", None).await.unwrap();
        assert_eq!(
            fresh.status(Path::new("source.txt")),
            ReviewStatus::ChangedAfterReview
        );
        assert_eq!(fresh.changes_since_review()[0].additions, None);
        assert!(
            fresh.changes_since_review()[0]
                .patch
                .contains("Since-review text unavailable")
        );
        assert_eq!(fs::read(&state_path).unwrap(), saved);
    }

    #[tokio::test]
    async fn externally_changed_snapshots_do_not_become_reviewed_in_a_stale_session() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        fs::write(root.join("source.txt"), "first capture\n").unwrap();
        fs::write(root.join("second.txt"), "new file\n").unwrap();
        let mut stale = engine.open_review("A", None).await.unwrap();
        fs::write(root.join("source.txt"), "other process capture\n").unwrap();
        let mut other = engine.open_review("A", None).await.unwrap();
        engine
            .set_file_reviewed(&mut other, Path::new("source.txt"), true)
            .unwrap();
        engine
            .set_file_reviewed(&mut stale, Path::new("second.txt"), true)
            .unwrap();
        assert_eq!(
            stale.status(Path::new("source.txt")),
            ReviewStatus::ChangedAfterReview
        );
        assert!(
            stale.changes_since_review()[0]
                .patch
                .contains("another process")
        );
        assert!(
            engine
                .open_review("A", None)
                .await
                .unwrap()
                .is_reviewed(Path::new("source.txt"))
        );
    }

    #[tokio::test]
    async fn submodule_pointer_corrections_compare_to_reviewed_pointer_not_original_base() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        let module = root.join("module");
        fs::create_dir(&module).unwrap();
        git(&module, &["init", "-b", "main"]);
        fs::write(module.join("file"), "initial\n").unwrap();
        git(&module, &["add", "file"]);
        git(&module, &["commit", "-m", "Synthetic module base"]);
        let first = git(&module, &["rev-parse", "HEAD"]).trim().to_owned();
        git(
            &root,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{first},module"),
            ],
        );
        git(&root, &["commit", "-m", "Synthetic gitlink base"]);
        fs::write(module.join("file"), "reviewed\n").unwrap();
        git(&module, &["add", "file"]);
        git(
            &module,
            &["commit", "-m", "Synthetic module implementation"],
        );
        let reviewed = git(&module, &["rev-parse", "HEAD"]).trim().to_owned();
        let mut session = engine.open_review("A", None).await.unwrap();
        mark_all(&engine, &mut session);
        fs::write(module.join("file"), "corrected\n").unwrap();
        git(&module, &["add", "file"]);
        git(&module, &["commit", "-m", "Synthetic module correction"]);
        let corrected = git(&module, &["rev-parse", "HEAD"]).trim().to_owned();
        let fresh = engine.open_review("A", None).await.unwrap();
        let delta = &fresh.changes_since_review()[0];
        assert!(
            delta
                .patch
                .contains(&format!("-Subproject commit {reviewed}")),
            "{}",
            delta.patch
        );
        assert!(
            delta
                .patch
                .contains(&format!("+Subproject commit {corrected}")),
            "{}",
            delta.patch
        );
        assert!(!delta.patch.contains(&format!("-Subproject commit {first}")));
    }
}
