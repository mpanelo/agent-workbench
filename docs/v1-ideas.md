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
