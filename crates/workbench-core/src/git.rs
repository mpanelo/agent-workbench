use std::{
    error::Error,
    ffi::OsString,
    fmt, io,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};

use crate::{Engine, WorkItem, WorkItemError};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const REVIEW_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT: usize = 2 * 1024 * 1024;
const MAX_TOTAL_PATCH: usize = 16 * 1024 * 1024;
const MAX_FILES: usize = 512;
const MAX_PATCH_LINES: usize = 60_000;
const MAX_LINE_BYTES: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    Untracked,
    Recreated,
}

impl fmt::Display for ChangeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::TypeChanged => "type changed",
            Self::Untracked => "untracked",
            Self::Recreated => "removed from index + untracked",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    /// Literal repository-relative path (never a Git pathspec expression).
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub kind: ChangeKind,
    /// None for binary content. Gitlinks/submodules show their pointer diff only.
    pub additions: Option<usize>,
    pub deletions: Option<usize>,
    pub patch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkItemDiff {
    pub work_item_id: String,
    pub title: String,
    pub workspace: PathBuf,
    pub base: String,
    pub base_revision: String,
    pub files: Vec<ChangedFile>,
}

impl WorkItemDiff {
    pub fn line_totals(&self) -> (usize, usize) {
        self.files.iter().fold((0, 0), |(a, d), file| {
            (
                a + file.additions.unwrap_or(0),
                d + file.deletions.unwrap_or(0),
            )
        })
    }
}

#[derive(Debug)]
pub enum GitError {
    WorkItems(WorkItemError),
    UnknownWorkItem(String),
    Io(io::Error),
    Failed(String),
    Invalid(String),
    TimedOut,
    TooLarge,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkItems(error) => error.fmt(f),
            Self::UnknownWorkItem(id) => write!(f, "Work item {id:?} is not registered."),
            Self::Io(error) if error.kind() == io::ErrorKind::NotFound => write!(
                f,
                "Git or the workspace was not found. Install Git on PATH and check the registered workspace."
            ),
            Self::Io(error) => write!(f, "Could not read Git workspace: {error}"),
            Self::Failed(message) => write!(f, "Git review failed: {message}"),
            Self::Invalid(message) => write!(f, "Cannot review workspace: {message}"),
            Self::TimedOut => write!(
                f,
                "Git review timed out. Retry or inspect the workspace with Git."
            ),
            Self::TooLarge => write!(
                f,
                "Diff exceeds the review limit (512 files, 2 MiB per command, 16 MiB total patches, 60000 lines per file, 8 KiB per line). Use an external diff viewer; no partial review was loaded."
            ),
        }
    }
}

impl Error for GitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::WorkItems(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct GitClient {
    executable: OsString,
    command_timeout: Duration,
}

impl Default for GitClient {
    fn default() -> Self {
        Self {
            executable: "git".into(),
            command_timeout: COMMAND_TIMEOUT,
        }
    }
}

impl Engine {
    /// Read local changes in the registered workspace, independent of tmux.
    /// Default base is HEAD; an explicit local commit/ref compares the final
    /// working tree directly to that commit (not an inferred merge-base).
    pub async fn diff(&self, id: &str, base: Option<&str>) -> Result<WorkItemDiff, GitError> {
        let item = self
            .work_items()
            .map_err(GitError::WorkItems)?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| GitError::UnknownWorkItem(id.into()))?;
        timeout(REVIEW_TIMEOUT, self.git.diff(&item, base.unwrap_or("HEAD")))
            .await
            .map_err(|_| GitError::TimedOut)?
    }
}

impl GitClient {
    /// Registration needs checkout metadata, not a valid diff base or clean index.
    pub(crate) async fn registration_metadata(
        &self,
        directory: &Path,
    ) -> Result<(PathBuf, PathBuf, Option<String>), GitError> {
        let root = self
            .run(directory, &args(&["rev-parse", "--show-toplevel"]), false)
            .await?;
        let workspace = PathBuf::from(text(&root)?.strip_suffix('\n').unwrap_or(text(&root)?));
        if !workspace.is_absolute() {
            return Err(GitError::Invalid(
                "Git returned a relative workspace".into(),
            ));
        }
        // The first porcelain worktree record is the main checkout (or bare
        // repository). NUL delimiting preserves spaces/newlines in its path.
        let worktrees = self
            .run(
                directory,
                &args(&["worktree", "list", "--porcelain", "-z"]),
                false,
            )
            .await?;
        let repository = nul_fields(&worktrees)?
            .first()
            .and_then(|line| line.strip_prefix("worktree "))
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| GitError::Invalid("Git returned no main repository path".into()))?;
        let branch = self
            .run(
                directory,
                &args(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
                true,
            )
            .await?;
        let branch = text(&branch)?.strip_suffix('\n').unwrap_or(text(&branch)?);
        Ok((
            repository,
            workspace,
            (!branch.is_empty()).then(|| branch.to_owned()),
        ))
    }

    fn command(&self, workspace: &Path) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .current_dir(workspace)
            .args([
                "--no-pager",
                "--literal-pathspecs",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "diff.renames=true",
            ])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // A launch from a hook/other checkout must still inspect the registered workspace.
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
        command
    }

    async fn run(
        &self,
        workspace: &Path,
        args: &[OsString],
        difference_exit: bool,
    ) -> Result<Vec<u8>, GitError> {
        let mut command = self.command(workspace);
        command.args(args);
        timeout(self.command_timeout, async {
            let mut child = command.spawn().map_err(GitError::Io)?;
            let stdout = child.stdout.take().expect("stdout is piped");
            let stderr = child.stderr.take().expect("stderr is piped");
            let (stdout, stderr, status) = tokio::try_join!(
                read_bounded(stdout, MAX_OUTPUT),
                read_bounded(stderr, 16 * 1024),
                async { child.wait().await.map_err(GitError::Io) }
            )?;
            if !(status.success() || difference_exit && status.code() == Some(1)) {
                return Err(GitError::Failed(
                    String::from_utf8_lossy(&stderr).trim().to_owned(),
                ));
            }
            Ok(stdout)
        })
        .await
        .map_err(|_| GitError::TimedOut)?
    }

    async fn diff(&self, item: &WorkItem, base: &str) -> Result<WorkItemDiff, GitError> {
        if base.is_empty() || base.chars().any(char::is_control) {
            return Err(GitError::Invalid(
                "base must be a nonempty local commit/ref, without control characters".into(),
            ));
        }
        let root = self
            .run(
                &item.workspace,
                &args(&["rev-parse", "--show-toplevel"]),
                false,
            )
            .await?;
        let root = text(&root)?.strip_suffix('\n').unwrap_or(text(&root)?);
        let workspace = std::fs::canonicalize(&item.workspace).map_err(GitError::Io)?;
        if Path::new(root) != workspace {
            return Err(GitError::Invalid("registered workspace must be the root of a Git checkout/worktree (not a subdirectory or a bare repository)".into()));
        }
        let revision = self
            .run(
                &workspace,
                &args(&[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{base}^{{commit}}"),
                ]),
                false,
            )
            .await?;
        let revision = text(&revision)?.trim().to_owned();
        if !matches!(revision.len(), 40 | 64)
            || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(GitError::Invalid(
                "Git returned an invalid base revision".into(),
            ));
        }
        if !self
            .run(&workspace, &args(&["ls-files", "--unmerged", "-z"]), false)
            .await?
            .is_empty()
        {
            return Err(GitError::Invalid(
                "resolve merge conflicts before reviewing the workspace".into(),
            ));
        }
        let names = self
            .run(
                &workspace,
                &diff_args(&["--name-status", "-z", &revision, "--"]),
                false,
            )
            .await?;
        let mut files = parse_names(&names)?;
        let untracked = self
            .run(
                &workspace,
                &args(&["ls-files", "--others", "--exclude-standard", "-z"]),
                false,
            )
            .await?;
        for path in nul_fields(&untracked)? {
            let path = parse_path(path)?;
            if let Some(file) = files.iter_mut().find(|file| file.path == path) {
                if file.kind != ChangeKind::Deleted {
                    return Err(GitError::Invalid(
                        "workspace changed during capture; reload the diff".into(),
                    ));
                }
                file.kind = ChangeKind::Recreated;
            } else {
                files.push(empty_file(path, None, ChangeKind::Untracked));
            }
        }
        if files.len() > MAX_FILES {
            return Err(GitError::TooLarge);
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut total = 0;
        for file in &mut files {
            let mut arguments = if file.kind == ChangeKind::Untracked {
                diff_args(&["--no-index", "--", "/dev/null"])
            } else {
                diff_args(&[&revision, "--"])
            };
            if let Some(old) = &file.old_path {
                arguments.push(old.as_os_str().to_owned());
            }
            arguments.push(file.path.as_os_str().to_owned());
            let mut patch = self
                .run(&workspace, &arguments, file.kind == ChangeKind::Untracked)
                .await?;
            if file.kind == ChangeKind::Recreated {
                let mut arguments = diff_args(&["--no-index", "--", "/dev/null"]);
                arguments.push(file.path.as_os_str().to_owned());
                patch.extend(self.run(&workspace, &arguments, true).await?);
            }
            total += patch.len();
            if total > MAX_TOTAL_PATCH {
                return Err(GitError::TooLarge);
            }
            file.patch = text(&patch)?.to_owned();
            if file.patch.lines().count() > MAX_PATCH_LINES
                || file.patch.lines().any(|line| line.len() > MAX_LINE_BYTES)
            {
                return Err(GitError::TooLarge);
            }
            (file.additions, file.deletions) = patch_counts(&file.patch);
            if file.patch.is_empty() {
                return Err(GitError::Invalid(
                    "workspace changed during capture; reload the diff".into(),
                ));
            }
        }
        Ok(WorkItemDiff {
            work_item_id: item.id.clone(),
            title: item.title.clone(),
            workspace,
            base: base.into(),
            base_revision: revision,
            files,
        })
    }
}

async fn read_bounded(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, GitError> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(GitError::Io)?;
    if bytes.len() > limit {
        Err(GitError::TooLarge)
    } else {
        Ok(bytes)
    }
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn diff_args(values: &[&str]) -> Vec<OsString> {
    let mut result = args(&[
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        "--submodule=short",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--unified=3",
    ]);
    result.extend(args(values));
    result
}

fn text(bytes: &[u8]) -> Result<&str, GitError> {
    std::str::from_utf8(bytes).map_err(|_| {
        GitError::Invalid(
            "Git paths/diff text must be UTF-8; use an external viewer for undecodable content"
                .into(),
        )
    })
}

fn nul_fields(bytes: &[u8]) -> Result<Vec<&str>, GitError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let data = text(bytes)?
        .strip_suffix('\0')
        .ok_or_else(|| GitError::Invalid("incomplete NUL-delimited Git output".into()))?;
    Ok(data.split('\0').collect())
}

fn parse_path(path: &str) -> Result<PathBuf, GitError> {
    let path = PathBuf::from(path);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(GitError::Invalid(
            "Git returned an unsafe repository-relative path".into(),
        ));
    }
    Ok(path)
}

fn empty_file(path: PathBuf, old_path: Option<PathBuf>, kind: ChangeKind) -> ChangedFile {
    ChangedFile {
        path,
        old_path,
        kind,
        additions: Some(0),
        deletions: Some(0),
        patch: String::new(),
    }
}

fn parse_names(bytes: &[u8]) -> Result<Vec<ChangedFile>, GitError> {
    let fields = nul_fields(bytes)?;
    let mut iter = fields.into_iter();
    let mut files = Vec::new();
    while let Some(status) = iter.next() {
        let kind = match status {
            "A" => ChangeKind::Added,
            "M" => ChangeKind::Modified,
            "D" => ChangeKind::Deleted,
            "T" => ChangeKind::TypeChanged,
            value
                if value.strip_prefix('R').is_some_and(|score| {
                    !score.is_empty() && score.bytes().all(|byte| byte.is_ascii_digit())
                }) =>
            {
                ChangeKind::Renamed
            }
            _ => {
                return Err(GitError::Invalid(format!(
                    "unsupported Git status {status:?}"
                )));
            }
        };
        let first = parse_path(
            iter.next()
                .ok_or_else(|| GitError::Invalid("missing diff path".into()))?,
        )?;
        let (path, old_path) = if kind == ChangeKind::Renamed {
            (
                parse_path(
                    iter.next()
                        .ok_or_else(|| GitError::Invalid("missing rename destination".into()))?,
                )?,
                Some(first),
            )
        } else {
            (first, None)
        };
        if files.iter().any(|file: &ChangedFile| file.path == path) {
            return Err(GitError::Invalid("duplicate diff path".into()));
        }
        files.push(empty_file(path, old_path, kind));
        if files.len() > MAX_FILES {
            return Err(GitError::TooLarge);
        }
    }
    Ok(files)
}

fn patch_counts(patch: &str) -> (Option<usize>, Option<usize>) {
    let mut in_hunk = false;
    let (mut added, mut deleted) = (0, 0);
    for line in patch.lines() {
        if line.starts_with("Binary files ") || line == "GIT binary patch" {
            return (None, None);
        }
        if line.starts_with("diff --git ") {
            in_hunk = false;
        }
        if line.starts_with("@@ ") {
            in_hunk = true;
        } else if in_hunk {
            if line.starts_with('+') {
                added += 1;
            }
            if line.starts_with('-') {
                deleted += 1;
            }
        }
    }
    (Some(added), Some(deleted))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::WorkItemKind;
    use std::fs;

    // Synthetic repositories have isolated Git config and never use the user's
    // signing key, hooks, index, branches, or working files.
    pub(crate) fn git(root: &Path, values: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args([
                "-c",
                "user.name=Workbench test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(values)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {values:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    pub(crate) fn fixture() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        fs::write(root.join("source.txt"), "one\nold\n").unwrap();
        fs::write(
            root.join("rename source.txt"),
            "preserve this renamed file\n",
        )
        .unwrap();
        fs::write(root.join("-delete.txt"), "deleted\n").unwrap();
        fs::write(root.join("binary"), b"old\0binary").unwrap();
        fs::write(root.join(".gitignore"), "*.ignored\n").unwrap();
        git(&root, &["add", "--", "."]);
        git(&root, &["commit", "-m", "Synthetic initial revision"]);
        let engine = Engine::new(dir.path().join("items.json"));
        engine
            .register_work_item(WorkItem {
                id: "A".into(),
                title: "Review test".into(),
                repository: dir.path().join("not-the-checkout"),
                workspace: root,
                branch: Some("main".into()),
                kind: WorkItemKind::Implementation,
                pane_id: "%999999".into(),
            })
            .unwrap();
        (dir, engine)
    }

    #[test]
    fn parses_nul_names_with_rename_unicode_controls_and_literal_pathspecs() {
        let files = parse_names(b"M\0:(glob)*\0R100\0old\tname\0new\nname\0A\0\xce\xbb\0").unwrap();
        assert_eq!(files[0].path, Path::new(":(glob)*"));
        assert_eq!(files[1].path, Path::new("new\nname"));
        assert_eq!(files[1].old_path.as_deref(), Some(Path::new("old\tname")));
        assert_eq!(files[2].path, Path::new("λ"));
        for bytes in [
            b"M\0file".as_slice(),
            b"R100\0old\0",
            b"U\0file\0",
            b"M\0../outside\0",
            b"M\0/absolute\0",
            b"M\0file\0M\0file\0",
            b"M\0\xff\0",
        ] {
            assert!(parse_names(bytes).is_err(), "accepted {bytes:?}");
        }
    }

    #[test]
    fn counts_only_hunk_lines_including_header_like_content_and_binary_exclusions() {
        assert_eq!(
            patch_counts(
                "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1,2 @@\n---old text\n+++new text\n+second\n\\ No newline at end of file\n"
            ),
            (Some(2), Some(1))
        );
        assert_eq!(
            patch_counts("Binary files a/f and b/f differ\n"),
            (None, None)
        );
        assert_eq!(
            patch_counts("old mode 100644\nnew mode 100755\n"),
            (Some(0), Some(0))
        );
    }

    #[tokio::test]
    async fn captures_staged_unstaged_untracked_deleted_renamed_and_binary_without_writes() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        fs::write(root.join("source.txt"), "one\nnew\n").unwrap();
        fs::rename(
            root.join("rename source.txt"),
            root.join("rename destination.txt"),
        )
        .unwrap();
        fs::remove_file(root.join("-delete.txt")).unwrap();
        fs::write(root.join("binary"), b"new\0binary").unwrap();
        git(&root, &["add", "--", "."]);
        fs::write(root.join("source.txt"), "one\nnew\nextra\n").unwrap();
        for (name, contents) in [
            ("untracked λ\t\n.txt", "new\nfile\n"),
            (":(glob)*", "literal\n"),
            ("-leading-option", "safe\n"),
            ("empty", ""),
            ("secret.ignored", "not shown"),
        ] {
            fs::write(root.join(name), contents).unwrap();
        }
        let index = fs::read(root.join(".git/index")).unwrap();
        let store = fs::read(dir.path().join("items.json")).unwrap();
        let diff = engine.diff("A", None).await.unwrap();
        assert_eq!(diff.files.len(), 8);
        assert_eq!(diff.workspace, fs::canonicalize(&root).unwrap());
        assert_eq!(diff.base, "HEAD");
        let source = diff
            .files
            .iter()
            .find(|file| file.path == Path::new("source.txt"))
            .unwrap();
        assert_eq!((source.additions, source.deletions), (Some(2), Some(1)));
        assert!(source.patch.contains("+extra"));
        let rename = diff
            .files
            .iter()
            .find(|file| file.kind == ChangeKind::Renamed)
            .unwrap();
        assert_eq!(
            rename.old_path.as_deref(),
            Some(Path::new("rename source.txt"))
        );
        let binary = diff
            .files
            .iter()
            .find(|file| file.path == Path::new("binary"))
            .unwrap();
        assert_eq!(binary.additions, None);
        assert_eq!(diff.line_totals(), (6, 2));
        assert!(
            !diff
                .files
                .iter()
                .any(|file| file.path == Path::new("secret.ignored"))
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(dir.path().join("items.json")).unwrap(), store);
        assert!(!root.join(".git/index.lock").exists());
    }

    #[tokio::test]
    async fn explicit_base_includes_committed_changes_and_default_head_does_not() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        let base = git(&root, &["rev-parse", "HEAD"]).trim().to_owned();
        fs::write(root.join("source.txt"), "one\ncommitted\n").unwrap();
        git(&root, &["add", "--", "."]);
        git(&root, &["commit", "-m", "Synthetic branch changes"]);
        assert!(engine.diff("A", None).await.unwrap().files.is_empty());
        let diff = engine.diff("A", Some(&base)).await.unwrap();
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.base_revision, base);
        assert!(diff.files[0].patch.contains("+committed"));
        assert!(engine.diff("A", Some("missing-ref")).await.is_err());
        assert!(
            engine
                .diff("A", Some("--output=/tmp/injected"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn deleted_from_index_and_recreated_untracked_content_are_both_visible() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        git(&root, &["rm", "--cached", "--", "source.txt"]);
        fs::write(root.join("source.txt"), "replacement\n").unwrap();
        let diff = engine.diff("A", None).await.unwrap();
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].kind, ChangeKind::Recreated);
        assert!(diff.files[0].patch.contains("-old"));
        assert!(diff.files[0].patch.contains("+replacement"));
    }

    #[tokio::test]
    async fn registered_workspace_not_repository_is_used_and_linked_worktrees_work() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        let linked = dir.path().join("linked");
        git(
            &root,
            &["worktree", "add", "-b", "task", linked.to_str().unwrap()],
        );
        let mut item = engine.work_items().unwrap().remove(0);
        item.id = "linked".into();
        item.workspace = linked.clone();
        engine.register_work_item(item).unwrap();
        fs::write(linked.join("source.txt"), "linked changes\n").unwrap();
        let diff = engine.diff("linked", None).await.unwrap();
        assert!(diff.files[0].patch.contains("+linked changes"));
        assert!(engine.diff("A", None).await.unwrap().files.is_empty());
    }

    #[tokio::test]
    async fn errors_are_actionable_for_unknown_missing_non_git_nested_unborn_and_conflicted_workspaces()
     {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        assert!(matches!(
            engine.diff("unknown", None).await,
            Err(GitError::UnknownWorkItem(_))
        ));
        for (id, path, initialize) in [
            ("missing", dir.path().join("missing"), false),
            ("non-git", dir.path().join("non-git"), false),
            ("nested", root.join("nested"), false),
            ("unborn", dir.path().join("unborn"), true),
        ] {
            if id != "missing" {
                fs::create_dir(&path).unwrap();
            }
            if initialize {
                git(&path, &["init", "-b", "main"]);
            }
            let mut item = engine.work_items().unwrap().remove(0);
            item.id = id.into();
            item.workspace = path;
            engine.register_work_item(item).unwrap();
            assert!(engine.diff(id, None).await.is_err(), "accepted {id}");
        }
        git(&root, &["checkout", "-b", "other"]);
        fs::write(root.join("source.txt"), "other branch\n").unwrap();
        git(&root, &["add", "--", "."]);
        git(&root, &["commit", "-m", "Synthetic other"]);
        git(&root, &["checkout", "main"]);
        fs::write(root.join("source.txt"), "main branch\n").unwrap();
        git(&root, &["add", "--", "."]);
        git(&root, &["commit", "-m", "Synthetic main"]);
        let output = std::process::Command::new("git")
            .current_dir(&root)
            .args(["merge", "other"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            matches!(engine.diff("A", None).await, Err(GitError::Invalid(message)) if message.contains("conflicts"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_show_targets_without_reading_external_content_and_helpers_are_disabled() {
        let (dir, engine) = fixture();
        let root = dir.path().join("workspace");
        fs::write(dir.path().join("outside"), "PRIVATE-OUTSIDE-CONTENT").unwrap();
        std::os::unix::fs::symlink(dir.path().join("outside"), root.join("link")).unwrap();
        git(&root, &["config", "diff.external", "false"]);
        git(&root, &["config", "diff.hostile.textconv", "false"]);
        fs::write(root.join(".gitattributes"), "*.txt diff=hostile\n").unwrap();
        fs::write(root.join("source.txt"), "new local changes\n").unwrap();
        let diff = engine.diff("A", None).await.unwrap();
        assert!(
            diff.files
                .iter()
                .any(|file| file.path == Path::new("source.txt"))
        );
        let link = diff
            .files
            .iter()
            .find(|file| file.path == Path::new("link"))
            .unwrap();
        assert!(link.patch.contains("120000"));
        assert!(!link.patch.contains("PRIVATE-OUTSIDE-CONTENT"));
    }

    #[tokio::test]
    async fn bounded_capture_never_silently_returns_a_partial_diff() {
        assert!(matches!(
            read_bounded(b"123456".as_slice(), 5).await,
            Err(GitError::TooLarge)
        ));
        let (dir, engine) = fixture();
        fs::write(
            dir.path().join("workspace/large"),
            "x".repeat(MAX_LINE_BYTES + 1),
        )
        .unwrap();
        assert!(matches!(
            engine.diff("A", None).await,
            Err(GitError::TooLarge)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn missing_executable_and_slow_commands_return_errors() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let missing = GitClient {
            executable: dir.path().join("absent").into_os_string(),
            command_timeout: COMMAND_TIMEOUT,
        };
        assert!(matches!(
            missing.run(dir.path(), &[], false).await,
            Err(GitError::Io(_))
        ));
        let executable = dir.path().join("slow");
        fs::write(&executable, "#!/bin/sh\nexec sleep 5\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let slow = GitClient {
            executable: executable.into_os_string(),
            command_timeout: Duration::from_millis(20),
        };
        assert!(matches!(
            slow.run(dir.path(), &[], false).await,
            Err(GitError::TimedOut)
        ));
    }
}
