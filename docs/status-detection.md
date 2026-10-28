# Status detection

This is the developer reference for current detection behavior. For user setup,
compatibility, troubleshooting, and disabling hooks, see [Codex hooks](codex-hooks.md).
Recognizing a coding-agent command in SESSIONS does not imply lifecycle support.
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

The separate WORK pane preview captures only the selected registration's recent
terminal output, independently of status detection. It reads up to 200 history
rows plus the viewport, with a 256 KiB output cap and a three-second command
timeout. Identity and dimensions must match before and after capture; changed
registrations discard results. Captures are decoded to presentation styles,
kept only in memory, and cleared on selection/view changes. Preview text never
establishes status or authorizes input, and is not a complete agent response.

Preview capture targets a 200 ms interval (up to 5 updates per second), backing
off to 500 ms after one second without a displayed change. The first detected
change restores 200 ms polling. New activity may take up to roughly 500 ms plus
capture/processing time to appear while backed off. A new selection starts fast.
There is at most one capture in flight. A dedicated deadline avoids rounding
starts to the input polling tick. Identical captures skip ANSI decoding, and
unchanged decoded output or repeated errors do not request a redraw or reset
the quiet period. Repeated errors also back off; recovery starts fast again.
Other application events can still redraw the screen.

### Preview performance

A local macOS arm64 release-build comparison used isolated tmux servers, 200
synthetic history rows, a 200x50 agent pane, and a 160x40 Workbench PTY. Each
idle/active scenario ran for ten seconds; active output changed at 20 Hz. No
real agents or user sessions were involved. These are local measurements, not
performance guarantees or measurements of terminal-app/GPU rendering.

The 10 FPS versus 5 FPS comparison repeated idle/active scenarios in reverse
binary order. Both versions backed off to 2 FPS after one second of quiet.
The ranges below are from two clean ten-second runs per case, without the
capture-counting shell wrapper. Idle averages include the initial fast period.

| Scenario | 10 FPS active: app + child CPU | 5 FPS active: app + child CPU |
| --- | --- | --- |
| Idle | 3.2% to 4.2% | 3.7% to 3.8% |
| Active | 15.3% to 15.6% | 7.7% to 9.1% |

CPU is relative to one core and includes Workbench's waited child commands.
Active CPU fell about 41% to 51% in the matched pairs. Idle CPU was similar,
as expected with the same idle polling rate. The separate tmux server used
about 0.9% to 1.1% while idle in both versions, and 5.3% at 10 FPS / 2.9% to
3.3% at 5 FPS while active, including processing synthetic pane output.
CPU varies between runs; compare these paired results rather than combining
them with earlier experiments run under different system load.

A separate instrumented run measured steady polling (seconds two through eight)
at 2.00 captures/sec idle and 4.99/sec active. With three seconds of quiet followed
by output changing at 20 Hz, the first visible change arrived after 308 ms.
This is one observed wake delay, not a tighter guarantee than the 500 ms polling
interval. Active redraws averaged about 5.6/sec and idle redraws about 0.9/sec;
other application events can also redraw the screen.

In the initial fixed-rate experiment, for 100 samples, a roughly 38 KiB capture
plus metadata CLI round trip took 7.76 ms median / 8.56 ms p95. With roughly
43 KiB of colored output and a 96x36 test viewport, changed-output decoding took
0.91 ms median / 1.84 ms p95,
wrapping plus TestBackend rendering 0.40 ms / 0.72 ms, and identical-output
comparison 1.7 microseconds / 3.5 microseconds. Capture subprocess overhead is
therefore the main remaining cost in this experiment. Idle redraw suppression
does not eliminate capture polling costs, and adaptive polling reduces those
costs only when output stops changing. An event-driven transport would need a
separate design.

Rerun the synthetic decode/render timing explicitly, without timing assertions:

```sh
cargo test --release -p workbench-tui preview::tests::test_timing -- --ignored --nocapture
```

### Experimental live preview

The opt-in `live-preview` build feature tests a persistent tmux control-mode
connection. It is not a proven active-CPU optimization. Normal builds retain
5 FPS active / 2 FPS idle polling. To try the prototype without replacing an
installed binary:

```sh
cargo run --release -p workbench-tui --features live-preview
```

The core attaches one client to the selected pane's session, with read-only,
ignore-size, independent-active-pane, and pause-after flags. It does not send
keys, select panes/windows, set client dimensions, update the session environment,
or create a session. Attaching is still a real tmux client attachment: tmux's
attached-client counts change, and user-configured attachment hooks may run.

Pane-output notifications are discarded after identifying their pane. They only
mark the preview as needing a fresh snapshot, capped at 10 captures/sec. No
periodic preview capture runs while quiet. Snapshots use the existing bounded
capture and safe display decoder, never raw terminal output or a terminal
emulator. The UI retains only the latest pending update and discards stale
generations. Selection/view changes cancel the observer and clear its output.

Capture identity and dimensions are rechecked, as is the pane's session. Command
reads time out after three seconds. Protocol lines and combined snapshot replies
are capped at 256 KiB; total connection traffic is capped at 4 MiB per second.
A disconnect, timeout, failed command, changed session, paused selected pane, or
exceeded limit stops the observer and falls back to normal polling for that
selection. A later selection starts a new attempt. Other panes in the attached
session also produce control notifications; their contents are ignored, but
their traffic counts toward the limit. This is one reason the prototype is not
the default.

The [tmux control-mode documentation](https://github.com/tmux/tmux/wiki/Control-Mode)
describes command framing, output notifications, and flow control. Parser tests
cover malformed/failed replies, non-UTF-8 notifications, bounds, and changes
between reply blocks. The opt-in disposable-server smoke test checks snapshots,
pane size/focus, cancellation, and pane disappearance:

```sh
cargo test -p workbench-core preview::tests::test_tmux_preview -- --ignored --nocapture
```

A separate local macOS arm64 comparison used the synthetic workload described
above with tmux 3.7c, two ten-second runs per case, and reversed binary order in
the second pair. The harness switched out of WORK before exit to cancel and reap
the observer while the runtime was still alive. CPU accounting includes that
roughly 250 ms teardown in both versions and includes waited child commands.

| Scenario | Polling: app/children + tmux CPU | Live: app/children + tmux CPU |
| --- | --- | --- |
| Idle | 3.7% to 4.6% | 0.6% to 0.8% |
| Active | 9.1% to 12.2% | 9.8% to 11.0% |

These are combined percentages of one core, not whole-machine utilization.
Active app/child CPU was 6.6% to 8.8% polling versus 4.2% to 4.6% live, but tmux
server CPU rose from 2.5% to 3.4% to 5.6% to 6.4%. Counting both, live active CPU
was about 7% higher in one pair and 9% lower in the other. There is no consistent
active CPU improvement. Idle combined CPU fell about 79% to 87% in the pairs.
The synthetic producer and terminal-app/GPU costs are not included.

The live UI redrew about ten times per second during active output. In a separate
quiet-to-active burst, the first visible change arrived after 116 ms. This is one
observation, not a latency guarantee. Safety tests and local results justify an
opt-in experiment, not enabling it by default. Busy sibling panes, custom tmux
hooks, and high-output real agents still need broader testing. Reducing snapshot
work and unrelated notification traffic would be follow-up experiments, not
reasons to raise the default polling rate.

The repeatable [six-pane stress test](preview-stress.md) includes Workbench's
own pane in the session. It found that long-line bursts can exceed the live
connection's traffic limit and cause unexpected polling fallback. The promotion
gate failed, so live preview remains opt-in and the polling implementation is
retained. See the guide for reproduction steps, thresholds, and limitations.

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
