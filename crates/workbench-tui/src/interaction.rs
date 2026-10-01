use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use workbench_core::{MAX_INPUT_BYTES, WorkItemState};

#[derive(Clone, Debug)]
pub(crate) struct Draft {
    pub item_id: String,
    pub text: String,
}

impl Draft {
    pub fn append(&mut self, text: &str) -> Result<(), String> {
        if text.chars().any(char::is_control) {
            return Err("Replies must be a single line with no control characters.".into());
        }
        if self.text.len() + text.len() > MAX_INPUT_BYTES {
            return Err(format!(
                "Replies are limited to {MAX_INPUT_BYTES} UTF-8 bytes."
            ));
        }
        self.text.push_str(text);
        Ok(())
    }

    pub fn edit(&mut self, key: KeyEvent) -> Result<(), String> {
        match key.code {
            KeyCode::Backspace => {
                self.text.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.clear()
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.append(&ch.to_string())?
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Default, Debug)]
pub(crate) struct Interaction {
    pub selected_id: Option<String>,
    pub draft: Option<Draft>,
    pub message: Option<String>,
    pub sending: bool,
    pub reveal_selection: bool,
}

impl Interaction {
    pub fn sync(&mut self, items: &[WorkItemState]) {
        if self
            .selected_id
            .as_ref()
            .is_some_and(|id| items.iter().any(|state| state.item.id == *id))
        {
            return;
        }
        self.selected_id = items.first().map(|state| state.item.id.clone());
        self.reveal_selection = true;
    }

    pub fn selected<'a>(&self, items: &'a [WorkItemState]) -> Option<&'a WorkItemState> {
        let id = self.selected_id.as_ref()?;
        items.iter().find(|state| state.item.id == *id)
    }

    pub fn move_selection(&mut self, items: &[WorkItemState], delta: isize) {
        self.sync(items);
        if items.is_empty() {
            return;
        }
        let position = items
            .iter()
            .position(|state| self.selected_id.as_ref() == Some(&state.item.id))
            .unwrap_or(0);
        let next = position.saturating_add_signed(delta).min(items.len() - 1);
        self.selected_id = Some(items[next].item.id.clone());
        self.reveal_selection = true;
    }

    pub fn next_attention(&mut self, items: &[WorkItemState]) {
        let start = items
            .iter()
            .position(|state| self.selected_id.as_ref() == Some(&state.item.id))
            .map_or(0, |index| index + 1);
        let next = (0..items.len())
            .map(|offset| (start + offset) % items.len())
            .find(|index| items[*index].needs_attention());
        if let Some(index) = next {
            self.selected_id = Some(items[index].item.id.clone());
            self.reveal_selection = true;
        } else {
            self.message = Some(
                "No items are currently known to need attention. UNKNOWN is inconclusive.".into(),
            );
        }
    }

    pub fn begin_reply(&mut self, items: &[WorkItemState]) {
        if self.sending {
            return;
        }
        self.draft = self.selected(items).map(|state| Draft {
            item_id: state.item.id.clone(),
            text: String::new(),
        });
        if self.draft.is_none() {
            self.message = Some("Select a registered work item first.".into());
        } else {
            self.message = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workbench_core::{AgentStatus, PaneAvailability, WorkItem, WorkItemKind};

    fn items(ids: &[&str]) -> Vec<WorkItemState> {
        ids.iter()
            .map(|id| WorkItemState {
                item: WorkItem {
                    id: (*id).into(),
                    title: "Task".into(),
                    repository: "/work".into(),
                    workspace: "/work".into(),
                    branch: None,
                    kind: WorkItemKind::Implementation,
                    pane_id: "%14".into(),
                },
                status: AgentStatus::Unknown,
                pane: PaneAvailability::Present,
                status_detail: "Unsupported foreground command.".into(),
                attention_prompt: None,
            })
            .collect()
    }

    #[test]
    fn selection_is_by_id_and_survives_reordering_and_missing_items() {
        let mut interaction = Interaction::default();
        interaction.sync(&items(&["A", "B", "C"]));
        interaction.move_selection(&items(&["A", "B", "C"]), 1);
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        interaction.sync(&items(&["C", "B", "A"]));
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        interaction.sync(&items(&["C", "A"]));
        assert_eq!(interaction.selected_id.as_deref(), Some("C"));
        interaction.sync(&[]);
        assert!(interaction.selected_id.is_none());
        interaction.move_selection(&[], 1);
    }

    #[test]
    fn draft_target_is_not_changed_by_selection_or_refresh() {
        let mut interaction = Interaction::default();
        interaction.sync(&items(&["A", "B"]));
        interaction.begin_reply(&items(&["A", "B"]));
        interaction.move_selection(&items(&["A", "B"]), 1);
        interaction.sync(&items(&["B"]));
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "A");
    }

    #[test]
    fn unknown_status_does_not_create_attention_items() {
        let mut interaction = Interaction::default();
        interaction.sync(&items(&["A", "B"]));
        interaction.next_attention(&items(&["A", "B"]));
        assert_eq!(interaction.selected_id.as_deref(), Some("A"));
        assert!(interaction.message.unwrap().contains("UNKNOWN"));
        Interaction::default().next_attention(&[]);
    }

    #[test]
    fn tab_cycles_waiting_and_completed_items_only() {
        let mut states = items(&["A", "B", "C", "D", "E"]);
        states[0].status = AgentStatus::Running;
        states[1].status = AgentStatus::WaitingForInput;
        states[2].status = AgentStatus::Idle;
        states[3].status = AgentStatus::Complete;
        let mut interaction = Interaction::default();
        interaction.sync(&states);
        for id in ["B", "D", "B"] {
            interaction.next_attention(&states);
            assert_eq!(interaction.selected_id.as_deref(), Some(id));
        }
    }

    #[test]
    fn unicode_editing_and_paste_preserve_text_and_reject_multiline_atomically() {
        let mut draft = Draft {
            item_id: "A".into(),
            text: String::new(),
        };
        draft.append("yes λ🙂").unwrap();
        draft
            .edit(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(draft.text, "yes λ");
        assert!(draft.append("\nsubmit").is_err());
        assert_eq!(draft.text, "yes λ");
        assert!(draft.append(&"a".repeat(MAX_INPUT_BYTES)).is_err());
        draft
            .edit(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(draft.text.is_empty());
        draft
            .edit(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(draft.text, "q");
    }
}
