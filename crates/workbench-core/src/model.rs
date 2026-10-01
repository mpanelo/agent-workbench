use std::path::PathBuf;

/// A complete discovery result; each refresh replaces the previous snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub sessions: Vec<Session>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// Stable tmux session ID, such as `$1`.
    pub id: String,
    pub name: String,
    pub windows: Vec<Window>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// Stable tmux window ID, such as `@2`; may be linked to multiple sessions.
    pub id: String,
    pub index: u32,
    pub name: String,
    pub panes: Vec<Pane>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    /// Stable tmux pane ID, such as `%14`.
    pub id: String,
    pub index: u32,
    pub title: String,
    pub current_command: Option<String>,
    pub working_directory: Option<PathBuf>,
}
