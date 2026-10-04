# Agent Workbench

A local-first terminal dashboard for supervising coding agents in tmux and
reviewing their code. Keep track of parallel work, see who needs your attention,
and jump between agents without hunting for panes.

Workbench complements tmux, Git, and workmux. It doesn't start agents or create
worktrees, and it requires no cloud account or API key.

## Features

- Track work items linked to agent panes.
- See input requests and finished turns in an attention queue.
- Open an agent's pane or send a reply from Workbench.
- Review Git diffs, save file-level review marks, and see what changed since review.
- Clean up a workmux window and worktree after confirmation.

## Installation

You need Rust 1.88+ and Cargo, Git, and tmux on your `PATH`.
Installation builds from source; prebuilt binaries aren't available yet.

```sh
cargo install --locked --git https://github.com/mpanelo/agent-workbench.git \
  --branch main workbench-tui
```

The executable is called `workbench`. Make sure `~/.cargo/bin` is on your `PATH`.
To update, rerun the installation command.

```sh
workbench
```

A truecolor terminal and Nerd Font v3+ display the Catppuccin Mocha colors and
session icons. Workmux is optional unless you want workspace cleanup.

## Quick start

1. Start your coding agents in tmux panes as usual.
2. Run `workbench`, preferably in its own tmux window.
3. Press `s` for SESSIONS, select an agent, and press `r` or `<enter>` to register it.
   Workbench fills in its pane, repository, worktree, and branch when available.
4. Confirm the Work ID and Short Description, then press `<enter>` to save.
5. Use `a` for ATTENTION or `w` for WORK to supervise your registered agents.

A Work ID is your unique task label, such as `fix-auth` or `PR-42`.
Short Description is a readable summary, limited to 120 characters. Work items
can represent implementation work or code review.

SESSIONS shows recognized coding-agent commands by default. Press `f` to show all
panes if your agent runs through a wrapper or isn't recognized.

## Keyboard shortcuts

Press `?` for the full bindings in the current view. In text inputs, `?` types a
literal question mark. `<c-d>` means Control+D; `<enter>` means the Enter key.

| Action | Key |
| --- | --- |
| Attention / work / sessions | `a` / `w` / `s` |
| Select an item | `j` / `k` or `↓` / `↑` |
| Open the selected agent pane | `<enter>` in WORK or ATTENTION |
| Reply to an agent | `r` in WORK or ATTENTION; `<enter>` sends |
| Review the selected item's diff | `d` in WORK or ATTENTION |
| Edit ID and description | `e` in WORK |
| Unregister an item | `u` in WORK |
| Clean up a workmux workspace | `c` in WORK |
| Acknowledge a finished turn | `x` in ATTENTION |
| Register a pane | `r` or `<enter>` in SESSIONS |
| Toggle agent-only / all panes | `f` in SESSIONS |
| Scroll half a page | `<c-d>` / `<c-u>` outside text inputs |
| Quit | `q` outside text inputs |

Forms support arrow-key cursor movement, Tab to switch fields, and `<c-u>` to
clear a field. `Esc` cancels an edit or reply before saving or sending.
Replies are single-line messages. If sending fails, inspect the agent pane before
retrying: some input may already have arrived.

Opening a pane leaves Workbench running. Use your tmux navigation to return; when
running outside tmux, detach from the opened session to return to Workbench.

## Reviewing code

Press `d` on a work item to open its Git diff. In REVIEW:

- `Space` marks the selected file reviewed or unreviewed.
- `Tab` selects the next file needing review.
- `r` reloads the diff and flags files changed after review.
- `c` switches between the full diff and changes since your saved review.
- `h` / `l` or Left / Right pans long lines; `Esc` returns to the previous view.

Review marks persist across restarts. Review is file-level, not hunk-level, and
diffs refresh only when you reload. Git review never stages, commits, or changes
your files. Binary content needs an external viewer.

By default, review compares your working tree against `HEAD`, including staged,
unstaged, and non-ignored untracked files. To include committed branch changes,
choose a local base:

```sh
workbench --diff-base origin/main
```

This compares directly against that revision, not an inferred merge-base, and
doesn't fetch remote refs. Changing the resolved base starts a separate review
context.

## Workspace cleanup

Unregistering with `u` removes only the Workbench entry; it leaves your panes,
files, branch, and review history intact.

Cleanup with `c` removes a linked worktree and closes its **entire workmux window**,
including companion panes and their running programs. The branch and review
history are kept. Nothing is merged or pushed.

The preview includes ignored files that will be deleted, such as `.env` and build
caches. **Cancel is selected by default.** Use Tab or Left / Right to select
cleanup, then `<enter>` to confirm. Stop ongoing agent work first.

Cleanup requires workmux with `list --json` and window ownership metadata. Only
one live, managed window-mode worktree is supported. Dirty, locked, shared,
ambiguous, or unsupported workspaces are blocked, as are the main checkout and
Workbench's own window or workspace. Force deletion isn't supported.

Workmux's configured cleanup hooks also run. Failures may leave partial cleanup;
Workbench keeps the registration unless removal is verified. Inspect what remains
before retrying.

## Status and local data

Automatic status detection currently recognizes the Codex CLI's visible terminal
UI. Other agents can be registered, opened, replied to, and reviewed, but may show
`UNKNOWN`. Detection is best-effort: scrolling, menus, or UI changes can hide the
evidence. `TURN FINISHED` means an agent response ended, not that the task is done.
Acknowledgements last only for the current Workbench run.

Registrations and review history stay on your computer. Review snapshots contain
local diff text; treat them like source files when backing up or sharing data.
The default registration file is `~/.local/state/agent-workbench/work-items.json`,
or under an absolute `$XDG_STATE_HOME` when set. Review history is stored beside it. Override
the location with `--state-file PATH` or `AGENT_WORKBENCH_STATE_FILE`.

Workbench uses one tmux server at a time. Check pane mappings after restarting
tmux, since pane IDs can be reused. For command-line registration and listing,
see `workbench --help` and `workbench list`.

## Contributing

Issues and pull requests are welcome. Include reproduction steps for bugs; for
status-detection problems, include the agent version and a screenshot with
sensitive information removed.

Run from a checkout:

```sh
cargo run -p workbench-tui --bin workbench
```

Before submitting changes:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

The Rust workspace separates the engine (`workbench-core`) from the terminal
interface (`workbench-tui`). Tests use temporary repositories; Git is required.
Tests requiring a live tmux server are opt-in.

## License

[MIT](LICENSE).
