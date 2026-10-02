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

/// Maximum Unicode scalar values in a newly registered short description.
pub const MAX_SHORT_DESCRIPTION_CHARS: usize = 120;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkItem {
    pub id: String,
    /// User-facing short description; keep the legacy storage key compatible.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStatus {
    Running,
    WaitingForInput,
    Idle,
    /// A visible agent turn finished; not proof that the work item is done.
    Complete,
    Unknown,
}

impl AgentStatus {
    /// Only affirmative input/completion evidence creates an attention item.
    pub fn needs_attention(self) -> bool {
        matches!(self, Self::WaitingForInput | Self::Complete)
    }
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Running => "RUNNING",
            Self::WaitingForInput => "WAITING_FOR_INPUT",
            Self::Idle => "IDLE",
            Self::Complete => "TURN FINISHED",
            Self::Unknown => "UNKNOWN",
        })
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
    /// Local observation only; never persisted and never contains terminal text.
    pub status_detail: String,
    /// Bounded visible approval context, only for a current detected input request.
    /// Ephemeral, not a full transcript; never persisted.
    pub attention_prompt: Option<String>,
    /// Opaque hash of visible completion evidence, never terminal text or a
    /// native agent turn ID. Used only for session-local acknowledgement.
    pub completion_fingerprint: Option<u64>,
}

#[derive(Debug)]
pub enum WorkItemError {
    Io { path: PathBuf, source: io::Error },
    Invalid(String),
    DuplicateId(String),
    NotFound(String),
    Changed(String),
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
            Self::NotFound(id) => write!(f, "Work item {id:?} is no longer registered."),
            Self::Changed(id) => write!(
                f,
                "Work item {id:?} changed since it was opened; cancel and reopen before retrying."
            ),
            Self::Busy(path) => write!(
                f,
                "Work-item state {} is being updated by another process; retry the operation.",
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

/// Validate a user-supplied short description before registration.
/// Existing saved descriptions remain readable without applying the new limit.
pub fn validate_short_description(value: &str) -> Result<(), WorkItemError> {
    if value.trim().is_empty() || value != value.trim() || value.chars().any(char::is_control) {
        return Err(WorkItemError::Invalid(
            "short description must be nonempty, with no control characters or surrounding whitespace".into(),
        ));
    }
    if value.chars().count() > MAX_SHORT_DESCRIPTION_CHARS {
        return Err(WorkItemError::Invalid(format!(
            "short description must be at most {MAX_SHORT_DESCRIPTION_CHARS} characters"
        )));
    }
    Ok(())
}

fn validate(item: &WorkItem) -> Result<(), WorkItemError> {
    for (name, value) in [
        ("ID", item.id.as_str()),
        ("short description", item.title.as_str()),
    ] {
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

/// Explicitly unlock on every return path. Merely closing our descriptor may
/// leave the lock briefly held by a concurrently forked command before exec.
pub(crate) struct StateLock(pub(crate) fs::File);

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

impl WorkItemStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn review_path(&self) -> PathBuf {
        let mut path = self.path.as_os_str().to_owned();
        path.push(".reviews.json");
        path.into()
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
        validate_short_description(&item.title)?;
        self.mutate(|work_items| {
            if work_items.iter().any(|existing| existing.id == item.id) {
                return Err(WorkItemError::DuplicateId(item.id));
            }
            work_items.push(item);
            Ok(())
        })
    }

    pub(crate) fn update_description(
        &self,
        expected: &WorkItem,
        description: &str,
    ) -> Result<WorkItem, WorkItemError> {
        validate_short_description(description)?;
        self.mutate(|work_items| {
            let index = unchanged_item(work_items, expected)?;
            work_items[index].title = description.into();
            Ok(work_items[index].clone())
        })
    }

    pub(crate) fn unregister(&self, expected: &WorkItem) -> Result<(), WorkItemError> {
        self.mutate(|work_items| {
            let index = unchanged_item(work_items, expected)?;
            work_items.remove(index);
            Ok(())
        })
    }

    /// Read/modify/write under the same stable lock for every registry mutation.
    fn mutate<T>(
        &self,
        edit: impl FnOnce(&mut Vec<WorkItem>) -> Result<T, WorkItemError>,
    ) -> Result<T, WorkItemError> {
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
        let _lock = StateLock(lock);
        // Reload under the lock so independent engine instances cannot lose updates.
        let mut work_items = self.load()?;
        let result = edit(&mut work_items)?;
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
        // StateLock releases the advisory lock, including on any error above.
        Ok(result)
    }
}

fn unchanged_item(items: &[WorkItem], expected: &WorkItem) -> Result<usize, WorkItemError> {
    let index = items
        .iter()
        .position(|item| item.id == expected.id)
        .ok_or_else(|| WorkItemError::NotFound(expected.id.clone()))?;
    if items[index] != *expected {
        return Err(WorkItemError::Changed(expected.id.clone()));
    }
    Ok(index)
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
    fn editing_and_unregistering_preserve_workspace_and_review_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let engine = Engine::new(&path);
        let mut target = item("A", WorkItemKind::Implementation);
        target.repository = directory.path().into();
        target.workspace = directory.path().join("workspace");
        fs::create_dir(&target.workspace).unwrap();
        let source = target.workspace.join("keep.txt");
        fs::write(&source, "uncommitted content").unwrap();
        let other = item("B", WorkItemKind::ExternalReview);
        engine.register_work_item(target.clone()).unwrap();
        engine.register_work_item(other.clone()).unwrap();
        let review_path = engine.store.review_path();
        fs::write(&review_path, "saved review history").unwrap();
        let updated = engine
            .update_work_item_description(&target, "Updated λ🙂 description")
            .unwrap();
        let mut expected = target.clone();
        expected.title = "Updated λ🙂 description".into();
        assert_eq!(updated, expected);
        assert_eq!(
            Engine::new(&path).work_items().unwrap(),
            [expected.clone(), other.clone()]
        );
        engine.unregister_work_item(&expected).unwrap();
        assert_eq!(
            Engine::new(&path).work_items().unwrap(),
            std::slice::from_ref(&other)
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), "uncommitted content");
        assert_eq!(
            fs::read_to_string(&review_path).unwrap(),
            "saved review history"
        );
        engine.unregister_work_item(&other).unwrap();
        assert!(Engine::new(&path).work_items().unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(&review_path).unwrap(),
            "saved review history"
        );
    }

    #[test]
    fn maintenance_rejects_invalid_missing_and_stale_targets_without_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let first = Engine::new(&path);
        let second = Engine::new(&path);
        let target = item("A", WorkItemKind::Implementation);
        first.register_work_item(target.clone()).unwrap();
        let original = fs::read(&path).unwrap();
        for description in [
            String::new(),
            "bad\ntext".into(),
            "λ".repeat(MAX_SHORT_DESCRIPTION_CHARS + 1),
        ] {
            assert!(
                first
                    .update_work_item_description(&target, &description)
                    .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), original);
        }
        let missing = item("missing", WorkItemKind::Implementation);
        assert!(matches!(
            first.update_work_item_description(&missing, "Updated"),
            Err(WorkItemError::NotFound(_))
        ));
        assert!(matches!(
            first.unregister_work_item(&missing),
            Err(WorkItemError::NotFound(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), original);
        let updated = second
            .update_work_item_description(&target, "Changed elsewhere")
            .unwrap();
        let stored = fs::read(&path).unwrap();
        assert!(matches!(
            first.update_work_item_description(&target, "Stale edit"),
            Err(WorkItemError::Changed(_))
        ));
        assert!(matches!(
            first.unregister_work_item(&target),
            Err(WorkItemError::Changed(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), stored);
        second.unregister_work_item(&updated).unwrap();
        assert!(matches!(
            first.unregister_work_item(&updated),
            Err(WorkItemError::NotFound(_))
        ));
    }

    #[test]
    fn maintenance_reloads_other_items_and_rejects_replaced_bindings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let first = Engine::new(&path);
        let second = Engine::new(&path);
        let a = item("A", WorkItemKind::Implementation);
        let b = item("B", WorkItemKind::Implementation);
        first.register_work_item(a.clone()).unwrap();
        second.register_work_item(b.clone()).unwrap();
        let updated = first.update_work_item_description(&a, "Updated").unwrap();
        assert_eq!(first.work_items().unwrap(), [updated.clone(), b.clone()]);
        second.unregister_work_item(&b).unwrap();
        assert_eq!(first.work_items().unwrap(), [updated]);
        let mut replacement = b.clone();
        replacement.pane_id = "%99".into();
        second.register_work_item(replacement.clone()).unwrap();
        let stored = fs::read(&path).unwrap();
        assert!(matches!(
            first.unregister_work_item(&b),
            Err(WorkItemError::Changed(_))
        ));
        assert!(matches!(
            first.update_work_item_description(&b, "Must not change"),
            Err(WorkItemError::Changed(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), stored);
    }

    #[test]
    fn maintenance_preserves_busy_corrupt_and_unwritable_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let engine = Engine::new(&path);
        let target = item("A", WorkItemKind::Implementation);
        engine.register_work_item(target.clone()).unwrap();
        let stored = fs::read(&path).unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.path().join("items.json.lock"))
            .unwrap();
        fs2::FileExt::lock_exclusive(&lock).unwrap();
        assert!(matches!(
            engine.update_work_item_description(&target, "Updated"),
            Err(WorkItemError::Busy(_))
        ));
        assert!(matches!(
            engine.unregister_work_item(&target),
            Err(WorkItemError::Busy(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), stored);
        fs2::FileExt::unlock(&lock).unwrap();
        for invalid in ["{broken", r#"{"version":2,"work_items":[]}"#] {
            fs::write(&path, invalid).unwrap();
            assert!(
                engine
                    .update_work_item_description(&target, "Updated")
                    .is_err()
            );
            assert!(engine.unregister_work_item(&target).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        }
        let blocked = directory.path().join("blocked-state");
        fs::create_dir(&blocked).unwrap();
        let unwritable = Engine::new(&blocked);
        assert!(
            unwritable
                .update_work_item_description(&target, "Updated")
                .is_err()
        );
        assert!(unwritable.unregister_work_item(&target).is_err());
        assert!(blocked.is_dir());
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
    fn short_description_limit_counts_characters_and_rejects_without_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/items.json");
        let engine = Engine::new(&path);
        let mut too_long = item("long", WorkItemKind::Implementation);
        too_long.title = "🙂".repeat(MAX_SHORT_DESCRIPTION_CHARS + 1);
        let error = engine.register_work_item(too_long.clone()).unwrap_err();
        assert!(error.to_string().contains("at most 120 characters"));
        assert!(!path.parent().unwrap().exists());
        for (id, character) in [("ascii", "a"), ("unicode", "🙂")] {
            let mut boundary = item(id, WorkItemKind::Implementation);
            boundary.title = character.repeat(MAX_SHORT_DESCRIPTION_CHARS);
            engine.register_work_item(boundary.clone()).unwrap();
            assert!(engine.work_items().unwrap().contains(&boundary));
        }
        let stored = fs::read(&path).unwrap();
        assert!(engine.register_work_item(too_long).is_err());
        assert_eq!(fs::read(&path).unwrap(), stored);
        for invalid in ["", " ", " surrounding ", "multiple\nlines"] {
            assert!(validate_short_description(invalid).is_err());
        }
    }

    #[test]
    fn legacy_long_descriptions_remain_readable_and_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let mut legacy = item("legacy", WorkItemKind::Implementation);
        legacy.title = "λ".repeat(MAX_SHORT_DESCRIPTION_CHARS + 20);
        fs::write(
            &path,
            serde_json::to_vec(&StoredState {
                version: 1,
                work_items: vec![legacy.clone()],
            })
            .unwrap(),
        )
        .unwrap();
        let stored = fs::read(&path).unwrap();
        let engine = Engine::new(&path);
        assert_eq!(engine.work_items().unwrap(), [legacy.clone()]);
        assert_eq!(fs::read(&path).unwrap(), stored);
        engine
            .register_work_item(item("new", WorkItemKind::Implementation))
            .unwrap();
        assert_eq!(engine.work_items().unwrap()[0], legacy);
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(json["work_items"][0].get("title").is_some());
        assert_eq!(json["version"], 1);
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
        let lock = StateLock(lock);
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
