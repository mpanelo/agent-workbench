# v2 ideas

Potential follow-ups deferred beyond v1. These are not committed implementation
plans.

## Attention age

Show how long a current input request has been waiting or how recently an agent
turn finished, without reordering work items or changing selection. Define which
events establish/reset the timestamp and distinguish first observation from a
verified agent event time. Decide restart behavior and privacy/retention before
implementation. Deferred from the v1 shortlist.

## Claude lifecycle-status support

Investigate reliable Claude activity, input-request, and turn-completion signals,
with explicit source validation and conservative unavailable/unknown behavior.
Recognizing a Claude pane is not equivalent to supervising its lifecycle. Define
supported versions, setup, event boundaries, and tests before implementation;
do not assume all agents share Codex's hooks or permission controls. Deferred
from the v1 shortlist; no agent configuration is changed by this backlog item.

## Complete agent-response viewing in WORK

Show the selected work item's complete latest agent response in a read-only,
scrollable detail area, especially after a turn finishes. Keep `<enter>` opening
the existing agent pane for interaction. Preserve the stable work list, selection,
reading position, and response formatting.

A partial visible-pane capture is not sufficient: it may omit the conclusion,
caveats, or follow-up question. Investigate reliable, agent-specific structured
message sources before implementing this feature. Do not claim a capture is a
complete response or silently substitute a truncated preview.

Decide source support, response boundaries, size limits, privacy, retention, and
missing-source behavior before implementation. Workbench does not currently
discover transcripts or retain assistant responses. Any such access or persistence
must be explicitly scoped; no model calls or terminal emulator are proposed.

Tmux popups were discussed as an alternative interaction surface but are not
required for this feature. Any future popup experiment needs separate lifecycle
and layout investigation and approval of its controls before implementation.
