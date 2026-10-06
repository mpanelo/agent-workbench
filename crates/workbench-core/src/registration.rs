//! Explicit registration from a discovered pane; no workspace/session creation.

use std::{error::Error, fmt, path::PathBuf};

use crate::{DiscoveryError, Engine, Pane, Snapshot, WorkItem, WorkItemError, WorkItemKind};

#[derive(Clone, Debug)]
pub struct RegistrationDraft {
    pub item: WorkItem,
    /// Original directory is retained to reject a retargeted pane before saving.
    pub pane_directory: Option<PathBuf>,
    pub notice: Option<String>,
}

#[derive(Debug)]
pub enum RegistrationError {
    State(WorkItemError),
    Discovery(DiscoveryError),
    MissingPane(String),
    PaneChanged,
    AlreadyRegistered(String),
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(error) => error.fmt(f),
            Self::Discovery(error) => error.fmt(f),
            Self::MissingPane(id) => {
                write!(f, "Pane {id} is no longer available; select a live pane.")
            }
            Self::PaneChanged => f.write_str(
                "Pane directory changed while registering; cancel and select the pane again.",
            ),
            Self::AlreadyRegistered(id) => write!(
                f,
                "This pane is already registered as {id:?}. Cancel, then press w to use its work item."
            ),
        }
    }
}

impl Error for RegistrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::State(error) => Some(error),
            Self::Discovery(error) => Some(error),
            _ => None,
        }
    }
}

impl Engine {
    /// Suggest editable fields from a fresh pane discovery and read-only Git metadata.
    /// Preparing a draft never creates a registration or writes a file.
    pub async fn prepare_pane_registration(
        &self,
        pane_id: &str,
    ) -> Result<RegistrationDraft, RegistrationError> {
        let snapshot = self
            .discover()
            .await
            .map_err(RegistrationError::Discovery)?;
        self.registration_from_snapshot(&snapshot, pane_id).await
    }

    async fn registration_from_snapshot(
        &self,
        snapshot: &Snapshot,
        pane_id: &str,
    ) -> Result<RegistrationDraft, RegistrationError> {
        let (pane, window_name) = selected_pane(snapshot, pane_id)?;
        let items = self.work_items().map_err(RegistrationError::State)?;
        check_unregistered(&items, pane_id)?;
        let directory = pane.working_directory.clone();
        let mut workspace = directory.clone().unwrap_or_default();
        let mut repository = workspace.clone();
        let mut branch = None;
        let notice = if let Some(directory) = directory.as_ref() {
            match self.git.registration_metadata(directory).await {
                Ok((repo, root, detected_branch)) => {
                    repository = repo;
                    workspace = root;
                    branch = detected_branch;
                    None
                }
                Err(_) => Some("Git metadata is unavailable; paths default to the pane directory. Check or edit them before saving.".into()),
            }
        } else {
            Some("Pane directory is unavailable. Enter absolute repository and workspace paths before saving.".into())
        };
        let suggestion = branch
            .as_deref()
            .unwrap_or(window_name)
            .chars()
            .filter(|ch| !ch.is_control())
            .collect::<String>();
        let suggestion = suggestion.trim();
        let suggestion = if suggestion.is_empty() {
            "Work item"
        } else {
            suggestion
        };
        let stem: String = suggestion
            .chars()
            .map(|ch| {
                if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                    ch
                } else {
                    '-'
                }
            })
            .take(100)
            .collect();
        let stem = stem.trim_matches('-');
        let stem = if stem.is_empty() { "work-item" } else { stem };
        let mut id = stem.to_owned();
        let mut suffix = 2;
        while items.iter().any(|item| item.id == id) {
            id = format!("{stem}-{suffix}");
            suffix += 1;
        }
        Ok(RegistrationDraft {
            item: WorkItem {
                id,
                title: String::new(),
                repository,
                workspace,
                branch,
                kind: WorkItemKind::Implementation,
                pane_id: pane_id.into(),
            },
            pane_directory: directory,
            notice,
        })
    }

    /// Explicitly save edited fields, after rechecking the original pane binding.
    /// Manual CLI registration remains usable offline via register_work_item.
    pub async fn register_discovered_work_item(
        &self,
        draft: RegistrationDraft,
    ) -> Result<WorkItem, RegistrationError> {
        let snapshot = self
            .discover()
            .await
            .map_err(RegistrationError::Discovery)?;
        validate_binding(&snapshot, &draft)?;
        check_unregistered(
            &self.work_items().map_err(RegistrationError::State)?,
            &draft.item.pane_id,
        )?;
        self.register_work_item(draft.item.clone())
            .map_err(RegistrationError::State)?;
        Ok(draft.item)
    }
}

fn selected_pane<'a>(
    snapshot: &'a Snapshot,
    id: &str,
) -> Result<(&'a Pane, &'a str), RegistrationError> {
    snapshot
        .sessions
        .iter()
        .flat_map(|session| &session.windows)
        .find_map(|window| {
            window
                .panes
                .iter()
                .find(|pane| pane.id == id)
                .map(|pane| (pane, window.name.as_str()))
        })
        .ok_or_else(|| RegistrationError::MissingPane(id.into()))
}

fn check_unregistered(items: &[WorkItem], pane_id: &str) -> Result<(), RegistrationError> {
    if let Some(item) = items.iter().find(|item| item.pane_id == pane_id) {
        return Err(RegistrationError::AlreadyRegistered(item.id.clone()));
    }
    Ok(())
}

fn validate_binding(
    snapshot: &Snapshot,
    draft: &RegistrationDraft,
) -> Result<(), RegistrationError> {
    let (pane, _) = selected_pane(snapshot, &draft.item.pane_id)?;
    if pane.working_directory != draft.pane_directory {
        return Err(RegistrationError::PaneChanged);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{fixture, git};
    use crate::{Session, Window};

    fn snapshot(directory: Option<PathBuf>) -> Snapshot {
        Snapshot {
            sessions: vec![Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![Window {
                    id: "@1".into(),
                    index: 1,
                    name: "Task λ".into(),
                    panes: vec![Pane {
                        id: "%1".into(),
                        index: 0,
                        title: "host name".into(),
                        current_command: Some("codex".into()),
                        working_directory: directory,
                    }],
                }],
            }],
        }
    }

    #[tokio::test]
    async fn descriptions_start_empty_without_changing_id_or_branch_suggestions() {
        let (dir, _) = fixture();
        let root = dir.path().join("workspace");
        let branch = "a".repeat(crate::MAX_SHORT_DESCRIPTION_CHARS + 10);
        git(&root, &["checkout", "-b", &branch]);
        let engine = Engine::new(dir.path().join("new-items.json"));
        let draft = engine
            .registration_from_snapshot(&snapshot(Some(root)), "%1")
            .await
            .unwrap();
        assert!(draft.item.title.is_empty());
        assert_eq!(draft.item.id, "a".repeat(100));
        assert_eq!(draft.item.branch.as_deref(), Some(branch.as_str()));
        assert!(draft.notice.is_none());
        let mut fallback = snapshot(None);
        fallback.sessions[0].windows[0].name = "🙂".repeat(crate::MAX_SHORT_DESCRIPTION_CHARS + 1);
        let fallback = engine
            .registration_from_snapshot(&fallback, "%1")
            .await
            .unwrap();
        assert!(fallback.item.title.is_empty());
        assert_eq!(fallback.item.id, "work-item");
        assert!(!fallback.notice.unwrap().contains("Short Description"));
        engine.register_work_item(draft.item).unwrap();
    }

    #[tokio::test]
    async fn prepares_checkout_and_nested_directory_without_writing_state() {
        let (dir, _) = fixture();
        let root = dir.path().join("workspace");
        let nested = root.join("nested directory");
        std::fs::create_dir(&nested).unwrap();
        let engine = Engine::new(dir.path().join("new-items.json"));
        let draft = engine
            .registration_from_snapshot(&snapshot(Some(nested.clone())), "%1")
            .await
            .unwrap();
        assert_eq!(draft.item.workspace, std::fs::canonicalize(&root).unwrap());
        assert_eq!(draft.item.repository, draft.item.workspace);
        assert_eq!(draft.item.branch.as_deref(), Some("main"));
        assert_eq!(draft.item.id, "main");
        assert!(draft.item.title.is_empty());
        assert_eq!(draft.pane_directory, Some(nested));
        assert!(draft.notice.is_none());
        assert!(!dir.path().join("new-items.json").exists());
        engine.register_work_item(draft.item).unwrap();
        assert!(matches!(
            engine
                .registration_from_snapshot(&snapshot(Some(root)), "%1")
                .await,
            Err(RegistrationError::AlreadyRegistered(_))
        ));
    }

    #[tokio::test]
    async fn linked_worktree_detached_and_unborn_metadata_are_supported() {
        let (dir, _) = fixture();
        let root = dir.path().join("workspace");
        let linked = dir.path().join("linked checkout λ");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "feature/task",
                linked.to_str().unwrap(),
            ],
        );
        let engine = Engine::new(dir.path().join("new-items.json"));
        let draft = engine
            .registration_from_snapshot(&snapshot(Some(linked.clone())), "%1")
            .await
            .unwrap();
        assert_eq!(draft.item.repository, std::fs::canonicalize(&root).unwrap());
        assert_eq!(
            draft.item.workspace,
            std::fs::canonicalize(&linked).unwrap()
        );
        assert_eq!(draft.item.id, "feature-task");
        assert_eq!(draft.item.branch.as_deref(), Some("feature/task"));
        git(&linked, &["checkout", "--detach"]);
        assert!(
            engine
                .registration_from_snapshot(&snapshot(Some(linked)), "%1")
                .await
                .unwrap()
                .item
                .branch
                .is_none()
        );
        let unborn = dir.path().join("unborn");
        std::fs::create_dir(&unborn).unwrap();
        git(&unborn, &["init", "-b", "initial"]);
        let draft = engine
            .registration_from_snapshot(&snapshot(Some(unborn)), "%1")
            .await
            .unwrap();
        assert_eq!(draft.item.branch.as_deref(), Some("initial"));
        assert!(draft.notice.is_none());
    }

    #[tokio::test]
    async fn fallback_missing_panes_changed_binding_and_collisions_are_safe() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("items.json"));
        let state = snapshot(Some(dir.path().into()));
        let draft = engine
            .registration_from_snapshot(&state, "%1")
            .await
            .unwrap();
        assert!(draft.notice.is_some());
        assert_eq!(draft.item.id, "Task-λ");
        assert_eq!(draft.item.workspace, dir.path());
        assert!(validate_binding(&state, &draft).is_ok());
        assert!(matches!(
            validate_binding(&Snapshot::default(), &draft),
            Err(RegistrationError::MissingPane(_))
        ));
        assert!(matches!(
            validate_binding(&snapshot(None), &draft),
            Err(RegistrationError::PaneChanged)
        ));
        let mut saved = draft.item.clone();
        saved.pane_id = "%2".into();
        engine.register_work_item(saved).unwrap();
        assert_eq!(
            engine
                .registration_from_snapshot(&state, "%1")
                .await
                .unwrap()
                .item
                .id,
            "Task-λ-2"
        );
        let missing = engine
            .registration_from_snapshot(&snapshot(None), "%1")
            .await
            .unwrap();
        assert!(missing.item.workspace.as_os_str().is_empty());
        assert!(missing.notice.is_some());
        assert!(matches!(
            engine.registration_from_snapshot(&state, "%9").await,
            Err(RegistrationError::MissingPane(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_registration_rechecks_binding_and_preserves_state_on_errors() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let (dir, _) = fixture();
        let state_file = dir.path().join("registered.json");
        let output_file = dir.path().join("discovery");
        let executable = dir.path().join("tmux-fixture");
        let quoted_path = output_file.to_string_lossy().replace('\'', "'\\''");
        fs::write(
            &executable,
            format!("#!/bin/sh\nexec /bin/cat '{quoted_path}'\n"),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let root = dir.path().join("workspace");
        let discovered = format!(
            "$1\x1fmain\x1f@1\x1f1\x1ftask\x1f%1\x1f0\x1fAgent\x1fcodex\x1f{}\x1e\n",
            root.display()
        );
        fs::write(&output_file, &discovered).unwrap();
        let mut engine = Engine::new(&state_file);
        engine.tmux.executable = executable.into();
        let draft = engine.prepare_pane_registration("%1").await.unwrap();
        assert!(!state_file.exists());
        fs::write(&output_file, "").unwrap();
        assert!(matches!(
            engine.register_discovered_work_item(draft.clone()).await,
            Err(RegistrationError::MissingPane(_))
        ));
        assert!(!state_file.exists());
        fs::write(
            &output_file,
            discovered.replace(root.to_str().unwrap(), "/different/directory"),
        )
        .unwrap();
        assert!(matches!(
            engine.register_discovered_work_item(draft.clone()).await,
            Err(RegistrationError::PaneChanged)
        ));
        assert!(!state_file.exists());
        fs::write(&output_file, &discovered).unwrap();
        let mut invalid = draft.clone();
        invalid.item.id.clear();
        assert!(matches!(
            engine.register_discovered_work_item(invalid).await,
            Err(RegistrationError::State(_))
        ));
        assert!(!state_file.exists());
        let saved = engine
            .register_discovered_work_item(draft.clone())
            .await
            .unwrap();
        assert!(saved.title.is_empty());
        assert_eq!(Engine::new(&state_file).work_items().unwrap(), vec![saved]);
        let original = fs::read(&state_file).unwrap();
        assert!(matches!(
            engine.register_discovered_work_item(draft.clone()).await,
            Err(RegistrationError::AlreadyRegistered(_))
        ));
        assert_eq!(fs::read(&state_file).unwrap(), original);
        fs::write(&state_file, "corrupt").unwrap();
        assert!(matches!(
            engine.prepare_pane_registration("%1").await,
            Err(RegistrationError::State(_))
        ));
        assert!(matches!(
            engine.register_discovered_work_item(draft).await,
            Err(RegistrationError::State(_))
        ));
        assert_eq!(fs::read(&state_file).unwrap(), b"corrupt");
    }
}
