# Agent Workbench

M1: a read-only terminal view of the sessions, windows, and panes on your tmux
server. Requires Rust 1.88+ and tmux on `PATH`.

```sh
cargo run -p workbench-tui --bin workbench
```

Run in an interactive terminal, inside or outside tmux. Discovery uses tmux's
normal server selection: the inherited `TMUX` environment when present, otherwise
the default socket. It does not create sessions, switch panes, or send input.

The view shows session names/IDs, window names/indices/IDs, and pane IDs, indices,
titles, current commands, and working directories. Missing command/path metadata
is shown as `unavailable`. Discovery refreshes in the background every two seconds;
errors replace the displayed snapshot and retry automatically. Each command has a
three-second timeout. Quit with `q`, `Esc`, or `Ctrl-C`; scroll with arrow keys or
Page Up/Page Down, and return to the top with Home. Resize is handled automatically.
Long metadata lines are clipped to terminal width; control characters are displayed
as escapes.

## Structure

- `workbench-core`: typed snapshots, tmux CLI adapter, parsing, operational errors.
  Its only external dependency is Tokio; it has no presentation dependencies.
- `workbench-tui`: Ratatui/Crossterm rendering and input, using the core engine.

Discovery uses one `list-panes -a` call and groups rows by stable session/window
IDs. Linked windows are represented in each session. Snapshot replacement removes
disappeared panes without retaining stale records. Sessions sort by name; windows
and panes sort numerically by index.

Metadata must be UTF-8. ASCII unit/record separators (`U+001F`/`U+001E`) are
reserved for the discovery format; metadata containing those rare characters is
reported as malformed. Ordinary spaces, tabs, newlines, and Unicode are preserved.
Only the selected tmux server is discovered; multi-server aggregation is not part
of M1. There is no agent classification, work-item registration, persistence,
navigation to panes, input forwarding, or Git/review functionality yet.

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
