# Agent Workbench

Agent Workbench is a local-first developer tool for supervising multiple coding agents working in parallel and reviewing the code they produce.

The first interface is a terminal UI. A native macOS application may be added later.

## Product Goal

The core problem is that developers can run several coding agents concurrently, but supervising them becomes cumbersome.

The product should make it easy to:

- see all active work items and agent sessions
- identify which agents need human attention
- jump to an agent quickly
- respond to an agent without manually navigating tmux
- review agent-generated Git diffs
- track which code has already been reviewed
- identify code that changed after it was reviewed

The product should complement existing tools such as tmux, workmux, Git, editors, and coding agents rather than replace them.

## Architecture

Use a Rust workspace.

The initial crates should be:

```text
crates/
  workbench-core/
  workbench-tui/
```

The dependency direction must always be:

```text
workbench-tui
      ↓
workbench-core
```

`workbench-core` must never depend on:

- Ratatui
- Crossterm
- Swift
- SwiftUI
- AppKit
- terminal rendering code
- other presentation-specific frameworks

The long-term architecture is:

```text
                ┌──────────────┐
                │ Ratatui TUI  │
                └──────┬───────┘
                       │
                ┌──────▼───────┐
                │ Rust Engine  │
                └──────┬───────┘
                       │
                ┌──────▼───────┐
                │ SwiftUI App  │
                └──────────────┘
```

The same engine should eventually support both interfaces.

## Technology

Use:

- Rust
- Tokio
- Ratatui
- Crossterm

Prefer the Rust standard library where reasonable.

Do not introduce heavy dependencies without a clear need.

## Core Responsibilities

`workbench-core` owns:

- domain models
- work item lifecycle
- agent/session state
- tmux interaction
- Git interaction
- review state
- application events
- persistence abstractions
- workspace discovery
- business logic

The core should expose high-level operations rather than leaking shell commands or tmux implementation details.

Prefer APIs conceptually similar to:

```rust
engine.work_items()
engine.attention_items()
engine.send_agent_input(...)
engine.diff(...)
engine.mark_reviewed(...)
```

The exact API may evolve.

## Domain Model

The initial domain concepts include:

### WorkItem

Represents a unit of developer work.

A work item may originate from:

- an implementation task
- an external pull request being reviewed

Potential fields:

```rust
struct WorkItem {
    id: WorkItemId,
    title: String,
    repository: PathBuf,
    workspace: PathBuf,
    branch: Option<String>,
    kind: WorkItemKind,
}
```

Do not couple work items directly to Jira or GitHub.

Those integrations may be added later.

### WorkItemKind

Conceptually:

```rust
enum WorkItemKind {
    Implementation,
    ExternalReview,
}
```

### Agent

Represents a coding-agent session associated with a work item.

Possible initial fields:

```rust
struct Agent {
    id: AgentId,
    work_item_id: WorkItemId,
    transport: AgentTransportId,
}
```

Do not assume only one coding-agent vendor.

### AgentStatus

Initial states:

```rust
enum AgentStatus {
    Running,
    WaitingForInput,
    Idle,
    Complete,
    Unknown,
}
```

Perfect status detection is not required.

Unknown state must be supported gracefully.

### EngineEvent

The engine should eventually expose changes as events.

Conceptually:

```rust
enum EngineEvent {
    WorkItemAdded(...),
    WorkItemUpdated(...),
    AgentStatusChanged(...),
    AgentNeedsInput(...),
    DiffChanged(...),
    ReviewInvalidated(...),
}
```

Do not over-engineer the event system during the first milestone.

## Adapters

External systems should sit behind clear boundaries.

Examples:

- tmux
- Git
- local filesystem
- persistence

Avoid putting direct `Command::new("tmux")` calls in UI code.

Prefer an abstraction such as:

```rust
trait AgentTransport {
    fn send_input(&self, agent: AgentId, input: &str) -> Result<()>;
}
```

The exact trait design is flexible.

The important requirement is separation between infrastructure and presentation.

## Tmux

For v0, tmux is the primary agent transport.

The core should be able to discover:

- sessions
- windows
- panes
- pane IDs
- pane titles
- pane working directories where available
- current pane commands where available

Prefer invoking tmux through its command-line interface.

Do not build a terminal emulator.

Do not replace tmux.

## Git

Git functionality will eventually include:

- changed files
- local diffs
- base revisions
- review snapshots
- detecting changes after review

Prefer using Git as the source of truth.

Initial versions may invoke the `git` CLI rather than embedding a Git implementation.

## Review Model

Review is a first-class feature.

Eventually the engine should track:

- files reviewed
- hunks reviewed
- review snapshot/revision
- files modified after review
- changes requiring re-review

The key product behavior is:

> Once a developer reviews agent-generated code, later agent edits should not force them to rediscover the entire diff.

This is not required in the first milestone.

## TUI

The TUI should be keyboard-first.

Long-term primary views:

```text
WORK
ATTENTION
REVIEW
```

Avoid putting business logic into widgets.

Rendering code should receive state from the core and emit user intents/actions back to the core.

## Error Handling

Do not panic for normal operational failures.

Examples:

- tmux not running
- pane disappearing
- repository removed
- Git command failure
- unknown agent state

Return useful errors and allow the UI to continue operating where possible.

## Testing

Prefer unit tests for:

- parsers
- state transitions
- domain logic
- command-output interpretation
- review calculations

External command invocation should be structured so parsing and business logic can be tested independently of actually running tmux or Git.

## Code Quality

Before completing a task, run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

If a command cannot be run successfully, explain why.

## Working Style

When given a task:

1. Inspect the existing repository first.
2. Preserve the architecture described here.
3. Propose major architectural changes before implementing them.
4. Keep changes narrowly scoped to the requested milestone.
5. Avoid speculative abstractions for features that do not yet exist.
6. Prefer simple implementations that can evolve.
7. Add tests for meaningful core behavior.
8. Run formatting, linting, and tests before finishing.
9. Summarize:
   - what changed
   - architectural decisions
   - tests performed
   - remaining limitations

## Explicitly Out of Scope for v0

Do not implement unless specifically requested:

- Jira API integration
- GitHub API integration
- GitLab integration
- automatic PR creation
- automatic Jira creation
- cloud accounts
- cloud sync
- telemetry backend
- subscriptions
- LLM API calls
- AI code review
- remote agent hosting
- workmux replacement
- tmux replacement
- terminal emulator
- custom editor
- automatic worktree creation
- native macOS frontend
- Swift bindings
- UniFFI
- mobile applications

## Current Priority

Build the smallest useful vertical slice first:

```text
tmux
  ↓
Rust core discovers sessions/windows/panes
  ↓
TUI displays them
```

Once that works reliably, proceed to work-item registration and agent supervision.
