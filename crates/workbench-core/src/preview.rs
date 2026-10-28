//! Bounded, read-only pane captures. No transcripts or preview persistence.
use std::{error::Error, fmt};

use crate::{DiscoveryError, WorkItemError};

mod control;

const MAX_CAPTURE: usize = 256 * 1024;
const FORMAT: &str = concat!(
    "\x1e#{pid}:#{start_time}\x1f#{socket_path}\x1f#{pane_id}\x1f",
    "#{pane_pid}\x1f#{pane_dead}\x1f#{pane_in_mode}\x1f",
    "#{pane_width}\x1f#{pane_height}\x1e"
);

/// Recent terminal output, not a complete agent response. ANSI styles are data
/// for the presentation layer to decode, never bytes to write to a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanePreview {
    pub text: String,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug)]
pub enum PreviewError {
    State(WorkItemError),
    Transport(DiscoveryError),
    Changed,
}

impl fmt::Display for PreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(error) => error.fmt(f),
            Self::Transport(error) => error.fmt(f),
            Self::Changed => f.write_str("Work item changed or was removed; preview discarded."),
        }
    }
}

impl Error for PreviewError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::State(error) => Some(error),
            Self::Transport(error) => Some(error),
            Self::Changed => None,
        }
    }
}

fn invalid(reason: impl Into<String>) -> DiscoveryError {
    DiscoveryError::InvalidOutput {
        record: 0,
        reason: reason.into(),
    }
}

fn parse_capture(bytes: &[u8], pane: &str) -> Result<PanePreview, DiscoveryError> {
    if bytes.len() > MAX_CAPTURE {
        return Err(invalid("Pane preview exceeded its capture limit."));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("Preview is not UTF-8."))?;
    let fields: Vec<_> = text.split('\x1e').collect();
    let ["", before, screen, after, "\n"] = fields.as_slice() else {
        return Err(invalid("Incomplete pane preview."));
    };
    if before != after {
        return Err(invalid(
            "Pane identity or size changed during preview capture.",
        ));
    }
    let fields: Vec<_> = before.split('\x1f').collect();
    let [
        server,
        socket,
        id,
        pid,
        dead @ ("0" | "1"),
        mode @ ("0" | "1"),
        width,
        height,
    ] = fields.as_slice()
    else {
        return Err(invalid("Invalid pane preview metadata."));
    };
    if id != &pane
        || server.is_empty()
        || socket.is_empty()
        || pid.parse::<u32>().ok().filter(|pid| *pid > 0).is_none()
    {
        return Err(invalid("Invalid pane preview identity."));
    }
    if *dead == "1" {
        return Err(invalid("Agent pane has exited; preview unavailable."));
    }
    if *mode == "1" {
        return Err(invalid("Pane is in tmux copy mode; preview paused."));
    }
    let size = |value: &str| {
        value
            .parse::<u16>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid("Invalid pane preview dimensions."))
    };
    Ok(PanePreview {
        text: screen.strip_prefix('\n').unwrap_or(screen).to_string(),
        width: size(width)?,
        height: size(height)?,
    })
}

#[cfg(test)]
mod tests;
