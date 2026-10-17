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
- Approval and structured question hooks require a recognized live dialog before
  claiming human attention. Scrolling a confirmed request out of view does not
  resolve it. Free-form answers are handled in Codex; Workbench does not store answers.
- Hooks are observational: they cannot grant permissions or bypass the existing
  approval shortcuts' exact-current-dialog safety check.
- A quiet turn does not time out after ten seconds. Signals expire after 24 hours
  without a callback. A lost completion callback can leave RUNNING until newer
  evidence, process exit, or expiry; this is not a fully authoritative event stream.
- Prose questions in an ordinary final response are finished turns, not native
  input requests. `request_user_input_async` in other clients isn't supported.

Callbacks return neutral `{}` and make no permission decisions, continuation
requests, or agent input. They do not read transcripts, retain submitted prompts,
tool output, answers, or assistant responses, contact a service, or call a model.
Only session/process/turn metadata and bounded pending question text/options are
stored beside the registration file in `*.codex-signals.json`, with owner-only
permissions. Pending content clears on resolution; old records are bounded and
expire. Treat pending questions like private project data when backing up state.

## Troubleshooting

- If status stays `UNKNOWN`, check that both callback commands can find the
  Workbench binary, use the same state file, and inherit the correct tmux context.
  Verify `/hooks` trust and restart/resume Codex so `SessionStart` is observed.
  Unsupported process layouts or missing callbacks leave terminal detection as
  the fallback, which can be inconclusive when activity is hidden.
- If RUNNING remains after a response, check the `notify` command and any wrapper.
  `Stop` is not a substitute for the completion notification. Inspect the agent
  pane before assuming a quiet turn or lost notification means the task finished.
- If waiting persists after denying an approval, inspect the agent pane. The
  tested integration lacks a denial-resolved hook; recognized resumed activity
  or another resolving event must clear the request. Scrolling alone cannot.

## Disable hooks

To disable this integration, remove only Workbench's hook entries and its notify
callback (preserving others), then restart Codex. Existing metadata can be removed
with the agent stopped; terminal detection requires no setup.

## Reference

See [status detection](status-detection.md) for signal precedence, lifecycle
transitions, identity validation, storage bounds, and developer tests.

See the [official OpenAI hook guide](https://learn.chatgpt.com/docs/hooks) and
[notification configuration](https://learn.chatgpt.com/docs/config-file/config-advanced#notifications)
for Codex's hook trust and callback contracts.
