//! Explicit, one-shot decisions for recognized terminal approval dialogs.
use std::{error::Error, fmt};

use crate::{
    ActionError, AgentStatus, Engine, PaneAvailability, WorkItem, WorkItemState,
    actions::target_for,
    agent_state::{approval_title, current_approval_prompt},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    ApproveOnce,
    RejectAndReply,
}

/// Binds a decision to the registration and complete visible prompt the user saw.
/// Never persisted: terminal captures do not provide native approval request IDs.
#[derive(Clone, Debug)]
pub struct ApprovalRequest {
    item: WorkItem,
    prompt: String,
    decision: ApprovalDecision,
}

#[derive(Debug)]
pub enum ApprovalError {
    Action(ActionError),
    Unsupported,
    Changed,
}

impl fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Action(error) => error.fmt(f),
            Self::Unsupported => f.write_str(
                "Select a WAITING item with a supported approval choice; open the pane to inspect other prompts.",
            ),
            Self::Changed => f.write_str(
                "The registration or approval prompt changed. Refresh or open the pane before trying again.",
            ),
        }
    }
}

impl Error for ApprovalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Action(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ActionError> for ApprovalError {
    fn from(error: ActionError) -> Self {
        Self::Action(error)
    }
}

impl ApprovalRequest {
    pub fn capture(
        state: &WorkItemState,
        decision: ApprovalDecision,
    ) -> Result<Self, ApprovalError> {
        if state.status != AgentStatus::WaitingForInput || state.pane != PaneAvailability::Present {
            return Err(ApprovalError::Unsupported);
        }
        let prompt = state
            .attention_prompt
            .as_deref()
            .ok_or(ApprovalError::Unsupported)?;
        choice_key(prompt, decision).ok_or(ApprovalError::Unsupported)?;
        Ok(Self {
            item: state.item.clone(),
            prompt: prompt.into(),
            decision,
        })
    }

    pub fn item_id(&self) -> &str {
        &self.item.id
    }

    pub fn decision(&self) -> ApprovalDecision {
        self.decision
    }
}

impl Engine {
    /// Recheck the captured dialog, then send exactly one native decision key.
    /// No Enter, focus change, persistent permission grant, or automatic retry.
    /// A capture and key delivery cannot be atomic without an agent-native API.
    pub async fn respond_to_approval(
        &self,
        request: &ApprovalRequest,
    ) -> Result<(), ApprovalError> {
        if self.registered_item(request.item_id())? != request.item {
            return Err(ApprovalError::Changed);
        }
        let snapshot = self.discover().await.map_err(ActionError::from)?;
        target_for(&request.item, &snapshot)?;
        let observation = self
            .tmux
            .observe_pane(&request.item.pane_id)
            .await
            .map_err(ActionError::from)?;
        let prompt = current_approval_prompt(&observation).ok_or(ApprovalError::Changed)?;
        if prompt != request.prompt || self.registered_item(request.item_id())? != request.item {
            return Err(ApprovalError::Changed);
        }
        let key = choice_key(&prompt, request.decision).ok_or(ApprovalError::Unsupported)?;
        self.tmux
            .execute(&[
                "send-keys".into(),
                "-t".into(),
                request.item.pane_id.clone(),
                key.into(),
            ])
            .await
            .map_err(ActionError::from)?;
        Ok(())
    }
}

fn choice_key(prompt: &str, decision: ApprovalDecision) -> Option<&'static str> {
    if !approval_title(prompt.lines().next()?.trim())
        || prompt.contains("exceeded the display limit")
        || prompt
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return None;
    }
    let (_, menu) = prompt.rsplit_once("\n\nOptions:\n")?;
    let mut options = Vec::<String>::new();
    let mut numbers = std::collections::BTreeSet::new();
    let mut selected = 0;
    for line in menu.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (line, marked) = line
            .strip_prefix("› ")
            .map_or((line, false), |rest| (rest, true));
        if let Some((number, label)) = line.split_once(". ")
            && let Ok(number) = number.parse::<u8>()
        {
            if number == 0
                || !numbers.insert(number)
                || !(label.starts_with("Yes, ") || label.starts_with("No, "))
            {
                return None;
            }
            selected += usize::from(marked);
            options.push(label.into());
        } else {
            if marked {
                return None;
            }
            let label = options.last_mut()?;
            label.push(' ');
            label.push_str(line);
        }
    }
    if options.len() < 2 || selected != 1 {
        return None;
    }
    let (suffix, key) = match decision {
        ApprovalDecision::ApproveOnce => (" (y)", "y"),
        ApprovalDecision::RejectAndReply => (" (esc)", "Escape"),
    };
    // Refuse aliases/ambiguous menus too, not just two exact suffix matches.
    let mut candidates = options
        .iter()
        .filter(|label| label.contains(suffix.trim_start()));
    let label = candidates.next()?.strip_suffix(suffix)?;
    if candidates.next().is_some() {
        return None;
    }
    let supported = match decision {
        ApprovalDecision::ApproveOnce => matches!(
            label,
            "Yes, proceed"
                | "Yes, allow once"
                | "Yes, just this once"
                | "Yes, grant these permissions for this turn"
        ),
        ApprovalDecision::RejectAndReply => label == "No, and tell Codex what to do differently",
    };
    supported.then_some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WorkItemKind, tmux::PaneObservation};

    const SCREEN: &str = "Would you like to run the following command?\n\nReason: Run tests\n$ cargo test\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands that start with `cargo test` (p)\n  3. No, and tell Codex what to do differently (esc)\n\nPress enter to confirm or esc to cancel\n";

    fn state() -> WorkItemState {
        WorkItemState {
            item: WorkItem {
                id: "task".into(),
                title: "Test".into(),
                repository: "/work".into(),
                workspace: "/work".into(),
                branch: None,
                kind: WorkItemKind::Implementation,
                pane_id: "%14".into(),
            },
            status: AgentStatus::WaitingForInput,
            pane: PaneAvailability::Present,
            status_detail: "approval".into(),
            attention_prompt: current_approval_prompt(&PaneObservation {
                command: "codex".into(),
                dead: false,
                in_mode: false,
                screen: SCREEN.into(),
            }),
            completion_fingerprint: None,
        }
    }

    #[test]
    fn decisions_use_explicit_shortcuts_not_numbers_or_highlighted_choice() {
        let prompt = state().attention_prompt.unwrap();
        for prompt in [
            prompt.clone(),
            prompt.replace("› 1.", "1.").replace("  2.", "› 2."),
            prompt
                .replace(
                    "  2. Yes, and don't ask again for commands that start with `cargo test` (p)\n",
                    "",
                )
                .replace("3. No,", "2. No,"),
            prompt.replace("what to do differently", "what to do\n    differently"),
        ] {
            assert_eq!(
                choice_key(&prompt, ApprovalDecision::ApproveOnce),
                Some("y")
            );
            assert_eq!(
                choice_key(&prompt, ApprovalDecision::RejectAndReply),
                Some("Escape")
            );
        }
    }

    #[test]
    fn ambiguous_clipped_custom_or_persistent_choices_are_not_approved() {
        let prompt = state().attention_prompt.unwrap();
        for prompt in [
            prompt.replace("Yes, proceed (y)", "Yes, and don't ask again (y)"),
            prompt.replace("(p)", "(y)"),
            prompt.replace("(p)", "(p) or (y)"),
            prompt.replace("(y)", "(ctrl-y)"),
            prompt.replace("  2.", "› 2."),
            prompt.replace("  2.", "  1."),
            prompt.replace("  2.", "  0."),
            prompt.replace("› ", ""),
            format!("{prompt}\n[Options exceeded the display limit.]"),
            format!("{prompt}\x1b"),
            prompt.replace("Would you like to run the following command?", "Question?"),
        ] {
            assert_eq!(
                choice_key(&prompt, ApprovalDecision::ApproveOnce),
                None,
                "{prompt}"
            );
        }
        assert_eq!(
            choice_key(
                &prompt.replace("(esc)", "(n)"),
                ApprovalDecision::RejectAndReply
            ),
            None
        );
    }

    #[test]
    fn permission_grants_are_limited_to_the_current_turn_not_the_session() {
        let prompt = state().attention_prompt.unwrap();
        let turn = prompt.replace("Yes, proceed", "Yes, grant these permissions for this turn");
        assert_eq!(choice_key(&turn, ApprovalDecision::ApproveOnce), Some("y"));
        let session = turn.replace("for this turn", "for this session");
        assert_eq!(choice_key(&session, ApprovalDecision::ApproveOnce), None);
        let deny = turn.replace(
            "No, and tell Codex what to do differently",
            "No, continue without permissions",
        );
        assert_eq!(choice_key(&deny, ApprovalDecision::RejectAndReply), None);
    }

    #[test]
    fn capture_requires_live_waiting_approval_and_binds_the_original_target() {
        let mut state = state();
        let request = ApprovalRequest::capture(&state, ApprovalDecision::RejectAndReply).unwrap();
        state.item.id = "other".into();
        assert_eq!(request.item_id(), "task");
        for status in [
            AgentStatus::Running,
            AgentStatus::Idle,
            AgentStatus::Complete,
            AgentStatus::Unknown,
        ] {
            state.status = status;
            assert!(ApprovalRequest::capture(&state, ApprovalDecision::ApproveOnce).is_err());
        }
        state.status = AgentStatus::WaitingForInput;
        state.pane = PaneAvailability::Missing;
        assert!(ApprovalRequest::capture(&state, ApprovalDecision::ApproveOnce).is_err());
        state.pane = PaneAvailability::Present;
        state.attention_prompt = Some("Please choose a plan".into());
        assert!(ApprovalRequest::capture(&state, ApprovalDecision::ApproveOnce).is_err());
    }

    #[cfg(unix)]
    struct Fixture {
        directory: tempfile::TempDir,
        engine: Engine,
    }

    #[cfg(unix)]
    impl Fixture {
        fn new() -> Self {
            use std::{fs, os::unix::fs::PermissionsExt};
            let directory = tempfile::tempdir().unwrap();
            let executable = directory.path().join("fake-tmux");
            fs::write(
                &executable,
                format!(
                    r#"#!/bin/sh
set -eu
base='{}'
[ "$1" = '-N' ] && shift
case "$1" in
list-panes) exec /bin/cat "$base/snapshot" ;;
display-message)
  [ ! -e "$base/capture-failure" ] || exit 31
  /bin/cat "$base/metadata" "$base/screen" "$base/metadata"
  if [ -e "$base/replacement" ]; then /bin/cp "$base/replacement" "$base/state.json"; fi ;;
send-keys)
  printf '%s\n' "$@" >> "$base/keys"
  [ ! -e "$base/send-failure" ] || exit 32 ;;
*) exit 99 ;;
esac
"#,
                    directory.path().display()
                ),
            )
            .unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(
                directory.path().join("snapshot"),
                "$1\x1fmain\x1f@2\x1f0\x1fagent\x1f%14\x1f0\x1fAgent\x1fcodex\x1f/work\x1e\n",
            )
            .unwrap();
            fs::write(directory.path().join("screen"), SCREEN).unwrap();
            fs::write(
                directory.path().join("metadata"),
                "\x1ecodex\x1f0\x1f0\x1e\n",
            )
            .unwrap();
            let mut engine = Engine::new(directory.path().join("state.json"));
            engine.tmux.executable = executable.into_os_string();
            engine.register_work_item(state().item).unwrap();
            Self { directory, engine }
        }

        fn path(&self, name: &str) -> std::path::PathBuf {
            self.directory.path().join(name)
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn decisions_send_one_fixed_key_without_enter_focus_or_registry_changes() {
        for (decision, key) in [
            (ApprovalDecision::ApproveOnce, "y"),
            (ApprovalDecision::RejectAndReply, "Escape"),
        ] {
            let fixture = Fixture::new();
            let request = ApprovalRequest::capture(&state(), decision).unwrap();
            fixture.engine.respond_to_approval(&request).await.unwrap();
            assert_eq!(
                std::fs::read_to_string(fixture.path("keys")).unwrap(),
                format!("send-keys\n-t\n%14\n{key}\n")
            );
            assert_eq!(fixture.engine.work_items().unwrap(), [state().item]);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_unavailable_or_unsupported_targets_never_receive_a_decision() {
        use std::fs;
        for change in [
            "command",
            "reason",
            "choices",
            "running",
            "pane",
            "copy",
            "dead",
            "vendor",
            "capture",
            "edit",
            "remap",
            "unregister",
            "during-capture",
        ] {
            let fixture = Fixture::new();
            let request =
                ApprovalRequest::capture(&state(), ApprovalDecision::ApproveOnce).unwrap();
            match change {
                "command" => fs::write(fixture.path("screen"), SCREEN.replace("cargo test", "git push")).unwrap(),
                "reason" => fs::write(fixture.path("screen"), SCREEN.replace("Run tests", "Push changes")).unwrap(),
                "choices" => fs::write(fixture.path("screen"), SCREEN.replace("(y)", "(a)")).unwrap(),
                "running" => fs::write(fixture.path("screen"), "• Working (1s • esc to interrupt)\n› Ask Codex to do anything\n? for shortcuts\n").unwrap(),
                "pane" => fs::write(fixture.path("snapshot"), "").unwrap(),
                "copy" => fs::write(fixture.path("metadata"), "\x1ecodex\x1f0\x1f1\x1e\n").unwrap(),
                "dead" => fs::write(fixture.path("metadata"), "\x1ecodex\x1f1\x1f0\x1e\n").unwrap(),
                "vendor" => fs::write(fixture.path("metadata"), "\x1eclaude\x1f0\x1f0\x1e\n").unwrap(),
                "capture" => fs::write(fixture.path("capture-failure"), "").unwrap(),
                "edit" => { fixture.engine.update_work_item_description(&state().item, "Changed").unwrap(); }
                "unregister" => { fixture.engine.unregister_work_item(&state().item).unwrap(); }
                "remap" => {
                    fixture.engine.unregister_work_item(&state().item).unwrap();
                    let mut item = state().item;
                    item.pane_id = "%15".into();
                    fixture.engine.register_work_item(item).unwrap();
                }
                _ => {
                    let replacement = Engine::new(fixture.path("replacement"));
                    let mut item = state().item;
                    item.pane_id = "%15".into();
                    replacement.register_work_item(item).unwrap();
                }
            }
            assert!(
                fixture.engine.respond_to_approval(&request).await.is_err(),
                "{change}"
            );
            assert!(!fixture.path("keys").exists(), "{change}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_key_delivery_is_never_retried() {
        let fixture = Fixture::new();
        std::fs::write(fixture.path("send-failure"), "").unwrap();
        let request = ApprovalRequest::capture(&state(), ApprovalDecision::RejectAndReply).unwrap();
        assert!(fixture.engine.respond_to_approval(&request).await.is_err());
        assert_eq!(
            std::fs::read_to_string(fixture.path("keys")).unwrap(),
            "send-keys\n-t\n%14\nEscape\n"
        );
        assert_eq!(fixture.engine.work_items().unwrap(), [state().item]);
    }
}
