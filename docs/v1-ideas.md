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

Implemented workspace cleanup: in WORK, `c`, then `c` previews and confirms removal
of one verified workmux window and its linked worktree, keeping branch and review history.
Cancel is selected by default; dirty, shared, own-workbench, legacy/unmanaged,
duplicate-window, and unsupported targets are blocked. Removal is delegated to
workmux without `--force`; the registry entry is removed only after verified success.
See the README's cleanup section for safeguards and partial-failure behavior.

Still potential follow-ups: orphaned review-history pruning, stale-registration
bulk selection, archiving, and broader owner/session/duplicate-window support.
No automatic cleanup or review-history deletion is implemented.

Metadata housekeeping is a v1 candidate. Its exact scope remains under
discussion; implementation has not been approved:

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

### Shared removal menu (implemented), then bulk unregister (proposed)

Implemented: `c` in WORK opens a lazygit-style removal menu for the captured work
item. The menu keeps the actions distinct and discloses what each keeps/removes:

- Unregister only (`u` inside the menu): remove the registration; keep pane,
  workspace, branch, and saved review history. Apply directly from the menu,
  without a second confirmation; core revalidates the captured registration.
- Clean up workspace (`c` inside the menu): open the existing ownership/safety
  preview and cancel-first confirmation for the entire workmux window and its
  linked worktree. Keep branch and review history; unregister only after verified
  cleanup success.

The compact **Clean Up Options** menu shows the work ID and key-first action rows:
`u    Unregister work item. Keep pane and files.` and
`c    Close workmux window. Remove worktree.`, followed by `Esc  Cancel`.
The WORK action is labeled **Clean Up**. All menu controls are shown inline;
there is no help entry or `?` help behavior here, and the bottom help bar is hidden.
Workspace paths and generic warning text are omitted; workspace deletion still
has its full preview and confirmation. Warnings appear when registration changes
or load failures block an action, or when direct unregister fails.

Opening the menu alone does not remove anything. Pressing `u` unregisters its
captured target directly; the popup locks input until the operation finishes,
closes on success, and shows errors in place. The old unregister confirmation
screen was removed. Refreshes cannot retarget the operation; changed/removed
entries disable choices until reopened and load errors pause choices until recovery.
Existing cleanup restrictions and revalidation are unchanged. Blocked or failed
cleanup never falls back to unregistering. `Esc` cancels; `<enter>` does not choose
a default action. These menu controls were approved; the old standalone `u`
binding in WORK was removed.

As a separate next slice, preview registrations with confirmed missing panes or
workspace directories and let the user explicitly select which to unregister.
Discovery/access failures and unknown agent activity are not proof of absence.
Recheck registrations and resource availability before confirmation/execution,
and require renewed approval if an entry changed or its resources returned.
Do not delete workspace resources or review history in this bulk operation.

Recommend leaving archive/restore, review-history pruning, automatic removal,
bulk workspace deletion, and broader cleanup-owner support outside this first
scope. These boundaries and the bulk-selection interaction still need agreement.
