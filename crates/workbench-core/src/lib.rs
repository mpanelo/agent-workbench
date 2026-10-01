//! Presentation-independent tmux discovery for Agent Workbench.

mod model;
mod tmux;

pub use model::{Pane, Session, Snapshot, Window};
pub use tmux::DiscoveryError;

/// The M1 engine exposes read-only discovery, without shell commands in its API.
#[derive(Debug, Default)]
pub struct Engine;

impl Engine {
    /// Discover all sessions, windows, and panes on the current tmux server.
    ///
    /// Uses tmux's normal server selection (including the inherited `TMUX`
    /// environment). Missing tmux, an unreachable server, malformed metadata,
    /// and command timeouts are returned as errors rather than panics.
    pub async fn discover(&self) -> Result<Snapshot, DiscoveryError> {
        tmux::discover().await
    }
}
