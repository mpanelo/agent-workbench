# v2 ideas

Potential follow-ups deferred beyond v1. These are not committed implementation
plans.

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
