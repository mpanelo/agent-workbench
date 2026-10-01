# Codex status signals

## Implemented terminal rules

Workbench still observes only registered panes through tmux. Detection stays in
`workbench-core`, separate from rendering, and replaces evidence every refresh.

The reported September 30, 2026 layout exposed three mismatches: bulletless
`Working (55s • esc to interrupt)`, a rate-limit banner between activity and the
composer, and a warning count after `? for shortcuts`. The detector now accepts
these forms, including a composer-free active layout with a bottom rate-limit
banner. It skips only recognized adjacent UI banners, with a bounded look-back.
Approval dialogs take precedence; historical/quoted activity, malformed timers,
unknown footers, unsupported processes, dead panes, and copy mode remain guarded.

The October 1 screenshots added `Compacting context` with its “Making room to
continue” detail, and a three-row queued-question block between `Working` and the
composer. Both active layouts now count as `RUNNING`. Only a complete adjacent
queued block (known header, positive question count, and answer shortcut) is
skipped; it does not itself imply running or waiting. The compaction detail is
accepted only immediately after a valid compaction timer. The look-back remains
bounded to 24 nonempty rows, and arbitrary intervening prose, malformed blocks,
quoted activity, and missing current UI anchors do not establish running state.

Additional captures confirmed the `tab to queue message` typing footer and “New
activity · ↓ Back to bottom · esc” scroll banner. Composer drafts may wrap (up
to 20 nonempty rows between the prompt start and footer). Queued-question counts
can carry an elapsed-time suffix. Pending-message previews are skipped only
after their complete known UI title and an arrow-prefixed preview; activity must
come from before that title, never arbitrary preview/draft text. If scrolling
hides the activity row, the detector stays `UNKNOWN`, not `IDLE` or `COMPLETE`.
tmux copy mode still invalidates observation; Codex's own scroll UI is separate.

The October 1 11:53 screenshot replaces the usual shortcut footer with
`New activity · Earlier messages available. enter/esc latest` (the `New activity`
prefix is optional; spacing may vary). This footer is now recognized, but still
requires a visible composer and an adjacent valid live activity timer before
reporting `RUNNING`. Hidden timers, old completion markers, unrelated output,
quoted/malformed chrome, dead panes and tmux copy mode remain `UNKNOWN`; merely
scrolling never establishes activity or completion.

## Hook investigation — not implemented

Local verification: `codex-cli 0.159.3`; `codex features list` reports `hooks`
as stable and enabled. This verifies availability, not an end-to-end callback.

The [official hook guide](https://learn.chatgpt.com/docs/hooks) documents command
hooks receiving JSON on stdin, session/turn identifiers, and user/project hook
configuration. Non-managed hooks require explicit trust through `/hooks`.
Subagent hooks share the parent session ID; callbacks may finish out of order.
Hosted web search has no tool hooks. `PermissionRequest` precedes approval and
other hooks can decide it automatically. `Stop` can trigger continuation rather
than final completion. Transcripts are not a stable hook interface.

The separate [notification callback](https://learn.chatgpt.com/docs/config-file/config-advanced#notifications)
currently documents only `agent-turn-complete`, so `notify` alone cannot cover
running and waiting states.

## Proposed opt-in adapter

This is a design recommendation, not a new milestone implementation. Keep the
terminal adapter as the zero-configuration fallback. Add a separate local signal
adapter in the core only after validating real callback payloads and ordering.

| Signal | Proposed interpretation |
| --- | --- |
| `UserPromptSubmit` | Candidate active turn, correlated to the current session. |
| `PermissionRequest` | Pending approval context; confirm a visible dialog before claiming human attention. |
| `PostToolUse` | Evidence of activity, not proof a turn is idle or finished. |
| `Stop` / turn-complete notification | Candidate completed turn; reconcile continuation and newer activity. |
| `Interrupt` / `SessionEnd` | Invalidate running evidence; do not imply the task succeeded. |

Use a small bounded local event inbox, explicitly bind session + turn identity
to a registered pane and tmux server, and validate any inherited `TMUX_PANE`
rather than treating workspace paths as identity. Isolate subagent events: they
must not complete or retarget the supervising root session. Reject delayed or
superseded events and reconcile liveness; a lost callback or expired observation
must not leave a pane permanently running/waiting/complete.

The callback must be observational: no permission decisions, prompt context,
continuation requests, or agent input. Return valid neutral JSON (including for
`Stop`), keep work short, and tolerate Workbench being closed. Retain only minimal
status metadata in private local storage; do not copy prompts, command output,
or transcripts. No network service, LLM API, or automatic configuration changes.

Before implementation, test callback trust and version compatibility, actual
pane identity propagation, overlapping turns, multiple permissions, denial,
continuation, interruption, subagents, delayed writes, process crashes, and restart
recovery. `PermissionRequest` is not an approval-resolved event; a hybrid design
still needs terminal confirmation or a validated complementary event source.
