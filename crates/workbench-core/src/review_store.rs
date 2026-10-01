//! Companion state; never migrates or rewrites work-item registrations.
use crate::work_items::StateLock;
use crate::{ChangedFile, ReviewError, ReviewSession, WorkItemDiff};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const MAX_STATE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredState {
    version: u32,
    reviews: Vec<StoredReview>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredReview {
    work_item_id: String,
    workspace: PathBuf,
    base_revision: String,
    files: Vec<ChangedFile>,
}

impl StoredReview {
    fn matches(&self, diff: &WorkItemDiff) -> bool {
        self.work_item_id == diff.work_item_id
            && self.workspace == diff.workspace
            && self.base_revision == diff.base_revision
    }
}

pub(crate) struct ReviewStore {
    path: PathBuf,
}

impl ReviewStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn io_error(&self, source: io::Error) -> ReviewError {
        ReviewError::Io {
            path: self.path.clone(),
            source,
        }
    }

    fn load(&self) -> Result<StoredState, ReviewError> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(StoredState {
                    version: 1,
                    reviews: vec![],
                });
            }
            Err(error) => return Err(self.io_error(error)),
        };
        let mut bytes = Vec::new();
        file.take((MAX_STATE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| self.io_error(error))?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(ReviewError::Invalid("review state exceeds 64 MiB".into()));
        }
        let state: StoredState = serde_json::from_slice(&bytes)
            .map_err(|error| ReviewError::Invalid(format!("{}: {error}", self.path.display())))?;
        validate(&state)?;
        Ok(state)
    }

    pub(crate) fn restore(&self, diff: WorkItemDiff) -> Result<ReviewSession, ReviewError> {
        let state = self.load()?;
        let reviewed = state
            .reviews
            .into_iter()
            .find(|review| review.matches(&diff))
            .map(|review| {
                review
                    .files
                    .into_iter()
                    .map(|file| (file.path.clone(), file))
                    .collect()
            })
            .unwrap_or_default();
        Ok(ReviewSession {
            diff,
            reviewed,
            since_review: None,
        })
    }

    pub(crate) fn set(
        &self,
        diff: &WorkItemDiff,
        file: &ChangedFile,
        reviewed: bool,
    ) -> Result<HashMap<PathBuf, ChangedFile>, ReviewError> {
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent).map_err(|error| self.io_error(error))?;
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
                ReviewError::Busy(self.path.clone())
            } else {
                self.io_error(error)
            }
        })?;
        let _lock = StateLock(lock);
        let mut state = self.load()?;
        let index = match state.reviews.iter().position(|review| review.matches(diff)) {
            Some(index) => index,
            None => {
                state.reviews.push(StoredReview {
                    work_item_id: diff.work_item_id.clone(),
                    workspace: diff.workspace.clone(),
                    base_revision: diff.base_revision.clone(),
                    files: vec![],
                });
                state.reviews.len() - 1
            }
        };
        let target = &mut state.reviews[index];
        target.files.retain(|snapshot| snapshot.path != file.path);
        if reviewed {
            target.files.push(file.clone());
        }
        target.files.sort_by(|a, b| a.path.cmp(&b.path));
        let restored = target
            .files
            .iter()
            .map(|file| (file.path.clone(), file.clone()))
            .collect();
        if target.files.is_empty() {
            state.reviews.remove(index);
        }
        validate(&state)?;
        let bytes = serde_json::to_vec_pretty(&state)
            .map_err(|error| ReviewError::Invalid(error.to_string()))?;
        if bytes.len() >= MAX_STATE_BYTES {
            return Err(ReviewError::Invalid("review state exceeds 64 MiB".into()));
        }
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| self.io_error(error))?;
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.write_all(b"\n"))
            .map_err(|error| self.io_error(error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| self.io_error(error))?;
        temporary
            .persist(&self.path)
            .map_err(|error| self.io_error(error.error))?;
        Ok(restored)
    }
}

fn validate(state: &StoredState) -> Result<(), ReviewError> {
    if state.version != 1 {
        return Err(ReviewError::Invalid(format!(
            "unsupported schema version {}",
            state.version
        )));
    }
    let mut identities = HashSet::new();
    for review in &state.reviews {
        if review.work_item_id.trim().is_empty()
            || review.work_item_id != review.work_item_id.trim()
            || review.work_item_id.chars().any(char::is_control)
            || !review.workspace.is_absolute()
            || !matches!(review.base_revision.len(), 40 | 64)
            || !review
                .base_revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || !identities.insert((
                &review.work_item_id,
                &review.workspace,
                &review.base_revision,
            ))
        {
            return Err(ReviewError::Invalid(
                "invalid or duplicate review identity".into(),
            ));
        }
        let mut paths = HashSet::new();
        for file in &review.files {
            for path in std::iter::once(&file.path).chain(file.old_path.iter()) {
                let text = path
                    .to_str()
                    .ok_or_else(|| ReviewError::Invalid("snapshot path is not UTF-8".into()))?;
                crate::git::parse_path(text)
                    .map_err(|error| ReviewError::Invalid(error.to_string()))?;
            }
            if file.patch.is_empty()
                || !paths.insert(&file.path)
                || file.additions.is_some() != file.deletions.is_some()
            {
                return Err(ReviewError::Invalid(
                    "empty or duplicate file snapshot".into(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChangeKind;

    fn diff() -> WorkItemDiff {
        WorkItemDiff {
            work_item_id: "Task λ".into(),
            title: "Task".into(),
            workspace: "/work/task".into(),
            base: "HEAD".into(),
            base_revision: "a".repeat(40),
            files: vec![ChangedFile {
                path: "source.rs".into(),
                old_path: None,
                kind: ChangeKind::Modified,
                additions: Some(1),
                deletions: Some(1),
                patch: "@@ -1 +1 @@\n-before\n+after\n".into(),
            }],
        }
    }

    #[test]
    fn corrupt_duplicate_and_unsafe_snapshots_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore::new(dir.path().join("reviews.json"));
        let capture = diff();
        store.set(&capture, &capture.files[0], true).unwrap();
        let good: serde_json::Value =
            serde_json::from_slice(&fs::read(&store.path).unwrap()).unwrap();
        let mut bad = Vec::new();
        let mut value = good.clone();
        value["extra"] = true.into();
        bad.push(value);
        let mut value = good.clone();
        value["reviews"][0]["base_revision"] = "not-a-revision".into();
        bad.push(value);
        let mut value = good.clone();
        value["reviews"][0]["workspace"] = "relative".into();
        bad.push(value);
        let mut value = good.clone();
        value["reviews"][0]["files"][0]["path"] = "../outside".into();
        bad.push(value);
        let mut value = good.clone();
        value["reviews"][0]["files"][0]["old_path"] = "/outside".into();
        bad.push(value);
        let mut value = good.clone();
        value["reviews"][0]["files"][0]["patch"] = "".into();
        bad.push(value);
        let mut value = good.clone();
        let duplicate = value["reviews"][0]["files"][0].clone();
        value["reviews"][0]["files"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        bad.push(value);
        let mut value = good;
        let duplicate = value["reviews"][0].clone();
        value["reviews"].as_array_mut().unwrap().push(duplicate);
        bad.push(value);
        for value in bad {
            let before = serde_json::to_vec(&value).unwrap();
            fs::write(&store.path, &before).unwrap();
            assert!(store.restore(capture.clone()).is_err());
            assert!(store.set(&capture, &capture.files[0], false).is_err());
            assert_eq!(fs::read(&store.path).unwrap(), before);
        }
    }

    #[test]
    fn oversized_state_is_rejected_before_json_parsing_or_writing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore::new(dir.path().join("reviews.json"));
        let file = fs::File::create(&store.path).unwrap();
        file.set_len((MAX_STATE_BYTES + 1) as u64).unwrap();
        assert!(
            store
                .restore(diff())
                .unwrap_err()
                .to_string()
                .contains("64 MiB")
        );
        let capture = diff();
        assert!(
            store
                .set(&capture, &capture.files[0], true)
                .unwrap_err()
                .to_string()
                .contains("64 MiB")
        );
        assert_eq!(
            fs::metadata(&store.path).unwrap().len(),
            (MAX_STATE_BYTES + 1) as u64
        );
    }

    #[test]
    fn workspace_and_base_contexts_never_share_marks() {
        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore::new(dir.path().join("reviews.json"));
        let capture = diff();
        store.set(&capture, &capture.files[0], true).unwrap();
        let mut other = capture.clone();
        other.workspace = "/other/task".into();
        assert_eq!(store.restore(other.clone()).unwrap().progress(), (0, 1));
        store.set(&other, &other.files[0], true).unwrap();
        other.base_revision = "b".repeat(40);
        assert_eq!(store.restore(other).unwrap().progress(), (0, 1));
        assert_eq!(store.restore(capture).unwrap().progress(), (1, 1));
    }
}
