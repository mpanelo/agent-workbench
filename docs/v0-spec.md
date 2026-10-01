# Agent Workbench — v0 Product Specification

## Overview

Agent Workbench is a local-first tool for developers supervising multiple coding agents in parallel.

Modern coding agents make it possible for one developer to have several independent tasks progressing simultaneously.

The bottleneck increasingly becomes the developer's ability to:

- know what each agent is doing
- notice when an agent needs attention
- respond quickly
- move between tasks without remembering terminal locations
- review large amounts of agent-generated code
- re-review only code that changed after feedback

Agent Workbench is intended to provide that control surface.

## Current Workflow

The initial workflow being optimized is roughly:

```text
Create Jira task
      ↓
Open Alacritty
      ↓
Navigate existing tmux environment
      ↓
Create task workspace using workmux
      ↓
workmux creates windows/worktree
      ↓
Start coding agent
      ↓
Repeat for multiple independent tasks
```

Typically 4–6 coding-agent sessions may be active at the same time.

Navigation currently involves switching among many tmux/workmux windows using keyboard shortcuts such as:

```text
Cmd+1
Cmd+2
Cmd+3
...
```

When the number of sessions becomes difficult to manage, additional work may spill into other applications.

A separate class of work also exists:

```text
Review someone else's pull request
      ↓
Create temporary review workspace
      ↓
Inspect diff
      ↓
Possibly use an agent to assist
      ↓
Perform human review
```

These external review tasks do not necessarily have associated Jira issues.

Therefore, Jira must not be the fundamental object in the data model.

## Core Product Model

The top-level object is a:

```text
Work Item
```

There are initially two types.

### Implementation

Example:

```text
ABC-123 — Fix telemetry retry handling
```

Typical lifecycle:

```text
Task
 ↓
Workspace
 ↓
Agent implementation
 ↓
Human review
 ↓
Agent corrections
 ↓
Human re-review
```

### External Review

Example:

```text
PR #1842 — Refactor authentication middleware
```

Typical lifecycle:

```text
Pull request
 ↓
Temporary workspace
 ↓
Human/agent analysis
 ↓
Human code review
```

No Jira task is required.

## Product Thesis

The product should optimize:

```text
human attention
```

rather than:

```text
terminal management
```

tmux already manages terminal sessions.

workmux already creates isolated workspaces.

Git already manages source history.

Agent Workbench should answer:

> What requires my attention right now?

and:

> What code have I already reviewed, and what changed afterward?

## v0 Goals

v0 should validate two primary hypotheses.

### Hypothesis 1

A developer can supervise 4–6 coding agents more efficiently through a centralized attention-oriented interface than by navigating individual tmux windows.

### Hypothesis 2

Persistent review state significantly reduces the effort required to verify agent-generated changes.

## v0 Architecture

```text
                  ┌──────────────┐
                  │ Ratatui TUI  │
                  └───────┬──────┘
                          │
                  ┌───────▼──────┐
                  │ Rust Engine  │
                  └───────┬──────┘
                     ┌────┴────┐
                     ↓         ↓
                   tmux       Git
```

The Rust engine must remain presentation-independent.

A native SwiftUI frontend may use the same engine in the future.

## v0 Milestones

### M1 — Tmux Discovery

Goal:

Prove the architecture end-to-end.

The engine discovers tmux state and the TUI renders it.

Discover:

- sessions
- windows
- panes
- pane IDs
- pane titles
- current command
- working directory where available

Example:

```text
SESSIONS

main
  1  telemetry        codex
  2  auth             codex
  3  tests            zsh

reviews
  1  PR-1842          codex
```

No agent intelligence is required.

Success criteria:

- core enumerates tmux state
- TUI displays it
- no tmux-specific logic exists in TUI code
- useful error shown when tmux is unavailable

---

### M2 — Work Items

Allow a tmux pane/workspace to be registered as a work item.

Initially manual registration is acceptable.

Example:

```text
ABC-123
Type: Implementation
Repo: telemetry-service
Workspace: ~/work/ABC-123
Pane: %14
```

or:

```text
PR #1842
Type: External Review
Repo: auth-service
Workspace: ~/reviews/1842
Pane: %21
```

Display:

```text
WORK

ABC-123      UNKNOWN
ABC-142      UNKNOWN
ABC-155      UNKNOWN
PR #1842     UNKNOWN
```

Success criteria:

- work items survive application restart
- work item can map to tmux pane
- pane can disappear without crashing application

---

### M3 — Navigation and Input

Provide fast navigation to a work item.

Required actions:

```text
Enter     Open/focus work item
j/k       Move selection
Tab       Next item requiring attention
```

The application should also allow sending text to the associated tmux pane.

Conceptual operation:

```text
send_agent_input(work_item, "yes, preserve backwards compatibility")
```

Initial tmux implementation may use:

```text
tmux send-keys
```

Success criteria:

- developer can send agent input without manually locating its tmux window
- developer can quickly jump into the full terminal session when necessary

---

### M4 — Basic Agent State

Infer coarse state for supported agents.

Initial states:

```text
RUNNING
WAITING_FOR_INPUT
IDLE
COMPLETE
UNKNOWN
```

Detection may initially rely on:

- terminal output
- process state
- known prompt patterns
- agent-local state/transcript files

Perfect detection is not required.

False certainty is worse than `UNKNOWN`.

Success criteria:

- supported agent can reliably identify common waiting states
- unknown agents remain usable as ordinary work items

---

### M5 — Attention Queue

Add a dedicated attention view.

Example:

```text
ATTENTION — 3

ABC-123
Agent asks:
"Should the retry include HTTP 429?"

ABC-155
Agent completed.
12 files changed.

PR #1842
Review workspace changed.
```

Actions:

```text
r       reply
Enter   open
d       review diff
j/k     navigate
```

This should become the primary supervision workflow.

Success criteria:

A user supervising several agents does not need to repeatedly inspect every session to discover whether it needs input.

---

### M6 — Git Diff Review

Add a local diff-review interface.

Initial functionality:

- list changed files
- render unified or split diff
- navigate files
- mark file reviewed/unreviewed
- display review progress

Example:

```text
ABC-155

11 files changed
+428 -117

✓ scheduler.go
✓ cache.go
● retry.go
○ retry_test.go
○ config.go

6 / 11 reviewed
```

No AI review.

No GitHub API.

No inline PR comments.

Success criteria:

The built-in review flow is more useful for agent-generated work than manually running `git diff`.

---

### M7 — Persistent Review State

Store the revision/snapshot associated with review state.

Track:

```text
reviewed
unreviewed
changed-after-review
```

Eventually track hunks, but file-level tracking is sufficient for the first implementation if hunk tracking would significantly delay validation.

Example:

```text
✓ scheduler.go
⚠ retry.go        changed after review
○ retry_test.go
```

Success criteria:

Review state survives process restart.

---

### M8 — Changed Since Review

This is a core differentiator.

Scenario:

```text
Agent produces 14 files / 782 changed lines
        ↓
Developer reviews everything
        ↓
Developer requests corrections
        ↓
Agent edits 3 files / 48 lines
```

The application should show:

```text
RE-REVIEW REQUIRED

retry.go
  19 lines changed since review

retry_test.go
  24 lines changed since review

config.go
  5 lines changed since review
```

The developer should not need to manually determine what changed after their previous review.

Success criteria:

The application can construct a meaningful diff between the developer's review snapshot and the current workspace.

## Primary TUI Views

v0 should aim for three primary views.

### Work

Example:

```text
WORK

NEEDS ATTENTION
ABC-123       Waiting for input
ABC-155       Ready for review

WORKING
ABC-142       Agent running
ABC-161       Tests running

REVIEWING
PR #1842      6 / 11 files reviewed
```

### Attention

Example:

```text
ATTENTION

ABC-123
"Should I modify the schema?"

[r] Reply
[Enter] Open

ABC-155
Agent finished.
9 files changed.

[d] Review
```

### Review

Example:

```text
ABC-155 — REVIEW

6 / 11 files reviewed

✓ worker.go
✓ scheduler.go
⚠ retry.go
○ retry_test.go
```

## Keyboard Philosophy

The TUI should be usable almost entirely without a mouse.

Potential mappings:

```text
j / ↓       next
k / ↑       previous
Enter       open
Tab         next attention item
r           reply
d           diff/review
Space       mark reviewed
Esc         back
q           quit
```

Avoid complex prefix sequences.

The point of the product is to reduce navigation friction.

## Persistence

v0 requires local persistence for:

- work items
- tmux pane mapping
- work-item type
- review state
- review snapshots

The exact persistence implementation is intentionally unspecified.

Possible options include:

- SQLite
- lightweight local serialized state

Choose the simplest solution once the shape of the data becomes clearer.

Do not add cloud persistence.

## Agent Support

Initial development may target one coding agent.

Architecture must avoid deeply coupling the entire application to a single vendor.

A future adapter model should allow:

```text
Codex
Claude Code
Gemini CLI
other PTY-based coding agents
```

Agent support should degrade gracefully:

```text
known agent
    → richer status detection

unknown agent
    → ordinary tmux-backed work item
```

## Non-Goals

v0 will not provide:

- Jira synchronization
- GitHub synchronization
- GitLab synchronization
- PR creation
- PR comments
- cloud execution
- agent hosting
- accounts
- authentication
- subscription billing
- team collaboration
- LLM inference
- AI-generated reviews
- automatic workspace creation
- workmux replacement
- terminal emulation
- editor functionality
- native macOS UI
- remote mobile access

## Design Principles

### Local First

Core functionality should operate entirely on the developer's machine.

### Agent Agnostic

Agent vendors will change.

Do not make the product depend on one vendor's existence.

### Human Review Remains Central

The product should make humans faster at understanding generated code.

It should not simply ask another model whether generated code is correct.

### Preserve Existing Tools

Prefer integrating with:

```text
tmux
workmux
Git
Neovim
existing coding agents
```

rather than replacing them.

### Attention Over Sessions

Users should think:

```text
ABC-123 needs me
```

not:

```text
session foo, window 7, pane 2 needs me
```

### Graceful Uncertainty

When the engine cannot determine state confidently:

```text
UNKNOWN
```

is acceptable.

Do not invent state.

## v0 Completion Definition

v0 is successful when the following workflow is reliable:

1. Developer creates several workmux workspaces.
2. Agent Workbench represents them as work items.
3. Each work item maps to its agent session.
4. The Workbench shows which sessions need attention.
5. Developer can reply to an agent or jump to its terminal quickly.
6. Agent completion produces a reviewable local diff.
7. Developer can track review progress.
8. Agent can make corrections.
9. Workbench identifies only the code requiring re-review.

The practical benchmark is:

> A developer should be able to supervise approximately 4–6 concurrent coding-agent workspaces without remembering tmux window numbers or repeatedly rediscovering review state.
