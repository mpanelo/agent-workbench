# Roadmap

These are open ideas, not approved implementation plans. Confirm scope and any
new keybindings before starting work. Current features and controls are described
in the [README](../README.md), not tracked here as completed backlog entries.

## v1 candidates

### Relink a missing agent pane

Allow an explicit replacement-pane choice from SESSIONS for an existing
registration. Preserve its Work ID, description, workspace, and saved review
history instead of unregistering and registering again. Revalidate the captured
registration and replacement pane before saving. Never infer a replacement from
a matching title, directory, or branch. The interaction still needs agreement.

### Git and review counts in WORK

Show numeric workspace/review counts, not a prose summary of the agent's response
or a claim about changes in its latest turn. Reuse the REVIEW engine's Git diff
and saved file-review snapshots with the same workspace and resolved base.

Potential counts include changed files, unreviewed files, and changes since
review, including reviewed paths that disappeared from the base diff. Define
the categories and denominators before implementation. Show freshness and
explicit unavailable states on capture failures or limits; never present errors
as zero changes or partial results as complete. A capture is not an atomic
snapshot of a concurrently edited workspace. This proposal remains under discussion.

### Metadata housekeeping

The proposed first slice is bulk unregistering explicitly selected entries with
confirmed missing panes or workspace directories. Missing resources do not prove
an entry is disposable; relinking may be preferable. Discovery/access failures
and unknown agent activity are not proof of absence.

Preview the exact selection, then recheck registrations and resource availability
before confirmation and execution. Require renewed approval if an entry changed
or its resources returned. Preserve code, branches, panes, workspaces, and saved
review history. Housekeeping must not trigger workspace cleanup as a side effect.

The bulk-selection interaction and scope still need agreement. Archive/restore,
review-history deletion, automatic removal, bulk workspace deletion, and broader
cleanup-owner support are outside this first slice.

## Deferred to v2

### Attention age

Show how long an input request has waited or how recently an agent turn finished,
without reordering work items or changing selection. Define which events set or
reset the timestamp and distinguish first observation from verified event time.
Decide restart behavior, privacy, and retention before implementation.

### Claude lifecycle-status support

Investigate reliable activity, input-request, and turn-completion signals with
source validation and conservative unavailable/unknown behavior. Recognizing a
Claude pane is not equivalent to supervising its lifecycle. Define supported
versions, setup, event boundaries, and tests; do not assume Codex's hooks or
permission controls apply. This proposal does not authorize agent configuration changes.

### Complete agent-response viewing in WORK

The current read-only pane preview shows bounded recent terminal output. This
proposal is for complete structured responses, not an extension of that capture.

Show the selected item's complete latest response in a read-only, scrollable
detail area, especially after a turn finishes. Keep opening the existing agent
pane for interaction. Preserve the stable work list, selection, reading position,
and response formatting.

A partial visible-pane capture is not sufficient: it may omit conclusions,
caveats, or questions. Investigate reliable, agent-specific structured message
sources. Never claim a truncated preview is a complete response.

Decide source support, response boundaries, size limits, privacy, retention, and
missing-source behavior first. Workbench does not currently discover transcripts
or persist assistant responses. Any such access or persistence needs explicit
scope; no model calls or terminal emulator are proposed.

Tmux popups are an optional alternative, not a requirement. Any popup experiment
needs separate lifecycle/layout investigation and approval of its controls.

## Follow-ups without an agreed release or scope

- Neovim integration: open the selected diff file in Neovim from REVIEW, ideally
  at the relevant line. Scope how to reuse an existing editor or open a new
  instance, where it appears in tmux, and how to return to Workbench. Decide how
  diff positions map to file lines, including deleted files, historical content,
  and changes-since-review. Distinguish the live workspace from the captured
  review snapshot and define refresh behavior after edits. Preserve unsaved
  editor buffers and review marks. The release target, interaction, and any
  keybindings need agreement before implementation.
- Review-history housekeeping: preview saved contexts without a matching current
  registration. They may remain useful after re-registration or an ID rename.
  Deletion must be opt-in, explain the loss of marks and snapshots, and establish
  backup/recovery behavior first. No automatic history pruning is planned.
- Archive/restore: consider an alternative to unregistering only after deciding
  restore behavior and the persistence model.
- Broader workspace cleanup: investigate additional ownership/session modes and
  duplicate-window cases separately from metadata housekeeping. Retain explicit
  target confirmation, dirty-file and unmerged-commit safeguards, and delegation
  to the workspace's owning tool. A finished agent turn is not permission to
  delete its workspace.
- Durable acknowledgements: require reliable completion identity and explicit
  restart semantics before retaining dismissals across Workbench runs. Native
  identity is available with validated Codex hooks, but terminal-only detection
  remains heuristic. Cross-restart dismissal is not implemented.
