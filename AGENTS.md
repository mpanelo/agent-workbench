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

The practical benchmark is supervising four to six concurrent workspaces without
remembering tmux window numbers or rediscovering which code was already reviewed.

## Architecture

Use a Rust workspace.

The workspace contains:

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

Do not add legacy or backward-compatibility aliases for commands, flags,
keybindings, or renamed APIs. Use the current names only; do not retain
superseded names as aliases. This does not authorize discarding existing saved
data or changing its schema without a migration.

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

The core should expose high-level operations rather than leaking shell commands
or tmux implementation details. Extend existing APIs before introducing new
abstractions for planned features.

## Domain Model

`WorkItem` represents a unit of developer work, originating from:

- an implementation task
- an external pull request being reviewed

Keep work items independent of Jira, GitHub, and any one coding-agent vendor.
Keep domain identity separate from transport details.

Support unknown agent state gracefully. Perfect status detection is not required.
Use the core's models as the source of truth for field names and state variants.
Keep event handling simple and driven by current needs.

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

Tmux is the primary agent transport.

Discovery belongs in the core and includes:

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

Git functionality includes:

- changed files
- local diffs
- base revisions
- review snapshots
- detecting changes after review

Prefer using Git as the source of truth.

Prefer the existing Git CLI adapter over introducing an embedded Git implementation.

## Review Model

Review is a first-class feature.

Preserve the engine's review tracking:

- files reviewed
- review snapshot/revision
- files modified after review
- changes requiring re-review

The key product behavior is:

> Once a developer reviews agent-generated code, later agent edits should not force them to rediscover the entire diff.

## TUI

The TUI should be keyboard-first.

Ask the user before adding any new keybindings, including alternative bindings.
Scope keybindings to the current view or input mode. Help must list only bindings
applicable to that context; a key may have different meanings in different views.

Keep help and footers lean. Omit common navigation keys such as `j/k` and arrows
without disabling them. Do not list the help-opening shortcut inside help.
Avoid duplicate explanations across dialogs, footers, and success messages.

Primary views:

```text
WORK
SESSIONS
REVIEW
```

Avoid putting business logic into widgets.

WORK combines supervision and attention. Keep its compact list in registration
order, with details in a separate viewport. Status/prompt changes must not move
the selected item or its list row. Acknowledging attention keeps the item visible.

Rendering code should receive state from the core and emit user intents/actions back to the core.

## Terminal Safety

Treat agent output, filenames, and diffs as untrusted display content. Escape
terminal control characters so they cannot execute terminal commands or distort
the layout. Render ordinary whitespace naturally in code diffs. Keep stored
patch bytes unchanged; display formatting must not alter review tracking.

## Cleanup Safety

Require user confirmation before closing agent panes or windows or removing
worktrees. Recheck the exact target and ownership immediately before acting.
Never discard dirty files implicitly. If the target changed or ownership is
uncertain, stop and explain the problem instead of forcing cleanup.

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

Add a regression test for bug fixes. For rendering changes, cover narrow
terminals, Unicode, and control characters where relevant.

## Code Quality

Use Rust's `std` and `core` code and tests as style references: clear APIs,
straightforward control flow, small focused helpers, and direct assertions.

Follow the [Rust Style Guide](https://doc.rust-lang.org/style-guide/) and
[Rust API Guidelines](https://rust-lang.github.io/api-guidelines/). Use the
repository's pinned Rust toolchain and stable rustfmt defaults. Keep explicit
2024 parsing and style editions in `rustfmt.toml`.

Use Clippy's default lint groups with warnings treated as errors. Do not enable
the entire `pedantic`, `nursery`, or `restriction` groups; review individual lint
additions for usefulness instead.

Use cargo-deny 0.20.2 to check dependency advisories, licenses, and sources with
`deny.toml`. Report findings rather than adding advisory ignores, license
exceptions, or source allowances without the user's approval. Duplicate
dependency versions are warnings, not automatic failures.

Keep function and test names short, descriptive, and in `snake_case`. Let the
module or type provide context instead of repeating it in every name. Name tests
for the operation and relevant case, not a sentence listing every assertion.
Put detailed explanations in the test body or comments. Do not replace long
names with cryptic abbreviations or add compatibility aliases when renaming.

Before completing a task, run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny --locked check
```

If a command cannot be run successfully, explain why.

## Working Style

When given a task:

1. Inspect the existing repository first.
2. Preserve the architecture described here.
3. Propose major architectural changes before implementing them.
4. Keep changes narrowly scoped to the requested task.
5. Avoid speculative abstractions for features that do not yet exist.
6. Prefer simple implementations that can evolve.
7. Add tests for meaningful core behavior.
8. Run formatting, linting, and tests before finishing.
9. Summarize:
   - what changed
   - architectural decisions
   - tests performed
   - remaining limitations

Use concise, plain language in documentation and UI text. Do not use em dashes.

## Commit Messages

Every commit must use this format:

```text
<Capitalized imperative verb> <specific change>

<explanation of what changed and why>
```

Start the subject with a capitalized imperative verb and describe the specific
change. Separate the subject from the body with a blank line. The body must
explain what changed and why.

Sign every commit with `git commit -S`. Never fall back to an unsigned commit.
Push only with explicit user approval. Push authorization is limited to the
current session and must not carry over to future sessions. Report signing or
push failures rather than bypassing them.

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

## Documentation

Keep current usage in the [README](README.md), with focused supporting guides:

- [Codex hook setup and troubleshooting](docs/codex-hooks.md)
- [Status detection internals and testing](docs/status-detection.md)
- [Open roadmap proposals](docs/roadmap.md)

Keep shipped behavior out of the backlog. Roadmap proposals provide context,
not authorization to begin unrequested work. Historical plans remain in Git.
