use crate::{AgentStatus, PaneAvailability, WorkItem, WorkItemState};
use std::{collections::HashMap, fmt};

/// Acknowledgements live only in this tracker, not registration/review storage.
/// Keep it for the lifetime of the supervising application (including focus).
#[derive(Debug, Default)]
pub struct AttentionTracker {
    completions: HashMap<String, TrackedCompletion>,
    generation: u64,
}

#[derive(Debug)]
struct TrackedCompletion {
    item: WorkItem,
    fingerprint: Option<u64>,
    generation: u64,
    acknowledged: bool,
    current: bool,
}

/// Captured target; a newer observed turn cannot be dismissed by an old action.
#[derive(Clone, Debug)]
pub struct CompletionAcknowledgement {
    item: WorkItem,
    fingerprint: u64,
    generation: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AttentionError {
    NotFinished,
    NoEvidence,
    Changed,
}

impl fmt::Display for AttentionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotFinished => "Only a present TURN FINISHED item can be acknowledged; input requests cannot be dismissed.",
            Self::NoEvidence => "Completion evidence is unavailable; wait for a fresh observation.",
            Self::Changed => "The observed turn changed; select the current attention item and retry.",
        })
    }
}
impl std::error::Error for AttentionError {}

fn same_target(left: &WorkItem, right: &WorkItem) -> bool {
    let mut left = left.clone();
    left.title.clone_from(&right.title);
    left == *right
}

impl AttentionTracker {
    /// Apply one successful registration/observation snapshot. On load failures,
    /// do not call this with an invented empty list: keep the tracker unchanged.
    pub fn observe(&mut self, states: &[WorkItemState]) -> Vec<WorkItemState> {
        self.completions
            .retain(|id, _| states.iter().any(|state| state.item.id == *id));
        for state in states {
            let entry = self
                .completions
                .entry(state.item.id.clone())
                .or_insert_with(|| TrackedCompletion {
                    item: state.item.clone(),
                    fingerprint: None,
                    generation: 0,
                    acknowledged: false,
                    current: false,
                });
            if !same_target(&entry.item, &state.item) {
                entry.item = state.item.clone();
                entry.fingerprint = None;
                entry.acknowledged = false;
            }
            entry.current = false;
            if state.pane == PaneAvailability::Missing
                || (state.pane == PaneAvailability::Present
                    && matches!(
                        state.status,
                        AgentStatus::Running | AgentStatus::WaitingForInput
                    ))
            {
                entry.fingerprint = None;
                entry.acknowledged = false;
            }
            if state.pane == PaneAvailability::Present
                && state.status == AgentStatus::Complete
                && let Some(fingerprint) = state.completion_fingerprint
            {
                if entry.fingerprint != Some(fingerprint) {
                    self.generation = self.generation.wrapping_add(1);
                    entry.generation = self.generation;
                    entry.fingerprint = Some(fingerprint);
                    entry.acknowledged = false;
                }
                entry.current = true;
            }
        }
        self.items(states)
    }

    /// Build the visible queue without changing registration or agent status.
    pub fn items(&self, states: &[WorkItemState]) -> Vec<WorkItemState> {
        states
            .iter()
            .filter(|state| {
                state.needs_attention()
                    && !(state.status == AgentStatus::Complete
                        && self.completions.get(&state.item.id).is_some_and(|entry| {
                            entry.acknowledged
                                && entry.current
                                && same_target(&entry.item, &state.item)
                                && entry.fingerprint == state.completion_fingerprint
                        }))
            })
            .cloned()
            .collect()
    }

    pub fn capture(
        &self,
        state: &WorkItemState,
    ) -> Result<CompletionAcknowledgement, AttentionError> {
        if state.status != AgentStatus::Complete || state.pane != PaneAvailability::Present {
            return Err(AttentionError::NotFinished);
        }
        let fingerprint = state
            .completion_fingerprint
            .ok_or(AttentionError::NoEvidence)?;
        let entry = self
            .completions
            .get(&state.item.id)
            .filter(|entry| {
                entry.current
                    && same_target(&entry.item, &state.item)
                    && entry.fingerprint == Some(fingerprint)
            })
            .ok_or(AttentionError::Changed)?;
        Ok(CompletionAcknowledgement {
            item: state.item.clone(),
            fingerprint,
            generation: entry.generation,
        })
    }

    pub fn acknowledge(
        &mut self,
        target: &CompletionAcknowledgement,
    ) -> Result<(), AttentionError> {
        let entry = self
            .completions
            .get_mut(&target.item.id)
            .filter(|entry| {
                entry.current
                    && same_target(&entry.item, &target.item)
                    && entry.fingerprint == Some(target.fingerprint)
                    && entry.generation == target.generation
            })
            .ok_or(AttentionError::Changed)?;
        entry.acknowledged = true;
        Ok(())
    }
}

/// Build an ephemeral attention queue from one observation snapshot, preserving
/// registration order. Unknown/missing/unavailable items are never presumed idle
/// or complete. This operation performs no additional discovery or persistence.
pub fn attention_items(states: &[WorkItemState]) -> Vec<WorkItemState> {
    states
        .iter()
        .filter(|state| state.needs_attention())
        .cloned()
        .collect()
}

impl WorkItemState {
    /// A usable pane and affirmative state evidence are both required.
    pub fn needs_attention(&self) -> bool {
        self.pane == PaneAvailability::Present && self.status.needs_attention()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentStatus, WorkItem, WorkItemKind};

    fn state(id: &str, status: AgentStatus, pane: PaneAvailability) -> WorkItemState {
        WorkItemState {
            item: WorkItem {
                id: id.into(),
                title: "Task".into(),
                repository: "/work".into(),
                workspace: "/work".into(),
                branch: None,
                kind: WorkItemKind::Implementation,
                pane_id: "%14".into(),
            },
            status,
            pane,
            status_detail: "Observed locally.".into(),
            attention_prompt: None,
            completion_fingerprint: None,
        }
    }

    #[test]
    fn acknowledgements_hide_only_one_observed_turn_and_never_change_work() {
        let mut finished = state("A", AgentStatus::Complete, PaneAvailability::Present);
        finished.completion_fingerprint = Some(14);
        let waiting = state("B", AgentStatus::WaitingForInput, PaneAvailability::Present);
        let states = vec![finished.clone(), waiting.clone()];
        let mut tracker = AttentionTracker::default();
        assert_eq!(tracker.observe(&states), states);
        let captured = tracker.capture(&finished).unwrap();
        tracker.acknowledge(&captured).unwrap();
        tracker.acknowledge(&captured).unwrap();
        for _ in 0..3 {
            assert_eq!(tracker.observe(&states), std::slice::from_ref(&waiting));
        }
        assert_eq!(states[0], finished);
        assert_eq!(
            tracker.capture(&waiting).unwrap_err(),
            AttentionError::NotFinished
        );
        let mut renamed = finished.clone();
        renamed.item.title = "Updated description".into();
        assert!(tracker.observe(&[renamed.clone()]).is_empty());
        assert_eq!(
            AttentionTracker::default().observe(&[renamed.clone()]),
            [renamed]
        );
        tracker.observe(&[]);
        assert_eq!(tracker.acknowledge(&captured), Err(AttentionError::Changed));
        assert_eq!(tracker.observe(&[finished.clone()]), [finished]);
    }

    #[test]
    fn unknown_idle_and_unavailable_do_not_repeat_a_dismissed_turn() {
        let mut finished = state("A", AgentStatus::Complete, PaneAvailability::Present);
        finished.completion_fingerprint = Some(14);
        let mut tracker = AttentionTracker::default();
        tracker.observe(&[finished.clone()]);
        let captured = tracker.capture(&finished).unwrap();
        tracker.acknowledge(&captured).unwrap();
        for (status, pane) in [
            (AgentStatus::Unknown, PaneAvailability::Present),
            (AgentStatus::Idle, PaneAvailability::Present),
            (AgentStatus::Unknown, PaneAvailability::Unavailable),
        ] {
            let mut uncertain = finished.clone();
            uncertain.status = status;
            uncertain.pane = pane;
            uncertain.completion_fingerprint = None;
            assert!(tracker.observe(&[uncertain]).is_empty());
            assert_eq!(tracker.acknowledge(&captured), Err(AttentionError::Changed));
            assert!(tracker.observe(&[finished.clone()]).is_empty());
        }
    }

    #[test]
    fn new_activity_permissions_evidence_and_bindings_invalidate_old_actions() {
        let mut finished = state("A", AgentStatus::Complete, PaneAvailability::Present);
        finished.completion_fingerprint = Some(14);
        for transition in [AgentStatus::Running, AgentStatus::WaitingForInput] {
            let mut tracker = AttentionTracker::default();
            tracker.observe(&[finished.clone()]);
            let captured = tracker.capture(&finished).unwrap();
            tracker.acknowledge(&captured).unwrap();
            let mut next = finished.clone();
            next.status = transition;
            next.completion_fingerprint = None;
            let queue = tracker.observe(&[next.clone()]);
            assert_eq!(
                queue.len(),
                usize::from(transition == AgentStatus::WaitingForInput)
            );
            assert_eq!(tracker.observe(&[finished.clone()]), [finished.clone()]);
            // Same fingerprint after observed activity is a new generation.
            assert_eq!(tracker.acknowledge(&captured), Err(AttentionError::Changed));
        }
        for change in 0..5 {
            let mut tracker = AttentionTracker::default();
            tracker.observe(&[finished.clone()]);
            let captured = tracker.capture(&finished).unwrap();
            tracker.acknowledge(&captured).unwrap();
            let mut next = finished.clone();
            match change {
                0 => next.completion_fingerprint = Some(15),
                1 => next.item.pane_id = "%99".into(),
                2 => next.item.workspace = "/other".into(),
                3 => next.item.branch = Some("other".into()),
                _ => {
                    next.pane = PaneAvailability::Missing;
                    next.status = AgentStatus::Unknown;
                    next.completion_fingerprint = None;
                }
            }
            let queue = tracker.observe(&[next]);
            assert_eq!(queue.len(), usize::from(change < 4));
            assert_eq!(tracker.acknowledge(&captured), Err(AttentionError::Changed));
            assert_eq!(tracker.observe(&[finished.clone()]), [finished.clone()]);
        }
        let mut missing_evidence = finished;
        missing_evidence.completion_fingerprint = None;
        let mut tracker = AttentionTracker::default();
        assert_eq!(
            tracker.observe(&[missing_evidence.clone()]),
            [missing_evidence.clone()]
        );
        assert_eq!(
            tracker.capture(&missing_evidence).unwrap_err(),
            AttentionError::NoEvidence
        );
    }

    #[test]
    fn queue_contains_only_live_waiting_and_complete_items_in_stable_order() {
        let states = vec![
            state("running", AgentStatus::Running, PaneAvailability::Present),
            state("complete", AgentStatus::Complete, PaneAvailability::Present),
            state("idle", AgentStatus::Idle, PaneAvailability::Present),
            state(
                "waiting",
                AgentStatus::WaitingForInput,
                PaneAvailability::Present,
            ),
            state("unknown", AgentStatus::Unknown, PaneAvailability::Present),
            state("missing", AgentStatus::Complete, PaneAvailability::Missing),
            state(
                "unavailable",
                AgentStatus::WaitingForInput,
                PaneAvailability::Unavailable,
            ),
        ];
        let queue = attention_items(&states);
        assert_eq!(
            queue
                .iter()
                .map(|state| state.item.id.as_str())
                .collect::<Vec<_>>(),
            ["complete", "waiting"]
        );
        assert_eq!(queue[0], states[1]);
        assert_eq!(queue[1], states[3]);
    }

    #[test]
    fn refresh_removes_resolved_and_uncertain_items_without_mutating_registrations() {
        let mut states = vec![
            state("A", AgentStatus::WaitingForInput, PaneAvailability::Present),
            state("B", AgentStatus::Complete, PaneAvailability::Present),
        ];
        states[0].attention_prompt = Some("Approve this command?".into());
        assert_eq!(
            attention_items(&states)[0].attention_prompt,
            states[0].attention_prompt
        );
        states[0].status = AgentStatus::Running;
        states[1].status = AgentStatus::Unknown;
        assert!(attention_items(&states).is_empty());
        assert_eq!(states.len(), 2);
        assert!(attention_items(&[]).is_empty());
    }
}
