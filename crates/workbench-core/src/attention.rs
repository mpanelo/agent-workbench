use crate::{PaneAvailability, WorkItemState};

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
        }
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
