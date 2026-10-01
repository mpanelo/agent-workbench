# Agent Workbench

M1–M2: persistent work items mapped to tmux panes, alongside a read-only terminal
view of your sessions, windows, and panes. Requires Rust 1.88+ and tmux on `PATH`
for live discovery. Registration and listing also work without tmux.

```sh
cargo run -p workbench-tui --bin workbench
```

Run in an interactive terminal, inside or outside tmux. Discovery uses tmux's
normal server selection: the inherited `TMUX` environment when present, otherwise
the default socket. It does not create sessions, switch panes, or send input.

The TUI opens in **WORK**, showing registered items with `UNKNOWN` agent status,
their type, repository, workspace, optional branch, and mapped pane ID. Pane
availability is separate from agent status: `present` means the pane was found,
`missing` means it is absent from a successful discovery, and `unavailable` means
discovery failed. A disappeared pane does not remove its registration. Press `s`
for the M1 session view and `w` to return to WORK.

The session view shows session names/IDs, window names/indices/IDs, and pane IDs, indices,
titles, current commands, and working directories. Missing command/path metadata
is shown as `unavailable`. Discovery refreshes in the background every two seconds;
errors replace the displayed snapshot and retry automatically. Each command has a
three-second timeout. Quit with `q`, `Esc`, or `Ctrl-C`; scroll with arrow keys or
Page Up/Page Down, and return to the top with Home. Resize is handled automatically.
Long metadata lines are clipped to terminal width; control characters are displayed
as escapes.

## Manual registration

Build the binary with `cargo build -p workbench-tui`, then invoke
`./target/debug/workbench` (or use `cargo run -p workbench-tui --bin workbench --`
before the arguments). For example:

```sh
./target/debug/workbench register \
  --id ABC-123 --title "Fix telemetry retry handling" \
  --kind implementation \
  --repository "$PWD" --workspace "$PWD" \
  --pane '%14' --branch fix/retries

./target/debug/workbench register \
  --id 'PR #1842' --title "Review authentication refactor" \
  --kind external-review \
  --repository /path/to/auth-service --workspace /path/to/reviews/1842 \
  --pane '%21'

./target/debug/workbench list
```

Use pane IDs from the sessions view. IDs and titles are free-form labels: no Jira
or GitHub account is involved. `--title` defaults to the ID; `--branch` is optional.
Repository/workspace paths are resolved from your current directory and saved as
absolute paths. Expand `~` with your shell. Registration does not require the paths
or pane to exist. Duplicate work-item IDs are rejected without changing the state.

State is a versioned JSON file, selected in this order:

1. `--state-file PATH` (available for the TUI, `register`, and `list`).
2. `AGENT_WORKBENCH_STATE_FILE`.
3. `$XDG_STATE_HOME/agent-workbench/work-items.json` if XDG_STATE_HOME is absolute.
4. `$HOME/.local/state/agent-workbench/work-items.json`.

Use the same state file for registration and the TUI. Parent directories are
created on first registration. Reads do not create files. The TUI reloads saved
items on each refresh, so registrations from another terminal appear automatically.
Writes use a same-directory temporary file, sync it, and atomically replace the
JSON file under an advisory lock (`work-items.json.lock`). A busy writer returns
a retryable error. Malformed or unsupported state is reported without overwriting
it; the sessions view remains usable while a state error is shown in WORK.

## Structure

- `workbench-core`: typed snapshots, work-item models and pane resolution, tmux CLI
  adapter, and JSON persistence. Tokio handles discovery; Serde/serde_json serialize
  state; tempfile/fs2 provide atomic replacement and advisory locking. It has no
  presentation dependencies.
- `workbench-tui`: Ratatui/Crossterm rendering and input, using the core engine.

Discovery uses one `list-panes -a` call and groups rows by stable session/window
IDs. Linked windows are represented in each session. Snapshot replacement removes
disappeared panes without retaining stale records. Sessions sort by name; windows
and panes sort numerically by index.

Metadata must be UTF-8. ASCII unit/record separators (`U+001F`/`U+001E`) are
reserved for the discovery format; metadata containing those rare characters is
reported as malformed. Ordinary spaces, tabs, newlines, and Unicode are preserved.
Only the selected tmux server is discovered; multi-server aggregation is not part
of M1–M2. Saved pane IDs refer to the selected server; tmux can reuse IDs after a
server restart, so check mappings after restarting tmux. There is no pane remapping
or deletion command yet. There is no agent classification, navigation to panes,
input forwarding, or Git/review functionality; those belong to later milestones.

## Checks

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Unit tests do not require tmux or a running server. An additional opt-in read-only
smoke test checks the real adapter against an existing tmux server:

```sh
cargo test -p workbench-core discovers_live_tmux -- --ignored
```
