# Reliable Codex status (opt-in)

Workbench can use its own local Codex hooks for lifecycle status and the Codex
completion notification for `TURN FINISHED`. Scrolling, multiline drafts, terminal
resizing, and tmux copy mode then leave valid lifecycle evidence intact. Without
callbacks, the existing terminal detector remains the fallback.

This adapter was tested with **codex-cli 0.159.3**. It currently supports Codex CLI
running directly in tmux with `--no-daemon`, not the desktop app, IDE, shared
daemon, or other agents. It does not start agents or change your configuration.

## Setup

1. Install this version of Workbench and locate the binary with
   `command -v workbench`. It must be available to the Codex hook process too.
2. Merge the groups in [the example hook file](../examples/codex-hooks.json) into
   your `$CODEX_HOME/hooks.json` (normally `~/.codex/hooks.json`). If you have no
   hook file, use the example as its contents. **Keep existing hooks.** Replace
   `workbench` in each command with your absolute binary path if needed; shell
   quote paths with spaces.
3. Add this top-level setting to your Codex `config.toml`:

   ```toml
   notify = ["/absolute/path/to/workbench", "codex-notify"]
   ```

   If you already have `notify`, do not replace it blindly: use your own wrapper
   to call both programs with the same notification JSON argument. Completion
   requires this callback; `Stop` alone cannot prove that a turn has finished.
4. Start a new `codex --no-daemon` session inside the agent's tmux pane. Open
   `/hooks`, inspect and trust the merged hook definitions, then submit a prompt.
   Restart/resume the session after enabling hooks so `SessionStart` is observed.
   Re-review `/hooks` whenever you change the hook definitions. Do not bypass trust.
5. Register the pane in Workbench's SESSIONS view as usual. Hooks can report before
   registration or while Workbench is closed; Workbench still supervises only
   registered panes. `workbench list` shows whether lifecycle signals supply status.

Callbacks and Workbench must use the **same state file**. For a custom location,
add `--state-file /absolute/path/work-items.json` to every hook command and to the
notification array before Codex's appended JSON argument:

```toml
notify = ["/absolute/path/to/workbench", "codex-notify", "--state-file", "/absolute/path/work-items.json"]
```

Alternatively, export `AGENT_WORKBENCH_STATE_FILE` to both processes. Hooks use the
inherited `TMUX` and `TMUX_PANE`; don't manually set those to another pane. The
callback checks the server, pane process, and Codex process ancestry. Missing or
ambiguous context is ignored, never guessed from the working directory.

## Behavior and limitations

- Submitted prompts establish running turns; typing drafts does not.
- `Stop` may trigger continuation, so only a matching `agent-turn-complete`
  notification marks completion. Native session/turn identity also makes
  session-only acknowledgements stable across scrolling and resizing.
- Permission hooks are candidates, not proof of a human decision: other hooks
  may auto-approve. Workbench requires a recognized visible dialog first.
- Structured `request_user_input` questions retain bounded questions/options;
  matching live dialog chrome confirms them. Invalid/rejected calls don't become
  WAITING. Free-form answers are handled in Codex; Workbench doesn't store answers.
- Answered questions clear through the matching tool callback. Interruptions,
  session end, and final completion clear pending requests without claiming task
  success. Subagent events and superseded turns cannot finish the root turn.
- Escape at a Codex TUI approval dialog interrupts the turn; it isn't the same
  event sequence as an app-server client declining an approval and continuing.
- Codex has no approval-denial-resolved hook or permission tool-call ID. An
  unrelated tool callback cannot clear waiting. After denial, recognized live
  activity clears waiting; merely hiding the dialog by scrolling does not.
  Until that evidence or another resolving callback arrives, waiting can persist.
- Liveness is checked on every refresh. Missing/dead panes, replaced processes,
  different tmux servers, and reused PIDs cannot reuse the signal. A quiet turn
  does **not** time out after ten seconds. Signals expire after 24 hours without
  a callback; a lost completion callback can leave RUNNING until newer evidence,
  process exit, or expiry. This isn't a fully authoritative event stream.
- Prose questions in an ordinary final response are finished turns, not native
  input requests. `request_user_input_async` in other clients isn't supported.
- Existing approval shortcuts still require the exact current terminal dialog;
  native question payloads cannot grant permissions or bypass that recheck.

Callbacks return neutral `{}` and make no permission decisions, continuation
requests, or agent input. They do not read transcripts, retain submitted prompts,
tool output, answers, or assistant responses, contact a service, or call a model.
Only session/process/turn metadata and bounded pending question text/options are
stored beside the registration file in `*.codex-signals.json`, with owner-only
permissions. Pending content clears on resolution; old records are bounded and
expire. Treat pending questions like private project data when backing up state.

To disable this integration, remove only Workbench's hook entries and its notify
callback (preserving others), then restart Codex. Existing metadata can be removed
with the agent stopped; terminal detection requires no setup.

See the [official OpenAI hook guide](https://learn.chatgpt.com/docs/hooks) and
[notification configuration](https://learn.chatgpt.com/docs/config-file/config-advanced#notifications)
for Codex's hook trust and callback contracts.

## Testing

Unit tests cover transitions, stale ordering, interruption, continuation, prompt
confirmation/resolution, storage limits, process ancestry, and stable completion
acknowledgements. For an optional real-runtime smoke test:

```sh
cargo build --workspace
python3 scripts/test-codex-signals.py
```

The script uses disposable configuration, its own tmux server and terminal, and a
fake localhost Responses provider. It does not touch live panes or user settings,
need authentication, or make paid model calls. Its trust bypass is restricted to
the generated fixture; production setup must use `/hooks` instead.
