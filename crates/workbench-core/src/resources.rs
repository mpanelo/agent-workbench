use std::{fs, io, path::Path};

use crate::WorkspaceAvailability;

/// Check only the saved directory's availability; do not run Git, recreate
/// paths, or infer completion from a missing workspace.
pub(crate) fn workspace_availability(path: &Path) -> WorkspaceAvailability {
    classify(fs::metadata(path).map(|metadata| metadata.is_dir()))
}

fn classify(result: io::Result<bool>) -> WorkspaceAvailability {
    match result {
        Ok(true) => WorkspaceAvailability::Present,
        Err(error) if error.kind() == io::ErrorKind::NotFound => WorkspaceAvailability::Missing,
        Ok(false) | Err(_) => WorkspaceAvailability::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_access_is_not_evidence_of_deletion() {
        assert_eq!(classify(Ok(true)), WorkspaceAvailability::Present);
        assert_eq!(classify(Ok(false)), WorkspaceAvailability::Unavailable);
        assert_eq!(
            classify(Err(io::ErrorKind::NotFound.into())),
            WorkspaceAvailability::Missing
        );
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::NotADirectory,
            io::ErrorKind::TimedOut,
            io::ErrorKind::Other,
        ] {
            assert_eq!(
                classify(Err(kind.into())),
                WorkspaceAvailability::Unavailable
            );
        }
    }

    #[test]
    fn directory_removal_and_restoration_are_observed_without_recreating_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workspace");
        assert_eq!(
            workspace_availability(&path),
            WorkspaceAvailability::Missing
        );
        assert!(!path.exists());
        fs::create_dir(&path).unwrap();
        assert_eq!(
            workspace_availability(&path),
            WorkspaceAvailability::Present
        );
        fs::remove_dir(&path).unwrap();
        assert_eq!(
            workspace_availability(&path),
            WorkspaceAvailability::Missing
        );
        fs::create_dir(&path).unwrap();
        assert_eq!(
            workspace_availability(&path),
            WorkspaceAvailability::Present
        );
        let file = root.path().join("file");
        fs::write(&file, "not a directory").unwrap();
        assert_eq!(
            workspace_availability(&file),
            WorkspaceAvailability::Unavailable
        );
        assert_eq!(
            workspace_availability(&file.join("child")),
            WorkspaceAvailability::Unavailable
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_follow_the_workspace_target() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let link = root.path().join("workspace");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            workspace_availability(&link),
            WorkspaceAvailability::Missing
        );
        fs::create_dir(&target).unwrap();
        assert_eq!(
            workspace_availability(&link),
            WorkspaceAvailability::Present
        );
        fs::remove_dir(&target).unwrap();
        assert_eq!(
            workspace_availability(&link),
            WorkspaceAvailability::Missing
        );
    }
}
