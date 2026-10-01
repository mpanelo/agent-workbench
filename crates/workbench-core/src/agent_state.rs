//! Conservative, ephemeral terminal observations; no transcript discovery.

use std::collections::{HashMap, HashSet};

use tokio::task::JoinSet;

use crate::tmux::PaneObservation;
use crate::{AgentStatus, Engine, PaneAvailability, Snapshot, WorkItemError, WorkItemState};

const MAX_CONCURRENT_CAPTURES: usize = 4;
const MAX_ACTIVITY_ROWS: usize = 10;
const MAX_APPROVAL_CHARS: usize = 16 * 1024;
const APPROVAL_CLIPPED: &str =
    "\n[Approval dialog exceeded the display limit. Press Enter to inspect the full pane.]";

impl Engine {
    /// Resolve registrations and infer status from live, read-only pane observations.
    /// Every refresh replaces previous evidence. An individual capture failure does
    /// not fail the list or remove the registration. Dynamic status is never saved.
    pub async fn observe_work_item_states(
        &self,
        snapshot: Option<&Snapshot>,
    ) -> Result<Vec<WorkItemState>, WorkItemError> {
        let mut states = self.work_item_states(snapshot)?;
        let Some(snapshot) = snapshot else {
            return Ok(states);
        };
        let panes: HashMap<_, _> = snapshot
            .sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes)
            .map(|pane| (pane.id.as_str(), pane))
            .collect();
        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        for state in &mut states {
            if state.pane != PaneAvailability::Present {
                continue;
            }
            let command = panes
                .get(state.item.pane_id.as_str())
                .and_then(|pane| pane.current_command.as_deref());
            if !supported(command) {
                state.status_detail = "Unsupported or unavailable foreground command.".into();
            } else if seen.insert(state.item.pane_id.clone()) {
                targets.push(state.item.pane_id.clone());
            }
        }
        let mut captures = JoinSet::new();
        let mut observations = HashMap::new();
        let mut targets = targets.into_iter();
        loop {
            while captures.len() < MAX_CONCURRENT_CAPTURES {
                let Some(id) = targets.next() else { break };
                let tmux = self.tmux.clone();
                captures.spawn(async move {
                    let result = tmux.observe_pane(&id).await;
                    (id, result)
                });
            }
            let Some(result) = captures.join_next().await else {
                break;
            };
            if let Ok((id, result)) = result {
                let detected = match result {
                    Ok(observation) => {
                        let (status, detail) = infer(&observation);
                        (status, detail, approval_prompt(&observation, status))
                    }
                    Err(_) => (
                        AgentStatus::Unknown,
                        "Pane capture failed or changed; retrying next refresh.",
                        None,
                    ),
                };
                observations.insert(id, detected);
            }
        }
        for state in &mut states {
            if let Some((status, detail, prompt)) = observations.get(&state.item.pane_id) {
                state.status = *status;
                state.status_detail = (*detail).into();
                state.attention_prompt.clone_from(prompt);
            }
        }
        Ok(states)
    }
}

fn supported(command: Option<&str>) -> bool {
    command.is_some_and(|command| {
        std::path::Path::new(command)
            .file_name()
            .is_some_and(|name| name == "codex")
    })
}

fn infer(observation: &PaneObservation) -> (AgentStatus, &'static str) {
    use AgentStatus::*;
    if observation.dead || observation.in_mode || !supported(Some(&observation.command)) {
        return (
            Unknown,
            "Pane is dead, in copy/view mode, or no longer running a supported agent.",
        );
    }
    // tmux capture-pane without -e supplies plain visible text, not ANSI bytes.
    // Unexpected controls are inconclusive rather than silently stripped.
    if observation
        .screen
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return (Unknown, "Pane contains unexpected control characters.");
    }
    let lines: Vec<_> = observation
        .screen
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let Some(last) = lines.last() else {
        return (Unknown, "Pane has no recognizable agent UI.");
    };
    // Require a current dialog footer, a known title, and a selected Yes/No
    // choice. A quoted/historical approval question alone is not evidence.
    let confirm = approval_footer(last);
    if confirm {
        let tail = &lines[lines.len().saturating_sub(40)..];
        let title = tail.iter().any(|line| approval_title(line));
        let choice = tail.iter().any(|line| {
            line.strip_prefix("› ")
                .and_then(|choice| choice.split_once(". "))
                .is_some_and(|(number, text)| {
                    number.parse::<u8>().is_ok()
                        && (text.starts_with("Yes, ") || text.starts_with("No, "))
                })
        });
        if title && choice {
            return (
                WaitingForInput,
                "Codex approval dialog is awaiting a decision.",
            );
        }
        return (Unknown, "Unrecognized confirmation dialog.");
    }
    // Some active layouts hide the composer/shortcuts, but retain a rate-limit
    // footer immediately after the activity row. Require both signals: a lone
    // interrupt line (or one buried in historical output) is not enough.
    if !shortcuts_footer(last) {
        let tail = &lines[lines.len().saturating_sub(4)..];
        if tail
            .iter()
            .rev()
            .take_while(|line| ui_banner(line))
            .any(|line| rate_limit_footer(line))
            && preceding_activity(&lines).is_some_and(running_indicator)
        {
            return (Running, "Codex displays an active interruptible turn.");
        }
        return (
            Unknown,
            "Codex screen lacks a recognized current composer/footer or active-turn layout.",
        );
    }
    // The ready composer is at the bottom, immediately above 1–3 footer rows.
    // No search through scrollback or arbitrary prose for 'done'/'waiting'.
    let prompt = lines
        .iter()
        .rposition(|line| *line == "›" || line.starts_with("› "));
    let Some(prompt) = prompt.filter(|index| (1..=3).contains(&(lines.len() - index - 1))) else {
        return (Unknown, "Codex composer is not visible.");
    };
    let activity = preceding_activity(&lines[..prompt]);
    if activity.is_some_and(running_indicator) {
        return (Running, "Codex displays an active interruptible turn.");
    }
    if lines[..prompt]
        .iter()
        .rev()
        .take(MAX_ACTIVITY_ROWS)
        .any(|line| line.contains("to interrupt"))
    {
        return (Unknown, "Codex activity indicator is unrecognized.");
    }
    if activity.is_some_and(completion_indicator) {
        return (
            Complete,
            "Codex turn finished; overall task completion is unverified.",
        );
    }
    (
        Idle,
        "Codex composer is ready; no explicit input request or completion marker detected.",
    )
}

fn running_indicator(line: &str) -> bool {
    let Some((label, rest)) = line.split_once(" (") else {
        return false;
    };
    matches!(
        label.strip_prefix("• ").unwrap_or(label),
        "Working" | "Thinking" | "Running" | "Searching" | "Compacting context"
    ) && rest
        .strip_suffix(" • esc to interrupt)")
        .is_some_and(duration)
}

fn preceding_activity<'a>(lines: &[&'a str]) -> Option<&'a str> {
    let mut tail = &lines[lines.len().saturating_sub(MAX_ACTIVITY_ROWS)..];
    while tail.last().is_some_and(|line| ui_banner(line)) {
        tail = &tail[..tail.len() - 1];
    }
    // A queued question is not a blocking approval. Skip only the complete
    // adjacent three-row UI block, never arbitrary question/transcript text.
    if tail.len() >= 3 && queued_questions(&tail[tail.len() - 3..]) {
        tail = &tail[..tail.len() - 3];
        while tail.last().is_some_and(|line| ui_banner(line)) {
            tail = &tail[..tail.len() - 1];
        }
    }
    // This detail belongs only to an active compaction row; a matching sentence
    // in ordinary output must not uncover an older Working/completion marker.
    if tail.last() == Some(&"└ Making room to continue") {
        tail = &tail[..tail.len() - 1];
        let activity = *tail.last()?;
        let label = activity.strip_prefix("• ").unwrap_or(activity);
        return (label.starts_with("Compacting context (") && running_indicator(activity))
            .then_some(activity);
    }
    tail.last().copied()
}

fn queued_questions(lines: &[&str]) -> bool {
    if !matches!(lines, [_, _, "shift+↵ to answer" | "shift+← to answer"])
        || lines[0].strip_prefix("• ").unwrap_or(lines[0]) != "Queued follow-up inputs"
    {
        return false;
    }
    let Some((count, noun)) = lines[1]
        .strip_prefix("? ")
        .and_then(|line| line.split_once(' '))
    else {
        return false;
    };
    !count.is_empty()
        && count.bytes().all(|byte| byte.is_ascii_digit())
        && count.parse::<usize>().is_ok_and(|count| {
            count > 0 && noun == if count == 1 { "question" } else { "questions" }
        })
}

fn ui_banner(line: &str) -> bool {
    rate_limit_footer(line)
        || line.starts_with("└ Tip: ")
        || (!line.is_empty() && line.chars().all(|ch| ch == '─'))
}

fn warning_text(line: &str) -> Option<&str> {
    line.strip_prefix('⚠')
        .map(|text| text.trim_start_matches('\u{fe0f}').trim())
}

fn rate_limit_footer(line: &str) -> bool {
    let line = warning_text(line).unwrap_or(line);
    let Some((window, rest)) = line.split_once(" limit: ") else {
        return false;
    };
    matches!(window, "5h" | "Weekly" | "weekly")
        && rest.strip_suffix("% left · /status").is_some_and(|value| {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && value.parse::<u8>().is_ok_and(|percent| percent <= 100)
        })
}

fn shortcuts_footer(line: &str) -> bool {
    let Some((_, suffix)) = line.rsplit_once("? for shortcuts") else {
        return false;
    };
    let suffix = suffix.trim();
    if suffix.is_empty() {
        return true;
    }
    let Some(warning) = warning_text(suffix) else {
        return false;
    };
    let parts: Vec<_> = warning.split_whitespace().collect();
    matches!(parts.as_slice(), [count, "warning" | "warnings", "·", "f2", "to", "view"]
        if !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit())
            && count.parse::<usize>().is_ok_and(|count| count > 0))
}

fn approval_title(line: &str) -> bool {
    matches!(
        line,
        "Would you like to run the following command?"
            | "Would you like to make the following edits?"
            | "Would you like to grant these permissions?"
            | "Would you like to send input to the existing terminal?"
    )
}

fn approval_footer(line: &str) -> bool {
    matches!(
        line,
        "Press enter to confirm or esc to cancel"
            | "Press Enter to confirm or Esc to cancel"
            | "Press enter to confirm or select"
            | "Press Enter to confirm or select"
    )
}

fn approval_option(line: &str) -> bool {
    line.strip_prefix("› ")
        .unwrap_or(line)
        .split_once(". ")
        .is_some_and(|(number, text)| {
            number.parse::<u8>().is_ok() && (text.starts_with("Yes, ") || text.starts_with("No, "))
        })
}

fn approval_prompt(observation: &PaneObservation, status: AgentStatus) -> Option<String> {
    if status != AgentStatus::WaitingForInput {
        return None;
    }
    let lines: Vec<_> = observation.screen.lines().map(str::trim_end).collect();
    let start = lines.iter().rposition(|line| approval_title(line.trim()))?;
    let context: Vec<_> = lines[start..]
        .iter()
        .take_while(|line| !approval_option(line.trim()) && !approval_footer(line.trim()))
        .copied()
        .collect();
    // Strip only the common UI margin, preserving command indentation, blank
    // lines and wrapped reason continuations. The whole visible dialog matters:
    // metadata/reasons can otherwise consume the budget before the command.
    let margin = context
        .iter()
        .filter(|line| !line.is_empty())
        .map(|line| line.bytes().take_while(|byte| *byte == b' ').count())
        .min()
        .unwrap_or(0);
    let text = context
        .iter()
        .map(|line| if line.is_empty() { "" } else { &line[margin..] })
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim_end();
    let mut preview: String = text.chars().take(MAX_APPROVAL_CHARS).collect();
    if text.chars().count() > MAX_APPROVAL_CHARS {
        preview.push_str(APPROVAL_CLIPPED);
    }
    Some(preview)
}

fn completion_indicator(line: &str) -> bool {
    let line = line.trim_matches('─').trim();
    line.strip_prefix("Worked for ").is_some_and(|rest| {
        duration(
            rest.split(" • ")
                .next()
                .unwrap_or(rest)
                .trim_end_matches('─')
                .trim(),
        )
    })
}

fn duration(text: &str) -> bool {
    !text.trim().is_empty()
        && text.split_whitespace().all(|part| {
            let Some(number) = part.strip_suffix(['h', 'm', 's']) else {
                return false;
            };
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOOTER: &str = "\n› Ask Codex to do anything\n\n  GPT-6.1-Sol high · ~/work · Task\n  ← for agents · ? for shortcuts\n";
    const APPROVAL: &str = "Would you like to run the following command?\n\n$ cargo test\n\n› 1. Yes, proceed (y)\n  2. No, and tell Codex what to do differently (esc)\n\nPress enter to confirm or esc to cancel\n";
    const LIMIT: &str = "⚠ 5h limit: 16% left · /status";
    // Only the UI chrome from the reported screenshots, not conversation text.
    const COMPACTING: &str = "• Compacting context (1m 44s • esc to interrupt)\n└ Making room to continue\n└ Tip: Use /vim to toggle Vim editing in the composer.";
    const QUEUED: &str = "• Queued follow-up inputs\n? 1 question\nshift+↵ to answer";
    const WARNINGS: &str = "⚠ 2 warnings · f2 to view";

    fn observation(screen: &str) -> PaneObservation {
        PaneObservation {
            command: "codex".into(),
            dead: false,
            in_mode: false,
            screen: screen.into(),
        }
    }

    #[test]
    fn common_codex_approvals_require_live_dialog_chrome() {
        for title in [
            "Would you like to run the following command?",
            "Would you like to make the following edits?",
            "Would you like to grant these permissions?",
            "Would you like to send input to the existing terminal?",
        ] {
            let screen = APPROVAL.replace("Would you like to run the following command?", title);
            assert_eq!(infer(&observation(&screen)).0, AgentStatus::WaitingForInput);
            assert_eq!(
                infer(&observation(&screen.replace(
                    "Yes, proceed (y)",
                    "No, and tell Codex what to do differently (esc)"
                )))
                .0,
                AgentStatus::WaitingForInput
            );
            assert_eq!(
                infer(&observation(
                    &screen.replace("or esc to cancel", "or select")
                ))
                .0,
                AgentStatus::WaitingForInput
            );
        }
        for screen in [
            "Would you like to run the following command?",
            "› 1. Yes, proceed (y)\nPress enter to confirm or esc to cancel",
            "Would you like to run the following command?\nPress enter to confirm or esc to cancel",
        ] {
            assert_eq!(infer(&observation(screen)).0, AgentStatus::Unknown);
        }
    }

    #[test]
    fn current_bottom_ui_wins_over_historical_approval_or_completion() {
        let screen = format!(
            "{APPROVAL}\nWorked for 18s • 7:08 PM\n• Working (5m 19s • esc to interrupt)\n└ Tip: Use /export.\n{FOOTER}"
        );
        assert_eq!(infer(&observation(&screen)).0, AgentStatus::Running);
        let screen = format!("{APPROVAL}\nWorked for 18s • 7:08 PM\n{FOOTER}");
        assert_eq!(infer(&observation(&screen)).0, AgentStatus::Complete);
        let screen = format!("{APPROVAL}\n• New output λ🙂\n{FOOTER}");
        assert_eq!(infer(&observation(&screen)).0, AgentStatus::Idle);
    }

    #[test]
    fn active_turns_support_bulletless_and_rate_limit_layouts() {
        // Reconstructed from the reported screenshot; no transcript contents.
        let screenshot = format!(
            "Browsing the web\nRan a command\nSearched the web\n\nWorking (55s • esc to interrupt)\n\n{LIMIT}\n"
        );
        for screen in [
            screenshot,
            format!("Working (55s • esc to interrupt)\n{FOOTER}"),
            format!("• Working (55s • esc to interrupt)\n{LIMIT}"),
            format!("Thinking (1m 5s • esc to interrupt)\n└ Tip: Use /export.\n{LIMIT}\n──────"),
            "Searching (2s • esc to interrupt)\n⚠ Weekly limit: 9% left · /status".into(),
        ] {
            assert_eq!(
                infer(&observation(&screen)).0,
                AgentStatus::Running,
                "{screen}"
            );
        }
        for label in ["Working", "Thinking", "Running", "Searching"] {
            for prefix in ["", "• "] {
                assert!(running_indicator(&format!(
                    "{prefix}{label} (1m 5s • esc to interrupt)"
                )));
            }
        }
    }

    #[test]
    fn rate_limit_and_warning_banners_preserve_ready_composer_status() {
        let footer = FOOTER.replace("? for shortcuts", &format!("? for shortcuts    {WARNINGS}"));
        for (activity, expected) in [
            ("Working (55s • esc to interrupt)", AgentStatus::Running),
            ("Worked for 12s • 9:47 PM", AgentStatus::Complete),
            ("New assistant output", AgentStatus::Idle),
        ] {
            let screen = format!("{activity}\n{LIMIT}\n{footer}");
            assert_eq!(infer(&observation(&screen)).0, expected, "{screen}");
        }
        assert!(shortcuts_footer(
            "? for shortcuts  ⚠ 1 warning · f2 to view"
        ));
        assert!(shortcuts_footer(
            "← for agents · ? for shortcuts  ⚠️ 2 warnings · f2 to view"
        ));
        assert!(rate_limit_footer("5h limit: 100% left · /status"));
    }

    #[test]
    fn compaction_and_adjacent_queued_questions_are_active_not_waiting() {
        for screen in [
            format!("{COMPACTING}\n{FOOTER}"),
            format!("{COMPACTING}\n{LIMIT}"),
            format!("Compacting context (2s • esc to interrupt)\n{FOOTER}"),
            format!("• Working (21s • esc to interrupt)\n{QUEUED}\n{FOOTER}"),
            format!("{COMPACTING}\n{QUEUED}\n{LIMIT}\n{FOOTER}"),
            format!("Working (21s • esc to interrupt)\n{QUEUED}\n{LIMIT}"),
            format!("Worked for 3s\nNew assistant output\n{COMPACTING}\n{FOOTER}"),
        ] {
            let observed = observation(&screen);
            let (status, _) = infer(&observed);
            assert_eq!(status, AgentStatus::Running, "{screen}");
            assert!(!status.needs_attention());
            assert!(approval_prompt(&observed, status).is_none());
            for in_mode in [false, true] {
                let mut inactive = observation(&screen);
                inactive.in_mode = in_mode;
                inactive.dead = !in_mode;
                assert_eq!(infer(&inactive).0, AgentStatus::Unknown);
            }
        }
        for count in [1, 2, 12] {
            for answer in ["shift+↵ to answer", "shift+← to answer"] {
                let noun = if count == 1 { "question" } else { "questions" };
                let block = format!("Queued follow-up inputs\n? {count} {noun}\n{answer}");
                let screen = format!("Thinking (5s • esc to interrupt)\n{block}\n{FOOTER}");
                assert_eq!(infer(&observation(&screen)).0, AgentStatus::Running);
            }
        }
        // A real approval dialog still wins over any older active/queued rows.
        let screen = format!("{COMPACTING}\n{QUEUED}\n{APPROVAL}");
        assert_eq!(infer(&observation(&screen)).0, AgentStatus::WaitingForInput);
    }

    #[test]
    fn activity_details_cannot_uncover_historical_or_malformed_timers() {
        let working = "• Working (21s • esc to interrupt)";
        for middle in [
            "• Queued follow-up inputs\n? 0 questions\nshift+↵ to answer",
            "• Queued follow-up inputs\n? 1 questions\nshift+↵ to answer",
            "• Queued follow-up inputs\n? 2 question\nshift+↵ to answer",
            "• Queued follow-up inputs\n? many questions\nshift+↵ to answer",
            "• Queued follow-up inputs\n? 999999999999999999999999 questions\nshift+↵ to answer",
            "• Queued follow-up inputs\n? 1 question",
            "? 1 question\nshift+↵ to answer",
            "• Queued follow-up inputs\n? 1 question\nenter to answer",
            "> • Queued follow-up inputs\n? 1 question\nshift+↵ to answer",
            "• Queued follow-up inputs\n? 1 question\nextra output\nshift+↵ to answer",
            "└ Making room to continue",
            "• New output",
        ] {
            let screen = format!("{working}\n{middle}\n{FOOTER}");
            assert_eq!(
                infer(&observation(&screen)).0,
                AgentStatus::Unknown,
                "{screen}"
            );
        }
        for screen in [
            format!("› {COMPACTING}\n{FOOTER}"),
            format!("{COMPACTING}\nNew output\n{FOOTER}"),
            format!(
                "Compacting context (soon • esc to interrupt)\n└ Making room to continue\n{FOOTER}"
            ),
            format!("{COMPACTING}\n{QUEUED}\nChoose a menu item"),
            format!("{COMPACTING}\n{QUEUED}"),
            format!("{working}\nNew output\n{QUEUED}\n{FOOTER}"),
        ] {
            assert_eq!(
                infer(&observation(&screen)).0,
                AgentStatus::Unknown,
                "{screen}"
            );
        }
        // Queued UI by itself is not sufficient running/waiting evidence.
        assert_eq!(
            infer(&observation(&format!("{QUEUED}\n{FOOTER}"))).0,
            AgentStatus::Idle
        );
        let distant = format!(
            "{working}\n{}{FOOTER}",
            "└ Tip: UI hint\n".repeat(MAX_ACTIVITY_ROWS)
        );
        assert_ne!(infer(&observation(&distant)).0, AgentStatus::Running);
        let screen = format!("Worked for 3s\n└ Making room to continue\n{FOOTER}");
        assert_ne!(infer(&observation(&screen)).0, AgentStatus::Complete);
    }

    #[test]
    fn new_layouts_do_not_match_historical_quoted_truncated_or_malformed_activity() {
        for screen in [
            format!("Working (55s • esc to interrupt)\nNew output\n{LIMIT}"),
            format!("Working (55s • esc to interrupt)\n{LIMIT}\nChoose a menu item"),
            format!("Working (55s • esc to interrupt)\n{LIMIT}\nWorked for 55s"),
            format!("› Working (55s • esc to interrupt)\n{LIMIT}"),
            format!("> Working (55s • esc to interrupt)\n{LIMIT}"),
            format!("Working (soon • esc to interrupt)\n{LIMIT}"),
            "Working (55s • esc to interrupt)\n⚠ 5h limit: 999% left · /status".into(),
            "Working (55s • esc to interrupt)".into(),
            format!("{LIMIT}\n{WARNINGS}"),
            format!("Working (55s • esc to interrupt)\n{LIMIT}\nNo recognized footer"),
            format!(
                "Working (55s • esc to interrupt)\n{}",
                FOOTER.replace("? for shortcuts", "? for shortcuts · unknown menu")
            ),
        ] {
            assert_eq!(
                infer(&observation(&screen)).0,
                AgentStatus::Unknown,
                "{screen}"
            );
        }
        for text in [
            "? for shortcuts · menu",
            "? for shortcuts ⚠ 0 warnings · f2 to view",
            "? for shortcuts ⚠ many warnings · f2 to view",
        ] {
            assert!(!shortcuts_footer(text));
        }
        let active = format!("Working (55s • esc to interrupt)\n{LIMIT}");
        let mut pane = observation(&active);
        pane.in_mode = true;
        assert_eq!(infer(&pane).0, AgentStatus::Unknown);
        pane.in_mode = false;
        pane.dead = true;
        assert_eq!(infer(&pane).0, AgentStatus::Unknown);
        pane.dead = false;
        pane.command = "fish".into();
        assert_eq!(infer(&pane).0, AgentStatus::Unknown);
        assert_eq!(
            infer(&observation(&format!("{active}\n{APPROVAL}"))).0,
            AgentStatus::WaitingForInput
        );
    }

    #[test]
    fn empty_ready_composer_and_legacy_context_footer_are_idle() {
        assert_eq!(infer(&observation(FOOTER)).0, AgentStatus::Idle);
        assert_eq!(
            infer(&observation("›\n  93% context left · ? for shortcuts\n")).0,
            AgentStatus::Idle
        );
        assert_eq!(
            infer(&observation(&format!(
                "─ Worked for 1m 5s ──────\n{FOOTER}"
            )))
            .0,
            AgentStatus::Complete
        );
        assert!(!completion_indicator("Worked for a while"));
        assert!(!running_indicator("• Working (soon • esc to interrupt)"));
        assert!(!running_indicator("• Working (  • esc to interrupt)"));
    }

    #[test]
    fn unsupported_dead_copy_mode_and_ambiguous_screens_are_unknown() {
        for screen in [
            "",
            "done",
            "Would you like to continue?",
            "• Working (6s • esc to interrupt)",
            "›\nMenu\n",
            "\x1b[32m›\n? for shortcuts",
        ] {
            assert_eq!(infer(&observation(screen)).0, AgentStatus::Unknown);
        }
        for command in ["fish", "node", "claude", "my-codex", ""] {
            let mut pane = observation(APPROVAL);
            pane.command = command.into();
            assert_eq!(infer(&pane).0, AgentStatus::Unknown);
        }
        let mut pane = observation(APPROVAL);
        pane.in_mode = true;
        assert_eq!(infer(&pane).0, AgentStatus::Unknown);
        pane.in_mode = false;
        pane.dead = true;
        assert_eq!(infer(&pane).0, AgentStatus::Unknown);
        assert!(supported(Some("/opt/bin/codex")));
    }

    #[test]
    fn only_waiting_and_complete_need_attention_and_all_labels_are_stable() {
        for (status, label, attention) in [
            (AgentStatus::Running, "RUNNING", false),
            (AgentStatus::WaitingForInput, "WAITING_FOR_INPUT", true),
            (AgentStatus::Idle, "IDLE", false),
            (AgentStatus::Complete, "COMPLETE", true),
            (AgentStatus::Unknown, "UNKNOWN", false),
        ] {
            assert_eq!(status.to_string(), label);
            assert_eq!(status.needs_attention(), attention);
        }
    }

    #[test]
    fn approval_context_is_bounded_unicode_safe_and_contains_only_the_current_dialog() {
        let screen = format!("Private earlier conversation\n{APPROVAL}");
        let pane = observation(&screen);
        let preview = approval_prompt(&pane, infer(&pane).0).unwrap();
        assert_eq!(
            preview,
            "Would you like to run the following command?\n\n$ cargo test"
        );
        assert!(!preview.contains("Private earlier"));
        assert!(!preview.contains("Yes, proceed"));
        let long = APPROVAL.replace(
            "$ cargo test",
            &format!("$ {}", "λ🙂".repeat(MAX_APPROVAL_CHARS)),
        );
        let long_pane = observation(&long);
        let preview = approval_prompt(&long_pane, infer(&long_pane).0).unwrap();
        assert_eq!(
            preview.chars().count(),
            MAX_APPROVAL_CHARS + APPROVAL_CLIPPED.chars().count()
        );
        assert!(preview.ends_with(APPROVAL_CLIPPED));
        assert!(preview.contains("λ🙂"));
        let multi_line = APPROVAL.replace("$ cargo test", "First\nSecond\nThird\nFourth\nFifth");
        let multi_pane = observation(&multi_line);
        let preview = approval_prompt(&multi_pane, infer(&multi_pane).0).unwrap();
        assert!(preview.contains("First\nSecond\nThird\nFourth\nFifth"));
        assert!(!preview.contains('…'));
        for status in [
            AgentStatus::Running,
            AgentStatus::Complete,
            AgentStatus::Idle,
            AgentStatus::Unknown,
        ] {
            assert!(approval_prompt(&pane, status).is_none());
        }
    }

    #[test]
    fn approval_retains_wrapped_reason_and_command_after_metadata() {
        let screen = "Earlier unrelated terminal output\n\n  Would you like to run the following command?\n\n  Environment: local\n\n  Reason: May I create the unsigned commit for the staged Packer\n  removal? Git must write to shared metadata outside this workspace.\n\n  $ git -c commit.gpgsign=false commit -m \"Remove obsolete Packer\n  installation\"\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for this command (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";
        let pane = observation(screen);
        assert_eq!(infer(&pane).0, AgentStatus::WaitingForInput);
        let prompt = approval_prompt(&pane, infer(&pane).0).unwrap();
        assert!(prompt.contains("Reason: May I create the unsigned commit for the staged Packer\nremoval? Git must write to shared metadata outside this workspace."));
        assert!(prompt.contains(
            "$ git -c commit.gpgsign=false commit -m \"Remove obsolete Packer\ninstallation\""
        ));
        assert!(!prompt.contains("Earlier unrelated"));
        assert!(!prompt.contains("Yes, proceed"));
        assert!(!prompt.contains('…'));
    }

    #[test]
    fn approval_retains_numbered_details_blank_lines_and_relative_command_indentation() {
        let screen = "  Would you like to run the following command?\n\n  Reason: Check these cases.\n  1. Read-only inspection\n  2. Preserve all existing files\n\n  $ printf '%s\\n' \\\n      'λ🙂 value'\n\n› 1. Yes, proceed (y)\n  2. No, and tell Codex what to do differently (esc)\nPress enter to confirm or esc to cancel\n";
        let pane = observation(screen);
        let prompt = approval_prompt(&pane, infer(&pane).0).unwrap();
        assert!(prompt.contains("1. Read-only inspection\n2. Preserve all existing files"));
        assert!(prompt.contains("$ printf '%s\\n' \\\n    'λ🙂 value'"));
        assert!(!prompt.contains("No, and tell"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refresh_replaces_evidence_deduplicates_panes_and_isolates_failures() {
        use crate::{Pane, Session, Window, WorkItem, WorkItemKind};
        use std::{fs, os::unix::fs::PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-tmux");
        let log = directory.path().join("log");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> '{}'
if [ \"$2\" != display-message ]; then exit 2; fi
cat '{}/'$5
",
                log.display(),
                directory.path().display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("state.json");
        let mut engine = Engine::new(&path);
        engine.tmux.executable = executable.into_os_string();
        let items: Vec<_> = [
            ("A", "%14"),
            ("B", "%14"),
            ("C", "%15"),
            ("D", "%99"),
            ("E", "%16"),
        ]
        .into_iter()
        .map(|(id, pane_id)| WorkItem {
            id: id.into(),
            title: "Task".into(),
            repository: "/work".into(),
            workspace: "/work".into(),
            branch: None,
            kind: WorkItemKind::Implementation,
            pane_id: pane_id.into(),
        })
        .collect();
        // Use an existing registry fixture; persistence locking is tested separately.
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"version": 1, "work_items": items})).unwrap(),
        )
        .unwrap();
        let stored = fs::read(&path).unwrap();
        let snapshot = Snapshot {
            sessions: vec![Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![Window {
                    id: "@1".into(),
                    index: 0,
                    name: "work".into(),
                    panes: [("%14", "codex"), ("%15", "fish"), ("%16", "codex")]
                        .into_iter()
                        .map(|(id, command)| Pane {
                            id: id.into(),
                            index: 0,
                            title: "Agent".into(),
                            current_command: Some(command.into()),
                            working_directory: None,
                        })
                        .collect(),
                }],
            }],
        };
        let fixture = |command: &str, screen: &str| {
            format!("\x1e{command}\x1f0\x1f0\x1e\n{screen}\x1e{command}\x1f0\x1f0\x1e\n")
        };
        for (screen, expected) in [
            (APPROVAL.into(), AgentStatus::WaitingForInput),
            (
                format!("• Working (2s • esc to interrupt)\n{FOOTER}"),
                AgentStatus::Running,
            ),
            (format!("{COMPACTING}\n{FOOTER}"), AgentStatus::Running),
            (
                format!("Working (21s • esc to interrupt)\n{QUEUED}\n{FOOTER}"),
                AgentStatus::Running,
            ),
            (format!("Worked for 2s\n{FOOTER}"), AgentStatus::Complete),
            (FOOTER.into(), AgentStatus::Idle),
            ("unrecognized menu".into(), AgentStatus::Unknown),
        ] {
            fs::write(directory.path().join("%14"), fixture("codex", &screen)).unwrap();
            let states = engine
                .observe_work_item_states(Some(&snapshot))
                .await
                .unwrap();
            assert_eq!(states[0].status, expected);
            assert_eq!(states[1].status, expected);
            assert_eq!(
                states[0].attention_prompt.is_some(),
                expected == AgentStatus::WaitingForInput
            );
            assert_eq!(states[0].attention_prompt, states[1].attention_prompt);
            assert_eq!(states[2].status, AgentStatus::Unknown);
            assert_eq!(states[3].pane, PaneAvailability::Missing);
            assert_eq!(states[4].status, AgentStatus::Unknown);
            assert!(states[4].status_detail.contains("capture failed"));
        }
        let calls = fs::read_to_string(&log).unwrap();
        // One capture for the shared pane and one failed capture per refresh.
        assert_eq!(calls.lines().count(), 14);
        for call in calls.lines() {
            assert!(call.contains("capture-pane -p -J -t"));
            assert!(!call.contains("send-keys"));
            assert!(!call.contains("%15"));
        }
        // A shell replacing Codex invalidates even a screen that looks like approval.
        fs::write(directory.path().join("%14"), fixture("fish", APPROVAL)).unwrap();
        assert_eq!(
            engine
                .observe_work_item_states(Some(&snapshot))
                .await
                .unwrap()[0]
                .status,
            AgentStatus::Unknown
        );
        let calls_before = fs::read_to_string(&log).unwrap();
        for snapshot in [None, Some(&Snapshot::default())] {
            let states = engine.observe_work_item_states(snapshot).await.unwrap();
            assert!(
                states
                    .iter()
                    .all(|state| state.status == AgentStatus::Unknown)
            );
            assert!(states.iter().all(|state| state.attention_prompt.is_none()));
        }
        assert_eq!(fs::read_to_string(&log).unwrap(), calls_before);
        assert_eq!(fs::read(&path).unwrap(), stored);
    }

    #[tokio::test]
    async fn observation_with_no_items_does_not_need_a_transport() {
        let directory = tempfile::tempdir().unwrap();
        let mut engine = Engine::new(directory.path().join("not-created.json"));
        engine.tmux.executable = "nonexistent-tmux".into();
        assert!(
            engine
                .observe_work_item_states(Some(&Snapshot::default()))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(!directory.path().join("not-created.json").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires tmux; replays Codex UI fixtures on its own isolated server"]
    async fn isolated_tmux_observations_handle_copy_mode_and_disappearance() {
        use crate::{WorkItem, WorkItemKind};
        use std::{fs, process::Command};
        struct Server(std::path::PathBuf);
        impl Drop for Server {
            fn drop(&mut self) {
                let _ = Command::new("tmux")
                    .arg("-S")
                    .arg(&self.0)
                    .arg("kill-server")
                    .output();
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("tmux.sock");
        // An inert process named codex replays fixtures; no agent/API is invoked.
        // Compile a helper rather than copying a platform-signed system binary.
        let executable = directory.path().join("codex");
        let source = directory.path().join("fixture.rs");
        fs::write(
            &source,
            r#"fn main() {
            let screen = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
            print!("\x1b[2J\x1b[H{}", screen);
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
            loop { std::thread::park(); }
        }"#,
        )
        .unwrap();
        let compiled = Command::new("rustc")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let fixture = directory.path().join("screen");
        fs::write(&fixture, APPROVAL).unwrap();
        let started = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "m4-test",
                "-x",
                "120",
                "-y",
                "40",
            ])
            .arg(&executable)
            .arg(&fixture)
            .output()
            .unwrap();
        assert!(
            started.status.success(),
            "{}",
            String::from_utf8_lossy(&started.stderr)
        );
        let _server = Server(socket.clone());
        let mut engine = Engine::new(directory.path().join("state.json"));
        engine.tmux.socket = Some(socket);
        let snapshot = engine.discover().await.unwrap();
        let id = snapshot.sessions[0].windows[0].panes[0].id.clone();
        engine
            .register_work_item(WorkItem {
                id: "M4".into(),
                title: "Observe".into(),
                repository: "/work".into(),
                workspace: "/work".into(),
                branch: None,
                kind: WorkItemKind::Implementation,
                pane_id: id.clone(),
            })
            .unwrap();
        for (screen, expected) in [
            (APPROVAL.into(), AgentStatus::WaitingForInput),
            (
                format!("• Working (3s • esc to interrupt)\n{FOOTER}"),
                AgentStatus::Running,
            ),
            (
                format!("Working (55s • esc to interrupt)\n{LIMIT}"),
                AgentStatus::Running,
            ),
            (format!("{COMPACTING}\n{FOOTER}"), AgentStatus::Running),
            (
                format!("Working (21s • esc to interrupt)\n{QUEUED}\n{FOOTER}"),
                AgentStatus::Running,
            ),
            (
                format!(
                    "Worked for 12s • 9:47 PM\n{LIMIT}\n{}",
                    FOOTER.replace("? for shortcuts", &format!("? for shortcuts    {WARNINGS}"))
                ),
                AgentStatus::Complete,
            ),
            (format!("Worked for 3s\n{FOOTER}"), AgentStatus::Complete),
            (FOOTER.into(), AgentStatus::Idle),
        ] {
            fs::write(&fixture, screen).unwrap();
            engine
                .tmux
                .execute(&[
                    "respawn-pane".into(),
                    "-k".into(),
                    "-t".into(),
                    id.clone(),
                    executable.to_string_lossy().into_owned(),
                    fixture.to_string_lossy().into_owned(),
                ])
                .await
                .unwrap();
            let mut observed = AgentStatus::Unknown;
            for _ in 0..50 {
                let snapshot = engine.discover().await.unwrap();
                let states = engine
                    .observe_work_item_states(Some(&snapshot))
                    .await
                    .unwrap();
                observed = states[0].status;
                if observed == expected {
                    assert_eq!(
                        crate::attention_items(&states).len(),
                        usize::from(expected.needs_attention())
                    );
                    assert_eq!(
                        states[0].attention_prompt.is_some(),
                        expected == AgentStatus::WaitingForInput
                    );
                    if let Some(prompt) = &states[0].attention_prompt {
                        assert!(prompt.contains("$ cargo test"));
                    }
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert_eq!(observed, expected);
        }
        engine
            .tmux
            .execute(&["copy-mode".into(), "-t".into(), id.clone()])
            .await
            .unwrap();
        let snapshot = engine.discover().await.unwrap();
        assert_eq!(
            engine
                .observe_work_item_states(Some(&snapshot))
                .await
                .unwrap()[0]
                .status,
            AgentStatus::Unknown
        );
        engine
            .tmux
            .execute(&[
                "new-window".into(),
                "-d".into(),
                "-t".into(),
                "m4-test".into(),
                "cat".into(),
            ])
            .await
            .unwrap();
        engine
            .tmux
            .execute(&["kill-pane".into(), "-t".into(), id])
            .await
            .unwrap();
        let snapshot = engine.discover().await.unwrap();
        let states = engine
            .observe_work_item_states(Some(&snapshot))
            .await
            .unwrap();
        assert_eq!(states[0].pane, PaneAvailability::Missing);
        assert_eq!(states[0].status, AgentStatus::Unknown);
        assert!(crate::attention_items(&states).is_empty());
        assert!(states[0].attention_prompt.is_none());
        assert_eq!(engine.work_items().unwrap().len(), 1);
    }
}
