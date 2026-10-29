# Status detection

This is the developer reference for current detection behavior. For user setup,
compatibility, troubleshooting, and disabling hooks, see [Codex hooks](codex-hooks.md).
Recognizing a coding-agent command in the session list does not imply lifecycle support.
Status inference currently targets Codex; other agents can still be registered,
opened, replied to, and reviewed.

## Evidence and precedence

The core supervises registered panes only. [Terminal inference](../crates/workbench-core/src/agent_state.rs)
uses read-only tmux observations. The optional [Codex adapter](../crates/workbench-core/src/codex_signals.rs)
validates lifecycle callbacks against a live process and pane before applying them.

- Valid lifecycle evidence survives ambiguous or hidden terminal UI, including
  scrolling, multiline drafts, resizing, and tmux copy mode.
- A recognized current approval dialog retains its exact visible text, even when
  lifecycle signals are available. Sending approval is a separate operation that
  rechecks the current dialog; hook payloads cannot grant permissions.
- An initial idle hook record does not override recognized live running activity
  when a prompt-submission callback may have been missed.
- Without usable lifecycle evidence, terminal inference remains the fallback.
  Missing/dead panes, unsupported commands, copy mode, failed captures, or
  unrecognized layouts cannot establish activity from screen text alone.

Missing resources are separate from uncertain activity. WORK displays missing
pane/workspace warnings or `UNAVAILABLE` when checks fail; it does not equate
these with task completion or remove registrations automatically.
`TURN FINISHED` means the agent's response ended, not that the task succeeded.

## Terminal fallback

Require current UI anchors, not arbitrary prose or historical output:

- Approval detection requires recognized dialog chrome, a known title, and a
  selected supported choice. A quoted approval question is not sufficient.
- Running detection requires a valid activity or compaction timer adjacent to
  recognized live UI. Some active layouts omit the composer but retain a
  rate-limit banner directly after the activity row.
- Recognized composer footers include shortcuts, message queuing, history
  navigation, and model/effort/workspace metadata. Metadata-only layouts require
  a known model/effort and absolute or home-relative workspace path.
- Skip only recognized adjacent banners and complete queued-question or pending
  message blocks. The queued blocks are not themselves proof of running or
  waiting. Activity must precede pending-message previews, not come from draft text.
- Bound activity look-back to 24 nonempty rows and composer continuations to 20.
  Preserve draft indentation so typed composer markers cannot expose draft prose
  as activity evidence. Compaction detail text requires an adjacent valid timer.
- A ready composer without running evidence can establish idle; completion also
  requires an adjacent recognized completion marker. Hidden timers while viewing
  history cannot establish running, idle, or completion.

Unexpected controls, malformed timers/footers, unsupported processes, and missing
anchors stay inconclusive. Merely scrolling does not establish a state transition.
The fixtures and parser tests define accepted layout variants; do not add broad
text matches to make an ambiguous screen appear certain.

## Lifecycle callbacks

`workbench codex-hook` reads JSON from stdin. `workbench codex-notify` reads the
appended notification JSON argument. Both return neutral `{}` and make no
permission decisions, continuation requests, or agent input. The adapter's tested
runtime scope is recorded in the [setup guide](codex-hooks.md).

| Signal | Interpretation |
| --- | --- |
| `SessionStart` | Bind a live root process to session, pane, and tmux server. |
| `UserPromptSubmit` | Start the active root turn and supersede earlier evidence. |
| `PreToolUse` for `request_user_input` | Retain a bounded question/options candidate; require a matching current dialog. |
| `PermissionRequest` | Record an approval candidate; confirm visible dialog evidence before claiming human attention. |
| `PostToolUse` | Resolve matching pending questions, not the whole turn. |
| `Stop` | Clear pending candidates but stay running, since continuation can reuse the turn. |
| `agent-turn-complete` notification | Finish only the current matching session/turn. |
| `Interrupt` / `SessionEnd` | Invalidate running evidence without claiming task success. |

Candidates alone do not establish waiting: other hooks can auto-approve a
permission request, and invalid question calls can lack a resolving callback.
`permission_mode` is not collaboration mode; do not use it to infer whether a
question is valid. Subagent/agent-tagged events and superseded turns cannot finish
the root turn. Completion notifications alone do not cover running and waiting.

Confirmed requests persist when their dialogs scroll out of view. Matching tool
callbacks resolve questions; interruption, session end, and completion clear
pending requests. Codex has no approval-denial-resolved hook or permission tool-call
ID in the tested integration. Recognized live resumed activity can clear a
confirmed approval, but unrelated tool callbacks or hidden dialogs cannot.
Escape in a Codex TUI approval dialog interrupts the turn; app-server denial can
instead continue the same turn. Stop continuation may emit multiple Stops before
one final completion notification.

Ordinary final-response questions are finished turns, not native input requests.
The adapter does not support other clients' `request_user_input_async` requests
or use transcripts as a hook interface. This hybrid adapter is not a complete
app-server request-resolution stream.

## Identity, lifetime, and acknowledgements

Inherited `TMUX`/`TMUX_PANE`, server identity, pane PID, Codex process ancestry,
and process start time must agree. Workspace paths are never identity. Check
liveness on each refresh; replaced processes, reused PIDs, and other servers
cannot inherit a signal. Callback entry timestamps and retired session/turn IDs
reject stale or superseded writes; reconciliation must not overwrite a newer callback.

Signals expire after 24 hours without a callback, not after a brief quiet period.
A lost completion notification may leave running evidence until another resolving
event, process exit, or expiry. Quiet generation is not proof of idle or failure.

[Attention tracking](../crates/workbench-core/src/attention.rs) uses completion
fingerprints and observation generations. With valid hooks, a fingerprint uses
process/session/turn identity and remains stable across viewport changes. The
terminal fallback hashes normalized visible completion evidence, excluding draft,
footer, and banner changes; it cannot reliably distinguish identical fast turns.

Acknowledgements last only for the current Workbench run. They survive view
switches and pane focus, not restart. New observed activity/input or different
completion evidence can require attention again; uncertainty alone does not
clear an acknowledgement. Acknowledgement never hides resource warnings, dismisses
input requests, removes work items, or changes code-review state.

## Storage and privacy

The owner-only `*.codex-signals.json` sidecar is atomically replaced under a
nonblocking lock and bounded to 128 process bindings. It stores lifecycle metadata
and bounded pending structured question text/options, not submitted prompts,
answers, assistant responses, tool output, or transcripts. Pending content clears
on resolution; records are bounded and expire. No service or model API is called,
and no agent configuration is changed automatically.

The separate WORK pane preview is independent of status detection. It reads up
to 200 history rows plus the viewport, with a 256 KiB capture limit and a
three-second command timeout. Pane identity and dimensions must match around a
capture, and registration changes discard results. ANSI styling is decoded into
presentation styles, never forwarded as raw terminal commands. Captures stay in
memory and never establish status or authorize input.

### Live pane preview

The core keeps one read-only tmux control client for the selected registration.
It starts with output disabled and lists the panes in that session. One
`refresh-client` command enables output and pauses nonselected panes for this
client. Those operations must be atomic: toggling tmux's `no-output` flag resets
its pane offsets, including paused state. Pause affects only the extra output
copy sent to Workbench; panes keep running, and their normal clients are
unaffected. The adapter never turns panes off, sends keys, changes client
dimensions, selects panes/windows, or updates the session environment.

Selected-pane output marks the preview dirty. Snapshots are coalesced to at
most ten per second with one capture in flight; quiet panes have no periodic
preview captures. Topology changes and output from a newly created pane cause
a bounded rescan to pause new nonselected panes. Pane IDs are validated before
being used in commands. The selected pane's session is rechecked after each
capture so a move causes reconnection to its current session.

Transport errors clear stale preview text and show `Preview unavailable` while
the core reconnects. Retry delays start at 200 ms, double to a three-second cap,
and reset after a stream attempt has lasted five seconds. Registration errors end
the stream. There is no polling preview implementation or fallback. Normal
status discovery and input handling still run independently.

Protocol lines and combined snapshot replies are capped at 256 KiB. Total
connection traffic, including snapshot replies, is capped at 4 MiB in each
one-second window. Session scans are capped at 1024 panes. If the selected pane
alone exceeds a limit or tmux pauses delivery, the preview reports an error and
reconnects with backoff; opening the agent pane remains available.

The TUI keeps only the latest pending update. Generation checks reject late
results, and selection/view/modal changes cancel the old client and clear its
output. Repeated errors and identical captures do not request redraws. Captures
are recent screen context, not complete agent responses or transcripts.

Control-mode attachment is still a real client attachment: tmux client counts
change; normal tmux detach policies and user attachment/command hooks still apply.
Workbench does not override those policies. The adapter uses read-only,
ignore-size, independent-active-pane, and pause-after flags. Tested with tmux
3.7c. The [tmux control-mode guide](https://github.com/tmux/tmux/wiki/Control-Mode)
and [client flag implementation](https://github.com/tmux/tmux/blob/3.7c/server-client.c)
describe the protocol and output-offset behavior.

See the [six-pane stress test](preview-stress.md) for reproduction, resource
measurements, recovery checks, and current results. Parser/state tests cover
bounds, failed/malformed replies, stale registrations, retry delays, and pane
list validation. The opt-in live test checks size/focus, quiet selection with a
busy sibling, producer progress, copy-mode recovery, observer reconnection,
cancellation, pane disappearance, and server disconnection:

```sh
cargo test -p workbench-core preview::tests::test_tmux_preview -- --ignored --nocapture
```

Rerun synthetic decode/wrap/render timing without timing assertions:

```sh
cargo test --release -p workbench-tui preview::tests::test_timing -- --ignored --nocapture
```

## Testing

Keep parser and transition tests independent of live agents. Cover layout
anchors, malformed/quoted evidence, approvals and questions, interruption,
continuation, quiet turns, stale ordering, process identity, storage bounds, and
acknowledgements across viewport changes. Test that uncertainty does not invent
completion or silently resolve a pending request.

For an optional real-runtime smoke test, install the compatible Codex CLI listed
in the setup guide and tmux, then run:

```sh
cargo build --workspace
python3 scripts/test-codex-signals.py
```

The [script](../scripts/test-codex-signals.py) uses disposable configuration, its
own tmux server and terminal, and a fake localhost Responses provider. It does not
touch live panes or user settings, require authentication, or make paid model
calls. Its trust bypass is limited to the generated fixture; production setup
must use `/hooks`. Run normal formatting, linting, and tests as described in
the [contribution guide](../README.md#contributing).
