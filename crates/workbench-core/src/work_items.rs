use std::{
    collections::HashSet,
    env,
    error::Error,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkItem {
    pub id: String,
    pub title: String,
    pub repository: PathBuf,
    pub workspace: PathBuf,
    pub branch: Option<String>,
    pub kind: WorkItemKind,
    pub pane_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkItemKind {
    Implementation,
    ExternalReview,
}

impl fmt::Display for WorkItemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Implementation => "Implementation",
            Self::ExternalReview => "External Review",
        })
    }
}

// M2 deliberately makes no inference about an agent's activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStatus {
    Unknown,
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UNKNOWN")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneAvailability {
    Present,
    Missing,
    Unavailable,
}

impl fmt::Display for PaneAvailability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkItemState {
    pub item: WorkItem,
    pub status: AgentStatus,
    pub pane: PaneAvailability,
}

#[derive(Debug)]
pub enum WorkItemError {
    Io { path: PathBuf, source: io::Error },
    Invalid(String),
    DuplicateId(String),
    Busy(PathBuf),
}

impl fmt::Display for WorkItemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(
                f,
                "Could not access work-item state {}: {source}",
                path.display()
            ),
            Self::Invalid(message) => write!(f, "Invalid work-item state: {message}"),
            Self::DuplicateId(id) => write!(
                f,
                "Work item {id:?} is already registered; choose a unique ID."
            ),
            Self::Busy(path) => write!(
                f,
                "Work-item state {} is being updated by another process; retry registration.",
                path.display()
            ),
        }
    }
}

impl Error for WorkItemError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Explicit override, then XDG state directory, then the user's local state directory.
pub fn default_state_file() -> Result<PathBuf, WorkItemError> {
    state_file_from(
        env::var_os("AGENT_WORKBENCH_STATE_FILE"),
        env::var_os("XDG_STATE_HOME"),
        env::var_os("HOME"),
    )
}

fn state_file_from(
    override_path: Option<std::ffi::OsString>,
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf, WorkItemError> {
    if let Some(path) = override_path.filter(|path| !path.is_empty()) {
        return Ok(path.into());
    }
    let directory = xdg
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".local/state"))
        })
        .ok_or_else(|| {
            WorkItemError::Invalid(
                "No state directory available. Set --state-file or AGENT_WORKBENCH_STATE_FILE."
                    .into(),
            )
        })?;
    Ok(directory.join("agent-workbench/work-items.json"))
}

fn validate(item: &WorkItem) -> Result<(), WorkItemError> {
    for (name, value) in [("ID", item.id.as_str()), ("title", item.title.as_str())] {
        if value.trim().is_empty() || value != value.trim() || value.chars().any(char::is_control) {
            return Err(WorkItemError::Invalid(format!(
                "{name} must be nonempty, with no control characters or surrounding whitespace"
            )));
        }
    }
    if !item.repository.is_absolute() || !item.workspace.is_absolute() {
        return Err(WorkItemError::Invalid(
            "repository and workspace must be absolute paths".into(),
        ));
    }
    if item
        .branch
        .as_ref()
        .is_some_and(|branch| branch.trim().is_empty() || branch.chars().any(char::is_control))
    {
        return Err(WorkItemError::Invalid(
            "branch must be nonempty with no control characters when supplied".into(),
        ));
    }
    if !item.pane_id.strip_prefix('%').is_some_and(|number| {
        !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && number.parse::<u64>().is_ok()
    }) {
        return Err(WorkItemError::Invalid(
            "pane ID must be a tmux pane ID such as %14".into(),
        ));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredState {
    version: u32,
    work_items: Vec<WorkItem>,
}

#[derive(Debug)]
pub(crate) struct WorkItemStore {
    path: PathBuf,
}

impl WorkItemStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn io_error(&self, source: io::Error) -> WorkItemError {
        WorkItemError::Io {
            path: self.path.clone(),
            source,
        }
    }

    pub(crate) fn load(&self) -> Result<Vec<WorkItem>, WorkItemError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(self.io_error(error)),
        };
        let state: StoredState = serde_json::from_slice(&bytes).map_err(|error| {
            WorkItemError::Invalid(format!(
                "{}: {error}; the file was not changed",
                self.path.display()
            ))
        })?;
        if state.version != 1 {
            return Err(WorkItemError::Invalid(format!(
                "unsupported schema version {} in {}; the file was not changed",
                state.version,
                self.path.display()
            )));
        }
        let mut ids = HashSet::new();
        for item in &state.work_items {
            validate(item)?;
            if !ids.insert(&item.id) {
                return Err(WorkItemError::DuplicateId(item.id.clone()));
            }
        }
        Ok(state.work_items)
    }

    pub(crate) fn register(&self, item: WorkItem) -> Result<(), WorkItemError> {
        validate(&item)?;
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent).map_err(|error| self.io_error(error))?;
        // A separate lock remains stable while the JSON inode is atomically replaced.
        let mut lock_path = self.path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(PathBuf::from(lock_path))
            .map_err(|error| self.io_error(error))?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                WorkItemError::Busy(self.path.clone())
            } else {
                self.io_error(error)
            }
        })?;
        // Reload under the lock so independent engine instances cannot lose updates.
        let mut work_items = self.load()?;
        if work_items.iter().any(|existing| existing.id == item.id) {
            return Err(WorkItemError::DuplicateId(item.id));
        }
        work_items.push(item);
        let bytes = serde_json::to_vec_pretty(&StoredState {
            version: 1,
            work_items,
        })
        .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| self.io_error(error))?;
        temporary
            .write_all(&bytes)
            .map_err(|error| self.io_error(error))?;
        temporary
            .write_all(b"\n")
            .map_err(|error| self.io_error(error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| self.io_error(error))?;
        temporary
            .persist(&self.path)
            .map_err(|error| self.io_error(error.error))?;
        // Dropping the file releases the advisory lock, including on any error above.
        drop(lock);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, Pane, Session, Snapshot, Window};

    fn item(id: &str, kind: WorkItemKind) -> WorkItem {
        WorkItem {
            id: id.into(),
            title: "Fix retries — λ".into(),
            repository: "/work/repo".into(),
            workspace: "/work/repo/ABC-123".into(),
            branch: Some("fix/retries".into()),
            kind,
            pane_id: "%14".into(),
        }
    }

    #[test]
    fn registration_survives_new_engine_and_preserves_all_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/work-items.json");
        let first = Engine::new(&path);
        assert!(first.work_items().unwrap().is_empty());
        assert!(!path.parent().unwrap().exists());
        let implementation = item("ABC-123", WorkItemKind::Implementation);
        first.register_work_item(implementation.clone()).unwrap();
        let mut review = item("PR #1842", WorkItemKind::ExternalReview);
        review.branch = None;
        Engine::new(&path)
            .register_work_item(review.clone())
            .unwrap();
        assert_eq!(
            Engine::new(&path).work_items().unwrap(),
            [implementation, review]
        );
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["version"], 1);
    }

    #[test]
    fn duplicate_registration_leaves_original_file_intact_and_releases_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let engine = Engine::new(&path);
        engine
            .register_work_item(item("ABC-123", WorkItemKind::Implementation))
            .unwrap();
        let original = fs::read(&path).unwrap();
        assert!(matches!(
            engine.register_work_item(item("ABC-123", WorkItemKind::ExternalReview)),
            Err(WorkItemError::DuplicateId(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), original);
        engine
            .register_work_item(item("PR #1842", WorkItemKind::ExternalReview))
            .unwrap();
    }

    #[test]
    fn invalid_registration_creates_no_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let engine = Engine::new(&path);
        for value in ["", " ABC-123", "ABC\n123"] {
            let mut invalid = item(value, WorkItemKind::Implementation);
            assert!(engine.register_work_item(invalid.clone()).is_err());
            invalid.id = "ABC-123".into();
            invalid.pane_id = value.into();
            assert!(engine.register_work_item(invalid).is_err());
        }
        let mut invalid = item("ABC-123", WorkItemKind::Implementation);
        invalid.workspace = "relative/path".into();
        assert!(engine.register_work_item(invalid).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn malformed_or_future_state_is_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let engine = Engine::new(&path);
        for contents in [
            "",
            "{broken",
            r#"{"version":2,"work_items":[]}"#,
            r#"{"version":1,"work_items":[],"unknown":true}"#,
        ] {
            fs::write(&path, contents).unwrap();
            assert!(engine.work_items().is_err());
            assert!(
                engine
                    .register_work_item(item("ABC-123", WorkItemKind::Implementation))
                    .is_err()
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
    }

    #[test]
    fn unreadable_state_is_not_treated_as_empty() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            Engine::new(directory.path()).work_items(),
            Err(WorkItemError::Io { .. })
        ));
    }

    #[test]
    fn concurrent_writer_is_reported_then_registration_can_retry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.path().join("items.json.lock"))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();
        let engine = Engine::new(&path);
        assert!(matches!(
            engine.register_work_item(item("ABC-123", WorkItemKind::Implementation)),
            Err(WorkItemError::Busy(_))
        ));
        assert!(!path.exists());
        drop(lock);
        engine
            .register_work_item(item("ABC-123", WorkItemKind::Implementation))
            .unwrap();
    }

    #[test]
    fn disappearance_and_discovery_failure_do_not_delete_registration() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        let registered = item("ABC-123", WorkItemKind::Implementation);
        engine.register_work_item(registered.clone()).unwrap();
        let present = Snapshot {
            sessions: vec![Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![Window {
                    id: "@1".into(),
                    index: 1,
                    name: "auth".into(),
                    panes: vec![Pane {
                        id: "%14".into(),
                        index: 0,
                        title: "Agent".into(),
                        current_command: None,
                        working_directory: None,
                    }],
                }],
            }],
        };
        for (snapshot, expected) in [
            (Some(&present), PaneAvailability::Present),
            (Some(&Snapshot::default()), PaneAvailability::Missing),
            (None, PaneAvailability::Unavailable),
            (Some(&present), PaneAvailability::Present),
        ] {
            let states = engine.work_item_states(snapshot).unwrap();
            assert_eq!(states[0].item, registered);
            assert_eq!(states[0].status, AgentStatus::Unknown);
            assert_eq!(states[0].pane, expected);
        }
        assert_eq!(engine.work_items().unwrap(), [registered]);
    }

    #[test]
    fn state_path_precedence_does_not_require_changing_process_environment() {
        assert_eq!(
            state_file_from(
                Some("custom.json".into()),
                Some("/xdg".into()),
                Some("/home/dev".into())
            )
            .unwrap(),
            PathBuf::from("custom.json")
        );
        assert_eq!(
            state_file_from(None, Some("/xdg".into()), None).unwrap(),
            PathBuf::from("/xdg/agent-workbench/work-items.json")
        );
        assert_eq!(
            state_file_from(None, Some("relative".into()), Some("/home/dev".into())).unwrap(),
            PathBuf::from("/home/dev/.local/state/agent-workbench/work-items.json")
        );
        assert!(state_file_from(None, None, None).is_err());
    }
}
