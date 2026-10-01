# v1 ideas

Potential follow-ups for v1, not committed scope or implemented behavior.

## Clarify turn completion and acknowledge attention items

Current behavior: `COMPLETE` means the agent's turn ended, not that the work item
is finished. These items appear in ATTENTION so their output can be reviewed,
but there is no acknowledgement action; the visible completion marker keeps
bringing the item into the queue.

Consider:

- Rename the displayed status to `TURN FINISHED` (or similarly explicit wording).
- Add an explicit acknowledgement action to remove that completed turn from
  ATTENTION while leaving the work item visible in WORK.
- Keep acknowledgement separate from marking code reviewed or finishing a task.
- Bring the item back when a new turn finishes or the agent needs input, not
  merely because the same completion marker is observed on another refresh.

Before implementation, decide how acknowledgement identifies a particular turn
and whether it should survive a Workbench restart. It must not suppress a newer
completion or a new input request.

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

This is a v1 candidate only; no cleanup or automatic deletion is implemented.
