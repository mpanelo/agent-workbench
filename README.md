# Agent Workbench

M1–M6: persistent work items mapped to tmux panes, navigation, reply submission,
conservative agent-state detection, an attention queue, and local Git diff review, alongside a read-only terminal
view of your sessions, windows, and panes. Requires Rust 1.88+ and tmux on `PATH`
for live discovery. Registration and listing also work without tmux.

## Installation

Install [Rust and Cargo](https://www.rust-lang.org/tools/install) (Rust 1.88 or
newer), Git, and tmux. Rust/Cargo and Git are needed to build from source; tmux
must be on `PATH` for live discovery, pane navigation, and replies.

Install the latest `main` branch from GitHub:

```sh
cargo install --locked --git https://github.com/mpanelo/agent-workbench.git \
  --branch main workbench-tui
```

The package is called `workbench-tui`, but the installed executable is
`workbench`. With Cargo's default installation directory, it is placed in
`~/.cargo/bin`. If that directory is not on `PATH`, add it in your shell:

```sh
# Bash/zsh: add this line to your shell configuration for future sessions.
export PATH="$HOME/.cargo/bin:$PATH"
```

For fish, run:

```fish
fish_add_path ~/.cargo/bin
```

If you use a custom `CARGO_HOME` or installation root, add its `bin` directory
instead. See [Cargo's installation documentation](https://doc.rust-lang.org/cargo/commands/cargo-install.html).

Verify the installation and launch the TUI:

```sh
workbench --help
workbench
```

To update, rerun the GitHub installation command. From an existing checkout, you
can instead install with `cargo install --locked --path crates/workbench-tui`.
Installation currently builds from source; no prebuilt binaries or `curl | bash`
installer are provided.

## Run from a checkout

For development, run from the repository root without installing:

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
| `d` | Review the selected item's local Git diff, even if its pane is missing or its agent is unknown. |

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

The attention queue itself does not poll Git or infer workspace changes. Use `d`
to open M6's on-demand review screen; `Enter` still opens the full terminal session.
Unknown agents retain M4's limitations and can be reviewed from WORK.

## Local Git diff review (M6)

Select an item in ATTENTION or WORK and press `d`. REVIEW lists changed files,
shows a colored unified diff for the selected file, totals text additions/deletions,
and displays file-level review progress. Git operations run in the background;
`Esc` cancels loading or returns to the previous view. No live tmux pane is required.

Review uses the registered **workspace**, not the `repository` label or tmux's
current directory. The workspace must be the root of a Git checkout or linked
worktree, with a valid base commit and no unresolved merge conflicts. Missing
workspaces, unavailable Git, invalid refs, and capture failures show useful errors
without deleting work items or showing an old diff as a successful new capture.

By default, the baseline is `HEAD`. The screen combines staged and unstaged
tracked changes against that commit, plus non-ignored untracked files. Changes
already committed to HEAD are not shown by default. To review committed branch
changes too, choose a local base when launching:

```sh
workbench --diff-base origin/main
# Or from a checkout:
cargo run -p workbench-tui --bin workbench -- --diff-base main
```

The chosen ref is resolved to a commit in each selected workspace. Comparison is
directly from that commit to the working tree, **not** an inferred merge-base or
three-dot branch diff. No refs are fetched and the saved work-item `branch` is not
used as a baseline. For an exact branch-point review, supply the merge-base commit
SHA yourself. The screen shows the chosen base and its resolved revision.

| Review key | Action |
| --- | --- |
| `j` / `↓`, `k` / `↑` | Select the next/previous changed file. |
| Space | Toggle the selected file reviewed/unreviewed. |
| `Tab` | Select the next unreviewed file, wrapping around. |
| `Ctrl+d`, `Ctrl+u` | Scroll the diff down/up half a page. |
| Page Down, Page Up | Scroll the diff down/up a page. |
| `h` / `←`, `l` / `→` | Pan long diff lines horizontally. |
| Home | Reset vertical and horizontal diff scrolling. |
| `r` | Reload the diff and clear every review mark for that item. |
| `Esc` | Return to the originating ATTENTION/WORK view. |
| `q` / `Ctrl-C` | Quit Workbench. |

Review marks and diff text are **in memory only**. Reopening an item during the
same application session retains its captured diff and progress, including after
tmux navigation. `r` discards the old capture/marks before loading again. Agent
edits are not automatically refreshed or detected as changed-after-review; use
`r` to inspect new changes. Persistent review state and re-review tracking remain
M7/M8, not part of M6. Captures are bounded, on-demand Git reads, not an atomic
filesystem snapshot; reload if the agent is actively editing during capture.

Added, modified, deleted, renamed, type-changed, untracked, and binary files are
listed. Binary content requires an external viewer and is excluded from line
totals. Submodules show only their Gitlink/pointer diff, not nested file changes.
UTF-8 paths and text are supported; terminal control characters are escaped.
Symlink diffs show link targets, not the linked file's contents. Review is read-only:
it never stages, commits, applies patches, fetches, or changes branches. External
diff/textconv helpers and filesystem-monitor hooks are disabled for these reads.

To keep large repositories responsive, capture is limited to 512 changed files,
2 MiB of stdout per command, 16 MiB of total patches, 60,000 patch lines per file,
and 8 KiB per line. Commands time out after five seconds, with a 30-second overall
capture limit. Oversized/undecodable diffs fail explicitly rather than silently
loading a partial review; use an external viewer in those cases. No AI review,
GitHub integration, inline PR comments, or review-state persistence is included.

## Basic agent state (M4)

Detection is automatic in ATTENTION, WORK, and `workbench list`; no re-registration, hooks,
API keys, or agent configuration is needed. The first supported agent is the
interactive **Codex CLI**, when tmux reports the foreground command as `codex`
(or a path ending in `/codex`). Other agents, shells, and wrappers such as `node`
stay `UNKNOWN` and remain fully navigable/replyable.

| State | Evidence |
| --- | --- |
| `RUNNING` | Current Codex activity line with elapsed time and `esc to interrupt`, anchored by a composer/shortcuts footer or a recognized bottom rate-limit banner. |
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
footers, with or without activity bullets). Known `5h`/weekly rate-limit banners
and the `⚠ … warnings · f2 to view` shortcuts suffix are supported. A bare activity
line, historical indicator followed by new output, or unfamiliar footer is not
enough evidence; changed keybindings, localization, narrow panes, menus, and UI changes
can produce `UNKNOWN`. Free-form questions and structured question pickers are
not reliably distinguished yet—open the full pane when uncertain. `IDLE` means a
ready composer, not proof that no human response is desired. A visible completion
marker remains `COMPLETE` until the screen changes; there is no acknowledgement
action yet. No state is inferred solely from process
presence, pane disappearance, a quiet terminal, or words such as “done” in prose.

Codex lifecycle hooks are a potential opt-in status source, not installed or
consumed by Workbench yet. See [the hook investigation](docs/codex-status-signals.md)
for the proposed approach and limitations. Terminal detection remains the default;
no Codex configuration is changed automatically.

## Manual registration

If installed, use `workbench register` and `workbench list`. From a checkout,
build the binary with `cargo build -p workbench-tui`, then invoke
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
  status interpretation, ephemeral attention queue and prompt previews, local Git
  diff capture and in-memory review progress, read-only observations, tmux CLI
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
of M1–M6. Saved pane IDs refer to the selected server; tmux can reuse IDs after a
server restart, so check mappings after restarting tmux. There is no pane remapping
or deletion command yet. Review persistence and change tracking belong to M7/M8.

## Checks

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Unit tests do not require tmux or a running server. Git must be installed: review
tests use isolated temporary repositories and synthetic commits, not your signing
keys, index, or working files. An additional opt-in read-only smoke test checks
the real adapter against an existing tmux server:

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
