use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use workbench_core::{
    ActionError, ApprovalDecision, ApprovalError, ApprovalRequest, MAX_INPUT_BYTES, Snapshot,
    WorkItemState,
};

/// Cursor measured in Unicode characters before the end. A default cursor is
/// at the end, including for prefilled fields; all edits stay on UTF-8 boundaries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InputCursor {
    from_end: usize,
}

impl InputCursor {
    pub fn byte_index(self, text: &str) -> usize {
        let position = text.chars().count().saturating_sub(self.from_end);
        text.char_indices()
            .nth(position)
            .map_or(text.len(), |(index, _)| index)
    }

    pub fn column(self, text: &str) -> u16 {
        ratatui::text::Line::from(crate::ui::visible(&text[..self.byte_index(text)]))
            .width()
            .min(usize::from(u16::MAX)) as u16
    }

    fn clamp(&mut self, text: &str) {
        self.from_end = self.from_end.min(text.chars().count());
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Draft {
    pub item_id: String,
    pub text: String,
    pub cursor: InputCursor,
}

impl Draft {
    pub fn insert(&mut self, text: &str) -> Result<(), String> {
        if text.chars().any(char::is_control) {
            return Err("Replies must be a single line with no control characters.".into());
        }
        if self.text.len() + text.len() > MAX_INPUT_BYTES {
            return Err(format!(
                "Replies are limited to {MAX_INPUT_BYTES} UTF-8 bytes."
            ));
        }
        self.cursor.clamp(&self.text);
        self.text
            .insert_str(self.cursor.byte_index(&self.text), text);
        Ok(())
    }

    pub fn edit(&mut self, key: KeyEvent) -> Result<(), String> {
        match key.code {
            KeyCode::Backspace => {
                self.cursor.clamp(&self.text);
                let position = self.cursor.byte_index(&self.text);
                if let Some((previous, _)) = self.text[..position].char_indices().next_back() {
                    self.text.replace_range(previous..position, "");
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.clear();
                self.cursor = InputCursor::default();
            }
            KeyCode::Left | KeyCode::Right
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.cursor.clamp(&self.text);
                self.cursor.from_end = if key.code == KeyCode::Left {
                    self.cursor
                        .from_end
                        .saturating_add(1)
                        .min(self.text.chars().count())
                } else {
                    self.cursor.from_end.saturating_sub(1)
                };
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert(&ch.to_string())?
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Default, Debug)]
pub(crate) struct Interaction {
    pub selected_id: Option<String>,
    pub work_list_offset: usize,
    pub detail_item_id: Option<String>,
    pub draft: Option<Draft>,
    pub message: Option<String>,
    pub sending: bool,
    pub reveal_selection: bool,
    pub review_requested: Option<String>,
    pub selected_pane: Option<String>,
    pub show_all_panes: bool,
    pub reveal_pane: bool,
    pub registration_requested: Option<String>,
    pub maintenance_requested: Option<crate::maintenance::Request>,
    pub cleanup_requested: Option<workbench_core::WorkItem>,
    pub attention_tracker: workbench_core::AttentionTracker,
    pub acknowledgement_requested: Option<workbench_core::CompletionAcknowledgement>,
    pub approval_requested: Option<ApprovalRequest>,
}

impl Interaction {
    pub fn finish_approval(
        &mut self,
        request: &ApprovalRequest,
        result: Result<(), ApprovalError>,
    ) {
        self.sending = false;
        self.message = Some(match result {
            Ok(()) if request.decision() == ApprovalDecision::RejectAndReply => {
                // The queue may have refreshed or moved while the key was sent.
                // Reply to the captured item, never the newly selected row.
                self.draft = Some(Draft {
                    item_id: request.item_id().into(),
                    text: String::new(),
                    cursor: InputCursor::default(),
                });
                format!(
                    "Rejected request for {}. Tell the agent what to do differently.",
                    request.item_id()
                )
            }
            Ok(()) => format!("Approved once for {}.", request.item_id()),
            Err(error) => format!(
                "Decision for {} failed: {error} Not retried; inspect the pane before retrying.",
                request.item_id()
            ),
        });
    }

    pub fn finish_focus(&mut self, id: &str, result: Result<(), ActionError>) {
        // Successful navigation needs no persistent notice; clear stale messages.
        self.message = result
            .err()
            .map(|error| format!("Could not open {id}: {error}"));
        self.reveal_selection = true;
    }

    pub fn sync_panes(&mut self, snapshot: &Snapshot) {
        let visible = self.sessions_snapshot(snapshot);
        let ids = pane_ids(&visible);
        if self
            .selected_pane
            .as_deref()
            .is_some_and(|id| ids.contains(&id))
        {
            return;
        }
        self.selected_pane = ids.first().map(|id| (*id).to_owned());
        self.reveal_pane = true;
    }

    pub fn move_pane_selection(&mut self, snapshot: &Snapshot, delta: isize) {
        self.sync_panes(snapshot);
        let visible = self.sessions_snapshot(snapshot);
        let ids = pane_ids(&visible);
        if ids.is_empty() {
            return;
        }
        let position = ids
            .iter()
            .position(|id| Some(*id) == self.selected_pane.as_deref())
            .unwrap_or(0);
        self.selected_pane =
            Some(ids[position.saturating_add_signed(delta).min(ids.len() - 1)].to_owned());
        self.reveal_pane = true;
    }

    pub fn sessions_snapshot(&self, snapshot: &Snapshot) -> Snapshot {
        if self.show_all_panes {
            snapshot.clone()
        } else {
            snapshot.coding_agent_panes()
        }
    }

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
            cursor: InputCursor::default(),
        });
        if self.draft.is_none() {
            self.message = Some("Select a registered work item first.".into());
        } else {
            self.message = None;
        }
    }
}

fn pane_ids(snapshot: &Snapshot) -> Vec<&str> {
    let mut ids = Vec::new();
    for pane in snapshot
        .sessions
        .iter()
        .flat_map(|session| &session.windows)
        .flat_map(|window| &window.panes)
    {
        if !ids.contains(&pane.id.as_str()) {
            ids.push(pane.id.as_str());
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use workbench_core::{AgentStatus, PaneAvailability, WorkItem, WorkItemKind};

    #[test]
    fn successful_focus_clears_notices_but_failures_remain_visible() {
        let mut interaction = Interaction {
            selected_id: Some("ABC-123".into()),
            message: Some("An old notice".into()),
            ..Interaction::default()
        };
        interaction.finish_focus("ABC-123", Ok(()));
        assert!(interaction.message.is_none());
        assert!(interaction.reveal_selection);
        assert_eq!(interaction.selected_id.as_deref(), Some("ABC-123"));

        interaction.finish_focus(
            "ABC-123",
            Err(ActionError::MissingPane {
                item: "ABC-123".into(),
                pane: "%14".into(),
            }),
        );
        let error = interaction.message.as_deref().unwrap();
        assert!(error.starts_with("Could not open ABC-123:"));
        assert!(error.contains("Pane %14"));
        assert!(error.contains("registration is preserved"));

        interaction.finish_focus("ABC-123", Ok(()));
        assert!(interaction.message.is_none());
    }

    #[test]
    fn pane_selection_survives_refresh_and_skips_linked_duplicates() {
        use workbench_core::{Pane, Session, Window};
        let mut snapshot = Snapshot {
            sessions: vec![Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![Window {
                    id: "@1".into(),
                    index: 0,
                    name: "task".into(),
                    panes: ["%1", "%2", "%3"]
                        .into_iter()
                        .map(|id| Pane {
                            id: id.into(),
                            index: 0,
                            title: "Agent".into(),
                            current_command: Some("codex".into()),
                            working_directory: None,
                        })
                        .collect(),
                }],
            }],
        };
        snapshot.sessions.push(snapshot.sessions[0].clone());
        let mut interaction = Interaction::default();
        interaction.sync_panes(&snapshot);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%1"));
        interaction.move_pane_selection(&snapshot, 1);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%2"));
        snapshot.sessions[0].windows[0].panes.reverse();
        interaction.sync_panes(&snapshot);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%2"));
        interaction.move_pane_selection(&snapshot, 20);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%1"));
        interaction.sync_panes(&Snapshot::default());
        assert!(interaction.selected_pane.is_none());
        interaction.move_pane_selection(&Snapshot::default(), 1);
    }

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
                completion_fingerprint: None,
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
            cursor: InputCursor::default(),
        };
        draft.insert("yes λ🙂").unwrap();
        draft
            .edit(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(draft.text, "yes λ");
        assert!(draft.insert("\nsubmit").is_err());
        assert_eq!(draft.text, "yes λ");
        assert!(draft.insert(&"a".repeat(MAX_INPUT_BYTES)).is_err());
        draft
            .edit(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(draft.text, "yes λ");
        draft
            .edit(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(draft.text.is_empty());
        draft
            .edit(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(draft.text, "q");
    }

    #[test]
    fn arrows_insert_paste_and_backspace_at_utf8_boundaries() {
        let mut draft = Draft {
            item_id: "A".into(),
            text: "aλ🙂z".into(),
            cursor: InputCursor::default(),
        };
        let edit =
            |draft: &mut Draft, code| draft.edit(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
        edit(&mut draft, KeyCode::Left);
        assert_eq!(draft.cursor.byte_index(&draft.text), "aλ🙂".len());
        edit(&mut draft, KeyCode::Left);
        assert_eq!(draft.cursor.column(&draft.text), 2);
        edit(&mut draft, KeyCode::Char('X'));
        assert_eq!(draft.text, "aλX🙂z");
        edit(&mut draft, KeyCode::Backspace);
        assert_eq!(draft.text, "aλ🙂z");
        edit(&mut draft, KeyCode::Right);
        draft.insert("YZ").unwrap();
        assert_eq!(draft.text, "aλ🙂YZz");
        assert_eq!(draft.cursor.byte_index(&draft.text), "aλ🙂YZ".len());
        for _ in 0..20 {
            edit(&mut draft, KeyCode::Left);
        }
        assert_eq!(draft.cursor.byte_index(&draft.text), 0);
        edit(&mut draft, KeyCode::Backspace);
        assert_eq!(draft.text, "aλ🙂YZz");
        draft.insert("Q").unwrap();
        assert_eq!(draft.text, "Qaλ🙂YZz");
        assert_eq!(draft.cursor.byte_index(&draft.text), 1);
        for _ in 0..20 {
            edit(&mut draft, KeyCode::Right);
        }
        assert_eq!(draft.cursor.byte_index(&draft.text), draft.text.len());
    }

    #[test]
    fn rejected_input_and_modified_arrows_preserve_cursor_and_clear_resets_it() {
        let mut draft = Draft {
            item_id: "A".into(),
            text: "λ🙂".into(),
            cursor: InputCursor::default(),
        };
        draft
            .edit(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        let cursor = draft.cursor;
        assert!(draft.insert("\ninvalid").is_err());
        assert!(draft.insert(&"x".repeat(MAX_INPUT_BYTES)).is_err());
        for modifier in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
        ] {
            draft.edit(KeyEvent::new(KeyCode::Left, modifier)).unwrap();
            draft.edit(KeyEvent::new(KeyCode::Right, modifier)).unwrap();
        }
        assert_eq!(draft.text, "λ🙂");
        assert_eq!(draft.cursor, cursor);
        draft
            .edit(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(draft.text.is_empty());
        assert_eq!(draft.cursor, InputCursor::default());
        draft.insert("fresh").unwrap();
        assert_eq!(draft.cursor.byte_index(&draft.text), 5);
    }
}
