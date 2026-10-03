# v1 ideas

Potential follow-ups for v1, plus completed ideas retained for context.

## Clarify turn completion and acknowledge attention items

Implemented: the displayed status is `TURN FINISHED` (core enum remains
`AgentStatus::Complete`). In ATTENTION, `x` acknowledges the selected finished
observation without changing the work item or code-review state. Input requests
cannot be dismissed. Repeated captures of the acknowledged completion stay out
of the queue; new observed activity/input or changed completion evidence requeues it.

Chosen scope: session-only acknowledgements, retained across view switches and
pane focus but reset on Workbench exit. The core tracker uses visible completion
fingerprints and observation generations, not native agent turn IDs. Uncertain
observations alone do not clear an acknowledgement. See the README's attention
section for viewport and identical-fast-turn limitations.

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
