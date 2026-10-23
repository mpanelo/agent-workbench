# Agent Workbench

A terminal dashboard for supervising coding agents in tmux and reviewing their
code. Track parallel work, respond to agents, and see what changed since your
last review.

Workbench runs locally alongside tmux, Git, and workmux. It does not start agents,
create worktrees, or require a cloud account or API key of its own.

## Features

- Track work items, input requests, and finished turns in one dashboard.
- Open agent panes, send replies, and respond to supported approval requests.
- Review Git diffs with persistent file-level marks and changes-since-review.
- Unregister work items or clean up managed workmux workspaces.

## Installation

Requires Git, tmux, and a Rust toolchain with Cargo. Install from source:

```sh
cargo install --locked --git https://github.com/mpanelo/agent-workbench.git \
  --branch main workbench-tui
```

The executable is `workbench`. Ensure `~/.cargo/bin` is on your `PATH`; rerun the
command to update. Prebuilt binaries are not currently available.

Use a truecolor terminal and Nerd Font v3+ for colors and session icons.
Workmux is optional unless you use workspace cleanup.

## Getting started

1. Start your coding agents in tmux panes.
2. Run `workbench` in a separate tmux window.
3. Press `s` for SESSIONS, select an agent, and press `<enter>` to register it.
4. Confirm the Work ID, optional Short Description, and work type. Workspace and
   branch are detected automatically. Press `<enter>` to save.
5. Press `w` to return to WORK.

WORK keeps registered items in a stable order. `Tab` selects the next item needing
attention. In SESSIONS, `f` switches between recognized agents and all panes.

The selected work item includes a read-only pane preview with live colors and
recent output. It sits beside details on wide screens and below them when space
allows. This is a bounded terminal capture, not a complete agent response;
`<enter>` still opens the agent pane for interaction. Preview text is not saved.
Long preview lines wrap to fit; the preview follows the latest wrapped rows.
Long input requests take priority when space is limited.

| Action in WORK | Key |
| --- | --- |
| Open agent pane | `<enter>` |
| Respond to agent | `r` |
| Review diff | `d` |
| Edit ID and description | `e` |
| Open cleanup options | `c` |
| Acknowledge finished turn | `x` |

Press `?` for contextual help and `q` to quit from the main views. Menu controls
are shown inline. Use tmux navigation to return after opening an agent pane.

Replies are single-line messages. Supported Codex approvals offer approve-once,
reject, and reply choices. Workbench rechecks the prompt before sending a decision
and never grants persistent permission. Inspect the agent pane before retrying a
failed send, since some input may already have arrived.

## Code review

Press `d` on a work item. `Space` toggles a file's review mark, `r` reloads changes,
and `c` switches between the full diff and changes since review. Use
`<c-d>` / `<c-u>` to scroll and `Esc` to return. Long lines wrap to fit the diff
viewport, preserving indentation and syntax colors.

Review marks persist across restarts. Review is file-level, and diffs update only
when reloaded. Review operations never stage, commit, or modify source files.
Binary content requires an external viewer.

Go files have syntax highlighting in both diff modes, using Catppuccin colors
over addition/deletion backgrounds. Highlighting uses captured source, not live
edits, and is cached while scrolling. Other languages, unavailable source, or
files exceeding the highlighting limits (512 KiB per source side or patch) keep
the standard diff colors. No language server or editor setup is required.

The default base is `HEAD`, including staged, unstaged, and non-ignored untracked
changes. To include committed branch changes, choose a local revision:

```sh
workbench --diff-base origin/main
```

Comparison uses that revision directly, not an inferred merge-base, and does not
fetch remote refs. A different resolved base uses a separate review context.

## Workspace cleanup

Press `c` in WORK to open cleanup options:

- `u` removes only the registration, keeping panes, files, branches, and review history.
- `c` previews cleanup and requires confirmation. It closes the **entire workmux
  window**, including running programs, and removes the linked worktree.
  Branch and review history remain. Cancel is selected by default.

Cleanup requires a supported workmux window-mode worktree. Dirty, shared, locked,
or ambiguous targets are blocked; force deletion is not supported. The preview
includes ignored files that will be deleted, such as `.env` and build caches.
Stop agent work first. Workmux cleanup hooks run, and failures can leave partial
cleanup. Inspect remaining resources before retrying.

## Status and local data

Status detection targets Codex. [Opt-in hooks](docs/codex-hooks.md) improve
reliability during scrolling and multiline input. Terminal-only detection may
report `UNKNOWN` when evidence is hidden. Other agents support pane navigation,
replies, and review without lifecycle detection.

`TURN FINISHED` means a response ended, not that the task succeeded.
Acknowledgements last for the current Workbench run. Missing or inaccessible
resources are reported separately; registrations are never removed automatically.

The default state file is `~/.local/state/agent-workbench/work-items.json`, or
under an absolute `$XDG_STATE_HOME` when set. Override it with `--state-file PATH`
or `AGENT_WORKBENCH_STATE_FILE`. Review history is stored alongside it and contains
local diff text; treat snapshots as source code when sharing or backing up data.

Run `workbench --help` for command-line registration and other options.

## Contributing

Issues and pull requests are welcome. Include reproduction steps and redact
sensitive data from screenshots or logs.

Development uses the [pinned Rust toolchain](rust-toolchain.toml):

```sh
cargo run -p workbench-tui --bin workbench
```

Install the dependency checker with
`cargo install cargo-deny --version 0.20.2 --locked`, then run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny --locked check
```

Tests require Git; live-tmux tests are opt-in. See [AGENTS.md](AGENTS.md) for
architecture and style rules, [status detection](docs/status-detection.md) for
implementation details and testing, and the [roadmap](docs/roadmap.md) for open proposals.

## License

[MIT](LICENSE).
