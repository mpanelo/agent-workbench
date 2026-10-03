//! Explicit workmux cleanup. Preview is read-only; execution revalidates every
//! target and keeps the registration on failures or incomplete owner cleanup.
use crate::{Engine, Snapshot, Window, WorkItem};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    error::Error,
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::{sleep, timeout},
};

#[derive(Debug)]
pub struct CleanupError(String);
impl fmt::Display for CleanupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for CleanupError {}
fn fail(message: impl Into<String>) -> CleanupError {
    CleanupError(message.into())
}
fn error(error: impl fmt::Display) -> CleanupError {
    fail(error.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupDetails {
    pub work_id: String,
    pub handle: String,
    pub repository: PathBuf,
    pub worktree: PathBuf,
    pub branch: String,
    pub window: Window,
    pub sessions: Vec<String>,
    /// Ignored files/directories are not Git dirt, but removal deletes them too.
    pub ignored_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupPreview {
    pub details: CleanupDetails,
    expected: WorkItem,
    identity: Identity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
    admin: PathBuf,
    common: PathBuf,
    head: Vec<u8>,
    metadata: Vec<u8>,
    token: String,
}

#[derive(Debug)]
pub(crate) struct WorkmuxClient {
    pub(crate) executable: OsString,
    pub(crate) limit: Duration,
    pub(crate) pane: Option<String>,
}
impl Default for WorkmuxClient {
    fn default() -> Self {
        Self {
            executable: "workmux".into(),
            limit: Duration::from_secs(30),
            pane: std::env::var("TMUX_PANE").ok(),
        }
    }
}

#[derive(Deserialize)]
struct ManagedWorktree {
    handle: String,
    branch: String,
    path: PathBuf,
    is_main: bool,
    mode: String,
    is_open: bool,
}

impl Engine {
    /// Resolve only a live, unambiguous, ownership-token-backed workmux window.
    /// Does not delete resources, alter tmux, or mutate registration state.
    pub async fn prepare_cleanup(
        &self,
        expected: &WorkItem,
    ) -> Result<CleanupPreview, CleanupError> {
        self.cleanup_preview(expected).await
    }

    /// Explicit confirmation belongs to the caller. A changed preview is never
    /// silently retargeted. Workmux owns deletion; no force flags or shell code.
    pub async fn cleanup_work_item(&self, preview: &CleanupPreview) -> Result<(), CleanupError> {
        let _lock = self.store.lock().map_err(error)?;
        let fresh = self.cleanup_preview(&preview.expected).await?;
        if &fresh != preview {
            return Err(fail(
                "Cleanup targets changed. Nothing was removed; cancel and reopen cleanup to inspect a fresh preview.",
            ));
        }
        let pending = self
            .store
            .stage_unregister(&preview.expected, &_lock)
            .map_err(error)?;
        let details = &preview.details;
        self.workmux.run(&details.repository, &["remove", "--keep-branch", "--", &details.handle], self.tmux.socket.as_deref())
            .await.map_err(|e| fail(format!("Workmux cleanup failed or timed out: {e}. Registration and review history were kept. Cleanup may be partial; inspect the window and worktree before retrying.")))?;
        // Owner commands can schedule window closure. Verify actual results,
        // never treat exit zero alone as proof of deletion or unregister early.
        let mut removed = false;
        for _ in 0..30 {
            if self.cleanup_resources_removed(preview).await.map_err(|e| fail(format!("Could not verify cleanup: {e}. Registration was kept; inspect resources before retrying.")))? {
                removed = true;
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        if !removed {
            return Err(fail(
                "Workmux returned, but cleanup is incomplete. Registration was kept; inspect the window and worktree. Nothing will be retried automatically.",
            ));
        }
        pending.commit().map_err(|e| fail(format!("Window and worktree were removed, but unregistering failed: {e}. Branch and review history remain; use Unregister after inspecting WORK.")))
    }

    async fn cleanup_resources_removed(
        &self,
        preview: &CleanupPreview,
    ) -> Result<bool, CleanupError> {
        let details = &preview.details;
        let exists = match fs::symlink_metadata(&details.worktree) {
            Ok(_) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(error(e)),
        };
        let output = self
            .tmux
            .query(&["list-windows", "-a", "-F", "#{window_id}"])
            .await;
        let window_exists = match output {
            Ok(text) => text.lines().any(|id| id == details.window.id),
            Err(crate::DiscoveryError::CommandFailed { message, .. })
                if message.starts_with("no server running") || message == "no sessions" =>
            {
                false
            }
            Err(e) => {
                return Err(fail(format!(
                    "Could not verify closed window: {e}. Registration was kept; inspect cleanup manually."
                )));
            }
        };
        let worktrees = self
            .git_read(
                &details.repository,
                &["worktree", "list", "--porcelain", "-z"],
            )
            .await?;
        let mut still_registered = false;
        for record in worktree_records(&worktrees)? {
            still_registered |= resolved_path(&record.path)? == details.worktree;
        }
        self.git_read(
            &details.repository,
            &[
                "show-ref",
                "--verify",
                &format!("refs/heads/{}", details.branch),
            ],
        )
        .await
        .map_err(|e| {
            fail(format!(
                "Could not verify retained branch: {e}. Registration was kept."
            ))
        })?;
        Ok(!exists && !window_exists && !still_registered)
    }

    async fn cleanup_preview(&self, expected: &WorkItem) -> Result<CleanupPreview, CleanupError> {
        let items = self.work_items().map_err(error)?;
        if items.iter().find(|item| item.id == expected.id) != Some(expected) {
            return Err(fail(
                "Work item changed or was removed; cancel and reopen cleanup.",
            ));
        }
        let (main, root, branch) = self
            .git
            .registration_metadata(&expected.workspace)
            .await
            .map_err(error)?;
        let root = root.canonicalize().map_err(error)?;
        let main = main.canonicalize().map_err(error)?;
        if root.parent().is_none()
            || std::env::var_os("HOME")
                .and_then(|path| PathBuf::from(path).canonicalize().ok())
                .as_ref()
                == Some(&root)
            || std::env::temp_dir().canonicalize().ok().as_ref() == Some(&root)
        {
            return Err(fail(
                "Cleanup cannot remove a home, filesystem root, or temporary-directory root.",
            ));
        }
        if root == main {
            return Err(fail(
                "Cleanup cannot remove the main checkout. Use Unregister to remove only the Workbench entry.",
            ));
        }
        if expected.workspace.canonicalize().map_err(error)? != root
            || expected.repository.canonicalize().map_err(error)? != main
        {
            return Err(fail(
                "Registered workspace/repository does not identify this exact linked worktree.",
            ));
        }
        let branch = branch.ok_or_else(|| fail("Detached worktrees are not supported for cleanup; no retained branch can be verified."))?;
        if expected.branch.as_deref() != Some(&branch) {
            let dirty = self
                .git_read(
                    &root,
                    &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
                )
                .await?;
            let dirt = if dirty.is_empty() {
                ""
            } else {
                " The worktree also has staged, unstaged, or untracked changes; resolve those before cleanup."
            };
            return Err(fail(format!(
                "Registered branch {:?} differs from current Git branch {branch:?}. Nothing was removed. Use Unregister, then register the pane again from SESSIONS to refresh the saved branch.{dirt}",
                expected.branch.as_deref().unwrap_or("(not recorded)")
            )));
        }
        let directory = fs::symlink_metadata(&expected.workspace).map_err(error)?;
        if !directory.is_dir() || directory.file_type().is_symlink() {
            return Err(fail(
                "Cleanup requires a real worktree directory, not a symlink.",
            ));
        }
        let (device, inode) = directory_identity(&directory)?;
        for path in [
            std::env::current_dir().map_err(error)?,
            std::env::current_exe().map_err(error)?,
            self.store.state_path().to_path_buf(),
            self.store.review_path(),
        ] {
            if resolved_path(&path)?.starts_with(&root) {
                return Err(fail(
                    "Cleanup would remove Workbench's own workspace, executable, or saved state. Run Workbench from outside that worktree.",
                ));
            }
        }
        let records = worktree_records(
            &self
                .git_read(&main, &["worktree", "list", "--porcelain", "-z"])
                .await
                .map_err(error)?,
        )?;
        let target = records
            .iter()
            .find(|r| r.path.canonicalize().ok().as_ref() == Some(&root))
            .ok_or_else(|| fail("Linked worktree is not registered with Git."))?;
        if target.locked {
            return Err(fail(
                "Worktree is locked. Unlock it explicitly outside Workbench before cleanup.",
            ));
        }
        let output = self
            .workmux
            .run(&main, &["list", "--json"], self.tmux.socket.as_deref())
            .await?;
        let managed = resolve_managed(&output, &root, &branch)?;
        if records
            .iter()
            .filter(|r| r.path.file_name().and_then(|s| s.to_str()) == Some(&managed.handle))
            .count()
            != 1
        {
            return Err(fail(
                "Worktree handle is ambiguous in this repository; cleanup cannot safely resolve it.",
            ));
        }
        let metadata = self
            .git
            .run(
                &main,
                &args(&[
                    "config",
                    "--local",
                    "--null",
                    "--get-regexp",
                    "^workmux\\.worktree\\.",
                ]),
                true,
            )
            .await
            .map_err(error)?;
        let config = config_values(&metadata)?;
        let prefix = format!("workmux.worktree.{}.", managed.handle);
        if config.get(&(prefix.clone() + "mode")).map(String::as_str) != Some("window") {
            return Err(fail(
                "Cleanup requires explicit workmux window-mode ownership metadata. Legacy, headless, and session-mode workspaces are not supported.",
            ));
        }
        if config
            .get(&(prefix.clone() + "attachment"))
            .is_some_and(|s| s != "multiplexer")
        {
            return Err(fail(
                "This workspace is not attached to a managed workmux window.",
            ));
        }
        let token = config.get(&(prefix + "window-token")).filter(|s| !s.is_empty() && !s.chars().any(char::is_control)).cloned()
            .ok_or_else(|| fail("Workmux ownership token is missing. Open the workspace with a current workmux version before cleanup; Workbench will not guess by window name."))?;
        let snapshot = self.discover().await.map_err(error)?;
        let ownership = ownership_records(
            &self
                .tmux
                .query(&[
                    "list-windows",
                    "-a",
                    "-F",
                    "#{window_id}\x1f#{@workmux_token}\x1e",
                ])
                .await
                .map_err(error)?,
        )?;
        let (window, sessions) = resolve_window(&snapshot, &ownership, &token, &expected.pane_id)?;
        if self
            .workmux
            .pane
            .as_ref()
            .is_some_and(|id| window.panes.iter().any(|p| &p.id == id))
            || window.panes.iter().any(|p| {
                p.current_command
                    .as_deref()
                    .and_then(|s| Path::new(s).file_name())
                    .is_some_and(|s| s == "awb" || s == "workbench")
            })
        {
            return Err(fail(
                "Cleanup cannot close the window containing Workbench. Move Workbench to another window first.",
            ));
        }
        for pane in &window.panes {
            let cwd = pane.working_directory.as_ref().ok_or_else(|| fail("A pane's working directory is unavailable; cleanup cannot verify a shared window."))?;
            if !cwd.canonicalize().map_err(error)?.starts_with(&root) {
                return Err(fail(
                    "The managed window contains a pane outside this worktree. Move that pane out before cleanup.",
                ));
            }
        }
        for item in items.iter().filter(|item| item.id != expected.id) {
            if resolved_path(&item.workspace)?.starts_with(&root)
                || window.panes.iter().any(|p| p.id == item.pane_id)
            {
                return Err(fail(format!(
                    "Workspace/window is also registered to {}. Resolve that shared registration before cleanup.",
                    item.id
                )));
            }
        }
        let dirty = self
            .git_read(
                &root,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            )
            .await?;
        if !dirty.is_empty() {
            return Err(fail(
                "Worktree has staged, unstaged, or untracked changes. Commit, stash, or remove them manually before cleanup. Force deletion is not supported.",
            ));
        }
        let ignored = self
            .git_read(
                &root,
                &[
                    "ls-files",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "--directory",
                    "-z",
                ],
            )
            .await?;
        let ignored_paths = paths(&ignored)?;
        let admin = PathBuf::from(single_text(
            &self
                .git_read(&root, &["rev-parse", "--absolute-git-dir"])
                .await?,
        )?);
        let common = PathBuf::from(single_text(
            &self
                .git_read(
                    &root,
                    &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                )
                .await?,
        )?);
        let head = self
            .git_read(&root, &["rev-parse", "--verify", "HEAD"])
            .await?;
        Ok(CleanupPreview {
            details: CleanupDetails {
                work_id: expected.id.clone(),
                handle: managed.handle,
                repository: main,
                worktree: root,
                branch,
                window,
                sessions,
                ignored_paths,
            },
            expected: expected.clone(),
            identity: Identity {
                device,
                inode,
                admin,
                common,
                head,
                metadata,
                token,
            },
        })
    }

    async fn git_read(&self, directory: &Path, input: &[&str]) -> Result<Vec<u8>, CleanupError> {
        self.git
            .run(directory, &args(input), false)
            .await
            .map_err(error)
    }
}

fn args(input: &[&str]) -> Vec<OsString> {
    input.iter().map(OsString::from).collect()
}
fn single_text(bytes: &[u8]) -> Result<String, CleanupError> {
    let text = std::str::from_utf8(bytes)
        .map_err(error)?
        .strip_suffix('\n')
        .unwrap_or(std::str::from_utf8(bytes).map_err(error)?);
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(fail("Invalid target metadata; cleanup was blocked."));
    }
    Ok(text.into())
}

fn resolve_managed(
    bytes: &[u8],
    root: &Path,
    branch: &str,
) -> Result<ManagedWorktree, CleanupError> {
    let mut entries: Vec<ManagedWorktree> = serde_json::from_slice(bytes).map_err(|e| fail(format!("Could not read workmux JSON listing: {e}. A workmux version supporting list --json is required.")))?;
    let matching: Vec<_> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.path.canonicalize().ok().as_deref() == Some(root))
        .map(|(i, _)| i)
        .collect();
    if matching.len() != 1 {
        return Err(fail(
            "Workmux did not identify exactly one matching worktree.",
        ));
    }
    let entry = entries.remove(matching[0]);
    if entry.is_main
        || entry.mode != "window"
        || !entry.is_open
        || entry.branch != branch
        || root.file_name().and_then(|s| s.to_str()) != Some(&entry.handle)
        || entry.handle.starts_with('-')
        || entry.handle.chars().any(char::is_control)
    {
        return Err(fail(
            "Workmux target is main, detached, closed, ambiguous, or not in window mode; cleanup was blocked.",
        ));
    }
    Ok(entry)
}

fn config_values(bytes: &[u8]) -> Result<BTreeMap<String, String>, CleanupError> {
    let mut values = BTreeMap::new();
    for record in nul_records(bytes)? {
        let (key, value) = record
            .split_once('\n')
            .ok_or_else(|| fail("Malformed workmux Git metadata."))?;
        if value.chars().any(char::is_control) || values.insert(key.into(), value.into()).is_some()
        {
            return Err(fail("Ambiguous or malformed workmux Git metadata."));
        }
    }
    Ok(values)
}

fn ownership_records(text: &str) -> Result<BTreeMap<String, String>, CleanupError> {
    if !text.is_empty() && !text.trim_end_matches('\n').ends_with('\x1e') {
        return Err(fail("Truncated tmux ownership metadata."));
    }
    let mut values = BTreeMap::new();
    for record in text.split('\x1e') {
        let record = record.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        let (id, token) = record
            .split_once('\x1f')
            .ok_or_else(|| fail("Malformed tmux ownership metadata."))?;
        if !id
            .strip_prefix('@')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            || token.contains(['\x1f', '\n'])
        {
            return Err(fail("Malformed tmux ownership metadata."));
        }
        if let Some(old) = values.insert(id.into(), token.into())
            && old != token
        {
            return Err(fail("Conflicting tmux ownership metadata."));
        }
    }
    Ok(values)
}

fn resolve_window(
    snapshot: &Snapshot,
    ownership: &BTreeMap<String, String>,
    token: &str,
    pane: &str,
) -> Result<(Window, Vec<String>), CleanupError> {
    let ids: Vec<_> = ownership
        .iter()
        .filter(|(_, value)| value.as_str() == token)
        .map(|(id, _)| id)
        .collect();
    if ids.len() != 1 {
        return Err(fail(
            "Workmux owns zero or multiple windows for this worktree. Close duplicate windows manually before cleanup.",
        ));
    }
    let mut result: Option<Window> = None;
    let mut sessions = Vec::new();
    for session in &snapshot.sessions {
        for window in session.windows.iter().filter(|w| &w.id == ids[0]) {
            if result.as_ref().is_some_and(|previous| previous != window) {
                return Err(fail(
                    "Linked window changed during discovery; reopen cleanup.",
                ));
            }
            result = Some(window.clone());
            sessions.push(session.name.clone());
        }
    }
    let window = result
        .filter(|w| w.panes.iter().any(|p| p.id == pane))
        .ok_or_else(|| fail("Registered pane is not in the verified workmux window."))?;
    sessions.sort();
    sessions.dedup();
    Ok((window, sessions))
}

struct WorktreeRecord {
    path: PathBuf,
    locked: bool,
}
fn worktree_records(bytes: &[u8]) -> Result<Vec<WorktreeRecord>, CleanupError> {
    let mut records: Vec<WorktreeRecord> = Vec::new();
    for field in nul_records(bytes)? {
        if let Some(path) = field.strip_prefix("worktree ") {
            if !Path::new(path).is_absolute() {
                return Err(fail("Git returned a relative worktree path."));
            }
            records.push(WorktreeRecord {
                path: path.into(),
                locked: false,
            });
        } else if field == "locked" || field.starts_with("locked ") {
            records
                .last_mut()
                .ok_or_else(|| fail("Malformed Git worktree list."))?
                .locked = true;
        }
    }
    if records.is_empty() {
        return Err(fail("Git returned no worktrees."));
    }
    Ok(records)
}
fn nul_records(bytes: &[u8]) -> Result<Vec<&str>, CleanupError> {
    if !bytes.is_empty() && bytes.last() != Some(&0) {
        return Err(fail("Truncated cleanup metadata; no targets were changed."));
    }
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| std::str::from_utf8(s).map_err(error))
        .collect()
}
fn paths(bytes: &[u8]) -> Result<Vec<PathBuf>, CleanupError> {
    let mut paths: Vec<_> = nul_records(bytes)?.into_iter().map(PathBuf::from).collect();
    if paths.iter().any(|p| {
        p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
    }) {
        return Err(fail("Git returned an unsafe ignored path."));
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}
fn resolved_path(path: &Path) -> Result<PathBuf, CleanupError> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir().map_err(error)?.join(path)
            };
            let parent = absolute
                .parent()
                .ok_or_else(|| fail("Could not resolve protected state path."))?;
            Ok(resolved_path(parent)?.join(
                absolute
                    .file_name()
                    .ok_or_else(|| fail("Invalid protected state path."))?,
            ))
        }
        Err(e) => Err(error(e)),
    }
}
#[cfg(unix)]
fn directory_identity(metadata: &fs::Metadata) -> Result<(u64, u64), CleanupError> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}
#[cfg(not(unix))]
fn directory_identity(_: &fs::Metadata) -> Result<(u64, u64), CleanupError> {
    Err(fail(
        "Verified cleanup currently requires Unix filesystem identities.",
    ))
}

impl WorkmuxClient {
    async fn run(
        &self,
        directory: &Path,
        input: &[&str],
        socket: Option<&Path>,
    ) -> Result<Vec<u8>, CleanupError> {
        let mut command = Command::new(&self.executable);
        command
            .current_dir(directory)
            .args(input)
            .env("WORKMUX_BACKEND", "tmux")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(socket) = socket {
            command.env("TMUX", format!("{},0,0", socket.display()));
        }
        for name in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_PREFIX",
        ] {
            command.env_remove(name);
        }
        timeout(self.limit, async {
            let mut child = command.spawn().map_err(|e| {
                fail(format!(
                    "Could not run workmux: {e}. Install workmux and make it available on PATH."
                ))
            })?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| fail("Workmux stdout unavailable."))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| fail("Workmux stderr unavailable."))?;
            let (stdout, stderr, status) = tokio::try_join!(
                read_bounded(stdout, 2 * 1024 * 1024),
                read_bounded(stderr, 64 * 1024),
                async { child.wait().await.map_err(error) }
            )?;
            if !status.success() {
                return Err(fail(format!(
                    "workmux exited {:?}: {}",
                    status.code(),
                    String::from_utf8_lossy(&stderr).trim()
                )));
            }
            Ok(stdout)
        })
        .await
        .map_err(|_| fail("Workmux timed out; inspect resources before retrying."))?
    }
}
async fn read_bounded(
    reader: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, CleanupError> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(error)?;
    if bytes.len() > limit {
        return Err(fail("Workmux output exceeded its safety limit."));
    }
    Ok(bytes)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::WorkItemKind;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        repo: PathBuf,
        tree: PathBuf,
        engine: Engine,
        item: WorkItem,
    }

    fn git(path: &Path, args: &[&str]) -> Vec<u8> {
        let result = std::process::Command::new("git")
            .current_dir(path)
            .args([
                "-c",
                "user.name=Workbench test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let repo = root.join("repository with spaces");
            let tree = root.join("task");
            fs::create_dir(&repo).unwrap();
            git(&repo, &["init", "-b", "main"]);
            fs::write(repo.join("source"), "original\n").unwrap();
            fs::write(repo.join(".gitignore"), ".env\ncache/\n").unwrap();
            git(&repo, &["add", "source", ".gitignore"]);
            git(&repo, &["commit", "-m", "test fixture"]);
            git(
                &repo,
                &["worktree", "add", "-b", "feature", tree.to_str().unwrap()],
            );
            for (key, value) in [
                ("mode", "window"),
                ("window-token", "test-token"),
                ("attachment", "multiplexer"),
            ] {
                git(
                    &repo,
                    &[
                        "config",
                        "--local",
                        &format!("workmux.worktree.task.{key}"),
                        value,
                    ],
                );
            }
            let item = WorkItem {
                id: "A".into(),
                title: "Task".into(),
                repository: repo.clone(),
                workspace: tree.clone(),
                branch: Some("feature".into()),
                kind: WorkItemKind::Implementation,
                pane_id: "%701".into(),
            };
            let mut engine = Engine::new(root.join("state.json"));
            engine.workmux.pane = Some("%999".into());
            let workmux = root.join("fake-workmux");
            let tmux = root.join("fake-tmux");
            // All destructive commands below target only this fixture's linked
            // worktree. No real tmux/workmux resources are contacted.
            fs::write(
                &workmux,
                format!(
                    r##"#!/bin/sh
base='{}'
case "$1" in
list) exec /bin/cat "$base/list.json" ;;
remove)
  /usr/bin/printf '%s\n' "$@" > "$base/invocation"
  [ "$2" = '--keep-branch' ] && [ "$3" = '--' ] && [ "$4" = 'task' ] && [ "$#" = 4 ] || exit 90
  [ -e "$base/failure" ] && exit 21
  [ -e "$base/noop" ] && exit 0
  git -C "$base/repository with spaces" worktree remove "$base/task" || exit 22
  [ -e "$base/partial" ] && exit 23
  /bin/rm -- "$base/window-present"
  exit 0 ;;
*) exit 99 ;;
esac
"##,
                    root.display()
                ),
            )
            .unwrap();
            fs::write(
                &tmux,
                format!(
                    r##"#!/bin/sh
base='{}'
[ "$1" = '-N' ] && shift
case "$1" in
list-panes) exec /bin/cat "$base/panes" ;;
list-windows)
  if [ "$4" = '#{{window_id}}' ]; then
    [ -e "$base/window-present" ] && /usr/bin/printf '@7\n'
    exit 0
  fi
  exec /bin/cat "$base/ownership" ;;
*) exit 99 ;;
esac
"##,
                    root.display()
                ),
            )
            .unwrap();
            for executable in [&workmux, &tmux] {
                fs::set_permissions(executable, fs::Permissions::from_mode(0o700)).unwrap();
            }
            engine.workmux.executable = workmux.into_os_string();
            engine.tmux.executable = tmux.into_os_string();
            engine.register_work_item(item.clone()).unwrap();
            fs::write(root.join("list.json"), serde_json::to_vec(&serde_json::json!([
                {"handle":"repository with spaces", "path":repo, "branch":"main", "is_main":true,"mode":"window", "is_open":false},
                {"handle":"task", "path":tree, "branch":"feature", "is_main":false,"mode":"window", "is_open":true}
            ])).unwrap()).unwrap();
            fs::write(root.join("ownership"), "@7\x1ftest-token\x1e\n").unwrap();
            fs::write(root.join("window-present"), "").unwrap();
            let fixture = Self {
                _temp: temp,
                root,
                repo,
                tree,
                engine,
                item,
            };
            fixture.panes(&fixture.tree);
            fixture
        }
        fn panes(&self, shell_path: &Path) {
            fs::write(self.root.join("panes"), format!("$1\x1fmain\x1f@7\x1f0\x1fwm-task\x1f%701\x1f0\x1fAgent\x1fcodex\x1f{}\x1e\n$1\x1fmain\x1f@7\x1f0\x1fwm-task\x1f%702\x1f1\x1fShell\x1ffish\x1f{}\x1e\n", self.tree.display(), shell_path.display())).unwrap();
        }
        async fn preview(&self) -> CleanupPreview {
            self.engine.prepare_cleanup(&self.item).await.unwrap()
        }
        fn untouched(&self) {
            assert!(self.tree.exists());
            assert!(self.root.join("window-present").exists());
            assert!(!self.root.join("invocation").exists());
            assert_eq!(self.engine.work_items().unwrap(), vec![self.item.clone()]);
        }
    }

    #[tokio::test]
    async fn preview_is_read_only_and_includes_every_pane_ignored_paths_and_retained_branch() {
        let fixture = Fixture::new();
        fs::write(fixture.tree.join(".env"), "private fixture data").unwrap();
        let before = fs::read(fixture.engine.store.state_path()).unwrap();
        let config = fs::read(fixture.repo.join(".git/config")).unwrap();
        let preview = fixture.preview().await;
        assert_eq!(preview.details.window.panes.len(), 2);
        assert_eq!(preview.details.sessions, ["main"]);
        assert_eq!(preview.details.branch, "feature");
        assert_eq!(preview.details.ignored_paths, [PathBuf::from(".env")]);
        assert_eq!(fs::read(fixture.engine.store.state_path()).unwrap(), before);
        assert_eq!(fs::read(fixture.repo.join(".git/config")).unwrap(), config);
        fixture.untouched();
    }

    #[tokio::test]
    async fn cleanup_delegates_exactly_one_handle_without_force_and_keeps_branch_and_reviews() {
        let fixture = Fixture::new();
        fs::write(fixture.engine.store.review_path(), "retained history").unwrap();
        let preview = fixture.preview().await;
        fixture.engine.cleanup_work_item(&preview).await.unwrap();
        assert!(!fixture.tree.exists());
        assert!(!fixture.root.join("window-present").exists());
        assert!(fixture.engine.work_items().unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(fixture.root.join("invocation")).unwrap(),
            "remove\n--keep-branch\n--\ntask\n"
        );
        git(
            &fixture.repo,
            &["show-ref", "--verify", "refs/heads/feature"],
        );
        assert_eq!(
            fs::read_to_string(fixture.engine.store.review_path()).unwrap(),
            "retained history"
        );
    }

    #[tokio::test]
    async fn dirty_staged_unstaged_untracked_and_locked_worktrees_block_before_removal() {
        for kind in ["unstaged", "staged", "untracked", "locked"] {
            let fixture = Fixture::new();
            match kind {
                "unstaged" | "staged" => {
                    fs::write(fixture.tree.join("source"), "changed\n").unwrap();
                    if kind == "staged" {
                        git(&fixture.tree, &["add", "source"]);
                    }
                }
                "untracked" => fs::write(fixture.tree.join("new"), "untracked").unwrap(),
                _ => {
                    git(
                        &fixture.repo,
                        &["worktree", "lock", fixture.tree.to_str().unwrap()],
                    );
                }
            }
            assert!(
                fixture.engine.prepare_cleanup(&fixture.item).await.is_err(),
                "{kind}"
            );
            fixture.untouched();
        }
    }

    #[tokio::test]
    async fn main_detached_unmanaged_and_session_targets_are_rejected() {
        let fixture = Fixture::new();
        let mut main = fixture.item.clone();
        main.workspace = fixture.repo.clone();
        main.branch = Some("main".into());
        fixture.engine.unregister_work_item(&fixture.item).unwrap();
        fixture.engine.register_work_item(main.clone()).unwrap();
        assert!(
            fixture
                .engine
                .prepare_cleanup(&main)
                .await
                .unwrap_err()
                .to_string()
                .contains("main checkout")
        );
        fixture.engine.unregister_work_item(&main).unwrap();
        fixture
            .engine
            .register_work_item(fixture.item.clone())
            .unwrap();
        git(
            &fixture.repo,
            &[
                "config",
                "--local",
                "--unset",
                "workmux.worktree.task.window-token",
            ],
        );
        assert!(
            fixture
                .engine
                .prepare_cleanup(&fixture.item)
                .await
                .unwrap_err()
                .to_string()
                .contains("token")
        );
        git(
            &fixture.repo,
            &[
                "config",
                "--local",
                "workmux.worktree.task.window-token",
                "test-token",
            ],
        );
        git(
            &fixture.repo,
            &["config", "--local", "workmux.worktree.task.mode", "session"],
        );
        assert!(fixture.engine.prepare_cleanup(&fixture.item).await.is_err());
        git(&fixture.tree, &["checkout", "--detach"]);
        assert!(
            fixture
                .engine
                .prepare_cleanup(&fixture.item)
                .await
                .unwrap_err()
                .to_string()
                .contains("Detached")
        );
        fixture.untouched();
    }

    #[tokio::test]
    async fn protect_workbench_shared_registrations_and_panes_outside_the_worktree() {
        let mut fixture = Fixture::new();
        fixture.engine.workmux.pane = Some("%702".into());
        assert!(
            fixture
                .engine
                .prepare_cleanup(&fixture.item)
                .await
                .unwrap_err()
                .to_string()
                .contains("Workbench")
        );
        fixture.engine.workmux.pane = Some("%999".into());
        fixture.panes(&fixture.repo);
        assert!(
            fixture
                .engine
                .prepare_cleanup(&fixture.item)
                .await
                .unwrap_err()
                .to_string()
                .contains("outside")
        );
        fixture.panes(&fixture.tree);
        let mut other = fixture.item.clone();
        other.id = "B".into();
        other.pane_id = "%703".into();
        fixture.engine.register_work_item(other.clone()).unwrap();
        assert!(
            fixture
                .engine
                .prepare_cleanup(&fixture.item)
                .await
                .unwrap_err()
                .to_string()
                .contains("also registered")
        );
        fixture.engine.unregister_work_item(&other).unwrap();
        let state_inside = Engine::new(fixture.tree.join("state.json"));
        state_inside
            .register_work_item(fixture.item.clone())
            .unwrap();
        assert!(
            state_inside
                .prepare_cleanup(&fixture.item)
                .await
                .unwrap_err()
                .to_string()
                .contains("saved state")
        );
        fs::remove_file(fixture.tree.join("state.json")).unwrap();
        fs::remove_file(fixture.tree.join("state.json.lock")).unwrap();
        fixture.untouched();
    }

    #[tokio::test]
    async fn confirmation_revalidates_dirty_changed_registration_branch_and_window_before_execution()
     {
        for change in [
            "dirty",
            "registration",
            "window",
            "ownership",
            "branch",
            "ignored",
            "duplicate",
        ] {
            let fixture = Fixture::new();
            let preview = fixture.preview().await;
            match change {
                "dirty" => fs::write(fixture.tree.join("source"), "new edits").unwrap(),
                "registration" => {
                    fixture
                        .engine
                        .update_work_item_description(&fixture.item, "New description")
                        .unwrap();
                }
                "window" => fs::write(
                    fixture.root.join("panes"),
                    fs::read_to_string(fixture.root.join("panes"))
                        .unwrap()
                        .replace("Shell", "New shell"),
                )
                .unwrap(),
                "ownership" => fs::write(
                    fixture.root.join("ownership"),
                    "@7\x1fdifferent-token\x1e\n",
                )
                .unwrap(),
                "branch" => {
                    git(&fixture.tree, &["checkout", "-b", "other"]);
                }
                "ignored" => fs::write(fixture.tree.join(".env"), "new ignored file").unwrap(),
                _ => fs::write(
                    fixture.root.join("ownership"),
                    "@7\x1ftest-token\x1e\n@8\x1ftest-token\x1e\n",
                )
                .unwrap(),
            }
            assert!(
                fixture.engine.cleanup_work_item(&preview).await.is_err(),
                "{change}"
            );
            assert!(!fixture.root.join("invocation").exists(), "{change}");
            assert!(fixture.tree.exists());
            assert_eq!(fixture.engine.work_items().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn failed_noop_and_partial_owner_cleanup_never_unregister_or_retry() {
        for outcome in ["failure", "noop", "partial"] {
            let fixture = Fixture::new();
            let preview = fixture.preview().await;
            fs::write(fixture.root.join(outcome), "").unwrap();
            let error = fixture
                .engine
                .cleanup_work_item(&preview)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("kept"), "{error}");
            assert_eq!(
                fixture.engine.work_items().unwrap(),
                vec![fixture.item.clone()]
            );
            assert!(fixture.root.join("window-present").exists());
            assert_eq!(
                fs::read_to_string(fixture.root.join("invocation"))
                    .unwrap()
                    .matches("remove")
                    .count(),
                1
            );
            git(
                &fixture.repo,
                &["show-ref", "--verify", "refs/heads/feature"],
            );
        }
    }

    #[tokio::test]
    async fn registry_lock_and_missing_commands_fail_before_destructive_work() {
        let mut fixture = Fixture::new();
        let preview = fixture.preview().await;
        let lock = fixture.engine.store.lock().unwrap();
        assert!(fixture.engine.cleanup_work_item(&preview).await.is_err());
        drop(lock);
        fixture.untouched();
        fixture.engine.workmux.executable = "/missing/workbench-test-workmux".into();
        assert!(
            fixture
                .engine
                .prepare_cleanup(&fixture.item)
                .await
                .unwrap_err()
                .to_string()
                .contains("Install workmux")
        );
        fixture.untouched();
    }

    #[tokio::test]
    async fn replaced_directory_and_symlink_targets_cannot_reuse_an_approved_preview() {
        let fixture = Fixture::new();
        let preview = fixture.preview().await;
        let original = fixture.root.join("original-task");
        fs::rename(&fixture.tree, &original).unwrap();
        fs::create_dir(&fixture.tree).unwrap();
        for name in [".git", ".gitignore", "source"] {
            fs::copy(original.join(name), fixture.tree.join(name)).unwrap();
        }
        assert!(
            fixture
                .engine
                .cleanup_work_item(&preview)
                .await
                .unwrap_err()
                .to_string()
                .contains("changed")
        );
        fixture.untouched();
        fs::rename(&fixture.tree, fixture.root.join("replacement-task")).unwrap();
        std::os::unix::fs::symlink(&original, &fixture.tree).unwrap();
        assert!(fixture.engine.prepare_cleanup(&fixture.item).await.is_err());
        fixture.untouched();
        assert!(original.join("source").exists());
    }

    #[test]
    fn staging_unregister_does_not_remove_registration_until_commit() {
        let fixture = Fixture::new();
        let before = fs::read(fixture.engine.store.state_path()).unwrap();
        let lock = fixture.engine.store.lock().unwrap();
        let pending = fixture
            .engine
            .store
            .stage_unregister(&fixture.item, &lock)
            .unwrap();
        assert_eq!(fs::read(fixture.engine.store.state_path()).unwrap(), before);
        drop(pending);
        fixture.untouched();
        let pending = fixture
            .engine
            .store
            .stage_unregister(&fixture.item, &lock)
            .unwrap();
        pending.commit().unwrap();
        assert!(fixture.engine.work_items().unwrap().is_empty());
        assert!(fixture.tree.exists());
        assert!(fixture.root.join("window-present").exists());
    }

    #[tokio::test]
    async fn stale_branch_message_identifies_both_names_and_dirty_files() {
        let mut fixture = Fixture::new();
        fixture.engine.unregister_work_item(&fixture.item).unwrap();
        fixture.item.branch = Some("feature-typo".into());
        fixture
            .engine
            .register_work_item(fixture.item.clone())
            .unwrap();
        for dirty in [false, true] {
            if dirty {
                fs::write(fixture.tree.join("source"), "changed\n").unwrap();
            }
            let message = fixture
                .engine
                .prepare_cleanup(&fixture.item)
                .await
                .unwrap_err()
                .to_string();
            assert!(message.contains("Registered branch \"feature-typo\""));
            assert!(message.contains("current Git branch \"feature\""));
            assert!(message.contains("SESSIONS"));
            assert_eq!(
                message.contains("also has staged, unstaged, or untracked"),
                dirty
            );
            fixture.untouched();
        }
    }

    #[test]
    fn parsers_reject_truncation_ambiguity_unsafe_paths_and_conflicting_ownership() {
        assert!(nul_records(b"incomplete").is_err());
        assert!(paths(b"../outside\0").is_err());
        assert!(paths(b"/outside\0").is_err());
        assert!(
            config_values(
                b"workmux.worktree.task.mode\nwindow\0workmux.worktree.task.mode\nsession\0"
            )
            .is_err()
        );
        assert!(ownership_records("@7\x1fone\x1e\n@7\x1ftwo\x1e\n").is_err());
        assert!(ownership_records("bad\x1fone\x1e").is_err());
        assert!(ownership_records("@7\x1fincomplete").is_err());
        assert!(worktree_records(b"worktree relative\0").is_err());
        let records =
            worktree_records(b"worktree /repo\0\0worktree /repo/task\0locked reason\0\0").unwrap();
        assert_eq!(records.len(), 2);
        assert!(!records[0].locked);
        assert!(records[1].locked);
    }

    #[tokio::test]
    async fn command_timeouts_and_output_limits_are_actionable() {
        let client = WorkmuxClient {
            executable: "/bin/sleep".into(),
            limit: Duration::from_millis(20),
            pane: None,
        };
        assert!(
            client
                .run(Path::new("/private/tmp"), &["2"], None)
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(read_bounded(&b"12345"[..], 4).await.is_err());
    }
}
