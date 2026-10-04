//! Presentation-independent discovery and persistent work items for Agent Workbench.

use std::path::PathBuf;

mod actions;
mod agent_state;
mod approvals;
mod attention;
mod cleanup;
mod codex_signals;
mod git;
mod model;
mod registration;
mod rereview;
mod review;
mod review_store;
mod tmux;
mod work_items;

pub use actions::{ActionError, MAX_INPUT_BYTES, validate_agent_input};
pub use approvals::{ApprovalDecision, ApprovalError, ApprovalRequest};
pub use attention::{AttentionError, AttentionTracker, CompletionAcknowledgement, attention_items};
pub use cleanup::{CleanupDetails, CleanupError, CleanupPreview};
pub use git::{ChangeKind, ChangedFile, GitError, WorkItemDiff};
pub use model::{Pane, Session, Snapshot, Window};
pub use registration::{RegistrationDraft, RegistrationError};
pub use review::{ReviewError, ReviewSession, ReviewStatus};
pub use tmux::DiscoveryError;
use work_items::WorkItemStore;
pub use work_items::{
    AgentStatus, MAX_SHORT_DESCRIPTION_CHARS, PaneAvailability, WorkItem, WorkItemError,
    WorkItemKind, WorkItemState, default_state_file, validate_short_description,
    validate_work_item_id,
};

/// Discovery and work-item operations, without shell or rendering code in the API.
#[derive(Debug)]
pub struct Engine {
    store: WorkItemStore,
    tmux: tmux::TmuxClient,
    git: git::GitClient,
    workmux: cleanup::WorkmuxClient,
}

impl Engine {
    /// Select a local state file. Construction does not create any files.
    pub fn new(state_file: impl Into<PathBuf>) -> Self {
        Self {
            store: WorkItemStore::new(state_file.into()),
            tmux: tmux::TmuxClient::default(),
            git: git::GitClient::default(),
            workmux: cleanup::WorkmuxClient::default(),
        }
    }

    pub fn work_items(&self) -> Result<Vec<WorkItem>, WorkItemError> {
        self.store.load()
    }

    /// Validate and persist a manual registration. Does not require a live pane.
    pub fn register_work_item(&self, item: WorkItem) -> Result<(), WorkItemError> {
        self.store.register(item)
    }

    /// Change only the short description of the captured registration.
    /// Reject stale edits rather than overwriting another process's changes.
    pub fn update_work_item_description(
        &self,
        expected: &WorkItem,
        description: &str,
    ) -> Result<WorkItem, WorkItemError> {
        self.store.update_description(expected, description)
    }

    /// Rename an unchanged registration and copy its review snapshots to the
    /// new ID. Retain old history; never alter pane, workspace, branch, or files.
    pub fn rename_work_item(
        &self,
        expected: &WorkItem,
        id: &str,
    ) -> Result<WorkItem, WorkItemError> {
        self.store.rename(expected, id)
    }

    /// Save ID and short description together under the registry lock. If the
    /// ID changes, preserve review marks using the same safe copy as renaming.
    pub fn update_work_item_details(
        &self,
        expected: &WorkItem,
        id: &str,
        description: &str,
    ) -> Result<WorkItem, WorkItemError> {
        self.store.update_details(expected, id, Some(description))
    }

    /// Remove only an unchanged registration. Never touches tmux, Git, workspace
    /// files, or saved review snapshots. Those are separate lifecycle decisions.
    pub fn unregister_work_item(&self, expected: &WorkItem) -> Result<(), WorkItemError> {
        self.store.unregister(expected)
    }

    /// Resolve saved pane mappings against the latest successful discovery.
    /// `None` means discovery is unavailable, not that all panes have disappeared.
    pub fn work_item_states(
        &self,
        snapshot: Option<&Snapshot>,
    ) -> Result<Vec<WorkItemState>, WorkItemError> {
        Ok(self
            .work_items()?
            .into_iter()
            .map(|item| {
                let pane = match snapshot {
                    None => PaneAvailability::Unavailable,
                    Some(snapshot)
                        if snapshot
                            .sessions
                            .iter()
                            .flat_map(|s| &s.windows)
                            .flat_map(|w| &w.panes)
                            .any(|pane| pane.id == item.pane_id) =>
                    {
                        PaneAvailability::Present
                    }
                    Some(_) => PaneAvailability::Missing,
                };
                WorkItemState {
                    item,
                    status: AgentStatus::Unknown,
                    pane,
                    status_detail: match pane {
                        PaneAvailability::Present => "Agent has not been observed.",
                        PaneAvailability::Missing => "Mapped pane is missing.",
                        PaneAvailability::Unavailable => "Tmux discovery is unavailable.",
                    }
                    .into(),
                    attention_prompt: None,
                    completion_fingerprint: None,
                }
            })
            .collect())
    }

    /// Discover all sessions, windows, and panes on the current tmux server.
    ///
    /// Uses tmux's normal server selection (including the inherited `TMUX`
    /// environment). Missing tmux, an unreachable server, malformed metadata,
    /// and command timeouts are returned as errors rather than panics.
    pub async fn discover(&self) -> Result<Snapshot, DiscoveryError> {
        self.tmux.discover().await
    }
}
