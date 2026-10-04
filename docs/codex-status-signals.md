# Codex status signals

## Implemented terminal rules

Workbench supervises only registered panes through tmux. Terminal detection stays
in `workbench-core`, separate from rendering, as the zero-configuration fallback.
Validated opt-in Codex lifecycle signals now take precedence over ambiguous screens.

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
hides the activity row, the detector stays `UNKNOWN`, not `IDLE` or `TURN FINISHED`.
tmux copy mode still invalidates observation; Codex's own scroll UI is separate.

The October 1 11:53 screenshot replaces the usual shortcut footer with
`New activity · Earlier messages available. enter/esc latest` (the `New activity`
prefix is optional; spacing may vary). This footer is now recognized, but still
requires a visible composer and an adjacent valid live activity timer before
reporting `RUNNING`. Hidden timers, old completion markers, unrelated output,
quoted/malformed chrome, dead panes and tmux copy mode remain `UNKNOWN`; merely
scrolling never establishes activity or completion.

The October 1 1:15 and 1:19 screenshots exposed two more variants: expanding
multiline input hides the shortcut footer, leaving the model/effort/workspace row,
and the compaction detail can end with a period. Both are now recognized. The
metadata-only footer requires an observed `GPT-…` model and known effort label,
an absolute or home-relative workspace path, and a visible first-column composer
marker within the same bounded composer window. Draft continuations retain their
indentation so a typed `›` cannot expose draft text as activity evidence. A live
timer still establishes `RUNNING`; without it, a ready composer is `IDLE` or
`TURN FINISHED` only when immediately following a recognized completion marker.
The punctuated compaction detail still requires an adjacent valid compaction
timer. Missing composers, malformed footers/timers, history without live
activity, unsupported processes, dead panes and copy mode remain inconclusive.

## Implemented opt-in signals

The adapter lives in `workbench-core/src/codex_signals.rs`. `workbench codex-hook`
accepts JSON on stdin; `workbench codex-notify` accepts the appended notification
JSON argument. Both are observational and return neutral `{}`. See
[setup and limitations](codex-hooks.md); nothing is installed automatically.

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

## Interpretation

Installed Codex 0.159.3 was exercised with an isolated app-server, fake localhost
provider, and disposable configuration before implementation. Those probes covered
valid/multiple questions, custom answers, invalid/free-text question tools, Default
mode rejection, question interruption, approval/denial/automatic approval, delayed
denial resolution, Stop continuation, quiet generation, prose questions, and
unsupported async questions. A real isolated TUI probe verified that unsubmitted
multiline drafts, scrolling and resizing don't emit turn lifecycle events.

| Signal | Interpretation |
| --- | --- |
| `SessionStart` | Bind a live root process to session, pane and tmux server. |
| `UserPromptSubmit` | Start the active root turn; supersede earlier turn evidence. |
| `PreToolUse` (`request_user_input`) | Bounded question/options candidate; require matching current dialog. |
| `PermissionRequest` | Pending approval context; confirm a visible dialog before claiming human attention. |
| `PostToolUse` | Resolve matching pending questions; not proof a turn finished. |
| `Stop` | Clear pending candidates, but stay running: continuation can reuse the turn. |
| `agent-turn-complete` notification | Complete only the currently active matching session/turn. |
| `Interrupt` / `SessionEnd` | Invalidate running evidence; do not imply the task succeeded. |

The owner-only sidecar store is atomically replaced under a nonblocking lock and
bounded to 128 process bindings. Callback entry timestamps reject out-of-order
writes; current session/turn plus bounded retired IDs reject superseded callbacks.
Inherited `TMUX`/`TMUX_PANE`, live server identity, pane PID, Codex ancestry, and
process start time must agree. Workspace paths are never identity. Agent-tagged
and subagent lifecycle events cannot complete the root turn. Signal evidence
expires after 24 hours without a callback, not after brief quiet generation.

Only lifecycle metadata and bounded pending structured questions/options are
retained. No submitted prompts, answers, assistant text, tool output, or transcripts
are stored. No service, model API, configuration writes, or permission decisions.
Native completion identity keeps local acknowledgements stable when viewport text
changes. Approval sending still rechecks the full exact visible dialog separately.

`PermissionRequest` is not an approval-resolved event. Recognized live resumed
activity clears a confirmed denied approval; scrolling it out of view does not.
An invalid question may emit PreToolUse but no PostToolUse, so a candidate alone
never establishes WAITING. `permission_mode` is not collaboration mode: real valid
Plan-mode question callbacks can still report `default`. Stop continuation can
emit two Stops for one turn and only one final notification. A lost notification
may leave RUNNING until another callback, process exit, or expiry; this hybrid
adapter does not claim full app-server request-resolution coverage.
