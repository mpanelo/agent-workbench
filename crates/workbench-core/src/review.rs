use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use crate::WorkItemDiff;

/// Review marks apply only to this captured diff and live only in memory (M6).
/// A new capture starts unreviewed; persistence and change tracking are M7/M8.
#[derive(Clone, Debug)]
pub struct ReviewSession {
    pub diff: WorkItemDiff,
    reviewed: HashSet<PathBuf>,
}

impl ReviewSession {
    pub fn new(diff: WorkItemDiff) -> Self {
        Self {
            diff,
            reviewed: HashSet::new(),
        }
    }

    pub fn is_reviewed(&self, path: &Path) -> bool {
        self.reviewed.contains(path)
    }

    /// Returns None for a path outside this diff, otherwise the new mark.
    pub fn toggle_reviewed(&mut self, path: &Path) -> Option<bool> {
        if !self.diff.files.iter().any(|file| file.path == path) {
            return None;
        }
        if self.reviewed.remove(path) {
            Some(false)
        } else {
            self.reviewed.insert(path.to_owned());
            Some(true)
        }
    }

    pub fn progress(&self) -> (usize, usize) {
        (self.reviewed.len(), self.diff.files.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use crate::git::tests::fixture;

    #[tokio::test]
    async fn marks_are_file_scoped_reversible_and_reset_for_a_new_capture() {
        let (dir, engine) = fixture();
        std::fs::write(dir.path().join("workspace/source.txt"), "review this\n").unwrap();
        let diff = engine.diff("A", None).await.unwrap();
        let mut review = ReviewSession::new(diff.clone());
        assert_eq!(review.progress(), (0, 1));
        let path = Path::new("source.txt");
        assert_eq!(review.toggle_reviewed(Path::new("not-in-this-diff")), None);
        assert_eq!(review.toggle_reviewed(path), Some(true));
        assert!(review.is_reviewed(path));
        assert_eq!(review.progress(), (1, 1));
        assert_eq!(review.toggle_reviewed(path), Some(false));
        assert_eq!(review.progress(), (0, 1));
        review.toggle_reviewed(path);
        assert_eq!(ReviewSession::new(diff).progress(), (0, 1));
        let restarted = Engine::new(dir.path().join("items.json"));
        assert_eq!(
            ReviewSession::new(restarted.diff("A", None).await.unwrap()).progress(),
            (0, 1)
        );
    }
}
