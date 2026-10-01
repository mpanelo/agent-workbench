# Agent Workbench

M1–M3: persistent work items mapped to tmux panes, navigation and reply submission,
alongside a read-only terminal
view of your sessions, windows, and panes. Requires Rust 1.88+ and tmux on `PATH`
for live discovery. Registration and listing also work without tmux.

```sh
cargo run -p workbench-tui --bin workbench
```

Run in an interactive terminal, inside or outside tmux. Discovery uses tmux's
normal server selection: the inherited `TMUX` environment when present, otherwise
the default socket. Discovery is read-only. Only explicit open/reply actions
switch to panes or submit input; no sessions or workspaces are created.

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
three-second timeout. Quit with `q`, `Esc`, or `Ctrl-C` when not composing a reply.
In WORK, `j/k` or arrows move selection. Page Up/Page Down scroll details; Home
selects the first item. The sessions view still uses arrows to scroll. Resize is handled automatically.
Long metadata lines are clipped to terminal width; control characters are displayed
as escapes.

## Navigation and replies

In WORK, the selected item is highlighted with `>`:

| Key | Action |
| --- | --- |
| `j` / `↓`, `k` / `↑` | Select the next/previous work item. |
| `Enter` | Open/focus the selected item's mapped pane. |
| `r` | Compose a reply to the selected item. |
| `Tab` | Select the next item known to need attention; currently reports none because all statuses are `UNKNOWN`. |
| `s`, `w` | Switch to sessions or work items. |

Opening from inside tmux switches its current client to the target session/window/
pane; Workbench stays running in its original pane. Use your tmux navigation keys
to return. Opening from outside tmux temporarily attaches this terminal to the
target; detach with your tmux binding (normally `Ctrl-b d`) to return to Workbench.
Workbench restores terminal mode before opening and reinitializes it afterward.
It never detaches other clients. tmux resolves the current client when more than
one is attached; client selection is not configurable yet.

In the reply editor, type or paste a single line, use Backspace to edit or `Ctrl-u`
to clear it, and press `Enter` to submit. `Esc`/`Ctrl-C` cancels before submission.
Replies are limited to 4096 UTF-8 bytes; multiline and control-character pastes are
rejected without changing the draft. The editor captures the work-item ID, so a
refresh or selection change cannot redirect the draft to another item. Replies
are temporary and are not persisted.

The core reloads the registration and rediscovers the pane before every action.
Text is encoded as literal UTF-8 bytes using `send-keys -H`, followed by one Enter;
key names, tmux formats, flags, and semicolons in the reply remain literal text.
Reply submission does not switch panes. Sending occurs in the background and is
never retried automatically. A failed send retains the draft: inspect the target
before manually resending, because a command can fail after partial delivery.
Once submission starts, cancellation is unavailable until it completes. Missing
panes and command failures are shown in WORK without deleting the registration.
Input is sent to whatever program is running in the mapped pane; agent detection
is not implemented yet. Exit tmux copy mode before sending a reply.

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
of M1–M3. Saved pane IDs refer to the selected server; tmux can reuse IDs after a
server restart, so check mappings after restarting tmux. There is no pane remapping
or deletion command yet. There is no agent classification or Git/review
functionality; those belong to later milestones.

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

An opt-in action test creates its own isolated tmux server, delivers literal
Unicode input to a `cat` pane, checks disappearing-pane errors, and cleans up only
that server:

```sh
cargo test -p workbench-core isolated_tmux_input_is_literal_and_missing_panes_are_safe -- --ignored
```
