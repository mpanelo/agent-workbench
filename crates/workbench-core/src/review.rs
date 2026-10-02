use std::{
    collections::HashMap,
    error::Error,
    fmt, io,
    path::{Path, PathBuf},
};

use crate::review_store::ReviewStore;
use crate::{ChangedFile, Engine, GitError, WorkItemDiff, WorkItemError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewStatus {
    Unreviewed,
    Reviewed,
    ChangedAfterReview,
}

/// Marks refer to the exact captured diff, not edits made after capture.
/// Use Engine::open_review and Engine::set_file_reviewed for durable state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewSession {
    pub diff: WorkItemDiff,
    pub(crate) reviewed: HashMap<PathBuf, ChangedFile>,
    pub(crate) since_review: Option<Vec<ChangedFile>>,
}

impl ReviewSession {
    pub fn new(diff: WorkItemDiff) -> Self {
        Self {
            diff,
            reviewed: HashMap::new(),
            since_review: None,
        }
    }

    pub fn status(&self, path: &Path) -> ReviewStatus {
        if let Some(changes) = &self.since_review {
            return if !self.reviewed.contains_key(path) {
                ReviewStatus::Unreviewed
            } else if changes.iter().any(|file| file.path == path) {
                ReviewStatus::ChangedAfterReview
            } else {
                ReviewStatus::Reviewed
            };
        }
        match self.reviewed.get(path) {
            None => ReviewStatus::Unreviewed,
            Some(snapshot)
                if self
                    .diff
                    .files
                    .iter()
                    .any(|file| file.path == path && file == snapshot) =>
            {
                ReviewStatus::Reviewed
            }
            Some(_) => ReviewStatus::ChangedAfterReview,
        }
    }

    pub fn is_reviewed(&self, path: &Path) -> bool {
        self.status(path) == ReviewStatus::Reviewed
    }

    pub fn progress(&self) -> (usize, usize) {
        (
            self.diff
                .files
                .iter()
                .filter(|file| self.is_reviewed(&file.path))
                .count(),
            self.diff.files.len(),
        )
    }

    pub fn changed_after_review(&self) -> usize {
        if let Some(changes) = &self.since_review {
            return changes.len();
        }
        self.diff
            .files
            .iter()
            .filter(|file| self.status(&file.path) == ReviewStatus::ChangedAfterReview)
            .count()
    }

    /// Captured file-content differences from saved review snapshots, including
    /// reviewed paths that reverted to the base or disappeared from its diff.
    pub fn changes_since_review(&self) -> &[ChangedFile] {
        self.since_review.as_deref().unwrap_or(&[])
    }
}

#[derive(Debug)]
pub enum ReviewError {
    Git(GitError),
    WorkItems(WorkItemError),
    Io { path: PathBuf, source: io::Error },
    Busy(PathBuf),
    Invalid(String),
}

impl fmt::Display for ReviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => error.fmt(f),
            Self::WorkItems(error) => error.fmt(f),
            Self::Io { path, source } => write!(
                f,
                "Could not access review state {}: {source}. No review mark was changed.",
                path.display()
            ),
            Self::Busy(path) => write!(
                f,
                "Review state {} is being updated by another process; retry the mark.",
                path.display()
            ),
            Self::Invalid(message) => write!(
                f,
                "Invalid review state: {message}. No review mark was changed."
            ),
        }
    }
}

impl Error for ReviewError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Git(error) => Some(error),
            Self::WorkItems(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl Engine {
    /// Capture Git changes and restore marks for the same item, canonical
    /// workspace and resolved base commit. Opening is read-only.
    pub async fn open_review(
        &self,
        id: &str,
        base: Option<&str>,
    ) -> Result<ReviewSession, ReviewError> {
        let diff = self.diff(id, base).await.map_err(ReviewError::Git)?;
        let store = ReviewStore::new(self.store.review_path());
        let mut session = tokio::task::spawn_blocking(move || store.restore(diff))
            .await
            .map_err(|error| {
                ReviewError::Invalid(format!("review-state read task stopped: {error}"))
            })??;
        session.since_review = Some(
            self.git
                .changes_since_review(&session)
                .await
                .map_err(ReviewError::Git)?,
        );
        Ok(session)
    }

    /// Persist the selected file's captured diff before updating the session.
    /// Never mark unseen edits made after this capture reviewed.
    pub fn set_file_reviewed(
        &self,
        session: &mut ReviewSession,
        path: &Path,
        reviewed: bool,
    ) -> Result<(), ReviewError> {
        // Prevent a mark validated against the old ID racing a rename's copy.
        let _registry_lock = self.store.lock().map_err(ReviewError::WorkItems)?;
        let item = self
            .work_items()
            .map_err(ReviewError::WorkItems)?
            .into_iter()
            .find(|item| item.id == session.diff.work_item_id)
            .ok_or_else(|| ReviewError::Invalid("work item is no longer registered".into()))?;
        let workspace =
            std::fs::canonicalize(&item.workspace).map_err(|source| ReviewError::Io {
                path: item.workspace,
                source,
            })?;
        if workspace != session.diff.workspace {
            return Err(ReviewError::Invalid(
                "registered workspace changed; reopen review".into(),
            ));
        }
        let captured = session.diff.files.iter().find(|file| file.path == path);
        let file = captured
            .or_else(|| {
                session
                    .changes_since_review()
                    .iter()
                    .find(|file| file.path == path)
                    .and_then(|_| session.reviewed.get(path))
            })
            .ok_or_else(|| {
                ReviewError::Invalid("selected path is not in the captured diff".into())
            })?;
        // A reverted/removed path has no remaining base diff to snapshot. Drop
        // its obsolete mark after the developer acknowledges the correction.
        let restored = ReviewStore::new(self.store.review_path()).set(
            &session.diff,
            file,
            reviewed && captured.is_some(),
        )?;
        if let Some(changes) = &mut session.since_review {
            changes.retain(|file| {
                file.path != path
                    && restored.contains_key(&file.path)
                    && !session
                        .diff
                        .files
                        .iter()
                        .any(|current| restored.get(&file.path) == Some(current))
            });
            for (other_path, snapshot) in &restored {
                if other_path != path
                    && session.reviewed.get(other_path) != Some(snapshot)
                    && !session.diff.files.contains(snapshot)
                {
                    changes.retain(|file| &file.path != other_path);
                    changes.push(ChangedFile {
                        path: other_path.clone(), old_path: None, kind: snapshot.kind,
                        additions: None, deletions: None,
                        patch: "Review snapshot changed in another process. Reload (r) to compare with it.\n".into(),
                    });
                }
            }
            changes.sort_by(|a, b| a.path.cmp(&b.path));
        }
        session.reviewed = restored;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        WorkItemKind,
        git::tests::{fixture, git},
    };
    use std::fs;

    #[tokio::test]
    async fn renamed_items_restore_reviews_reject_old_sessions_and_keep_workspace_untouched() {
        let (dir, engine) = fixture();
        let source = dir.path().join("workspace/source.txt");
        fs::write(&source, "reviewed code\n").unwrap();
        let path = Path::new("source.txt");
        let mut old_session = engine.open_review("A", None).await.unwrap();
        engine
            .set_file_reviewed(&mut old_session, path, true)
            .unwrap();
        let target = engine.work_items().unwrap().remove(0);
        let disk = fs::read(&source).unwrap();
        let status = git(&target.workspace, &["status", "--porcelain"]);
        // Exercise a retry after review copying completed but registry saving did not.
        ReviewStore::new(engine.store.review_path())
            .copy_identity("A", "Renamed λ🙂")
            .unwrap();
        let updated = engine.rename_work_item(&target, "Renamed λ🙂").unwrap();
        assert_eq!(updated.pane_id, target.pane_id);
        assert_eq!(updated.workspace, target.workspace);
        let restarted = Engine::new(dir.path().join("items.json"));
        let mut fresh = restarted.open_review("Renamed λ🙂", None).await.unwrap();
        assert_eq!(fresh.status(path), ReviewStatus::Reviewed);
        let original = old_session.clone();
        assert!(
            engine
                .set_file_reviewed(&mut old_session, path, false)
                .unwrap_err()
                .to_string()
                .contains("no longer registered")
        );
        assert_eq!(old_session, original);
        let registry_lock = engine.store.lock().unwrap();
        assert!(matches!(
            engine.set_file_reviewed(&mut fresh, path, false),
            Err(ReviewError::WorkItems(crate::WorkItemError::Busy(_)))
        ));
        assert_eq!(fresh.status(path), ReviewStatus::Reviewed);
        drop(registry_lock);
        assert_eq!(fs::read(&source).unwrap(), disk);
        assert_eq!(git(&target.workspace, &["status", "--porcelain"]), status);
        fs::write(&source, "later agent edits\n").unwrap();
        let changed = restarted.open_review("Renamed λ🙂", None).await.unwrap();
        assert_eq!(changed.status(path), ReviewStatus::ChangedAfterReview);
    }

    #[tokio::test]
    async fn marks_survive_restart_reload_and_unmark_without_changing_registrations() {
        let (dir, engine) = fixture();
        let workspace = dir.path().join("workspace");
        fs::write(workspace.join("source.txt"), "review this\n").unwrap();
        fs::write(workspace.join("second λ.txt"), "new file\n").unwrap();
        let registrations = fs::read(dir.path().join("items.json")).unwrap();
        let state = engine.store.review_path();
        let mut review = engine.open_review("A", None).await.unwrap();
        assert!(!state.exists());
        assert_eq!(review.progress(), (0, 2));
        engine
            .set_file_reviewed(&mut review, Path::new("source.txt"), true)
            .unwrap();
        assert_eq!(review.progress(), (1, 2));
        let restarted = Engine::new(dir.path().join("items.json"));
        let mut fresh = restarted.open_review("A", None).await.unwrap();
        assert_eq!(fresh.progress(), (1, 2));
        assert!(fresh.is_reviewed(Path::new("source.txt")));
        assert_eq!(
            fresh.status(Path::new("second λ.txt")),
            ReviewStatus::Unreviewed
        );
        // Moving changes into the index alone does not invalidate identical code.
        git(&workspace, &["add", "--", "source.txt"]);
        assert_eq!(
            engine.open_review("A", None).await.unwrap().progress(),
            (1, 2)
        );
        restarted
            .set_file_reviewed(&mut fresh, Path::new("source.txt"), false)
            .unwrap();
        assert_eq!(
            engine.open_review("A", None).await.unwrap().progress(),
            (0, 2)
        );
        assert!(
            engine
                .set_file_reviewed(&mut fresh, Path::new("../outside"), true)
                .is_err()
        );
        assert_eq!(
            fs::read(dir.path().join("items.json")).unwrap(),
            registrations
        );
        assert!(!git(&workspace, &["status", "--porcelain"]).is_empty());
    }

    #[tokio::test]
    async fn reviewed_snapshot_is_the_seen_capture_not_unseen_new_agent_edits() {
        let (dir, engine) = fixture();
        let path = Path::new("source.txt");
        let working = dir.path().join("workspace/source.txt");
        fs::write(&working, "seen code\n").unwrap();
        let mut seen = engine.open_review("A", None).await.unwrap();
        fs::write(&working, "unseen agent edit\n").unwrap();
        engine.set_file_reviewed(&mut seen, path, true).unwrap();
        assert_eq!(seen.status(path), ReviewStatus::Reviewed);
        let mut fresh = engine.open_review("A", None).await.unwrap();
        assert_eq!(fresh.status(path), ReviewStatus::ChangedAfterReview);
        assert_eq!(fresh.progress(), (0, 1));
        assert_eq!(fresh.changed_after_review(), 1);
        engine.set_file_reviewed(&mut fresh, path, true).unwrap();
        assert_eq!(
            engine.open_review("A", None).await.unwrap().progress(),
            (1, 1)
        );
        fs::write(&working, "seen code\n").unwrap();
        assert_eq!(
            engine.open_review("A", None).await.unwrap().status(path),
            ReviewStatus::ChangedAfterReview
        );
    }

    #[tokio::test]
    async fn state_is_isolated_by_item_workspace_and_resolved_base_not_ref_spelling() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        let base = git(&root, &["rev-parse", "HEAD"]).trim().to_owned();
        fs::write(root.join("source.txt"), "change\n").unwrap();
        let mut review = engine.open_review("A", None).await.unwrap();
        engine
            .set_file_reviewed(&mut review, Path::new("source.txt"), true)
            .unwrap();
        assert_eq!(
            engine
                .open_review("A", Some(&base))
                .await
                .unwrap()
                .progress(),
            (1, 1)
        );
        let mut other = engine.work_items().unwrap().remove(0);
        other.id = "B".into();
        other.kind = WorkItemKind::ExternalReview;
        engine.register_work_item(other.clone()).unwrap();
        assert_eq!(
            engine.open_review("B", None).await.unwrap().progress(),
            (0, 1)
        );
        let linked = dir.path().join("linked");
        git(
            &root,
            &["worktree", "add", "-b", "linked", linked.to_str().unwrap()],
        );
        fs::write(linked.join("source.txt"), "change\n").unwrap();
        other.workspace = linked;
        other.id = "C".into();
        engine.register_work_item(other).unwrap();
        assert_eq!(
            engine.open_review("C", None).await.unwrap().progress(),
            (0, 1)
        );
        git(&root, &["add", "--", "source.txt"]);
        git(&root, &["commit", "-m", "Synthetic second revision"]);
        fs::write(root.join("source.txt"), "new change\n").unwrap();
        assert_eq!(
            engine.open_review("A", None).await.unwrap().progress(),
            (0, 1)
        );
        // Changing HEAD does not destroy the previous explicit-base context.
        assert_eq!(
            engine
                .open_review("A", Some(&base))
                .await
                .unwrap()
                .status(Path::new("source.txt")),
            ReviewStatus::ChangedAfterReview
        );
    }

    #[tokio::test]
    async fn binary_rename_deletion_and_unicode_snapshots_are_durable() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        fs::write(root.join("binary"), b"first\0binary change").unwrap();
        fs::remove_file(root.join("-delete.txt")).unwrap();
        git(&root, &["mv", "--", "rename source.txt", "renamed λ.txt"]);
        fs::write(root.join("new\tλ.txt"), "untracked\n").unwrap();
        let mut review = engine.open_review("A", None).await.unwrap();
        let paths: Vec<_> = review
            .diff
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect();
        for path in &paths {
            engine.set_file_reviewed(&mut review, path, true).unwrap();
        }
        assert_eq!(review.progress(), (4, 4));
        assert_eq!(
            engine.open_review("A", None).await.unwrap().progress(),
            (4, 4)
        );
        fs::write(root.join("binary"), b"other\0binary change").unwrap();
        let changed = engine.open_review("A", None).await.unwrap();
        assert_eq!(
            changed.status(Path::new("binary")),
            ReviewStatus::ChangedAfterReview
        );
        assert_eq!(changed.progress(), (3, 4));
        // Removed files leave the current progress count; old snapshots remain
        // available should exactly the reviewed changes reappear later.
        fs::remove_file(root.join("new\tλ.txt")).unwrap();
        assert_eq!(
            engine.open_review("A", None).await.unwrap().progress(),
            (2, 3)
        );
    }

    #[tokio::test]
    async fn stale_sessions_and_independent_engines_merge_file_marks() {
        let (dir, first) = fixture();
        fs::write(dir.path().join("workspace/source.txt"), "change\n").unwrap();
        fs::write(dir.path().join("workspace/other.txt"), "new\n").unwrap();
        let second = Engine::new(dir.path().join("items.json"));
        let mut a = first.open_review("A", None).await.unwrap();
        let mut b = second.open_review("A", None).await.unwrap();
        first
            .set_file_reviewed(&mut a, Path::new("source.txt"), true)
            .unwrap();
        second
            .set_file_reviewed(&mut b, Path::new("other.txt"), true)
            .unwrap();
        assert_eq!(b.progress(), (2, 2));
        first
            .set_file_reviewed(&mut a, Path::new("source.txt"), false)
            .unwrap();
        assert_eq!(
            second.open_review("A", None).await.unwrap().progress(),
            (1, 2)
        );
    }

    #[tokio::test]
    async fn corrupt_busy_and_write_failures_preserve_disk_and_in_memory_marks() {
        let (dir, engine) = fixture();
        fs::write(dir.path().join("workspace/source.txt"), "change\n").unwrap();
        let path = Path::new("source.txt");
        let mut review = engine.open_review("A", None).await.unwrap();
        engine.set_file_reviewed(&mut review, path, true).unwrap();
        let original = review.clone();
        let state = engine.store.review_path();
        let before = fs::read(&state).unwrap();
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(state.with_file_name("items.json.reviews.json.lock"))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&lock_file).unwrap();
        let lock = crate::work_items::StateLock(lock_file);
        assert!(matches!(
            engine.set_file_reviewed(&mut review, path, false),
            Err(ReviewError::Busy(_))
        ));
        assert_eq!(review, original);
        assert_eq!(fs::read(&state).unwrap(), before);
        drop(lock);
        for bytes in [b"broken".as_slice(), b"{\"version\":9,\"reviews\":[]}"] {
            fs::write(&state, bytes).unwrap();
            assert!(engine.open_review("A", None).await.is_err());
            assert!(engine.set_file_reviewed(&mut review, path, false).is_err());
            assert_eq!(review, original);
            assert_eq!(fs::read(&state).unwrap(), bytes);
        }
        fs::remove_file(&state).unwrap();
        fs::create_dir(&state).unwrap();
        assert!(engine.set_file_reviewed(&mut review, path, false).is_err());
        assert_eq!(review, original);
        assert!(
            engine
                .set_file_reviewed(&mut review, Path::new("not-in-diff"), true)
                .is_err()
        );
    }

    #[tokio::test]
    async fn independent_process_restores_review_snapshot() {
        // Child invocation of this exact test is a separate reader process.
        const STATE_ENV: &str = "WORKBENCH_REVIEW_CHILD_STATE";
        if let Some(state) = std::env::var_os(STATE_ENV) {
            let restored = Engine::new(state).open_review("A", None).await.unwrap();
            assert_eq!(restored.progress(), (1, 1));
            return;
        }
        let (dir, engine) = fixture();
        fs::write(dir.path().join("workspace/source.txt"), "change\n").unwrap();
        let mut review = engine.open_review("A", None).await.unwrap();
        engine
            .set_file_reviewed(&mut review, Path::new("source.txt"), true)
            .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "review::tests::independent_process_restores_review_snapshot",
                "--nocapture",
            ])
            .env(STATE_ENV, dir.path().join("items.json"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
