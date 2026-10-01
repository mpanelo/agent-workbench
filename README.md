# Agent Workbench

M1–M5: persistent work items mapped to tmux panes, navigation, reply submission,
conservative agent-state detection, and a dedicated attention queue, alongside a read-only terminal
view of your sessions, windows, and panes. Requires Rust 1.88+ and tmux on `PATH`
for live discovery. Registration and listing also work without tmux.

```sh
cargo run -p workbench-tui --bin workbench
```

Run in an interactive terminal, inside or outside tmux. Discovery uses tmux's
normal server selection: the inherited `TMUX` environment when present, otherwise
the default socket. Discovery is read-only. Only explicit open/reply actions
switch to panes or submit input; no sessions or workspaces are created.

The TUI opens in **ATTENTION**, showing items currently known to need input or
whose agent turn completed. Press `a` to return there, `w` for **WORK**, or `s`
for the session view. WORK shows all registered items with observed agent status,
their type, repository, workspace, optional branch, and mapped pane ID. Pane
availability is separate from agent status: `present` means the pane was found,
`missing` means it is absent from a successful discovery, and `unavailable` means
discovery failed. A disappeared pane does not remove its registration.

The session view shows session names/IDs, window names/indices/IDs, and pane IDs, indices,
titles, current commands, and working directories. Missing command/path metadata
is shown as `unavailable`. Discovery refreshes in the background every two seconds;
errors replace the displayed snapshot and retry automatically. Each command has a
three-second timeout. Quit with `q`, `Esc`, or `Ctrl-C` when not composing a reply.
In ATTENTION and WORK, `j/k` or arrows move selection. `Ctrl+d`/`Ctrl+u` scroll
down/up half a page in all views; Page Down/Page Up still scroll a full page. Home
selects the first item. The sessions view still uses arrows to scroll. Resize is handled automatically.
Long metadata lines are clipped to terminal width; control characters are displayed
as escapes. Repository, workspace, and session working-directory paths under the
current `$HOME` are displayed with `~` (also in `workbench list`). Stored paths and
approval command text are unchanged; paths outside `$HOME` remain absolute.

## Navigation and replies

In ATTENTION and WORK, the selected item is highlighted with `>`:

| Key | Action |
| --- | --- |
| `j` / `↓`, `k` / `↑` | Select the next/previous work item. |
| `Enter` | Open/focus the selected item's mapped pane. |
| `r` | Compose a reply to the selected item. |
| `Tab` | Select the next `WAITING_FOR_INPUT` or `COMPLETE` item, wrapping around. |
| `a`, `w`, `s` | Switch to attention, all work items, or sessions. |
| `Ctrl+d`, `Ctrl+u` | Scroll down/up half a page without changing selection (outside the reply editor). |
| Page Down, Page Up | Scroll down/up a full page. |
| `d` | Explain that built-in diff review is deferred to M6; no Git operation is performed yet. |

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
Input is sent to whatever program is running in the mapped pane; classification
does not restrict open/reply actions. Exit tmux copy mode before sending a reply.

## Attention queue (M5)

ATTENTION is the primary supervision view. Its header displays the current queue
count. It contains only live, `present` panes with `WAITING_FOR_INPUT` or `COMPLETE`
status, in registration order. Running, idle, and unknown items remain in WORK;
unknowns are explicitly reported as unclassified, not assumed idle. WORK's header
also shows the attention count. Store/discovery errors and empty queues are visible.

Waiting cards show the full visible approval question, environment, reason, and
command, when available. Only the current dialog is retained, excluding prior
conversation and approval choices. Blank lines and relative command indentation
are preserved; reasons and commands are not cut off after a fixed number of lines.
Dialog text is bounded at 16,384 Unicode characters, with an explicit warning if
that limit is exceeded. Text wraps to terminal width; use Page Up/Page Down to
scroll long dialogs, or `Enter` for the full pane. Content already clipped by the
agent's own screen cannot be recovered from a viewport capture. Completion cards say the **turn**
finished, without claiming the whole task is done or inventing changed-file counts.

The core builds the queue from the same observation snapshot used by WORK, with no
extra tmux captures. Queue entries and prompt previews are not persisted or logged.
Refreshes remove entries when an agent resumes, becomes unknown, or its pane
disappears. Selection follows work-item IDs, falling back to the first visible
entry when the selected item leaves. An existing reply draft stays bound to its
original item even if the queue changes. Empty queues cannot open or reply to hidden
WORK items. Switching views or returning from a focused pane preserves the chosen
view. Completed turns remain queued while their visible completion marker remains;
there is no dismiss/acknowledge action yet.

M5 does not implement Git diffs, changed-file counts, workspace-change detection,
or review state. The `d` key explains the M6 boundary; `Enter` still opens the full
terminal session for manual inspection. Unknown agents retain M4's limitations.

## Basic agent state (M4)

Detection is automatic in ATTENTION, WORK, and `workbench list`; no re-registration, hooks,
API keys, or agent configuration is needed. The first supported agent is the
interactive **Codex CLI**, when tmux reports the foreground command as `codex`
(or a path ending in `/codex`). Other agents, shells, and wrappers such as `node`
stay `UNKNOWN` and remain fully navigable/replyable.

| State | Evidence |
| --- | --- |
| `RUNNING` | Current Codex activity line with elapsed time and `esc to interrupt`. |
| `WAITING_FOR_INPUT` | Current command/edit/permission/terminal-input approval dialog: known title, selected Yes/No option, and confirmation footer. |
| `IDLE` | Bottom-of-screen Codex composer and shortcuts footer, without an active indicator or explicit completion marker. |
| `COMPLETE` | Ready composer immediately following Codex's `Worked for …` marker. **The turn finished; the overall task may not be done.** |
| `UNKNOWN` | Unsupported agent, missing/unavailable/dead pane, copy/view mode, failed capture, unrecognized menu, or inconclusive/truncated UI. |

The work list shows a short explanation beside each status. Detection reads only
the current visible viewport of registered supported-agent panes, not scrollback,
session transcripts, or unregistered panes. Captures include foreground/dead/mode
metadata before and after the screen; inconsistent observations are discarded.
Linked panes or multiple registrations sharing a pane are captured once per refresh.
At most four captures run concurrently, each with the same three-second timeout.
Observations are replaced on every refresh, not persisted; failures never retain
a stale waiting/completed status or remove registrations. `list` falls back to
`UNKNOWN` with unavailable panes when tmux cannot be reached.

These are terminal heuristics, not an agent protocol. They recognize observed
English Codex UI patterns (including legacy context footers and current model/path
footers); changed keybindings, localization, narrow panes, menus, and UI changes
can produce `UNKNOWN`. Free-form questions and structured question pickers are
not reliably distinguished yet—open the full pane when uncertain. `IDLE` means a
ready composer, not proof that no human response is desired. A visible completion
marker remains `COMPLETE` until the screen changes; there is no acknowledgement
action yet. No state is inferred solely from process
presence, pane disappearance, a quiet terminal, or words such as “done” in prose.

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

- `workbench-core`: typed snapshots, work-item models, pane resolution, pure agent
  status interpretation, ephemeral attention queue and prompt previews, read-only observations, tmux CLI
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
of M1–M5. Saved pane IDs refer to the selected server; tmux can reuse IDs after a
server restart, so check mappings after restarting tmux. There is no pane remapping
or deletion command yet. Git/review functionality belongs to later milestones.

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

The M4 opt-in transport test replays Codex UI fixtures with an inert process on
its own isolated server, checking all recognized statuses, copy mode, and a
disappearing pane. It does not start a real agent or call an API:

```sh
cargo test -p workbench-core isolated_tmux_observations_handle_copy_mode_and_disappearance -- --ignored
```
