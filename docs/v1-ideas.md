# v1 ideas

Potential follow-ups for v1, plus completed ideas retained for context.

Complete agent-response viewing is deferred to [v2](v2-ideas.md), not a v1
pane-preview or popup feature.

## Relink a missing agent pane

Keep this candidate in v1. Allow an explicit choice of a replacement pane from
SESSIONS for an existing registration, preserving its Work ID, description,
workspace, and saved review history instead of unregistering and registering
again. Revalidate both the captured registration and replacement pane before
saving. Do not infer a replacement from a matching title, directory, or branch.
Confirm the interaction and any new keybindings before implementation.

## Git and review counts in WORK (under discussion)

This is a numeric workspace/review summary, not a prose summary of the agent's
response or a claim about changes made in its latest turn. Reuse the existing
REVIEW engine's Git diff and saved file-review snapshots for the selected work
item, using the same workspace and resolved diff base.

Potential counts include changed files, unreviewed files, and changes since
review, including previously reviewed paths that disappeared from the base diff.
Define the count categories before implementation; do not assume all counts have
the same denominator. Show freshness and explicit unavailable states on capture
failures or limits. Concurrent edits mean a capture is not an atomic snapshot of
the entire live workspace. Do not display a partial result as complete or an
error as zero changes. This idea has not been approved for implementation.

Attention age and Claude lifecycle-status support are deferred to [v2](v2-ideas.md).

## Clarify turn completion and acknowledge attention items

Implemented: the displayed status is `TURN FINISHED` (core enum remains
`AgentStatus::Complete`). In WORK, `x` acknowledges the selected finished
observation without changing the work item or code-review state. Input requests
cannot be dismissed. Repeated captures of the acknowledged completion stay out
of attention; new observed activity/input or changed completion evidence flags it again.
The work item stays visible and selected in the unified WORK view.

Chosen scope: session-only acknowledgements, retained across view switches and
pane focus but reset on Workbench exit. The core tracker uses visible completion
fingerprints and observation generations, not native agent turn IDs. Uncertain
observations alone do not clear an acknowledgement. Terminal captures cannot
reliably distinguish identical fast turns without native agent turn IDs.

Implemented unified WORK view: stable registration-order list, attention markers,
and a separate scrollable detail area for the selected item. Status/prompt changes
do not reorder rows or change selection. Further layout/UX refinements remain open.

Possible follow-up: reliable opt-in turn identifiers before considering durable
cross-restart acknowledgements. No durable dismissal is implemented.

## Clean up stale work and retained state

Investigate an explicit cleanup workflow for stale registrations, retained review
snapshots, and unused agent workspaces. Unregistering currently removes only the
Workbench entry and intentionally leaves panes, branches, worktrees, files and
review history intact.

Potential scope, to decide before implementation:

- Preview orphaned review snapshots and stale/missing-pane registrations, with
  per-item selection rather than automatic pruning.
- Clearly separate metadata cleanup from closing panes or deleting worktrees
  and branches; never infer that a finished agent turn makes those safe to delete.
- If workspace cleanup is included, protect dirty worktrees, unmerged commits,
  active agents and unrelated/unmanaged targets. Delegate managed-workspace
  cleanup to the owning tool and require explicit confirmation of exact targets.
- Prefer a dry-run preview and recoverable actions; explain which data/history
  will be lost and how removal relates to archiving or future re-registration.

Implemented workspace cleanup: in WORK, `c` previews and confirms removal of one
verified workmux window and its linked worktree, keeping branch and review history.
Cancel is selected by default; dirty, shared, own-workbench, legacy/unmanaged,
duplicate-window, and unsupported targets are blocked. Removal is delegated to
workmux without `--force`; the registry entry is removed only after verified success.
See the README's cleanup section for safeguards and partial-failure behavior.

Still potential follow-ups: orphaned review-history pruning, stale-registration
bulk selection, archiving, and broader owner/session/duplicate-window support.
No automatic cleanup or review-history deletion is implemented.

Metadata housekeeping remains under discussion, not approved for implementation:

- Bulk unregister explicitly selected missing-resource entries, without touching
  code, branches, panes, or saved review history. Missing resources alone do not
  prove an entry is disposable; some entries should be relinked instead.
- Separately preview saved review contexts without a current matching
  registration. These may still be useful after re-registration or an ID rename.
  Deleting them loses saved review marks and diff snapshots, so any removal must
  be opt-in, explain the loss, and decide backup/recovery behavior first.
- Archiving is a possible alternative to unregistering, but its restore behavior
  and persistence model need a separate decision.

This is distinct from the already implemented workspace cleanup: metadata
housekeeping must not close windows or delete workspaces as a side effect.
