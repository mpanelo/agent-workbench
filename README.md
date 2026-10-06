# Agent Workbench

A local-first terminal dashboard for supervising coding agents in tmux and
reviewing their code. Keep track of parallel work, see who needs your attention,
and jump between agents without hunting for panes.

Workbench complements tmux, Git, and workmux. It doesn't start agents or create
worktrees, and it requires no cloud account or API key.

## Features

- Track work items linked to agent panes.
- See all work, input requests, and finished turns in one dashboard.
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
   Workbench detects its Git workspace and branch and shows them read-only.
4. Confirm the Work ID, optional Short Description, and Type, then press `<enter>` to save.
5. Return to WORK with `w` to supervise your registered agents.

WORK keeps an aligned ID/status list in registration order and shows details,
including work type, for the selected item separately. `!` marks items needing
attention; `Tab` jumps to the next one.
Status changes and expanding prompts don't reorder the list or change selection.
Press `r` to respond. Recognized approvals replace the help bar with response
choices without covering the request: `y` approves once, `n` rejects and opens a
reply, `<enter>` opens the reply editor, and `Esc` cancels. Other prompts open
the reply editor directly. A changed request disables the response choices until
you cancel and reopen the bar.
Approving shows inline feedback: a green checkmark confirms the key
was sent, then clears after 1.5 seconds. It does not imply the agent has resumed;
the displayed agent status continues to reflect observations.
Use `<c-d>` / `<c-u>` to scroll the selected item's details. The old `a` shortcut
also returns to WORK.

A Work ID is your unique task label, such as `fix-auth` or `PR-42`.
Short Description is an optional readable summary, limited to 120 characters.
It starts empty when registering from SESSIONS and can be left blank. Work items
can represent implementation work or code review.

Missing resources are marked `PANE MISSING`, `WORKSPACE MISSING`, or
`RESOURCES MISSING` and need attention. `UNAVAILABLE` means a resource could not
be checked, not that it was deleted. `UNKNOWN` is reserved for uncertain agent
activity. Details show which resource needs attention and any live agent status.
These warnings clear when resources return; Workbench never removes registrations
automatically or treats missing resources as completed work. After manual cleanup,
use `c`, then `u` to unregister the item. Acknowledging a finished turn cannot
hide a resource warning.

SESSIONS shows recognized coding-agent commands by default. Press `f` to show all
panes if your agent runs through a wrapper or isn't recognized.
Registration shows the repository only when it differs from the workspace.
`Detached HEAD` means the workspace has no checked-out branch, not that detection
failed. If Git metadata is unavailable, SESSIONS registration is blocked; use
`workbench register` for manual registration. If the detected target changes
while the form is open, cancel and reopen it before saving.

## Keyboard shortcuts

Press `?` for the full bindings in the current view. Clean Up Options shows its
controls inline instead. In text inputs, `?` types a literal question mark.
`<c-d>` means Control+D; `<enter>` means the Enter key.

| Action | Key |
| --- | --- |
| Work / sessions | `w` / `s` |
| Select an item | `j` / `k` or `↓` / `↑` |
| Open the selected agent pane | `<enter>` in WORK |
| Respond to an agent | `r` in WORK; reply editor uses `<enter>` to send |
| Review the selected item's diff | `d` in WORK |
| Edit ID and description | `e` in WORK |
| Open Clean Up Options | `c` in WORK |
| Unregister only / clean up workspace | `u` / `c` in the removal menu |
| Approve a request once | `y` in the response bar |
| Reject and compose instructions | `n` in the response bar |
| Write a reply / cancel | `<enter>` / `Esc` in the response bar |
| Acknowledge a finished turn | `x` in WORK |
| Next item needing attention | `Tab` in WORK |
| Register a pane | `r` or `<enter>` in SESSIONS |
| Toggle agent-only / all panes | `f` in SESSIONS |
| Scroll half a page | `<c-d>` / `<c-u>` outside text inputs |
| Quit | `q` in the main views |

Forms support arrow-key cursor movement, Tab to switch fields, and `<c-u>` to
clear a field. `Esc` cancels an edit or reply before saving or sending.
Replies are single-line messages. If sending fails, inspect the agent pane before
retrying: some input may already have arrived.

Approval shortcuts work only on recognized Codex approval dialogs. Workbench
rechecks the prompt before sending a decision; `y` never grants persistent
permission. `n` rejects first, then opens a reply for your instructions. Other
prompts or customized agent shortcuts should be handled in the agent pane.

Opening a pane inside tmux keeps Workbench's display, selection, and background
refresh running. Use your tmux navigation to return. When running outside tmux,
Workbench restores the terminal before attaching; detach to return to Workbench.

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

Press `c` in WORK to open **Clean Up Options** for the selected item. Inside the menu,
`u` opens unregister confirmation, `c` opens workspace-cleanup preview, and `Esc`
cancels. Neither opening the menu nor choosing an action removes anything.
All menu controls are shown inline; `?` does not open help here, and `<enter>` does
not pick a default action. The bottom help bar is hidden while this menu is open.
Changed or removed registrations disable the menu until you cancel and reopen it.

Unregistering with `c`, then `u` removes only the Workbench entry after confirmation;
it leaves your panes, files, branch, and review history intact.

Cleanup with `c`, then `c` removes a linked worktree and closes its **entire workmux
window**, including companion panes and their running programs. The branch and
review history are kept. Nothing is merged or pushed.

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

Codex CLI supports [opt-in Workbench-owned hooks](docs/codex-hooks.md) for status
that survives scrolling and multiline drafts. Setup is manual; existing hooks
and notifications aren't changed automatically. Terminal detection remains the
zero-configuration fallback and can show `UNKNOWN` when UI evidence is hidden.
Other agents can be registered, opened, replied to, and reviewed, but may show
`UNKNOWN`. `TURN FINISHED` means a response ended, not that the task is done.
Acknowledgements last only for the current Workbench run.
Acknowledging removes the attention marker, not the work item or its selection.

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
